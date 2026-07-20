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

/// A lowered value type: a scalar, a nominal int type, or the project record,
/// each bare or optional. A nominal is int-shaped in the image; its distinct
/// check-time identity lives here and in the [`TypeRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LTy {
    Scalar {
        scalar: ScalarType,
        optional: bool,
    },
    Nominal {
        id: NominalId,
        optional: bool,
    },
    Record {
        ty: TypeId,
        optional: bool,
    },
    /// A dense `struct` value. Like [`LTy::Record`] it is image-`Record`-shaped and
    /// runtime-`Value::Record`-shaped (the one product representation owner), but
    /// it is a distinct value type: constructible and returnable, every field
    /// present. The `TypeId` names its image record def.
    Struct {
        ty: TypeId,
        optional: bool,
    },
    /// A closed enum value, image-`Enum`- and runtime-`Value::Enum`-shaped. Like
    /// the other nominal products it is a distinct value type; the `EnumId` names
    /// its image ENUMS-table entry.
    Enum {
        ty: EnumId,
        optional: bool,
    },
    /// A finite collection value (`List<T>` / `Map<K, V>`), image-`Collection`- and
    /// runtime-`Value::List`/`Value::Map`-shaped. `idx` names its image COLLTYPES
    /// entry; the source element/key/value types live in the registry's collection
    /// table.
    Collection {
        idx: u16,
        optional: bool,
    },
    /// An abstract generic type parameter, present only while the once-checked
    /// template pass lowers a generic body against a throwaway draft. `index` is the
    /// parameter's declaration position; its constraint is read from the lowerer's
    /// type environment. A monomorphized instantiation never carries a `Param`.
    Param {
        index: u16,
        optional: bool,
    },
    /// An entry identity `Id(^root)`, image-`Identity`- and runtime-`Value::Id`-shaped.
    /// `root` is the store root's ROOTS-table index (0 — a program has one root). A
    /// distinct value type: a by-value runtime/lookup value, not a durable field or key.
    Identity {
        root: u16,
        optional: bool,
    },
}

impl LTy {
    fn bare_scalar(scalar: ScalarType) -> Self {
        LTy::Scalar {
            scalar,
            optional: false,
        }
    }

    fn is_optional(self) -> bool {
        match self {
            LTy::Scalar { optional, .. }
            | LTy::Nominal { optional, .. }
            | LTy::Record { optional, .. }
            | LTy::Struct { optional, .. }
            | LTy::Enum { optional, .. }
            | LTy::Collection { optional, .. }
            | LTy::Param { optional, .. }
            | LTy::Identity { optional, .. } => optional,
        }
    }

    fn to_optional(self) -> Self {
        match self {
            LTy::Scalar { scalar, .. } => LTy::Scalar {
                scalar,
                optional: true,
            },
            LTy::Nominal { id, .. } => LTy::Nominal { id, optional: true },
            LTy::Record { ty, .. } => LTy::Record { ty, optional: true },
            LTy::Struct { ty, .. } => LTy::Struct { ty, optional: true },
            LTy::Enum { ty, .. } => LTy::Enum { ty, optional: true },
            LTy::Collection { idx, .. } => LTy::Collection {
                idx,
                optional: true,
            },
            LTy::Param { index, .. } => LTy::Param {
                index,
                optional: true,
            },
            LTy::Identity { root, .. } => LTy::Identity {
                root,
                optional: true,
            },
        }
    }

    fn to_bare(self) -> Self {
        match self {
            LTy::Scalar { scalar, .. } => LTy::bare_scalar(scalar),
            LTy::Nominal { id, .. } => LTy::Nominal {
                id,
                optional: false,
            },
            LTy::Record { ty, .. } => LTy::Record {
                ty,
                optional: false,
            },
            LTy::Struct { ty, .. } => LTy::Struct {
                ty,
                optional: false,
            },
            LTy::Enum { ty, .. } => LTy::Enum {
                ty,
                optional: false,
            },
            LTy::Collection { idx, .. } => LTy::Collection {
                idx,
                optional: false,
            },
            LTy::Param { index, .. } => LTy::Param {
                index,
                optional: false,
            },
            LTy::Identity { root, .. } => LTy::Identity {
                root,
                optional: false,
            },
        }
    }

    /// The abstract type-parameter index, if this is a bare one.
    fn bare_param(self) -> Option<u16> {
        match self {
            LTy::Param {
                index,
                optional: false,
            } => Some(index),
            _ => None,
        }
    }

    fn bare_scalar_type(self) -> Option<ScalarType> {
        match self {
            LTy::Scalar {
                scalar,
                optional: false,
            } => Some(scalar),
            _ => None,
        }
    }

    fn spelling(self, records: &TypeRegistry) -> String {
        let (base, optional) = match self {
            LTy::Scalar { scalar, optional } => (scalar.spelling().to_string(), optional),
            LTy::Nominal { id, optional } => (records.nominal(id).name.clone(), optional),
            LTy::Record { optional, .. } => ("record".to_string(), optional),
            LTy::Struct { ty, optional } => (
                records
                    .inst_spelling(TypeInstId::Record(ty))
                    .or_else(|| records.struct_by_type(ty).map(|info| info.name.clone()))
                    .unwrap_or_else(|| "struct".to_string()),
                optional,
            ),
            LTy::Enum { ty, optional } => {
                let base = records
                    .inst_spelling(TypeInstId::Enum(ty))
                    .or_else(|| records.enum_by_id(ty).map(|info| info.name.clone()))
                    .unwrap_or_else(|| "enum".to_string());
                (base, optional)
            }
            LTy::Collection { idx, optional } => (records.collection_spelling(idx), optional),
            LTy::Param { index, optional } => (format!("type parameter #{index}"), optional),
            // A program declares one store root, so the identity spelling needs no root
            // discriminator to stay unambiguous in a diagnostic.
            LTy::Identity { optional, .. } => ("Id(^root)".to_string(), optional),
        };
        if optional { format!("{base}?") } else { base }
    }

    fn spelling_in(
        self,
        records: &TypeRegistry,
        metadata: &mut TypeMetadataSession<'_>,
    ) -> Result<String, LowerInvariant> {
        let (base, optional) = match self {
            LTy::Scalar { scalar, optional } => (scalar.spelling().to_string(), optional),
            LTy::Nominal { id, optional } => (records.nominal(id).name.clone(), optional),
            LTy::Record { optional, .. } => ("record".to_string(), optional),
            LTy::Struct { ty, optional } => (metadata.garg_spelling(GArg::Struct(ty))?, optional),
            LTy::Enum { ty, optional } => (metadata.garg_spelling(GArg::Enum(ty))?, optional),
            LTy::Collection { idx, optional } => {
                (metadata.garg_spelling(GArg::Collection(idx))?, optional)
            }
            LTy::Param { index, optional } => (format!("type parameter #{index}"), optional),
            LTy::Identity { optional, .. } => ("Id(^root)".to_string(), optional),
        };
        Ok(if optional { format!("{base}?") } else { base })
    }

