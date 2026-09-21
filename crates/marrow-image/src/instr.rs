//! The opcode set.
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
// Finite collection values. Element/key/value shapes come
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

    /// The instruction index a transfer target names within its function.
    fn index_of(jump: &Self::Jump) -> usize;
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

    fn index_of(jump: &u32) -> usize {
        *jump as usize
    }
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

    fn index_of(jump: &usize) -> usize {
        *jump
    }
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
    /// Construct enum `enum_idx`'s variant `variant` from its dense payload popped
    /// in reverse (p0 pushed first). Operands: `u16 enum_idx ‖ u16 variant`.
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

/// What an instruction does to durable state: the coarse partition of
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

/// One instruction's frozen wire facts: its opcode byte, its immediate-operand width,
/// and the durable authority atom it stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpSpec {
    opcode: u8,
    operand_len: usize,
    class: Option<OperationClass>,
}

const fn pure(opcode: u8, operand_len: usize) -> OpSpec {
    OpSpec {
        opcode,
        operand_len,
        class: None,
    }
}

const fn durable(opcode: u8, operand_len: usize, class: OperationClass) -> OpSpec {
    OpSpec {
        opcode,
        operand_len,
        class: Some(class),
    }
}

impl<R: Operands> Instruction<R> {
    /// The one per-opcode table: the opcode byte, the immediate-operand width, and the
    /// durable authority atom the instruction stages — `None` for an instruction naming
    /// no `^` place, the transaction markers included.
    ///
    /// The durable arms are the closed projection of the durable operation algebra onto
    /// authority atoms: `create`, `replace` and the field set are writes; the erases are
    /// erases; presence is a probe; field, entry and group reads are reads; the bounded
    /// traversal and every managed-index access are ordered index reads — a unique-index
    /// presence probe reads the same index cell family as the lookup and reveals strictly
    /// less, so it demands the same authority rather than a novel atom.
    ///
    /// Operand widths: a table reference, local slot, or key-column count is a big-endian
    /// `u16`; a transfer target a `u32` byte offset; a keyed sparse access a `u16` site, a
    /// `u16` slot count and one `u16` per slot; a bounded traversal a `u16` site, a `u32`
    /// bound, a one-byte `from` flag and a `u16` COLLTYPES index; `VacantLoad` one full
    /// optional `ImageType`; `RangeGuard` two `i64` bounds.
    fn spec(&self) -> OpSpec {
        use OperationClass::{Erase, IndexRead, Presence, Read, Write};
        match self {
            Self::ConstLoad(_) => pure(OP_CONST_LOAD, 2),
            Self::LocalGet(_) => pure(OP_LOCAL_GET, 2),
            Self::LocalSet(_) => pure(OP_LOCAL_SET, 2),
            Self::Pop => pure(OP_POP, 0),
            Self::Return => pure(OP_RETURN, 0),
            Self::Call(_) => pure(OP_CALL, 2),
            Self::Jump(_) => pure(OP_JUMP, 4),
            Self::JumpIfFalse(_) => pure(OP_JUMP_IF_FALSE, 4),
            Self::BranchPresent(_) => pure(OP_BRANCH_PRESENT, 4),
            Self::Unreachable(_) => pure(OP_UNREACHABLE, 2),
            Self::Todo(_) => pure(OP_TODO, 2),
            Self::Assert => pure(OP_ASSERT, 0),
            Self::IntAdd => pure(OP_INT_ADD, 0),
            Self::IntSub => pure(OP_INT_SUB, 0),
            Self::IntMul => pure(OP_INT_MUL, 0),
            Self::IntRem => pure(OP_INT_REM, 0),
            Self::IntDiv => pure(OP_INT_DIV, 0),
            Self::IntNeg => pure(OP_INT_NEG, 0),
            Self::BoolNot => pure(OP_BOOL_NOT, 0),
            Self::IntLt => pure(OP_INT_LT, 0),
            Self::IntLe => pure(OP_INT_LE, 0),
            Self::IntGt => pure(OP_INT_GT, 0),
            Self::IntGe => pure(OP_INT_GE, 0),
            Self::EqInt => pure(OP_EQ_INT, 0),
            Self::EqBool => pure(OP_EQ_BOOL, 0),
            Self::EqText => pure(OP_EQ_TEXT, 0),
            Self::TextConcat => pure(OP_TEXT_CONCAT, 0),
            Self::TextLt => pure(OP_TEXT_LT, 0),
            Self::TextLe => pure(OP_TEXT_LE, 0),
            Self::TextGt => pure(OP_TEXT_GT, 0),
            Self::TextGe => pure(OP_TEXT_GE, 0),
            Self::EqBytes => pure(OP_EQ_BYTES, 0),
            Self::BytesLt => pure(OP_BYTES_LT, 0),
            Self::BytesLe => pure(OP_BYTES_LE, 0),
            Self::BytesGt => pure(OP_BYTES_GT, 0),
            Self::BytesGe => pure(OP_BYTES_GE, 0),
            Self::ConvString => pure(OP_CONV_STRING, 0),
            Self::ConvBytesText => pure(OP_CONV_BYTES_TEXT, 0),
            Self::TextIsEmpty => pure(OP_TEXT_IS_EMPTY, 0),
            Self::TextContains => pure(OP_TEXT_CONTAINS, 0),
            Self::TextTrim => pure(OP_TEXT_TRIM, 0),
            Self::TextSplit(_) => pure(OP_TEXT_SPLIT, 2),
            Self::TextLines(_) => pure(OP_TEXT_LINES, 2),
            Self::TextJoin => pure(OP_TEXT_JOIN, 0),
            Self::EqDate => pure(OP_EQ_DATE, 0),
            Self::DateLt => pure(OP_DATE_LT, 0),
            Self::DateLe => pure(OP_DATE_LE, 0),
            Self::DateGt => pure(OP_DATE_GT, 0),
            Self::DateGe => pure(OP_DATE_GE, 0),
            Self::EqInstant => pure(OP_EQ_INSTANT, 0),
            Self::InstantLt => pure(OP_INSTANT_LT, 0),
            Self::InstantLe => pure(OP_INSTANT_LE, 0),
            Self::InstantGt => pure(OP_INSTANT_GT, 0),
            Self::InstantGe => pure(OP_INSTANT_GE, 0),
            Self::EqDuration => pure(OP_EQ_DURATION, 0),
            Self::DurationLt => pure(OP_DURATION_LT, 0),
            Self::DurationLe => pure(OP_DURATION_LE, 0),
            Self::DurationGt => pure(OP_DURATION_GT, 0),
            Self::DurationGe => pure(OP_DURATION_GE, 0),
            Self::DateAddDays => pure(OP_DATE_ADD_DAYS, 0),
            Self::DateDaysBetween => pure(OP_DATE_DAYS_BETWEEN, 0),
            Self::DurationAdd => pure(OP_DURATION_ADD, 0),
            Self::DurationSub => pure(OP_DURATION_SUB, 0),
            Self::InstantAddDuration => pure(OP_INSTANT_ADD_DURATION, 0),
            Self::InstantSubDuration => pure(OP_INSTANT_SUB_DURATION, 0),
            Self::IntAddChecked(_) => pure(OP_INT_ADD_CHECKED, 4),
            Self::IntSubChecked(_) => pure(OP_INT_SUB_CHECKED, 4),
            Self::IntMulChecked(_) => pure(OP_INT_MUL_CHECKED, 4),
            Self::IntNegChecked(_) => pure(OP_INT_NEG_CHECKED, 4),
            Self::IntDivChecked(_) => pure(OP_INT_DIV_CHECKED, 4),
            Self::IntRemChecked(_) => pure(OP_INT_REM_CHECKED, 4),
            Self::RangeGuard { .. } => pure(OP_RANGE_GUARD, 16),
            Self::RecordNew(_) => pure(OP_RECORD_NEW, 2),
            Self::FieldGet(_) => pure(OP_FIELD_GET, 2),
            Self::FieldSet(_) => pure(OP_FIELD_SET, 2),
            Self::FieldUnset(_) => pure(OP_FIELD_UNSET, 2),
            Self::SomeWrap => pure(OP_SOME_WRAP, 0),
            Self::VacantLoad(ty) => pure(OP_VACANT_LOAD, ty.encoded_len()),
            Self::EnumConstruct { .. } => pure(OP_ENUM_CONSTRUCT, 4),
            Self::EnumTag => pure(OP_ENUM_TAG, 0),
            Self::EnumPayloadGet { .. } => pure(OP_ENUM_PAYLOAD_GET, 4),
            Self::EqEnum => pure(OP_EQ_ENUM, 0),
            Self::EqId => pure(OP_EQ_ID, 0),
            Self::MakeIdentity { .. } => pure(OP_MAKE_IDENTITY, 4),
            Self::IdentityKeyPath(_) => pure(OP_IDENTITY_KEY_PATH, 2),
            Self::DurExists(_) => durable(OP_DUR_EXISTS, 2, Presence),
            Self::DurFamilyExists(_) => durable(OP_DUR_FAMILY_EXISTS, 2, Presence),
            Self::DurReadField(_) => durable(OP_DUR_READ_FIELD, 2, Read),
            Self::DurReadFieldPresent { key_slots, .. } => {
                durable(OP_DUR_READ_FIELD_PRESENT, 4 + 2 * key_slots.len(), Read)
            }
            Self::DurReadEntry(_) => durable(OP_DUR_READ_ENTRY, 2, Read),
            Self::DurSetField { key_slots, .. } => {
                durable(OP_DUR_SET_FIELD, 4 + 2 * key_slots.len(), Write)
            }
            Self::DurCreateEntry(_) => durable(OP_DUR_CREATE_ENTRY, 2, Write),
            Self::DurReplaceEntry(_) => durable(OP_DUR_REPLACE_ENTRY, 2, Write),
            Self::DurEraseField(_) => durable(OP_DUR_ERASE_FIELD, 2, Erase),
            Self::DurEraseEntry(_) => durable(OP_DUR_ERASE_ENTRY, 2, Erase),
            Self::DurReadGroup(_) => durable(OP_DUR_READ_GROUP, 2, Read),
            Self::DurReadGroupPresent { key_slots, .. } => {
                durable(OP_DUR_READ_GROUP_PRESENT, 4 + 2 * key_slots.len(), Read)
            }
            Self::DurReplaceGroup { key_slots, .. } => {
                durable(OP_DUR_REPLACE_GROUP, 4 + 2 * key_slots.len(), Write)
            }
            Self::DurEraseGroup(_) => durable(OP_DUR_ERASE_GROUP, 2, Erase),
            Self::DurIterateBounded { .. } => durable(OP_DUR_ITERATE_BOUNDED, 9, IndexRead),
            Self::TxnBegin => pure(OP_TXN_BEGIN, 0),
            Self::TxnCommit => pure(OP_TXN_COMMIT, 0),
            Self::DurIndexScan { .. } => durable(OP_DUR_INDEX_SCAN, 9, IndexRead),
            Self::DurIndexLookup(_) => durable(OP_DUR_INDEX_LOOKUP, 2, IndexRead),
            Self::DurIndexExists(_) => durable(OP_DUR_INDEX_EXISTS, 2, IndexRead),
            Self::ListNew(_) => pure(OP_LIST_NEW, 2),
            Self::ListAppend => pure(OP_LIST_APPEND, 0),
            Self::ListLen => pure(OP_LIST_LEN, 0),
            Self::ListGet => pure(OP_LIST_GET, 0),
            Self::ListIndex => pure(OP_LIST_INDEX, 0),
            Self::MapNew(_) => pure(OP_MAP_NEW, 2),
            Self::MapInsert => pure(OP_MAP_INSERT, 0),
            Self::MapRemove => pure(OP_MAP_REMOVE, 0),
            Self::MapGet => pure(OP_MAP_GET, 0),
            Self::MapLen => pure(OP_MAP_LEN, 0),
            Self::MapKeyAt => pure(OP_MAP_KEY_AT, 0),
            Self::MapValueAt => pure(OP_MAP_VALUE_AT, 0),
        }
    }

