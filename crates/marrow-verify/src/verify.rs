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
    ExportId, ImageId, ImageType, OP_ASSERT, OP_BOOL_NOT, OP_BRANCH_PRESENT, OP_BYTES_GE,
    OP_BYTES_GT, OP_BYTES_LE, OP_BYTES_LT, OP_CALL, OP_CONST_LOAD, OP_CONV_BYTES_TEXT,
    OP_CONV_STRING_BOOL, OP_CONV_STRING_INT, OP_DUR_CREATE_ENTRY, OP_DUR_ERASE_ENTRY,
    OP_DUR_ERASE_FIELD, OP_DUR_EXISTS, OP_DUR_NEXT_KEY, OP_DUR_READ_ENTRY, OP_DUR_READ_FIELD,
    OP_DUR_REPLACE_ENTRY, OP_DUR_SET_REQUIRED, OP_DUR_SET_SPARSE, OP_ENUM_CONSTRUCT,
    OP_ENUM_PAYLOAD_GET, OP_ENUM_TAG, OP_EQ_BOOL, OP_EQ_BYTES, OP_EQ_ENUM, OP_EQ_INT, OP_EQ_TEXT,
    OP_FIELD_GET, OP_FIELD_SET, OP_FIELD_UNSET, OP_INT_ADD, OP_INT_ADD_CHECKED, OP_INT_DIV,
    OP_INT_DIV_CHECKED, OP_INT_GE, OP_INT_GT, OP_INT_LE, OP_INT_LT, OP_INT_MUL, OP_INT_MUL_CHECKED,
    OP_INT_NEG, OP_INT_NEG_CHECKED, OP_INT_REM, OP_INT_REM_CHECKED, OP_INT_SUB, OP_INT_SUB_CHECKED,
    OP_JUMP, OP_JUMP_IF_FALSE, OP_LOCAL_GET, OP_LOCAL_SET, OP_POP, OP_RANGE_GUARD, OP_RECORD_NEW,
    OP_RETURN, OP_SOME_WRAP, OP_TEXT_CONCAT, OP_TEXT_CONTAINS, OP_TEXT_GE, OP_TEXT_GT,
    OP_TEXT_IS_EMPTY, OP_TEXT_LE, OP_TEXT_LT, OP_TEXT_TRIM, OP_TXN_BEGIN, OP_TXN_COMMIT,
    OP_UNREACHABLE, OP_VACANT_LOAD, OPTIONAL_FLAG, Scalar, TAG_BOOL, TAG_BYTES, TAG_ENUM, TAG_INT,
    TAG_RECORD, TAG_TEXT, TAG_UNIT, image_id,
};