    /// The bare nominal identity, if this is one.
    fn bare_nominal(self) -> Option<NominalId> {
        match self {
            LTy::Nominal {
                id,
                optional: false,
            } => Some(id),
            _ => None,
        }
    }

    /// The bare enum identity, if this is one.
    fn bare_enum(self) -> Option<EnumId> {
        match self {
            LTy::Enum {
                ty,
                optional: false,
            } => Some(ty),
            _ => None,
        }
    }

    /// The bare entry-identity root, if this is one.
    fn bare_identity(self) -> Option<u16> {
        match self {
            LTy::Identity {
                root,
                optional: false,
            } => Some(root),
            _ => None,
        }
    }

    /// This type as a built-in generic argument (a bare value type), or `None` for
    /// an optional or the durable resource record, which are not value arguments.
    fn as_garg(self) -> Option<GArg> {
        match self {
            LTy::Scalar {
                scalar,
                optional: false,
            } => Some(GArg::Scalar(scalar)),
            LTy::Nominal {
                id,
                optional: false,
            } => Some(GArg::Nominal(id)),
            LTy::Struct {
                ty,
                optional: false,
            } => Some(GArg::Struct(ty)),
            LTy::Enum {
                ty,
                optional: false,
            } => Some(GArg::Enum(ty)),
            LTy::Collection {
                idx,
                optional: false,
            } => Some(GArg::Collection(idx)),
            LTy::Param {
                index,
                optional: false,
            } => Some(GArg::Param(index)),
            _ => None,
        }
    }

    fn image(self) -> ImageType {
        match self {
            LTy::Scalar {
                scalar,
                optional: false,
            } => ImageType::scalar(scalar.image()),
            LTy::Scalar {
                scalar,
                optional: true,
            } => ImageType::opt_scalar(scalar.image()),
            // A nominal is int-shaped in the image; its interval is enforced by
            // the emitted range guards, not by the recorded type.
            LTy::Nominal {
                optional: false, ..
            } => ImageType::scalar(Scalar::Int),
            LTy::Nominal { optional: true, .. } => ImageType::opt_scalar(Scalar::Int),
            LTy::Record { ty, optional } | LTy::Struct { ty, optional } => ImageType::Record {
                idx: ty.index(),
                optional,
            },
            LTy::Enum { ty, optional } => ImageType::Enum {
                idx: ty.index(),
                optional,
            },
            LTy::Collection { idx, optional } => ImageType::Collection { idx, optional },
            // Only reached in the discarded template-check draft; the sentinel keeps
            // that throwaway image well-formed and is never encoded.
            LTy::Param {
                optional: false, ..
            } => ImageType::scalar(Scalar::Int),
            LTy::Param { optional: true, .. } => ImageType::opt_scalar(Scalar::Int),
            LTy::Identity { root, optional } => ImageType::Identity { root, optional },
        }
    }
}

/// The bare lowered type a built-in generic argument denotes (the inverse of
/// [`LTy::as_garg`] over the value cases).
fn garg_to_lty(arg: GArg) -> LTy {
    match arg {
        GArg::Scalar(scalar) => LTy::bare_scalar(scalar),
        GArg::Nominal(id) => LTy::Nominal {
            id,
            optional: false,
        },
        GArg::Struct(ty) => LTy::Struct {
            ty,
            optional: false,
        },
        // A group value materializes as a nested record; its leaves resolve through
        // the group owner (`group_by_type`), reached from the record field path.
        GArg::Group(ty) => LTy::Record {
            ty,
            optional: false,
        },
        GArg::Enum(ty) => LTy::Enum {
            ty,
            optional: false,
        },
        GArg::Collection(idx) => LTy::Collection {
            idx,
            optional: false,
        },
        GArg::Param(index) => LTy::Param {
            index,
            optional: false,
        },
    }
}

/// One constructor member plan entry: a field or group leaf's name, value type, and
/// required flag, collected before emission so evaluation follows declaration order.
type MemberPlan = (String, GArg, bool);

/// One group slot's constructor plan: the group's name, its materialized-value
/// record type, whether it has a required leaf (so an omitted argument cannot be
/// auto-completed), and the plan of its leaves.
type GroupPlan = (String, TypeId, bool, Vec<MemberPlan>);

/// The source spelling of a built-in generic argument, recursing through nested
/// `Option`/`Result` arguments.
fn garg_spelling(arg: GArg, records: &TypeRegistry) -> String {
    garg_to_lty(arg).spelling(records)
}

/// A built-in `Option`/`Result` constructor form in expression position. The
/// constructor names are reserved, so any `none`, `some(_)`, `ok(_)`, or `err(_)`
/// is this built-in rather than a name or call the surrounding scope resolves.
#[derive(Debug, Clone, Copy)]
enum CtorKind {
    None,
    Some,
    Ok,
    Err,
}

impl CtorKind {
    fn name(self) -> &'static str {
        match self {
            CtorKind::None => "none",
            CtorKind::Some => "some",
            CtorKind::Ok => "ok",
            CtorKind::Err => "err",
        }
    }
}

/// A value-level built-in the compiler intercepts before user resolution: the
/// `Option`/`Result` constructors (`none`/`some`/`ok`/`err`), the presence test
/// (`exists`), the divergence marker (`unreachable`), and the pure text floor
/// (`isEmpty`/`contains`/`trim`/`split`/`lines`/`join`). None of these spellings is
/// a keyword, so the parser admits them as identifiers; the reservation is enforced
/// here instead.
///
/// This enum is the single owner of that name set. Call interception dispatches
/// on `from_name` (see `lower_unqualified_call`), and declaration rejection
/// consults the same classifier through [`is_reserved_builtin_name`], so a name
/// that is intercepted at a use site can never be silently shadowed by a
/// colliding value declaration. Adding a built-in is a new variant, which the
/// exhaustive dispatch match forces every consumer to account for.
#[derive(Debug, Clone, Copy)]
enum Builtin {
    None,
    Some,
    Ok,
    Err,
    Exists,
    Unreachable,
    Todo,
    IsEmpty,
    Contains,
    Trim,
    /// The collection-returning text floor: `split(text, sep): List[string]`,
    /// `lines(text): List[string]`, `join(List[string], sep): string`. Like the rest
    /// of the floor these are reserved, so a colliding value declaration is rejected;
    /// they mint the `List[string]` COLLTYPES instantiation their result or argument
    /// names.
    Split,
    Lines,
    Join,
    /// The named temporal arithmetic floor: `addDays(date, int): date` and
    /// `daysBetween(date, date): int`. Named rather than operators so a date
    /// offset never reads as an ambiguous `date + int`; they are reserved, so a
    /// colliding value declaration is rejected. `marrow-temporal` owns the checked
    /// operations, which fault `run.temporal_overflow` past the supported range.
    DateAddDays,
    DateDaysBetween,
    /// The empty-collection constructors `List()`/`Map()`, type-directed by the
    /// expected type. They are reserved (blocking a colliding value declaration)
    /// because a bare `List`/`Map` at a use site is always the built-in constructor.
    /// The procedural collection operations (`append`/`insert`/`get`/`length`) are
    /// deliberately *not* reserved: they are common verbs, so a same-module function
    /// of that name wins and the collection op is a fallback (see
    /// [`FnLowerer::lower_collection_fallback`]).
    List,
    Map,
    /// The entry-identity constructor `Id(^root, keys…)`: a nominal value constructor
    /// wrapping the explicit key tuple as an `Id(^root)`. Reserved so a colliding value
    /// declaration is rejected; the leading `^root` argument is a saved-root reference,
    /// not an ordinary value, so it is dispatched to its own lowering.
    Id,
}

