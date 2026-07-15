//! The sealed, verified image the VM accepts (design §A/§E phase 6).
//!
//! [`VerifiedImage`] has all-private fields and a single constructor,
//! [`crate::verify`]. Sealing produces a typed instruction tape per function:
//! [`SealedInstr`] values with jumps resolved to instruction indices and operands
//! as bounds-checked typed handles, so the VM never sees raw opcode bytes and a
//! verifier/VM width or discriminant disagreement is unrepresentable.

use std::rc::Rc;

use marrow_image::{DurableContractId, ExportId, ImageId, ImageType, Scalar};

/// A resolved constant value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealedConst {
    Int(i64),
    Bool(bool),
    Text(Rc<str>),
    /// A temporal scalar: a `date` (days since the epoch), an `instant` (signed
    /// nanoseconds since the epoch), or a `duration` (signed nanoseconds).
    Date(i32),
    Instant(i128),
    Duration(i128),
}

/// A verified instruction with typed, bounds-checked operands. Jump targets are
/// instruction indices into the owning function's tape. This enum grows one slice
/// at a time; an opcode whose vertical has not landed is rejected at verify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealedInstr {
    /// Push constant `consts[idx]`.
    ConstLoad(u16),
    /// Push local slot `l`.
    LocalGet(u16),
    /// Pop into local slot `l`.
    LocalSet(u16),
    /// Discard the top of the stack.
    Pop,
    /// Return the frame's value (or nothing for a Unit return).
    Return,
    /// Unconditional jump to the instruction at this tape index.
    Jump(usize),
    /// Pop a bool; if false, jump to this tape index, else fall through.
    JumpIfFalse(usize),
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
    /// `int → string` (decimal), `bool → string`, and `string → bytes` (UTF-8): the
    /// closed scalar conversions.
    ConvStringInt,
    ConvStringBool,
    ConvBytesText,
    /// The closed pure text floor: `string → bool`, `string, string → bool`,
    /// `string → string`.
    TextIsEmpty,
    TextContains,
    TextTrim,
    /// The collection-returning text floor. `split`/`lines` produce a `List[string]`
    /// of COLLTYPES index `_0` (which the verifier proves names a `List[string]`),
    /// faulting `run.collection_limit` on a bound excess; `join` concatenates a
    /// `List[string]` with a separator into a string, faulting `run.text_limit` on a
    /// concatenation-ceiling excess.
    TextSplit(u16),
    TextLines(u16),
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
    /// The closed temporal arithmetic floor. Each faults `run.temporal_overflow`
    /// when its result would leave the supported day/nanosecond domain.
    DateAddDays,
    DateDaysBetween,
    DurationAdd,
    DurationSub,
    InstantAddDuration,
    InstantSubDuration,
    /// Checked arithmetic: `_0` is the fault-handler tape index. On overflow the op
    /// transfers there (carrying the post-pop stack) instead of faulting; otherwise
    /// it pushes the int result and falls through.
    IntAddChecked(usize),
    IntSubChecked(usize),
    IntMulChecked(usize),
    IntNegChecked(usize),
    IntDivChecked(usize),
    IntRemChecked(usize),
    /// Peek the int on top of the stack; fault `run.range` when it lies outside
    /// the inclusive `[lo, hi]` interval, else fall through with no stack
    /// effect. Decode rejects an empty interval (`lo > hi`).
    RangeGuard {
        lo: i64,
        hi: i64,
    },
    /// Construct a record of type index `_0` from its field values popped in
    /// reverse (f0 pushed first). The field count and per-field required flag come
    /// from the sealed record type.
    RecordNew(u16),
    /// Read field `_0` of the record on the stack; required-ness comes from the
    /// record value's sealed type (bare value vs optional).
    FieldGet(u16),
    /// `[record, value] → [record]`: store the bare value into field `_0`'s slot
    /// present, returning the updated record. Local product assignment.
    FieldSet(u16),
    /// `[record] → [record]`: clear field `_0`'s slot to vacant, returning the
    /// updated record. Only a sparse field is unset (the verifier proves it).
    FieldUnset(u16),
    /// Coerce a bare value into an optional (`Some`).
    SomeWrap,
    /// Push a vacant optional of the given optional type (a scalar or an enum).
    /// The runtime pushes only the vacant marker; the operand records the
    /// verifier-checked type — an optional scalar, or an optional enum for a
    /// defaulted sparse enum field.
    VacantLoad(ImageType),
    /// Construct enum `enum_idx`'s variant `variant` from its dense scalar payload
    /// popped in reverse (p0 pushed first).
    EnumConstruct {
        enum_idx: u16,
        variant: u16,
    },
    /// Pop an enum value and push its variant index as a bare int.
    EnumTag,
    /// Read payload leaf `field` of `variant` from the enum value on the stack,
    /// pushing its bare scalar. The variant operand types the leaf; the VM faults
    /// if the runtime value carries a different variant.
    EnumPayloadGet {
        variant: u16,
        field: u16,
    },
    /// `E, E → bool`: exact equality of two values of the same enum.
    EqEnum,
    /// Pop an optional; if present, push its bare value and fall through, else jump
    /// to this tape index. The only way to obtain a bare value from an optional.
    BranchPresent(usize),
    /// Fault with `run.unreachable`, carrying the static text at const index `_0`.
    /// Terminates the frame; it never falls through.
    Unreachable(u16),
    /// Pop a bool; on false fault with `run.assert` at this instruction's span, else
    /// fall through. The verifier admits it only in a test-entry function.
    Assert,
    /// Call function `_0` directly: pop its arguments (a0 pushed first, lands in
    /// callee local 0) and push its return value (nothing for a Unit return).
    Call(u16),
    /// `K → bool`: whether the cell site `_0` addresses is present.
    DurExists(u16),
    /// `K → T?`: read field site `_0`.
    DurReadField(u16),
    /// `K → Rec?`: read the whole entry at site `_0`.
    DurReadEntry(u16),
    /// `K, T →`: set the required field site `_0` (transaction-region only).
    DurSetRequired(u16),
    /// `K, T? →`: set (present) or clear (vacant) the sparse field site `_0`.
    DurSetSparse(u16),
    /// `K, Rec →`: create the entry at site `_0` (algebra `create`).
    DurCreateEntry(u16),
    /// `K, Rec →`: replace the entry at site `_0` (algebra `replace`).
    DurReplaceEntry(u16),
    /// `K →`: erase the sparse field site `_0` (no-op on absent).
    DurEraseField(u16),
    /// `K →`: erase the entry at site `_0` (no-op on absent).
    DurEraseEntry(u16),
    /// `K? → K?`: the next key at entry site `_0` (vacant in = first key).
    DurNextKey(u16),
    /// Open the export's single transaction region.
    TxnBegin,
    /// Close the export's single transaction region.
    TxnCommit,
    /// Push an empty `List` of COLLTYPES index `_0`.
    ListNew(u16),
    /// `[list, value] → [list]`: append the bare element, faulting
    /// `run.collection_limit` on a bound excess.
    ListAppend,
    /// `[list] → [int]`: the element count.
    ListLen,
    /// `[list, int] → [element]`: the bare element at the 0-based index.
    ListGet,
    /// Push an empty `Map` of COLLTYPES index `_0`.
    MapNew(u16),
    /// `[map, key, value] → [map]`: insert or replace by key in key order.
    MapInsert,
    /// `[map, key] → [value?]`: the value at `key`, or absent.
    MapGet,
    /// `[map] → [int]`: the entry count.
    MapLen,
    /// `[map, int] → [key]`: the bare key at the 0-based position in key order.
    MapKeyAt,
    /// `[map, int] → [value]`: the bare value at the 0-based position in key order.
    MapValueAt,
}

