//! Function-body lowering (design §B/§D).
//!
//! [`FnLowerer`] type-checks the compiled subset and lowers one function body to
//! a draft instruction stream. Locals are allocated one fresh slot per `const`/
//! `var`/param/`if const` binding — slots are never reused — so every read is
//! dominated by the slot's single write and the independent verifier's
//! definite-init dataflow is satisfied. Jumps are emitted with placeholder targets
//! and patched to instruction indices once the target position is known; the
//! encoder rewrites indices to byte offsets.

use std::collections::{BTreeMap, BTreeSet};

use marrow_codes::Code;
use marrow_image::{
    EnumId, FuncId, FunctionDef, ImageDraft, ImageType, Instr, Scalar, SpanEntry, TypeId,
};
use marrow_syntax::{
    Argument, BinaryOp, Block, CheckedBind, ElseIf, Expression, ForBinding, FunctionDecl,
    InterpolationPart, LiteralKind, MatchArm, SourceSpan, Statement, TraversalBound, TypeExpr,
    UnaryOp, decode_interpolation_text, decode_string_literal,
};

use crate::diag::SourceDiagnostic;
use crate::durable::DurableRegistry;
use crate::konst::{ConstRegistry, ConstScalar};
use crate::scalar::ScalarType;
use crate::types::{
    CollSpec, GArg, MAX_INSTANTIATIONS, NominalId, OPTION_NONE, OPTION_SOME, RESULT_ERR, RESULT_OK,
    SupportSet, TypeConstraint, TypeInstId, TypeRegistry,
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

/// Classify an expression as an empty-collection constructor call `List()` or
/// `Map()` (a zero-argument call on the reserved type name), returning the head.
fn collection_ctor_call(expr: &Expression) -> Option<&'static str> {
    let Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    match &**callee {
        Expression::Name { segments, .. } => match segments.as_slice() {
            [n] if n == "List" => Some("List"),
            [n] if n == "Map" => Some("Map"),
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

/// Whether control continues past a statement or block, or leaves it (via `return`,
/// `break`, or `continue`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Fallthrough,
    Terminates,
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

/// The structural durable shape of a place expression.
enum DurShape {
    Entry,
    Field,
}

/// How one key column of a durable operation's key-path reaches the stack.
#[derive(Clone, Copy)]
enum PlaceKey<'e> {
    /// A key operand expression, lowered — and therefore evaluated — at the
    /// operation site (the inline `^root(key)` form).
    Expr(&'e Expression),
    /// A key already evaluated once into a local slot (a named `place`); each use
    /// reads the slot with `LocalGet`, so the operand runs exactly once at the
    /// binding no matter how many operations flow through the place.
    Bound(u16),
    /// The whole root key-path supplied by one entry-identity operand (`^root[id]`):
    /// the identity is lowered, then `IdentityKeyPath` spreads it into the root's key
    /// columns. One `Identity` key stands for every root key column, so it is only the
    /// root whole-key form and never mixes with per-column keys.
    Identity(&'e Expression),
}

/// One column of a durable operation's key-path: how it reaches the stack and its
/// scalar type. A root entry is a one-column path `[root_key]`; a single-level branch
/// entry is a two-column path `[root_key, branch_key]`, pushed root-first so the
/// innermost key is on top (the order the kernel's `pop_key_path` expects).
#[derive(Clone, Copy)]
struct DurKey<'e> {
    key: PlaceKey<'e>,
    key_ty: ScalarType,
}

/// A resolved durable place: the key-path that addresses its node and its target. The
/// path columns are inline operand expressions or a source-local `place`'s
/// pre-evaluated slots; the target is the whole entry or one field.
struct DurablePlace<'e> {
    keys: Vec<DurKey<'e>>,
    target: DurTarget,
    span: SourceSpan,
}

impl DurablePlace<'_> {
    /// The single root key slot when this place's whole key-path is one pre-evaluated
    /// `Bound` column. `None` for an inline key or a nested (branch) path. Used only by
    /// the whole-entry root upsert, which establishes root presence for that one slot.
    fn root_bound_slot(&self) -> Option<u16> {
        match self.keys.as_slice() {
            [
                DurKey {
                    key: PlaceKey::Bound(slot),
                    ..
                },
            ] => Some(*slot),
            _ => None,
        }
    }

    /// This place's whole key-path as pre-evaluated slots (root-first) when *every*
    /// column is a `Bound` slot — the shape a strict present-entry field set and a
    /// place-entry presence guard require, for a root or a branch place. `None` if any
    /// column is an inline key expression (the strict form needs pre-evaluated slots).
    fn bound_key_path(&self) -> Option<Vec<u16>> {
        self.keys
            .iter()
            .map(|column| match column.key {
                PlaceKey::Bound(slot) => Some(slot),
                PlaceKey::Expr(_) | PlaceKey::Identity(_) => None,
            })
            .collect()
    }
}

/// A source-local named `place`: a durable entry designation whose key columns were
/// evaluated exactly once into `key_slots` at the binding. Whole-entry and field
/// operations through the place read those slots rather than re-evaluating the key
/// operands. A root place has one column; a branch place has two (`[root, branch]`)
/// and addresses a nested entry, over which only whole-entry operations lower.
struct PlaceLocal {
    name: String,
    key_slots: Vec<(u16, ScalarType)>,
    entry_site: u16,
    record: TypeId,
}

impl PlaceLocal {
    /// Whether this place addresses a nested (branch) entry rather than the root.
    fn is_nested(&self) -> bool {
        self.key_slots.len() > 1
    }

    /// This place's whole key-path as pre-evaluated slots (root-first) — the key-path a
    /// strict present-entry field set reads and a presence guard proves, for a root or a
    /// branch place uniformly.
    fn key_path_slots(&self) -> Vec<u16> {
        self.key_slots.iter().map(|(slot, _)| *slot).collect()
    }

    /// This place's key-path as resolved [`DurKey`] columns reading the pre-evaluated
    /// slots, root column first.
    fn bound_keys(&self) -> Vec<DurKey<'static>> {
        self.key_slots
            .iter()
            .map(|(slot, ty)| DurKey {
                key: PlaceKey::Bound(*slot),
                key_ty: *ty,
            })
            .collect()
    }
}

/// A resolved durable target: the whole entry, one field, a whole root-level group, or one
/// group leaf.
#[derive(Clone, Copy)]
enum DurTarget {
    Entry {
        entry_site: u16,
        record: TypeId,
    },
    Field {
        site: u16,
        /// The field's value type (a scalar or a widened composite), from which the
        /// read result and written-value type are built.
        ty: GArg,
        required: bool,
    },
    /// A whole root-level `group` (`^root(k).group`): read, replaced, or erased as one
    /// materialized `record` value through the `GroupEntry` site `entry_site`.
    Group {
        entry_site: u16,
        record: TypeId,
    },
    /// One leaf of a root-level group (`^root(k).group.leaf`). A read materializes the
    /// whole group through `entry_site` and projects `slot`; a write or clear is a
    /// whole-group read-modify-write that rewrites `slot` on the read-back group record and
    /// replaces the group, so a leaf never has a durable site of its own.
    GroupLeaf {
        entry_site: u16,
        slot: u16,
        ty: GArg,
        required: bool,
    },
}

/// The leaf edit a group-leaf read-modify-write applies to the materialized group record:
/// set the leaf present to a bare value, or clear a sparse leaf to vacant.
enum GroupLeafEdit<'e> {
    Set { value: &'e Expression, ty: GArg },
    Unset,
}

/// A node reached along a resolved durable entry address: the root, or a keyed branch on
/// the address's branch chain. Both expose the same navigation — a nested branch by name,
/// a stored field, a whole-entry site, and a materialized record — so the recursive address
/// resolver walks them uniformly at any depth.
#[derive(Clone, Copy)]
enum DurNode<'a> {
    Root(&'a crate::durable::DurableRoot),
    Branch(&'a crate::durable::DurableBranch),
}

/// The pieces of one resolved durable field a [`DurTarget::Field`] needs, projected from a
/// root field or a branch field uniformly.
struct DurFieldRef {
    site: u16,
    /// The field's value type: a root field's widened value set, or a branch field's
    /// scalar (branch fields stay scalar-only this lane) lifted to `GArg::Scalar`.
    ty: GArg,
    required: bool,
}

impl<'a> DurNode<'a> {
    fn entry_site(&self) -> u16 {
        match self {
            DurNode::Root(root) => root.entry_site,
            DurNode::Branch(branch) => branch.entry_site,
        }
    }

    fn record(&self) -> TypeId {
        match self {
            DurNode::Root(root) => root.record,
            DurNode::Branch(branch) => branch.record,
        }
    }

    fn branch(&self, name: &str) -> Option<&'a crate::durable::DurableBranch> {
        match self {
            DurNode::Root(root) => root.branch(name),
            DurNode::Branch(branch) => branch.branch(name),
        }
    }

    fn field(&self, name: &str) -> Option<DurFieldRef> {
        match self {
            DurNode::Root(root) => root.field(name).map(|field| DurFieldRef {
                site: field.site,
                ty: field.ty,
                required: field.required,
            }),
            DurNode::Branch(branch) => branch.field(name).map(|field| DurFieldRef {
                site: field.site,
                ty: GArg::Scalar(field.scalar),
                required: field.required,
            }),
        }
    }

    fn name(&self) -> &str {
        match self {
            DurNode::Root(root) => &root.name,
            DurNode::Branch(branch) => &branch.name,
        }
    }

    fn no_field_message(&self, field: &str) -> String {
        match self {
            DurNode::Root(root) => format!("`{}` has no field `{field}`", root.name),
            DurNode::Branch(branch) => {
                format!("branch `{}` has no field `{field}`", branch.name)
            }
        }
    }

    fn no_branch_message(&self, branch: &str) -> String {
        format!("`{}` has no keyed branch `{branch}`", self.name())
    }
}

/// A resolved durable traversal place: the traversed layer's whole-entry site, the
/// immediate key type it enumerates, and the ancestor key-path locating its parent
/// entry (empty for a root family, `[root_key]` for a single-level branch family). The
/// bounded traversal opcode pushes the ancestor path root-first, then the optional
/// inclusive `from` key, and freezes the traversed layer's immediate keys.
struct TraversalTarget<'e> {
    entry_site: u16,
    key_ty: ScalarType,
    /// The materialized record of the traversed family's entry — the shape a two-binding
    /// traversal's per-iteration address pin (`for k, p in …`) binds `p` over.
    record: TypeId,
    ancestor_keys: Vec<DurKey<'e>>,
    span: SourceSpan,
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
}

