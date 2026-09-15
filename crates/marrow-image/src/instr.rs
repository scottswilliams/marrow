//! The opcode set (design §D).
//!
//! The byte encoding is the frozen container contract: opcode `u8` followed by
//! big-endian immediate operands. `marrow-image` owns the *encoder* side; the
//! verifier owns the only decoder. Jump operands are `u32` **byte offsets** in the
//! container. In this draft form a jump instead carries the *instruction index* of
//! its target, and the encoder resolves indices to byte offsets once the code
//! layout is known — so the compiler never computes byte offsets by hand.

use crate::demand::OperationClass;
use crate::draft::{CollTypeId, ConstId, EnumId, RootId, TypeId};
use crate::site_plan::PlannedSiteRef;
use crate::ty::ImageType;

// Opcode bytes. These are the frozen wire discriminants; any byte not listed here
// rejects at verify.
pub const OP_CONST_LOAD: u8 = 0x01;
pub const OP_LOCAL_GET: u8 = 0x02;
pub const OP_LOCAL_SET: u8 = 0x03;
pub const OP_POP: u8 = 0x04;
pub const OP_RETURN: u8 = 0x05;
pub const OP_CALL: u8 = 0x06;
pub const OP_JUMP: u8 = 0x07;
pub const OP_JUMP_IF_FALSE: u8 = 0x08;
pub const OP_BRANCH_PRESENT: u8 = 0x09;
pub const OP_UNREACHABLE: u8 = 0x0A;
pub const OP_ASSERT: u8 = 0x0B;
pub const OP_TODO: u8 = 0x0C;
pub const OP_INT_ADD: u8 = 0x10;
pub const OP_INT_SUB: u8 = 0x11;
pub const OP_INT_MUL: u8 = 0x12;
pub const OP_INT_REM: u8 = 0x13;
pub const OP_INT_DIV: u8 = 0x1E;
pub const OP_INT_NEG: u8 = 0x14;
pub const OP_BOOL_NOT: u8 = 0x15;
pub const OP_INT_LT: u8 = 0x16;
pub const OP_INT_LE: u8 = 0x17;
pub const OP_INT_GT: u8 = 0x18;
pub const OP_INT_GE: u8 = 0x19;
pub const OP_EQ_INT: u8 = 0x1A;
pub const OP_EQ_BOOL: u8 = 0x1B;
pub const OP_EQ_TEXT: u8 = 0x1C;
pub const OP_TEXT_CONCAT: u8 = 0x1D;
pub const OP_TEXT_LT: u8 = 0x40;
pub const OP_TEXT_LE: u8 = 0x41;
pub const OP_TEXT_GT: u8 = 0x42;
pub const OP_TEXT_GE: u8 = 0x43;
pub const OP_EQ_BYTES: u8 = 0x44;
pub const OP_BYTES_LT: u8 = 0x45;
pub const OP_BYTES_LE: u8 = 0x46;
pub const OP_BYTES_GT: u8 = 0x47;
pub const OP_BYTES_GE: u8 = 0x48;
pub const OP_CONV_STRING: u8 = 0x50;
pub const OP_CONV_BYTES_TEXT: u8 = 0x52;
pub const OP_TEXT_IS_EMPTY: u8 = 0x60;
pub const OP_TEXT_CONTAINS: u8 = 0x61;
pub const OP_TEXT_TRIM: u8 = 0x62;
// Checked arithmetic: on the fault the op does not raise `run.*`; it transfers to
// the fault-handler tape index in its `u32` operand (an out-of-range handler). A
// zero divisor is handled by a compiler-emitted branch before the checked op, so
// every checked op carries exactly one target.
pub const OP_INT_ADD_CHECKED: u8 = 0x70;
pub const OP_INT_SUB_CHECKED: u8 = 0x71;
pub const OP_INT_MUL_CHECKED: u8 = 0x72;
pub const OP_INT_NEG_CHECKED: u8 = 0x73;
pub const OP_INT_DIV_CHECKED: u8 = 0x74;
pub const OP_INT_REM_CHECKED: u8 = 0x75;
// Nominal-interval guard: peek the int on top of the stack; fault `run.range`
// when it lies outside the inclusive `[lo, hi]` immediate. No stack effect.
pub const OP_RANGE_GUARD: u8 = 0x76;
pub const OP_RECORD_NEW: u8 = 0x20;
pub const OP_FIELD_GET: u8 = 0x21;
pub const OP_SOME_WRAP: u8 = 0x22;
pub const OP_VACANT_LOAD: u8 = 0x23;
pub const OP_ENUM_CONSTRUCT: u8 = 0x24;
pub const OP_ENUM_TAG: u8 = 0x25;
pub const OP_ENUM_PAYLOAD_GET: u8 = 0x26;
pub const OP_EQ_ENUM: u8 = 0x27;
pub const OP_FIELD_SET: u8 = 0x28;
pub const OP_FIELD_UNSET: u8 = 0x29;
// Entry-identity value ops. An identity is a store root plus a key tuple; it is a
// runtime/lookup value only (not a durable cell value on this line).
pub const OP_EQ_ID: u8 = 0x2A;
// Whole-group durable ops: read/replace/erase the materialized value of one unkeyed
// `group` node, addressed by its containing entry's key-path. The group-scoped
// payload-only law scopes replace/erase to the group's own field set.
pub const OP_DUR_READ_GROUP: u8 = 0x2B;
pub const OP_DUR_REPLACE_GROUP: u8 = 0xBB;
pub const OP_DUR_ERASE_GROUP: u8 = 0x2D;
pub const OP_DUR_EXISTS: u8 = 0x30;
pub const OP_DUR_READ_FIELD: u8 = 0x31;
pub const OP_DUR_READ_FIELD_PRESENT: u8 = 0xBC;
pub const OP_DUR_READ_ENTRY: u8 = 0x32;
pub const OP_DUR_CREATE_ENTRY: u8 = 0x35;
pub const OP_DUR_REPLACE_ENTRY: u8 = 0x36;
pub const OP_DUR_ERASE_FIELD: u8 = 0x37;
pub const OP_DUR_ERASE_ENTRY: u8 = 0x38;
pub const OP_DUR_FAMILY_EXISTS: u8 = 0x39;
// The present-entry field set and group read: both address a stored node through the
// containing entry's pre-evaluated key slots and assert that entry is present.
pub const OP_DUR_SET_FIELD: u8 = 0xB9;
pub const OP_DUR_READ_GROUP_PRESENT: u8 = 0xBA;
pub const OP_DUR_ITERATE_BOUNDED: u8 = 0x3B;
pub const OP_TXN_BEGIN: u8 = 0x3C;
pub const OP_TXN_COMMIT: u8 = 0x3D;
// Construct an entry identity from `cols` bare key scalars on the stack (column order,
// last column on top); spread an identity back into its `cols` key scalars for a keyed
// entry read. Both name the store root by its ROOTS-table index.
pub const OP_MAKE_IDENTITY: u8 = 0x3E;
pub const OP_IDENTITY_KEY_PATH: u8 = 0x3F;
// Finite collection values (design §D collections). Element/key/value shapes come
// from the COLLTYPES entry the `*New` operand names; the runtime enforces the
// length and aggregate-byte bounds as typed `run.collection_limit` faults.
pub const OP_LIST_NEW: u8 = 0x90;
pub const OP_LIST_APPEND: u8 = 0x91;
pub const OP_LIST_LEN: u8 = 0x92;
pub const OP_LIST_GET: u8 = 0x93;
pub const OP_MAP_NEW: u8 = 0x94;
pub const OP_MAP_INSERT: u8 = 0x95;
pub const OP_MAP_GET: u8 = 0x96;
pub const OP_MAP_LEN: u8 = 0x97;
pub const OP_MAP_KEY_AT: u8 = 0x98;
pub const OP_MAP_VALUE_AT: u8 = 0x99;
// Collection-returning text floor. `split`/`lines` produce a `List[string]` of the
// COLLTYPES index their operand names; `join` consumes one and produces a string.
// Split/lines results honor the same `run.collection_limit` length/aggregate bounds
// as `append`; `join` honors the `run.text_limit` concatenation ceiling.
pub const OP_TEXT_SPLIT: u8 = 0x9A;
pub const OP_TEXT_LINES: u8 = 0x9B;
pub const OP_TEXT_JOIN: u8 = 0x9C;
// The source-level local list bracket read `xs[i]`: a 1-based keyed lookup yielding
// the optional element. No out-of-bounds fault class exists; an index outside
// `1..=length` yields absent.
pub const OP_LIST_INDEX: u8 = 0x9D;
// The source-level local map key removal `unset m[k]`: remove the key if present,
// idempotent no-op if absent. No fault class; keys stay in `CollectionKeyOrder`.
pub const OP_MAP_REMOVE: u8 = 0x9E;
// Temporal comparison and equality. Operands are two bare temporals of the named
// type; the result is a bool. The order agrees with the kernel key-codec byte order
// (pinned in `marrow-vm`'s `temporal_order_agreement` test).
pub const OP_EQ_DATE: u8 = 0xA0;
pub const OP_DATE_LT: u8 = 0xA1;
pub const OP_DATE_LE: u8 = 0xA2;
pub const OP_DATE_GT: u8 = 0xA3;
pub const OP_DATE_GE: u8 = 0xA4;
pub const OP_EQ_INSTANT: u8 = 0xA5;
pub const OP_INSTANT_LT: u8 = 0xA6;
pub const OP_INSTANT_LE: u8 = 0xA7;
pub const OP_INSTANT_GT: u8 = 0xA8;
pub const OP_INSTANT_GE: u8 = 0xA9;
pub const OP_EQ_DURATION: u8 = 0xAA;
pub const OP_DURATION_LT: u8 = 0xAB;
pub const OP_DURATION_LE: u8 = 0xAC;
pub const OP_DURATION_GT: u8 = 0xAD;
pub const OP_DURATION_GE: u8 = 0xAE;
// The closed temporal arithmetic floor. Each faults `run.temporal_overflow` when its
// result would leave the supported day/nanosecond domain; there is no general
// temporal arithmetic (no `date +/- int` operator, no `duration * int`, no calendar
// months/years). `marrow-temporal` owns the checked operations.
pub const OP_DATE_ADD_DAYS: u8 = 0xB0;
pub const OP_DATE_DAYS_BETWEEN: u8 = 0xB1;
pub const OP_DURATION_ADD: u8 = 0xB2;
pub const OP_DURATION_SUB: u8 = 0xB3;
pub const OP_INSTANT_ADD_DURATION: u8 = 0xB4;
pub const OP_INSTANT_SUB_DURATION: u8 = 0xB5;
// Managed-index reads. A nonunique index is scanned progressively (a `DurIterateBounded`
// mirror over the index family, holding a leading-component prefix); a unique index is an
// exact complete-projection lookup yielding the optional source identity.
pub const OP_DUR_INDEX_SCAN: u8 = 0xB6;
pub const OP_DUR_INDEX_LOOKUP: u8 = 0xB7;
pub const OP_DUR_INDEX_EXISTS: u8 = 0xB8;

