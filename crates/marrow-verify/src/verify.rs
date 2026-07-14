//! The phased image verifier (design §E).
//!
//! Phases run in order; each consumes only prior output; every failure is a typed
//! [`VerifyRejection`], never a panic. The compiler emits image bytes but cannot
//! mint a [`VerifiedImage`]: this is the only path from bytes to a checked image,
//! and the sole `VerifiedImage` constructor.
//!
//! Coverage grows one slice at a time. The container framing and every table are
//! decoded in full; the per-function instruction set the interpreter admits is the
//! current subset, and an opcode whose vertical has not landed is a phase-3
//! rejection rather than a silent pass.

use std::rc::Rc;

use marrow_image::{
    ImageId, OP_BOOL_NOT, OP_BRANCH_PRESENT, OP_CONST_LOAD, OP_EQ_BOOL, OP_EQ_INT, OP_EQ_TEXT,
    OP_FIELD_GET, OP_INT_ADD, OP_INT_GE, OP_INT_GT, OP_INT_LE, OP_INT_LT, OP_INT_MUL, OP_INT_NEG,
    OP_INT_REM, OP_INT_SUB, OP_JUMP, OP_JUMP_IF_FALSE, OP_LOCAL_GET, OP_LOCAL_SET, OP_POP,
    OP_RECORD_NEW, OP_RETURN, OP_SOME_WRAP, OP_TEXT_CONCAT, OP_VACANT_LOAD, OPTIONAL_FLAG, Scalar,
    TAG_BOOL, TAG_INT, TAG_RECORD, TAG_TEXT, TAG_UNIT, image_id,
};

use crate::reader::Reader;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::{
    RetShape, SealedConst, SealedExport, SealedField, SealedFunction, SealedInstr,
    SealedRecordType, SpanRow, VerifiedImage,
};
use crate::vtype::VType;

const MAGIC: &[u8; 4] = b"MWI\0";
const VERSION: u8 = 0x00;
const DIGEST_SLOT_END: usize = 37;

type Reject = VerifyRejection;

fn reject(phase: VerifyPhase, detail: &'static str) -> Reject {
    VerifyRejection::new(phase, detail)
}

/// Verify `bytes` into a sealed [`VerifiedImage`], or reject at the earliest phase
/// whose invariant the image violates.
pub fn verify(bytes: &[u8]) -> Result<VerifiedImage, VerifyRejection> {
    let decoded = decode_container(bytes)?;
    seal(decoded)
}

// ---------------------------------------------------------------------------
// Phase 1 (envelope) + phase 2 (table closure).
// ---------------------------------------------------------------------------

// The record-type table is decoded and grammar-checked in full now; its fields are
// consumed by the record slice (RecordNew/FieldGet). Until then they are validated
// but unread.
#[allow(dead_code)]
struct DecodedRecordType {
    name: u16,
    fields: Vec<DecodedField>,
}

#[allow(dead_code)]
struct DecodedField {
    name: u16,
    ty: Scalar,
    required: bool,
}

struct DecodedFunction {
    name: u16,
    source: u16,
    params: Vec<Scalar>,
    ret: RetShape,
    local_count: u16,
    code: Vec<u8>,
    spans: Vec<(u32, u32, u32)>,
}

struct DecodedImage {
    image_id: ImageId,
    strings: Vec<Rc<str>>,
    #[allow(dead_code)]
    types: Vec<DecodedRecordType>,
    consts: Vec<SealedConst>,
    functions: Vec<DecodedFunction>,
    exports: Vec<(u16, u16)>,
}

