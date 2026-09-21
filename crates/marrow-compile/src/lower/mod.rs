//! Function-body lowering.
//!
//! [`FnLowerer`] type-checks the compiled subset and lowers one function body to
//! a draft instruction stream. Locals are allocated one fresh slot per `const`/
//! `var`/param/`if const` binding — slots are never reused — so every read is
//! dominated by the slot's single write and the independent verifier's
//! definite-init dataflow is satisfied. Jumps are emitted with placeholder targets
//! and patched to instruction indices once the target position is known; the
//! encoder rewrites indices to byte offsets.
//!
//! ## Panic surface
//!
//! Every source-level problem is reported by pushing a typed [`SourceDiagnostic`] and
//! returning a private lowering failure; lowering never aborts on ill-typed or
//! unsupported source. Each remaining `expect`/`unreachable!`/`panic!` asserts an
//! invariant established *before* the panicking line, so no source shape can reach one,
//! and each carries a message naming its guarantor. The classes are: a type the checker
//! already classified; a dispatch whose earlier arms removed every other case; a shape
//! the parser guarantees; and lowering's own bookkeeping (a loop context pushed at loop
//! entry, a jump placeholder emitted here). A new panic-class site falls into one of
//! these and says so, or becomes a diagnostic.

use std::collections::{BTreeMap, BTreeSet};

use crate::source::{ProjectFile, ScopedName};
use marrow_codes::Code;
use marrow_image::{
    CanonicalDeclarationPathSelector, CollTypeId, DraftTxn, EnumId, FuncId, FunctionDef,
    ImageDraft, ImageType, Instr, OccurrenceSiteHandle, OpClass, PlannedSiteRef, RootId,
    RootOccurrenceSelector, Scalar, SemanticTarget, SpanEntry, TypeId,
};

use crate::analysis::{AnalysisFactCollector, DefinitionTarget, FactSink, FileRef, StagedBodyTxn};
use marrow_syntax::{
    Argument, BinaryOp, Block, CheckedBind, ElseIf, Expression, ForBinding, FunctionDecl,
    InterpolationPart, LiteralKind, MatchArm, NameSegment, RangeExpr, SourceSpan, Statement,
    TestDecl, TraversalBound, TypeExpr, UnaryOp, decode_interpolation_text, decode_string_literal,
    duration_unit_seconds, range_expr,
};

use crate::decl::{
    Binding, DeclarationIndexDrift, DeclarationNamespace, DeclarationRefusalId,
    DeclarationRefusalSummary, MemberNamespace, declaration_refused,
};
use crate::diag::{
    DiagnosticCollector, NameFamily, SourceDiagnostic, operator_symbol, unsupported,
};
use crate::durable::{DurableRegistry, Family, ProductBinding, RootBinding};
use crate::konst::{ConstRegistry, ConstScalar};
use crate::scalar::ScalarType;
use crate::types::{
    CollSpec, EnumVariantSelection, GArg, GenericDiagnostics, GenericInvariant as LowerInvariant,
    MintSite, NominalId, OPTION_NONE, OPTION_SOME, ProductFieldProjection, RESULT_ERR, RESULT_OK,
    ReservedEnumArgs, ResolveError, ResolveRefusal, StaticNamedType, StructFieldProjection,
    SupportSet, TypeConstraint, TypeInstId, TypeMetadataSession, TypeParamIndex, TypeRegistry,
};
use marrow_project::SourceOrigin;

/// Whether control continues past a statement or block, leaves it (via `return`,
/// `break`, or `continue`), or is terminally rejected. `Rejected` is propagated by every
/// nested control owner, so later branches and structural checks cannot observe a
/// partially lowered body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Fallthrough,
    Terminates,
    Rejected,
}

/// Why a lowering operation produced no value. Ordinary source failures remain
/// recoverable at the statement boundary; the first instruction that would cross a
/// function's code-byte bound instead propagates out of the entire body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoweringFailure {
    Recoverable,
    CodeLimitReached,
}

type ConstructResult<T> = Result<T, LoweringFailure>;

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

/// A body's role in the ownership passes: which transaction laws apply to it and
/// whether it admits the owned `assert`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BodyRole {
    /// A function that runs inside its caller's region: a private function, a generic
    /// instance, or a dependency's `pub fn`.
    Helper,
    /// An export of this compilation: an invocation boundary that owns its transaction
    /// when it mutates durable state.
    Export,
    /// A `test` body: it drives exports as a terminal would, each call its own
    /// invocation boundary, and is the only body that admits `assert`.
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
    func: FuncId,
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

/// A settled body: its image index, its identity for the ownership passes, and the
/// facts those passes read — the functions it calls directly, the durable mutations
/// and calls it performs outside any `transaction` block, and its erasures.
pub(crate) struct LoweredFn {
    pub(crate) func: FuncId,
    pub(crate) file: ProjectFile,
    pub(crate) name: String,
    /// Where a body-level ownership report lands: the declaration for a function, the
    /// title for a test.
    pub(crate) span: SourceSpan,
    pub(crate) role: BodyRole,
    pub(crate) callees: Vec<u16>,
    /// Spans of durable mutations this body performs outside any `transaction` block.
    pub(crate) unwrapped_mutations: Vec<SourceSpan>,
    /// Calls this body performs outside any `transaction` block, with their spans.
    pub(crate) unwrapped_calls: Vec<(u16, SourceSpan)>,
    /// Whether this body performs a durable-place operation directly rather than only
    /// through calls. Consumed by the test-body direct-operation refusal.
    pub(crate) has_direct_durable_op: bool,
    /// Entry families directly erased by this body. Calls inherit only these
    /// erasures when checking the lifetime of a presence fact.
    pub(crate) erased_families: Vec<Family>,
    /// Present-form writes whose proofs a call may have ended.
    pub(crate) presence_obligations: Vec<PresenceObligation>,
    /// Full source spans parallel to the instructions owned by `func` in the draft.
    /// Transaction-ownership validation borrows that code after body settlement.
    pub(crate) code_spans: Vec<SourceSpan>,
}

