//! The durable-identity ledger and its committed artifact, `marrow.ids`.
//!
//! The ledger is the source-side authority for entropy-minted durable identity:
//! each row binds a `(kind, path)` anchor to a random 128-bit id, and the
//! append-only tombstone list plus a monotonic retirement high-water keep a
//! retired id (and its retired anchor) from ever being reused — even across
//! store loss, because the artifact is committed with the source. Entropy ids
//! are a separate identity family from the deterministic 32-byte hash
//! identities: they are minted once from OS entropy (by the CLI; this owner is
//! pure and only validates candidate draws) and never derived from content.
//!
//! `marrow.ids` is machine-written only. Developers never edit, copy, or cite
//! ids; the artifact is committed and line-diffable so parallel branches merge
//! textually, and a conflicting double-mint (two rows claiming one anchor or
//! one id) is rejected whole as [`IdsError`] — the artifact is never half-read.
//! Parsing accepts rows in any order so a textual merge stays valid;
//! serialization is canonical (sorted, bounded), so publication is
//! deterministic byte-for-byte.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use marrow_codes::Code;

/// The identity artifact's file name at the project root.
pub const IDS_FILE: &str = "marrow.ids";

/// The artifact header line. The version is part of the frozen line grammar.
const IDS_HEADER: &str = "marrow ids v0";
/// The machine-written notice, the artifact's second fixed line.
const IDS_NOTICE: &str = "machine-written by marrow; do not edit";
/// The end marker. A file without it is torn and rejected whole.
const IDS_END: &str = "end";

/// The fixed artifact bounds: total bytes and total rows (entries plus
/// tombstones). Both are far above any real project and guard the reader
/// against an unbounded or hostile file.
pub const MAX_IDS_BYTES: usize = 1 << 20;
pub const MAX_IDS_ROWS: usize = 4096;
/// The longest anchor path a row may carry.
const MAX_PATH_BYTES: usize = 512;

/// The kind of durable identity a ledger row anchors. A `Root` (placement)
/// anchors either a `store` root or a keyed `branch` — both are keyed placements
/// in the durable graph, distinguished by their nested anchor path. A `Sum` (5)
/// anchors a durable-reachable closed enum's identity and a `Member` (6) one of its
/// variants, so append-only enum member evolution has stable per-member codes;
/// `Group` (7) anchors an unkeyed static field-path namespace.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum IdentityKind {
    /// The application itself (one per project; anchor path `.`).
    Application,
    /// A stored resource (product) type.
    Product,
    /// One stored field of a stored resource, group, or branch.
    Field,
    /// A keyed placement: a `store` root or a keyed `branch`.
    Root,
    /// A placement's key column.
    Key,
    /// A durable-reachable closed enum (sum) type, anchored at its canonical type
    /// spelling.
    Sum,
    /// One variant of a durable-reachable closed enum, anchored at
    /// `<enum spelling>.<variant>`.
    Member,
    /// An unkeyed static field-path namespace (`group`) inside a resource,
    /// branch, or group.
    Group,
}

impl IdentityKind {
    /// Every kind, in tag order.
    pub const ALL: &'static [IdentityKind] = &[
        IdentityKind::Application,
        IdentityKind::Product,
        IdentityKind::Field,
        IdentityKind::Root,
        IdentityKind::Key,
        IdentityKind::Sum,
        IdentityKind::Member,
        IdentityKind::Group,
    ];

    /// The frozen numeric tag (also the canonical sort major).
    pub const fn tag(self) -> u8 {
        match self {
            IdentityKind::Application => 0,
            IdentityKind::Product => 1,
            IdentityKind::Field => 2,
            IdentityKind::Root => 3,
            IdentityKind::Key => 4,
            IdentityKind::Sum => 5,
            IdentityKind::Member => 6,
            IdentityKind::Group => 7,
        }
    }

    /// The artifact keyword for this kind.
    pub const fn keyword(self) -> &'static str {
        match self {
            IdentityKind::Application => "application",
            IdentityKind::Product => "product",
            IdentityKind::Field => "field",
            IdentityKind::Root => "root",
            IdentityKind::Key => "key",
            IdentityKind::Sum => "sum",
            IdentityKind::Member => "member",
            IdentityKind::Group => "group",
        }
    }

    fn from_keyword(word: &str) -> Option<IdentityKind> {
        IdentityKind::ALL
            .iter()
            .copied()
            .find(|kind| kind.keyword() == word)
    }
}

