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
    DurableBranchShape, DurableContractDescriptor, DurableContractId, DurableEnumMemberShape,
    DurableFieldShape, DurableGroupShape, DurableIndexComponent, DurableIndexShape,
    DurableKeyShape, DurableMemberShape, DurableRootShape, DurableValueShape, ExportDemand,
    ExportId, ImageType, LedgerIdBytes, OPTIONAL_FLAG, Scalar, SemanticNode, SemanticNodeKind,
    SemanticPath, SemanticStep, SemanticStepKind, SemanticTarget, TAG_BOOL, TAG_BYTES,
    TAG_COLLECTION, TAG_DATE, TAG_DURATION, TAG_ENUM, TAG_IDENTITY, TAG_INSTANT, TAG_INT,
    TAG_RECORD, TAG_TEXT, TAG_UNIT, image_id,
};

use crate::reader::Reader;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::{
    RetShape, SealedBranch, SealedCollectionType, SealedConst, SealedEnumType, SealedExport,
    SealedField, SealedFunction, SealedGroup, SealedIndex, SealedIndexComponent, SealedInstr,
    SealedRecordType, SealedRoot, SealedSite, SealedSiteTarget, SealedTestEntry, SealedVariant,
    TestKind, VerifiedImage,
};

mod context;
mod decode_code;
mod flow;
mod model;
mod presence;
mod spans;

use context::{Ctx, Effects, FnSig};
use flow::durable_op_class;
use presence::{call_targets, check_presence_flow, reject_call_cycles, verify_function};

use model::{
    DecodedEnum, DecodedField, DecodedFunction, DecodedImage, DecodedIndex, DecodedMember,
    DecodedRecordType, DecodedRoot, DecodedVariant,
};

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
    if section_count != 10 {
        return Err(reject(VerifyPhase::Envelope, "section count must be 10"));
    }
    let mut sections: Vec<(u8, &[u8])> = Vec::with_capacity(10);
    let mut last_id = 0u8;
    for _ in 0..10 {
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
    // Section ids strictly ascend and there are exactly 10, so they are exactly 1..10.
    for (index, (id, _)) in sections.iter().enumerate() {
        if *id != (index as u8 + 1) {
            return Err(reject(
                VerifyPhase::Envelope,
                "section ids must be exactly 1..10",
            ));
        }
    }

    // Phase 2: decode each table. Spans are decoded per function, in FUNCTIONS
    // order, so they are attached to the already-decoded function list.
    let strings = decode_strings(sections[0].1)?;
    let types = decode_types(sections[1].1, strings.len())?;
    let enums = decode_enums(sections[8].1, strings.len(), types.len())?;
    let collections = decode_collections(sections[9].1, types.len(), enums.len())?;
    validate_record_field_refs(&types, enums.len(), collections.len())?;
    reject_value_type_cycles(&types, &enums)?;
    let (roots, sites, site_paths, durable_contract, durable_descriptor) =
        decode_durable(sections[2].1, &strings, &types, &enums)?;
    let consts = decode_consts(sections[3].1, &strings)?;
    let mut functions = decode_functions(
        sections[4].1,
        strings.len(),
        types.len(),
        enums.len(),
        collections.len(),
        roots.len(),
    )?;
    let exports = decode_exports(sections[5].1, functions.len())?;
    decode_spans(sections[6].1, &mut functions)?;
    let test_entries = decode_test_entries(sections[7].1, strings.len(), functions.len())?;

    Ok(DecodedImage {
        image_id: image_id(payload),
        strings,
        types,
        enums,
        collections,
        roots,
        sites,
        site_paths,
        durable_contract,
        durable_descriptor,
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
        TAG_DATE => Some(Scalar::Date),
        TAG_INSTANT => Some(Scalar::Instant),
        TAG_DURATION => Some(Scalar::Duration),
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
        if field_count > marrow_image::bounds::MAX_RECORD_FIELDS {
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

/// Decode the COLLTYPES table (section 0x0A): a count, then per collection type a
/// one-byte kind tag (`0x00` List, `0x01` Map) and its bare-`ImageType` element
/// reference (List) or key then value references (Map). A referenced `Collection`
/// index must name a strictly earlier row, so the collection reference graph is
/// acyclic by construction (a nested collection is always minted after its inner
/// shape). A `Map` key must be a bare scalar key type (`int`/`bool`/`string`/`bytes`;
/// a nominal key is int-shaped) — the one durable-key scalar family the ordered map
/// compares over.
fn decode_collections(
    body: &[u8],
    type_count: usize,
    enum_count: usize,
) -> Result<Vec<SealedCollectionType>, VerifyRejection> {
    let mut reader = Reader::new(body);
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short collection count"))? as usize;
    if count > marrow_image::bounds::MAX_COLLECTIONS {
        return Err(reject(VerifyPhase::Table, "too many collection types"));
    }
    let mut collections = Vec::with_capacity(count);
    for row in 0..count {
        let kind = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short collection kind"))?;
        let coll = match kind {
            0x00 => {
                let elem = decode_collection_inner_ref(&mut reader, type_count, enum_count, row)?;
                SealedCollectionType::List { elem }
            }
            0x01 => {
                let key = decode_collection_inner_ref(&mut reader, type_count, enum_count, row)?;
                if !matches!(
                    key,
                    ImageType::Scalar {
                        optional: false,
                        ..
                    }
                ) {
                    return Err(reject(
                        VerifyPhase::Table,
                        "map key must be a bare scalar key type",
                    ));
                }
                let value = decode_collection_inner_ref(&mut reader, type_count, enum_count, row)?;
                SealedCollectionType::Map { key, value }
            }
            _ => {
                return Err(reject(
                    VerifyPhase::Table,
                    "collection kind must be 0 (list) or 1 (map)",
                ));
            }
        };
        collections.push(coll);
    }
    if !reader.is_empty() {
        return Err(reject(
            VerifyPhase::Table,
            "trailing bytes in collection table",
        ));
    }
    Ok(collections)
}

/// Decode one bare element/key/value type inside a COLLTYPES row: a scalar, a record
/// (index in range), an enum (index in range), or a collection (a strictly earlier
/// row `< current`). Never optional — a collection's leaf types are bare.
fn decode_collection_inner_ref(
    reader: &mut Reader,
    type_count: usize,
    enum_count: usize,
    current_row: usize,
) -> Result<ImageType, VerifyRejection> {
    let tag = reader
        .u8()
        .ok_or(reject(VerifyPhase::Table, "short collection leaf type"))?;
    if tag & OPTIONAL_FLAG != 0 {
        return Err(reject(
            VerifyPhase::Table,
            "collection leaf type cannot be optional",
        ));
    }
    match tag {
        TAG_INT | TAG_BOOL | TAG_TEXT | TAG_BYTES | TAG_DATE | TAG_INSTANT | TAG_DURATION => Ok(
            ImageType::scalar(decode_bare_scalar(tag).expect("scalar base")),
        ),
        TAG_RECORD => {
            let idx = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, "short collection record index"))?;
            if idx as usize >= type_count {
                return Err(reject(
                    VerifyPhase::Table,
                    "collection record index out of range",
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
                .ok_or(reject(VerifyPhase::Table, "short collection enum index"))?;
            if idx as usize >= enum_count {
                return Err(reject(
                    VerifyPhase::Table,
                    "collection enum index out of range",
                ));
            }
            Ok(ImageType::Enum {
                idx,
                optional: false,
            })
        }
        TAG_COLLECTION => {
            let idx = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, "short nested collection index"))?;
            if idx as usize >= current_row {
                return Err(reject(
                    VerifyPhase::Table,
                    "nested collection index must name an earlier collection",
                ));
            }
            Ok(ImageType::Collection {
                idx,
                optional: false,
            })
        }
        _ => Err(reject(
            VerifyPhase::Table,
            "collection leaf type must be a bare scalar, record, enum, or earlier collection",
        )),
    }
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
        TAG_INT | TAG_BOOL | TAG_TEXT | TAG_BYTES | TAG_DATE | TAG_INSTANT | TAG_DURATION => Ok(
            ImageType::scalar(decode_bare_scalar(tag).expect("scalar base")),
        ),
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
        TAG_INT | TAG_BOOL | TAG_TEXT | TAG_BYTES | TAG_DATE | TAG_INSTANT | TAG_DURATION => Ok(
            ImageType::scalar(decode_bare_scalar(tag).expect("scalar base")),
        ),
        TAG_ENUM => {
            let idx = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, "short field enum index"))?;
            Ok(ImageType::Enum {
                idx,
                optional: false,
            })
        }
        TAG_RECORD => {
            let idx = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, "short field record index"))?;
            Ok(ImageType::Record {
                idx,
                optional: false,
            })
        }
        TAG_COLLECTION => {
            let idx = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, "short field collection index"))?;
            Ok(ImageType::Collection {
                idx,
                optional: false,
            })
        }
        _ => Err(reject(
            VerifyPhase::Table,
            "record field type must be a bare scalar, record, enum, or collection",
        )),
    }
}

