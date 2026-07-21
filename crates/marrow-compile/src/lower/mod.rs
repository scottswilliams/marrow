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
    EnumId, FuncId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry, TypeId,
};
use marrow_project::FileIdentity;
use marrow_syntax::{
    Argument, BinaryOp, Block, CheckedBind, ElseIf, Expression, ForBinding, FunctionDecl,
    InterpolationPart, LiteralKind, MatchArm, RangeExpr, SourceSpan, Statement, TraversalBound,
    TypeExpr, UnaryOp, decode_interpolation_text, decode_string_literal, duration_unit_seconds,
    range_expr,
};

use crate::diag::SourceDiagnostic;
use crate::durable::DurableRegistry;
use crate::konst::{ConstRegistry, ConstScalar};
use crate::scalar::ScalarType;
use crate::types::{
    CollSpec, EnumVariantSelection, GArg, GenericDiagnostics, GenericInvariant as LowerInvariant,
    MintSite, NominalId, OPTION_NONE, OPTION_SOME, ProductFieldProjection, RESULT_ERR, RESULT_OK,
    ReservedEnumArgs, ResolveError, ResolveRefusal, StaticNamedType, StructFieldProjection,
    SupportSet, TypeConstraint, TypeInstId, TypeMetadataSession, TypeRegistry,
};

/// Whether control continues past a statement or block, leaves it (via `return`,
/// `break`, or `continue`), or is rejected by the shared instantiation-limit owner.
/// `Rejected` is propagated by every nested control owner, so later branches and
/// structural checks cannot observe a partially lowered body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Fallthrough,
    Terminates,
    Rejected,
}

/// The only two outcomes of a finite positional walk: completed lowering with its
/// deferred `break` jumps, or terminal rejection by the shared generic owner.
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
    name: String,
    /// The dotted module the function is declared in (path-derived).
    module: String,
    index: u16,
    params: Vec<LTy>,
    ret: RetType,
    public: bool,
    /// The file the function is declared in, for the editor definition target.
    file: FileIdentity,
    /// The span of the function's declared name — the definition selection range.
    name_span: SourceSpan,
    /// The function's header-through-body span — the full definition range.
    decl_range: SourceSpan,
}

/// The editor definition target of a resolved function callee: the file it is declared
/// in, its declared-name span (the selection range), and its header-through-body range.
#[derive(Clone)]
pub(crate) struct DefinitionTarget {
    pub(crate) file: FileIdentity,
    pub(crate) name_span: SourceSpan,
    pub(crate) decl_range: SourceSpan,
}

