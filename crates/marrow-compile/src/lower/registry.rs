//! The function and generic-template registries resolved before body lowering.

use super::*;

use crate::decl::{
    Binding, DeclarationBudget, DeclarationIndexDrift, DeclarationLedger, DeclarationNamespace,
    DeclarationOccurrence, DeclarationRefusalSummary, DeclarationSite, ModuleScopedName,
    refuse_covered, refuse_first,
};
use crate::types::BuildError;

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

/// One function's key in the signature namespace: its dotted module and its name.
pub(crate) type FnKey = ModuleScopedName;

/// The project's functions and the module scope a call resolves against: every
/// function signature (resolved before body lowering so a forward call resolves),
/// the module ledger, and each module's `use` bindings. A duplicate name in one
/// module is reported by its own check before this is built.
pub(crate) struct FunctionRegistry {
    sigs: DeclarationLedger<FnKey, FnSignature>,
    /// Each monomorphic function declaration in the source order body lowering
    /// walks, so a body asks about the declaration it is lowering rather than about
    /// its name. See [`SignatureWalk`].
    declarations: Vec<DeclaredSignature>,
    modules: ModuleLedger,
    /// `module -> [(final-segment binding, dotted target module)]`.
    imports: BTreeMap<String, Vec<(String, String)>>,
}

/// How the signature build resolved one monomorphic function declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignatureOutcome {
    /// Every annotation resolved; the body lowers against the signature.
    Resolved,
    /// A parameter or return type was refused and reported at this declaration.
    /// The body is refused with it: there is no parameter list to bind, and
    /// resolving the same annotation again would report the cause a second time.
    Refused,
}

/// One monomorphic function declaration as the signature build resolved it, with
/// the `Copy` site its name is written at.
#[derive(Debug, Clone, Copy)]
struct DeclaredSignature {
    at: FileRef,
    name_span: SourceSpan,
    outcome: SignatureOutcome,
}

/// A cursor over the monomorphic signature declarations, in the source order both
/// the signature build and body lowering walk them.
///
/// The refusal is keyed to the *occurrence*, never to the name. A module may
/// declare one name twice — the repeat is reported by its own duplicate check and
/// both declarations still lower a body — so a name-keyed answer serves the first
/// occurrence's outcome to every later one: it lowers a refused body, which
/// re-resolves the annotation already reported, and withholds an accepted body
/// whose image index was minted.
pub(crate) struct SignatureWalk<'a> {
    remaining: std::slice::Iter<'a, DeclaredSignature>,
}

impl SignatureWalk<'_> {
    /// How the declaration whose name is written at `at`/`name_span` resolved,
    /// consuming one entry.
    ///
    /// The site is checked, not assumed: the two walks derive their declaration
    /// sequence separately from the same parse, and a divergence is
    /// [`DeclarationIndexDrift`] rather than a refusal answered for a neighbouring
    /// declaration.
    pub(crate) fn next_at(
        &mut self,
        at: FileRef,
        name_span: SourceSpan,
    ) -> Result<SignatureOutcome, DeclarationIndexDrift> {
        let declared = self.remaining.next().ok_or(DeclarationIndexDrift)?;
        if declared.at != at || declared.name_span != name_span {
            return Err(DeclarationIndexDrift);
        }
        Ok(declared.outcome)
    }
}

pub(crate) struct TemplateProofOutcome {
    /// The proof's finished local diagnostic terminal, sealed by the local
    /// collector's one `finish` for the outer stage owner to absorb.
    pub(crate) diagnostics: BoundedDiagnostics,
    pub(crate) generic: GenericDiagnostics,
}

impl FunctionRegistry {
    /// The registry of a project with no modules, functions, or imports. Each ledger
    /// carries its namespace tag and the pass's retention budget, so there is
    /// neither an untagged nor an unbudgeted empty ledger.
    ///
    /// Production builds the registry through [`Self::build`]; this exists for the
    /// lowering tests that need a scope with no function in it.
    #[cfg(test)]
    pub(crate) fn empty(budget: DeclarationBudget) -> Self {
        Self {
            sigs: DeclarationLedger::new(DeclarationNamespace::Function, budget.clone()),
            declarations: Vec::new(),
            modules: ModuleLedger::new(DeclarationNamespace::Module, budget),
            imports: BTreeMap::new(),
        }
    }