/// The compiler's draft instruction.
pub type Instr = Instruction<Draft>;

/// A verified instruction with bounds-checked operands, as the VM receives it.
pub type SealedInstr = Instruction<Sealed>;

/// How one instruction spells its table references.
///
/// The opcode set, the operand widths, the durable partition and every variant's
/// meaning are one owner; only the spelling of a reference differs between the two
/// sides of the trust boundary. The draft form carries the compiler's typed
/// pre-encode ids and its unforgeable site capability; the sealed form carries the
/// ordinals and tape indices the verifier bounds-checked out of received bytes.
pub trait Operands {
    /// A constant-pool reference.
    type Const: std::fmt::Debug + Clone + PartialEq + Eq;
    /// A transfer target within the function's own instruction list.
    type Jump: std::fmt::Debug + Clone + PartialEq + Eq;
    /// A TYPES-table reference.
    type Type: std::fmt::Debug + Clone + PartialEq + Eq;
    /// An ENUMS-table reference.
    type Enum: std::fmt::Debug + Clone + PartialEq + Eq;
    /// A COLLTYPES-table reference.
    type Coll: std::fmt::Debug + Clone + PartialEq + Eq;
    /// A ROOTS-table reference.
    type Root: std::fmt::Debug + Clone + PartialEq + Eq;
    /// A durable operation site.
    type Site: std::fmt::Debug + Clone + PartialEq + Eq;
}

