//! Function-body lowering (design §B/§D).
//!
//! [`FnLowerer`] type-checks the compiled subset and lowers one function body to
//! a draft instruction stream. Locals are allocated one fresh slot per `const`/
//! `var`/param/`if const` binding — slots are never reused — so every read is
//! dominated by the slot's single write and the independent verifier's
//! definite-init dataflow is satisfied. Jumps are emitted with placeholder targets
//! and patched to instruction indices once the target position is known; the
//! encoder rewrites indices to byte offsets.
//!
//! ## Panic surface (never reachable from a source shape)
//!
//! Every source-level problem lowering can encounter is reported by pushing a typed
//! [`SourceDiagnostic`] onto `diagnostics` and returning `None`; lowering never aborts
//! on ill-typed or unsupported source. The remaining `expect`/`unreachable!`/`panic!`
//! sites assert invariants established *before* the panicking line, so a source shape
//! cannot reach one — only a compiler bug could. Each falls into one class, and each
//! carries a message naming its guarantor:
//!
//! - **Checker-classified type** — a scrutinee already resolved to an enum, a type
//!   already classified as a struct or nominal, a bare enum value already bound to its
//!   variants. The checker rejects the mismatched source (`check.type`,
//!   `check.match_arm`, `check.unsupported`) before lowering runs.
//! - **Match-arm narrowing** — a dispatch whose earlier arms removed every other case
//!   (an admitted arithmetic op, `and`/`or` short-circuit, a text-floor or temporal
//!   builtin the caller already matched by name).
//! - **Parser-guaranteed shape** — a binary operation has both operands; a list
//!   literal reaching the inferred path is non-empty (the empty case is handled first).
//! - **Lowering's own bookkeeping** — a loop context pushed at loop entry is present at
//!   `break`/loop-exit; a jump placeholder patched here was emitted here as a jump; a
//!   group-leaf `delete` was routed to its dedicated path before the shared emit.
//!
//! The audit that established this (2026-07-18): every `panic!`/`unwrap`/`expect`/
//! `unreachable!` in this file was enumerated and classified into the four classes
//! above; the one bare `unwrap` was given a message; and a battery of adversarial
//! source shapes (`break`/`continue` outside a loop, a `match` on a non-enum, a
//! mis-arity builtin call, an ill-typed operator, an unresolved enum member, an empty
//! inferred list) was driven through the production pipeline and each produced a typed
//! diagnostic, never a panic. New panic-class sites must fall into one of these
//! classes and say so, or become a diagnostic.

use std::collections::{BTreeMap, BTreeSet};

use marrow_codes::Code;
use marrow_image::{
    EnumId, FuncId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SemanticPath, SiteDef,
    SpanEntry, TypeId,
};
use marrow_project::FileIdentity;

use crate::analysis::{DefinitionTarget, FactSink, FileRef};
use marrow_syntax::{
    Argument, BinaryOp, Block, CheckedBind, ElseIf, Expression, ForBinding, FunctionDecl,
    InterpolationPart, LiteralKind, MatchArm, NameSegment, RangeExpr, SourceSpan, Statement,
    TraversalBound, TypeExpr, UnaryOp, decode_interpolation_text, decode_string_literal,
    duration_unit_seconds, range_expr,
};

use crate::decl::{
    Binding, DeclarationIndexDrift, DeclarationNamespace, DeclarationRefusalId,
    DeclarationRefusalSummary, declaration_refused,
};
use crate::diag::{BoundedDiagnostics, DiagnosticCollector, SourceDiagnostic};
use crate::durable::{DurableRegistry, RootBinding};
use crate::konst::{ConstRegistry, ConstScalar};
use crate::scalar::ScalarType;
use crate::types::{
    CollSpec, EnumVariantSelection, GArg, GenericDiagnostics, GenericInvariant as LowerInvariant,
    MintSite, NominalId, OPTION_NONE, OPTION_SOME, ProductFieldProjection, RESULT_ERR, RESULT_OK,
    ReservedEnumArgs, ResolveError, ResolveRefusal, StaticNamedType, StructFieldProjection,
    SupportSet, TemplateProofScope, TypeConstraint, TypeInstId, TypeMetadataSession, TypeRegistry,
};

/// Whether control continues past a statement or block, leaves it (via `return`,
/// `break`, or `continue`), or is terminally rejected by a lowering owner.
/// `Rejected` is propagated by every nested control owner, so later branches and
/// structural checks cannot observe a partially lowered body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Fallthrough,
    Terminates,
    Rejected,
}

/// The only two outcomes of a finite positional walk: completed lowering with its
/// deferred `break` jumps, or terminal rejection by a lowering owner.
enum PositionalWalkOutcome {
    Complete(Vec<usize>),
    Rejected,
}

/// The declared return shape of a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetType {
    Unit,
    Value(LTy),
}

/// Which body is being lowered. Only a `test` body admits the owned `assert`
/// statement; an ordinary function body rejects it with `check.assert_outside_test`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BodyKind {
    Function,
    Test,
}

/// The outcome of lowering a call: whether it yields a value, nothing, or diverges
/// (never returns to the caller, e.g. `unreachable`).
enum CallResult {
    Unit,
    Value(LTy),
    Diverges,
}

/// A resolved function signature, keyed by index (the image FUNCTIONS position,
/// which equals declaration order).
pub(crate) struct FnSignature {
    /// The dotted module the function is declared in (path-derived).
    module: String,
    index: u16,
    params: Vec<LTy>,
    ret: RetType,
    public: bool,
    /// The snapshot coordinate this function's definition target is retained under.
    at: FileRef,
    /// The span of the function's declared name — the definition selection range.
    name_span: SourceSpan,
    /// The function's header-through-body span — the full definition range.
    decl_range: SourceSpan,
}

impl FnSignature {
    /// The definition target of this function for an editor definition query.
    pub(super) fn definition_target(&self) -> DefinitionTarget {
        DefinitionTarget::new(self.at, self.name_span, self.decl_range)
    }
}

/// The header-through-body declaration range of a function: the header-only `span`
/// joined with the body span. The single owner of this join, for both the monomorphic
/// signature table and the generic-template definition target.
pub(super) fn decl_range(decl: &FunctionDecl) -> SourceSpan {
    SourceSpan {
        start_byte: decl.span.start_byte,
        end_byte: decl.body.span.end_byte,
        line: decl.span.line,
        column: decl.span.column,
    }
}

/// A successfully lowered function: its image index and the indices of the
/// functions it calls directly (for check-time recursion detection).
pub(crate) struct Lowered {
    pub func: FuncId,
    pub callees: Vec<u16>,
    /// Spans of durable mutations this body performs outside any `transaction` block.
    pub unwrapped_mutations: Vec<SourceSpan>,
    /// Calls this body performs outside any `transaction` block, with their spans.
    pub unwrapped_calls: Vec<(u16, SourceSpan)>,
    /// Whether this body performs a durable-place operation directly (as opposed to
    /// reaching durable data only through calls). Consumed by the test-body
    /// strict-separation check.
    pub has_direct_durable_op: bool,
    /// Whether this body owns a `transaction` block (emits a begin). A test body that
    /// drives such a function mixes invocation boundaries and is refused.
    pub owns_transaction: bool,
    /// This body's lowered instruction tape, and the full source span of each
    /// instruction (parallel to `code`). The check-time transaction-ownership pass
    /// walks this tape — the same instruction sequence the verifier reconstructs from
    /// the image — to report the ownership-lattice laws at their source spans.
    pub code: Vec<Instr>,
    pub code_spans: Vec<SourceSpan>,
}

/// The outcome of resolving a call target against module scope.
pub(crate) enum CallResolution<'a> {
    /// A resolved callable signature.
    Found(&'a FnSignature),
    /// A function with the name exists in the target module but is not `pub`, so it
    /// is not callable across the module boundary.
    NotPublic,
    /// A function of that name is declared in the target module and its signature
    /// was refused, so it is callable from nowhere. The declaration reported the
    /// cause; this call reuses it.
    SignatureRefused(&'a DeclarationRefusalSummary),
    /// The qualifying prefix names a module this project contains and refused, so no
    /// scope to resolve `item` in exists. The module's declaration reported the
    /// cause; this call reuses it.
    ModuleRefused(&'a DeclarationRefusalSummary),
    /// No function with that name is reachable from the calling module.
    NotFound,
}

/// Whether a body produced an image function. A refused body is the ordinary
/// outcome of a source error inside it: its diagnostics are already pushed, it
/// consumed no image index, and the phase artifact that depends on every declared
/// body having lowered is thereby unavailable. Naming the two cases keeps that
/// consequence at the call site instead of leaving it to an untyped `None`.
pub(crate) enum BodyOutcome {
    Lowered(Lowered),
    Refused,
}

type LowerResult = Result<BodyOutcome, LowerInvariant>;

/// Which lowering pass a body is in: an ordinary or instance body that emits an
/// image function and monomorphizes its generic calls, or the once-checked template
/// pass that lowers a generic body against abstract type parameters into a throwaway
/// draft and only checks (never monomorphizes) the generic calls it makes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LowerMode {
    Concrete,
    Template,
}

/// One in-scope local binding.
struct Local {
    name: String,
    ty: LTy,
    mutable: bool,
    slot: u16,
}

/// A resolved nested place path rooted at a local. `indices` are the field slots
/// descended from the local (empty for the bare local); `ty` is the value type at
/// the end of that descent — the container a leaf field is then read or written in.
/// Every descended field is a present composite, so the path supports a read-modify-
/// write without a presence test.
struct PlaceChain {
    slot: u16,
    mutable: bool,
    root_span: SourceSpan,
    root_name: String,
    ty: LTy,
    indices: Vec<u16>,
}

/// A loop's patch targets: where `continue` jumps, and the jumps `break` emits that
/// must be patched to the loop's exit once it is known.
struct LoopCtx {
    continue_target: usize,
    break_jumps: Vec<usize>,
}

/// The refusal a handle addresses, from the namespace ledger that minted it.
///
/// The one place a `Copy` refusal handle becomes a renderable cause, so no
/// consumer picks a ledger by guesswork. The tag is checked by the ledger itself:
/// a handle presented to the wrong owner is drift, not a neighbouring summary.
pub(super) fn refusal_summary<'r>(
    records: &'r TypeRegistry,
    durable: &'r DurableRegistry,
    id: DeclarationRefusalId,
) -> Result<&'r DeclarationRefusalSummary, DeclarationIndexDrift> {
    match id.namespace() {
        DeclarationNamespace::NamedType => records.refusal(id),
        DeclarationNamespace::DurableRoot => durable.refusal(id),
        // A constant is looked up by its own name at its own use site, a resource
        // member by its owner and its own name, a module by its dotted path, and a
        // function by its module and name, so none travels through type resolution
        // and no handle of one reaches here.
        DeclarationNamespace::Constant
        | DeclarationNamespace::Function
        | DeclarationNamespace::Module
        | DeclarationNamespace::ResourceMember => Err(DeclarationIndexDrift),
    }
}

