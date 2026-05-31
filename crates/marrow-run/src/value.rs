//! Runtime values and their conversions to and from saved values.

use crate::*;

/// A runtime value: the scalars a pure function manipulates plus the in-memory
/// and saved-tree shapes the data features produce (sequences, resource trees,
/// identities).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Str(String),
    /// A UTC instant in nanoseconds since the Unix epoch, e.g. from
    /// `std::clock::now()`. Saves and loads as the `instant` type.
    Instant(i128),
    /// A UTC calendar date as days since the Unix epoch, e.g. from
    /// `std::clock::today()`. Saves and loads as the `date` type.
    Date(i32),
    /// A signed time span in nanoseconds, e.g. from `std::clock::parseDuration`.
    /// Saves and loads as the `duration` type.
    Duration(i128),
    /// An exact base-10 decimal. Saves and loads as the `decimal` type.
    Decimal(Decimal),
    /// Arbitrary bytes. Saves and loads as the `bytes` type; has no direct text
    /// form (use `std::bytes::base64Encode`).
    Bytes(Vec<u8>),
    /// An ordered, in-memory `sequence[T]` value, e.g. from `std::text::split`.
    /// Iterated by a `for` loop; not itself a scalar saved value.
    Sequence(Vec<Value>),
    /// A materialized resource tree: its present top-level fields, in schema
    /// order. Produced by a whole-resource read and consumed by a whole-resource
    /// write or `merge`.
    Resource(Vec<(String, Value)>),
    /// A composite resource identity (`Enrollment::Id(...)`): its lowered key
    /// segments in declared identity-key order. Single-key identities use their
    /// key's scalar value, matching `nextId` and root traversal. Composite
    /// identities use this wrapper so a keyed lookup can splice all segments.
    ///
    /// The owning resource is not carried here: two identities with the same key
    /// scalars are byte-identical, so `Book::Id(1)` and `Magazine::Id(1)` are one
    /// value. Nominal identity is enforced statically by the checker and again at
    /// lowering against the declared key types, which covers well-typed programs;
    /// the residual — a value that already lost its nominal resource through
    /// dynamic code — waits on unifying the type IR so the value can name its
    /// resource.
    Identity(Vec<SavedKey>),
}

impl Value {
    /// A total, never-faulting one-line rendering for a debugger's Variables view.
    /// Scalars render exactly as the normal text renderer does (so a debugged
    /// value reads like a printed one); the shapes the normal renderer refuses
    /// (bytes, sequences, resources, identities) get a compact structural preview
    /// instead of a fault, since the debugger must display every local. This is a
    /// preview, not a re-parseable form.
    pub fn display_debug(&self) -> String {
        match self {
            Value::Int(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Str(s) => s.clone(),
            Value::Decimal(d) => d.to_text(),
            Value::Instant(n) => format!("instant({n})"),
            Value::Date(d) => format!("date({d})"),
            Value::Duration(n) => format!("duration({n})"),
            Value::Bytes(bytes) => format!("bytes[{}]", bytes.len()),
            Value::Sequence(items) => format!("sequence[{}]", items.len()),
            // Preview the present field names, in schema order, without recursing
            // into their values (which could be large or nested resources).
            Value::Resource(fields) => {
                let names: Vec<&str> = fields.iter().map(|(name, _)| name.as_str()).collect();
                format!("resource{{{}}}", names.join(", "))
            }
            // Preview the identity's lowered key segments.
            Value::Identity(keys) => {
                let rendered: Vec<String> = keys.iter().map(saved_key_preview).collect();
                format!("identity({})", rendered.join(", "))
            }
        }
    }
}

/// A compact preview of one identity key segment for [`Value::display_debug`].
pub(crate) fn saved_key_preview(key: &SavedKey) -> String {
    match key {
        SavedKey::Int(n) => n.to_string(),
        SavedKey::Bool(b) => b.to_string(),
        SavedKey::Str(s) => s.clone(),
        SavedKey::Date(d) => format!("date({d})"),
        SavedKey::Duration(n) => format!("duration({n})"),
        SavedKey::Instant(n) => format!("instant({n})"),
        SavedKey::Bytes(bytes) => format!("bytes[{}]", bytes.len()),
    }
}

/// The result of running an entry function: its returned value (if any) and
/// everything it wrote to the output stream via `print`/`write`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub value: Option<Value>,
    pub output: String,
}

/// Convert a child key to a runtime value, or `None` for a key type this
/// conversion does not produce (the temporal keys: date, duration, instant).
pub(crate) fn saved_key_to_value(key: SavedKey) -> Option<Value> {
    match key {
        SavedKey::Int(n) => Some(Value::Int(n)),
        SavedKey::Bool(b) => Some(Value::Bool(b)),
        SavedKey::Str(s) => Some(Value::Str(s)),
        SavedKey::Bytes(b) => Some(Value::Bytes(b)),
        _ => None,
    }
}

/// The runtime value for an identity's lowered keys. Single-key identities use
/// the key's scalar shape, matching `nextId` and root traversal; composite
/// identities need the opaque wrapper so they can splice all key segments later.
pub(crate) fn identity_value(keys: Vec<SavedKey>) -> Value {
    match keys.as_slice() {
        [SavedKey::Int(n)] => Value::Int(*n),
        [SavedKey::Bool(b)] => Value::Bool(*b),
        [SavedKey::Str(s)] => Value::Str(s.clone()),
        [SavedKey::Bytes(bytes)] => Value::Bytes(bytes.clone()),
        [SavedKey::Instant(n)] => Value::Instant(*n),
        [SavedKey::Date(d)] => Value::Date(*d),
        [SavedKey::Duration(n)] => Value::Duration(*n),
        _ => Value::Identity(keys),
    }
}

