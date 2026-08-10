//! The function and generic-template registries resolved before body lowering.

use super::*;

use crate::decl::{DeclarationLedger, DeclarationNamespace};

/// One declared function paired with where it was declared: the file identity its
/// diagnostics point into, the snapshot coordinate its editor facts are retained
/// under, and its dotted module.
pub(crate) struct DeclaredFn<'p> {
    pub(crate) file: FileIdentity,
    pub(crate) at: FileRef,
    pub(crate) module: String,
    pub(crate) decl: &'p FunctionDecl,
}

/// What an importable module name binds to.
///
/// The binding carries no payload: module scope resolves a name *through* the dotted
/// path, and every signature carries its own module string, so nothing is looked up
/// on the module itself. It exists to make the accepted set a typed ledger entry
/// rather than a bare name in a set, which is what lets a refused module answer with
/// its cause instead of reading as a module the project does not contain.
pub(crate) struct ModuleBinding;

/// The project's modules keyed by dotted path: importable when accepted, and refused
/// with the cause when the header disagrees with the path or the stage that produced
/// the source refused it whole. A file with no `module` header is a script — it is
/// not a module of this namespace at all, and naming it is a genuine absence.
pub(crate) type ModuleLedger = DeclarationLedger<String, ModuleBinding>;

/// The project's functions and the module scope a call resolves against: every
/// function signature (resolved before body lowering so a forward call resolves),
/// the module ledger, and each module's `use` bindings. Names are unique within a
/// module (a duplicate is rejected before this is built).
pub(crate) struct FunctionRegistry {
    sigs: Vec<FnSignature>,
    modules: ModuleLedger,
    /// `module -> [(final-segment binding, dotted target module)]`.
    imports: BTreeMap<String, Vec<(String, String)>>,
}

pub(crate) struct TemplateProofOutcome {
    /// The proof's finished local diagnostic terminal, sealed by the local
    /// collector's one `finish` for the outer stage owner to absorb.
    pub(crate) diagnostics: BoundedDiagnostics,
    pub(crate) generic: GenericDiagnostics,
}

/// Whether every declared signature resolved. A refused signature has already pushed
/// its diagnostic; the registry is withheld because a partial signature table cannot
/// resolve call sites, which makes the whole `CompleteFunctionRegistry` artifact
/// unavailable rather than leaving the caller to read that from an untyped `None`.
pub(crate) enum FunctionRegistryOutcome {
    Complete(FunctionRegistry),
    Refused,
}

impl Default for FunctionRegistry {
    /// The registry of a project with no modules, functions, or imports. The module
    /// ledger carries its namespace tag, so there is no untagged empty ledger.
    fn default() -> Self {
        Self {
            sigs: Vec::new(),
            modules: ModuleLedger::new(DeclarationNamespace::Module),
            imports: BTreeMap::new(),
        }
    }
}

impl FunctionRegistry {
    /// Resolve every function's signature in declaration order. The i-th function
    /// takes image index `i`, matching the order [`FnLowerer::lower`] adds them.
    /// `functions` pairs each declaration with its dotted module name.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build(
        records: &TypeRegistry,
        draft: &mut ImageDraft,
        durable: &DurableRegistry,
        functions: &[DeclaredFn<'_>],
        modules: ModuleLedger,
        imports: BTreeMap<String, Vec<(String, String)>>,
        diagnostics: &mut DiagnosticCollector,
    ) -> Result<FunctionRegistryOutcome, LowerInvariant> {
        let mut sigs = Vec::with_capacity(functions.len());
        let mut accepted = true;
        // Only monomorphic functions take an image index and enter the signature
        // table; a generic function is a template with no single image entry (its
        // per-application instances are minted lazily), so it is skipped here and
        // resolved through the separate [`GenericRegistry`]. The concrete index runs
        // over non-generic functions only, matching the order [`FnLowerer::lower`]
        // adds them into the image FUNCTIONS table.
        let mut index: u16 = 0;
        for declared in functions {
            let (file, module, function) = (&declared.file, &declared.module, declared.decl);
            if !function.type_params.is_empty() {
                continue;
            }
            let mut params = Vec::with_capacity(function.params.len());
            for param in &function.params {
                let site = MintSite {
                    file,
                    span: param.ty.span(),
                };
                match param_type(records, draft, durable, &param.ty, TypeEnv::EMPTY, site) {
                    Ok(ty) => params.push(ty),
                    Err(ResolveError::Refusal(refusal)) => {
                        if let Some(row) = annotation_refusal_row(
                            records,
                            durable,
                            refusal,
                            file,
                            param.ty.span(),
                            "this parameter type",
                        )? {
                            diagnostics.push(row);
                        }
                        accepted = false;
                    }
                    Err(ResolveError::Invariant(invariant)) => return Err(invariant),
                }
            }
            let ret = match &function.return_type {
                None => RetType::Unit,
                Some(annotation) => {
                    let site = MintSite {
                        file,
                        span: annotation.span(),
                    };
                    match resolve_type(records, draft, durable, annotation, TypeEnv::EMPTY, site) {
                        Err(ResolveError::Refusal(refusal)) => {
                            if let Some(row) = annotation_refusal_row(
                                records,
                                durable,
                                refusal,
                                file,
                                annotation.span(),
                                "this return type",
                            )? {
                                diagnostics.push(row);
                            }
                            accepted = false;
                            RetType::Unit
                        }
                        Err(ResolveError::Invariant(invariant)) => return Err(invariant),
                        Ok(ty) => RetType::Value(ty),
                    }
                }
            };
            sigs.push(FnSignature {
                name: function.name.clone(),
                module: module.clone(),
                at: declared.at,
                index,
                params,
                ret,
                public: function.public,
                name_span: function.name_span,
                decl_range: decl_range(function),
            });
            index += 1;
        }
        if !accepted {
            return Ok(FunctionRegistryOutcome::Refused);
        }
        Ok(FunctionRegistryOutcome::Complete(Self {
            sigs,
            modules,
            imports,
        }))
    }

