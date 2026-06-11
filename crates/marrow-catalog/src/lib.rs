//! The Marrow accepted catalog semantic model: the committed snapshot of durable
//! identity bindings (epoch, digest, entries), its validation invariants, and the
//! identity-aware structural-signature decode every catalog consumer reads shape
//! through.

use std::collections::HashMap;
use std::fmt;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Stable error code for an invalid accepted catalog metadata file.
pub const CATALOG_INVALID: &str = "catalog.invalid";

/// A committed accepted catalog snapshot. Source checks may read it and propose
/// replacement contents, but they never write it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogMetadata {
    pub epoch: u64,
    pub digest: String,
    pub entries: Vec<CatalogEntry>,
}

impl CatalogMetadata {
    pub fn new(epoch: u64, entries: Vec<CatalogEntry>) -> Self {
        let digest = catalog_digest(epoch, &entries);
        Self {
            epoch,
            digest,
            entries,
        }
    }

    pub fn from_stored_parts(
        epoch: u64,
        stored_digest: String,
        entries: Vec<CatalogEntry>,
    ) -> Result<Self, CatalogError> {
        let digest = catalog_digest(epoch, &entries);
        if stored_digest != digest
            && stored_digest != legacy_order_sensitive_catalog_digest(epoch, &entries)
        {
            return Err(CatalogError::new(format!(
                "catalog digest `{stored_digest}` does not match computed digest `{digest}`"
            )));
        }
        let catalog = Self {
            epoch,
            digest,
            entries,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn from_json(json: &str) -> Result<Self, CatalogError> {
        let catalog: Self =
            serde_json::from_str(json).map_err(|error| CatalogError::new(error.to_string()))?;
        Self::from_stored_parts(catalog.epoch, catalog.digest, catalog.entries)
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("catalog metadata serializes")
    }

    /// Check the identity invariants a committed catalog must hold: non-empty
    /// paths and stable IDs, a unique stable ID per entry, and a unique
    /// `(kind, path)` across both canonical paths and aliases. A proposal built by
    /// the checker is validated through this so an identity collision fails closed
    /// at check time rather than at apply.
    pub fn validate(&self) -> Result<(), CatalogError> {
        let mut paths: HashMap<(CatalogEntryKind, &str), usize> = HashMap::new();
        let mut stable_ids: HashMap<&str, usize> = HashMap::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.path.is_empty() {
                return Err(CatalogError::new("catalog entry path must not be empty"));
            }
            if !is_catalog_stable_id(&entry.stable_id) {
                return Err(CatalogError::new(
                    "catalog stable ID must match cat_<32 lowercase hex>",
                ));
            }
            if let Some(first) = stable_ids.insert(entry.stable_id.as_str(), index) {
                return Err(CatalogError::new(format!(
                    "catalog stable ID `{}` is used by entries {first} and {index}",
                    entry.stable_id
                )));
            }
            insert_catalog_path(&mut paths, entry.kind, &entry.path, index)?;
            for alias in &entry.aliases {
                if alias.is_empty() {
                    return Err(CatalogError::new("catalog alias must not be empty"));
                }
                if alias == &entry.path {
                    return Err(CatalogError::new(format!(
                        "catalog alias `{alias}` repeats its canonical path"
                    )));
                }
                insert_catalog_path(&mut paths, entry.kind, alias, index)?;
            }
        }
        Ok(())
    }
}

/// One accepted durable identity binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogEntry {
    pub kind: CatalogEntryKind,
    pub path: String,
    pub stable_id: String,
    pub aliases: Vec<String>,
    pub lifecycle: CatalogLifecycle,
    /// The identity-key shape a store's durable records are keyed under: the comma-joined
    /// scalar type names of its identity keys in order (`int`, `int,string`), so the
    /// arity and each key type are both recorded. v0.1 has no graceful store-key migration,
    /// so a discharge compares this against the current declared shape and fails closed when
    /// they differ: re-keying would orphan every record addressed by the old key shape. Only
    /// a store entry records it; every other kind leaves it `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_key_shape: Option<String>,
    /// The identity-aware structural signature a resource member's durable data was accepted
    /// under, decoded through [`structural_signature`] into a [`StructuralSignature`] (which owns
    /// the wire grammar). It records the member's shape by referent identity rather than source
    /// spelling, so a leaf token names a type the way it is durably addressed and a keyed layer's
    /// key shape is recorded here rather than in `accepted_key_shape` (which holds only store
    /// identity keys). The discharge fails closed when a member present in both the accepted
    /// snapshot and current source has a signature that changed and no explicit obligation
    /// already covers it, so a structural transition no targeted classifier handles cannot
    /// silently activate over existing data. Only a resource-member entry records it; every other
    /// kind leaves it `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_struct: Option<String>,
}

