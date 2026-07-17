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

use std::collections::BTreeSet;
use std::rc::Rc;

use marrow_image::{
    DemandAtom, DurableBranchShape, DurableContractDescriptor, DurableContractId,
    DurableEnumMemberShape, DurableFieldShape, DurableGroupShape, DurableIndexComponent,
    DurableIndexShape, DurableKeyShape, DurableMemberShape, DurableRootShape, DurableValueShape,
    ExportDemand, ExportId, ImageId, ImageType, LedgerIdBytes, OP_ASSERT, OP_BOOL_NOT,
    OP_BRANCH_PRESENT, OP_BYTES_GE, OP_BYTES_GT, OP_BYTES_LE, OP_BYTES_LT, OP_CALL, OP_CONST_LOAD,
    OP_CONV_BYTES_TEXT, OP_CONV_STRING_BOOL, OP_CONV_STRING_INT, OP_DATE_ADD_DAYS,
    OP_DATE_DAYS_BETWEEN, OP_DATE_GE, OP_DATE_GT, OP_DATE_LE, OP_DATE_LT, OP_DUR_CREATE_ENTRY,
    OP_DUR_ERASE_ENTRY, OP_DUR_ERASE_FIELD, OP_DUR_EXISTS, OP_DUR_ITERATE_BOUNDED,
    OP_DUR_READ_ENTRY, OP_DUR_READ_FIELD, OP_DUR_REPLACE_ENTRY, OP_DUR_SET_REQUIRED,
    OP_DUR_SET_SPARSE, OP_DUR_SET_SPARSE_PRESENT, OP_DURATION_ADD, OP_DURATION_GE, OP_DURATION_GT,
    OP_DURATION_LE, OP_DURATION_LT, OP_DURATION_SUB, OP_ENUM_CONSTRUCT, OP_ENUM_PAYLOAD_GET,
    OP_ENUM_TAG, OP_EQ_BOOL, OP_EQ_BYTES, OP_EQ_DATE, OP_EQ_DURATION, OP_EQ_ENUM, OP_EQ_INSTANT,
    OP_EQ_INT, OP_EQ_TEXT, OP_FIELD_GET, OP_FIELD_SET, OP_FIELD_UNSET, OP_INSTANT_ADD_DURATION,
    OP_INSTANT_GE, OP_INSTANT_GT, OP_INSTANT_LE, OP_INSTANT_LT, OP_INSTANT_SUB_DURATION,
    OP_INT_ADD, OP_INT_ADD_CHECKED, OP_INT_DIV, OP_INT_DIV_CHECKED, OP_INT_GE, OP_INT_GT,
    OP_INT_LE, OP_INT_LT, OP_INT_MUL, OP_INT_MUL_CHECKED, OP_INT_NEG, OP_INT_NEG_CHECKED,
    OP_INT_REM, OP_INT_REM_CHECKED, OP_INT_SUB, OP_INT_SUB_CHECKED, OP_JUMP, OP_JUMP_IF_FALSE,
    OP_LIST_APPEND, OP_LIST_GET, OP_LIST_LEN, OP_LIST_NEW, OP_LOCAL_GET, OP_LOCAL_SET, OP_MAP_GET,
    OP_MAP_INSERT, OP_MAP_KEY_AT, OP_MAP_LEN, OP_MAP_NEW, OP_MAP_VALUE_AT, OP_POP, OP_RANGE_GUARD,
    OP_RECORD_NEW, OP_RETURN, OP_SOME_WRAP, OP_TEXT_CONCAT, OP_TEXT_CONTAINS, OP_TEXT_GE,
    OP_TEXT_GT, OP_TEXT_IS_EMPTY, OP_TEXT_JOIN, OP_TEXT_LE, OP_TEXT_LINES, OP_TEXT_LT,
    OP_TEXT_SPLIT, OP_TEXT_TRIM, OP_TXN_BEGIN, OP_TXN_COMMIT, OP_UNREACHABLE, OP_VACANT_LOAD,
    OPTIONAL_FLAG, OperationClass, Scalar, SemanticNode, SemanticNodeKind, SemanticPath,
    SemanticStep, SemanticStepKind, SemanticTarget, TAG_BOOL, TAG_BYTES, TAG_COLLECTION, TAG_DATE,
    TAG_DURATION, TAG_ENUM, TAG_INSTANT, TAG_INT, TAG_RECORD, TAG_TEXT, TAG_UNIT, image_id,
};

