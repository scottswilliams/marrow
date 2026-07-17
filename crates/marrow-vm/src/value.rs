//! The runtime value model (design §D).
//!
//! The vacant state of an optional is the typed `Optional(None)`; there is no
//! dedicated absent value variant. Records and optionals arrive with their
//! slices; the current subset uses the scalar values.

use std::rc::Rc;

use marrow_kernel::codec::key::KeyScalar;

/// A runtime value.
///
/// A record carries its type index and one slot per field in declared order; a
/// required field's slot is always present, a sparse field's slot may be vacant.
/// An optional's vacant state is `Optional(None)` — there is no separate absent
/// variant.
///
/// An enum value carries its enum-type index, its selected variant index, and one
/// slot per dense payload leaf in declaration order (empty for a payloadless
/// member). Equality is exact — same variant and equal payload — so it derives
/// structurally.
///
/// A `List` carries its COLLTYPES index, its cached aggregate structural byte size,
/// and its elements in insertion order. A `Map` carries its COLLTYPES index, the same
/// cached aggregate size, and its entries kept sorted by `CollectionKeyOrder` (the
/// kernel's `KeyScalar` order), so positional access yields key order and a lookup is
/// a binary search. Both are ordinary copied values; the shared `Rc` backing gives
/// copy-on-write growth (`Rc::make_mut`) rather than a copy per read.
///
/// The cached size is the aggregate structural byte size of the elements (a `List`) or
/// the key/value pairs (a `Map`), the quantity the collection-limit law bounds. It is
/// a deterministic function of the contents, maintained by the [`Value::list`] and
/// [`Value::map`] constructors and by incremental `append`/`insert` updates, so
/// growing a collection is amortized `O(1)` rather than an `O(n)` re-measure per step.
/// Equality ignores it (see the manual [`PartialEq`]); it is a memoized measurement,
/// not part of a value's identity.
#[derive(Debug, Clone, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Text(Rc<str>),
    Bytes(Rc<[u8]>),
    /// A `date`, held as days since the Unix epoch (1970-01-01).
    Date(i32),
    /// An `instant`, held as signed nanoseconds since the epoch.
    Instant(i128),
    /// A `duration`, held as a signed count of nanoseconds.
    Duration(i128),
    Record(u16, Box<[Option<Value>]>),
    Optional(Option<Box<Value>>),
    Enum(u16, u16, Box<[Value]>),
    List(u16, usize, Rc<Vec<Value>>),
    Map(u16, usize, Rc<Vec<(KeyScalar, Value)>>),
    /// An entry identity `Id(^root)`: its ROOTS-table root index and the key tuple
    /// (one [`KeyScalar`] per key column, in declaration order) that addresses one
    /// entry. Equality is root plus key-tuple equality — the runtime half of the
    /// kernel's `ValueDomain::Identity` specification. Not a durable cell value here.
    Id(u16, Rc<[KeyScalar]>),
}

impl PartialEq for Value {
    /// Structural equality over contents; the cached collection size is a
    /// deterministic memoized measurement and never participates. This is the one
    /// runtime equality relation, held to agree with the kernel's `value_equality`
    /// (see `tests/equality_agreement.rs`).
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Text(a), Value::Text(b)) => a == b,
            (Value::Bytes(a), Value::Bytes(b)) => a == b,
            (Value::Date(a), Value::Date(b)) => a == b,
            (Value::Instant(a), Value::Instant(b)) => a == b,
            (Value::Duration(a), Value::Duration(b)) => a == b,
            (Value::Record(ta, a), Value::Record(tb, b)) => ta == tb && a == b,
            (Value::Optional(a), Value::Optional(b)) => a == b,
            (Value::Enum(ta, va, pa), Value::Enum(tb, vb, pb)) => ta == tb && va == vb && pa == pb,
            (Value::List(ia, _, a), Value::List(ib, _, b)) => ia == ib && a == b,
            (Value::Map(ia, _, a), Value::Map(ib, _, b)) => ia == ib && a == b,
            (Value::Id(ra, a), Value::Id(rb, b)) => ra == rb && a == b,
            _ => false,
        }
    }
}

impl Value {
    /// Construct a list, measuring and caching its aggregate structural byte size.
    pub fn list(idx: u16, items: Rc<Vec<Value>>) -> Value {
        let bytes = list_bytes(&items);
        Value::List(idx, bytes, items)
    }

    /// Construct a map, measuring and caching its aggregate structural byte size.
    pub fn map(idx: u16, entries: Rc<Vec<(KeyScalar, Value)>>) -> Value {
        let bytes = map_bytes(&entries);
        Value::Map(idx, bytes, entries)
    }

    /// The structural byte size of this value: a cheap measured size (scalar payload
    /// bytes plus one byte of framing per node) that bounds runtime memory without
    /// depending on any wire codec. A collection reads its cached aggregate, so the
    /// measure is `O(1)` in a collection's element count.
    pub fn structural_bytes(&self) -> usize {
        match self {
            Value::Int(_) => 8,
            Value::Bool(_) => 1,
            Value::Text(text) => text.len(),
            Value::Bytes(bytes) => bytes.len(),
            Value::Date(_) => 8,
            Value::Instant(_) | Value::Duration(_) => 16,
            Value::Optional(None) => 1,
            Value::Optional(Some(inner)) => 1 + inner.structural_bytes(),
            Value::Record(_, slots) => {
                1 + slots
                    .iter()
                    .map(|slot| slot.as_ref().map_or(1, Value::structural_bytes))
                    .sum::<usize>()
            }
            Value::Enum(_, _, payload) => {
                1 + payload.iter().map(Value::structural_bytes).sum::<usize>()
            }
            Value::List(_, bytes, _) | Value::Map(_, bytes, _) => 1 + bytes,
            Value::Id(_, keys) => 1 + keys.iter().map(key_bytes).sum::<usize>(),
        }
    }
}

/// The aggregate structural byte size of a list's elements (the collection-limit
/// law's measured quantity, excluding the one-byte list framing).
pub fn list_bytes(items: &[Value]) -> usize {
    items.iter().map(Value::structural_bytes).sum()
}

/// The aggregate structural byte size of a map's key/value pairs.
pub fn map_bytes(entries: &[(KeyScalar, Value)]) -> usize {
    entries
        .iter()
        .map(|(key, value)| key_bytes(key) + value.structural_bytes())
        .sum()
}

/// The structural byte size of a map key.
pub fn key_bytes(key: &KeyScalar) -> usize {
    match key {
        KeyScalar::Bool(_) => 1,
        KeyScalar::Int(_) | KeyScalar::Date(_) => 8,
        KeyScalar::Duration(_) | KeyScalar::Instant(_) => 16,
        KeyScalar::Str(text) => text.len(),
        KeyScalar::Bytes(bytes) => bytes.len(),
    }
}