/// The compiler's spelling: typed draft ids the encoder resolves, and site
/// capabilities only the draft can mint. Jump targets are instruction indices into
/// the function's own list; the encoder rewrites them to container byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Draft;

impl Operands for Draft {
    type Const = ConstId;
    type Jump = u32;
    type Type = TypeId;
    type Enum = EnumId;
    type Coll = CollTypeId;
    type Root = RootId;
    type Site = PlannedSiteRef;
}

/// The verifier's spelling: wire ordinals it bounds-checked against the image's own
/// tables, and jump targets resolved back from container byte offsets to indices
/// into the owning function's tape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sealed;

impl Operands for Sealed {
    type Const = u16;
    type Jump = usize;
    type Type = u16;
    type Enum = u16;
    type Coll = u16;
    type Root = u16;
    type Site = u16;
}

/// One instruction of the v0 tape, in the operand spelling `R` states.
///
/// The set grows one slice at a time; an opcode whose vertical has not landed is a
/// verify rejection rather than a silent pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction<R: Operands> {
    ConstLoad(R::Const),
    LocalGet(u16),
    LocalSet(u16),
    Pop,
    Return,
    Call(u16),
    Jump(R::Jump),
    JumpIfFalse(R::Jump),
    BranchPresent(R::Jump),
    /// Fault with `run.unreachable`, carrying the static text at const index `_0`.
    /// The sole application-invariant fault; it never falls through.
    Unreachable(R::Const),
    /// Fault with `run.todo`, carrying the static text at const index `_0`. A deferred
    /// path the author has not implemented; like `Unreachable` it never falls through.
    Todo(R::Const),
    /// Pop a bool; on false fault with `run.assert` at this instruction's span, else
    /// fall through. Legal only in a test-entry function (the verifier enforces it).
    Assert,
    IntAdd,
    IntSub,
    IntMul,
    IntRem,
    IntDiv,
    IntNeg,
    BoolNot,
    IntLt,
    IntLe,
    IntGt,
    IntGe,
    EqInt,
    EqBool,
    EqText,
    TextConcat,
    TextLt,
    TextLe,
    TextGt,
    TextGe,
    EqBytes,
    BytesLt,
    BytesLe,
    BytesGt,
    BytesGe,
    /// Render the top-of-stack value to its canonical text (`value → string`). The
    /// operand is an interpolable value — a scalar, an enum, or an entry identity — as
    /// proved by the verifier; the runtime renders it through the one canonical owner.
    /// Shared by `$"{}"` interpolation and `string(value)`.
    ConvString,
    /// `string → bytes`: the UTF-8 bytes of the text.
    ConvBytesText,
    /// The closed pure text floor. `string → bool`, `string, string → bool`, and
    /// `string → string`.
    TextIsEmpty,
    TextContains,
    TextTrim,
    /// The collection-returning text floor. `split`/`lines` split a string into a
    /// `List[string]` of the COLLTYPES index `_0` (which names a `List[string]`),
    /// honoring the `run.collection_limit` bounds; `join` concatenates a
    /// `List[string]` with a separator into a string, honoring the `run.text_limit`
    /// ceiling.
    TextSplit(R::Coll),
    TextLines(R::Coll),
    TextJoin,
    /// Temporal equality and order over two bare temporals of the same type,
    /// producing a bool. The order agrees with the kernel key-codec byte order.
    EqDate,
    DateLt,
    DateLe,
    DateGt,
    DateGe,
    EqInstant,
    InstantLt,
    InstantLe,
    InstantGt,
    InstantGe,
    EqDuration,
    DurationLt,
    DurationLe,
    DurationGt,
    DurationGe,
    /// `addDays(date, int) → date`, faulting `run.temporal_overflow` when the
    /// result leaves the supported calendar range (years 0001-9999).
    DateAddDays,
    /// `daysBetween(date, date) → int`: the signed day span from the first to
    /// the second. Total (both operands are supported dates).
    DateDaysBetween,
    /// `duration +/- duration → duration`, faulting `run.temporal_overflow` on
    /// `i128` overflow.
    DurationAdd,
    DurationSub,
    /// `instant +/- duration → instant`, faulting `run.temporal_overflow` when the
    /// result leaves the supported instant range.
    InstantAddDuration,
    InstantSubDuration,
    /// Checked arithmetic: `_0` is the fault-handler target (instruction index in
    /// the draft, rewritten to a byte offset by the encoder). On overflow the op
    /// jumps to the target instead of faulting; otherwise it pushes the result.
    IntAddChecked(R::Jump),
    IntSubChecked(R::Jump),
    IntMulChecked(R::Jump),
    IntNegChecked(R::Jump),
    IntDivChecked(R::Jump),
    IntRemChecked(R::Jump),
    /// Peek the int on top of the stack; fault `run.range` when it lies outside
    /// the inclusive `[lo, hi]` immediate, else fall through with no stack
    /// effect. The compiler emits one after every operation that produces a
    /// nominal interval-constrained value; a well-formed guard has `lo <= hi`.
    RangeGuard {
        lo: i64,
        hi: i64,
    },
    RecordNew(R::Type),
    FieldGet(u16),
    /// Pop a value and a bare record, store the value into the record's field
    /// `_0` slot (present), and push the updated record. Local product mutation:
    /// `r.f = v` sets the slot present with the bare field value, for a required
    /// or a sparse field alike. The one owner of the record representation is the
    /// runtime `Value::Record` slot vector, which this rewrites functionally.
    FieldSet(u16),
    /// Pop a bare record, clear its field `_0` slot to vacant, and push the
    /// updated record. `unset r.f` clears a sparse field; the verifier proves the
    /// field is sparse (a required field is never unset).
    FieldUnset(u16),
    SomeWrap,
    VacantLoad(ImageType),
    /// Construct enum `enum_idx`'s variant `variant` from its dense scalar payload
    /// popped in reverse (p0 pushed first). Operands: `u16 enum_idx ‖ u16 variant`.
    EnumConstruct {
        enum_idx: R::Enum,
        variant: u16,
    },
    /// Pop an enum value and push its variant index as a bare int. The one match
    /// primitive: a branch chain over the tag dispatches the arms.
    EnumTag,
    /// Read payload leaf `field` of `variant` from the enum value on the stack,
    /// pushing its bare scalar. Operands: `u16 variant ‖ u16 field`. The variant
    /// operand types the leaf; the VM faults (defense in depth) if the runtime
    /// value carries a different variant, so a hostile image cannot confuse types.
    EnumPayloadGet {
        variant: u16,
        field: u16,
    },
    /// `E, E → bool`: exact equality of two values of the same enum (variant and
    /// payload).
    EqEnum,
    /// `Id, Id → bool`: equality of two entry identities of the same store root — the
    /// same key tuple. The checker admits the comparison only for identities of one
    /// root, so the operands always share a root and equality reduces to key-tuple
    /// equality.
    EqId,
    /// `[k0, …, k(cols-1)] → Id`: construct the entry identity of store root `root`
    /// from `cols` bare key scalars popped in reverse (k0 pushed first, in key-column
    /// declaration order). The `Id(^root, keys…)` constructor. Operands:
    /// `u16 root ‖ u16 cols`.
    MakeIdentity {
        root: R::Root,
        cols: u16,
    },
    /// `Id → [k0, …, k(cols-1)]`: spread an entry identity into its `cols` key scalars,
    /// pushed root-first in key-column order so the key-path sits exactly as an inline
    /// `^root[k…]` access would leave it. The thin adapter that lets `^root[id]`
    /// dereference reuse the ordinary keyed entry read. `cols` is the root's key-column
    /// count; the VM faults `run.corruption` (defense in depth) if the identity's tuple
    /// length disagrees.
    IdentityKeyPath(u16),
    DurExists(R::Site),
    /// `[ancestor-keys] → bool`: whether the family the whole-entry `site` names (the
    /// root's entry family, or a keyed branch family beneath the parent entry the
    /// ancestor key-path locates) has at least one payload-bearing immediate child. Pops
    /// the ancestor key-path (a root site pops none; a single-level branch site pops
    /// `[root_key]`) and pushes the family-populated bool. Unlike [`DurExists`] it names
    /// no immediate child key: it is the family-populated probe, not a keyed presence.
    DurFamilyExists(R::Site),
    DurReadField(R::Site),
    /// `→ T`: read a required field through captured place slots. The verifier
    /// proves its containing entry present; a missing value faults as corruption.
    DurReadFieldPresent {
        site: R::Site,
        key_slots: Vec<u16>,
    },
    DurReadEntry(R::Site),
    /// `T →`: set the field `site` (required or sparse) to a definite value, reading
    /// the containing entry's key-path from local slots `key_slots` (root-first, one
    /// slot per key column of every node from the root down to the field's containing
    /// entry) and asserting that entry is present. The one field-set form: emitted only
    /// for a set through a `place` binding a presence fact dominates, so the key-path is
    /// the place's pre-evaluated slots rather than a stack operand. The compiler proves
    /// the entry present; the runtime faults `run.corruption` if the marker is absent
    /// (defense in depth over the trust boundary). A field is cleared only by
    /// [`Instr::DurEraseField`].
    DurSetField {
        site: R::Site,
        key_slots: Vec<u16>,
    },
    DurCreateEntry(R::Site),
    DurReplaceEntry(R::Site),
    DurEraseField(R::Site),
    DurEraseEntry(R::Site),
    /// `K → Rec?`: read the whole materialized value of the unkeyed `group` the
    /// `GroupEntry` site `_0` names, as one record, or absent when the containing entry
    /// is absent.
    DurReadGroup(R::Site),
    /// `→ Rec`: read the whole materialized value of the unkeyed `group` the
    /// `GroupEntry` site names, as a bare record, reading the containing entry's
    /// key-path from local slots `key_slots` (root-first) and asserting that entry is
    /// present. The read half of a group-leaf rewrite through a proven place; the
    /// runtime faults `run.corruption` if the marker is absent.
    DurReadGroupPresent {
        site: R::Site,
        key_slots: Vec<u16>,
    },
    /// `Rec →`: replace the group at `site`, reading the containing entry's key path
    /// from `key_slots` (root-first). The presence lattice proves the entry present;
    /// only this group's fields change, preserving sibling groups and branches.
    DurReplaceGroup {
        site: R::Site,
        key_slots: Vec<u16>,
    },
    /// `K →`: erase the group the `GroupEntry` site `_0` names — clears only that
    /// group's leaves (no-op on an absent entry).
    DurEraseGroup(R::Site),
    /// The bounded nested traversal `for … at most N … on more`. Freeze the first
    /// `limit` immediate keys of the layer the whole-entry `site` belongs to — the
    /// root's entry family (a root site) or a keyed branch family under a fixed parent
    /// entry (a branch site) — then push the frozen key list (bounded by `limit`) and
    /// whether a further key existed (the `on more` bit).
    ///
    /// Stack effect `[ancestor-keys, from?] → List[K], Bool`: pop the layer's ancestor
    /// key-path (a root site pops none; a single-level branch site pops `[root_key]`),
    /// then the inclusive `from` key of the traversed key type `K` when `from` is set,
    /// and push `List[K]` then `Bool`. `limit` is the positive compile-time `N`, and
    /// `list_ty` is the COLLTYPES index of the frozen `List[K]` the frozen keys
    /// materialize into (the same list value every list operation produces, so it obeys
    /// the one collection aggregate-byte ceiling). The keys are frozen before any loop
    /// body runs, so a body's writes cannot change the set; no cursor, page, or
    /// continuation is threaded — the frozen list is the whole result.
    DurIterateBounded {
        site: R::Site,
        limit: u32,
        from: bool,
        list_ty: R::Coll,
    },
    TxnBegin,
    TxnCommit,
    /// The bounded progressive scan of a nonunique managed index — the `DurIterateBounded`
    /// mirror over an index family. Freeze the first `limit` distinct values of the index's
    /// trailing identity component that hold the leading-field prefix on the stack, then push
    /// the frozen source identities as one `List[K]` and whether a further distinct value
    /// existed (the `on more` bit).
    ///
    /// Stack effect `[prefix-keys, from?] → List[K], Bool`: pop the held prefix (the index's
    /// leading field components, in projection order) then the inclusive `from` key of the
    /// scanned component when `from` is set; push `List[K]` then `Bool`. `site` names the
    /// index scan site, `limit` the positive compile-time `N`, and `list_ty` the frozen
    /// `List[K]` COLLTYPES index. The frozen list holds the scanned component's raw key
    /// scalars; the compiler wraps each into the source `Id(^root)` at the loop binding.
    DurIndexScan {
        site: R::Site,
        limit: u32,
        from: bool,
        list_ty: R::Coll,
    },
    /// The exact complete-key lookup of a unique managed index. Pop the index's whole
    /// projection (one key per component, in projection order) and push the matching source
    /// identity as an optional `Id(^root)` — present with the source key tuple, or vacant.
    /// `site` names the index lookup site. Stack effect `[projection-keys] → Id(^root)?`.
    DurIndexLookup(R::Site),
    /// The presence half of [`DurIndexLookup`] — the unique-index arm of `exists`. Pop the
    /// index's whole projection (one key per component, projection order) and push whether a
    /// matching entry exists, without materializing its identity. `site` names the same
    /// unique-index lookup site the lookup uses. Stack effect `[projection-keys] → bool`.
    DurIndexExists(R::Site),
    /// Push an empty `List` of the COLLTYPES index `_0`.
    ListNew(R::Coll),
    /// `[list, value] → [list']`: append the bare value after the last element,
    /// faulting `run.collection_limit` when the length or aggregate-byte bound is
    /// exceeded. Collections are values, so this yields a new list.
    ListAppend,
    /// `[list] → [int]`: the element count.
    ListLen,
    /// `[list, int] → [element]`: the bare element at the 0-based index. The
    /// verifier proves the element type. Emitted only by the compiler's positional
    /// `for` lowering, which keeps every index in `0..length`, so the read is total.
    ListGet,
    /// `[list, int] → [element?]`: the source-level local list bracket read `xs[i]`.
    /// The index is 1-based (position `1` is the first element); an index outside
    /// `1..=length` yields absent. Marrow has no out-of-bounds fault class, so this
    /// read never faults.
    ListIndex,
    /// Push an empty `Map` of the COLLTYPES index `_0`.
    MapNew(R::Coll),
    /// `[map, key, value] → [map']`: insert or replace the value at `key`, keeping
    /// keys in `CollectionKeyOrder`. Faults `run.collection_limit` on bound excess.
    MapInsert,
    /// `[map, key] → [map']`: remove the entry at `key` if present, or leave the map
    /// unchanged if absent (idempotent). Keys stay in `CollectionKeyOrder`. No fault
    /// class: removal never exceeds a collection bound and never faults.
    MapRemove,
    /// `[map, key] → [value?]`: the value at `key`, or absent.
    MapGet,
    /// `[map] → [int]`: the entry count.
    MapLen,
    /// `[map, int] → [key]`: the bare key at the 0-based position in key order.
    MapKeyAt,
    /// `[map, int] → [value]`: the bare value at the 0-based position in key order.
    MapValueAt,
}

