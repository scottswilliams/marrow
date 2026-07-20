//! Phase 2 code tables: const, function-signature, export, and span decoding.

use super::model::DecodedFunction;
use super::reject;
use super::tables::decode_bare_scalar;
use crate::reader::Reader;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::{RetShape, SealedConst};
use marrow_image::{
    ExportId, ImageType, OPTIONAL_FLAG, TAG_BOOL, TAG_BYTES, TAG_COLLECTION, TAG_DATE,
    TAG_DURATION, TAG_ENUM, TAG_IDENTITY, TAG_INSTANT, TAG_INT, TAG_RECORD, TAG_TEXT, TAG_UNIT,
};
use std::rc::Rc;

pub(super) fn decode_consts(
    body: &[u8],
    strings: &[Rc<str>],
) -> Result<Vec<SealedConst>, VerifyRejection> {
    let mut reader = Reader::new(body);
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short const count"))? as usize;
    if count > marrow_image::bounds::MAX_CONSTS {
        return Err(reject(VerifyPhase::Table, "too many constants"));
    }
    let mut consts = Vec::with_capacity(count);
    let mut previous: Option<(u8, Vec<u8>)> = None;
    for _ in 0..count {
        let tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short const tag"))?;
        let (value, key) = match tag {
            0x01 => {
                let raw = reader
                    .i64()
                    .ok_or(reject(VerifyPhase::Table, "short int const"))?;
                (SealedConst::Int(raw), raw.to_be_bytes().to_vec())
            }
            0x02 => {
                let byte = reader
                    .u8()
                    .ok_or(reject(VerifyPhase::Table, "short bool const"))?;
                let value = match byte {
                    0 => false,
                    1 => true,
                    _ => return Err(reject(VerifyPhase::Table, "bool const must be 0 or 1")),
                };
                (SealedConst::Bool(value), vec![byte])
            }
            0x03 => {
                let idx = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, "short text const"))?;
                if idx as usize >= strings.len() {
                    return Err(reject(
                        VerifyPhase::Table,
                        "text const string index out of range",
                    ));
                }
                (
                    SealedConst::Text(strings[idx as usize].clone()),
                    idx.to_be_bytes().to_vec(),
                )
            }
            0x04 => {
                let days = reader
                    .i32()
                    .ok_or(reject(VerifyPhase::Table, "short date const"))?;
                if !marrow_temporal::supported_date_days(days) {
                    return Err(reject(
                        VerifyPhase::Table,
                        "date const out of supported range",
                    ));
                }
                (SealedConst::Date(days), days.to_be_bytes().to_vec())
            }
            0x05 => {
                let nanos = reader
                    .i128()
                    .ok_or(reject(VerifyPhase::Table, "short instant const"))?;
                if !marrow_temporal::supported_instant_nanos(nanos) {
                    return Err(reject(
                        VerifyPhase::Table,
                        "instant const out of supported range",
                    ));
                }
                (SealedConst::Instant(nanos), nanos.to_be_bytes().to_vec())
            }
            0x06 => {
                let nanos = reader
                    .i128()
                    .ok_or(reject(VerifyPhase::Table, "short duration const"))?;
                (SealedConst::Duration(nanos), nanos.to_be_bytes().to_vec())
            }
            _ => return Err(reject(VerifyPhase::Table, "unknown const tag")),
        };
        if let Some((ptag, pkey)) = &previous
            && (tag, &key) <= (*ptag, pkey)
        {
            return Err(reject(
                VerifyPhase::Table,
                "constants must be sorted and unique by (tag, payload)",
            ));
        }
        previous = Some((tag, key));
        consts.push(value);
    }
    if !reader.is_empty() {
        return Err(reject(VerifyPhase::Table, "trailing bytes in const table"));
    }
    Ok(consts)
}

