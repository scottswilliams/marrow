//! The function and generic-template registries resolved before body lowering.

use super::*;

use crate::analysis::ReleasedBody;
use crate::decl::{
    Binding, DeclarationBudget, DeclarationIndexDrift, DeclarationLedger, DeclarationNamespace,
    DeclarationOccurrence, DeclarationRefusalSummary, DeclarationSite, ModuleScopedName,
    refuse_covered, refuse_first,
};
use crate::source::CapturedOrigins;
use crate::types::{BuildError, NominalBoundaryKind, NominalBoundaryRoot, NominalBoundaryValue};

/// One declared function paired with where it was declared: the file identity its
/// diagnostics point into, the snapshot coordinate its editor facts are retained
/// under, and its dotted module.
pub(crate) struct DeclaredFn<'p> {
    pub(crate) file: ProjectFile,
    pub(crate) at: FileRef,
    pub(crate) module: String,
    pub(crate) decl: &'p FunctionDecl,
}

/// What an importable module name binds to.
///
/// The binding carries no payload: module scope resolves a name *through* the dotted
/// path, and every signature carries its own module string. It exists so the accepted
/// set is a typed ledger entry, which lets a refused module answer with its cause
/// instead of reading as a module the project does not contain.
pub(crate) struct ModuleBinding;

/// The project's modules keyed by dotted path: importable when accepted, and refused
/// with the cause when the header disagrees with the path or the stage that produced
/// the source refused it whole. A file with no `module` header is a script — it is
/// not a module of this namespace at all, and naming it is a genuine absence.
pub(crate) type ModuleLedger = DeclarationLedger<String, ModuleBinding>;

/// One function's key in the signature namespace: its dotted module and its name.
pub(crate) type FnKey = ModuleScopedName;

/// The module scope a signature build resolves names in, with the retention budget its
/// own ledger charges against.
pub(crate) struct ModuleScope {
    pub(crate) modules: ModuleLedger,
    /// `module -> [(final-segment binding, dotted target module)]`.
    pub(crate) imports: BTreeMap<String, Vec<(String, String)>>,
    pub(crate) origins: CapturedOrigins,
    pub(crate) budget: DeclarationBudget,
}

/// The project's functions and the module scope a call resolves against: every
/// function signature (resolved before body lowering so a forward call resolves),
/// the module ledger, and each module's `use` bindings.
pub(crate) struct FunctionRegistry {
    sigs: DeclarationLedger<FnKey, FnSignature>,
    /// Each monomorphic function declaration in the source order body lowering
    /// walks, so a body asks about the declaration it is lowering rather than about
    /// its name. See [`SignatureWalk`].
    declarations: Vec<DeclaredSignature>,
    modules: ModuleLedger,
    /// `module -> [(final-segment binding, dotted target module)]`.
    imports: BTreeMap<String, Vec<(String, String)>>,
    /// The trees this compilation captured, so a single-segment prefix that is a
    /// declared dependency alias is recognized as one.
    origins: CapturedOrigins,
}

/// How the signature build resolved one monomorphic function declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignatureOutcome {
    /// Every annotation resolved; the body lowers against the signature.
    Resolved(FuncId),
    /// A parameter or return type was refused and reported at this declaration.
    /// The body is refused with it: there is no parameter list to bind.
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
/// The refusal is keyed to the *occurrence*, never to the name: a module may declare
/// one name twice and both declarations still lower a body, so a name-keyed answer
/// would serve the first occurrence's outcome to every later one.
pub(crate) struct SignatureWalk<'a> {
    remaining: std::slice::Iter<'a, DeclaredSignature>,
}

impl SignatureWalk<'_> {
    /// How the declaration whose name is written at `at`/`name_span` resolved,
    /// consuming one entry.
    ///
    /// The site is checked, not assumed: the two walks derive their declaration
    /// sequence separately from the same parse, so a divergence is
    /// [`DeclarationIndexDrift`] rather than an answer for a neighbouring declaration.
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
    pub(crate) generic: GenericDiagnostics,
    /// Diagnostics and facts released together only after the proof's exact producer was
    /// erased.
    pub(crate) body: ReleasedBody,
}