/// The outcome of resolving a call target against module scope.
pub(crate) enum CallResolution<'a> {
    /// A resolved callable signature.
    Found(&'a FnSignature),
    /// A function with the name exists in the target module but is not `pub`, so it
    /// is not callable across the module boundary.
    NotPublic,
    /// A function of that name is declared in the target module and its signature was
    /// refused, so it is callable from nowhere and reuses the declaration's cause.
    SignatureRefused(&'a DeclarationRefusalSummary),
    /// The qualifying prefix names a module this project contains and refused, so no
    /// scope to resolve `item` in exists; the call reuses the module's cause.
    ModuleRefused(&'a DeclarationRefusalSummary),
    /// No function with that name is reachable from the calling module.
    NotFound,
}

/// Whether a body produced an image function. A refused body is the ordinary outcome
/// of a source error inside it: its diagnostics are already pushed and it consumed no
/// image index, so the artifact that needs every declared body lowered is unavailable.
/// Naming the two cases keeps that consequence at the call site rather than in an
/// untyped `None`.
pub(crate) enum BodyOutcome {
    /// Boxed: the settled body's spans dominate this outcome, and a refusal is the
    /// common arm on a failing edit.
    Lowered(Box<LoweredFn>),
    Refused,
}

type LowerResult = Result<BodyOutcome, LowerInvariant>;

/// Which lowering pass a body is in: an ordinary or instance body that emits an image
/// function and monomorphizes its generic calls, carrying the role the ownership passes
/// give it, or the once-checked template pass that lowers against abstract parameters
/// in a transaction whose additions are erased.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LowerMode {
    Concrete(BodyRole),
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
/// descended from the local (empty for the bare local); `ty` is the value type at the
/// end of that descent. Every descended field is a present composite, so the path
/// supports a read-modify-write without a presence test.
struct PlaceChain {
    slot: u16,
    mutable: bool,
    root_span: SourceSpan,
    root_name: String,
    ty: LTy,
    indices: Vec<u16>,
}

/// The refusal a handle addresses, from the namespace ledger that minted it.
///
/// The one place a `Copy` refusal handle becomes a renderable cause, so no consumer picks
/// a ledger by guesswork. The ledger checks the tag itself: a handle presented to the
/// wrong owner is drift, not a neighbouring summary.
pub(super) fn refusal_summary<'r>(
    records: &'r TypeRegistry,
    durable: &'r DurableRegistry,
    id: DeclarationRefusalId,
) -> Result<&'r DeclarationRefusalSummary, DeclarationIndexDrift> {
    match id.namespace() {
        DeclarationNamespace::NamedType => records.refusal(id),
        DeclarationNamespace::DurableRoot => durable.refusal(id),
        // None of these namespaces travels through type resolution, so no handle of
        // one reaches here.
        DeclarationNamespace::Constant
        | DeclarationNamespace::Function
        | DeclarationNamespace::Module
        | DeclarationNamespace::ResourceMember => Err(DeclarationIndexDrift),
    }
}

/// What a type-annotation position does with a resolution refusal.
///
/// `row` is the report this site owns, once per refused key: the causal steer for a
/// refused declaration, the subset-gap phrase for a genuine gap, and `None` where the
/// report is owed elsewhere. `code` carries the cause either way, so a declaration
/// refused for an unresolvable annotation retains a cause even when it owes no row.
pub(super) struct AnnotationRefusal {
    pub(super) row: Option<SourceDiagnostic>,
    pub(super) code: Code,
}

/// The refusal a type-annotation position reports.
///
/// The signature-building and return-type sites have no `FnLowerer`, so they take
/// this directly; `reject_resolution` is the same decision inside one.
pub(super) fn annotation_refusal_row(
    records: &TypeRegistry,
    durable: &DurableRegistry,
    refusal: ResolveRefusal,
    file: &ProjectFile,
    span: SourceSpan,
    subject: &str,
) -> Result<AnnotationRefusal, LowerInvariant> {
    Ok(match refusal {
        ResolveRefusal::Limit => AnnotationRefusal {
            row: None,
            code: Code::CheckInstantiationLimit,
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
                    .then(|| declaration_refused(file, span, id.namespace(), summary)),
            }
        }
    })
}

/// The registries a body's lowering resolves names through. Shared and read-only for
/// the whole region, so they travel as one `Copy` bundle.
///
/// The type registry is deliberately not among them: it is the one owner these phases
/// mutate, so it travels beside this bundle as an exclusive borrow.
#[derive(Clone, Copy)]
pub(crate) struct Resolution<'a, 'p> {
    pub(crate) durable: &'a DurableRegistry,
    pub(crate) functions: &'a FunctionRegistry,
    pub(crate) generics: &'a GenericRegistry<'p>,
    pub(crate) consts: &'a ConstRegistry,
}

