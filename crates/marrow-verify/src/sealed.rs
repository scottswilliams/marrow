//! The sealed, verified image the VM accepts (design §A/§E phase 6).
//!
//! [`VerifiedImage`] has all-private fields and a single constructor,
//! [`crate::verify`]. Sealing produces a typed instruction tape per function:
//! [`SealedInstr`] values with jumps resolved to instruction indices and operands
//! as bounds-checked typed handles, so the VM never sees raw opcode bytes and a
//! verifier/VM width or discriminant disagreement is unrepresentable.

use std::rc::Rc;

use marrow_image::{ExportId, ImageId, Scalar};

/// A resolved constant value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealedConst {
    Int(i64),
    Bool(bool),
    Text(Rc<str>),
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
    /// Construct a record of type index `_0` from its field values popped in
    /// reverse (f0 pushed first). The field count and per-field required flag come
    /// from the sealed record type.
    RecordNew(u16),
    /// Read field `_0` of the record on the stack; required-ness comes from the
    /// record value's sealed type (bare value vs optional).
    FieldGet(u16),
    /// Coerce a bare value into an optional (`Some`).
    SomeWrap,
    /// Push a vacant optional of the given scalar type. The runtime pushes only the
    /// vacant marker; the scalar records the verifier-checked operand type.
    VacantLoad(Scalar),
    /// Pop an optional; if present, push its bare value and fall through, else jump
    /// to this tape index. The only way to obtain a bare value from an optional.
    BranchPresent(usize),
    /// Fault with `run.unreachable`, carrying the static text at const index `_0`.
    /// Terminates the frame; it never falls through.
    Unreachable(u16),
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

/// A verified durable root: one keyed placement of a record type.
#[derive(Debug, Clone)]
pub struct SealedRoot {
    pub(crate) name: Rc<str>,
    pub(crate) key: Scalar,
    pub(crate) record: u16,
}

impl SealedRoot {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn key(&self) -> Scalar {
        self.key
    }
    pub fn record(&self) -> u16 {
        self.record
    }
}

/// A sealed record field: its name, scalar type, and whether it is required. The
/// name is carried so the path kernel can key physical field leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedField {
    pub name: Rc<str>,
    pub scalar: Scalar,
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

/// A function's return shape, used to check `Return` and to render the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetShape {
    Unit,
    Scalar { scalar: Scalar, optional: bool },
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
    pub(crate) params: Vec<Scalar>,
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
    pub fn params(&self) -> &[Scalar] {
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

/// The verified, sealed program image.
#[derive(Debug, Clone)]
pub struct VerifiedImage {
    pub(crate) image_id: ImageId,
    pub(crate) types: Vec<SealedRecordType>,
    pub(crate) roots: Vec<SealedRoot>,
    pub(crate) sites: Vec<SealedSite>,
    pub(crate) consts: Vec<SealedConst>,
    pub(crate) functions: Vec<SealedFunction>,
    pub(crate) exports: Vec<SealedExport>,
}

impl VerifiedImage {
    pub fn image_id(&self) -> ImageId {
        self.image_id
    }

    pub fn record_type(&self, index: u16) -> &SealedRecordType {
        &self.types[index as usize]
    }

    /// The durable roots (0 or 1 at v0).
    pub fn roots(&self) -> &[SealedRoot] {
        &self.roots
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

    /// The export bound to `id`, if any. The VM and CLI dispatch only through this
    /// verified id — a source name is resolved to an id outside the image (through
    /// the compiler's export directory) and never crosses this boundary.
    pub fn export_by_id(&self, id: ExportId) -> Option<&SealedExport> {
        self.exports.iter().find(|export| export.id == id)
    }
}