/// An entropy-minted 128-bit durable identity. Its artifact spelling is 32
/// lowercase hex digits. Distinct by construction from the 32-byte hash
/// identity family: it carries no content and is never recomputed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DurableIdentityId([u8; 16]);

impl DurableIdentityId {
    /// Wrap 16 raw entropy bytes.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The 16 identity bytes.
    pub fn bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// The canonical 32-digit lowercase-hex artifact spelling.
    pub fn to_hex(self) -> String {
        let mut hex = String::with_capacity(32);
        for byte in self.0 {
            hex.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex nibble"));
            hex.push(char::from_digit(u32::from(byte & 0xf), 16).expect("hex nibble"));
        }
        hex
    }

    fn parse_hex(text: &str) -> Option<Self> {
        if text.len() != 32 {
            return None;
        }
        let mut bytes = [0u8; 16];
        for (index, chunk) in text.as_bytes().chunks(2).enumerate() {
            let hi = hex_digit(chunk[0])?;
            let lo = hex_digit(chunk[1])?;
            bytes[index] = (hi << 4) | lo;
        }
        Some(Self(bytes))
    }
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// A `(kind, path)` anchor: the source-place identity a ledger row keys on. A
/// rename moves the anchor while the id stays; delete-then-re-add cannot reuse
/// the retired id or anchor.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct IdentityAnchor {
    pub kind: IdentityKind,
    pub path: String,
}

impl IdentityAnchor {
    pub fn new(kind: IdentityKind, path: impl Into<String>) -> Self {
        Self {
            kind,
            path: path.into(),
        }
    }
}

/// One retired identity: the id, the anchor it was retired at, and the
/// retirement high-water at which it was recorded. Tombstones are append-only
/// history; they are why a retired id stays dead across store loss.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IdentityTombstone {
    pub anchor: IdentityAnchor,
    pub id: DurableIdentityId,
    pub high_water: u64,
}

/// The durable-identity ledger: the live anchor→id rows, the append-only
/// tombstones, and the monotonic retirement high-water. Immutable in place;
/// [`IdentityLedger::with_minted`] and [`IdentityLedger::with_retired`] return
/// a new validated ledger, so a failed operation leaves the original — and its
/// serialized bytes — untouched.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct IdentityLedger {
    entries: BTreeMap<IdentityAnchor, DurableIdentityId>,
    tombstones: Vec<IdentityTombstone>,
    high_water: u64,
}

impl IdentityLedger {
    /// The id bound to `(kind, path)`, if the anchor has a live row.
    pub fn lookup(&self, kind: IdentityKind, path: &str) -> Option<DurableIdentityId> {
        self.entries.get(&IdentityAnchor::new(kind, path)).copied()
    }

    /// Whether `(kind, path)` names a retired anchor. A retired anchor can
    /// never be re-minted; re-declaring at it fails closed.
    pub fn is_retired(&self, kind: IdentityKind, path: &str) -> bool {
        self.tombstones
            .iter()
            .any(|tombstone| tombstone.anchor.kind == kind && tombstone.anchor.path == path)
    }

    /// The live rows, in canonical anchor order.
    pub fn entries(&self) -> impl Iterator<Item = (&IdentityAnchor, DurableIdentityId)> {
        self.entries.iter().map(|(anchor, id)| (anchor, *id))
    }

    /// The retirement high-water: the count of retire events this line has seen.
    pub fn high_water(&self) -> u64 {
        self.high_water
    }

    /// A new ledger with one fresh row per anchor, each bound to the caller's
    /// pre-drawn entropy candidate. Minting never retries: a candidate that
    /// collides with any live id, tombstoned id, or earlier candidate is a
    /// [`MintError::IdCollision`], an anchor with a live row is
    /// [`MintError::AnchorActive`], and a retired anchor is
    /// [`MintError::AnchorRetired`] — in every case `self` (and therefore the
    /// published artifact) is left unchanged.
    pub fn with_minted(
        &self,
        mints: &[(IdentityAnchor, DurableIdentityId)],
    ) -> Result<IdentityLedger, MintError> {
        let mut minted = self.clone();
        let mut used: BTreeSet<DurableIdentityId> = self.entries.values().copied().collect();
        used.extend(self.tombstones.iter().map(|tombstone| tombstone.id));
        for (anchor, id) in mints {
            if self.is_retired(anchor.kind, &anchor.path) {
                return Err(MintError::AnchorRetired(anchor.clone()));
            }
            if !used.insert(*id) {
                return Err(MintError::IdCollision(*id));
            }
            if minted.entries.insert(anchor.clone(), *id).is_some() {
                return Err(MintError::AnchorActive(anchor.clone()));
            }
        }
        Ok(minted)
    }

