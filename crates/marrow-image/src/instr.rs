//! The opcode set (design §D).
//!
//! The byte encoding is the frozen container contract: opcode `u8` followed by
//! big-endian immediate operands. `marrow-image` owns the *encoder* side; the
//! verifier owns the only decoder. Jump operands are `u32` **byte offsets** in the
//! container. In this draft form a jump instead carries the *instruction index* of
//! its target, and the encoder resolves indices to byte offsets once the code
//! layout is known — so the compiler never computes byte offsets by hand.

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
pub const OP_CONV_STRING_INT: u8 = 0x50;
pub const OP_CONV_STRING_BOOL: u8 = 0x51;
pub const OP_CONV_BYTES_TEXT: u8 = 0x52;
pub const OP_TEXT_IS_EMPTY: u8 = 0x60;
pub const OP_TEXT_CONTAINS: u8 = 0x61;
pub const OP_TEXT_TRIM: u8 = 0x62;
pub const OP_RECORD_NEW: u8 = 0x20;
pub const OP_FIELD_GET: u8 = 0x21;
pub const OP_SOME_WRAP: u8 = 0x22;
pub const OP_VACANT_LOAD: u8 = 0x23;
pub const OP_DUR_EXISTS: u8 = 0x30;
pub const OP_DUR_READ_FIELD: u8 = 0x31;
pub const OP_DUR_READ_ENTRY: u8 = 0x32;
pub const OP_DUR_SET_REQUIRED: u8 = 0x33;
pub const OP_DUR_SET_SPARSE: u8 = 0x34;
pub const OP_DUR_CREATE_ENTRY: u8 = 0x35;
pub const OP_DUR_REPLACE_ENTRY: u8 = 0x36;
pub const OP_DUR_ERASE_FIELD: u8 = 0x37;
pub const OP_DUR_ERASE_ENTRY: u8 = 0x38;
pub const OP_DUR_NEXT_KEY: u8 = 0x39;
pub const OP_TXN_BEGIN: u8 = 0x3C;
pub const OP_TXN_COMMIT: u8 = 0x3D;

/// A draft instruction. Jump targets are instruction indices into the function's
/// own instruction list; the encoder rewrites them to container byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instr {
    ConstLoad(u16),
    LocalGet(u16),
    LocalSet(u16),
    Pop,
    Return,
    Call(u16),
    Jump(u32),
    JumpIfFalse(u32),
    BranchPresent(u32),
    /// Fault with `run.unreachable`, carrying the static text at const index `_0`.
    /// The sole application-invariant fault; it never falls through.
    Unreachable(u16),
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
    /// `int → string`: canonical decimal rendering.
    ConvStringInt,
    /// `bool → string`: `"true"`/`"false"`.
    ConvStringBool,
    /// `string → bytes`: the UTF-8 bytes of the text.
    ConvBytesText,
    /// The closed pure text floor. `string → bool`, `string, string → bool`, and
    /// `string → string`.
    TextIsEmpty,
    TextContains,
    TextTrim,
    RecordNew(u16),
    FieldGet(u16),
    SomeWrap,
    VacantLoad(ImageType),
    DurExists(u16),
    DurReadField(u16),
    DurReadEntry(u16),
    DurSetRequired(u16),
    DurSetSparse(u16),
    DurCreateEntry(u16),
    DurReplaceEntry(u16),
    DurEraseField(u16),
    DurEraseEntry(u16),
    DurNextKey(u16),
    TxnBegin,
    TxnCommit,
}