/// What a type-annotation position does with a resolution refusal.
///
/// `row` is the report this site owns, once per refused key: the causal steer for a
/// refused declaration, the subset-gap phrase for a genuine gap, and `None` where
/// the report is owed elsewhere — to the use that already steered to this cause, or
/// to the monomorphization owner that reports the shared instantiation limit once.
/// `code` carries the cause either way, so a declaration refused for an annotation
/// it could not resolve retains a cause even when it owes no row.
pub(super) struct AnnotationRefusal {
    pub(super) row: Option<SourceDiagnostic>,
    pub(super) code: &'static str,
}

/// The refusal a type-annotation position reports.
///
/// The signature-building and return-type sites have no `FnLowerer`, so they take
/// this directly; `reject_resolution` is the same decision inside one.
pub(super) fn annotation_refusal_row(
    records: &TypeRegistry,
    durable: &DurableRegistry,
    refusal: ResolveRefusal,
    file: &FileIdentity,
    span: SourceSpan,
    subject: &str,
) -> Result<AnnotationRefusal, LowerInvariant> {
    Ok(match refusal {
        ResolveRefusal::Limit => AnnotationRefusal {
            row: None,
            code: Code::CheckInstantiationLimit.as_str(),
        },
        ResolveRefusal::Unsupported => {
            let row = unsupported(file, span, subject);
            AnnotationRefusal {
                code: row.code(),
                row: Some(row),
            }
        }
        ResolveRefusal::RefusedDeclaration(id) => {
            let summary = refusal_summary(records, durable, id)?;
            AnnotationRefusal {
                code: summary.code(),
                row: summary
                    .steer_once()
                    .then(|| declaration_refused(file, span, summary)),
            }
        }
    })
}

pub(crate) struct FnLowerer<'a> {
    draft: &'a mut ImageDraft,
    records: &'a TypeRegistry,
    durable: &'a DurableRegistry,
    functions: &'a FunctionRegistry,
    /// The generic function templates, for resolving a generic call target.
    generics: &'a GenericRegistry<'a>,
    consts: &'a ConstRegistry,
    diagnostics: &'a mut DiagnosticCollector,
    /// The scoped editor-fact borrow for this body. A dependency gap is written
    /// through it as it is discovered — like a diagnostic — so the gap survives even
    /// when the body it sits in fails to lower (an unresolved call fails the body).
    /// Hover facts stage in this body's own buffer and are admitted by the caller only
    /// when the body lowers, exactly as they were before.
    facts: FactSink<'a>,
    /// Store roots whose durable identity failed admission that have already had one
    /// reference-site steer emitted this compile. A dropped root is referenced from
    /// many sites; the primary `check.durable_identity` reports name the fix once per
    /// missing row, so the reference steer fires once per root rather than at every
    /// use, keeping one dropped root from flooding the transcript. Shared across every
    /// body lowered in the compile.
    file: &'a FileIdentity,
    /// The dotted module the function being lowered belongs to; unqualified calls
    /// resolve within it.
    module: &'a str,
    /// The type-parameter environment: empty for a monomorphic body, the abstract
    /// parameters for the template pass, or the concrete substitutions for an
    /// instance body.
    type_env: Vec<TypeParamSlot>,
    /// Whether this body emits an image function and monomorphizes, or is the
    /// once-checked template pass over abstract parameters.
    mode: LowerMode,
    code: Vec<Instr>,
    spans: Vec<SpanEntry>,
    /// Full UTF-8 source span of each emitted instruction, parallel to `code`. The
    /// image itself keeps only the line/column [`SpanEntry`]; these byte-accurate
    /// spans stay compiler-local so the check-time transaction-ownership pass can point
    /// a diagnostic at the exact offending construct. Never enters the image.
    full_spans: Vec<SourceSpan>,
    /// The image indices of every function this body calls directly, in emission
    /// order. The caller uses these to detect a recursive call cycle at check time.
    calls: Vec<u16>,
    /// Lexical `transaction`-block nesting depth at the current emission point. A
    /// durable mutation or a call emitted at depth zero is not covered by an ambient
    /// transaction owned by this body; the requires-ambient-transaction check consumes
    /// the sites recorded below.
    txn_depth: u32,
    /// Spans of durable mutations emitted outside any `transaction` block in this body.
    unwrapped_mutations: Vec<SourceSpan>,
    /// Calls emitted outside any `transaction` block in this body, paired with their
    /// call-site span. A call to a callee that itself requires an ambient transaction
    /// is refused here when this body is an export entry.
    unwrapped_calls: Vec<(u16, SourceSpan)>,
    locals: Vec<Local>,
    /// Names of `const`/`var` bindings whose initializer failed to type-check, so no
    /// `Local` was bound. A later reference to such a name is the consequence of the
    /// initializer's own error, not a fresh undefined name; suppressing it keeps one
    /// bad initializer from spawning an `is not in scope` report at every later use.
    poisoned_bindings: BTreeSet<String>,
    /// In-scope source-local named `place` bindings, scoped like `locals`.
    places: Vec<PlaceLocal>,
    /// The key-paths of `place` bindings a presence fact currently dominates: the
    /// containing entry is known present here, so a sparse-field set through the
    /// place lowers to the strict present-entry form. Each fact is the place's whole
    /// key-path as pre-evaluated slots (root-first), so a root and a branch place are
    /// tracked uniformly. Scoped like `locals` (a fact established in a guarded block or
    /// after an upsert does not outlive its block); the verifier rechecks each strict
    /// set independently.
    present_places: Vec<Vec<u16>>,
    loops: Vec<LoopCtx>,
    /// Monotonic slot allocator; never decreases, so slots are never reused.
    slot_count: u16,
    /// The frame's first over-bound request is a source-located terminal refusal.
    /// Keeping this state distinct from `failed` suppresses duplicate reports and
    /// makes every nested flow owner stop before it can use a missing slot.
    local_limit_reached: bool,
    ret: RetType,
    /// Whether this is a function or a test body; gates the owned `assert`.
    body_kind: BodyKind,
    failed: bool,
    invariant: Option<LowerInvariant>,
}

mod builtins;
mod diagnostics;
mod durable;
mod exprs;
mod ltype;
mod registry;
mod stmts;
mod types;

pub(in crate::lower) use self::builtins::*;
pub(in crate::lower) use self::diagnostics::*;
pub(in crate::lower) use self::durable::*;
pub(in crate::lower) use self::ltype::*;
pub(in crate::lower) use self::registry::*;
pub(in crate::lower) use self::types::*;

pub(crate) use self::builtins::{
    builtin_const_int, builtin_value_names, is_reserved_builtin_name, reserved_builtin_name,
};
pub(crate) use self::durable::{is_durable_place_op, is_mutation_instr};
pub(crate) use self::registry::{
    DeclaredFn, FunctionRegistry, GenericRegistry, ModuleBinding, ModuleLedger, SignatureOutcome,
};
pub(crate) use self::types::parse_int;

impl<'a> FnLowerer<'a> {
    /// A fresh lowerer over an empty body, for one function or test body. The
    /// shared field set has this single owner; `ret` and `body_kind` are the only
    /// per-body-kind inputs.
    #[allow(clippy::too_many_arguments)]
    fn new(
        draft: &'a mut ImageDraft,
        records: &'a TypeRegistry,
        durable: &'a DurableRegistry,
        functions: &'a FunctionRegistry,
        generics: &'a GenericRegistry<'a>,
        consts: &'a ConstRegistry,
        diagnostics: &'a mut DiagnosticCollector,
        facts: FactSink<'a>,
        file: &'a FileIdentity,
        module: &'a str,
        ret: RetType,
        body_kind: BodyKind,
    ) -> Self {
        FnLowerer {
            draft,
            records,
            durable,
            functions,
            generics,
            consts,
            diagnostics,
            facts,
            file,
            module,
            type_env: Vec::new(),
            mode: LowerMode::Concrete,
            code: Vec::new(),
            spans: Vec::new(),
            full_spans: Vec::new(),
            calls: Vec::new(),
            txn_depth: 0,
            unwrapped_mutations: Vec::new(),
            unwrapped_calls: Vec::new(),
            locals: Vec::new(),
            poisoned_bindings: BTreeSet::new(),
            places: Vec::new(),
            present_places: Vec::new(),
            loops: Vec::new(),
            slot_count: 0,
            local_limit_reached: false,
            ret,
            body_kind,
            failed: false,
            invariant: None,
        }
    }

    /// Whether this body's hover displays are still worth rendering: its facts are
    /// retained (a generic instance's duplicate its template's and are discarded), and no
    /// snapshot ceiling has been crossed. A caller renders the display inside this guard
    /// so a discarded body never pays the O(depth) spelling render, which on a deeply
    /// monomorphized instance would be Σ = O(instances²) across a divergent
    /// monomorphization.
    fn collects_hover(&self) -> bool {
        self.facts.renders_facts()
    }

