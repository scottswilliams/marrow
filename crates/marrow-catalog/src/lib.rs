//! The Marrow accepted catalog semantic model. Two committed projections share this owner:
//! the full-shape [`CatalogMetadata`] the backup path persists, and the thin, inert
//! [`CatalogLock`] the source tree commits — a per-entry `(kind, path)` adoption anchor, stable
//! id, lifecycle, and shape fingerprint, the append-only cross-lifecycle id ledger, a monotonic
//! epoch high-water, and the producing source digest. The lock is data only: it carries no field
//! or method that
//! could write to, repair, or override a store. The identity-aware structural-signature decode
//! every catalog consumer reads shape through lives here too.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Stable error code for an invalid accepted catalog metadata file.
pub const CATALOG_INVALID: &str = "catalog.invalid";
/// Stable error code for a corrupt committed catalog lock projection. This is the wire and
/// documentation constant every consumer matches the lock's fail-closed rejection against.
pub const LOCK_CORRUPT: &str = "catalog.lock_corrupt";
const LOWER_HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

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
    pub fn new(epoch: u64, entries: Vec<CatalogEntry>) -> Result<Self, CatalogError> {
        let digest = catalog_digest(epoch, &entries)?;
        Ok(Self {
            epoch,
            digest,
            entries,
        })
    }

    pub fn from_stored_parts(
        epoch: u64,
        stored_digest: String,
        entries: Vec<CatalogEntry>,
    ) -> Result<Self, CatalogError> {
        let digest = catalog_digest(epoch, &entries)?;
        if stored_digest != digest {
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

    pub fn to_json_pretty(&self) -> Result<String, CatalogError> {
        serde_json::to_string_pretty(self).map_err(|error| CatalogError::new(error.to_string()))
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
            reject_nul("entry path", &entry.path)?;
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
                reject_nul("alias", alias)?;
                if alias == &entry.path {
                    return Err(CatalogError::new(format!(
                        "catalog alias `{alias}` repeats its canonical path"
                    )));
                }
                insert_catalog_path(&mut paths, entry.kind, alias, index)?;
            }
            if let Some(shape) = &entry.accepted_key_shape {
                reject_nul("accepted key shape", shape)?;
            }
            if let Some(shape) = &entry.accepted_index_shape {
                reject_nul("accepted index shape", shape)?;
            } else if entry.kind == CatalogEntryKind::StoreIndex
                && entry.lifecycle == CatalogLifecycle::Active
            {
                // Only an active store index has live derived cells whose shape must be recorded.
                // A Reserved store index is a materialized ledger tombstone: identity-only, holding
                // no shape, so a fresh store re-seeded from a valid lock that retired an index
                // re-seeds instead of failing closed on a synthesized corruption.
                return Err(CatalogError::new(
                    "store index catalog entry must record an accepted index shape",
                ));
            }
            if let Some(signature) = &entry.accepted_struct {
                reject_nul("accepted structural signature", signature)?;
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
    /// The declaration shape a store index's derived cells were accepted under. It records
    /// uniqueness and each ordered key column by durable source identity, so a same-name index
    /// whose key list or uniqueness changes is discharged as a derived rebuild even though its
    /// catalog path and stable id stay fixed. Only a store-index entry records it; every other
    /// kind leaves it `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_index_shape: Option<String>,
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
    /// The per-transform identity under which an `evolve transform` last recomputed this member's
    /// stored cells — a hash of this member's stable id and the transform body. A transform is
    /// shape-neutral — it rewrites every record of an existing member without changing its type —
    /// so it moves no structural signature the fence or the other classifiers can see, and its
    /// data work would otherwise re-derive on every preview. Recording this identity here makes
    /// the transform a catalog change that advances the epoch on apply, and lets the discharge
    /// recognize a transform already applied at the current source. Keying on the transform's own
    /// target and body — not the whole-program shape — means a later unrelated durable edit cannot
    /// drift the mark and re-run the migration against already-migrated data, while a changed body
    /// computes a different identity that reads as a fresh obligation. Only a transformed
    /// resource-member entry records it; every other kind leaves it `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_transform: Option<String>,
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
/// plain field or a keyed leaf) carries its value-by-identity leaf token, an unkeyed group
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

/// The thin committed source-tree projection of catalog state. Unlike [`CatalogMetadata`], it
/// records each entry's shape as an opaque [`shape fingerprint`](LockEntry::shape_fingerprint)
/// rather than its full text, while still carrying the entry's `(kind, path)` as the first-run
/// adoption anchor, and adds the complete append-only cross-lifecycle id ledger, a monotonic
/// epoch high-water, and the producing source digest. It is inert: it owns no path to a store and
/// self-validates only, so a checked-in lock can be read and compared but never repairs,
/// overrides, or writes durable state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogLock {
    pub entries: Vec<LockEntry>,
    pub ledger: Vec<LockLedgerTombstone>,
    pub epoch_high_water: u64,
    pub source_digest: String,
}

impl CatalogLock {
    /// Build a lock from already-fingerprinted entries, validating its self-consistency
    /// invariants so an in-memory lock cannot be constructed in a state the wire form would
    /// reject.
    pub fn new(
        entries: Vec<LockEntry>,
        ledger: Vec<LockLedgerTombstone>,
        epoch_high_water: u64,
        source_digest: String,
    ) -> Result<Self, CatalogError> {
        let lock = Self {
            entries,
            ledger,
            epoch_high_water,
            source_digest,
        };
        lock.validate()?;
        Ok(lock)
    }

    /// Project the lock to canonical pretty JSON. Entries and ledger tombstones are emitted in a
    /// stable identity order, so the same logical lock with its vectors in any order renders
    /// byte-identically.
    pub fn to_lock_json_pretty(&self) -> Result<String, CatalogError> {
        serde_json::to_string_pretty(&self.canonical())
            .map_err(|error| CatalogError::new(error.to_string()))
    }

    /// Parse a committed lock projection, failing closed with [`LOCK_CORRUPT`] on any corruption:
    /// Git conflict markers, an unknown field, a malformed fingerprint or source digest, a NUL
    /// byte in any id or path, an empty entry path, a duplicate `(kind, path)` adoption anchor, a
    /// ledger id reused by an active entry, a ledger tombstone recording the active lifecycle, a
    /// duplicate ledger id, an epoch high-water below a tombstone's recorded high-water, or an
    /// epoch high-water no real change could advance past. It never panics and is never lenient.
    pub fn from_lock_json(json: &str) -> Result<Self, CatalogError> {
        if contains_conflict_marker(json) {
            return Err(CatalogError::lock_unparseable());
        }
        let lock: Self =
            serde_json::from_str(json).map_err(|_| CatalogError::lock_unparseable())?;
        lock.validate()?;
        Ok(lock)
    }

    fn canonical(&self) -> Self {
        let mut entries = self.entries.clone();
        entries.sort_by(|left, right| left.stable_id.cmp(&right.stable_id));
        let mut ledger = self.ledger.clone();
        ledger.sort_by(|left, right| left.id.cmp(&right.id));
        Self {
            entries,
            ledger,
            epoch_high_water: self.epoch_high_water,
            source_digest: self.source_digest.clone(),
        }
    }

    fn validate(&self) -> Result<(), CatalogError> {
        self.validate_epoch_high_water()?;
        let active = self.validate_entries()?;
        self.validate_ledger(&active)?;
        validate_sha256("source digest", &self.source_digest)
    }

    /// Reject an epoch high-water no real change could advance past. The single advance owner
    /// mints `max(_, high_water) + 1`, so a high-water within one of `u64::MAX` cannot yield a
    /// strictly greater epoch: the advance would saturate and pin the version line, silently
    /// reusing a witnessed epoch for different identity on the next evolve. An un-advanceable
    /// high-water is therefore itself a corrupt lock, caught at decode before it reaches any
    /// write-adjacent path.
    fn validate_epoch_high_water(&self) -> Result<(), CatalogError> {
        if self.epoch_high_water >= u64::MAX - 1 {
            return Err(CatalogError::lock_corrupt(format!(
                "epoch high-water {} cannot be advanced",
                self.epoch_high_water
            )));
        }
        Ok(())
    }

    fn validate_entries(&self) -> Result<ActiveLockKeys<'_>, CatalogError> {
        let mut ids: HashSet<&str> = HashSet::new();
        let mut keys: HashSet<(CatalogEntryKind, &str)> = HashSet::new();
        for entry in &self.entries {
            require_lock_stable_id("entry stable id", &entry.stable_id)?;
            validate_sha256("shape fingerprint", &entry.shape_fingerprint)?;
            if entry.path.is_empty() {
                return Err(CatalogError::lock_corrupt("entry path must not be empty"));
            }
            reject_lock_nul("entry path", &entry.path)?;
            if !ids.insert(entry.stable_id.as_str()) {
                return Err(CatalogError::lock_corrupt(format!(
                    "entry stable id `{}` appears twice",
                    entry.stable_id
                )));
            }
            // First-run adoption resolves a source declaration to its committed id by
            // `(kind, path)`, so a duplicate anchor would bind two committed ids to one
            // declaration. Reject it here rather than silently adopt an arbitrary one.
            if !keys.insert((entry.kind, entry.path.as_str())) {
                return Err(CatalogError::lock_corrupt(format!(
                    "entry path `{}` for `{:?}` appears twice",
                    entry.path, entry.kind
                )));
            }
        }
        Ok(ActiveLockKeys { ids, keys })
    }

    fn validate_ledger(&self, active: &ActiveLockKeys<'_>) -> Result<(), CatalogError> {
        let mut ledger_ids: HashSet<&str> = HashSet::new();
        let mut reserved_keys: HashSet<(CatalogEntryKind, &str)> = HashSet::new();
        for tombstone in &self.ledger {
            require_lock_stable_id("ledger id", &tombstone.id)?;
            if tombstone.path.is_empty() {
                return Err(CatalogError::lock_corrupt(format!(
                    "ledger id `{}` records an empty path",
                    tombstone.id
                )));
            }
            reject_lock_nul("ledger path", &tombstone.path)?;
            if tombstone.lifecycle == CatalogLifecycle::Active {
                return Err(CatalogError::lock_corrupt(format!(
                    "ledger id `{}` records the active lifecycle",
                    tombstone.id
                )));
            }
            if active.ids.contains(tombstone.id.as_str()) {
                return Err(CatalogError::lock_corrupt(format!(
                    "ledger id `{}` is reissued by an active entry",
                    tombstone.id
                )));
            }
            if !ledger_ids.insert(tombstone.id.as_str()) {
                return Err(CatalogError::lock_corrupt(format!(
                    "ledger id `{}` is recorded twice",
                    tombstone.id
                )));
            }
            // A reserved `(kind, path)` and a live `(kind, path)` are mutually exclusive: the
            // same path cannot be both a retired tombstone and an active entry, and one path
            // cannot be reserved under two ids. Reject either collision so adoption never
            // reconstructs a reserved row that shadows or duplicates an active declaration.
            let key = (tombstone.kind, tombstone.path.as_str());
            if active.keys.contains(&key) {
                return Err(CatalogError::lock_corrupt(format!(
                    "ledger path `{}` for `{:?}` is also a live active entry",
                    tombstone.path, tombstone.kind
                )));
            }
            if !reserved_keys.insert(key) {
                return Err(CatalogError::lock_corrupt(format!(
                    "ledger path `{}` for `{:?}` is reserved twice",
                    tombstone.path, tombstone.kind
                )));
            }
            if tombstone.high_water > self.epoch_high_water {
                return Err(CatalogError::lock_corrupt(format!(
                    "epoch high-water {} is below ledger high-water {} for id `{}`",
                    self.epoch_high_water, tombstone.high_water, tombstone.id
                )));
            }
        }
        Ok(())
    }
}

/// The active entries' ids and `(kind, path)` keys, threaded from lock entry validation into
/// ledger validation so a reserved tombstone can be rejected when it reuses a live id or shadows
/// a live path.
struct ActiveLockKeys<'a> {
    ids: HashSet<&'a str>,
    keys: HashSet<(CatalogEntryKind, &'a str)>,
}

/// One entry in the committed lock: the `(kind, path)` first-run adoption keys onto, a stable id,
/// its lifecycle, and an opaque shape fingerprint. The `(kind, path)` is the identity anchor — a
/// fresh checkout binds a source declaration to its committed stable id by matching this pair, so
/// the same program over a wiped store mints no new identity. The fingerprint stands in for the
/// full accepted shape so the lock records a fingerprint of the shape each identity was last
/// accepted under without committing the shape text; it is a drift signal, not an identity key.
/// The lock does not cryptographically bind a fingerprint to its identity or to the source digest,
/// so it detects an accidental shape change, not a hostile re-pairing of valid parts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockEntry {
    pub kind: CatalogEntryKind,
    pub path: String,
    pub stable_id: String,
    pub lifecycle: CatalogLifecycle,
    /// A `sha256:`-prefixed fold of the entry kind and its accepted shape fields, so two entries
    /// fingerprint identically exactly when their kind and shape match: a key-shape, struct-leaf,
    /// or index-uniqueness change shifts the fingerprint, while a pure rename preserving the shape
    /// leaves it unchanged, letting the lock detect a shape change without re-parsing the grammar.
    pub shape_fingerprint: String,
}