/// Bounds-check every record field's referenced value type against the decoded
/// tables: an enum-typed field against the ENUMS table and a record-typed field
/// (a struct-valued field) against the RECORD-TYPES table. The field decoder reads
/// each index before the referenced table exists, so this runs once both tables are
/// decoded. Cycles among the in-range references are rejected separately.
fn validate_record_field_refs(
    types: &[DecodedRecordType],
    enum_count: usize,
    collection_count: usize,
) -> Result<(), VerifyRejection> {
    for record in types {
        for field in &record.fields {
            match field.ty {
                ImageType::Enum { idx, .. } if idx as usize >= enum_count => {
                    return Err(reject(
                        VerifyPhase::Table,
                        "record field enum index out of range",
                    ));
                }
                ImageType::Record { idx, .. } if idx as usize >= types.len() => {
                    return Err(reject(
                        VerifyPhase::Table,
                        "record field record index out of range",
                    ));
                }
                ImageType::Collection { idx, .. } if idx as usize >= collection_count => {
                    return Err(reject(
                        VerifyPhase::Table,
                        "record field collection index out of range",
                    ));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Reject any cycle in the combined value-type reference graph over records and
/// enums: a record field may reference another record (a struct-typed field) or an
/// enum, and an enum payload leaf may reference a record or another enum, so a value
/// type that (directly or transitively) contains itself would be infinite. Records
/// occupy node indices `0..R` and enums `R..R+E`. A three-colour DFS; a back edge to
/// a node on the current stack is a cycle.
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
                    ImageType::Record { idx, .. } => Some(idx as usize),
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

/// Decode the DURABLE table (design §C 0x03): up to `MAX_ROOTS` roots — preceded,
/// when any root is present, by the application's 16-byte ledger id — then the operation
/// sites, then the 32-byte durable-contract id closing the section. Each root
/// carries its ledger identity block (placement, product, and key ids plus one id
/// per record field). Every site is revalidated against the roots and record
/// types, every ledger id in the section must be pairwise distinct, and the
/// contract id is independently recomputed from the decoded graph and checked
/// against the carried bytes.
/// The decoded durable graph: the roots, the sealed operation sites, each site's
/// resolved graph-node path (parallel to the sites), the recomputed contract id, and
/// the canonical descriptor the paths and id were derived from.
type DecodedDurable = (
    Vec<DecodedRoot>,
    Vec<SealedSite>,
    Vec<SemanticPath>,
    DurableContractId,
    DurableContractDescriptor,
);

fn decode_durable(
    body: &[u8],
    strings: &[Rc<str>],
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
) -> Result<DecodedDurable, VerifyRejection> {
    let string_count = strings.len();
    let mut reader = Reader::new(body);
    let root_count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short root count"))? as usize;
    if root_count > marrow_image::bounds::MAX_ROOTS {
        return Err(reject(VerifyPhase::Table, "too many durable roots"));
    }
    let mut ledger_ids: Vec<LedgerIdBytes> = Vec::new();
    let application = if root_count > 0 {
        Some(take_distinct_id(
            &mut reader,
            &mut ledger_ids,
            "short application identity",
        )?)
    } else {
        None
    };
    let mut roots = Vec::with_capacity(root_count);
    for _ in 0..root_count {
        let name = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short root name"))?;
        if name as usize >= string_count {
            return Err(reject(VerifyPhase::Table, "root name index out of range"));
        }
        // The key tuple: a count, then each column's scalar type and distinct
        // ledger id. Zero columns is a singleton root; the closed orderable
        // durable-key scalar set (frozen at C04) admits int, string, bool, bytes,
        // date, and instant per column (`duration` is a span, not an identity).
        let key_count = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short root key count"))?
            as usize;
        if key_count > marrow_image::bounds::MAX_KEY_COLUMNS {
            return Err(reject(VerifyPhase::Table, "too many root key columns"));
        }
        let keys = decode_key_tuple(&mut reader, key_count, &mut ledger_ids)?;
        let record = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short root record"))?;
        if record as usize >= types.len() {
            return Err(reject(
                VerifyPhase::Table,
                "root record type index out of range",
            ));
        }
        let placement = take_distinct_id(&mut reader, &mut ledger_ids, "short placement identity")?;
        let product = take_distinct_id(&mut reader, &mut ledger_ids, "short product identity")?;
        // The resource's durable member tree: top-level fields interleaved with
        // static `group` namespaces and keyed `branch` placements. A field's stored
        // value is drawn from the closed acyclic durable value set (a bare scalar, a
        // dense struct, or a closed enum with sum/member ids).
        let mut member_budget = marrow_image::bounds::MAX_DURABLE_MEMBERS;
        let members = decode_members(&mut reader, 1, &mut member_budget, &mut ledger_ids)?;
        // The member tree's top-level fields and groups are exactly the materialized
        // record's stored field slots followed by its trailing group slots, in order and
        // value shape: this ties the durable identity to the executable record so a
        // hostile image cannot claim one identity while executing over a different field
        // or group shape. A field slot's value-shape match recurses through the record and
        // enum tables, so a widened field (a nominal, struct, or enum) is checked as
        // thoroughly as a plain scalar; each group slot is a group record whose own fields
        // tie to its `Group` member's direct fields one level down.
        let record_fields = &types[record as usize].fields;
        tie_root_record(record_fields, &members, types, enums)?;
        // Every keyed `branch` nested in the tree ties its own materialized record to
        // its direct field members the same way, one level down, so a hostile image
        // cannot claim a branch identity while executing over a different record shape.
        validate_branch_records(&members, types, enums, string_count)?;
        // The root's managed indexes follow its member tree. Each index's `Index`
        // ledger id is a distinct id across the whole table; each projected component
        // must reference a real top-level field or identity key of this same root, so a
        // hostile image cannot forge a projection over a leaf that does not exist.
        let indexes = decode_indexes(&mut reader, &keys, &members, &mut ledger_ids)?;
        roots.push(DecodedRoot {
            name,
            keys,
            record,
            placement,
            product,
            members,
            indexes,
        });
    }

    // A root's name keys its physical cell family, so two roots that resolve to the same
    // name would share one family — a later write to one silently overwriting the other.
    // The escape encoding is injective, so distinct name strings never collide physically;
    // reject only an image whose roots resolve to the same name string. Placement/product/
    // key ledger ids are already distinct across the table (`take_distinct_id`), so this
    // closes the one remaining cross-root physical-collision axis.
    for (i, root) in roots.iter().enumerate() {
        for other in &roots[..i] {
            if strings[root.name as usize] == strings[other.name as usize] {
                return Err(reject(VerifyPhase::Table, "two durable roots share a name"));
            }
        }
    }

    // Reconstruct the durable graph's node set now, from the same descriptor the
    // contract id is computed over, so every operation site resolves against this
    // verifier's own derivation of the graph rather than a compiler-side summary.
    let descriptor = durable_descriptor(application, &roots);
    let nodes = descriptor.semantic_nodes();

    let site_count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short site count"))? as usize;
    if site_count > marrow_image::bounds::MAX_SITES {
        return Err(reject(VerifyPhase::Table, "too many durable sites"));
    }
    let mut sites: Vec<SealedSite> = Vec::with_capacity(site_count);
    // Each site's resolved graph-node path, parallel to `sites` by index. The demand
    // reconstruction maps a durable opcode's site index to the semantic path of the
    // node it addresses; a flat site drops the path from its executable form, so it
    // is retained here rather than re-derived.
    let mut site_paths: Vec<SemanticPath> = Vec::with_capacity(site_count);
    for _ in 0..site_count {
        let (site, path) = decode_site(&mut reader, &nodes, &roots)?;
        // Sites are unique by their resolved identity: a flat site by (root, target),
        // a parked site by (path, target). Full structural equality covers both, and a
        // flat and a parked site can never collide.
        if sites.contains(&site) {
            return Err(reject(VerifyPhase::Table, "duplicate durable site"));
        }
        sites.push(site);
        site_paths.push(path);
    }

    // The section closes with the 32-byte durable-contract id. Recompute it
    // independently from the decoded graph — never trust the carried bytes — and
    // reject a mismatch, so a hostile image that mutates a root or field shape
    // without re-minting the contract is refused here.
    let carried: [u8; 32] = reader
        .take(32)
        .ok_or(reject(VerifyPhase::Table, "short durable contract id"))?
        .try_into()
        .expect("take(32) yields 32 bytes");
    if !reader.is_empty() {
        return Err(reject(
            VerifyPhase::Table,
            "trailing bytes in durable table",
        ));
    }
    let recomputed = descriptor.contract_id();
    if recomputed.bytes() != &carried {
        return Err(reject(
            VerifyPhase::Table,
            "durable contract id does not match the durable graph",
        ));
    }
    Ok((roots, sites, site_paths, recomputed, descriptor))
}

/// Decode one operation site — its semantic path then its target-kind byte — and
/// resolve it against the reconstructed node set. The path is `u8(step_count) ‖
/// [u8(ledger_kind) ‖ 16 id bytes]*`; the target byte is `0x00` whole-payload or
/// `0x01` field-leaf. Nothing here is trusted: the path is resolved to a node and
/// its kind cross-checked, and the executable physical facts are re-derived, so a
/// forged path, a flipped target byte, or a mutated ledger id is refused.
fn decode_site(
    reader: &mut Reader<'_>,
    nodes: &[SemanticNode],
    roots: &[DecodedRoot],
) -> Result<(SealedSite, SemanticPath), VerifyRejection> {
    let step_count = reader
        .u8()
        .ok_or(reject(VerifyPhase::Table, "short site path length"))? as usize;
    if step_count < marrow_image::bounds::MIN_SITE_PATH_STEPS {
        return Err(reject(
            VerifyPhase::Table,
            "durable site path names no graph node",
        ));
    }
    if step_count > marrow_image::bounds::MAX_SITE_PATH_STEPS {
        return Err(reject(VerifyPhase::Table, "durable site path too deep"));
    }
    let mut steps = Vec::with_capacity(step_count);
    for _ in 0..step_count {
        let kind_byte = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short site path step kind"))?;
        let kind = SemanticStepKind::from_ledger_kind(kind_byte)
            .ok_or(reject(VerifyPhase::Table, "unknown site path step kind"))?;
        let id_bytes: [u8; 16] = reader
            .take(16)
            .ok_or(reject(VerifyPhase::Table, "short site path step id"))?
            .try_into()
            .expect("take(16) yields 16 bytes");
        steps.push(SemanticStep::new(kind, LedgerIdBytes::from_bytes(id_bytes)));
    }
    let target = match reader
        .u8()
        .ok_or(reject(VerifyPhase::Table, "short site target"))?
    {
        0x00 => SemanticTarget::WholePayload,
        0x01 => SemanticTarget::FieldLeaf,
        0x02 => SemanticTarget::IndexScan,
        0x03 => SemanticTarget::IndexLookup,
        0x04 => SemanticTarget::GroupEntry,
        _ => return Err(reject(VerifyPhase::Table, "unknown site target tag")),
    };
    let site = resolve_site(&steps, target, nodes, roots)?;
    // The site's node path is the chain it resolved against — retained parallel to
    // the sealed site so demand reconstruction can name the node a flat site
    // addresses without re-deriving it from the executable form.
    Ok((site, SemanticPath::from_steps(steps)))
}

/// Resolve a decoded site path plus target kind to a [`SealedSite`]. A path that
/// names no reconstructed node, or a target whose kind disagrees with the resolved
/// node's kind, is refused. A whole-payload, keyed-branch-entry, or field-leaf site on
/// a flat-executable keyed root seals as [`SealedSite::Flat`] with its re-derived root
/// index and (for a field leaf) resolved field index — widened field values, composite
/// keys, and keyed branches nested to any depth all execute. Every other resolved site
/// — a singleton (keyless) root, a group-bearing root, or a managed-index read — seals
/// as [`SealedSite::Parked`], carrying the resolved path and target. Both forms
/// re-derive everything from the reconstructed graph, never trusting the image.
fn resolve_site(
    steps: &[SemanticStep],
    target: SemanticTarget,
    nodes: &[SemanticNode],
    roots: &[DecodedRoot],
) -> Result<SealedSite, VerifyRejection> {
    let node = nodes
        .iter()
        .find(|node| node.path.steps() == steps)
        .ok_or(reject(
            VerifyPhase::Table,
            "durable site path does not resolve to a graph node",
        ))?;
    // The target kind must agree with the resolved node's kind: a whole-payload
    // target names a keyed placement, a field-leaf target names a stored field, and an
    // index scan/lookup target names a managed index node.
    match (target, node.kind) {
        (SemanticTarget::WholePayload, SemanticNodeKind::Root | SemanticNodeKind::Branch) => {}
        (SemanticTarget::FieldLeaf, SemanticNodeKind::Field) => {}
        (SemanticTarget::GroupEntry, SemanticNodeKind::Group) => {}
        (SemanticTarget::IndexScan | SemanticTarget::IndexLookup, SemanticNodeKind::Index) => {}
        _ => {
            return Err(reject(
                VerifyPhase::Table,
                "durable site target kind does not match its resolved graph node",
            ));
        }
    }
    // An index read site resolves to its managed index and seals parked (an index node
    // is never a flat-executable node; runtime traversal/lookup lands at E05). The
    // read kind must agree with the index's `unique` flag: a nonunique index admits
    // only a progressive-prefix `IndexScan`, and a unique index admits only a
    // complete-key `IndexLookup`. This is where a site that claims to *traverse* a
    // unique index — or to exact-lookup a nonunique one — is refused, so source can
    // never observe siblings through a unique index.
    if matches!(
        target,
        SemanticTarget::IndexScan | SemanticTarget::IndexLookup
    ) {
        let placement = steps[1].id;
        let root_pos = roots
            .iter()
            .position(|root| root.placement == placement)
            .ok_or(reject(
                VerifyPhase::Table,
                "durable index site path is not rooted at a durable root",
            ))?;
        let root = &roots[root_pos];
        let index_id = steps.last().expect("an index path has an index step").id;
        let local = root
            .indexes
            .iter()
            .position(|index| index.id == index_id)
            .ok_or(reject(
                VerifyPhase::Table,
                "durable index site names no managed index of its root",
            ))?;
        let index = &root.indexes[local];
        let agrees = match target {
            SemanticTarget::IndexScan => !index.unique,
            SemanticTarget::IndexLookup => index.unique,
            _ => unreachable!("guarded to index targets"),
        };
        if !agrees {
            return Err(reject(
                VerifyPhase::Table,
                "durable index site read kind disagrees with the index's unique flag",
            ));
        }
        // The index's position in the image-wide index table, assembled by iterating the
        // roots in order and each root's indexes in order (the same order used when the
        // sealed index list is built), so this position indexes that list directly.
        let global: usize = roots[..root_pos]
            .iter()
            .map(|root| root.indexes.len())
            .sum::<usize>()
            + local;
        let global = global as u16;
        let sealed_target = match target {
            SemanticTarget::IndexScan => SealedSiteTarget::IndexScan(global),
            SemanticTarget::IndexLookup => SealedSiteTarget::IndexLookup(global),
            _ => unreachable!("guarded to index targets"),
        };
        return Ok(SealedSite::Flat {
            root: root_pos as u16,
            target: sealed_target,
        });
    }
    // Every node carries its enclosing root's placement as its second step, so the
    // root index is that placement's position. A flat-executable keyed root — keyed, with
    // every member a field or a simple keyed branch (no group at any level) — is
    // kernel-executable: a whole-payload or keyed-branch-entry site, or a field-leaf site
    // (scalar or widened value), at any branch depth. A site on a non-flat root — a
    // singleton, or a group at any level — seals as parked (identity complete, execution
    // deferred).
    let placement = steps[1].id;
    let root_index = roots
        .iter()
        .position(|root| root.placement == placement)
        .ok_or(reject(
            VerifyPhase::Table,
            "durable site path is not rooted at a durable root",
        ))? as u16;
    let root = &roots[root_index as usize];
    let parked = || SealedSite::Parked {
        path: SemanticPath::from_steps(steps.to_vec()),
        target,
    };
    if !is_flat_executable_root(root) {
        return Ok(parked());
    }
    // The root is flat-executable, so every intermediate placement step below the root is
    // a keyed-branch placement (no groups on the flat path). `steps[2..]` are the branch
    // placements from the root down; a field target's last step is the field id.
    let below_root = &steps[marrow_image::bounds::MIN_SITE_PATH_STEPS..];
    let sealed = match target {
        SemanticTarget::WholePayload => match node.kind {
            // The root's own whole entry: exactly the two root steps.
            SemanticNodeKind::Root => {
                if !below_root.is_empty() {
                    return Ok(parked());
                }
                SealedSite::Flat {
                    root: root_index,
                    target: SealedSiteTarget::WholePayload,
                }
            }
            // A keyed branch entry at any depth: every step below the root is a branch
            // placement. Walk the placement chain through the recursive member tree into a
            // per-level branch path; a step that names no branch at its level parks.
            SemanticNodeKind::Branch => match walk_branch_path(&root.members, below_root) {
                Some((path, _)) => SealedSite::Flat {
                    root: root_index,
                    target: SealedSiteTarget::BranchEntry(path.into()),
                },
                None => parked(),
            },
            _ => unreachable!("a whole-payload target resolved to a root or branch node"),
        },
        SemanticTarget::FieldLeaf => {
            // The last step is the field id; the steps before it are the branch placements
            // from the root down to the field's containing node (empty for a top-level
            // field). Walk the branch chain, then resolve the field within the reached
            // node's own members.
            let Some((&field_step, branch_steps)) = below_root.split_last() else {
                return Ok(parked());
            };
            match walk_branch_path(&root.members, branch_steps) {
                Some((path, node_members)) => {
                    match top_level_field_index(node_members, field_step.id) {
                        Some(field) if path.is_empty() => SealedSite::Flat {
                            root: root_index,
                            target: SealedSiteTarget::FieldLeaf(field),
                        },
                        Some(field) => SealedSite::Flat {
                            root: root_index,
                            target: SealedSiteTarget::BranchField {
                                branch: path.into(),
                                field,
                            },
                        },
                        None => parked(),
                    }
                }
                None => parked(),
            }
        }
        // A root-level unkeyed group is addressed by exactly one group step below the
        // root; the flat kernel serves only root-level groups, so a group nested in a
        // branch or another group (more or fewer steps, or a non-root-level group id)
        // parks.
        SemanticTarget::GroupEntry => match below_root {
            [group_step] => match root_group_index(&root.members, group_step.id) {
                Some(group) => SealedSite::Flat {
                    root: root_index,
                    target: SealedSiteTarget::GroupEntry(group),
                },
                None => parked(),
            },
            _ => parked(),
        },
        // Index scan/lookup targets returned parked above, before the flat/field logic.
        SemanticTarget::IndexScan | SemanticTarget::IndexLookup => {
            unreachable!("index read targets are sealed and returned before this point")
        }
    };
    Ok(sealed)
}

/// Whether a decoded root is the flat keyed root the kernel executes: at least one key
/// column and a member tree of top-level storable-value fields (scalar or widened) and
/// keyed branches of the same shape (no group). The key may be single-column or a composite
/// tuple, at the root and at every branch. Re-derived from the decoded graph, so the
/// flat/parked classification never trusts a compiler summary.
fn is_flat_executable_root(root: &DecodedRoot) -> bool {
    !root.keys.is_empty() && root.members.iter().all(member_flat_at_root)
}

/// Whether a root's *direct* member keeps the root flat-executable. It admits one more
/// shape than [`DecodedMember::keeps_root_flat`]: a root-level unkeyed `group` whose own
/// members are all storable-value fields (a scalar or widened composite). A group is a
/// value unit of the root entry, executable at the root level, but a group nested in a
/// branch or in another group still parks — [`keeps_root_flat`] (used for branch
/// members) keeps `Group => false`, so a group below the root's direct members never
/// makes its enclosing branch flat.
fn member_flat_at_root(member: &DecodedMember) -> bool {
    match member {
        DecodedMember::Field { .. } => true,
        DecodedMember::Group { members, .. } => members
            .iter()
            .all(|m| matches!(m, DecodedMember::Field { .. })),
        DecodedMember::Branch { .. } => member.is_simple_branch(),
    }
}

/// The index of the root-level unkeyed group with ledger id `id` among a member tree's
/// direct `Group` members, in declaration order. `None` when no direct group carries the
/// id — a nested or in-branch group is not a root-level group node.
fn root_group_index(members: &[DecodedMember], id: LedgerIdBytes) -> Option<u16> {
    members
        .iter()
        .filter(|member| matches!(member, DecodedMember::Group { .. }))
        .position(|member| matches!(member, DecodedMember::Group { id: gid, .. } if *gid == id))
        .map(|position| position as u16)
}

/// Seal a member tree's keyed branches into the recursive [`SealedBranch`] tree, in
/// declaration order, so a [`SealedSiteTarget::BranchEntry`] branch path indexes it level
/// by level. Called only for a flat-executable root, so every branch is a scalar-field
/// keyed branch (its `keys` are its ordered key columns) and its own members recurse
/// through the same rule.
fn seal_branches(members: &[DecodedMember], strings: &[Rc<str>]) -> Vec<SealedBranch> {
    members
        .iter()
        .filter_map(|member| match member {
            DecodedMember::Branch {
                name,
                record,
                keys,
                members,
                ..
            } => Some(SealedBranch {
                name: strings[*name as usize].clone(),
                keys: keys.iter().map(|(scalar, _)| *scalar).collect(),
                record: *record,
                branches: seal_branches(members, strings),
            }),
            _ => None,
        })
        .collect()
}

/// Seal a flat-executable root's root-level unkeyed groups into [`SealedGroup`]s, in
/// declaration order, so a [`SealedSiteTarget::GroupEntry`] group index selects one.
/// Each group's name and materialized record come from the root's own record: the
/// verifier's record↔member tie (validated in the table phase) places one trailing
/// group slot per `Group` member, after the leading scalar/widened field slots, in
/// declaration order — so the group slot at `field_count + ordinal` is exactly this
/// group's slot. Called only for a flat-executable root, whose groups are all
/// storable-value-field groups.
fn seal_groups(root: &DecodedRoot, types: &[SealedRecordType]) -> Vec<SealedGroup> {
    let record = &types[root.record as usize];
    let field_count = root
        .members
        .iter()
        .filter(|member| matches!(member, DecodedMember::Field { .. }))
        .count();
    root.members
        .iter()
        .filter(|member| matches!(member, DecodedMember::Group { .. }))
        .enumerate()
        .map(|(ordinal, _group)| {
            let slot = &record.fields[field_count + ordinal];
            let record = match slot.ty {
                ImageType::Record { idx, .. } => idx,
                _ => unreachable!("the record↔member tie places a Record slot per group member"),
            };
            SealedGroup {
                name: slot.name.clone(),
                record,
            }
        })
        .collect()
}

/// The index of the top-level field with ledger id `field_id` among a root's member
/// tree, counting only its direct field members in declaration order. This is the
/// field's index into the root's materialized record (their orders are tied during
/// root decode), so a resolved field-leaf site addresses the same field the record
/// types.
fn top_level_field_index(members: &[DecodedMember], field_id: LedgerIdBytes) -> Option<u16> {
    members
        .iter()
        .filter_map(|member| match member {
            DecodedMember::Field { id, .. } => Some(*id),
            _ => None,
        })
        .position(|id| id == field_id)
        .map(|index| index as u16)
}

/// Resolve an index's ledger-id projection to record/key positions the path kernel
/// maintains, against the same decoded root the components were re-resolved against in
/// `decode_indexes`. A field component names its position in the root's materialized
/// record (tied to the durable member order); a key component names its column in the
/// root's key tuple. Every component already resolved to a real leaf during decode, so a
/// miss here is an internal inconsistency the verifier refuses rather than mis-addressing
/// a maintained index cell.
fn resolve_index_projection(
    root: &DecodedRoot,
    components: &[DurableIndexComponent],
) -> Result<Vec<SealedIndexComponent>, VerifyRejection> {
    components
        .iter()
        .map(|component| match component {
            DurableIndexComponent::Field(id) => top_level_field_index(&root.members, *id)
                .map(SealedIndexComponent::Field)
                .ok_or(reject(
                    VerifyPhase::Table,
                    "durable index field component resolves to no record position",
                )),
            DurableIndexComponent::Key(id) => root
                .keys
                .iter()
                .position(|(_, key_id)| key_id == id)
                .map(|column| SealedIndexComponent::Key(column as u16))
                .ok_or(reject(
                    VerifyPhase::Table,
                    "durable index key component resolves to no key column",
                )),
        })
        .collect()
}

/// Walk a chain of branch placement steps through a member tree, accumulating the
/// per-level branch index at each hop and descending into that branch's own members. The
/// returned path indexes the recursive sealed branch tree level by level, and the returned
/// member slice is the deepest reached node's own members (the whole tree when the chain is
/// empty), against which a field leaf resolves. `None` when a step names no branch at its
/// level — a group-scoped or otherwise non-branch step parks rather than mis-resolving.
/// Only branch steps appear here on the flat-executable path (no groups), so a resolved
/// walk is a pure branch chain.
fn walk_branch_path<'a>(
    mut members: &'a [DecodedMember],
    steps: &[SemanticStep],
) -> Option<(Vec<u16>, &'a [DecodedMember])> {
    let mut path = Vec::with_capacity(steps.len());
    for step in steps {
        let index = branch_index(members, step.id)?;
        path.push(index);
        members = members.iter().find_map(|member| match member {
            DecodedMember::Branch {
                placement, members, ..
            } if *placement == step.id => Some(members.as_slice()),
            _ => None,
        })?;
    }
    Some((path, members))
}

/// The index of the keyed `branch` with placement id `placement` among a root's
/// declaration-ordered branch members. This is the index into the root's sealed
/// branch list (both count only the direct branch members, in order), so a resolved
/// branch-entry site addresses the same branch the schema derives.
fn branch_index(members: &[DecodedMember], placement_id: LedgerIdBytes) -> Option<u16> {
    members
        .iter()
        .filter_map(|member| match member {
            DecodedMember::Branch { placement, .. } => Some(*placement),
            _ => None,
        })
        .position(|id| id == placement_id)
        .map(|index| index as u16)
}

/// Read one 16-byte ledger id, rejecting a duplicate against those already seen in
/// this durable table. Entropy-minted ids are distinct by construction, so two
/// equal ids are a forged or corrupted identity block.
fn take_distinct_id(
    reader: &mut Reader<'_>,
    seen: &mut Vec<LedgerIdBytes>,
    what: &'static str,
) -> Result<LedgerIdBytes, VerifyRejection> {
    let bytes: [u8; 16] = reader
        .take(16)
        .ok_or(reject(VerifyPhase::Table, what))?
        .try_into()
        .expect("take(16) yields 16 bytes");
    let id = LedgerIdBytes::from_bytes(bytes);
    if seen.contains(&id) {
        return Err(reject(VerifyPhase::Table, "duplicate durable ledger id"));
    }
    seen.push(id);
    Ok(id)
}

/// Decode a placement key tuple: `count` columns, each a bare orderable durable-key
/// scalar and a distinct ledger id. Shared by roots and branches; the caller has
/// already validated `count` against `MAX_KEY_COLUMNS`.
fn decode_key_tuple(
    reader: &mut Reader<'_>,
    count: usize,
    seen: &mut Vec<LedgerIdBytes>,
) -> Result<Vec<(Scalar, LedgerIdBytes)>, VerifyRejection> {
    let mut keys = Vec::with_capacity(count);
    for _ in 0..count {
        let key_tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short key type"))?;
        let scalar = match decode_bare_scalar(key_tag) {
            Some(
                scalar @ (Scalar::Int
                | Scalar::Text
                | Scalar::Bool
                | Scalar::Bytes
                | Scalar::Date
                | Scalar::Instant),
            ) => scalar,
            _ => {
                return Err(reject(
                    VerifyPhase::Table,
                    "key type must be an orderable durable-key scalar",
                ));
            }
        };
        let key_id = take_distinct_id(reader, seen, "short key identity")?;
        keys.push((scalar, key_id));
    }
    Ok(keys)
}

/// Tie a root's group-inclusive materialized record to its durable member tree. The
/// record's slots run in the member tree's own top-level order with keyed branches
/// dropped: each `Field` member matches the next slot by value shape and required flag,
/// and each `Group` member matches the next slot — a bare group record — by tying its own
/// fields to the group's direct fields one level down. A slot count that disagrees, a
/// group slot that is not a group record, or any field mismatch is refused, so a hostile
/// image cannot claim one identity while executing over a different field or group shape.
///
/// Field slots precede group slots: a `Field` member after any `Group` member is refused,
/// so the record's leading scalar/widened field slots and its trailing group slots occupy
/// disjoint contiguous ranges. Sealing relies on this — a group's slot is `field_count +
/// ordinal` — so the fields-first invariant is verifier-enforced here rather than trusted
/// from the compiler.
fn tie_root_record(
    record_fields: &[DecodedField],
    members: &[DecodedMember],
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
) -> Result<(), VerifyRejection> {
    let mut slots = record_fields.iter();
    let mut seen_group = false;
    for member in members {
        match member {
            DecodedMember::Field {
                value, required, ..
            } => {
                if seen_group {
                    return Err(reject(
                        VerifyPhase::Table,
                        "root member tree places a field after a group",
                    ));
                }
                let Some(slot) = slots.next() else {
                    return Err(reject(
                        VerifyPhase::Table,
                        "root member tree has more top-level members than the record",
                    ));
                };
                if *required != slot.required || !value_shape_matches(value, slot.ty, types, enums)
                {
                    return Err(reject(
                        VerifyPhase::Table,
                        "root member tree fields do not match the record fields",
                    ));
                }
            }
            DecodedMember::Group {
                members: group_members,
                ..
            } => {
                seen_group = true;
                let Some(slot) = slots.next() else {
                    return Err(reject(
                        VerifyPhase::Table,
                        "root member tree has more top-level members than the record",
                    ));
                };
                tie_group_slot(slot, group_members, types, enums)?;
            }
            // A keyed branch is a distinct durable node, not a materialized record slot.
            DecodedMember::Branch { .. } => {}
        }
    }
    if slots.next().is_some() {
        return Err(reject(
            VerifyPhase::Table,
            "root member tree has fewer top-level members than the record",
        ));
    }
    Ok(())
}

/// Tie one trailing group slot of a root record to its `Group` member: the slot is a
/// bare group record whose fields match the member's direct `Field` members by value
/// shape and required flag, one level down — the same field tie the root and a branch
/// apply. A group holds only leaf fields on the executable line, so a non-record slot,
/// an optional record slot, an out-of-range record index, or a field/member mismatch is
/// refused.
fn tie_group_slot(
    slot: &DecodedField,
    group_members: &[DecodedMember],
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
) -> Result<(), VerifyRejection> {
    let ImageType::Record { idx, optional } = slot.ty else {
        return Err(reject(
            VerifyPhase::Table,
            "a root group slot is not a group record",
        ));
    };
    if optional {
        return Err(reject(
            VerifyPhase::Table,
            "a root group slot must be a bare group record",
        ));
    }
    if idx as usize >= types.len() {
        return Err(reject(
            VerifyPhase::Table,
            "root group slot record index out of range",
        ));
    }
    let group_fields = &types[idx as usize].fields;
    let mut direct_fields = group_members.iter().filter_map(|member| match member {
        DecodedMember::Field {
            value, required, ..
        } => Some((value, *required)),
        _ => None,
    });
    for field in group_fields {
        match direct_fields.next() {
            Some((value, member_required))
                if member_required == field.required
                    && value_shape_matches(value, field.ty, types, enums) => {}
            _ => {
                return Err(reject(
                    VerifyPhase::Table,
                    "group member tree fields do not match its record fields",
                ));
            }
        }
    }
    if direct_fields.next().is_some() {
        return Err(reject(
            VerifyPhase::Table,
            "group member tree has more direct fields than its record",
        ));
    }
    Ok(())
}

/// Validate every keyed `branch` in a decoded member tree: its surface name and
/// materialized record type indices are in range, and its record's fields match its
/// own direct scalar field members in order, value shape, and required flag — the
/// same tie the root's record has to its member tree, one level down. Recurses
/// through groups and branches. The name and record are surface (not identity), so
/// this is the only place they are checked; a hostile image that names a branch
/// record disagreeing with the branch's field shapes is refused here.
fn validate_branch_records(
    members: &[DecodedMember],
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
    string_count: usize,
) -> Result<(), VerifyRejection> {
    for member in members {
        match member {
            DecodedMember::Field { .. } => {}
            DecodedMember::Group { members, .. } => {
                validate_branch_records(members, types, enums, string_count)?;
            }
            DecodedMember::Branch {
                name,
                record,
                members,
                ..
            } => {
                if *name as usize >= string_count {
                    return Err(reject(VerifyPhase::Table, "branch name index out of range"));
                }
                if *record as usize >= types.len() {
                    return Err(reject(
                        VerifyPhase::Table,
                        "branch record type index out of range",
                    ));
                }
                let record_fields = &types[*record as usize].fields;
                let mut direct_fields = members.iter().filter_map(|member| match member {
                    DecodedMember::Field {
                        value, required, ..
                    } => Some((value, *required)),
                    _ => None,
                });
                for field in record_fields {
                    match direct_fields.next() {
                        Some((value, member_required))
                            if member_required == field.required
                                && value_shape_matches(value, field.ty, types, enums) => {}
                        _ => {
                            return Err(reject(
                                VerifyPhase::Table,
                                "branch member tree fields do not match its record fields",
                            ));
                        }
                    }
                }
                if direct_fields.next().is_some() {
                    return Err(reject(
                        VerifyPhase::Table,
                        "branch member tree has more direct fields than its record",
                    ));
                }
                validate_branch_records(members, types, enums, string_count)?;
            }
        }
    }
    Ok(())
}

/// Decode a durable member tree: `u16(count) ‖ member*`. A field is tag `0x00`; a
/// group is tag `0x01`; a branch is tag `0x02`. `budget` bounds the total member
/// records across the whole tree and `depth` bounds nesting, so a hostile image
/// cannot drive unbounded recursion or allocation before the bounds are rechecked
/// (§ law 9). Every ledger id is distinct across the table.
fn decode_members(
    reader: &mut Reader<'_>,
    depth: usize,
    budget: &mut usize,
    seen: &mut Vec<LedgerIdBytes>,
) -> Result<Vec<DecodedMember>, VerifyRejection> {
    if depth > marrow_image::bounds::MAX_DURABLE_DEPTH {
        return Err(reject(VerifyPhase::Table, "durable member tree too deep"));
    }
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short durable member count"))? as usize;
    let mut members = Vec::with_capacity(count.min(*budget));
    for _ in 0..count {
        if *budget == 0 {
            return Err(reject(VerifyPhase::Table, "too many durable members"));
        }
        *budget -= 1;
        let tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short durable member tag"))?;
        let member = match tag {
            0x00 => {
                let id = take_distinct_id(reader, seen, "short durable field identity")?;
                let required = match reader.u8().ok_or(reject(
                    VerifyPhase::Table,
                    "short durable field required flag",
                ))? {
                    0 => false,
                    1 => true,
                    _ => {
                        return Err(reject(
                            VerifyPhase::Table,
                            "durable field required flag must be 0 or 1",
                        ));
                    }
                };
                let value = decode_value_shape(reader, 1, seen)?;
                DecodedMember::Field {
                    id,
                    required,
                    value,
                }
            }
            0x01 => {
                let id = take_distinct_id(reader, seen, "short durable group identity")?;
                let inner = decode_members(reader, depth + 1, budget, seen)?;
                DecodedMember::Group { id, members: inner }
            }
            0x02 => {
                let placement = take_distinct_id(reader, seen, "short durable branch identity")?;
                // The branch's surface name and materialized record type index follow
                // the placement. Their ranges (against the string and type tables) and
                // the record/member-field alignment are checked in
                // `validate_branch_records`, where the type and enum tables are in scope.
                let name = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, "short durable branch name"))?;
                let record = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, "short durable branch record"))?;
                let key_count = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, "short branch key count"))?
                    as usize;
                if key_count > marrow_image::bounds::MAX_KEY_COLUMNS {
                    return Err(reject(VerifyPhase::Table, "too many branch key columns"));
                }
                let keys = decode_key_tuple(reader, key_count, seen)?;
                let inner = decode_members(reader, depth + 1, budget, seen)?;
                DecodedMember::Branch {
                    placement,
                    name,
                    record,
                    keys,
                    members: inner,
                }
            }
            _ => return Err(reject(VerifyPhase::Table, "unknown durable member tag")),
        };
        members.push(member);
    }
    Ok(members)
}