/// Every owner one body's lowering runs against: the two it mutates, the registries it
/// resolves through, and the two payload sinks it writes to.
pub(crate) struct LowerCtx<'a, 'd> {
    pub(crate) draft: &'a mut DraftTxn<'d>,
    pub(crate) records: &'a mut TypeRegistry,
    pub(crate) resolution: Resolution<'a, 'a>,
    pub(crate) diagnostics: &'a mut DiagnosticCollector,
    pub(crate) facts: FactSink<'a>,
}

pub(crate) struct FnLowerer<'a, 'd> {
    draft: &'a mut DraftTxn<'d>,
    records: &'a mut TypeRegistry,
    durable: &'a DurableRegistry,
    functions: &'a FunctionRegistry,
    /// The generic function templates, for resolving a generic call target.
    generics: &'a GenericRegistry<'a>,
    consts: &'a ConstRegistry,
    diagnostics: &'a mut DiagnosticCollector,
    /// The scoped editor-fact borrow for this body. A dependency gap is written
    /// through it as it is discovered — like a diagnostic — so the gap survives even
    /// when the body it sits in fails to lower. Hover facts stage in this body's own
    /// buffer and are admitted by the caller only when the body lowers.
    facts: FactSink<'a>,
    /// The file identity every diagnostic reported against this body names.
    file: &'a ProjectFile,
    /// The dotted module the function being lowered belongs to; unqualified calls
    /// resolve within it.
    module: &'a str,
    /// The type-parameter environment: empty for a monomorphic body, the abstract
    /// parameters for the template pass, the concrete substitutions for an instance.
    type_env: Vec<TypeParamSlot>,
    /// Whether this body emits an image function and monomorphizes, or is the
    /// once-checked template pass over abstract parameters.
    mode: LowerMode,
    code: Vec<Instr>,
    /// Exact encoded bytes already retained in `code`, charged from the image
    /// instruction-width owner before each append.
    code_bytes: usize,
    /// The first instruction that would cross the per-function code-byte bound is a
    /// source-located terminal refusal, propagating out of the whole body.
    code_limit_reached: bool,
    spans: Vec<SpanEntry>,
    /// Full UTF-8 source span of each emitted instruction, parallel to `code`. The
    /// image keeps only the line/column [`SpanEntry`]; these byte-accurate spans stay
    /// compiler-local so the check-time transaction-ownership pass can point a
    /// diagnostic at the exact offending construct.
    full_spans: Vec<SourceSpan>,
    /// The image indices of every function this body calls directly, in emission
    /// order. The caller uses these to detect a recursive call cycle at check time.
    calls: Vec<u16>,
    /// Lexical `transaction`-block nesting depth at the current emission point. A
    /// durable mutation or a call emitted at depth zero is not covered by an ambient
    /// transaction owned by this body.
    txn_depth: u32,
    /// Spans of durable mutations emitted outside any `transaction` block in this body.
    unwrapped_mutations: Vec<SourceSpan>,
    /// Calls emitted outside any `transaction` block in this body, paired with their
    /// call-site span. A call to a callee that itself requires an ambient transaction
    /// is refused here when this body is an export entry.
    unwrapped_calls: Vec<(u16, SourceSpan)>,
    locals: Vec<Local>,
    /// Names of `const`/`var` bindings whose initializer failed to type-check, so no
    /// `Local` was bound. Suppressing a later reference keeps one bad initializer from
    /// spawning an `is not in scope` report at every later use.
    poisoned_bindings: BTreeSet<String>,
    /// In-scope source-local named `place` bindings, scoped like `locals`.
    places: Vec<PlaceLocal<'a>>,
    /// Lexically scoped presence proofs, including invalidated ones, newest last. A
    /// fact established in a guarded block or after an upsert does not outlive its
    /// block. The verifier rechecks each present-form operation independently.
    present_places: Vec<PresenceFact<'a>>,
    loops: Vec<LoopCtx<'a>>,
    /// The entry families this body erases directly.
    erased_families: Vec<&'a Family>,
    presence_obligations: Vec<PresenceObligation>,
    /// Monotonic slot allocator; never decreases, so slots are never reused.
    slot_count: u16,
    /// The frame's first over-bound request is a source-located terminal refusal.
    /// Keeping this state distinct from `failed` suppresses duplicate reports and
    /// makes every nested flow owner stop before it can use a missing slot.
    local_limit_reached: bool,
    ret: RetType,
    /// Where a body-level ownership report lands.
    span: SourceSpan,
    failed: bool,
    invariant: Option<LowerInvariant>,
}

mod builtins;
mod collections;
mod diagnostics;
mod durable;
mod exprs;
mod literals;
mod ltype;
mod presence;
mod registry;
mod stmts;
mod types;

pub(in crate::lower) use self::builtins::*;
pub(in crate::lower) use self::diagnostics::*;
pub(in crate::lower) use self::durable::*;
pub(in crate::lower) use self::ltype::*;
pub(in crate::lower) use self::presence::*;
pub(in crate::lower) use self::registry::*;
pub(in crate::lower) use self::types::*;

pub(crate) use self::builtins::{
    builtin_const_int, builtin_value_names, is_reserved_builtin_name, reserved_builtin_name,
};
pub(crate) use self::diagnostics::requires_presence;
pub(crate) use self::durable::{is_durable_place_op, is_mutation_instr};
pub(crate) use self::presence::PresenceObligation;
pub(crate) use self::registry::{
    DeclaredFn, FunctionRegistry, GenericRegistry, GenericTemplate, ModuleBinding, ModuleLedger,
    ModuleScope, SignatureOutcome, dotted_module_path,
};
pub(crate) use self::types::parse_int;

