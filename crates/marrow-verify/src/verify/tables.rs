//! Phase 2 tables: string, type, enum, and collection decoding with value-type closure.

use super::reject;
use super::type_ref::{Optionality, TagSet, TypeRules, decode_type_ref};
use crate::reader::Reader;
use crate::reject::{
    Bound, Duplicate, Flag, Ref, Region, RejectionKind as Kind, Tag, TypePosition, VerifyPhase,
    VerifyRejection,
};
use crate::sealed::{
    SealedCollectionType, SealedEnumType, SealedField, SealedRecordType, SealedVariant,
};
use marrow_image::{
    EnumId, ImageType, Scalar, TAG_BOOL, TAG_BYTES, TAG_DATE, TAG_DURATION, TAG_INSTANT, TAG_INT,
    TAG_TEXT,
};
use std::rc::Rc;

#[cfg(test)]
mod duplicate_name_tests;

/// Per-row duplicate-name detection over the whole string pool: one generation mark per
/// string, bumped per row, so a table costs its name count and no per-row set.
struct NameMarks {
    generations: Vec<u16>,
    generation: u16,
}

impl NameMarks {
    fn new(string_count: usize) -> Self {
        Self {
            generations: vec![0; string_count],
            generation: 0,
        }
    }

    fn begin_row(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("table row count is bounded below u16::MAX");
    }

    fn insert(&mut self, name: u16) -> bool {
        let mark = &mut self.generations[name as usize];
        let duplicate = *mark == self.generation;
        *mark = self.generation;
        duplicate
    }
}

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
    let count = reader.u16().ok_or(reject(
        VerifyPhase::Table,
        Kind::Truncated(Region::TestEntries),
    ))? as usize;
    if count > marrow_image::bounds::MAX_TEST_ENTRIES {
        return Err(reject(
            VerifyPhase::Table,
            Kind::OverBound(Bound::TestEntries),
        ));
    }
    let mut entries = Vec::with_capacity(count);
    let mut previous_name: Option<u16> = None;
    for _ in 0..count {
        let name = reader.u16().ok_or(reject(
            VerifyPhase::Table,
            Kind::Truncated(Region::TestEntries),
        ))?;
        let func = reader.u16().ok_or(reject(
            VerifyPhase::Table,
            Kind::Truncated(Region::TestEntries),
        ))?;
        if name as usize >= string_count {
            return Err(reject(VerifyPhase::Table, Kind::OutOfRange(Ref::String)));
        }
        if func as usize >= function_count {
            return Err(reject(VerifyPhase::Table, Kind::OutOfRange(Ref::Function)));
        }
        if let Some(prev) = previous_name
            && name <= prev
        {
            return Err(reject(
                VerifyPhase::Table,
                Kind::Unsorted(Region::TestEntries),
            ));
        }
        previous_name = Some(name);
        entries.push((name, func));
    }
    if !reader.is_empty() {
        return Err(reject(
            VerifyPhase::Table,
            Kind::Trailing(Region::TestEntries),
        ));
    }
    Ok(entries)
}

