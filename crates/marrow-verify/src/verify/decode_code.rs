//! Instruction decoding: operand readers, jump resolution, and the decoded-op model.

use super::reject;
use super::tables::decode_bare_scalar;
use crate::reader::Reader;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::SealedInstr;
use marrow_image::{
    ImageType, OP_ASSERT, OP_BOOL_NOT, OP_BRANCH_PRESENT, OP_BYTES_GE, OP_BYTES_GT, OP_BYTES_LE,
    OP_BYTES_LT, OP_CALL, OP_CONST_LOAD, OP_CONV_BYTES_TEXT, OP_CONV_STRING, OP_DATE_ADD_DAYS,
    OP_DATE_DAYS_BETWEEN, OP_DATE_GE, OP_DATE_GT, OP_DATE_LE, OP_DATE_LT, OP_DUR_CREATE_ENTRY,
    OP_DUR_ERASE_ENTRY, OP_DUR_ERASE_FIELD, OP_DUR_ERASE_GROUP, OP_DUR_EXISTS,
    OP_DUR_FAMILY_EXISTS, OP_DUR_INDEX_EXISTS, OP_DUR_INDEX_LOOKUP, OP_DUR_INDEX_SCAN,
    OP_DUR_ITERATE_BOUNDED, OP_DUR_READ_ENTRY, OP_DUR_READ_FIELD, OP_DUR_READ_GROUP,
    OP_DUR_REPLACE_ENTRY, OP_DUR_REPLACE_GROUP, OP_DUR_SET_REQUIRED, OP_DUR_SET_SPARSE,
    OP_DUR_SET_SPARSE_PRESENT, OP_DURATION_ADD, OP_DURATION_GE, OP_DURATION_GT, OP_DURATION_LE,
    OP_DURATION_LT, OP_DURATION_SUB, OP_ENUM_CONSTRUCT, OP_ENUM_PAYLOAD_GET, OP_ENUM_TAG,
    OP_EQ_BOOL, OP_EQ_BYTES, OP_EQ_DATE, OP_EQ_DURATION, OP_EQ_ENUM, OP_EQ_ID, OP_EQ_INSTANT,
    OP_EQ_INT, OP_EQ_TEXT, OP_FIELD_GET, OP_FIELD_SET, OP_FIELD_UNSET, OP_IDENTITY_KEY_PATH,
    OP_INSTANT_ADD_DURATION, OP_INSTANT_GE, OP_INSTANT_GT, OP_INSTANT_LE, OP_INSTANT_LT,
    OP_INSTANT_SUB_DURATION, OP_INT_ADD, OP_INT_ADD_CHECKED, OP_INT_DIV, OP_INT_DIV_CHECKED,
    OP_INT_GE, OP_INT_GT, OP_INT_LE, OP_INT_LT, OP_INT_MUL, OP_INT_MUL_CHECKED, OP_INT_NEG,
    OP_INT_NEG_CHECKED, OP_INT_REM, OP_INT_REM_CHECKED, OP_INT_SUB, OP_INT_SUB_CHECKED, OP_JUMP,
    OP_JUMP_IF_FALSE, OP_LIST_APPEND, OP_LIST_GET, OP_LIST_INDEX, OP_LIST_LEN, OP_LIST_NEW,
    OP_LOCAL_GET, OP_LOCAL_SET, OP_MAKE_IDENTITY, OP_MAP_GET, OP_MAP_INSERT, OP_MAP_KEY_AT,
    OP_MAP_LEN, OP_MAP_NEW, OP_MAP_REMOVE, OP_MAP_VALUE_AT, OP_POP, OP_RANGE_GUARD, OP_RECORD_NEW,
    OP_RETURN, OP_SOME_WRAP, OP_TEXT_CONCAT, OP_TEXT_CONTAINS, OP_TEXT_GE, OP_TEXT_GT,
    OP_TEXT_IS_EMPTY, OP_TEXT_JOIN, OP_TEXT_LE, OP_TEXT_LINES, OP_TEXT_LT, OP_TEXT_SPLIT,
    OP_TEXT_TRIM, OP_TODO, OP_TXN_BEGIN, OP_TXN_COMMIT, OP_UNREACHABLE, OP_VACANT_LOAD,
    OPTIONAL_FLAG, TAG_BOOL, TAG_BYTES, TAG_COLLECTION, TAG_DATE, TAG_DURATION, TAG_ENUM,
    TAG_INSTANT, TAG_INT, TAG_RECORD, TAG_TEXT,
};

/// A decoded instruction with resolved operands and its byte offset. Jump targets
/// are resolved from byte offsets to tape indices by [`resolve_jumps`] before flow
/// analysis, so a jump can only name an instruction boundary in its own function.
pub(super) struct Decoded {
    pub(super) instr: SealedInstr,
    /// Byte offset of this instruction in the function code (for span mapping).
    pub(super) offset: u32,
}

