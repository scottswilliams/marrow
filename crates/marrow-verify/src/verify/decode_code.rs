//! Instruction decoding: operand readers, jump resolution, and the decoded-op model.

use super::reject;
use super::type_ref::{Optionality, TagSet, TypePosition, decode_type_ref, type_position};
use crate::reader::Reader;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::SealedInstr;
use marrow_image::{
    OP_ASSERT, OP_BOOL_NOT, OP_BRANCH_PRESENT, OP_BYTES_GE, OP_BYTES_GT, OP_BYTES_LE, OP_BYTES_LT,
    OP_CALL, OP_CONST_LOAD, OP_CONV_BYTES_TEXT, OP_CONV_STRING, OP_DATE_ADD_DAYS,
    OP_DATE_DAYS_BETWEEN, OP_DATE_GE, OP_DATE_GT, OP_DATE_LE, OP_DATE_LT, OP_DUR_CREATE_ENTRY,
    OP_DUR_ERASE_ENTRY, OP_DUR_ERASE_FIELD, OP_DUR_ERASE_GROUP, OP_DUR_EXISTS,
    OP_DUR_FAMILY_EXISTS, OP_DUR_INDEX_EXISTS, OP_DUR_INDEX_LOOKUP, OP_DUR_INDEX_SCAN,
    OP_DUR_ITERATE_BOUNDED, OP_DUR_READ_ENTRY, OP_DUR_READ_FIELD, OP_DUR_READ_FIELD_PRESENT,
    OP_DUR_READ_GROUP, OP_DUR_READ_GROUP_PRESENT, OP_DUR_REPLACE_ENTRY, OP_DUR_REPLACE_GROUP,
    OP_DUR_SET_FIELD, OP_DURATION_ADD, OP_DURATION_GE, OP_DURATION_GT, OP_DURATION_LE,
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
};

/// A decoded instruction with resolved operands and its byte offset. Jump targets
/// are resolved from byte offsets to tape indices by [`resolve_jumps`] before flow
/// analysis, so a jump can only name an instruction boundary in its own function.
pub(super) struct Decoded {
    pub(super) instr: SealedInstr,
    /// Byte offset of this instruction in the function code (for span mapping).
    pub(super) offset: u32,
}

/// The opcode groups, tried in order. Each returns `None` for an opcode it does not
/// own and consumes no operand bytes then, so the reader is only advanced by the group
/// that decodes the instruction. Verification decodes a program image once, so the
/// walk down this short list costs nothing the VM's own dispatch would.
type Decoder = fn(u8, &mut Reader<'_>) -> Result<Option<SealedInstr>, VerifyRejection>;

const DECODERS: &[Decoder] = &[
    decode_operand_and_scalar,
    decode_value_shape,
    decode_durable,
    decode_collection,
];

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
        let mut decoded = None;
        for decode in DECODERS {
            if let Some(instr) = decode(opcode, &mut reader)? {
                decoded = Some(instr);
                break;
            }
        }
        let Some(instr) = decoded else {
            return Err(reject(
                VerifyPhase::Function,
                "unknown or not-yet-supported opcode",
            ));
        };
        out.push(Decoded { instr, offset });
    }
    Ok(out)
}