fn decode_type_ref_ret(
    tag: u8,
    reader: &mut Reader,
    type_count: usize,
    enum_count: usize,
    collection_count: usize,
    root_count: usize,
) -> Result<RetShape, VerifyRejection> {
    let optional = tag & OPTIONAL_FLAG != 0;
    let base = tag & !OPTIONAL_FLAG;
    match base {
        TAG_UNIT => {
            if optional {
                return Err(reject(VerifyPhase::Table, "unit return cannot be optional"));
            }
            Ok(RetShape::Unit)
        }
        TAG_INT | TAG_BOOL | TAG_TEXT | TAG_BYTES | TAG_DATE | TAG_INSTANT | TAG_DURATION => {
            let scalar = decode_bare_scalar(base).expect("scalar base");
            Ok(RetShape::Scalar { scalar, optional })
        }
        TAG_RECORD => {
            let idx = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, "short record return type index"))?;
            if idx as usize >= type_count {
                return Err(reject(
                    VerifyPhase::Table,
                    "record return type index out of range",
                ));
            }
            Ok(RetShape::Record { idx, optional })
        }
        TAG_ENUM => {
            let idx = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, "short enum return type index"))?;
            if idx as usize >= enum_count {
                return Err(reject(
                    VerifyPhase::Table,
                    "enum return type index out of range",
                ));
            }
            Ok(RetShape::Enum { idx, optional })
        }
        TAG_COLLECTION => {
            let idx = reader.u16().ok_or(reject(
                VerifyPhase::Table,
                "short collection return type index",
            ))?;
            if idx as usize >= collection_count {
                return Err(reject(
                    VerifyPhase::Table,
                    "collection return type index out of range",
                ));
            }
            Ok(RetShape::Collection { idx, optional })
        }
        TAG_IDENTITY => {
            let root = reader.u16().ok_or(reject(
                VerifyPhase::Table,
                "short identity return type root index",
            ))?;
            if root as usize >= root_count {
                return Err(reject(
                    VerifyPhase::Table,
                    "identity return type root index out of range",
                ));
            }
            Ok(RetShape::Identity { root, optional })
        }
        _ => Err(reject(VerifyPhase::Table, "unknown return type tag")),
    }
}