use crate::reader::Reader;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::{
    Demand, RetShape, SealedConst, SealedEnumType, SealedExport, SealedField, SealedFunction,
    SealedInstr, SealedRecordType, SealedRoot, SealedSite, SealedSiteTarget, SealedTestEntry,
    SealedVariant, SpanRow, VerifiedImage,
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

struct DecodedRecordType {
    #[allow(dead_code)]
    name: u16,
    fields: Vec<DecodedField>,
}

struct DecodedField {
    name: u16,
    /// A bare (non-optional) type: a scalar for a durable-storable field, or a
    /// closed enum for a local-only value field. The enum index is bounds-checked
    /// against the ENUMS table after it decodes (`validate_record_field_enums`).
    ty: ImageType,
    required: bool,
}

/// A decoded enum type: name string index and its ordered variants.
struct DecodedEnum {
    name: u16,
    variants: Vec<DecodedVariant>,
}

/// A decoded enum variant: name string index, `category` flag, and dense payload
/// in declaration order. Each leaf is a bare (non-optional) [`ImageType`].
struct DecodedVariant {
    name: u16,
    category: bool,
    payload: Vec<ImageType>,
}

/// A decoded durable root: name string index, key scalar, and record type index.
struct DecodedRoot {
    name: u16,
    key: Scalar,
    record: u16,
}

/// A decoded durable site: root index and entry-or-field target.
struct DecodedSite {
    root: u16,
    target: SealedSiteTarget,
}

struct DecodedFunction {
    name: u16,
    source: u16,
    params: Vec<ImageType>,
    ret: RetShape,
    local_count: u16,
    code: Vec<u8>,
    spans: Vec<(u32, u32, u32)>,
}

struct DecodedImage {
    image_id: ImageId,
    strings: Vec<Rc<str>>,
    types: Vec<DecodedRecordType>,
    enums: Vec<DecodedEnum>,
    roots: Vec<DecodedRoot>,
    sites: Vec<DecodedSite>,
    consts: Vec<SealedConst>,
    functions: Vec<DecodedFunction>,
    exports: Vec<(ExportId, u16)>,
    /// Decoded TEST-ENTRY rows: `(name-string-index, function-index)`, ascending by
    /// name index.
    test_entries: Vec<(u16, u16)>,
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
    if section_count != 9 {
        return Err(reject(VerifyPhase::Envelope, "section count must be 9"));
    }
    let mut sections: Vec<(u8, &[u8])> = Vec::with_capacity(9);
    let mut last_id = 0u8;
    for _ in 0..9 {
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
    // Section ids strictly ascend and there are exactly 9, so they are exactly 1..9.
    for (index, (id, _)) in sections.iter().enumerate() {
        if *id != (index as u8 + 1) {
            return Err(reject(
                VerifyPhase::Envelope,
                "section ids must be exactly 1..9",
            ));
        }
    }

    // Phase 2: decode each table. Spans are decoded per function, in FUNCTIONS
    // order, so they are attached to the already-decoded function list.
    let strings = decode_strings(sections[0].1)?;
    let types = decode_types(sections[1].1, strings.len())?;
    let enums = decode_enums(sections[8].1, strings.len(), types.len())?;
    validate_record_field_enums(&types, enums.len())?;
    reject_value_type_cycles(&types, &enums)?;
    let (roots, sites) = decode_durable(sections[2].1, strings.len(), &types)?;
    let consts = decode_consts(sections[3].1, &strings)?;
    let mut functions = decode_functions(sections[4].1, strings.len(), types.len(), enums.len())?;
    let exports = decode_exports(sections[5].1, functions.len())?;
    decode_spans(sections[6].1, &mut functions)?;
    let test_entries = decode_test_entries(sections[7].1, strings.len(), functions.len())?;

    Ok(DecodedImage {
        image_id: image_id(payload),
        strings,
        types,
        enums,
        roots,
        sites,
        consts,
        functions,
        exports,
        test_entries,
    })
}

/// Decode the TEST-ENTRY table (section 0x08): a count, then each `u16 name index
/// ‖ u16 function index` entry in strictly ascending, unique name-index order. The
/// name index resolves a report label; the function index a storeless test body.
/// Structural violations are phase-`Table` rejections; the test-entry semantic
/// constraints (assert legality, storelessness, disjointness from exports) are
/// checked in the later TestEntry phase.
fn decode_test_entries(
    body: &[u8],
    string_count: usize,
    function_count: usize,
) -> Result<Vec<(u16, u16)>, VerifyRejection> {
    let mut reader = Reader::new(body);
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short test-entry count"))? as usize;
    if count > marrow_image::bounds::MAX_TEST_ENTRIES {
        return Err(reject(VerifyPhase::Table, "too many test entries"));
    }
    let mut entries = Vec::with_capacity(count);
    let mut previous_name: Option<u16> = None;
    for _ in 0..count {
        let name = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short test-entry name"))?;
        let func = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short test-entry function"))?;
        if name as usize >= string_count {
            return Err(reject(
                VerifyPhase::Table,
                "test-entry name index out of range",
            ));
        }
        if func as usize >= function_count {
            return Err(reject(
                VerifyPhase::Table,
                "test-entry function index out of range",
            ));
        }
        if let Some(prev) = previous_name
            && name <= prev
        {
            return Err(reject(
                VerifyPhase::Table,
                "test entries must be sorted and unique by name",
            ));
        }
        previous_name = Some(name);
        entries.push((name, func));
    }
    if !reader.is_empty() {
        return Err(reject(
            VerifyPhase::Table,
            "trailing bytes in test-entry table",
        ));
    }
    Ok(entries)
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
        TAG_BYTES => Some(Scalar::Bytes),
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
            let ty = decode_record_field_type(tag, &mut reader)?;
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

/// Decode the ENUMS table (section 0x09): a count, then per enum its name string
/// index, a variant count, and per variant a name string index, a `category` flag
/// byte, a payload count, and one bare-`ImageType` reference per payload leaf.
/// Variant names are unique within an enum; a payload leaf is a bare scalar, a
/// bare record (index in range), or a bare enum (index in range) — never optional.
/// The enum-payload reference graph must be acyclic (a value type cannot contain
/// itself), which the caller-facing acyclicity pass proves after decoding.
fn decode_enums(
    body: &[u8],
    string_count: usize,
    type_count: usize,
) -> Result<Vec<DecodedEnum>, VerifyRejection> {
    let mut reader = Reader::new(body);
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short enum count"))? as usize;
    if count > marrow_image::bounds::MAX_ENUMS {
        return Err(reject(VerifyPhase::Table, "too many enums"));
    }
    let mut enums = Vec::with_capacity(count);
    for _ in 0..count {
        let name = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short enum name"))?;
        if name as usize >= string_count {
            return Err(reject(VerifyPhase::Table, "enum name index out of range"));
        }
        let variant_count = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short variant count"))?
            as usize;
        if variant_count > marrow_image::bounds::MAX_VARIANTS {
            return Err(reject(VerifyPhase::Table, "too many enum variants"));
        }
        let mut variants = Vec::with_capacity(variant_count);
        let mut seen_names: Vec<u16> = Vec::with_capacity(variant_count);
        for _ in 0..variant_count {
            let vname = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, "short variant name"))?;
            if vname as usize >= string_count {
                return Err(reject(
                    VerifyPhase::Table,
                    "variant name index out of range",
                ));
            }
            if seen_names.contains(&vname) {
                return Err(reject(VerifyPhase::Table, "duplicate variant name in enum"));
            }
            seen_names.push(vname);
            let category_byte = reader
                .u8()
                .ok_or(reject(VerifyPhase::Table, "short variant category flag"))?;
            let category = match category_byte {
                0 => false,
                1 => true,
                _ => {
                    return Err(reject(
                        VerifyPhase::Table,
                        "variant category flag must be 0 or 1",
                    ));
                }
            };
            let payload_count = reader
                .u8()
                .ok_or(reject(VerifyPhase::Table, "short payload count"))?
                as usize;
            if payload_count > marrow_image::bounds::MAX_PAYLOAD_FIELDS {
                return Err(reject(VerifyPhase::Table, "too many payload fields"));
            }
            let mut payload = Vec::with_capacity(payload_count);
            for _ in 0..payload_count {
                let tag = reader
                    .u8()
                    .ok_or(reject(VerifyPhase::Table, "short payload type"))?;
                payload.push(decode_bare_payload_type(
                    tag,
                    &mut reader,
                    type_count,
                    count,
                )?);
            }
            variants.push(DecodedVariant {
                name: vname,
                category,
                payload,
            });
        }
        enums.push(DecodedEnum { name, variants });
    }
    if !reader.is_empty() {
        return Err(reject(VerifyPhase::Table, "trailing bytes in enum table"));
    }
    Ok(enums)
}

/// Decode one bare enum-payload leaf type: a scalar, a record, or an enum
/// reference, never optional. Record and enum indices are validated in range
/// (`type_count`/`enum_count`) so a payload can never name a type outside the
/// image.
fn decode_bare_payload_type(
    tag: u8,
    reader: &mut Reader,
    type_count: usize,
    enum_count: usize,
) -> Result<ImageType, VerifyRejection> {
    if tag & OPTIONAL_FLAG != 0 {
        return Err(reject(
            VerifyPhase::Table,
            "enum payload leaf cannot be optional",
        ));
    }
    match tag {
        TAG_INT | TAG_BOOL | TAG_TEXT | TAG_BYTES => Ok(ImageType::scalar(
            decode_bare_scalar(tag).expect("scalar base"),
        )),
        TAG_RECORD => {
            let idx = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, "short payload record index"))?;
            if idx as usize >= type_count {
                return Err(reject(
                    VerifyPhase::Table,
                    "payload record index out of range",
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
                .ok_or(reject(VerifyPhase::Table, "short payload enum index"))?;
            if idx as usize >= enum_count {
                return Err(reject(
                    VerifyPhase::Table,
                    "payload enum index out of range",
                ));
            }
            Ok(ImageType::Enum {
                idx,
                optional: false,
            })
        }
        _ => Err(reject(
            VerifyPhase::Table,
            "enum payload leaf must be a bare scalar, record, or enum",
        )),
    }
}

/// Decode a record field type: a bare scalar or a bare enum. A field is a scalar
/// leaf (durable-storable) or a closed enum value (local-only); it is never
/// optional (sparseness is the `required` flag) and never a directly nested
/// record. The enum index is only read here; `validate_record_field_enums`
/// bounds-checks it once the ENUMS table has decoded.
fn decode_record_field_type(tag: u8, reader: &mut Reader) -> Result<ImageType, VerifyRejection> {
    if tag & OPTIONAL_FLAG != 0 {
        return Err(reject(
            VerifyPhase::Table,
            "record field type cannot be optional",
        ));
    }
    match tag {
        TAG_INT | TAG_BOOL | TAG_TEXT | TAG_BYTES => Ok(ImageType::scalar(
            decode_bare_scalar(tag).expect("scalar base"),
        )),
        TAG_ENUM => {
            let idx = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, "short field enum index"))?;
            Ok(ImageType::Enum {
                idx,
                optional: false,
            })
        }
        _ => Err(reject(
            VerifyPhase::Table,
            "record field type must be a bare scalar or enum",
        )),
    }
}

/// Bounds-check every enum-typed record field against the decoded ENUMS table.
/// The field decoder reads the enum index before the table exists, so this runs
/// once both tables are decoded.
fn validate_record_field_enums(
    types: &[DecodedRecordType],
    enum_count: usize,
) -> Result<(), VerifyRejection> {
    for record in types {
        for field in &record.fields {
            if let ImageType::Enum { idx, .. } = field.ty
                && idx as usize >= enum_count
            {
                return Err(reject(
                    VerifyPhase::Table,
                    "record field enum index out of range",
                ));
            }
        }
    }
    Ok(())
}