use crate::reader::Reader;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::{
    RetShape, SealedBranch, SealedCollectionType, SealedConst, SealedEnumType, SealedExport,
    SealedField, SealedFunction, SealedIndex, SealedInstr, SealedRecordType, SealedRoot,
    SealedSite, SealedSiteTarget, SealedTestEntry, SealedVariant, SpanRow, VerifiedImage,
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

/// A decoded durable root: name string index, its ordered key tuple (each column a
/// scalar and its ledger id; empty for a singleton root), record type index, the
/// rest of the root's placement/product ledger identity, and the resource's durable
/// member tree.
struct DecodedRoot {
    name: u16,
    keys: Vec<(Scalar, LedgerIdBytes)>,
    record: u16,
    placement: LedgerIdBytes,
    product: LedgerIdBytes,
    members: Vec<DecodedMember>,
    indexes: Vec<DecodedIndex>,
}

/// A decoded managed index of a root: its `Index` ledger id, its `unique` flag, and
/// its ordered projection of leaf references. Each component is re-resolved against
/// the root's own top-level fields and identity keys during decode, so a component
/// referencing no real leaf is refused.
struct DecodedIndex {
    id: LedgerIdBytes,
    unique: bool,
    components: Vec<DurableIndexComponent>,
}

/// One decoded durable member, in the image's declaration order: a stored scalar
/// field, a static `group` namespace, or a keyed `branch` placement. Groups and
/// branches recurse.
enum DecodedMember {
    Field {
        id: LedgerIdBytes,
        required: bool,
        value: DurableValueShape,
    },
    Group {
        id: LedgerIdBytes,
        members: Vec<DecodedMember>,
    },
    Branch {
        placement: LedgerIdBytes,
        /// The branch's simple name (string index), for the physical layer.
        name: u16,
        /// The branch entry's materialized record type index.
        record: u16,
        keys: Vec<(Scalar, LedgerIdBytes)>,
        members: Vec<DecodedMember>,
    },
}

impl DecodedMember {
    /// Whether this member is a field-only keyed branch, recursively — the branch shape
    /// the kernel executes at any depth. Its key is one or more columns and every direct
    /// member itself keeps flat: a field (scalar or widened composite), or a nested branch
    /// that is itself a simple branch. A static `group` breaks it. The recursion admits an
    /// arbitrarily deep chain of field-only branches with composite keys, which the
    /// recursive physical layout and profile serve.
    fn is_simple_branch(&self) -> bool {
        matches!(
            self,
            DecodedMember::Branch { keys, members, .. }
                if !keys.is_empty()
                    && members.iter().all(DecodedMember::keeps_root_flat)
        )
    }

    /// Whether this member keeps its containing root flat-executable: a field (scalar or
    /// widened composite — the durable field codec frames a composite inline in the one
    /// field-leaf cell) or a simple (recursively field-only, keyed) branch. A static
    /// `group` and a composite/nested branch still park the root.
    fn keeps_root_flat(&self) -> bool {
        match self {
            DecodedMember::Field { .. } => true,
            DecodedMember::Group { .. } => false,
            DecodedMember::Branch { .. } => self.is_simple_branch(),
        }
    }
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
    collections: Vec<SealedCollectionType>,
    roots: Vec<DecodedRoot>,
    sites: Vec<SealedSite>,
    /// Each site's resolved graph-node path, parallel to `sites` by index.
    site_paths: Vec<SemanticPath>,
    durable_contract: DurableContractId,
    durable_descriptor: DurableContractDescriptor,
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

/// Decode the DURABLE table (design §C 0x03): 0 or 1 roots — preceded, when any
/// root is present, by the application's 16-byte ledger id — then the operation
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
        // The member tree's top-level fields are exactly the materialized record's
        // stored fields, in order and value shape: this ties the durable identity to
        // the executable record so a hostile image cannot claim one identity while
        // executing over a different field shape. The value-shape match recurses
        // through the record and enum tables, so a widened field (a nominal, struct,
        // or enum) is checked as thoroughly as a plain scalar.
        let record_fields = &types[record as usize].fields;
        let mut top_fields = members.iter().filter_map(|member| match member {
            DecodedMember::Field {
                value, required, ..
            } => Some((value, *required)),
            _ => None,
        });
        for field in record_fields {
            match top_fields.next() {
                Some((value, member_required))
                    if member_required == field.required
                        && value_shape_matches(value, field.ty, types, enums) => {}
                _ => {
                    return Err(reject(
                        VerifyPhase::Table,
                        "root member tree fields do not match the record fields",
                    ));
                }
            }
        }
        if top_fields.next().is_some() {
            return Err(reject(
                VerifyPhase::Table,
                "root member tree has more top-level fields than the record",
            ));
        }
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
        let root = roots
            .iter()
            .find(|root| root.placement == placement)
            .ok_or(reject(
                VerifyPhase::Table,
                "durable index site path is not rooted at a durable root",
            ))?;
        let index_id = steps.last().expect("an index path has an index step").id;
        let index = root
            .indexes
            .iter()
            .find(|index| index.id == index_id)
            .ok_or(reject(
                VerifyPhase::Table,
                "durable index site names no managed index of its root",
            ))?;
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
        return Ok(SealedSite::Parked {
            path: SemanticPath::from_steps(steps.to_vec()),
            target,
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
    !root.keys.is_empty() && root.members.iter().all(DecodedMember::keeps_root_flat)
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
    // Index eligibility is decoupled from field executability: an index component must
    // project an orderable durable-key *scalar* leaf, which a widened (struct/enum) field
    // is not. A widened field is executable (framed inline in its cell) but never an index
    // component, so the eligible set is the scalar top-level fields only — refused
    // independently of `keeps_root_flat`, which now admits widened fields.
    let scalar_field_ids: Vec<LedgerIdBytes> = members
        .iter()
        .filter_map(|member| match member {
            DecodedMember::Field {
                id,
                value: DurableValueShape::Scalar(_),
                ..
            } => Some(*id),
            _ => None,
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
                    if !scalar_field_ids.contains(&leaf) {
                        return Err(reject(
                            VerifyPhase::Table,
                            if field_ids.contains(&leaf) {
                                "durable index field component names a widened (non-scalar) \
                                 field, which is not index-eligible"
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
            if count > marrow_image::bounds::MAX_FIELDS {
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
            SealedRoot {
                name: decoded.strings[root.name as usize].clone(),
                keys: root.keys.iter().map(|(scalar, _)| *scalar).collect(),
                record: root.record,
                // A root is flat-executable (no extras) when every member keeps it flat:
                // a field (scalar or widened composite) or a simple branch. A static
                // `group` or a nested/composite branch is an extra that parks the root's
                // operations; a widened field no longer parks (it is framed inline).
                has_extras: !root.members.iter().all(DecodedMember::keeps_root_flat),
                branches,
            }
        })
        .collect();
    // The managed indexes seal from the decoded roots, each carrying the index of the
    // root it belongs to. Their projections were re-resolved against the decoded graph
    // in `decode_indexes`, so the sealed set trusts no image-side incidence summary.
    let indexes: Vec<SealedIndex> = decoded
        .roots
        .iter()
        .enumerate()
        .flat_map(|(root_index, root)| {
            root.indexes.iter().map(move |index| SealedIndex {
                id: index.id,
                root: root_index as u16,
                unique: index.unique,
                components: index.components.clone(),
            })
        })
        .collect();
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
    for (index, function) in functions.iter().enumerate() {
        effects.check_transaction_flow(index, function, export_entries[index])?;
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

    Ok(decoded
        .test_entries
        .iter()
        .map(|(name, func)| SealedTestEntry {
            name: decoded.strings[*name as usize].clone(),
            func: *func,
            demand: effects.demand(*func),
        })
        .collect())
}

/// The sealed tables the per-function checks consult.
struct Ctx<'a> {
    types: &'a [SealedRecordType],
    enums: &'a [SealedEnumType],
    collections: &'a [SealedCollectionType],
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

/// Phase 4/5 durable-demand closure and the transaction-flow lattice (design §E).
///
/// The single effects owner: it reconstructs each function's durable-access atom set
/// over its whole acyclic call closure from the sealed sites its opcodes reference,
/// and the boolean mutate/read coverage the transaction-flow lattice and the store
/// ceiling consume are projected from that atom set — there is no second effects
/// model.
struct Effects {
    /// Per function: the durable-access atoms it or a transitive callee performs.
    atoms_closure: Vec<BTreeSet<DemandAtom>>,
    /// Per function: the image-local site indices it or a transitive callee reaches.
    sites_closure: Vec<BTreeSet<u16>>,
    /// Per function: whether its atom closure mutates (write/erase). Projected from
    /// the atom set; consumed by the transaction-flow lattice and the export effect
    /// class.
    mutates_closure: Vec<bool>,
    /// Per function: whether it contains a `TxnBegin` (a transaction owner).
    has_begin: Vec<bool>,
    /// Per function: whether it contains a `TxnCommit`.
    has_commit: Vec<bool>,
}

impl Effects {
    /// Reconstruct demand over the acyclic call graph. `site_paths[s]` is the
    /// semantic path of the node site `s` addresses (parallel to the sealed sites).
    fn compute(functions: &[SealedFunction], site_paths: &[SemanticPath]) -> Self {
        let count = functions.len();
        // Each function's own atoms and reached sites, before closure.
        let mut atoms_closure: Vec<BTreeSet<DemandAtom>> = functions
            .iter()
            .map(|function| {
                let mut set = BTreeSet::new();
                for instr in function.instrs() {
                    if let (Some(site), Some(class)) =
                        (durable_site(instr), durable_op_class(instr))
                    {
                        set.insert(DemandAtom::new(site_paths[site as usize].clone(), class));
                    }
                }
                set
            })
            .collect();
        let mut sites_closure: Vec<BTreeSet<u16>> = functions
            .iter()
            .map(|function| function.instrs().iter().filter_map(durable_site).collect())
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

        // Fixpoint over the acyclic call graph: a caller's closure unions each
        // callee's closure. The graph is acyclic (recursion is rejected), so
        // iterating `count` times converges; the monotone growth stops earlier. The
        // caller index `f` also indexes the two closures a split borrow updates in
        // place, so a range loop is used deliberately.
        #[allow(clippy::needless_range_loop)]
        for _ in 0..count {
            let mut changed = false;
            for f in 0..count {
                for callee_index in 0..callees[f].len() {
                    let callee = callees[f][callee_index];
                    // Split the borrow: a call graph edge never self-loops (no
                    // recursion), so `f != callee`.
                    let (dst, src) = borrow_two(&mut atoms_closure, f, callee);
                    for atom in src.iter() {
                        if dst.insert(atom.clone()) {
                            changed = true;
                        }
                    }
                    let (dst_sites, src_sites) = borrow_two(&mut sites_closure, f, callee);
                    for &site in src_sites.iter() {
                        if dst_sites.insert(site) {
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }

        let mutates_closure: Vec<bool> = atoms_closure
            .iter()
            .map(|atoms| atoms.iter().any(|atom| atom.class().mutates()))
            .collect();

        Self {
            atoms_closure,
            sites_closure,
            mutates_closure,
            has_begin,
            has_commit,
        }
    }

    /// The verifier-reconstructed durable demand of the entry at `func`: its stable
    /// atom set over its whole call closure.
    fn demand(&self, func: u16) -> ExportDemand {
        ExportDemand::from_atoms(self.atoms_closure[func as usize].iter().cloned())
    }

    /// The image-local operation sites the entry at `func` can reach, ascending.
    fn reachable_sites(&self, func: u16) -> Vec<u16> {
        self.sites_closure[func as usize].iter().copied().collect()
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
                    let mutating_here = is_mutation(instr)
                        || matches!(instr, SealedInstr::Call(target) if self.mutates_closure[*target as usize]);
                    if mutating_here && state != State::InTxn {
                        return Err(reject(
                            VerifyPhase::Flow,
                            "a mutation sits outside the transaction region",
                        ));
                    }
                    // The commit consumes the session's engine transaction, so no
                    // durable operation — read or write, direct or through a callee's
                    // closure — may follow it. A mutating export observes the store
                    // inside its region and returns values it captured there; a read
                    // after commit is refused here so the runtime never reaches a
                    // consumed transaction.
                    let durable_here = durable_op_class(instr).is_some()
                        || matches!(instr, SealedInstr::Call(target) if !self.atoms_closure[*target as usize].is_empty());
                    if durable_here && state == State::AfterCommit {
                        return Err(reject(
                            VerifyPhase::Flow,
                            "a durable operation follows the transaction's commit",
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

/// Phase 5 (presence): the place-slot presence lattice (design §D). A
/// `DurSetSparsePresent` (the strict sparse set) asserts its containing entry is
/// present; this recheck proves that independently of the compiler, so a forged or
/// mis-lowered strict set whose graph cannot imply its payload is refused.
///
/// The lattice state at each program point is the set of key-slot locals whose entry
/// a dominating fact has proven present. A fact is *established* by a guard that
/// tests the entry keyed by a slot — `LocalGet(S); DurExists(entry); JumpIfFalse` on
/// its present (fallthrough) edge, or `LocalGet(S); DurReadEntry; BranchPresent` on
/// its present edge — or by a whole-entry `DurCreateEntry` keyed by that slot (create
/// leaves the entry present whether it was created or already present). It is
/// *killed* by an entry erase keyed by the slot or by any `LocalSet` of the slot (a
/// rebind; a `place` key slot is bind-once, so this never fires on compiler output —
/// it hardens the recheck against a mutated tape). Facts join by intersection at
/// merges: a slot is present only if it holds on every incoming edge. Calls are
/// transparent (no aliasing model): a mutation reached through a call that erases the
/// entry is caught by the kernel's runtime presence assertion, not here.
fn check_presence_flow(function: &SealedFunction, ctx: &Ctx) -> Result<(), VerifyRejection> {
    let code = function.instrs();
    if !code
        .iter()
        .any(|instr| matches!(instr, SealedInstr::DurSetSparsePresent { .. }))
    {
        return Ok(());
    }
    let mut entry: Vec<Option<BTreeSet<PresenceFact>>> = vec![None; code.len()];
    entry[0] = Some(BTreeSet::new());
    let mut worklist = vec![0usize];
    while let Some(index) = worklist.pop() {
        let present = entry[index]
            .clone()
            .expect("worklist only enqueues reached instructions");
        if let SealedInstr::DurSetSparsePresent { site, key_slots } = &code[index] {
            // The strict set is proven only if a dominating fact names the exact
            // containing entry — its branch path and its whole key-path — not merely a
            // matching slot tuple (sibling branches of equal arity share slot tuples).
            let branch = field_site_branch_path(ctx, *site).ok_or(reject(
                VerifyPhase::Flow,
                "a present-entry sparse set does not resolve to a field site",
            ))?;
            if !present.contains(&(branch, key_slots.clone())) {
                return Err(reject(
                    VerifyPhase::Flow,
                    "a present-entry sparse set is not dominated by a presence fact on its containing entry",
                ));
            }
        }
        for (successor, set) in presence_edges(code, ctx, index, &present) {
            if successor >= code.len() {
                return Err(reject(VerifyPhase::Flow, "presence edge out of range"));
            }
            match &mut entry[successor] {
                None => {
                    entry[successor] = Some(set);
                    worklist.push(successor);
                }
                Some(existing) => {
                    let merged: BTreeSet<PresenceFact> =
                        existing.intersection(&set).cloned().collect();
                    if merged.len() != existing.len() {
                        *existing = merged;
                        worklist.push(successor);
                    }
                }
            }
        }
    }
    Ok(())
}

/// A proven-present containing entry in the presence-flow lattice: the entry's branch
/// path (empty for the root) paired with its whole key-path as pre-evaluated local
/// slots (root-first). Keying on the branch path — not the slot tuple alone —
/// distinguishes sibling branches of equal key arity that share slot values.
type PresenceFact = (Vec<u16>, Vec<u16>);

/// The presence-set carried on each successor edge of the instruction at `index`.
/// Most instructions pass the set through unchanged; guards split the set (adding the
/// proven slot only on the present edge); create adds and erase/rebind remove.
fn presence_edges(
    code: &[SealedInstr],
    ctx: &Ctx,
    index: usize,
    present: &BTreeSet<PresenceFact>,
) -> Vec<(usize, BTreeSet<PresenceFact>)> {
    match &code[index] {
        SealedInstr::JumpIfFalse(target) => match exists_guard_fact(code, ctx, index) {
            // The present (true) edge falls through into the guarded block; the false
            // edge (target) is the absent branch.
            Some(fact) => {
                let mut present_edge = present.clone();
                present_edge.insert(fact);
                vec![(*target, present.clone()), (index + 1, present_edge)]
            }
            None => flow_successors(code, index)
                .into_iter()
                .map(|s| (s, present.clone()))
                .collect(),
        },
        SealedInstr::BranchPresent(target) => match read_entry_guard_fact(code, ctx, index) {
            Some(fact) => {
                let mut present_edge = present.clone();
                present_edge.insert(fact);
                vec![(*target, present.clone()), (index + 1, present_edge)]
            }
            None => flow_successors(code, index)
                .into_iter()
                .map(|s| (s, present.clone()))
                .collect(),
        },
        SealedInstr::DurCreateEntry(site) => {
            let mut next = present.clone();
            // Only a single-column root whole-entry create establishes root-entry
            // presence for its key slot (`entry_write_key_slot` reads one adjacent key).
            // A branch create (a `BranchEntry` site) leaves the root descendant-only, and
            // a composite-root create's misread single slot never matches a set's full
            // key-path — so neither falsely establishes a fact a strict set relies on.
            if is_entry_site(ctx, *site)
                && let Some(slot) = entry_write_key_slot(code, index)
            {
                next.insert((Vec::new(), vec![slot]));
            }
            vec![(index + 1, next)]
        }
        SealedInstr::DurEraseEntry(site) => {
            let mut next = present.clone();
            // An entry erase whose whole key-path is pre-evaluated slots kills that exact
            // entry's presence fact (its branch path and key-path): a root erase kills the
            // root fact, a branch erase kills only its own branch entry.
            if let Some((branch, arity)) = entry_site(ctx, *site)
                && let Some(keys) = read_key_path_before(code, index, arity)
            {
                next.remove(&(branch, keys));
            }
            vec![(index + 1, next)]
        }
        SealedInstr::LocalSet(slot) => {
            let mut next = present.clone();
            // A rebind of any key-path slot invalidates every fact that reads it.
            next.retain(|(_, keys)| !keys.contains(slot));
            vec![(index + 1, next)]
        }
        _ => flow_successors(code, index)
            .into_iter()
            .map(|s| (s, present.clone()))
            .collect(),
    }
}

/// Whether `site` is a flat whole-payload (entry marker) site — the presence a
/// containing-payload fact is about.
fn is_entry_site(ctx: &Ctx, site: u16) -> bool {
    matches!(
        ctx.sites.get(site as usize),
        Some(SealedSite::Flat {
            target: SealedSiteTarget::WholePayload,
            ..
        })
    )
}

/// The containing entry a flat entry (whole-payload or branch-entry) `site` names: its
/// branch path (empty for the root) and its whole key-path column arity. `None` for a
/// non-entry site (a field leaf or index), which names no entry to prove present.
fn entry_site(ctx: &Ctx, site: u16) -> Option<(Vec<u16>, usize)> {
    let SealedSite::Flat { root, target } = ctx.sites.get(site as usize)? else {
        return None;
    };
    let root = ctx.roots.get(*root as usize)?;
    match target {
        SealedSiteTarget::WholePayload => Some((Vec::new(), root.keys.len())),
        SealedSiteTarget::BranchEntry(path) => {
            let extra = branch_key_columns(root, path).ok()?;
            Some((path.to_vec(), root.keys.len() + extra.len()))
        }
        SealedSiteTarget::FieldLeaf(_) | SealedSiteTarget::BranchField { .. } => None,
    }
}

/// The branch path of a flat field-leaf `site`: empty for a root field, the branch
/// placement path for a branch field. `None` for a non-field site.
fn field_site_branch_path(ctx: &Ctx, site: u16) -> Option<Vec<u16>> {
    let SealedSite::Flat { target, .. } = ctx.sites.get(site as usize)? else {
        return None;
    };
    match target {
        SealedSiteTarget::FieldLeaf(_) => Some(Vec::new()),
        SealedSiteTarget::BranchField { branch, .. } => Some(branch.to_vec()),
        _ => None,
    }
}

/// The `arity` key-path slots pushed immediately before position `at` (root-first): each
/// must be a `LocalGet`, or the guard establishes no fact. `at` is the position of the
/// consuming `DurExists`/`DurReadEntry`.
fn read_key_path_before(code: &[SealedInstr], at: usize, arity: usize) -> Option<Vec<u16>> {
    if arity == 0 || at < arity {
        return None;
    }
    let mut keys = Vec::with_capacity(arity);
    for offset in 0..arity {
        let SealedInstr::LocalGet(slot) = &code[at - arity + offset] else {
            return None;
        };
        keys.push(*slot);
    }
    Some(keys)
}

/// The presence fact an `exists`-guard proves at a `JumpIfFalse`: `LocalGet(S0); …;
/// LocalGet(Sn); DurExists(entry site); JumpIfFalse`. The fact is the entry site's
/// branch path paired with the whole key-path it reads. `None` when the shape does not
/// match (a non-entry site, a non-local key, or an unrelated condition).
fn exists_guard_fact(code: &[SealedInstr], ctx: &Ctx, index: usize) -> Option<PresenceFact> {
    if index < 1 {
        return None;
    }
    let SealedInstr::DurExists(site) = &code[index - 1] else {
        return None;
    };
    let (branch, arity) = entry_site(ctx, *site)?;
    let keys = read_key_path_before(code, index - 1, arity)?;
    Some((branch, keys))
}

/// The presence fact an `if const x = p` guard proves at a `BranchPresent`:
/// `LocalGet(S0); …; LocalGet(Sn); DurReadEntry(entry site); BranchPresent`.
fn read_entry_guard_fact(code: &[SealedInstr], ctx: &Ctx, index: usize) -> Option<PresenceFact> {
    if index < 1 {
        return None;
    }
    let SealedInstr::DurReadEntry(site) = &code[index - 1] else {
        return None;
    };
    let (branch, arity) = entry_site(ctx, *site)?;
    let keys = read_key_path_before(code, index - 1, arity)?;
    Some((branch, keys))
}

/// The key slot of a single-column whole-entry create at `index`: `LocalGet(S);
/// LocalGet(record); DurCreateEntry`. The key is the operand below the record, so the
/// create's key comes from the `LocalGet` two back when the record is a single local
/// push.
///
/// Soundness of shape-adjacent slot identification: the caller applies this only to a
/// root `WholePayload` create (it gates on `is_entry_site`), so a branch create — whose
/// key-path leaves a *branch* key adjacent to the op — never reaches here and never
/// establishes root-entry presence, and a composite-root create's misread single slot
/// forms a 1-tuple fact no full-key-path strict set ever matches. The durable graph
/// admits a single root (`MAX_ROOTS == 1`, `marrow_image::bounds`), so for the gated
/// single-column root create the key slot fully discriminates the entry it establishes.
/// When `MAX_ROOTS` widens this no longer holds: two writes through the same slot value
/// could touch different roots' entries, and the presence lattice must key on
/// (root, slot). Revisit this helper for per-root slot discrimination before admitting
/// more than one root.
fn entry_write_key_slot(code: &[SealedInstr], index: usize) -> Option<u16> {
    if index < 2 {
        return None;
    }
    let SealedInstr::LocalGet(_) = &code[index - 1] else {
        return None;
    };
    let SealedInstr::LocalGet(slot) = &code[index - 2] else {
        return None;
    };
    Some(*slot)
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
            OP_BRANCH_PRESENT => SealedInstr::BranchPresent(operand_u32(&mut reader)? as usize),
            OP_UNREACHABLE => SealedInstr::Unreachable(operand_u16(&mut reader)?),
            OP_ASSERT => SealedInstr::Assert,
            OP_CALL => SealedInstr::Call(operand_u16(&mut reader)?),
            OP_DUR_EXISTS => SealedInstr::DurExists(operand_u16(&mut reader)?),
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
            OP_DUR_ITERATE_BOUNDED => SealedInstr::DurIterateBounded {
                site: operand_u16(&mut reader)?,
                limit: operand_u32(&mut reader)?,
                from: operand_bool(&mut reader)?,
                list_ty: operand_u16(&mut reader)?,
            },
            OP_TXN_BEGIN => SealedInstr::TxnBegin,
            OP_TXN_COMMIT => SealedInstr::TxnCommit,
            OP_LIST_NEW => SealedInstr::ListNew(operand_u16(&mut reader)?),
            OP_LIST_APPEND => SealedInstr::ListAppend,
            OP_LIST_LEN => SealedInstr::ListLen,
            OP_LIST_GET => SealedInstr::ListGet,
            OP_MAP_NEW => SealedInstr::MapNew(operand_u16(&mut reader)?),
            OP_MAP_INSERT => SealedInstr::MapInsert,
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
            RetShape::Collection { idx, optional } => {
                frame.stack.push(VType::Collection { idx, optional });
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
            // A record/enum/collection operand names a value type; bounds-check it.
            match ty {
                ImageType::Record { idx, .. } if ctx.types.get(*idx as usize).is_none() => {
                    return Err(reject(
                        VerifyPhase::Function,
                        "vacant-load record index out of range",
                    ));
                }
                ImageType::Enum { idx, .. } if ctx.enums.get(*idx as usize).is_none() => {
                    return Err(reject(
                        VerifyPhase::Function,
                        "vacant-load enum index out of range",
                    ));
                }
                ImageType::Collection { idx, .. }
                    if ctx.collections.get(*idx as usize).is_none() =>
                {
                    return Err(reject(
                        VerifyPhase::Function,
                        "vacant-load collection index out of range",
                    ));
                }
                _ => {}
            }
            frame.stack.push(
                VType::from_image(*ty).expect("vacant-load operand is a value type, not unit"),
            );
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
        SealedInstr::ListNew(idx) => {
            match ctx.collections.get(*idx as usize) {
                Some(SealedCollectionType::List { .. }) => {}
                _ => {
                    return Err(reject(
                        VerifyPhase::Function,
                        "list-new operand does not name a list collection type",
                    ));
                }
            }
            frame.stack.push(VType::bare_collection(*idx));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::MapNew(idx) => {
            match ctx.collections.get(*idx as usize) {
                Some(SealedCollectionType::Map { .. }) => {}
                _ => {
                    return Err(reject(
                        VerifyPhase::Function,
                        "map-new operand does not name a map collection type",
                    ));
                }
            }
            frame.stack.push(VType::bare_collection(*idx));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::ListAppend => {
            let value = pop(&mut frame.stack)?;
            let (idx, elem) = list_elem(ctx, pop(&mut frame.stack)?)?;
            if value != VType::from_image(elem).expect("a list element type is never unit") {
                return Err(reject(
                    VerifyPhase::Function,
                    "list-append value type does not match the element type",
                ));
            }
            frame.stack.push(VType::bare_collection(idx));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::ListLen => {
            list_elem(ctx, pop(&mut frame.stack)?)?;
            frame.stack.push(VType::bare_scalar(Scalar::Int));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::ListGet => {
            expect_scalar(pop(&mut frame.stack)?, Scalar::Int)?;
            let (_, elem) = list_elem(ctx, pop(&mut frame.stack)?)?;
            frame
                .stack
                .push(VType::from_image(elem).expect("a list element type is never unit"));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::MapInsert => {
            let value = pop(&mut frame.stack)?;
            let key = pop(&mut frame.stack)?;
            let (idx, key_ty, value_ty) = map_kv(ctx, pop(&mut frame.stack)?)?;
            if key != VType::from_image(key_ty).expect("a map key type is never unit") {
                return Err(reject(
                    VerifyPhase::Function,
                    "map-insert key type does not match the map key type",
                ));
            }
            if value != VType::from_image(value_ty).expect("a map value type is never unit") {
                return Err(reject(
                    VerifyPhase::Function,
                    "map-insert value type does not match the map value type",
                ));
            }
            frame.stack.push(VType::bare_collection(idx));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::MapGet => {
            let key = pop(&mut frame.stack)?;
            let (_, key_ty, value_ty) = map_kv(ctx, pop(&mut frame.stack)?)?;
            if key != VType::from_image(key_ty).expect("a map key type is never unit") {
                return Err(reject(
                    VerifyPhase::Function,
                    "map-get key type does not match the map key type",
                ));
            }
            frame.stack.push(
                VType::from_image(value_ty)
                    .expect("a map value type is never unit")
                    .to_optional(),
            );
            return Ok(Control::Fallthrough);
        }
        SealedInstr::MapLen => {
            map_kv(ctx, pop(&mut frame.stack)?)?;
            frame.stack.push(VType::bare_scalar(Scalar::Int));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::MapKeyAt => {
            expect_scalar(pop(&mut frame.stack)?, Scalar::Int)?;
            let (_, key_ty, _) = map_kv(ctx, pop(&mut frame.stack)?)?;
            frame
                .stack
                .push(VType::from_image(key_ty).expect("a map key type is never unit"));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::MapValueAt => {
            expect_scalar(pop(&mut frame.stack)?, Scalar::Int)?;
            let (_, _, value_ty) = map_kv(ctx, pop(&mut frame.stack)?)?;
            frame
                .stack
                .push(VType::from_image(value_ty).expect("a map value type is never unit"));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::TextSplit(idx) => {
            // `split(text, sep): List[string]`: separator then text on the stack.
            expect_scalar(pop(&mut frame.stack)?, Scalar::Text)?;
            expect_scalar(pop(&mut frame.stack)?, Scalar::Text)?;
            list_of_string(ctx, *idx)?;
            frame.stack.push(VType::bare_collection(*idx));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::TextLines(idx) => {
            // `lines(text): List[string]`.
            expect_scalar(pop(&mut frame.stack)?, Scalar::Text)?;
            list_of_string(ctx, *idx)?;
            frame.stack.push(VType::bare_collection(*idx));
            return Ok(Control::Fallthrough);
        }
        SealedInstr::TextJoin => {
            // `join(List[string], sep): string`: separator then list on the stack.
            expect_scalar(pop(&mut frame.stack)?, Scalar::Text)?;
            let (idx, _) = list_elem(ctx, pop(&mut frame.stack)?)?;
            list_of_string(ctx, idx)?;
            frame.stack.push(VType::bare_scalar(Scalar::Text));
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
        SealedInstr::EqDate
        | SealedInstr::DateLt
        | SealedInstr::DateLe
        | SealedInstr::DateGt
        | SealedInstr::DateGe => {
            binary(stack, Scalar::Date, Scalar::Bool)?;
            Ok(Control::Fallthrough)
        }
        SealedInstr::EqInstant
        | SealedInstr::InstantLt
        | SealedInstr::InstantLe
        | SealedInstr::InstantGt
        | SealedInstr::InstantGe => {
            binary(stack, Scalar::Instant, Scalar::Bool)?;
            Ok(Control::Fallthrough)
        }
        SealedInstr::EqDuration
        | SealedInstr::DurationLt
        | SealedInstr::DurationLe
        | SealedInstr::DurationGt
        | SealedInstr::DurationGe => {
            binary(stack, Scalar::Duration, Scalar::Bool)?;
            Ok(Control::Fallthrough)
        }
        // `date_add_days(date, int) → date`: pop the int, then the date.
        SealedInstr::DateAddDays => {
            expect_scalar(pop(stack)?, Scalar::Int)?;
            expect_scalar(pop(stack)?, Scalar::Date)?;
            stack.push(VType::bare_scalar(Scalar::Date));
            Ok(Control::Fallthrough)
        }
        SealedInstr::DateDaysBetween => {
            binary(stack, Scalar::Date, Scalar::Int)?;
            Ok(Control::Fallthrough)
        }
        SealedInstr::DurationAdd | SealedInstr::DurationSub => {
            binary(stack, Scalar::Duration, Scalar::Duration)?;
            Ok(Control::Fallthrough)
        }
        // `instant +/- duration → instant`: pop the duration, then the instant.
        SealedInstr::InstantAddDuration | SealedInstr::InstantSubDuration => {
            expect_scalar(pop(stack)?, Scalar::Duration)?;
            expect_scalar(pop(stack)?, Scalar::Instant)?;
            stack.push(VType::bare_scalar(Scalar::Instant));
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
        | SealedInstr::DurSetSparsePresent { .. }
        | SealedInstr::DurCreateEntry(_)
        | SealedInstr::DurReplaceEntry(_)
        | SealedInstr::DurEraseField(_)
        | SealedInstr::DurEraseEntry(_)
        | SealedInstr::DurIterateBounded { .. }
        | SealedInstr::TxnBegin
        | SealedInstr::TxnCommit
        | SealedInstr::ListNew(_)
        | SealedInstr::ListAppend
        | SealedInstr::ListLen
        | SealedInstr::ListGet
        | SealedInstr::MapNew(_)
        | SealedInstr::MapInsert
        | SealedInstr::MapGet
        | SealedInstr::MapLen
        | SealedInstr::MapKeyAt
        | SealedInstr::MapValueAt
        | SealedInstr::TextSplit(_)
        | SealedInstr::TextLines(_)
        | SealedInstr::TextJoin => {
            unreachable!(
                "record, optional, call, durable, collection, and text-collection opcodes return from the earlier matches"
            )
        }
    }
}

/// The COLLTYPES index and element type of a bare list `VType`, or a phase-3
/// rejection when the operand is not a bare list collection.
fn list_elem(ctx: &Ctx, value: VType) -> Result<(u16, ImageType), VerifyRejection> {
    let VType::Collection {
        idx,
        optional: false,
    } = value
    else {
        return Err(reject(VerifyPhase::Function, "operand is not a bare list"));
    };
    match ctx.collections.get(idx as usize) {
        Some(SealedCollectionType::List { elem }) => Ok((idx, *elem)),
        _ => Err(reject(
            VerifyPhase::Function,
            "collection index does not name a list type",
        )),
    }
}

/// Prove COLLTYPES index `idx` names a `List[string]`, the only collection the
/// text-floor `split`/`lines`/`join` opcodes produce or consume. A hand-built image
/// naming any other collection there is rejected.
fn list_of_string(ctx: &Ctx, idx: u16) -> Result<(), VerifyRejection> {
    match ctx.collections.get(idx as usize) {
        Some(SealedCollectionType::List { elem }) if *elem == ImageType::scalar(Scalar::Text) => {
            Ok(())
        }
        _ => Err(reject(
            VerifyPhase::Function,
            "text split/lines/join collection index does not name a list of string",
        )),
    }
}

/// The COLLTYPES index and `(key, value)` image types of a bare map `VType`, or a
/// phase-3 rejection when the operand is not a bare map collection.
fn map_kv(ctx: &Ctx, value: VType) -> Result<(u16, ImageType, ImageType), VerifyRejection> {
    let VType::Collection {
        idx,
        optional: false,
    } = value
    else {
        return Err(reject(VerifyPhase::Function, "operand is not a bare map"));
    };
    match ctx.collections.get(idx as usize) {
        Some(SealedCollectionType::Map { key, value }) => Ok((idx, *key, *value)),
        _ => Err(reject(
            VerifyPhase::Function,
            "collection index does not name a map type",
        )),
    }
}

/// Whether `instr` is handled by [`apply_durable`] (a durable op or a transaction
/// marker).
fn is_durable(instr: &SealedInstr) -> bool {
    is_mutation(instr)
        || is_durable_read(instr)
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
        | SealedInstr::DurSetSparsePresent { site, .. }
        | SealedInstr::DurCreateEntry(site)
        | SealedInstr::DurReplaceEntry(site)
        | SealedInstr::DurEraseField(site)
        | SealedInstr::DurEraseEntry(site)
        | SealedInstr::DurIterateBounded { site, .. } => Some(*site),
        _ => None,
    }
}

/// The single owner of the durable-opcode → [`OperationClass`] partition. The
/// closed projection of the durable operation algebra onto authority atoms:
/// `create`, `replace`, and required/sparse field sets are all writes; the two
/// erases are erases; presence is a probe; field/entry reads are reads; and the
/// bounded traversal is ordered traversal. Transaction markers and every pure opcode
/// make no atom.
///
/// The match is exhaustive with no `_` fallthrough, so a new durable opcode added
/// to [`SealedInstr`] without a class here fails to compile rather than silently
/// projecting to "no access". [`is_mutation`] and [`is_durable_read`] derive from
/// this partition and never restate it.
fn durable_op_class(instr: &SealedInstr) -> Option<OperationClass> {
    match instr {
        SealedInstr::DurExists(_) => Some(OperationClass::Presence),
        SealedInstr::DurReadField(_) | SealedInstr::DurReadEntry(_) => Some(OperationClass::Read),
        SealedInstr::DurSetRequired(_)
        | SealedInstr::DurSetSparse(_)
        | SealedInstr::DurSetSparsePresent { .. }
        | SealedInstr::DurCreateEntry(_)
        | SealedInstr::DurReplaceEntry(_) => Some(OperationClass::Write),
        SealedInstr::DurEraseField(_) | SealedInstr::DurEraseEntry(_) => {
            Some(OperationClass::Erase)
        }
        SealedInstr::DurIterateBounded { .. } => Some(OperationClass::IndexRead),
        // Region markers open and close the transaction but stage no access.
        SealedInstr::TxnBegin | SealedInstr::TxnCommit => None,
        // The closed complement: every pure opcode stages no durable access.
        SealedInstr::ConstLoad(_)
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
        | SealedInstr::ConvStringInt
        | SealedInstr::ConvStringBool
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
        | SealedInstr::BranchPresent(_)
        | SealedInstr::Unreachable(_)
        | SealedInstr::Assert
        | SealedInstr::Call(_)
        | SealedInstr::ListNew(_)
        | SealedInstr::ListAppend
        | SealedInstr::ListLen
        | SealedInstr::ListGet
        | SealedInstr::MapNew(_)
        | SealedInstr::MapInsert
        | SealedInstr::MapGet
        | SealedInstr::MapLen
        | SealedInstr::MapKeyAt
        | SealedInstr::MapValueAt => None,
    }
}

/// Whether `instr` stages a durable mutation (a write or erase). Derived from the
/// [`durable_op_class`] partition so the mutation set never drifts from the atom it
/// projects.
fn is_mutation(instr: &SealedInstr) -> bool {
    durable_op_class(instr).is_some_and(|class| class.mutates())
}

/// Whether `instr` reads durable data — a presence probe, a field/entry read, or an
/// ordered bounded traversal. The classified durable ops that are not mutations.
fn is_durable_read(instr: &SealedInstr) -> bool {
    durable_op_class(instr).is_some_and(|class| !class.mutates())
}

/// Mutably borrow two distinct elements of a slice. The demand fixpoint unions a
/// callee's closure into its caller; the call graph is acyclic, so `dst != src`.
fn borrow_two<T>(slice: &mut [T], dst: usize, src: usize) -> (&mut T, &T) {
    assert_ne!(dst, src, "a call graph edge never self-loops");
    if dst < src {
        let (left, right) = slice.split_at_mut(src);
        (&mut left[dst], &right[0])
    } else {
        let (left, right) = slice.split_at_mut(dst);
        (&mut right[0], &left[src])
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
    // A durable opcode may reference only a kernel-executable flat site. A parked
    // site (a nested placement, a group-scoped field, or a site on a singleton or
    // group/branch-bearing root) carries a complete identity but no executable
    // operation; an opcode over one is a forged or not-yet-executable image and is
    // refused here, independently of the compiler. A widened field site is executable.
    let (site_root, site_target) = match site {
        SealedSite::Flat { root, target } => (*root, target),
        SealedSite::Parked { .. } => {
            return Err(reject(
                VerifyPhase::Function,
                "a durable operation site is not yet executable",
            ));
        }
    };
    let root = ctx.roots.get(site_root as usize).ok_or(reject(
        VerifyPhase::Function,
        "durable site root out of range",
    ))?;
    // Defense in depth: a flat site's root is free of groups and composite/nested
    // branches by construction (field-only branches with composite keys are executable;
    // a widened field is inline-framed and executable), but recheck at the opcode.
    if root.has_extras {
        return Err(reject(
            VerifyPhase::Function,
            "a durable operation site requires a flat-executable root",
        ));
    }
    if root.keys.is_empty() {
        return Err(reject(
            VerifyPhase::Function,
            "a durable operation site requires a keyed root",
        ));
    }
    // A site addresses a key-path of one column per key column of every node from the root
    // down to the addressed node: the root's key columns, then each branch hop's key
    // columns. The lowering pushes them root-first in column-declaration order, so the
    // stack-pop order is that whole column sequence reversed. `entry_record` is the record
    // a whole-entry op reads or writes (the addressed branch's own for a branch site);
    // `traversed_arity` is the addressed layer's own key column count, used to park
    // composite-keyed traversal.
    let mut columns: Vec<VType> = root
        .keys
        .iter()
        .map(|scalar| VType::bare_scalar(*scalar))
        .collect();
    let (entry_record, traversed_arity) = match site_target {
        SealedSiteTarget::BranchEntry(path)
        | SealedSiteTarget::BranchField { branch: path, .. } => {
            let branch = resolve_sealed_branch(root, path)?;
            for scalar in branch_key_columns(root, path)? {
                columns.push(VType::bare_scalar(scalar));
            }
            (branch.record, branch.keys().len())
        }
        SealedSiteTarget::WholePayload | SealedSiteTarget::FieldLeaf(_) => {
            (root.record, root.keys.len())
        }
    };
    // Stack-pop order is the root-first column sequence reversed (deepest last column on
    // top of the stack, popped first).
    let mut key_path = columns;
    key_path.reverse();
    let stack = &mut frame.stack;
    match instr {
        SealedInstr::DurExists(_) => {
            pop_key_path(stack, &key_path)?;
            stack.push(VType::bare_scalar(Scalar::Bool));
        }
        SealedInstr::DurReadField(_) => {
            let field = field_of(ctx, site_target, root)?;
            let value = durable_field_vtype(field).to_optional();
            pop_key_path(stack, &key_path)?;
            stack.push(value);
        }
        SealedInstr::DurReadEntry(_) => {
            require_entry(site_target)?;
            pop_key_path(stack, &key_path)?;
            stack.push(VType::bare_record(entry_record).to_optional());
        }
        SealedInstr::DurSetRequired(_) => {
            let field = field_of(ctx, site_target, root)?;
            if !field.required {
                return Err(reject(
                    VerifyPhase::Function,
                    "set-required targets a sparse field",
                ));
            }
            let value = durable_field_vtype(field);
            expect(pop(stack)?, value)?;
            pop_key_path(stack, &key_path)?;
        }
        SealedInstr::DurSetSparse(_) => {
            let field = field_of(ctx, site_target, root)?;
            if field.required {
                return Err(reject(
                    VerifyPhase::Function,
                    "set-sparse targets a required field",
                ));
            }
            let value = durable_field_vtype(field).to_optional();
            expect(pop(stack)?, value)?;
            pop_key_path(stack, &key_path)?;
        }
        SealedInstr::DurSetSparsePresent { key_slots, .. } => {
            // The strict present form reads its containing entry's whole key-path from
            // place slots rather than the stack. It addresses a stored field leaf — a
            // root field (`FieldLeaf`) or a branch field (`BranchField`); a whole-payload,
            // branch-entry, or index site is not a field set and is refused. The field's
            // containing entry is the root (root field) or the branch (branch field), and
            // either way the site's full key-path is `columns` (root-first).
            if !matches!(
                site_target,
                SealedSiteTarget::FieldLeaf(_) | SealedSiteTarget::BranchField { .. }
            ) {
                return Err(reject(
                    VerifyPhase::Function,
                    "set-sparse-present requires a field-leaf site",
                ));
            }
            // The opcode's key-path must address the field's containing entry exactly:
            // one slot per key column of every node from the root down (the root-first
            // `columns` sequence). A forged image with too few or too many slots — e.g. a
            // single root key over a branch-field site, the slice-A write-safety concern —
            // is refused here so the kernel is never handed a mis-arity key-path.
            let columns_root_first: Vec<VType> = key_path.iter().rev().cloned().collect();
            if key_slots.len() != columns_root_first.len() {
                return Err(reject(
                    VerifyPhase::Function,
                    "set-sparse-present key-path arity does not match its field site",
                ));
            }
            let field = field_of(ctx, site_target, root)?;
            if field.required {
                return Err(reject(
                    VerifyPhase::Function,
                    "set-sparse-present targets a required field",
                ));
            }
            // The strict form reads its key-path from the place's pre-evaluated local
            // slots rather than the stack, so only the value is popped. Each slot must be
            // definitely initialized with its column's key type, root-first.
            let value = durable_field_vtype(field).to_optional();
            expect(pop(stack)?, value)?;
            for (slot, column_ty) in key_slots.iter().zip(&columns_root_first) {
                match frame.locals.get(*slot as usize) {
                    Some(Some(slot_ty)) if slot_ty == column_ty => {}
                    Some(Some(_)) => {
                        return Err(reject(
                            VerifyPhase::Function,
                            "set-sparse-present key slot has the wrong type",
                        ));
                    }
                    _ => {
                        return Err(reject(
                            VerifyPhase::Function,
                            "set-sparse-present key slot is uninitialized or out of range",
                        ));
                    }
                }
            }
        }
        SealedInstr::DurCreateEntry(_) | SealedInstr::DurReplaceEntry(_) => {
            require_entry(site_target)?;
            expect(pop(stack)?, VType::bare_record(entry_record))?;
            pop_key_path(stack, &key_path)?;
        }
        SealedInstr::DurEraseField(_) => {
            let field = field_of(ctx, site_target, root)?;
            if field.required {
                return Err(reject(
                    VerifyPhase::Function,
                    "erase targets a required field",
                ));
            }
            pop_key_path(stack, &key_path)?;
        }
        SealedInstr::DurEraseEntry(_) => {
            require_entry(site_target)?;
            pop_key_path(stack, &key_path)?;
        }
        SealedInstr::DurIterateBounded {
            limit,
            from,
            list_ty,
            ..
        } => {
            // Bounded traversal iterates the layer the site's placement belongs to: a
            // root site (WholePayload) the root's entry family, a branch site
            // (BranchEntry) that branch's children under a fixed root key. A field site
            // names no traversable layer.
            require_entry(site_target)?;
            // The `at most N` bound is a positive compile-time constant no larger than
            // the frozen-list ceiling.
            if *limit == 0 || *limit > marrow_image::bounds::MAX_TRAVERSAL_BOUND {
                return Err(reject(
                    VerifyPhase::Function,
                    "bounded traversal bound is out of range",
                ));
            }
            // Bounded traversal iterates a single key column: the loop binds one immediate
            // key and takes one inclusive `from`. A composite-keyed traversed layer has no
            // spelled single-column iteration in the current language, so it parks with a
            // typed rejection rather than inventing a last-column-under-prefix semantics.
            if traversed_arity != 1 {
                return Err(reject(
                    VerifyPhase::Function,
                    "bounded traversal over a composite-keyed layer is not yet executable",
                ));
            }
            // The traversed key is what iteration enumerates — the first element of the
            // site's whole-entry key-path (the single traversed column); the remainder is
            // the ancestor key-path locating the traversed layer's parent entry (empty for
            // a root site, the parent columns for a branch site).
            let (traversed_key, ancestor_path) = key_path
                .split_first()
                .expect("an entry site has a non-empty key-path");
            let VType::Scalar {
                scalar: key_scalar,
                optional: false,
            } = *traversed_key
            else {
                unreachable!("a durable key-path element is a bare scalar");
            };
            // The inclusive `from` key sits on top of the ancestor key-path and types
            // as the traversed key `K`.
            if *from {
                expect(pop(stack)?, *traversed_key)?;
            }
            for ty in ancestor_path {
                expect(pop(stack)?, *ty)?;
            }
            // `list_ty` must name exactly `List[K]`: the frozen keys materialize into
            // this one list value, so a hostile image naming a wider or wrong-element
            // list is refused before the runtime builds it.
            match ctx.collections.get(*list_ty as usize) {
                Some(SealedCollectionType::List { elem })
                    if *elem == ImageType::scalar(key_scalar) => {}
                _ => {
                    return Err(reject(
                        VerifyPhase::Function,
                        "bounded traversal list type does not name a list of the traversed key",
                    ));
                }
            }
            stack.push(VType::bare_collection(*list_ty));
            stack.push(VType::bare_scalar(Scalar::Bool));
        }
        _ => unreachable!("durable_site returned a site for this opcode"),
    }
    Ok(Control::Fallthrough)
}

/// Pop and type-check a durable operation's key-path against `key_path` (the key
/// types in stack-pop order — the branch key then the root key for a branch entry
/// site, the single root key otherwise).
fn pop_key_path(stack: &mut Vec<VType>, key_path: &[VType]) -> Result<(), VerifyRejection> {
    for ty in key_path {
        expect(pop(stack)?, *ty)?;
    }
    Ok(())
}

/// The field a field-target site addresses, or a rejection when the site is an
/// entry site. A top-level field resolves against the root's record; a branch field
/// against its branch's record, one level down.
fn field_of<'a>(
    ctx: &'a Ctx,
    target: &SealedSiteTarget,
    root: &SealedRoot,
) -> Result<&'a SealedField, VerifyRejection> {
    let (record, field) = match target {
        SealedSiteTarget::FieldLeaf(field) => (root.record, *field),
        SealedSiteTarget::BranchField { branch, field } => {
            (resolve_sealed_branch(root, branch)?.record, *field)
        }
        SealedSiteTarget::WholePayload | SealedSiteTarget::BranchEntry(_) => {
            return Err(reject(
                VerifyPhase::Function,
                "operation requires a field site",
            ));
        }
    };
    ctx.types[record as usize]
        .fields()
        .get(field as usize)
        .ok_or(reject(
            VerifyPhase::Function,
            "site field index out of range",
        ))
}

/// The bare value type of a durable field: a scalar, a record (dense product), or an
/// enum/`Option`/`Result` (a sum) — the storable durable value set. A collection or unit
/// is never an admitted durable field type, so `from_image` always resolves here.
fn durable_field_vtype(field: &SealedField) -> VType {
    VType::from_image(field.ty).expect("a durable field type is scalar, record, or enum")
}

/// Require an entry-target site: the root's whole payload or a keyed branch entry. A
/// field-leaf site (top-level or branch) is not an entry.
fn require_entry(target: &SealedSiteTarget) -> Result<(), VerifyRejection> {
    match target {
        SealedSiteTarget::WholePayload | SealedSiteTarget::BranchEntry(_) => Ok(()),
        SealedSiteTarget::FieldLeaf(_) | SealedSiteTarget::BranchField { .. } => Err(reject(
            VerifyPhase::Function,
            "operation requires an entry site",
        )),
    }
}

/// Walk a branch path through the recursive sealed branch tree, returning the deepest
/// addressed branch. Refuses a path element out of range at any level, so a forged image
/// naming a branch index past the sealed tree is rejected here — the trust backstop over
/// the verifier's own site resolution. An empty path is not a branch site.
fn resolve_sealed_branch<'a>(
    root: &'a SealedRoot,
    path: &[u16],
) -> Result<&'a SealedBranch, VerifyRejection> {
    let mut branches: &'a [SealedBranch] = &root.branches;
    let mut deepest: Option<&'a SealedBranch> = None;
    for &index in path {
        let branch = branches.get(index as usize).ok_or(reject(
            VerifyPhase::Function,
            "durable branch index out of range",
        ))?;
        deepest = Some(branch);
        branches = &branch.branches;
    }
    deepest.ok_or(reject(
        VerifyPhase::Function,
        "a branch site requires a non-empty branch path",
    ))
}

/// The branch chain's key columns, root-first and flattened across hops (each hop
/// contributes its whole ordered key tuple), along a branch path. Refuses a path element
/// out of range at any level, the same forged-image backstop as [`resolve_sealed_branch`].
fn branch_key_columns(root: &SealedRoot, path: &[u16]) -> Result<Vec<Scalar>, VerifyRejection> {
    let mut branches = &root.branches;
    let mut columns = Vec::with_capacity(path.len());
    for &index in path {
        let branch = branches.get(index as usize).ok_or(reject(
            VerifyPhase::Function,
            "durable branch index out of range",
        ))?;
        columns.extend_from_slice(&branch.keys);
        branches = &branch.branches;
    }
    Ok(columns)
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
        SealedConst::Date(_) => Scalar::Date,
        SealedConst::Instant(_) => Scalar::Instant,
        SealedConst::Duration(_) => Scalar::Duration,
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