/// Decode one parameter type reference: a bare scalar or a bare record (a dense
/// `struct` value). Optional parameters and a unit parameter are outside the
/// parameter subset the compiler emits, and are rejected.
fn decode_param_ref(
    tag: u8,
    reader: &mut Reader,
    type_count: usize,
    enum_count: usize,
    collection_count: usize,
    root_count: usize,
) -> Result<ImageType, VerifyRejection> {
    if tag & OPTIONAL_FLAG != 0 {
        return Err(reject(
            VerifyPhase::Table,
            "parameter type cannot be optional",
        ));
    }
    match tag {
        TAG_INT | TAG_BOOL | TAG_TEXT | TAG_BYTES | TAG_DATE | TAG_INSTANT | TAG_DURATION => Ok(
            ImageType::scalar(decode_bare_scalar(tag).expect("scalar base")),
        ),
        TAG_RECORD => {
            let idx = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, "short record param type index"))?;
            if idx as usize >= type_count {
                return Err(reject(
                    VerifyPhase::Table,
                    "record param type index out of range",
                ));
            }
            Ok(ImageType::Record {
                idx,
                optional: false,
            })
        }
        TAG_ENUM => {
            let idx = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, "short enum param type index"))?;
            if idx as usize >= enum_count {
                return Err(reject(
                    VerifyPhase::Table,
                    "enum param type index out of range",
                ));
            }
            Ok(ImageType::Enum {
                idx,
                optional: false,
            })
        }
        TAG_COLLECTION => {
            let idx = reader.u16().ok_or(reject(
                VerifyPhase::Table,
                "short collection param type index",
            ))?;
            if idx as usize >= collection_count {
                return Err(reject(
                    VerifyPhase::Table,
                    "collection param type index out of range",
                ));
            }
            Ok(ImageType::Collection {
                idx,
                optional: false,
            })
        }
        TAG_IDENTITY => {
            let root = reader.u16().ok_or(reject(
                VerifyPhase::Table,
                "short identity param type root index",
            ))?;
            if root as usize >= root_count {
                return Err(reject(
                    VerifyPhase::Table,
                    "identity param type root index out of range",
                ));
            }
            Ok(ImageType::Identity {
                root,
                optional: false,
            })
        }
        _ => Err(reject(
            VerifyPhase::Table,
            "param type must be a bare scalar, record, enum, or collection",
        )),
    }
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
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short function count"))? as usize;
    if count > marrow_image::bounds::MAX_FUNCTIONS {
        return Err(reject(VerifyPhase::Table, "too many functions"));
    }
    let mut functions = Vec::with_capacity(count);
    for _ in 0..count {
        let name = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short function name"))?;
        let source = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short function source"))?;
        if name as usize >= string_count || source as usize >= string_count {
            return Err(reject(
                VerifyPhase::Table,
                "function name/source index out of range",
            ));
        }
        let param_count = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short param count"))?
            as usize;
        if param_count > marrow_image::bounds::MAX_PARAMS {
            return Err(reject(VerifyPhase::Table, "too many params"));
        }
        let mut params = Vec::with_capacity(param_count);
        for _ in 0..param_count {
            let tag = reader
                .u8()
                .ok_or(reject(VerifyPhase::Table, "short param type"))?;
            params.push(decode_param_ref(
                tag,
                &mut reader,
                type_count,
                enum_count,
                collection_count,
                root_count,
            )?);
        }
        let ret_tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short return type"))?;
        let ret = decode_type_ref_ret(
            ret_tag,
            &mut reader,
            type_count,
            enum_count,
            collection_count,
            root_count,
        )?;
        let local_count = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short local count"))?;
        if local_count as usize > marrow_image::bounds::MAX_LOCALS {
            return Err(reject(VerifyPhase::Table, "too many locals"));
        }
        if (local_count as usize) < param_count {
            return Err(reject(VerifyPhase::Table, "local count below param count"));
        }
        let code_len = reader
            .u32()
            .ok_or(reject(VerifyPhase::Table, "short code length"))?
            as usize;
        if code_len > marrow_image::bounds::MAX_CODE_BYTES {
            return Err(reject(VerifyPhase::Table, "code exceeds byte bound"));
        }
        let code = reader
            .take(code_len)
            .ok_or(reject(VerifyPhase::Table, "code past input"))?
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
            "trailing bytes in function table",
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
        .ok_or(reject(VerifyPhase::Table, "short export count"))? as usize;
    if count > marrow_image::bounds::MAX_EXPORTS {
        return Err(reject(VerifyPhase::Table, "too many exports"));
    }
    let mut exports = Vec::with_capacity(count);
    let mut seen_funcs: Vec<u16> = Vec::with_capacity(count);
    let mut previous_id: Option<[u8; 32]> = None;
    for _ in 0..count {
        let id_bytes: [u8; 32] = reader
            .take(32)
            .ok_or(reject(VerifyPhase::Table, "short export id"))?
            .try_into()
            .expect("take(32) yields 32 bytes");
        let func = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short export function"))?;
        if func as usize >= function_count {
            return Err(reject(
                VerifyPhase::Table,
                "export function index out of range",
            ));
        }
        if let Some(prev) = previous_id
            && id_bytes <= prev
        {
            return Err(reject(
                VerifyPhase::Table,
                "exports must be sorted and unique by id",
            ));
        }
        previous_id = Some(id_bytes);
        if seen_funcs.contains(&func) {
            return Err(reject(
                VerifyPhase::Table,
                "duplicate export function index",
            ));
        }
        seen_funcs.push(func);
        exports.push((ExportId::from_bytes(id_bytes), func));
    }
    if !reader.is_empty() {
        return Err(reject(VerifyPhase::Table, "trailing bytes in export table"));
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
            .ok_or(reject(VerifyPhase::Table, "short span count"))? as usize;
        let mut spans = Vec::with_capacity(count);
        let mut previous_offset: Option<u32> = None;
        for _ in 0..count {
            let offset = reader
                .u32()
                .ok_or(reject(VerifyPhase::Table, "short span offset"))?;
            let line = reader
                .u32()
                .ok_or(reject(VerifyPhase::Table, "short span line"))?;
            let column = reader
                .u32()
                .ok_or(reject(VerifyPhase::Table, "short span column"))?;
            if line < 1 || column < 1 {
                return Err(reject(
                    VerifyPhase::Table,
                    "span line/column must be 1-based",
                ));
            }
            if let Some(prev) = previous_offset
                && offset <= prev
            {
                return Err(reject(
                    VerifyPhase::Table,
                    "span offsets must strictly ascend",
                ));
            }
            previous_offset = Some(offset);
            spans.push((offset, line, column));
        }
        function.spans = spans;
    }
    if !reader.is_empty() {
        return Err(reject(VerifyPhase::Table, "trailing bytes in span table"));
    }
    Ok(())
}