pub(super) fn decode_strings(body: &[u8]) -> Result<Vec<Rc<str>>, VerifyRejection> {
    let mut reader = Reader::new(body);
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Strings)))?
        as usize;
    if count > marrow_image::bounds::MAX_STRINGS {
        return Err(reject(VerifyPhase::Table, Kind::OverBound(Bound::Strings)));
    }
    let mut strings: Vec<Rc<str>> = Vec::with_capacity(count);
    let mut previous: Option<Vec<u8>> = None;
    for _ in 0..count {
        let len = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Strings)))?
            as usize;
        if len > marrow_image::bounds::MAX_STRING_BYTES {
            return Err(reject(
                VerifyPhase::Table,
                Kind::OverBound(Bound::StringBytes),
            ));
        }
        let raw = reader
            .take(len)
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Strings)))?;
        if let Some(prev) = &previous
            && raw <= prev.as_slice()
        {
            return Err(reject(VerifyPhase::Table, Kind::Unsorted(Region::Strings)));
        }
        previous = Some(raw.to_vec());
        let text =
            std::str::from_utf8(raw).map_err(|_| reject(VerifyPhase::Table, Kind::InvalidUtf8))?;
        strings.push(Rc::from(text));
    }
    if !reader.is_empty() {
        return Err(reject(VerifyPhase::Table, Kind::Trailing(Region::Strings)));
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

/// A record field: a scalar leaf, or a closed enum, record or collection value.
/// Never optional — sparseness is the `required` flag, not the type — and its
/// referenced indices are checked by `validate_record_field_refs` once the tables
/// they name have decoded.
const FIELD: TypeRules =
    TypeRules::new(TypePosition::RecordField, TagSet::VALUE, Optionality::Bare);

/// An enum payload leaf: a bare scalar, record or enum reference.
const PAYLOAD_LEAF: TypeRules = TypeRules::new(
    TypePosition::EnumPayloadLeaf,
    TagSet::SCALAR.with(TagSet::RECORD).with(TagSet::ENUM),
    Optionality::Bare,
);

/// A COLLTYPES element, key or value: a bare scalar, record, enum, or a collection
/// strictly earlier than `row`, so the collection reference graph is acyclic by
/// construction.
fn collection_leaf(type_count: usize, enum_count: usize, row: usize) -> TypeRules {
    TypeRules::new(
        TypePosition::CollectionLeaf,
        TagSet::VALUE,
        Optionality::Bare,
    )
    .types(type_count)
    .enums(enum_count)
    .collections(row)
}

pub(super) fn decode_types(
    body: &[u8],
    strings: &[Rc<str>],
) -> Result<Vec<SealedRecordType>, VerifyRejection> {
    let string_count = strings.len();
    let mut reader = Reader::new(body);
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Types)))?
        as usize;
    if count > marrow_image::bounds::MAX_TYPES {
        return Err(reject(
            VerifyPhase::Table,
            Kind::OverBound(Bound::RecordTypes),
        ));
    }
    let mut types = Vec::with_capacity(count);
    let mut names = NameMarks::new(string_count);
    for _ in 0..count {
        let name = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Types)))?;
        if name as usize >= string_count {
            return Err(reject(VerifyPhase::Table, Kind::OutOfRange(Ref::String)));
        }
        let field_count = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Types)))?
            as usize;
        if field_count > marrow_image::bounds::MAX_RECORD_FIELDS {
            return Err(reject(VerifyPhase::Table, Kind::OverBound(Bound::Fields)));
        }
        names.begin_row();
        let mut fields = Vec::with_capacity(field_count);
        for _ in 0..field_count {
            let fname = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Types)))?;
            if fname as usize >= string_count {
                return Err(reject(VerifyPhase::Table, Kind::OutOfRange(Ref::String)));
            }
            if names.insert(fname) {
                return Err(reject(
                    VerifyPhase::Table,
                    Kind::Duplicate(Duplicate::FieldName),
                ));
            }
            // A field is a scalar leaf (durable-storable) or a closed enum, record
            // or collection value; sparseness is the `required` flag, never the
            // optional bit. The referenced indices are read before the tables that
            // bound them exist, so `validate_record_field_refs` range-checks them.
            let ty = decode_type_ref(&mut reader, &FIELD)?;
            let required_byte = reader
                .u8()
                .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Types)))?;
            let required = match required_byte {
                0 => false,
                1 => true,
                _ => {
                    return Err(reject(VerifyPhase::Table, Kind::Flag(Flag::FieldRequired)));
                }
            };
            fields.push(SealedField {
                name: strings[fname as usize].clone(),
                ty,
                required,
            });
        }
        types.push(SealedRecordType { fields });
    }
    if !reader.is_empty() {
        return Err(reject(VerifyPhase::Table, Kind::Trailing(Region::Types)));
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
    strings: &[Rc<str>],
    type_count: usize,
) -> Result<Vec<SealedEnumType>, VerifyRejection> {
    let string_count = strings.len();
    let mut reader = Reader::new(body);
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Enums)))?
        as usize;
    if count > marrow_image::bounds::MAX_ENUMS {
        return Err(reject(VerifyPhase::Table, Kind::OverBound(Bound::Enums)));
    }
    let mut enums = Vec::with_capacity(count);
    let mut names = NameMarks::new(string_count);
    for _ in 0..count {
        let name = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Enums)))?;
        if name as usize >= string_count {
            return Err(reject(VerifyPhase::Table, Kind::OutOfRange(Ref::String)));
        }
        let variant_count = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Enums)))?
            as usize;
        if variant_count > marrow_image::bounds::MAX_VARIANTS {
            return Err(reject(VerifyPhase::Table, Kind::OverBound(Bound::Variants)));
        }
        names.begin_row();
        let mut variants = Vec::with_capacity(variant_count);
        for _ in 0..variant_count {
            let vname = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Enums)))?;
            if vname as usize >= string_count {
                return Err(reject(VerifyPhase::Table, Kind::OutOfRange(Ref::String)));
            }
            if names.insert(vname) {
                return Err(reject(
                    VerifyPhase::Table,
                    Kind::Duplicate(Duplicate::VariantName),
                ));
            }
            let category_byte = reader
                .u8()
                .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Enums)))?;
            let category = match category_byte {
                0 => false,
                1 => true,
                _ => {
                    return Err(reject(
                        VerifyPhase::Table,
                        Kind::Flag(Flag::VariantCategory),
                    ));
                }
            };
            let payload_count = reader
                .u8()
                .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Enums)))?
                as usize;
            if payload_count > marrow_image::bounds::MAX_PAYLOAD_FIELDS {
                return Err(reject(
                    VerifyPhase::Table,
                    Kind::OverBound(Bound::PayloadFields),
                ));
            }
            let mut payload = Vec::with_capacity(payload_count);
            for _ in 0..payload_count {
                payload.push(decode_type_ref(
                    &mut reader,
                    &PAYLOAD_LEAF.types(type_count).enums(count),
                )?);
            }
            variants.push(SealedVariant {
                name: strings[vname as usize].clone(),
                category,
                payload,
            });
        }
        enums.push(SealedEnumType {
            name: strings[name as usize].clone(),
            variants,
        });
    }
    if !reader.is_empty() {
        return Err(reject(VerifyPhase::Table, Kind::Trailing(Region::Enums)));
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
    let count = reader.u16().ok_or(reject(
        VerifyPhase::Table,
        Kind::Truncated(Region::Collections),
    ))? as usize;
    if count > marrow_image::bounds::MAX_COLLECTIONS {
        return Err(reject(
            VerifyPhase::Table,
            Kind::OverBound(Bound::Collections),
        ));
    }
    let mut collections = Vec::with_capacity(count);
    for row in 0..count {
        let kind = reader.u8().ok_or(reject(
            VerifyPhase::Table,
            Kind::Truncated(Region::Collections),
        ))?;
        let coll = match kind {
            0x00 => {
                let elem =
                    decode_type_ref(&mut reader, &collection_leaf(type_count, enum_count, row))?;
                SealedCollectionType::List { elem }
            }
            0x01 => {
                let key =
                    decode_type_ref(&mut reader, &collection_leaf(type_count, enum_count, row))?;
                if !matches!(
                    key,
                    ImageType::Scalar {
                        optional: false,
                        ..
                    }
                ) {
                    return Err(reject(VerifyPhase::Table, Kind::MapKeyNotScalar));
                }
                let value =
                    decode_type_ref(&mut reader, &collection_leaf(type_count, enum_count, row))?;
                SealedCollectionType::Map { key, value }
            }
            _ => {
                return Err(reject(
                    VerifyPhase::Table,
                    Kind::Unknown(Tag::CollectionKind),
                ));
            }
        };
        collections.push(coll);
    }
    if !reader.is_empty() {
        return Err(reject(
            VerifyPhase::Table,
            Kind::Trailing(Region::Collections),
        ));
    }
    Ok(collections)
}