/// Reject any cycle in the combined value-type reference graph over records and
/// enums: a record field may reference an enum, and an enum payload leaf may
/// reference a record or another enum, so a value type that (directly or
/// transitively) contains itself would be infinite. Records occupy node indices
/// `0..R` and enums `R..R+E`. A three-colour DFS; a back edge to a node on the
/// current stack is a cycle.
fn reject_value_type_cycles(
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
) -> Result<(), VerifyRejection> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Colour {
        White,
        Gray,
        Black,
    }
    let record_count = types.len();
    let enum_node = |idx: u16| record_count + idx as usize;
    let mut edges: Vec<Vec<usize>> = Vec::with_capacity(record_count + enums.len());
    for record in types {
        edges.push(
            record
                .fields
                .iter()
                .filter_map(|field| match field.ty {
                    ImageType::Enum { idx, .. } => Some(enum_node(idx)),
                    _ => None,
                })
                .collect(),
        );
    }
    for enum_def in enums {
        edges.push(
            enum_def
                .variants
                .iter()
                .flat_map(|variant| variant.payload.iter())
                .filter_map(|ty| match ty {
                    ImageType::Enum { idx, .. } => Some(enum_node(*idx)),
                    ImageType::Record { idx, .. } => Some(*idx as usize),
                    _ => None,
                })
                .collect(),
        );
    }
    let node_count = edges.len();
    let mut colour = vec![Colour::White; node_count];
    for start in 0..node_count {
        if colour[start] != Colour::White {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        colour[start] = Colour::Gray;
        while let Some(&(node, cursor)) = stack.last() {
            if cursor < edges[node].len() {
                stack.last_mut().expect("frame present").1 += 1;
                let next = edges[node][cursor];
                match colour[next] {
                    Colour::Gray => {
                        return Err(reject(
                            VerifyPhase::Table,
                            "the value type graph contains a cycle",
                        ));
                    }
                    Colour::White => {
                        colour[next] = Colour::Gray;
                        stack.push((next, 0));
                    }
                    Colour::Black => {}
                }
            } else {
                colour[node] = Colour::Black;
                stack.pop();
            }
        }
    }
    Ok(())
}

/// Decode the DURABLE table (design §C 0x03): 0 or 1 roots, then the operation
/// sites, revalidating every site against the roots and record types.
fn decode_durable(
    body: &[u8],
    string_count: usize,
    types: &[DecodedRecordType],
) -> Result<(Vec<DecodedRoot>, Vec<DecodedSite>), VerifyRejection> {
    let mut reader = Reader::new(body);
    let root_count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short root count"))? as usize;
    if root_count > marrow_image::bounds::MAX_ROOTS {
        return Err(reject(VerifyPhase::Table, "too many durable roots"));
    }
    let mut roots = Vec::with_capacity(root_count);
    for _ in 0..root_count {
        let name = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short root name"))?;
        if name as usize >= string_count {
            return Err(reject(VerifyPhase::Table, "root name index out of range"));
        }
        let key_tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short root key type"))?;
        let key = match decode_bare_scalar(key_tag) {
            Some(scalar @ (Scalar::Int | Scalar::Text)) => scalar,
            _ => {
                return Err(reject(
                    VerifyPhase::Table,
                    "root key type must be int or string",
                ));
            }
        };
        let record = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short root record"))?;
        if record as usize >= types.len() {
            return Err(reject(
                VerifyPhase::Table,
                "root record type index out of range",
            ));
        }
        // A durable record is stored as scalar leaves; a non-scalar (enum) field
        // has no store representation, so a resource carrying one cannot be a
        // durable root. This keeps the store-schema projection total.
        if types[record as usize]
            .fields
            .iter()
            .any(|field| !matches!(field.ty, ImageType::Scalar { .. }))
        {
            return Err(reject(
                VerifyPhase::Table,
                "a durable root record must have only scalar fields",
            ));
        }
        roots.push(DecodedRoot { name, key, record });
    }

    let site_count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short site count"))? as usize;
    if site_count > marrow_image::bounds::MAX_SITES {
        return Err(reject(VerifyPhase::Table, "too many durable sites"));
    }
    let mut sites: Vec<DecodedSite> = Vec::with_capacity(site_count);
    for _ in 0..site_count {
        let root = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short site root"))?;
        if root as usize >= roots.len() {
            return Err(reject(VerifyPhase::Table, "site root index out of range"));
        }
        let target_tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short site target"))?;
        let target = match target_tag {
            0x00 => SealedSiteTarget::Entry,
            0x01 => {
                let field = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, "short site field"))?;
                let record = &types[roots[root as usize].record as usize];
                if field as usize >= record.fields.len() {
                    return Err(reject(
                        VerifyPhase::Table,
                        "site field index out of range for its root record",
                    ));
                }
                SealedSiteTarget::Field(field)
            }
            _ => return Err(reject(VerifyPhase::Table, "unknown site target tag")),
        };
        // Sites are unique by (root, target).
        if sites
            .iter()
            .any(|existing| existing.root == root && existing.target == target)
        {
            return Err(reject(VerifyPhase::Table, "duplicate durable site"));
        }
        sites.push(DecodedSite { root, target });
    }
    if !reader.is_empty() {
        return Err(reject(
            VerifyPhase::Table,
            "trailing bytes in durable table",
        ));
    }
    Ok((roots, sites))
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