impl<'a, 'd> FnLowerer<'a, 'd> {
    /// Run one checked draft mint. A carrier-domain refusal is unreachable under the
    /// admitted source envelope, so it is remembered as the lowering invariant and the
    /// current path stops with `None`, never a diagnostic against the source.
    fn checked_mint<T>(
        &mut self,
        mint: impl FnOnce(&mut DraftTxn<'d>) -> Result<T, marrow_image::DraftStateError>,
    ) -> Option<T> {
        match mint(self.draft) {
            Ok(value) => Some(value),
            Err(refusal) => {
                self.invariant
                    .get_or_insert(LowerInvariant::BuilderDomain(refusal));
                None
            }
        }
    }

    /// A fresh lowerer over an empty body, for one function or test body.
    fn new(
        ctx: LowerCtx<'a, 'd>,
        file: &'a ProjectFile,
        module: &'a str,
        ret: RetType,
        mode: LowerMode,
        span: SourceSpan,
    ) -> Self {
        let LowerCtx {
            draft,
            records,
            resolution:
                Resolution {
                    durable,
                    functions,
                    generics,
                    consts,
                },
            diagnostics,
            facts,
        } = ctx;
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
            mode,
            code: Vec::new(),
            code_bytes: 0,
            code_limit_reached: false,
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
            erased_families: Vec::new(),
            presence_obligations: Vec::new(),
            loops: Vec::new(),
            slot_count: 0,
            local_limit_reached: false,
            ret,
            span,
            failed: false,
            invariant: None,
        }
    }

    /// The role the ownership passes give this body. The template pass emits nothing
    /// they read, so it reports as the helper its instances are.
    fn role(&self) -> BodyRole {
        match self.mode {
            LowerMode::Concrete(role) => role,
            LowerMode::Template => BodyRole::Helper,
        }
    }

    /// Whether this body's hover displays are still worth rendering: its facts are
    /// retained (a generic instance's duplicate its template's) and no snapshot ceiling
    /// has been crossed. A caller renders inside this guard so a discarded body never
    /// pays the O(depth) spelling render, which across a divergent monomorphization
    /// would sum to O(instances²).
    fn collects_hover(&self) -> bool {
        self.facts.renders_facts()
    }

    /// Admit one editor hover fact at `span` through the ledger: a resolved local or
    /// parameter use carries a type display and no definition; a resolved function
    /// callee carries its signature display and its definition target.
    fn record_hover(
        &mut self,
        span: SourceSpan,
        display: Box<str>,
        definition: Option<DefinitionTarget>,
    ) {
        self.facts.hover(span, display, definition);
    }

    /// The hover display of a local or parameter's value type. A bare template type
    /// parameter renders by its declared spelling from the type-parameter environment
    /// (`T`) rather than the positional `type parameter #N` form, so a hover inside a
    /// generic template body reads the source name; every other type defers to the
    /// canonical spelling unchanged.
    fn hover_type_display(&self, ty: LTy) -> String {
        if let LTy::Param { index, optional } = ty
            && let Some(slot) = self.type_env.get(index.position())
        {
            return if optional {
                format!("{}?", slot.name)
            } else {
                slot.name.clone()
            };
        }
        ty.spelling(self.records)
    }

    /// Lower `function` into its reserved draft slot under `role`. Export minting is the
    /// caller's job: it holds the dotted module name the export's
    /// [`marrow_image::ExportId`] needs.
    pub(crate) fn lower(
        ctx: LowerCtx<'a, 'd>,
        file: &'a ProjectFile,
        module: &'a str,
        function: &FunctionDecl,
        func: FuncId,
        role: BodyRole,
    ) -> LowerResult {
        Self::lower_with_env(
            ctx,
            file,
            module,
            function,
            func,
            Vec::new(),
            LowerMode::Concrete(role),
        )
    }