    /// Resolve every function's signature in declaration order.
    ///
    /// A signature is refused whole: one parameter or return type the compiler
    /// could not resolve refuses the declaration and takes no image index, so no
    /// signature with a short parameter list enters the table and no later
    /// declaration inherits an index that was never minted. The declaration keeps
    /// its name, so a call to it reuses that cause rather than reading as unknown.
    ///
    /// **Slot order.** `index` is an image coordinate: the i-th accepted signature
    /// must be the function the draft's i-th monomorphic slot holds. The counter
    /// below advances once per accepted occurrence, in the source order
    /// [`DeclarationLedger::accepted_occurrences`] walks, and `lower_declared_functions`
    /// visits the same declarations in the same order and refuses exactly the ones
    /// refused here — a body whose parameter or return annotation does not resolve
    /// refuses before it takes a slot. So the ledger's accepted occurrences and the
    /// image's monomorphic slots are the same sequence.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build(
        records: &TypeRegistry,
        draft: &mut DraftTxn<'_>,
        durable: &DurableRegistry,
        functions: &[DeclaredFn<'_>],
        modules: ModuleLedger,
        imports: BTreeMap<String, Vec<(String, String)>>,
        diagnostics: &mut DiagnosticCollector,
        budget: DeclarationBudget,
    ) -> Result<FunctionRegistry, BuildError> {
        let mut sigs = DeclarationLedger::new(DeclarationNamespace::Function, budget);
        let mut declarations = Vec::new();
        // Only monomorphic functions take an image index and enter the signature
        // table; a generic function is a template with no single image entry (its
        // per-application instances are minted lazily), so it is skipped here and
        // resolved through the separate [`GenericRegistry`].
        let mut index: u16 = 0;
        for declared in functions {
            let (file, module, function) = (&declared.file, &declared.module, declared.decl);
            if !function.type_params.is_empty() {
                continue;
            }
            let at = DeclarationSite {
                name: &function.name,
                file,
                at: declared.at,
                span: function.name_span,
            };
            let mut refusal: Option<DeclarationRefusalSummary> = None;
            let mut params = Vec::with_capacity(function.params.len());
            for param in &function.params {
                let site = MintSite {
                    file,
                    span: param.ty.span(),
                };
                match param_type(records, draft, durable, &param.ty, TypeEnv::EMPTY, site) {
                    Ok(ty) => params.push(ty),
                    Err(ResolveError::Refusal(refused)) => {
                        let refused = annotation_refusal_row(
                            records,
                            durable,
                            refused,
                            file,
                            param.ty.span(),
                            "this parameter type",
                        )?;
                        refuse_annotation(&mut refusal, diagnostics, at, refused);
                    }
                    Err(ResolveError::Invariant(invariant)) => return Err(invariant.into()),
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
                        Err(ResolveError::Refusal(refused)) => {
                            let refused = annotation_refusal_row(
                                records,
                                durable,
                                refused,
                                file,
                                annotation.span(),
                                "this return type",
                            )?;
                            refuse_annotation(&mut refusal, diagnostics, at, refused);
                            RetType::Unit
                        }
                        Err(ResolveError::Invariant(invariant)) => return Err(invariant.into()),
                        Ok(ty) => RetType::Value(ty),
                    }
                }
            };
            let occurrence = match refusal {
                Some(refusal) => {
                    declarations.push(DeclaredSignature {
                        at: declared.at,
                        name_span: function.name_span,
                        outcome: SignatureOutcome::Refused,
                    });
                    DeclarationOccurrence::Refused(refusal)
                }
                None => {
                    let signature = FnSignature {
                        module: module.clone(),
                        at: declared.at,
                        index,
                        params,
                        ret,
                        public: function.public,
                        name_span: function.name_span,
                        decl_range: decl_range(function),
                    };
                    index += 1;
                    declarations.push(DeclaredSignature {
                        at: declared.at,
                        name_span: function.name_span,
                        outcome: SignatureOutcome::Resolved,
                    });
                    DeclarationOccurrence::Accepted(signature)
                }
            };
            sigs.declare(ModuleScopedName::new(module, &function.name), occurrence)?;
        }
        Ok(Self {
            sigs,
            declarations,
            modules,
            imports,
        })
    }

    /// Whether every declared signature was accepted — the completeness predicate
    /// read from the ledger rather than from a flag the build loop maintained.
    pub(crate) fn every_signature_accepted(&self) -> bool {
        self.sigs.refused().next().is_none()
    }

    /// The number of monomorphic functions, which is the number of image FUNCTIONS
    /// entries lowered before tests and generic instantiations. One per accepted
    /// occurrence, not per accepted name: a repeated function name is reported by
    /// its own duplicate check and both declarations still lower a body.
    pub(crate) fn concrete_count(&self) -> u16 {
        self.sigs.accepted_occurrences().count() as u16
    }

    /// The names of every function declared in `module`, accepted or refused, so an
    /// unresolved call can offer the nearest one as a did-you-mean. Used for both an
    /// unqualified call (the caller's own module) and a qualified call (the resolved
    /// target module). A refused name is still a name the source wrote, so a
    /// near-miss on one still suggests it.
    pub(super) fn module_function_names<'s>(
        &'s self,
        module: &'s str,
    ) -> impl Iterator<Item = &'s str> {
        self.sigs
            .keys()
            .filter(move |key| key.owner() == module)
            .map(ModuleScopedName::name)
    }

    /// Resolve an unqualified call from within `module`: a function of that name in
    /// the same module, or the cause its declaration was refused for.
    pub(super) fn same_module(
        &self,
        module: &str,
        name: &str,
    ) -> Result<Binding<'_, FnSignature>, DeclarationIndexDrift> {
        self.sigs.lookup(&ModuleScopedName::new(module, name))
    }

    /// The monomorphic signature declarations, for the body-lowering walk that
    /// visits the same declarations in the same order.
    pub(crate) fn declarations(&self) -> SignatureWalk<'_> {
        SignatureWalk {
            remaining: self.declarations.iter(),
        }
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
    ) -> Result<CallResolution<'_>, DeclarationIndexDrift> {
        let module = match self.prefix_module(current, prefix)? {
            ModuleResolution::Accepted(module) => module,
            // The prefix names a module this project contains and refused. The
            // declaration reported the cause, so the call reuses it rather than
            // resolving into a scope that does not exist.
            ModuleResolution::Refused(summary) => {
                return Ok(CallResolution::ModuleRefused(summary));
            }
            ModuleResolution::Absent => return Ok(CallResolution::NotFound),
        };
        Ok(
            match self.sigs.lookup(&ModuleScopedName::new(&module, item))? {
                Binding::Accepted(sig) if sig.public || sig.module == current => {
                    CallResolution::Found(sig)
                }
                Binding::Accepted(_) => CallResolution::NotPublic,
                // A refused signature is not callable from anywhere, so visibility
                // is not the question: the declaration reported its cause and this
                // call reuses it.
                Binding::Refused(_, summary) => CallResolution::SignatureRefused(summary),
                Binding::Absent => CallResolution::NotFound,
            },
        )
    }

    /// The dotted module a `::`-qualified prefix names from within `current`, shared
    /// with generic-call resolution so both read module scope one way.
    pub(super) fn resolved_module(
        &self,
        current: &str,
        prefix: &[NameSegment],
    ) -> Result<Option<String>, DeclarationIndexDrift> {
        Ok(match self.prefix_module(current, prefix)? {
            ModuleResolution::Accepted(module) => Some(module),
            ModuleResolution::Refused(_) | ModuleResolution::Absent => None,
        })
    }

    /// What a `::`-qualified prefix names from within `current`: an importable
    /// module, a module this project contains and refused, or nothing.
    ///
    /// A failed `use` leaves no binding, so a refused dependency presents as a direct
    /// reference to its own name; a surviving binding to a since-refused target is
    /// resolved through its dotted target. One owner for both call resolution and
    /// generic-call resolution, so the two cannot disagree about module scope.
    fn prefix_module(
        &self,
        current: &str,
        prefix: &[NameSegment],
    ) -> Result<ModuleResolution<'_>, DeclarationIndexDrift> {
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
        Ok(match self.modules.lookup(dotted.as_str())? {
            Binding::Accepted(ModuleBinding) => ModuleResolution::Accepted(dotted),
            Binding::Refused(_, summary) => ModuleResolution::Refused(summary),
            Binding::Absent => ModuleResolution::Absent,
        })
    }
}

/// What a qualified prefix names: an importable dotted module, the cause a module of
/// this project was refused for, or nothing.
enum ModuleResolution<'a> {
    Accepted(String),
    Refused(&'a DeclarationRefusalSummary),
    Absent,
}

/// Fold one annotation refusal into the signature's retained cause.
///
/// The row is pushed where this site owns it, and the first cause is what the
/// declaration keeps, so a signature refused for several annotations steers its uses
/// to the first thing the reader has to fix.
fn refuse_annotation(
    refusal: &mut Option<DeclarationRefusalSummary>,
    diagnostics: &mut DiagnosticCollector,
    at: DeclarationSite<'_>,
    refused: AnnotationRefusal,
) {
    match refused.row {
        Some(row) => refuse_first(refusal, diagnostics, at, row),
        // The row is owed elsewhere — to the use that already steered to this cause,
        // or to the monomorphization owner that reports the shared instantiation
        // limit once. The signature is refused all the same, under the code that
        // covering report carries.
        None if refusal.is_none() => *refusal = Some(refuse_covered(at, refused.code)),
        None => {}
    }
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