/// Control flow, integer and boolean arithmetic, the text and bytes operators, the
/// temporal comparisons and arithmetic, and the scalar conversions.
fn decode_operand_and_scalar(
    opcode: u8,
    reader: &mut Reader<'_>,
) -> Result<Option<SealedInstr>, VerifyRejection> {
    Ok(Some(match opcode {
        OP_CONST_LOAD => SealedInstr::ConstLoad(operand_u16(reader)?),
        OP_LOCAL_GET => SealedInstr::LocalGet(operand_u16(reader)?),
        OP_LOCAL_SET => SealedInstr::LocalSet(operand_u16(reader)?),
        OP_POP => SealedInstr::Pop,
        OP_RETURN => SealedInstr::Return,
        // Jump targets are decoded as byte offsets, resolved to tape indices below.
        OP_JUMP => SealedInstr::Jump(operand_u32(reader)? as usize),
        OP_JUMP_IF_FALSE => SealedInstr::JumpIfFalse(operand_u32(reader)? as usize),
        OP_INT_ADD => SealedInstr::IntAdd,
        OP_INT_SUB => SealedInstr::IntSub,
        OP_INT_MUL => SealedInstr::IntMul,
        OP_INT_REM => SealedInstr::IntRem,
        OP_INT_DIV => SealedInstr::IntDiv,
        OP_INT_ADD_CHECKED => SealedInstr::IntAddChecked(operand_u32(reader)? as usize),
        OP_INT_SUB_CHECKED => SealedInstr::IntSubChecked(operand_u32(reader)? as usize),
        OP_INT_MUL_CHECKED => SealedInstr::IntMulChecked(operand_u32(reader)? as usize),
        OP_INT_NEG_CHECKED => SealedInstr::IntNegChecked(operand_u32(reader)? as usize),
        OP_INT_DIV_CHECKED => SealedInstr::IntDivChecked(operand_u32(reader)? as usize),
        OP_INT_REM_CHECKED => SealedInstr::IntRemChecked(operand_u32(reader)? as usize),
        OP_RANGE_GUARD => {
            let lo = operand_i64(reader)?;
            let hi = operand_i64(reader)?;
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
        OP_TEXT_SPLIT => SealedInstr::TextSplit(operand_u16(reader)?),
        OP_TEXT_LINES => SealedInstr::TextLines(operand_u16(reader)?),
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
        _ => return Ok(None),
    }))
}

/// Record fields, optionals, enum construction and payload reads, identity values,
/// the diverging markers, and the call opcode.
fn decode_value_shape(
    opcode: u8,
    reader: &mut Reader<'_>,
) -> Result<Option<SealedInstr>, VerifyRejection> {
    Ok(Some(match opcode {
        OP_RECORD_NEW => SealedInstr::RecordNew(operand_u16(reader)?),
        OP_FIELD_GET => SealedInstr::FieldGet(operand_u16(reader)?),
        OP_FIELD_SET => SealedInstr::FieldSet(operand_u16(reader)?),
        OP_FIELD_UNSET => SealedInstr::FieldUnset(operand_u16(reader)?),
        OP_SOME_WRAP => SealedInstr::SomeWrap,
        OP_VACANT_LOAD => SealedInstr::VacantLoad(decode_type_ref(reader, &VACANT_LOAD)?),
        OP_ENUM_CONSTRUCT => SealedInstr::EnumConstruct {
            enum_idx: operand_u16(reader)?,
            variant: operand_u16(reader)?,
        },
        OP_ENUM_TAG => SealedInstr::EnumTag,
        OP_ENUM_PAYLOAD_GET => SealedInstr::EnumPayloadGet {
            variant: operand_u16(reader)?,
            field: operand_u16(reader)?,
        },
        OP_EQ_ENUM => SealedInstr::EqEnum,
        OP_EQ_ID => SealedInstr::EqId,
        OP_MAKE_IDENTITY => SealedInstr::MakeIdentity {
            root: operand_u16(reader)?,
            cols: operand_u16(reader)?,
        },
        OP_IDENTITY_KEY_PATH => SealedInstr::IdentityKeyPath(operand_u16(reader)?),
        OP_BRANCH_PRESENT => SealedInstr::BranchPresent(operand_u32(reader)? as usize),
        OP_UNREACHABLE => SealedInstr::Unreachable(operand_u16(reader)?),
        OP_TODO => SealedInstr::Todo(operand_u16(reader)?),
        OP_ASSERT => SealedInstr::Assert,
        OP_CALL => SealedInstr::Call(operand_u16(reader)?),
        _ => return Ok(None),
    }))
}

/// The durable reads, writes, traversals and index operations, and the transaction
/// markers that bracket them.
fn decode_durable(
    opcode: u8,
    reader: &mut Reader<'_>,
) -> Result<Option<SealedInstr>, VerifyRejection> {
    Ok(Some(match opcode {
        OP_DUR_EXISTS => SealedInstr::DurExists(operand_u16(reader)?),
        OP_DUR_FAMILY_EXISTS => SealedInstr::DurFamilyExists(operand_u16(reader)?),
        OP_DUR_READ_FIELD => SealedInstr::DurReadField(operand_u16(reader)?),
        OP_DUR_READ_FIELD_PRESENT => {
            let (site, key_slots) = operand_site_key_slots(reader)?;
            SealedInstr::DurReadFieldPresent { site, key_slots }
        }
        OP_DUR_READ_ENTRY => SealedInstr::DurReadEntry(operand_u16(reader)?),
        OP_DUR_SET_FIELD => {
            let (site, key_slots) = operand_site_key_slots(reader)?;
            SealedInstr::DurSetField { site, key_slots }
        }
        OP_DUR_READ_GROUP_PRESENT => {
            let (site, key_slots) = operand_site_key_slots(reader)?;
            SealedInstr::DurReadGroupPresent { site, key_slots }
        }
        OP_DUR_CREATE_ENTRY => SealedInstr::DurCreateEntry(operand_u16(reader)?),
        OP_DUR_REPLACE_ENTRY => SealedInstr::DurReplaceEntry(operand_u16(reader)?),
        OP_DUR_ERASE_FIELD => SealedInstr::DurEraseField(operand_u16(reader)?),
        OP_DUR_ERASE_ENTRY => SealedInstr::DurEraseEntry(operand_u16(reader)?),
        OP_DUR_READ_GROUP => SealedInstr::DurReadGroup(operand_u16(reader)?),
        OP_DUR_REPLACE_GROUP => {
            let (site, key_slots) = operand_site_key_slots(reader)?;
            SealedInstr::DurReplaceGroup { site, key_slots }
        }
        OP_DUR_ERASE_GROUP => SealedInstr::DurEraseGroup(operand_u16(reader)?),
        OP_DUR_ITERATE_BOUNDED => SealedInstr::DurIterateBounded {
            site: operand_u16(reader)?,
            limit: operand_u32(reader)?,
            from: operand_bool(reader)?,
            list_ty: operand_u16(reader)?,
        },
        OP_DUR_INDEX_SCAN => SealedInstr::DurIndexScan {
            site: operand_u16(reader)?,
            limit: operand_u32(reader)?,
            from: operand_bool(reader)?,
            list_ty: operand_u16(reader)?,
        },
        OP_DUR_INDEX_LOOKUP => SealedInstr::DurIndexLookup(operand_u16(reader)?),
        OP_DUR_INDEX_EXISTS => SealedInstr::DurIndexExists(operand_u16(reader)?),
        OP_TXN_BEGIN => SealedInstr::TxnBegin,
        OP_TXN_COMMIT => SealedInstr::TxnCommit,
        _ => return Ok(None),
    }))
}

/// The `List` and `Map` operations.
fn decode_collection(
    opcode: u8,
    reader: &mut Reader<'_>,
) -> Result<Option<SealedInstr>, VerifyRejection> {
    Ok(Some(match opcode {
        OP_LIST_NEW => SealedInstr::ListNew(operand_u16(reader)?),
        OP_LIST_APPEND => SealedInstr::ListAppend,
        OP_LIST_LEN => SealedInstr::ListLen,
        OP_LIST_GET => SealedInstr::ListGet,
        OP_LIST_INDEX => SealedInstr::ListIndex,
        OP_MAP_NEW => SealedInstr::MapNew(operand_u16(reader)?),
        OP_MAP_INSERT => SealedInstr::MapInsert,
        OP_MAP_REMOVE => SealedInstr::MapRemove,
        OP_MAP_GET => SealedInstr::MapGet,
        OP_MAP_LEN => SealedInstr::MapLen,
        OP_MAP_KEY_AT => SealedInstr::MapKeyAt,
        OP_MAP_VALUE_AT => SealedInstr::MapValueAt,
        _ => return Ok(None),
    }))
}

fn operand_u16(reader: &mut Reader) -> Result<u16, VerifyRejection> {
    reader
        .u16()
        .ok_or(reject(VerifyPhase::Function, "short u16 operand"))
}

/// The `site ‖ len ‖ slot…` operand of a present-entry op that reads its containing
/// entry's key-path from local slots. The key-path length is bounded before allocation:
/// the deepest executable key-path is one column set per node from the root down, capped
/// by the per-node column and site-path caps. The exact arity is rechecked against the
/// site's reconstructed key-path in phase 3.
fn operand_site_key_slots(reader: &mut Reader) -> Result<(u16, Vec<u16>), VerifyRejection> {
    let site = operand_u16(reader)?;
    let len = operand_u16(reader)? as usize;
    if len == 0
        || len > marrow_image::bounds::MAX_KEY_COLUMNS * marrow_image::bounds::MAX_SITE_PATH_STEPS
    {
        return Err(reject(
            VerifyPhase::Function,
            "present-entry key-path length out of range",
        ));
    }
    let mut key_slots = Vec::with_capacity(len);
    for _ in 0..len {
        key_slots.push(operand_u16(reader)?);
    }
    Ok((site, key_slots))
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

/// A `VacantLoad` operand: a full optional type reference. Its referenced indices
/// are bounds-checked by the abstract interpreter against the sealed tables.
const VACANT_LOAD: TypePosition = type_position!(
    "vacant-load operand",
    TagSet::SCALAR
        .with(TagSet::RECORD)
        .with(TagSet::ENUM)
        .with(TagSet::COLLECTION),
    Optionality::Optional
)
.in_phase(VerifyPhase::Function);

/// Rewrite jump operands from container byte offsets to tape indices, rejecting a
/// target that is not an instruction boundary in this function. Record targets
/// with a predecessor other than their immediately preceding instruction.
pub(super) fn resolve_jumps(code: &mut [Decoded]) -> Result<Vec<bool>, VerifyRejection> {
    let offsets: Vec<u32> = code.iter().map(|decoded| decoded.offset).collect();
    let index_of = |byte_offset: usize| -> Result<usize, VerifyRejection> {
        offsets
            .binary_search(&(byte_offset as u32))
            .map_err(|_| reject(VerifyPhase::Function, "jump target is not a boundary"))
    };
    let mut non_fallthrough_entries = vec![false; code.len()];
    for (index, decoded) in code.iter_mut().enumerate() {
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
                if *target != index + 1 {
                    non_fallthrough_entries[*target] = true;
                }
            }
            _ => {}
        }
    }
    Ok(non_fallthrough_entries)
}