impl LockEntry {
    /// Fingerprint a catalog entry into a lock entry, reusing the kind, path, and shape fields
    /// [`CatalogEntry`] already owns. The `(kind, path)` is carried verbatim as the adoption
    /// anchor; the accepted key shape, index shape, and structural signature are folded into the
    /// fingerprint as their canonical wire text alongside the kind, so the lock site never
    /// re-parses the structural-signature grammar.
    pub fn from_catalog_entry(entry: &CatalogEntry) -> Self {
        Self {
            kind: entry.kind,
            path: entry.path.clone(),
            stable_id: entry.stable_id.clone(),
            lifecycle: entry.lifecycle,
            shape_fingerprint: shape_fingerprint(entry),
        }
    }
}

/// An append-only cross-lifecycle id tombstone: an id that was reserved or retired, the
/// `(kind, path)` it was retired at, the lifecycle it rests in, and the epoch high-water at
/// which it was recorded. Tombstones make the ledger a complete history, so a retired id is
/// never silently reissued. Carrying the `(kind, path)` lets the committed lock represent a
/// reserved path: a fresh checkout seeded from the lock alone reconstructs the reserved store
/// entry, so re-declaring a retired path fails closed even when the store was lost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockLedgerTombstone {
    pub kind: CatalogEntryKind,
    pub path: String,
    pub id: String,
    pub lifecycle: CatalogLifecycle,
    pub high_water: u64,
}

