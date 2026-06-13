//! Runtime values and their conversions to and from saved values.

use marrow_store::Decimal;
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, decode_identity_payload_arity, encode_identity_index_key};
use marrow_store::tree::{TreeEnumMember, decode_tree_enum_member, encode_tree_enum_member};
use marrow_store::value::{SavedValue, ScalarType, decode_value, encode_value};
use marrow_syntax::SourceSpan;

use marrow_check::{
    CheckedEnumRef, CheckedFacts, CheckedRuntimeProgram, EnumId, EnumMemberId, StoreLeafKind,
    StoredValueMeaning,
};

use crate::error::{Located, RuntimeError, type_error, unsupported};

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
    Enum(EnumValue),
    /// An ordered, in-memory `sequence[T]` value, e.g. from `std::text::split`.
    /// Iterated by a `for` loop; not itself a scalar saved value.
    Sequence(Vec<Value>),
    LocalTree(Vec<LocalTreeEntry>),
    /// A materialized resource tree: its present top-level fields, in schema
    /// order. Produced by a whole-resource read and consumed by a whole-resource
    /// write.
    Resource(Vec<(String, Value)>),
    /// A store identity: its checked root plus lowered key segments in declared order.
    Identity(IdentityValue),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityValue {
    root: String,
    keys: Vec<SavedKey>,
}

impl IdentityValue {
    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn keys(&self) -> &[SavedKey] {
        &self.keys
    }

    pub(crate) fn for_root(root: impl Into<String>, keys: Vec<SavedKey>) -> Self {
        Self {
            root: root.into(),
            keys,
        }
    }