fn decode_container(bytes: &[u8]) -> Result<DecodedImage, VerifyRejection> {
    if bytes.len() > marrow_image::bounds::MAX_IMAGE_BYTES {
        return Err(reject(
            VerifyPhase::Envelope,
            "image exceeds the size bound",
        ));
    }
    let mut reader = Reader::new(bytes);
    let magic = reader
        .take(4)
        .ok_or(reject(VerifyPhase::Envelope, "short magic"))?;
    if magic != MAGIC {
        return Err(reject(VerifyPhase::Envelope, "bad magic"));
    }
    let version = reader
        .u8()
        .ok_or(reject(VerifyPhase::Envelope, "short version"))?;
    if version != VERSION {
        return Err(reject(VerifyPhase::Envelope, "unsupported version"));
    }
    let stored_digest = reader
        .take(32)
        .ok_or(reject(VerifyPhase::Envelope, "short digest slot"))?;
    // Recompute the digest over the payload (every byte after the digest slot).
    let payload = &bytes[DIGEST_SLOT_END..];
    if image_id(payload).0.as_slice() != stored_digest {
        return Err(reject(VerifyPhase::Envelope, "digest mismatch"));
    }

    let section_count = reader
        .u8()
        .ok_or(reject(VerifyPhase::Envelope, "short section count"))?;
    if section_count != 7 {
        return Err(reject(VerifyPhase::Envelope, "section count must be 7"));
    }
    let mut sections: Vec<(u8, &[u8])> = Vec::with_capacity(7);
    let mut last_id = 0u8;
    for _ in 0..7 {
        let id = reader
            .u8()
            .ok_or(reject(VerifyPhase::Envelope, "short section id"))?;
        if id <= last_id {
            return Err(reject(
                VerifyPhase::Envelope,
                "section ids must strictly ascend",
            ));
        }
        last_id = id;
        let len = reader
            .u32()
            .ok_or(reject(VerifyPhase::Envelope, "short section length"))?
            as usize;
        let body = reader
            .take(len)
            .ok_or(reject(VerifyPhase::Envelope, "section length past input"))?;
        sections.push((id, body));
    }
    if !reader.is_empty() {
        return Err(reject(
            VerifyPhase::Envelope,
            "trailing bytes after sections",
        ));
    }
    // Section ids strictly ascend and there are exactly 7, so they are exactly 1..7.
    for (index, (id, _)) in sections.iter().enumerate() {
        if *id != (index as u8 + 1) {
            return Err(reject(
                VerifyPhase::Envelope,
                "section ids must be exactly 1..7",
            ));
        }
    }

    // Phase 2: decode each table. Spans are decoded per function, in FUNCTIONS
    // order, so they are attached to the already-decoded function list.
    let strings = decode_strings(sections[0].1)?;
    let types = decode_types(sections[1].1, strings.len())?;
    decode_durable_empty(sections[2].1)?;
    let consts = decode_consts(sections[3].1, &strings)?;
    let mut functions = decode_functions(sections[4].1, strings.len())?;
    let exports = decode_exports(sections[5].1, strings.len(), functions.len())?;
    decode_spans(sections[6].1, &mut functions)?;

    Ok(DecodedImage {
        image_id: image_id(payload),
        strings,
        types,
        consts,
        functions,
        exports,
    })
}

fn decode_strings(body: &[u8]) -> Result<Vec<Rc<str>>, VerifyRejection> {
    let mut reader = Reader::new(body);
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short string count"))? as usize;
    if count > marrow_image::bounds::MAX_STRINGS {
        return Err(reject(VerifyPhase::Table, "too many strings"));
    }
    let mut strings: Vec<Rc<str>> = Vec::with_capacity(count);
    let mut previous: Option<Vec<u8>> = None;
    for _ in 0..count {
        let len = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short string length"))? as usize;
        if len > marrow_image::bounds::MAX_STRING_BYTES {
            return Err(reject(VerifyPhase::Table, "string exceeds byte bound"));
        }
        let raw = reader
            .take(len)
            .ok_or(reject(VerifyPhase::Table, "string past input"))?;
        if let Some(prev) = &previous
            && raw <= prev.as_slice()
        {
            return Err(reject(
                VerifyPhase::Table,
                "strings must be byte-sorted and unique",
            ));
        }
        previous = Some(raw.to_vec());
        let text = std::str::from_utf8(raw)
            .map_err(|_| reject(VerifyPhase::Table, "string is not valid UTF-8"))?;
        strings.push(Rc::from(text));
    }
    if !reader.is_empty() {
        return Err(reject(VerifyPhase::Table, "trailing bytes in string table"));
    }
    Ok(strings)
}