    /// The opcode byte for this instruction.
    pub fn opcode(&self) -> u8 {
        self.spec().opcode
    }

    /// This instruction's exact encoded width (opcode plus operands) in the v0 code
    /// tape. Lowering reads it before retaining an instruction, so the per-function byte
    /// limit is enforced at the source construct that would cross it.
    pub fn encoded_len(&self) -> usize {
        1 + self.spec().operand_len
    }

    /// The durable authority atom this instruction stages, or `None` when it names no
    /// `^` place.
    pub fn operation_class(&self) -> Option<OperationClass> {
        self.spec().class
    }

    /// This instruction's place in the coarse durable partition.
    pub fn op_class(&self) -> OpClass {
        match self.operation_class() {
            Some(class) if class.mutates() => OpClass::DurableMutation,
            Some(_) => OpClass::DurableRead,
            None => OpClass::Pure,
        }
    }

    /// The operation site this instruction names, if it names one: the one closed set of
    /// site-bearing opcodes, read by the checked function append, the encoder, and the
    /// verifier's site resolution alike.
    pub fn site(&self) -> Option<&R::Site> {
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

    /// The transfer target this instruction carries, if it carries one: the one closed
    /// set of jump-bearing opcodes, read by the flow successors, the encoder's offset
    /// resolution, the measure core's target check, and the verifier's offset rewrite.
    pub fn jump_target(&self) -> Option<&R::Jump> {
        match self {
            Self::Jump(target)
            | Self::JumpIfFalse(target)
            | Self::BranchPresent(target)
            | Self::IntAddChecked(target)
            | Self::IntSubChecked(target)
            | Self::IntMulChecked(target)
            | Self::IntNegChecked(target)
            | Self::IntDivChecked(target)
            | Self::IntRemChecked(target) => Some(target),
            _ => None,
        }
    }

    /// The transfer target, mutably, for the decoder's byte-offset-to-index rewrite.
    pub fn jump_target_mut(&mut self) -> Option<&mut R::Jump> {
        match self {
            Self::Jump(target)
            | Self::JumpIfFalse(target)
            | Self::BranchPresent(target)
            | Self::IntAddChecked(target)
            | Self::IntSubChecked(target)
            | Self::IntMulChecked(target)
            | Self::IntNegChecked(target)
            | Self::IntDivChecked(target)
            | Self::IntRemChecked(target) => Some(target),
            _ => None,
        }
    }

    /// The control-flow successors of this instruction at tape index `index`: the jump
    /// target first, then the fallthrough. A terminator has none, a plain jump only its
    /// target, and a conditional branch or a checked-arithmetic trap edge both.
    pub fn successors(&self, index: usize) -> impl Iterator<Item = usize> {
        let falls_through = !matches!(
            self,
            Self::Return | Self::Unreachable(_) | Self::Todo(_) | Self::Jump(_)
        );
        self.jump_target()
            .map(R::index_of)
            .into_iter()
            .chain(falls_through.then_some(index + 1))
    }
}