    pub(crate) fn into_keys_for_root(
        self,
        expected_root: &str,
        span: SourceSpan,
    ) -> Result<Vec<SavedKey>, RuntimeError> {
        if self.root != expected_root {
            return Err(type_error(
                &format!("this identity belongs to a different store than `^{expected_root}`"),
                span,
            ));
        }
        Ok(self.keys)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumValue {
    pub(crate) enum_id: EnumId,
    pub(crate) member_id: EnumMemberId,
    pub(crate) enum_catalog_id: String,
    pub(crate) member_catalog_id: String,
    display_name: String,
}

impl EnumValue {
    pub fn enum_id(&self) -> EnumId {
        self.enum_id
    }

    pub fn member_id(&self) -> EnumMemberId {
        self.member_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LeafValue {
    Scalar(SavedValue),
    Enum { bytes: Vec<u8>, index_key: SavedKey },
}

impl LeafValue {
    pub(crate) fn bytes(&self) -> Result<Vec<u8>, marrow_store::value::ValueError> {
        match self {
            Self::Scalar(value) => encode_value(value),
            Self::Enum { bytes, .. } => Ok(bytes.clone()),
        }
    }

    pub(crate) fn as_key(&self) -> Option<SavedKey> {
        match self {
            Self::Scalar(value) => value.as_key(),
            Self::Enum { index_key, .. } => Some(index_key.clone()),
        }
    }

    pub(crate) fn is_enum(&self) -> bool {
        matches!(self, Self::Enum { .. })
    }
}

impl Value {
    /// A total, never-faulting one-line rendering for a debugger's Variables view.
    /// Scalars render as the normal text renderer does; the shapes it refuses get
    /// a compact structural preview instead of a fault. A preview, not re-parseable.
    pub fn display_debug(&self) -> String {
        scalar_text(self).unwrap_or_else(|| match self {
            Value::Instant(n) => format!("instant({n})"),
            Value::Date(d) => format!("date({d})"),
            Value::Duration(n) => format!("duration({n})"),
            Value::Bytes(bytes) => format!("bytes[{}]", bytes.len()),
            Value::Enum(value) => value.display_name.clone(),
            Value::Sequence(items) => format!("sequence[{}]", items.len()),
            Value::LocalTree(entries) => format!("tree[{}]", entries.len()),
            // Field names only, in schema order; values may be large or nested.
            Value::Resource(fields) => {
                let names: Vec<&str> = fields.iter().map(|(name, _)| name.as_str()).collect();
                format!("resource{{{}}}", names.join(", "))
            }
            Value::Identity(identity) => render_debug_identity(identity),
            Value::Int(_) | Value::Bool(_) | Value::Str(_) | Value::Decimal(_) => {
                unreachable!("scalar_text rendered every scalar before this match")
            }
        })
    }
}

/// The single owner of the scalar-to-text mapping: canonical text for the scalars
/// that render identically in a debugger preview and the text renderer, `None` for
/// the shapes that render differently in each.
fn scalar_text(value: &Value) -> Option<String> {
    Some(match value {
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Str(s) => s.clone(),
        Value::Decimal(d) => d.to_text(),
        Value::Bytes(_)
        | Value::Instant(_)
        | Value::Date(_)
        | Value::Duration(_)
        | Value::Enum(_)
        | Value::Sequence(_)
        | Value::LocalTree(_)
        | Value::Resource(_)
        | Value::Identity(_) => return None,
    })
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

fn render_debug_identity(identity: &IdentityValue) -> String {
    let rendered: Vec<String> = identity.keys.iter().map(saved_key_preview).collect();
    format!("^{}({})", identity.root, rendered.join(", "))
}

/// Receives an entry function's `print` output as the run produces it.
pub trait RunOutputSink {
    fn write(&mut self, text: &str);
}

impl<F> RunOutputSink for F
where
    F: FnMut(&str),
{
    fn write(&mut self, text: &str) {
        self(text);
    }
}

impl RunOutputSink for String {
    fn write(&mut self, text: &str) {
        self.push_str(text);
    }
}

/// An entry function's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub value: Option<Value>,
}

/// A child key as its runtime value. Every saved key kind converts — including
/// the temporal date, duration, and instant keys — so a layer keyed by any scalar
/// type iterates to usable values.
pub(crate) fn saved_key_to_value(key: SavedKey) -> Value {
    match key {
        SavedKey::Int(n) => Value::Int(n),
        SavedKey::Bool(b) => Value::Bool(b),
        SavedKey::Str(s) => Value::Str(s),
        SavedKey::Date(d) => Value::Date(d),
        SavedKey::Duration(n) => Value::Duration(n),
        SavedKey::Instant(n) => Value::Instant(n),
        SavedKey::Bytes(b) => Value::Bytes(b),
    }
}

/// The runtime value for an identity's lowered keys. Every identity carries its
/// checked store root, including single-key identities, so dynamic and host
/// boundaries cannot confuse a raw scalar key with an `Id(^store)`.
pub(crate) fn identity_value(root: &str, keys: Vec<SavedKey>) -> Value {
    Value::Identity(IdentityValue::for_root(root, keys))
}

/// The scalar type of a scalar runtime value, or `None` for the composite shapes
/// (enums, sequences, trees, resources, identities) that are not scalars. The
/// single owner of value-to-scalar-type classification across the runtime.
pub(crate) fn value_scalar_type(value: &Value) -> Option<ScalarType> {
    Some(match value {
        Value::Int(_) => ScalarType::Int,
        Value::Bool(_) => ScalarType::Bool,
        Value::Str(_) => ScalarType::Str,
        Value::Instant(_) => ScalarType::Instant,
        Value::Date(_) => ScalarType::Date,
        Value::Duration(_) => ScalarType::Duration,
        Value::Decimal(_) => ScalarType::Decimal,
        Value::Bytes(_) => ScalarType::Bytes,
        Value::Enum(_)
        | Value::Sequence(_)
        | Value::LocalTree(_)
        | Value::Resource(_)
        | Value::Identity(_) => return None,
    })
}

/// The canonical text of a saved scalar. The canonical byte encoding is always
/// valid UTF-8, so the only failure is an out-of-range temporal value that cannot
/// be formatted, which surfaces as a located error.
pub(crate) fn canonical_scalar_text(
    value: SavedValue,
    span: SourceSpan,
) -> Result<String, RuntimeError> {
    let bytes = encode_value(&value).map_err(|error| error.located(span))?;
    Ok(String::from_utf8(bytes).expect("a canonical scalar encodes as UTF-8 text"))
}

/// Convert a runtime value to the saved scalar a managed write stores. Enum
/// members need checked leaf context and are lowered by [`value_to_leaf`].
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
        Value::Sequence(_)
        | Value::LocalTree(_)
        | Value::Resource(_)
        | Value::Identity(_)
        | Value::Enum(_) => return None,
    })
}

/// The identity key segments a typed-reference field stores. Every identity
/// arrives as a [`Value::Identity`] carrying checked root provenance; raw scalar
/// keys are not accepted as identity values at dynamic runtime boundaries.
pub(crate) fn identity_keys_of(
    value: Value,
    expected_root: &str,
    span: SourceSpan,
) -> Result<Vec<SavedKey>, RuntimeError> {
    match value {
        Value::Identity(identity) => identity.into_keys_for_root(expected_root, span),
        _ => Err(type_error(
            "an identity-typed field takes an Id(^store) value",
            span,
        )),
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
        Value::Enum(value) => Some(SavedKey::Str(value.member_catalog_id)),
        // Decimal keys are deferred; sequences and resources are not scalar keys.
        // An identity is not a single key — lowering splices its segments in
        // before reaching here.
        Value::Decimal(_)
        | Value::Sequence(_)
        | Value::Resource(_)
        | Value::Identity(_)
        | Value::LocalTree(_) => None,
    }
}

pub(crate) fn value_to_index_key(
    value: Value,
    meaning: &StoredValueMeaning,
    span: SourceSpan,
) -> Result<SavedKey, RuntimeError> {
    match meaning {
        StoredValueMeaning::Identity {
            root,
            store_catalog_id,
            ..
        } => {
            let Value::Identity(identity) = value else {
                return Err(type_error("this index key takes an identity value", span));
            };
            let keys = identity.into_keys_for_root(root, span)?;
            let Some(store_catalog_id) = store_catalog_id.as_deref() else {
                return Err(unsupported(
                    "this identity index key before catalog activation",
                    span,
                ));
            };
            Ok(SavedKey::Bytes(encode_identity_index_key(
                store_catalog_id,
                &keys,
            )))
        }
        StoredValueMeaning::Scalar(_) | StoredValueMeaning::Enum { .. } => {
            value_to_key(value).ok_or_else(|| unsupported("an index key of this type", span))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTreeEntry {
    pub keys: Vec<SavedKey>,
    pub value: Value,
}

/// Decode a stored leaf's bytes to its runtime value by the leaf's kind: a scalar
/// leaf through the canonical scalar codec, a typed-reference leaf through
/// `decode_identity` against the referenced identity arity. `None` when the bytes
/// are not a canonical form for the leaf.
pub(crate) fn value_to_leaf(
    value: Value,
    leaf: &StoreLeafKind,
    span: SourceSpan,
) -> Result<LeafValue, RuntimeError> {
    match leaf {
        StoreLeafKind::Scalar(_) => value_to_saved(value)
            .map(LeafValue::Scalar)
            .ok_or_else(|| unsupported("writing this value to a scalar field", span)),
        StoreLeafKind::Enum { enum_id } => enum_value_to_leaf(value, *enum_id, span),
        StoreLeafKind::Identity { .. } => Err(unsupported("writing this identity field", span)),
    }
}

fn enum_value_to_leaf(
    value: Value,
    expected: EnumId,
    span: SourceSpan,
) -> Result<LeafValue, RuntimeError> {
    match value {
        Value::Enum(value) if value.enum_id == expected => {
            let enum_catalog = catalog_id(&value.enum_catalog_id, span)?;
            let member_catalog = catalog_id(&value.member_catalog_id, span)?;
            let bytes = encode_tree_enum_member(&TreeEnumMember::new(enum_catalog, member_catalog))
                .map_err(|error| error.located(span))?;
            Ok(LeafValue::Enum {
                index_key: SavedKey::Str(value.member_catalog_id),
                bytes,
            })
        }
        Value::Enum(_) => Err(type_error("this field takes a different enum", span)),
        _ => Err(type_error("this field takes an enum value", span)),
    }
}

fn catalog_id(raw: &str, span: SourceSpan) -> Result<CatalogId, RuntimeError> {
    CatalogId::new(raw.to_string()).map_err(|_| unsupported("this enum catalog id", span))
}

pub(crate) fn decode_leaf(
    program: &CheckedRuntimeProgram,
    bytes: &[u8],
    leaf: &StoreLeafKind,
) -> Option<Value> {
    match leaf {
        StoreLeafKind::Scalar(ty) => decode_value(bytes, *ty).map(saved_value_to_value),
        StoreLeafKind::Enum { enum_id } => decode_enum(program, bytes, *enum_id).map(Value::Enum),
        StoreLeafKind::Identity { store_root, arity } => {
            let keys = decode_identity_payload_arity(bytes, *arity)?;
            let store = program.facts().store_by_root(store_root)?;
            store
                .identity_keys_match(&keys)
                .then(|| identity_value(store_root, keys))
        }
    }
}

fn decode_enum(
    program: &CheckedRuntimeProgram,
    bytes: &[u8],
    enum_id: EnumId,
) -> Option<EnumValue> {
    let stored = decode_tree_enum_member(bytes).ok()?;
    let enum_fact = program.facts().enum_(enum_id)?;
    if enum_fact.catalog_id.as_deref() != Some(stored.enum_id().as_str()) {
        return None;
    }
    let member = program.facts().enum_members().iter().find(|member| {
        member.enum_id == enum_id
            && member.catalog_id.as_deref() == Some(stored.member_id().as_str())
    })?;
    enum_value_from_member(program.facts(), member.id)
}

pub(crate) fn enum_value_from_member(
    facts: &CheckedFacts,
    member_id: EnumMemberId,
) -> Option<EnumValue> {
    let member = facts.enum_member(member_id)?;
    let enum_fact = facts.enum_(member.enum_id)?;
    if !facts.enum_member_is_selectable(member_id) {
        return None;
    }
    Some(EnumValue {
        enum_id: member.enum_id,
        member_id,
        enum_catalog_id: enum_fact.catalog_id.clone()?,
        member_catalog_id: member.catalog_id.clone()?,
        display_name: facts.enum_member_catalog_path(member_id)?,
    })
}

pub(crate) fn enum_id_from_ref(enum_ref: CheckedEnumRef) -> EnumId {
    enum_ref.enum_id
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
    if let Some(text) = scalar_text(&value) {
        return Ok(text);
    }
    match value {
        Value::Identity(identity) => Ok(render_identity(&identity)),
        Value::Bytes(_) => Err(unsupported("rendering a bytes value", span)),
        Value::Sequence(_) => Err(unsupported("rendering a sequence value", span)),
        Value::LocalTree(_) => Err(unsupported("rendering a local tree value", span)),
        Value::Instant(_) => Err(unsupported("rendering an instant value", span)),
        Value::Date(_) => Err(unsupported("rendering a date value", span)),
        Value::Duration(_) => Err(unsupported("rendering a duration value", span)),
        Value::Resource(_) => Err(unsupported("rendering a resource value", span)),
        Value::Enum(_) => Err(unsupported("rendering an enum value", span)),
        Value::Int(_) | Value::Bool(_) | Value::Str(_) | Value::Decimal(_) => {
            unreachable!("scalar_text rendered every scalar before this match")
        }
    }
}

fn render_identity(identity: &IdentityValue) -> String {
    let rendered: Vec<String> = identity.keys.iter().map(saved_key_preview).collect();
    match rendered.as_slice() {
        [key] => key.clone(),
        _ => format!("identity({})", rendered.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::{Value, saved_key_to_value};
    use marrow_store::key::SavedKey;

    #[test]
    fn saved_key_to_value_carries_every_scalar_key_kind() {
        // Every saved key kind converts to its runtime value, including the temporal
        // ones, so iterating a layer keyed by any scalar type yields a usable value
        // rather than faulting `run.unsupported`.
        assert!(matches!(
            saved_key_to_value(SavedKey::Int(7)),
            Value::Int(7)
        ));
        assert!(matches!(
            saved_key_to_value(SavedKey::Bool(true)),
            Value::Bool(true)
        ));
        assert!(matches!(
            saved_key_to_value(SavedKey::Str("k".into())),
            Value::Str(_)
        ));
        assert!(matches!(
            saved_key_to_value(SavedKey::Bytes(vec![1])),
            Value::Bytes(_)
        ));
        assert!(matches!(
            saved_key_to_value(SavedKey::Date(18_000)),
            Value::Date(18_000)
        ));
        assert!(matches!(
            saved_key_to_value(SavedKey::Duration(5_000)),
            Value::Duration(5_000)
        ));
        assert!(matches!(
            saved_key_to_value(SavedKey::Instant(1_600)),
            Value::Instant(1_600)
        ));
    }
}