/// Decode the function bytecode into instructions on boundaries. Jump operands are
/// container byte offsets here; [`resolve_jumps`] rewrites them to tape indices.
pub(super) fn decode_code(code: &[u8]) -> Result<Vec<Decoded>, VerifyRejection> {
    let mut reader = Reader::new(code);
    let mut out = Vec::new();
    while !reader.is_empty() {
        let offset = (code.len() - reader.remaining()) as u32;
        let opcode = reader
            .u8()
            .ok_or(reject(VerifyPhase::Function, "short opcode"))?;
        let instr = match opcode {
            OP_CONST_LOAD => SealedInstr::ConstLoad(operand_u16(&mut reader)?),
            OP_LOCAL_GET => SealedInstr::LocalGet(operand_u16(&mut reader)?),
            OP_LOCAL_SET => SealedInstr::LocalSet(operand_u16(&mut reader)?),
            OP_POP => SealedInstr::Pop,
            OP_RETURN => SealedInstr::Return,
            // Jump targets are decoded as byte offsets, resolved to tape indices below.
            OP_JUMP => SealedInstr::Jump(operand_u32(&mut reader)? as usize),
            OP_JUMP_IF_FALSE => SealedInstr::JumpIfFalse(operand_u32(&mut reader)? as usize),
            OP_INT_ADD => SealedInstr::IntAdd,
            OP_INT_SUB => SealedInstr::IntSub,
            OP_INT_MUL => SealedInstr::IntMul,
            OP_INT_REM => SealedInstr::IntRem,
            OP_INT_DIV => SealedInstr::IntDiv,
            OP_INT_ADD_CHECKED => SealedInstr::IntAddChecked(operand_u32(&mut reader)? as usize),
            OP_INT_SUB_CHECKED => SealedInstr::IntSubChecked(operand_u32(&mut reader)? as usize),
            OP_INT_MUL_CHECKED => SealedInstr::IntMulChecked(operand_u32(&mut reader)? as usize),
            OP_INT_NEG_CHECKED => SealedInstr::IntNegChecked(operand_u32(&mut reader)? as usize),
            OP_INT_DIV_CHECKED => SealedInstr::IntDivChecked(operand_u32(&mut reader)? as usize),
            OP_INT_REM_CHECKED => SealedInstr::IntRemChecked(operand_u32(&mut reader)? as usize),
            OP_RANGE_GUARD => {
                let lo = operand_i64(&mut reader)?;
                let hi = operand_i64(&mut reader)?;
                if lo > hi {
                    return Err(reject(
                        VerifyPhase::Function,
                        "range-guard interval is empty",
                    ));
                }
                SealedInstr::RangeGuard { lo, hi }
            }
            OP_INT_NEG => SealedInstr::IntNeg,
            OP_BOOL_NOT => SealedInstr::BoolNot,
            OP_INT_LT => SealedInstr::IntLt,
            OP_INT_LE => SealedInstr::IntLe,
            OP_INT_GT => SealedInstr::IntGt,
            OP_INT_GE => SealedInstr::IntGe,
            OP_EQ_INT => SealedInstr::EqInt,
            OP_EQ_BOOL => SealedInstr::EqBool,
            OP_EQ_TEXT => SealedInstr::EqText,
            OP_TEXT_CONCAT => SealedInstr::TextConcat,
            OP_TEXT_LT => SealedInstr::TextLt,
            OP_TEXT_LE => SealedInstr::TextLe,
            OP_TEXT_GT => SealedInstr::TextGt,
            OP_TEXT_GE => SealedInstr::TextGe,
            OP_EQ_BYTES => SealedInstr::EqBytes,
            OP_BYTES_LT => SealedInstr::BytesLt,
            OP_BYTES_LE => SealedInstr::BytesLe,
            OP_BYTES_GT => SealedInstr::BytesGt,
            OP_BYTES_GE => SealedInstr::BytesGe,
            OP_CONV_STRING => SealedInstr::ConvString,
            OP_CONV_BYTES_TEXT => SealedInstr::ConvBytesText,
            OP_TEXT_IS_EMPTY => SealedInstr::TextIsEmpty,
            OP_TEXT_CONTAINS => SealedInstr::TextContains,
            OP_TEXT_TRIM => SealedInstr::TextTrim,
            OP_TEXT_SPLIT => SealedInstr::TextSplit(operand_u16(&mut reader)?),
            OP_TEXT_LINES => SealedInstr::TextLines(operand_u16(&mut reader)?),
            OP_TEXT_JOIN => SealedInstr::TextJoin,
            OP_EQ_DATE => SealedInstr::EqDate,
            OP_DATE_LT => SealedInstr::DateLt,
            OP_DATE_LE => SealedInstr::DateLe,
            OP_DATE_GT => SealedInstr::DateGt,
            OP_DATE_GE => SealedInstr::DateGe,
            OP_EQ_INSTANT => SealedInstr::EqInstant,
            OP_INSTANT_LT => SealedInstr::InstantLt,
            OP_INSTANT_LE => SealedInstr::InstantLe,
            OP_INSTANT_GT => SealedInstr::InstantGt,
            OP_INSTANT_GE => SealedInstr::InstantGe,
            OP_EQ_DURATION => SealedInstr::EqDuration,
            OP_DURATION_LT => SealedInstr::DurationLt,
            OP_DURATION_LE => SealedInstr::DurationLe,
            OP_DURATION_GT => SealedInstr::DurationGt,
            OP_DURATION_GE => SealedInstr::DurationGe,
            OP_DATE_ADD_DAYS => SealedInstr::DateAddDays,
            OP_DATE_DAYS_BETWEEN => SealedInstr::DateDaysBetween,
            OP_DURATION_ADD => SealedInstr::DurationAdd,
            OP_DURATION_SUB => SealedInstr::DurationSub,
            OP_INSTANT_ADD_DURATION => SealedInstr::InstantAddDuration,
            OP_INSTANT_SUB_DURATION => SealedInstr::InstantSubDuration,
            OP_RECORD_NEW => SealedInstr::RecordNew(operand_u16(&mut reader)?),
            OP_FIELD_GET => SealedInstr::FieldGet(operand_u16(&mut reader)?),
            OP_FIELD_SET => SealedInstr::FieldSet(operand_u16(&mut reader)?),
            OP_FIELD_UNSET => SealedInstr::FieldUnset(operand_u16(&mut reader)?),
            OP_SOME_WRAP => SealedInstr::SomeWrap,
            OP_VACANT_LOAD => SealedInstr::VacantLoad(decode_vacant_operand(&mut reader)?),
            OP_ENUM_CONSTRUCT => SealedInstr::EnumConstruct {
                enum_idx: operand_u16(&mut reader)?,
                variant: operand_u16(&mut reader)?,
            },
            OP_ENUM_TAG => SealedInstr::EnumTag,
            OP_ENUM_PAYLOAD_GET => SealedInstr::EnumPayloadGet {
                variant: operand_u16(&mut reader)?,
                field: operand_u16(&mut reader)?,
            },
            OP_EQ_ENUM => SealedInstr::EqEnum,
            OP_EQ_ID => SealedInstr::EqId,
            OP_MAKE_IDENTITY => SealedInstr::MakeIdentity {
                root: operand_u16(&mut reader)?,
                cols: operand_u16(&mut reader)?,
            },
            OP_IDENTITY_KEY_PATH => SealedInstr::IdentityKeyPath(operand_u16(&mut reader)?),
            OP_BRANCH_PRESENT => SealedInstr::BranchPresent(operand_u32(&mut reader)? as usize),
            OP_UNREACHABLE => SealedInstr::Unreachable(operand_u16(&mut reader)?),
            OP_TODO => SealedInstr::Todo(operand_u16(&mut reader)?),
            OP_ASSERT => SealedInstr::Assert,
            OP_CALL => SealedInstr::Call(operand_u16(&mut reader)?),
            OP_DUR_EXISTS => SealedInstr::DurExists(operand_u16(&mut reader)?),
            OP_DUR_FAMILY_EXISTS => SealedInstr::DurFamilyExists(operand_u16(&mut reader)?),
            OP_DUR_READ_FIELD => SealedInstr::DurReadField(operand_u16(&mut reader)?),
            OP_DUR_READ_ENTRY => SealedInstr::DurReadEntry(operand_u16(&mut reader)?),
            OP_DUR_SET_REQUIRED => SealedInstr::DurSetRequired(operand_u16(&mut reader)?),
            OP_DUR_SET_SPARSE => SealedInstr::DurSetSparse(operand_u16(&mut reader)?),
            OP_DUR_SET_SPARSE_PRESENT => {
                let site = operand_u16(&mut reader)?;
                let len = operand_u16(&mut reader)? as usize;
                // Bound the key-path length before allocation: the deepest executable
                // key-path is one column set per node from the root down, capped by the
                // per-node column and site-path caps. The exact arity is rechecked
                // against the site's reconstructed key-path in phase 3.
                if len == 0
                    || len
                        > marrow_image::bounds::MAX_KEY_COLUMNS
                            * marrow_image::bounds::MAX_SITE_PATH_STEPS
                {
                    return Err(reject(
                        VerifyPhase::Function,
                        "set-sparse-present key-path length out of range",
                    ));
                }
                let mut key_slots = Vec::with_capacity(len);
                for _ in 0..len {
                    key_slots.push(operand_u16(&mut reader)?);
                }
                SealedInstr::DurSetSparsePresent { site, key_slots }
            }
            OP_DUR_CREATE_ENTRY => SealedInstr::DurCreateEntry(operand_u16(&mut reader)?),
            OP_DUR_REPLACE_ENTRY => SealedInstr::DurReplaceEntry(operand_u16(&mut reader)?),
            OP_DUR_ERASE_FIELD => SealedInstr::DurEraseField(operand_u16(&mut reader)?),
            OP_DUR_ERASE_ENTRY => SealedInstr::DurEraseEntry(operand_u16(&mut reader)?),
            OP_DUR_READ_GROUP => SealedInstr::DurReadGroup(operand_u16(&mut reader)?),
            OP_DUR_REPLACE_GROUP => SealedInstr::DurReplaceGroup(operand_u16(&mut reader)?),
            OP_DUR_ERASE_GROUP => SealedInstr::DurEraseGroup(operand_u16(&mut reader)?),
            OP_DUR_ITERATE_BOUNDED => SealedInstr::DurIterateBounded {
                site: operand_u16(&mut reader)?,
                limit: operand_u32(&mut reader)?,
                from: operand_bool(&mut reader)?,
                list_ty: operand_u16(&mut reader)?,
            },
            OP_DUR_INDEX_SCAN => SealedInstr::DurIndexScan {
                site: operand_u16(&mut reader)?,
                limit: operand_u32(&mut reader)?,
                from: operand_bool(&mut reader)?,
                list_ty: operand_u16(&mut reader)?,
            },
            OP_DUR_INDEX_LOOKUP => SealedInstr::DurIndexLookup(operand_u16(&mut reader)?),
            OP_DUR_INDEX_EXISTS => SealedInstr::DurIndexExists(operand_u16(&mut reader)?),
            OP_TXN_BEGIN => SealedInstr::TxnBegin,
            OP_TXN_COMMIT => SealedInstr::TxnCommit,
            OP_LIST_NEW => SealedInstr::ListNew(operand_u16(&mut reader)?),
            OP_LIST_APPEND => SealedInstr::ListAppend,
            OP_LIST_LEN => SealedInstr::ListLen,
            OP_LIST_GET => SealedInstr::ListGet,
            OP_LIST_INDEX => SealedInstr::ListIndex,
            OP_MAP_NEW => SealedInstr::MapNew(operand_u16(&mut reader)?),
            OP_MAP_INSERT => SealedInstr::MapInsert,
            OP_MAP_REMOVE => SealedInstr::MapRemove,
            OP_MAP_GET => SealedInstr::MapGet,
            OP_MAP_LEN => SealedInstr::MapLen,
            OP_MAP_KEY_AT => SealedInstr::MapKeyAt,
            OP_MAP_VALUE_AT => SealedInstr::MapValueAt,
            _ => {
                return Err(reject(
                    VerifyPhase::Function,
                    "unknown or not-yet-supported opcode",
                ));
            }
        };
        out.push(Decoded { instr, offset });
    }
    Ok(out)
}