fn decode_bare_scalar(tag: u8) -> Option<Scalar> {
    match tag {
        TAG_INT => Some(Scalar::Int),
        TAG_BOOL => Some(Scalar::Bool),
        TAG_TEXT => Some(Scalar::Text),
        _ => None,
    }
}

fn decode_types(
    body: &[u8],
    string_count: usize,
) -> Result<Vec<DecodedRecordType>, VerifyRejection> {
    let mut reader = Reader::new(body);
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short type count"))? as usize;
    if count > marrow_image::bounds::MAX_TYPES {
        return Err(reject(VerifyPhase::Table, "too many record types"));
    }
    let mut types = Vec::with_capacity(count);
    for _ in 0..count {
        let name = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short type name"))?;
        if name as usize >= string_count {
            return Err(reject(VerifyPhase::Table, "type name index out of range"));
        }
        let field_count = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short field count"))?
            as usize;
        if field_count > marrow_image::bounds::MAX_FIELDS {
            return Err(reject(VerifyPhase::Table, "too many fields"));
        }
        let mut fields = Vec::with_capacity(field_count);
        let mut seen_names: Vec<u16> = Vec::with_capacity(field_count);
        for _ in 0..field_count {
            let fname = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, "short field name"))?;
            if fname as usize >= string_count {
                return Err(reject(VerifyPhase::Table, "field name index out of range"));
            }
            if seen_names.contains(&fname) {
                return Err(reject(VerifyPhase::Table, "duplicate field name in record"));
            }
            seen_names.push(fname);
            let tag = reader
                .u8()
                .ok_or(reject(VerifyPhase::Table, "short field type"))?;
            let ty = decode_bare_scalar(tag).ok_or(reject(
                VerifyPhase::Table,
                "field type must be a bare scalar",
            ))?;
            let required_byte = reader
                .u8()
                .ok_or(reject(VerifyPhase::Table, "short field required flag"))?;
            let required = match required_byte {
                0 => false,
                1 => true,
                _ => {
                    return Err(reject(
                        VerifyPhase::Table,
                        "field required flag must be 0 or 1",
                    ));
                }
            };
            fields.push(DecodedField {
                name: fname,
                ty,
                required,
            });
        }
        types.push(DecodedRecordType { name, fields });
    }
    if !reader.is_empty() {
        return Err(reject(VerifyPhase::Table, "trailing bytes in type table"));
    }
    Ok(types)
}

/// The T01 subset compiles no durable roots or sites yet; the DURABLE table must
/// therefore decode to zero roots and zero sites. A populated table is a
/// not-yet-supported artifact and rejects until the durable slice lands.
fn decode_durable_empty(body: &[u8]) -> Result<(), VerifyRejection> {
    let mut reader = Reader::new(body);
    let roots = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short root count"))?;
    if roots != 0 {
        return Err(reject(
            VerifyPhase::Table,
            "durable roots are not yet supported",
        ));
    }
    let sites = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short site count"))?;
    if sites != 0 {
        return Err(reject(
            VerifyPhase::Table,
            "durable sites are not yet supported",
        ));
    }
    if !reader.is_empty() {
        return Err(reject(
            VerifyPhase::Table,
            "trailing bytes in durable table",
        ));
    }
    Ok(())
}