impl Builtin {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "none" => Builtin::None,
            "some" => Builtin::Some,
            "ok" => Builtin::Ok,
            "err" => Builtin::Err,
            "exists" => Builtin::Exists,
            "unreachable" => Builtin::Unreachable,
            "todo" => Builtin::Todo,
            "isEmpty" => Builtin::IsEmpty,
            "contains" => Builtin::Contains,
            "trim" => Builtin::Trim,
            "split" => Builtin::Split,
            "lines" => Builtin::Lines,
            "join" => Builtin::Join,
            "addDays" => Builtin::DateAddDays,
            "daysBetween" => Builtin::DateDaysBetween,
            "List" => Builtin::List,
            "Map" => Builtin::Map,
            "Id" => Builtin::Id,
            _ => return None,
        })
    }
}

/// Whether `name` is a reserved value-level built-in that a `fn`, `const`,
/// parameter, or local binding may not redeclare. A colliding value declaration
/// would be admitted and then silently shadowed at every use site the compiler
/// intercepts (`some(v)`, bare `none`, `trim(s)`, ...), surfacing later as a
/// confusing type error; rejecting the declaration keeps the reserved name and
/// its interception the single fact.
///
/// Struct fields and enum variants are excluded: both are reached only through
/// member syntax (`r.none`, `Color::err`), never a bare or unqualified-call use,
/// so they cannot collide with an intercepted built-in.
pub(crate) fn is_reserved_builtin_name(name: &str) -> bool {
    Builtin::from_name(name).is_some()
}

/// The diagnostic for a value declaration whose name is a reserved built-in.
pub(crate) fn reserved_builtin_name(file: &str, span: SourceSpan, name: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckNameConflict.as_str(),
        file,
        span,
        format!("`{name}` is a built-in and cannot be redeclared"),
    )
}

/// Classify an expression as a collection constructor call on the reserved type name
/// `List`/`Map`, returning the head and its positional arguments. An empty argument
/// list is the empty constructor; a non-empty one is variadic list construction (the
/// map literal is deferred, rejected by the ctor lowering).
fn collection_ctor_call(expr: &Expression) -> Option<(&'static str, &[Argument])> {
    let Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    match &**callee {
        Expression::Name { segments, .. } => match segments.as_slice() {
            [n] if n == "List" => Some(("List", args)),
            [n] if n == "Map" => Some(("Map", args)),
            _ => None,
        },
        _ => None,
    }
}

/// The diagnostic for a built-in called with the wrong argument shape.
fn builtin_arity(file: &str, span: SourceSpan, name: &str, arity: usize) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("`{name}` takes {arity} positional argument(s)"),
    )
}