    /// A new ledger with the live row at `(kind, path)` retired into a
    /// tombstone, advancing the retirement high-water. The single retire
    /// operation; consumed by the accepted change-review action when it lands
    /// (F03) and by conformance tests today. Retiring an anchor with no live
    /// row is `None` — there is nothing to retire.
    pub fn with_retired(&self, kind: IdentityKind, path: &str) -> Option<IdentityLedger> {
        let anchor = IdentityAnchor::new(kind, path);
        let mut retired = self.clone();
        let id = retired.entries.remove(&anchor)?;
        retired.high_water = self.high_water.checked_add(1)?;
        retired.tombstones.push(IdentityTombstone {
            anchor,
            id,
            high_water: retired.high_water,
        });
        Some(retired)
    }

    /// Parse the committed artifact, rejecting any corruption whole with a
    /// typed [`IdsError`]: a torn (truncated) file, Git conflict markers, a
    /// malformed line, a duplicate anchor or id, a retired id or anchor
    /// reissued live, an inconsistent high-water, or a size past the fixed
    /// bounds. Rows may arrive in any order (a textual merge is order-blind);
    /// every invariant is validated regardless.
    pub fn parse(bytes: &[u8]) -> Result<IdentityLedger, IdsError> {
        if bytes.len() > MAX_IDS_BYTES {
            return Err(IdsError::new(IdsErrorKind::Bound, "artifact too large"));
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|_| IdsError::new(IdsErrorKind::Malformed, "artifact is not UTF-8"))?;
        for marker in ["<<<<<<< ", "=======", ">>>>>>> "] {
            if text.lines().any(|line| line.starts_with(marker)) {
                return Err(IdsError::new(
                    IdsErrorKind::ConflictMarker,
                    "unresolved Git conflict markers",
                ));
            }
        }

        let mut lines = text.lines();
        if lines.next() != Some(IDS_HEADER) {
            return Err(IdsError::new(IdsErrorKind::Header, "bad or missing header"));
        }
        if lines.next() != Some(IDS_NOTICE) {
            return Err(IdsError::new(
                IdsErrorKind::Header,
                "missing machine-written notice",
            ));
        }

        let mut ledger = IdentityLedger::default();
        let mut ids: BTreeMap<DurableIdentityId, ()> = BTreeMap::new();
        let mut high_water: Option<u64> = None;
        let mut ended = false;
        let mut rows = 0usize;
        for line in lines {
            if ended {
                return Err(IdsError::new(
                    IdsErrorKind::Malformed,
                    "content after the end marker",
                ));
            }
            if line == IDS_END {
                ended = true;
                continue;
            }
            let mut fields = line.split(' ');
            match fields.next() {
                Some("id") => {
                    rows += 1;
                    let (anchor, id) = parse_row(&mut fields, line)?;
                    if ids.insert(id, ()).is_some() {
                        return Err(IdsError::new(
                            IdsErrorKind::DuplicateId,
                            format!("id `{}` appears twice", id.to_hex()),
                        ));
                    }
                    if ledger.entries.insert(anchor.clone(), id).is_some() {
                        return Err(IdsError::new(
                            IdsErrorKind::DuplicateAnchor,
                            format!(
                                "anchor `{} {}` has two rows",
                                anchor.kind.keyword(),
                                anchor.path
                            ),
                        ));
                    }
                }
                Some("retired") => {
                    rows += 1;
                    let (anchor, id) = parse_row(&mut fields, line)?;
                    let row_water = fields
                        .next()
                        .and_then(|word| word.parse::<u64>().ok())
                        .filter(|_| fields.next().is_none())
                        .ok_or_else(|| malformed_line(line))?;
                    if row_water == 0 {
                        return Err(IdsError::new(
                            IdsErrorKind::HighWater,
                            "a retirement high-water is at least 1",
                        ));
                    }
                    if ids.insert(id, ()).is_some() {
                        return Err(IdsError::new(
                            IdsErrorKind::DuplicateId,
                            format!("retired id `{}` appears twice", id.to_hex()),
                        ));
                    }
                    ledger.tombstones.push(IdentityTombstone {
                        anchor,
                        id,
                        high_water: row_water,
                    });
                }
                Some("high-water") => {
                    let value = fields
                        .next()
                        .and_then(|word| word.parse::<u64>().ok())
                        .filter(|_| fields.next().is_none())
                        .ok_or_else(|| malformed_line(line))?;
                    if high_water.replace(value).is_some() {
                        return Err(IdsError::new(
                            IdsErrorKind::Malformed,
                            "two high-water lines",
                        ));
                    }
                }
                _ => return Err(malformed_line(line)),
            }
            if rows > MAX_IDS_ROWS {
                return Err(IdsError::new(IdsErrorKind::Bound, "too many rows"));
            }
        }
        if !ended {
            return Err(IdsError::new(
                IdsErrorKind::Torn,
                "missing end marker; the artifact is truncated",
            ));
        }
        ledger.high_water = high_water
            .ok_or_else(|| IdsError::new(IdsErrorKind::Malformed, "missing high-water line"))?;
        // The retirement counter must be advanceable; a saturated value could
        // silently reuse a witnessed retirement number.
        if ledger.high_water >= u64::MAX - 1 {
            return Err(IdsError::new(
                IdsErrorKind::HighWater,
                "high-water cannot be advanced",
            ));
        }
        // Cross-row invariants: a retired anchor or id must not also be live,
        // and no tombstone can record a retirement past the ledger high-water.
        let mut reserved_anchors: BTreeSet<&IdentityAnchor> = BTreeSet::new();
        for tombstone in &ledger.tombstones {
            if tombstone.high_water > ledger.high_water {
                return Err(IdsError::new(
                    IdsErrorKind::HighWater,
                    format!(
                        "retired id `{}` records high-water {} past the ledger's {}",
                        tombstone.id.to_hex(),
                        tombstone.high_water,
                        ledger.high_water
                    ),
                ));
            }
            if ledger.entries.contains_key(&tombstone.anchor) {
                return Err(IdsError::new(
                    IdsErrorKind::RetiredReuse,
                    format!(
                        "retired anchor `{} {}` also has a live row",
                        tombstone.anchor.kind.keyword(),
                        tombstone.anchor.path
                    ),
                ));
            }
            if !reserved_anchors.insert(&tombstone.anchor) {
                return Err(IdsError::new(
                    IdsErrorKind::RetiredReuse,
                    format!(
                        "anchor `{} {}` is retired twice",
                        tombstone.anchor.kind.keyword(),
                        tombstone.anchor.path
                    ),
                ));
            }
        }
        Ok(ledger)
    }