/// What an instruction does to durable state: the coarse partition, derived from
/// [`Instruction::operation_class`].
///
/// The compiler's requires-ambient-transaction check and its direct-durable-operation
/// check both read it, so neither can classify an opcode differently from the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpClass {
    /// Stages a durable mutation over a `^` place: a field write, an entry or group
    /// create, replace, or erase.
    DurableMutation,
    /// Reads a `^` place without changing it: a value read, a presence probe, a bounded
    /// traversal, or a managed-index access.
    DurableRead,
    /// Names no `^` place. Transaction markers and `Duration*` arithmetic are here.
    Pure,
}

impl<R: Operands> Instruction<R> {
    /// The durable authority atom this instruction stages, or `None` when it names no
    /// `^` place.
    ///
    /// The sole owner of the durable opcode partition, and the closed projection of
    /// the durable operation algebra onto authority atoms: `create`, `replace` and
    /// the field set are writes; the two erases are erases; presence is a probe;
    /// field, entry and group reads are reads; the bounded traversal and every
    /// managed-index access are ordered index reads — a unique-index presence probe
    /// reads the same index cell family as the lookup and reveals strictly less, so
    /// it demands the same authority rather than a novel atom. Transaction markers
    /// open and close the region but stage no access.
    ///
    /// The match is exhaustive with no `_` fallthrough — the pure complement is
    /// listed rather than elided — so a new opcode fails to compile until it is
    /// classified, welding the partition to the instruction set.
    pub fn operation_class(&self) -> Option<OperationClass> {
        match self {
            Self::DurExists(_) | Self::DurFamilyExists(_) => Some(OperationClass::Presence),
            Self::DurReadField(_)
            | Self::DurReadFieldPresent { .. }
            | Self::DurReadEntry(_)
            | Self::DurReadGroup(_)
            | Self::DurReadGroupPresent { .. } => Some(OperationClass::Read),
            Self::DurSetField { .. }
            | Self::DurCreateEntry(_)
            | Self::DurReplaceEntry(_)
            | Self::DurReplaceGroup { .. } => Some(OperationClass::Write),
            Self::DurEraseField(_) | Self::DurEraseEntry(_) | Self::DurEraseGroup(_) => {
                Some(OperationClass::Erase)
            }
            Self::DurIterateBounded { .. }
            | Self::DurIndexScan { .. }
            | Self::DurIndexLookup(_)
            | Self::DurIndexExists(_) => Some(OperationClass::IndexRead),
            Self::TxnBegin | Self::TxnCommit => None,
            Self::ConstLoad(_)
            | Self::LocalGet(_)
            | Self::LocalSet(_)
            | Self::Pop
            | Self::Return
            | Self::Call(_)
            | Self::Jump(_)
            | Self::JumpIfFalse(_)
            | Self::BranchPresent(_)
            | Self::Unreachable(_)
            | Self::Todo(_)
            | Self::Assert
            | Self::IntAdd
            | Self::IntSub
            | Self::IntMul
            | Self::IntRem
            | Self::IntDiv
            | Self::IntNeg
            | Self::BoolNot
            | Self::IntLt
            | Self::IntLe
            | Self::IntGt
            | Self::IntGe
            | Self::EqInt
            | Self::EqBool
            | Self::EqText
            | Self::TextConcat
            | Self::TextLt
            | Self::TextLe
            | Self::TextGt
            | Self::TextGe
            | Self::EqBytes
            | Self::BytesLt
            | Self::BytesLe
            | Self::BytesGt
            | Self::BytesGe
            | Self::ConvString
            | Self::ConvBytesText
            | Self::TextIsEmpty
            | Self::TextContains
            | Self::TextTrim
            | Self::TextSplit(_)
            | Self::TextLines(_)
            | Self::TextJoin
            | Self::EqDate
            | Self::DateLt
            | Self::DateLe
            | Self::DateGt
            | Self::DateGe
            | Self::EqInstant
            | Self::InstantLt
            | Self::InstantLe
            | Self::InstantGt
            | Self::InstantGe
            | Self::EqDuration
            | Self::DurationLt
            | Self::DurationLe
            | Self::DurationGt
            | Self::DurationGe
            | Self::DateAddDays
            | Self::DateDaysBetween
            | Self::DurationAdd
            | Self::DurationSub
            | Self::InstantAddDuration
            | Self::InstantSubDuration
            | Self::IntAddChecked(_)
            | Self::IntSubChecked(_)
            | Self::IntMulChecked(_)
            | Self::IntNegChecked(_)
            | Self::IntDivChecked(_)
            | Self::IntRemChecked(_)
            | Self::RangeGuard { .. }
            | Self::RecordNew(_)
            | Self::FieldGet(_)
            | Self::FieldSet(_)
            | Self::FieldUnset(_)
            | Self::SomeWrap
            | Self::VacantLoad(_)
            | Self::EnumConstruct { .. }
            | Self::EnumTag
            | Self::EnumPayloadGet { .. }
            | Self::EqEnum
            | Self::EqId
            | Self::MakeIdentity { .. }
            | Self::IdentityKeyPath(_)
            | Self::ListNew(_)
            | Self::ListAppend
            | Self::ListLen
            | Self::ListGet
            | Self::ListIndex
            | Self::MapNew(_)
            | Self::MapInsert
            | Self::MapRemove
            | Self::MapGet
            | Self::MapLen
            | Self::MapKeyAt
            | Self::MapValueAt => None,
        }
    }

