//! Phase 2 code tables: const, function-signature, export, and span decoding.

use super::model::DecodedFunction;
use super::reject;
use super::type_ref::{Optionality, TagSet, TypeRules, decode_type_ref};
use crate::reader::Reader;
use crate::reject::{
    Bound, Duplicate, Flag, Ref, Region, RejectionKind as Kind, Tag, TypePosition, VerifyPhase,
    VerifyRejection,
};
use crate::sealed::SealedConst;
use marrow_image::{ExportId, Scalar};
use std::rc::Rc;

pub(super) fn decode_consts(
    body: &[u8],
    strings: &[Rc<str>],
) -> Result<Vec<SealedConst>, VerifyRejection> {
    let mut reader = Reader::new(body);
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Consts)))?
        as usize;
    if count > marrow_image::bounds::MAX_CONSTS {
        return Err(reject(VerifyPhase::Table, Kind::OverBound(Bound::Consts)));
    }
    let mut consts = Vec::with_capacity(count);
    let mut previous: Option<(u8, Vec<u8>)> = None;
    for _ in 0..count {
        let tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Consts)))?;
        let (value, key) = match tag {
            0x01 => {
                let raw = reader
                    .i64()
                    .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Consts)))?;
                (SealedConst::Int(raw), raw.to_be_bytes().to_vec())
            }
            0x02 => {
                let byte = reader
                    .u8()
                    .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Consts)))?;
                let value = match byte {
                    0 => false,
                    1 => true,
                    _ => return Err(reject(VerifyPhase::Table, Kind::Flag(Flag::BoolConst))),
                };
                (SealedConst::Bool(value), vec![byte])
            }
            0x03 => {
                let idx = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Consts)))?;
                if idx as usize >= strings.len() {
                    return Err(reject(VerifyPhase::Table, Kind::OutOfRange(Ref::String)));
                }
                (
                    SealedConst::Text(strings[idx as usize].clone()),
                    idx.to_be_bytes().to_vec(),
                )
            }
            0x04 => {
                let days = reader
                    .i32()
                    .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Consts)))?;
                if !marrow_temporal::supported_date_days(days) {
                    return Err(reject(VerifyPhase::Table, Kind::ConstDomain(Scalar::Date)));
                }
                (SealedConst::Date(days), days.to_be_bytes().to_vec())
            }
            0x05 => {
                let nanos = reader
                    .i128()
                    .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Consts)))?;
                if !marrow_temporal::supported_instant_nanos(nanos) {
                    return Err(reject(
                        VerifyPhase::Table,
                        Kind::ConstDomain(Scalar::Instant),
                    ));
                }
                (SealedConst::Instant(nanos), nanos.to_be_bytes().to_vec())
            }
            0x06 => {
                let nanos = reader
                    .i128()
                    .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Consts)))?;
                (SealedConst::Duration(nanos), nanos.to_be_bytes().to_vec())
            }
            _ => return Err(reject(VerifyPhase::Table, Kind::Unknown(Tag::Const))),
        };
        if let Some((ptag, pkey)) = &previous
            && (tag, &key) <= (*ptag, pkey)
        {
            return Err(reject(VerifyPhase::Table, Kind::Unsorted(Region::Consts)));
        }
        previous = Some((tag, key));
        consts.push(value);
    }
    if !reader.is_empty() {
        return Err(reject(VerifyPhase::Table, Kind::Trailing(Region::Consts)));
    }
    Ok(consts)
}

/// A function parameter: a bare scalar, record, enum, collection or entry identity.
/// An optional parameter and a unit parameter are outside the subset the compiler
/// emits.
const PARAM: TypeRules = TypeRules::new(
    TypePosition::Param,
    TagSet::VALUE.with(TagSet::IDENTITY),
    Optionality::Bare,
);

/// A function return: any value type, optional or bare, plus unit.
const RETURN: TypeRules = TypeRules::new(
    TypePosition::Return,
    TagSet::UNIT.with(TagSet::VALUE).with(TagSet::IDENTITY),
    Optionality::Either,
);

/// Both signature positions, bound to this image's tables.
fn signature_types(
    rules: TypeRules,
    type_count: usize,
    enum_count: usize,
    collection_count: usize,
    root_count: usize,
) -> TypeRules {
    rules
        .types(type_count)
        .enums(enum_count)
        .collections(collection_count)
        .roots(root_count)
}