    /// The number of monomorphic functions, which is the number of image FUNCTIONS
    /// entries lowered before tests and generic instantiations.
    pub(crate) fn concrete_count(&self) -> u16 {
        self.sigs.len() as u16
    }

    /// The names of every function declared in `module`, so an unresolved call can
    /// offer the nearest one as a did-you-mean. Used for both an unqualified call (the
    /// caller's own module) and a qualified call (the resolved target module).
    pub(super) fn module_function_names<'s>(
        &'s self,
        module: &'s str,
    ) -> impl Iterator<Item = &'s str> {
        self.sigs
            .iter()
            .filter(move |sig| sig.module == module)
            .map(|sig| sig.name.as_str())
    }

    /// Resolve an unqualified call from within `module`: a function of that name in
    /// the same module.
    pub(super) fn same_module(&self, module: &str, name: &str) -> Option<&FnSignature> {
        self.sigs
            .iter()
            .find(|sig| sig.name == name && sig.module == module)
    }

    /// Resolve a `::`-qualified call `prefix::item` from within `current`. A single
    /// prefix segment binds through a `use` first, then a root-level module of the
    /// same name; a multi-segment prefix names a fully-qualified module path. The
    /// target must be `pub`, except a module qualifying its own function.
    pub(super) fn resolve_qualified(
        &self,
        current: &str,
        prefix: &[NameSegment],
        item: &str,
    ) -> CallResolution<'_> {
        let module = match self.prefix_module(current, prefix) {
            ModuleResolution::Accepted(module) => module,
            // The prefix names a module this project contains and refused. The
            // declaration reported the cause, so the call reuses it rather than
            // resolving into a scope that does not exist.
            ModuleResolution::Refused(summary) => return CallResolution::ModuleRefused(summary),
            ModuleResolution::Absent => return CallResolution::NotFound,
        };
        match self
            .sigs
            .iter()
            .find(|sig| sig.name == item && sig.module == module)
        {
            Some(sig) if sig.public || sig.module == current => CallResolution::Found(sig),
            Some(_) => CallResolution::NotPublic,
            None => CallResolution::NotFound,
        }
    }

    /// The dotted module a `::`-qualified prefix names from within `current`, shared
    /// with generic-call resolution so both read module scope one way.
    pub(super) fn resolved_module(&self, current: &str, prefix: &[NameSegment]) -> Option<String> {
        match self.prefix_module(current, prefix) {
            ModuleResolution::Accepted(module) => Some(module),
            ModuleResolution::Refused(_) | ModuleResolution::Absent => None,
        }
    }

    /// What a `::`-qualified prefix names from within `current`: an importable
    /// module, a module this project contains and refused, or nothing.
    ///
    /// A failed `use` leaves no binding, so a refused dependency presents as a direct
    /// reference to its own name; a surviving binding to a since-refused target is
    /// resolved through its dotted target. One owner for both call resolution and
    /// generic-call resolution, so the two cannot disagree about module scope.
    fn prefix_module(&self, current: &str, prefix: &[NameSegment]) -> ModuleResolution<'_> {
        let dotted = if let [single] = prefix {
            match self
                .imports
                .get(current)
                .and_then(|bindings| bindings.iter().find(|(seg, _)| seg == single.text()))
            {
                Some((_, target)) => target.clone(),
                None => single.text().to_string(),
            }
        } else {
            dotted_module_path(prefix)
        };
        match self.modules.lookup(&dotted) {
            Binding::Accepted(ModuleBinding) => ModuleResolution::Accepted(dotted),
            Binding::Refused(_, summary) => ModuleResolution::Refused(summary),
            Binding::Absent => ModuleResolution::Absent,
        }
    }
}