    /// Lower one monomorphized instance into its reserved image slot, binding the
    /// template's parameters to concrete arguments.
    pub(crate) fn lower_instance(
        ctx: LowerCtx<'a, 'd>,
        template: &'a GenericTemplate<'a>,
        args: &[GArg],
        func: FuncId,
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
            ctx,
            &template.file,
            &template.module,
            template.decl,
            func,
            type_env,
            LowerMode::Concrete(BodyRole::Helper),
        )
    }

    /// Lower one generic template against its abstract parameter constraints. Called by
    /// the staging guard, which never exposes its draft, registry, diagnostic, or fact
    /// owners to the driver.
    pub(crate) fn lower_template(
        ctx: LowerCtx<'a, 'd>,
        template: &'a GenericTemplate<'a>,
    ) -> LowerResult {
        let func = ctx.draft.reserve_function()?;
        let type_env = template
            .type_params
            .iter()
            .map(|(name, constraint)| TypeParamSlot {
                name: name.clone(),
                binding: ParamBinding::Abstract(*constraint),
            })
            .collect();
        Self::lower_with_env(
            ctx,
            &template.file,
            &template.module,
            template.decl,
            func,
            type_env,
            LowerMode::Template,
        )
    }

    /// Run the once-checked template pass over a generic function: lower its body
    /// against abstract type parameters (each admitting only its declared constraint)
    /// inside a composite savepoint over the in-progress registry and draft. The body is
    /// checked once — including rejecting `==`/`<` on an unconstrained parameter —
    /// independently of whether or how it is instantiated. Its diagnostics and editor
    /// facts survive; the emitted code and proof-appended owner suffixes are erased.
    pub(crate) fn check_template(
        draft: &mut ImageDraft,
        records: &mut TypeRegistry,
        resolution: Resolution<'_, '_>,
        facts: &AnalysisFactCollector,
        template: &GenericTemplate,
    ) -> Result<TemplateProofOutcome, LowerInvariant> {
        // Prove the body directly on the in-progress registry and draft — so it sees every
        // already-minted type at its real index — inside the composite guard that erases
        // the abstract-parameter instantiations and throwaway code the pass appends. The
        // guard restores both owners on every path (normal return, lowering invariant, or
        // unwind): registry inverse first, then the armed draft guard, exactly once.
        let scope = StagedBodyTxn::enter_proof(records, draft)?;
        // Proof and erasure are one consuming operation: no throwaway function identity
        // or staged payload can leave while its producer remains armed, so a failure
        // drops the pass's diagnostics and editor facts with it.
        let (generic, body) = scope.prove_template(resolution, facts, template)?;
        Ok(TemplateProofOutcome { generic, body })
    }

    /// The shared driver for an ordinary function, a generic instance, and the template
    /// pass, which `type_env` and `mode` distinguish: resolve the return type, bind the
    /// value parameters, lower the body, and fill the reserved image function.
    fn lower_with_env(
        ctx: LowerCtx<'a, 'd>,
        file: &'a ProjectFile,
        module: &'a str,
        function: &FunctionDecl,
        func: FuncId,
        type_env: Vec<TypeParamSlot>,
        mode: LowerMode,
    ) -> LowerResult {
        let durable = ctx.resolution.durable;
        let ret = {
            let env = TypeEnv { params: &type_env };
            match &function.return_type {
                None => RetType::Unit,
                Some(annotation) => {
                    let site = MintSite {
                        file,
                        span: annotation.span(),
                    };
                    match resolve_type(ctx.records, ctx.draft, durable, annotation, env, site) {
                        Ok(ty) => RetType::Value(ty),
                        Err(ResolveError::Refusal(refusal)) => {
                            if let Some(row) = annotation_refusal_row(
                                ctx.records,
                                durable,
                                refusal,
                                file,
                                annotation.span(),
                                "this return type",
                            )?
                            .row
                            {
                                ctx.diagnostics.push(row);
                            }
                            return Ok(BodyOutcome::Refused);
                        }
                        Err(ResolveError::Invariant(invariant)) => return Err(invariant),
                    }
                }
            }
        };

        let mut lowerer = FnLowerer::new(ctx, file, module, ret, mode, function.span);
        lowerer.type_env = type_env;

        // Params occupy the first slots, pre-initialized to their type: a bare scalar, a
        // bare nominal (int-shaped), or a bare struct record ref.
        //
        // One entry per source parameter, in source order: its lowered type, or `None`
        // for one whose type was refused. The image's parameter list is built from this
        // rather than from a positional zip against `locals`, which body lowering also
        // grows — a dropped parameter would otherwise shift the correspondence and give
        // the image a signature the source never wrote.
        let mut declared_params: Vec<Option<LTy>> = Vec::with_capacity(function.params.len());
        // A repeated type parameter or parameter name is refused at the repeat, before it
        // could take a slot the body would read in place of the name the reader wrote.
        // The rows belong to the declaration's one once-checked lowering — a concrete
        // body or the template pass — never to an instance.
        let reports_signature = mode == LowerMode::Template || lowerer.type_env.is_empty();
        let mut type_param_names = MemberNamespace::new(&function.name);
        for param in &function.type_params {
            if let Some(row) = type_param_names.claim(file, &param.name, param.name_span)
                && reports_signature
            {
                lowerer.fail(row);
            }
        }
        let mut param_names = MemberNamespace::new(&function.name);
        for param in &function.params {
            if let Some(row) = param_names.claim(file, &param.name, param.name_span) {
                if reports_signature {
                    lowerer.fail(row);
                } else {
                    lowerer.failed = true;
                }
                declared_params.push(None);
                continue;
            }
            if !param.keys.is_empty() {
                lowerer.fail(unsupported(file, function.span, "a keyed parameter"));
            }
            if is_reserved_builtin_name(&param.name) {
                lowerer.fail(reserved_builtin_name(file, function.span, &param.name));
            }
            let Some(ty) = lowerer.param_type(&param.ty) else {
                if lowerer.terminal_rejection() {
                    return lowerer.finish(func, &function.name, Vec::new(), ImageType::Unit);
                }
                // The parameter keeps its name: its type was reported at the annotation,
                // so a use in the body reuses that cause rather than calling a name the
                // reader can see written unknown, once per use.
                lowerer.poisoned_bindings.insert(param.name.clone());
                declared_params.push(None);
                continue;
            };
            let Some(slot) = lowerer.alloc_slot(param.ty.span()) else {
                return lowerer.finish(func, &function.name, Vec::new(), ImageType::Unit);
            };
            declared_params.push(Some(ty));
            lowerer.locals.push(Local {
                name: param.name.clone(),
                ty,
                mutable: false,
                slot,
            });
            // A bare nominal parameter revalidates its interval on entry: the image
            // records only the base int, so a terminal or wire caller could otherwise
            // inject an out-of-interval value into the type.
            if let Some(id) = ty.bare_nominal() {
                let info = lowerer.records.nominal(id);
                let (lo, hi) = (info.lo, info.hi);
                if lowerer
                    .push(Instr::LocalGet(slot), function.span)
                    .and_then(|()| lowerer.push(Instr::RangeGuard { lo, hi }, function.span))
                    .and_then(|()| lowerer.push(Instr::Pop, function.span))
                    .is_err()
                {
                    return lowerer.finish(func, &function.name, Vec::new(), ImageType::Unit);
                }
            }
        }

        if lowerer.terminal_rejection() {
            return lowerer.finish(func, &function.name, Vec::new(), ImageType::Unit);
        }

        let body_flow = match lowerer.lower_block(&function.body) {
            Ok(flow) => flow,
            Err(LoweringFailure::Recoverable | LoweringFailure::CodeLimitReached) => {
                return lowerer.finish(func, &function.name, Vec::new(), ImageType::Unit);
            }
        };
        match (body_flow, lowerer.ret) {
            (Flow::Terminates, _) => {}
            (Flow::Fallthrough, RetType::Unit) => {
                if lowerer.push(Instr::Return, function.body.span).is_err() {
                    return lowerer.finish(func, &function.name, Vec::new(), ImageType::Unit);
                }
            }
            (Flow::Fallthrough, RetType::Value(_)) => {
                lowerer.fail(SourceDiagnostic::at(
                    Code::CheckType,
                    file,
                    function.span,
                    "not all paths return a value".to_string(),
                ));
            }
            (Flow::Rejected, _) => {
                return lowerer.finish(func, &function.name, Vec::new(), ImageType::Unit);
            }
        }

        // A bare nominal param erases to its base int in the image; the entry guard
        // emitted above revalidates its interval. Aggregate parameters carry only their
        // erased image types; public nominal-containing aggregates are refused when
        // signatures settle.
        let Some(params) = declared_params
            .iter()
            .map(|ty| ty.as_ref().map(|ty| ty.image()))
            .collect::<Option<Vec<ImageType>>>()
        else {
            // A refused parameter has no image type, so this function has no parameter
            // list to emit — a shortened one would read as a signature the source never
            // wrote.
            return lowerer.finish(func, &function.name, Vec::new(), ImageType::Unit);
        };
        let ret_ref = match ret {
            RetType::Unit => ImageType::Unit,
            RetType::Value(ty) => ty.image(),
        };
        lowerer.finish(func, &function.name, params, ret_ref)
    }

    /// Lower a `test` body into a storeless, zero-argument, unit-returning function. The
    /// body is the only place the owned `assert` is legal; the title is interned as the
    /// function name and bound by the caller into the TEST-ENTRY table.
    pub(crate) fn lower_test(
        ctx: LowerCtx<'a, 'd>,
        file: &'a ProjectFile,
        module: &'a str,
        test: &TestDecl,
        func: FuncId,
    ) -> LowerResult {
        let mut lowerer = FnLowerer::new(
            ctx,
            file,
            module,
            RetType::Unit,
            LowerMode::Concrete(BodyRole::Test),
            test.name_span,
        );
        let name = &test.name;
        match lowerer.lower_block(&test.body) {
            Ok(Flow::Fallthrough) => {
                if lowerer.push(Instr::Return, test.body.span).is_err() {
                    return lowerer.finish(func, name, Vec::new(), ImageType::Unit);
                }
            }
            Ok(Flow::Terminates) => {}
            Ok(Flow::Rejected)
            | Err(LoweringFailure::Recoverable | LoweringFailure::CodeLimitReached) => {
                return lowerer.finish(func, name, Vec::new(), ImageType::Unit);
            }
        }
        lowerer.finish(func, name, Vec::new(), ImageType::Unit)
    }

    /// Intern the function name and source, fill the reserved draft slot, and return its
    /// identity — the shared tail of function and test lowering.
    fn finish(
        mut self,
        func_id: FuncId,
        name: &str,
        params: Vec<ImageType>,
        ret_ref: ImageType,
    ) -> LowerResult {
        if let Some(invariant) = self.invariant {
            return Err(invariant);
        }
        if self.failed || self.terminal_rejection() {
            return Ok(BodyOutcome::Refused);
        }
        let name_id = self.draft.intern_string(name)?;
        let source_id = self.draft.intern_string(&self.file.spelling())?;
        let code = std::mem::take(&mut self.code);
        let spans = std::mem::take(&mut self.spans);
        let code_spans = std::mem::take(&mut self.full_spans);
        let has_direct_durable_op = code.iter().any(is_durable_place_op);
        self.draft.fill_function(
            func_id,
            FunctionDef {
                name: name_id,
                source: source_id,
                params,
                ret: ret_ref,
                local_count: self.slot_count,
                code,
                spans,
            },
        )?;
        Ok(BodyOutcome::Lowered(Box::new(LoweredFn {
            func: func_id,
            file: self.file.clone(),
            name: name.to_string(),
            span: self.span,
            role: self.role(),
            callees: std::mem::take(&mut self.calls),
            unwrapped_mutations: std::mem::take(&mut self.unwrapped_mutations),
            unwrapped_calls: std::mem::take(&mut self.unwrapped_calls),
            erased_families: self.erased_families.drain(..).cloned().collect(),
            presence_obligations: std::mem::take(&mut self.presence_obligations),
            has_direct_durable_op,
            code_spans,
        })))
    }

    // --- emission helpers ---

    fn here(&self) -> usize {
        self.code.len()
    }

    fn push(&mut self, instr: Instr, span: SourceSpan) -> ConstructResult<()> {
        if self.code_limit_reached {
            return Err(LoweringFailure::CodeLimitReached);
        }
        let next_code_bytes = self
            .code_bytes
            .checked_add(instr.encoded_len())
            .filter(|bytes| *bytes <= marrow_image::bounds::MAX_CODE_BYTES);
        let Some(next_code_bytes) = next_code_bytes else {
            self.code_limit_reached = true;
            self.failed = true;
            self.diagnostics.push(SourceDiagnostic::at(
                Code::CheckResourceLimit,
                self.file,
                span,
                format!(
                    "this construct would make the function's compiled code exceed the fixed \
                     limit of {} bytes",
                    marrow_image::bounds::MAX_CODE_BYTES
                ),
            ));
            return Err(LoweringFailure::CodeLimitReached);
        };
        if self.txn_depth == 0 {
            match &instr {
                Instr::Call(target) => self.unwrapped_calls.push((*target, span)),
                _ if is_mutation_instr(&instr) => self.unwrapped_mutations.push(span),
                _ => {}
            }
        }
        if let Instr::Call(target) = &instr {
            self.calls.push(*target);
        }
        let index = self.code.len() as u32;
        self.code_bytes = next_code_bytes;
        self.code.push(instr);
        self.spans.push(SpanEntry {
            instr_index: index,
            line: span.line.max(1),
            column: span.column.max(1),
        });
        self.full_spans.push(span);
        Ok(())
    }

    fn push_jump(&mut self, span: SourceSpan) -> ConstructResult<usize> {
        let at = self.here();
        self.push(Instr::Jump(0), span)?;
        Ok(at)
    }

    fn push_jif(&mut self, span: SourceSpan) -> ConstructResult<usize> {
        let at = self.here();
        self.push(Instr::JumpIfFalse(0), span)?;
        Ok(at)
    }

    fn push_branch_present(&mut self, span: SourceSpan) -> ConstructResult<usize> {
        let at = self.here();
        self.push(Instr::BranchPresent(0), span)?;
        Ok(at)
    }

    fn patch(&mut self, at: usize, target: usize) {
        let instr = &mut self.code[at];
        match instr {
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
                Code::CheckResourceLimit,
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
        file: &ProjectFile,
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
        // A use of a declaration this project refused is steered to that declaration's
        // own cause, not described as a form the language does not support.
        match annotation_refusal_row(self.records, self.durable, refusal, file, span, subject) {
            Ok(AnnotationRefusal { row: Some(row), .. }) => self.fail(row),
            Ok(AnnotationRefusal { row: None, .. }) => self.failed = true,
            Err(invariant) => self.record_invariant(invariant),
        }
    }

    /// Steer one use of a refused declaration to the cause its declaration reported,
    /// once per refused key: the first use carries the row and every later one fails
    /// silently, holding amplification to the number of refused declarations rather
    /// than the number of uses.
    ///
    /// A missing ledger identity is the one refusal class whose cause is a report
    /// *family* rather than one row, so its steer names that family. Every other cause
    /// reuses the row its declaration pushed.
    fn steer_refusal(
        &mut self,
        namespace: DeclarationNamespace,
        summary: &DeclarationRefusalSummary,
        span: SourceSpan,
    ) {
        let row = self.steer_row(namespace, summary, span);
        self.settle_steer(row);
    }

    /// The row a steered refusal owes, derived under a shared borrow alone.
    ///
    /// Splitting derivation from reporting lets a steer read its summary straight out of
    /// the exclusively held registry: the summary's borrow ends with the owned row, so
    /// the reporting mutation follows it rather than overlapping it.
    fn steer_row(
        &self,
        namespace: DeclarationNamespace,
        summary: &DeclarationRefusalSummary,
        span: SourceSpan,
    ) -> Option<SourceDiagnostic> {
        if !summary.steer_once() {
            return None;
        }
        Some(match summary.gap() {
            Some(_) => identity_admission_failed(self.file, span, namespace, summary),
            None => declaration_refused(self.file, span, namespace, summary),
        })
    }

    /// Report a derived steer: the first use of a refused key carries the row, every
    /// later one fails silently.
    fn settle_steer(&mut self, row: Option<SourceDiagnostic>) {
        match row {
            Some(row) => self.fail(row),
            None => self.failed = true,
        }
    }

    /// The origin-scoped key a bare spelling written in this body addresses: a type
    /// at a construction site, qualified name or steer, or the `^root` placement,
    /// resource spelling, or branch path of a durable reference.
    ///
    /// Both namespaces are scoped to the declaring tree, so a bare spelling resolves
    /// in the tree that wrote it: a dependency's `Pair` and the root's `Pair` are two
    /// types, and neither answers the other's name. Which namespace a key reaches is
    /// decided by the registry it is handed to.
    fn scoped_name(&self, name: &str) -> ScopedName {
        ScopedName::new(self.file.origin(), name)
    }

    /// Steer a use that named a refused type to that declaration's cause, if the name is
    /// one, reporting once per refused key.
    ///
    /// A construction site and a qualified name resolve through the kind-specific tables
    /// rather than through type-annotation resolution, so this probe is what keeps those
    /// paths from calling a refused type undeclared.
    fn steer_refused_type(&mut self, name: &ScopedName, span: SourceSpan) -> bool {
        let steer = match self.records.named_type(name) {
            Ok(Binding::Refused(id, summary)) => {
                Ok(Some(self.steer_row(id.namespace(), summary, span)))
            }
            Ok(Binding::Accepted(_) | Binding::Absent) => Ok(None),
            Err(drift) => Err(LowerInvariant::from(drift)),
        };
        match steer {
            Ok(None) => false,
            Ok(Some(row)) => {
                self.settle_steer(row);
                true
            }
            Err(invariant) => {
                self.record_invariant(invariant);
                true
            }
        }
    }

    /// The same steer for a member of a resource record or one of its unkeyed groups.
    /// `owner` is the record's scoped name, or the `Record.group` anchor of an unkeyed
    /// group. `false` means the owner never declared the member, which is the one case
    /// a "has no field" report may describe.
    fn steer_refused_member(&mut self, owner: &ScopedName, member: &str, span: SourceSpan) -> bool {
        let steer = match self.records.member(owner, member) {
            Ok(Binding::Refused(id, summary)) => {
                Ok(Some(self.steer_row(id.namespace(), summary, span)))
            }
            Ok(Binding::Accepted(_) | Binding::Absent) => Ok(None),
            // The ledger cannot say whether the owner declared this member, so no
            // "has no field" report may be made from here either.
            Err(drift) => Err(LowerInvariant::from(drift)),
        };
        match steer {
            Ok(None) => false,
            Ok(Some(row)) => {
                self.settle_steer(row);
                true
            }
            Err(invariant) => {
                self.record_invariant(invariant);
                true
            }
        }
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

    /// Route a namespace ledger's coherence failure to the invariant path.
    ///
    /// A drifted lookup answers nothing about the source, so no diagnostic is pushed and
    /// no binding is invented for it.
    fn ledger_drift<T>(&mut self, drift: DeclarationIndexDrift) -> Option<T> {
        self.record_invariant(LowerInvariant::from(drift));
        None
    }

    fn record_invariant(&mut self, invariant: LowerInvariant) {
        if self.invariant.is_none() {
            self.invariant = Some(invariant);
        }
        self.failed = true;
    }

    /// Bind one durable place — a root occurrence, the canonical declaration path of the
    /// node it addresses, and the operation target over it — into a site handle.
    ///
    /// The draft published both selectors and admits exactly one target per node, so a
    /// refusal here means the compiler and the image owner disagree about the graph the
    /// compiler just built: it is an invariant, never a diagnostic.
    fn bind_site(
        &mut self,
        occurrence: &RootOccurrenceSelector,
        path: &CanonicalDeclarationPathSelector,
        target: SemanticTarget,
    ) -> Option<OccurrenceSiteHandle> {
        match self.draft.bind_occurrence_site(occurrence, path, target) {
            Ok(handle) => Some(handle),
            Err(refused) => {
                self.record_invariant(LowerInvariant::from(refused));
                None
            }
        }
    }

    /// Mint-or-return the operand the instruction being emitted names. The eager pass
    /// already requested every bounded per-node site, so a re-request of one of those
    /// returns the id it minted; a field leaf is minted here on its first reference.
    fn site_operand(&mut self, handle: &OccurrenceSiteHandle) -> Option<PlannedSiteRef> {
        match self.draft.request_site(handle) {
            Ok(operand) => Some(operand),
            Err(refused) => {
                self.record_invariant(LowerInvariant::from(refused));
                None
            }
        }
    }

    /// Bind and immediately request one durable site — the shape every emission that does
    /// not retain a handle takes.
    fn resolve_site(
        &mut self,
        occurrence: &RootOccurrenceSelector,
        path: &CanonicalDeclarationPathSelector,
        target: SemanticTarget,
    ) -> Option<PlannedSiteRef> {
        let handle = self.bind_site(occurrence, path, target)?;
        self.site_operand(&handle)
    }

    /// Whether lowering must stop before any later handler, interning, patching, or
    /// emission: the shared instantiation limit, the frame's or code tape's first
    /// over-bound request, and the first private generic invariant are all terminal.
    fn terminal_rejection(&self) -> bool {
        self.records.has_instantiation_limit()
            || self.local_limit_reached
            || self.code_limit_reached
            || self.invariant.is_some()
    }

    /// Preserve the code-byte refusal's structural carrier when a shared terminal check
    /// runs inside expression lowering; every other terminal owner is classified at the
    /// statement boundary.
    fn terminal_lowering_failure(&self) -> LoweringFailure {
        if self.code_limit_reached {
            LoweringFailure::CodeLimitReached
        } else {
            LoweringFailure::Recoverable
        }
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
                Code::CheckType,
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
    /// name is declared but parked (its identity is complete but the kernel does not
    /// serve its shape), or a name error when none is declared. The returned reference
    /// borrows the durable registry (lifetime `'a`), not `self`, so it stays valid
    /// across later mutating lowering calls.
    fn resolve_root(
        &mut self,
        name: &str,
        span: SourceSpan,
    ) -> Option<&'a crate::durable::DurableRoot> {
        let durable: &'a DurableRegistry = self.durable;
        let binding = match durable.root(&self.scoped_name(name)) {
            Ok(binding) => binding,
            Err(drift) => return self.ledger_drift(drift),
        };
        match binding {
            RootBinding::Executable(root) => Some(root),
            RootBinding::NotYetExecutable => {
                self.fail(not_yet_executable(self.file, span, name));
                None
            }
            RootBinding::Refused(id, refusal) => {
                // One refused store does not echo at every use: the first reference is
                // steered to the declaration's cause and the rest fail silently.
                self.steer_refusal(id.namespace(), refusal, span);
                None
            }
            RootBinding::Absent => {
                // A genuinely undeclared root: a plain unknown name, with the nearest
                // declared store root offered when one is a close misspelling.
                let suggestion = nearest_name(name, durable.root_names(self.file.origin()));
                self.fail(name_not_in_scope(
                    self.file,
                    span,
                    NameFamily::Root,
                    name,
                    suggestion.as_deref(),
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
mod presence_interval_tests;