pub(super) fn decode_functions(
    body: &[u8],
    string_count: usize,
    type_count: usize,
    enum_count: usize,
    collection_count: usize,
    root_count: usize,
) -> Result<Vec<DecodedFunction>, VerifyRejection> {
    let mut reader = Reader::new(body);
    let count = reader.u16().ok_or(reject(
        VerifyPhase::Table,
        Kind::Truncated(Region::Functions),
    ))? as usize;
    if count > marrow_image::bounds::MAX_FUNCTIONS {
        return Err(reject(
            VerifyPhase::Table,
            Kind::OverBound(Bound::Functions),
        ));
    }
    let mut functions = Vec::with_capacity(count);
    for _ in 0..count {
        let name = reader.u16().ok_or(reject(
            VerifyPhase::Table,
            Kind::Truncated(Region::Functions),
        ))?;
        let source = reader.u16().ok_or(reject(
            VerifyPhase::Table,
            Kind::Truncated(Region::Functions),
        ))?;
        if name as usize >= string_count || source as usize >= string_count {
            return Err(reject(VerifyPhase::Table, Kind::OutOfRange(Ref::String)));
        }
        let param_count = reader.u8().ok_or(reject(
            VerifyPhase::Table,
            Kind::Truncated(Region::Functions),
        ))? as usize;
        if param_count > marrow_image::bounds::MAX_PARAMS {
            return Err(reject(VerifyPhase::Table, Kind::OverBound(Bound::Params)));
        }
        let mut params = Vec::with_capacity(param_count);
        let param_rules =
            signature_types(PARAM, type_count, enum_count, collection_count, root_count);
        for _ in 0..param_count {
            params.push(decode_type_ref(&mut reader, &param_rules)?);
        }
        let ret = decode_type_ref(
            &mut reader,
            &signature_types(RETURN, type_count, enum_count, collection_count, root_count),
        )?;
        let local_count = reader.u16().ok_or(reject(
            VerifyPhase::Table,
            Kind::Truncated(Region::Functions),
        ))?;
        if local_count as usize > marrow_image::bounds::MAX_LOCALS {
            return Err(reject(VerifyPhase::Table, Kind::OverBound(Bound::Locals)));
        }
        if (local_count as usize) < param_count {
            return Err(reject(VerifyPhase::Table, Kind::LocalsBelowParams));
        }
        let code_len = reader.u32().ok_or(reject(
            VerifyPhase::Table,
            Kind::Truncated(Region::Functions),
        ))? as usize;
        if code_len > marrow_image::bounds::MAX_CODE_BYTES {
            return Err(reject(
                VerifyPhase::Table,
                Kind::OverBound(Bound::CodeBytes),
            ));
        }
        let code = reader
            .take(code_len)
            .ok_or(reject(
                VerifyPhase::Table,
                Kind::Truncated(Region::Functions),
            ))?
            .to_vec();
        functions.push(DecodedFunction {
            name,
            source,
            params,
            ret,
            local_count,
            code,
            spans: Vec::new(),
        });
    }
    if !reader.is_empty() {
        return Err(reject(
            VerifyPhase::Table,
            Kind::Trailing(Region::Functions),
        ));
    }
    Ok(functions)
}

/// Decode the EXPORTS table: `32-byte ExportId ‖ u16 func` entries in strictly
/// ascending id order. The id is reconstructed from bytes, not recomputed — the
/// compiler that minted it is untrusted, so the id is only an opaque, verified
/// dispatch key. Each function is the target of at most one export (the v0
/// one-export-per-function invariant); admitting more than one export per function,
/// or an alternate id shape, is a v1 format change that would bump the container
/// version, so it is rejected here.
pub(super) fn decode_exports(
    body: &[u8],
    function_count: usize,
) -> Result<Vec<(ExportId, u16)>, VerifyRejection> {
    let mut reader = Reader::new(body);
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Exports)))?
        as usize;
    if count > marrow_image::bounds::MAX_EXPORTS {
        return Err(reject(VerifyPhase::Table, Kind::OverBound(Bound::Exports)));
    }
    let mut exports = Vec::with_capacity(count);
    let mut seen_funcs: Vec<u16> = Vec::with_capacity(count);
    let mut previous_id: Option<[u8; 32]> = None;
    for _ in 0..count {
        let id_bytes: [u8; 32] = reader
            .take(32)
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Exports)))?
            .try_into()
            .expect("take(32) yields 32 bytes");
        let func = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Exports)))?;
        if func as usize >= function_count {
            return Err(reject(VerifyPhase::Table, Kind::OutOfRange(Ref::Function)));
        }
        if let Some(prev) = previous_id
            && id_bytes <= prev
        {
            return Err(reject(VerifyPhase::Table, Kind::Unsorted(Region::Exports)));
        }
        previous_id = Some(id_bytes);
        if seen_funcs.contains(&func) {
            return Err(reject(
                VerifyPhase::Table,
                Kind::Duplicate(Duplicate::ExportFunction),
            ));
        }
        seen_funcs.push(func);
        exports.push((ExportId::from_bytes(id_bytes), func));
    }
    if !reader.is_empty() {
        return Err(reject(VerifyPhase::Table, Kind::Trailing(Region::Exports)));
    }
    Ok(exports)
}

pub(super) fn decode_spans(
    body: &[u8],
    functions: &mut [DecodedFunction],
) -> Result<(), VerifyRejection> {
    let mut reader = Reader::new(body);
    for function in functions.iter_mut() {
        let count = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Spans)))?
            as usize;
        let mut spans = Vec::with_capacity(count);
        let mut previous_offset: Option<u32> = None;
        for _ in 0..count {
            let offset = reader
                .u32()
                .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Spans)))?;
            let line = reader
                .u32()
                .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Spans)))?;
            let column = reader
                .u32()
                .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Spans)))?;
            if line < 1 || column < 1 {
                return Err(reject(VerifyPhase::Table, Kind::SpanNotOneBased));
            }
            if let Some(prev) = previous_offset
                && offset <= prev
            {
                return Err(reject(VerifyPhase::Table, Kind::Unsorted(Region::Spans)));
            }
            previous_offset = Some(offset);
            spans.push((offset, line, column));
        }
        function.spans = spans;
    }
    if !reader.is_empty() {
        return Err(reject(VerifyPhase::Table, Kind::Trailing(Region::Spans)));
    }
    Ok(())
}