/// Whether an instruction stages a durable mutation (a write, replacement, or
/// erase). The requires-ambient-transaction check treats these as the sites that
/// demand a transaction; it mirrors the verifier's mutation classification over the
/// same opcode set. The match is exhaustive over `Instr` — the closed complement is
/// listed rather than elided — so a new opcode fails to compile until it is
/// classified here, welding this owner to the instruction set.
fn is_mutation_instr(instr: &Instr) -> bool {
    match instr {
        Instr::DurSetRequired(_)
        | Instr::DurSetSparse(_)
        | Instr::DurSetSparsePresent { .. }
        | Instr::DurCreateEntry(_)
        | Instr::DurReplaceEntry(_)
        | Instr::DurReplaceGroup(_)
        | Instr::DurEraseField(_)
        | Instr::DurEraseEntry(_)
        | Instr::DurEraseGroup(_) => true,
        Instr::ConstLoad(_)
        | Instr::LocalGet(_)
        | Instr::LocalSet(_)
        | Instr::Pop
        | Instr::Return
        | Instr::Call(_)
        | Instr::Jump(_)
        | Instr::JumpIfFalse(_)
        | Instr::BranchPresent(_)
        | Instr::Unreachable(_)
        | Instr::Assert
        | Instr::IntAdd
        | Instr::IntSub
        | Instr::IntMul
        | Instr::IntRem
        | Instr::IntDiv
        | Instr::IntNeg
        | Instr::BoolNot
        | Instr::IntLt
        | Instr::IntLe
        | Instr::IntGt
        | Instr::IntGe
        | Instr::EqInt
        | Instr::EqBool
        | Instr::EqText
        | Instr::TextConcat
        | Instr::TextLt
        | Instr::TextLe
        | Instr::TextGt
        | Instr::TextGe
        | Instr::EqBytes
        | Instr::BytesLt
        | Instr::BytesLe
        | Instr::BytesGt
        | Instr::BytesGe
        | Instr::ConvStringInt
        | Instr::ConvStringBool
        | Instr::ConvBytesText
        | Instr::TextIsEmpty
        | Instr::TextContains
        | Instr::TextTrim
        | Instr::TextSplit(_)
        | Instr::TextLines(_)
        | Instr::TextJoin
        | Instr::EqDate
        | Instr::DateLt
        | Instr::DateLe
        | Instr::DateGt
        | Instr::DateGe
        | Instr::EqInstant
        | Instr::InstantLt
        | Instr::InstantLe
        | Instr::InstantGt
        | Instr::InstantGe
        | Instr::EqDuration
        | Instr::DurationLt
        | Instr::DurationLe
        | Instr::DurationGt
        | Instr::DurationGe
        | Instr::DateAddDays
        | Instr::DateDaysBetween
        | Instr::DurationAdd
        | Instr::DurationSub
        | Instr::InstantAddDuration
        | Instr::InstantSubDuration
        | Instr::IntAddChecked(_)
        | Instr::IntSubChecked(_)
        | Instr::IntMulChecked(_)
        | Instr::IntNegChecked(_)
        | Instr::IntDivChecked(_)
        | Instr::IntRemChecked(_)
        | Instr::RangeGuard { .. }
        | Instr::RecordNew(_)
        | Instr::FieldGet(_)
        | Instr::FieldSet(_)
        | Instr::FieldUnset(_)
        | Instr::SomeWrap
        | Instr::VacantLoad(_)
        | Instr::EnumConstruct { .. }
        | Instr::EnumTag
        | Instr::EnumPayloadGet { .. }
        | Instr::EqEnum
        | Instr::EqId
        | Instr::MakeIdentity { .. }
        | Instr::IdentityKeyPath(_)
        | Instr::DurExists(_)
        | Instr::DurFamilyExists(_)
        | Instr::DurReadField(_)
        | Instr::DurReadEntry(_)
        | Instr::DurReadGroup(_)
        | Instr::DurIterateBounded { .. }
        | Instr::TxnBegin
        | Instr::TxnCommit
        | Instr::DurIndexScan { .. }
        | Instr::DurIndexLookup(_)
        | Instr::ListNew(_)
        | Instr::ListAppend
        | Instr::ListLen
        | Instr::ListGet
        | Instr::MapNew(_)
        | Instr::MapInsert
        | Instr::MapGet
        | Instr::MapLen
        | Instr::MapKeyAt
        | Instr::MapValueAt => false,
    }
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

impl FunctionRegistry {
    /// Resolve every function's signature in declaration order. The i-th function
    /// takes image index `i`, matching the order [`FnLowerer::lower`] adds them.
    /// `functions` pairs each declaration with its dotted module name.
    pub(crate) fn build(
        records: &TypeRegistry,
        draft: &mut ImageDraft,
        durable: &DurableRegistry,
        functions: &[(String, &FunctionDecl)],
        modules: BTreeSet<String>,
        imports: BTreeMap<String, Vec<(String, String)>>,
    ) -> Self {
        let mut sigs = Vec::with_capacity(functions.len());
        // Only monomorphic functions take an image index and enter the signature
        // table; a generic function is a template with no single image entry (its
        // per-application instances are minted lazily), so it is skipped here and
        // resolved through the separate [`GenericRegistry`]. The concrete index runs
        // over non-generic functions only, matching the order [`FnLowerer::lower`]
        // adds them into the image FUNCTIONS table.
        let mut index: u16 = 0;
        for (module, function) in functions {
            if !function.type_params.is_empty() {
                continue;
            }
            let mut params = Vec::with_capacity(function.params.len());
            for param in &function.params {
                if let Some(ty) = param_type(records, draft, durable, &param.ty, TypeEnv::EMPTY) {
                    params.push(ty);
                }
            }
            let ret = match &function.return_type {
                None => RetType::Unit,
                Some(annotation) => {
                    match resolve_type(records, draft, durable, annotation, TypeEnv::EMPTY) {
                        Some(LTy::Record { .. }) | None => {
                            // A resource-record or unsupported return; the function's own
                            // lowering reports it. Record Unit here so indices stay aligned.
                            RetType::Unit
                        }
                        Some(ty) => RetType::Value(ty),
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
        Self {
            sigs,
            modules,
            imports,
        }
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
}

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
    ) -> Option<Lowered> {
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
    ) -> Option<Lowered> {
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
        diagnostics: &mut Vec<SourceDiagnostic>,
        template: &GenericTemplate,
    ) {
        let file = &template.file;
        let module = &template.module;
        // Clone the registry and the in-progress draft together so the template body
        // sees every already-minted type at its real index (a concrete callee's
        // signature stays consistent), while abstract-parameter instantiations and
        // the emitted code land only in the discarded clones.
        let check_records = records.clone_for_generic_check();
        let mut throwaway = draft.clone();
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
            diagnostics,
            file,
            module,
            template.decl,
            type_env,
            LowerMode::Template,
        );
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
    ) -> Option<Lowered> {
        let ret = {
            let env = TypeEnv { params: &type_env };
            match &function.return_type {
                None => RetType::Unit,
                Some(annotation) => match resolve_type(records, draft, durable, annotation, env) {
                    Some(LTy::Record { .. }) => {
                        diagnostics.push(unsupported(
                            file,
                            annotation.span(),
                            "a resource return type",
                        ));
                        return None;
                    }
                    Some(ty) => RetType::Value(ty),
                    None => {
                        diagnostics.push(unsupported(file, annotation.span(), "this return type"));
                        return None;
                    }
                },
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
    ) -> Option<Lowered> {
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
        if lowerer.lower_block(body) == Flow::Fallthrough {
            lowerer.push(Instr::Return, body.span);
        }
        lowerer.finish(name, Vec::new(), ImageType::Unit)
    }

    /// Intern the function name and source, add the lowered function to the draft,
    /// and return its identity — the shared tail of function and test lowering. A
    /// body that failed to lower returns `None`.
    fn finish(mut self, name: &str, params: Vec<ImageType>, ret_ref: ImageType) -> Option<Lowered> {
        if self.failed {
            return None;
        }
        let name_id = self.draft.intern_string(name);
        let source_id = self.draft.intern_string(self.file);
        let code = std::mem::take(&mut self.code);
        let spans = std::mem::take(&mut self.spans);
        let func_id = self.draft.add_function(FunctionDef {
            name: name_id,
            source: source_id,
            params,
            ret: ret_ref,
            local_count: self.slot_count,
            code,
            spans,
        });
        Some(Lowered {
            func: func_id,
            callees: std::mem::take(&mut self.calls),
            unwrapped_mutations: std::mem::take(&mut self.unwrapped_mutations),
            unwrapped_calls: std::mem::take(&mut self.unwrapped_calls),
        })
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

    /// The diagnostic for a durable operation reaching no executable root: a
    /// not-yet-executable rejection when a parked root is declared (a singleton
    /// root, or a root whose resource declares a group or a nominal-typed field —
    /// its identity is complete but the kernel does not serve its shape), or the
    /// caller's no-store rejection when no root is declared at all.
    fn no_executable_root_diagnostic(
        &self,
        span: SourceSpan,
        no_store_subject: &str,
    ) -> SourceDiagnostic {
        match self.durable.not_yet_executable_root() {
            Some(root) => not_yet_executable(self.file, span, root),
            None => unsupported(self.file, span, no_store_subject),
        }
    }

    fn lookup(&self, name: &str) -> Option<&Local> {
        self.locals.iter().rev().find(|local| local.name == name)
    }

    // --- statements ---

    fn lower_block(&mut self, block: &Block) -> Flow {
        let mark = self.locals.len();
        let place_mark = self.places.len();
        // Presence facts established inside this block (e.g. after an upsert) do not
        // outlive it; facts the caller established for the block (a guard) sit below
        // this mark and are preserved here, dropped by the caller.
        let present_mark = self.present_places.len();
        let mut flow = Flow::Fallthrough;
        for statement in &block.statements {
            if flow == Flow::Terminates {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    statement.span(),
                    "this statement is unreachable".to_string(),
                ));
                break;
            }
            flow = self.lower_statement(statement);
        }
        self.locals.truncate(mark);
        self.places.truncate(place_mark);
        self.present_places.truncate(present_mark);
        flow
    }

    fn lower_statement(&mut self, statement: &Statement) -> Flow {
        match statement {
            Statement::Const {
                name, ty, value, ..
            } => {
                self.lower_binding(name, ty.as_ref(), value, false);
                Flow::Fallthrough
            }
            Statement::Var {
                name,
                keys,
                ty,
                value,
                span,
            } => {
                if !keys.is_empty() {
                    self.fail(unsupported(self.file, *span, "a keyed local"));
                    return Flow::Fallthrough;
                }
                let Some(value) = value else {
                    self.fail(unsupported(
                        self.file,
                        *span,
                        "a `var` without an initializer",
                    ));
                    return Flow::Fallthrough;
                };
                self.lower_binding(name, ty.as_ref(), value, true);
                Flow::Fallthrough
            }
            Statement::Assign { target, value, .. } => {
                self.lower_assign(target, value);
                Flow::Fallthrough
            }
            Statement::CompoundAssign {
                target, op, value, ..
            } => {
                self.lower_compound_assign(target, op.binary(), value);
                Flow::Fallthrough
            }
            Statement::Return { value, span } => self.lower_return(value.as_ref(), *span),
            Statement::Break { span } => self.lower_break(*span),
            Statement::Continue { span } => self.lower_continue(*span),
            Statement::Expr { value, .. } => {
                // A call statement may return nothing (no `Pop`); any other expression
                // statement produces a value that is discarded.
                if let Expression::Call {
                    callee, args, span, ..
                } = value
                {
                    match self.lower_call_core(callee, args, *span) {
                        Some(CallResult::Value(_)) => self.push(Instr::Pop, value.span()),
                        Some(CallResult::Diverges) => return Flow::Terminates,
                        Some(CallResult::Unit) | None => {}
                    }
                } else if self.lower_expr(value).is_some() {
                    self.push(Instr::Pop, value.span());
                }
                Flow::Fallthrough
            }
            Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                let mut branches: Vec<(&Expression, &Block)> = vec![(condition, then_block)];
                for else_if in else_ifs {
                    branches.push((&else_if.condition, &else_if.block));
                }
                self.lower_cond_chain(&branches, else_block.as_ref())
            }
            Statement::IfConst {
                name,
                ty,
                value,
                then_block,
                else_ifs,
                else_block,
                ..
            } => self.lower_if_const(
                name,
                ty.as_ref(),
                value,
                then_block,
                else_ifs,
                else_block.as_ref(),
            ),
            Statement::IfConstChain {
                bindings,
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                let bindings: Vec<(&str, Option<&TypeExpr>, &Expression)> = bindings
                    .iter()
                    .map(|b| (b.name.as_str(), b.ty.as_ref(), &b.value))
                    .collect();
                self.lower_if_const_bindings(
                    &bindings,
                    condition.as_ref(),
                    then_block,
                    else_ifs,
                    else_block.as_ref(),
                )
            }
            Statement::LetElse {
                is_var,
                name,
                ty,
                value,
                else_block,
                ..
            } => self.lower_let_else(*is_var, name, ty.as_ref(), value, else_block),
            Statement::While {
                condition, body, ..
            } => self.lower_while(condition, body),
            Statement::For {
                binding,
                order,
                iterable,
                step,
                bound,
                body,
                span,
            } => self.lower_for(
                binding,
                *order,
                iterable,
                step.as_ref(),
                bound.as_ref(),
                body,
                *span,
            ),
            Statement::Checked {
                bind,
                op,
                out_of_range,
                zero_divisor,
                span,
            } => self.lower_checked(
                bind,
                op,
                out_of_range.as_ref(),
                zero_divisor.as_ref(),
                *span,
            ),
            Statement::Transaction { body, .. } => {
                self.push(Instr::TxnBegin, body.span);
                self.txn_depth += 1;
                self.lower_block(body);
                self.txn_depth -= 1;
                self.push(Instr::TxnCommit, body.span);
                Flow::Fallthrough
            }
            Statement::Delete { path, span } => {
                self.lower_durable_delete(path, *span);
                Flow::Fallthrough
            }
            Statement::PlaceBinding {
                name,
                name_span,
                place,
                ..
            } => {
                self.lower_place_binding(name, *name_span, place);
                Flow::Fallthrough
            }
            Statement::Unset { place, span } => {
                self.lower_unset(place, *span);
                Flow::Fallthrough
            }
            Statement::Assert { value, span } => {
                self.lower_assert(value, *span);
                Flow::Fallthrough
            }
            Statement::Match {
                scrutinee,
                arms,
                span,
            } => self.lower_match(scrutinee, arms, *span),
            other => {
                self.fail(unsupported(self.file, other.span(), "this statement"));
                Flow::Fallthrough
            }
        }
    }

    /// Lower `assert <expr>`. The condition must be bool; on false the emitted
    /// `Assert` op faults the running test with `run.assert`. Legal only in a test
    /// body — in an ordinary function it is `check.assert_outside_test`.
    fn lower_assert(&mut self, value: &Expression, span: SourceSpan) {
        if self.body_kind != BodyKind::Test {
            self.fail(SourceDiagnostic::at(
                Code::CheckAssertOutsideTest.as_str(),
                self.file,
                span,
                "`assert` is legal only inside a `test` declaration".to_string(),
            ));
            return;
        }
        if self.lower_condition(value).is_some() {
            self.push(Instr::Assert, span);
        }
    }

    fn lower_binding(
        &mut self,
        name: &str,
        annotation: Option<&TypeExpr>,
        value: &Expression,
        mutable: bool,
    ) {
        if is_reserved_builtin_name(name) {
            self.fail(reserved_builtin_name(self.file, value.span(), name));
            return;
        }
        // A `const`/`var` never reuses an in-scope `place` name: the place and its
        // designation stay distinct, so a name resolves to exactly one of them.
        if self.lookup_place(name).is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                value.span(),
                format!("`{name}` is already bound as a place in this scope"),
            ));
            return;
        }
        let ty = if let Some(annotation) = annotation {
            let Some(expected) = self.resolve(annotation) else {
                self.fail(unsupported(
                    self.file,
                    annotation.span(),
                    "this type annotation",
                ));
                return;
            };
            if self.lower_as(value, expected).is_none() {
                return;
            }
            expected
        } else {
            let Some(ty) = self.lower_expr(value) else {
                return;
            };
            ty
        };
        let slot = self.alloc_slot();
        self.push(Instr::LocalSet(slot), value.span());
        self.locals.push(Local {
            name: name.to_string(),
            ty,
            mutable,
            slot,
        });
    }

    fn lower_assign(&mut self, target: &Expression, value: &Expression) {
        if self.durable_access(target).is_some() {
            if let Some(place) = self.resolve_durable(target) {
                self.lower_durable_assign(place, value);
            }
            return;
        }
        // `local.field = value`: a local product mutation. The base names a mutable
        // local record/struct; the assignment sets the field slot present.
        if let Expression::Field {
            base, name, span, ..
        } = target
        {
            self.lower_local_field_assign(base, name, *span, value);
            return;
        }
        let Some((slot, ty, mutable, span, name)) = self.resolve_place(target) else {
            return;
        };
        if !mutable {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("`{name}` is a `const` and cannot be reassigned"),
            ));
            return;
        }
        if self.lower_as(value, ty).is_none() {
            return;
        }
        self.push(Instr::LocalSet(slot), value.span());
    }

    /// Lower `local.…field = value`: read-modify-write a field, possibly nested one
    /// or more group levels deep. Every container on the path — the local and each
    /// intervening group sub-record — is loaded, the leaf is coerced to its bare value
    /// type and stored present, and each container is written back into its parent up
    /// to the local. A required or a sparse leaf alike becomes present. The path root
    /// must be a mutable local and every container above the leaf must be present.
    fn lower_local_field_assign(
        &mut self,
        base: &Expression,
        field_name: &str,
        span: SourceSpan,
        value: &Expression,
    ) {
        let Some(chain) = self.resolve_place_chain(base) else {
            return;
        };
        if !chain.mutable {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                chain.root_span,
                format!(
                    "`{}` is a `const` and cannot be reassigned",
                    chain.root_name
                ),
            ));
            return;
        }
        let Some((leaf_index, leaf_ty, _required)) =
            self.resolve_product_field(chain.ty, field_name, base.span(), span)
        else {
            return;
        };
        self.push_place_containers(chain.slot, &chain.indices, span);
        if self.lower_as(value, garg_to_lty(leaf_ty)).is_none() {
            return;
        }
        self.push(Instr::FieldSet(leaf_index), span);
        self.writeback_place_containers(chain.slot, &chain.indices, span);
    }

    /// Push the container stack for a nested field mutation: the local at `slot` and
    /// each ancestor container reached by descending `indices`, one per depth from the
    /// local (depth 0) through the leaf's own container (depth `indices.len()`). Every
    /// descended field is present (a required group slot), so each `FieldGet` yields a
    /// bare record. Leaves the containers on the stack for the leaf `FieldSet`/
    /// `FieldUnset` and a matching [`Self::writeback_place_containers`].
    fn push_place_containers(&mut self, slot: u16, indices: &[u16], span: SourceSpan) {
        for depth in 0..=indices.len() {
            self.push(Instr::LocalGet(slot), span);
            for index in &indices[..depth] {
                self.push(Instr::FieldGet(*index), span);
            }
        }
    }

    /// Write each mutated container back into its parent (innermost first) and store
    /// the updated local. Pairs with [`Self::push_place_containers`] after the leaf op
    /// has left the innermost container's new value on the stack.
    fn writeback_place_containers(&mut self, slot: u16, indices: &[u16], span: SourceSpan) {
        for index in indices.iter().rev() {
            self.push(Instr::FieldSet(*index), span);
        }
        self.push(Instr::LocalSet(slot), span);
    }

    /// Lower `unset local.field`: clear a sparse field of a local product to
    /// absent. A required field cannot be unset (`check.type`); a durable place uses
    /// `delete`, not `unset`; a non-field place is unsupported.
    fn lower_unset(&mut self, place: &Expression, span: SourceSpan) {
        if Self::durable_shape(place).is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`unset` clears a local field; use `delete` for a durable place".to_string(),
            ));
            return;
        }
        let Expression::Field {
            base,
            name,
            span: field_span,
            ..
        } = place
        else {
            self.fail(unsupported(self.file, span, "this `unset` target"));
            return;
        };
        let Some(chain) = self.resolve_place_chain(base) else {
            return;
        };
        if !chain.mutable {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                chain.root_span,
                format!("`{}` is a `const` and cannot be modified", chain.root_name),
            ));
            return;
        }
        let Some((leaf_index, _field_ty, required)) =
            self.resolve_product_field(chain.ty, name, base.span(), *field_span)
        else {
            return;
        };
        if required {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                *field_span,
                format!("`{name}` is a required field and cannot be unset"),
            ));
            return;
        }
        self.push_place_containers(chain.slot, &chain.indices, span);
        self.push(Instr::FieldUnset(leaf_index), span);
        self.writeback_place_containers(chain.slot, &chain.indices, span);
    }

    fn lower_compound_assign(&mut self, target: &Expression, op: BinaryOp, value: &Expression) {
        let Some((slot, ty, mutable, span, name)) = self.resolve_place(target) else {
            return;
        };
        if !mutable {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("`{name}` is a `const` and cannot be reassigned"),
            ));
            return;
        }
        if ty.bare_scalar_type().is_none() && ty.bare_nominal().is_none() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "cannot apply a compound assignment to {}",
                    ty.spelling(self.records)
                ),
            ));
            return;
        }
        self.push(Instr::LocalGet(slot), span);
        let Some(result) = self.lower_binary_op(op, ty, value) else {
            return;
        };
        if result != ty {
            self.fail(type_mismatch(
                self.records,
                self.file,
                value.span(),
                result,
                ty,
            ));
            return;
        }
        self.push(Instr::LocalSet(slot), value.span());
    }

    /// Resolve an assignment target to a mutable-checked local place.
    fn resolve_place(
        &mut self,
        target: &Expression,
    ) -> Option<(u16, LTy, bool, SourceSpan, String)> {
        let Expression::Name { segments, span, .. } = target else {
            self.fail(unsupported(
                self.file,
                target.span(),
                "this assignment target",
            ));
            return None;
        };
        let [name] = segments.as_slice() else {
            self.fail(unsupported(self.file, *span, "this assignment target"));
            return None;
        };
        let Some(local) = self.lookup(name) else {
            self.fail(name_error(self.file, *span, name));
            return None;
        };
        Some((local.slot, local.ty, local.mutable, *span, name.clone()))
    }

    /// Resolve a place expression to its chain of present composite containers rooted
    /// at a local: a bare local, or a local descended through one or more group
    /// members. Each intervening member must be a present (required) composite so a
    /// read-modify-write reaches it; a possibly-absent member is a `check.type`
    /// rejection. Reports name and non-place errors like [`Self::resolve_place`].
    fn resolve_place_chain(&mut self, target: &Expression) -> Option<PlaceChain> {
        match target {
            Expression::Name { segments, span, .. } => {
                let [name] = segments.as_slice() else {
                    self.fail(unsupported(self.file, *span, "this assignment target"));
                    return None;
                };
                let Some(local) = self.lookup(name) else {
                    self.fail(name_error(self.file, *span, name));
                    return None;
                };
                Some(PlaceChain {
                    slot: local.slot,
                    mutable: local.mutable,
                    root_span: *span,
                    root_name: name.clone(),
                    ty: local.ty,
                    indices: Vec::new(),
                })
            }
            Expression::Field {
                base, name, span, ..
            } => {
                let mut chain = self.resolve_place_chain(base)?;
                let (index, field_ty, required) =
                    self.resolve_product_field(chain.ty, name, base.span(), *span)?;
                if !required {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *span,
                        format!(
                            "cannot assign through the possibly-absent member `{name}`. A member \
                             that is not `required` is absent until it holds a value, and a \
                             read-modify-write cannot begin from an absent place. Assign `{name}` \
                             a present value first."
                        ),
                    ));
                    return None;
                }
                chain.indices.push(index);
                chain.ty = garg_to_lty(field_ty);
                Some(chain)
            }
            _ => {
                self.fail(unsupported(
                    self.file,
                    target.span(),
                    "this assignment target",
                ));
                None
            }
        }
    }

    fn lower_return(&mut self, value: Option<&Expression>, span: SourceSpan) -> Flow {
        match (value, self.ret) {
            (None, RetType::Unit) => {
                self.push(Instr::Return, span);
            }
            (None, RetType::Value(_)) => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    "this function must return a value".to_string(),
                ));
            }
            (Some(expr), RetType::Unit) => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    expr.span(),
                    "this function returns nothing".to_string(),
                ));
            }
            (Some(expr), RetType::Value(want)) => {
                if self.lower_as(expr, want).is_some() {
                    self.push(Instr::Return, span);
                }
            }
        }
        Flow::Terminates
    }

    fn lower_break(&mut self, span: SourceSpan) -> Flow {
        if self.loops.is_empty() {
            self.fail(loop_error(self.file, span, "break"));
            return Flow::Terminates;
        }
        let at = self.push_jump(span);
        self.loops
            .last_mut()
            .expect("loop present")
            .break_jumps
            .push(at);
        Flow::Terminates
    }

    fn lower_continue(&mut self, span: SourceSpan) -> Flow {
        let Some(ctx) = self.loops.last() else {
            self.fail(loop_error(self.file, span, "continue"));
            return Flow::Terminates;
        };
        let target = ctx.continue_target;
        self.push(Instr::Jump(target as u32), span);
        Flow::Terminates
    }

    /// Lower a chain of conditional branches followed by an optional `else`. Used for
    /// `if`/`else if`/`else` and for the absent tail of `if const`.
    fn lower_cond_chain(
        &mut self,
        branches: &[(&Expression, &Block)],
        else_block: Option<&Block>,
    ) -> Flow {
        let mut end_jumps: Vec<usize> = Vec::new();
        let mut all_terminate = else_block.is_some();

        for (cond, block) in branches {
            // `exists(p)` over a named place proves the entry present in the guarded
            // block: a sparse-field set through `p` there lowers to the strict form.
            let guard_path = self.exists_guard_path(cond);
            if self.lower_condition(cond).is_none() {
                return Flow::Fallthrough;
            }
            let jif = self.push_jif(cond.span());
            let present_mark = self.present_places.len();
            if let Some(path) = guard_path {
                self.mark_present(path);
            }
            let flow = self.lower_block(block);
            self.present_places.truncate(present_mark);
            all_terminate &= flow == Flow::Terminates;
            if flow == Flow::Fallthrough {
                end_jumps.push(self.push_jump(block.span));
            }
            let next = self.here();
            self.patch(jif, next);
        }

        if let Some(else_block) = else_block {
            let flow = self.lower_block(else_block);
            all_terminate &= flow == Flow::Terminates;
        }

        let end = self.here();
        self.patch_all(end_jumps, end);
        if all_terminate {
            Flow::Terminates
        } else {
            Flow::Fallthrough
        }
    }

    fn lower_if_const(
        &mut self,
        name: &str,
        annotation: Option<&TypeExpr>,
        value: &Expression,
        then_block: &Block,
        else_ifs: &[ElseIf],
        else_block: Option<&Block>,
    ) -> Flow {
        // The single `if const a = e` is the one-binding, no-condition case of the
        // general chained form.
        self.lower_if_const_bindings(
            &[(name, annotation, value)],
            None,
            then_block,
            else_ifs,
            else_block,
        )
    }

    /// Lower the general `if const` form (B5): a left-to-right chain of existence
    /// bindings joined by `and` and an optional trailing bare condition, with the
    /// then and `else if`/`else` tails. Each binding's value is proven present
    /// before the next is evaluated (short-circuit), each binding scopes rightward
    /// into later binding values, the condition, and the then block, and any absent
    /// binding or false condition takes the else tail. This is the one owner of
    /// `if const` lowering; the single form is one binding with no condition.
    fn lower_if_const_bindings(
        &mut self,
        bindings: &[(&str, Option<&TypeExpr>, &Expression)],
        condition: Option<&Expression>,
        then_block: &Block,
        else_ifs: &[ElseIf],
        else_block: Option<&Block>,
    ) -> Flow {
        let mark = self.locals.len();
        let present_mark = self.present_places.len();

        // The present path threads through every binding and the condition into the
        // then block; every failure edge (an absent binding or a false condition)
        // jumps to the shared absent tail. Each `BranchPresent`/`JumpIfFalse` pops its
        // own operand, so all failure edges reach the tail with a balanced stack.
        let Some(fail_jumps) = self.lower_if_const_head(bindings, condition) else {
            self.present_places.truncate(present_mark);
            self.locals.truncate(mark);
            return Flow::Fallthrough;
        };

        let then_flow = self.lower_block(then_block);
        self.present_places.truncate(present_mark);
        self.locals.truncate(mark);

        let mut end_jumps = Vec::new();
        if then_flow == Flow::Fallthrough {
            end_jumps.push(self.push_jump(then_block.span));
        }

        // Absent/false tail: the `else if`/`else` chain.
        let absent = self.here();
        self.patch_all(fail_jumps, absent);
        let branches: Vec<(&Expression, &Block)> = else_ifs
            .iter()
            .map(|else_if| (&else_if.condition, &else_if.block))
            .collect();
        let else_flow = self.lower_cond_chain(&branches, else_block);

        let end = self.here();
        self.patch_all(end_jumps, end);

        if then_flow == Flow::Terminates && else_flow == Flow::Terminates {
            Flow::Terminates
        } else {
            Flow::Fallthrough
        }
    }

    /// Emit the present-threading head of an `if const` chain: for each binding,
    /// prove its value present and bind it to a fresh local scoped rightward; then
    /// evaluate the optional trailing condition. Returns the failure jumps to patch
    /// to the absent tail, leaving the bindings' locals in scope for the then block;
    /// `None` on a hard type error (the caller restores the local stack).
    fn lower_if_const_head(
        &mut self,
        bindings: &[(&str, Option<&TypeExpr>, &Expression)],
        condition: Option<&Expression>,
    ) -> Option<Vec<usize>> {
        let mut fail_jumps: Vec<usize> = Vec::new();
        for (name, annotation, value) in bindings.iter().copied() {
            if is_reserved_builtin_name(name) {
                self.fail(reserved_builtin_name(self.file, value.span(), name));
                return None;
            }
            // A whole durable entry address (`if const book = ^books(id)` or the named
            // `place` form) reads the entry here and proves it present on the guarded
            // edge, so a sparse-field set through the same place in the then block
            // lowers strict; a bare place name is otherwise not a value.
            let mut guard_path: Option<Vec<u16>> = None;
            let optional = if matches!(self.durable_access(value), Some(DurShape::Entry)) {
                let place = self.resolve_durable(value)?;
                guard_path = place.bound_key_path();
                self.lower_durable_read(place)?
            } else {
                self.lower_expr(value)?
            };
            if !optional.is_optional() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    value.span(),
                    format!(
                        "`if const` needs an optional, found {}",
                        optional.spelling(self.records)
                    ),
                ));
                return None;
            }
            let bound = optional.to_bare();
            if let Some(annotation) = annotation
                && let Some(declared) = self.resolve(annotation)
                && declared != bound
            {
                self.fail(type_mismatch(
                    self.records,
                    self.file,
                    annotation.span(),
                    bound,
                    declared,
                ));
                return None;
            }

            // Present edge falls through with the unwrapped bare value; absent edge
            // jumps to the shared tail.
            fail_jumps.push(self.push_branch_present(value.span()));
            let slot = self.alloc_slot();
            self.push(Instr::LocalSet(slot), value.span());
            self.locals.push(Local {
                name: name.to_string(),
                ty: bound,
                mutable: false,
                slot,
            });
            if let Some(path) = guard_path {
                self.mark_present(path);
            }
        }

        if let Some(cond) = condition {
            let cond_ty = self.lower_expr(cond)?;
            if cond_ty != LTy::bare_scalar(ScalarType::Bool) {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    cond.span(),
                    format!(
                        "an `if const` chain condition must be `bool`, found {}",
                        cond_ty.spelling(self.records)
                    ),
                ));
                return None;
            }
            fail_jumps.push(self.push_jif(cond.span()));
        }

        Some(fail_jumps)
    }

    /// Lower `const x = e else { … }` / `var x = e else { … }` (B6, let-else): bind
    /// `x` from the present value of the optional `e` and continue with `x` in scope
    /// for the rest of the enclosing block; when `e` is absent, run the `else` block,
    /// which must diverge. Reuses the one-binding `if const` head for the present
    /// path, and the existing `Flow::Terminates` divergence analysis proves the else
    /// diverges — so let-else adds no new control-flow analysis.
    fn lower_let_else(
        &mut self,
        is_var: bool,
        name: &str,
        annotation: Option<&TypeExpr>,
        value: &Expression,
        else_block: &Block,
    ) -> Flow {
        let mark = self.locals.len();
        let present_mark = self.present_places.len();
        let Some(fail_jumps) = self.lower_if_const_head(&[(name, annotation, value)], None) else {
            self.locals.truncate(mark);
            self.present_places.truncate(present_mark);
            return Flow::Fallthrough;
        };
        // The head bound `x` (and, for a durable entry read, a presence fact) on the
        // present edge. They belong to the continuation after the statement, not to
        // the `else` — the absent edge, where `x` is not established. Lift them out so
        // the `else` cannot see the binding (a reference there is a scoped unknown
        // name, not an uninitialized-slot image rejection) and restore them for the
        // continuation. A `var` let-else binds mutably.
        let mut bound_locals = self.locals.split_off(mark);
        let bound_present = self.present_places.split_off(present_mark);
        if is_var {
            for local in &mut bound_locals {
                local.mutable = true;
            }
        }

        // The present path continues past the `else`; the absent edge runs the
        // diverging `else` block, so control only reaches past the statement with `x`
        // bound.
        let to_after = self.push_jump(value.span());
        let absent = self.here();
        self.patch_all(fail_jumps, absent);
        let else_flow = self.lower_block(else_block);
        if else_flow != Flow::Terminates {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                else_block.span,
                "the `else` of a let-else binding must diverge, for example with \
                 `return`, `throw`, or `unreachable`"
                    .to_string(),
            ));
        }
        let after = self.here();
        self.patch(to_after, after);

        // Restore the binding and presence fact for the continuation: past the
        // statement `x` is always present, because the absent edge diverged.
        self.locals.extend(bound_locals);
        self.present_places.extend(bound_present);
        Flow::Fallthrough
    }

    /// Lower a `match` over a flat enum scrutinee (design §B). The scrutinee is
    /// evaluated once into a fresh local; the arms dispatch through a branch chain
    /// over the enum tag (`EnumTag` + `EqInt` + `JumpIfFalse`), the simplest form
    /// the verifier admits without a tag-switch opcode. The match must cover every
    /// member exactly once with no wildcard arm; exhaustiveness is a check-time
    /// rule, not an image invariant. Because the match is exhaustive, the last arm
    /// in source order runs unconditionally (no test): every other member is caught
    /// by an earlier arm, so only its own member reaches it, which also makes its
    /// positional payload reads (`EnumPayloadGet`) sound.
    fn lower_match(&mut self, scrutinee: &Expression, arms: &[MatchArm], span: SourceSpan) -> Flow {
        let Some(scrut_ty) = self.lower_expr(scrutinee) else {
            return Flow::Fallthrough;
        };
        let Some(enum_id) = scrut_ty.bare_enum() else {
            self.fail(SourceDiagnostic::at(
                Code::CheckMatchArm.as_str(),
                self.file,
                scrutinee.span(),
                format!(
                    "`match` needs an enum value, found {}",
                    scrut_ty.spelling(self.records)
                ),
            ));
            return Flow::Fallthrough;
        };
        // The scrutinee's variants: member name plus payload type list, owned so the
        // arm loop can borrow `self` mutably while resolving each arm. A concrete
        // user `enum`, a generic enum instantiation, and the reserved `Option`/
        // `Result` (themselves generic enums) all supply their variants through the
        // one enum-shape owner.
        let enum_name = scrut_ty.spelling(self.records);
        let variants: Vec<(String, Vec<LTy>)> = self
            .records
            .enum_variants(enum_id)
            .expect("a bare enum value resolves to enum variants")
            .into_iter()
            .map(|(name, payload)| (name, payload.into_iter().map(garg_to_lty).collect()))
            .collect();

        let scrut_slot = self.alloc_slot();
        self.push(Instr::LocalSet(scrut_slot), span);

        let mut covered = vec![false; variants.len()];
        let mut end_jumps: Vec<usize> = Vec::new();
        let mut all_terminate = true;
        let mut any_arm = false;
        let arm_count = arms.len();

        for (position, arm) in arms.iter().enumerate() {
            let is_last = position + 1 == arm_count;
            let [member] = arm.path.as_slice() else {
                self.fail(unsupported(
                    self.file,
                    arm.span,
                    "a hierarchical (`::`) match arm",
                ));
                continue;
            };
            let Some(variant_index) = variants.iter().position(|(name, _)| name == member) else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckMatchArm.as_str(),
                    self.file,
                    arm.span,
                    format!("`{member}` is not a member of `{enum_name}`"),
                ));
                continue;
            };
            if covered[variant_index] {
                self.fail(SourceDiagnostic::at(
                    Code::CheckMatchArm.as_str(),
                    self.file,
                    arm.span,
                    format!("member `{member}` is already covered by an earlier arm"),
                ));
                continue;
            }
            covered[variant_index] = true;
            any_arm = true;
            let payload = variants[variant_index].1.clone();
            if !arm.bindings.is_empty() && arm.bindings.len() != payload.len() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckMatchArm.as_str(),
                    self.file,
                    arm.span,
                    format!(
                        "member `{member}` carries {} payload field(s), but the arm binds {}",
                        payload.len(),
                        arm.bindings.len()
                    ),
                ));
                continue;
            }

            // Non-last arms test the tag and skip to the next arm on a mismatch;
            // the exhaustive last arm runs unconditionally.
            let to_next = if is_last {
                None
            } else {
                self.push(Instr::LocalGet(scrut_slot), arm.span);
                self.push(Instr::EnumTag, arm.span);
                let konst = self.draft.intern_int(variant_index as i64);
                self.push(Instr::ConstLoad(konst.index()), arm.span);
                self.push(Instr::EqInt, arm.span);
                Some(self.push_jif(arm.span))
            };

            // Bind the payload positionally into fresh locals scoped to the arm.
            let mark = self.locals.len();
            for (field, (binding, leaf_ty)) in arm.bindings.iter().zip(&payload).enumerate() {
                let slot = self.alloc_slot();
                self.push(Instr::LocalGet(scrut_slot), binding.span);
                self.push(
                    Instr::EnumPayloadGet {
                        variant: variant_index as u16,
                        field: field as u16,
                    },
                    binding.span,
                );
                self.push(Instr::LocalSet(slot), binding.span);
                self.locals.push(Local {
                    name: binding.name.clone(),
                    ty: *leaf_ty,
                    mutable: false,
                    slot,
                });
            }
            let flow = self.lower_block(&arm.block);
            self.locals.truncate(mark);
            all_terminate &= flow == Flow::Terminates;
            if flow == Flow::Fallthrough && !is_last {
                end_jumps.push(self.push_jump(arm.block.span));
            }
            if let Some(to_next) = to_next {
                let next = self.here();
                self.patch(to_next, next);
            }
        }

        // Exhaustiveness: every member covered exactly once, no wildcard arm.
        let missing: Vec<(&str, usize)> = variants
            .iter()
            .zip(&covered)
            .filter(|(_, covered)| !**covered)
            .map(|((name, payload), _)| (name.as_str(), payload.len()))
            .collect();
        if !missing.is_empty() {
            let names = missing
                .iter()
                .map(|(name, _)| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ");
            // The canonical arm head each missing member needs: a payloadless member
            // takes `member =>`; a member with an N-value payload takes N positional
            // bindings, spelled `member(_, …) =>` with author-neutral `_` placeholders.
            let arms = missing
                .iter()
                .map(|(name, arity)| match arity {
                    0 => format!("`{name} =>`"),
                    n => format!("`{name}({}) =>`", vec!["_"; *n].join(", ")),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let arm_word = if missing.len() == 1 { "arm" } else { "arms" };
            self.fail(SourceDiagnostic::at(
                Code::CheckMatchNonexhaustive.as_str(),
                self.file,
                span,
                format!(
                    "the `match` on `{enum_name}` does not cover {names}. A match covers every \
                     member of an enum exactly once and admits no wildcard arm. Add the missing \
                     {arm_word}: {arms}."
                ),
            ));
        }

        let end = self.here();
        self.patch_all(end_jumps, end);
        // The match terminates only when it is exhaustive (so the unconditional last
        // arm is reached) and every arm diverges.
        if any_arm && missing.is_empty() && all_terminate {
            Flow::Terminates
        } else {
            Flow::Fallthrough
        }
    }

    /// Lower a `for` loop. A durable root/branch traversal place (`^root` or
    /// `^root(k).branch`) takes the bounded freeze-then-run path; a range or local
    /// `List`/`Map` iterable takes the collection path. Reversed order and a range
    /// step apply only to the latter.
    #[allow(clippy::too_many_arguments)]
    fn lower_for(
        &mut self,
        binding: &ForBinding,
        order: marrow_syntax::LoopOrder,
        iterable: &Expression,
        step: Option<&Expression>,
        bound: Option<&TraversalBound>,
        body: &Block,
        span: SourceSpan,
    ) -> Flow {
        // A `for` head over a managed index scans it. Only a nonunique index is scanned
        // (progressive-prefix); a unique index is an exact lookup, not an iteration.
        if let Some((index, keys)) = self.resolve_index_read(iterable) {
            if index.unique {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!(
                        "unique index `{}` is an exact lookup, not a scan; read it with \
                         `if const x = ^root.{}[keys]`",
                        index.name, index.name
                    ),
                ));
                return Flow::Fallthrough;
            }
            if order != marrow_syntax::LoopOrder::Forward {
                self.fail(unsupported(self.file, span, "reversed index scan"));
                return Flow::Fallthrough;
            }
            return self.lower_index_scan(binding, index, keys, bound, body, span);
        }
        // A durable traversal place iterates the store; it is always bounded.
        if Self::is_traversal_place(iterable) {
            if order != marrow_syntax::LoopOrder::Forward {
                self.fail(unsupported(self.file, span, "reversed durable traversal"));
                return Flow::Fallthrough;
            }
            let Some(target) = self.resolve_traversal_place(iterable) else {
                return Flow::Fallthrough;
            };
            return self.lower_bounded_traversal(binding, target, bound, body, span);
        }
        // A range or local collection takes no `at most N` / `on more` clause.
        if bound.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`at most N` and `on more` apply only to a durable root or branch \
                 traversal (`for k in ^root at most N`)"
                    .to_string(),
            ));
            return Flow::Fallthrough;
        }
        if order != marrow_syntax::LoopOrder::Forward {
            self.fail(unsupported(self.file, span, "reversed iteration"));
            return Flow::Fallthrough;
        }
        if step.is_some() {
            self.fail(unsupported(self.file, span, "a loop step"));
            return Flow::Fallthrough;
        }
        self.lower_for_collection(binding, iterable, body, span)
    }

    /// Whether `iterable` names a durable traversal place syntactically: a bare store root
    /// `^root` (the root entry family) or an entry address extended by a bare branch-layer
    /// name `^root(key)….branch` (a keyed branch family under a fixed ancestor key-path, at
    /// any depth). The resolver rechecks the store and branch names; this only routes the
    /// head to the durable path.
    fn is_traversal_place(iterable: &Expression) -> bool {
        match iterable {
            Expression::SavedRoot { .. } => true,
            Expression::Field { base, .. } => is_entry_address(base),
            _ => false,
        }
    }

    /// Whether an `exists` argument names a family (a store root, or a keyed branch family)
    /// rather than a specific entry or field. A store root is always a family; a `.tail`
    /// selection on an entry address is a family only when `tail` is a declared keyed
    /// branch — a scalar-field selection is a specific-cell probe. Non-emitting: it
    /// classifies the argument before a probe is chosen, since a branch family and a
    /// scalar field share the `Field`-on-entry-address syntax.
    fn arg_is_family(&self, expr: &Expression) -> bool {
        match expr {
            Expression::SavedRoot { .. } => true,
            Expression::Field { base, name, .. } => self
                .entry_address_node(base)
                .is_some_and(|parent| parent.branch(name).is_some()),
            _ => false,
        }
    }

    /// The durable node an entry-address expression addresses, resolved against the single
    /// durable root without emitting a diagnostic. `None` when `expr` is not a resolvable
    /// entry address (a wrong root name, an unknown branch, or a non-address shape). Used
    /// only to classify an `exists` tail; the real resolvers own diagnostics.
    fn entry_address_node(&self, expr: &Expression) -> Option<DurNode<'a>> {
        let root = self.durable.root()?;
        let Expression::Keyed { base, .. } = expr else {
            return None;
        };
        match &**base {
            Expression::SavedRoot { name, .. } => {
                (root.name == *name).then_some(DurNode::Root(root))
            }
            Expression::Field {
                base: parent_base,
                name: branch_name,
                ..
            } => {
                let parent = self.entry_address_node(parent_base)?;
                parent.branch(branch_name).map(DurNode::Branch)
            }
            _ => None,
        }
    }

    /// Resolve a durable traversal place into the traversed layer's entry site, its
    /// immediate key type, and the ancestor key-path locating its parent entry (empty for a
    /// root family, `[root_key]` for a single-level branch family, deeper for a nested
    /// branch layer). The iterable is the root itself, or an entry address extended by a
    /// bare branch-layer name `^root(k)….b(bk).layer`; the branch chain before the layer
    /// resolves through the recursive entry-address walker, so an inner branch layer
    /// iterates under a full ancestor key-path. Reports a precise diagnostic and returns
    /// `None` on a missing store, a wrong store name, or an unknown branch.
    fn resolve_traversal_place<'e>(
        &mut self,
        iterable: &'e Expression,
    ) -> Option<TraversalTarget<'e>> {
        match iterable {
            Expression::SavedRoot { name, span } => {
                let Some(root) = self.durable.root() else {
                    let diagnostic =
                        self.no_executable_root_diagnostic(*span, "iterating without a store");
                    self.fail(diagnostic);
                    return None;
                };
                let entry_site = root.entry_site;
                let record = root.record;
                self.check_root_name(root, name, *span)?;
                let key_ty = self.single_traversal_column(&root.key, *span)?;
                Some(TraversalTarget {
                    entry_site,
                    key_ty,
                    record,
                    ancestor_keys: Vec::new(),
                    span: *span,
                })
            }
            Expression::Field {
                base,
                name: layer_name,
                name_span: layer_span,
                span,
                ..
            } => {
                let Some(root) = self.durable.root() else {
                    let diagnostic = self.no_executable_root_diagnostic(
                        iterable.span(),
                        "iterating without a store",
                    );
                    self.fail(diagnostic);
                    return None;
                };
                // The base is the addressed parent entry `^root(k)….b(bk)`; the final bare
                // name is the branch family iterated under it. Its ancestor key-path is the
                // parent entry's whole key-path (root-first).
                let (ancestor_keys, parent) = self.resolve_entry_address(root, base)?;
                let Some(layer) = parent.branch(layer_name) else {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *layer_span,
                        parent.no_branch_message(layer_name),
                    ));
                    return None;
                };
                let entry_site = layer.entry_site;
                let record = layer.record;
                let key_ty = self.single_traversal_column(&layer.key, *span)?;
                Some(TraversalTarget {
                    entry_site,
                    key_ty,
                    record,
                    ancestor_keys,
                    span: *span,
                })
            }
            _ => None,
        }
    }

    /// The single key column of a traversable layer, or a typed `check.unsupported` when
    /// the layer is composite-keyed. Bounded traversal binds one immediate key and takes
    /// one inclusive `from`; the current language spells no composite-key iteration, so a
    /// composite-keyed layer parks rather than inventing a last-column-under-prefix
    /// semantics.
    fn single_traversal_column(
        &mut self,
        columns: &[ScalarType],
        span: SourceSpan,
    ) -> Option<ScalarType> {
        match columns {
            [only] => Some(*only),
            _ => {
                self.fail(unsupported(
                    self.file,
                    span,
                    "bounded traversal over a composite-keyed layer",
                ));
                None
            }
        }
    }

    /// Lower a bounded durable traversal `for k in <place> at most N [from f] on more`.
    /// Freeze the first `N` immediate keys of the traversed layer (after an inclusive
    /// `from`), run the body once per frozen key in order, then run the `on more` block
    /// when an `(N+1)`th key existed and every frozen body completed normally.
    fn lower_bounded_traversal(
        &mut self,
        binding: &ForBinding,
        target: TraversalTarget,
        bound: Option<&TraversalBound>,
        body: &Block,
        span: SourceSpan,
    ) -> Flow {
        let Some(bound) = bound else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "this durable traversal is unbounded. A `for` head over a durable root or \
                 branch is always bounded and states its overflow behavior. Add `at most N` \
                 and an `on more { … }` block."
                    .to_string(),
            ));
            return Flow::Fallthrough;
        };
        // A durable traversal binds the immediate key, and optionally a second name: a
        // per-iteration address pin (`place` semantics) over the entry at that key. More
        // than two names has no durable meaning.
        let (var, place_var) = match binding.names.as_slice() {
            [key] => (key, None),
            [key, address] => (key, Some(address)),
            _ => {
                self.fail(unsupported(
                    self.file,
                    span,
                    "binding more than a key and a per-iteration address in a traversal",
                ));
                return Flow::Fallthrough;
            }
        };
        let Some(on_more) = &bound.on_more else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "this bounded traversal has no overflow arm. A bounded `for` head states its \
                 overflow behavior in a trailing `on more` block. Add an `on more { … }` block."
                    .to_string(),
            ));
            return Flow::Fallthrough;
        };
        let Some(limit) = self.traversal_limit(&bound.limit) else {
            return Flow::Fallthrough;
        };
        let key_ty = target.key_ty;
        // The frozen keys materialize as one `List[K]`; mint (deduplicated) that row.
        let list_ty = self
            .records
            .instantiate_list(self.draft, GArg::Scalar(key_ty));

        // Evaluate the ancestor key-path (root-first) then the inclusive `from` key, so
        // the opcode pops `from` (top) then the ancestor path. Keys are captured once,
        // before any body runs. A two-binding traversal captures the ancestor keys into
        // slots first — its per-iteration address pin reads them alongside the loop key —
        // then pushes the same slots as the opcode operands; a single-binding traversal
        // pushes the ancestor keys straight.
        let ancestor_slots: Vec<(u16, ScalarType)> = if place_var.is_some() {
            let mut slots = Vec::with_capacity(target.ancestor_keys.len());
            for column in &target.ancestor_keys {
                let PlaceKey::Expr(key_expr) = column.key else {
                    self.fail(unsupported(
                        self.file,
                        target.span,
                        "a per-iteration address over this traversal",
                    ));
                    return Flow::Fallthrough;
                };
                let slot = self.alloc_slot();
                if self
                    .lower_as(key_expr, LTy::bare_scalar(column.key_ty))
                    .is_none()
                {
                    return Flow::Fallthrough;
                }
                self.push(Instr::LocalSet(slot), target.span);
                slots.push((slot, column.key_ty));
            }
            for (slot, _) in &slots {
                self.push(Instr::LocalGet(*slot), target.span);
            }
            slots
        } else {
            if self
                .emit_key_path(&target.ancestor_keys, target.span)
                .is_none()
            {
                return Flow::Fallthrough;
            }
            Vec::new()
        };
        let has_from = bound.from.is_some();
        if let Some(from_expr) = &bound.from
            && self.lower_as(from_expr, LTy::bare_scalar(key_ty)).is_none()
        {
            return Flow::Fallthrough;
        }
        self.push(
            Instr::DurIterateBounded {
                site: target.entry_site,
                limit,
                from: has_from,
                list_ty,
            },
            span,
        );
        // Bind the on-more bit and the frozen list into fresh slots.
        let more_slot = self.alloc_slot();
        self.push(Instr::LocalSet(more_slot), span);
        let coll_slot = self.alloc_slot();
        self.push(Instr::LocalSet(coll_slot), span);

        // A positional walk over the frozen `List[K]` binds `k` per position.
        // `continue` advances to the loop top; a body `break`/`return` skips past
        // the `on more` block.
        let entry_site = target.entry_site;
        let record = target.record;
        let key_name = var.name.clone();
        let place_name = place_var.map(|name| name.name.clone());
        let break_jumps = self.lower_positional_walk(
            coll_slot,
            Instr::ListLen,
            body,
            span,
            move |lower, index_slot| {
                let key_slot = lower.alloc_slot();
                lower.push(Instr::LocalGet(coll_slot), span);
                lower.push(Instr::LocalGet(index_slot), span);
                lower.push(Instr::ListGet, span);
                // Rebinding the key slot each iteration kills, through the verifier's
                // LocalSet presence-lattice rule, any presence fact an earlier iteration
                // established on this key: a fact proven in iteration N cannot survive into
                // N+1.
                lower.push(Instr::LocalSet(key_slot), span);
                // Traversal establishes no presence fact for the body: `k` names a
                // frozen key whose entry an earlier body iteration may already have
                // erased.
                lower.locals.push(Local {
                    name: key_name,
                    ty: LTy::bare_scalar(key_ty),
                    mutable: false,
                    slot: key_slot,
                });
                // The optional second binding is a per-iteration address pin: a `place`
                // over the entry at the current key. Its key-path is the captured ancestor
                // slots followed by this iteration's key slot; it reads nothing and
                // establishes no presence fact, so a write through it is an ordinary
                // sparse set unless a dominating `exists` proves the entry present.
                if let Some(place_name) = place_name {
                    let mut key_slots = ancestor_slots;
                    key_slots.push((key_slot, key_ty));
                    lower.places.push(PlaceLocal {
                        name: place_name,
                        key_slots,
                        entry_site,
                        record,
                    });
                }
            },
        );

        // Normal exhaustion falls through to here: run `on more` iff a further key
        // existed.
        self.push(Instr::LocalGet(more_slot), span);
        let skip_on_more = self.push_jif(span);
        self.lower_block(on_more);
        let end = self.here();
        self.patch(skip_on_more, end);
        // A body break jumps past the whole loop, skipping the `on more` decision.
        self.patch_all(break_jumps, end);
        Flow::Fallthrough
    }

    /// Lower a bounded scan of a nonunique managed index `^root.index[prefix…]`. The scan
    /// holds the index's leading field components as a prefix and yields the trailing
    /// identity component as the source `Id(^root)`, so the loop variable binds an
    /// identity: the frozen raw identity keys materialize as one `List[K]`, and each is
    /// wrapped into an `Id(^root)` at the binding. The scan requires a single-column
    /// identity root (so the yielded component is a whole identity) and does not admit a
    /// `from` cursor or a per-iteration address pin on this line.
    fn lower_index_scan(
        &mut self,
        binding: &ForBinding,
        index: &crate::durable::DurableIndex,
        keys: &[Expression],
        bound: Option<&TraversalBound>,
        body: &Block,
        span: SourceSpan,
    ) -> Flow {
        let var = match binding.names.as_slice() {
            [key] => key,
            _ => {
                self.fail(unsupported(
                    self.file,
                    span,
                    "binding a per-iteration address in an index scan",
                ));
                return Flow::Fallthrough;
            }
        };
        let Some(bound) = bound else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "this index scan is unbounded. A `for` head over a managed index is always \
                 bounded and states its overflow behavior. Add `at most N` and an \
                 `on more { … }` block."
                    .to_string(),
            ));
            return Flow::Fallthrough;
        };
        let Some(on_more) = &bound.on_more else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "this bounded scan has no overflow arm. A bounded `for` head states its \
                 overflow behavior in a trailing `on more` block. Add an `on more { … }` block."
                    .to_string(),
            ));
            return Flow::Fallthrough;
        };
        if bound.from.is_some() {
            self.fail(unsupported(
                self.file,
                span,
                "a `from` cursor on an index scan",
            ));
            return Flow::Fallthrough;
        }
        // The scan yields a whole source identity, so the root's identity is a single key
        // column; the scanned (trailing) projection component is that key.
        let Some(root) = self.durable.root() else {
            let diagnostic =
                self.no_executable_root_diagnostic(span, "an index scan without a store");
            self.fail(diagnostic);
            return Flow::Fallthrough;
        };
        if root.key.len() != 1 {
            self.fail(unsupported(
                self.file,
                span,
                "an index scan over a composite-identity root",
            ));
            return Flow::Fallthrough;
        }
        let id_scalar = root.key[0];
        let site = index.site;
        // The held prefix is every projection component except the trailing identity key.
        let projection: Vec<ScalarType> = index.projection.clone();
        let Some((scanned, prefix_types)) = projection.split_last() else {
            self.fail(unsupported(self.file, span, "a scan over an empty index"));
            return Flow::Fallthrough;
        };
        if *scanned != id_scalar {
            self.fail(unsupported(
                self.file,
                span,
                "an index scan whose trailing component is not the source identity",
            ));
            return Flow::Fallthrough;
        }
        if keys.len() != prefix_types.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "index scan of `{}` holds its {} leading field component(s) as a prefix",
                    index.name,
                    prefix_types.len()
                ),
            ));
            return Flow::Fallthrough;
        }
        let Some(limit) = self.traversal_limit(&bound.limit) else {
            return Flow::Fallthrough;
        };
        // The frozen keys are the raw identity scalars; they materialize as `List[K]`.
        let list_ty = self
            .records
            .instantiate_list(self.draft, GArg::Scalar(id_scalar));
        // Emit the held prefix (leading field components, in projection order), then scan.
        for (key, key_ty) in keys.iter().zip(prefix_types) {
            if self.lower_as(key, LTy::bare_scalar(*key_ty)).is_none() {
                return Flow::Fallthrough;
            }
        }
        self.push(
            Instr::DurIndexScan {
                site,
                limit,
                from: false,
                list_ty,
            },
            span,
        );
        let more_slot = self.alloc_slot();
        self.push(Instr::LocalSet(more_slot), span);
        let coll_slot = self.alloc_slot();
        self.push(Instr::LocalSet(coll_slot), span);

        // A positional walk over the frozen `List[K]`: each raw identity key is wrapped
        // into the source `Id(^root)` the loop variable binds.
        let key_name = var.name.clone();
        let break_jumps = self.lower_positional_walk(
            coll_slot,
            Instr::ListLen,
            body,
            span,
            move |lower, index_slot| {
                let id_slot = lower.alloc_slot();
                lower.push(Instr::LocalGet(coll_slot), span);
                lower.push(Instr::LocalGet(index_slot), span);
                lower.push(Instr::ListGet, span);
                lower.push(Instr::MakeIdentity { root: 0, cols: 1 }, span);
                lower.push(Instr::LocalSet(id_slot), span);
                lower.locals.push(Local {
                    name: key_name,
                    ty: LTy::Identity {
                        root: 0,
                        optional: false,
                    },
                    mutable: false,
                    slot: id_slot,
                });
            },
        );

        self.push(Instr::LocalGet(more_slot), span);
        let skip_on_more = self.push_jif(span);
        self.lower_block(on_more);
        let end = self.here();
        self.patch(skip_on_more, end);
        self.patch_all(break_jumps, end);
        Flow::Fallthrough
    }

    /// Evaluate an `at most N` bound: a positive compile-time integer literal within
    /// `MAX_TRAVERSAL_BOUND`. A non-literal, non-positive, or oversized bound is a
    /// precise diagnostic.
    fn traversal_limit(&mut self, expr: &Expression) -> Option<u32> {
        let Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            span,
        } = expr
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                expr.span(),
                "`at most` requires a positive integer literal".to_string(),
            ));
            return None;
        };
        let value = parse_int(text).filter(|value| *value > 0);
        let Some(value) = value else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                *span,
                "`at most N` requires a positive integer literal".to_string(),
            ));
            return None;
        };
        if value as u128 > u128::from(marrow_image::bounds::MAX_TRAVERSAL_BOUND) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                *span,
                format!(
                    "`at most N` may not exceed {}",
                    marrow_image::bounds::MAX_TRAVERSAL_BOUND
                ),
            ));
            return None;
        }
        Some(value as u32)
    }

    /// Lower `for x in list` / `for k in map` / `for k, v in map`: a forward
    /// positional walk over a finite collection. A list yields elements in insertion
    /// order; a map yields keys (and values) in `CollectionKeyOrder`. The collection
    /// is evaluated once into a local, then indexed `0..length`; `continue` advances
    /// to the next position, `break` exits.
    fn lower_for_collection(
        &mut self,
        binding: &marrow_syntax::ForBinding,
        iterable: &Expression,
        body: &Block,
        span: SourceSpan,
    ) -> Flow {
        let Some(coll_ty) = self.lower_expr(iterable) else {
            return Flow::Fallthrough;
        };
        let LTy::Collection {
            idx,
            optional: false,
        } = coll_ty
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                iterable.span(),
                format!(
                    "a `for` loop iterates a list, map, or store, found {}",
                    coll_ty.spelling(self.records)
                ),
            ));
            return Flow::Fallthrough;
        };

        // The loop variables and the per-position bind instructions, resolved from
        // the collection kind and binding arity.
        enum Bind {
            List { elem: LTy },
            MapKey { key: LTy },
            MapKeyValue { key: LTy, value: LTy },
        }
        let bind = match (self.records.collection_spec(idx), binding.names.as_slice()) {
            (CollSpec::List { elem }, [_var]) => Bind::List {
                elem: garg_to_lty(elem),
            },
            (CollSpec::Map { key, .. }, [_k]) => Bind::MapKey {
                key: garg_to_lty(key),
            },
            (CollSpec::Map { key, value }, [_k, _v]) => Bind::MapKeyValue {
                key: garg_to_lty(key),
                value: garg_to_lty(value),
            },
            (CollSpec::List { .. }, _) => {
                self.fail(unsupported(
                    self.file,
                    span,
                    "a list `for` binds exactly one element name",
                ));
                return Flow::Fallthrough;
            }
            (CollSpec::Map { .. }, _) => {
                self.fail(unsupported(
                    self.file,
                    span,
                    "a map `for` binds a key or a key and a value (`for k, v`)",
                ));
                return Flow::Fallthrough;
            }
        };

        // The collection value is on the stack; keep it in a local to index it.
        let coll_slot = self.alloc_slot();
        self.push(Instr::LocalSet(coll_slot), span);
        let len_instr = match self.records.collection_spec(idx) {
            CollSpec::List { .. } => Instr::ListLen,
            CollSpec::Map { .. } => Instr::MapLen,
        };

        let break_jumps =
            self.lower_positional_walk(coll_slot, len_instr, body, span, |lower, index_slot| {
                // Bind the loop variable(s) from the current position.
                let bind_var = |lower: &mut Self, name: &str, ty: LTy, at: Instr| {
                    let slot = lower.alloc_slot();
                    lower.push(Instr::LocalGet(coll_slot), span);
                    lower.push(Instr::LocalGet(index_slot), span);
                    lower.push(at, span);
                    lower.push(Instr::LocalSet(slot), span);
                    lower.locals.push(Local {
                        name: name.to_string(),
                        ty,
                        mutable: false,
                        slot,
                    });
                };
                match bind {
                    Bind::List { elem } => {
                        bind_var(lower, &binding.names[0].name, elem, Instr::ListGet);
                    }
                    Bind::MapKey { key } => {
                        bind_var(lower, &binding.names[0].name, key, Instr::MapKeyAt);
                    }
                    Bind::MapKeyValue { key, value } => {
                        bind_var(lower, &binding.names[0].name, key, Instr::MapKeyAt);
                        bind_var(lower, &binding.names[1].name, value, Instr::MapValueAt);
                    }
                }
            });
        self.patch_all(break_jumps, self.here());
        Flow::Fallthrough
    }

    /// Lower a forward positional walk over a finite collection already resident in
    /// `coll_slot`. A `-1` cursor is incremented at the loop top, then an
    /// `index < len` guard (`len_instr` is the collection kind's length opcode)
    /// exits the loop; on each live position `bind` binds the loop variable(s) from
    /// the current index and pushes their [`Local`]s, then the body runs once.
    ///
    /// `continue` targets the increment at the loop top; the exhaustion exit is
    /// patched to fall through immediately after the loop, and the returned break
    /// jumps are left unpatched so the caller can route them past whatever trailing
    /// code it emits (a bounded traversal skips them past its `on more` block; a
    /// plain collection walk patches them to the same fall-through point).
    fn lower_positional_walk(
        &mut self,
        coll_slot: u16,
        len_instr: Instr,
        body: &Block,
        span: SourceSpan,
        bind: impl FnOnce(&mut Self, u16),
    ) -> Vec<usize> {
        // The cursor starts at -1 so the increment at the loop top reaches 0 first,
        // which lets `continue` jump to that increment and always make progress.
        let index_slot = self.alloc_slot();
        let neg_one = self.draft.intern_int(-1);
        self.push(Instr::ConstLoad(neg_one.index()), span);
        self.push(Instr::LocalSet(index_slot), span);
        let one = self.draft.intern_int(1);

        let top = self.here();
        // index += 1
        self.push(Instr::LocalGet(index_slot), span);
        self.push(Instr::ConstLoad(one.index()), span);
        self.push(Instr::IntAdd, span);
        self.push(Instr::LocalSet(index_slot), span);
        // index < length
        self.push(Instr::LocalGet(index_slot), span);
        self.push(Instr::LocalGet(coll_slot), span);
        self.push(len_instr, span);
        self.push(Instr::IntLt, span);
        let exit = self.push_jif(span);

        let mark = self.locals.len();
        let place_mark = self.places.len();
        bind(self, index_slot);
        self.loops.push(LoopCtx {
            continue_target: top,
            break_jumps: Vec::new(),
        });
        let body_flow = self.lower_block(body);
        if body_flow == Flow::Fallthrough {
            self.push(Instr::Jump(top as u32), body.span);
        }
        let ctx = self.loops.pop().expect("loop was pushed");
        self.locals.truncate(mark);
        // A two-binding durable traversal binds a per-iteration address pin as a place;
        // drop it with the loop-variable locals so it does not escape the body.
        self.places.truncate(place_mark);

        let after_loop = self.here();
        self.patch(exit, after_loop);
        ctx.break_jumps
    }

    fn lower_while(&mut self, condition: &Expression, body: &Block) -> Flow {
        let top = self.here();
        if self.lower_condition(condition).is_none() {
            return Flow::Fallthrough;
        }
        let exit = self.push_jif(condition.span());
        self.loops.push(LoopCtx {
            continue_target: top,
            break_jumps: Vec::new(),
        });
        let body_flow = self.lower_block(body);
        if body_flow == Flow::Fallthrough {
            self.push(Instr::Jump(top as u32), body.span);
        }
        let ctx = self.loops.pop().expect("loop was pushed");
        let end = self.here();
        self.patch(exit, end);
        self.patch_all(ctx.break_jumps, end);
        Flow::Fallthrough
    }

    /// Lower the adjacent single-operation checked-arithmetic form. It wraps one int
    /// arithmetic operation; on a fault the diverging `on` arms run instead of the
    /// runtime raising `run.*`. Lowered to a checked op that branches to the
    /// out-of-range handler, with the zero divisor tested by an explicit branch
    /// before a checked `/`/`%`. The operands are evaluated into fresh locals so the
    /// checked op runs with exactly its two operands on the stack, leaving the fault
    /// edge at the statement-boundary (empty) stack.
    fn lower_checked(
        &mut self,
        bind: &CheckedBind,
        op: &Expression,
        out_of_range: Option<&Block>,
        zero_divisor: Option<&Block>,
        span: SourceSpan,
    ) -> Flow {
        // The wrapped operation: a single int `+`/`-`/`*`/`/`/`%` or negation.
        enum Wrapped<'e> {
            Binary(BinaryOp, &'e Expression, &'e Expression),
            Neg(&'e Expression),
        }
        let wrapped = match op {
            Expression::Binary {
                op:
                    bop @ (BinaryOp::Add
                    | BinaryOp::Subtract
                    | BinaryOp::Multiply
                    | BinaryOp::Divide
                    | BinaryOp::Remainder),
                left,
                right,
                ..
            } => Wrapped::Binary(*bop, left, right),
            Expression::Unary {
                op: UnaryOp::Neg,
                operand,
                ..
            } => Wrapped::Neg(operand),
            _ => {
                self.fail(unsupported(
                    self.file,
                    op.span(),
                    "a checked form wrapping anything but one int `+`, `-`, `*`, `/`, `%`, or negation",
                ));
                return Flow::Fallthrough;
            }
        };
        let is_div = matches!(
            wrapped,
            Wrapped::Binary(BinaryOp::Divide | BinaryOp::Remainder, _, _)
        );

        // Arm requirements: out_of_range is always possible; a zero divisor is
        // possible for `/`/`%` and never for the others.
        let Some(out_of_range) = out_of_range else {
            self.fail(checked_arm_error(
                self.file,
                span,
                "requires an `on out_of_range` arm",
            ));
            return Flow::Fallthrough;
        };
        if is_div && zero_divisor.is_none() {
            self.fail(checked_arm_error(
                self.file,
                span,
                "a checked `/` or `%` requires an `on zero_divisor` arm",
            ));
            return Flow::Fallthrough;
        }
        if !is_div && zero_divisor.is_some() {
            self.fail(checked_arm_error(
                self.file,
                span,
                "this checked operation cannot fault with a zero divisor, so it takes no `on zero_divisor` arm",
            ));
            return Flow::Fallthrough;
        }

        let int = LTy::bare_scalar(ScalarType::Int);
        // Evaluate the operands into fresh locals.
        let la = self.alloc_slot();
        let (checked, lb) = match wrapped {
            Wrapped::Binary(bop, left, right) => {
                if self.lower_as(left, int).is_none() {
                    return Flow::Fallthrough;
                }
                self.push(Instr::LocalSet(la), span);
                let lb = self.alloc_slot();
                if self.lower_as(right, int).is_none() {
                    return Flow::Fallthrough;
                }
                self.push(Instr::LocalSet(lb), span);
                let checked = match bop {
                    BinaryOp::Add => Instr::IntAddChecked(0),
                    BinaryOp::Subtract => Instr::IntSubChecked(0),
                    BinaryOp::Multiply => Instr::IntMulChecked(0),
                    BinaryOp::Divide => Instr::IntDivChecked(0),
                    BinaryOp::Remainder => Instr::IntRemChecked(0),
                    _ => unreachable!("classified as an admitted binary op"),
                };
                (checked, Some(lb))
            }
            Wrapped::Neg(operand) => {
                if self.lower_as(operand, int).is_none() {
                    return Flow::Fallthrough;
                }
                self.push(Instr::LocalSet(la), span);
                (Instr::IntNegChecked(0), None)
            }
        };

        // A checked `/`/`%` tests its divisor first; a zero divisor runs the diverging
        // `on zero_divisor` arm.
        if is_div {
            let lb = lb.expect("division has a right operand");
            self.push(Instr::LocalGet(lb), span);
            let zero = self.draft.intern_int(0);
            self.push(Instr::ConstLoad(zero.index()), span);
            self.push(Instr::EqInt, span);
            let to_nonzero = self.push_jif(span);
            let zero_block = zero_divisor.expect("checked division requires the arm");
            if self.lower_block(zero_block) != Flow::Terminates {
                self.fail(checked_arm_error(
                    self.file,
                    zero_block.span,
                    "an `on zero_divisor` arm must diverge (every path must return, break, continue, throw, or be unreachable)",
                ));
            }
            let nonzero = self.here();
            self.patch(to_nonzero, nonzero);
        }

        // The checked operation. On the fault edge it transfers to the handler with
        // the operands already popped (the statement-boundary stack); on success it
        // pushes the int result.
        self.push(Instr::LocalGet(la), span);
        if let Some(lb) = lb {
            self.push(Instr::LocalGet(lb), span);
        }
        let checked_at = self.here();
        self.push(checked, span);

        // Success path: coerce the int result to the binding and store it. A
        // `const`/`var` binding (`pending` is `Some`) falls through and jumps over the
        // handler; a `return` binding leaves the frame, so no skip jump is needed.
        let pending = self.store_checked_result(bind, span);
        let end_jump = pending.is_some().then(|| self.push_jump(span));

        // The out-of-range handler.
        let handler = self.here();
        self.patch(checked_at, handler);
        if self.lower_block(out_of_range) != Flow::Terminates {
            self.fail(checked_arm_error(
                self.file,
                out_of_range.span,
                "an `on out_of_range` arm must diverge (every path must return, break, continue, throw, or be unreachable)",
            ));
        }

        // The binding is in scope only after the whole form, on the success path.
        if let Some(end_jump) = end_jump {
            let end = self.here();
            self.patch(end_jump, end);
        }
        if let Some(local) = pending {
            self.locals.push(local);
            Flow::Fallthrough
        } else {
            // A `return` binding leaves the frame on the success path; the arms
            // diverge, so the whole form terminates.
            Flow::Terminates
        }
    }

    /// Emit the store of a checked form's int result into its binding, on the success
    /// path. Returns the local to bring into scope *after* the handler (for
    /// `const`/`var`, so the name is not visible inside the arms), or `None` for a
    /// `return` binding (which stores by returning).
    fn store_checked_result(&mut self, bind: &CheckedBind, span: SourceSpan) -> Option<Local> {
        let int = LTy::bare_scalar(ScalarType::Int);
        match bind {
            CheckedBind::Const { name, ty } | CheckedBind::Var { name, ty } => {
                let mutable = matches!(bind, CheckedBind::Var { .. });
                let target = self.coerce_int_result(ty.as_ref(), int, span);
                let slot = self.alloc_slot();
                self.push(Instr::LocalSet(slot), span);
                Some(Local {
                    name: name.clone(),
                    ty: target,
                    mutable,
                    slot,
                })
            }
            CheckedBind::Return => {
                match self.ret {
                    RetType::Value(want) => {
                        self.coerce_bare_int_to(want, span, span);
                        self.push(Instr::Return, span);
                    }
                    RetType::Unit => {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            span,
                            "this function returns nothing, so it cannot `return checked`"
                                .to_string(),
                        ));
                    }
                }
                None
            }
        }
    }

    /// Coerce the bare-int checked result to a `const`/`var` annotation (`int` or
    /// `int?`), emitting a `SomeWrap` for the optional case. An annotation that is not
    /// int-compatible is a type error; a missing annotation infers `int`.
    fn coerce_int_result(
        &mut self,
        annotation: Option<&TypeExpr>,
        int: LTy,
        span: SourceSpan,
    ) -> LTy {
        let Some(annotation) = annotation else {
            return int;
        };
        let Some(declared) = self.resolve(annotation) else {
            self.fail(unsupported(
                self.file,
                annotation.span(),
                "this type annotation",
            ));
            return int;
        };
        self.coerce_bare_int_to(declared, annotation.span(), span);
        declared
    }

    /// Coerce the bare-int result already on the stack to `target` (`int` or `int?`),
    /// emitting a `SomeWrap` for the optional case. A `target` that is not
    /// int-compatible is a type error reported at `err_span`. One owner for the two
    /// checked-result binding sites (`const`/`var` annotation and `return`).
    fn coerce_bare_int_to(&mut self, target: LTy, err_span: SourceSpan, wrap_span: SourceSpan) {
        let int = LTy::bare_scalar(ScalarType::Int);
        if target == int.to_optional() {
            self.push(Instr::SomeWrap, wrap_span);
        } else if target != int {
            self.fail(type_mismatch(
                self.records,
                self.file,
                err_span,
                int,
                target,
            ));
        }
    }

    fn lower_condition(&mut self, expr: &Expression) -> Option<()> {
        let ty = self.lower_expr(expr)?;
        if ty != LTy::bare_scalar(ScalarType::Bool) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                expr.span(),
                format!(
                    "condition must be bool, found {}",
                    ty.spelling(self.records)
                ),
            ));
            return None;
        }
        Some(())
    }

    // --- expressions ---

    /// Lower `expr`, emitting code that pushes its value, then coerce that value to
    /// exactly `expected` (bare-to-optional via `SomeWrap`; `absent` becomes a vacant
    /// optional). Reports a diagnostic and returns `None` on mismatch.
    fn lower_as(&mut self, expr: &Expression, expected: LTy) -> Option<()> {
        // A built-in constructor is directed by the expected type: it supplies the
        // exact `Option`/`Result` instantiation, so `none`/`some`/`ok`/`err` need no
        // annotation of their own here.
        if let Some(kind) = constructor_kind(expr) {
            return self.lower_ctor_as(kind, expr, expected);
        }
        // `List()` / `Map()` are empty-collection constructors directed by the
        // expected type, which supplies the exact instantiation.
        if let Some(head) = collection_ctor_call(expr) {
            return self.lower_collection_ctor(head, expr.span(), expected);
        }
        if let Expression::Absent { span } = expr {
            // `absent` supplies the vacant value of any optional type, including an
            // optional generic parameter (`T?`) in a template body; the image vacant
            // carries the expected optional's image shape.
            if !expected.is_optional() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    *span,
                    format!(
                        "`absent` needs an optional type, found {}",
                        expected.spelling(self.records)
                    ),
                ));
                return None;
            }
            self.push(Instr::VacantLoad(expected.image()), *span);
            return Some(());
        }
        let got = self.lower_expr(expr)?;
        if got == expected {
            return Some(());
        }
        if !got.is_optional() && expected.is_optional() && got.to_optional() == expected {
            self.push(Instr::SomeWrap, expr.span());
            return Some(());
        }
        self.fail(type_mismatch(
            self.records,
            self.file,
            expr.span(),
            got,
            expected,
        ));
        None
    }

    /// Lower `expr`, emitting code that pushes its value and returning its type.
    fn lower_expr(&mut self, expr: &Expression) -> Option<LTy> {
        // A read through a managed index `^root.index[keys]`: a unique index is an exact
        // complete-key lookup yielding the optional `Id(^root)`; a nonunique index is read
        // by scanning it with a `for` head, so naming one in value position is rejected.
        if let Some((index, keys)) = self.resolve_index_read(expr) {
            if index.unique {
                return self.lower_index_lookup(index, keys, expr.span());
            }
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                expr.span(),
                format!(
                    "read the non-unique index `{}` by scanning it with a `for` head, \
                     not as a value",
                    index.name
                ),
            ));
            return None;
        }
        // Inline `^root(key)` addresses and a field projected off a named `place`
        // read here; a bare place name is a durable designation, handled below.
        if self.durable_shape_here(expr).is_some() {
            let place = self.resolve_durable(expr)?;
            return self.lower_durable_read(place);
        }
        if let Expression::Field { base, .. } = expr
            && self.is_place_name(base)
        {
            let place = self.resolve_durable(expr)?;
            return self.lower_durable_read(place);
        }
        match expr {
            Expression::Literal { kind, text, span } => self.lower_literal(*kind, text, *span),
            Expression::Name { segments, span, .. } => match segments.as_slice() {
                // `none` is a reserved Option constructor; it needs an expected type
                // (an annotation, argument, return, or coerced position) to know its
                // instantiation, so a bare `none` in value position is a type error.
                [name] if name == "none" => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *span,
                        "the Option type of `none` cannot be inferred here; use it where an Option is expected".to_string(),
                    ));
                    None
                }
                [name] => {
                    if let Some(local) = self.lookup(name) {
                        let (slot, ty) = (local.slot, local.ty);
                        self.push(Instr::LocalGet(slot), *span);
                        return Some(ty);
                    }
                    // A place is a durable designation, not a first-class value:
                    // its bare name cannot be read, passed, or returned.
                    if self.lookup_place(name).is_some() {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            *span,
                            format!(
                                "`{name}` is a durable place, not a value; read a field with \
                                 `{name}.field`, guard the entry with `if const x = {name}`, \
                                 or test it with `exists({name})`"
                            ),
                        ));
                        return None;
                    }
                    // A module-private constant, folded to a constant load. Locals
                    // and parameters shadow it (checked first).
                    if let Some(value) = self.consts.get(self.module, name).cloned() {
                        return Some(self.lower_const_value(&value, *span));
                    }
                    self.fail(name_error(self.file, *span, name));
                    None
                }
                // `Enum::member` for a payloadless member is an enum value.
                [enum_name, variant] if self.records.enum_by_name(enum_name).is_some() => {
                    self.lower_enum_construct(enum_name, variant, &[], *span)
                }
                _ => {
                    self.fail(unsupported(self.file, *span, "a qualified name"));
                    None
                }
            },
            Expression::Absent { span } => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    *span,
                    "the type of `absent` cannot be inferred here".to_string(),
                ));
                None
            }
            Expression::Unary { op, operand, span } => self.lower_unary(*op, operand, *span),
            Expression::Binary {
                op, left, right, ..
            } => self.lower_binary(*op, left, right),
            Expression::Call {
                callee, args, span, ..
            } => match self.lower_call_core(callee, args, *span)? {
                CallResult::Value(ty) => Some(ty),
                CallResult::Unit => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *span,
                        "this call returns nothing and has no value here".to_string(),
                    ));
                    None
                }
                CallResult::Diverges => {
                    // `unreachable` is a diverging statement, not a value; it is only
                    // valid in statement position.
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *span,
                        "`unreachable` is a statement and cannot be used as a value".to_string(),
                    ));
                    None
                }
            },
            Expression::Field {
                base, name, span, ..
            } => self.lower_field(base, name, *span),
            Expression::OptionalField {
                base, name, span, ..
            } => self.lower_optional_field(base, name, *span),
            Expression::Try { inner, span } => self.lower_try(inner, *span),
            Expression::Interpolation { parts, span } => self.lower_interpolation(parts, *span),
            other => {
                self.fail(unsupported(self.file, other.span(), "this expression"));
                None
            }
        }
    }

    /// Emit a folded module constant as a constant load of its scalar value.
    fn lower_const_value(&mut self, value: &ConstScalar, span: SourceSpan) -> LTy {
        let (scalar, const_id) = match value {
            ConstScalar::Int(value) => (ScalarType::Int, self.draft.intern_int(*value)),
            ConstScalar::Bool(value) => (ScalarType::Bool, self.draft.intern_bool(*value)),
            ConstScalar::Text(text) => (ScalarType::Text, self.draft.intern_text(text)),
        };
        self.push(Instr::ConstLoad(const_id.index()), span);
        LTy::bare_scalar(scalar)
    }

    fn lower_literal(&mut self, kind: LiteralKind, text: &str, span: SourceSpan) -> Option<LTy> {
        let (scalar, const_id) = match kind {
            LiteralKind::Integer => {
                let Some(value) = parse_int(text) else {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        "integer literal is out of the 64-bit range".to_string(),
                    ));
                    return None;
                };
                (ScalarType::Int, self.draft.intern_int(value))
            }
            LiteralKind::Bool => (ScalarType::Bool, self.draft.intern_bool(text == "true")),
            LiteralKind::String => {
                let Ok(decoded) = decode_string_literal(text) else {
                    self.fail(unsupported(self.file, span, "this string literal"));
                    return None;
                };
                (ScalarType::Text, self.draft.intern_text(&decoded))
            }
            // The prototype's `1.second` duration-suffix literal is not in the beta
            // floor: a duration is constructed from a canonical text literal. Point
            // at the constructor rather than reporting a generic unsupported literal.
            LiteralKind::Duration => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckUnsupported.as_str(),
                    self.file,
                    span,
                    "duration suffix literals are not supported; construct a duration \
                     from canonical text, e.g. `duration(\"PT1S\")`"
                        .to_string(),
                ));
                return None;
            }
            _ => {
                self.fail(unsupported(self.file, span, "this literal"));
                return None;
            }
        };
        self.push(Instr::ConstLoad(const_id.index()), span);
        Some(LTy::bare_scalar(scalar))
    }

    /// Lower an interpolated string `$"...{expr}..."` to a left-folded
    /// [`Instr::TextConcat`] over its parts. A literal text segment loads its
    /// decoded text; a hole renders its value through the same closed conversions
    /// `string(int)`/`string(bool)` expose, so an interpolable hole is a `string`,
    /// `int`, or `bool`. The whole expression is a `string`, and an empty
    /// interpolation is the empty string.
    fn lower_interpolation(
        &mut self,
        parts: &[InterpolationPart],
        span: SourceSpan,
    ) -> Option<LTy> {
        let mut pushed = false;
        let mut ok = true;
        for part in parts {
            let part_ok = self.lower_interpolation_part(part);
            ok &= part_ok;
            if part_ok {
                if pushed {
                    self.push(Instr::TextConcat, span);
                } else {
                    pushed = true;
                }
            }
        }
        if !ok {
            return None;
        }
        if !pushed {
            let empty = self.draft.intern_text("");
            self.push(Instr::ConstLoad(empty.index()), span);
        }
        Some(LTy::bare_scalar(ScalarType::Text))
    }

    /// Push one interpolation part as a `string` value; return whether it lowered
    /// cleanly (a failed part has already reported its diagnostic).
    fn lower_interpolation_part(&mut self, part: &InterpolationPart) -> bool {
        match part {
            InterpolationPart::Text { text, span } => {
                let Ok(decoded) = decode_interpolation_text(text) else {
                    self.fail(unsupported(self.file, *span, "this interpolation text"));
                    return false;
                };
                let const_id = self.draft.intern_text(&decoded);
                self.push(Instr::ConstLoad(const_id.index()), *span);
                true
            }
            InterpolationPart::Expr(expr) => {
                let Some(ty) = self.lower_expr(expr) else {
                    return false;
                };
                match ty.bare_scalar_type() {
                    Some(ScalarType::Text) => true,
                    Some(ScalarType::Int) => {
                        self.push(Instr::ConvStringInt, expr.span());
                        true
                    }
                    Some(ScalarType::Bool) => {
                        self.push(Instr::ConvStringBool, expr.span());
                        true
                    }
                    _ => {
                        self.fail(unsupported(
                            self.file,
                            expr.span(),
                            &format!("interpolating a {} value", ty.spelling(self.records)),
                        ));
                        false
                    }
                }
            }
        }
    }

    fn lower_unary(&mut self, op: UnaryOp, operand: &Expression, span: SourceSpan) -> Option<LTy> {
        let ty = self.lower_expr(operand)?;
        match op {
            UnaryOp::Neg => {
                if ty != LTy::bare_scalar(ScalarType::Int) {
                    self.fail(unary_error(self.records, self.file, span, "negate", ty));
                    return None;
                }
                self.push(Instr::IntNeg, span);
                Some(LTy::bare_scalar(ScalarType::Int))
            }
            UnaryOp::Not => {
                if ty != LTy::bare_scalar(ScalarType::Bool) {
                    self.fail(unary_error(
                        self.records,
                        self.file,
                        span,
                        "apply `not` to",
                        ty,
                    ));
                    return None;
                }
                self.push(Instr::BoolNot, span);
                Some(LTy::bare_scalar(ScalarType::Bool))
            }
        }
    }

    fn lower_binary(&mut self, op: BinaryOp, left: &Expression, right: &Expression) -> Option<LTy> {
        match op {
            BinaryOp::And | BinaryOp::Or => self.lower_short_circuit(op, left, right),
            BinaryOp::Coalesce => self.lower_coalesce(left, right),
            _ => {
                let left_ty = self.lower_expr(left)?;
                self.lower_binary_op(op, left_ty, right)
            }
        }
    }

    /// Lower the right operand and the arithmetic/comparison operator, given the left
    /// operand's already-pushed type. Both operands must be bare scalars or bare
    /// nominals; a nominal operand routes to the capability-gated nominal table.
    fn lower_binary_op(&mut self, op: BinaryOp, left_ty: LTy, right: &Expression) -> Option<LTy> {
        // The `step` capability admits only the literal `1`, so the right operand's
        // shape is read before it is lowered.
        let right_is_one = matches!(
            right,
            Expression::Literal {
                kind: LiteralKind::Integer,
                text,
                ..
            } if parse_int(text) == Some(1)
        );
        let right_ty = self.lower_expr(right)?;
        let span = right.span();
        // An abstract type parameter (template pass only) admits `==`/`!=` when it
        // supports equality and `<`/`<=`/`>`/`>=` when it supports order; every other
        // operator over it is rejected. An unconstrained parameter admits neither, so
        // it falls through to the standard operator error.
        if left_ty.bare_param().is_some() || right_ty.bare_param().is_some() {
            return self.lower_param_binary(op, left_ty, right_ty, span);
        }
        if left_ty.bare_nominal().is_some() || right_ty.bare_nominal().is_some() {
            return self.lower_nominal_binary(op, left_ty, right_ty, right_is_one, span);
        }
        if left_ty.bare_enum().is_some() || right_ty.bare_enum().is_some() {
            return self.lower_enum_binary(op, left_ty, right_ty, span);
        }
        if left_ty.bare_identity().is_some() || right_ty.bare_identity().is_some() {
            return self.lower_identity_binary(op, left_ty, right_ty, span);
        }
        let (Some(left), Some(right_scalar)) =
            (left_ty.bare_scalar_type(), right_ty.bare_scalar_type())
        else {
            self.fail(binary_error(
                self.records,
                self.file,
                span,
                op,
                left_ty,
                right_ty,
            ));
            return None;
        };
        use ScalarType::{Bool, Bytes, Date, Duration, Instant, Int, Text};
        let (instr, result): (Instr, ScalarType) = match (op, left, right_scalar) {
            (BinaryOp::Add, Int, Int) => (Instr::IntAdd, Int),
            (BinaryOp::Add, Text, Text) => (Instr::TextConcat, Text),
            (BinaryOp::Subtract, Int, Int) => (Instr::IntSub, Int),
            (BinaryOp::Multiply, Int, Int) => (Instr::IntMul, Int),
            (BinaryOp::Remainder, Int, Int) => (Instr::IntRem, Int),
            (BinaryOp::Divide, Int, Int) => (Instr::IntDiv, Int),
            (op, Int, Int) if int_comparison(op).is_some() => {
                (int_comparison(op).expect("guard matched"), Bool)
            }
            (BinaryOp::Less, Text, Text) => (Instr::TextLt, Bool),
            (BinaryOp::LessEqual, Text, Text) => (Instr::TextLe, Bool),
            (BinaryOp::Greater, Text, Text) => (Instr::TextGt, Bool),
            (BinaryOp::GreaterEqual, Text, Text) => (Instr::TextGe, Bool),
            (BinaryOp::Less, Bytes, Bytes) => (Instr::BytesLt, Bool),
            (BinaryOp::LessEqual, Bytes, Bytes) => (Instr::BytesLe, Bool),
            (BinaryOp::Greater, Bytes, Bytes) => (Instr::BytesGt, Bool),
            (BinaryOp::GreaterEqual, Bytes, Bytes) => (Instr::BytesGe, Bool),
            // Temporal order (same-type only). The closed arithmetic floor: a
            // duration sums/differences with a duration, and a duration shifts an
            // instant; there is no `date +/- int` operator (use `addDays`), no
            // `duration * int`, and no calendar-month arithmetic.
            (op, Date, Date) if temporal_comparison(op).is_some() => {
                (date_comparison(op).expect("guard matched"), Bool)
            }
            (op, Instant, Instant) if temporal_comparison(op).is_some() => {
                (instant_comparison(op).expect("guard matched"), Bool)
            }
            (op, Duration, Duration) if temporal_comparison(op).is_some() => {
                (duration_comparison(op).expect("guard matched"), Bool)
            }
            (BinaryOp::Add, Duration, Duration) => (Instr::DurationAdd, Duration),
            (BinaryOp::Subtract, Duration, Duration) => (Instr::DurationSub, Duration),
            (BinaryOp::Add, Instant, Duration) => (Instr::InstantAddDuration, Instant),
            (BinaryOp::Subtract, Instant, Duration) => (Instr::InstantSubDuration, Instant),
            (BinaryOp::Equal, a, b) if a == b => (eq_instr(a), Bool),
            (BinaryOp::NotEqual, a, b) if a == b => {
                self.push(eq_instr(a), span);
                self.push(Instr::BoolNot, span);
                return Some(LTy::bare_scalar(Bool));
            }
            _ => {
                self.fail(binary_error(
                    self.records,
                    self.file,
                    span,
                    op,
                    left_ty,
                    right_ty,
                ));
                return None;
            }
        };
        self.push(instr, span);
        Some(LTy::bare_scalar(result))
    }

    /// Lower a binary operator with a bare nominal operand. The capability table
    /// (documented in `docs/language/types-and-values.md`):
    ///
    /// - comparisons between two values of the same nominal are always admitted
    ///   (they construct nothing);
    /// - `add`: `N + int` and `int + N`, guarded to `N`;
    /// - `subtract`: `N - int` guarded to `N`; `N - N` to plain `int`, unguarded
    ///   (a difference is a count, not a value of the type);
    /// - `scale`: `N * int` and `int * N`, guarded to `N`;
    /// - `step`: `N + 1` and `N - 1` (the int literal `1`), guarded to `N`.
    ///
    /// Every operator that produces a nominal value re-guards the result, so no
    /// path constructs an out-of-interval value. A missing capability is a typed
    /// diagnostic naming it.
    fn lower_nominal_binary(
        &mut self,
        op: BinaryOp,
        left_ty: LTy,
        right_ty: LTy,
        right_is_one: bool,
        span: SourceSpan,
    ) -> Option<LTy> {
        let bool_ty = LTy::bare_scalar(ScalarType::Bool);
        let int_ty = LTy::bare_scalar(ScalarType::Int);
        let same_nominal = left_ty.bare_nominal().is_some() && left_ty == right_ty;
        if same_nominal {
            if let Some(instr) = int_comparison(op) {
                self.push(instr, span);
                return Some(bool_ty);
            }
            match op {
                BinaryOp::Equal => {
                    self.push(eq_instr(ScalarType::Int), span);
                    return Some(bool_ty);
                }
                BinaryOp::NotEqual => {
                    self.push(eq_instr(ScalarType::Int), span);
                    self.push(Instr::BoolNot, span);
                    return Some(bool_ty);
                }
                BinaryOp::Subtract => {
                    return if self.nominal_supports(left_ty).subtract {
                        self.push(Instr::IntSub, span);
                        Some(int_ty)
                    } else {
                        self.fail_missing_capability(left_ty, "subtract", op, span);
                        None
                    };
                }
                _ => {
                    self.fail(binary_error(
                        self.records,
                        self.file,
                        span,
                        op,
                        left_ty,
                        right_ty,
                    ));
                    return None;
                }
            }
        }
        // Mixed nominal/int arithmetic; the result is the nominal, re-guarded.
        let (nominal, nominal_on_left) = match (left_ty.bare_nominal(), right_ty.bare_nominal()) {
            (Some(_), None) if right_ty == int_ty => (left_ty, true),
            (None, Some(_)) if left_ty == int_ty => (right_ty, false),
            _ => {
                self.fail(binary_error(
                    self.records,
                    self.file,
                    span,
                    op,
                    left_ty,
                    right_ty,
                ));
                return None;
            }
        };
        let supports = self.nominal_supports(nominal);
        let stepped = supports.step && nominal_on_left && right_is_one;
        let instr = match op {
            BinaryOp::Add if supports.add || stepped => Instr::IntAdd,
            BinaryOp::Subtract if nominal_on_left && (supports.subtract || stepped) => {
                Instr::IntSub
            }
            BinaryOp::Multiply if supports.scale => Instr::IntMul,
            BinaryOp::Add => {
                self.fail_missing_capability(nominal, "add", op, span);
                return None;
            }
            BinaryOp::Subtract if nominal_on_left => {
                self.fail_missing_capability(nominal, "subtract", op, span);
                return None;
            }
            BinaryOp::Multiply => {
                self.fail_missing_capability(nominal, "scale", op, span);
                return None;
            }
            _ => {
                self.fail(binary_error(
                    self.records,
                    self.file,
                    span,
                    op,
                    left_ty,
                    right_ty,
                ));
                return None;
            }
        };
        self.push(instr, span);
        let id = nominal.bare_nominal().expect("classified as a nominal");
        let info = self.records.nominal(id);
        self.push(
            Instr::RangeGuard {
                lo: info.lo,
                hi: info.hi,
            },
            span,
        );
        Some(nominal)
    }

    /// Lower `==`/`!=` on two values of the same enum to `EqEnum` (exact variant
    /// and payload equality). Any other operator, or two different enums, is a
    /// typed diagnostic — an enum has no ordering.
    fn lower_enum_binary(
        &mut self,
        op: BinaryOp,
        left_ty: LTy,
        right_ty: LTy,
        span: SourceSpan,
    ) -> Option<LTy> {
        let bool_ty = LTy::bare_scalar(ScalarType::Bool);
        let same_enum =
            left_ty.bare_enum().is_some() && left_ty.bare_enum() == right_ty.bare_enum();
        match op {
            BinaryOp::Equal if same_enum => {
                self.push(Instr::EqEnum, span);
                Some(bool_ty)
            }
            BinaryOp::NotEqual if same_enum => {
                self.push(Instr::EqEnum, span);
                self.push(Instr::BoolNot, span);
                Some(bool_ty)
            }
            _ => {
                self.fail(binary_error(
                    self.records,
                    self.file,
                    span,
                    op,
                    left_ty,
                    right_ty,
                ));
                None
            }
        }
    }

    /// Lower `==`/`!=` between two entry identities of the same store root — the only
    /// operators identities admit. Equality is key-tuple equality; a mismatched root
    /// (impossible with one declared root, but kept as the general rule) or any other
    /// operator is the standard binary error.
    fn lower_identity_binary(
        &mut self,
        op: BinaryOp,
        left_ty: LTy,
        right_ty: LTy,
        span: SourceSpan,
    ) -> Option<LTy> {
        let bool_ty = LTy::bare_scalar(ScalarType::Bool);
        let same_root = left_ty.bare_identity().is_some()
            && left_ty.bare_identity() == right_ty.bare_identity();
        match op {
            BinaryOp::Equal if same_root => {
                self.push(Instr::EqId, span);
                Some(bool_ty)
            }
            BinaryOp::NotEqual if same_root => {
                self.push(Instr::EqId, span);
                self.push(Instr::BoolNot, span);
                Some(bool_ty)
            }
            _ => {
                self.fail(binary_error(
                    self.records,
                    self.file,
                    span,
                    op,
                    left_ty,
                    right_ty,
                ));
                None
            }
        }
    }

    /// Lower `==`/`!=` and the ordering operators over an abstract type parameter,
    /// reached only in the template pass. Both operands must be the same type
    /// parameter (two distinct parameters are distinct opaque types). Equality is
    /// admitted when the parameter's constraint licenses it (`supports equality`, or
    /// `supports order`, which subsumes equality); ordering requires `supports
    /// order`. Any other operator, an unconstrained parameter, or a mismatch is the
    /// standard operator error. The emitted instruction is a stack-shape placeholder:
    /// the template pass discards its code, and a monomorphized instance re-lowers
    /// the body over the concrete type, emitting the real comparison.
    fn lower_param_binary(
        &mut self,
        op: BinaryOp,
        left_ty: LTy,
        right_ty: LTy,
        span: SourceSpan,
    ) -> Option<LTy> {
        let bool_ty = LTy::bare_scalar(ScalarType::Bool);
        let same_param = left_ty.bare_param().is_some() && left_ty == right_ty;
        let constraint = left_ty
            .bare_param()
            .and_then(|index| self.type_param_constraint(index));
        let admitted = match op {
            BinaryOp::Equal | BinaryOp::NotEqual => {
                constraint.is_some_and(TypeConstraint::admits_equality)
            }
            BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => {
                constraint.is_some_and(TypeConstraint::admits_order)
            }
            _ => false,
        };
        if same_param && admitted {
            // Placeholder with the right stack shape (pop two, push one bool); the
            // code is discarded by the template pass.
            self.push(Instr::EqInt, span);
            return Some(bool_ty);
        }
        if same_param {
            let want = match op {
                BinaryOp::Less
                | BinaryOp::LessEqual
                | BinaryOp::Greater
                | BinaryOp::GreaterEqual => "order",
                _ => "equality",
            };
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "operator `{}` needs the type parameter to `supports {want}`",
                    operator_symbol(op)
                ),
            ));
            return None;
        }
        self.fail(binary_error(
            self.records,
            self.file,
            span,
            op,
            left_ty,
            right_ty,
        ));
        None
    }

    /// The constraint on the abstract type parameter at `index`, in the template
    /// pass. `None` outside that pass or for an unconstrained parameter.
    fn type_param_constraint(&self, index: u16) -> Option<TypeConstraint> {
        let env = TypeEnv {
            params: &self.type_env,
        };
        env.constraint_at(index)
    }

    fn nominal_supports(&self, ty: LTy) -> SupportSet {
        let id = ty.bare_nominal().expect("caller classified a nominal");
        self.records.nominal(id).supports
    }

    fn fail_missing_capability(
        &mut self,
        ty: LTy,
        capability: &str,
        op: BinaryOp,
        span: SourceSpan,
    ) {
        let name = ty.spelling(self.records);
        self.fail(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            self.file,
            span,
            format!(
                "type `{name}` does not support `{capability}`, so `{}` is not defined for it",
                operator_symbol(op)
            ),
        ));
    }

    /// `left ?? right`: yield the present value of the optional `left`, else `right`.
    /// Lowered to the atomic present-branch (design §D), so no unchecked unwrap.
    fn lower_coalesce(&mut self, left: &Expression, right: &Expression) -> Option<LTy> {
        let left_ty = self.lower_expr(left)?;
        if !left_ty.is_optional() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                left.span(),
                format!(
                    "`??` needs an optional on the left, found {}",
                    left_ty.spelling(self.records)
                ),
            ));
            return None;
        }
        let bare = left_ty.to_bare();
        let bp = self.push_branch_present(left.span());
        let to_end = self.push_jump(left.span());
        let absent = self.here();
        self.patch(bp, absent);
        self.lower_as(right, bare)?;
        let end = self.here();
        self.patch(to_end, end);
        Some(bare)
    }

    fn lower_short_circuit(
        &mut self,
        op: BinaryOp,
        left: &Expression,
        right: &Expression,
    ) -> Option<LTy> {
        let bool_ty = LTy::bare_scalar(ScalarType::Bool);
        let left_ty = self.lower_expr(left)?;
        if left_ty != bool_ty {
            self.fail(logic_operand(
                self.records,
                self.file,
                left.span(),
                op,
                left_ty,
            ));
            return None;
        }
        match op {
            BinaryOp::And => {
                let jif = self.push_jif(left.span());
                let right_ty = self.lower_expr(right)?;
                if right_ty != bool_ty {
                    self.fail(logic_operand(
                        self.records,
                        self.file,
                        right.span(),
                        op,
                        right_ty,
                    ));
                    return None;
                }
                let to_end = self.push_jump(right.span());
                let false_at = self.here();
                self.patch(jif, false_at);
                let konst = self.draft.intern_bool(false);
                self.push(Instr::ConstLoad(konst.index()), left.span());
                let end = self.here();
                self.patch(to_end, end);
            }
            BinaryOp::Or => {
                let jif = self.push_jif(left.span());
                let konst = self.draft.intern_bool(true);
                self.push(Instr::ConstLoad(konst.index()), left.span());
                let to_end = self.push_jump(left.span());
                let rhs_at = self.here();
                self.patch(jif, rhs_at);
                let right_ty = self.lower_expr(right)?;
                if right_ty != bool_ty {
                    self.fail(logic_operand(
                        self.records,
                        self.file,
                        right.span(),
                        op,
                        right_ty,
                    ));
                    return None;
                }
                let end = self.here();
                self.patch(to_end, end);
            }
            _ => unreachable!("only and/or reach short-circuit lowering"),
        }
        Some(bool_ty)
    }

    /// A parenthesized application is a record constructor (`Note(title: t, ...)`)
    /// or a direct function call.
    fn lower_call_core(
        &mut self,
        callee: &Expression,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<CallResult> {
        let Expression::Name { segments, .. } = callee else {
            // `Age.checked(n)`: the nominal range test, the one member call the
            // subset admits. Any other field-shaped callee stays unsupported.
            if let Expression::Field { base, name, .. } = callee {
                if name == "checked"
                    && let Expression::Name { segments, .. } = &**base
                    && let [type_name] = segments.as_slice()
                    && let Some((id, _)) = self.records.nominal_by_name(type_name)
                {
                    return self
                        .lower_checked_nominal(id, args, span)
                        .map(CallResult::Value);
                }
                // `Resource.branch.…(field: value, …)`: a keyed branch entry constructor at
                // any depth, symmetric with the root constructor `Resource(field: value, …)`
                // and resolved through the one type-namespace owner (the store's resource and
                // its executable branch tree).
                if let Some((resource, head_span, mut path)) = split_dotted_head(base) {
                    path.push(name.as_str());
                    if let Some(branch) = self.executable_branch_path(resource, &path) {
                        let display = branch_ctor_display(resource, &path);
                        return self
                            .lower_branch_constructor(
                                resource, &display, branch, head_span, args, span,
                            )
                            .map(CallResult::Value);
                    }
                    // `Resource.group(field: value, …)`: a group value constructor,
                    // symmetric with the branch entry constructor one level down. A
                    // group is an unkeyed single-level namespace, so its qualified head
                    // is the resource then the group name.
                    if let [group_name] = path.as_slice()
                        && self
                            .records
                            .by_name(resource)
                            .is_some_and(|record| record.group(group_name).is_some())
                    {
                        return self
                            .lower_group_constructor(resource, group_name, head_span, args, span)
                            .map(CallResult::Value);
                    }
                }
                // A method-shaped call on a value: `s.trim()`. Member syntax reaches
                // fields and constructor paths only, so this is not a call the subset
                // admits; the teaching form is the free-function spelling of the same
                // call, written with the receiver as the first argument.
                self.fail(SourceDiagnostic::at(
                    Code::CheckUnsupported.as_str(),
                    self.file,
                    span,
                    format!(
                        "`{name}` is written as a method call on `{receiver}`. A value has no \
                         methods; an operation on a value is an ordinary function call. Write \
                         `{name}({receiver})`.",
                        receiver = marrow_syntax::format_expression(base)
                    ),
                ));
                return None;
            }
            self.fail(unsupported(self.file, span, "this call"));
            return None;
        };
        match segments.as_slice() {
            [name] => self.lower_unqualified_call(name, args, span),
            // `Enum::member(payload...)` constructs a payload-carrying enum value.
            [enum_name, item] if self.records.enum_by_name(enum_name).is_some() => self
                .lower_enum_construct(enum_name, item, args, span)
                .map(CallResult::Value),
            // A generic enum template's variant infers its instantiation from the
            // payload values.
            [enum_name, item]
                if self
                    .records
                    .type_template_by_name(enum_name)
                    .is_some_and(|t| self.records.template_is_enum(t)) =>
            {
                let template = self.records.type_template_by_name(enum_name).unwrap();
                self.lower_generic_enum_construct(template, item, args, span)
                    .map(CallResult::Value)
            }
            [prefix @ .., item] => self.lower_qualified_call(prefix, item, args, span),
            [] => {
                self.fail(unsupported(self.file, span, "this call"));
                None
            }
        }
    }

    /// An unqualified call: a builtin, a constructor, or a function in the same
    /// module. It never reaches another module — that requires a `::` qualifier.
    fn lower_unqualified_call(
        &mut self,
        name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<CallResult> {
        // The reserved built-ins are intercepted before any user resolution, so a
        // colliding declaration (rejected at its declaration site) can never reach
        // here. Dispatching on the shared classifier keeps interception and
        // declaration rejection reading the same fact.
        if let Some(builtin) = Builtin::from_name(name) {
            return match builtin {
                Builtin::Exists => self.lower_exists(args, span).map(CallResult::Value),
                Builtin::Unreachable => self.lower_unreachable(args, span),
                // `some(v)` infers its Option from `v`; `ok`/`err` cannot infer the
                // whole Result, so they need an expected type (an annotation,
                // argument, return, or coerced position).
                Builtin::Some => self.lower_some_infer(args, span).map(CallResult::Value),
                Builtin::Ok | Builtin::Err => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!(
                            "the Result type of `{name}` cannot be inferred here; use it where a Result is expected"
                        ),
                    ));
                    None
                }
                // `isEmpty` accepts a string or a collection; the other two are
                // text-only.
                Builtin::IsEmpty => self.lower_is_empty(args, span).map(CallResult::Value),
                Builtin::Contains | Builtin::Trim => self
                    .lower_text_builtin(name, args, span)
                    .map(CallResult::Value),
                // `split`/`lines` return a `List[string]`; `join` consumes one.
                Builtin::Split | Builtin::Lines => self
                    .lower_text_split(name, args, span)
                    .map(CallResult::Value),
                Builtin::Join => self.lower_text_join(args, span).map(CallResult::Value),
                Builtin::DateAddDays | Builtin::DateDaysBetween => self
                    .lower_date_arith(builtin, args, span)
                    .map(CallResult::Value),
                // `List()`/`Map()` are the empty-collection constructors; they infer
                // nothing on their own, so they need an expected List/Map type (an
                // annotation, argument, return, or coerced position).
                Builtin::List | Builtin::Map => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!(
                            "the type of `{name}()` cannot be inferred here; use it where a {name} type is expected"
                        ),
                    ));
                    None
                }
                Builtin::Id => self.lower_identity_ctor(args, span).map(CallResult::Value),
                // `none` is the payloadless Option constructor; it carries no
                // arguments, so a call form has no meaning.
                Builtin::None => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        "`none` takes no arguments; write `none` where an Option is expected"
                            .to_string(),
                    ));
                    None
                }
            };
        }
        // A scalar-type spelling in call position is a conversion (or, for a
        // temporal type, a compile-time-validated literal constructor), resolved
        // before records/functions so it is never shadowed. The admitted set is
        // closed; an unadmitted pair is a typed `check.unsupported`.
        if let Some(scalar) = ScalarType::from_spelling(name) {
            if scalar.is_temporal() {
                return self
                    .lower_temporal_construct(scalar, args, span)
                    .map(CallResult::Value);
            }
            return self
                .lower_conversion(name, args, span)
                .map(CallResult::Value);
        }
        if let Some((id, _)) = self.records.nominal_by_name(name) {
            return self
                .lower_nominal_construct(id, args, span)
                .map(CallResult::Value);
        }
        if self.records.struct_by_name(name).is_some() {
            return self
                .lower_struct_literal(name, args, span)
                .map(CallResult::Value);
        }
        // A generic struct template infers its instantiation from the field values.
        if let Some(template) = self.records.type_template_by_name(name)
            && !self.records.template_is_enum(template)
        {
            return self
                .lower_generic_struct_literal(template, args, span)
                .map(CallResult::Value);
        }
        if self.records.by_name(name).is_some() {
            return self
                .lower_constructor(name, args, span)
                .map(CallResult::Value);
        }
        if let Some(sig) = self.functions.same_module(self.module, name) {
            let (index, params, ret) = (sig.index, sig.params.clone(), sig.ret);
            return self.lower_function_call(index, &params, ret, args, span);
        }
        // A same-module generic function is monomorphized at the call site (its type
        // arguments inferred from the arguments), resolved before the collection
        // fallbacks so a generic named `get`/`append`/... shadows them.
        if let Some(template) = self.generics.same_module(self.module, name) {
            return self.lower_generic_call(template, args, span);
        }
        // The procedural collection operations resolve last, so a same-module
        // function of the same common name shadows them.
        if let Some(result) = self.lower_collection_fallback(name, args, span) {
            return result;
        }
        self.fail(name_error(self.file, span, name));
        None
    }

    /// Resolve `append`/`insert`/`get`/`length` as collection operations, or `None`
    /// when `name` is not one of them (so the caller reports it as an unknown name).
    /// These are non-reserved fallbacks: a same-module function of the same name is
    /// resolved before this is reached.
    fn lower_collection_fallback(
        &mut self,
        name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<Option<CallResult>> {
        let value = match name {
            "append" => self.lower_append(args, span),
            "insert" => self.lower_insert(args, span),
            "get" => self.lower_map_get(args, span),
            "length" => self.lower_length(args, span),
            _ => return None,
        };
        Some(value.map(CallResult::Value))
    }

    /// A `::`-qualified call `prefix::item`: resolved against the calling module's
    /// `use` bindings and the project module set, to a `pub` function.
    fn lower_qualified_call(
        &mut self,
        prefix: &[String],
        item: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<CallResult> {
        match self.functions.resolve_qualified(self.module, prefix, item) {
            CallResolution::Found(sig) => {
                let (index, params, ret) = (sig.index, sig.params.clone(), sig.ret);
                self.lower_function_call(index, &params, ret, args, span)
            }
            CallResolution::NotPublic => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckVisibility.as_str(),
                    self.file,
                    span,
                    format!("`{item}` is not `pub`, so it cannot be called from another module"),
                ));
                None
            }
            CallResolution::NotFound => {
                // A qualified generic function is resolved through the same module
                // scope and monomorphized, after the monomorphic table misses.
                if let Some(module) = self.functions.resolved_module(self.module, prefix)
                    && let Some((template, public)) = self.generics.in_module(&module, item)
                {
                    if !public && module != self.module {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckVisibility.as_str(),
                            self.file,
                            span,
                            format!(
                                "`{item}` is not `pub`, so it cannot be called from another module"
                            ),
                        ));
                        return None;
                    }
                    return self.lower_generic_call(template, args, span);
                }
                let path = prefix
                    .iter()
                    .map(String::as_str)
                    .chain(std::iter::once(item))
                    .collect::<Vec<_>>()
                    .join("::");
                self.fail(name_error(self.file, span, &path));
                None
            }
        }
    }

    /// Lower a direct function call: positional arguments matched to the callee's
    /// bare scalar params, then `Call`.
    fn lower_function_call(
        &mut self,
        index: u16,
        params: &[LTy],
        ret: RetType,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<CallResult> {
        if args.len() != params.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("expected {} arguments, found {}", params.len(), args.len()),
            ));
            return None;
        }
        for (argument, param) in args.iter().zip(params) {
            if argument.name.is_some() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    "function arguments are positional".to_string(),
                ));
                return None;
            }
            self.lower_as(&argument.value, *param)?;
        }
        self.push(Instr::Call(index), span);
        self.calls.push(index);
        Some(match ret {
            RetType::Unit => CallResult::Unit,
            RetType::Value(ty) => CallResult::Value(ty),
        })
    }

    /// Lower a call to a generic function: infer each type argument from the call's
    /// arguments, revalidate the type-parameter constraints against the inferred
    /// concrete types, monomorphize one image function for the exact argument list,
    /// and emit a call to it. A type parameter that no argument determines, an
    /// argument whose type does not match the parameter shape, or an inferred type
    /// that violates a constraint is a typed `check.type`. Inference is exact: a
    /// generic argument matches the parameter type structurally with no implicit
    /// bare-to-optional widening.
    fn lower_generic_call(
        &mut self,
        template_index: usize,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<CallResult> {
        let template: &'a GenericTemplate<'a> = &self.generics.templates[template_index];
        let params = &template.decl.params;
        if args.len() != params.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("expected {} arguments, found {}", params.len(), args.len()),
            ));
            return None;
        }
        let mut subst: Vec<Option<GArg>> = vec![None; template.type_params.len()];
        for (argument, param) in args.iter().zip(params) {
            if argument.name.is_some() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    "function arguments are positional".to_string(),
                ));
                return None;
            }
            let got = self.lower_expr(&argument.value)?;
            let expanded = self.records.expand(&param.ty);
            if let Err(message) = unify_type_param(
                self.records,
                &template.type_params,
                &expanded,
                got,
                &mut subst,
            ) {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    message,
                ));
                return None;
            }
        }
        // Every type parameter must be determined by an argument: there is no
        // explicit instantiation syntax, so an undetermined parameter cannot be
        // resolved and the call is rejected at its site.
        let mut concrete = Vec::with_capacity(subst.len());
        for (slot, (name, _)) in subst.iter().zip(&template.type_params) {
            match slot {
                Some(arg) => concrete.push(*arg),
                None => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!(
                            "cannot infer type parameter `{name}` of `{}`; \
                             pass an argument whose type determines it",
                            template.decl.name
                        ),
                    ));
                    return None;
                }
            }
        }
        // Per-application constraint revalidation: the concrete type substituted for
        // each constrained parameter must support the constraint's operators.
        for ((name, constraint), arg) in template.type_params.iter().zip(&concrete) {
            let Some(constraint) = constraint else {
                continue;
            };
            let satisfied = match arg {
                // In the template pass an argument may itself be an abstract
                // parameter; it satisfies the constraint when its own constraint does.
                GArg::Param(index) => {
                    self.type_param_constraint(*index)
                        .is_some_and(|outer| match constraint {
                            TypeConstraint::Equality => outer.admits_equality(),
                            TypeConstraint::Order => outer.admits_order(),
                        })
                }
                other => other.satisfies(*constraint),
            };
            if !satisfied {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!(
                        "type parameter `{name}` of `{}` is instantiated with `{}`, \
                         which does not `supports {}`",
                        template.decl.name,
                        garg_to_lty(*arg).spelling(self.records),
                        constraint.spelling(),
                    ),
                ));
                return None;
            }
        }
        // Resolve the return type against the concrete substitution, minting any
        // collection/enum instantiation the return shape needs into the draft (the
        // real draft for an instance, the throwaway draft for the template pass).
        let ret = self.resolve_generic_return(template, &concrete);
        match self.mode {
            LowerMode::Template => {
                // The once-checked pass validates the call shape but cannot
                // monomorphize an abstract instantiation; a placeholder keeps the
                // discarded stream value-shaped.
                if let RetType::Value(_) = ret {
                    let zero = self.draft.intern_int(0);
                    self.push(Instr::ConstLoad(zero.index()), span);
                }
                Some(match ret {
                    RetType::Unit => CallResult::Unit,
                    RetType::Value(ty) => CallResult::Value(ty),
                })
            }
            LowerMode::Concrete => {
                let func = match self.records.reserve_fn_instance(template_index, concrete) {
                    Some(func) => func,
                    None => {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckInstantiationLimit.as_str(),
                            self.file,
                            span,
                            format!(
                                "monomorphizing this program requires more than {MAX_INSTANTIATIONS} \
                                 generic instantiations; a generic function likely recurses over an \
                                 ever-growing type"
                            ),
                        ));
                        return None;
                    }
                };
                self.push(Instr::Call(func), span);
                self.calls.push(func);
                Some(match ret {
                    RetType::Unit => CallResult::Unit,
                    RetType::Value(ty) => CallResult::Value(ty),
                })
            }
        }
    }

    /// Resolve a generic template's return type under a concrete substitution,
    /// minting any instantiation it needs into the current draft.
    fn resolve_generic_return(&mut self, template: &GenericTemplate, concrete: &[GArg]) -> RetType {
        let Some(annotation) = &template.decl.return_type else {
            return RetType::Unit;
        };
        let env: Vec<TypeParamSlot> = template
            .type_params
            .iter()
            .zip(concrete)
            .map(|((name, _), arg)| TypeParamSlot {
                name: name.clone(),
                binding: ParamBinding::Concrete(*arg),
            })
            .collect();
        match resolve_type(
            self.records,
            self.draft,
            self.durable,
            annotation,
            TypeEnv { params: &env },
        ) {
            Some(LTy::Record { .. }) | None => RetType::Unit,
            Some(ty) => RetType::Value(ty),
        }
    }

    /// Lower a nominal construction `Age(n)`: coerce the one positional argument
    /// to the base int, then guard it against the type's inclusive interval. An
    /// out-of-interval value faults `run.range` at runtime; every path that
    /// produces a value of the type revalidates the interval this way.
    fn lower_nominal_construct(
        &mut self,
        id: NominalId,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let value = self.single_nominal_arg(id, args, span)?;
        self.lower_as(value, LTy::bare_scalar(ScalarType::Int))?;
        let info = self.records.nominal(id);
        self.push(
            Instr::RangeGuard {
                lo: info.lo,
                hi: info.hi,
            },
            span,
        );
        Some(LTy::Nominal {
            id,
            optional: false,
        })
    }

    /// Lower the nominal range test `Age.checked(n): Age?`: present exactly when
    /// the int lies in the interval, vacant otherwise, never a fault. Reuses the
    /// comparison and optional ops; no dedicated opcode.
    fn lower_checked_nominal(
        &mut self,
        id: NominalId,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let value = self.single_nominal_arg(id, args, span)?;
        self.lower_as(value, LTy::bare_scalar(ScalarType::Int))?;
        let slot = self.alloc_slot();
        self.push(Instr::LocalSet(slot), span);
        let (lo, hi) = {
            let info = self.records.nominal(id);
            (info.lo, info.hi)
        };
        // lo <= n && n <= hi, with each failed test jumping to the vacant edge.
        let lo_const = self.draft.intern_int(lo);
        self.push(Instr::LocalGet(slot), span);
        self.push(Instr::ConstLoad(lo_const.index()), span);
        let below = {
            self.push(Instr::IntGe, span);
            self.push_jif(span)
        };
        let hi_const = self.draft.intern_int(hi);
        self.push(Instr::LocalGet(slot), span);
        self.push(Instr::ConstLoad(hi_const.index()), span);
        let above = {
            self.push(Instr::IntLe, span);
            self.push_jif(span)
        };
        self.push(Instr::LocalGet(slot), span);
        self.push(Instr::SomeWrap, span);
        let to_end = self.push_jump(span);
        let vacant = self.here();
        self.patch(below, vacant);
        self.patch(above, vacant);
        self.push(Instr::VacantLoad(ImageType::opt_scalar(Scalar::Int)), span);
        let end = self.here();
        self.patch(to_end, end);
        Some(LTy::Nominal { id, optional: true })
    }

    /// The one positional argument of a nominal construction or range test.
    fn single_nominal_arg<'e>(
        &mut self,
        id: NominalId,
        args: &'e [Argument],
        span: SourceSpan,
    ) -> Option<&'e Expression> {
        match args {
            [arg] if arg.name.is_none() => Some(&arg.value),
            _ => {
                let name = self.records.nominal(id).name.clone();
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!("`{name}` takes one positional int value"),
                ));
                None
            }
        }
    }

    /// Lower a record constructor: each field's argument in declaration order.
    fn lower_constructor(
        &mut self,
        name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let record_type_id = self.records.by_name(name)?.type_id;

        // Resolve each named argument against a top-level field or a group before
        // emitting, so evaluation order is the declaration order (fields first, then
        // groups; f0 pushed first). A group argument is the group's value, built with
        // the qualified group constructor `Resource.group(…)`.
        for argument in args {
            let Some(arg_name) = &argument.name else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    "constructor arguments must be named".to_string(),
                ));
                return None;
            };
            let record = self.records.by_name(name)?;
            if record.field(arg_name).is_none() && record.group(arg_name).is_none() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{name}` has no field `{arg_name}`"),
                ));
                return None;
            }
        }

        let field_plan: Vec<MemberPlan> = self
            .records
            .by_name(name)?
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty, field.required))
            .collect();
        for (field_name, field_ty, required) in field_plan {
            let arg = args
                .iter()
                .find(|a| a.name.as_deref() == Some(field_name.as_str()));
            let bare = garg_to_lty(field_ty);
            let expected = if required { bare } else { bare.to_optional() };
            match arg {
                Some(argument) => {
                    self.lower_as(&argument.value, expected)?;
                }
                None if required => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!("missing required field `{field_name}`"),
                    ));
                    return None;
                }
                None => {
                    // A sparse field defaults to vacant: a typed vacant optional of
                    // the field's value type.
                    self.push(Instr::VacantLoad(bare.to_optional().image()), span);
                }
            }
        }
        // Each unkeyed group slot follows the top-level fields, in group order. A
        // supplied `group: Resource.group(…)` argument carries the group value; an
        // omitted all-sparse group defaults to present with vacant leaves; an omitted
        // group with a required leaf cannot be auto-completed, so it is the
        // required-completeness rejection here rather than a silent incomplete value.
        let group_plan: Vec<GroupPlan> = self
            .records
            .by_name(name)?
            .groups
            .iter()
            .map(|group| {
                (
                    group.name.clone(),
                    group.type_id,
                    group.fields.iter().any(|leaf| leaf.required),
                    group
                        .fields
                        .iter()
                        .map(|leaf| (leaf.name.clone(), leaf.ty, leaf.required))
                        .collect(),
                )
            })
            .collect();
        for (group_name, group_type, has_required, leaves) in group_plan {
            let arg = args
                .iter()
                .find(|a| a.name.as_deref() == Some(group_name.as_str()));
            if let Some(argument) = arg {
                self.lower_as(
                    &argument.value,
                    LTy::Record {
                        ty: group_type,
                        optional: false,
                    },
                )?;
                continue;
            }
            if has_required {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!("missing required field `{group_name}`"),
                ));
                return None;
            }
            for (_leaf_name, leaf_ty, _required) in leaves {
                self.push(
                    Instr::VacantLoad(garg_to_lty(leaf_ty).to_optional().image()),
                    span,
                );
            }
            self.push(Instr::RecordNew(group_type.index()), span);
        }
        self.push(Instr::RecordNew(record_type_id.index()), span);
        Some(LTy::Record {
            ty: record_type_id,
            optional: false,
        })
    }

    /// The executable branch `resource.branch`, if `resource` is the store's executable
    /// resource and `branch` is one of its single-level keyed branches. The returned
    /// reference borrows the durable registry (lifetime `'a`), not `self`, so it stays
    /// valid across later mutating calls.
    /// The executable branch reached by the branch-name `path` from `resource`, if
    /// `resource` is the store's executable resource and each name is a keyed branch at its
    /// level. Walks the recursive branch tree so `Book.notes.tags` resolves the nested
    /// `tags` branch of `notes`. The returned reference borrows the durable registry
    /// (lifetime `'a`), not `self`, so it stays valid across later mutating calls.
    fn executable_branch_path(
        &self,
        resource: &str,
        path: &[&str],
    ) -> Option<&'a crate::durable::DurableBranch> {
        let root = self.durable.root()?;
        if root.resource != resource {
            return None;
        }
        let (first, rest) = path.split_first()?;
        let mut branch = root.branch(first)?;
        for name in rest {
            branch = branch.branch(name)?;
        }
        Some(branch)
    }

    /// Lower a keyed branch entry constructor `Resource.branch(field: value, …)`. The
    /// branch's materialized record is built from its declared scalar fields in
    /// declaration order (f0 pushed first), each required field supplied and each sparse
    /// field defaulting to vacant — the same shape as the root constructor, one level
    /// down. The shadowing rule holds: a value binding may not shadow the resource type
    /// name in dotted-constructor head position.
    fn lower_branch_constructor(
        &mut self,
        resource: &str,
        display: &str,
        branch: &'a crate::durable::DurableBranch,
        head_span: SourceSpan,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        if self.lookup(resource).is_some() || self.lookup_place(resource).is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                head_span,
                format!(
                    "`{resource}` is a resource type here (the head of a branch constructor \
                     `{display}(…)`); a value binding may not shadow it"
                ),
            ));
            return None;
        }
        let record = branch.record;

        // Validate argument names against the branch's fields before emitting, so
        // evaluation order is the field declaration order.
        for argument in args {
            let Some(arg_name) = &argument.name else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    "constructor arguments must be named".to_string(),
                ));
                return None;
            };
            if branch.field(arg_name).is_none() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{display}` has no field `{arg_name}`"),
                ));
                return None;
            }
        }

        // `branch` borrows the registry (lifetime independent of `self`), so it stays
        // valid across the mutating `lower_as`/`push` calls below.
        for field in &branch.fields {
            let arg = args
                .iter()
                .find(|a| a.name.as_deref() == Some(field.name.as_str()));
            let bare = LTy::bare_scalar(field.scalar);
            let expected = if field.required {
                bare
            } else {
                bare.to_optional()
            };
            match arg {
                Some(argument) => {
                    self.lower_as(&argument.value, expected)?;
                }
                None if field.required => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!("missing required field `{}`", field.name),
                    ));
                    return None;
                }
                None => {
                    // A sparse field defaults to a typed vacant optional.
                    self.push(Instr::VacantLoad(bare.to_optional().image()), span);
                }
            }
        }
        self.push(Instr::RecordNew(record.index()), span);
        Some(LTy::Record {
            ty: record,
            optional: false,
        })
    }

    /// Lower a qualified group value constructor `Resource.group(field: value, …)`.
    /// The group's materialized record is built from its declared leaves in
    /// declaration order (f0 pushed first), each required leaf supplied and each
    /// sparse leaf defaulting to vacant — symmetric with the root and branch
    /// constructors. The shadowing rule holds: a value binding may not shadow the
    /// resource type name in dotted-constructor head position.
    fn lower_group_constructor(
        &mut self,
        resource: &str,
        group_name: &str,
        head_span: SourceSpan,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        if self.lookup(resource).is_some() || self.lookup_place(resource).is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                head_span,
                format!(
                    "`{resource}` is a resource type here (the head of a group constructor \
                     `{resource}.{group_name}(…)`); a value binding may not shadow it"
                ),
            ));
            return None;
        }
        let display = format!("{resource}.{group_name}");
        let (_, group) = self.records.by_name(resource)?.group(group_name)?;
        let group_type_id = group.type_id;
        let leaf_plan: Vec<MemberPlan> = group
            .fields
            .iter()
            .map(|leaf| (leaf.name.clone(), leaf.ty, leaf.required))
            .collect();

        for argument in args {
            let Some(arg_name) = &argument.name else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    "constructor arguments must be named".to_string(),
                ));
                return None;
            };
            if !leaf_plan.iter().any(|(name, _, _)| name == arg_name) {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{display}` has no field `{arg_name}`"),
                ));
                return None;
            }
        }

        for (leaf_name, leaf_ty, required) in leaf_plan {
            let arg = args
                .iter()
                .find(|a| a.name.as_deref() == Some(leaf_name.as_str()));
            let bare = garg_to_lty(leaf_ty);
            let expected = if required { bare } else { bare.to_optional() };
            match arg {
                Some(argument) => {
                    self.lower_as(&argument.value, expected)?;
                }
                None if required => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!("missing required field `{leaf_name}`"),
                    ));
                    return None;
                }
                None => {
                    self.push(Instr::VacantLoad(bare.to_optional().image()), span);
                }
            }
        }
        self.push(Instr::RecordNew(group_type_id.index()), span);
        Some(LTy::Record {
            ty: group_type_id,
            optional: false,
        })
    }

    /// Lower a dense struct literal `Point(x: a, y: b)`: named-only arguments, the
    /// exact field set with none missing, duplicated, or unknown, each coerced to
    /// its required field scalar in field declaration order (f0 pushed first) so
    /// the canonical product-leaf order owns evaluation. Emits `RecordNew` over the
    /// struct's shared image record def.
    fn lower_struct_literal(
        &mut self,
        name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let type_id = self.records.struct_by_name(name)?.type_id;

        let mut ok = true;
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for argument in args {
            let Some(arg_name) = &argument.name else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{name}` fields are named, as `{name}(field: value, ...)`"),
                ));
                ok = false;
                continue;
            };
            if self.records.struct_by_name(name)?.field(arg_name).is_none() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{name}` has no field `{arg_name}`"),
                ));
                ok = false;
                continue;
            }
            if !seen.insert(arg_name.as_str()) {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("field `{arg_name}` is set more than once"),
                ));
                ok = false;
            }
        }
        if !ok {
            return None;
        }

        let field_plan: Vec<(String, GArg)> = self
            .records
            .struct_by_name(name)?
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty))
            .collect();
        for (field_name, field_ty) in field_plan {
            let arg = args
                .iter()
                .find(|a| a.name.as_deref() == Some(field_name.as_str()));
            match arg {
                Some(argument) => {
                    self.lower_as(&argument.value, garg_to_lty(field_ty))?;
                }
                None => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!("missing field `{field_name}`"),
                    ));
                    return None;
                }
            }
        }
        self.push(Instr::RecordNew(type_id.index()), span);
        Some(LTy::Struct {
            ty: type_id,
            optional: false,
        })
    }

    /// Lower a generic struct construction `Pair(first: v, second: w)`: infer each
    /// type parameter from the field values (there is no explicit `Pair<int, string>`
    /// construction syntax), monomorphize the instantiation, and construct the record.
    /// Field values are lowered in declaration order so evaluation order is stable.
    fn lower_generic_struct_literal(
        &mut self,
        template: usize,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let name = self.records.template_name(template).to_string();
        let fields = self
            .records
            .template_struct_fields(template)
            .expect("a struct template has struct fields");
        if !self.check_named_args(
            &name,
            args,
            &fields.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            span,
        ) {
            return None;
        }
        let params = self.records.template_type_params(template).to_vec();
        let mut subst: Vec<Option<GArg>> = vec![None; params.len()];
        for (field_name, field_ty) in &fields {
            let Some(argument) = args
                .iter()
                .find(|a| a.name.as_deref() == Some(field_name.as_str()))
            else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!("missing field `{field_name}`"),
                ));
                return None;
            };
            let got = self.lower_expr(&argument.value)?;
            let expanded = self.records.expand(field_ty);
            if let Err(message) =
                unify_type_param(self.records, &params, &expanded, got, &mut subst)
            {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    message,
                ));
                return None;
            }
        }
        let concrete = self.determined_args(&name, &params, &subst, span)?;
        if !self.constraints_satisfied(template, &name, &concrete, span) {
            return None;
        }
        let TypeInstId::Record(type_id) = self
            .records
            .mint_type_instance(self.draft, template, &concrete)?
        else {
            return None;
        };
        self.push(Instr::RecordNew(type_id.index()), span);
        Some(LTy::Struct {
            ty: type_id,
            optional: false,
        })
    }

    /// Lower a generic enum construction `Maybe::just(value: v)`: infer each type
    /// parameter from the variant's payload values, monomorphize the instantiation,
    /// and construct the variant. A payloadless variant or one that does not
    /// determine every parameter cannot be inferred at the construction site.
    fn lower_generic_enum_construct(
        &mut self,
        template: usize,
        variant_name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let name = self.records.template_name(template).to_string();
        let Some(payload) = self
            .records
            .template_variant_payload(template, variant_name)
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("enum `{name}` has no member `{variant_name}`"),
            ));
            return None;
        };
        if !self.check_named_args(
            &format!("{name}::{variant_name}"),
            args,
            &payload.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            span,
        ) {
            return None;
        }
        let params = self.records.template_type_params(template).to_vec();
        let mut subst: Vec<Option<GArg>> = vec![None; params.len()];
        for (field_name, field_ty) in &payload {
            let Some(argument) = args
                .iter()
                .find(|a| a.name.as_deref() == Some(field_name.as_str()))
            else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!("missing payload field `{field_name}`"),
                ));
                return None;
            };
            let got = self.lower_expr(&argument.value)?;
            let expanded = self.records.expand(field_ty);
            if let Err(message) =
                unify_type_param(self.records, &params, &expanded, got, &mut subst)
            {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    message,
                ));
                return None;
            }
        }
        let concrete = self.determined_args(&name, &params, &subst, span)?;
        if !self.constraints_satisfied(template, &name, &concrete, span) {
            return None;
        }
        let TypeInstId::Enum(enum_id) = self
            .records
            .mint_type_instance(self.draft, template, &concrete)?
        else {
            return None;
        };
        let variant_index = self
            .records
            .enum_variants(enum_id)
            .and_then(|variants| variants.iter().position(|(v, _)| v == variant_name))?
            as u16;
        self.push(
            Instr::EnumConstruct {
                enum_idx: enum_id.index(),
                variant: variant_index,
            },
            span,
        );
        Some(LTy::Enum {
            ty: enum_id,
            optional: false,
        })
    }

    /// Validate that every argument is named, names a known field, and is set once.
    /// Shared by generic struct and enum construction. Returns whether the arguments
    /// are well-formed; each defect is reported.
    fn check_named_args(
        &mut self,
        subject: &str,
        args: &[Argument],
        field_names: &[String],
        _span: SourceSpan,
    ) -> bool {
        let mut ok = true;
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for argument in args {
            let Some(arg_name) = &argument.name else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{subject}` fields are named, as `{subject}(field: value, ...)`"),
                ));
                ok = false;
                continue;
            };
            if !field_names.iter().any(|name| name == arg_name) {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{subject}` has no field `{arg_name}`"),
                ));
                ok = false;
                continue;
            }
            if !seen.insert(arg_name.as_str()) {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("field `{arg_name}` is set more than once"),
                ));
                ok = false;
            }
        }
        ok
    }

    /// Per-application constraint revalidation for an inferred instantiation: every
    /// concrete argument must support its parameter's constraint. Construction always
    /// infers concrete arguments, so no abstract-parameter entailment applies here.
    fn constraints_satisfied(
        &mut self,
        template: usize,
        name: &str,
        concrete: &[GArg],
        span: SourceSpan,
    ) -> bool {
        for ((param_name, constraint), arg) in self
            .records
            .template_type_params(template)
            .iter()
            .zip(concrete)
        {
            if let Some(constraint) = constraint
                && !arg.satisfies(*constraint)
            {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!(
                        "type parameter `{param_name}` of `{name}` is instantiated with `{}`, \
                         which does not `supports {}`",
                        garg_to_lty(*arg).spelling(self.records),
                        constraint.spelling(),
                    ),
                ));
                return false;
            }
        }
        true
    }

    /// Turn an inference substitution into the concrete argument list, reporting an
    /// undetermined type parameter (which the construction site cannot resolve).
    fn determined_args(
        &mut self,
        name: &str,
        params: &[(String, Option<TypeConstraint>)],
        subst: &[Option<GArg>],
        span: SourceSpan,
    ) -> Option<Vec<GArg>> {
        let mut concrete = Vec::with_capacity(params.len());
        for (slot, (param_name, _)) in subst.iter().zip(params) {
            match slot {
                Some(arg) => concrete.push(*arg),
                None => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!(
                            "cannot infer type parameter `{param_name}` of `{name}`; \
                             a field value must determine it"
                        ),
                    ));
                    return None;
                }
            }
        }
        Some(concrete)
    }

    /// Lower an enum construction `Enum::member` or `Enum::member(field: v, ...)`.
    /// A payloadless member takes no arguments; a payload member takes the exact
    /// named payload set, each coerced to its declared scalar in payload
    /// declaration order (p0 pushed first), then `EnumConstruct`.
    fn lower_enum_construct(
        &mut self,
        enum_name: &str,
        variant_name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let (enum_id, variant_index) = {
            let info = self.records.enum_by_name(enum_name)?;
            let Some((index, _)) = info.variant(variant_name) else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!("enum `{enum_name}` has no member `{variant_name}`"),
                ));
                return None;
            };
            (info.enum_id, index)
        };
        // The payload plan, resolved before emission so evaluation order is the
        // payload declaration order.
        let plan: Vec<(String, ScalarType)> = self
            .records
            .enum_by_name(enum_name)?
            .variant(variant_name)?
            .1
            .payload
            .iter()
            .map(|field| (field.name.clone(), field.scalar))
            .collect();

        if plan.is_empty() {
            if !args.is_empty() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!("`{enum_name}::{variant_name}` carries no payload"),
                ));
                return None;
            }
        } else {
            let mut ok = true;
            let mut seen: BTreeSet<&str> = BTreeSet::new();
            for argument in args {
                let Some(arg_name) = &argument.name else {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        argument.value.span(),
                        format!(
                            "`{enum_name}::{variant_name}` payload fields are named, as \
                             `{variant_name}(field: value, ...)`"
                        ),
                    ));
                    ok = false;
                    continue;
                };
                if !plan.iter().any(|(name, _)| name == arg_name) {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        argument.value.span(),
                        format!("`{enum_name}::{variant_name}` has no payload field `{arg_name}`"),
                    ));
                    ok = false;
                    continue;
                }
                if !seen.insert(arg_name.as_str()) {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        argument.value.span(),
                        format!("payload field `{arg_name}` is set more than once"),
                    ));
                    ok = false;
                }
            }
            if !ok {
                return None;
            }
            for (field_name, scalar) in &plan {
                let arg = args
                    .iter()
                    .find(|a| a.name.as_deref() == Some(field_name.as_str()));
                match arg {
                    Some(argument) => {
                        self.lower_as(&argument.value, LTy::bare_scalar(*scalar))?;
                    }
                    None => {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            span,
                            format!("missing payload field `{field_name}`"),
                        ));
                        return None;
                    }
                }
            }
        }
        self.push(
            Instr::EnumConstruct {
                enum_idx: enum_id.index(),
                variant: variant_index,
            },
            span,
        );
        Some(LTy::Enum {
            ty: enum_id,
            optional: false,
        })
    }

    /// The image enum index of the reserved `Option[inner]`, minting it on first use.
    fn opt_enum(&mut self, inner: GArg) -> EnumId {
        self.records.instantiate_reserved_option(self.draft, inner)
    }

    /// Lower a reserved `Option`/`Result` constructor directed by an expected type:
    /// `none`, `some(v)`, `ok(v)`, or `err(e)`. The expected type supplies the exact
    /// instantiation, so the argument (if any) is coerced to the matching member
    /// type. A constructor used where its reserved enum is not expected is a typed
    /// error. `Option`/`Result` are ordinary generic enums; these reserved spellings
    /// resolve to their variants recovered from the minting template.
    fn lower_ctor_as(&mut self, kind: CtorKind, expr: &Expression, expected: LTy) -> Option<()> {
        let span = expr.span();
        // A sparse optional enum target (`Option<T>?`/`Result<T, E>?`) takes a bare
        // constructor wrapped present: lower against the bare enum, then `SomeWrap`.
        // This makes `= none`/`= some(v)` write a sparse optional-enum field or
        // local in one line — the present-value analogue of `= absent`.
        if let LTy::Enum { ty, optional: true } = expected {
            self.lower_ctor_as(
                kind,
                expr,
                LTy::Enum {
                    ty,
                    optional: false,
                },
            )?;
            self.push(Instr::SomeWrap, span);
            return Some(());
        }
        let LTy::Enum {
            ty: enum_id,
            optional: false,
        } = expected
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "`{}` needs an Option or Result type here, but the expected type is {}",
                    kind.name(),
                    expected.spelling(self.records)
                ),
            ));
            return None;
        };
        let option_arg = self.records.as_option(enum_id);
        let result_args = self.records.as_result(enum_id);
        match (kind, option_arg, result_args) {
            (CtorKind::None, Some(_), _) => {
                self.push(
                    Instr::EnumConstruct {
                        enum_idx: enum_id.index(),
                        variant: OPTION_NONE,
                    },
                    span,
                );
                Some(())
            }
            (CtorKind::Some, Some(inner), _) => {
                let arg = self.single_ctor_arg(expr, "some")?;
                self.lower_as(arg, garg_to_lty(inner))?;
                self.push(
                    Instr::EnumConstruct {
                        enum_idx: enum_id.index(),
                        variant: OPTION_SOME,
                    },
                    span,
                );
                Some(())
            }
            (CtorKind::Ok, _, Some((ok, _))) => {
                let arg = self.single_ctor_arg(expr, "ok")?;
                self.lower_as(arg, garg_to_lty(ok))?;
                self.push(
                    Instr::EnumConstruct {
                        enum_idx: enum_id.index(),
                        variant: RESULT_OK,
                    },
                    span,
                );
                Some(())
            }
            (CtorKind::Err, _, Some((_, err))) => {
                let arg = self.single_ctor_arg(expr, "err")?;
                self.lower_as(arg, garg_to_lty(err))?;
                self.push(
                    Instr::EnumConstruct {
                        enum_idx: enum_id.index(),
                        variant: RESULT_ERR,
                    },
                    span,
                );
                Some(())
            }
            _ => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!(
                        "`{}` does not construct {}",
                        kind.name(),
                        expected.spelling(self.records)
                    ),
                ));
                None
            }
        }
    }

    /// Lower a bare `some(v)` whose Option type is inferred from `v`. `none`, `ok`,
    /// and `err` cannot infer their full type argument set, so they require an
    /// expected type and are rejected here.
    fn lower_some_infer(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`some` takes exactly one value, as `some(value)`".to_string(),
            ));
            return None;
        };
        if arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                "`some` takes a positional value".to_string(),
            ));
            return None;
        }
        let inner_ty = self.lower_expr(&arg.value)?;
        let Some(inner) = inner_ty.as_garg() else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                format!(
                    "{} cannot be the value of an Option",
                    inner_ty.spelling(self.records)
                ),
            ));
            return None;
        };
        let id = self.opt_enum(inner);
        self.push(
            Instr::EnumConstruct {
                enum_idx: id.index(),
                variant: OPTION_SOME,
            },
            span,
        );
        Some(LTy::Enum {
            ty: id,
            optional: false,
        })
    }

    /// The single positional argument of a `some`/`ok`/`err` constructor call.
    fn single_ctor_arg<'e>(&mut self, expr: &'e Expression, name: &str) -> Option<&'e Expression> {
        let Expression::Call { args, .. } = expr else {
            return None;
        };
        match args.as_slice() {
            [arg] if arg.name.is_none() => Some(&arg.value),
            _ => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    expr.span(),
                    format!("`{name}` takes exactly one value, as `{name}(value)`"),
                ));
                None
            }
        }
    }

    /// Lower prefix `try <expr>`: propagate a `Result<T, E>`'s `err` out of the
    /// enclosing `Result[U, E]`-returning function (same `E`, no conversion),
    /// yielding the `ok` value `T`. Dispatches on the tag: on `err` it rebuilds the
    /// error in the return `Result` and returns; on `ok` it extracts the value.
    fn lower_try(&mut self, inner: &Expression, span: SourceSpan) -> Option<LTy> {
        let inner_ty = self.lower_expr(inner)?;
        let src = inner_ty
            .bare_enum()
            .and_then(|id| self.records.as_result(id));
        let Some((t_arg, e_arg)) = src else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                inner.span(),
                format!(
                    "`try` needs a Result value, found {}",
                    inner_ty.spelling(self.records)
                ),
            ));
            return None;
        };
        let ret_id = match self.ret {
            RetType::Value(ty) => ty.bare_enum(),
            RetType::Unit => None,
        };
        let ret_result = ret_id.and_then(|id| self.records.as_result(id).map(|args| (id, args)));
        let Some((ret_id, (_, ret_err))) = ret_result else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`try` is only valid in a function that returns a Result".to_string(),
            ));
            return None;
        };
        if ret_err != e_arg {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "`try` propagates the error type {}, but the function returns {}",
                    garg_spelling(e_arg, self.records),
                    garg_spelling(ret_err, self.records)
                ),
            ));
            return None;
        }
        let slot = self.alloc_slot();
        self.push(Instr::LocalSet(slot), span);
        self.push(Instr::LocalGet(slot), span);
        self.push(Instr::EnumTag, span);
        let err_tag = self.draft.intern_int(i64::from(RESULT_ERR));
        self.push(Instr::ConstLoad(err_tag.index()), span);
        self.push(Instr::EqInt, span);
        // False (not err, i.e. ok) jumps to the ok extraction; true (err) falls
        // through to rebuild the error in the return Result and return it.
        let to_ok = self.push_jif(span);
        self.push(Instr::LocalGet(slot), span);
        self.push(
            Instr::EnumPayloadGet {
                variant: RESULT_ERR,
                field: 0,
            },
            span,
        );
        self.push(
            Instr::EnumConstruct {
                enum_idx: ret_id.index(),
                variant: RESULT_ERR,
            },
            span,
        );
        self.push(Instr::Return, span);
        let ok_here = self.here();
        self.patch(to_ok, ok_here);
        self.push(Instr::LocalGet(slot), span);
        self.push(
            Instr::EnumPayloadGet {
                variant: RESULT_OK,
                field: 0,
            },
            span,
        );
        Some(garg_to_lty(t_arg))
    }

    fn lower_field(&mut self, base: &Expression, name: &str, span: SourceSpan) -> Option<LTy> {
        let base_ty = self.lower_expr(base)?;
        let (index, field_ty, required) =
            self.resolve_product_field(base_ty, name, base.span(), span)?;
        self.push(Instr::FieldGet(index), span);
        let bare = garg_to_lty(field_ty);
        Some(if required { bare } else { bare.to_optional() })
    }

    /// Lower `base?.name`: a member read through an *optional composite value*. The
    /// base is an optional record/struct value (local, or the value of a durable
    /// read); an absent base short-circuits the whole read to absent, and a present
    /// base yields the field wrapped optional. The result is always optional, so
    /// `?.` is the present-propagating analogue of `.` — its one meaning. This is a
    /// local-value operator: a durable address propagates absence structurally on
    /// its own and needs no `?.`.
    fn lower_optional_field(
        &mut self,
        base: &Expression,
        name: &str,
        span: SourceSpan,
    ) -> Option<LTy> {
        let base_ty = self.lower_expr(base)?;
        if !base_ty.is_optional() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                base.span(),
                format!(
                    "`?.` needs an optional value on the left, found {}; use `.` for a \
                     present value",
                    base_ty.spelling(self.records)
                ),
            ));
            return None;
        }
        let (index, field_ty, required) =
            self.resolve_product_field(base_ty.to_bare(), name, base.span(), span)?;
        let result = garg_to_lty(field_ty).to_optional();

        // Present: unwrap the optional composite to its bare record and read the
        // field; a required field is wrapped present, a sparse field already reads
        // optional. Absent: short-circuit to a vacant of the result type. Both paths
        // join at `result`.
        let to_absent = self.push_branch_present(base.span());
        self.push(Instr::FieldGet(index), span);
        if required {
            self.push(Instr::SomeWrap, span);
        }
        let to_end = self.push_jump(span);
        let absent = self.here();
        self.patch(to_absent, absent);
        self.push(Instr::VacantLoad(result.image()), span);
        let end = self.here();
        self.patch(to_end, end);
        Some(result)
    }

    /// Resolve `name` against a bare product (`record` or `struct`) value type to
    /// its slot index, bare value type, and required flag. The one owner of product
    /// field resolution, shared by field reads, assignments, and `unset`.
    /// `base_span` locates a non-product base; `field_span` locates an unknown field.
    fn resolve_product_field(
        &mut self,
        base_ty: LTy,
        name: &str,
        base_span: SourceSpan,
        field_span: SourceSpan,
    ) -> Option<(u16, GArg, bool)> {
        match base_ty {
            LTy::Record {
                ty,
                optional: false,
            } => {
                if let Some(record) = self.records.by_name_for_type(ty) {
                    if let Some((index, field)) = record.field(name) {
                        return Some((index, field.ty, field.required));
                    }
                    // An unkeyed group member reads as its nested group sub-record
                    // value (always present); its own leaves resolve one level down.
                    if let Some((index, group)) = record.group(name) {
                        return Some((index, GArg::Group(group.type_id), true));
                    }
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        field_span,
                        format!("record has no field `{name}`"),
                    ));
                    return None;
                }
                // A group sub-record value resolves its scalar/enum leaves against the
                // group's field layout (a sparse leaf reads `T?`).
                if let Some(group) = self.records.group_by_type(ty) {
                    let Some((index, field)) = group.field(name) else {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            field_span,
                            format!("group has no field `{name}`"),
                        ));
                        return None;
                    };
                    return Some((index, field.ty, field.required));
                }
                // A materialized keyed branch entry value (from `if const n =
                // ^root(k).branch(bk)`) is an image record the resource registry does not
                // own; resolve its scalar fields against the branch's field layout.
                if let Some(branch) = self.durable.branch_by_record(ty) {
                    let Some((index, field)) = branch.field_index(name) else {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            field_span,
                            format!("record has no field `{name}`"),
                        ));
                        return None;
                    };
                    return Some((index, GArg::Scalar(field.scalar), field.required));
                }
                self.fail(unsupported(self.file, field_span, "this field access"));
                None
            }
            LTy::Struct {
                ty,
                optional: false,
            } => {
                // A concrete struct reads its fields from the registry; a generic
                // struct instantiation reads them from its resolved body.
                if let Some(info) = self.records.struct_by_type(ty) {
                    let Some((index, field)) = info.field(name) else {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            field_span,
                            format!("`{}` has no field `{name}`", info.name),
                        ));
                        return None;
                    };
                    return Some((index, field.ty, field.required));
                }
                let fields = match self.records.type_inst_body(TypeInstId::Record(ty)) {
                    Some(crate::types::InstBody::Struct(fields)) => fields,
                    _ => panic!("a bare struct type resolves to a struct info or instantiation"),
                };
                match fields.iter().position(|(fname, _)| fname == name) {
                    Some(index) => Some((index as u16, fields[index].1, true)),
                    None => {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            field_span,
                            format!("`{}` has no field `{name}`", base_ty.spelling(self.records)),
                        ));
                        None
                    }
                }
            }
            _ => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    base_span,
                    format!(
                        "field access needs a record or struct, found {}",
                        base_ty.spelling(self.records)
                    ),
                ));
                None
            }
        }
    }

    // --- durable places (design §D) ---

    /// Detect the inline durable shape of a place expression: a whole-entry address
    /// `^root(key)….b(bkey)` at any depth, or a field-exact address `<entry-address>.field`.
    /// No diagnostics. Does not see source-local `place` bindings, which need instance
    /// state; use [`Self::durable_access`] for the full detection.
    fn durable_shape(expr: &Expression) -> Option<DurShape> {
        if is_entry_address(expr) {
            Some(DurShape::Entry)
        } else if is_field_address(expr) || is_group_leaf_address(expr) {
            // A field-exact address, a whole root-level group (both `<entry>.name`), or a
            // group-leaf address `<entry>.group.leaf`. The resolver disambiguates a group
            // from a field by name; a group leaf is one field selection deeper.
            Some(DurShape::Field)
        } else {
            None
        }
    }

    /// The inline durable ^-address shape of `expr`, confirming a group-leaf address
    /// against the resolved durable model. [`Self::durable_shape`] recognizes a group-leaf
    /// address `<entry>.mid.leaf` syntactically; here `mid` must actually name a root-level
    /// group. A `mid` that is a stored field (or an unknown name) leaves the expression an
    /// ordinary field projection on a durable field value, lowered and diagnosed by the
    /// ordinary field path rather than compiling to a codeless durable body.
    fn durable_shape_here(&self, expr: &Expression) -> Option<DurShape> {
        if is_group_leaf_address(expr) {
            return self.middle_names_a_group(expr).then_some(DurShape::Field);
        }
        Self::durable_shape(expr)
    }

    /// Whether the middle selector of a group-leaf address `<entry>.mid.leaf` names a
    /// root-level `group`: the entry is the root itself (`^root[k]`, not a nested branch,
    /// which offers no executable group) and the root declares a group named `mid`.
    fn middle_names_a_group(&self, expr: &Expression) -> bool {
        let Expression::Field { base, .. } = expr else {
            return false;
        };
        let Expression::Field {
            base: entry,
            name: mid,
            ..
        } = base.as_ref()
        else {
            return false;
        };
        let Expression::Keyed {
            base: root_base, ..
        } = entry.as_ref()
        else {
            return false;
        };
        if !matches!(root_base.as_ref(), Expression::SavedRoot { .. }) {
            return false;
        }
        self.durable
            .root()
            .is_some_and(|root| root.group(mid).is_some())
    }

    /// The most recent in-scope `place` binding named `name`, if any.
    fn lookup_place(&self, name: &str) -> Option<&PlaceLocal> {
        self.places.iter().rev().find(|place| place.name == name)
    }

    /// Record that the entry of the `place` addressed by `key_path` (its whole key-path
    /// as pre-evaluated slots, root-first) is known present from here (a dominating guard
    /// or a completed upsert). Idempotent.
    fn mark_present(&mut self, key_path: Vec<u16>) {
        if !self.present_places.contains(&key_path) {
            self.present_places.push(key_path);
        }
    }

    /// Whether a presence fact currently dominates the entry addressed by `key_path`.
    fn is_present_path(&self, key_path: &[u16]) -> bool {
        self.present_places.iter().any(|path| path == key_path)
    }

    /// Drop the presence fact on the entry addressed by `key_path` (its entry may no
    /// longer be present, e.g. after `delete p`).
    fn clear_present_path(&mut self, key_path: &[u16]) {
        self.present_places.retain(|path| path != key_path);
    }

    /// If `cond` is `exists(p)` over an in-scope named `place`, that place's whole
    /// key-path slots (root-first). The guarded (then) block may set the place's sparse
    /// fields in the strict form. Both root and branch places carry a strict-set
    /// presence consumer — the key-path form addresses either uniformly.
    fn exists_guard_path(&self, cond: &Expression) -> Option<Vec<u16>> {
        let Expression::Call { callee, args, .. } = cond else {
            return None;
        };
        let Expression::Name { segments, .. } = &**callee else {
            return None;
        };
        if segments.as_slice() != ["exists"] {
            return None;
        }
        let [arg] = args.as_slice() else {
            return None;
        };
        if arg.name.is_some() {
            return None;
        }
        let Expression::Name { segments, .. } = &arg.value else {
            return None;
        };
        let [name] = segments.as_slice() else {
            return None;
        };
        self.lookup_place(name).map(PlaceLocal::key_path_slots)
    }

    /// Whether `name` names an in-scope `place`.
    fn is_place_name(&self, expr: &Expression) -> bool {
        matches!(
            expr,
            Expression::Name { segments, .. }
                if matches!(segments.as_slice(), [name] if self.lookup_place(name).is_some())
        )
    }

    /// The durable shape of a place expression, extending [`Self::durable_shape`]
    /// with source-local `place` bindings: a bare place name is a whole-entry
    /// address, and a field access on a place name is a field address.
    /// Resolve a source managed-index read `^root.index[keys]` to its index and the
    /// bracket key operands, or `None` when the expression is not an index read (a
    /// `Keyed` whose base is a field of the store root naming a declared index). The
    /// index reference lives as long as the durable registry, so it may be held across a
    /// mutable lowering call.
    fn resolve_index_read<'e>(
        &self,
        expr: &'e Expression,
    ) -> Option<(&'a crate::durable::DurableIndex, &'e [Expression])> {
        let Expression::Keyed { base, keys, .. } = expr else {
            return None;
        };
        let Expression::Field {
            base: field_base,
            name,
            ..
        } = base.as_ref()
        else {
            return None;
        };
        let Expression::SavedRoot {
            name: root_name, ..
        } = field_base.as_ref()
        else {
            return None;
        };
        let durable: &'a DurableRegistry = self.durable;
        let root = durable.root()?;
        if root.name != *root_name {
            return None;
        }
        let index = root.index(name)?;
        Some((index, keys.as_slice()))
    }

    /// Lower a unique index's exact lookup `^root.index[keys]`: check the operands against
    /// the whole projection, then emit `DurIndexLookup`. The result is the optional source
    /// identity `Id(^root)?` — present with the matching entry's identity, or absent — which
    /// an `if const` head unwraps to a bare `Id(^root)`.
    fn lower_index_lookup(
        &mut self,
        index: &crate::durable::DurableIndex,
        keys: &[Expression],
        span: SourceSpan,
    ) -> Option<LTy> {
        if keys.len() != index.projection.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "unique index `{}` is looked up by its whole projection of {} key(s)",
                    index.name,
                    index.projection.len()
                ),
            ));
            return None;
        }
        let site = index.site;
        // The projection scalar types are copied out first so the operand lowering (a
        // mutable borrow of `self`) does not overlap the index borrow.
        let projection: Vec<ScalarType> = index.projection.clone();
        for (key, key_ty) in keys.iter().zip(&projection) {
            self.lower_as(key, LTy::bare_scalar(*key_ty))?;
        }
        self.push(Instr::DurIndexLookup(site), span);
        Some(LTy::Identity {
            root: 0,
            optional: true,
        })
    }

    fn durable_access(&self, expr: &Expression) -> Option<DurShape> {
        if let Some(shape) = self.durable_shape_here(expr) {
            return Some(shape);
        }
        match expr {
            Expression::Name { .. } if self.is_place_name(expr) => Some(DurShape::Entry),
            Expression::Field { base, .. } if self.is_place_name(base) => Some(DurShape::Field),
            _ => None,
        }
    }

    /// Resolve a source-local `place` access (`p` whole-entry, or `p.field`) to its
    /// pre-evaluated address, or `None` when `expr` is not a place access. A missing
    /// field is a precise diagnostic.
    fn resolve_place_access<'e>(&mut self, expr: &'e Expression) -> Option<DurablePlace<'e>> {
        match expr {
            Expression::Name { segments, span, .. } => {
                let [name] = segments.as_slice() else {
                    return None;
                };
                let place = self.lookup_place(name)?;
                Some(DurablePlace {
                    keys: place.bound_keys(),
                    target: DurTarget::Entry {
                        entry_site: place.entry_site,
                        record: place.record,
                    },
                    span: *span,
                })
            }
            Expression::Field {
                base,
                name: field_name,
                name_span,
                span,
                ..
            } => {
                let Expression::Name { segments, .. } = &**base else {
                    return None;
                };
                let [name] = segments.as_slice() else {
                    return None;
                };
                let place = self.lookup_place(name)?;
                // The field-exact site comes from the node the place addresses: a branch
                // place's branch record, or the flat root's record. Copy the key-path and
                // the scalar facts out before any diagnostic borrow of `self`.
                let keys = place.bound_keys();
                let nested = place.is_nested();
                let record = place.record;
                let field = if nested {
                    self.durable
                        .branch_by_record(record)
                        .and_then(|branch| branch.field(field_name))
                        .map(|field| (field.site, GArg::Scalar(field.scalar), field.required))
                } else {
                    self.durable
                        .root()
                        .and_then(|root| root.field(field_name))
                        .map(|field| (field.site, field.ty, field.required))
                };
                match field {
                    Some((site, ty, required)) => Some(DurablePlace {
                        keys,
                        target: DurTarget::Field { site, ty, required },
                        span: *span,
                    }),
                    None => {
                        let container = if nested {
                            self.durable
                                .branch_by_record(record)
                                .map(|branch| branch.name.clone())
                                .unwrap_or_default()
                        } else {
                            self.durable
                                .root()
                                .map(|root| root.name.clone())
                                .unwrap_or_default()
                        };
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            *name_span,
                            format!("`{container}` has no field `{field_name}`"),
                        ));
                        None
                    }
                }
            }
            _ => None,
        }
    }

    /// Emit one key column of a durable operation: lower the inline key expression
    /// (evaluating it here) or read the `place`'s pre-evaluated key slot.
    fn emit_key(&mut self, key: PlaceKey, key_ty: ScalarType, span: SourceSpan) -> Option<()> {
        match key {
            PlaceKey::Expr(expr) => self.lower_as(expr, LTy::bare_scalar(key_ty)),
            PlaceKey::Bound(slot) => {
                self.push(Instr::LocalGet(slot), span);
                Some(())
            }
            // Lower the identity, then spread it into the root's key columns. The one
            // `Identity` key supplies the whole root key-path, so this pushes every root
            // key column, matching the entry site's key arity.
            PlaceKey::Identity(expr) => {
                self.lower_as(
                    expr,
                    LTy::Identity {
                        root: 0,
                        optional: false,
                    },
                )?;
                let cols = self.durable.root().map_or(0, |root| root.key.len()) as u16;
                self.push(Instr::IdentityKeyPath(cols), span);
                Some(())
            }
        }
    }

    /// Emit a durable operation's whole key-path, root column first, so the innermost
    /// key is left on top — the order the kernel's `pop_key_path` reads back to a
    /// root-first path. A one-column path is a root address; a two-column path a branch.
    fn emit_key_path(&mut self, keys: &[DurKey], span: SourceSpan) -> Option<()> {
        for column in keys {
            self.emit_key(column.key, column.key_ty, span)?;
        }
        Some(())
    }

    /// Lower `place name = ^root(key)`: evaluate the entry address's key tuple
    /// exactly once into a fresh local slot and record the binding. The binding is
    /// immutable and does not shadow an existing name; the target must be a whole
    /// durable entry address (not a field, another place, or a non-durable value).
    /// A place over a not-yet-executable root reports the same trough diagnostic as
    /// an inline operation over it.
    fn lower_place_binding(&mut self, name: &str, name_span: SourceSpan, place_expr: &Expression) {
        if is_reserved_builtin_name(name) {
            self.fail(reserved_builtin_name(self.file, name_span, name));
            return;
        }
        if self.lookup(name).is_some() || self.lookup_place(name).is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                name_span,
                format!("`{name}` is already bound in this scope"),
            ));
            return;
        }
        if !matches!(self.durable_access(place_expr), Some(DurShape::Entry)) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                place_expr.span(),
                "a `place` names a whole durable entry address such as `^root(key)`".to_string(),
            ));
            return;
        }
        let Some(place) = self.resolve_durable(place_expr) else {
            return;
        };
        let span = place.span;
        let DurTarget::Entry { entry_site, record } = place.target else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                place_expr.span(),
                "a `place` names a whole durable entry address such as `^root(key)`, not a field"
                    .to_string(),
            ));
            return;
        };
        // Evaluate each key column of the address exactly once into a fresh slot, root
        // column first, so every later operation through the place reads the slots
        // rather than re-running the key operands. A branch place binds two columns.
        let mut key_slots = Vec::with_capacity(place.keys.len());
        for column in place.keys {
            let PlaceKey::Expr(key_expr) = column.key else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    place_expr.span(),
                    "a `place` names a store address `^root(key)`, not another place".to_string(),
                ));
                return;
            };
            let key_slot = self.alloc_slot();
            if self
                .lower_as(key_expr, LTy::bare_scalar(column.key_ty))
                .is_none()
            {
                return;
            }
            self.push(Instr::LocalSet(key_slot), span);
            key_slots.push((key_slot, column.key_ty));
        }
        self.places.push(PlaceLocal {
            name: name.to_string(),
            key_slots,
            entry_site,
            record,
        });
    }

    /// Resolve a durable place against the store root, reporting a diagnostic on a
    /// bad root name, key arity, or field name. The returned place holds no borrow of
    /// the registry.
    fn resolve_durable<'e>(&mut self, expr: &'e Expression) -> Option<DurablePlace<'e>> {
        // A source-local named place resolves through its pre-evaluated address.
        let is_place_access = match expr {
            Expression::Name { .. } => self.is_place_name(expr),
            Expression::Field { base, .. } => self.is_place_name(base),
            _ => false,
        };
        if is_place_access {
            return self.resolve_place_access(expr);
        }
        let Some(root) = self.durable.root() else {
            let diagnostic = self.no_executable_root_diagnostic(
                expr.span(),
                "durable access without a declared store",
            );
            self.fail(diagnostic);
            return None;
        };
        match expr {
            // A whole-entry address `^root[key].b1[k1]….bn[kn]` at any depth.
            Expression::Keyed { span, .. } => {
                let (keys, node) = self.resolve_entry_address(root, expr)?;
                Some(DurablePlace {
                    keys,
                    target: DurTarget::Entry {
                        entry_site: node.entry_site(),
                        record: node.record(),
                    },
                    span: *span,
                })
            }
            // A field-exact address `<entry-address>.field`, a whole root-level group
            // `<root-address>.group`, or a group-leaf address `<root-address>.group.leaf`.
            Expression::Field {
                base,
                name: field_name,
                name_span,
                span,
                ..
            } => {
                // A group-leaf address: the base resolves to a root-level group, and this
                // selector names one of its leaves. Resolved before the entry-address forms
                // because its base is a group address, not an entry address.
                if let Some((keys, group)) = self.resolve_group_address(root, base) {
                    let Some((slot, leaf)) = group.field_index(field_name) else {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            *name_span,
                            format!("group `{}` has no field `{field_name}`", group.name),
                        ));
                        return None;
                    };
                    return Some(DurablePlace {
                        keys,
                        target: DurTarget::GroupLeaf {
                            entry_site: group.entry_site,
                            slot,
                            ty: leaf.ty,
                            required: leaf.required,
                        },
                        span: *span,
                    });
                }
                let (keys, node) = self.resolve_entry_address(root, base)?;
                if let Some(field) = node.field(field_name) {
                    return Some(DurablePlace {
                        keys,
                        target: DurTarget::Field {
                            site: field.site,
                            ty: field.ty,
                            required: field.required,
                        },
                        span: *span,
                    });
                }
                // A whole root-level group address `^root(k).group`. Groups are executable
                // only at the root level, so only a root node offers one.
                if let DurNode::Root(root) = node
                    && let Some(group) = root.group(field_name)
                {
                    return Some(DurablePlace {
                        keys,
                        target: DurTarget::Group {
                            entry_site: group.entry_site,
                            record: group.record,
                        },
                        span: *span,
                    });
                }
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    *name_span,
                    node.no_field_message(field_name),
                ));
                None
            }
            _ => None,
        }
    }

    /// Resolve a durable group address `^root(k).group` to its root key-path and the
    /// addressed root-level group, or `None` when `expr` is not a group address. Only a
    /// syntactic entry-address base is followed, and only a root node offers groups, so a
    /// field or branch selector resolves cleanly to `None` without a diagnostic — the
    /// caller falls through to the entry-address forms.
    fn resolve_group_address<'e>(
        &mut self,
        root: &'a crate::durable::DurableRoot,
        expr: &'e Expression,
    ) -> Option<(Vec<DurKey<'e>>, &'a crate::durable::DurableGroup)> {
        let Expression::Field { base, name, .. } = expr else {
            return None;
        };
        if !is_entry_address(base) {
            return None;
        }
        let (keys, node) = self.resolve_entry_address(root, base)?;
        let DurNode::Root(root) = node else {
            return None;
        };
        let group = root.group(name)?;
        Some((keys, group))
    }

    /// Resolve a durable whole-entry address expression `^root[key].b1[k1]….bn[kn]` into
    /// its key-path (root-first, one column per hop) and the addressed node, walking the
    /// nested branch chain level by level. Returns `None` on a shape that is not an entry
    /// address, and reports a diagnostic then `None` on a bad root or branch name. The
    /// key-path columns are pushed root-first so the innermost key is on top, the order the
    /// kernel's `pop_key_path` expects.
    fn resolve_entry_address<'e>(
        &mut self,
        root: &'a crate::durable::DurableRoot,
        expr: &'e Expression,
    ) -> Option<(Vec<DurKey<'e>>, DurNode<'a>)> {
        let Expression::Keyed {
            base, keys, span, ..
        } = expr
        else {
            return None;
        };
        match &**base {
            // The base case `^root[k1, …]`: the root whole-entry address, one key operand
            // per root key column in declaration order.
            Expression::SavedRoot {
                name,
                span: root_span,
            } => {
                self.check_root_name(root, name, *root_span)?;
                // `^root[id]`: one entry-identity operand supplies the whole root key
                // tuple. The identity is spread into the root's key columns at emit, so a
                // single `Identity` key stands for every root column (including a composite
                // key). A per-column key list keeps the ordinary scalar path.
                if let [only] = keys.as_slice()
                    && self.identity_operand_root(only) == Some(0)
                {
                    let columns = vec![DurKey {
                        key: PlaceKey::Identity(only),
                        key_ty: root.key[0],
                    }];
                    return Some((columns, DurNode::Root(root)));
                }
                let mut columns = Vec::new();
                self.push_key_columns(&mut columns, keys, &root.key, *span)?;
                Some((columns, DurNode::Root(root)))
            }
            // The recursive case `<entry-address>.branch[bk1, …]`: extend the parent
            // entry's key-path with this branch's own key columns in declaration order.
            Expression::Field {
                base: parent_base,
                name: branch_name,
                name_span: branch_span,
                ..
            } => {
                let (mut columns, parent) = self.resolve_entry_address(root, parent_base)?;
                let Some(branch) = parent.branch(branch_name) else {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *branch_span,
                        parent.no_branch_message(branch_name),
                    ));
                    return None;
                };
                self.push_key_columns(&mut columns, keys, &branch.key, *span)?;
                Some((columns, DurNode::Branch(branch)))
            }
            _ => None,
        }
    }

    /// Match the positional key operands of one node against its ordered key columns,
    /// pushing one [`DurKey`] per column onto `columns` in declaration order (so the whole
    /// key-path is assembled root-first, column order, the order the kernel expects).
    /// Reports a diagnostic and returns `None` on a wrong operand count. The keyed-access
    /// grammar already forbids a named key, so only arity is checked here.
    fn push_key_columns<'e>(
        &mut self,
        columns: &mut Vec<DurKey<'e>>,
        keys: &'e [Expression],
        key_columns: &[ScalarType],
        span: SourceSpan,
    ) -> Option<()> {
        if keys.len() != key_columns.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "a store access takes {} positional key column(s), one per key column",
                    key_columns.len()
                ),
            ));
            return None;
        }
        for (key, &key_ty) in keys.iter().zip(key_columns) {
            columns.push(DurKey {
                key: PlaceKey::Expr(key),
                key_ty,
            });
        }
        Some(())
    }

    fn check_root_name(
        &mut self,
        root: &crate::durable::DurableRoot,
        name: &str,
        span: SourceSpan,
    ) -> Option<()> {
        if root.name == name {
            Some(())
        } else {
            self.fail(name_error(self.file, span, name));
            None
        }
    }

    /// The store-root index a key operand names when it is statically an entry identity:
    /// a binding of identity type (`^root[id]`), or an `Id(^root, …)` constructor call
    /// (`^root[Id(^root, k)]`). `None` for any other operand — an ordinary scalar key.
    /// Non-emitting: it only inspects the binding environment and the call spelling.
    fn identity_operand_root(&self, expr: &Expression) -> Option<u16> {
        match expr {
            Expression::Name { segments, .. } => match segments.as_slice() {
                [name] => self.lookup(name).and_then(|local| local.ty.bare_identity()),
                _ => None,
            },
            Expression::Call { callee, .. } => match &**callee {
                Expression::Name { segments, .. } if matches!(segments.as_slice(), [n] if n == "Id") => {
                    Some(0)
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Lower a durable read (`^r(k)` entry, `^r(k).branch(bk)` branch entry, `^r(k).f`
    /// field, or the place forms).
    fn lower_durable_read(&mut self, place: DurablePlace) -> Option<LTy> {
        self.emit_key_path(&place.keys, place.span)?;
        Some(match place.target {
            DurTarget::Entry { entry_site, record } => {
                self.push(Instr::DurReadEntry(entry_site), place.span);
                LTy::Record {
                    ty: record,
                    optional: true,
                }
            }
            DurTarget::Field { site, ty, .. } => {
                self.push(Instr::DurReadField(site), place.span);
                garg_to_lty(ty).to_optional()
            }
            // A whole root-level group materializes as one optional group record: the
            // group's own leaves, present exactly when the entry is present.
            DurTarget::Group { entry_site, record } => {
                self.push(Instr::DurReadGroup(entry_site), place.span);
                LTy::Record {
                    ty: record,
                    optional: true,
                }
            }
            // A group leaf reads as group-read-then-project: materialize the whole group,
            // then project the leaf slot. An absent entry (absent group) short-circuits to
            // a vacant of the leaf's optional type; a present group yields the leaf wrapped
            // optional (a required leaf is `SomeWrap`ped, a sparse leaf already reads `T?`).
            DurTarget::GroupLeaf {
                entry_site,
                slot,
                ty,
                required,
                ..
            } => {
                self.push(Instr::DurReadGroup(entry_site), place.span);
                let result = garg_to_lty(ty).to_optional();
                let to_absent = self.push_branch_present(place.span);
                self.push(Instr::FieldGet(slot), place.span);
                if required {
                    self.push(Instr::SomeWrap, place.span);
                }
                let to_end = self.push_jump(place.span);
                let absent = self.here();
                self.patch(to_absent, absent);
                self.push(Instr::VacantLoad(result.image()), place.span);
                let end = self.here();
                self.patch(to_end, end);
                result
            }
        })
    }

    /// Lower `exists(place)`: the presence of the cell the place addresses, or — when the
    /// argument is a store root or a keyed branch family rather than one addressed cell —
    /// whether that family has at least one payload-bearing child. A specific entry or
    /// field address (`^root(key)`, `^root(key).field`, a named `place`) is a keyed
    /// presence probe; a store root (`^root`) or a keyed branch family (`^root(key).notes`)
    /// is the family-populated probe.
    fn lower_exists(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`exists` takes one store place".to_string(),
            ));
            return None;
        };
        // A family argument (a store root, or a keyed branch family whose tail names a
        // declared branch) is the family-populated probe: it names no immediate child key,
        // so it reuses the traversal place resolver and emits only the ancestor key-path.
        // A scalar-field tail is not a family — it falls through to the keyed cell probe.
        if self.arg_is_family(&arg.value) {
            let target = self.resolve_traversal_place(&arg.value)?;
            self.emit_key_path(&target.ancestor_keys, target.span)?;
            self.push(Instr::DurFamilyExists(target.entry_site), span);
            return Some(LTy::bare_scalar(ScalarType::Bool));
        }
        // A specific addressed cell (an entry or a field) probes that one cell's presence.
        if self.durable_access(&arg.value).is_some() {
            let place = self.resolve_durable(&arg.value)?;
            self.emit_key_path(&place.keys, place.span)?;
            let site = match place.target {
                DurTarget::Entry { entry_site, .. } => entry_site,
                DurTarget::Field { site, .. } => site,
                // A group is markerless — its presence is the entry's presence — so a
                // group-cell presence probe has no distinct meaning yet; a group leaf has no
                // site of its own. Probe the containing entry instead.
                DurTarget::Group { .. } | DurTarget::GroupLeaf { .. } => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckUnsupported.as_str(),
                        self.file,
                        arg.value.span(),
                        "`exists` over a group or a group leaf is not supported; probe the \
                         containing entry `^root(key)`"
                            .to_string(),
                    ));
                    return None;
                }
            };
            self.push(Instr::DurExists(site), place.span);
            return Some(LTy::bare_scalar(ScalarType::Bool));
        }
        self.fail(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            self.file,
            arg.value.span(),
            "`exists` takes a store place such as `^root(key)`, a field, a store root, or a \
             keyed branch family"
                .to_string(),
        ));
        None
    }

    /// Lower `Id(^root, keys…)`: construct the entry identity of the declared store
    /// root from its explicit key columns, without reading the store. The first
    /// argument is the saved-root reference `^root`; the rest are one value per key
    /// column in declaration order, each checked against that column's scalar type. The
    /// key operands are pushed root-first, then `MakeIdentity` wraps them into the
    /// `Id(^root)` value.
    fn lower_identity_ctor(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        if args.iter().any(|arg| arg.name.is_some()) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`Id` takes positional arguments: a store root then one value per key column"
                    .to_string(),
            ));
            return None;
        }
        let Some((root_arg, key_args)) = args.split_first() else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`Id` takes a store root `^root` then one value per key column".to_string(),
            ));
            return None;
        };
        let Expression::SavedRoot {
            name: root_name,
            span: root_span,
        } = &root_arg.value
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                root_arg.value.span(),
                "`Id`'s first argument is the store root `^root`".to_string(),
            ));
            return None;
        };
        let Some(root) = self.durable.root() else {
            let diagnostic =
                self.no_executable_root_diagnostic(*root_span, "an entry identity without a store");
            self.fail(diagnostic);
            return None;
        };
        self.check_root_name(root, root_name, *root_span)?;
        let key_columns = root.key.clone();
        if key_args.len() != key_columns.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "`Id(^{root_name}, …)` takes {} key column value(s), one per key column",
                    key_columns.len()
                ),
            ));
            return None;
        }
        // Push each key column root-first in declaration order, coerced to the column's
        // scalar type, so `MakeIdentity` pops them into the tuple in column order.
        for (arg, &key_ty) in key_args.iter().zip(&key_columns) {
            self.lower_as(&arg.value, LTy::bare_scalar(key_ty))?;
        }
        self.push(
            Instr::MakeIdentity {
                root: 0,
                cols: key_columns.len() as u16,
            },
            span,
        );
        Some(LTy::Identity {
            root: 0,
            optional: false,
        })
    }

    /// Lower a call in the closed pure text floor: `isEmpty(string): bool`,
    /// `contains(string, string): bool`, `trim(string): string`. One owner for the
    /// whole floor; there is no general string library.
    fn lower_text_builtin(
        &mut self,
        name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let text = LTy::bare_scalar(ScalarType::Text);
        let bool_ty = LTy::bare_scalar(ScalarType::Bool);
        let (arity, instr, result): (usize, Instr, LTy) = match name {
            "isEmpty" => (1, Instr::TextIsEmpty, bool_ty),
            "contains" => (2, Instr::TextContains, bool_ty),
            "trim" => (1, Instr::TextTrim, text),
            _ => unreachable!("caller matched the text-floor names"),
        };
        if args.len() != arity || args.iter().any(|arg| arg.name.is_some()) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("`{name}` takes {arity} positional string argument(s)"),
            ));
            return None;
        }
        for arg in args {
            self.lower_as(&arg.value, text)?;
        }
        self.push(instr, span);
        Some(result)
    }

    /// Lower a collection-returning text-floor call: `split(text, sep): List[string]`
    /// or `lines(text): List[string]`. Both mint (and reuse) the one `List[string]`
    /// COLLTYPES instantiation and emit the split/lines opcode carrying it; the VM
    /// bounds the result by the same law-9 collection limits `append` observes.
    fn lower_text_split(&mut self, name: &str, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let text = LTy::bare_scalar(ScalarType::Text);
        let arity = if name == "split" { 2 } else { 1 };
        if args.len() != arity || args.iter().any(|arg| arg.name.is_some()) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("`{name}` takes {arity} positional string argument(s)"),
            ));
            return None;
        }
        for arg in args {
            self.lower_as(&arg.value, text)?;
        }
        let idx = self
            .records
            .instantiate_list(self.draft, GArg::Scalar(ScalarType::Text));
        let instr = if name == "split" {
            Instr::TextSplit(idx)
        } else {
            Instr::TextLines(idx)
        };
        self.push(instr, span);
        Some(LTy::Collection {
            idx,
            optional: false,
        })
    }

    /// Lower `join(parts: List[string], sep: string): string`: concatenate the list's
    /// text elements with a separator. A first argument that is not a `List[string]`
    /// is a typed diagnostic; the VM bounds the result by the `run.text_limit`
    /// concatenation ceiling.
    fn lower_text_join(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let text = LTy::bare_scalar(ScalarType::Text);
        if args.len() != 2 || args.iter().any(|arg| arg.name.is_some()) {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`join` takes 2 positional argument(s): a list of string and a separator"
                    .to_string(),
            ));
            return None;
        }
        let idx = self.collection_arg(&args[0].value)?;
        match self.records.collection_spec(idx) {
            CollSpec::List {
                elem: GArg::Scalar(ScalarType::Text),
            } => {}
            _ => {
                self.fail(unsupported(
                    self.file,
                    args[0].value.span(),
                    "`join` on this type (it joins a list of string)",
                ));
                return None;
            }
        }
        self.lower_as(&args[1].value, text)?;
        self.push(Instr::TextJoin, span);
        Some(text)
    }

    /// Lower an empty-collection constructor `List()`/`Map()` against the expected
    /// type: the expected `Collection` supplies the exact instantiation, so the
    /// constructor emits the `ListNew`/`MapNew` for that COLLTYPES index. A `List()`
    /// against a `Map` type (or the reverse), or against a non-collection type, is a
    /// typed diagnostic.
    fn lower_collection_ctor(&mut self, head: &str, span: SourceSpan, expected: LTy) -> Option<()> {
        let LTy::Collection {
            idx,
            optional: false,
        } = expected
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "`{head}()` constructs a collection, but {} is expected here",
                    expected.spelling(self.records)
                ),
            ));
            return None;
        };
        let (instr, ok) = match (head, self.records.collection_spec(idx)) {
            ("List", CollSpec::List { .. }) => (Instr::ListNew(idx), true),
            ("Map", CollSpec::Map { .. }) => (Instr::MapNew(idx), true),
            _ => (Instr::ListNew(idx), false),
        };
        if !ok {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "`{head}()` does not construct {}",
                    self.records.collection_spelling(idx)
                ),
            ));
            return None;
        }
        self.push(instr, span);
        Some(())
    }

    /// Lower `isEmpty(x)` over a string or a finite collection. A string routes to
    /// the text floor; a `List`/`Map` lowers to `length(x) == 0`.
    fn lower_is_empty(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let [arg] = args else {
            self.fail(builtin_arity(self.file, span, "isEmpty", 1));
            return None;
        };
        if arg.name.is_some() {
            self.fail(builtin_arity(self.file, span, "isEmpty", 1));
            return None;
        }
        let ty = self.lower_expr(&arg.value)?;
        match ty {
            LTy::Scalar {
                scalar: ScalarType::Text,
                optional: false,
            } => {
                self.push(Instr::TextIsEmpty, span);
                Some(LTy::bare_scalar(ScalarType::Bool))
            }
            LTy::Collection {
                idx,
                optional: false,
            } => {
                let len = match self.records.collection_spec(idx) {
                    CollSpec::List { .. } => Instr::ListLen,
                    CollSpec::Map { .. } => Instr::MapLen,
                };
                self.push(len, span);
                let zero = self.draft.intern_int(0);
                self.push(Instr::ConstLoad(zero.index()), span);
                self.push(Instr::EqInt, span);
                Some(LTy::bare_scalar(ScalarType::Bool))
            }
            _ => {
                self.fail(unsupported(
                    self.file,
                    arg.value.span(),
                    "`isEmpty` on this type (it accepts a string, list, or map)",
                ));
                None
            }
        }
    }

    /// Lower `length(x): int` over a finite collection: the element or entry count.
    fn lower_length(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let [arg] = args else {
            self.fail(builtin_arity(self.file, span, "length", 1));
            return None;
        };
        if arg.name.is_some() {
            self.fail(builtin_arity(self.file, span, "length", 1));
            return None;
        }
        let idx = self.collection_arg(&arg.value)?;
        let len = match self.records.collection_spec(idx) {
            CollSpec::List { .. } => Instr::ListLen,
            CollSpec::Map { .. } => Instr::MapLen,
        };
        self.push(len, span);
        Some(LTy::bare_scalar(ScalarType::Int))
    }

    /// Lower `append(list, value): List<T>`: append `value` after the last element,
    /// yielding the grown list (collections are values). A non-list first argument,
    /// or a `value` not of the element type, is a typed diagnostic.
    fn lower_append(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let [list_arg, value_arg] = args else {
            self.fail(builtin_arity(self.file, span, "append", 2));
            return None;
        };
        if args.iter().any(|arg| arg.name.is_some()) {
            self.fail(builtin_arity(self.file, span, "append", 2));
            return None;
        }
        let idx = self.collection_arg(&list_arg.value)?;
        let CollSpec::List { elem } = self.records.collection_spec(idx) else {
            self.fail(unsupported(
                self.file,
                list_arg.value.span(),
                "`append` on a map (a map is updated with `insert`)",
            ));
            return None;
        };
        self.lower_as(&value_arg.value, garg_to_lty(elem))?;
        self.push(Instr::ListAppend, span);
        Some(LTy::Collection {
            idx,
            optional: false,
        })
    }

    /// Lower `insert(map, key, value): Map<K, V>`: insert or replace `value` at
    /// `key`, yielding the updated map. A non-map first argument, or a key/value not
    /// of the map's types, is a typed diagnostic.
    fn lower_insert(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let [map_arg, key_arg, value_arg] = args else {
            self.fail(builtin_arity(self.file, span, "insert", 3));
            return None;
        };
        if args.iter().any(|arg| arg.name.is_some()) {
            self.fail(builtin_arity(self.file, span, "insert", 3));
            return None;
        }
        let idx = self.collection_arg(&map_arg.value)?;
        let CollSpec::Map { key, value } = self.records.collection_spec(idx) else {
            self.fail(unsupported(
                self.file,
                map_arg.value.span(),
                "`insert` on a list (a list is grown with `append`)",
            ));
            return None;
        };
        self.lower_as(&key_arg.value, garg_to_lty(key))?;
        self.lower_as(&value_arg.value, garg_to_lty(value))?;
        self.push(Instr::MapInsert, span);
        Some(LTy::Collection {
            idx,
            optional: false,
        })
    }

    /// Lower `get(map, key): V?`: the value at `key`, present when the key is in the
    /// map and absent otherwise (presence-typed per the `T?` primitive).
    fn lower_map_get(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let [map_arg, key_arg] = args else {
            self.fail(builtin_arity(self.file, span, "get", 2));
            return None;
        };
        if args.iter().any(|arg| arg.name.is_some()) {
            self.fail(builtin_arity(self.file, span, "get", 2));
            return None;
        }
        let idx = self.collection_arg(&map_arg.value)?;
        let CollSpec::Map { key, value } = self.records.collection_spec(idx) else {
            self.fail(unsupported(
                self.file,
                map_arg.value.span(),
                "`get` on a list (a list has no key lookup)",
            ));
            return None;
        };
        self.lower_as(&key_arg.value, garg_to_lty(key))?;
        self.push(Instr::MapGet, span);
        Some(garg_to_lty(value).to_optional())
    }

    /// Lower an expression that must be a bare collection, returning its COLLTYPES
    /// index. A non-collection value is a typed diagnostic.
    fn collection_arg(&mut self, expr: &Expression) -> Option<u16> {
        let ty = self.lower_expr(expr)?;
        match ty {
            LTy::Collection {
                idx,
                optional: false,
            } => Some(idx),
            other => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    expr.span(),
                    format!(
                        "expected a list or map here, found {}",
                        other.spelling(self.records)
                    ),
                ));
                None
            }
        }
    }

    /// Lower a closed scalar conversion `target(value)`. The admitted set is
    /// `string(int)`, `string(bool)`, and `bytes(string)`; any other conversion is a
    /// typed `check.unsupported` on the beta line.
    /// Lower a temporal constructor `date("…")` / `instant("…")` / `duration("…")`.
    /// Construction is from exactly one static string literal, validated and folded
    /// at compile time: a malformed or out-of-range canonical form is a typed
    /// `check.type` diagnostic here, so no ordinary program produces an out-of-range
    /// temporal value at runtime. The folded raw scalar is interned as a temporal
    /// constant. `marrow-temporal` owns the canonical text grammar.
    fn lower_temporal_construct(
        &mut self,
        scalar: ScalarType,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let spelling = scalar.spelling();
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("`{spelling}` takes one string-literal argument"),
            ));
            return None;
        };
        if arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                format!("the `{spelling}` argument is positional"),
            ));
            return None;
        }
        // A temporal value is constructed only from a static string literal, so its
        // canonical form is validated once at compile time rather than parsed at
        // runtime (there is no ambient clock or runtime temporal parse in the floor).
        let Expression::Literal {
            kind: LiteralKind::String,
            text,
            span: arg_span,
        } = &arg.value
        else {
            self.fail(unsupported(
                self.file,
                arg.value.span(),
                &format!("constructing a `{spelling}` from a non-literal value"),
            ));
            return None;
        };
        let Ok(decoded) = decode_string_literal(text) else {
            self.fail(unsupported(self.file, *arg_span, "this string literal"));
            return None;
        };
        let bytes = decoded.as_bytes();
        let const_id = match scalar {
            ScalarType::Date => match marrow_temporal::parse_date(bytes) {
                Some(days) => self.draft.intern_date(days),
                None => return self.fail_temporal_literal(scalar, &decoded, *arg_span),
            },
            ScalarType::Instant => match marrow_temporal::parse_instant(bytes) {
                Some(nanos) => self.draft.intern_instant(nanos),
                None => return self.fail_temporal_literal(scalar, &decoded, *arg_span),
            },
            ScalarType::Duration => match marrow_temporal::parse_duration(bytes) {
                Some(nanos) => self.draft.intern_duration(nanos),
                None => return self.fail_temporal_literal(scalar, &decoded, *arg_span),
            },
            _ => unreachable!("caller passes only a temporal scalar"),
        };
        self.push(Instr::ConstLoad(const_id.index()), span);
        Some(LTy::bare_scalar(scalar))
    }

    /// Report a malformed or out-of-range temporal literal and return `None`.
    fn fail_temporal_literal(
        &mut self,
        scalar: ScalarType,
        value: &str,
        span: SourceSpan,
    ) -> Option<LTy> {
        let form = match scalar {
            ScalarType::Date => "a canonical date `YYYY-MM-DD` in years 0001-9999",
            ScalarType::Instant => {
                "a canonical UTC instant `YYYY-MM-DDTHH:MM:SS[.fraction]Z` in years 0001-9999"
            }
            ScalarType::Duration => "a canonical duration `[-]PT<seconds>[.fraction]S`",
            _ => unreachable!("caller passes only a temporal scalar"),
        };
        self.fail(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            self.file,
            span,
            format!(
                "`{value}` is not {form}, so it is not a `{}` literal",
                scalar.spelling()
            ),
        ));
        None
    }

    /// Lower `addDays(date, int): date` or `daysBetween(date, date): int`,
    /// emitting the checked temporal instruction after type-checking the operands.
    fn lower_date_arith(
        &mut self,
        builtin: Builtin,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let (name, second, instr, result) = match builtin {
            Builtin::DateAddDays => (
                "addDays",
                ScalarType::Int,
                Instr::DateAddDays,
                ScalarType::Date,
            ),
            Builtin::DateDaysBetween => (
                "daysBetween",
                ScalarType::Date,
                Instr::DateDaysBetween,
                ScalarType::Int,
            ),
            _ => unreachable!("caller passes only a date-arithmetic builtin"),
        };
        let [first_arg, second_arg] = args else {
            self.fail(builtin_arity(self.file, span, name, 2));
            return None;
        };
        if first_arg.name.is_some() || second_arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("`{name}` arguments are positional"),
            ));
            return None;
        }
        self.expect_bare_scalar(&first_arg.value, ScalarType::Date, name)?;
        self.expect_bare_scalar(&second_arg.value, second, name)?;
        self.push(instr, span);
        Some(LTy::bare_scalar(result))
    }

    /// Lower `expr` and require it to be exactly the bare scalar `expected`, failing
    /// with a `check.type` diagnostic (naming `builtin`) otherwise.
    fn expect_bare_scalar(
        &mut self,
        expr: &Expression,
        expected: ScalarType,
        builtin: &str,
    ) -> Option<()> {
        let ty = self.lower_expr(expr)?;
        if ty == LTy::bare_scalar(expected) {
            return Some(());
        }
        self.fail(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            self.file,
            expr.span(),
            format!(
                "`{builtin}` expects a `{}` argument, found `{}`",
                expected.spelling(),
                ty.spelling(self.records)
            ),
        ));
        None
    }

    fn lower_conversion(
        &mut self,
        target: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("`{target}` conversion takes one value"),
            ));
            return None;
        };
        if arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                "a conversion argument is positional".to_string(),
            ));
            return None;
        }
        let source = self.lower_expr(&arg.value)?;
        use ScalarType::{Bool, Bytes, Int, Text};
        let (instr, result) = match (target, source.bare_scalar_type()) {
            ("string", Some(Int)) => (Instr::ConvStringInt, Text),
            ("string", Some(Bool)) => (Instr::ConvStringBool, Text),
            ("bytes", Some(Text)) => (Instr::ConvBytesText, Bytes),
            _ => {
                self.fail(unsupported(
                    self.file,
                    span,
                    &format!("converting {} to {target}", source.spelling(self.records)),
                ));
                return None;
            }
        };
        self.push(instr, span);
        Some(LTy::bare_scalar(result))
    }

    /// Lower `unreachable("static text")`: the sole application-invariant fault. It
    /// takes exactly one static string literal, emits a fault instruction carrying
    /// that text, and diverges (control never continues past it).
    fn lower_unreachable(&mut self, args: &[Argument], span: SourceSpan) -> Option<CallResult> {
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`unreachable` takes one static string literal".to_string(),
            ));
            return None;
        };
        if arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                "`unreachable` takes one positional static string literal".to_string(),
            ));
            return None;
        }
        let Expression::Literal {
            kind: LiteralKind::String,
            text,
            span: lit_span,
        } = &arg.value
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                "`unreachable` requires a static string literal, not a computed value".to_string(),
            ));
            return None;
        };
        let Ok(decoded) = decode_string_literal(text) else {
            self.fail(unsupported(self.file, *lit_span, "this string literal"));
            return None;
        };
        let const_id = self.draft.intern_text(&decoded);
        self.push(Instr::Unreachable(const_id.index()), span);
        Some(CallResult::Diverges)
    }

    /// Lower a durable assignment: a whole-entry upsert (root or branch) or a root
    /// field set.
    fn lower_durable_assign(&mut self, place: DurablePlace, value: &Expression) {
        match place.target {
            DurTarget::Entry { entry_site, record } => {
                let root_slot = place.root_bound_slot();
                if self
                    .lower_upsert(&place.keys, entry_site, record, value, place.span)
                    .is_some()
                    && let Some(slot) = root_slot
                {
                    // A root upsert leaves the root entry present on every path from
                    // here, so subsequent sparse sets through the root place lower to the
                    // strict form. A branch upsert (multi-column key-path) has no bound
                    // single root slot here and marks nothing — a guarded branch set uses
                    // the `exists`/`if const` presence path instead.
                    self.mark_present(vec![slot]);
                }
            }
            DurTarget::Field { site, ty, required } => {
                // A sparse set through a `place` a presence fact dominates lowers to the
                // strict present-entry form: it reads the containing entry's whole
                // key-path from the place's pre-evaluated slots and asserts the entry is
                // present, so it pushes no key operand. A root or a branch field is
                // handled uniformly by the key-path. Every other field set keeps the bare
                // form (create-or-reconcile at commit for a sparse set).
                let bare = garg_to_lty(ty);
                if !required
                    && let Some(key_slots) = place.bound_key_path()
                    && self.is_present_path(&key_slots)
                {
                    let expected = bare.to_optional();
                    if self.lower_as(value, expected).is_none() {
                        return;
                    }
                    self.push(Instr::DurSetSparsePresent { site, key_slots }, place.span);
                    return;
                }
                if self.emit_key_path(&place.keys, place.span).is_none() {
                    return;
                }
                let expected = if required { bare } else { bare.to_optional() };
                if self.lower_as(value, expected).is_none() {
                    return;
                }
                let instr = if required {
                    Instr::DurSetRequired(site)
                } else {
                    Instr::DurSetSparse(site)
                };
                self.push(instr, place.span);
            }
            // `^root(k).group = R.group(…)`: an exact whole-group replacement, group-scoped
            // (the entry's other groups, top-level fields, and branches are untouched). The
            // key-path is pushed first, then the group record, the order `DurReplaceGroup`
            // reads. A replace over an absent entry is Missing and touches nothing — a group
            // is a value unit of an existing entry, never created on its own.
            DurTarget::Group { entry_site, record } => {
                if self.emit_key_path(&place.keys, place.span).is_none() {
                    return;
                }
                if self
                    .lower_as(
                        value,
                        LTy::Record {
                            ty: record,
                            optional: false,
                        },
                    )
                    .is_none()
                {
                    return;
                }
                self.push(Instr::DurReplaceGroup(entry_site), place.span);
            }
            // `^root(k).group.leaf = value`: a whole-group read-modify-write.
            DurTarget::GroupLeaf {
                entry_site,
                slot,
                ty,
                ..
            } => {
                self.lower_group_leaf_rmw(
                    &place.keys,
                    entry_site,
                    slot,
                    GroupLeafEdit::Set { value, ty },
                    place.span,
                );
            }
        }
    }

    /// Lower a group-leaf read-modify-write `^root(k).group.leaf = value` or
    /// `delete ^root(k).group.leaf`: evaluate the key-path once into slots, read the whole
    /// group, and — only when the entry (and so the group) is present — rewrite the leaf
    /// slot (set present, or unset to vacant) on the materialized group record and replace
    /// the whole group. An absent entry short-circuits to a no-op: a group is a value unit
    /// of an existing entry, never created on its own. The group is materialized whole and
    /// written back, so a sibling leaf is preserved.
    fn lower_group_leaf_rmw(
        &mut self,
        keys: &[DurKey],
        entry_site: u16,
        slot: u16,
        edit: GroupLeafEdit,
        span: SourceSpan,
    ) -> Option<()> {
        // Evaluate each key column once into a fresh slot (root-first) so the read and the
        // replace key off the same evaluated columns.
        let mut key_slots = Vec::with_capacity(keys.len());
        for column in keys {
            let key_slot = match column.key {
                PlaceKey::Bound(existing) => existing,
                PlaceKey::Expr(_) => {
                    let key_slot = self.alloc_slot();
                    self.emit_key(column.key, column.key_ty, span)?;
                    self.push(Instr::LocalSet(key_slot), span);
                    key_slot
                }
                PlaceKey::Identity(_) => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckUnsupported.as_str(),
                        self.file,
                        span,
                        "writing a group leaf through an identity key is not yet supported"
                            .to_string(),
                    ));
                    return None;
                }
            };
            key_slots.push(key_slot);
        }
        // A set evaluates its bare leaf value once into a slot before the read, so the read
        // record is on top of the stack when the leaf op runs.
        let value_slot = match &edit {
            GroupLeafEdit::Set { value, ty } => {
                let value_slot = self.alloc_slot();
                self.lower_as(value, garg_to_lty(*ty))?;
                self.push(Instr::LocalSet(value_slot), span);
                Some(value_slot)
            }
            GroupLeafEdit::Unset => None,
        };
        // Read the group; present -> its materialized record is on the stack and the write
        // back runs; absent -> jump past the write back, a clean no-op (the group was never
        // there to modify).
        self.emit_slots(&key_slots, span);
        self.push(Instr::DurReadGroup(entry_site), span);
        let to_end = self.push_branch_present(span);
        // Present: rewrite the leaf slot on the materialized record, then replace the group.
        match edit {
            GroupLeafEdit::Set { .. } => {
                self.push(
                    Instr::LocalGet(value_slot.expect("a set evaluates its value")),
                    span,
                );
                self.push(Instr::FieldSet(slot), span);
            }
            GroupLeafEdit::Unset => {
                self.push(Instr::FieldUnset(slot), span);
            }
        }
        let rec_slot = self.alloc_slot();
        self.push(Instr::LocalSet(rec_slot), span);
        self.emit_slots(&key_slots, span);
        self.push(Instr::LocalGet(rec_slot), span);
        self.push(Instr::DurReplaceGroup(entry_site), span);
        let end = self.here();
        self.patch(to_end, end);
        Some(())
    }

    /// Lower `^r(k) = record` or `^r(k).branch(bk) = Resource.branch(...)` to the
    /// transaction-local presence branch (design §D): `DurExists` over the entry's whole
    /// key-path decides `replace` vs `create` against the coherent staged view. The
    /// key-path is materialized into slots (one per column, root first) so the exists,
    /// replace, and create ops all key off the same evaluated columns.
    fn lower_upsert(
        &mut self,
        keys: &[DurKey],
        entry_site: u16,
        record: TypeId,
        value: &Expression,
        span: SourceSpan,
    ) -> Option<()> {
        // A bound (place) column already holds its key in a pre-evaluated slot; reuse it
        // so the create/replace ops key off it (the verifier's presence lattice
        // recognizes a root create as establishing that slot's entry). An inline column
        // is evaluated once into a fresh slot.
        let mut key_slots = Vec::with_capacity(keys.len());
        for column in keys {
            let slot = match column.key {
                PlaceKey::Bound(slot) => slot,
                PlaceKey::Expr(_) => {
                    let slot = self.alloc_slot();
                    self.emit_key(column.key, column.key_ty, span)?;
                    self.push(Instr::LocalSet(slot), span);
                    slot
                }
                // Writing a whole entry through an identity key is not part of the
                // identity value slice (an identity is a read/lookup value here); a
                // whole-entry write keys by explicit columns.
                PlaceKey::Identity(_) => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckUnsupported.as_str(),
                        self.file,
                        span,
                        "writing a whole entry through an identity key is not yet supported"
                            .to_string(),
                    ));
                    return None;
                }
            };
            key_slots.push(slot);
        }
        let rec_slot = self.alloc_slot();
        self.lower_as(
            value,
            LTy::Record {
                ty: record,
                optional: false,
            },
        )?;
        self.push(Instr::LocalSet(rec_slot), span);

        self.emit_slots(&key_slots, span);
        self.push(Instr::DurExists(entry_site), span);
        let to_create = self.push_jif(span);
        // Present: replace.
        self.emit_slots(&key_slots, span);
        self.push(Instr::LocalGet(rec_slot), span);
        self.push(Instr::DurReplaceEntry(entry_site), span);
        let to_end = self.push_jump(span);
        // Absent: create.
        let create_at = self.here();
        self.patch(to_create, create_at);
        self.emit_slots(&key_slots, span);
        self.push(Instr::LocalGet(rec_slot), span);
        self.push(Instr::DurCreateEntry(entry_site), span);
        let end = self.here();
        self.patch(to_end, end);
        Some(())
    }

    /// Push a durable operation's key-path from pre-evaluated slots, root column first,
    /// so the innermost key lands on top — the order the kernel's `pop_key_path` reads.
    fn emit_slots(&mut self, slots: &[u16], span: SourceSpan) {
        for slot in slots {
            self.push(Instr::LocalGet(*slot), span);
        }
    }

    /// Lower `delete ^r(k)` / `delete ^r(k).branch(bk)` (entry payload erase) or
    /// `delete ^r(k).f` (sparse-field erase).
    fn lower_durable_delete(&mut self, path: &Expression, span: SourceSpan) {
        if self.durable_access(path).is_none() {
            self.fail(unsupported(self.file, span, "this delete target"));
            return;
        }
        let Some(place) = self.resolve_durable(path) else {
            return;
        };
        // A group-leaf clear is a whole-group read-modify-write (its key-path is evaluated
        // inside the helper), so it is handled before the shared single key-path emission.
        if let DurTarget::GroupLeaf {
            entry_site,
            slot,
            required,
            ..
        } = place.target
        {
            if required {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    place.span,
                    "a required group leaf cannot be deleted".to_string(),
                ));
                return;
            }
            self.lower_group_leaf_rmw(
                &place.keys,
                entry_site,
                slot,
                GroupLeafEdit::Unset,
                place.span,
            );
            return;
        }
        let key_path = place.bound_key_path();
        if self.emit_key_path(&place.keys, place.span).is_none() {
            return;
        }
        match place.target {
            DurTarget::Entry { entry_site, .. } => {
                self.push(Instr::DurEraseEntry(entry_site), place.span);
                // The entry's payload is gone; a later sparse set through the same place
                // must not assume presence.
                if let Some(path) = &key_path {
                    self.clear_present_path(path);
                }
            }
            DurTarget::Field { site, required, .. } => {
                if required {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        place.span,
                        "a required field cannot be deleted".to_string(),
                    ));
                    return;
                }
                self.push(Instr::DurEraseField(site), place.span);
            }
            // `delete ^root(k).group`: erase only that group's leaves; the entry's other
            // groups, top-level fields, and branches are untouched.
            DurTarget::Group { entry_site, .. } => {
                self.push(Instr::DurEraseGroup(entry_site), place.span);
            }
            DurTarget::GroupLeaf { .. } => {
                unreachable!("a group-leaf delete is handled before the shared key-path emit")
            }
        }
    }

    // --- type resolution ---

    fn resolve(&mut self, annotation: &TypeExpr) -> Option<LTy> {
        let env = TypeEnv {
            params: &self.type_env,
        };
        resolve_type(self.records, self.draft, self.durable, annotation, env)
    }

    fn param_type(&mut self, ty: &TypeExpr) -> Option<LTy> {
        let env = TypeEnv {
            params: &self.type_env,
        };
        match param_type(self.records, self.draft, self.durable, ty, env) {
            Some(param) => Some(param),
            None => {
                self.fail(unsupported(self.file, ty.span(), "this parameter type"));
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
/// nominal, or a bare `struct` value. Optionals, the durable resource record, and
/// unresolved names are outside the parameter subset. One owner for signature
/// building and body lowering, so the two can never disagree on a parameter's type.
fn param_type(
    records: &TypeRegistry,
    draft: &mut ImageDraft,
    durable: &DurableRegistry,
    ty: &TypeExpr,
    env: TypeEnv,
) -> Option<LTy> {
    match resolve_type(records, draft, durable, ty, env) {
        Some(
            param @ (LTy::Scalar {
                optional: false, ..
            }
            | LTy::Nominal {
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
        ) => Some(param),
        _ => None,
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
) -> Option<LTy> {
    resolve_expanded(records, draft, durable, &records.expand(annotation), env)
}

fn resolve_expanded(
    records: &TypeRegistry,
    draft: &mut ImageDraft,
    durable: &DurableRegistry,
    annotation: &TypeExpr,
    env: TypeEnv,
) -> Option<LTy> {
    match annotation {
        TypeExpr::Name { text, .. } => {
            // A type-parameter name resolves before scalar/named-type classification,
            // so a parameter cannot be shadowed by a same-named scalar spelling.
            if let Some((index, binding)) = env.lookup(text) {
                return Some(match binding {
                    ParamBinding::Abstract(_) => LTy::Param {
                        index,
                        optional: false,
                    },
                    ParamBinding::Concrete(arg) => garg_to_lty(arg),
                });
            }
            if let Some(scalar) = ScalarType::from_spelling(text) {
                Some(LTy::bare_scalar(scalar))
            } else if let Some((id, _)) = records.nominal_by_name(text) {
                Some(LTy::Nominal {
                    id,
                    optional: false,
                })
            } else if let Some(info) = records.struct_by_name(text) {
                Some(LTy::Struct {
                    ty: info.type_id,
                    optional: false,
                })
            } else if let Some(info) = records.enum_by_name(text) {
                Some(LTy::Enum {
                    ty: info.enum_id,
                    optional: false,
                })
            } else {
                records.by_name(text).map(|record| LTy::Record {
                    ty: record.type_id,
                    optional: false,
                })
            }
        }
        TypeExpr::Optional { inner, .. } => {
            let inner = resolve_expanded(records, draft, durable, inner, env)?;
            if inner.is_optional() {
                None
            } else {
                Some(inner.to_optional())
            }
        }
        TypeExpr::Apply { head, args, .. } => {
            resolve_generic(records, draft, durable, head, args, env)
        }
        // `Id(^root)`: the entry-identity value type of the one declared store root.
        // The root name must match the declared root; a program has zero or one root,
        // so the executable root is image ROOTS index 0. An identity over a root that
        // is not declared, or over a not-yet-executable root, is an unsupported type
        // (`None`), reported by the caller like any other unresolved annotation.
        TypeExpr::Identity(identity) => {
            let root = durable.root()?;
            if root.name != identity.root {
                return None;
            }
            Some(LTy::Identity {
                root: 0,
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
) -> Option<LTy> {
    match head {
        "List" => {
            let [elem] = args else { return None };
            let elem = resolve_expanded(records, draft, durable, elem, env)?.as_garg()?;
            Some(LTy::Collection {
                idx: records.instantiate_list(draft, elem),
                optional: false,
            })
        }
        "Map" => {
            let [key, value] = args else { return None };
            let key = resolve_expanded(records, draft, durable, key, env)?.as_garg()?;
            // A type parameter is not admitted as a map key: keys are drawn from the
            // durable-key scalar family, and a generic key would need an order
            // constraint the collection contract does not model here.
            if !key.is_key_type() {
                return None;
            }
            let value = resolve_expanded(records, draft, durable, value, env)?.as_garg()?;
            Some(LTy::Collection {
                idx: records.instantiate_map(draft, key, value),
                optional: false,
            })
        }
        _ => {
            let template = records.type_template_by_name(head)?;
            let params = records.template_type_params(template);
            if args.len() != params.len() {
                return None;
            }
            let mut resolved = Vec::with_capacity(args.len());
            for arg in args {
                resolved.push(resolve_expanded(records, draft, durable, arg, env)?.as_garg()?);
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
                        return None;
                    }
                }
            }
            match records.mint_type_instance(draft, template, &resolved)? {
                TypeInstId::Record(ty) => Some(LTy::Struct {
                    ty,
                    optional: false,
                }),
                TypeInstId::Enum(id) => Some(LTy::Enum {
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
fn unify_type_param(
    records: &TypeRegistry,
    type_params: &[(String, Option<TypeConstraint>)],
    annotation: &TypeExpr,
    got: LTy,
    subst: &mut [Option<GArg>],
) -> Result<(), String> {
    match annotation {
        TypeExpr::Name { text, .. } => {
            if let Some(index) = type_params.iter().position(|(name, _)| name == text) {
                if got.is_optional() {
                    return Err(format!(
                        "type parameter `{text}` matches a bare value, but the argument is `{}`",
                        got.spelling(records)
                    ));
                }
                let arg = got.as_garg().ok_or_else(|| {
                    format!(
                        "`{}` is not a value type that can instantiate `{text}`",
                        got.spelling(records)
                    )
                })?;
                match subst[index] {
                    None => subst[index] = Some(arg),
                    Some(previous) if previous == arg => {}
                    Some(previous) => {
                        return Err(format!(
                            "type parameter `{text}` is inferred as both `{}` and `{}`",
                            garg_to_lty(previous).spelling(records),
                            garg_to_lty(arg).spelling(records)
                        ));
                    }
                }
                Ok(())
            } else {
                match named_type(records, text) {
                    Some(expected) if expected == got => Ok(()),
                    Some(expected) => Err(format!(
                        "expected `{}`, found `{}`",
                        expected.spelling(records),
                        got.spelling(records)
                    )),
                    None => Err(format!("unknown type `{text}` in a generic parameter")),
                }
            }
        }
        TypeExpr::Optional { inner, .. } => {
            if !got.is_optional() {
                return Err(format!(
                    "expected an optional argument, found `{}`",
                    got.spelling(records)
                ));
            }
            unify_type_param(records, type_params, inner, got.to_bare(), subst)
        }
        TypeExpr::Apply { head, args, .. } => {
            unify_apply(records, type_params, head, args, got, subst)
        }
        _ => Err("this parameter type is not supported for generic inference".to_string()),
    }
}

/// Unify a built-in generic parameter application (`List`/`Map`/`Option`/`Result`)
/// against an argument, recursing into the argument's element/key/value/payload
/// types.
fn unify_apply(
    records: &TypeRegistry,
    type_params: &[(String, Option<TypeConstraint>)],
    head: &str,
    args: &[TypeExpr],
    got: LTy,
    subst: &mut [Option<GArg>],
) -> Result<(), String> {
    let mismatch = |what: &str| format!("expected a {what}, found `{}`", got.spelling(records));
    match head {
        "List" => {
            let [elem] = args else {
                return Err("`List` takes one type argument".to_string());
            };
            let LTy::Collection {
                idx,
                optional: false,
            } = got
            else {
                return Err(mismatch("List"));
            };
            match records.collection_spec(idx) {
                CollSpec::List { elem: got_elem } => {
                    unify_type_param(records, type_params, elem, garg_to_lty(got_elem), subst)
                }
                CollSpec::Map { .. } => Err(mismatch("List")),
            }
        }
        "Map" => {
            let [key, value] = args else {
                return Err("`Map` takes two type arguments".to_string());
            };
            let LTy::Collection {
                idx,
                optional: false,
            } = got
            else {
                return Err(mismatch("Map"));
            };
            match records.collection_spec(idx) {
                CollSpec::Map {
                    key: got_key,
                    value: got_value,
                } => {
                    unify_type_param(records, type_params, key, garg_to_lty(got_key), subst)?;
                    unify_type_param(records, type_params, value, garg_to_lty(got_value), subst)
                }
                CollSpec::List { .. } => Err(mismatch("Map")),
            }
        }
        // Every other generic head is a value-type template (the reserved
        // `Option`/`Result` or a user `struct`/`enum`): the argument must be an
        // instantiation of the same template, and each type argument unifies
        // positionally against its parameter.
        _ => {
            let template = records
                .type_template_by_name(head)
                .ok_or_else(|| format!("`{head}` is not a generic type usable in a parameter"))?;
            if args.len() != records.template_type_params(template).len() {
                return Err(format!(
                    "`{head}` takes {} type argument(s)",
                    records.template_type_params(template).len()
                ));
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
                _ => return Err(mismatch(head)),
            };
            let (got_template, got_args) = records
                .instantiation_of(inst_id)
                .ok_or_else(|| mismatch(head))?;
            if got_template != template {
                return Err(mismatch(head));
            }
            for (arg, got_arg) in args.iter().zip(&got_args) {
                unify_type_param(records, type_params, arg, garg_to_lty(*got_arg), subst)?;
            }
            Ok(())
        }
    }
}

/// Resolve a concrete named type (a scalar spelling or a declared nominal/struct/
/// enum/record) to its bare lowered type without minting into any draft, for
/// exact-match generic inference.
fn named_type(records: &TypeRegistry, text: &str) -> Option<LTy> {
    if let Some(scalar) = ScalarType::from_spelling(text) {
        Some(LTy::bare_scalar(scalar))
    } else if let Some((id, _)) = records.nominal_by_name(text) {
        Some(LTy::Nominal {
            id,
            optional: false,
        })
    } else if let Some(info) = records.struct_by_name(text) {
        Some(LTy::Struct {
            ty: info.type_id,
            optional: false,
        })
    } else if let Some(info) = records.enum_by_name(text) {
        Some(LTy::Enum {
            ty: info.enum_id,
            optional: false,
        })
    } else {
        records.by_name(text).map(|record| LTy::Record {
            ty: record.type_id,
            optional: false,
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

fn unsupported(file: &str, span: SourceSpan, subject: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        span,
        format!("{subject} is not yet supported on the beta line"),
    )
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
