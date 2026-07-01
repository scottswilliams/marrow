//! Runtime values and their conversions to and from saved values.

use marrow_store::Decimal;
use marrow_store::cell::CatalogId;
use marrow_store::key::{
    SavedKey, decode_identity_index_key, decode_identity_payload_arity, encode_identity_index_key,
    encode_identity_payload,
};
use marrow_store::tree::{TreeEnumMember, decode_tree_enum_member, encode_tree_enum_member};
use marrow_store::value::{
    SavedValue, ScalarType, decode_value, encode_value, scalar_key_matches_type,
    validate_scalar_key,
};
use marrow_syntax::SourceSpan;

use marrow_check::{
    CheckedEnumRef, CheckedFacts, CheckedRuntimeProgram, CheckedSavedPlace, EnumId, EnumMemberId,
    StoreLeafKind, StoredValueMeaning,
};

use crate::error::{Located, RuntimeError, type_error, unsupported};

const DIAGNOSTIC_PREVIEW_SCALAR_LIMIT: usize = 64;

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
    /// The empty optional: the in-memory value of a `T?` that is absent. A present
    /// optional carries its inner value directly, so this is only ever the absent
    /// arm. It is confined to code: the resolution forms (`??`, `if const`,
    /// `exists`, `?.`), an optional binding, and an optional argument observe it,
    /// and it converts to a `None` `Option<Value>` at the return and call
    /// boundaries. It never reaches the store — a present-or-clear write routes the
    /// absent arm to the node-delete planner, and `value_to_saved` yields no cell.
    Absent,
    /// An in-memory `sequence[T]` value: a 1-based integer-keyed local tree, the same
    /// shape `^`-saved sequences carry. A materializing producer (`std::text::split`,
    /// `values`/`keys`) builds a dense `1..n`, while a bound `var xs: sequence` may
    /// grow holes through a positional write past the dense range or a delete, exactly
    /// as the saved side does. Iterated by a `for` loop; not itself a scalar saved value.
    Sequence(Sequence),
    LocalTree(LocalTree),
    /// A materialized resource tree: its present top-level fields, in schema
    /// order. Produced by a whole-resource read and consumed by a whole-resource
    /// write.
    Resource(Vec<(String, Value)>),
    /// A store identity: its checked root plus lowered key segments in declared order.
    Identity(IdentityValue),
}

/// A 1-based integer-keyed local sequence held as an ordered map from a populated
/// position to its value. Holes are absent positions, not stored entries, so a
/// positional write past the dense range or a delete leaves a gap that iteration,
/// `count`, and a positional read all skip — the same stored-only, gap-skipping,
/// key-ordered contract a saved sequence guarantees, with no in-memory-versus-saved
/// distinction. Backing it with a `BTreeMap` keeps a positional write, read, and
/// delete `O(log n)` for any position, where a sorted `Vec` shifted its whole tail on
/// a low-position write or delete and degraded a front-drain to `O(n^2)` — the same
/// representation the byte-identical `(k: int): int` keyed tree carries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sequence(std::collections::BTreeMap<i64, Value>);