    /// Admit one editor hover fact at `span` through the ledger, at the push: a resolved
    /// local or parameter use carries a type display and no definition; a resolved
    /// function callee carries its signature display and its definition target.
    fn record_hover(
        &mut self,
        span: SourceSpan,
        display: Box<str>,
        definition: Option<DefinitionTarget>,
    ) {
        #[cfg(test)]
        crate::types::bump_hover_spelling_chars(display.len());
        self.facts.hover(span, display, definition);
    }

    /// The hover display of a local or parameter's value type. A bare template type
    /// parameter renders by its declared spelling from the type-parameter environment
    /// (`T`) rather than the positional `type parameter #N` form, so a hover inside a
    /// generic template body reads the source name; every other type, and a monomorphic
    /// body (whose environment is empty and whose types are never [`LTy::Param`]), defers
    /// to the canonical spelling unchanged.
    fn hover_type_display(&self, ty: LTy) -> String {
        if let LTy::Param { index, optional } = ty
            && let Some(slot) = self.type_env.get(index as usize)
        {
            return if optional {
                format!("{}?", slot.name)
            } else {
                slot.name.clone()
            };
        }
        ty.spelling(self.records)
    }

    /// Lower `function` and add it to the draft, returning its assigned [`FuncId`]
    /// and the indices of the functions it calls directly. Export minting is the
    /// caller's job: it holds the dotted module name needed to compute the export's
    /// [`marrow_image::ExportId`]. A function that fails to lower pushes its
    /// diagnostics and returns [`BodyOutcome::Refused`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower(
        draft: &'a mut ImageDraft,
        records: &'a TypeRegistry,
        durable: &'a DurableRegistry,
        functions: &'a FunctionRegistry,
        generics: &'a GenericRegistry<'a>,
        consts: &'a ConstRegistry,
        diagnostics: &'a mut DiagnosticCollector,
        facts: FactSink<'a>,
        file: &'a FileIdentity,
        module: &'a str,
        function: &FunctionDecl,
    ) -> LowerResult {
        Self::lower_with_env(
            draft,
            records,
            durable,
            functions,
            generics,
            consts,
            diagnostics,
            facts,
            file,
            module,
            function,
            Vec::new(),
            LowerMode::Concrete,
        )
    }

    /// Lower one monomorphized instance of a generic template: bind each type
    /// parameter to its concrete argument, then lower the template body exactly like
    /// an ordinary function into the real draft. The returned [`FuncId`] must equal
    /// the index the registry reserved for this instance (asserted by the driver),
    /// since instances are added to the image in the order they were minted.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower_instance(
        draft: &'a mut ImageDraft,
        records: &'a TypeRegistry,
        durable: &'a DurableRegistry,
        functions: &'a FunctionRegistry,
        generics: &'a GenericRegistry<'a>,
        consts: &'a ConstRegistry,
        diagnostics: &'a mut DiagnosticCollector,
        facts: FactSink<'a>,
        template: &'a GenericTemplate<'a>,
        args: &[GArg],
    ) -> LowerResult {
        let type_env = template
            .type_params
            .iter()
            .zip(args)
            .map(|((name, _), arg)| TypeParamSlot {
                name: name.clone(),
                binding: ParamBinding::Concrete(*arg),
            })
            .collect();
        Self::lower_with_env(
            draft,
            records,
            durable,
            functions,
            generics,
            consts,
            diagnostics,
            facts,
            &template.file,
            &template.module,
            template.decl,
            type_env,
            LowerMode::Concrete,
        )
    }

    /// Run the once-checked template pass over a generic function: lower its body
    /// against abstract type parameters (each admitting only its declared
    /// constraint) into a throwaway draft paired with an isolated registry clone, so
    /// the body is type-checked once — including rejecting `==`/`<` on an
    /// unconstrained parameter — independently of whether or how it is instantiated.
    /// Only its diagnostics are kept; the emitted code and throwaway image are
    /// discarded.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn check_template(
        draft: &mut ImageDraft,
        records: &TypeRegistry,
        durable: &DurableRegistry,
        functions: &FunctionRegistry,
        generics: &GenericRegistry,
        consts: &ConstRegistry,
        facts: FactSink<'_>,
        template: &GenericTemplate,
    ) -> Result<TemplateProofOutcome, LowerInvariant> {
        let file = &template.file;
        let module = &template.module;
        // Prove the body directly on the in-progress registry and draft — so it sees every
        // already-minted type at its real index (a concrete callee's signature stays
        // consistent) — inside a scope that erases the abstract-parameter instantiations and
        // throwaway emitted code the pass appends. A fill batch never mutates a settled prefix
        // row, so rewinding the appended suffix restores the exact pre-proof state. The scope
        // guard restores both owners on every path — a normal return, an early lowering
        // invariant, or an unwind — so no throwaway type or instruction survives a failed
        // proof. It restores the draft and the registry, and nothing else: the proof's
        // diagnostics live in a local collector that a failure drops, while its editor facts
        // are admitted where they are derived and are not retracted by a later failure of
        // this pass (see `FactSink`).
        let mut scope = TemplateProofScope::enter(records, draft)?;
        // The proof's local collector: success seals it into the outcome's
        // terminal for the outer stage owner to absorb; an invariant failure
        // drops it with the scope.
        let mut diagnostics = DiagnosticCollector::new();
        // Each parameter's position in this vector is its abstract `LTy::Param`
        // index, and its constraint is read back from here by `constraint_at`.
        let type_env = template
            .type_params
            .iter()
            .map(|(name, constraint)| TypeParamSlot {
                name: name.clone(),
                binding: ParamBinding::Abstract(*constraint),
            })
            .collect::<Vec<_>>();
        // The template body is checked exactly once (never per instance), so its editor
        // facts are collected here: a template-parameter use renders by its declared
        // spelling and no divergent-monomorphization O(N²) rendering occurs. They reach
        // the ledger through the sink as they are derived; only the throwaway image
        // function this pass emits is discarded with the scope.
        FnLowerer::lower_with_env(
            scope.draft(),
            records,
            durable,
            functions,
            generics,
            consts,
            &mut diagnostics,
            facts,
            file,
            module,
            template.decl,
            type_env,
            LowerMode::Template,
        )?;
        // Take the proof's diagnostics before the scope drops: `take_generic_diagnostics`
        // drains the swapped-in buffer and limit owner that the guard then re-seats.
        Ok(TemplateProofOutcome {
            diagnostics: diagnostics.finish(),
            generic: records.take_generic_diagnostics(),
        })
    }

    /// The shared driver for an ordinary function, a generic instance, and the
    /// template pass: resolve the return type in the type environment, bind the
    /// value parameters, lower the body, and (for an emitting pass) add the image
    /// function. The `type_env` and `mode` distinguish the three.
    #[allow(clippy::too_many_arguments)]
    fn lower_with_env(
        draft: &'a mut ImageDraft,
        records: &'a TypeRegistry,
        durable: &'a DurableRegistry,
        functions: &'a FunctionRegistry,
        generics: &'a GenericRegistry<'a>,
        consts: &'a ConstRegistry,
        diagnostics: &'a mut DiagnosticCollector,
        facts: FactSink<'a>,
        file: &'a FileIdentity,
        module: &'a str,
        function: &FunctionDecl,
        type_env: Vec<TypeParamSlot>,
        mode: LowerMode,
    ) -> LowerResult {
        let ret = {
            let env = TypeEnv { params: &type_env };
            match &function.return_type {
                None => RetType::Unit,
                Some(annotation) => {
                    let site = MintSite {
                        file,
                        span: annotation.span(),
                    };
                    match resolve_type(records, draft, durable, annotation, env, site) {
                        Ok(ty) => RetType::Value(ty),
                        Err(ResolveError::Refusal(refusal)) => {
                            if let Some(row) = annotation_refusal_row(
                                records,
                                durable,
                                refusal,
                                file,
                                annotation.span(),
                                "this return type",
                            )?
                            .row
                            {
                                diagnostics.push(row);
                            }
                            return Ok(BodyOutcome::Refused);
                        }
                        Err(ResolveError::Invariant(invariant)) => return Err(invariant),
                    }
                }
            }
        };

        let mut lowerer = FnLowerer::new(
            draft,
            records,
            durable,
            functions,
            generics,
            consts,
            diagnostics,
            facts,
            file,
            module,
            ret,
            BodyKind::Function,
        );
        lowerer.type_env = type_env;
        lowerer.mode = mode;

        // Params occupy the first slots, pre-initialized to their type: a bare
        // scalar, a bare nominal (int-shaped), or a bare struct record ref.
        //
        // One entry per source parameter, in source order: the bound parameter's
        // lowered type, or `None` for one whose type was refused. The image's
        // parameter list is built from this rather than from a positional zip
        // against `locals`, which body lowering also grows — a dropped parameter
        // would otherwise shift the correspondence and give the image a signature
        // the source never wrote.
        let mut declared_params: Vec<Option<LTy>> = Vec::with_capacity(function.params.len());
        for param in &function.params {
            if !param.keys.is_empty() {
                lowerer.fail(unsupported(file, function.span, "a keyed parameter"));
            }
            if is_reserved_builtin_name(&param.name) {
                lowerer.fail(reserved_builtin_name(file, function.span, &param.name));
            }
            let Some(ty) = lowerer.param_type(&param.ty) else {
                if lowerer.terminal_rejection() {
                    return lowerer.finish(&function.name, Vec::new(), ImageType::Unit);
                }
                // The parameter keeps its name. Its type was reported at the
                // annotation, so a use of it in the body reuses that cause and
                // fails silently instead of calling a name the reader can see
                // written unknown, once per use.
                lowerer.poisoned_bindings.insert(param.name.clone());
                declared_params.push(None);
                continue;
            };
            let Some(slot) = lowerer.alloc_slot(param.ty.span()) else {
                return lowerer.finish(&function.name, Vec::new(), ImageType::Unit);
            };
            declared_params.push(Some(ty));
            lowerer.locals.push(Local {
                name: param.name.clone(),
                ty,
                mutable: false,
                slot,
            });
            // A nominal parameter revalidates its interval on entry. In-language
            // callers already passed the type, but the image records only the base
            // int, so a terminal or wire caller could otherwise inject an
            // out-of-interval value into the type.
            if let Some(id) = ty.bare_nominal() {
                let info = lowerer.records.nominal(id);
                let (lo, hi) = (info.lo, info.hi);
                lowerer.push(Instr::LocalGet(slot), function.span);
                lowerer.push(Instr::RangeGuard { lo, hi }, function.span);
                lowerer.push(Instr::Pop, function.span);
            }
        }

        if lowerer.terminal_rejection() {
            return lowerer.finish(&function.name, Vec::new(), ImageType::Unit);
        }

        let body_flow = lowerer.lower_block(&function.body);
        match (body_flow, lowerer.ret) {
            (Flow::Terminates, _) => {}
            (Flow::Fallthrough, RetType::Unit) => {
                lowerer.push(Instr::Return, function.body.span);
            }
            (Flow::Fallthrough, RetType::Value(_)) => {
                lowerer.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    file,
                    function.span,
                    "not all paths return a value".to_string(),
                ));
            }
            (Flow::Rejected, _) => {
                return lowerer.finish(&function.name, Vec::new(), ImageType::Unit);
            }
        }

        // A nominal param erases to its base int in the image; in-language callers
        // passed the type, and the entry guard emitted above revalidates the
        // interval against out-of-language callers. A struct param carries its image
        // record ref (`ImageType::Record`).
        let Some(params) = declared_params
            .iter()
            .map(|ty| ty.as_ref().map(|ty| ty.image()))
            .collect::<Option<Vec<ImageType>>>()
        else {
            // A refused parameter has no image type, so this function has no
            // parameter list to emit. The body already failed; refusing the list
            // here is what keeps a shortened one from ever being read as the
            // signature the source wrote.
            return lowerer.finish(&function.name, Vec::new(), ImageType::Unit);
        };
        let ret_ref = match ret {
            RetType::Unit => ImageType::Unit,
            RetType::Value(ty) => ty.image(),
        };
        lowerer.finish(&function.name, params, ret_ref)
    }

    /// Lower a `test` body into a storeless, zero-argument, unit-returning function
    /// and return its [`Lowered`] identity. The body is the only place the owned
    /// `assert` is legal; `name` is the test title (interned as the function name),
    /// and the caller binds it into the image's TEST-ENTRY table.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower_test(
        draft: &'a mut ImageDraft,
        records: &'a TypeRegistry,
        durable: &'a DurableRegistry,
        functions: &'a FunctionRegistry,
        generics: &'a GenericRegistry<'a>,
        consts: &'a ConstRegistry,
        diagnostics: &'a mut DiagnosticCollector,
        facts: FactSink<'a>,
        file: &'a FileIdentity,
        module: &'a str,
        name: &str,
        body: &Block,
    ) -> LowerResult {
        let mut lowerer = FnLowerer::new(
            draft,
            records,
            durable,
            functions,
            generics,
            consts,
            diagnostics,
            facts,
            file,
            module,
            RetType::Unit,
            BodyKind::Test,
        );
        // A test body is a unit-returning block: control that falls through ends with
        // an implicit return, exactly like a unit function.
        match lowerer.lower_block(body) {
            Flow::Fallthrough => lowerer.push(Instr::Return, body.span),
            Flow::Terminates => {}
            Flow::Rejected => return lowerer.finish(name, Vec::new(), ImageType::Unit),
        }
        lowerer.finish(name, Vec::new(), ImageType::Unit)
    }

    /// Intern the function name and source, add the lowered function to the draft,
    /// and return its identity — the shared tail of function and test lowering. A
    /// body that failed to lower returns [`BodyOutcome::Refused`].
    fn finish(mut self, name: &str, params: Vec<ImageType>, ret_ref: ImageType) -> LowerResult {
        if let Some(invariant) = self.invariant {
            return Err(invariant);
        }
        if self.failed || self.terminal_rejection() {
            return Ok(BodyOutcome::Refused);
        }
        let name_id = self.draft.intern_string(name);
        let source_id = self.draft.intern_string(self.file.as_str());
        let code = std::mem::take(&mut self.code);
        let spans = std::mem::take(&mut self.spans);
        let code_spans = std::mem::take(&mut self.full_spans);
        let has_direct_durable_op = code.iter().any(is_durable_place_op);
        let owns_transaction = code.iter().any(|instr| matches!(instr, Instr::TxnBegin));
        let func_id = self.draft.add_function(FunctionDef {
            name: name_id,
            source: source_id,
            params,
            ret: ret_ref,
            local_count: self.slot_count,
            code: code.clone(),
            spans,
        });
        Ok(BodyOutcome::Lowered(Lowered {
            func: func_id,
            callees: std::mem::take(&mut self.calls),
            unwrapped_mutations: std::mem::take(&mut self.unwrapped_mutations),
            unwrapped_calls: std::mem::take(&mut self.unwrapped_calls),
            has_direct_durable_op,
            owns_transaction,
            code,
            code_spans,
        }))
    }

    // --- emission helpers ---

    fn here(&self) -> usize {
        self.code.len()
    }

    fn push(&mut self, instr: Instr, span: SourceSpan) {
        if self.txn_depth == 0 {
            match &instr {
                Instr::Call(target) => self.unwrapped_calls.push((*target, span)),
                _ if is_mutation_instr(&instr) => self.unwrapped_mutations.push(span),
                _ => {}
            }
        }
        let index = self.code.len() as u32;
        self.code.push(instr);
        self.spans.push(SpanEntry {
            instr_index: index,
            line: span.line.max(1),
            column: span.column.max(1),
        });
        self.full_spans.push(span);
    }

    fn push_jump(&mut self, span: SourceSpan) -> usize {
        let at = self.here();
        self.push(Instr::Jump(0), span);
        at
    }

    fn push_jif(&mut self, span: SourceSpan) -> usize {
        let at = self.here();
        self.push(Instr::JumpIfFalse(0), span);
        at
    }

    fn push_branch_present(&mut self, span: SourceSpan) -> usize {
        let at = self.here();
        self.push(Instr::BranchPresent(0), span);
        at
    }

    fn patch(&mut self, at: usize, target: usize) {
        match &mut self.code[at] {
            Instr::Jump(t)
            | Instr::JumpIfFalse(t)
            | Instr::BranchPresent(t)
            | Instr::IntAddChecked(t)
            | Instr::IntSubChecked(t)
            | Instr::IntMulChecked(t)
            | Instr::IntNegChecked(t)
            | Instr::IntDivChecked(t)
            | Instr::IntRemChecked(t) => *t = target as u32,
            #[expect(
                clippy::unreachable,
                reason = "lowering bookkeeping: a patch site is recorded only for the jump instructions matched above, so no other instruction is ever patched here"
            )]
            other => unreachable!("patch target is not a jump: {other:?}"),
        }
    }

    fn patch_all(&mut self, jumps: Vec<usize>, target: usize) {
        for jump in jumps {
            self.patch(jump, target);
        }
    }

    fn alloc_slot(&mut self, request_span: SourceSpan) -> Option<u16> {
        const _: () = assert!(marrow_image::bounds::MAX_LOCALS <= u16::MAX as usize);

        if self.local_limit_reached {
            return None;
        }
        if usize::from(self.slot_count) >= marrow_image::bounds::MAX_LOCALS {
            self.local_limit_reached = true;
            self.failed = true;
            self.diagnostics.push(SourceDiagnostic::at(
                Code::CheckResourceLimit.as_str(),
                self.file,
                request_span,
                format!(
                    "a function frame cannot allocate another local slot; the fixed limit is {}",
                    marrow_image::bounds::MAX_LOCALS
                ),
            ));
            return None;
        }

        let slot = self.slot_count;
        #[expect(
            clippy::expect_used,
            reason = "MAX_LOCALS is statically no greater than u16::MAX and the precheck excludes every over-bound count"
        )]
        let next = self
            .slot_count
            .checked_add(1)
            .expect("an admitted local-slot count fits u16");
        self.slot_count = next;
        Some(slot)
    }

    fn fail(&mut self, diagnostic: SourceDiagnostic) {
        self.diagnostics.push(diagnostic);
        self.failed = true;
    }

    fn reject_resolution(&mut self, error: ResolveError, span: SourceSpan, subject: &str) {
        self.reject_at(error, self.file, span, subject);
    }

    /// Report a resolution failure against `file`, which is the body's own file
    /// except when a generic instantiation is rejected against its template's.
    fn reject_at(
        &mut self,
        error: ResolveError,
        file: &FileIdentity,
        span: SourceSpan,
        subject: &str,
    ) {
        let refusal = match error {
            ResolveError::Refusal(refusal) => refusal,
            ResolveError::Invariant(invariant) => {
                self.record_invariant(invariant);
                return;
            }
        };
        // A use of a declaration this project refused is steered to that
        // declaration's own cause, once, rather than described as a form the
        // language does not support.
        match annotation_refusal_row(self.records, self.durable, refusal, file, span, subject) {
            Ok(AnnotationRefusal { row: Some(row), .. }) => self.fail(row),
            Ok(AnnotationRefusal { row: None, .. }) => self.failed = true,
            Err(invariant) => self.record_invariant(invariant),
        }
    }

    /// Steer one use of a refused declaration to the cause its declaration
    /// reported, once per refused key: the first use carries the row and every
    /// later one fails silently, which is what holds amplification to the number of
    /// refused declarations rather than the number of uses.
    fn steer_refusal(&mut self, summary: &DeclarationRefusalSummary, span: SourceSpan) {
        match summary.steer_once() {
            true => {
                let row = declaration_refused(self.file, span, summary);
                self.fail(row);
            }
            false => self.failed = true,
        }
    }

    /// Steer a use that named a refused type to that declaration's cause, if the
    /// name is one, reporting once per refused key.
    ///
    /// A construction site and a qualified name resolve through the kind-specific
    /// tables rather than through type-annotation resolution, so they reach their
    /// own not-in-scope report without ever consulting a `ResolveRefusal`. This is
    /// the one probe that keeps those paths from calling a refused type undeclared.
    fn steer_refused_type(&mut self, name: &str, span: SourceSpan) -> bool {
        let Binding::Refused(_, summary) = self.records.named_type(name) else {
            return false;
        };
        self.steer_refusal(summary, span);
        true
    }

    /// The same steer for a member of a resource record or one of its unkeyed
    /// groups, named by its owner. `false` means the owner never declared the
    /// member, which is the one case a "has no field" report may describe.
    ///
    /// `owner` is a resource record's name, or the `Record.group` anchor of an
    /// unkeyed group.
    fn steer_refused_member(&mut self, owner: &str, member: &str, span: SourceSpan) -> bool {
        let Binding::Refused(_, summary) = self.records.member(owner, member) else {
            return false;
        };
        self.steer_refusal(summary, span);
        true
    }

    /// The same steer for a member a projection already resolved to its refusal
    /// handle.
    fn steer_refused_member_id(&mut self, id: DeclarationRefusalId, span: SourceSpan) {
        match self.records.refused_member_steer(id, self.file, span) {
            Ok(Some(row)) => self.fail(row),
            Ok(None) => self.failed = true,
            Err(drift) => self.record_invariant(LowerInvariant::from(drift)),
        }
    }

    fn record_invariant(&mut self, invariant: LowerInvariant) {
        if self.invariant.is_none() {
            self.invariant = Some(invariant);
        }
        self.failed = true;
    }

    /// Whether lowering must stop before any later handler, interning, patching, or
    /// emission. The shared instantiation limit, the frame's first over-bound local
    /// request, and the first private generic invariant are terminal for this body.
    fn terminal_rejection(&self) -> bool {
        self.records.has_instantiation_limit()
            || self.local_limit_reached
            || self.invariant.is_some()
    }

    fn accept_resolution<T>(
        &mut self,
        result: Result<T, ResolveError>,
        span: SourceSpan,
        subject: &str,
    ) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.reject_resolution(error, span, subject);
                None
            }
        }
    }

    fn reject_unification(&mut self, error: UnifyError, span: SourceSpan, subject: &str) {
        match error {
            UnifyError::Mismatch(message) => self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                message,
            )),
            UnifyError::Invariant(invariant) => {
                self.reject_resolution(ResolveError::Invariant(invariant), span, subject);
            }
        }
    }

    /// Resolve the store root named `name` to its executable descriptor, reporting the
    /// precise diagnostic on failure: a not-yet-executable rejection when a root of that
    /// name is declared but parked (a singleton root, or a root whose resource declares a
    /// group or a nominal-typed field — its identity is complete but the kernel does not
    /// serve its shape), or a name error when no root of that name is declared at all. The
    /// returned reference borrows the durable registry (lifetime `'a`), not `self`, so it
    /// stays valid across later mutating lowering calls.
    fn resolve_root(
        &mut self,
        name: &str,
        span: SourceSpan,
    ) -> Option<&'a crate::durable::DurableRoot> {
        let durable: &'a DurableRegistry = self.durable;
        match durable.root(name) {
            RootBinding::Executable(root) => Some(root),
            RootBinding::NotYetExecutable => {
                self.fail(not_yet_executable(self.file, span, name));
                None
            }
            RootBinding::Refused(_, refusal) => {
                // A refused root is referenced from many sites; the declaration
                // already reported the cause, so the first reference is steered to it
                // and the rest fail silently. One refused store does not echo at every
                // use.
                if refusal.steer_once() {
                    // A missing ledger identity is the one refusal with a report
                    // *family* rather than a single row, so it names that family.
                    // Every other cause — a refused member, index, bound, value cycle,
                    // or admission check — reuses the one row it pushed, which is what
                    // keeps nine of the ten refusal sites from claiming an identity
                    // failure that was never reported.
                    let row = match refusal.gap() {
                        Some(_) => identity_admission_failed(self.file, span, name),
                        None => declaration_refused(self.file, span, refusal),
                    };
                    self.fail(row);
                } else {
                    self.failed = true;
                }
                None
            }
            RootBinding::Absent => {
                // A genuinely undeclared root: a plain unknown name, with the nearest
                // declared store root offered when one is a close misspelling.
                let suggestion = nearest_name(name, durable.root_names());
                self.fail(name_not_in_scope(
                    self.file,
                    span,
                    name,
                    suggestion.as_deref(),
                    NameKind::Root,
                ));
                None
            }
        }
    }

    fn lookup(&self, name: &str) -> Option<&Local> {
        self.locals.iter().rev().find(|local| local.name == name)
    }
}