#[cfg(test)]
mod opcode_bijection {
    //! Decode-bijection enforcement. Each opcode byte must decode to its own
    //! `SealedInstr` variant. A decode arm whose right-hand `OP_*` const is not in
    //! scope silently becomes an irrefutable binding pattern that swallows every
    //! opcode listed after it onto one variant; that class is caught here (and, at
    //! build time, by the workspace `unreachable_patterns`/`unused_variables` deny
    //! lints). The 256-byte sweep is the growth gate: a new opcode the decoder knows
    //! cannot land without an entry in [`samples`].
    use std::collections::HashMap;

    use marrow_image::{ImageType, OPTIONAL_FLAG, Scalar, TAG_INT};

    use super::*;

    /// The smallest valid byte encoding the decoder accepts for `instr`, derived from
    /// the shared width owner: its opcode, then `encoded_len() - 1` operand bytes
    /// whose zero value is in range. Three families need a nonzero minimum and spell
    /// it here — a `VacantLoad` type must carry the optional flag, and a present-set
    /// key-path must claim at least one slot.
    fn canonical_bytes(instr: &SealedInstr) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(instr.encoded_len());
        bytes.push(instr.opcode());
        match instr {
            SealedInstr::VacantLoad(_) => bytes.push(TAG_INT | OPTIONAL_FLAG),
            SealedInstr::DurSetField { key_slots, .. }
            | SealedInstr::DurReadFieldPresent { key_slots, .. }
            | SealedInstr::DurReadGroupPresent { key_slots, .. }
            | SealedInstr::DurReplaceGroup { key_slots, .. } => {
                bytes.extend_from_slice(&0u16.to_be_bytes());
                bytes.extend_from_slice(&(key_slots.len() as u16).to_be_bytes());
                bytes.resize(instr.encoded_len(), 0);
            }
            _ => bytes.resize(instr.encoded_len(), 0),
        }
        assert_eq!(
            bytes.len(),
            instr.encoded_len(),
            "the canonical encoding of {instr:?} must be exactly its shared width",
        );
        bytes
    }

    /// One value of every `SealedInstr` variant. Kept complete by
    /// [`every_decodable_opcode_has_a_sample`], which sweeps all 256 opcode bytes and
    /// demands that the ones the decoder knows are exactly the ones listed here.
    /// Reused by the sibling `index_site_partition` sweep as the canonical opcode
    /// enumeration. The four groups are the decoder's own.
    pub(super) fn samples() -> Vec<SealedInstr> {
        let mut all = operand_and_scalar_samples();
        all.extend(value_shape_samples());
        all.extend(durable_samples());
        all.extend(collection_samples());
        all
    }

    /// The control-flow, arithmetic, text, bytes, temporal and conversion opcodes.
    fn operand_and_scalar_samples() -> Vec<SealedInstr> {
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
        ]
    }

    /// The record, optional, enum, identity, diverging-marker and call opcodes.
    fn value_shape_samples() -> Vec<SealedInstr> {
        let optional_int = ImageType::Scalar {
            scalar: Scalar::Int,
            optional: true,
        };
        vec![
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
        ]
    }

    /// The durable read, write, traversal and index opcodes with their transaction markers.
    fn durable_samples() -> Vec<SealedInstr> {
        vec![
            SealedInstr::DurExists(0),
            SealedInstr::DurFamilyExists(0),
            SealedInstr::DurReadField(0),
            SealedInstr::DurReadFieldPresent {
                site: 0,
                key_slots: vec![0],
            },
            SealedInstr::DurReadEntry(0),
            SealedInstr::DurSetField {
                site: 0,
                key_slots: vec![0],
            },
            SealedInstr::DurReadGroupPresent {
                site: 0,
                key_slots: vec![0],
            },
            SealedInstr::DurCreateEntry(0),
            SealedInstr::DurReplaceEntry(0),
            SealedInstr::DurEraseField(0),
            SealedInstr::DurEraseEntry(0),
            SealedInstr::DurReadGroup(0),
            SealedInstr::DurReplaceGroup {
                site: 0,
                key_slots: vec![0],
            },
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
        ]
    }

    /// The `List` and `Map` opcodes.
    fn collection_samples() -> Vec<SealedInstr> {
        vec![
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

    /// The sample list is complete: over all 256 opcode bytes, the ones the decoder
    /// recognizes are exactly the ones a sample spells.
    #[test]
    fn every_decodable_opcode_has_a_sample() {
        let sampled: std::collections::BTreeSet<u8> =
            samples().iter().map(SealedInstr::opcode).collect();
        for byte in 0..=u8::MAX {
            // A lone opcode either decodes (no operands) or fails on its operands;
            // only an unrecognized byte fails on the opcode itself.
            let known = match decode_code(&[byte]) {
                Ok(_) => true,
                Err(rejection) => rejection.detail() != "unknown or not-yet-supported opcode",
            };
            assert_eq!(
                known,
                sampled.contains(&byte),
                "opcode {byte:#04x}: decoder knows it = {known}, a sample spells it = {}",
                sampled.contains(&byte),
            );
        }
    }

    #[test]
    fn strict_place_tags_retain_site_and_ordered_key_slots() {
        assert_eq!(
            [
                OP_DUR_SET_FIELD,
                OP_DUR_READ_GROUP_PRESENT,
                OP_DUR_REPLACE_GROUP,
                OP_DUR_READ_FIELD_PRESENT,
            ],
            [0xB9, 0xBA, 0xBB, 0xBC],
        );
        for (opcode, expected) in [
            (
                0xB9,
                SealedInstr::DurSetField {
                    site: 0x1234,
                    key_slots: vec![7, 165, 254],
                },
            ),
            (
                0xBA,
                SealedInstr::DurReadGroupPresent {
                    site: 0x1234,
                    key_slots: vec![7, 165, 254],
                },
            ),
            (
                0xBB,
                SealedInstr::DurReplaceGroup {
                    site: 0x1234,
                    key_slots: vec![7, 165, 254],
                },
            ),
            (
                0xBC,
                SealedInstr::DurReadFieldPresent {
                    site: 0x1234,
                    key_slots: vec![7, 165, 254],
                },
            ),
        ] {
            let bytes = [
                opcode, 0x12, 0x34, 0x00, 0x03, 0x00, 0x07, 0x00, 0xA5, 0x00, 0xFE, OP_RETURN,
            ];
            let decoded = decode_code(&bytes).expect("complete strict-place operands decode");
            assert_eq!(decoded.len(), 2);
            assert_eq!(decoded[0].offset, 0);
            assert_eq!(decoded[0].instr, expected);
            assert_eq!(decoded[1].offset, 11);
            assert_eq!(decoded[1].instr, SealedInstr::Return);
        }
    }

    #[test]
    fn every_truncated_strict_place_operand_is_refused() {
        for opcode in [0xB9, 0xBA, 0xBB, 0xBC] {
            let bytes = [
                opcode, 0x12, 0x34, 0x00, 0x03, 0x00, 0x07, 0x00, 0xA5, 0x00, 0xFE,
            ];
            // Empty code has no instruction to truncate. Every nonempty proper
            // prefix loses some part of the site, count or ordered slot tuple.
            for end in 1..bytes.len() {
                let rejection = decode_code(&bytes[..end])
                    .err()
                    .expect("an incomplete operand cannot produce a decoded instruction");
                assert_eq!(
                    rejection.phase(),
                    VerifyPhase::Function,
                    "opcode {opcode:#04x}, prefix length {end}",
                );
            }
        }
    }

    #[test]
    fn strict_place_key_counts_accept_the_limit_and_refuse_zero_or_excess() {
        let limit =
            marrow_image::bounds::MAX_KEY_COLUMNS * marrow_image::bounds::MAX_SITE_PATH_STEPS;
        for opcode in [0xB9, 0xBA, 0xBB, 0xBC] {
            for (count, accepted) in [(0, false), (limit, true), (limit + 1, false)] {
                let count_operand = u16::try_from(count).expect("the policy boundary fits u16");
                let mut bytes = vec![opcode, 0x12, 0x34];
                bytes.extend_from_slice(&count_operand.to_be_bytes());
                // Supply every claimed slot so an excessive count cannot pass
                // this test by being rejected only as truncated input.
                bytes.resize(5 + 2 * count, 0);
                let result = decode_code(&bytes);
                if accepted {
                    let decoded = result.expect("the key-count ceiling is inclusive");
                    assert_eq!(decoded.len(), 1);
                    let (site, key_slots) = match &decoded[0].instr {
                        SealedInstr::DurSetField { site, key_slots }
                        | SealedInstr::DurReadGroupPresent { site, key_slots }
                        | SealedInstr::DurReplaceGroup { site, key_slots }
                        | SealedInstr::DurReadFieldPresent { site, key_slots } => {
                            (*site, key_slots)
                        }
                        other => panic!("unexpected strict-place variant: {other:?}"),
                    };
                    assert_eq!(site, 0x1234);
                    assert_eq!(key_slots.as_slice(), vec![0; limit]);
                } else {
                    let rejection = result
                        .err()
                        .expect("zero and excessive key counts are invalid");
                    assert_eq!(rejection.phase(), VerifyPhase::Function);
                }
            }
        }
    }
}

#[cfg(test)]
mod index_site_partition {
    //! The index-site opcode partition in [`super::super::flow::apply_durable`] refuses a
    //! forged image aiming a managed-index read at a non-index site, so the mismatch is a
    //! typed rejection rather than a fall-through to the whole-entry `unreachable!`. It
    //! cannot derive from `operation_class`: `DurIterateBounded` is IndexRead-class yet
    //! iterates an *entry* family, so the entry-site guard owns it with a different typed
    //! detail, and an index-site opcode omitted from the guard would reach the
    //! `unreachable!` on a forged image.
    //!
    //! This sweep enumerates every opcode from the decode-bijection [`samples()`] source
    //! of truth, classifies each with the exhaustive [`role`] match (no wildcard arm, so a
    //! new opcode fails to compile until it is classified), forges each IndexRead-class
    //! opcode over a non-index field-leaf site, and asserts it rejects typed rather than
    //! panicking.

    use marrow_image::{
        CollTypeId, CollectionTypeDef, DeclarationMemberDef, DeclarationMemberShape, DraftTxn,
        ExportId, FieldDef, FunctionDef, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes,
        PlannedSiteRef, RecordTypeDef, RootOccurrenceDef, Scalar, SemanticTarget, SpanEntry,
    };

    use super::opcode_bijection::samples;
    use super::*;

    const APPLICATION_ID: [u8; 16] = [0x0a; 16];
    const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
    const PRODUCT_ID: [u8; 16] = [0x0d; 16];
    const KEY_ID: [u8; 16] = [0x0c; 16];
    const VALUE_FIELD_ID: [u8; 16] = [0x0e; 16];
    const LABEL_FIELD_ID: [u8; 16] = [0x0f; 16];

    /// The role of an opcode in the index-site partition, decided by an exhaustive
    /// match over [`SealedInstr`] with no wildcard arm. The forged draft instruction is
    /// carried in the arm, aimed at the non-index field-leaf `value_site`, so the
    /// classification and the forged image cannot drift, and a new IndexRead-class
    /// opcode is handled in exactly one place.
    enum Role {
        /// A managed-index read (`DurIndexScan`/`DurIndexLookup`/`DurIndexExists`):
        /// executable only over a managed-index site. Over a non-index site the
        /// `apply_durable` managed-index guard refuses it.
        ManagedIndexRead(Instr),
        /// An IndexRead-class op that iterates an *entry* family (`DurIterateBounded`):
        /// not an index-site op. Over a field-leaf site the entry-site guard refuses it.
        EntryFamilyTraversal(Instr),
        /// Any opcode outside the IndexRead class — not exercised by this sweep.
        Unrelated,
    }

    fn role(instr: &SealedInstr, site: &PlannedSiteRef, list_ty: CollTypeId) -> Role {
        match instr {
            SealedInstr::DurIndexScan { .. } => Role::ManagedIndexRead(Instr::DurIndexScan {
                site: site.clone(),
                limit: 1,
                from: false,
                list_ty,
            }),
            SealedInstr::DurIndexLookup(_) => {
                Role::ManagedIndexRead(Instr::DurIndexLookup(site.clone()))
            }
            SealedInstr::DurIndexExists(_) => {
                Role::ManagedIndexRead(Instr::DurIndexExists(site.clone()))
            }
            SealedInstr::DurIterateBounded { .. } => {
                Role::EntryFamilyTraversal(Instr::DurIterateBounded {
                    site: site.clone(),
                    limit: 1,
                    from: false,
                    list_ty,
                })
            }
            SealedInstr::DurExists(_)
            | SealedInstr::DurFamilyExists(_)
            | SealedInstr::DurReadField(_)
            | SealedInstr::DurReadFieldPresent { .. }
            | SealedInstr::DurReadEntry(_)
            | SealedInstr::DurReadGroup(_)
            | SealedInstr::DurReadGroupPresent { .. }
            | SealedInstr::DurSetField { .. }
            | SealedInstr::DurCreateEntry(_)
            | SealedInstr::DurReplaceEntry(_)
            | SealedInstr::DurReplaceGroup { .. }
            | SealedInstr::DurEraseField(_)
            | SealedInstr::DurEraseEntry(_)
            | SealedInstr::DurEraseGroup(_)
            | SealedInstr::TxnBegin
            | SealedInstr::TxnCommit
            | SealedInstr::ConstLoad(_)
            | SealedInstr::LocalGet(_)
            | SealedInstr::LocalSet(_)
            | SealedInstr::Pop
            | SealedInstr::Return
            | SealedInstr::Jump(_)
            | SealedInstr::JumpIfFalse(_)
            | SealedInstr::IntAdd
            | SealedInstr::IntSub
            | SealedInstr::IntMul
            | SealedInstr::IntRem
            | SealedInstr::IntDiv
            | SealedInstr::IntNeg
            | SealedInstr::BoolNot
            | SealedInstr::IntLt
            | SealedInstr::IntLe
            | SealedInstr::IntGt
            | SealedInstr::IntGe
            | SealedInstr::EqInt
            | SealedInstr::EqBool
            | SealedInstr::EqText
            | SealedInstr::TextConcat
            | SealedInstr::TextLt
            | SealedInstr::TextLe
            | SealedInstr::TextGt
            | SealedInstr::TextGe
            | SealedInstr::EqBytes
            | SealedInstr::BytesLt
            | SealedInstr::BytesLe
            | SealedInstr::BytesGt
            | SealedInstr::BytesGe
            | SealedInstr::ConvString
            | SealedInstr::ConvBytesText
            | SealedInstr::TextIsEmpty
            | SealedInstr::TextContains
            | SealedInstr::TextTrim
            | SealedInstr::TextSplit(_)
            | SealedInstr::TextLines(_)
            | SealedInstr::TextJoin
            | SealedInstr::EqDate
            | SealedInstr::DateLt
            | SealedInstr::DateLe
            | SealedInstr::DateGt
            | SealedInstr::DateGe
            | SealedInstr::EqInstant
            | SealedInstr::InstantLt
            | SealedInstr::InstantLe
            | SealedInstr::InstantGt
            | SealedInstr::InstantGe
            | SealedInstr::EqDuration
            | SealedInstr::DurationLt
            | SealedInstr::DurationLe
            | SealedInstr::DurationGt
            | SealedInstr::DurationGe
            | SealedInstr::DateAddDays
            | SealedInstr::DateDaysBetween
            | SealedInstr::DurationAdd
            | SealedInstr::DurationSub
            | SealedInstr::InstantAddDuration
            | SealedInstr::InstantSubDuration
            | SealedInstr::IntAddChecked(_)
            | SealedInstr::IntSubChecked(_)
            | SealedInstr::IntMulChecked(_)
            | SealedInstr::IntNegChecked(_)
            | SealedInstr::IntDivChecked(_)
            | SealedInstr::IntRemChecked(_)
            | SealedInstr::RangeGuard { .. }
            | SealedInstr::RecordNew(_)
            | SealedInstr::FieldGet(_)
            | SealedInstr::FieldSet(_)
            | SealedInstr::FieldUnset(_)
            | SealedInstr::SomeWrap
            | SealedInstr::VacantLoad(_)
            | SealedInstr::EnumConstruct { .. }
            | SealedInstr::EnumTag
            | SealedInstr::EnumPayloadGet { .. }
            | SealedInstr::EqEnum
            | SealedInstr::EqId
            | SealedInstr::MakeIdentity { .. }
            | SealedInstr::IdentityKeyPath(_)
            | SealedInstr::BranchPresent(_)
            | SealedInstr::Unreachable(_)
            | SealedInstr::Todo(_)
            | SealedInstr::Assert
            | SealedInstr::Call(_)
            | SealedInstr::ListNew(_)
            | SealedInstr::ListAppend
            | SealedInstr::ListLen
            | SealedInstr::ListGet
            | SealedInstr::ListIndex
            | SealedInstr::MapNew(_)
            | SealedInstr::MapInsert
            | SealedInstr::MapRemove
            | SealedInstr::MapGet
            | SealedInstr::MapLen
            | SealedInstr::MapKeyAt
            | SealedInstr::MapValueAt => Role::Unrelated,
        }
    }

    use marrow_test_support::{admitted_plan, site};

    /// A minimal single-root durable schema with a whole-entry site and a field-leaf
    /// (non-index) site, plus a `List[int]` collection type. Mirrors the tracer schema
    /// the integration hostile suite uses so a forged managed-index or bounded-traversal
    /// opcode over the field-leaf site reaches the same `apply_durable` guards. Returns
    /// the draft, the field-leaf (non-index) site operand, and the list-type index.
    /// The armed transaction over `owner`.
    fn admitted(owner: &mut marrow_image::ImageDraft) -> DraftTxn<'_> {
        owner.begin_transaction()
    }

    fn field_leaf_schema() -> (ImageDraft, PlannedSiteRef, CollTypeId) {
        let mut draft_owner = ImageDraft::new();
        let mut draft = admitted(&mut draft_owner);
        let counter = draft
            .intern_string("Counter")
            .expect("a within-domain mint");
        let value = draft.intern_string("value").expect("a within-domain mint");
        let label = draft.intern_string("label").expect("a within-domain mint");
        let record = draft
            .add_record_type(RecordTypeDef {
                name: counter,
                fields: vec![
                    FieldDef {
                        name: value,
                        ty: ImageType::scalar(Scalar::Int),
                        required: true,
                    },
                    FieldDef {
                        name: label,
                        ty: ImageType::scalar(Scalar::Text),
                        required: false,
                    },
                ],
            })
            .expect("a within-domain mint");
        let root_name = draft
            .intern_string("counters")
            .expect("a within-domain mint");
        draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
        let int_value = draft
            .value_scalar(Scalar::Int)
            .expect("the test arena mints");
        let text_value = draft
            .value_scalar(Scalar::Text)
            .expect("the test arena mints");
        draft
            .declare_product(
                &admitted_plan(),
                LedgerIdBytes::from_bytes(PRODUCT_ID),
                record,
                vec![
                    DeclarationMemberDef {
                        parent: None,
                        shape: DeclarationMemberShape::Field {
                            id: LedgerIdBytes::from_bytes(VALUE_FIELD_ID),
                            required: true,
                            value: int_value,
                        },
                    },
                    DeclarationMemberDef {
                        parent: None,
                        shape: DeclarationMemberShape::Field {
                            id: LedgerIdBytes::from_bytes(LABEL_FIELD_ID),
                            required: false,
                            value: text_value,
                        },
                    },
                ],
            )
            .expect("a well-formed declaration");
        let root = draft
            .add_root_occurrence(
                &admitted_plan(),
                LedgerIdBytes::from_bytes(PRODUCT_ID),
                RootOccurrenceDef {
                    name: root_name,
                    keys: vec![KeyColumn {
                        scalar: Scalar::Text,
                        id: LedgerIdBytes::from_bytes(KEY_ID),
                    }],
                    placement: LedgerIdBytes::from_bytes(PLACEMENT_ID),
                    indexes: Vec::new().into(),
                },
            )
            .expect("the Product is declared");
        let members = draft
            .product_members(LedgerIdBytes::from_bytes(PRODUCT_ID))
            .expect("declared");
        site(
            &mut draft,
            root.occurrence(),
            root.placement_path(),
            SemanticTarget::WholePayload,
        );
        let value_site = site(
            &mut draft,
            root.occurrence(),
            members[0].path(),
            SemanticTarget::FieldLeaf,
        );
        let list_ty = draft
            .add_collection_type(CollectionTypeDef::List {
                elem: ImageType::scalar(Scalar::Int),
            })
            .expect("a within-domain mint");
        draft.commit();
        (draft_owner, value_site, list_ty)
    }

    fn install(draft: &mut DraftTxn<'_>, forged: Instr) -> Vec<u8> {
        let src = draft
            .intern_string("src/main.mw")
            .expect("a within-domain mint");
        let name = draft.intern_string("probe").expect("a within-domain mint");
        let code = vec![forged, Instr::Return];
        let spans = (0..code.len())
            .map(|index| SpanEntry {
                instr_index: index as u32,
                line: 1,
                column: 1,
            })
            .collect();
        let func = draft
            .add_function(FunctionDef {
                name,
                source: src,
                params: Vec::new(),
                ret: ImageType::Unit,
                local_count: 0,
                spans,
                code,
            })
            .expect("every site operand is live");
        draft.add_export(ExportId::of_local("", "probe"), func);
        draft.encode().expect("encode a forged image").bytes
    }

    #[test]
    fn every_index_read_class_opcode_over_a_non_index_site_rejects_typed() {
        let mut exercised = 0usize;
        for sample in samples() {
            let (mut draft_owner, value_site, list_ty) = field_leaf_schema();
            let mut draft = admitted(&mut draft_owner);
            let (forged, expected_detail) = match role(&sample, &value_site, list_ty) {
                Role::ManagedIndexRead(forged) => {
                    (forged, "a managed-index opcode over a non-index site")
                }
                Role::EntryFamilyTraversal(forged) => (forged, "operation requires an entry site"),
                Role::Unrelated => continue,
            };
            exercised += 1;
            let bytes = install(&mut draft, forged);
            // A reachable `unreachable!` in the partition surfaces as a panic; catch it
            // so the failure names the offending opcode instead of aborting the sweep.
            let outcome = std::panic::catch_unwind(|| crate::verify(&bytes));
            match outcome {
                Err(_) => panic!(
                    "IndexRead-class opcode {sample:?} over a non-index site PANICKED (reached a \
                     `unreachable!`) instead of a typed rejection — the index-site partition in \
                     `apply_durable` is missing an arm for it",
                ),
                Ok(Ok(_)) => panic!(
                    "IndexRead-class opcode {sample:?} over a non-index site was ACCEPTED — the \
                     partition failed to reject a forged image",
                ),
                Ok(Err(rejection)) => {
                    assert_eq!(
                        rejection.code(),
                        marrow_codes::Code::ImageFunction,
                        "opcode {sample:?} rejected under the wrong phase",
                    );
                    assert_eq!(
                        rejection.detail(),
                        expected_detail,
                        "opcode {sample:?} rejected with the wrong typed detail",
                    );
                }
            }
        }
        // A liveness sanity check, not a completeness one: it guards against a wholly
        // dead classifier (everything routed to `Unrelated`). Family completeness is the
        // no-wildcard `role` match — a new opcode cannot join without being classified.
        assert!(
            exercised >= 1,
            "the sweep classified no opcode as IndexRead-class — the partition is mis-wired",
        );
    }
}