impl SealedInstr {
    /// Whether this instruction stages a durable mutation (design §D). Read and
    /// iteration ops are not mutations; the transaction markers are not either.
    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            SealedInstr::DurSetRequired(_)
                | SealedInstr::DurSetSparse(_)
                | SealedInstr::DurCreateEntry(_)
                | SealedInstr::DurReplaceEntry(_)
                | SealedInstr::DurEraseField(_)
                | SealedInstr::DurEraseEntry(_)
        )
    }

    /// Whether this instruction reads durable data (presence, field/entry read, or
    /// iteration).
    pub fn is_durable_read(&self) -> bool {
        matches!(
            self,
            SealedInstr::DurExists(_)
                | SealedInstr::DurReadField(_)
                | SealedInstr::DurReadEntry(_)
                | SealedInstr::DurNextKey(_)
        )
    }
}

/// What a durable operation site addresses within its root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealedSiteTarget {
    Entry,
    /// A field of the root's record, by field index.
    Field(u16),
}

/// A verified durable operation site: a root index plus an entry-or-field target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealedSite {
    pub root: u16,
    pub target: SealedSiteTarget,
}

/// A verified durable root: one placement of a record type over its ordered key
/// tuple. The tuple is empty for a singleton root and holds one or more
/// orderable durable-key scalars for a keyed root. The executable subset served
/// by the single-root kernel is the single-column keyed root; wider key arities
/// carry identity but reject at their operation sites (rechecked during flow
/// validation).
#[derive(Debug, Clone)]
pub struct SealedRoot {
    pub(crate) name: Rc<str>,
    pub(crate) keys: Vec<Scalar>,
    pub(crate) record: u16,
}