/// Classify an expression as a built-in constructor form: bare `none`, or a call
/// `some(..)`/`ok(..)`/`err(..)`. Returns `None` for anything else.
fn constructor_kind(expr: &Expression) -> Option<CtorKind> {
    match expr {
        Expression::Name { segments, .. } if matches!(segments.as_slice(), [n] if n == "none") => {
            Some(CtorKind::None)
        }
        Expression::Call { callee, .. } => match &**callee {
            Expression::Name { segments, .. } => match segments.as_slice() {
                [n] if n == "some" => Some(CtorKind::Some),
                [n] if n == "ok" => Some(CtorKind::Ok),
                [n] if n == "err" => Some(CtorKind::Err),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// Split a dotted constructor base into its single-segment head name (with span) and the
/// branch-name chain before the final call segment. `Book` yields `("Book", span, [])`;
/// `Book.notes` yields `("Book", span, ["notes"])`; deeper chains accumulate. `None` for a
/// head that is not a single-segment name (a `::`-qualified or otherwise non-head base).
fn split_dotted_head(expr: &Expression) -> Option<(&str, SourceSpan, Vec<&str>)> {
    match expr {
        Expression::Name { segments, span, .. } if segments.len() == 1 => {
            Some((segments[0].as_str(), *span, Vec::new()))
        }
        Expression::Field { base, name, .. } => {
            let (head, span, mut names) = split_dotted_head(base)?;
            names.push(name.as_str());
            Some((head, span, names))
        }
        _ => None,
    }
}

/// The source-shaped display of a branch constructor head, `Resource.b1.….bn`, for a
/// diagnostic.
fn branch_ctor_display(resource: &str, path: &[&str]) -> String {
    std::iter::once(resource)
        .chain(path.iter().copied())
        .collect::<Vec<_>>()
        .join(".")
}

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
}

pub(crate) struct TemplateProofOutcome {
    pub(crate) diagnostics: Vec<SourceDiagnostic>,
    pub(crate) generic: GenericDiagnostics,
}

type LowerResult = Result<Option<Lowered>, LowerInvariant>;

impl FunctionRegistry {
    /// Resolve every function's signature in declaration order. The i-th function
    /// takes image index `i`, matching the order [`FnLowerer::lower`] adds them.
    /// `functions` pairs each declaration with its dotted module name.
    pub(crate) fn build(
        records: &TypeRegistry,
        draft: &mut ImageDraft,
        durable: &DurableRegistry,
        functions: &[(String, String, &FunctionDecl)],
        modules: BTreeSet<String>,
        imports: BTreeMap<String, Vec<(String, String)>>,
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
            });
            index += 1;
        }
        Ok(accepted.then_some(Self {
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

    /// Resolve an unqualified call from within `module`: a function of that name in
    /// the same module.
    fn same_module(&self, module: &str, name: &str) -> Option<&FnSignature> {
        self.sigs
            .iter()
            .find(|sig| sig.name == name && sig.module == module)
    }

    /// Resolve a `::`-qualified call `prefix::item` from within `current`. A single
    /// prefix segment binds through a `use` first, then a root-level module of the
    /// same name; a multi-segment prefix names a fully-qualified module path. The
    /// target must be `pub`, except a module qualifying its own function.
    fn resolve_qualified(
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
    fn resolved_module(&self, current: &str, prefix: &[String]) -> Option<String> {
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
}

/// One generic function template: the source declaration plus its type-parameter
/// names and constraints, held for lazy monomorphization. A template has no image
/// index; each concrete application is a distinct image function.
pub(crate) struct GenericTemplate<'p> {
    file: String,
    module: String,
    public: bool,
    decl: &'p FunctionDecl,
    type_params: Vec<(String, Option<TypeConstraint>)>,
}

/// The project's generic function templates and the module scope a generic call
/// resolves against — the same visibility rules the [`FunctionRegistry`] applies to
/// monomorphic functions, but keyed to templates rather than image indices.
#[derive(Default)]
pub(crate) struct GenericRegistry<'p> {
    templates: Vec<GenericTemplate<'p>>,
}

impl<'p> GenericRegistry<'p> {
    /// Collect every generic function (one carrying type parameters) as a template,
    /// paired with its source file and dotted module name.
    pub(crate) fn build(functions: &[(String, String, &'p FunctionDecl)]) -> Self {
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
    fn same_module(&self, module: &str, name: &str) -> Option<usize> {
        self.templates
            .iter()
            .position(|template| template.decl.name == name && template.module == module)
    }

    /// The template named `item` in `module`, with its `pub` flag, for a qualified
    /// generic call. The caller checks visibility against the calling module.
    fn in_module(&self, module: &str, item: &str) -> Option<(usize, bool)> {
        self.templates
            .iter()
            .position(|template| template.decl.name == item && template.module == module)
            .map(|index| (index, self.templates[index].public))
    }
}

impl<'p> GenericTemplate<'p> {
    pub(crate) fn source_file(&self) -> &str {
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
    file: &'a str,
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
    failed: bool,
    invariant: Option<LowerInvariant>,
}

mod stmts;

mod exprs;

mod durable;
pub(in crate::lower) use self::durable::*;

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
        file: &'a str,
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
        file: &'a str,
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
        file: &'a str,
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
        file: &'a str,
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
        let source_id = self.draft.intern_string(self.file);
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

    // --- type resolution ---

    fn resolve(&mut self, annotation: &TypeExpr) -> Result<LTy, ResolveError> {
        let env = TypeEnv {
            params: &self.type_env,
        };
        let site = MintSite {
            file: self.file,
            span: annotation.span(),
        };
        resolve_type(
            self.records,
            self.draft,
            self.durable,
            annotation,
            env,
            site,
        )
    }

    fn param_type(&mut self, ty: &TypeExpr) -> Option<LTy> {
        let env = TypeEnv {
            params: &self.type_env,
        };
        let site = MintSite {
            file: self.file,
            span: ty.span(),
        };
        match param_type(self.records, self.draft, self.durable, ty, env, site) {
            Ok(param) => Some(param),
            Err(refusal) => {
                self.reject_resolution(refusal, ty.span(), "this parameter type");
                None
            }
        }
    }
}

/// A generic type parameter's binding in the body being lowered.
#[derive(Clone, Copy)]
enum ParamBinding {
    /// The once-checked template pass: an opaque type admitting only its declared
    /// constraint's operators.
    Abstract(Option<TypeConstraint>),
    /// A monomorphized instantiation: the concrete value type the parameter denotes.
    Concrete(GArg),
}

/// One declared type parameter in the body being lowered: its source name and how
/// a use of that name resolves.
struct TypeParamSlot {
    name: String,
    binding: ParamBinding,
}

/// The type-parameter environment threaded through type resolution. An empty
/// environment is an ordinary monomorphic body; a non-empty one resolves a use of
/// a type-parameter name to an abstract [`LTy::Param`] (template pass) or the bound
/// concrete type (instantiation), before scalar/named-type classification.
#[derive(Clone, Copy)]
struct TypeEnv<'a> {
    params: &'a [TypeParamSlot],
}

impl TypeEnv<'_> {
    const EMPTY: TypeEnv<'static> = TypeEnv { params: &[] };

    /// The declaration index and binding of the type parameter named `name`.
    fn lookup(&self, name: &str) -> Option<(u16, ParamBinding)> {
        self.params
            .iter()
            .position(|slot| slot.name == name)
            .map(|index| (index as u16, self.params[index].binding))
    }

    /// The constraint on the type parameter at `index`, in the abstract pass.
    fn constraint_at(&self, index: u16) -> Option<TypeConstraint> {
        match self.params.get(index as usize).map(|slot| slot.binding) {
            Some(ParamBinding::Abstract(constraint)) => constraint,
            _ => None,
        }
    }
}

/// Resolve a parameter annotation to its lowered type: a bare scalar, a bare
/// nominal, a bare `struct`, or a bare resource-record value. Optionals and
/// unresolved names are outside the parameter subset. A resource value crosses the
/// boundary by value like any other record, sharing the image `Record` shape. One
/// owner for signature building and body lowering, so the two can never disagree on
/// a parameter's type.
fn param_type(
    records: &TypeRegistry,
    draft: &mut ImageDraft,
    durable: &DurableRegistry,
    ty: &TypeExpr,
    env: TypeEnv,
    site: MintSite<'_>,
) -> Result<LTy, ResolveError> {
    match resolve_type(records, draft, durable, ty, env, site) {
        Ok(
            param @ (LTy::Scalar {
                optional: false, ..
            }
            | LTy::Nominal {
                optional: false, ..
            }
            | LTy::Record {
                optional: false, ..
            }
            | LTy::Struct {
                optional: false, ..
            }
            | LTy::Enum {
                optional: false, ..
            }
            // A finite collection is a by-value value type, admitted as a parameter
            // (its element/key/value types may themselves be type parameters).
            | LTy::Collection {
                optional: false, ..
            }
            // A generic parameter is admitted as a value parameter; the collection
            // element/value positions admit it through `resolve_generic`.
            | LTy::Param {
                optional: false, ..
            }
            // An entry identity is a by-value value type, admitted as a parameter.
            | LTy::Identity {
                optional: false, ..
            }),
        ) => Ok(param),
        Ok(_) | Err(ResolveError::Refusal(ResolveRefusal::Unsupported)) => {
            Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
        }
        Err(ResolveError::Refusal(ResolveRefusal::Limit)) => {
            Err(ResolveError::Refusal(ResolveRefusal::Limit))
        }
        Err(ResolveError::Invariant(invariant)) => Err(ResolveError::Invariant(invariant)),
    }
}

/// Resolve a type annotation into a lowered type, or `None` for an unsupported
/// spelling. Aliases expand first, so classification reads only scalar spellings
/// and declared type names; the no-nested-optional rule applies to the expanded
/// form, so an alias cannot smuggle a doubled optional.
fn resolve_type(
    records: &TypeRegistry,
    draft: &mut ImageDraft,
    durable: &DurableRegistry,
    annotation: &TypeExpr,
    env: TypeEnv,
    site: MintSite<'_>,
) -> Result<LTy, ResolveError> {
    resolve_expanded(
        records,
        draft,
        durable,
        &records.expand(annotation),
        env,
        site,
    )
}

fn resolve_expanded(
    records: &TypeRegistry,
    draft: &mut ImageDraft,
    durable: &DurableRegistry,
    annotation: &TypeExpr,
    env: TypeEnv,
    site: MintSite<'_>,
) -> Result<LTy, ResolveError> {
    match annotation {
        TypeExpr::Name { text, .. } => {
            // A type-parameter name resolves before scalar/named-type classification,
            // so a parameter cannot be shadowed by a same-named scalar spelling.
            if let Some((index, binding)) = env.lookup(text) {
                return Ok(match binding {
                    ParamBinding::Abstract(_) => LTy::Param {
                        index,
                        optional: false,
                    },
                    ParamBinding::Concrete(arg) => garg_to_lty(arg),
                });
            }
            if let Some(scalar) = ScalarType::from_spelling(text) {
                Ok(LTy::bare_scalar(scalar))
            } else if let Some((id, _)) = records.nominal_by_name(text) {
                Ok(LTy::Nominal {
                    id,
                    optional: false,
                })
            } else {
                match records.static_named_type_projection(text)? {
                    Some(StaticNamedType::Struct(ty)) => Ok(LTy::Struct {
                        ty,
                        optional: false,
                    }),
                    Some(StaticNamedType::Enum(ty)) => Ok(LTy::Enum {
                        ty,
                        optional: false,
                    }),
                    Some(StaticNamedType::Record(ty)) => Ok(LTy::Record {
                        ty,
                        optional: false,
                    }),
                    None => Err(ResolveError::Refusal(ResolveRefusal::Unsupported)),
                }
            }
        }
        TypeExpr::Optional { inner, .. } => {
            let inner = resolve_expanded(records, draft, durable, inner, env, site)?;
            if inner.is_optional() {
                Err(ResolveError::Refusal(ResolveRefusal::Unsupported))
            } else {
                Ok(inner.to_optional())
            }
        }
        TypeExpr::Apply { head, args, .. } => {
            resolve_generic(records, draft, durable, head, args, env, site)
        }
        // `Id(^root)`: the entry-identity value type of the named store root, carrying
        // that root's declaration-ordered RootId. An identity over a root that is not
        // declared, or over a not-yet-executable root, is an unsupported type (`None`),
        // reported by the caller like any other unresolved annotation.
        TypeExpr::Identity(identity) => {
            let root = durable
                .root_by_name(&identity.root)
                .ok_or(ResolveError::Refusal(ResolveRefusal::Unsupported))?;
            Ok(LTy::Identity {
                root: root.root_id,
                optional: false,
            })
        }
    }
}

/// Resolve a generic type application to a bare instantiation, monomorphizing it
/// into the draft on first use. `List`/`Map` are the compiler collections; every
/// other head is a value-type template (the reserved `Option`/`Result` or a user
/// `struct`/`enum`) resolved through the one instantiation owner. A wrong arity, an
/// argument that is not a value type, or a constraint violation yields `None`, so
/// the caller reports it as an unsupported type. An argument may itself be an
/// abstract type parameter in the once-checked template pass; its constraint then
/// stands in for the concrete one during revalidation.
fn resolve_generic(
    records: &TypeRegistry,
    draft: &mut ImageDraft,
    durable: &DurableRegistry,
    head: &str,
    args: &[TypeExpr],
    env: TypeEnv,
    site: MintSite<'_>,
) -> Result<LTy, ResolveError> {
    match head {
        "List" => {
            let [elem] = args else {
                return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
            };
            let elem = resolve_expanded(records, draft, durable, elem, env, site)?
                .as_garg()
                .ok_or(ResolveError::Refusal(ResolveRefusal::Unsupported))?;
            Ok(LTy::Collection {
                idx: records.instantiate_list(draft, elem)?,
                optional: false,
            })
        }
        "Map" => {
            let [key, value] = args else {
                return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
            };
            let key = resolve_expanded(records, draft, durable, key, env, site)?
                .as_garg()
                .ok_or(ResolveError::Refusal(ResolveRefusal::Unsupported))?;
            records.check_map_key_admissibility(key)?;
            let value = resolve_expanded(records, draft, durable, value, env, site)?
                .as_garg()
                .ok_or(ResolveError::Refusal(ResolveRefusal::Unsupported))?;
            Ok(LTy::Collection {
                idx: records.instantiate_map(draft, key, value)?,
                optional: false,
            })
        }
        _ => {
            let template = records.application_template(head)?;
            let params = records.template_type_params(template);
            if args.len() != params.len() {
                return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
            }
            let mut resolved = Vec::with_capacity(args.len());
            for arg in args {
                resolved.push(
                    resolve_expanded(records, draft, durable, arg, env, site)?
                        .as_garg()
                        .ok_or(ResolveError::Refusal(ResolveRefusal::Unsupported))?,
                );
            }
            // Per-application constraint revalidation: a concrete argument must
            // support the constraint; an abstract parameter satisfies it when its own
            // declared constraint does.
            for ((_, constraint), arg) in
                records.template_type_params(template).iter().zip(&resolved)
            {
                if let Some(constraint) = constraint {
                    let satisfied = match arg {
                        GArg::Param(index) => {
                            env.constraint_at(*index)
                                .is_some_and(|outer| match constraint {
                                    TypeConstraint::Equality => outer.admits_equality(),
                                    TypeConstraint::Order => outer.admits_order(),
                                })
                        }
                        other => other.satisfies(*constraint),
                    };
                    if !satisfied {
                        // A malformed registry remains an invariant even when this
                        // application also violates a source constraint. The normal
                        // successful mint path owns the same preflight and must not
                        // rebuild it here.
                        records.validate_type_arguments(&resolved)?;
                        return Err(ResolveError::Refusal(ResolveRefusal::Unsupported));
                    }
                }
            }
            match records.mint_type_instance(draft, template, &resolved, site)? {
                TypeInstId::Record(ty) => Ok(LTy::Struct {
                    ty,
                    optional: false,
                }),
                TypeInstId::Enum(id) => Ok(LTy::Enum {
                    ty: id,
                    optional: false,
                }),
            }
        }
    }
}

/// Structurally unify a generic parameter's declared type against an argument's
/// inferred type, binding each type parameter to the concrete value type filling
/// its position. `annotation` is already alias-expanded. Inference is exact: a bare
/// parameter position requires a bare argument (no implicit bare-to-optional
/// widening), and a concrete named position requires an exactly matching argument. A
/// conflicting binding or a shape mismatch is an error the caller reports.
enum UnifyError {
    Mismatch(String),
    Invariant(LowerInvariant),
}

impl From<LowerInvariant> for UnifyError {
    fn from(invariant: LowerInvariant) -> Self {
        Self::Invariant(invariant)
    }
}

fn unify_type_param(
    records: &TypeRegistry,
    type_params: &[(String, Option<TypeConstraint>)],
    annotation: &TypeExpr,
    got: LTy,
    subst: &mut [Option<GArg>],
) -> Result<(), UnifyError> {
    records.with_metadata_session(|metadata| {
        if let Some(arg) = got.to_bare().as_garg() {
            metadata.validate_type_arguments(&[arg])?;
        }
        unify_type_param_with(records, metadata, type_params, annotation, got, subst)
    })
}

fn unify_type_param_with(
    records: &TypeRegistry,
    metadata: &mut TypeMetadataSession<'_>,
    type_params: &[(String, Option<TypeConstraint>)],
    annotation: &TypeExpr,
    got: LTy,
    subst: &mut [Option<GArg>],
) -> Result<(), UnifyError> {
    match annotation {
        TypeExpr::Name { text, .. } => {
            if let Some(index) = type_params.iter().position(|(name, _)| name == text) {
                if got.is_optional() {
                    return Err(UnifyError::Mismatch(format!(
                        "type parameter `{text}` matches a bare value, but the argument is `{}`",
                        got.spelling_in(records, metadata)?
                    )));
                }
                let arg = got.as_garg().ok_or_else(|| {
                    UnifyError::Mismatch(format!(
                        "`{}` is not a value type that can instantiate `{text}`",
                        got.spelling(records)
                    ))
                })?;
                match subst[index] {
                    None => subst[index] = Some(arg),
                    Some(previous) if previous == arg => {}
                    Some(previous) => {
                        let previous = garg_to_lty(previous).spelling_in(records, metadata)?;
                        let current = garg_to_lty(arg).spelling_in(records, metadata)?;
                        return Err(UnifyError::Mismatch(format!(
                            "type parameter `{text}` is inferred as both `{}` and `{}`",
                            previous, current
                        )));
                    }
                }
                Ok(())
            } else {
                match named_type(records, metadata, text)? {
                    Some(expected) if expected == got => Ok(()),
                    Some(expected) => Err(UnifyError::Mismatch(format!(
                        "expected `{}`, found `{}`",
                        expected.spelling_in(records, metadata)?,
                        got.spelling_in(records, metadata)?
                    ))),
                    None => Err(UnifyError::Mismatch(format!(
                        "unknown type `{text}` in a generic parameter"
                    ))),
                }
            }
        }
        TypeExpr::Optional { inner, .. } => {
            if !got.is_optional() {
                return Err(UnifyError::Mismatch(format!(
                    "expected an optional argument, found `{}`",
                    got.spelling_in(records, metadata)?
                )));
            }
            unify_type_param_with(records, metadata, type_params, inner, got.to_bare(), subst)
        }
        TypeExpr::Apply { head, args, .. } => {
            unify_apply_with(records, metadata, type_params, head, args, got, subst)
        }
        _ => Err(UnifyError::Mismatch(
            "this parameter type is not supported for generic inference".to_string(),
        )),
    }
}

/// Unify a built-in generic parameter application (`List`/`Map`/`Option`/`Result`)
/// against an argument, recursing into the argument's element/key/value/payload
/// types.
fn unify_apply_with(
    records: &TypeRegistry,
    metadata: &mut TypeMetadataSession<'_>,
    type_params: &[(String, Option<TypeConstraint>)],
    head: &str,
    args: &[TypeExpr],
    got: LTy,
    subst: &mut [Option<GArg>],
) -> Result<(), UnifyError> {
    match head {
        "List" => {
            let [elem] = args else {
                return Err(UnifyError::Mismatch(
                    "`List` takes one type argument".to_string(),
                ));
            };
            let LTy::Collection {
                idx,
                optional: false,
            } = got
            else {
                return Err(UnifyError::Mismatch(format!(
                    "expected a List, found `{}`",
                    got.spelling_in(records, metadata)?
                )));
            };
            match metadata.collection_spec(idx)? {
                CollSpec::List { elem: got_elem } => unify_type_param_with(
                    records,
                    metadata,
                    type_params,
                    elem,
                    garg_to_lty(got_elem),
                    subst,
                ),
                CollSpec::Map { .. } => Err(UnifyError::Mismatch(format!(
                    "expected a List, found `{}`",
                    got.spelling_in(records, metadata)?
                ))),
            }
        }
        "Map" => {
            let [key, value] = args else {
                return Err(UnifyError::Mismatch(
                    "`Map` takes two type arguments".to_string(),
                ));
            };
            let LTy::Collection {
                idx,
                optional: false,
            } = got
            else {
                return Err(UnifyError::Mismatch(format!(
                    "expected a Map, found `{}`",
                    got.spelling_in(records, metadata)?
                )));
            };
            match metadata.collection_spec(idx)? {
                CollSpec::Map {
                    key: got_key,
                    value: got_value,
                } => {
                    unify_type_param_with(
                        records,
                        metadata,
                        type_params,
                        key,
                        garg_to_lty(got_key),
                        subst,
                    )?;
                    unify_type_param_with(
                        records,
                        metadata,
                        type_params,
                        value,
                        garg_to_lty(got_value),
                        subst,
                    )
                }
                CollSpec::List { .. } => Err(UnifyError::Mismatch(format!(
                    "expected a Map, found `{}`",
                    got.spelling_in(records, metadata)?
                ))),
            }
        }
        // Every other generic head is a value-type template (the reserved
        // `Option`/`Result` or a user `struct`/`enum`): the argument must be an
        // instantiation of the same template, and each type argument unifies
        // positionally against its parameter.
        _ => {
            let template = records.type_template_by_name(head).ok_or_else(|| {
                UnifyError::Mismatch(format!(
                    "`{head}` is not a generic type usable in a parameter"
                ))
            })?;
            if args.len() != records.template_type_params(template).len() {
                return Err(UnifyError::Mismatch(format!(
                    "`{head}` takes {} type argument(s)",
                    records.template_type_params(template).len()
                )));
            }
            let inst_id = match got {
                LTy::Struct {
                    ty,
                    optional: false,
                } => TypeInstId::Record(ty),
                LTy::Enum {
                    ty,
                    optional: false,
                } => TypeInstId::Enum(ty),
                _ => {
                    return Err(UnifyError::Mismatch(format!(
                        "expected a {head}, found `{}`",
                        got.spelling_in(records, metadata)?
                    )));
                }
            };
            let Some((got_template, got_args)) = metadata.instantiation_of(inst_id)? else {
                return Err(UnifyError::Mismatch(format!(
                    "expected a {head}, found `{}`",
                    got.spelling_in(records, metadata)?
                )));
            };
            if got_template != template {
                return Err(UnifyError::Mismatch(format!(
                    "expected a {head}, found `{}`",
                    got.spelling_in(records, metadata)?
                )));
            }
            for (arg, got_arg) in args.iter().zip(&got_args) {
                unify_type_param_with(
                    records,
                    metadata,
                    type_params,
                    arg,
                    garg_to_lty(*got_arg),
                    subst,
                )?;
            }
            Ok(())
        }
    }
}

/// Resolve a concrete named type (a scalar spelling or a declared nominal/struct/
/// enum/record) to its bare lowered type without minting into any draft, for
/// exact-match generic inference.
fn named_type(
    records: &TypeRegistry,
    metadata: &mut TypeMetadataSession<'_>,
    text: &str,
) -> Result<Option<LTy>, LowerInvariant> {
    if let Some(scalar) = ScalarType::from_spelling(text) {
        Ok(Some(LTy::bare_scalar(scalar)))
    } else if let Some((id, _)) = records.nominal_by_name(text) {
        Ok(Some(LTy::Nominal {
            id,
            optional: false,
        }))
    } else {
        Ok(match metadata.static_named_type(text)? {
            Some(StaticNamedType::Struct(ty)) => Some(LTy::Struct {
                ty,
                optional: false,
            }),
            Some(StaticNamedType::Enum(ty)) => Some(LTy::Enum {
                ty,
                optional: false,
            }),
            Some(StaticNamedType::Record(ty)) => Some(LTy::Record {
                ty,
                optional: false,
            }),
            None => None,
        })
    }
}

/// The instruction an int ordering comparison lowers to, shared by the bare-int
/// operator table and the same-nominal comparison path (one owner). Equality
/// stays with [`eq_instr`].
fn int_comparison(op: BinaryOp) -> Option<Instr> {
    Some(match op {
        BinaryOp::Less => Instr::IntLt,
        BinaryOp::LessEqual => Instr::IntLe,
        BinaryOp::Greater => Instr::IntGt,
        BinaryOp::GreaterEqual => Instr::IntGe,
        _ => return None,
    })
}

/// Whether `op` is one of the four order comparisons, the guard the temporal
/// operator arms share before selecting the per-type instruction.
fn temporal_comparison(op: BinaryOp) -> Option<()> {
    matches!(
        op,
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual
    )
    .then_some(())
}

fn date_comparison(op: BinaryOp) -> Option<Instr> {
    Some(match op {
        BinaryOp::Less => Instr::DateLt,
        BinaryOp::LessEqual => Instr::DateLe,
        BinaryOp::Greater => Instr::DateGt,
        BinaryOp::GreaterEqual => Instr::DateGe,
        _ => return None,
    })
}

fn instant_comparison(op: BinaryOp) -> Option<Instr> {
    Some(match op {
        BinaryOp::Less => Instr::InstantLt,
        BinaryOp::LessEqual => Instr::InstantLe,
        BinaryOp::Greater => Instr::InstantGt,
        BinaryOp::GreaterEqual => Instr::InstantGe,
        _ => return None,
    })
}

fn duration_comparison(op: BinaryOp) -> Option<Instr> {
    Some(match op {
        BinaryOp::Less => Instr::DurationLt,
        BinaryOp::LessEqual => Instr::DurationLe,
        BinaryOp::Greater => Instr::DurationGt,
        BinaryOp::GreaterEqual => Instr::DurationGe,
        _ => return None,
    })
}

fn eq_instr(scalar: ScalarType) -> Instr {
    match scalar {
        ScalarType::Int => Instr::EqInt,
        ScalarType::Bool => Instr::EqBool,
        ScalarType::Text => Instr::EqText,
        ScalarType::Bytes => Instr::EqBytes,
        ScalarType::Date => Instr::EqDate,
        ScalarType::Instant => Instr::EqInstant,
        ScalarType::Duration => Instr::EqDuration,
    }
}

fn operator_symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Remainder => "%",
        BinaryOp::Less => "<",
        BinaryOp::LessEqual => "<=",
        BinaryOp::Greater => ">",
        BinaryOp::GreaterEqual => ">=",
        BinaryOp::Equal => "==",
        BinaryOp::NotEqual => "!=",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
        _ => "operator",
    }
}

pub(crate) fn parse_int(text: &str) -> Option<i64> {
    text.replace('_', "").parse().ok()
}

/// Whether `ty` is a value that renders to canonical text — a bare scalar, enum, or
/// entry identity. A record, collection, or optional is not renderable; those are not
/// interpolation holes and cannot ride `string(...)`.
fn is_interpolable(ty: LTy) -> bool {
    matches!(
        ty,
        LTy::Scalar {
            optional: false,
            ..
        } | LTy::Enum {
            optional: false,
            ..
        } | LTy::Identity {
            optional: false,
            ..
        }
    )
}

/// Whether `expr` is an integer literal, possibly negated, whose value is provably
/// nonzero. A checked `/`/`%` with such a divisor cannot fault with a zero divisor, so
/// the `on zero_divisor` arm is dead. A non-literal divisor is assumed possibly zero.
fn divisor_nonzero_literal(expr: &Expression) -> bool {
    let literal = match expr {
        Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } => Some(text),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => match operand.as_ref() {
            Expression::Literal {
                kind: LiteralKind::Integer,
                text,
                ..
            } => Some(text),
            _ => None,
        },
        _ => None,
    };
    literal
        .and_then(|text| parse_int(text))
        .is_some_and(|value| value != 0)
}

/// Fold a duration word literal `COUNT UNIT` to signed nanoseconds: the count times
/// the unit's whole seconds times a second in nanoseconds. Returns `None` when the
/// shape is unexpected or the product leaves the representable range.
fn duration_words_nanos(text: &str) -> Option<i128> {
    let mut parts = text.split_whitespace();
    let (Some(count), Some(unit), None) = (parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    let count = i128::from(parse_int(count)?);
    let seconds = i128::from(duration_unit_seconds(unit)?);
    count.checked_mul(seconds)?.checked_mul(1_000_000_000)
}

/// The rendered index text of a statically dead list index literal — `0` or any
/// negative literal — or `None` when the key is not a dead-index literal. List
/// positions are 1-based, so a literal `0` or negative names no position and is
/// refused at check time; a positive literal past the length is not statically dead
/// (the length is a runtime fact) and reads absent instead.
fn dead_list_index_literal(key: &Expression) -> Option<String> {
    match key {
        Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } if parse_int(text) == Some(0) => Some("0".to_string()),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => match operand.as_ref() {
            Expression::Literal {
                kind: LiteralKind::Integer,
                text,
                ..
            } => Some(format!("-{}", text.replace('_', ""))),
            _ => None,
        },
        _ => None,
    }
}

/// The bare local name of a bracket base for a teaching diagnostic (`xs` in `xs[0]`),
/// or `None` when the base is a compound expression that has no single-name spelling.
fn simple_base_label(base: &Expression) -> Option<&str> {
    match base {
        Expression::Name { segments, .. } => match segments.as_slice() {
            [name] => Some(name.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// The source spelling of a simple assigned value, for a fix line that names the
/// user's own right-hand side (`v` in `xs[i] = v`, `9` in `xs[1] = 9`). Compound
/// expressions have no short spelling here; the caller falls back to the canonical
/// `_` placeholder.
fn simple_value_spelling(value: &Expression) -> Option<String> {
    match value {
        Expression::Name { segments, .. } => match segments.as_slice() {
            [name] => Some(name.clone()),
            _ => None,
        },
        Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } => Some(text.clone()),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => match operand.as_ref() {
            Expression::Literal {
                kind: LiteralKind::Integer,
                text,
                ..
            } => Some(format!("-{text}")),
            _ => None,
        },
        _ => None,
    }
}

fn unsupported(file: &str, span: SourceSpan, subject: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        span,
        format!("{subject} is not yet supported on the beta line"),
    )
}

/// The store-root name a durable address expression bottoms out at: the leftmost
/// `^name` leaf reached through keyed accesses and field/branch selectors, or `None`
/// when `expr` is not rooted at a `SavedRoot` (not a `^root` durable access at all).
/// The one owner that extracts which store an address names, so the resolvers dispatch
/// against a single root lookup.
fn saved_root_name(expr: &Expression) -> Option<&str> {
    match expr {
        Expression::SavedRoot { name, .. } => Some(name),
        Expression::Keyed { base, .. } | Expression::Field { base, .. } => saved_root_name(base),
        _ => None,
    }
}

/// Whether `expr` is a durable whole-entry address `^root[key]….b[bkey]` at any depth: a
/// keyed access whose base bottoms out at the store root, chained through branch
/// selectors. The single syntactic recognizer of a durable entry address; the resolver
/// rechecks the store and branch names.
fn is_entry_address(expr: &Expression) -> bool {
    let Expression::Keyed { base, .. } = expr else {
        return false;
    };
    match base.as_ref() {
        Expression::SavedRoot { .. } => true,
        Expression::Field { base, .. } => is_entry_address(base),
        _ => false,
    }
}

/// Whether `expr` is a durable field-exact address `<entry-address>.field` at any depth: a
/// field selection on an entry address. A whole root-level group `^root(k).group` has the
/// same shape; the resolver tells a group from a field by name.
fn is_field_address(expr: &Expression) -> bool {
    matches!(expr, Expression::Field { base, .. } if is_entry_address(base))
}

/// Whether `expr` is a durable group-leaf address `^root(k).group.leaf`: a field selection
/// whose base is itself a field-of-an-entry-address (the whole-group address). The resolver
/// confirms the middle selector names a root-level group; a base that turns out to be a
/// stored field is a clean resolution failure, not a group leaf.
fn is_group_leaf_address(expr: &Expression) -> bool {
    matches!(expr, Expression::Field { base, .. } if is_field_address(base))
}

/// A durable operation over a declared-but-not-executable root (a singleton root, a root
/// whose resource declares a nominal-typed field, or one whose only durable content is a
/// group nested in a branch or another group): the shape's identity is complete and in the
/// image, but the kernel does not yet serve it, so the operation is rejected precisely
/// rather than silently dropped. Keyed roots — single-column or a composite tuple — whose
/// top-level fields are scalars or widened values (`struct`/`enum`/`Option`), their
/// root-level `group` members, and their `branch` placements, are executable.
fn not_yet_executable(file: &str, span: SourceSpan, root: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        span,
        format!(
            "durable operations over `^{root}` are not yet executable: a singleton root, a root \
             whose resource declares a nominal-typed field, or a group nested in a branch or \
             another group, declares and verifies its identity but cannot yet be read or written"
        ),
    )
}

fn name_error(file: &str, span: SourceSpan, name: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("`{name}` is not in scope"),
    )
}

fn checked_arm_error(file: &str, span: SourceSpan, detail: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("this checked form {detail}"),
    )
}

fn loop_error(file: &str, span: SourceSpan, keyword: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("`{keyword}` is not inside a loop"),
    )
}

