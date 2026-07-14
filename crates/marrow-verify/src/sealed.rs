//! The sealed, verified image the VM accepts (design §A/§E phase 6).
//!
//! [`VerifiedImage`] has all-private fields and a single constructor,
//! [`crate::verify`]. Sealing produces a typed instruction tape per function:
//! [`SealedInstr`] values with jumps resolved to instruction indices and operands
//! as bounds-checked typed handles, so the VM never sees raw opcode bytes and a
//! verifier/VM width or discriminant disagreement is unrepresentable.

use std::rc::Rc;

use marrow_image::{ImageId, Scalar};

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
    /// Call function `_0` directly: pop its arguments (a0 pushed first, lands in
    /// callee local 0) and push its return value (nothing for a Unit return).
    Call(u16),
}

/// A sealed record field: its scalar type and whether it is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealedField {
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

/// A public export: a name bound to a function, with its verifier-derived effect
/// class.
#[derive(Debug, Clone)]
pub struct SealedExport {
    pub(crate) name: Rc<str>,
    pub(crate) func: u16,
    pub(crate) mutating: bool,
}

impl SealedExport {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn function(&self) -> u16 {
        self.func
    }
    pub fn is_mutating(&self) -> bool {
        self.mutating
    }
}

/// The verified, sealed program image.
#[derive(Debug, Clone)]
pub struct VerifiedImage {
    pub(crate) image_id: ImageId,
    pub(crate) types: Vec<SealedRecordType>,
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

    /// The export bound to `name`, if any.
    pub fn export(&self, name: &str) -> Option<&SealedExport> {
        self.exports.iter().find(|export| export.name() == name)
    }
}
