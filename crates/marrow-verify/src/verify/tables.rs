//! Phase 2 tables: string, type, enum, and collection decoding with value-type closure.

use super::model::{DecodedEnum, DecodedField, DecodedRecordType, DecodedVariant};
use super::reject;
use crate::reader::Reader;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::SealedCollectionType;
use marrow_image::{
    ImageType, OPTIONAL_FLAG, Scalar, TAG_BOOL, TAG_BYTES, TAG_COLLECTION, TAG_DATE, TAG_DURATION,
    TAG_ENUM, TAG_INSTANT, TAG_INT, TAG_RECORD, TAG_TEXT,
};
use std::rc::Rc;

/// Decode the TEST-ENTRY table (section 0x08): a count, then each `u16 name index
/// ‖ u16 function index` entry in strictly ascending, unique name-index order. The
/// name index resolves a report label; the function index a storeless test body.
/// Structural violations are phase-`Table` rejections; the test-entry semantic
/// constraints (assert legality, storelessness, disjointness from exports) are
/// checked in the later TestEntry phase.
pub(super) fn decode_test_entries(
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

pub(super) fn decode_strings(body: &[u8]) -> Result<Vec<Rc<str>>, VerifyRejection> {
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

pub(super) fn decode_bare_scalar(tag: u8) -> Option<Scalar> {
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

pub(super) fn decode_types(
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
pub(super) fn decode_enums(
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
pub(super) fn decode_collections(
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
pub(super) fn validate_record_field_refs(
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
pub(super) fn reject_value_type_cycles(
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