    /// The canonical artifact bytes: header, notice, live rows in anchor order,
    /// tombstones in `(anchor, id)` order, the high-water, and the end marker.
    /// The same logical ledger always serializes byte-identically.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        out.push_str(IDS_HEADER);
        out.push('\n');
        out.push_str(IDS_NOTICE);
        out.push('\n');
        for (anchor, id) in &self.entries {
            out.push_str("id ");
            out.push_str(anchor.kind.keyword());
            out.push(' ');
            out.push_str(&anchor.path);
            out.push(' ');
            out.push_str(&id.to_hex());
            out.push('\n');
        }
        let mut tombstones: Vec<&IdentityTombstone> = self.tombstones.iter().collect();
        tombstones.sort_by(|a, b| (&a.anchor, a.id).cmp(&(&b.anchor, b.id)));
        for tombstone in tombstones {
            out.push_str("retired ");
            out.push_str(tombstone.anchor.kind.keyword());
            out.push(' ');
            out.push_str(&tombstone.anchor.path);
            out.push(' ');
            out.push_str(&tombstone.id.to_hex());
            out.push(' ');
            out.push_str(&tombstone.high_water.to_string());
            out.push('\n');
        }
        out.push_str("high-water ");
        out.push_str(&self.high_water.to_string());
        out.push('\n');
        out.push_str(IDS_END);
        out.push('\n');
        out.into_bytes()
    }
}