/// Decode a root's managed indexes: `u16(count) ‖ index*`. Each index is its distinct
/// `Index` ledger id, a `unique` flag byte, a `u16(component_count)`, and per component
/// a one-byte leaf kind (`0x02` field, `0x04` key) and the leaf's 16-byte ledger id.
/// Every component id is re-resolved against this root's own top-level field ids
/// (kind `0x02`) or identity key ids (kind `0x04`), so a projection over a leaf that
/// does not exist on the root is refused. The index id is distinct across the whole
/// durable table (via `seen`); component ids are references to already-seen leaf ids
/// and so are not added to `seen`.
fn decode_indexes(
    reader: &mut Reader<'_>,
    keys: &[(Scalar, LedgerIdBytes)],
    members: &[DecodedMember],
    seen: &mut Vec<LedgerIdBytes>,
) -> Result<Vec<DecodedIndex>, VerifyRejection> {
    let field_ids: Vec<LedgerIdBytes> = members
        .iter()
        .filter_map(|member| match member {
            DecodedMember::Field { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    // A managed-index field component must project one of the compiler's closed set of
    // orderable durable-key scalar shapes. Field executability is independent: Duration
    // and widened values can be stored but are not index-eligible.
    let index_eligible_field_ids: Vec<LedgerIdBytes> = members
        .iter()
        .filter_map(|member| match member {
            DecodedMember::Field { id, value, .. } => match value {
                DurableValueShape::Scalar(
                    Scalar::Int
                    | Scalar::Text
                    | Scalar::Bool
                    | Scalar::Bytes
                    | Scalar::Date
                    | Scalar::Instant,
                ) => Some(*id),
                DurableValueShape::Scalar(Scalar::Duration)
                | DurableValueShape::Struct(_)
                | DurableValueShape::Enum { .. } => None,
            },
            DecodedMember::Group { .. } | DecodedMember::Branch { .. } => None,
        })
        .collect();
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short durable index count"))? as usize;
    if count > marrow_image::bounds::MAX_INDEXES {
        return Err(reject(VerifyPhase::Table, "too many durable indexes"));
    }
    let mut indexes = Vec::with_capacity(count);
    for _ in 0..count {
        let id = take_distinct_id(reader, seen, "short durable index identity")?;
        let unique = match reader.u8().ok_or(reject(
            VerifyPhase::Table,
            "short durable index unique flag",
        ))? {
            0 => false,
            1 => true,
            _ => {
                return Err(reject(
                    VerifyPhase::Table,
                    "durable index unique flag must be 0 or 1",
                ));
            }
        };
        let component_count = reader.u16().ok_or(reject(
            VerifyPhase::Table,
            "short durable index component count",
        ))? as usize;
        if component_count > marrow_image::bounds::MAX_INDEX_COMPONENTS {
            return Err(reject(
                VerifyPhase::Table,
                "too many durable index components",
            ));
        }
        let mut components = Vec::with_capacity(component_count);
        for _ in 0..component_count {
            let kind = reader.u8().ok_or(reject(
                VerifyPhase::Table,
                "short durable index component kind",
            ))?;
            let leaf: [u8; 16] = reader
                .take(16)
                .ok_or(reject(
                    VerifyPhase::Table,
                    "short durable index component identity",
                ))?
                .try_into()
                .expect("take(16) yields 16 bytes");
            let leaf = LedgerIdBytes::from_bytes(leaf);
            let component = match kind {
                0x02 => {
                    if !index_eligible_field_ids.contains(&leaf) {
                        return Err(reject(
                            VerifyPhase::Table,
                            if field_ids.contains(&leaf) {
                                "durable index field component names a field that is not \
                                 index-eligible"
                            } else {
                                "durable index field component names no top-level field of its root"
                            },
                        ));
                    }
                    DurableIndexComponent::Field(leaf)
                }
                0x04 => {
                    if !keys.iter().any(|(_, key_id)| *key_id == leaf) {
                        return Err(reject(
                            VerifyPhase::Table,
                            "durable index key component names no identity key of its root",
                        ));
                    }
                    DurableIndexComponent::Key(leaf)
                }
                _ => {
                    return Err(reject(
                        VerifyPhase::Table,
                        "unknown durable index component kind",
                    ));
                }
            };
            components.push(component);
        }
        // Re-enforce projection well-formedness the compiler owns: a reference-valid but
        // malformed projection (an empty projection, a repeated component, or a
        // non-unique index whose identity suffix is missing, misordered, or preceded by a
        // key) must never reach the sealed index model the runtime trusts to order rows.
        if let Err(detail) = validate_index_projection(unique, &components, keys) {
            return Err(reject(VerifyPhase::Table, detail));
        }
        indexes.push(DecodedIndex {
            id,
            unique,
            components,
        });
    }
    Ok(indexes)
}

/// Re-check one decoded index's projection against the closed well-formedness rules the
/// compiler owns, so a hostile image cannot smuggle a malformed projection past the
/// verifier. Every component id is already re-resolved to a real scalar field or identity
/// key of the root (the orderable-key predicate); this owns the ordering and cardinality
/// rules: the projection is non-empty, no component repeats, and a non-unique index ends
/// with exactly the identity keys in declaration order — the row-distinguishing suffix. A
/// unique index carries no suffix obligation. Returns a static detail describing the first
/// violation.
///
/// The no-leading-key rule (a non-unique index carries no identity key before its suffix)
/// needs no separate branch: distinctness forbids any component from repeating, and the
/// suffix must already hold every identity key, so a leading identity key would duplicate
/// a suffix key and is rejected by the distinctness check.
fn validate_index_projection(
    unique: bool,
    components: &[DurableIndexComponent],
    keys: &[(Scalar, LedgerIdBytes)],
) -> Result<(), &'static str> {
    if components.is_empty() {
        return Err("durable index has an empty projection");
    }
    for (position, component) in components.iter().enumerate() {
        if components[..position]
            .iter()
            .any(|earlier| earlier.id() == component.id())
        {
            return Err("durable index repeats a projection component");
        }
    }
    if !unique {
        // The trailing `keys.len()` components must be exactly the identity keys in
        // declaration order.
        if components.len() < keys.len() {
            return Err("non-unique durable index does not end with the identity suffix");
        }
        let suffix_start = components.len() - keys.len();
        for (offset, (_, key_id)) in keys.iter().enumerate() {
            match components[suffix_start + offset] {
                DurableIndexComponent::Key(id) if id == *key_id => {}
                _ => {
                    return Err(
                        "non-unique durable index does not end with the identity keys in \
                         declaration order",
                    );
                }
            }
        }
    }
    Ok(())
}

/// Decode a durable field's stored value shape: `u8(value_tag) ‖ body`. A scalar is
/// tag `0x00` (a bare scalar); a dense struct is tag `0x01` (`u16(count) ‖ value*`);
/// a closed enum is tag `0x02` (`sum id ‖ u16(count) ‖ [member id ‖ u16(payload) ‖
/// value*]*`). Every sum and member id is a distinct ledger id across the durable
/// table (via `seen`). `depth` bounds nesting so a hostile image cannot drive
/// unbounded recursion before the value shape is rechecked (§ law 9).
fn decode_value_shape(
    reader: &mut Reader<'_>,
    depth: usize,
    seen: &mut Vec<LedgerIdBytes>,
) -> Result<DurableValueShape, VerifyRejection> {
    if depth > marrow_image::bounds::MAX_DURABLE_VALUE_DEPTH {
        return Err(reject(
            VerifyPhase::Table,
            "durable field value shape too deep",
        ));
    }
    let tag = reader
        .u8()
        .ok_or(reject(VerifyPhase::Table, "short durable value tag"))?;
    match tag {
        0x00 => {
            let scalar_tag = reader
                .u8()
                .ok_or(reject(VerifyPhase::Table, "short durable value scalar"))?;
            let scalar = decode_bare_scalar(scalar_tag).ok_or(reject(
                VerifyPhase::Table,
                "durable value scalar must be a bare scalar",
            ))?;
            Ok(DurableValueShape::Scalar(scalar))
        }
        0x01 => {
            let count = reader.u16().ok_or(reject(
                VerifyPhase::Table,
                "short durable struct leaf count",
            ))? as usize;
            if count > marrow_image::bounds::MAX_STRUCT_LEAVES {
                return Err(reject(VerifyPhase::Table, "too many durable struct leaves"));
            }
            let mut leaves = Vec::with_capacity(count);
            for _ in 0..count {
                leaves.push(decode_value_shape(reader, depth + 1, seen)?);
            }
            Ok(DurableValueShape::Struct(leaves))
        }
        0x02 => {
            let sum = take_distinct_id(reader, seen, "short durable enum sum identity")?;
            let member_count = reader.u16().ok_or(reject(
                VerifyPhase::Table,
                "short durable enum member count",
            ))? as usize;
            if member_count > marrow_image::bounds::MAX_VARIANTS {
                return Err(reject(VerifyPhase::Table, "too many durable enum members"));
            }
            let mut members = Vec::with_capacity(member_count);
            for _ in 0..member_count {
                let id = take_distinct_id(reader, seen, "short durable enum member identity")?;
                let payload_count = reader.u16().ok_or(reject(
                    VerifyPhase::Table,
                    "short durable enum member payload count",
                ))? as usize;
                if payload_count > marrow_image::bounds::MAX_PAYLOAD_FIELDS {
                    return Err(reject(
                        VerifyPhase::Table,
                        "too many durable enum member payload leaves",
                    ));
                }
                let mut payload = Vec::with_capacity(payload_count);
                for _ in 0..payload_count {
                    payload.push(decode_value_shape(reader, depth + 1, seen)?);
                }
                members.push(DurableEnumMemberShape { id, payload });
            }
            Ok(DurableValueShape::Enum { sum, members })
        }
        _ => Err(reject(VerifyPhase::Table, "unknown durable value tag")),
    }
}

/// Whether a decoded durable field value shape structurally matches the materialized
/// record field type it claims, recursing through the record and enum tables. The
/// ledger ids a value shape carries (a struct records none; an enum a sum and per-
/// member id) are durable identity, verified by pairwise distinctness and the
/// contract-id recomputation — this match ties the *structure* to the executable
/// record so a hostile image cannot claim one durable identity while its record
/// carries a different value shape. A nominal field erases to its base scalar, so it
/// matches a bare scalar exactly like a plain scalar field.
fn value_shape_matches(
    shape: &DurableValueShape,
    ty: ImageType,
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
) -> bool {
    match (shape, ty) {
        (
            DurableValueShape::Scalar(shape_scalar),
            ImageType::Scalar {
                scalar,
                optional: false,
            },
        ) => *shape_scalar == scalar,
        (
            DurableValueShape::Struct(leaves),
            ImageType::Record {
                idx,
                optional: false,
            },
        ) => {
            let Some(record) = types.get(idx as usize) else {
                return false;
            };
            // A durable struct value is dense: every leaf is a required bare field,
            // matched positionally.
            record.fields.len() == leaves.len()
                && record.fields.iter().zip(leaves).all(|(field, leaf)| {
                    field.required && value_shape_matches(leaf, field.ty, types, enums)
                })
        }
        (
            DurableValueShape::Enum { members, .. },
            ImageType::Enum {
                idx,
                optional: false,
            },
        ) => {
            let Some(enum_def) = enums.get(idx as usize) else {
                return false;
            };
            enum_def.variants.len() == members.len()
                && enum_def
                    .variants
                    .iter()
                    .zip(members)
                    .all(|(variant, member)| {
                        variant.payload.len() == member.payload.len()
                            && variant
                                .payload
                                .iter()
                                .zip(&member.payload)
                                .all(|(leaf_ty, leaf)| {
                                    value_shape_matches(leaf, *leaf_ty, types, enums)
                                })
                    })
        }
        _ => false,
    }
}

/// Rebuild the canonical durable-graph descriptor from the decoded tables. This is
/// the verifier's independent reconstruction: it shares the canonical encoding owned
/// by `marrow-image` but reads only the decoded application id, roots, key tuples,
/// and member trees, so the recomputed id depends on nothing the compiler asserted
/// directly.
fn durable_descriptor(
    application: Option<LedgerIdBytes>,
    roots: &[DecodedRoot],
) -> DurableContractDescriptor {
    let Some(application) = application else {
        return DurableContractDescriptor::empty();
    };
    let shapes = roots
        .iter()
        .map(|root| DurableRootShape {
            placement: root.placement,
            product: root.product,
            keys: key_shapes(&root.keys),
            members: member_shapes(&root.members),
            indexes: index_shapes(&root.indexes),
        })
        .collect();
    DurableContractDescriptor::new(application, shapes)
}

/// The descriptor index shapes for a decoded root's managed indexes.
fn index_shapes(indexes: &[DecodedIndex]) -> Vec<DurableIndexShape> {
    indexes
        .iter()
        .map(|index| DurableIndexShape {
            id: index.id,
            unique: index.unique,
            components: index.components.clone(),
        })
        .collect()
}

/// The descriptor key-tuple shapes for a decoded placement's key columns.
fn key_shapes(keys: &[(Scalar, LedgerIdBytes)]) -> Vec<DurableKeyShape> {
    keys.iter()
        .map(|(scalar, id)| DurableKeyShape {
            scalar: *scalar,
            id: *id,
        })
        .collect()
}

/// Convert a decoded member tree into the descriptor's member shapes, recursing
/// through groups and branches.
fn member_shapes(members: &[DecodedMember]) -> Vec<DurableMemberShape> {
    members
        .iter()
        .map(|member| match member {
            DecodedMember::Field {
                id,
                required,
                value,
            } => DurableMemberShape::Field(DurableFieldShape {
                id: *id,
                required: *required,
                value: value.clone(),
            }),
            DecodedMember::Group { id, members } => DurableMemberShape::Group(DurableGroupShape {
                id: *id,
                members: member_shapes(members),
            }),
            // Name and record are surface, not identity: the descriptor carries only
            // the branch's placement, key tuple, and member value shapes.
            DecodedMember::Branch {
                placement,
                keys,
                members,
                ..
            } => DurableMemberShape::Branch(DurableBranchShape {
                placement: *placement,
                keys: key_shapes(keys),
                members: member_shapes(members),
            }),
        })
        .collect()
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

fn decode_functions(
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
        .map(|root| {
            let flat = is_flat_executable_root(root);
            // A flat-executable root's branches are all scalar-field keyed
            // branches, each carrying its own nested branches; seal the whole tree in
            // declaration order so a BranchEntry branch path indexes it level by level. A
            // non-flat root parks every branch site, so it needs no sealed branch list.
            let branches = if flat {
                seal_branches(&root.members, &decoded.strings)
            } else {
                Vec::new()
            };
            let groups = if flat {
                seal_groups(root, &types)
            } else {
                Vec::new()
            };
            SealedRoot {
                name: decoded.strings[root.name as usize].clone(),
                keys: root.keys.iter().map(|(scalar, _)| *scalar).collect(),
                record: root.record,
                // A root's members are extra-free when every direct member keeps it flat:
                // a field (scalar or widened composite), a root-level unkeyed group of
                // storable-value fields, or a simple branch. A nested/composite branch, or
                // a group nested below the root, is an extra that parks the root's
                // operations; a widened field no longer parks (it is framed inline). This
                // is a member-shape predicate independent of keyed-ness — a keyless
                // singleton parks separately.
                has_extras: !root.members.iter().all(member_flat_at_root),
                branches,
                groups,
            }
        })
        .collect();
    // The managed indexes seal from the decoded roots, each carrying the index of the
    // root it belongs to. Their projections were re-resolved against the decoded graph
    // in `decode_indexes`, so the sealed set trusts no image-side incidence summary. Each
    // ledger-id projection component also resolves to a record/key position here — the
    // form the path kernel maintains — against the same decoded root.
    let mut indexes: Vec<SealedIndex> = Vec::new();
    for (root_index, root) in decoded.roots.iter().enumerate() {
        for index in &root.indexes {
            let projection = resolve_index_projection(root, &index.components)?;
            indexes.push(SealedIndex {
                id: index.id,
                root: root_index as u16,
                unique: index.unique,
                components: index.components.clone(),
                projection,
            });
        }
    }
    let sites: Vec<SealedSite> = decoded.sites.clone();
    // Function signatures feed the per-function `Call` type check (phase 3).
    let signatures: Vec<FnSig> = decoded
        .functions
        .iter()
        .map(|function| FnSig {
            params: function.params.clone(),
            ret: function.ret,
        })
        .collect();
    let collections = decoded.collections.clone();
    let ctx = Ctx {
        types: &types,
        enums: &enums,
        collections: &collections,
        roots: &roots,
        sites: &sites,
        indexes: &indexes,
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
    let effects = Effects::compute(&functions, &decoded.site_paths);
    let export_entries: Vec<bool> = {
        let mut entries = vec![false; functions.len()];
        for (_, func) in &decoded.exports {
            entries[*func as usize] = true;
        }
        entries
    };
    let test_entry_mask: Vec<bool> = {
        let mut entries = vec![false; functions.len()];
        for (_, func) in &decoded.test_entries {
            entries[*func as usize] = true;
        }
        entries
    };
    for (index, function) in functions.iter().enumerate() {
        effects.check_transaction_flow(
            index,
            function,
            export_entries[index],
            test_entry_mask[index],
        )?;
    }

    // Phase 5 (presence): every present-entry sparse set is dominated by a presence
    // fact on its key slot, rechecked independently of the compiler.
    for function in &functions {
        check_presence_flow(function, &ctx)?;
    }

    let exports = decoded
        .exports
        .iter()
        .map(|(id, func)| {
            let demand = effects.demand(*func);
            let demand_id = demand.demand_set_id();
            SealedExport {
                id: *id,
                func: *func,
                mutating: effects.mutates_closure[*func as usize],
                demand,
                demand_id,
                reachable_sites: effects.reachable_sites(*func),
            }
        })
        .collect();

    // Record each export's effect class on its entry function too, for tools.
    for (_, func) in &decoded.exports {
        functions[*func as usize].mutating = effects.mutates_closure[*func as usize];
    }

    let test_entries = check_test_entries(&decoded, &functions, &export_entries, &effects)?;

    // Per-function demand from the same effects owner, so a test-body driver can open
    // the session one export call requires without a second demand model.
    let function_demands: Vec<ExportDemand> = (0..functions.len() as u16)
        .map(|f| effects.demand(f))
        .collect();

    Ok(VerifiedImage {
        image_id: decoded.image_id,
        types,
        enums,
        collections,
        roots,
        indexes,
        sites,
        durable_contract: decoded.durable_contract,
        durable_descriptor: decoded.durable_descriptor,
        consts: decoded.consts,
        functions,
        exports,
        test_entries,
        function_demands,
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
        // A test entry may touch durable data: its demand is recorded in the parallel
        // test-entry table below so an E01 ephemeral test attachment can bound its
        // authority by the test-image union. It is still never an export and carries
        // no wire identity.
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

    // A test body is one of two disjoint kinds: it performs durable operations
    // directly (running in the harness session) or it drives exports, where each
    // export call is its own invocation boundary. Mixing the two — a direct durable
    // op together with a call to a transaction owner — is refused: the owner's commit
    // would consume the harness session out from under the direct op, and no single
    // session can carry both. The compiler reports the same shape at check time; this
    // is the independent artifact-level mirror.
    for (_, func) in &decoded.test_entries {
        let function = &functions[*func as usize];
        let has_direct_durable = function
            .instrs()
            .iter()
            .any(|instr| durable_op_class(instr).is_some());
        let drives_owner = call_targets(function)
            .iter()
            .any(|&callee| effects.has_begin[callee]);
        if has_direct_durable && drives_owner {
            return Err(reject(
                VerifyPhase::TestEntry,
                "a test body performs a direct durable operation and also drives a \
                 transaction-owning export",
            ));
        }
    }

    Ok(decoded
        .test_entries
        .iter()
        .map(|(name, func)| {
            let demand = effects.demand(*func);
            let kind = if demand.is_empty() {
                TestKind::Storeless
            } else if functions[*func as usize]
                .instrs()
                .iter()
                .any(|instr| durable_op_class(instr).is_some())
            {
                TestKind::DirectDurable
            } else {
                TestKind::Driver
            };
            SealedTestEntry {
                name: decoded.strings[*name as usize].clone(),
                func: *func,
                demand,
                kind,
            }
        })
        .collect())
}