/// Convert a runtime value to the saved value a managed write stores. Total over
/// the scalar values; trees and identities have no scalar saved form. The write
/// planner checks the value against the field's declared type.
pub(crate) fn value_to_saved(value: Value) -> Option<SavedValue> {
    Some(match value {
        Value::Int(n) => SavedValue::Int(n),
        Value::Bool(b) => SavedValue::Bool(b),
        Value::Str(s) => SavedValue::Str(s),
        Value::Instant(n) => SavedValue::Instant(n),
        Value::Date(d) => SavedValue::Date(d),
        Value::Duration(n) => SavedValue::Duration(n),
        Value::Decimal(d) => SavedValue::Decimal(d),
        Value::Bytes(b) => SavedValue::Bytes(b),
        // A whole sequence or resource is a tree, not a scalar saved value; an
        // identity is opaque and is not stored as a field value.
        Value::Sequence(_) | Value::Resource(_) | Value::Identity(_) => return None,
    })
}

/// The identity key segments a typed-reference field stores. A composite identity
/// arrives as a [`Value::Identity`]; a single-key identity collapses to its bare key
/// value at runtime — the same way `nextId` and a single-key record lookup do — so a
/// scalar value is taken as that one key. A non-key value is a type fault.
///
/// The checker's nominal rule already rejects a wrong-resource or scalar value
/// statically; this guards the well-typed path and gives a catchable `run.type`
/// fault for a value that lost its nominal type through dynamic code, and the write
/// planner's arity check rejects a single key written to a composite reference.
pub(crate) fn identity_keys_of(
    value: Value,
    span: SourceSpan,
) -> Result<Vec<SavedKey>, RuntimeError> {
    match value {
        Value::Identity(keys) => Ok(keys),
        other => match value_to_key(other) {
            Some(key) => Ok(vec![key]),
            None => Err(type_error(
                "an identity-typed field takes a resource identity (`Resource::Id(...)`)",
                span,
            )),
        },
    }
}

/// Convert a record-key value to a [`SavedKey`], or `None` for a type that is not
/// a valid key (decimals, sequences, resources, and identities are not keys).
pub(crate) fn value_to_key(value: Value) -> Option<SavedKey> {
    match value {
        Value::Int(n) => Some(SavedKey::Int(n)),
        Value::Bool(b) => Some(SavedKey::Bool(b)),
        Value::Str(s) => Some(SavedKey::Str(s)),
        Value::Instant(n) => Some(SavedKey::Instant(n)),
        Value::Date(d) => Some(SavedKey::Date(d)),
        Value::Duration(n) => Some(SavedKey::Duration(n)),
        Value::Bytes(b) => Some(SavedKey::Bytes(b)),
        // Decimal keys are deferred; sequences and resources are not scalar keys.
        // An identity is not a single key — lowering splices its segments in
        // before reaching here.
        Value::Decimal(_) | Value::Sequence(_) | Value::Resource(_) | Value::Identity(_) => None,
    }
}

/// Decode a stored leaf's bytes to its runtime value by the leaf's kind: a scalar
/// leaf through the canonical scalar codec, a typed-reference leaf through
/// `decode_identity` against the referenced identity arity. `None` when the bytes
/// are not a canonical form for the leaf.
pub(crate) fn decode_leaf(bytes: &[u8], leaf: &LeafKind) -> Option<Value> {
    match leaf {
        LeafKind::Scalar(ty) => decode_value(bytes, *ty).map(saved_value_to_value),
        LeafKind::Identity { arity, .. } => {
            decode_identity_arity(bytes, *arity).map(identity_value)
        }
    }
}

/// Convert a decoded saved value to its runtime value. Total: every scalar has a
/// runtime form.
pub(crate) fn saved_value_to_value(value: SavedValue) -> Value {
    match value {
        SavedValue::Int(n) => Value::Int(n),
        SavedValue::Bool(b) => Value::Bool(b),
        SavedValue::Str(s) => Value::Str(s),
        SavedValue::Instant(n) => Value::Instant(n),
        SavedValue::Date(d) => Value::Date(d),
        SavedValue::Duration(n) => Value::Duration(n),
        SavedValue::Decimal(d) => Value::Decimal(d),
        SavedValue::Bytes(b) => Value::Bytes(b),
    }
}

/// Render a scalar value as text: integers in decimal, booleans as
/// `true`/`false`, strings as themselves. Resource values have no text form, and
/// an instant is rendered through `std::clock::formatInstant`, not directly.
pub(crate) fn render(value: Value, span: SourceSpan) -> Result<String, RuntimeError> {
    Ok(match value {
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Str(s) => s,
        Value::Decimal(d) => d.to_text(),
        Value::Bytes(_) => return Err(unsupported("rendering a bytes value", span)),
        Value::Sequence(_) => return Err(unsupported("rendering a sequence value", span)),
        Value::Instant(_) => return Err(unsupported("rendering an instant value", span)),
        Value::Date(_) => return Err(unsupported("rendering a date value", span)),
        Value::Duration(_) => return Err(unsupported("rendering a duration value", span)),
        Value::Resource(_) => return Err(unsupported("rendering a resource value", span)),
        Value::Identity(_) => return Err(unsupported("rendering an identity value", span)),
    })
}