/// Parse the shared `<kind> <path> <hex-id>` core of an `id` or `retired` row.
/// The iterator is left positioned after the id, so a `retired` row reads its
/// high-water next and an `id` row must be exhausted by the caller.
fn parse_row<'a>(
    fields: &mut std::str::Split<'a, char>,
    line: &str,
) -> Result<(IdentityAnchor, DurableIdentityId), IdsError> {
    let kind = fields
        .next()
        .and_then(IdentityKind::from_keyword)
        .ok_or_else(|| malformed_line(line))?;
    let path = fields.next().ok_or_else(|| malformed_line(line))?;
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || !path.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(malformed_line(line));
    }
    let id = fields
        .next()
        .and_then(DurableIdentityId::parse_hex)
        .ok_or_else(|| malformed_line(line))?;
    Ok((IdentityAnchor::new(kind, path), id))
}

fn malformed_line(line: &str) -> IdsError {
    let mut shown: String = line.chars().take(80).collect();
    if shown.len() < line.len() {
        shown.push('…');
    }
    IdsError::new(IdsErrorKind::Malformed, format!("malformed row `{shown}`"))
}

/// Why a `marrow.ids` artifact was rejected whole.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IdsErrorKind {
    /// A bad or missing header or machine-written notice.
    Header,
    /// Unresolved Git conflict markers.
    ConflictMarker,
    /// The end marker is missing: the file is truncated (torn).
    Torn,
    /// A row or line does not match the artifact grammar, or content trails
    /// the end marker.
    Malformed,
    /// One id appears on two rows (live or retired).
    DuplicateId,
    /// One `(kind, path)` anchor has two live rows.
    DuplicateAnchor,
    /// A retired anchor also appears live, or is retired twice.
    RetiredReuse,
    /// A retirement high-water is inconsistent or not advanceable.
    HighWater,
    /// The artifact exceeds a fixed size bound.
    Bound,
}

/// A corrupt `marrow.ids` artifact: the stable `project.ids_corrupt` code, a
/// typed reason, and a human message. The artifact is rejected whole; nothing
/// is half-read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IdsError {
    pub code: &'static str,
    pub kind: IdsErrorKind,
    pub message: String,
}

impl IdsError {
    fn new(kind: IdsErrorKind, message: impl Into<String>) -> Self {
        Self {
            code: Code::ProjectIdsCorrupt.as_str(),
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for IdsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for IdsError {}

/// Why a mint could not be published. Minting never retries: every failure
/// leaves the existing ledger — and its artifact bytes — unchanged.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MintError {
    /// A drawn id collided with a live id, a tombstoned id, or another draw.
    IdCollision(DurableIdentityId),
    /// The anchor already has a live row; there is nothing to mint.
    AnchorActive(IdentityAnchor),
    /// The anchor is retired; a retired identity can never be reused.
    AnchorRetired(IdentityAnchor),
}

impl fmt::Display for MintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MintError::IdCollision(id) => {
                write!(f, "freshly drawn id `{}` collides", id.to_hex())
            }
            MintError::AnchorActive(anchor) => write!(
                f,
                "anchor `{} {}` already has a live identity",
                anchor.kind.keyword(),
                anchor.path
            ),
            MintError::AnchorRetired(anchor) => write!(
                f,
                "anchor `{} {}` is retired and can never be reused",
                anchor.kind.keyword(),
                anchor.path
            ),
        }
    }
}

impl std::error::Error for MintError {}

#[cfg(test)]
mod tests {
    use super::{
        DurableIdentityId, IdentityAnchor, IdentityKind, IdentityLedger, IdsErrorKind, MintError,
    };

    fn id(byte: u8) -> DurableIdentityId {
        DurableIdentityId::from_bytes([byte; 16])
    }

    fn anchor(kind: IdentityKind, path: &str) -> IdentityAnchor {
        IdentityAnchor::new(kind, path)
    }