impl Sequence {
    /// A dense `1..n` sequence from values in order, the shape every materializing
    /// producer builds.
    pub fn dense(values: Vec<Value>) -> Self {
        Self(
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| (index as i64 + 1, value))
                .collect(),
        )
    }

    /// The number of populated positions, skipping holes.
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    /// The value at `position`, or `None` when the position is a hole or non-positive.
    pub(crate) fn get(&self, position: i64) -> Option<&Value> {
        self.0.get(&position)
    }

    /// Write `value` at `position`, replacing any existing entry or inserting a new one.
    /// The map keeps positions in ascending order, so any position is `O(log n)`. A
    /// non-positive position addresses no node and is rejected by the caller first.
    pub(crate) fn set(&mut self, position: i64, value: Value) {
        self.0.insert(position, value);
    }

    /// Remove the entry at `position`, returning whether one was present. Deleting a
    /// hole removes nothing.
    pub(crate) fn remove(&mut self, position: i64) -> bool {
        self.0.remove(&position).is_some()
    }

    /// The highest populated position, or `None` when empty. Callers allocate the
    /// next append position one past this through the shared key-space allocator, so
    /// the strictly-ascending, exhaustion-faulting contract has a single owner.
    pub(crate) fn highest_position(&self) -> Option<i64> {
        self.0.keys().next_back().copied()
    }

    /// Store `value` at `position`, which the caller has allocated strictly above
    /// every existing key. Append never fills a hole, so this only ever extends the
    /// tail and the ascending invariant holds.
    pub(crate) fn append(&mut self, position: i64, value: Value) {
        debug_assert!(
            self.0
                .keys()
                .next_back()
                .is_none_or(|last| position > *last),
            "append position must exceed every existing key"
        );
        self.0.insert(position, value);
    }

    /// The populated positions in ascending key order.
    pub(crate) fn positions(&self) -> impl Iterator<Item = i64> + '_ {
        self.0.keys().copied()
    }

    /// The stored values in ascending position order.
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.0.values()
    }

    /// The `(position, value)` rows in ascending position order.
    pub(crate) fn rows(&self) -> impl Iterator<Item = (i64, &Value)> {
        self.0.iter().map(|(position, value)| (*position, value))
    }

    /// Consume the sequence into its stored values in ascending position order.
    pub(crate) fn into_values(self) -> Vec<Value> {
        self.0.into_values().collect()
    }
}

/// An in-memory keyed local tree (`var m(k: …): …`), held as an ordered map from a
/// row's full key tuple to its value. Iteration, the key-ordered `keys`/`entries`
/// walk, `count`, and a keyed read all observe rows in ascending key-tuple order, the
/// same key-ordered contract a `^`-saved keyed group guarantees. Backing it with a
/// `BTreeMap` keeps insert, lookup, and delete `O(log n)` for any key arrival order,
/// where a sorted `Vec` shifted its whole tail on a low-index insert and degraded to
/// `O(n^2)` for descending or scattered keys.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalTree(std::collections::BTreeMap<Vec<SavedKey>, Value>);

impl LocalTree {
    /// The value stored under the exact key tuple `keys`, or `None` when no row is
    /// addressed.
    pub(crate) fn get(&self, keys: &[SavedKey]) -> Option<&Value> {
        self.0.get(keys)
    }

    /// Overwrite the row at `keys`, or insert a new one. The map keeps rows in
    /// ascending key-tuple order, so no caller re-sorts and any arrival order is
    /// `O(log n)`.
    pub(crate) fn insert(&mut self, keys: Vec<SavedKey>, value: Value) {
        self.0.insert(keys, value);
    }

    /// Remove the row at `keys`. Deleting an absent key removes nothing.
    pub(crate) fn remove(&mut self, keys: &[SavedKey]) {
        self.0.remove(keys);
    }

    /// The number of stored rows.
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    /// The `(keys, value)` rows in ascending key-tuple order.
    pub(crate) fn rows(&self) -> impl Iterator<Item = (&[SavedKey], &Value)> {
        self.0.iter().map(|(keys, value)| (keys.as_slice(), value))
    }

    /// Consume the tree into its `(keys, value)` rows in ascending key-tuple order.
    pub(crate) fn into_rows(self) -> impl Iterator<Item = (Vec<SavedKey>, Value)> {
        self.0.into_iter()
    }
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
    /// The full `module::Enum::member` catalog path, used where a value names its
    /// stored identity (debugger preview, index-conflict diagnostic with dump parity).
    display_name: String,
    /// The `Enum::member` source spelling a `print`/`string` rendering produces.
    render_name: String,
}

impl EnumValue {
    pub fn enum_id(&self) -> EnumId {
        self.enum_id
    }

    pub fn member_id(&self) -> EnumMemberId {
        self.member_id
    }

    pub fn enum_catalog_id(&self) -> &str {
        &self.enum_catalog_id
    }

    pub fn member_catalog_id(&self) -> &str {
        &self.member_catalog_id
    }