fn decode_type_ref_ret(
    tag: u8,
    reader: &mut Reader,
    type_count: usize,
    enum_count: usize,
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
        TAG_INT | TAG_BOOL | TAG_TEXT | TAG_BYTES => {
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
) -> Result<ImageType, VerifyRejection> {
    if tag & OPTIONAL_FLAG != 0 {
        return Err(reject(
            VerifyPhase::Table,
            "parameter type cannot be optional",
        ));
    }
    match tag {
        TAG_INT | TAG_BOOL | TAG_TEXT | TAG_BYTES => Ok(ImageType::scalar(
            decode_bare_scalar(tag).expect("scalar base"),
        )),
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
        _ => Err(reject(
            VerifyPhase::Table,
            "param type must be a bare scalar, record, or enum",
        )),
    }
}

fn decode_functions(
    body: &[u8],
    string_count: usize,
    type_count: usize,
    enum_count: usize,
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
            params.push(decode_param_ref(tag, &mut reader, type_count, enum_count)?);
        }
        let ret_tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short return type"))?;
        let ret = decode_type_ref_ret(ret_tag, &mut reader, type_count, enum_count)?;
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
fn decode_exports(
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
                    name: decoded.strings[field.name as usize].clone(),
                    ty: field.ty,
                    required: field.required,
                })
                .collect(),
        })
        .collect();
    let enums: Vec<SealedEnumType> = decoded
        .enums
        .iter()
        .map(|enum_def| SealedEnumType {
            name: decoded.strings[enum_def.name as usize].clone(),
            variants: enum_def
                .variants
                .iter()
                .map(|variant| SealedVariant {
                    name: decoded.strings[variant.name as usize].clone(),
                    category: variant.category,
                    payload: variant.payload.clone(),
                })
                .collect(),
        })
        .collect();
    let roots: Vec<SealedRoot> = decoded
        .roots
        .iter()
        .map(|root| SealedRoot {
            name: decoded.strings[root.name as usize].clone(),
            key: root.key,
            record: root.record,
        })
        .collect();
    let sites: Vec<SealedSite> = decoded
        .sites
        .iter()
        .map(|site| SealedSite {
            root: site.root,
            target: site.target,
        })
        .collect();
    // Function signatures feed the per-function `Call` type check (phase 3).
    let signatures: Vec<FnSig> = decoded
        .functions
        .iter()
        .map(|function| FnSig {
            params: function.params.clone(),
            ret: function.ret,
        })
        .collect();
    let ctx = Ctx {
        types: &types,
        enums: &enums,
        roots: &roots,
        sites: &sites,
        signatures: &signatures,
    };
    let mut functions = Vec::with_capacity(decoded.functions.len());
    for function in &decoded.functions {
        functions.push(verify_function(function, &ctx, &decoded)?);
    }

    // Phase 4: the call graph over the recorded direct calls must be acyclic
    // (recursion is not admitted).
    reject_call_cycles(&functions)?;

    // Phase 4/5: closure-informed effect and transaction-flow validation. An export
    // entry that mutates in closure is the owner of a transaction.
    let effects = Effects::compute(&functions);
    let export_entries: Vec<bool> = {
        let mut entries = vec![false; functions.len()];
        for (_, func) in &decoded.exports {
            entries[*func as usize] = true;
        }
        entries
    };
    for (index, function) in functions.iter().enumerate() {
        effects.check_transaction_flow(index, function, export_entries[index])?;
    }

    let exports = decoded
        .exports
        .iter()
        .map(|(id, func)| SealedExport {
            id: *id,
            func: *func,
            mutating: effects.mutates_closure[*func as usize],
            demand: effects.demand(*func),
        })
        .collect();

    // Record each export's effect class on its entry function too, for tools.
    for (_, func) in &decoded.exports {
        functions[*func as usize].mutating = effects.mutates_closure[*func as usize];
    }

    let test_entries = check_test_entries(&decoded, &functions, &export_entries, &effects)?;

    Ok(VerifiedImage {
        image_id: decoded.image_id,
        types,
        enums,
        roots,
        sites,
        consts: decoded.consts,
        functions,
        exports,
        test_entries,
    })
}

/// The test-entry phase (design §E extension): the TEST-ENTRY table names storeless
/// zero-argument entry points, `assert` is legal only inside one, and a test entry
/// is never an export, a mutating/reading closure, or a call target. Returns the
/// sealed entries in the table's ascending-name order.
fn check_test_entries(
    decoded: &DecodedImage,
    functions: &[SealedFunction],
    export_entries: &[bool],
    effects: &Effects,
) -> Result<Vec<SealedTestEntry>, VerifyRejection> {
    let mut is_test_entry = vec![false; functions.len()];
    for (_, func) in &decoded.test_entries {
        // The decoder proved every function index in range. Two names aliasing
        // one function would make the report double-count it; entries are unique
        // by function as well as by name.
        if is_test_entry[*func as usize] {
            return Err(reject(
                VerifyPhase::TestEntry,
                "duplicate test-entry function index",
            ));
        }
        is_test_entry[*func as usize] = true;
    }

    // `assert` may appear only in a test-entry function.
    for (index, function) in functions.iter().enumerate() {
        let has_assert = function
            .instrs()
            .iter()
            .any(|instr| matches!(instr, SealedInstr::Assert));
        if has_assert && !is_test_entry[index] {
            return Err(reject(
                VerifyPhase::TestEntry,
                "an assert instruction sits outside a test entry",
            ));
        }
    }

    // Each test entry is a storeless zero-argument entry point, disjoint from the
    // export table.
    for (_, func) in &decoded.test_entries {
        let function = &functions[*func as usize];
        if export_entries[*func as usize] {
            return Err(reject(
                VerifyPhase::TestEntry,
                "a test entry is also an export",
            ));
        }
        if !function.params.is_empty() {
            return Err(reject(
                VerifyPhase::TestEntry,
                "a test entry takes no parameters",
            ));
        }
        if function.ret != RetShape::Unit {
            return Err(reject(
                VerifyPhase::TestEntry,
                "a test entry must return unit",
            ));
        }
        if !effects.demand(*func).is_empty() {
            return Err(reject(
                VerifyPhase::TestEntry,
                "a test entry reads or writes durable data",
            ));
        }
    }

    // A test entry is an entry point: no function may call one.
    for function in functions {
        for callee in call_targets(function) {
            if is_test_entry[callee] {
                return Err(reject(
                    VerifyPhase::TestEntry,
                    "a test entry may not be called",
                ));
            }
        }
    }

    Ok(decoded
        .test_entries
        .iter()
        .map(|(name, func)| SealedTestEntry {
            name: decoded.strings[*name as usize].clone(),
            func: *func,
        })
        .collect())
}

/// The sealed tables the per-function checks consult.
struct Ctx<'a> {
    types: &'a [SealedRecordType],
    enums: &'a [SealedEnumType],
    roots: &'a [SealedRoot],
    sites: &'a [SealedSite],
    signatures: &'a [FnSig],
}

/// A callee's signature, consulted by the per-function `Call` type check.
struct FnSig {
    params: Vec<ImageType>,
    ret: RetShape,
}

/// Phase 4: reject any cycle in the direct-call graph (recursion is not admitted).
/// A three-colour DFS over the recorded calls; a back edge to a node on the current
/// stack is a cycle.
fn reject_call_cycles(functions: &[SealedFunction]) -> Result<(), VerifyRejection> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Colour {
        White,
        Gray,
        Black,
    }
    let mut colour = vec![Colour::White; functions.len()];
    // Iterative DFS: a frame is (node, next-child-cursor).
    for start in 0..functions.len() {
        if colour[start] != Colour::White {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        colour[start] = Colour::Gray;
        while let Some(&(node, cursor)) = stack.last() {
            let callees: Vec<usize> = call_targets(&functions[node]);
            if cursor < callees.len() {
                stack.last_mut().expect("frame present").1 += 1;
                let next = callees[cursor];
                match colour[next] {
                    Colour::Gray => {
                        return Err(reject(
                            VerifyPhase::Closure,
                            "the call graph contains a cycle",
                        ));
                    }
                    Colour::White => {
                        colour[next] = Colour::Gray;
                        stack.push((next, 0));
                    }
                    Colour::Black => {}
                }
            } else {
                colour[node] = Colour::Black;
                stack.pop();
            }
        }
    }
    Ok(())
}

/// The direct-call targets of a sealed function, in tape order.
fn call_targets(function: &SealedFunction) -> Vec<usize> {
    function
        .instrs()
        .iter()
        .filter_map(|instr| match instr {
            SealedInstr::Call(target) => Some(*target as usize),
            _ => None,
        })
        .collect()
}

/// Phase 4/5 durable-effect closure and the transaction-flow lattice (design §E).
struct Effects {
    /// Per function: whether it or a transitive callee stages a mutation.
    mutates_closure: Vec<bool>,
    /// Per function: whether it or a transitive callee reads durable data.
    reads_closure: Vec<bool>,
    /// Per function: whether it contains a `TxnBegin` (a transaction owner).
    has_begin: Vec<bool>,
    /// Per function: whether it contains a `TxnCommit`.
    has_commit: Vec<bool>,
}