fn decode_consts(body: &[u8], strings: &[Rc<str>]) -> Result<Vec<SealedConst>, VerifyRejection> {
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

fn decode_type_ref_ret(tag: u8, reader: &mut Reader) -> Result<RetShape, VerifyRejection> {
    let optional = tag & OPTIONAL_FLAG != 0;
    let base = tag & !OPTIONAL_FLAG;
    match base {
        TAG_UNIT => {
            if optional {
                return Err(reject(VerifyPhase::Table, "unit return cannot be optional"));
            }
            Ok(RetShape::Unit)
        }
        TAG_INT | TAG_BOOL | TAG_TEXT => {
            let scalar = decode_bare_scalar(base).expect("scalar base");
            Ok(RetShape::Scalar { scalar, optional })
        }
        TAG_RECORD => {
            let _ = reader;
            Err(reject(
                VerifyPhase::Table,
                "record return type is not admitted",
            ))
        }
        _ => Err(reject(VerifyPhase::Table, "unknown return type tag")),
    }
}

fn decode_functions(
    body: &[u8],
    string_count: usize,
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
            let scalar = decode_bare_scalar(tag).ok_or(reject(
                VerifyPhase::Table,
                "param type must be a bare scalar",
            ))?;
            params.push(scalar);
        }
        let ret_tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short return type"))?;
        let ret = decode_type_ref_ret(ret_tag, &mut reader)?;
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

fn decode_exports(
    body: &[u8],
    string_count: usize,
    function_count: usize,
) -> Result<Vec<(u16, u16)>, VerifyRejection> {
    let mut reader = Reader::new(body);
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short export count"))? as usize;
    if count > marrow_image::bounds::MAX_EXPORTS {
        return Err(reject(VerifyPhase::Table, "too many exports"));
    }
    let mut exports = Vec::with_capacity(count);
    let mut seen_funcs: Vec<u16> = Vec::with_capacity(count);
    let mut previous_name: Option<u16> = None;
    for _ in 0..count {
        let name = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short export name"))?;
        let func = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short export function"))?;
        if name as usize >= string_count {
            return Err(reject(VerifyPhase::Table, "export name index out of range"));
        }
        if func as usize >= function_count {
            return Err(reject(
                VerifyPhase::Table,
                "export function index out of range",
            ));
        }
        if let Some(prev) = previous_name
            && name <= prev
        {
            return Err(reject(
                VerifyPhase::Table,
                "exports must be sorted and unique by name",
            ));
        }
        previous_name = Some(name);
        if seen_funcs.contains(&func) {
            return Err(reject(
                VerifyPhase::Table,
                "duplicate export function index",
            ));
        }
        seen_funcs.push(func);
        exports.push((name, func));
    }
    if !reader.is_empty() {
        return Err(reject(VerifyPhase::Table, "trailing bytes in export table"));
    }
    Ok(exports)
}

fn decode_spans(body: &[u8], functions: &mut [DecodedFunction]) -> Result<(), VerifyRejection> {
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

// ---------------------------------------------------------------------------
// Phase 3 (per-function structural/type/local-init) + phases 4-6.
// ---------------------------------------------------------------------------

/// A decoded instruction with resolved operands and its byte offset. Jump targets
/// are resolved from byte offsets to tape indices by [`resolve_jumps`] before flow
/// analysis, so a jump can only name an instruction boundary in its own function.
struct Decoded {
    instr: SealedInstr,
    /// Byte offset of this instruction in the function code (for span mapping).
    offset: u32,
}

fn seal(decoded: DecodedImage) -> Result<VerifiedImage, VerifyRejection> {
    let types: Vec<SealedRecordType> = decoded
        .types
        .iter()
        .map(|record| SealedRecordType {
            fields: record
                .fields
                .iter()
                .map(|field| SealedField {
                    scalar: field.ty,
                    required: field.required,
                })
                .collect(),
        })
        .collect();
    let mut functions = Vec::with_capacity(decoded.functions.len());
    for function in &decoded.functions {
        functions.push(verify_function(function, &types, &decoded)?);
    }
    // Phase 4/5 at the T01 storeless subset: no calls and no durable effects yet,
    // so every export is read-only. Call-cycle and transaction-flow closure land
    // with the call and durable slices.
    let exports = decoded
        .exports
        .iter()
        .map(|(name, func)| SealedExport {
            name: decoded.strings[*name as usize].clone(),
            func: *func,
            mutating: false,
        })
        .collect();
    Ok(VerifiedImage {
        image_id: decoded.image_id,
        types,
        consts: decoded.consts,
        functions,
        exports,
    })
}

fn verify_function(
    function: &DecodedFunction,
    types: &[SealedRecordType],
    decoded: &DecodedImage,
) -> Result<SealedFunction, VerifyRejection> {
    let mut decoded_code = decode_code(&function.code)?;
    resolve_jumps(&mut decoded_code)?;
    let (instrs, max_stack) = check_flow(function, types, &decoded_code, &decoded.consts)?;
    let spans = map_spans(function, &decoded_code)?;
    Ok(SealedFunction {
        name: decoded.strings[function.name as usize].clone(),
        source: decoded.strings[function.source as usize].clone(),
        params: function.params.clone(),
        ret: function.ret,
        local_count: function.local_count,
        instrs,
        spans,
        max_stack,
        mutating: false,
    })
}

/// Decode the function bytecode into instructions on boundaries. Jump operands are
/// container byte offsets here; [`resolve_jumps`] rewrites them to tape indices.
fn decode_code(code: &[u8]) -> Result<Vec<Decoded>, VerifyRejection> {
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
            OP_RECORD_NEW => SealedInstr::RecordNew(operand_u16(&mut reader)?),
            OP_FIELD_GET => SealedInstr::FieldGet(operand_u16(&mut reader)?),
            OP_SOME_WRAP => SealedInstr::SomeWrap,
            OP_VACANT_LOAD => SealedInstr::VacantLoad(decode_optional_scalar_operand(&mut reader)?),
            OP_BRANCH_PRESENT => SealedInstr::BranchPresent(operand_u32(&mut reader)? as usize),
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

/// Decode a `VacantLoad` operand: a single type-ref tag that must be an optional
/// scalar (design §C, §D `VacantLoad`).
fn decode_optional_scalar_operand(reader: &mut Reader) -> Result<Scalar, VerifyRejection> {
    let tag = reader
        .u8()
        .ok_or(reject(VerifyPhase::Function, "short vacant-load operand"))?;
    if tag & OPTIONAL_FLAG == 0 {
        return Err(reject(
            VerifyPhase::Function,
            "vacant-load operand must be optional",
        ));
    }
    decode_bare_scalar(tag & !OPTIONAL_FLAG).ok_or(reject(
        VerifyPhase::Function,
        "vacant-load operand must be an optional scalar",
    ))
}

/// Rewrite jump operands from container byte offsets to tape indices, rejecting a
/// target that is not an instruction boundary in this function.
fn resolve_jumps(code: &mut [Decoded]) -> Result<(), VerifyRejection> {
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
            | SealedInstr::BranchPresent(target) => {
                *target = index_of(*target)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// The control transfer an instruction performs, in tape indices. Successor
/// indices are derived from this by `check_flow`.
enum Control {
    /// Continue to the next instruction.
    Fallthrough,
    /// End the frame (no successor).
    Return,
    /// Unconditional transfer to one tape index.
    Jump(usize),
    /// Conditional: the branch target plus fallthrough (identical stack on both).
    Branch(usize),
    /// Optional-presence branch: the absent edge (`target`) carries the current
    /// stack; the present edge (fallthrough) additionally carries the unwrapped
    /// bare value.
    BranchPresent { target: usize, present: VType },
}

/// The abstract machine state at a program point: the typed operand stack and the
/// definite-init/type state of each local slot.
#[derive(Clone, PartialEq, Eq)]
struct Frame {
    stack: Vec<VType>,
    /// Per-slot type when definitely initialized on every path reaching this point,
    /// else `None`. Reading an uninitialized slot rejects.
    locals: Vec<Option<VType>>,
}

/// Phase-3 structural, type, and local-init checks via a CFG worklist over the
/// typed operand stack and locals. Returns the sealed instruction tape and the true
/// max stack depth (computed here, never read from the image).
fn check_flow(
    function: &DecodedFunction,
    types: &[SealedRecordType],
    code: &[Decoded],
    consts: &[SealedConst],
) -> Result<(Vec<SealedInstr>, usize), VerifyRejection> {
    if code.is_empty() {
        return Err(reject(VerifyPhase::Function, "function has no code"));
    }

    // Params occupy locals `0..param_count`, pre-initialized to their param type;
    // the rest start uninitialized. The entry operand stack is empty.
    let mut initial_locals: Vec<Option<VType>> = vec![None; function.local_count as usize];
    for (slot, scalar) in function.params.iter().enumerate() {
        initial_locals[slot] = Some(VType::bare_scalar(*scalar));
    }
    let mut entry: Vec<Option<Frame>> = vec![None; code.len()];
    entry[0] = Some(Frame {
        stack: Vec::new(),
        locals: initial_locals,
    });
    let mut max_stack = 0usize;
    let mut worklist = vec![0usize];

    while let Some(index) = worklist.pop() {
        let mut frame = entry[index]
            .clone()
            .expect("worklist only enqueues reached instructions");
        let control = apply(function, types, &code[index].instr, consts, &mut frame)?;
        if frame.stack.len() > marrow_image::bounds::MAX_STACK_DEPTH {
            return Err(reject(
                VerifyPhase::Function,
                "operand stack exceeds depth bound",
            ));
        }
        max_stack = max_stack.max(frame.stack.len());
        // Each successor edge carries a frame; `BranchPresent` differs between edges.
        let edges: Vec<(usize, Frame)> = match control {
            Control::Return => Vec::new(),
            Control::Fallthrough => vec![(index + 1, frame.clone())],
            Control::Jump(target) => vec![(target, frame.clone())],
            Control::Branch(target) => vec![(target, frame.clone()), (index + 1, frame.clone())],
            Control::BranchPresent { target, present } => {
                let mut present_frame = frame.clone();
                present_frame.stack.push(present);
                if present_frame.stack.len() > marrow_image::bounds::MAX_STACK_DEPTH {
                    return Err(reject(
                        VerifyPhase::Function,
                        "operand stack exceeds depth bound",
                    ));
                }
                max_stack = max_stack.max(present_frame.stack.len());
                vec![(target, frame.clone()), (index + 1, present_frame)]
            }
        };
        for (successor, edge_frame) in edges {
            if successor >= code.len() {
                return Err(reject(
                    VerifyPhase::Function,
                    "execution falls off the end without returning",
                ));
            }
            propagate(&mut entry, &mut worklist, successor, &edge_frame)?;
        }
    }

    if entry.iter().any(Option::is_none) {
        return Err(reject(VerifyPhase::Function, "unreachable instruction"));
    }

    let instrs = code.iter().map(|decoded| decoded.instr.clone()).collect();
    Ok((instrs, max_stack))
}

/// Merge `frame` into the entry state of `successor`, enqueueing it when its state
/// changes. Stacks must agree exactly; locals meet per slot (init on both paths
/// with the same type stays init, otherwise the slot becomes uninit).
fn propagate(
    entry: &mut [Option<Frame>],
    worklist: &mut Vec<usize>,
    successor: usize,
    frame: &Frame,
) -> Result<(), VerifyRejection> {
    match &entry[successor] {
        None => {
            entry[successor] = Some(frame.clone());
            worklist.push(successor);
            Ok(())
        }
        Some(existing) => {
            if existing.stack != frame.stack {
                return Err(reject(
                    VerifyPhase::Function,
                    "operand stack shapes disagree at a merge",
                ));
            }
            let mut merged = existing.locals.clone();
            for (slot, cell) in merged.iter_mut().enumerate() {
                let incoming = frame.locals[slot];
                *cell = match (*cell, incoming) {
                    (Some(a), Some(b)) if a == b => Some(a),
                    _ => None,
                };
            }
            if merged != existing.locals {
                entry[successor] = Some(Frame {
                    stack: existing.stack.clone(),
                    locals: merged,
                });
                worklist.push(successor);
            }
            Ok(())
        }
    }
}

/// Apply one instruction to the abstract frame and return its control transfer.
/// Grows one slice at a time with the opcode set.
fn apply(
    function: &DecodedFunction,
    types: &[SealedRecordType],
    instr: &SealedInstr,
    consts: &[SealedConst],
    frame: &mut Frame,
) -> Result<Control, VerifyRejection> {
    // Record/optional opcodes need the whole frame; the scalar opcodes work on the
    // stack alone, borrowed here after the frame-level ones return.
    match instr {
        SealedInstr::RecordNew(ty) => {
            let record = types.get(*ty as usize).ok_or(reject(
                VerifyPhase::Function,
                "record type index out of range",
            ))?;
            // f0 is pushed first, so pop fields in reverse declaration order.
            for field in record.fields.iter().rev() {
                let want = if field.required {
                    VType::bare_scalar(field.scalar)
                } else {
                    VType::bare_scalar(field.scalar).to_optional()
                };
                let got = pop(&mut frame.stack)?;
                if got != want {
                    return Err(reject(
                        VerifyPhase::Function,
                        "record field operand type mismatch",
                    ));
                }
            }
            frame.stack.push(VType::bare_record(*ty));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::FieldGet(field) => {
            let record = pop(&mut frame.stack)?;
            let VType::Record {
                idx,
                optional: false,
            } = record
            else {
                return Err(reject(
                    VerifyPhase::Function,
                    "field read requires a bare record",
                ));
            };
            let record_type = types.get(idx as usize).ok_or(reject(
                VerifyPhase::Function,
                "record type index out of range",
            ))?;
            let field_def = record_type
                .fields
                .get(*field as usize)
                .ok_or(reject(VerifyPhase::Function, "field index out of range"))?;
            let result = if field_def.required {
                VType::bare_scalar(field_def.scalar)
            } else {
                VType::bare_scalar(field_def.scalar).to_optional()
            };
            frame.stack.push(result);
            return Ok(Control::Fallthrough);
        }
        SealedInstr::SomeWrap => {
            let value = pop(&mut frame.stack)?;
            if value.is_optional() {
                return Err(reject(
                    VerifyPhase::Function,
                    "some-wrap operand is already optional",
                ));
            }
            frame.stack.push(value.to_optional());
            return Ok(Control::Fallthrough);
        }
        SealedInstr::VacantLoad(scalar) => {
            frame.stack.push(VType::bare_scalar(*scalar).to_optional());
            return Ok(Control::Fallthrough);
        }
        SealedInstr::BranchPresent(target) => {
            let value = pop(&mut frame.stack)?;
            if !value.is_optional() {
                return Err(reject(
                    VerifyPhase::Function,
                    "branch-present requires an optional",
                ));
            }
            return Ok(Control::BranchPresent {
                target: *target,
                present: value.to_bare(),
            });
        }
        _ => {}
    }

    let stack = &mut frame.stack;
    match instr {
        SealedInstr::ConstLoad(idx) => {
            let value = consts
                .get(*idx as usize)
                .ok_or(reject(VerifyPhase::Function, "const index out of range"))?;
            stack.push(VType::bare_scalar(const_scalar(value)));
            Ok(Control::Fallthrough)
        }
        SealedInstr::LocalGet(slot) => {
            let ty = frame
                .locals
                .get(*slot as usize)
                .ok_or(reject(VerifyPhase::Function, "local index out of range"))?
                .ok_or(reject(VerifyPhase::Function, "local read before init"))?;
            stack.push(ty);
            Ok(Control::Fallthrough)
        }
        SealedInstr::LocalSet(slot) => {
            let value = pop(stack)?;
            let cell = frame
                .locals
                .get_mut(*slot as usize)
                .ok_or(reject(VerifyPhase::Function, "local index out of range"))?;
            match cell {
                Some(existing) if *existing != value => {
                    return Err(reject(
                        VerifyPhase::Function,
                        "local slot reused at a different type",
                    ));
                }
                _ => *cell = Some(value),
            }
            Ok(Control::Fallthrough)
        }
        SealedInstr::Pop => {
            pop(stack)?;
            Ok(Control::Fallthrough)
        }
        SealedInstr::Return => {
            match (stack.pop(), function.ret) {
                (Some(top), ret) if top.matches_ret(ret) => {}
                (None, RetShape::Unit) => {}
                _ => {
                    return Err(reject(
                        VerifyPhase::Function,
                        "return stack shape does not match the return type",
                    ));
                }
            }
            if !stack.is_empty() {
                return Err(reject(
                    VerifyPhase::Function,
                    "operand stack not empty at return",
                ));
            }
            Ok(Control::Return)
        }
        SealedInstr::Jump(target) => Ok(Control::Jump(*target)),
        SealedInstr::JumpIfFalse(target) => {
            expect_scalar(pop(stack)?, Scalar::Bool)?;
            Ok(Control::Branch(*target))
        }
        SealedInstr::IntAdd | SealedInstr::IntSub | SealedInstr::IntMul | SealedInstr::IntRem => {
            binary(stack, Scalar::Int, Scalar::Int)?;
            Ok(Control::Fallthrough)
        }
        SealedInstr::IntNeg => {
            expect_scalar(pop(stack)?, Scalar::Int)?;
            stack.push(VType::bare_scalar(Scalar::Int));
            Ok(Control::Fallthrough)
        }
        SealedInstr::BoolNot => {
            expect_scalar(pop(stack)?, Scalar::Bool)?;
            stack.push(VType::bare_scalar(Scalar::Bool));
            Ok(Control::Fallthrough)
        }
        SealedInstr::IntLt | SealedInstr::IntLe | SealedInstr::IntGt | SealedInstr::IntGe => {
            binary(stack, Scalar::Int, Scalar::Bool)?;
            Ok(Control::Fallthrough)
        }
        SealedInstr::EqInt => {
            binary(stack, Scalar::Int, Scalar::Bool)?;
            Ok(Control::Fallthrough)
        }
        SealedInstr::EqBool => {
            binary(stack, Scalar::Bool, Scalar::Bool)?;
            Ok(Control::Fallthrough)
        }
        SealedInstr::EqText => {
            binary(stack, Scalar::Text, Scalar::Bool)?;
            Ok(Control::Fallthrough)
        }
        SealedInstr::TextConcat => {
            binary(stack, Scalar::Text, Scalar::Text)?;
            Ok(Control::Fallthrough)
        }
        SealedInstr::RecordNew(_)
        | SealedInstr::FieldGet(_)
        | SealedInstr::SomeWrap
        | SealedInstr::VacantLoad(_)
        | SealedInstr::BranchPresent(_) => {
            unreachable!("record and optional opcodes return from the frame-level match")
        }
    }
}

/// Pop the top operand, rejecting an empty stack (a verifier-internal shape error).
fn pop(stack: &mut Vec<VType>) -> Result<VType, VerifyRejection> {
    stack
        .pop()
        .ok_or(reject(VerifyPhase::Function, "operand stack underflow"))
}

/// Require `value` to be a bare scalar of `scalar`.
fn expect_scalar(value: VType, scalar: Scalar) -> Result<(), VerifyRejection> {
    if value == VType::bare_scalar(scalar) {
        Ok(())
    } else {
        Err(reject(
            VerifyPhase::Function,
            "operand type mismatch for opcode",
        ))
    }
}

/// Pop two bare `operand`-typed scalars (right then left) and push a bare `result`.
fn binary(stack: &mut Vec<VType>, operand: Scalar, result: Scalar) -> Result<(), VerifyRejection> {
    let right = pop(stack)?;
    let left = pop(stack)?;
    expect_scalar(right, operand)?;
    expect_scalar(left, operand)?;
    stack.push(VType::bare_scalar(result));
    Ok(())
}

fn const_scalar(value: &SealedConst) -> Scalar {
    match value {
        SealedConst::Int(_) => Scalar::Int,
        SealedConst::Bool(_) => Scalar::Bool,
        SealedConst::Text(_) => Scalar::Text,
    }
}

fn map_spans(
    function: &DecodedFunction,
    code: &[Decoded],
) -> Result<Vec<SpanRow>, VerifyRejection> {
    if !function.spans.is_empty() {
        if function.spans[0].0 != 0 {
            return Err(reject(
                VerifyPhase::Function,
                "first span must map instruction offset 0",
            ));
        }
    } else if !code.is_empty() {
        return Err(reject(VerifyPhase::Function, "code has no span mappings"));
    }
    let mut rows = Vec::with_capacity(function.spans.len());
    for (offset, line, column) in &function.spans {
        let instr_index = code.iter().position(|d| d.offset == *offset).ok_or(reject(
            VerifyPhase::Function,
            "span offset is not an instruction boundary",
        ))?;
        rows.push(SpanRow {
            instr_index,
            line: *line,
            column: *column,
        });
    }
    Ok(rows)
}