    pub fn render_label(&self) -> &str {
        &self.display_name
    }

    /// The `Enum::member` text this value renders as through `print`, interpolation,
    /// and `string(...)`.
    pub fn render_name(&self) -> &str {
        &self.render_name
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LeafValue {
    Scalar(SavedValue),
    Enum {
        bytes: Vec<u8>,
        index_key: SavedKey,
        /// The member's canonical catalog path, so an index diagnostic renders an
        /// enum key by its member name rather than its opaque stored catalog id.
        display_name: String,
    },
    /// A typed-reference leaf: the key segments of an `Id(^store)`, stored as the
    /// canonical identity payload. Identity leaves are not value-derived index keys,
    /// so they have no `as_key`.
    Identity {
        keys: Vec<SavedKey>,
    },
}

impl LeafValue {
    pub(crate) fn bytes(&self) -> Result<Vec<u8>, marrow_store::value::ValueError> {
        match self {
            Self::Scalar(value) => encode_value(value),
            Self::Enum { bytes, .. } => Ok(bytes.clone()),
            Self::Identity { keys } => Ok(encode_identity_payload(keys)),
        }
    }

    pub(crate) fn as_key(&self) -> Result<Option<SavedKey>, marrow_store::value::ValueError> {
        match self {
            Self::Scalar(value) => value.as_key(),
            Self::Enum { index_key, .. } => Ok(Some(index_key.clone())),
            Self::Identity { .. } => Ok(None),
        }
    }

    pub(crate) fn is_enum(&self) -> bool {
        matches!(self, Self::Enum { .. })
    }

    /// The member's canonical path when this leaf is an enum value, for rendering
    /// an enum index key by name rather than its stored catalog id.
    pub(crate) fn enum_display_name(&self) -> Option<&str> {
        match self {
            Self::Enum { display_name, .. } => Some(display_name),
            Self::Scalar(_) | Self::Identity { .. } => None,
        }
    }
}

impl Value {
    /// A total, never-faulting one-line rendering for a debugger's Variables view.
    /// A preview, not re-parseable.
    pub fn display_debug(&self) -> String {
        match self {
            Value::Int(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Str(s) => s.clone(),
            Value::Decimal(d) => d.to_text(),
            Value::Instant(n) => preview_temporal_text(SavedValue::Instant(*n)),
            Value::Date(d) => preview_temporal_text(SavedValue::Date(*d)),
            Value::Duration(n) => preview_temporal_text(SavedValue::Duration(*n)),
            Value::Bytes(bytes) => render_bytes_hex(bytes),
            Value::Absent => "absent".to_string(),
            Value::Enum(value) => value.display_name.clone(),
            Value::Sequence(items) => format!("sequence[{}]", items.len()),
            Value::LocalTree(entries) => format!("tree[{}]", entries.len()),
            Value::Resource(fields) => {
                let names: Vec<&str> = fields.iter().map(|(name, _)| name.as_str()).collect();
                format!("resource{{{}}}", names.join(", "))
            }
            Value::Identity(identity) => render_debug_identity(identity),
        }
    }
}

/// A compact preview of one identity key segment for [`Value::display_debug`].
pub(crate) fn saved_key_preview(key: &SavedKey) -> String {
    saved_key_preview_with_text_limit(key, usize::MAX)
}

pub(crate) fn saved_key_preview_with_text_limit(key: &SavedKey, text_limit: usize) -> String {
    match key {
        SavedKey::Int(n) => n.to_string(),
        SavedKey::Bool(b) => b.to_string(),
        SavedKey::Str(s) => truncate_preview_chars(s, text_limit),
        SavedKey::Date(d) => preview_temporal_text(SavedValue::Date(*d)),
        SavedKey::Duration(n) => preview_temporal_text(SavedValue::Duration(*n)),
        SavedKey::Instant(n) => preview_temporal_text(SavedValue::Instant(*n)),
        SavedKey::Bytes(bytes) => render_bytes_hex(bytes),
    }
}

pub(crate) fn truncate_preview_chars(text: &str, limit: usize) -> String {
    for (count, (index, _)) in text.char_indices().enumerate() {
        if count == limit {
            let mut truncated = String::from(&text[..index]);
            truncated.push_str("...");
            return truncated;
        }
    }
    text.to_string()
}

pub(crate) fn diagnostic_text_preview(text: &str) -> String {
    let mut rendered = String::from("\"");
    let mut truncated = false;
    for (index, ch) in text.chars().enumerate() {
        if index == DIAGNOSTIC_PREVIEW_SCALAR_LIMIT {
            truncated = true;
            break;
        }
        rendered.extend(ch.escape_default());
    }
    if truncated {
        rendered.push_str("...");
    }
    rendered.push('"');
    rendered
}

pub(crate) fn diagnostic_value_preview(value: &Value) -> Option<String> {
    Some(match value {
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Str(text) => diagnostic_text_preview(text),
        Value::Date(d) => preview_temporal_text(SavedValue::Date(*d)),
        Value::Instant(n) => preview_temporal_text(SavedValue::Instant(*n)),
        Value::Duration(n) => preview_temporal_text(SavedValue::Duration(*n)),
        Value::Decimal(decimal) => decimal.to_text(),
        Value::Bytes(bytes) => diagnostic_bytes_preview(bytes),
        Value::Absent
        | Value::Enum(_)
        | Value::Sequence(_)
        | Value::LocalTree(_)
        | Value::Resource(_)
        | Value::Identity(_) => return None,
    })
}

/// A bounded `0x`-hex preview of a bytes value for a diagnostic: canonical so two
/// distinct values render distinctly, truncated past the scalar limit so a large
/// blob does not flood the message.
fn diagnostic_bytes_preview(bytes: &[u8]) -> String {
    if bytes.len() <= DIAGNOSTIC_PREVIEW_SCALAR_LIMIT {
        return render_bytes_hex(bytes);
    }
    format!(
        "{}...",
        render_bytes_hex(&bytes[..DIAGNOSTIC_PREVIEW_SCALAR_LIMIT])
    )
}

pub(crate) fn diagnostic_saved_key_preview(key: &SavedKey) -> String {
    match key {
        SavedKey::Str(text) => diagnostic_text_preview(text),
        _ => saved_key_preview(key),
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
pub(crate) fn saved_key_to_value(key: SavedKey, span: SourceSpan) -> Result<Value, RuntimeError> {
    validate_scalar_key(&key).map_err(|error| error.located(span))?;
    Ok(match key {
        SavedKey::Int(n) => Value::Int(n),
        SavedKey::Bool(b) => Value::Bool(b),
        SavedKey::Str(s) => Value::Str(s),
        SavedKey::Date(d) => Value::Date(d),
        SavedKey::Duration(n) => Value::Duration(n),
        SavedKey::Instant(n) => Value::Instant(n),
        SavedKey::Bytes(b) => Value::Bytes(b),
    })
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
        Value::Absent
        | Value::Enum(_)
        | Value::Sequence(_)
        | Value::LocalTree(_)
        | Value::Resource(_)
        | Value::Identity(_) => return None,
    })
}

/// The canonical text of a saved scalar whose canonical encoding is textual.
/// Temporal range failures surface as located codec errors; bytes may not be
/// valid UTF-8 and are rejected as a type fault if routed here.
pub(crate) fn canonical_scalar_text(
    value: SavedValue,
    span: SourceSpan,
) -> Result<String, RuntimeError> {
    if matches!(value, SavedValue::Bytes(_)) {
        return Err(type_error("cannot render bytes as canonical text", span));
    }
    let bytes = encode_value(&value).map_err(|error| error.located(span))?;
    String::from_utf8(bytes).map_err(|_| type_error("cannot render bytes as canonical text", span))
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
        Value::Absent
        | Value::Sequence(_)
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
pub(crate) fn value_to_key(
    value: Value,
    span: SourceSpan,
) -> Result<Option<SavedKey>, RuntimeError> {
    let key = match value {
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
        // before reaching here. An absent optional is never a key.
        Value::Decimal(_)
        | Value::Absent
        | Value::Sequence(_)
        | Value::Resource(_)
        | Value::Identity(_)
        | Value::LocalTree(_) => None,
    };
    if let Some(key) = &key {
        validate_scalar_key(key).map_err(|error| error.located(span))?;
    }
    Ok(key)
}

pub(crate) fn value_to_index_key(
    value: Value,
    meaning: &StoredValueMeaning,
    span: SourceSpan,
) -> Result<SavedKey, RuntimeError> {
    match meaning {
        StoredValueMeaning::Scalar(scalar) => {
            if value_scalar_type(&value) != Some(*scalar) {
                return Err(type_error("this index key has the wrong scalar type", span));
            }
            value_to_key(value, span)?.ok_or_else(|| unsupported("an index key of this type", span))
        }
        StoredValueMeaning::Enum { enum_id, members } => match value {
            Value::Enum(value)
                if value.enum_id == *enum_id && members.contains(&value.member_id) =>
            {
                Ok(SavedKey::Str(value.member_catalog_id))
            }
            Value::Enum(_) => Err(type_error("this index key takes a different enum", span)),
            _ => Err(type_error("this index key takes an enum value", span)),
        },
        StoredValueMeaning::Identity {
            root,
            store_catalog_id,
            key_scalars,
            ..
        } => {
            let Value::Identity(identity) = value else {
                return Err(type_error("this index key takes an identity value", span));
            };
            let keys = identity.into_keys_for_root(root, span)?;
            validate_identity_key_scalars(&keys, key_scalars, span)?;
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
    }
}

pub(crate) fn index_key_to_value(
    program: &CheckedRuntimeProgram,
    key: &SavedKey,
    meaning: &StoredValueMeaning,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    match meaning {
        StoredValueMeaning::Scalar(scalar) => {
            if !scalar_key_matches_type(key, *scalar) {
                return Err(type_error(
                    "stored index key has the wrong scalar type",
                    span,
                ));
            }
            saved_key_to_value(key.clone(), span)
        }
        StoredValueMeaning::Enum { members, .. } => {
            let SavedKey::Str(member_catalog_id) = key else {
                return Err(type_error("stored index key is not an enum member", span));
            };
            let member_id = members
                .iter()
                .copied()
                .find(|member_id| {
                    program
                        .facts()
                        .enum_member(*member_id)
                        .and_then(|member| member.catalog_id.as_deref())
                        == Some(member_catalog_id.as_str())
                })
                .ok_or_else(|| type_error("stored index key is not a valid enum member", span))?;
            enum_value_from_member(program.facts(), member_id)
                .map(Value::Enum)
                .ok_or_else(|| type_error("stored index key is not a valid enum member", span))
        }
        StoredValueMeaning::Identity {
            root,
            store_catalog_id,
            arity,
            key_scalars,
            ..
        } => {
            let SavedKey::Bytes(bytes) = key else {
                return Err(type_error(
                    "stored index key is not an identity value",
                    span,
                ));
            };
            let Some(store_catalog_id) = store_catalog_id.as_deref() else {
                return Err(unsupported(
                    "this identity index key before catalog activation",
                    span,
                ));
            };
            let keys =
                decode_identity_index_key(bytes, store_catalog_id, *arity).ok_or_else(|| {
                    type_error("stored index key is not a valid identity value", span)
                })?;
            validate_identity_key_scalars(&keys, key_scalars, span)?;
            Ok(identity_value(root, keys))
        }
    }
}

pub(crate) fn validate_identity_key_scalars(
    keys: &[SavedKey],
    key_scalars: &[ScalarType],
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    if keys.len() != key_scalars.len() {
        return Err(type_error(
            "stored identity keys do not match the store identity type",
            span,
        ));
    }
    for (key, scalar) in keys.iter().zip(key_scalars) {
        validate_scalar_key(key).map_err(|error| error.located(span))?;
        if !scalar_key_matches_type(key, *scalar) {
            return Err(type_error(
                "stored identity keys do not match the store identity type",
                span,
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_place_identity_keys(
    place: &CheckedSavedPlace,
    keys: &[SavedKey],
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    if keys.len() != place.identity_keys.len() {
        return Err(type_error(
            "stored identity keys do not match the store identity type",
            span,
        ));
    }
    for (key, declared) in keys.iter().zip(&place.identity_keys) {
        validate_scalar_key(key).map_err(|error| error.located(span))?;
        if let Some(expected) = declared.scalar
            && !scalar_key_matches_type(key, expected)
        {
            return Err(type_error(
                "stored identity keys do not match the store identity type",
                span,
            ));
        }
    }
    Ok(())
}

/// Lower a runtime value to the [`LeafValue`] a managed write stores for `leaf`: a
/// scalar leaf through [`value_to_saved`], an enum leaf through `enum_value_to_leaf`,
/// an identity leaf through its checked key segments. The single owner of value-to-leaf
/// lowering, so the append, group, evolution, and direct positional write paths encode a
/// leaf identically. Errors (not `None`) on a value the leaf cannot accept.
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
        StoreLeafKind::Identity { store_root, arity } => {
            let keys = identity_keys_of(value, store_root, span)?;
            if keys.len() != *arity {
                return Err(type_error(
                    "this identity has the wrong number of keys for the field",
                    span,
                ));
            }
            Ok(LeafValue::Identity { keys })
        }
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
                display_name: value.display_name,
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
    let member = stored_enum_member(program.facts(), enum_id, bytes)?;
    enum_value_from_member(program.facts(), member)
}

/// The member a stored enum leaf decodes to, identified against its declared
/// enum so a tampered or foreign member id yields `None`. The one place that maps
/// durable enum bytes back to a checked member; both the runtime decode and the
/// index-conflict diagnostic share it rather than re-matching catalog ids.
pub(crate) fn stored_enum_member(
    facts: &CheckedFacts,
    enum_id: EnumId,
    bytes: &[u8],
) -> Option<EnumMemberId> {
    let stored = decode_tree_enum_member(bytes).ok()?;
    let enum_fact = facts.enum_(enum_id)?;
    if enum_fact.catalog_id.as_deref() != Some(stored.enum_id().as_str()) {
        return None;
    }
    facts
        .enum_members()
        .iter()
        .find(|member| {
            member.enum_id == enum_id
                && member.catalog_id.as_deref() == Some(stored.member_id().as_str())
        })
        .map(|member| member.id)
}

/// The canonical member path a stored enum leaf renders as (`module::Enum::member`),
/// or `None` when the bytes do not decode to a selectable member of `enum_id`. Shared
/// by every user-facing surface that names a stored enum, so the index-conflict
/// diagnostic matches `data dump`.
pub(crate) fn stored_enum_member_path(
    facts: &CheckedFacts,
    enum_id: EnumId,
    bytes: &[u8],
) -> Option<String> {
    let member = stored_enum_member(facts, enum_id, bytes)?;
    if !facts.enum_member_is_selectable(member) {
        return None;
    }
    facts.enum_member_catalog_path(member)
}

/// The referent path an identity-typed index key renders as in a diagnostic,
/// for example `^authors(1)`, recovered from the key's physical encoding. An
/// index key over an `Id(^store)` field is stored as opaque bytes, so a
/// unique-conflict diagnostic decodes it here rather than leaking the encoding.
pub(crate) fn stored_identity_referent_path(
    meaning: &StoredValueMeaning,
    key: &SavedKey,
) -> Option<String> {
    let StoredValueMeaning::Identity {
        root,
        store_catalog_id,
        arity,
        ..
    } = meaning
    else {
        return None;
    };
    let SavedKey::Bytes(bytes) = key else {
        return None;
    };
    let store_catalog_id = store_catalog_id.as_deref()?;
    let keys = decode_identity_index_key(bytes, store_catalog_id, *arity)?;
    Some(render_debug_identity(&IdentityValue::for_root(
        root.clone(),
        keys,
    )))
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
        render_name: facts.enum_member_short_path(member_id)?,
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

/// The one renderer for `print`, interpolation, and `string(...)`: scalars in their
/// canonical form, an enum as its `Enum::member` source spelling, bytes as
/// `0x`-prefixed lowercase hex, a saved identity by its key(s), and sequences as
/// bracketed rendered elements. `string(...)` narrows the saved identity and
/// sequence shapes out before calling, so its acceptance is decided by the
/// conversion matrix, not by this function. The non-renderable shapes — local
/// trees and resources — are rejected at check for a statically-typed argument, so
/// they reach here only through an `unknown`-typed value, which faults at run.
pub(crate) fn render(value: Value, span: SourceSpan) -> Result<String, RuntimeError> {
    match value {
        Value::Int(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Str(s) => Ok(s),
        Value::Decimal(d) => Ok(d.to_text()),
        Value::Identity(identity) => Ok(render_identity(&identity)),
        Value::Bytes(bytes) => Ok(render_bytes_hex(&bytes)),
        Value::Enum(value) => Ok(value.render_name),
        Value::Instant(nanos) => canonical_scalar_text(SavedValue::Instant(nanos), span),
        Value::Date(days) => canonical_scalar_text(SavedValue::Date(days), span),
        Value::Duration(nanos) => canonical_scalar_text(SavedValue::Duration(nanos), span),
        Value::Sequence(sequence) => render_sequence(sequence, span),
        Value::Absent => Err(unsupported("rendering an absent optional", span)),
        Value::LocalTree(_) => Err(unsupported("rendering a local tree value", span)),
        Value::Resource(_) => Err(unsupported("rendering a resource value", span)),
    }
}

fn render_sequence(sequence: Sequence, span: SourceSpan) -> Result<String, RuntimeError> {
    let mut text = String::from("[");
    for (index, value) in sequence.into_values().into_iter().enumerate() {
        if index > 0 {
            text.push_str(", ");
        }
        text.push_str(&render(value, span)?);
    }
    text.push(']');
    Ok(text)
}

/// The `0x`-prefixed lowercase hex a bytes value renders as, matching `data dump`.
pub(crate) fn render_bytes_hex(bytes: &[u8]) -> String {
    let mut text = String::from("0x");
    text.push_str(&crate::hex::encode(bytes));
    text
}

/// The canonical text a temporal scalar renders as in a preview (`2024-01-02`,
/// not `date(19723)`), matching `print`. Total for previews: an out-of-range
/// value that the canonical formatter rejects falls back to the raw form rather
/// than faulting, since a preview must never raise.
fn preview_temporal_text(value: SavedValue) -> String {
    let fallback = match &value {
        SavedValue::Date(d) => format!("date({d})"),
        SavedValue::Instant(n) => format!("instant({n})"),
        SavedValue::Duration(n) => format!("duration({n})"),
        _ => unreachable!("preview_temporal_text takes a temporal value"),
    };
    canonical_scalar_text(value, SourceSpan::default()).unwrap_or(fallback)
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
    use super::{
        DIAGNOSTIC_PREVIEW_SCALAR_LIMIT, Value, canonical_scalar_text,
        diagnostic_saved_key_preview, diagnostic_text_preview, diagnostic_value_preview,
        saved_key_preview_with_text_limit, saved_key_to_value,
    };
    use crate::error::RUN_TYPE;
    use marrow_store::key::SavedKey;
    use marrow_store::value::SavedValue;
    use marrow_syntax::SourceSpan;

    const SIXTY_FOUR_CHAR_PREFIX: &str =
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!?";
    const LONG_TEXT_PREVIEW: &str =
        "\"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!?...\"";
    fn long_diagnostic_text() -> String {
        format!("{SIXTY_FOUR_CHAR_PREFIX}-tail-marker")
    }

    #[test]
    fn diagnostic_text_preview_truncates_after_sixty_four_chars() {
        let text = long_diagnostic_text();
        let preview = diagnostic_text_preview(&text);

        assert_eq!(SIXTY_FOUR_CHAR_PREFIX.chars().count(), 64);
        assert_eq!(preview, LONG_TEXT_PREVIEW);
        assert!(!preview.contains("tail-marker"), "{preview}");
    }

    #[test]
    fn diagnostic_value_preview_renders_bytes_as_bounded_hex() {
        // Distinct bytes render distinctly as canonical `0x`-hex, the form `print`
        // produces, so an assertion failure is legible rather than length-only.
        assert_eq!(
            diagnostic_value_preview(&Value::Bytes(b"abc".to_vec())).as_deref(),
            Some("0x616263")
        );

        let long = vec![0xab; DIAGNOSTIC_PREVIEW_SCALAR_LIMIT + 8];
        let preview = diagnostic_value_preview(&Value::Bytes(long)).unwrap();
        assert!(preview.ends_with("..."), "{preview}");
        assert_eq!(
            preview.trim_end_matches("..."),
            format!("0x{}", "ab".repeat(DIAGNOSTIC_PREVIEW_SCALAR_LIMIT))
        );
    }

    #[test]
    fn diagnostic_value_preview_renders_temporals_as_canonical_text() {
        // A date renders as its canonical calendar text, not a raw epoch integer.
        assert_eq!(
            diagnostic_value_preview(&Value::Date(19_723)).as_deref(),
            Some("2024-01-01")
        );
    }

    #[test]
    fn diagnostic_value_preview_uses_bounded_string_preview() {
        let text = long_diagnostic_text();
        let preview = diagnostic_value_preview(&Value::Str(text));

        assert_eq!(preview.as_deref(), Some(LONG_TEXT_PREVIEW));
        assert!(
            !matches!(preview.as_deref(), Some(preview) if preview.contains("tail-marker")),
            "{preview:?}"
        );
    }

    #[test]
    fn diagnostic_saved_key_preview_uses_bounded_string_preview() {
        let text = long_diagnostic_text();
        let preview = diagnostic_saved_key_preview(&SavedKey::Str(text));

        assert_eq!(preview, LONG_TEXT_PREVIEW);
        assert!(!preview.contains("tail-marker"), "{preview}");
    }

    #[test]
    fn saved_key_preview_with_text_limit_truncates_unquoted_string_keys() {
        let preview = saved_key_preview_with_text_limit(&SavedKey::Str("abcdef".into()), 3);

        assert_eq!(preview, "abc...");
    }

    #[test]
    fn saved_key_to_value_carries_every_scalar_key_kind() {
        // Every saved key kind converts to its runtime value, including the temporal
        // ones, so iterating a layer keyed by any scalar type yields a usable value
        // rather than faulting `run.unsupported`.
        let span = SourceSpan::default();
        assert!(matches!(
            saved_key_to_value(SavedKey::Int(7), span),
            Ok(Value::Int(7))
        ));
        assert!(matches!(
            saved_key_to_value(SavedKey::Bool(true), span),
            Ok(Value::Bool(true))
        ));
        assert!(matches!(
            saved_key_to_value(SavedKey::Str("k".into()), span),
            Ok(Value::Str(_))
        ));
        assert!(matches!(
            saved_key_to_value(SavedKey::Bytes(vec![1]), span),
            Ok(Value::Bytes(_))
        ));
        assert!(matches!(
            saved_key_to_value(SavedKey::Date(18_000), span),
            Ok(Value::Date(18_000))
        ));
        assert!(matches!(
            saved_key_to_value(SavedKey::Duration(5_000), span),
            Ok(Value::Duration(5_000))
        ));
        assert!(matches!(
            saved_key_to_value(SavedKey::Instant(1_600), span),
            Ok(Value::Instant(1_600))
        ));
    }

    #[test]
    fn canonical_scalar_text_rejects_bytes() {
        for bytes in [b"abc".to_vec(), vec![0xff]] {
            let error =
                canonical_scalar_text(SavedValue::Bytes(bytes), SourceSpan::default()).unwrap_err();

            assert_eq!(error.code(), RUN_TYPE);
            assert!(error.catchable);
        }
    }
}