impl CatalogEntry {
    /// The leaf token from this entry's accepted structural signature, or `None` for any non-leaf
    /// member or one recording no signature. Because the token names the value type by referent
    /// identity, a later type change is detected even when the new type's decoder would also
    /// accept the old bytes (an `int` stored as `1` reads as a `bool` `true`), while a pure enum
    /// or store rename is correctly not a type change.
    pub fn accepted_leaf_token(&self) -> Option<&str> {
        self.accepted_struct
            .as_deref()
            .and_then(structural_signature_leaf_token)
    }
}

/// The structural shape a resource member's durable data occupies, decoded from its identity-aware
/// structural signature. The signature is a discriminated union over a member's shape: a leaf (a
/// plain field or a keyed-leaf map) carries its value-by-identity leaf token, an unkeyed group
/// carries nothing, and a keyed group carries the key shape its entries are addressed under. This
/// enum is the single owner of that convention's decode: every consumer — the durable accepted
/// side ([`CatalogEntry::accepted_leaf_token`]) and the live declared side in the evolution
/// discharge — reads shape out of a signature through [`structural_signature`] rather than
/// matching prefixes at its own use site. The encode side lives at the catalog (de)serialization
/// boundary in `marrow-check`; this is its sole reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuralSignature<'a> {
    /// A leaf member, carrying its value-by-identity leaf token.
    Leaf(&'a str),
    /// An unkeyed group.
    Group,
    /// A keyed group, carrying the key shape its entries are addressed under.
    KeyedGroup(&'a str),
}

impl<'a> StructuralSignature<'a> {
    /// The leaf token this signature names, or `None` for a non-leaf shape (a group or keyed
    /// group records no leaf token).
    fn leaf_token(self) -> Option<&'a str> {
        match self {
            Self::Leaf(token) => Some(token),
            Self::Group | Self::KeyedGroup(_) => None,
        }
    }
}

/// Decode a member's structural signature into its typed shape, or `None` when the text matches no
/// known shape. The wire form stays a string at the catalog boundary; every interior consumer
/// branches on this enum rather than re-parsing the convention.
pub fn structural_signature(signature: &str) -> Option<StructuralSignature<'_>> {
    if let Some(token) = signature.strip_prefix("leaf:") {
        Some(StructuralSignature::Leaf(token))
    } else if signature == "group" {
        Some(StructuralSignature::Group)
    } else {
        signature
            .strip_prefix("keyed-group:[")
            .and_then(|rest| rest.strip_suffix(']'))
            .map(StructuralSignature::KeyedGroup)
    }
}

/// The leaf token a member's structural signature encodes, or `None` when the signature names a
/// non-leaf shape. Both the durable accepted side and the live declared side read leaf tokens
/// through [`structural_signature`], so the convention is owned in one place.
pub fn structural_signature_leaf_token(signature: &str) -> Option<&str> {
    structural_signature(signature).and_then(StructuralSignature::leaf_token)
}

#[cfg(test)]
mod tag_tests {
    use super::{CatalogEntryKind, CatalogLifecycle};

    #[test]
    fn entry_kind_tags_round_trip_for_every_variant() {
        for kind in [
            CatalogEntryKind::Resource,
            CatalogEntryKind::Store,
            CatalogEntryKind::StoreIndex,
            CatalogEntryKind::ResourceMember,
            CatalogEntryKind::Enum,
            CatalogEntryKind::EnumMember,
        ] {
            assert_eq!(CatalogEntryKind::from_tag(kind.tag()), Some(kind));
        }
    }