fn operand_u16(reader: &mut Reader) -> Result<u16, VerifyRejection> {
    reader
        .u16()
        .ok_or(reject(VerifyPhase::Function, "short u16 operand"))
}

fn operand_u32(reader: &mut Reader) -> Result<u32, VerifyRejection> {
    reader
        .u32()
        .ok_or(reject(VerifyPhase::Function, "short u32 operand"))
}

fn operand_i64(reader: &mut Reader) -> Result<i64, VerifyRejection> {
    reader
        .i64()
        .ok_or(reject(VerifyPhase::Function, "short i64 operand"))
}

/// A one-byte flag operand strictly `0x00` or `0x01`; any other byte is a malformed
/// image (a hostile image cannot smuggle a third state through a bool operand).
fn operand_bool(reader: &mut Reader) -> Result<bool, VerifyRejection> {
    match reader.u8() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(reject(VerifyPhase::Function, "malformed bool operand")),
    }
}

/// Decode a `VacantLoad` operand: a full optional type-ref — an optional scalar
/// (one byte) or an optional enum (a tag plus a big-endian `u16` index, the enum
/// bounds-checked in the abstract interpreter). Design §C, §D `VacantLoad`.
fn decode_vacant_operand(reader: &mut Reader) -> Result<ImageType, VerifyRejection> {
    let tag = reader
        .u8()
        .ok_or(reject(VerifyPhase::Function, "short vacant-load operand"))?;
    if tag & OPTIONAL_FLAG == 0 {
        return Err(reject(
            VerifyPhase::Function,
            "vacant-load operand must be optional",
        ));
    }
    let base = tag & !OPTIONAL_FLAG;
    match base {
        TAG_INT | TAG_BOOL | TAG_TEXT | TAG_BYTES | TAG_DATE | TAG_INSTANT | TAG_DURATION => {
            Ok(ImageType::Scalar {
                scalar: decode_bare_scalar(base).expect("scalar base"),
                optional: true,
            })
        }
        TAG_RECORD => {
            let idx = reader.u16().ok_or(reject(
                VerifyPhase::Function,
                "short vacant-load record index",
            ))?;
            Ok(ImageType::Record {
                idx,
                optional: true,
            })
        }
        TAG_ENUM => {
            let idx = reader.u16().ok_or(reject(
                VerifyPhase::Function,
                "short vacant-load enum index",
            ))?;
            Ok(ImageType::Enum {
                idx,
                optional: true,
            })
        }
        TAG_COLLECTION => {
            let idx = reader.u16().ok_or(reject(
                VerifyPhase::Function,
                "short vacant-load collection index",
            ))?;
            Ok(ImageType::Collection {
                idx,
                optional: true,
            })
        }
        _ => Err(reject(
            VerifyPhase::Function,
            "vacant-load operand must be an optional scalar, record, enum, or collection",
        )),
    }
}