impl LockLedgerTombstone {
    /// Project a reserved catalog entry to its ledger tombstone, carrying the `(kind, path)`
    /// it was retired at alongside its id, lifecycle, and the recorded high-water. The inverse
    /// of [`reserved_catalog_entry`].
    pub fn from_reserved_entry(entry: &CatalogEntry, high_water: u64) -> Self {
        Self {
            kind: entry.kind,
            path: entry.path.clone(),
            id: entry.stable_id.clone(),
            lifecycle: entry.lifecycle,
            high_water,
        }
    }

    /// Reconstruct the reserved catalog entry this tombstone records: a Reserved entry at the
    /// retired `(kind, path)` carrying the tombstoned id. A fresh checkout seeded from the lock
    /// alone materializes these into its baseline proposal, so the re-seeded store carries the
    /// reserved row a live store would and the never-reuse path-reuse defense survives store loss.
    /// Shape signatures stay `None`: a reserved entry is identity-only and holds no live shape.
    pub fn reserved_catalog_entry(&self) -> CatalogEntry {
        CatalogEntry {
            kind: self.kind,
            path: self.path.clone(),
            stable_id: self.id.clone(),
            aliases: Vec::new(),
            lifecycle: self.lifecycle,
            accepted_key_shape: None,
            accepted_index_shape: None,
            accepted_struct: None,
            applied_transform: None,
        }
    }
}