    #[test]
    fn lifecycle_tags_round_trip_for_every_variant() {
        for lifecycle in [
            CatalogLifecycle::Active,
            CatalogLifecycle::Deprecated,
            CatalogLifecycle::Reserved,
        ] {
            assert_eq!(CatalogLifecycle::from_tag(lifecycle.tag()), Some(lifecycle));
        }
    }

    #[test]
    fn an_unknown_tag_decodes_to_none() {
        assert_eq!(CatalogEntryKind::from_tag(99), None);
        assert_eq!(CatalogLifecycle::from_tag(99), None);
    }
}

#[cfg(test)]
mod digest_tests {
    use super::{
        CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata, catalog_digest,
        legacy_order_sensitive_catalog_digest,
    };

    fn entry(kind: CatalogEntryKind, path: &str, suffix: u8) -> CatalogEntry {
        CatalogEntry {
            kind,
            path: path.to_string(),
            stable_id: format!("cat_{suffix:032x}"),
            aliases: Vec::new(),
            lifecycle: CatalogLifecycle::Active,
            accepted_key_shape: None,
            accepted_struct: None,
        }
    }

    fn reordered_entries() -> Vec<CatalogEntry> {
        vec![
            entry(CatalogEntryKind::EnumMember, "books::Status::archived", 3),
            entry(CatalogEntryKind::Enum, "books::Status", 1),
            entry(CatalogEntryKind::EnumMember, "books::Status::active", 2),
        ]
    }

    #[test]
    fn stored_parts_accept_legacy_order_sensitive_digest_and_normalize_it() {
        let entries = reordered_entries();
        let legacy = legacy_order_sensitive_catalog_digest(7, &entries);
        assert_ne!(legacy, catalog_digest(7, &entries));

        let metadata =
            CatalogMetadata::from_stored_parts(7, legacy, entries).expect("legacy digest accepted");

        assert_eq!(metadata.digest, catalog_digest(7, &metadata.entries));
    }

    #[test]
    fn json_accepts_legacy_order_sensitive_digest_and_normalizes_it() {
        let entries = reordered_entries();
        let legacy = legacy_order_sensitive_catalog_digest(7, &entries);
        let json = serde_json::json!({
            "epoch": 7,
            "digest": legacy,
            "entries": entries,
        })
        .to_string();

        let metadata = CatalogMetadata::from_json(&json).expect("legacy JSON digest accepted");

        assert_eq!(metadata.digest, catalog_digest(7, &metadata.entries));
    }
}

#[cfg(test)]
mod structural_signature_tests {
    use super::{StructuralSignature, structural_signature, structural_signature_leaf_token};

    #[test]
    fn decodes_a_leaf_signature_to_its_token() {
        assert_eq!(
            structural_signature("leaf:int"),
            Some(StructuralSignature::Leaf("int"))
        );
        assert_eq!(
            structural_signature("leaf:enum:cat_0123456789abcdef0123456789abcdef"),
            Some(StructuralSignature::Leaf(
                "enum:cat_0123456789abcdef0123456789abcdef"
            ))
        );
        assert_eq!(
            structural_signature("leaf:"),
            Some(StructuralSignature::Leaf(""))
        );
        assert_eq!(structural_signature_leaf_token("leaf:int"), Some("int"));
    }

    #[test]
    fn decodes_an_unkeyed_group() {
        assert_eq!(
            structural_signature("group"),
            Some(StructuralSignature::Group)
        );
        assert_eq!(structural_signature_leaf_token("group"), None);
    }

    #[test]
    fn decodes_a_keyed_group_to_its_key_shape() {
        assert_eq!(
            structural_signature("keyed-group:[int]"),
            Some(StructuralSignature::KeyedGroup("int"))
        );
        assert_eq!(
            structural_signature("keyed-group:[int,string]"),
            Some(StructuralSignature::KeyedGroup("int,string"))
        );
        assert_eq!(
            structural_signature("keyed-group:[]"),
            Some(StructuralSignature::KeyedGroup(""))
        );
        assert_eq!(structural_signature_leaf_token("keyed-group:[int]"), None);
    }