#[cfg(test)]
#[path = "lower_metadata_successor_tests.rs"]
mod lower_metadata_successor_tests;

#[cfg(test)]
mod generic_cache_boundary_tests {
    use super::*;
    use crate::decl::DeclarationBudget;
    use crate::types::{GenericInvariant, Reserved, TypeInstKind, count_metadata_directory_builds};
    use marrow_image::{EnumTypeDef, RecordTypeDef};
    use marrow_syntax::{Declaration, parse_source};

    fn span() -> SourceSpan {
        SourceSpan {
            line: 1,
            column: 1,
            ..SourceSpan::default()
        }
    }

    fn name(name: &str) -> Expression {
        Expression::Name {
            segments: Box::new([NameSegment::new(name, span())]),
            span: span(),
        }
    }

    fn generic_enum_registry(draft: &mut ImageDraft) -> TypeRegistry {
        let mut diagnostics = DiagnosticCollector::new();
        TypeRegistry::build(
            draft,
            &[],
            &[],
            &[],
            &[],
            &[],
            &mut diagnostics,
            DeclarationBudget::default(),
        )
        .expect("the test registry stays within the ledger budget")
    }

    fn generic_struct_registry(draft: &mut ImageDraft) -> TypeRegistry {
        let parsed = parse_source(
            r#"struct Box<T> {
    value: T
}
"#,
        );
        assert!(!parsed.has_errors());
        let declaration = parsed
            .file
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Struct(declaration) => Some(declaration),
                _ => None,
            })
            .expect("generic struct parses");
        let mut diagnostics = DiagnosticCollector::new();
        let records = TypeRegistry::build(
            draft,
            &[],
            &[],
            &[(
                crate::analysis::FileRef::admitted(0),
                crate::test_file_identity("src/main.mw"),
                declaration,
            )],
            &[],
            &[],
            &mut diagnostics,
            DeclarationBudget::default(),
        );
        assert!(diagnostics.is_empty());
        records.expect("the test registry stays within the ledger budget")
    }

    #[test]
    fn recursive_generic_unification_builds_one_metadata_directory() {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let list = records
            .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
            .expect("List<int> mints");
        let map = records
            .instantiate_map(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                GArg::Collection(list),
            )
            .expect("Map<int,List<int>> mints");
        let parameter = || TypeExpr::Name {
            text: "T".to_string(),
            segment_spans: Vec::new(),
            span: span(),
        };
        let annotation = TypeExpr::Apply {
            head: "Map".to_string(),
            head_span: span(),
            args: vec![
                parameter(),
                TypeExpr::Apply {
                    head: "List".to_string(),
                    head_span: span(),
                    args: vec![parameter()],
                    span: span(),
                },
            ],
            span: span(),
        };
        let type_params = vec![("T".to_string(), None)];
        let mut subst = vec![None];

        let (result, builds) = count_metadata_directory_builds(|| {
            unify_type_param(
                &records,
                &type_params,
                &annotation,
                LTy::Collection {
                    idx: map,
                    optional: false,
                },
                &mut subst,
            )
        });

        assert!(matches!(result, Ok(())));
        assert_eq!(subst, vec![Some(GArg::Scalar(ScalarType::Int))]);
        assert_eq!(builds, 1);
    }

    #[test]
    fn generic_unification_prevalidates_inferred_metadata_before_named_mismatch() {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let (_, orphan) = orphan_enum_and_struct(&mut draft);
        let arg = GArg::Struct(orphan);
        let expected = GenericInvariant::TypeArgumentTargetMissing(arg);
        let draft_before = draft.encode().expect("hostile draft still encodes");
        let type_params = vec![("T".to_string(), None)];
        let sentinel = vec![Some(GArg::Scalar(ScalarType::Bool))];

        assert_eq!(records.validate_type_arguments(&[arg]), Err(expected));
        let (_, builds) = count_metadata_directory_builds(|| {
            for (name, optional) in [
                ("int", false),
                ("MissingType", false),
                ("MissingType", true),
            ] {
                let annotation = TypeExpr::Name {
                    text: name.to_string(),
                    segment_spans: Vec::new(),
                    span: span(),
                };
                let mut subst = sentinel.clone();
                let result = unify_type_param(
                    &records,
                    &type_params,
                    &annotation,
                    LTy::Struct {
                        ty: orphan,
                        optional,
                    },
                    &mut subst,
                );

                assert!(matches!(
                    result,
                    Err(UnifyError::Invariant(found)) if found == expected
                ));
                assert_eq!(subst, sentinel);
            }
        });
        assert_eq!(
            builds, 1,
            "the hostile preflight builds the shared directory once and reuses it across \
             every inferred-metadata check rather than rebuilding per argument"
        );
        assert_eq!(records.validate_type_arguments(&[arg]), Err(expected));
        let draft_after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    #[test]
    fn map_resolution_validates_hostile_key_metadata_before_refusal() {
        let annotation = TypeExpr::Apply {
            head: "Map".to_string(),
            head_span: span(),
            args: vec![
                TypeExpr::Name {
                    text: "K".to_string(),
                    segment_spans: Vec::new(),
                    span: span(),
                },
                TypeExpr::Name {
                    text: "int".to_string(),
                    segment_spans: Vec::new(),
                    span: span(),
                },
            ],
            span: span(),
        };

        for family in ["struct", "enum", "collection"] {
            let mut draft = ImageDraft::new();
            let records = generic_enum_registry(&mut draft);
            let (orphan_enum, orphan_struct) = orphan_enum_and_struct(&mut draft);
            let arg = match family {
                "struct" => GArg::Struct(orphan_struct),
                "enum" => GArg::Enum(orphan_enum),
                "collection" => GArg::Collection(0),
                _ => unreachable!("the hostile family table is closed"),
            };
            let expected = GenericInvariant::TypeArgumentTargetMissing(arg);
            let params = [TypeParamSlot {
                name: "K".to_string(),
                binding: ParamBinding::Concrete(arg),
            }];
            let draft_before = draft.encode().expect("hostile draft still encodes");
            assert_eq!(records.validate_type_arguments(&[arg]), Err(expected));

            let (result, builds) = count_metadata_directory_builds(|| {
                resolve_type(
                    &records,
                    &mut draft,
                    &DurableRegistry::empty(DeclarationBudget::default()),
                    &annotation,
                    TypeEnv { params: &params },
                    MintSite {
                        file: crate::test_main_file_identity(),
                        span: span(),
                    },
                )
            });
            assert!(matches!(
                result,
                Err(ResolveError::Invariant(found)) if found == expected
            ));
            assert_eq!(builds, 1, "{family} key uses one metadata proof");
            assert_eq!(records.validate_type_arguments(&[arg]), Err(expected));
            let draft_after = draft.encode().expect("rejected draft still encodes");
            assert_eq!(draft_after.bytes, draft_before.bytes);
            assert_eq!(draft_after.image_id, draft_before.image_id);
        }
    }

    #[test]
    fn lower_map_resolution_rejects_a_missing_nominal_before_value_mint() {
        let annotation = TypeExpr::Apply {
            head: "Map".to_string(),
            head_span: span(),
            args: vec![
                TypeExpr::Name {
                    text: "K".to_string(),
                    segment_spans: Vec::new(),
                    span: span(),
                },
                TypeExpr::Apply {
                    head: "List".to_string(),
                    head_span: span(),
                    args: vec![TypeExpr::Name {
                        text: "int".to_string(),
                        segment_spans: Vec::new(),
                        span: span(),
                    }],
                    span: span(),
                },
            ],
            span: span(),
        };
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let missing = GArg::Nominal(NominalId(0));
        let params = [TypeParamSlot {
            name: "K".to_string(),
            binding: ParamBinding::Concrete(missing),
        }];
        let expected = GenericInvariant::TypeArgumentTargetMissing(missing);
        let draft_before = draft.encode().expect("empty draft encodes");

        let (resolved, builds) = count_metadata_directory_builds(|| {
            resolve_type(
                &records,
                &mut draft,
                &DurableRegistry::empty(DeclarationBudget::default()),
                &annotation,
                TypeEnv { params: &params },
                MintSite {
                    file: crate::test_main_file_identity(),
                    span: span(),
                },
            )
        });
        assert!(matches!(
            resolved,
            Err(ResolveError::Invariant(found)) if found == expected
        ));
        assert_eq!(
            builds, 0,
            "the nominal owner rejects before List resolution"
        );
        let draft_after = draft.encode().expect("rejected draft encodes");
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
        assert_eq!(
            records
                .instantiate_list(&mut draft, GArg::Scalar(ScalarType::Int))
                .expect("the first post-refusal collection mints"),
            0,
            "the refused Map did not mint its List value"
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn lowerer<'a>(
        draft: &'a mut ImageDraft,
        records: &'a TypeRegistry,
        durable: &'a DurableRegistry,
        functions: &'a FunctionRegistry,
        generics: &'a GenericRegistry<'a>,
        consts: &'a ConstRegistry,
        diagnostics: &'a mut DiagnosticCollector,
        facts: FactSink<'a>,
    ) -> FnLowerer<'a> {
        FnLowerer::new(
            draft,
            records,
            durable,
            functions,
            generics,
            consts,
            diagnostics,
            facts,
            crate::test_main_file_identity(),
            "main",
            RetType::Unit,
            BodyKind::Function,
        )
    }

    #[test]
    fn local_slot_limit_rejection_is_atomic_and_reported_once() {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let before = draft.encode().expect("empty draft encodes");
        let durable = DurableRegistry::empty(DeclarationBudget::default());
        let functions = FunctionRegistry::empty(DeclarationBudget::default());
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::empty(DeclarationBudget::default());
        let mut diagnostics = DiagnosticCollector::new();
        let request_span = SourceSpan {
            start_byte: 40,
            end_byte: 41,
            line: 3,
            column: 15,
        };
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
            FactSink::Discarding,
        );

        for expected in 0..marrow_image::bounds::MAX_LOCALS {
            assert_eq!(
                lowerer.alloc_slot(request_span).map(usize::from),
                Some(expected)
            );
        }
        assert_eq!(
            usize::from(lowerer.slot_count),
            marrow_image::bounds::MAX_LOCALS
        );
        assert!(lowerer.alloc_slot(request_span).is_none());
        assert_eq!(
            usize::from(lowerer.slot_count),
            marrow_image::bounds::MAX_LOCALS,
            "a rejected request does not mutate the admitted count"
        );
        assert!(lowerer.alloc_slot(request_span).is_none());
        assert!(lowerer.terminal_rejection());
        assert_eq!(lowerer.diagnostics.probe_rows().len(), 1);
        assert_eq!(
            lowerer.diagnostics.probe_rows()[0].code(),
            Code::CheckResourceLimit.as_str()
        );
        assert_eq!(lowerer.diagnostics.probe_rows()[0].span(), request_span);
        assert!(matches!(
            lowerer.finish("rejected", Vec::new(), ImageType::Unit),
            Ok(BodyOutcome::Refused)
        ));

        let after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(after.bytes, before.bytes);
        assert_eq!(after.image_id, before.image_id);
        assert_eq!(diagnostics.probe_rows().len(), 1);
    }

    fn orphan_enum_and_struct(draft: &mut ImageDraft) -> (EnumId, TypeId) {
        let enum_name = draft.intern_string("OrphanEnum");
        let enum_id = draft.add_enum_type(EnumTypeDef {
            name: enum_name,
            variants: Vec::new(),
        });
        let struct_name = draft.intern_string("OrphanStruct");
        let struct_id = draft.add_record_type(RecordTypeDef {
            name: struct_name,
            fields: Vec::new(),
        });
        (enum_id, struct_id)
    }

    fn assert_typed_invariant_rejects_consumer(invariant: GenericInvariant) {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let before = draft.encode().expect("empty draft encodes");
        let durable = DurableRegistry::empty(DeclarationBudget::default());
        let functions = FunctionRegistry::empty(DeclarationBudget::default());
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::empty(DeclarationBudget::default());
        let mut diagnostics = DiagnosticCollector::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
            FactSink::Discarding,
        );

        assert!(
            lowerer
                .accept_resolution::<()>(
                    Err(ResolveError::Invariant(invariant)),
                    span(),
                    "this generic consumer",
                )
                .is_none()
        );
        assert!(lowerer.terminal_rejection());
        assert!(matches!(
            lowerer.finish("broken", Vec::new(), ImageType::Unit),
            Err(found) if found == invariant
        ));
        assert!(diagnostics.is_empty());
        let after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(after.bytes, before.bytes);
        assert_eq!(after.image_id, before.image_id);
    }

    #[test]
    fn lower_generic_reports_exact_missing_option_and_result_templates() {
        for reserved in [Reserved::Option, Reserved::Result] {
            assert_typed_invariant_rejects_consumer(GenericInvariant::ReservedTemplateMissing(
                reserved,
            ));
        }
    }

    #[test]
    fn lower_generic_reports_exact_wrong_option_and_result_template_kinds() {
        for template in [0, 1] {
            assert_typed_invariant_rejects_consumer(GenericInvariant::TemplateKindMismatch {
                template,
                expected: TypeInstKind::Enum,
                actual: TypeInstKind::Struct,
            });
        }
    }

    /// An enum-shaped local whose row is not semantically Ready is a
    /// typed internal failure, not an `enum_variants` expectation unwind.
    #[test]
    fn bare_enum_without_ready_variants_fails_without_unwinding() {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let (enum_id, _) = orphan_enum_and_struct(&mut draft);
        let draft_before = draft.encode().expect("seeded draft encodes");
        let durable = DurableRegistry::empty(DeclarationBudget::default());
        let functions = FunctionRegistry::empty(DeclarationBudget::default());
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::empty(DeclarationBudget::default());
        let mut diagnostics = DiagnosticCollector::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
            FactSink::Discarding,
        );
        lowerer.locals.push(Local {
            name: "value".to_string(),
            ty: LTy::Enum {
                ty: enum_id,
                optional: false,
            },
            mutable: false,
            slot: 0,
        });

        assert!(matches!(
            lowerer.lower_match(&name("value"), &[], span()),
            Flow::Rejected
        ));
        assert!(
            lowerer
                .lower_generic_struct_literal(0, &[], span())
                .is_none(),
            "a later template-kind invariant also rejects lowering"
        );
        let result = lowerer.finish("broken", Vec::new(), ImageType::Unit);
        let Err(invariant) = result else {
            panic!("the first generic invariant must reject the real finish path")
        };
        let draft_after = draft.encode().expect("rejected draft still encodes");

        assert_eq!(
            invariant,
            GenericInvariant::ReadyBodyMissing(TypeInstId::Enum(enum_id))
        );
        assert!(diagnostics.is_empty());
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    /// An enum template routed to the generic-struct constructor is
    /// classified by the template owner rather than unwinding at `expect`.
    #[test]
    fn enum_template_at_struct_constructor_fails_without_unwinding() {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let (_, struct_id) = orphan_enum_and_struct(&mut draft);
        let draft_before = draft.encode().expect("seeded draft encodes");
        let durable = DurableRegistry::empty(DeclarationBudget::default());
        let functions = FunctionRegistry::empty(DeclarationBudget::default());
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::empty(DeclarationBudget::default());
        let mut diagnostics = DiagnosticCollector::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
            FactSink::Discarding,
        );

        assert!(
            lowerer
                .lower_generic_struct_literal(0, &[], span())
                .is_none()
        );
        assert!(
            lowerer
                .resolve_product_field(
                    LTy::Struct {
                        ty: struct_id,
                        optional: false,
                    },
                    "value",
                    span(),
                    span(),
                )
                .is_none(),
            "a later missing-body invariant also rejects lowering"
        );
        let result = lowerer.finish("broken", Vec::new(), ImageType::Unit);
        let Err(invariant) = result else {
            panic!("the first generic invariant must reject the real finish path")
        };
        let draft_after = draft.encode().expect("rejected draft still encodes");

        assert_eq!(
            invariant,
            GenericInvariant::TemplateKindMismatch {
                template: 0,
                expected: TypeInstKind::Struct,
                actual: TypeInstKind::Enum,
            }
        );
        assert!(diagnostics.is_empty());
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    /// A bare struct id with no Ready body is a typed internal
    /// failure, not a cache-body panic.
    #[test]
    fn bare_struct_without_ready_body_fails_without_unwinding() {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let (_, type_id) = orphan_enum_and_struct(&mut draft);
        let draft_before = draft.encode().expect("seeded draft encodes");
        let durable = DurableRegistry::empty(DeclarationBudget::default());
        let functions = FunctionRegistry::empty(DeclarationBudget::default());
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::empty(DeclarationBudget::default());
        let mut diagnostics = DiagnosticCollector::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
            FactSink::Discarding,
        );

        assert!(
            lowerer
                .resolve_product_field(
                    LTy::Struct {
                        ty: type_id,
                        optional: false,
                    },
                    "value",
                    span(),
                    span(),
                )
                .is_none()
        );
        assert!(
            lowerer
                .lower_generic_struct_literal(0, &[], span())
                .is_none(),
            "a later template-kind invariant also rejects lowering"
        );
        let result = lowerer.finish("broken", Vec::new(), ImageType::Unit);
        let Err(invariant) = result else {
            panic!("the first generic invariant must reject the real finish path")
        };
        let draft_after = draft.encode().expect("rejected draft still encodes");

        assert_eq!(
            invariant,
            GenericInvariant::ReadyBodyMissing(TypeInstId::Record(type_id))
        );
        assert!(diagnostics.is_empty());
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    #[test]
    fn generic_struct_minted_as_enum_is_an_exact_invariant() {
        let mut draft = ImageDraft::new();
        let records = generic_struct_registry(&mut draft);
        let template = records
            .type_template_by_name("Box")
            .expect("Box template exists");
        let record_id = records
            .mint_type_instance(
                &mut draft,
                template,
                &[GArg::Scalar(ScalarType::Int)],
                MintSite {
                    file: crate::test_main_file_identity(),
                    span: span(),
                },
            )
            .expect("Box row mints ready");
        let TypeInstId::Record(_) = record_id else {
            panic!("Box mints a record")
        };
        let (enum_id, _) = orphan_enum_and_struct(&mut draft);
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(enum_id),
            body: TypeInstKind::Struct,
        };
        let before = draft.encode().expect("seeded draft encodes");
        let durable = DurableRegistry::empty(DeclarationBudget::default());
        let functions = FunctionRegistry::empty(DeclarationBudget::default());
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::empty(DeclarationBudget::default());
        let mut diagnostics = DiagnosticCollector::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
            FactSink::Discarding,
        );
        lowerer.reject_unification(
            UnifyError::Invariant(expected),
            span(),
            "this generic struct inference",
        );
        lowerer.locals.push(Local {
            name: "item".to_string(),
            ty: LTy::bare_scalar(ScalarType::Int),
            mutable: false,
            slot: 0,
        });
        let args = [Argument {
            name: Some(NameSegment::new("value", span())),
            value: name("item"),
        }];

        assert!(
            lowerer
                .lower_generic_struct_literal(template, &args, span())
                .is_none()
        );
        let Err(invariant) = lowerer.finish("broken", Vec::new(), ImageType::Unit) else {
            panic!("wrong minted ID kind rejects finish")
        };
        assert_eq!(invariant, expected);
        assert!(diagnostics.is_empty());
        let after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(after.bytes, before.bytes);
        assert_eq!(after.image_id, before.image_id);
    }

    #[test]
    fn generic_enum_minted_as_record_is_an_exact_invariant() {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let template = records
            .type_template_by_name("Option")
            .expect("Option template exists");
        let _enum_id = records
            .instantiate_reserved_option(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                MintSite {
                    file: crate::test_main_file_identity(),
                    span: span(),
                },
            )
            .expect("Option row mints ready");
        let (_, record_id) = orphan_enum_and_struct(&mut draft);
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Record(record_id),
            body: TypeInstKind::Enum,
        };
        let before = draft.encode().expect("seeded draft encodes");
        let durable = DurableRegistry::empty(DeclarationBudget::default());
        let functions = FunctionRegistry::empty(DeclarationBudget::default());
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::empty(DeclarationBudget::default());
        let mut diagnostics = DiagnosticCollector::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
            FactSink::Discarding,
        );
        lowerer.reject_unification(
            UnifyError::Invariant(expected),
            span(),
            "this generic enum inference",
        );
        lowerer.locals.push(Local {
            name: "item".to_string(),
            ty: LTy::bare_scalar(ScalarType::Int),
            mutable: false,
            slot: 0,
        });
        let args = [Argument {
            name: Some(NameSegment::new("value", span())),
            value: name("item"),
        }];

        assert!(
            lowerer
                .lower_generic_enum_construct(template, "some", &args, span())
                .is_none()
        );
        let Err(invariant) = lowerer.finish("broken", Vec::new(), ImageType::Unit) else {
            panic!("wrong minted ID kind rejects finish")
        };
        assert_eq!(invariant, expected);
        assert!(diagnostics.is_empty());
        let after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(after.bytes, before.bytes);
        assert_eq!(after.image_id, before.image_id);
    }

    #[test]
    fn ready_enum_id_with_struct_body_rejects_lowering_exactly() {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let enum_id = records
            .instantiate_reserved_option(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                MintSite {
                    file: crate::test_main_file_identity(),
                    span: span(),
                },
            )
            .expect("Option row mints ready");
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(enum_id),
            body: TypeInstKind::Struct,
        };
        let draft_before = draft.encode().expect("seeded draft encodes");
        let durable = DurableRegistry::empty(DeclarationBudget::default());
        let functions = FunctionRegistry::empty(DeclarationBudget::default());
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::empty(DeclarationBudget::default());
        let mut diagnostics = DiagnosticCollector::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
            FactSink::Discarding,
        );
        assert!(
            lowerer
                .accept_resolution::<()>(
                    Err(ResolveError::Invariant(expected)),
                    span(),
                    "this enum match",
                )
                .is_none()
        );
        lowerer.locals.push(Local {
            name: "value".to_string(),
            ty: LTy::Enum {
                ty: enum_id,
                optional: false,
            },
            mutable: false,
            slot: 0,
        });

        assert_eq!(
            lowerer.lower_match(&name("value"), &[], span()),
            Flow::Rejected
        );
        let Err(invariant) = lowerer.finish("broken", Vec::new(), ImageType::Unit) else {
            panic!("wrong Ready body rejects finish")
        };
        assert_eq!(invariant, expected);
        assert!(diagnostics.is_empty());
        let draft_after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    #[test]
    fn template_confirmed_generic_enum_missing_ready_variant_is_invariant() {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let template = records
            .type_template_by_name("Option")
            .expect("Option template exists");
        let enum_id = records
            .instantiate_reserved_option(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                MintSite {
                    file: crate::test_main_file_identity(),
                    span: span(),
                },
            )
            .expect("Option row mints ready");
        let expected = GenericInvariant::ReadyEnumVariantMissing {
            id: enum_id,
            template,
            variant: 1,
        };
        let draft_before = draft.encode().expect("seeded draft encodes");
        let durable = DurableRegistry::empty(DeclarationBudget::default());
        let functions = FunctionRegistry::empty(DeclarationBudget::default());
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::empty(DeclarationBudget::default());
        let mut diagnostics = DiagnosticCollector::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
            FactSink::Discarding,
        );
        assert!(
            lowerer
                .accept_resolution::<()>(
                    Err(ResolveError::Invariant(expected)),
                    span(),
                    "this generic enum construction",
                )
                .is_none()
        );
        lowerer.locals.push(Local {
            name: "item".to_string(),
            ty: LTy::bare_scalar(ScalarType::Int),
            mutable: false,
            slot: 0,
        });
        let args = [Argument {
            name: Some(NameSegment::new("value", span())),
            value: name("item"),
        }];

        assert!(
            lowerer
                .lower_generic_enum_construct(template, "some", &args, span())
                .is_none()
        );
        let Err(invariant) = lowerer.finish("broken", Vec::new(), ImageType::Unit) else {
            panic!("missing Ready variant rejects finish")
        };
        assert_eq!(invariant, expected);
        assert!(diagnostics.is_empty());
        let draft_after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    #[test]
    fn interpolation_invariant_stops_before_later_literal_emission() {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let template = records
            .type_template_by_name("Option")
            .expect("Option template exists");
        let enum_id = records
            .instantiate_reserved_option(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                MintSite {
                    file: crate::test_main_file_identity(),
                    span: span(),
                },
            )
            .expect("Option row mints ready");
        let expected = GenericInvariant::ReadyEnumVariantMissing {
            id: enum_id,
            template,
            variant: 1,
        };
        let draft_before = draft.encode().expect("seeded draft encodes");
        let durable = DurableRegistry::empty(DeclarationBudget::default());
        let functions = FunctionRegistry::empty(DeclarationBudget::default());
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::empty(DeclarationBudget::default());
        let mut diagnostics = DiagnosticCollector::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
            FactSink::Discarding,
        );
        assert!(
            lowerer
                .accept_resolution::<()>(
                    Err(ResolveError::Invariant(expected)),
                    span(),
                    "this interpolation expression",
                )
                .is_none()
        );
        lowerer.locals.push(Local {
            name: "item".to_string(),
            ty: LTy::bare_scalar(ScalarType::Int),
            mutable: false,
            slot: 0,
        });
        let parts = [
            InterpolationPart::Expr(Expression::Call {
                callee: Box::new(Expression::Name {
                    segments: Box::new([
                        NameSegment::new("Option", span()),
                        NameSegment::new("some", span()),
                    ]),
                    span: span(),
                }),
                args: vec![Argument {
                    name: Some(NameSegment::new("value", span())),
                    value: name("item"),
                }],
                multiline: false,
                span: span(),
            }),
            InterpolationPart::Text {
                text: "later-sentinel".into(),
                span: span(),
            },
        ];

        assert!(lowerer.lower_interpolation(&parts, span()).is_none());
        assert!(lowerer.code.is_empty());
        let Err(invariant) = lowerer.finish("broken", Vec::new(), ImageType::Unit) else {
            panic!("interpolation invariant rejects finish")
        };
        assert_eq!(invariant, expected);
        assert!(diagnostics.is_empty());
        let draft_after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    #[test]
    fn reserved_constructor_and_try_stop_before_effects_after_typed_reader_failure() {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let option = records
            .instantiate_reserved_option(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                MintSite {
                    file: crate::test_main_file_identity(),
                    span: span(),
                },
            )
            .expect("Option row mints ready");
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(option),
            body: TypeInstKind::Struct,
        };
        let before = draft.encode().expect("seeded draft encodes");
        let durable = DurableRegistry::empty(DeclarationBudget::default());
        let functions = FunctionRegistry::empty(DeclarationBudget::default());
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::empty(DeclarationBudget::default());
        let mut diagnostics = DiagnosticCollector::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
            FactSink::Discarding,
        );
        assert!(
            lowerer
                .accept_resolution::<()>(
                    Err(ResolveError::Invariant(expected)),
                    span(),
                    "this reserved type reader",
                )
                .is_none()
        );

        assert!(
            lowerer
                .lower_ctor_as(
                    CtorKind::None,
                    &Expression::Name {
                        segments: Box::new([NameSegment::new("none", span())]),
                        span: span(),
                    },
                    LTy::Enum {
                        ty: option,
                        optional: false,
                    },
                )
                .is_none()
        );
        assert!(lowerer.lower_try(&name("value"), span()).is_none());
        assert!(lowerer.code.is_empty());
        assert!(matches!(
            lowerer.finish("broken", Vec::new(), ImageType::Unit),
            Err(found) if found == expected
        ));
        assert!(diagnostics.is_empty());
        let after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(after.bytes, before.bytes);
        assert_eq!(after.image_id, before.image_id);
    }

    #[test]
    fn checked_result_invariant_stops_before_handler_and_patch_work() {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let expected = GenericInvariant::ReservedTemplateMissing(Reserved::Option);
        draft.intern_int(1);
        draft.intern_int(2);
        let draft_before = draft.encode().expect("seeded draft encodes");
        let durable = DurableRegistry::empty(DeclarationBudget::default());
        let functions = FunctionRegistry::empty(DeclarationBudget::default());
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::empty(DeclarationBudget::default());
        let mut diagnostics = DiagnosticCollector::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
            FactSink::Discarding,
        );
        assert!(
            lowerer
                .accept_resolution::<()>(
                    Err(ResolveError::Invariant(expected)),
                    span(),
                    "this checked result annotation",
                )
                .is_none()
        );
        let integer = |text: &str| Expression::Literal {
            kind: LiteralKind::Integer,
            text: text.into(),
            span: span(),
        };
        let operation = Expression::Binary {
            op: BinaryOp::Add,
            left: Box::new(integer("1")),
            right: Box::new(integer("2")),
            span: span(),
        };
        let annotation = TypeExpr::Apply {
            head: "Option".to_string(),
            head_span: span(),
            args: vec![TypeExpr::Name {
                text: "int".to_string(),
                segment_spans: Vec::new(),
                span: span(),
            }],
            span: span(),
        };
        let handler = Block {
            statements: Box::new([Statement::Expr {
                value: Expression::Literal {
                    kind: LiteralKind::String,
                    text: "handler-sentinel".into(),
                    span: span(),
                },
                span: span(),
            }]),
            comments: Vec::new(),
            span: span(),
        };

        assert_eq!(
            lowerer.lower_checked(
                &CheckedBind::Const {
                    name: "result".to_string(),
                    name_span: span(),
                    ty: Some(Box::new(annotation)),
                },
                &operation,
                Some(&handler),
                None,
                span(),
            ),
            Flow::Rejected
        );
        assert!(lowerer.code.is_empty());
        let Err(invariant) = lowerer.finish("broken", Vec::new(), ImageType::Unit) else {
            panic!("checked-result invariant rejects finish")
        };
        assert_eq!(invariant, expected);
        assert!(diagnostics.is_empty());
        let draft_after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    #[test]
    fn nested_else_if_terminal_invariant_never_falls_through_or_patches() {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let expected = GenericInvariant::ReservedTemplateMissing(Reserved::Result);
        let before = draft.encode().expect("empty draft encodes");
        let durable = DurableRegistry::empty(DeclarationBudget::default());
        let functions = FunctionRegistry::empty(DeclarationBudget::default());
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::empty(DeclarationBudget::default());
        let mut diagnostics = DiagnosticCollector::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
            FactSink::Discarding,
        );
        assert!(
            lowerer
                .accept_resolution::<()>(
                    Err(ResolveError::Invariant(expected)),
                    span(),
                    "this nested condition",
                )
                .is_none()
        );
        let condition = Expression::Literal {
            kind: LiteralKind::Bool,
            text: "true".into(),
            span: span(),
        };
        let empty = Block {
            statements: Box::new([]),
            comments: Vec::new(),
            span: span(),
        };
        let else_ifs = [ElseIf {
            condition: condition.clone(),
            block: empty.clone(),
        }];

        assert_eq!(
            lowerer
                .lower_if_const_bindings(&[], Some(&condition), &empty, &else_ifs, Some(&empty),),
            Flow::Rejected
        );
        assert_eq!(
            lowerer.lower_cond_chain(&[(&condition, &empty)], Some(&empty)),
            Flow::Rejected
        );
        assert!(lowerer.code.is_empty());
        assert!(matches!(
            lowerer.finish("broken", Vec::new(), ImageType::Unit),
            Err(found) if found == expected
        ));
        assert!(diagnostics.is_empty());
        let after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(after.bytes, before.bytes);
        assert_eq!(after.image_id, before.image_id);
    }

    #[test]
    fn first_invariant_stops_real_block_before_later_owner_mutation() {
        let mut draft = ImageDraft::new();
        let records = generic_enum_registry(&mut draft);
        let template = records
            .type_template_by_name("Option")
            .expect("Option template exists");
        let enum_id = records
            .instantiate_reserved_option(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                MintSite {
                    file: crate::test_main_file_identity(),
                    span: span(),
                },
            )
            .expect("Option row mints ready");
        let expected = GenericInvariant::ReadyBodyMissing(TypeInstId::Enum(enum_id));
        let draft_before = draft.encode().expect("seeded draft encodes");
        let durable = DurableRegistry::empty(DeclarationBudget::default());
        let functions = FunctionRegistry::empty(DeclarationBudget::default());
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::empty(DeclarationBudget::default());
        let mut diagnostics = DiagnosticCollector::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
            FactSink::Discarding,
        );
        assert!(
            lowerer
                .accept_resolution::<()>(
                    Err(ResolveError::Invariant(expected)),
                    span(),
                    "this enum match",
                )
                .is_none()
        );
        lowerer.locals.push(Local {
            name: "value".to_string(),
            ty: LTy::Enum {
                ty: enum_id,
                optional: false,
            },
            mutable: false,
            slot: 0,
        });
        let block = Block {
            statements: Box::new([
                Statement::Match {
                    scrutinee: name("value"),
                    arms: Vec::new(),
                    span: span(),
                },
                Statement::Const {
                    name: "later_generic".to_string(),
                    name_span: span(),
                    ty: Some(Box::new(TypeExpr::Apply {
                        head: "Option".to_string(),
                        head_span: span(),
                        args: vec![TypeExpr::Name {
                            text: "int".to_string(),
                            segment_spans: Vec::new(),
                            span: span(),
                        }],
                        span: span(),
                    })),
                    value: Expression::Absent { span: span() },
                    span: span(),
                },
                Statement::Expr {
                    value: Expression::Literal {
                        kind: LiteralKind::String,
                        text: "later-sentinel".into(),
                        span: span(),
                    },
                    span: span(),
                },
                Statement::Expr {
                    value: name("value"),
                    span: span(),
                },
            ]),
            comments: Vec::new(),
            span: span(),
        };

        assert_eq!(lowerer.lower_block(&block), Flow::Rejected);
        assert!(lowerer.code.is_empty());
        assert_eq!(lowerer.locals.len(), 1);
        assert_eq!(lowerer.locals[0].name, "value");
        assert_eq!(lowerer.slot_count, 0);
        let Err(invariant) = lowerer.finish("broken", Vec::new(), ImageType::Unit) else {
            panic!("first block invariant rejects finish")
        };
        assert_eq!(invariant, expected);
        assert!(diagnostics.is_empty());
        let draft_after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);

        assert_eq!(
            records.mint_type_instance(
                &mut draft,
                template,
                &[GArg::Scalar(ScalarType::Int)],
                MintSite {
                    file: crate::test_main_file_identity(),
                    span: span(),
                },
            ),
            Ok(TypeInstId::Enum(enum_id))
        );
        let after_probe = draft.encode().expect("cache probe leaves draft intact");
        assert_eq!(after_probe.bytes, draft_before.bytes);
        assert_eq!(after_probe.image_id, draft_before.image_id);
    }
}