/// Rewrite jump operands from container byte offsets to tape indices, rejecting a
/// target that is not an instruction boundary in this function.
pub(super) fn resolve_jumps(code: &mut [Decoded]) -> Result<(), VerifyRejection> {
    let offsets: Vec<u32> = code.iter().map(|decoded| decoded.offset).collect();
    let index_of = |byte_offset: usize| -> Result<usize, VerifyRejection> {
        offsets
            .binary_search(&(byte_offset as u32))
            .map_err(|_| reject(VerifyPhase::Function, "jump target is not a boundary"))
    };
    for decoded in code.iter_mut() {
        match &mut decoded.instr {
            SealedInstr::Jump(target)
            | SealedInstr::JumpIfFalse(target)
            | SealedInstr::BranchPresent(target)
            | SealedInstr::IntAddChecked(target)
            | SealedInstr::IntSubChecked(target)
            | SealedInstr::IntMulChecked(target)
            | SealedInstr::IntNegChecked(target)
            | SealedInstr::IntDivChecked(target)
            | SealedInstr::IntRemChecked(target) => {
                *target = index_of(*target)?;
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod opcode_bijection {
    //! Decode-bijection enforcement. Each opcode byte must decode to its own
    //! `SealedInstr` variant. A decode arm whose right-hand `OP_*` const is not in
    //! scope silently becomes an irrefutable binding pattern that swallows every
    //! opcode listed after it onto one variant; that class is caught here (and, at
    //! build time, by the workspace `unreachable_patterns`/`unused_variables` deny
    //! lints). The match over `SealedInstr` is the growth gate: a new opcode cannot
    //! land without extending both `canonical_bytes` and `SAMPLES`.
    use std::collections::HashMap;

    use marrow_image::Scalar;

    use super::*;

    /// The smallest valid byte encoding the decoder accepts for `instr`: its opcode
    /// followed by minimal in-range operands (a `u16`/`u32` is `0`, a `RangeGuard`
    /// interval is the non-empty `[0, 0]`, a `VacantLoad` type is optional `int`, a
    /// present-set key-path is one slot, a bounded traversal is a false `from`).
    fn canonical_bytes(instr: &SealedInstr) -> Vec<u8> {
        fn none(op: u8) -> Vec<u8> {
            vec![op]
        }
        fn u16op(op: u8) -> Vec<u8> {
            vec![op, 0, 0]
        }
        fn u32op(op: u8) -> Vec<u8> {
            vec![op, 0, 0, 0, 0]
        }
        fn two_u16(op: u8) -> Vec<u8> {
            vec![op, 0, 0, 0, 0]
        }
        match instr {
            SealedInstr::ConstLoad(_) => u16op(OP_CONST_LOAD),
            SealedInstr::LocalGet(_) => u16op(OP_LOCAL_GET),
            SealedInstr::LocalSet(_) => u16op(OP_LOCAL_SET),
            SealedInstr::Pop => none(OP_POP),
            SealedInstr::Return => none(OP_RETURN),
            SealedInstr::Jump(_) => u32op(OP_JUMP),
            SealedInstr::JumpIfFalse(_) => u32op(OP_JUMP_IF_FALSE),
            SealedInstr::IntAdd => none(OP_INT_ADD),
            SealedInstr::IntSub => none(OP_INT_SUB),
            SealedInstr::IntMul => none(OP_INT_MUL),
            SealedInstr::IntRem => none(OP_INT_REM),
            SealedInstr::IntDiv => none(OP_INT_DIV),
            SealedInstr::IntNeg => none(OP_INT_NEG),
            SealedInstr::BoolNot => none(OP_BOOL_NOT),
            SealedInstr::IntLt => none(OP_INT_LT),
            SealedInstr::IntLe => none(OP_INT_LE),
            SealedInstr::IntGt => none(OP_INT_GT),
            SealedInstr::IntGe => none(OP_INT_GE),
            SealedInstr::EqInt => none(OP_EQ_INT),
            SealedInstr::EqBool => none(OP_EQ_BOOL),
            SealedInstr::EqText => none(OP_EQ_TEXT),
            SealedInstr::TextConcat => none(OP_TEXT_CONCAT),
            SealedInstr::TextLt => none(OP_TEXT_LT),
            SealedInstr::TextLe => none(OP_TEXT_LE),
            SealedInstr::TextGt => none(OP_TEXT_GT),
            SealedInstr::TextGe => none(OP_TEXT_GE),
            SealedInstr::EqBytes => none(OP_EQ_BYTES),
            SealedInstr::BytesLt => none(OP_BYTES_LT),
            SealedInstr::BytesLe => none(OP_BYTES_LE),
            SealedInstr::BytesGt => none(OP_BYTES_GT),
            SealedInstr::BytesGe => none(OP_BYTES_GE),
            SealedInstr::ConvString => none(OP_CONV_STRING),
            SealedInstr::ConvBytesText => none(OP_CONV_BYTES_TEXT),
            SealedInstr::TextIsEmpty => none(OP_TEXT_IS_EMPTY),
            SealedInstr::TextContains => none(OP_TEXT_CONTAINS),
            SealedInstr::TextTrim => none(OP_TEXT_TRIM),
            SealedInstr::TextSplit(_) => u16op(OP_TEXT_SPLIT),
            SealedInstr::TextLines(_) => u16op(OP_TEXT_LINES),
            SealedInstr::TextJoin => none(OP_TEXT_JOIN),
            SealedInstr::EqDate => none(OP_EQ_DATE),
            SealedInstr::DateLt => none(OP_DATE_LT),
            SealedInstr::DateLe => none(OP_DATE_LE),
            SealedInstr::DateGt => none(OP_DATE_GT),
            SealedInstr::DateGe => none(OP_DATE_GE),
            SealedInstr::EqInstant => none(OP_EQ_INSTANT),
            SealedInstr::InstantLt => none(OP_INSTANT_LT),
            SealedInstr::InstantLe => none(OP_INSTANT_LE),
            SealedInstr::InstantGt => none(OP_INSTANT_GT),
            SealedInstr::InstantGe => none(OP_INSTANT_GE),
            SealedInstr::EqDuration => none(OP_EQ_DURATION),
            SealedInstr::DurationLt => none(OP_DURATION_LT),
            SealedInstr::DurationLe => none(OP_DURATION_LE),
            SealedInstr::DurationGt => none(OP_DURATION_GT),
            SealedInstr::DurationGe => none(OP_DURATION_GE),
            SealedInstr::DateAddDays => none(OP_DATE_ADD_DAYS),
            SealedInstr::DateDaysBetween => none(OP_DATE_DAYS_BETWEEN),
            SealedInstr::DurationAdd => none(OP_DURATION_ADD),
            SealedInstr::DurationSub => none(OP_DURATION_SUB),
            SealedInstr::InstantAddDuration => none(OP_INSTANT_ADD_DURATION),
            SealedInstr::InstantSubDuration => none(OP_INSTANT_SUB_DURATION),
            SealedInstr::IntAddChecked(_) => u32op(OP_INT_ADD_CHECKED),
            SealedInstr::IntSubChecked(_) => u32op(OP_INT_SUB_CHECKED),
            SealedInstr::IntMulChecked(_) => u32op(OP_INT_MUL_CHECKED),
            SealedInstr::IntNegChecked(_) => u32op(OP_INT_NEG_CHECKED),
            SealedInstr::IntDivChecked(_) => u32op(OP_INT_DIV_CHECKED),
            SealedInstr::IntRemChecked(_) => u32op(OP_INT_REM_CHECKED),
            SealedInstr::RangeGuard { .. } => {
                let mut bytes = vec![OP_RANGE_GUARD];
                bytes.extend_from_slice(&[0u8; 16]);
                bytes
            }
            SealedInstr::RecordNew(_) => u16op(OP_RECORD_NEW),
            SealedInstr::FieldGet(_) => u16op(OP_FIELD_GET),
            SealedInstr::FieldSet(_) => u16op(OP_FIELD_SET),
            SealedInstr::FieldUnset(_) => u16op(OP_FIELD_UNSET),
            SealedInstr::SomeWrap => none(OP_SOME_WRAP),
            SealedInstr::VacantLoad(_) => vec![OP_VACANT_LOAD, OPTIONAL_FLAG | TAG_INT],
            SealedInstr::EnumConstruct { .. } => two_u16(OP_ENUM_CONSTRUCT),
            SealedInstr::EnumTag => none(OP_ENUM_TAG),
            SealedInstr::EnumPayloadGet { .. } => two_u16(OP_ENUM_PAYLOAD_GET),
            SealedInstr::EqEnum => none(OP_EQ_ENUM),
            SealedInstr::EqId => none(OP_EQ_ID),
            SealedInstr::MakeIdentity { .. } => two_u16(OP_MAKE_IDENTITY),
            SealedInstr::IdentityKeyPath(_) => u16op(OP_IDENTITY_KEY_PATH),
            SealedInstr::BranchPresent(_) => u32op(OP_BRANCH_PRESENT),
            SealedInstr::Unreachable(_) => u16op(OP_UNREACHABLE),
            SealedInstr::Todo(_) => u16op(OP_TODO),
            SealedInstr::Assert => none(OP_ASSERT),
            SealedInstr::Call(_) => u16op(OP_CALL),
            SealedInstr::DurExists(_) => u16op(OP_DUR_EXISTS),
            SealedInstr::DurFamilyExists(_) => u16op(OP_DUR_FAMILY_EXISTS),
            SealedInstr::DurReadField(_) => u16op(OP_DUR_READ_FIELD),
            SealedInstr::DurReadEntry(_) => u16op(OP_DUR_READ_ENTRY),
            SealedInstr::DurSetRequired(_) => u16op(OP_DUR_SET_REQUIRED),
            SealedInstr::DurSetSparse(_) => u16op(OP_DUR_SET_SPARSE),
            SealedInstr::DurSetSparsePresent { .. } => {
                vec![OP_DUR_SET_SPARSE_PRESENT, 0, 0, 0, 1, 0, 0]
            }
            SealedInstr::DurCreateEntry(_) => u16op(OP_DUR_CREATE_ENTRY),
            SealedInstr::DurReplaceEntry(_) => u16op(OP_DUR_REPLACE_ENTRY),
            SealedInstr::DurEraseField(_) => u16op(OP_DUR_ERASE_FIELD),
            SealedInstr::DurEraseEntry(_) => u16op(OP_DUR_ERASE_ENTRY),
            SealedInstr::DurReadGroup(_) => u16op(OP_DUR_READ_GROUP),
            SealedInstr::DurReplaceGroup(_) => u16op(OP_DUR_REPLACE_GROUP),
            SealedInstr::DurEraseGroup(_) => u16op(OP_DUR_ERASE_GROUP),
            SealedInstr::DurIterateBounded { .. } => {
                let mut bytes = vec![OP_DUR_ITERATE_BOUNDED];
                bytes.extend_from_slice(&[0u8; 9]);
                bytes
            }
            SealedInstr::DurIndexScan { .. } => {
                let mut bytes = vec![OP_DUR_INDEX_SCAN];
                bytes.extend_from_slice(&[0u8; 9]);
                bytes
            }
            SealedInstr::DurIndexLookup(_) => u16op(OP_DUR_INDEX_LOOKUP),
            SealedInstr::DurIndexExists(_) => u16op(OP_DUR_INDEX_EXISTS),
            SealedInstr::TxnBegin => none(OP_TXN_BEGIN),
            SealedInstr::TxnCommit => none(OP_TXN_COMMIT),
            SealedInstr::ListNew(_) => u16op(OP_LIST_NEW),
            SealedInstr::ListAppend => none(OP_LIST_APPEND),
            SealedInstr::ListLen => none(OP_LIST_LEN),
            SealedInstr::ListGet => none(OP_LIST_GET),
            SealedInstr::ListIndex => none(OP_LIST_INDEX),
            SealedInstr::MapNew(_) => u16op(OP_MAP_NEW),
            SealedInstr::MapInsert => none(OP_MAP_INSERT),
            SealedInstr::MapRemove => none(OP_MAP_REMOVE),
            SealedInstr::MapGet => none(OP_MAP_GET),
            SealedInstr::MapLen => none(OP_MAP_LEN),
            SealedInstr::MapKeyAt => none(OP_MAP_KEY_AT),
            SealedInstr::MapValueAt => none(OP_MAP_VALUE_AT),
        }
    }

    /// One value of every `SealedInstr` variant. Kept complete by the exhaustive match
    /// in [`canonical_bytes`]: a new opcode fails that match to compile, and this list
    /// gains the matching entry so the round trip covers it.
    fn samples() -> Vec<SealedInstr> {
        let optional_int = ImageType::Scalar {
            scalar: Scalar::Int,
            optional: true,
        };
        vec![
            SealedInstr::ConstLoad(0),
            SealedInstr::LocalGet(0),
            SealedInstr::LocalSet(0),
            SealedInstr::Pop,
            SealedInstr::Return,
            SealedInstr::Jump(0),
            SealedInstr::JumpIfFalse(0),
            SealedInstr::IntAdd,
            SealedInstr::IntSub,
            SealedInstr::IntMul,
            SealedInstr::IntRem,
            SealedInstr::IntDiv,
            SealedInstr::IntNeg,
            SealedInstr::BoolNot,
            SealedInstr::IntLt,
            SealedInstr::IntLe,
            SealedInstr::IntGt,
            SealedInstr::IntGe,
            SealedInstr::EqInt,
            SealedInstr::EqBool,
            SealedInstr::EqText,
            SealedInstr::TextConcat,
            SealedInstr::TextLt,
            SealedInstr::TextLe,
            SealedInstr::TextGt,
            SealedInstr::TextGe,
            SealedInstr::EqBytes,
            SealedInstr::BytesLt,
            SealedInstr::BytesLe,
            SealedInstr::BytesGt,
            SealedInstr::BytesGe,
            SealedInstr::ConvString,
            SealedInstr::ConvBytesText,
            SealedInstr::TextIsEmpty,
            SealedInstr::TextContains,
            SealedInstr::TextTrim,
            SealedInstr::TextSplit(0),
            SealedInstr::TextLines(0),
            SealedInstr::TextJoin,
            SealedInstr::EqDate,
            SealedInstr::DateLt,
            SealedInstr::DateLe,
            SealedInstr::DateGt,
            SealedInstr::DateGe,
            SealedInstr::EqInstant,
            SealedInstr::InstantLt,
            SealedInstr::InstantLe,
            SealedInstr::InstantGt,
            SealedInstr::InstantGe,
            SealedInstr::EqDuration,
            SealedInstr::DurationLt,
            SealedInstr::DurationLe,
            SealedInstr::DurationGt,
            SealedInstr::DurationGe,
            SealedInstr::DateAddDays,
            SealedInstr::DateDaysBetween,
            SealedInstr::DurationAdd,
            SealedInstr::DurationSub,
            SealedInstr::InstantAddDuration,
            SealedInstr::InstantSubDuration,
            SealedInstr::IntAddChecked(0),
            SealedInstr::IntSubChecked(0),
            SealedInstr::IntMulChecked(0),
            SealedInstr::IntNegChecked(0),
            SealedInstr::IntDivChecked(0),
            SealedInstr::IntRemChecked(0),
            SealedInstr::RangeGuard { lo: 0, hi: 0 },
            SealedInstr::RecordNew(0),
            SealedInstr::FieldGet(0),
            SealedInstr::FieldSet(0),
            SealedInstr::FieldUnset(0),
            SealedInstr::SomeWrap,
            SealedInstr::VacantLoad(optional_int),
            SealedInstr::EnumConstruct {
                enum_idx: 0,
                variant: 0,
            },
            SealedInstr::EnumTag,
            SealedInstr::EnumPayloadGet {
                variant: 0,
                field: 0,
            },
            SealedInstr::EqEnum,
            SealedInstr::EqId,
            SealedInstr::MakeIdentity { root: 0, cols: 0 },
            SealedInstr::IdentityKeyPath(0),
            SealedInstr::BranchPresent(0),
            SealedInstr::Unreachable(0),
            SealedInstr::Todo(0),
            SealedInstr::Assert,
            SealedInstr::Call(0),
            SealedInstr::DurExists(0),
            SealedInstr::DurFamilyExists(0),
            SealedInstr::DurReadField(0),
            SealedInstr::DurReadEntry(0),
            SealedInstr::DurSetRequired(0),
            SealedInstr::DurSetSparse(0),
            SealedInstr::DurSetSparsePresent {
                site: 0,
                key_slots: vec![0],
            },
            SealedInstr::DurCreateEntry(0),
            SealedInstr::DurReplaceEntry(0),
            SealedInstr::DurEraseField(0),
            SealedInstr::DurEraseEntry(0),
            SealedInstr::DurReadGroup(0),
            SealedInstr::DurReplaceGroup(0),
            SealedInstr::DurEraseGroup(0),
            SealedInstr::DurIterateBounded {
                site: 0,
                limit: 0,
                from: false,
                list_ty: 0,
            },
            SealedInstr::DurIndexScan {
                site: 0,
                limit: 0,
                from: false,
                list_ty: 0,
            },
            SealedInstr::DurIndexLookup(0),
            SealedInstr::DurIndexExists(0),
            SealedInstr::TxnBegin,
            SealedInstr::TxnCommit,
            SealedInstr::ListNew(0),
            SealedInstr::ListAppend,
            SealedInstr::ListLen,
            SealedInstr::ListGet,
            SealedInstr::ListIndex,
            SealedInstr::MapNew(0),
            SealedInstr::MapInsert,
            SealedInstr::MapRemove,
            SealedInstr::MapGet,
            SealedInstr::MapLen,
            SealedInstr::MapKeyAt,
            SealedInstr::MapValueAt,
        ]
    }

    #[test]
    fn every_opcode_decodes_to_its_own_variant() {
        let mut by_opcode: HashMap<u8, std::mem::Discriminant<SealedInstr>> = HashMap::new();
        for sample in samples() {
            let want = std::mem::discriminant(&sample);
            let bytes = canonical_bytes(&sample);
            let opcode = bytes[0];
            let decoded = decode_code(&bytes)
                .unwrap_or_else(|e| panic!("canonical bytes for {sample:?} must decode: {e}"));
            assert_eq!(
                decoded.len(),
                1,
                "{sample:?} must encode as exactly one instruction",
            );
            assert_eq!(
                std::mem::discriminant(&decoded[0].instr),
                want,
                "opcode {opcode:#04x} decoded to {:?}, expected {sample:?} — a decode arm \
                 is mapping this opcode to the wrong variant",
                decoded[0].instr,
            );
            // Injectivity: two variants sharing one opcode is the wildcard-binding
            // collapse (an unimported `OP_*` const bound as a catch-all).
            if let Some(previous) = by_opcode.insert(opcode, want) {
                assert_eq!(
                    previous, want,
                    "opcode {opcode:#04x} decodes to two different variants",
                );
            }
        }
    }
}