/// Bounds-check every record field's referenced value type against the decoded
/// tables: an enum-typed field against the ENUMS table and a record-typed field
/// (a struct-valued field) against the RECORD-TYPES table. The field decoder reads
/// each index before the referenced table exists, so this runs once both tables are
/// decoded. Cycles among the in-range references are rejected separately.
pub(super) fn validate_record_field_refs(
    types: &[SealedRecordType],
    enum_count: usize,
    collection_count: usize,
) -> Result<(), VerifyRejection> {
    for record in types {
        for field in &record.fields {
            match field.ty {
                ImageType::Enum { idx, .. } if idx.index() as usize >= enum_count => {
                    return Err(reject(VerifyPhase::Table, Kind::OutOfRange(Ref::Enum)));
                }
                ImageType::Record { idx, .. } if idx.index() as usize >= types.len() => {
                    return Err(reject(
                        VerifyPhase::Table,
                        Kind::OutOfRange(Ref::RecordType),
                    ));
                }
                ImageType::Collection { idx, .. } if idx.index() as usize >= collection_count => {
                    return Err(reject(
                        VerifyPhase::Table,
                        Kind::OutOfRange(Ref::Collection),
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
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
) -> Result<(), VerifyRejection> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Colour {
        White,
        Gray,
        Black,
    }
    let record_count = types.len();
    let enum_node = |idx: EnumId| record_count + idx.index() as usize;
    let mut edges: Vec<Vec<usize>> = Vec::with_capacity(record_count + enums.len());
    for record in types {
        edges.push(
            record
                .fields
                .iter()
                .filter_map(|field| match field.ty {
                    ImageType::Enum { idx, .. } => Some(enum_node(idx)),
                    ImageType::Record { idx, .. } => Some(idx.index() as usize),
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
                    ImageType::Record { idx, .. } => Some(idx.index() as usize),
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
                        return Err(reject(VerifyPhase::Table, Kind::ValueTypeCycle));
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