/// The pre-image a [`shape fingerprint`](LockEntry::shape_fingerprint) folds: the entry kind tag
/// and the three accepted shape fields, by their canonical wire text. Folding the kind alongside
/// the shape keeps two kinds with coincidentally equal shape text distinct.
#[derive(Serialize)]
struct FingerprintPreimage<'a> {
    kind: u8,
    key_shape: &'a Option<String>,
    index_shape: &'a Option<String>,
    struct_signature: &'a Option<String>,
    applied_transform: &'a Option<String>,
}

fn shape_fingerprint(entry: &CatalogEntry) -> String {
    let preimage = FingerprintPreimage {
        kind: entry.kind.tag(),
        key_shape: &entry.accepted_key_shape,
        index_shape: &entry.accepted_index_shape,
        struct_signature: &entry.accepted_struct,
        applied_transform: &entry.applied_transform,
    };
    let json = serde_json::to_string(&preimage)
        .expect("a fingerprint pre-image of owned shape fields serializes");
    digest_json(&json)
}

fn validate_sha256(label: &str, value: &str) -> Result<(), CatalogError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(CatalogError::lock_corrupt(format!(
            "{label} is not a sha256 digest"
        )));
    };
    let well_formed = hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    if well_formed {
        Ok(())
    } else {
        Err(CatalogError::lock_corrupt(format!(
            "{label} is not a sha256 digest"
        )))
    }
}