impl SealedRoot {
    pub fn name(&self) -> &str {
        &self.name
    }
    /// The ordered key tuple: empty for a singleton root, one scalar per column
    /// otherwise.
    pub fn keys(&self) -> &[Scalar] {
        &self.keys
    }
    pub fn record(&self) -> u16 {
        self.record
    }
}

/// A sealed record field: its name, bare value type, and whether it is required.
/// The type is a scalar for a durable-storable field or a closed enum for a
/// local-only value field. The name is carried so the path kernel can key
/// physical field leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedField {
    pub name: Rc<str>,
    pub ty: ImageType,
    pub required: bool,
}

/// A sealed record type: an ordered field list in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedRecordType {
    pub(crate) fields: Vec<SealedField>,
}

impl SealedRecordType {
    pub fn fields(&self) -> &[SealedField] {
        &self.fields
    }
}

/// One sealed enum variant: its member name, `category` flag, and dense payload in
/// declaration order. Each payload leaf is a bare (non-optional) [`ImageType`]: a
/// user `enum` member carries only bare scalars, while a built-in `Option`/`Result`
/// instantiation carries whatever concrete type its argument monomorphized to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedVariant {
    pub name: Rc<str>,
    pub category: bool,
    pub payload: Vec<ImageType>,
}

/// A sealed collection value type: a finite `List[T]` or ordered `Map[K, V]`. The
/// element/key/value types are bare [`ImageType`]s (possibly `Collection` tags into
/// an earlier row); the verifier proved every referenced index in range and that a
/// `Map` key is a bare scalar key type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealedCollectionType {
    List { elem: ImageType },
    Map { key: ImageType, value: ImageType },
}

/// A sealed enum type: an ordered variant list in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedEnumType {
    pub(crate) name: Rc<str>,
    pub(crate) variants: Vec<SealedVariant>,
}

impl SealedEnumType {
    /// The enum's declared name, used to render an enum value.
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn variants(&self) -> &[SealedVariant] {
        &self.variants
    }
}

/// A function's return shape, used to check `Return` and to render the result. A
/// record return names a sealed record type by index (a dense `struct` value); the
/// verifier proved the index in range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetShape {
    Unit,
    Scalar { scalar: Scalar, optional: bool },
    Record { idx: u16, optional: bool },
    Enum { idx: u16, optional: bool },
    Collection { idx: u16, optional: bool },
}

/// A source-position row: the instruction it maps and its 1-based line/column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpanRow {
    pub instr_index: usize,
    pub line: u32,
    pub column: u32,
}

/// A sealed function body.
#[derive(Debug, Clone)]
pub struct SealedFunction {
    pub(crate) name: Rc<str>,
    pub(crate) source: Rc<str>,
    pub(crate) params: Vec<ImageType>,
    pub(crate) ret: RetShape,
    pub(crate) local_count: u16,
    pub(crate) instrs: Vec<SealedInstr>,
    pub(crate) spans: Vec<SpanRow>,
    pub(crate) max_stack: usize,
    pub(crate) mutating: bool,
}

impl SealedFunction {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn params(&self) -> &[ImageType] {
        &self.params
    }
    pub fn ret(&self) -> RetShape {
        self.ret
    }
    pub fn local_count(&self) -> u16 {
        self.local_count
    }
    pub fn instrs(&self) -> &[SealedInstr] {
        &self.instrs
    }
    pub fn max_stack(&self) -> usize {
        self.max_stack
    }
    pub fn is_mutating(&self) -> bool {
        self.mutating
    }

    /// The source line/column for the instruction at `pc`, using the greatest span
    /// offset ≤ `pc` (design §C SPANS lookup rule). Every function with code has a
    /// span at instruction 0, so a mapping always exists.
    pub fn span_at(&self, pc: usize) -> Option<(u32, u32)> {
        self.spans
            .iter()
            .rev()
            .find(|row| row.instr_index <= pc)
            .map(|row| (row.line, row.column))
    }
}

/// A public export: a stable [`ExportId`] bound to a function, with its
/// verifier-derived effect class and per-root durable demand. The image carries no
/// export name, so an export is addressed only by its verified id.
#[derive(Debug, Clone)]
pub struct SealedExport {
    pub(crate) id: ExportId,
    pub(crate) func: u16,
    pub(crate) mutating: bool,
    pub(crate) demand: Demand,
}