impl FnSignature {
    /// The definition target of this function for an editor definition query.
    pub(super) fn definition_target(&self) -> DefinitionTarget {
        DefinitionTarget {
            file: self.file.clone(),
            name_span: self.name_span,
            decl_range: self.decl_range,
        }
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
    /// Editor facts from this body: `(span, hover display, optional definition target)`
    /// for each resolved local/parameter use and function callee.
    pub hover_facts: Vec<(SourceSpan, String, Option<DefinitionTarget>)>,
}

/// The outcome of resolving a call target against module scope.
pub(crate) enum CallResolution<'a> {
    /// A resolved callable signature.
    Found(&'a FnSignature),
    /// A function with the name exists in the target module but is not `pub`, so it
    /// is not callable across the module boundary.
    NotPublic,
    /// No function with that name is reachable from the calling module.
    NotFound,
}

type LowerResult = Result<Option<Lowered>, LowerInvariant>;

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

pub(crate) struct FnLowerer<'a> {
    draft: &'a mut ImageDraft,
    records: &'a TypeRegistry,
    durable: &'a DurableRegistry,
    functions: &'a FunctionRegistry,
    /// The generic function templates, for resolving a generic call target.
    generics: &'a GenericRegistry<'a>,
    consts: &'a ConstRegistry,
    diagnostics: &'a mut Vec<SourceDiagnostic>,
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
    ret: RetType,
    /// Whether this is a function or a test body; gates the owned `assert`.
    body_kind: BodyKind,
    /// Editor facts collected while lowering this body: `(span, hover display,
    /// optional definition target)`. A resolved local/parameter use carries a type
    /// display and no definition; a resolved function callee carries its signature
    /// display and its definition target. The caller keeps these for a monomorphic
    /// function or test body and discards them for a generic instance (whose spans
    /// would duplicate the template's).
    hover_facts: Vec<(SourceSpan, String, Option<DefinitionTarget>)>,
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

pub(crate) use self::builtins::{is_reserved_builtin_name, reserved_builtin_name};
pub(crate) use self::registry::{FunctionRegistry, GenericRegistry};
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
        diagnostics: &'a mut Vec<SourceDiagnostic>,
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
            file,
            module,
            type_env: Vec::new(),
            mode: LowerMode::Concrete,
            code: Vec::new(),
            spans: Vec::new(),
            calls: Vec::new(),
            txn_depth: 0,
            unwrapped_mutations: Vec::new(),
            unwrapped_calls: Vec::new(),
            locals: Vec::new(),
            places: Vec::new(),
            present_places: Vec::new(),
            loops: Vec::new(),
            slot_count: 0,
            ret,
            body_kind,
            hover_facts: Vec::new(),
            failed: false,
            invariant: None,
        }
    }

    /// Lower `function` and add it to the draft, returning its assigned [`FuncId`]
    /// and the indices of the functions it calls directly. Export minting is the
    /// caller's job: it holds the dotted module name needed to compute the export's
    /// [`marrow_image::ExportId`]. A function that fails to lower pushes its
    /// diagnostics and returns `None`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower(
        draft: &'a mut ImageDraft,
        records: &'a TypeRegistry,
        durable: &'a DurableRegistry,
        functions: &'a FunctionRegistry,
        generics: &'a GenericRegistry<'a>,
        consts: &'a ConstRegistry,
        diagnostics: &'a mut Vec<SourceDiagnostic>,
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
        diagnostics: &'a mut Vec<SourceDiagnostic>,
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
        draft: &ImageDraft,
        records: &TypeRegistry,
        durable: &DurableRegistry,
        functions: &FunctionRegistry,
        generics: &GenericRegistry,
        consts: &ConstRegistry,
        template: &GenericTemplate,
    ) -> Result<TemplateProofOutcome, LowerInvariant> {
        let file = &template.file;
        let module = &template.module;
        // Clone the registry and the in-progress draft together so the template body
        // sees every already-minted type at its real index (a concrete callee's
        // signature stays consistent), while abstract-parameter instantiations and
        // the emitted code land only in the discarded clones.
        let check_records = records.clone_for_generic_check()?;
        let mut throwaway = draft.clone();
        let mut diagnostics = Vec::new();
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
        FnLowerer::lower_with_env(
            &mut throwaway,
            &check_records,
            durable,
            functions,
            generics,
            consts,
            &mut diagnostics,
            file,
            module,
            template.decl,
            type_env,
            LowerMode::Template,
        )?;
        let generic = check_records.take_generic_diagnostics();
        Ok(TemplateProofOutcome {
            diagnostics,
            generic,
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
        diagnostics: &'a mut Vec<SourceDiagnostic>,
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
                        Err(ResolveError::Refusal(ResolveRefusal::Unsupported)) => {
                            diagnostics.push(unsupported(
                                file,
                                annotation.span(),
                                "this return type",
                            ));
                            return Ok(None);
                        }
                        Err(ResolveError::Refusal(ResolveRefusal::Limit)) => return Ok(None),
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
            file,
            module,
            ret,
            BodyKind::Function,
        );
        lowerer.type_env = type_env;
        lowerer.mode = mode;

        // Params occupy the first slots, pre-initialized to their type: a bare
        // scalar, a bare nominal (int-shaped), or a bare struct record ref.
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
                continue;
            };
            let slot = lowerer.alloc_slot();
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

        let params: Vec<ImageType> = function
            .params
            .iter()
            .zip(&lowerer.locals)
            // A nominal param erases to its base int in the image; in-language
            // callers passed the type, and the entry guard emitted above
            // revalidates the interval against out-of-language callers. A struct
            // param carries its image record ref (`ImageType::Record`).
            .map(|(_, local)| local.ty.image())
            .collect();
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
        diagnostics: &'a mut Vec<SourceDiagnostic>,
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
    /// body that failed to lower returns `None`.
    fn finish(mut self, name: &str, params: Vec<ImageType>, ret_ref: ImageType) -> LowerResult {
        if let Some(invariant) = self.invariant {
            return Err(invariant);
        }
        if self.failed || self.terminal_rejection() {
            return Ok(None);
        }
        let name_id = self.draft.intern_string(name);
        let source_id = self.draft.intern_string(self.file.as_str());
        let code = std::mem::take(&mut self.code);
        let spans = std::mem::take(&mut self.spans);
        let has_direct_durable_op = code.iter().any(is_durable_place_op);
        let owns_transaction = code.iter().any(|instr| matches!(instr, Instr::TxnBegin));
        let func_id = self.draft.add_function(FunctionDef {
            name: name_id,
            source: source_id,
            params,
            ret: ret_ref,
            local_count: self.slot_count,
            code,
            spans,
        });
        Ok(Some(Lowered {
            func: func_id,
            callees: std::mem::take(&mut self.calls),
            unwrapped_mutations: std::mem::take(&mut self.unwrapped_mutations),
            unwrapped_calls: std::mem::take(&mut self.unwrapped_calls),
            has_direct_durable_op,
            owns_transaction,
            hover_facts: std::mem::take(&mut self.hover_facts),
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

    fn alloc_slot(&mut self) -> u16 {
        let slot = self.slot_count;
        self.slot_count += 1;
        slot
    }

    fn fail(&mut self, diagnostic: SourceDiagnostic) {
        self.diagnostics.push(diagnostic);
        self.failed = true;
    }

    fn reject_resolution(&mut self, error: ResolveError, span: SourceSpan, subject: &str) {
        match error {
            ResolveError::Refusal(ResolveRefusal::Limit) => self.failed = true,
            ResolveError::Refusal(ResolveRefusal::Unsupported) => {
                self.fail(unsupported(self.file, span, subject));
            }
            ResolveError::Invariant(invariant) => {
                if self.invariant.is_none() {
                    self.invariant = Some(invariant);
                }
                self.failed = true;
            }
        }
    }

    /// Whether lowering must stop before any later handler, interning, patching, or
    /// emission. Both the shared instantiation limit and the first private generic
    /// invariant are terminal for the current body.
    fn terminal_rejection(&self) -> bool {
        self.records.has_instantiation_limit() || self.invariant.is_some()
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
        if let Some(root) = durable.root_by_name(name) {
            return Some(root);
        }
        let diagnostic = if durable.not_yet_executable_root_named(name).is_some() {
            not_yet_executable(self.file, span, name)
        } else {
            name_error(self.file, span, name)
        };
        self.fail(diagnostic);
        None
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
            segments: vec![name.to_string()],
            segment_spans: vec![span()],
            span: span(),
        }
    }

    fn generic_enum_registry(draft: &mut ImageDraft) -> TypeRegistry {
        let mut diagnostics = Vec::new();
        TypeRegistry::build(draft, &[], &[], &[], &[], &[], &mut diagnostics)
    }

    fn generic_struct_registry(draft: &mut ImageDraft) -> TypeRegistry {
        let parsed = parse_source(
            r#"struct Box<T> {
    value: T
}
"#,
        );
        assert!(parsed.diagnostics.is_empty());
        let declaration = parsed
            .file
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Struct(declaration) => Some(declaration),
                _ => None,
            })
            .expect("generic struct parses");
        let mut diagnostics = Vec::new();
        let records = TypeRegistry::build(
            draft,
            &[],
            &[],
            &[(crate::test_file_identity("src/main.mw"), declaration)],
            &[],
            &[],
            &mut diagnostics,
        );
        assert!(diagnostics.is_empty());
        records
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
            span: span(),
        };
        let annotation = TypeExpr::Apply {
            head: "Map".to_string(),
            args: vec![
                parameter(),
                TypeExpr::Apply {
                    head: "List".to_string(),
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
        for (name, optional) in [
            ("int", false),
            ("MissingType", false),
            ("MissingType", true),
        ] {
            let annotation = TypeExpr::Name {
                text: name.to_string(),
                span: span(),
            };
            let mut subst = sentinel.clone();
            let (result, builds) = count_metadata_directory_builds(|| {
                unify_type_param(
                    &records,
                    &type_params,
                    &annotation,
                    LTy::Struct {
                        ty: orphan,
                        optional,
                    },
                    &mut subst,
                )
            });

            assert!(matches!(
                result,
                Err(UnifyError::Invariant(found)) if found == expected
            ));
            assert_eq!(builds, 1, "one session owns the hostile preflight");
            assert_eq!(subst, sentinel);
        }
        assert_eq!(records.validate_type_arguments(&[arg]), Err(expected));
        let draft_after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(draft_after.bytes, draft_before.bytes);
        assert_eq!(draft_after.image_id, draft_before.image_id);
    }

    #[test]
    fn map_resolution_validates_hostile_key_metadata_before_refusal() {
        let annotation = TypeExpr::Apply {
            head: "Map".to_string(),
            args: vec![
                TypeExpr::Name {
                    text: "K".to_string(),
                    span: span(),
                },
                TypeExpr::Name {
                    text: "int".to_string(),
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
                    &DurableRegistry::default(),
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
            args: vec![
                TypeExpr::Name {
                    text: "K".to_string(),
                    span: span(),
                },
                TypeExpr::Apply {
                    head: "List".to_string(),
                    args: vec![TypeExpr::Name {
                        text: "int".to_string(),
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
                &DurableRegistry::default(),
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
        diagnostics: &'a mut Vec<SourceDiagnostic>,
    ) -> FnLowerer<'a> {
        FnLowerer::new(
            draft,
            records,
            durable,
            functions,
            generics,
            consts,
            diagnostics,
            crate::test_main_file_identity(),
            "main",
            RetType::Unit,
            BodyKind::Function,
        )
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
        let durable = DurableRegistry::default();
        let functions = FunctionRegistry::default();
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::default();
        let mut diagnostics = Vec::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
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
        let durable = DurableRegistry::default();
        let functions = FunctionRegistry::default();
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::default();
        let mut diagnostics = Vec::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
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
        let durable = DurableRegistry::default();
        let functions = FunctionRegistry::default();
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::default();
        let mut diagnostics = Vec::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
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
        let durable = DurableRegistry::default();
        let functions = FunctionRegistry::default();
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::default();
        let mut diagnostics = Vec::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
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
        let durable = DurableRegistry::default();
        let functions = FunctionRegistry::default();
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::default();
        let mut diagnostics = Vec::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
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
            name: Some("value".to_string()),
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
        let durable = DurableRegistry::default();
        let functions = FunctionRegistry::default();
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::default();
        let mut diagnostics = Vec::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
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
            name: Some("value".to_string()),
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
        let durable = DurableRegistry::default();
        let functions = FunctionRegistry::default();
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::default();
        let mut diagnostics = Vec::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
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
        let durable = DurableRegistry::default();
        let functions = FunctionRegistry::default();
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::default();
        let mut diagnostics = Vec::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
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
            name: Some("value".to_string()),
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
        let durable = DurableRegistry::default();
        let functions = FunctionRegistry::default();
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::default();
        let mut diagnostics = Vec::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
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
                    segments: vec!["Option".to_string(), "some".to_string()],
                    segment_spans: vec![span(), span()],
                    span: span(),
                }),
                args: vec![Argument {
                    name: Some("value".to_string()),
                    value: name("item"),
                }],
                multiline: false,
                span: span(),
            }),
            InterpolationPart::Text {
                text: "later-sentinel".to_string(),
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
        let durable = DurableRegistry::default();
        let functions = FunctionRegistry::default();
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::default();
        let mut diagnostics = Vec::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
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
                        segments: vec!["none".to_string()],
                        segment_spans: vec![span()],
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
        let durable = DurableRegistry::default();
        let functions = FunctionRegistry::default();
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::default();
        let mut diagnostics = Vec::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
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
            text: text.to_string(),
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
            args: vec![TypeExpr::Name {
                text: "int".to_string(),
                span: span(),
            }],
            span: span(),
        };
        let handler = Block {
            statements: vec![Statement::Expr {
                value: Expression::Literal {
                    kind: LiteralKind::String,
                    text: "handler-sentinel".to_string(),
                    span: span(),
                },
                span: span(),
            }],
            comments: Vec::new(),
            span: span(),
        };

        assert_eq!(
            lowerer.lower_checked(
                &CheckedBind::Const {
                    name: "result".to_string(),
                    ty: Some(annotation),
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
        let durable = DurableRegistry::default();
        let functions = FunctionRegistry::default();
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::default();
        let mut diagnostics = Vec::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
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
            text: "true".to_string(),
            span: span(),
        };
        let empty = Block {
            statements: Vec::new(),
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
        let durable = DurableRegistry::default();
        let functions = FunctionRegistry::default();
        let generics = GenericRegistry::default();
        let consts = ConstRegistry::default();
        let mut diagnostics = Vec::new();
        let mut lowerer = lowerer(
            &mut draft,
            &records,
            &durable,
            &functions,
            &generics,
            &consts,
            &mut diagnostics,
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
            statements: vec![
                Statement::Match {
                    scrutinee: name("value"),
                    arms: Vec::new(),
                    span: span(),
                },
                Statement::Const {
                    name: "later_generic".to_string(),
                    ty: Some(TypeExpr::Apply {
                        head: "Option".to_string(),
                        args: vec![TypeExpr::Name {
                            text: "int".to_string(),
                            span: span(),
                        }],
                        span: span(),
                    }),
                    value: Expression::Absent { span: span() },
                    span: span(),
                },
                Statement::Expr {
                    value: Expression::Literal {
                        kind: LiteralKind::String,
                        text: "later-sentinel".to_string(),
                        span: span(),
                    },
                    span: span(),
                },
                Statement::Expr {
                    value: name("value"),
                    span: span(),
                },
            ],
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