    #[test]
    fn an_unknown_shape_decodes_to_none() {
        assert_eq!(structural_signature("mystery"), None);
        assert_eq!(structural_signature("keyed-group:[int"), None);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CatalogEntryKind {
    Resource,
    Store,
    StoreIndex,
    ResourceMember,
    Enum,
    EnumMember,
}

impl CatalogEntryKind {
    /// The stable wire tag a durable encoder writes for this kind.
    pub fn tag(self) -> u8 {
        match self {
            Self::Resource => 0,
            Self::Store => 1,
            Self::StoreIndex => 2,
            Self::ResourceMember => 3,
            Self::Enum => 4,
            Self::EnumMember => 5,
        }
    }

    /// The kind a wire tag names, or `None` for a tag this build does not know.
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Resource),
            1 => Some(Self::Store),
            2 => Some(Self::StoreIndex),
            3 => Some(Self::ResourceMember),
            4 => Some(Self::Enum),
            5 => Some(Self::EnumMember),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CatalogLifecycle {
    Active,
    Deprecated,
    Reserved,
}

impl CatalogLifecycle {
    /// The stable wire tag a durable encoder writes for this lifecycle.
    pub fn tag(self) -> u8 {
        match self {
            Self::Active => 0,
            Self::Deprecated => 1,
            Self::Reserved => 2,
        }
    }

    /// The lifecycle a wire tag names, or `None` for a tag this build does not know.
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Active),
            1 => Some(Self::Deprecated),
            2 => Some(Self::Reserved),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogError {
    pub code: &'static str,
    pub message: String,
}

impl CatalogError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            code: CATALOG_INVALID,
            message: message.into(),
        }
    }
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CatalogError {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DigestPayload<'a> {
    epoch: u64,
    entries: Vec<&'a CatalogEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LegacyDigestPayload<'a> {
    epoch: u64,
    entries: &'a [CatalogEntry],
}

fn catalog_digest(epoch: u64, entries: &[CatalogEntry]) -> String {
    let mut canonical_entries: Vec<&CatalogEntry> = entries.iter().collect();
    canonical_entries
        .sort_by(|left, right| digest_entry_order(left).cmp(&digest_entry_order(right)));
    let json = serde_json::to_string(&DigestPayload {
        epoch,
        entries: canonical_entries,
    })
    .expect("catalog digest payload serializes");
    digest_json(&json)
}

fn legacy_order_sensitive_catalog_digest(epoch: u64, entries: &[CatalogEntry]) -> String {
    let json = serde_json::to_string(&LegacyDigestPayload { epoch, entries })
        .expect("catalog digest payload serializes");
    digest_json(&json)
}

fn digest_json(json: &str) -> String {
    let digest = Sha256::digest(json.as_bytes());
    let mut out = String::with_capacity("sha256:".len() + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

fn digest_entry_order(
    entry: &CatalogEntry,
) -> (
    u8,
    &str,
    &str,
    &[String],
    u8,
    &Option<String>,
    &Option<String>,
) {
    (
        entry.kind.tag(),
        entry.path.as_str(),
        entry.stable_id.as_str(),
        &entry.aliases,
        entry.lifecycle.tag(),
        &entry.accepted_key_shape,
        &entry.accepted_struct,
    )
}

fn is_catalog_stable_id(id: &str) -> bool {
    let Some(hex) = id.strip_prefix("cat_") else {
        return false;
    };
    hex.len() == 32
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn insert_catalog_path<'a>(
    paths: &mut HashMap<(CatalogEntryKind, &'a str), usize>,
    kind: CatalogEntryKind,
    path: &'a str,
    index: usize,
) -> Result<(), CatalogError> {
    if let Some(first) = paths.insert((kind, path), index) {
        return Err(CatalogError::new(format!(
            "catalog path `{path}` for `{kind:?}` is used by entries {first} and {index}"
        )));
    }
    Ok(())
}