    /// This instruction's place in the coarse durable partition.
    pub fn op_class(&self) -> OpClass {
        match self.operation_class() {
            Some(class) if class.mutates() => OpClass::DurableMutation,
            Some(_) => OpClass::DurableRead,
            None => OpClass::Pure,
        }
    }
}

impl<R: Operands> Instruction<R> {
    /// The opcode byte for this instruction.
    pub fn opcode(&self) -> u8 {
        match self {
            Self::ConstLoad(_) => OP_CONST_LOAD,
            Self::LocalGet(_) => OP_LOCAL_GET,
            Self::LocalSet(_) => OP_LOCAL_SET,
            Self::Pop => OP_POP,
            Self::Return => OP_RETURN,
            Self::Call(_) => OP_CALL,
            Self::Jump(_) => OP_JUMP,
            Self::JumpIfFalse(_) => OP_JUMP_IF_FALSE,
            Self::BranchPresent(_) => OP_BRANCH_PRESENT,
            Self::Unreachable(_) => OP_UNREACHABLE,
            Self::Todo(_) => OP_TODO,
            Self::Assert => OP_ASSERT,
            Self::IntAdd => OP_INT_ADD,
            Self::IntSub => OP_INT_SUB,
            Self::IntMul => OP_INT_MUL,
            Self::IntRem => OP_INT_REM,
            Self::IntDiv => OP_INT_DIV,
            Self::IntNeg => OP_INT_NEG,
            Self::BoolNot => OP_BOOL_NOT,
            Self::IntLt => OP_INT_LT,
            Self::IntLe => OP_INT_LE,
            Self::IntGt => OP_INT_GT,
            Self::IntGe => OP_INT_GE,
            Self::EqInt => OP_EQ_INT,
            Self::EqBool => OP_EQ_BOOL,
            Self::EqText => OP_EQ_TEXT,
            Self::TextConcat => OP_TEXT_CONCAT,
            Self::TextLt => OP_TEXT_LT,
            Self::TextLe => OP_TEXT_LE,
            Self::TextGt => OP_TEXT_GT,
            Self::TextGe => OP_TEXT_GE,
            Self::EqBytes => OP_EQ_BYTES,
            Self::BytesLt => OP_BYTES_LT,
            Self::BytesLe => OP_BYTES_LE,
            Self::BytesGt => OP_BYTES_GT,
            Self::BytesGe => OP_BYTES_GE,
            Self::ConvString => OP_CONV_STRING,
            Self::ConvBytesText => OP_CONV_BYTES_TEXT,
            Self::TextIsEmpty => OP_TEXT_IS_EMPTY,
            Self::TextContains => OP_TEXT_CONTAINS,
            Self::TextTrim => OP_TEXT_TRIM,
            Self::TextSplit(_) => OP_TEXT_SPLIT,
            Self::TextLines(_) => OP_TEXT_LINES,
            Self::TextJoin => OP_TEXT_JOIN,
            Self::EqDate => OP_EQ_DATE,
            Self::DateLt => OP_DATE_LT,
            Self::DateLe => OP_DATE_LE,
            Self::DateGt => OP_DATE_GT,
            Self::DateGe => OP_DATE_GE,
            Self::EqInstant => OP_EQ_INSTANT,
            Self::InstantLt => OP_INSTANT_LT,
            Self::InstantLe => OP_INSTANT_LE,
            Self::InstantGt => OP_INSTANT_GT,
            Self::InstantGe => OP_INSTANT_GE,
            Self::EqDuration => OP_EQ_DURATION,
            Self::DurationLt => OP_DURATION_LT,
            Self::DurationLe => OP_DURATION_LE,
            Self::DurationGt => OP_DURATION_GT,
            Self::DurationGe => OP_DURATION_GE,
            Self::DateAddDays => OP_DATE_ADD_DAYS,
            Self::DateDaysBetween => OP_DATE_DAYS_BETWEEN,
            Self::DurationAdd => OP_DURATION_ADD,
            Self::DurationSub => OP_DURATION_SUB,
            Self::InstantAddDuration => OP_INSTANT_ADD_DURATION,
            Self::InstantSubDuration => OP_INSTANT_SUB_DURATION,
            Self::IntAddChecked(_) => OP_INT_ADD_CHECKED,
            Self::IntSubChecked(_) => OP_INT_SUB_CHECKED,
            Self::IntMulChecked(_) => OP_INT_MUL_CHECKED,
            Self::IntNegChecked(_) => OP_INT_NEG_CHECKED,
            Self::IntDivChecked(_) => OP_INT_DIV_CHECKED,
            Self::IntRemChecked(_) => OP_INT_REM_CHECKED,
            Self::RangeGuard { .. } => OP_RANGE_GUARD,
            Self::RecordNew(_) => OP_RECORD_NEW,
            Self::FieldGet(_) => OP_FIELD_GET,
            Self::FieldSet(_) => OP_FIELD_SET,
            Self::FieldUnset(_) => OP_FIELD_UNSET,
            Self::SomeWrap => OP_SOME_WRAP,
            Self::VacantLoad(_) => OP_VACANT_LOAD,
            Self::EnumConstruct { .. } => OP_ENUM_CONSTRUCT,
            Self::EnumTag => OP_ENUM_TAG,
            Self::EnumPayloadGet { .. } => OP_ENUM_PAYLOAD_GET,
            Self::EqEnum => OP_EQ_ENUM,
            Self::EqId => OP_EQ_ID,
            Self::MakeIdentity { .. } => OP_MAKE_IDENTITY,
            Self::IdentityKeyPath(_) => OP_IDENTITY_KEY_PATH,
            Self::DurExists(_) => OP_DUR_EXISTS,
            Self::DurFamilyExists(_) => OP_DUR_FAMILY_EXISTS,
            Self::DurReadField(_) => OP_DUR_READ_FIELD,
            Self::DurReadFieldPresent { .. } => OP_DUR_READ_FIELD_PRESENT,
            Self::DurReadEntry(_) => OP_DUR_READ_ENTRY,
            Self::DurSetField { .. } => OP_DUR_SET_FIELD,
            Self::DurCreateEntry(_) => OP_DUR_CREATE_ENTRY,
            Self::DurReplaceEntry(_) => OP_DUR_REPLACE_ENTRY,
            Self::DurEraseField(_) => OP_DUR_ERASE_FIELD,
            Self::DurEraseEntry(_) => OP_DUR_ERASE_ENTRY,
            Self::DurReadGroup(_) => OP_DUR_READ_GROUP,
            Self::DurReadGroupPresent { .. } => OP_DUR_READ_GROUP_PRESENT,
            Self::DurReplaceGroup { .. } => OP_DUR_REPLACE_GROUP,
            Self::DurEraseGroup(_) => OP_DUR_ERASE_GROUP,
            Self::DurIterateBounded { .. } => OP_DUR_ITERATE_BOUNDED,
            Self::TxnBegin => OP_TXN_BEGIN,
            Self::TxnCommit => OP_TXN_COMMIT,
            Self::DurIndexScan { .. } => OP_DUR_INDEX_SCAN,
            Self::DurIndexLookup(_) => OP_DUR_INDEX_LOOKUP,
            Self::DurIndexExists(_) => OP_DUR_INDEX_EXISTS,
            Self::ListNew(_) => OP_LIST_NEW,
            Self::ListAppend => OP_LIST_APPEND,
            Self::ListLen => OP_LIST_LEN,
            Self::ListGet => OP_LIST_GET,
            Self::ListIndex => OP_LIST_INDEX,
            Self::MapNew(_) => OP_MAP_NEW,
            Self::MapInsert => OP_MAP_INSERT,
            Self::MapRemove => OP_MAP_REMOVE,
            Self::MapGet => OP_MAP_GET,
            Self::MapLen => OP_MAP_LEN,
            Self::MapKeyAt => OP_MAP_KEY_AT,
            Self::MapValueAt => OP_MAP_VALUE_AT,
        }
    }