fn require_lock_stable_id(label: &str, id: &str) -> Result<(), CatalogError> {
    reject_lock_nul(label, id)?;
    if is_catalog_stable_id(id) {
        Ok(())
    } else {
        Err(CatalogError::lock_corrupt(format!(
            "{label} must match cat_<32 lowercase hex>"
        )))
    }
}

fn reject_lock_nul(label: &str, value: &str) -> Result<(), CatalogError> {
    if value.contains('\0') {
        return Err(CatalogError::lock_corrupt(format!(
            "{label} must not contain a NUL byte"
        )));
    }
    Ok(())
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
        for lifecycle in [CatalogLifecycle::Active, CatalogLifecycle::Reserved] {
            assert_eq!(CatalogLifecycle::from_tag(lifecycle.tag()), Some(lifecycle));
        }
    }

    #[test]
    fn an_unknown_tag_decodes_to_none() {
        assert_eq!(CatalogEntryKind::from_tag(99), None);
        assert_eq!(CatalogLifecycle::from_tag(1), None);
        assert_eq!(CatalogLifecycle::from_tag(99), None);
    }
}

#[cfg(test)]
mod digest_tests {
    use super::{
        CATALOG_INVALID, CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata,
    };

    const STALE_ORDER_SENSITIVE_DIGEST: &str =
        "sha256:295d5c4a5198276642b56d3239b893234b30ebc44db5c36137d5d21f374381e2";

