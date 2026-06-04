//! Runtime values and their conversions to and from saved values.

use marrow_store::Decimal;
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, decode_identity_payload_arity};
use marrow_store::tree::{TreeEnumMember, decode_tree_enum_member, encode_tree_enum_member};
use marrow_store::value::{SavedValue, decode_value, encode_value};
use marrow_syntax::SourceSpan;

use marrow_check::{
    CheckedEnumRef, CheckedFacts, CheckedRuntimeProgram, EnumId, EnumMemberId, StoreLeafKind,
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
            Value::Enum(value) => format!("enum({}.{})", value.enum_id.0, value.member_id.0),
            Value::Sequence(items) => format!("sequence[{}]", items.len()),
            Value::LocalTree(entries) => format!("tree[{}]", entries.len()),
            // Preview the present field names, in schema order, without recursing
            // into their values (which could be large or nested resources).
            Value::Resource(fields) => {
                let names: Vec<&str> = fields.iter().map(|(name, _)| name.as_str()).collect();
                format!("resource{{{}}}", names.join(", "))
            }
            // Preview the identity's lowered key segments.
            Value::Identity(identity) => {
                let rendered: Vec<String> = identity.keys.iter().map(saved_key_preview).collect();
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

/// The runtime value for an identity's lowered keys. Every identity carries its
/// checked store root, including single-key identities, so dynamic and host
/// boundaries cannot confuse a raw scalar key with an `Id(^store)`.
pub(crate) fn identity_value(root: &str, keys: Vec<SavedKey>) -> Value {
    Value::Identity(IdentityValue::for_root(root, keys))
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
            decode_identity_payload_arity(bytes, *arity)
                .map(|keys| identity_value(store_root, keys))
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
    Ok(match value {
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Str(s) => s,
        Value::Decimal(d) => d.to_text(),
        Value::Bytes(_) => return Err(unsupported("rendering a bytes value", span)),
        Value::Sequence(_) => return Err(unsupported("rendering a sequence value", span)),
        Value::LocalTree(_) => return Err(unsupported("rendering a local tree value", span)),
        Value::Instant(_) => return Err(unsupported("rendering an instant value", span)),
        Value::Date(_) => return Err(unsupported("rendering a date value", span)),
        Value::Duration(_) => return Err(unsupported("rendering a duration value", span)),
        Value::Resource(_) => return Err(unsupported("rendering a resource value", span)),
        Value::Identity(identity) => render_identity(&identity),
        Value::Enum(_) => return Err(unsupported("rendering an enum value", span)),
    })
}

fn render_identity(identity: &IdentityValue) -> String {
    let rendered: Vec<String> = identity.keys.iter().map(saved_key_preview).collect();
    match rendered.as_slice() {
        [key] => key.clone(),
        _ => format!("identity({})", rendered.join(", ")),
    }
}