impl Instr {
    /// The opcode byte for this instruction.
    pub(crate) fn opcode(&self) -> u8 {
        match self {
            Instr::ConstLoad(_) => OP_CONST_LOAD,
            Instr::LocalGet(_) => OP_LOCAL_GET,
            Instr::LocalSet(_) => OP_LOCAL_SET,
            Instr::Pop => OP_POP,
            Instr::Return => OP_RETURN,
            Instr::Call(_) => OP_CALL,
            Instr::Jump(_) => OP_JUMP,
            Instr::JumpIfFalse(_) => OP_JUMP_IF_FALSE,
            Instr::BranchPresent(_) => OP_BRANCH_PRESENT,
            Instr::Unreachable(_) => OP_UNREACHABLE,
            Instr::IntAdd => OP_INT_ADD,
            Instr::IntSub => OP_INT_SUB,
            Instr::IntMul => OP_INT_MUL,
            Instr::IntRem => OP_INT_REM,
            Instr::IntDiv => OP_INT_DIV,
            Instr::IntNeg => OP_INT_NEG,
            Instr::BoolNot => OP_BOOL_NOT,
            Instr::IntLt => OP_INT_LT,
            Instr::IntLe => OP_INT_LE,
            Instr::IntGt => OP_INT_GT,
            Instr::IntGe => OP_INT_GE,
            Instr::EqInt => OP_EQ_INT,
            Instr::EqBool => OP_EQ_BOOL,
            Instr::EqText => OP_EQ_TEXT,
            Instr::TextConcat => OP_TEXT_CONCAT,
            Instr::TextLt => OP_TEXT_LT,
            Instr::TextLe => OP_TEXT_LE,
            Instr::TextGt => OP_TEXT_GT,
            Instr::TextGe => OP_TEXT_GE,
            Instr::EqBytes => OP_EQ_BYTES,
            Instr::BytesLt => OP_BYTES_LT,
            Instr::BytesLe => OP_BYTES_LE,
            Instr::BytesGt => OP_BYTES_GT,
            Instr::BytesGe => OP_BYTES_GE,
            Instr::ConvStringInt => OP_CONV_STRING_INT,
            Instr::ConvStringBool => OP_CONV_STRING_BOOL,
            Instr::ConvBytesText => OP_CONV_BYTES_TEXT,
            Instr::TextIsEmpty => OP_TEXT_IS_EMPTY,
            Instr::TextContains => OP_TEXT_CONTAINS,
            Instr::TextTrim => OP_TEXT_TRIM,
            Instr::RecordNew(_) => OP_RECORD_NEW,
            Instr::FieldGet(_) => OP_FIELD_GET,
            Instr::SomeWrap => OP_SOME_WRAP,
            Instr::VacantLoad(_) => OP_VACANT_LOAD,
            Instr::DurExists(_) => OP_DUR_EXISTS,
            Instr::DurReadField(_) => OP_DUR_READ_FIELD,
            Instr::DurReadEntry(_) => OP_DUR_READ_ENTRY,
            Instr::DurSetRequired(_) => OP_DUR_SET_REQUIRED,
            Instr::DurSetSparse(_) => OP_DUR_SET_SPARSE,
            Instr::DurCreateEntry(_) => OP_DUR_CREATE_ENTRY,
            Instr::DurReplaceEntry(_) => OP_DUR_REPLACE_ENTRY,
            Instr::DurEraseField(_) => OP_DUR_ERASE_FIELD,
            Instr::DurEraseEntry(_) => OP_DUR_ERASE_ENTRY,
            Instr::DurNextKey(_) => OP_DUR_NEXT_KEY,
            Instr::TxnBegin => OP_TXN_BEGIN,
            Instr::TxnCommit => OP_TXN_COMMIT,
        }
    }

    /// The number of immediate-operand bytes after the opcode.
    fn operand_len(&self) -> usize {
        match self {
            Instr::ConstLoad(_)
            | Instr::LocalGet(_)
            | Instr::LocalSet(_)
            | Instr::Unreachable(_)
            | Instr::Call(_)
            | Instr::RecordNew(_)
            | Instr::FieldGet(_)
            | Instr::DurExists(_)
            | Instr::DurReadField(_)
            | Instr::DurReadEntry(_)
            | Instr::DurSetRequired(_)
            | Instr::DurSetSparse(_)
            | Instr::DurCreateEntry(_)
            | Instr::DurReplaceEntry(_)
            | Instr::DurEraseField(_)
            | Instr::DurEraseEntry(_)
            | Instr::DurNextKey(_) => 2,
            Instr::Jump(_) | Instr::JumpIfFalse(_) | Instr::BranchPresent(_) => 4,
            // A `ImageType` operand is a 1-byte tag; the only draft producer of a
            // `VacantLoad` uses an optional scalar, which never carries a record
            // index, so the operand is exactly one byte.
            Instr::VacantLoad(_) => 1,
            _ => 0,
        }
    }

    /// This instruction's total encoded byte width (opcode + operands).
    pub(crate) fn encoded_len(&self) -> usize {
        1 + self.operand_len()
    }
}