    fn entry(kind: CatalogEntryKind, path: &str, suffix: u8) -> CatalogEntry {
        CatalogEntry {
            kind,
            path: path.to_string(),
            stable_id: format!("cat_{suffix:032x}"),
            aliases: Vec::new(),
            lifecycle: CatalogLifecycle::Active,
            accepted_key_shape: None,
            accepted_index_shape: None,
            accepted_struct: None,
            applied_transform: None,
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
    fn stored_parts_reject_stale_order_sensitive_digest() {
        let entries = reordered_entries();

        let error = CatalogMetadata::from_stored_parts(
            7,
            STALE_ORDER_SENSITIVE_DIGEST.to_string(),
            entries,
        )
        .expect_err("stale order-sensitive digest rejected");

        assert_eq!(error.code, CATALOG_INVALID);
    }

    #[test]
    fn json_rejects_stale_order_sensitive_digest() {
        let entries = reordered_entries();
        let json = serde_json::json!({
            "epoch": 7,
            "digest": STALE_ORDER_SENSITIVE_DIGEST,
            "entries": entries,
        })
        .to_string();

        let error = CatalogMetadata::from_json(&json).expect_err("stale JSON digest rejected");

        assert_eq!(error.code, CATALOG_INVALID);
    }
}

#[cfg(test)]
mod validate_tests {
    use super::{
        CATALOG_INVALID, CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata,
    };

    fn store_index_entry(lifecycle: CatalogLifecycle, shape: Option<&str>) -> CatalogEntry {
        CatalogEntry {
            kind: CatalogEntryKind::StoreIndex,
            path: "books::by_title".to_string(),
            stable_id: "cat_00000000000000000000000000000001".to_string(),
            aliases: Vec::new(),
            lifecycle,
            accepted_key_shape: None,
            accepted_index_shape: shape.map(str::to_string),
            accepted_struct: None,
            applied_transform: None,
        }
    }

    #[test]
    fn an_active_store_index_must_record_an_accepted_index_shape() {
        let catalog =
            CatalogMetadata::new(1, vec![store_index_entry(CatalogLifecycle::Active, None)])
                .expect("digest computes for an in-memory catalog");
        let error = catalog
            .validate()
            .expect_err("an active store index with no shape is rejected");
        assert_eq!(error.code, CATALOG_INVALID);
    }

    #[test]
    fn a_reserved_store_index_tombstone_validates_without_a_shape() {
        // A Reserved store-index row is the materialized form of a ledger tombstone: identity-only
        // by design, it legitimately holds no live index shape. Requiring one would brick a fresh
        // store re-seeded from a valid committed lock that retired a store index.
        let catalog =
            CatalogMetadata::new(1, vec![store_index_entry(CatalogLifecycle::Reserved, None)])
                .expect("digest computes for an in-memory catalog");
        catalog
            .validate()
            .expect("a reserved store-index tombstone validates without a shape");
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

#[cfg(test)]
mod lock_epoch_tests {
    use super::{CatalogLock, LOCK_CORRUPT};

    /// A canonical lock JSON with no entries or tombstones, carrying `epoch_high_water`. The
    /// source digest is a well-formed zero sha256 so only the epoch high-water is under test.
    fn lock_json(epoch_high_water: u64) -> String {
        serde_json::json!({
            "entries": [],
            "ledger": [],
            "epochHighWater": epoch_high_water,
            "sourceDigest":
                "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        })
        .to_string()
    }

    #[test]
    fn an_unadvanceable_epoch_high_water_is_rejected_as_lock_corrupt() {
        // A committed lock whose epoch high-water is u64::MAX is codec-valid in every other
        // respect (no entries, no tombstones, a well-formed digest), yet no real change could
        // ever advance past it: the single advance owner would have to mint epoch u64::MAX + 1.
        // The lock is therefore corrupt at decode, before it can reach any write-adjacent path.
        let error = CatalogLock::from_lock_json(&lock_json(u64::MAX))
            .expect_err("an unadvanceable epoch high-water is rejected");
        assert_eq!(error.code, LOCK_CORRUPT);

        // Within one of the ceiling is equally unadvanceable: advancing would saturate and pin
        // the epoch, breaking strict monotonicity for the next evolve. Reject it too.
        let error = CatalogLock::from_lock_json(&lock_json(u64::MAX - 1))
            .expect_err("an epoch high-water one below the ceiling is rejected");
        assert_eq!(error.code, LOCK_CORRUPT);
    }

    #[test]
    fn an_advanceable_epoch_high_water_decodes() {
        // The largest still-advanceable high-water decodes cleanly: max(_, N) + 1 stays in range.
        let lock = CatalogLock::from_lock_json(&lock_json(u64::MAX - 2))
            .expect("an advanceable epoch high-water decodes");
        assert_eq!(lock.epoch_high_water, u64::MAX - 2);
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
    Reserved,
}

impl CatalogLifecycle {
    /// The stable wire tag a durable encoder writes for this lifecycle.
    pub fn tag(self) -> u8 {
        match self {
            Self::Active => 0,
            Self::Reserved => 2,
        }
    }

    /// The lifecycle a wire tag names, or `None` for a tag this build does not know.
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Active),
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

    /// A corrupt committed lock projection. Owned by the lock codec; its message names
    /// `marrow.lock` so an operator knows which committed file to resolve.
    fn lock_corrupt(detail: impl fmt::Display) -> Self {
        Self {
            code: LOCK_CORRUPT,
            message: format!("marrow.lock is corrupt: {detail}"),
        }
    }

    /// A committed lock that is not valid JSON at all — malformed JSON or unresolved Git conflict
    /// markers. The lock is generated and never hand-edited, so the recovery is always the same:
    /// delete it and let a write path re-project it, or restore the committed file. The message
    /// gives that recovery directly rather than leaking parser line/column detail, which an
    /// everyday developer cannot act on.
    fn lock_unparseable() -> Self {
        Self {
            code: LOCK_CORRUPT,
            message: "marrow.lock is not valid (malformed JSON); it is generated and never \
                      hand-edited -- delete it and run marrow run <projectdir> (or marrow evolve \
                      apply) to re-project it from the store, or restore the committed file"
                .to_string(),
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

fn catalog_digest(epoch: u64, entries: &[CatalogEntry]) -> Result<String, CatalogError> {
    let mut canonical_entries: Vec<&CatalogEntry> = entries.iter().collect();
    canonical_entries
        .sort_by(|left, right| digest_entry_order(left).cmp(&digest_entry_order(right)));
    let json = serde_json::to_string(&DigestPayload {
        epoch,
        entries: canonical_entries,
    })
    .map_err(|error| CatalogError::new(error.to_string()))?;
    Ok(digest_json(&json))
}

type DigestEntryOrder<'a> = (
    u8,
    &'a str,
    &'a str,
    &'a [String],
    u8,
    &'a Option<String>,
    &'a Option<String>,
    &'a Option<String>,
    &'a Option<String>,
);

fn digest_json(json: &str) -> String {
    let digest = Sha256::digest(json.as_bytes());
    let mut out = String::with_capacity("sha256:".len() + digest.len() * 2);
    out.push_str("sha256:");
    push_lower_hex(&mut out, &digest);
    out
}

fn push_lower_hex(out: &mut String, bytes: &[u8]) {
    for &byte in bytes {
        out.push(char::from(LOWER_HEX_DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(LOWER_HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
}

fn digest_entry_order(entry: &CatalogEntry) -> DigestEntryOrder<'_> {
    (
        entry.kind.tag(),
        entry.path.as_str(),
        entry.stable_id.as_str(),
        &entry.aliases,
        entry.lifecycle.tag(),
        &entry.accepted_key_shape,
        &entry.accepted_index_shape,
        &entry.accepted_struct,
        &entry.applied_transform,
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

fn reject_nul(label: &str, value: &str) -> Result<(), CatalogError> {
    if value.contains('\0') {
        return Err(CatalogError::new(format!(
            "catalog {label} must not contain a NUL byte"
        )));
    }
    Ok(())
}

fn contains_conflict_marker(json: &str) -> bool {
    json.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("<<<<<<<") || line.starts_with("=======") || line.starts_with(">>>>>>>")
    })
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