impl Effects {
    fn compute(functions: &[SealedFunction]) -> Self {
        let count = functions.len();
        let mutates_self: Vec<bool> = functions
            .iter()
            .map(|function| function.instrs().iter().any(SealedInstr::is_mutation))
            .collect();
        let reads_self: Vec<bool> = functions
            .iter()
            .map(|function| function.instrs().iter().any(SealedInstr::is_durable_read))
            .collect();
        let has_begin: Vec<bool> = functions
            .iter()
            .map(|function| {
                function
                    .instrs()
                    .iter()
                    .any(|instr| matches!(instr, SealedInstr::TxnBegin))
            })
            .collect();
        let has_commit: Vec<bool> = functions
            .iter()
            .map(|function| {
                function
                    .instrs()
                    .iter()
                    .any(|instr| matches!(instr, SealedInstr::TxnCommit))
            })
            .collect();
        let callees: Vec<Vec<usize>> = functions.iter().map(call_targets).collect();

        // Fixpoint over the acyclic call graph: iterating `count` times converges.
        let mut mutates_closure = mutates_self.clone();
        let mut reads_closure = reads_self;
        for _ in 0..count {
            let mut changed = false;
            for f in 0..count {
                for &callee in &callees[f] {
                    if mutates_closure[callee] && !mutates_closure[f] {
                        mutates_closure[f] = true;
                        changed = true;
                    }
                    if reads_closure[callee] && !reads_closure[f] {
                        reads_closure[f] = true;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }

        Self {
            mutates_closure,
            reads_closure,
            has_begin,
            has_commit,
        }
    }

    /// The verifier-derived per-root demand of the export entered at `func`.
    fn demand(&self, func: u16) -> Demand {
        Demand {
            read: self.reads_closure[func as usize],
            write: self.mutates_closure[func as usize],
        }
    }

    /// Phase 5: validate one function's transaction flow. A transaction owner (a
    /// function that mutates in closure and contains `TxnBegin`) runs the
    /// {BeforeBegin, InTxn, AfterCommit} lattice; every other function must contain
    /// no transaction marker; and no function may call a transaction owner.
    fn check_transaction_flow(
        &self,
        index: usize,
        function: &SealedFunction,
        is_export_entry: bool,
    ) -> Result<(), VerifyRejection> {
        // A function containing `TxnBegin` is a transaction owner and may never be
        // called.
        for &callee in &call_targets(function) {
            if self.has_begin[callee] {
                return Err(reject(
                    VerifyPhase::Flow,
                    "a transaction owner may not be called",
                ));
            }
        }

        // A mutating export entry owns exactly one transaction; the lattice requires
        // it to begin and commit on every path with all mutations inside.
        if is_export_entry && self.mutates_closure[index] {
            return self.check_owner_lattice(function);
        }

        // Every other function is a read-only function or a mutating helper (wholly
        // inside its caller's transaction). Neither may carry a transaction marker.
        if self.has_begin[index] || self.has_commit[index] {
            return Err(reject(
                VerifyPhase::Flow,
                "a transaction marker sits outside its owning export",
            ));
        }
        Ok(())
    }

    /// The three-state transaction lattice over a transaction owner's CFG.
    fn check_owner_lattice(&self, function: &SealedFunction) -> Result<(), VerifyRejection> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum State {
            BeforeBegin,
            InTxn,
            AfterCommit,
        }
        let code = function.instrs();
        let mut entry: Vec<Option<State>> = vec![None; code.len()];
        entry[0] = Some(State::BeforeBegin);
        let mut worklist = vec![0usize];
        while let Some(index) = worklist.pop() {
            let state = entry[index].expect("worklist only enqueues reached instructions");
            let instr = &code[index];
            let next_state = match instr {
                SealedInstr::TxnBegin => {
                    if state != State::BeforeBegin {
                        return Err(reject(
                            VerifyPhase::Flow,
                            "the transaction is begun more than once",
                        ));
                    }
                    State::InTxn
                }
                SealedInstr::TxnCommit => {
                    if state != State::InTxn {
                        return Err(reject(
                            VerifyPhase::Flow,
                            "a transaction is committed outside its region",
                        ));
                    }
                    State::AfterCommit
                }
                SealedInstr::Return => {
                    if state != State::AfterCommit {
                        return Err(reject(
                            VerifyPhase::Flow,
                            "a path returns without committing the transaction",
                        ));
                    }
                    continue; // no successors
                }
                _ => {
                    let mutating_here = instr.is_mutation()
                        || matches!(instr, SealedInstr::Call(target) if self.mutates_closure[*target as usize]);
                    if mutating_here && state != State::InTxn {
                        return Err(reject(
                            VerifyPhase::Flow,
                            "a mutation sits outside the transaction region",
                        ));
                    }
                    state
                }
            };
            for successor in flow_successors(code, index) {
                match entry[successor] {
                    None => {
                        entry[successor] = Some(next_state);
                        worklist.push(successor);
                    }
                    Some(existing) if existing == next_state => {}
                    Some(_) => {
                        return Err(reject(
                            VerifyPhase::Flow,
                            "transaction state disagrees at a merge",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// The control-flow successors of the sealed instruction at `index`.
fn flow_successors(code: &[SealedInstr], index: usize) -> Vec<usize> {
    match &code[index] {
        SealedInstr::Return | SealedInstr::Unreachable(_) => Vec::new(),
        SealedInstr::Jump(target) => vec![*target],
        SealedInstr::JumpIfFalse(target)
        | SealedInstr::BranchPresent(target)
        | SealedInstr::IntAddChecked(target)
        | SealedInstr::IntSubChecked(target)
        | SealedInstr::IntMulChecked(target)
        | SealedInstr::IntNegChecked(target)
        | SealedInstr::IntDivChecked(target)
        | SealedInstr::IntRemChecked(target) => {
            vec![*target, index + 1]
        }
        _ => vec![index + 1],
    }
}

/// The successor edges for a two-way branch that keeps the current stack on the
/// `target` edge and pushes one value on the fallthrough edge (`index + 1`). Shared
/// by `BranchPresent` (present value) and the checked ops (int result).
fn push_on_fallthrough(
    frame: &Frame,
    target: usize,
    index: usize,
    pushed: VType,
    max_stack: &mut usize,
) -> Result<Vec<(usize, Frame)>, VerifyRejection> {
    let mut fallthrough = frame.clone();
    fallthrough.stack.push(pushed);
    if fallthrough.stack.len() > marrow_image::bounds::MAX_STACK_DEPTH {
        return Err(reject(
            VerifyPhase::Function,
            "operand stack exceeds depth bound",
        ));
    }
    *max_stack = (*max_stack).max(fallthrough.stack.len());
    Ok(vec![(target, frame.clone()), (index + 1, fallthrough)])
}

fn verify_function(
    function: &DecodedFunction,
    ctx: &Ctx,
    decoded: &DecodedImage,
) -> Result<SealedFunction, VerifyRejection> {
    let mut decoded_code = decode_code(&function.code)?;
    resolve_jumps(&mut decoded_code)?;
    let (instrs, max_stack) = check_flow(function, ctx, &decoded_code, &decoded.consts)?;
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
            OP_CONV_STRING_INT => SealedInstr::ConvStringInt,
            OP_CONV_STRING_BOOL => SealedInstr::ConvStringBool,
            OP_CONV_BYTES_TEXT => SealedInstr::ConvBytesText,
            OP_TEXT_IS_EMPTY => SealedInstr::TextIsEmpty,
            OP_TEXT_CONTAINS => SealedInstr::TextContains,
            OP_TEXT_TRIM => SealedInstr::TextTrim,
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
            OP_BRANCH_PRESENT => SealedInstr::BranchPresent(operand_u32(&mut reader)? as usize),
            OP_UNREACHABLE => SealedInstr::Unreachable(operand_u16(&mut reader)?),
            OP_ASSERT => SealedInstr::Assert,
            OP_CALL => SealedInstr::Call(operand_u16(&mut reader)?),
            OP_DUR_EXISTS => SealedInstr::DurExists(operand_u16(&mut reader)?),
            OP_DUR_READ_FIELD => SealedInstr::DurReadField(operand_u16(&mut reader)?),
            OP_DUR_READ_ENTRY => SealedInstr::DurReadEntry(operand_u16(&mut reader)?),
            OP_DUR_SET_REQUIRED => SealedInstr::DurSetRequired(operand_u16(&mut reader)?),
            OP_DUR_SET_SPARSE => SealedInstr::DurSetSparse(operand_u16(&mut reader)?),
            OP_DUR_CREATE_ENTRY => SealedInstr::DurCreateEntry(operand_u16(&mut reader)?),
            OP_DUR_REPLACE_ENTRY => SealedInstr::DurReplaceEntry(operand_u16(&mut reader)?),
            OP_DUR_ERASE_FIELD => SealedInstr::DurEraseField(operand_u16(&mut reader)?),
            OP_DUR_ERASE_ENTRY => SealedInstr::DurEraseEntry(operand_u16(&mut reader)?),
            OP_DUR_NEXT_KEY => SealedInstr::DurNextKey(operand_u16(&mut reader)?),
            OP_TXN_BEGIN => SealedInstr::TxnBegin,
            OP_TXN_COMMIT => SealedInstr::TxnCommit,
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
        TAG_INT | TAG_BOOL | TAG_TEXT | TAG_BYTES => Ok(ImageType::Scalar {
            scalar: decode_bare_scalar(base).expect("scalar base"),
            optional: true,
        }),
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
        _ => Err(reject(
            VerifyPhase::Function,
            "vacant-load operand must be an optional scalar or enum",
        )),
    }
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
    /// Checked-arithmetic branch: the operands are already popped. The fault edge
    /// (`target`) carries the current stack; the success edge (fallthrough)
    /// additionally carries the operation's `result`.
    CheckedResult { target: usize, result: VType },
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
    ctx: &Ctx,
    code: &[Decoded],
    consts: &[SealedConst],
) -> Result<(Vec<SealedInstr>, usize), VerifyRejection> {
    if code.is_empty() {
        return Err(reject(VerifyPhase::Function, "function has no code"));
    }

    // Params occupy locals `0..param_count`, pre-initialized to their param type;
    // the rest start uninitialized. The entry operand stack is empty.
    let mut initial_locals: Vec<Option<VType>> = vec![None; function.local_count as usize];
    for (slot, param) in function.params.iter().enumerate() {
        initial_locals[slot] =
            Some(VType::from_image(*param).expect("a parameter type is never unit"));
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
        let control = apply(function, ctx, &code[index].instr, consts, &mut frame)?;
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
            // Both carry the current stack on the `target` edge and push one value on
            // the fallthrough edge; only which edge is the "taken" one differs in
            // meaning (present vs fault), not in the CFG edge shapes.
            Control::BranchPresent { target, present } => {
                push_on_fallthrough(&frame, target, index, present, &mut max_stack)?
            }
            Control::CheckedResult { target, result } => {
                push_on_fallthrough(&frame, target, index, result, &mut max_stack)?
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
    ctx: &Ctx,
    instr: &SealedInstr,
    consts: &[SealedConst],
    frame: &mut Frame,
) -> Result<Control, VerifyRejection> {
    let types = ctx.types;
    let signatures = ctx.signatures;
    if is_durable(instr) {
        return apply_durable(ctx, instr, frame);
    }
    // Record/optional/call opcodes need the whole frame or the signatures; the
    // scalar opcodes work on the stack alone, borrowed here after these return.
    if let SealedInstr::Call(target) = instr {
        let sig = signatures.get(*target as usize).ok_or(reject(
            VerifyPhase::Function,
            "call target index out of range",
        ))?;
        // a0 is pushed first, so pop arguments in reverse parameter order.
        for param in sig.params.iter().rev() {
            let got = pop(&mut frame.stack)?;
            let want = VType::from_image(*param).expect("a parameter type is never unit");
            if got != want {
                return Err(reject(VerifyPhase::Function, "call argument type mismatch"));
            }
        }
        match sig.ret {
            RetShape::Unit => {}
            RetShape::Scalar { scalar, optional } => {
                frame.stack.push(VType::Scalar { scalar, optional });
            }
            RetShape::Record { idx, optional } => {
                frame.stack.push(VType::Record { idx, optional });
            }
            RetShape::Enum { idx, optional } => {
                frame.stack.push(VType::Enum { idx, optional });
            }
        }
        return Ok(Control::Fallthrough);
    }
    match instr {
        SealedInstr::RecordNew(ty) => {
            let record = types.get(*ty as usize).ok_or(reject(
                VerifyPhase::Function,
                "record type index out of range",
            ))?;
            // f0 is pushed first, so pop fields in reverse declaration order.
            for field in record.fields.iter().rev() {
                let bare = VType::from_image(field.ty).expect("a record field type is never unit");
                let want = if field.required {
                    bare
                } else {
                    bare.to_optional()
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
            let bare = VType::from_image(field_def.ty).expect("a record field type is never unit");
            let result = if field_def.required {
                bare
            } else {
                bare.to_optional()
            };
            frame.stack.push(result);
            return Ok(Control::Fallthrough);
        }
        SealedInstr::FieldSet(field) => {
            // `[record, value] → [record]`: store a bare field value present.
            let value = pop(&mut frame.stack)?;
            let record = pop(&mut frame.stack)?;
            let VType::Record {
                idx,
                optional: false,
            } = record
            else {
                return Err(reject(
                    VerifyPhase::Function,
                    "field set requires a bare record",
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
            let want = VType::from_image(field_def.ty).expect("a record field type is never unit");
            if value != want {
                return Err(reject(
                    VerifyPhase::Function,
                    "field set operand type mismatch",
                ));
            }
            frame.stack.push(VType::bare_record(idx));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::FieldUnset(field) => {
            // `[record] → [record]`: clear a sparse field to vacant.
            let record = pop(&mut frame.stack)?;
            let VType::Record {
                idx,
                optional: false,
            } = record
            else {
                return Err(reject(
                    VerifyPhase::Function,
                    "field unset requires a bare record",
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
            if field_def.required {
                return Err(reject(
                    VerifyPhase::Function,
                    "a required field cannot be unset",
                ));
            }
            frame.stack.push(VType::bare_record(idx));
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
        SealedInstr::VacantLoad(ty) => {
            // An enum operand names a value type; bounds-check it against the table.
            if let ImageType::Enum { idx, .. } = ty
                && ctx.enums.get(*idx as usize).is_none()
            {
                return Err(reject(
                    VerifyPhase::Function,
                    "vacant-load enum index out of range",
                ));
            }
            frame
                .stack
                .push(VType::from_image(*ty).expect("vacant-load operand is optional"));
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
        SealedInstr::EnumConstruct { enum_idx, variant } => {
            let enum_def = ctx.enums.get(*enum_idx as usize).ok_or(reject(
                VerifyPhase::Function,
                "enum type index out of range",
            ))?;
            let variant_def = enum_def.variants().get(*variant as usize).ok_or(reject(
                VerifyPhase::Function,
                "enum variant index out of range",
            ))?;
            // p0 is pushed first, so pop the payload in reverse declaration order.
            for ty in variant_def.payload.iter().rev() {
                let want = VType::from_image(*ty).expect("a payload leaf is never unit");
                let got = pop(&mut frame.stack)?;
                if got != want {
                    return Err(reject(
                        VerifyPhase::Function,
                        "enum payload operand type mismatch",
                    ));
                }
            }
            frame.stack.push(VType::bare_enum(*enum_idx));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::EnumTag => {
            let value = pop(&mut frame.stack)?;
            if !matches!(
                value,
                VType::Enum {
                    optional: false,
                    ..
                }
            ) {
                return Err(reject(
                    VerifyPhase::Function,
                    "enum-tag requires a bare enum",
                ));
            }
            frame.stack.push(VType::bare_scalar(Scalar::Int));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::EnumPayloadGet { variant, field } => {
            let value = pop(&mut frame.stack)?;
            let VType::Enum {
                idx,
                optional: false,
            } = value
            else {
                return Err(reject(
                    VerifyPhase::Function,
                    "enum-payload-get requires a bare enum",
                ));
            };
            let enum_def = ctx.enums.get(idx as usize).ok_or(reject(
                VerifyPhase::Function,
                "enum type index out of range",
            ))?;
            let variant_def = enum_def.variants().get(*variant as usize).ok_or(reject(
                VerifyPhase::Function,
                "enum variant index out of range",
            ))?;
            // The variant operand types the payload leaf; the VM faults if the
            // runtime value carries a different variant, so the pushed type is
            // never observed on a mismatch.
            let leaf = variant_def.payload.get(*field as usize).ok_or(reject(
                VerifyPhase::Function,
                "enum payload field index out of range",
            ))?;
            frame
                .stack
                .push(VType::from_image(*leaf).expect("a payload leaf is never unit"));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::EqEnum => {
            let right = pop(&mut frame.stack)?;
            let left = pop(&mut frame.stack)?;
            let (
                VType::Enum {
                    idx: r,
                    optional: false,
                },
                VType::Enum {
                    idx: l,
                    optional: false,
                },
            ) = (right, left)
            else {
                return Err(reject(
                    VerifyPhase::Function,
                    "enum equality requires two bare enums",
                ));
            };
            if l != r {
                return Err(reject(
                    VerifyPhase::Function,
                    "enum equality operands are different enums",
                ));
            }
            frame.stack.push(VType::bare_scalar(Scalar::Bool));
            return Ok(Control::Fallthrough);
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
        SealedInstr::Unreachable(idx) => match consts.get(*idx as usize) {
            // The operand is the static invariant text; it must be a text const. The
            // instruction never falls through, so it ends the frame like `Return`
            // without a return-value check — it always faults.
            Some(SealedConst::Text(_)) => Ok(Control::Return),
            Some(_) => Err(reject(
                VerifyPhase::Function,
                "unreachable operand must be a text const",
            )),
            None => Err(reject(VerifyPhase::Function, "const index out of range")),
        },
        SealedInstr::Jump(target) => Ok(Control::Jump(*target)),
        SealedInstr::JumpIfFalse(target) => {
            expect_scalar(pop(stack)?, Scalar::Bool)?;
            Ok(Control::Branch(*target))
        }
        SealedInstr::Assert => {
            // Pops the bool condition and pushes nothing; the test-entry phase
            // separately proves it appears only in a test-entry function.
            expect_scalar(pop(stack)?, Scalar::Bool)?;
            Ok(Control::Fallthrough)
        }
        SealedInstr::IntAdd
        | SealedInstr::IntSub
        | SealedInstr::IntMul
        | SealedInstr::IntRem
        | SealedInstr::IntDiv => {
            binary(stack, Scalar::Int, Scalar::Int)?;
            Ok(Control::Fallthrough)
        }
        SealedInstr::IntNeg => {
            expect_scalar(pop(stack)?, Scalar::Int)?;
            stack.push(VType::bare_scalar(Scalar::Int));
            Ok(Control::Fallthrough)
        }
        SealedInstr::IntAddChecked(target)
        | SealedInstr::IntSubChecked(target)
        | SealedInstr::IntMulChecked(target)
        | SealedInstr::IntDivChecked(target)
        | SealedInstr::IntRemChecked(target) => {
            expect_scalar(pop(stack)?, Scalar::Int)?;
            expect_scalar(pop(stack)?, Scalar::Int)?;
            Ok(Control::CheckedResult {
                target: *target,
                result: VType::bare_scalar(Scalar::Int),
            })
        }
        SealedInstr::IntNegChecked(target) => {
            expect_scalar(pop(stack)?, Scalar::Int)?;
            Ok(Control::CheckedResult {
                target: *target,
                result: VType::bare_scalar(Scalar::Int),
            })
        }
        SealedInstr::RangeGuard { .. } => {
            // Peeks the guarded value: the top of the stack must be a bare int,
            // which the guard leaves in place (fault or fall through).
            let top = *stack.last().ok_or(reject(
                VerifyPhase::Function,
                "range guard on an empty stack",
            ))?;
            expect_scalar(top, Scalar::Int)?;
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
        SealedInstr::TextLt | SealedInstr::TextLe | SealedInstr::TextGt | SealedInstr::TextGe => {
            binary(stack, Scalar::Text, Scalar::Bool)?;
            Ok(Control::Fallthrough)
        }
        SealedInstr::EqBytes
        | SealedInstr::BytesLt
        | SealedInstr::BytesLe
        | SealedInstr::BytesGt
        | SealedInstr::BytesGe => {
            binary(stack, Scalar::Bytes, Scalar::Bool)?;
            Ok(Control::Fallthrough)
        }
        SealedInstr::ConvStringInt => {
            expect_scalar(pop(stack)?, Scalar::Int)?;
            stack.push(VType::bare_scalar(Scalar::Text));
            Ok(Control::Fallthrough)
        }
        SealedInstr::ConvStringBool => {
            expect_scalar(pop(stack)?, Scalar::Bool)?;
            stack.push(VType::bare_scalar(Scalar::Text));
            Ok(Control::Fallthrough)
        }
        SealedInstr::ConvBytesText => {
            expect_scalar(pop(stack)?, Scalar::Text)?;
            stack.push(VType::bare_scalar(Scalar::Bytes));
            Ok(Control::Fallthrough)
        }
        SealedInstr::TextIsEmpty => {
            expect_scalar(pop(stack)?, Scalar::Text)?;
            stack.push(VType::bare_scalar(Scalar::Bool));
            Ok(Control::Fallthrough)
        }
        SealedInstr::TextContains => {
            expect_scalar(pop(stack)?, Scalar::Text)?;
            expect_scalar(pop(stack)?, Scalar::Text)?;
            stack.push(VType::bare_scalar(Scalar::Bool));
            Ok(Control::Fallthrough)
        }
        SealedInstr::TextTrim => {
            expect_scalar(pop(stack)?, Scalar::Text)?;
            stack.push(VType::bare_scalar(Scalar::Text));
            Ok(Control::Fallthrough)
        }
        SealedInstr::RecordNew(_)
        | SealedInstr::FieldGet(_)
        | SealedInstr::FieldSet(_)
        | SealedInstr::FieldUnset(_)
        | SealedInstr::SomeWrap
        | SealedInstr::VacantLoad(_)
        | SealedInstr::BranchPresent(_)
        | SealedInstr::EnumConstruct { .. }
        | SealedInstr::EnumTag
        | SealedInstr::EnumPayloadGet { .. }
        | SealedInstr::EqEnum
        | SealedInstr::Call(_)
        | SealedInstr::DurExists(_)
        | SealedInstr::DurReadField(_)
        | SealedInstr::DurReadEntry(_)
        | SealedInstr::DurSetRequired(_)
        | SealedInstr::DurSetSparse(_)
        | SealedInstr::DurCreateEntry(_)
        | SealedInstr::DurReplaceEntry(_)
        | SealedInstr::DurEraseField(_)
        | SealedInstr::DurEraseEntry(_)
        | SealedInstr::DurNextKey(_)
        | SealedInstr::TxnBegin
        | SealedInstr::TxnCommit => {
            unreachable!(
                "record, optional, call, and durable opcodes return from the earlier matches"
            )
        }
    }
}

/// Whether `instr` is handled by [`apply_durable`] (a durable op or a transaction
/// marker).
fn is_durable(instr: &SealedInstr) -> bool {
    instr.is_mutation()
        || instr.is_durable_read()
        || matches!(instr, SealedInstr::TxnBegin | SealedInstr::TxnCommit)
}

/// The site operand of a durable op, or `None` for a transaction marker.
fn durable_site(instr: &SealedInstr) -> Option<u16> {
    match instr {
        SealedInstr::DurExists(site)
        | SealedInstr::DurReadField(site)
        | SealedInstr::DurReadEntry(site)
        | SealedInstr::DurSetRequired(site)
        | SealedInstr::DurSetSparse(site)
        | SealedInstr::DurCreateEntry(site)
        | SealedInstr::DurReplaceEntry(site)
        | SealedInstr::DurEraseField(site)
        | SealedInstr::DurEraseEntry(site)
        | SealedInstr::DurNextKey(site) => Some(*site),
        _ => None,
    }
}

/// Phase-3 type check for durable opcodes and transaction markers (design §D). The
/// transaction markers leave the stack unchanged; phase 5 checks their flow.
fn apply_durable(
    ctx: &Ctx,
    instr: &SealedInstr,
    frame: &mut Frame,
) -> Result<Control, VerifyRejection> {
    let Some(site_index) = durable_site(instr) else {
        // TxnBegin / TxnCommit: no stack effect here.
        return Ok(Control::Fallthrough);
    };
    let site = ctx.sites.get(site_index as usize).ok_or(reject(
        VerifyPhase::Function,
        "durable site index out of range",
    ))?;
    let root = ctx.roots.get(site.root as usize).ok_or(reject(
        VerifyPhase::Function,
        "durable site root out of range",
    ))?;
    let key_ty = VType::bare_scalar(root.key);
    let stack = &mut frame.stack;
    match instr {
        SealedInstr::DurExists(_) => {
            expect(pop(stack)?, key_ty)?;
            stack.push(VType::bare_scalar(Scalar::Bool));
        }
        SealedInstr::DurReadField(_) => {
            let field = field_of(ctx, site, root)?;
            let value = durable_field_vtype(field).to_optional();
            expect(pop(stack)?, key_ty)?;
            stack.push(value);
        }
        SealedInstr::DurReadEntry(_) => {
            require_entry(site)?;
            expect(pop(stack)?, key_ty)?;
            stack.push(VType::bare_record(root.record).to_optional());
        }
        SealedInstr::DurSetRequired(_) => {
            let field = field_of(ctx, site, root)?;
            if !field.required {
                return Err(reject(
                    VerifyPhase::Function,
                    "set-required targets a sparse field",
                ));
            }
            let value = durable_field_vtype(field);
            expect(pop(stack)?, value)?;
            expect(pop(stack)?, key_ty)?;
        }
        SealedInstr::DurSetSparse(_) => {
            let field = field_of(ctx, site, root)?;
            if field.required {
                return Err(reject(
                    VerifyPhase::Function,
                    "set-sparse targets a required field",
                ));
            }
            let value = durable_field_vtype(field).to_optional();
            expect(pop(stack)?, value)?;
            expect(pop(stack)?, key_ty)?;
        }
        SealedInstr::DurCreateEntry(_) | SealedInstr::DurReplaceEntry(_) => {
            require_entry(site)?;
            expect(pop(stack)?, VType::bare_record(root.record))?;
            expect(pop(stack)?, key_ty)?;
        }
        SealedInstr::DurEraseField(_) => {
            let field = field_of(ctx, site, root)?;
            if field.required {
                return Err(reject(
                    VerifyPhase::Function,
                    "erase targets a required field",
                ));
            }
            expect(pop(stack)?, key_ty)?;
        }
        SealedInstr::DurEraseEntry(_) => {
            require_entry(site)?;
            expect(pop(stack)?, key_ty)?;
        }
        SealedInstr::DurNextKey(_) => {
            require_entry(site)?;
            let opt_key = key_ty.to_optional();
            expect(pop(stack)?, opt_key)?;
            stack.push(opt_key);
        }
        _ => unreachable!("durable_site returned a site for this opcode"),
    }
    Ok(Control::Fallthrough)
}

/// The field a field-target site addresses, or a rejection when the site is an
/// entry site.
fn field_of<'a>(
    ctx: &'a Ctx,
    site: &SealedSite,
    root: &SealedRoot,
) -> Result<&'a SealedField, VerifyRejection> {
    let SealedSiteTarget::Field(field) = site.target else {
        return Err(reject(
            VerifyPhase::Function,
            "operation requires a field site",
        ));
    };
    ctx.types[root.record as usize]
        .fields()
        .get(field as usize)
        .ok_or(reject(
            VerifyPhase::Function,
            "site field index out of range",
        ))
}

/// The bare value type of a durable field. A durable root record is verified to
/// carry only scalar fields, so the field type is always a bare scalar here.
fn durable_field_vtype(field: &SealedField) -> VType {
    VType::from_image(field.ty).expect("a durable field type is a bare scalar")
}

/// Require an entry-target site.
fn require_entry(site: &SealedSite) -> Result<(), VerifyRejection> {
    match site.target {
        SealedSiteTarget::Entry => Ok(()),
        SealedSiteTarget::Field(_) => Err(reject(
            VerifyPhase::Function,
            "operation requires an entry site",
        )),
    }
}

/// Require `value` to be exactly `want`.
fn expect(value: VType, want: VType) -> Result<(), VerifyRejection> {
    if value == want {
        Ok(())
    } else {
        Err(reject(
            VerifyPhase::Function,
            "durable operand type mismatch",
        ))
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