    fn counter_ledger() -> IdentityLedger {
        IdentityLedger::default()
            .with_minted(&[
                (anchor(IdentityKind::Application, "."), id(0x0a)),
                (anchor(IdentityKind::Root, "counters"), id(0x0b)),
                (anchor(IdentityKind::Key, "counters.name"), id(0x0c)),
                (anchor(IdentityKind::Product, "Counter"), id(0x0d)),
                (anchor(IdentityKind::Field, "Counter.value"), id(0x0e)),
                (anchor(IdentityKind::Field, "Counter.label"), id(0x0f)),
            ])
            .expect("mint the counter rows")
    }

    #[test]
    fn serialization_is_canonical_and_round_trips() {
        let ledger = counter_ledger();
        let bytes = ledger.to_bytes();
        let reparsed = IdentityLedger::parse(&bytes).expect("reparse");
        assert_eq!(reparsed, ledger);
        assert_eq!(reparsed.to_bytes(), bytes, "canonical bytes are stable");
    }

    #[test]
    fn parse_accepts_any_row_order_but_write_is_sorted() {
        let canonical = counter_ledger().to_bytes();
        let text = String::from_utf8(canonical.clone()).unwrap();
        let mut lines: Vec<&str> = text.lines().collect();
        // Reverse the row block (between the two header lines and the
        // high-water/end tail) to simulate a merge that interleaved rows.
        lines[2..8].reverse();
        let shuffled = format!("{}\n", lines.join("\n"));
        let reparsed = IdentityLedger::parse(shuffled.as_bytes()).expect("order-blind parse");
        assert_eq!(reparsed.to_bytes(), canonical);
    }

    #[test]
    fn a_conflicting_double_mint_is_rejected_as_duplicate_anchor_or_id() {
        let base = String::from_utf8(counter_ledger().to_bytes()).unwrap();
        // Two branches minted the same anchor with different entropy: the
        // merged file carries both rows and is rejected whole.
        let dup_anchor = base.replace(
            "id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n",
            "id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
             id field Counter.value 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n",
        );
        let error = IdentityLedger::parse(dup_anchor.as_bytes()).unwrap_err();
        assert_eq!(error.kind, IdsErrorKind::DuplicateAnchor);

        let dup_id = base.replace(
            "id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n",
            "id product Counter 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n",
        );
        let error = IdentityLedger::parse(dup_id.as_bytes()).unwrap_err();
        assert_eq!(error.kind, IdsErrorKind::DuplicateId);
    }

    #[test]
    fn conflict_markers_and_torn_files_reject_whole() {
        let base = String::from_utf8(counter_ledger().to_bytes()).unwrap();
        for marker in ["<<<<<<< ours", "=======", ">>>>>>> theirs"] {
            let conflicted = base.replace("high-water", &format!("{marker}\nhigh-water"));
            assert_eq!(
                IdentityLedger::parse(conflicted.as_bytes())
                    .unwrap_err()
                    .kind,
                IdsErrorKind::ConflictMarker,
                "marker {marker:?} must reject as a conflict"
            );
        }

        // A torn write loses the tail: no end marker, rejected whole.
        let torn = base.replace("end\n", "");
        assert_eq!(
            IdentityLedger::parse(torn.as_bytes()).unwrap_err().kind,
            IdsErrorKind::Torn
        );

        // Content after the end marker is equally torn state.
        let trailing = format!("{base}id root extra 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n");
        assert_eq!(
            IdentityLedger::parse(trailing.as_bytes()).unwrap_err().kind,
            IdsErrorKind::Malformed
        );
    }

    #[test]
    fn retire_then_re_add_cannot_reuse_the_anchor_or_id() {
        let ledger = counter_ledger();
        let retired = ledger
            .with_retired(IdentityKind::Field, "Counter.label")
            .expect("retire a live row");
        assert_eq!(retired.high_water(), 1);
        assert!(
            retired
                .lookup(IdentityKind::Field, "Counter.label")
                .is_none()
        );
        assert!(retired.is_retired(IdentityKind::Field, "Counter.label"));

        // Re-adding at the retired anchor is refused.
        let re_add =
            retired.with_minted(&[(anchor(IdentityKind::Field, "Counter.label"), id(0x20))]);
        assert!(matches!(re_add, Err(MintError::AnchorRetired(_))));

        // The retired id can never be drawn again either.
        let reuse = retired.with_minted(&[(anchor(IdentityKind::Field, "Counter.note"), id(0x0f))]);
        assert!(matches!(reuse, Err(MintError::IdCollision(_))));

        // The tombstone round-trips through the artifact.
        let reparsed = IdentityLedger::parse(&retired.to_bytes()).expect("reparse");
        assert!(reparsed.is_retired(IdentityKind::Field, "Counter.label"));
        assert_eq!(reparsed, retired);
    }

