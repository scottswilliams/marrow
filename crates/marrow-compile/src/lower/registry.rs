//! The function and generic-template registries resolved before body lowering.

use super::*;

/// The project's functions and the module scope a call resolves against: every
/// function signature (resolved before body lowering so a forward call resolves),
/// the set of module names, and each module's `use` bindings. Names are unique
/// within a module (a duplicate is rejected before this is built).
#[derive(Default)]
pub(crate) struct FunctionRegistry {
    sigs: Vec<FnSignature>,
    modules: BTreeSet<String>,
    /// `module -> [(final-segment binding, dotted target module)]`.
    imports: BTreeMap<String, Vec<(String, String)>>,
    /// The dotted names of project modules that did not parse. A qualified call whose
    /// target module is one of these is a dependency gap — an editor fact that is
    /// unavailable because a required owner is invalid, never simply absent.
    broken_modules: BTreeSet<String>,
}

pub(crate) struct TemplateProofOutcome {
    pub(crate) diagnostics: Vec<SourceDiagnostic>,
    pub(crate) generic: GenericDiagnostics,
    /// Editor hover facts from the once-checked template body: `(span, hover display,
    /// optional definition target)` for each resolved local/parameter use and function
    /// callee, collected exactly once at the template. A template-parameter use renders by
    /// its declared spelling. Instances never re-collect these (their use-site spans
    /// duplicate the template's), so the divergent-monomorphization O(N²) hot path is not
    /// entered.
    pub(crate) hover_facts: Vec<(SourceSpan, String, Option<DefinitionTarget>)>,
    /// Editor dependency gaps from the template body: `(file, callee span)` for each
    /// qualified call to a module that did not parse.
    pub(crate) dependency_gaps: Vec<(FileIdentity, SourceSpan)>,
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
        functions: &[(FileIdentity, String, &FunctionDecl)],
        modules: BTreeSet<String>,
        imports: BTreeMap<String, Vec<(String, String)>>,
        broken_modules: BTreeSet<String>,
        diagnostics: &mut Vec<SourceDiagnostic>,
    ) -> Result<Option<Self>, LowerInvariant> {
        let mut sigs = Vec::with_capacity(functions.len());
        let mut accepted = true;
        // Only monomorphic functions take an image index and enter the signature
        // table; a generic function is a template with no single image entry (its
        // per-application instances are minted lazily), so it is skipped here and
        // resolved through the separate [`GenericRegistry`]. The concrete index runs
        // over non-generic functions only, matching the order [`FnLowerer::lower`]
        // adds them into the image FUNCTIONS table.
        let mut index: u16 = 0;
        for (file, module, function) in functions {
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
                    Err(ResolveError::Refusal(ResolveRefusal::Unsupported)) => {
                        diagnostics.push(unsupported(file, param.ty.span(), "this parameter type"));
                        accepted = false;
                    }
                    Err(ResolveError::Refusal(ResolveRefusal::Limit)) => accepted = false,
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
                        Err(ResolveError::Refusal(ResolveRefusal::Unsupported)) => {
                            diagnostics.push(unsupported(
                                file,
                                annotation.span(),
                                "this return type",
                            ));
                            accepted = false;
                            RetType::Unit
                        }
                        Err(ResolveError::Refusal(ResolveRefusal::Limit)) => {
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
                index,
                params,
                ret,
                public: function.public,
                file: file.clone(),
                name_span: function.name_span,
                decl_range: decl_range(function),
            });
            index += 1;
        }
        Ok(accepted.then_some(Self {
            sigs,
            modules,
            imports,
            broken_modules,
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
        prefix: &[String],
        item: &str,
    ) -> CallResolution<'_> {
        let module = if let [single] = prefix {
            if let Some((_, target)) = self
                .imports
                .get(current)
                .and_then(|bindings| bindings.iter().find(|(seg, _)| seg == single))
            {
                target.clone()
            } else if self.modules.contains(single) {
                single.clone()
            } else {
                return CallResolution::NotFound;
            }
        } else {
            let dotted = prefix.join(".");
            if self.modules.contains(&dotted) {
                dotted
            } else {
                return CallResolution::NotFound;
            }
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
    pub(super) fn resolved_module(&self, current: &str, prefix: &[String]) -> Option<String> {
        if let [single] = prefix {
            if let Some((_, target)) = self
                .imports
                .get(current)
                .and_then(|bindings| bindings.iter().find(|(seg, _)| seg == single))
            {
                Some(target.clone())
            } else if self.modules.contains(single) {
                Some(single.clone())
            } else {
                None
            }
        } else {
            let dotted = prefix.join(".");
            self.modules.contains(&dotted).then_some(dotted)
        }
    }

    /// Whether a qualified call's `prefix` names a project module that did not parse.
    /// A failed `use` leaves no binding, so a broken dependency presents as a direct
    /// reference to the broken module name; a surviving binding to a since-broken
    /// target is resolved through its dotted target.
    pub(super) fn names_broken_module(&self, current: &str, prefix: &[String]) -> bool {
        if let [single] = prefix {
            match self
                .imports
                .get(current)
                .and_then(|bindings| bindings.iter().find(|(seg, _)| seg == single))
            {
                Some((_, target)) => self.broken_modules.contains(target),
                None => self.broken_modules.contains(single),
            }
        } else {
            self.broken_modules.contains(&prefix.join("."))
        }
    }
}

/// One generic function template: the source declaration plus its type-parameter
/// names and constraints, held for lazy monomorphization. A template has no image
/// index; each concrete application is a distinct image function.
pub(crate) struct GenericTemplate<'p> {
    pub(super) file: FileIdentity,
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
    pub(crate) fn build(functions: &[(FileIdentity, String, &'p FunctionDecl)]) -> Self {
        let templates = functions
            .iter()
            .filter(|(_, _, function)| !function.type_params.is_empty())
            .map(|(file, module, function)| GenericTemplate {
                file: file.clone(),
                module: module.clone(),
                public: function.public,
                decl: function,
                type_params: function
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