impl FunctionRegistry {
    /// Resolve every function's signature in declaration order.
    ///
    /// A signature is refused whole: one unresolvable parameter or return type
    /// refuses the declaration and takes no image index, so no signature with a
    /// short parameter list enters the table. The declaration keeps its name, so a
    /// call to it reuses that cause rather than reading as unknown.
    ///
    /// Each accepted occurrence reserves its image slot here. Body lowering fills
    /// that exact slot; a refused body leaves it vacant without changing later IDs.
    pub(crate) fn build<'source>(
        records: &mut TypeRegistry,
        draft: &mut DraftTxn<'_>,
        durable: &DurableRegistry,
        functions: &'source [DeclaredFn<'_>],
        scope: ModuleScope,
        diagnostics: &mut DiagnosticCollector,
        boundary_roots: &mut Vec<NominalBoundaryRoot<'source>>,
    ) -> Result<FunctionRegistry, BuildError> {
        let ModuleScope {
            modules,
            imports,
            origins,
            budget,
        } = scope;
        let mut sigs = DeclarationLedger::new(DeclarationNamespace::Function, budget);
        let mut declarations = Vec::new();
        // Only monomorphic functions take an image index and enter the signature
        // table; a generic function is a template with no single image entry, so it
        // is resolved through the separate [`GenericRegistry`].
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
            let boundary_start = boundary_roots.len();
            let mut params = Vec::with_capacity(function.params.len());
            for param in &function.params {
                let site = MintSite {
                    file,
                    span: param.ty.span(),
                };
                match param_type(records, draft, durable, &param.ty, TypeEnv::EMPTY, site) {
                    Ok(ty) => {
                        if function.public {
                            let value = match ty {
                                LTy::Record { ty, .. } => Some(NominalBoundaryValue::Resource(ty)),
                                LTy::Struct { .. } | LTy::Enum { .. } | LTy::Collection { .. } => {
                                    ty.as_garg().map(NominalBoundaryValue::Value)
                                }
                                _ => None,
                            };
                            if let Some(value) = value {
                                boundary_roots.push(NominalBoundaryRoot {
                                    value,
                                    kind: NominalBoundaryKind::Input,
                                    file,
                                    span: param.ty.span(),
                                });
                            }
                        }
                        params.push(ty);
                    }
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
                    boundary_roots.truncate(boundary_start);
                    declarations.push(DeclaredSignature {
                        at: declared.at,
                        name_span: function.name_span,
                        outcome: SignatureOutcome::Refused,
                    });
                    DeclarationOccurrence::Refused(refusal)
                }
                None => {
                    let func = draft.reserve_function()?;
                    let signature = FnSignature {
                        module: module.clone(),
                        at: declared.at,
                        func,
                        params,
                        ret,
                        public: function.public,
                        name_span: function.name_span,
                        decl_range: decl_range(function),
                    };
                    declarations.push(DeclaredSignature {
                        at: declared.at,
                        name_span: function.name_span,
                        outcome: SignatureOutcome::Resolved(func),
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
            origins,
        })
    }

    /// Whether every declared signature was accepted — the completeness predicate
    /// read from the ledger rather than from a flag the build loop maintained.
    pub(crate) fn every_signature_accepted(&self) -> bool {
        self.sigs.refused().next().is_none()
    }

    /// The names of every function declared in `module`, accepted or refused, so an
    /// unresolved call can offer the nearest one as a did-you-mean. A refused name is
    /// still a name the source wrote, so a near-miss on one still suggests it.
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
    /// prefix segment binds through a `use` first, then a declared dependency alias,
    /// then a root-level module of the same name; a multi-segment prefix names a
    /// fully-qualified module path. The
    /// target must be `pub`, except a module qualifying its own function.
    pub(super) fn resolve_qualified(
        &self,
        current: &str,
        prefix: &[NameSegment],
        item: &str,
    ) -> Result<CallResolution<'_>, DeclarationIndexDrift> {
        let module = match self.prefix_module(current, prefix)? {
            ModuleResolution::Accepted(module) => module,
            // The prefix names a module this project contains and refused: reuse the
            // declaration's cause rather than resolving into a scope that does not
            // exist.
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
                // A refused signature is not callable from anywhere, so visibility is
                // not the question: this call reuses the declaration's cause.
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
    /// reference to its own name. One owner for both call resolution and generic-call
    /// resolution, so the two cannot disagree about module scope.
    ///
    /// The single-segment fallbacks are ordered: a declared dependency alias roots
    /// that dependency's modules and names no module of its own, so it is consulted
    /// before the root-level-module fallback and a root module can never shadow it.
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
                None if self.origins.declared(single.text()).is_some() => {
                    return Ok(ModuleResolution::Absent);
                }
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
/// The declaration keeps the first cause, so a signature refused for several
/// annotations steers its uses to the first thing the reader has to fix.
fn refuse_annotation(
    refusal: &mut Option<DeclarationRefusalSummary>,
    diagnostics: &mut DiagnosticCollector,
    at: DeclarationSite<'_>,
    refused: AnnotationRefusal,
) {
    match refused.row {
        Some(row) => refuse_first(refusal, diagnostics, at, row),
        // The row is owed elsewhere — to the use that already steered to this cause,
        // or to the monomorphization owner reporting the shared instantiation limit
        // once. The signature is refused under the code that covering report carries.
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
    /// from `at`: a diagnostic carries the identity, a retained fact the compact
    /// coordinate.
    pub(super) file: ProjectFile,
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
    /// `(module, name)` to template index, keyed exactly as the signature ledger keys
    /// its own declarations, so a generic call and a monomorphic call resolve a name
    /// the same way and at the same cost. A repeated declaration keeps the first.
    by_name: BTreeMap<(String, String), usize>,
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
            .collect::<Vec<GenericTemplate<'p>>>();
        let mut by_name = BTreeMap::new();
        for (index, template) in templates.iter().enumerate() {
            by_name
                .entry((template.module.clone(), template.decl.name.clone()))
                .or_insert(index);
        }
        Self { templates, by_name }
    }

    /// The templates, for the once-checked template pass and instance draining.
    pub(crate) fn templates(&self) -> &[GenericTemplate<'p>] {
        &self.templates
    }

    /// The template index of a generic call to `name` in `module`, qualified or not.
    pub(super) fn same_module(&self, module: &str, name: &str) -> Option<usize> {
        self.by_name
            .get(&(module.to_string(), name.to_string()))
            .copied()
    }

    /// The template named `item` in `module`, with its `pub` flag, for a qualified
    /// generic call. The caller checks visibility against the calling module.
    pub(super) fn in_module(&self, module: &str, item: &str) -> Option<(usize, bool)> {
        self.same_module(module, item)
            .map(|index| (index, self.templates[index].public))
    }
}

impl GenericTemplate<'_> {
    /// The snapshot coordinate this template's editor facts are retained under.
    pub(crate) fn at(&self) -> FileRef {
        self.at
    }
}

// Generic instantiation identity — for functions and value types together — is
// owned by the [`TypeRegistry`]'s single monomorphization table (see
// `reserve_fn_instance`/`next_fn_pending`), keyed by `(template, args)` and bounded
// by `MAX_INSTANTIATIONS`.