/// What a qualified prefix names: an importable dotted module, the cause a module of
/// this project was refused for, or nothing.
enum ModuleResolution<'a> {
    Accepted(String),
    Refused(&'a DeclarationRefusalSummary),
    Absent,
}

/// The dotted module name a multi-segment `::` prefix spells. A module path joins on
/// `.`, unlike a name path, so this is the registry's own spelling and not the syntax
/// crate's `::` join.
fn dotted_module_path(prefix: &[NameSegment]) -> String {
    prefix
        .iter()
        .map(NameSegment::text)
        .collect::<Vec<_>>()
        .join(".")
}

/// One generic function template: the source declaration plus its type-parameter
/// names and constraints, held for lazy monomorphization. A template has no image
/// index; each concrete application is a distinct image function.
pub(crate) struct GenericTemplate<'p> {
    /// The file spelling a diagnostic reported against this template names. Distinct
    /// from `at`: a diagnostic is rendered for a person and carries the identity, while
    /// a retained fact carries the compact coordinate.
    pub(super) file: FileIdentity,
    /// The snapshot coordinate this template's editor facts are retained under.
    pub(super) at: FileRef,
    pub(super) module: String,
    pub(super) public: bool,
    pub(super) decl: &'p FunctionDecl,
    pub(super) type_params: Vec<(String, Option<TypeConstraint>)>,
}

/// The project's generic function templates and the module scope a generic call
/// resolves against — the same visibility rules the [`FunctionRegistry`] applies to
/// monomorphic functions, but keyed to templates rather than image indices.
#[derive(Default)]
pub(crate) struct GenericRegistry<'p> {
    pub(super) templates: Vec<GenericTemplate<'p>>,
}

impl<'p> GenericRegistry<'p> {
    /// Collect every generic function (one carrying type parameters) as a template,
    /// paired with its source file and dotted module name.
    pub(crate) fn build(functions: &[DeclaredFn<'p>]) -> Self {
        let templates = functions
            .iter()
            .filter(|declared| !declared.decl.type_params.is_empty())
            .map(|declared| GenericTemplate {
                file: declared.file.clone(),
                at: declared.at,
                module: declared.module.clone(),
                public: declared.decl.public,
                decl: declared.decl,
                type_params: declared
                    .decl
                    .type_params
                    .iter()
                    .map(|param| {
                        (
                            param.name.clone(),
                            param.constraint.map(TypeConstraint::from_syntax),
                        )
                    })
                    .collect(),
            })
            .collect();
        Self { templates }
    }

    /// The templates, for the once-checked template pass and instance draining.
    pub(crate) fn templates(&self) -> &[GenericTemplate<'p>] {
        &self.templates
    }

    /// The template index of an unqualified generic call `name` from `module`.
    pub(super) fn same_module(&self, module: &str, name: &str) -> Option<usize> {
        self.templates
            .iter()
            .position(|template| template.decl.name == name && template.module == module)
    }

    /// The template named `item` in `module`, with its `pub` flag, for a qualified
    /// generic call. The caller checks visibility against the calling module.
    pub(super) fn in_module(&self, module: &str, item: &str) -> Option<(usize, bool)> {
        self.templates
            .iter()
            .position(|template| template.decl.name == item && template.module == module)
            .map(|index| (index, self.templates[index].public))
    }
}

impl<'p> GenericTemplate<'p> {
    pub(crate) fn source_file(&self) -> &FileIdentity {
        &self.file
    }

    /// The snapshot coordinate this template's editor facts are retained under.
    pub(crate) fn at(&self) -> FileRef {
        self.at
    }

    pub(crate) fn name(&self) -> &str {
        &self.decl.name
    }

    pub(crate) fn span(&self) -> SourceSpan {
        self.decl.span
    }
}

// Generic instantiation identity — for functions and value types together — is
// owned by the [`TypeRegistry`]'s single monomorphization table (see
// `reserve_fn_instance`/`next_fn_pending`), keyed by `(template, args)` and bounded
// by `MAX_INSTANTIATIONS`. The lowerer mints function instances through the shared
// `records` registry, exactly as it mints generic type instantiations.