fn type_mismatch(
    records: &TypeRegistry,
    file: &str,
    span: SourceSpan,
    found: LTy,
    want: LTy,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!(
            "found {} where {} is required",
            found.spelling(records),
            want.spelling(records)
        ),
    )
}

fn unary_error(
    records: &TypeRegistry,
    file: &str,
    span: SourceSpan,
    verb: &str,
    ty: LTy,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("cannot {verb} {}", ty.spelling(records)),
    )
}

fn binary_error(
    records: &TypeRegistry,
    file: &str,
    span: SourceSpan,
    op: BinaryOp,
    left: LTy,
    right: LTy,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!(
            "`{}` is not defined for {} and {}",
            operator_symbol(op),
            left.spelling(records),
            right.spelling(records)
        ),
    )
}

fn logic_operand(
    records: &TypeRegistry,
    file: &str,
    span: SourceSpan,
    op: BinaryOp,
    ty: LTy,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!(
            "`{}` operand must be bool, found {}",
            operator_symbol(op),
            ty.spelling(records)
        ),
    )
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
            &[("src/main.mw".to_string(), declaration)],
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
                        file: "src/main.mw",
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
                    file: "src/main.mw",
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
            "src/main.mw",
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
                    file: "src/main.mw",
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
                    file: "src/main.mw",
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
                    file: "src/main.mw",
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
                    file: "src/main.mw",
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
                    file: "src/main.mw",
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
                    file: "src/main.mw",
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
                    file: "src/main.mw",
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
                    file: "src/main.mw",
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