    /// The number of immediate-operand bytes after the opcode.
    fn operand_len(&self) -> usize {
        match self {
            Self::ConstLoad(_)
            | Self::LocalGet(_)
            | Self::LocalSet(_)
            | Self::Unreachable(_)
            | Self::Todo(_)
            | Self::Call(_)
            | Self::RecordNew(_)
            | Self::FieldGet(_)
            | Self::FieldSet(_)
            | Self::FieldUnset(_)
            | Self::DurExists(_)
            | Self::DurFamilyExists(_)
            | Self::DurReadField(_)
            | Self::DurReadEntry(_)
            | Self::DurCreateEntry(_)
            | Self::DurReplaceEntry(_)
            | Self::DurEraseField(_)
            | Self::DurEraseEntry(_)
            | Self::DurReadGroup(_)
            | Self::DurEraseGroup(_)
            | Self::ListNew(_)
            | Self::MapNew(_)
            | Self::TextSplit(_)
            | Self::TextLines(_)
            // A big-endian `u16` root key-column count.
            | Self::IdentityKeyPath(_)
            // A big-endian `u16` index lookup site.
            | Self::DurIndexLookup(_)
            // A big-endian `u16` unique-index presence-probe site.
            | Self::DurIndexExists(_) => 2,
            // Two big-endian `u16` operands: the store-root index and the key-column count.
            Self::MakeIdentity { .. } => 4,
            Self::Jump(_)
            | Self::JumpIfFalse(_)
            | Self::BranchPresent(_)
            | Self::IntAddChecked(_)
            | Self::IntSubChecked(_)
            | Self::IntMulChecked(_)
            | Self::IntNegChecked(_)
            | Self::IntDivChecked(_)
            | Self::IntRemChecked(_) => 4,
            // A `VacantLoad` operand is a full optional `ImageType`: one tag byte
            // for an optional scalar, or a tag plus a big-endian `u16` index for an
            // optional enum (a defaulted sparse enum field).
            Self::VacantLoad(ty) => ty.encoded_len(),
            // Two big-endian `i64` interval bounds.
            Self::RangeGuard { .. } => 16,
            // Two big-endian `u16` operands.
            Self::EnumConstruct { .. } | Self::EnumPayloadGet { .. } => 4,
            // A big-endian `u16` site, a big-endian `u16` key-path length, then one
            // big-endian `u16` per key-path slot.
            Self::DurSetField { key_slots, .. }
            | Self::DurReadFieldPresent { key_slots, .. }
            | Self::DurReadGroupPresent { key_slots, .. }
            | Self::DurReplaceGroup { key_slots, .. } => {
                4 + 2 * key_slots.len()
            }
            // A big-endian `u16` site, a big-endian `u32` bound, a one-byte
            // `from`-present flag, and a big-endian `u16` frozen-`List[K]` COLLTYPES
            // index.
            Self::DurIterateBounded { .. } | Self::DurIndexScan { .. } => 9,
            _ => 0,
        }
    }