    #[test]
    fn a_retired_id_or_anchor_reissued_live_rejects_at_parse() {
        let retired = counter_ledger()
            .with_retired(IdentityKind::Field, "Counter.label")
            .unwrap();
        let base = String::from_utf8(retired.to_bytes()).unwrap();

        // The retired anchor also live.
        let live_anchor = base.replace(
            "high-water 1",
            "id field Counter.label 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\nhigh-water 1",
        );
        assert_eq!(
            IdentityLedger::parse(live_anchor.as_bytes())
                .unwrap_err()
                .kind,
            IdsErrorKind::RetiredReuse
        );

        // The retired id reissued on a live row.
        let live_id = base.replace(
            "high-water 1",
            "id field Counter.note 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\nhigh-water 1",
        );
        assert_eq!(
            IdentityLedger::parse(live_id.as_bytes()).unwrap_err().kind,
            IdsErrorKind::DuplicateId
        );

        // A tombstone past the ledger high-water is inconsistent history.
        let bad_water = base.replace("high-water 1", "high-water 0");
        assert_eq!(
            IdentityLedger::parse(bad_water.as_bytes())
                .unwrap_err()
                .kind,
            IdsErrorKind::HighWater
        );
    }

    #[test]
    fn mint_failure_leaves_the_ledger_bytes_unchanged() {
        let ledger = counter_ledger();
        let before = ledger.to_bytes();
        // A colliding draw (the no-retry contract): the operation fails and the
        // original ledger — the bytes the artifact holds — is untouched.
        let result = ledger.with_minted(&[(anchor(IdentityKind::Field, "Counter.note"), id(0x0a))]);
        assert!(matches!(result, Err(MintError::IdCollision(_))));
        assert_eq!(ledger.to_bytes(), before);
    }

    #[test]
    fn header_and_grammar_violations_reject() {
        assert_eq!(
            IdentityLedger::parse(b"nonsense\n").unwrap_err().kind,
            IdsErrorKind::Header
        );
        let missing_notice = "marrow ids v0\nhigh-water 0\nend\n";
        assert_eq!(
            IdentityLedger::parse(missing_notice.as_bytes())
                .unwrap_err()
                .kind,
            IdsErrorKind::Header
        );
        let bad_row = "marrow ids v0\nmachine-written by marrow; do not edit\n\
                       id widget thing 00000000000000000000000000000000\nhigh-water 0\nend\n";
        assert_eq!(
            IdentityLedger::parse(bad_row.as_bytes()).unwrap_err().kind,
            IdsErrorKind::Malformed
        );
        let short_id = "marrow ids v0\nmachine-written by marrow; do not edit\n\
                        id root counters 0000\nhigh-water 0\nend\n";
        assert_eq!(
            IdentityLedger::parse(short_id.as_bytes()).unwrap_err().kind,
            IdsErrorKind::Malformed
        );
        let no_water = "marrow ids v0\nmachine-written by marrow; do not edit\nend\n";
        assert_eq!(
            IdentityLedger::parse(no_water.as_bytes()).unwrap_err().kind,
            IdsErrorKind::Malformed
        );
    }

    #[test]
    fn artifact_bounds_reject_oversize_input() {
        let huge = vec![b'a'; super::MAX_IDS_BYTES + 1];
        assert_eq!(
            IdentityLedger::parse(&huge).unwrap_err().kind,
            IdsErrorKind::Bound
        );
    }

    #[test]
    fn the_kind_tag_space_is_frozen_and_reserved() {
        // The frozen kind tag space: application/product/field/root/key, sum/member
        // (durable enum identity), and group.
        let tags: Vec<u8> = IdentityKind::ALL.iter().map(|kind| kind.tag()).collect();
        assert_eq!(tags, vec![0, 1, 2, 3, 4, 5, 6, 7]);
        for kind in IdentityKind::ALL {
            assert_eq!(
                IdentityKind::from_keyword(kind.keyword()),
                Some(*kind),
                "keyword round-trips"
            );
        }
    }
}