/// The verifier-derived durable demand of an export over the single root: whether
/// its closure reads or writes. An input to the authority check, never a grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Demand {
    pub read: bool,
    pub write: bool,
}

impl Demand {
    /// Whether the export touches the store at all.
    pub fn is_empty(self) -> bool {
        !self.read && !self.write
    }
}

impl SealedExport {
    /// The stable identity this export is addressed by.
    pub fn id(&self) -> ExportId {
        self.id
    }
    pub fn function(&self) -> u16 {
        self.func
    }
    pub fn is_mutating(&self) -> bool {
        self.mutating
    }
    pub fn demand(&self) -> Demand {
        self.demand
    }
}

/// A verified test entry: a report name bound to the storeless zero-argument
/// function `marrow test` runs. It carries no wire identity — unlike an export the
/// name is a human report label, never an interface, demand, or durable identity.
#[derive(Debug, Clone)]
pub struct SealedTestEntry {
    pub(crate) name: Rc<str>,
    pub(crate) func: u16,
}

impl SealedTestEntry {
    /// The report name (the `test "..."` title).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The image function index this test runs.
    pub fn func(&self) -> u16 {
        self.func
    }
}

/// The verified, sealed program image.
#[derive(Debug, Clone)]
pub struct VerifiedImage {
    pub(crate) image_id: ImageId,
    pub(crate) types: Vec<SealedRecordType>,
    pub(crate) enums: Vec<SealedEnumType>,
    pub(crate) collections: Vec<SealedCollectionType>,
    pub(crate) roots: Vec<SealedRoot>,
    pub(crate) sites: Vec<SealedSite>,
    pub(crate) durable_contract: DurableContractId,
    pub(crate) consts: Vec<SealedConst>,
    pub(crate) functions: Vec<SealedFunction>,
    pub(crate) exports: Vec<SealedExport>,
    pub(crate) test_entries: Vec<SealedTestEntry>,
}

impl VerifiedImage {
    pub fn image_id(&self) -> ImageId {
        self.image_id
    }

    pub fn record_type(&self, index: u16) -> &SealedRecordType {
        &self.types[index as usize]
    }

    /// The sealed record types, indexed by image record-type index. Consumed by the
    /// CLI to render a returned record value's field names.
    pub fn record_types(&self) -> &[SealedRecordType] {
        &self.types
    }

    /// The sealed enum types, indexed by image enum index. Consumed by the CLI to
    /// render an enum value's declared and variant names.
    pub fn enums(&self) -> &[SealedEnumType] {
        &self.enums
    }

    /// The sealed collection value types, indexed by image COLLTYPES index. Consumed
    /// by the VM to type collection operands and by the CLI to render a value.
    pub fn collections(&self) -> &[SealedCollectionType] {
        &self.collections
    }

    /// The sealed collection type at `index`. The verifier proved every operand and
    /// return index in range.
    pub fn collection_type(&self, index: u16) -> SealedCollectionType {
        self.collections[index as usize]
    }

    /// The durable roots (0 or 1 at v0).
    pub fn roots(&self) -> &[SealedRoot] {
        &self.roots
    }

    /// The durable-contract identity of this image's durable graph, independently
    /// recomputed by the verifier and proven to match the bytes the image carried.
    /// A later store-admission phase binds an activated store to this id.
    pub fn durable_contract(&self) -> DurableContractId {
        self.durable_contract
    }

    /// The durable operation sites, indexed by image site index.
    pub fn sites(&self) -> &[SealedSite] {
        &self.sites
    }

    pub fn consts(&self) -> &[SealedConst] {
        &self.consts
    }

    pub fn function(&self, index: u16) -> &SealedFunction {
        &self.functions[index as usize]
    }

    pub fn functions(&self) -> &[SealedFunction] {
        &self.functions
    }

    pub fn exports(&self) -> &[SealedExport] {
        &self.exports
    }

    /// The test entries, in ascending report-name order. `marrow test` runs each
    /// storeless; a test entry is never dispatched as an export.
    pub fn test_entries(&self) -> &[SealedTestEntry] {
        &self.test_entries
    }

    /// The export bound to `id`, if any. The VM and CLI dispatch only through this
    /// verified id — a source name is resolved to an id outside the image (through
    /// the compiler's export directory) and never crosses this boundary.
    pub fn export_by_id(&self, id: ExportId) -> Option<&SealedExport> {
        self.exports.iter().find(|export| export.id == id)
    }
}