    /// This instruction's exact encoded width (opcode plus operands) in the v0 code
    /// tape.
    ///
    /// Lowering reads the same owner before retaining an instruction, so the
    /// per-function byte limit is enforced at the source construct that would cross
    /// it rather than rediscovered only after a complete tape has been built.
    #[doc(hidden)]
    pub fn encoded_len(&self) -> usize {
        1 + self.operand_len()
    }
}

impl Instr {
    /// The operation site this instruction names, if it names one.
    ///
    /// This is the one place that answers "which instructions carry a site", so the
    /// checked function append and the encoder read one closed set rather than each
    /// keeping its own list of site-bearing opcodes.
    pub(crate) fn site_operand(&self) -> Option<&PlannedSiteRef> {
        match self {
            Self::DurExists(site)
            | Self::DurFamilyExists(site)
            | Self::DurReadField(site)
            | Self::DurReadFieldPresent { site, .. }
            | Self::DurReadEntry(site)
            | Self::DurSetField { site, .. }
            | Self::DurCreateEntry(site)
            | Self::DurReplaceEntry(site)
            | Self::DurEraseField(site)
            | Self::DurEraseEntry(site)
            | Self::DurReadGroup(site)
            | Self::DurReadGroupPresent { site, .. }
            | Self::DurReplaceGroup { site, .. }
            | Self::DurEraseGroup(site)
            | Self::DurIterateBounded { site, .. }
            | Self::DurIndexScan { site, .. }
            | Self::DurIndexLookup(site)
            | Self::DurIndexExists(site) => Some(site),
            _ => None,
        }
    }
}
