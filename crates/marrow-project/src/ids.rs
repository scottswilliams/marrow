//! The durable-identity ledger and its committed artifact, `.marrow/ids`.
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
//! The artifact is machine-written only. Developers never edit, copy, or cite
//! ids; the artifact is committed and line-diffable so parallel branches merge
//! textually, and a conflicting double-mint (two rows claiming one anchor or
//! one id) is rejected whole as [`IdsError`] — the artifact is never half-read.
//! Parsing accepts rows in any order so a textual merge stays valid;
//! serialization is canonical (sorted, bounded), so publication is
//! deterministic byte-for-byte.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use marrow_codes::Code;

/// The behind-the-scenes project-metadata directory at the project root. It
/// holds machine-written project artifacts only. Exactly one of them is
/// committed — the identity ledger, which is part of the program and travels
/// with the source; everything else is machine-local runtime state that no
/// checkout carries. Caches and stores never live here.
pub const META_DIR: &str = ".marrow";

/// The identity ledger's entry name inside [`META_DIR`]. The physical adapter
/// spells the ledger through this owner rather than repeating either half.
pub const IDS_ENTRY: &str = "ids";

/// The identity artifact's root-relative path: the ledger's one home, inside
/// the project-metadata directory.
pub const IDS_FILE: &str = ".marrow/ids";

// The joined path and its two parts are one spelling, checked at compile time so
// a rename of either half cannot leave a second live ledger location behind.
const _: () = assert!(
    joins_to(IDS_FILE, META_DIR, IDS_ENTRY),
    "the ledger's root-relative path must be its directory and entry spellings joined"
);

/// Whether `joined` is exactly `dir`, a separator, and `entry`.
const fn joins_to(joined: &str, dir: &str, entry: &str) -> bool {
    let (joined, dir, entry) = (joined.as_bytes(), dir.as_bytes(), entry.as_bytes());
    if joined.len() != dir.len() + 1 + entry.len() {
        return false;
    }
    let mut head = 0;
    while head < dir.len() {
        if joined[head] != dir[head] {
            return false;
        }
        head += 1;
    }
    if joined[dir.len()] != b'/' {
        return false;
    }
    let mut tail = 0;
    while tail < entry.len() {
        if joined[dir.len() + 1 + tail] != entry[tail] {
            return false;
        }
        tail += 1;
    }
    true
}

/// The ledger's retired pre-relocation path at the project root. Nothing reads
/// it: capture refuses a file here with a one-line steer to the ledger's home,
/// so a project never has two live ledger locations.
pub const LEGACY_IDS_FILE: &str = "marrow.ids";

/// The artifact header line. The version is part of the frozen line grammar.
const IDS_HEADER: &str = "marrow ids v0";
/// The machine-written notice, the artifact's second fixed line.
const IDS_NOTICE: &str = "machine-written by marrow; do not edit";
/// The end marker. A file without it is torn and rejected whole.
const IDS_END: &str = "end";
/// The one lowercase alphabet used by both public and artifact id rendering.
const ID_HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// The fixed artifact bounds: total bytes and total rows (entries plus
/// tombstones). Both guard the reader against an unbounded or hostile file, and
/// like every Marrow decode bound they are monotone reject-guards, not
/// stored-format bytes — the `marrow ids v0` header is unchanged, so an older
/// toolchain meeting a larger artifact rejects it rather than misreading it.
///
/// The row cap tracks the durable member-tree scale so the full record-field
/// width guard is reachable for a single wide resource: a resource of
/// `marrow-image`'s `MAX_RECORD_FIELDS` (4096) declared fields anchors one `Field`
/// row per field plus a small fixed overhead (application, product, root
/// placement, and its key columns), ~4100 rows — past the former 4096 cap. The
/// value matches `marrow-image`'s `MAX_DURABLE_MEMBERS` (8192, the member-tree
/// total) as the one obvious ceiling; `MAX_IDS_BYTES` carries a single wide
/// resource with headroom (~4100 rows ≈ 250 KB « 1 MiB) but is not the binder at
/// this width — the field-count guard is. A multi-root project carrying several
/// wide resources can still exceed this row cap; sizing for that is a separate
/// future widen.
pub const MAX_IDS_BYTES: usize = 1 << 20;
pub const MAX_IDS_ROWS: usize = 8192;
/// The longest anchor path a row may carry.
const MAX_PATH_BYTES: usize = 512;

// The ledger row cap must admit a full record-field-width resource plus its fixed
// placement overhead, or the durable width guard would be unreachable through the
// ledger. `marrow-image`'s `MAX_RECORD_FIELDS` is 4096; this crate does not depend on
// `marrow-image`, so the width is stated as the documented cross-crate invariant here
// and `marrow-image::bounds` carries the image-side half.
const _: () = assert!(
    MAX_IDS_ROWS >= 4096 + 16,
    "the ledger row cap must admit a full MAX_RECORD_FIELDS-width resource plus overhead",
);

/// The kind of durable identity a ledger row anchors. A `Root` (placement)
/// anchors either a `store` root or a keyed `branch` — both are keyed placements
/// in the durable graph, distinguished by their nested anchor path. A `Sum` (5)
/// anchors a durable-reachable closed enum's identity and a `Member` (6) one of its
/// variants, so append-only enum member evolution has stable per-member codes;
/// `Group` (7) anchors an unkeyed static field-path namespace; `Index` (8) anchors a
/// narrow compiler-maintained managed index of a keyed store root.
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
    /// A narrow compiler-maintained managed index of a keyed store root, anchored
    /// at `<root>.<index name>`.
    Index,
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
        IdentityKind::Index,
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
            IdentityKind::Index => 8,
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
            IdentityKind::Index => "index",
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
        let mut bytes = [0; 32];
        self.write_canonical_hex(&mut bytes);
        String::from_utf8(bytes.to_vec()).expect("canonical identity hex is ASCII")
    }

    fn write_canonical_hex(self, output: &mut [u8; 32]) {
        for (index, byte) in self.0.into_iter().enumerate() {
            output[index * 2] = ID_HEX_DIGITS[usize::from(byte >> 4)];
            output[index * 2 + 1] = ID_HEX_DIGITS[usize::from(byte & 0x0f)];
        }
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
/// tombstones, and the monotonic retirement high-water. This is a read-only
/// semantic view: mutation and canonical serialization belong only to captured
/// project admission.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct IdentityLedger {
    entries: BTreeMap<IdentityAnchor, DurableIdentityId>,
    tombstones: Vec<IdentityTombstone>,
    high_water: u64,
}

#[cfg(test)]
std::thread_local! {
    static TOMBSTONE_LOOKUP_COMPARISONS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

fn compare_tombstone_anchor(
    tombstone: &IdentityTombstone,
    kind: IdentityKind,
    path: &str,
) -> Ordering {
    #[cfg(test)]
    TOMBSTONE_LOOKUP_COMPARISONS.set(TOMBSTONE_LOOKUP_COMPARISONS.get() + 1);
    tombstone
        .anchor
        .kind
        .cmp(&kind)
        .then_with(|| tombstone.anchor.path.as_str().cmp(path))
}

#[cfg(test)]
fn reset_tombstone_lookup_comparisons() {
    TOMBSTONE_LOOKUP_COMPARISONS.set(0);
}

#[cfg(test)]
fn tombstone_lookup_comparisons() -> usize {
    TOMBSTONE_LOOKUP_COMPARISONS.get()
}

impl IdentityLedger {
    /// The id bound to `(kind, path)`, if the anchor has a live row.
    pub fn lookup(&self, kind: IdentityKind, path: &str) -> Option<DurableIdentityId> {
        self.entries.get(&IdentityAnchor::new(kind, path)).copied()
    }

    /// Whether `(kind, path)` names a retired anchor. A retired anchor can
    /// never be re-minted; re-declaring at it fails closed.
    pub fn is_retired(&self, kind: IdentityKind, path: &str) -> bool {
        self.tombstone_index(kind, path).is_ok()
    }

    /// The live rows, in canonical anchor order.
    pub fn entries(&self) -> impl Iterator<Item = (&IdentityAnchor, DurableIdentityId)> {
        self.entries.iter().map(|(anchor, id)| (anchor, *id))
    }

    /// The retirement high-water: the count of retire events this line has seen.
    pub fn high_water(&self) -> u64 {
        self.high_water
    }

    fn tombstone_index(&self, kind: IdentityKind, path: &str) -> Result<usize, usize> {
        self.tombstones
            .binary_search_by(|tombstone| compare_tombstone_anchor(tombstone, kind, path))
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
                    if fields.next().is_some() {
                        return Err(malformed_line(line));
                    }
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
        // Preserve the parser's input-order rejection precedence above, then
        // normalize only the admitted semantic state for equality, lookup, and
        // canonical successor construction.
        ledger.tombstones.sort_by(canonical_tombstone_order);
        Ok(ledger)
    }
}

fn canonical_tombstone_order(left: &IdentityTombstone, right: &IdentityTombstone) -> Ordering {
    (&left.anchor, left.id).cmp(&(&right.anchor, right.id))
}

/// A validated captured identity artifact and its read-only semantic ledger.
///
/// Only project capture constructs this proof. `Present` retains the exact
/// parser-validated artifact bytes; `Absent` carries the private empty ledger.
#[derive(Clone)]
pub(crate) struct CapturedLedger {
    ledger: IdentityLedger,
    artifact: CapturedLedgerArtifact,
}

#[derive(Clone)]
enum CapturedLedgerArtifact {
    Absent,
    Present(Arc<[u8]>),
}

impl CapturedLedger {
    pub(crate) fn capture(bytes: Option<&[u8]>) -> Result<Self, IdsError> {
        match bytes {
            Some(bytes) => Ok(Self {
                ledger: IdentityLedger::parse(bytes)?,
                artifact: CapturedLedgerArtifact::Present(Arc::from(bytes)),
            }),
            None => Ok(Self {
                ledger: IdentityLedger::default(),
                artifact: CapturedLedgerArtifact::Absent,
            }),
        }
    }

    pub(crate) fn present_ledger(&self) -> Option<&IdentityLedger> {
        match self.artifact {
            CapturedLedgerArtifact::Absent => None,
            CapturedLedgerArtifact::Present(_) => Some(&self.ledger),
        }
    }

    pub(crate) fn admit_identity_mints_with<E>(
        &self,
        first: IdentityAnchor,
        rest: Vec<IdentityAnchor>,
        supply: impl FnOnce(usize) -> Result<Vec<DurableIdentityId>, E>,
    ) -> Result<LedgerPublicationPlan, IdentityMintFailure<E>> {
        let plan =
            LedgerMutationPlan::mint(self, first, rest).map_err(IdentityMintFailure::Mutation)?;
        let exact_count = plan.change_count();
        let candidates = supply(exact_count).map_err(IdentityMintFailure::Supply)?;
        plan.bind_candidates(candidates)
            .and_then(AdmittedLedger::publication)
            .map_err(IdentityMintFailure::Mutation)
    }

    fn expected_artifact(&self) -> LedgerExpectedArtifactOwned {
        match &self.artifact {
            CapturedLedgerArtifact::Absent => LedgerExpectedArtifactOwned::Absent,
            CapturedLedgerArtifact::Present(bytes) => {
                LedgerExpectedArtifactOwned::Present(Arc::clone(bytes))
            }
        }
    }
}

impl PartialEq for CapturedLedger {
    fn eq(&self, other: &Self) -> bool {
        let same_presence = matches!(
            (&self.artifact, &other.artifact),
            (
                CapturedLedgerArtifact::Absent,
                CapturedLedgerArtifact::Absent
            ) | (
                CapturedLedgerArtifact::Present(_),
                CapturedLedgerArtifact::Present(_)
            )
        );
        same_presence && self.ledger == other.ledger
    }
}

impl Eq for CapturedLedger {}

impl fmt::Debug for CapturedLedger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let artifact = match self.artifact {
            CapturedLedgerArtifact::Absent => "Absent",
            CapturedLedgerArtifact::Present(_) => "Present",
        };
        f.debug_struct("CapturedLedger")
            .field("ledger", &self.ledger)
            .field("artifact", &artifact)
            .finish()
    }
}

/// A typed refusal from identity mutation admission or candidate binding.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum IdentityMutationError {
    /// An anchor path is empty, non-printable, contains a space, or exceeds 512 bytes.
    InvalidAnchor(IdentityAnchor),
    /// One operation requested the same anchor more than once.
    DuplicateRequest(IdentityAnchor),
    /// A mint request names an already-live anchor.
    AnchorActive(IdentityAnchor),
    /// A mint request names a retired anchor.
    AnchorRetired(IdentityAnchor),
    /// A retirement request names no live anchor.
    AnchorNotActive(IdentityAnchor),
    /// The successor would exceed the fixed live-plus-tombstone row ceiling.
    RowLimit { projected: usize, limit: usize },
    /// The retirement successor would violate the parser's advanceable high-water law.
    RetirementHighWater,
    /// The canonical successor would exceed the fixed artifact-byte ceiling.
    ByteLimit { projected: usize, limit: usize },
    /// The supplier returned a different number of candidates from the admitted request.
    CandidateCount { expected: usize, actual: usize },
    /// A candidate collides with a live, retired, or earlier candidate id.
    IdCollision(DurableIdentityId),
    /// Checked canonical-length arithmetic could not represent the successor.
    CanonicalLengthOverflow,
    /// The serializer disagreed with its admitted exact canonical length.
    CanonicalLengthMismatch { projected: usize, actual: usize },
    /// An immutable admitted state changed between preflight and binding.
    AdmittedStateMismatch,
}

impl IdentityMutationError {
    /// The stable outward code for every planning or binding refusal.
    pub const fn code(&self) -> Code {
        Code::ProjectIdsMint
    }
}

impl fmt::Display for IdentityMutationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAnchor(anchor) => write!(
                f,
                "anchor `{} {}` is outside the identity path grammar",
                anchor.kind.keyword(),
                anchor.path
            ),
            Self::DuplicateRequest(anchor) => write!(
                f,
                "anchor `{} {}` was requested more than once",
                anchor.kind.keyword(),
                anchor.path
            ),
            Self::AnchorActive(anchor) => write!(
                f,
                "anchor `{} {}` already has a live identity",
                anchor.kind.keyword(),
                anchor.path
            ),
            Self::AnchorRetired(anchor) => write!(
                f,
                "anchor `{} {}` is retired and can never be reused",
                anchor.kind.keyword(),
                anchor.path
            ),
            Self::AnchorNotActive(anchor) => write!(
                f,
                "anchor `{} {}` has no live identity to retire",
                anchor.kind.keyword(),
                anchor.path
            ),
            Self::RowLimit { projected, limit } => {
                write!(
                    f,
                    "identity successor has {projected} rows; the limit is {limit}"
                )
            }
            Self::RetirementHighWater => {
                f.write_str("retirement successor cannot keep an advanceable high-water")
            }
            Self::ByteLimit { projected, limit } => write!(
                f,
                "identity successor has {projected} canonical bytes; the limit is {limit}"
            ),
            Self::CandidateCount { expected, actual } => write!(
                f,
                "identity supplier returned {actual} candidates; exactly {expected} were required"
            ),
            Self::IdCollision(id) => {
                write!(f, "freshly drawn id `{}` collides", id.to_hex())
            }
            Self::CanonicalLengthOverflow => {
                f.write_str("identity successor canonical length overflowed")
            }
            Self::CanonicalLengthMismatch { projected, actual } => write!(
                f,
                "identity successor length was projected as {projected} bytes but serialized as {actual}"
            ),
            Self::AdmittedStateMismatch => {
                f.write_str("identity successor disagreed with its admitted state")
            }
        }
    }
}

impl std::error::Error for IdentityMutationError {}

/// Candidate supply and admitted-mutation failures remain distinct while sharing
/// the stable `project.ids_mint` outward mapping.
#[derive(Debug)]
pub enum IdentityMintFailure<E> {
    /// The caller's candidate supplier failed.
    Supply(E),
    /// Grammar, state, capacity, candidate, or canonicalization admission failed.
    Mutation(IdentityMutationError),
}

impl<E> IdentityMintFailure<E> {
    /// The stable outward code for either failure arm.
    pub const fn code(&self) -> Code {
        Code::ProjectIdsMint
    }
}

impl<E: fmt::Display> fmt::Display for IdentityMintFailure<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Supply(error) => write!(f, "identity candidate supply failed: {error}"),
            Self::Mutation(error) => error.fmt(f),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for IdentityMintFailure<E> {}

/// The exact captured artifact state bound into a publication plan.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LedgerExpectedArtifact<'a> {
    /// Capture found no `.marrow/ids` artifact.
    Absent,
    /// Capture validated and retained these exact artifact bytes.
    Present(&'a [u8]),
}

impl fmt::Debug for LedgerExpectedArtifact<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent => f.write_str("Absent"),
            Self::Present(bytes) => f
                .debug_struct("Present")
                .field("byte_len", &bytes.len())
                .finish(),
        }
    }
}

/// One borrowed view of both halves of an affine publication binding.
pub struct LedgerPublicationView<'a> {
    expected: LedgerExpectedArtifact<'a>,
    next: &'a [u8],
}

impl<'a> LedgerPublicationView<'a> {
    /// The exact captured state this successor was admitted against.
    pub fn expected(&self) -> LedgerExpectedArtifact<'a> {
        self.expected
    }

    /// The canonical admitted successor bytes.
    pub fn next(&self) -> &'a [u8] {
        self.next
    }
}

impl fmt::Debug for LedgerPublicationView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LedgerPublicationView")
            .field("expected", &self.expected)
            .field("next_byte_len", &self.next.len())
            .finish()
    }
}

enum LedgerExpectedArtifactOwned {
    Absent,
    Present(Arc<[u8]>),
}

/// An affine binding from one exact captured identity artifact state to one
/// canonical admitted successor. It is constructible only by project admission
/// and consumed as one borrowed two-half view.
///
/// Raw halves cannot construct a capability because its fields are private:
///
/// ```compile_fail
/// use marrow_project::LedgerPublicationPlan;
///
/// let _ = LedgerPublicationPlan {
///     expected: todo!(),
///     next: Vec::new(),
/// };
/// ```
///
/// A capability is not cloneable:
///
/// ```compile_fail
/// fn duplicate(plan: marrow_project::LedgerPublicationPlan) {
///     let _ = plan.clone();
/// }
/// ```
#[must_use = "a ledger publication plan must be consumed by the publication owner"]
pub struct LedgerPublicationPlan {
    expected: LedgerExpectedArtifactOwned,
    next: Vec<u8>,
}

impl LedgerPublicationPlan {
    /// Consume this capability and visit its expected state and successor together.
    ///
    /// The visitor may necessarily copy borrowed bytes, but no raw halves can be
    /// supplied back to construct a plan.
    pub fn visit<R>(self, visitor: impl for<'a> FnOnce(LedgerPublicationView<'a>) -> R) -> R {
        let expected = match &self.expected {
            LedgerExpectedArtifactOwned::Absent => LedgerExpectedArtifact::Absent,
            LedgerExpectedArtifactOwned::Present(bytes) => LedgerExpectedArtifact::Present(bytes),
        };
        visitor(LedgerPublicationView {
            expected,
            next: &self.next,
        })
    }
}

impl fmt::Debug for LedgerPublicationPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let expected = match self.expected {
            LedgerExpectedArtifactOwned::Absent => "Absent",
            LedgerExpectedArtifactOwned::Present(_) => "Present",
        };
        f.debug_struct("LedgerPublicationPlan")
            .field("expected", &expected)
            .field("next_byte_len", &self.next.len())
            .finish()
    }
}

enum LedgerMutationKind {
    Mint(Vec<IdentityAnchor>),
    #[cfg(test)]
    Retire {
        anchor: IdentityAnchor,
        successor_high_water: u64,
    },
}

/// The private borrowing owner of one structurally nonempty mutation kind.
struct LedgerMutationPlan<'a> {
    captured: &'a CapturedLedger,
    kind: LedgerMutationKind,
    projected_len: usize,
}

impl<'a> LedgerMutationPlan<'a> {
    fn mint(
        captured: &'a CapturedLedger,
        first: IdentityAnchor,
        mut rest: Vec<IdentityAnchor>,
    ) -> Result<Self, IdentityMutationError> {
        rest.push(first);
        rest.sort();
        let rest = validate_requests(rest)?;
        for anchor in &rest {
            if captured.ledger.entries.contains_key(anchor) {
                return Err(IdentityMutationError::AnchorActive(anchor.clone()));
            }
            if captured
                .ledger
                .tombstone_index(anchor.kind, &anchor.path)
                .is_ok()
            {
                return Err(IdentityMutationError::AnchorRetired(anchor.clone()));
            }
        }
        let base_rows = captured
            .ledger
            .entries
            .len()
            .checked_add(captured.ledger.tombstones.len())
            .ok_or(IdentityMutationError::CanonicalLengthOverflow)?;
        let projected_rows = base_rows
            .checked_add(rest.len())
            .ok_or(IdentityMutationError::CanonicalLengthOverflow)?;
        if projected_rows > MAX_IDS_ROWS {
            return Err(IdentityMutationError::RowLimit {
                projected: projected_rows,
                limit: MAX_IDS_ROWS,
            });
        }
        let kind = LedgerMutationKind::Mint(rest);
        let projected_len = projected_canonical_len(&captured.ledger, &kind)?;
        if projected_len > MAX_IDS_BYTES {
            return Err(IdentityMutationError::ByteLimit {
                projected: projected_len,
                limit: MAX_IDS_BYTES,
            });
        }
        Ok(Self {
            captured,
            kind,
            projected_len,
        })
    }

    #[cfg(test)]
    fn retire(
        captured: &'a CapturedLedger,
        anchor: IdentityAnchor,
    ) -> Result<Self, IdentityMutationError> {
        if !valid_anchor_path(&anchor.path) {
            return Err(IdentityMutationError::InvalidAnchor(anchor));
        }
        if !captured.ledger.entries.contains_key(&anchor) {
            return Err(IdentityMutationError::AnchorNotActive(anchor));
        }
        let successor_high_water = captured
            .ledger
            .high_water
            .checked_add(1)
            .ok_or(IdentityMutationError::RetirementHighWater)?;
        if successor_high_water >= u64::MAX - 1 {
            return Err(IdentityMutationError::RetirementHighWater);
        }
        let kind = LedgerMutationKind::Retire {
            anchor,
            successor_high_water,
        };
        let projected_len = projected_canonical_len(&captured.ledger, &kind)?;
        if projected_len > MAX_IDS_BYTES {
            return Err(IdentityMutationError::ByteLimit {
                projected: projected_len,
                limit: MAX_IDS_BYTES,
            });
        }
        Ok(Self {
            captured,
            kind,
            projected_len,
        })
    }

    fn change_count(&self) -> usize {
        match &self.kind {
            LedgerMutationKind::Mint(requests) => requests.len(),
            #[cfg(test)]
            LedgerMutationKind::Retire { .. } => 1,
        }
    }

    fn bind_candidates(
        self,
        candidates: Vec<DurableIdentityId>,
    ) -> Result<AdmittedLedger<'a>, IdentityMutationError> {
        #[cfg(not(test))]
        let LedgerMutationKind::Mint(requests) = self.kind;
        #[cfg(test)]
        let requests = match self.kind {
            LedgerMutationKind::Mint(requests) => requests,
            LedgerMutationKind::Retire { .. } => {
                return Err(IdentityMutationError::AdmittedStateMismatch);
            }
        };
        if candidates.len() != requests.len() {
            return Err(IdentityMutationError::CandidateCount {
                expected: requests.len(),
                actual: candidates.len(),
            });
        }
        let mut used: BTreeSet<DurableIdentityId> =
            self.captured.ledger.entries.values().copied().collect();
        used.extend(
            self.captured
                .ledger
                .tombstones
                .iter()
                .map(|tombstone| tombstone.id),
        );
        for candidate in &candidates {
            if !used.insert(*candidate) {
                return Err(IdentityMutationError::IdCollision(*candidate));
            }
        }

        let mut ledger = self.captured.ledger.clone();
        for (anchor, candidate) in requests.into_iter().zip(candidates) {
            if ledger.entries.insert(anchor, candidate).is_some() {
                return Err(IdentityMutationError::AdmittedStateMismatch);
            }
        }
        Ok(AdmittedLedger {
            captured: self.captured,
            ledger,
            projected_len: self.projected_len,
        })
    }

    #[cfg(test)]
    fn bind_retirement(self) -> Result<AdmittedLedger<'a>, IdentityMutationError> {
        let LedgerMutationKind::Retire {
            anchor,
            successor_high_water,
        } = self.kind
        else {
            return Err(IdentityMutationError::AdmittedStateMismatch);
        };
        let mut ledger = self.captured.ledger.clone();
        let Some(id) = ledger.entries.remove(&anchor) else {
            return Err(IdentityMutationError::AdmittedStateMismatch);
        };
        let insertion = match ledger.tombstone_index(anchor.kind, &anchor.path) {
            Ok(_) => return Err(IdentityMutationError::AdmittedStateMismatch),
            Err(insertion) => insertion,
        };
        ledger.high_water = successor_high_water;
        ledger.tombstones.insert(
            insertion,
            IdentityTombstone {
                anchor,
                id,
                high_water: successor_high_water,
            },
        );
        Ok(AdmittedLedger {
            captured: self.captured,
            ledger,
            projected_len: self.projected_len,
        })
    }
}

fn validate_requests(
    mut requests: Vec<IdentityAnchor>,
) -> Result<Vec<IdentityAnchor>, IdentityMutationError> {
    if let Some(index) = requests
        .iter()
        .position(|anchor| !valid_anchor_path(&anchor.path))
    {
        return Err(IdentityMutationError::InvalidAnchor(requests.remove(index)));
    }
    for pair in requests.windows(2) {
        if pair[0] == pair[1] {
            return Err(IdentityMutationError::DuplicateRequest(pair[0].clone()));
        }
    }
    Ok(requests)
}

fn valid_anchor_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH_BYTES
        && path.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

fn projected_canonical_len(
    ledger: &IdentityLedger,
    kind: &LedgerMutationKind,
) -> Result<usize, IdentityMutationError> {
    let mut total = IDS_HEADER
        .len()
        .checked_add(1)
        .and_then(|length| length.checked_add(IDS_NOTICE.len()))
        .and_then(|length| length.checked_add(1))
        .ok_or(IdentityMutationError::CanonicalLengthOverflow)?;
    for anchor in ledger.entries.keys() {
        #[cfg(test)]
        if matches!(
            kind,
            LedgerMutationKind::Retire {
                anchor: retired,
                ..
            } if retired == anchor
        ) {
            continue;
        }
        total = total
            .checked_add(live_row_len(anchor)?)
            .ok_or(IdentityMutationError::CanonicalLengthOverflow)?;
    }
    for tombstone in &ledger.tombstones {
        total = total
            .checked_add(retired_row_len(&tombstone.anchor, tombstone.high_water)?)
            .ok_or(IdentityMutationError::CanonicalLengthOverflow)?;
    }
    let high_water = match kind {
        LedgerMutationKind::Mint(requests) => {
            for anchor in requests {
                total = total
                    .checked_add(live_row_len(anchor)?)
                    .ok_or(IdentityMutationError::CanonicalLengthOverflow)?;
            }
            ledger.high_water
        }
        #[cfg(test)]
        LedgerMutationKind::Retire {
            anchor,
            successor_high_water,
        } => {
            total = total
                .checked_add(retired_row_len(anchor, *successor_high_water)?)
                .ok_or(IdentityMutationError::CanonicalLengthOverflow)?;
            *successor_high_water
        }
    };
    total
        .checked_add(canonical_tail_len(high_water)?)
        .ok_or(IdentityMutationError::CanonicalLengthOverflow)
}

fn live_row_len(anchor: &IdentityAnchor) -> Result<usize, IdentityMutationError> {
    3usize
        .checked_add(anchor.kind.keyword().len())
        .and_then(|length| length.checked_add(1))
        .and_then(|length| length.checked_add(anchor.path.len()))
        .and_then(|length| length.checked_add(1 + 32 + 1))
        .ok_or(IdentityMutationError::CanonicalLengthOverflow)
}

fn retired_row_len(
    anchor: &IdentityAnchor,
    high_water: u64,
) -> Result<usize, IdentityMutationError> {
    8usize
        .checked_add(anchor.kind.keyword().len())
        .and_then(|length| length.checked_add(1))
        .and_then(|length| length.checked_add(anchor.path.len()))
        .and_then(|length| length.checked_add(1 + 32 + 1))
        .and_then(|length| length.checked_add(decimal_len(high_water)))
        .and_then(|length| length.checked_add(1))
        .ok_or(IdentityMutationError::CanonicalLengthOverflow)
}

fn canonical_tail_len(high_water: u64) -> Result<usize, IdentityMutationError> {
    "high-water "
        .len()
        .checked_add(decimal_len(high_water))
        .and_then(|length| length.checked_add(1))
        .and_then(|length| length.checked_add(IDS_END.len()))
        .and_then(|length| length.checked_add(1))
        .ok_or(IdentityMutationError::CanonicalLengthOverflow)
}

fn decimal_len(mut value: u64) -> usize {
    let mut length = 1;
    while value >= 10 {
        value /= 10;
        length += 1;
    }
    length
}

/// The only canonical serializer and the only constructor of a publication plan.
struct AdmittedLedger<'a> {
    captured: &'a CapturedLedger,
    ledger: IdentityLedger,
    projected_len: usize,
}

impl AdmittedLedger<'_> {
    fn publication(self) -> Result<LedgerPublicationPlan, IdentityMutationError> {
        let mut next = String::with_capacity(self.projected_len);
        next.push_str(IDS_HEADER);
        next.push('\n');
        next.push_str(IDS_NOTICE);
        next.push('\n');
        for (anchor, id) in &self.ledger.entries {
            write_live_row(&mut next, anchor, *id)?;
        }
        for tombstone in &self.ledger.tombstones {
            write_retired_row(&mut next, tombstone)?;
        }
        let tail_start = next.len();
        let projected_tail = canonical_tail_len(self.ledger.high_water)?;
        next.push_str("high-water ");
        next.push_str(&self.ledger.high_water.to_string());
        next.push('\n');
        next.push_str(IDS_END);
        next.push('\n');
        let actual_tail = next.len() - tail_start;
        if actual_tail != projected_tail {
            return Err(IdentityMutationError::CanonicalLengthMismatch {
                projected: projected_tail,
                actual: actual_tail,
            });
        }
        if next.len() != self.projected_len {
            return Err(IdentityMutationError::CanonicalLengthMismatch {
                projected: self.projected_len,
                actual: next.len(),
            });
        }
        Ok(LedgerPublicationPlan {
            expected: self.captured.expected_artifact(),
            next: next.into_bytes(),
        })
    }
}

fn write_live_row(
    out: &mut String,
    anchor: &IdentityAnchor,
    id: DurableIdentityId,
) -> Result<(), IdentityMutationError> {
    let start = out.len();
    let projected = live_row_len(anchor)?;
    out.push_str("id ");
    out.push_str(anchor.kind.keyword());
    out.push(' ');
    out.push_str(&anchor.path);
    out.push(' ');
    append_id_hex(out, id);
    out.push('\n');
    let actual = out.len() - start;
    if actual != projected {
        return Err(IdentityMutationError::CanonicalLengthMismatch { projected, actual });
    }
    Ok(())
}

fn write_retired_row(
    out: &mut String,
    tombstone: &IdentityTombstone,
) -> Result<(), IdentityMutationError> {
    let start = out.len();
    let projected = retired_row_len(&tombstone.anchor, tombstone.high_water)?;
    out.push_str("retired ");
    out.push_str(tombstone.anchor.kind.keyword());
    out.push(' ');
    out.push_str(&tombstone.anchor.path);
    out.push(' ');
    append_id_hex(out, tombstone.id);
    out.push(' ');
    out.push_str(&tombstone.high_water.to_string());
    out.push('\n');
    let actual = out.len() - start;
    if actual != projected {
        return Err(IdentityMutationError::CanonicalLengthMismatch { projected, actual });
    }
    Ok(())
}

fn append_id_hex(out: &mut String, id: DurableIdentityId) {
    let mut bytes = [0; 32];
    id.write_canonical_hex(&mut bytes);
    out.push_str(std::str::from_utf8(&bytes).expect("canonical identity hex is ASCII"));
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
    if !valid_anchor_path(path) {
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

/// Why a `.marrow/ids` artifact was rejected whole.
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

/// A corrupt `.marrow/ids` artifact: the stable `project.ids_corrupt` code, a
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

#[cfg(test)]
mod tests {
    use super::{
        CapturedLedger, DurableIdentityId, IdentityAnchor, IdentityKind, IdentityLedger,
        IdentityMintFailure, IdentityMutationError, IdsErrorKind, LedgerExpectedArtifact,
        LedgerMutationPlan, LedgerPublicationPlan, reset_tombstone_lookup_comparisons,
        tombstone_lookup_comparisons,
    };

    fn id(byte: u8) -> DurableIdentityId {
        DurableIdentityId::from_bytes([byte; 16])
    }

    fn anchor(kind: IdentityKind, path: &str) -> IdentityAnchor {
        IdentityAnchor::new(kind, path)
    }

    fn id_number(value: usize) -> DurableIdentityId {
        DurableIdentityId::from_bytes(((value as u128) + 1).to_be_bytes())
    }

    fn plan_mints(
        captured: &CapturedLedger,
        mut mints: Vec<(IdentityAnchor, DurableIdentityId)>,
    ) -> Result<LedgerPublicationPlan, IdentityMutationError> {
        mints.sort_by(|a, b| a.0.cmp(&b.0));
        let mut anchors = mints.iter().map(|(anchor, _)| anchor.clone());
        let first = anchors.next().expect("test mint is structurally nonempty");
        let rest = anchors.collect();
        let candidates: Vec<DurableIdentityId> =
            mints.into_iter().map(|(_, candidate)| candidate).collect();
        captured
            .admit_identity_mints_with(first, rest, |_| {
                Ok::<_, std::convert::Infallible>(candidates)
            })
            .map_err(|failure| match failure {
                IdentityMintFailure::Supply(never) => match never {},
                IdentityMintFailure::Mutation(error) => error,
            })
    }

    fn next_bytes(plan: LedgerPublicationPlan) -> Vec<u8> {
        plan.visit(|view| view.next().to_vec())
    }

    #[derive(Debug, PartialEq, Eq)]
    enum ExpectedBytes {
        Absent,
        Present(Vec<u8>),
    }

    fn plan_parts(plan: LedgerPublicationPlan) -> (ExpectedBytes, Vec<u8>) {
        plan.visit(|view| {
            let expected = match view.expected() {
                LedgerExpectedArtifact::Absent => ExpectedBytes::Absent,
                LedgerExpectedArtifact::Present(bytes) => ExpectedBytes::Present(bytes.to_vec()),
            };
            (expected, view.next().to_vec())
        })
    }

    fn empty_artifact() -> Vec<u8> {
        b"marrow ids v0\nmachine-written by marrow; do not edit\nhigh-water 0\nend\n".to_vec()
    }

    fn live_artifact(rows: usize, path_bytes: usize, high_water: u64) -> Vec<u8> {
        assert!((5..=super::MAX_PATH_BYTES).contains(&path_bytes));
        let mut out = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
        for row in 0..rows {
            let prefix = format!("p{row:04}");
            let path = format!("{prefix}{}", "x".repeat(path_bytes - prefix.len()));
            out.push_str(&format!("id field {path} {}\n", id_number(row).to_hex()));
        }
        out.push_str(&format!("high-water {high_water}\nend\n"));
        let bytes = out.into_bytes();
        IdentityLedger::parse(&bytes).expect("generated live artifact parses");
        bytes
    }

    fn retired_artifact(rows: usize) -> Vec<u8> {
        let mut out = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
        for row in 0..rows {
            out.push_str(&format!(
                "retired field Old.f{row:04} {} {}\n",
                id_number(row).to_hex(),
                row + 1,
            ));
        }
        out.push_str(&format!("high-water {rows}\nend\n"));
        let bytes = out.into_bytes();
        let parsed = IdentityLedger::parse(&bytes).expect("generated retired artifact parses");
        assert_eq!(parsed.tombstones.len(), rows);
        bytes
    }

    /// A canonical live-only artifact with exactly `target` bytes.
    fn live_artifact_of_exact_len(target: usize) -> Vec<u8> {
        let fixed = empty_artifact().len();
        let row_bytes = target.checked_sub(fixed).expect("target includes framing");
        let minimum_path = 7usize;
        let minimum_row = 43 + minimum_path;
        let maximum_row = 43 + super::MAX_PATH_BYTES;
        let mut count = row_bytes.div_ceil(maximum_row);
        while count * minimum_row > row_bytes {
            count += 1;
        }
        assert!(count <= super::MAX_IDS_ROWS);
        let mut remaining_extra = row_bytes - count * minimum_row;
        let mut out = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
        for row in 0..count {
            let extra = remaining_extra.min(super::MAX_PATH_BYTES - minimum_path);
            remaining_extra -= extra;
            let prefix = format!("p{row:04}x");
            let path = format!(
                "{prefix}{}",
                "x".repeat(minimum_path + extra - prefix.len())
            );
            out.push_str(&format!("id field {path} {}\n", id_number(row).to_hex()));
        }
        assert_eq!(remaining_extra, 0);
        out.push_str("high-water 0\nend\n");
        let bytes = out.into_bytes();
        assert_eq!(bytes.len(), target);
        IdentityLedger::parse(&bytes).expect("exact-length artifact parses");
        bytes
    }

    fn live_artifact_with_rows_and_exact_len(
        rows: usize,
        target: usize,
        high_water: u64,
    ) -> Vec<u8> {
        let header = "marrow ids v0\nmachine-written by marrow; do not edit\n";
        let tail = format!("high-water {high_water}\nend\n");
        let minimum_path = "p0000".len();
        let minimum_row = 43 + minimum_path;
        let fixed = header.len() + tail.len();
        assert!(fixed + rows * minimum_row <= target);
        assert!(target <= fixed + rows * (43 + super::MAX_PATH_BYTES));
        let mut remaining_extra = target - fixed - rows * minimum_row;

        let mut out = String::from(header);
        for row in 0..rows {
            let base = format!("p{row:04}");
            let extra = remaining_extra.min(super::MAX_PATH_BYTES - base.len());
            remaining_extra -= extra;
            let path = format!("{base}{}", "x".repeat(extra));
            out.push_str(&format!("id field {path} {}\n", id_number(row).to_hex()));
        }
        assert_eq!(remaining_extra, 0);
        out.push_str(&tail);
        let bytes = out.into_bytes();
        assert_eq!(bytes.len(), target);
        let parsed = IdentityLedger::parse(&bytes).expect("exact row/byte artifact parses");
        assert_eq!(parsed.entries().count(), rows);
        bytes
    }

    /// Canonically sorted fresh field anchors whose serialized live rows total
    /// exactly `target` bytes.
    fn field_requests_of_exact_len(target: usize, prefix: &str) -> Vec<IdentityAnchor> {
        let minimum_path = prefix.len() + 4;
        assert!(minimum_path <= super::MAX_PATH_BYTES);
        let minimum_row = 43 + minimum_path;
        let maximum_row = 43 + super::MAX_PATH_BYTES;
        let mut count = target.div_ceil(maximum_row);
        while count * minimum_row > target {
            count += 1;
        }
        let mut remaining_extra = target - count * minimum_row;
        let mut anchors = Vec::with_capacity(count);
        for row in 0..count {
            let extra = remaining_extra.min(super::MAX_PATH_BYTES - minimum_path);
            remaining_extra -= extra;
            let base = format!("{prefix}{row:04}");
            anchors.push(IdentityAnchor::new(
                IdentityKind::Field,
                format!("{base}{}", "x".repeat(minimum_path + extra - base.len())),
            ));
        }
        assert_eq!(remaining_extra, 0);
        anchors
    }

    fn admit_requests(
        captured: &CapturedLedger,
        mut requests: Vec<IdentityAnchor>,
        candidates: Vec<DurableIdentityId>,
    ) -> Result<LedgerPublicationPlan, IdentityMutationError> {
        let first = requests.remove(0);
        captured
            .admit_identity_mints_with(first, requests, |_| {
                Ok::<_, std::convert::Infallible>(candidates)
            })
            .map_err(|failure| match failure {
                IdentityMintFailure::Supply(never) => match never {},
                IdentityMintFailure::Mutation(error) => error,
            })
    }

    fn counter_mints() -> Vec<(IdentityAnchor, DurableIdentityId)> {
        vec![
            (anchor(IdentityKind::Application, "."), id(0x0a)),
            (anchor(IdentityKind::Root, "counters"), id(0x0b)),
            (anchor(IdentityKind::Key, "counters.name"), id(0x0c)),
            (anchor(IdentityKind::Product, "Counter"), id(0x0d)),
            (anchor(IdentityKind::Field, "Counter.value"), id(0x0e)),
            (anchor(IdentityKind::Field, "Counter.label"), id(0x0f)),
        ]
    }

    fn counter_bytes() -> Vec<u8> {
        let captured = CapturedLedger::capture(None).expect("absent capture");
        next_bytes(plan_mints(&captured, counter_mints()).expect("mint the counter rows"))
    }

    fn counter_ledger() -> IdentityLedger {
        IdentityLedger::parse(&counter_bytes()).expect("counter artifact parses")
    }

    fn retired_counter_plan() -> LedgerPublicationPlan {
        let bytes = counter_bytes();
        let captured = CapturedLedger::capture(Some(&bytes)).expect("capture counter");
        LedgerMutationPlan::retire(&captured, anchor(IdentityKind::Field, "Counter.label"))
            .expect("retirement admits")
            .bind_retirement()
            .expect("retirement binds")
            .publication()
            .expect("retirement serializes")
    }

    #[test]
    fn serialization_is_canonical_and_round_trips() {
        let bytes = counter_bytes();
        let reparsed = IdentityLedger::parse(&bytes).expect("reparse");
        assert_eq!(reparsed, counter_ledger());
        assert_eq!(
            String::from_utf8(bytes).expect("canonical UTF-8"),
            "marrow ids v0\n\
             machine-written by marrow; do not edit\n\
             id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
             id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
             id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
             id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
             id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
             id key counters.name 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
             high-water 0\n\
             end\n",
        );
    }

    #[test]
    fn public_and_artifact_id_hex_share_the_lowercase_known_answer() {
        let identity = DurableIdentityId::from_bytes([
            0x00, 0x01, 0x0f, 0x10, 0x2a, 0x7f, 0x80, 0xab, 0xcd, 0xef, 0x55, 0xaa, 0x09, 0x90,
            0xfe, 0xff,
        ]);
        let expected = "00010f102a7f80abcdef55aa0990feff";
        assert_eq!(identity.to_hex(), expected);

        let captured = CapturedLedger::capture(None).expect("absent capture");
        let bytes = next_bytes(
            plan_mints(
                &captured,
                vec![(anchor(IdentityKind::Application, "."), identity)],
            )
            .expect("known-answer mint"),
        );
        let text = std::str::from_utf8(&bytes).expect("artifact UTF-8");
        assert!(text.contains(&format!("id application . {expected}\n")));
        IdentityLedger::parse(&bytes).expect("known-answer successor parses");
    }

    #[test]
    fn parse_accepts_any_row_order_but_write_is_sorted() {
        let canonical = counter_bytes();
        let text = String::from_utf8(canonical.clone()).unwrap();
        let mut lines: Vec<&str> = text.lines().collect();
        // Reverse the row block (between the two header lines and the
        // high-water/end tail) to simulate a merge that interleaved rows.
        lines[2..8].reverse();
        let shuffled = format!("{}\n", lines.join("\n"));
        let reparsed = IdentityLedger::parse(shuffled.as_bytes()).expect("order-blind parse");
        assert_eq!(reparsed, counter_ledger());

        let new = vec![(anchor(IdentityKind::Field, "Counter.note"), id(0x20))];
        let canonical_capture =
            CapturedLedger::capture(Some(&canonical)).expect("capture canonical artifact");
        let shuffled_capture =
            CapturedLedger::capture(Some(shuffled.as_bytes())).expect("capture shuffled artifact");
        assert_eq!(
            next_bytes(plan_mints(&canonical_capture, new.clone()).expect("canonical successor")),
            next_bytes(plan_mints(&shuffled_capture, new).expect("shuffled successor")),
            "the admitted successor is canonical regardless of valid captured row order",
        );
    }

    #[test]
    fn live_row_suffixes_are_malformed_before_insertion() {
        let canonical = counter_bytes();
        let text = String::from_utf8(canonical.clone()).unwrap();
        let target_row = format!("id field Counter.label {}\n", id(0x0f).to_hex());
        let target_without_newline = target_row
            .strip_suffix('\n')
            .expect("the target row has its artifact newline");

        for suffix in [" ", "  ", " extra", " extra more", " extra "] {
            let replacement = format!("{target_without_newline}{suffix}\n");
            let malformed = text.replacen(&target_row, &replacement, 1);
            assert_eq!(
                IdentityLedger::parse(malformed.as_bytes())
                    .unwrap_err()
                    .kind,
                IdsErrorKind::Malformed,
                "suffix {suffix:?} must reject before the row is inserted"
            );
        }

        let duplicate_with_suffix = text.replacen(
            &target_row,
            &format!("id application . {} extra\n", id(0x0a).to_hex()),
            1,
        );
        assert_eq!(
            IdentityLedger::parse(duplicate_with_suffix.as_bytes())
                .unwrap_err()
                .kind,
            IdsErrorKind::Malformed,
            "row grammar wins before duplicate-id or duplicate-anchor classification"
        );

        let suffix_and_conflict = text
            .replacen(&target_row, &format!("{target_without_newline} extra\n"), 1)
            .replacen("high-water ", "<<<<<<< ours\nhigh-water ", 1);
        assert_eq!(
            IdentityLedger::parse(suffix_and_conflict.as_bytes())
                .unwrap_err()
                .kind,
            IdsErrorKind::ConflictMarker,
            "the artifact-wide conflict scan retains its earlier precedence"
        );

        let reparsed = IdentityLedger::parse(&canonical).expect("canonical ledger remains valid");
        assert_eq!(reparsed, counter_ledger(), "valid semantics remain exact");
    }

    #[test]
    fn a_conflicting_double_mint_is_rejected_as_duplicate_anchor_or_id() {
        let base = String::from_utf8(counter_bytes()).unwrap();
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
        let base = String::from_utf8(counter_bytes()).unwrap();
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
        let retired_bytes = next_bytes(retired_counter_plan());
        let retired = IdentityLedger::parse(&retired_bytes).expect("retired artifact parses");
        assert_eq!(retired.high_water(), 1);
        assert!(
            retired
                .lookup(IdentityKind::Field, "Counter.label")
                .is_none()
        );
        assert!(retired.is_retired(IdentityKind::Field, "Counter.label"));

        // Re-adding at the retired anchor is refused.
        let captured =
            CapturedLedger::capture(Some(&retired_bytes)).expect("capture retired artifact");
        let re_add = plan_mints(
            &captured,
            vec![(anchor(IdentityKind::Field, "Counter.label"), id(0x20))],
        );
        assert!(matches!(
            re_add,
            Err(IdentityMutationError::AnchorRetired(_))
        ));

        // The retired id can never be drawn again either.
        let reuse = plan_mints(
            &captured,
            vec![(anchor(IdentityKind::Field, "Counter.note"), id(0x0f))],
        );
        assert!(matches!(reuse, Err(IdentityMutationError::IdCollision(_))));

        // The tombstone round-trips through the artifact.
        let reparsed = IdentityLedger::parse(&retired_bytes).expect("reparse");
        assert!(reparsed.is_retired(IdentityKind::Field, "Counter.label"));
        assert_eq!(reparsed, retired);
    }

    #[test]
    fn a_retired_id_or_anchor_reissued_live_rejects_at_parse() {
        let base = String::from_utf8(next_bytes(retired_counter_plan())).unwrap();

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
        let before = counter_bytes();
        let captured = CapturedLedger::capture(Some(&before)).expect("capture counter");
        // A colliding draw (the no-retry contract): the operation fails and the
        // original ledger — the bytes the artifact holds — is untouched.
        let result = plan_mints(
            &captured,
            vec![(anchor(IdentityKind::Field, "Counter.note"), id(0x0a))],
        );
        assert!(matches!(result, Err(IdentityMutationError::IdCollision(_))));
        assert_eq!(
            captured.present_ledger(),
            Some(&IdentityLedger::parse(&before).expect("original parses")),
        );
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
    fn mutation_refuses_a_path_past_512_bytes_before_building_a_successor() {
        let overlong = "x".repeat(super::MAX_PATH_BYTES + 1);
        let invalid = anchor(IdentityKind::Field, &overlong);
        let path_allocation = invalid.path.as_ptr();
        let captured = CapturedLedger::capture(None).expect("absent capture");
        let calls = std::cell::Cell::new(0);
        let result = captured.admit_identity_mints_with(invalid, Vec::new(), |_| {
            calls.set(calls.get() + 1);
            Ok::<_, std::convert::Infallible>(vec![id(0x01)])
        });
        let Err(IdentityMintFailure::Mutation(IdentityMutationError::InvalidAnchor(offender))) =
            result
        else {
            panic!("expected invalid-anchor refusal");
        };
        assert_eq!(
            offender.path.as_ptr(),
            path_allocation,
            "the canonical owned offender moves into the error without cloning its unbounded path",
        );
        assert_eq!(calls.get(), 0, "grammar refusal precedes candidate supply");
    }

    #[test]
    fn near_cap_retired_lookup_is_logarithmic_and_precedes_candidate_supply() {
        let retired_rows = super::MAX_IDS_ROWS / 2;
        let bytes = retired_artifact(retired_rows);
        let captured =
            CapturedLedger::capture(Some(&bytes)).expect("capture retired near-cap base");
        let request_count = super::MAX_IDS_ROWS - retired_rows + 1;
        let mut requests = (0..request_count)
            .map(|row| IdentityAnchor::new(IdentityKind::Field, format!("New.f{row:04}")));
        let first = requests.next().expect("nonempty near-cap request");
        let calls = std::cell::Cell::new(0);
        reset_tombstone_lookup_comparisons();
        let result = captured.admit_identity_mints_with(first, requests.collect(), |_| {
            calls.set(calls.get() + 1);
            Ok::<_, std::convert::Infallible>(Vec::new())
        });
        assert!(matches!(
            result,
            Err(IdentityMintFailure::Mutation(
                IdentityMutationError::RowLimit {
                    projected: 8193,
                    limit: 8192,
                }
            ))
        ));
        assert_eq!(calls.get(), 0, "row refusal precedes candidate supply");
        let comparisons = tombstone_lookup_comparisons();
        assert!(
            comparisons <= request_count * 16,
            "{request_count} requests used {comparisons} retired-anchor comparisons",
        );
    }

    #[test]
    fn mutation_refuses_a_successor_past_8192_rows() {
        let mut requests = (0..=super::MAX_IDS_ROWS)
            .map(|row| IdentityAnchor::new(IdentityKind::Field, format!("R.f{row:04}")));
        let first = requests.next().expect("8,193 requests");
        let captured = CapturedLedger::capture(None).expect("absent capture");
        let calls = std::cell::Cell::new(0);
        let result = captured.admit_identity_mints_with(first, requests.collect(), |_| {
            calls.set(calls.get() + 1);
            Ok::<_, std::convert::Infallible>(Vec::new())
        });
        assert!(matches!(
            result,
            Err(IdentityMintFailure::Mutation(
                IdentityMutationError::RowLimit {
                    projected: 8193,
                    limit: 8192
                }
            ))
        ));
        assert_eq!(calls.get(), 0, "row refusal precedes candidate supply");
    }

    #[test]
    fn anchor_and_row_boundaries_admit_n_and_refuse_n_plus_one() {
        let captured = CapturedLedger::capture(None).expect("absent capture");
        let at_path_limit = anchor(IdentityKind::Field, &"x".repeat(super::MAX_PATH_BYTES));
        let calls = std::cell::Cell::new(0);
        let plan = captured
            .admit_identity_mints_with(at_path_limit.clone(), Vec::new(), |count| {
                calls.set(calls.get() + 1);
                assert_eq!(count, 1);
                Ok::<_, std::convert::Infallible>(vec![id(0x01)])
            })
            .expect("a 512-byte path admits");
        assert_eq!(calls.get(), 1);
        let parsed = IdentityLedger::parse(&next_bytes(plan)).expect("successor parses");
        assert_eq!(
            parsed.lookup(at_path_limit.kind, &at_path_limit.path),
            Some(id(0x01)),
        );

        let requests: Vec<IdentityAnchor> = (0..super::MAX_IDS_ROWS)
            .map(|row| IdentityAnchor::new(IdentityKind::Field, format!("R.f{row:04}")))
            .collect();
        let candidates: Vec<DurableIdentityId> = (0..super::MAX_IDS_ROWS).map(id_number).collect();
        let plan = admit_requests(&captured, requests, candidates).expect("8,192 rows admit");
        assert_eq!(
            IdentityLedger::parse(&next_bytes(plan))
                .expect("maximum-row successor parses")
                .entries()
                .count(),
            super::MAX_IDS_ROWS,
        );
    }

    #[test]
    fn planning_precedence_is_grammar_duplicate_state_rows_then_bytes() {
        let counter = counter_bytes();
        let captured = CapturedLedger::capture(Some(&counter)).expect("capture counter");
        let calls = std::cell::Cell::new(0);
        let invalid = anchor(IdentityKind::Field, &"z".repeat(super::MAX_PATH_BYTES + 1));
        let result =
            captured.admit_identity_mints_with(invalid.clone(), vec![invalid.clone()], |_| {
                calls.set(calls.get() + 1);
                Ok::<_, std::convert::Infallible>(Vec::new())
            });
        assert!(matches!(
            result,
            Err(IdentityMintFailure::Mutation(
                IdentityMutationError::InvalidAnchor(anchor)
            )) if anchor == invalid
        ));
        assert_eq!(calls.get(), 0);

        let active = anchor(IdentityKind::Field, "Counter.label");
        let result =
            captured.admit_identity_mints_with(active.clone(), vec![active.clone()], |_| {
                calls.set(calls.get() + 1);
                Ok::<_, std::convert::Infallible>(Vec::new())
            });
        assert!(matches!(
            result,
            Err(IdentityMintFailure::Mutation(
                IdentityMutationError::DuplicateRequest(anchor)
            )) if anchor == active
        ));
        assert_eq!(calls.get(), 0);

        let retired_bytes = next_bytes(retired_counter_plan());
        let retired_capture =
            CapturedLedger::capture(Some(&retired_bytes)).expect("capture retired counter");
        let retired = anchor(IdentityKind::Field, "Counter.label");
        let result = retired_capture.admit_identity_mints_with(retired.clone(), Vec::new(), |_| {
            calls.set(calls.get() + 1);
            Ok::<_, std::convert::Infallible>(vec![id(0xff)])
        });
        assert!(matches!(
            result,
            Err(IdentityMintFailure::Mutation(
                IdentityMutationError::AnchorRetired(anchor)
            )) if anchor == retired
        ));
        assert_eq!(calls.get(), 0);

        let full = live_artifact(super::MAX_IDS_ROWS, 12, 0);
        let full_capture = CapturedLedger::capture(Some(&full)).expect("capture full ledger");
        let first_active = full_capture
            .ledger
            .entries
            .keys()
            .next()
            .expect("full ledger has rows")
            .clone();
        let result =
            full_capture.admit_identity_mints_with(first_active.clone(), Vec::new(), |_| {
                calls.set(calls.get() + 1);
                Ok::<_, std::convert::Infallible>(vec![id(0xff)])
            });
        assert!(matches!(
            result,
            Err(IdentityMintFailure::Mutation(
                IdentityMutationError::AnchorActive(anchor)
            )) if anchor == first_active
        ));
        assert_eq!(calls.get(), 0);

        let full_near_byte_limit =
            live_artifact_with_rows_and_exact_len(super::MAX_IDS_ROWS, super::MAX_IDS_BYTES - 1, 0);
        let full_near_byte_capture = CapturedLedger::capture(Some(&full_near_byte_limit))
            .expect("capture simultaneous row/byte base");
        let result = full_near_byte_capture.admit_identity_mints_with(
            anchor(IdentityKind::Field, "fresh"),
            Vec::new(),
            |_| {
                calls.set(calls.get() + 1);
                Ok::<_, std::convert::Infallible>(vec![id(0xfd)])
            },
        );
        assert!(matches!(
            result,
            Err(IdentityMintFailure::Mutation(
                IdentityMutationError::RowLimit {
                    projected: 8193,
                    limit: 8192
                }
            ))
        ));
        assert_eq!(calls.get(), 0, "row admission precedes byte admission");

        let exact_max = live_artifact_of_exact_len(super::MAX_IDS_BYTES);
        let exact_capture =
            CapturedLedger::capture(Some(&exact_max)).expect("capture exact-byte ledger");
        let result = exact_capture.admit_identity_mints_with(
            anchor(IdentityKind::Field, "fresh"),
            Vec::new(),
            |_| {
                calls.set(calls.get() + 1);
                Ok::<_, std::convert::Infallible>(vec![id(0xfe)])
            },
        );
        assert!(matches!(
            result,
            Err(IdentityMintFailure::Mutation(
                IdentityMutationError::ByteLimit { .. }
            ))
        ));
        assert_eq!(calls.get(), 0, "byte admission precedes candidate supply");
    }

    #[test]
    fn canonical_byte_projection_ignores_crlf_and_missing_final_lf_raw_lengths() {
        let canonical_base = live_artifact(100, super::MAX_PATH_BYTES, 0);
        let exact_delta = super::MAX_IDS_BYTES - canonical_base.len();
        let exact_requests = field_requests_of_exact_len(exact_delta, "n");
        assert!(
            100 + exact_requests.len() <= super::MAX_IDS_ROWS,
            "the exact-byte fixture stays below the row ceiling",
        );
        let crlf = String::from_utf8(canonical_base.clone())
            .expect("base UTF-8")
            .replace('\n', "\r\n")
            .into_bytes();
        assert!(crlf.len() > canonical_base.len());
        let captured = CapturedLedger::capture(Some(&crlf)).expect("CRLF capture");
        let candidates: Vec<DurableIdentityId> = (0..exact_requests.len())
            .map(|index| DurableIdentityId::from_bytes(((10_000 + index) as u128).to_be_bytes()))
            .collect();
        let plan =
            admit_requests(&captured, exact_requests.clone(), candidates).expect("1 MiB admits");
        let (expected, next) = plan_parts(plan);
        assert_eq!(expected, ExpectedBytes::Present(crlf));
        assert_eq!(next.len(), super::MAX_IDS_BYTES);
        IdentityLedger::parse(&next).expect("exact 1 MiB successor parses");

        let mut over_requests = exact_requests;
        let extended = over_requests
            .iter_mut()
            .find(|anchor| anchor.path.len() < super::MAX_PATH_BYTES)
            .expect("one request has path headroom");
        extended.path.push('y');
        let no_final_lf = canonical_base
            .strip_suffix(b"\n")
            .expect("canonical artifact ends in LF")
            .to_vec();
        assert!(no_final_lf.len() < canonical_base.len());
        let captured = CapturedLedger::capture(Some(&no_final_lf)).expect("no-final-LF capture");
        let calls = std::cell::Cell::new(0);
        let first = over_requests.remove(0);
        let result = captured.admit_identity_mints_with(first, over_requests, |_| {
            calls.set(calls.get() + 1);
            Ok::<_, std::convert::Infallible>(Vec::new())
        });
        assert!(matches!(
            result,
            Err(IdentityMintFailure::Mutation(
                IdentityMutationError::ByteLimit {
                    projected,
                    limit
                }
            )) if projected == super::MAX_IDS_BYTES + 1 && limit == super::MAX_IDS_BYTES
        ));
        assert_eq!(
            calls.get(),
            0,
            "1 MiB+1 canonical refusal precedes candidate supply",
        );
    }

    #[test]
    fn candidate_count_precedes_live_collision_and_supply_failure_is_preserved() {
        let counter = counter_bytes();
        let captured = CapturedLedger::capture(Some(&counter)).expect("capture counter");
        let requests = vec![
            anchor(IdentityKind::Field, "Counter.a"),
            anchor(IdentityKind::Field, "Counter.b"),
        ];
        for candidates in [vec![id(0x0a)], vec![id(0x0a), id(0x20), id(0x21)]] {
            let error = admit_requests(&captured, requests.clone(), candidates)
                .expect_err("wrong count refuses before collision");
            assert!(matches!(
                error,
                IdentityMutationError::CandidateCount {
                    expected: 2,
                    actual: 1 | 3
                }
            ));
        }

        let failure = captured.admit_identity_mints_with(
            anchor(IdentityKind::Field, "Counter.c"),
            Vec::new(),
            |_| Err::<Vec<DurableIdentityId>, _>("entropy unavailable"),
        );
        assert!(matches!(
            failure,
            Err(IdentityMintFailure::Supply("entropy unavailable"))
        ));
    }

    #[test]
    fn live_tombstone_and_intra_draw_collisions_are_distinct_admission_failures() {
        let counter = counter_bytes();
        let captured = CapturedLedger::capture(Some(&counter)).expect("capture counter");
        let live = admit_requests(
            &captured,
            vec![anchor(IdentityKind::Field, "Counter.new")],
            vec![id(0x0a)],
        );
        assert!(matches!(
            live,
            Err(IdentityMutationError::IdCollision(candidate)) if candidate == id(0x0a)
        ));

        let retired_bytes = next_bytes(retired_counter_plan());
        let retired =
            CapturedLedger::capture(Some(&retired_bytes)).expect("capture retired counter");
        let tombstone = admit_requests(
            &retired,
            vec![anchor(IdentityKind::Field, "Counter.new")],
            vec![id(0x0f)],
        );
        assert!(matches!(
            tombstone,
            Err(IdentityMutationError::IdCollision(candidate)) if candidate == id(0x0f)
        ));

        let intra = admit_requests(
            &captured,
            vec![
                anchor(IdentityKind::Field, "Counter.a"),
                anchor(IdentityKind::Field, "Counter.b"),
            ],
            vec![id(0x20), id(0x20)],
        );
        assert!(matches!(
            intra,
            Err(IdentityMutationError::IdCollision(candidate)) if candidate == id(0x20)
        ));
    }

    #[test]
    fn retirement_decimal_growth_and_advanceability_match_parser_law() {
        let empty = CapturedLedger::capture(None).expect("absent capture");
        let invalid = anchor(IdentityKind::Field, &"z".repeat(super::MAX_PATH_BYTES + 1));
        assert!(matches!(
            LedgerMutationPlan::retire(&empty, invalid),
            Err(IdentityMutationError::InvalidAnchor(_))
        ));
        assert!(matches!(
            LedgerMutationPlan::retire(&empty, anchor(IdentityKind::Field, "missing")),
            Err(IdentityMutationError::AnchorNotActive(_))
        ));

        for high_water in [9, 99] {
            let base = live_artifact(1, 12, high_water);
            let captured = CapturedLedger::capture(Some(&base)).expect("capture retirement base");
            let active = captured
                .ledger
                .entries
                .keys()
                .next()
                .expect("one live row")
                .clone();
            let next = next_bytes(
                LedgerMutationPlan::retire(&captured, active)
                    .expect("retirement admits")
                    .bind_retirement()
                    .expect("retirement binds")
                    .publication()
                    .expect("retirement serializes"),
            );
            let successor = high_water + 1;
            let text = String::from_utf8(next).expect("retirement UTF-8");
            assert!(text.contains(&format!(" {successor}\nhigh-water {successor}\n")));
        }

        let base = live_artifact(2, 12, u64::MAX - 3);
        let captured = CapturedLedger::capture(Some(&base)).expect("capture high-water base");
        let first = captured
            .ledger
            .entries
            .keys()
            .next()
            .expect("first active")
            .clone();
        let admitted = next_bytes(
            LedgerMutationPlan::retire(&captured, first)
                .expect("MAX-3 to MAX-2 admits")
                .bind_retirement()
                .expect("retirement binds")
                .publication()
                .expect("retirement serializes"),
        );
        let admitted_capture =
            CapturedLedger::capture(Some(&admitted)).expect("MAX-2 successor parses");
        assert_eq!(admitted_capture.ledger.high_water(), u64::MAX - 2);
        let second = admitted_capture
            .ledger
            .entries
            .keys()
            .next()
            .expect("second active")
            .clone();
        assert!(matches!(
            LedgerMutationPlan::retire(&admitted_capture, second),
            Err(IdentityMutationError::RetirementHighWater)
        ));

        let byte_full =
            live_artifact_with_rows_and_exact_len(2_000, super::MAX_IDS_BYTES - 1, u64::MAX - 2);
        let byte_full_capture =
            CapturedLedger::capture(Some(&byte_full)).expect("capture high-water/byte base");
        let active = byte_full_capture
            .ledger
            .entries
            .keys()
            .next()
            .expect("active row")
            .clone();
        assert!(matches!(
            LedgerMutationPlan::retire(&byte_full_capture, active),
            Err(IdentityMutationError::RetirementHighWater)
        ));
    }

    #[test]
    fn absent_present_empty_and_shuffled_witnesses_remain_exact() {
        let absent = CapturedLedger::capture(None).expect("absent capture");
        let empty_bytes = empty_artifact();
        let present_empty =
            CapturedLedger::capture(Some(&empty_bytes)).expect("present-empty capture");
        assert_ne!(absent, present_empty);

        let mint = vec![(anchor(IdentityKind::Application, "."), id(0x01))];
        let (absent_expected, absent_next) =
            plan_parts(plan_mints(&absent, mint.clone()).expect("absent mint"));
        let (present_expected, present_next) =
            plan_parts(plan_mints(&present_empty, mint).expect("present-empty mint"));
        assert_eq!(absent_expected, ExpectedBytes::Absent);
        assert_eq!(
            present_expected,
            ExpectedBytes::Present(empty_bytes.clone())
        );
        assert_eq!(absent_next, present_next);

        let canonical = counter_bytes();
        let mut lines: Vec<&str> = std::str::from_utf8(&canonical)
            .expect("canonical UTF-8")
            .lines()
            .collect();
        lines[2..8].reverse();
        let shuffled = format!("{}\n", lines.join("\n")).into_bytes();
        let canonical_capture =
            CapturedLedger::capture(Some(&canonical)).expect("canonical capture");
        let shuffled_capture = CapturedLedger::capture(Some(&shuffled)).expect("shuffled capture");
        assert_eq!(canonical_capture, shuffled_capture);
        let addition = vec![(anchor(IdentityKind::Field, "Counter.note"), id(0x20))];
        let (canonical_expected, canonical_next) = plan_parts(
            plan_mints(&canonical_capture, addition.clone()).expect("canonical successor"),
        );
        let (shuffled_expected, shuffled_next) =
            plan_parts(plan_mints(&shuffled_capture, addition).expect("shuffled successor"));
        assert_eq!(canonical_expected, ExpectedBytes::Present(canonical));
        assert_eq!(shuffled_expected, ExpectedBytes::Present(shuffled));
        assert_eq!(canonical_next, shuffled_next);

        for debug in [
            format!("{canonical_capture:?}"),
            format!("{shuffled_capture:?}"),
        ] {
            assert!(!debug.contains("marrow ids v0"));
            assert!(!debug.contains("\\r\\n"));
        }
    }

    #[test]
    fn reversed_valid_tombstones_are_semantically_equal_but_keep_exact_witnesses() {
        let header = "marrow ids v0\nmachine-written by marrow; do not edit\n";
        let first = format!("retired field Old.a {} 1\n", id(0x11).to_hex(),);
        let second = format!("retired field Old.b {} 2\n", id(0x22).to_hex(),);
        let tail = "high-water 2\nend\n";
        let canonical = format!("{header}{first}{second}{tail}").into_bytes();
        let reversed = format!("{header}{second}{first}{tail}").into_bytes();
        let canonical_capture =
            CapturedLedger::capture(Some(&canonical)).expect("canonical tombstones capture");
        let reversed_capture =
            CapturedLedger::capture(Some(&reversed)).expect("reversed tombstones capture");

        let addition = vec![(anchor(IdentityKind::Field, "Fresh.value"), id(0x33))];
        let (canonical_expected, canonical_next) = plan_parts(
            plan_mints(&canonical_capture, addition.clone()).expect("canonical successor"),
        );
        let (reversed_expected, reversed_next) =
            plan_parts(plan_mints(&reversed_capture, addition).expect("reversed successor"));

        assert_eq!(
            canonical_capture, reversed_capture,
            "valid row order is not part of captured ledger semantics",
        );
        assert_eq!(canonical_expected, ExpectedBytes::Present(canonical));
        assert_eq!(reversed_expected, ExpectedBytes::Present(reversed));
        assert_ne!(canonical_expected, reversed_expected);
        assert_eq!(canonical_next, reversed_next);
        IdentityLedger::parse(&canonical_next).expect("canonical successor parses");
    }

    #[test]
    fn mixed_live_tombstone_mint_and_retire_parse_back_with_exact_lengths() {
        let retired_bytes = next_bytes(retired_counter_plan());
        let captured = CapturedLedger::capture(Some(&retired_bytes)).expect("capture mixed ledger");
        let minted = next_bytes(
            plan_mints(
                &captured,
                vec![(anchor(IdentityKind::Field, "Counter.note"), id(0x20))],
            )
            .expect("mint over mixed base"),
        );
        let minted_ledger = IdentityLedger::parse(&minted).expect("mint successor parses");
        assert!(minted_ledger.is_retired(IdentityKind::Field, "Counter.label"));
        assert_eq!(
            minted_ledger.lookup(IdentityKind::Field, "Counter.note"),
            Some(id(0x20)),
        );

        let active = captured
            .ledger
            .entries
            .keys()
            .find(|anchor| anchor.path == "Counter.value")
            .expect("active field")
            .clone();
        let retired = next_bytes(
            LedgerMutationPlan::retire(&captured, active)
                .expect("second retirement admits")
                .bind_retirement()
                .expect("second retirement binds")
                .publication()
                .expect("second retirement serializes"),
        );
        let retired_ledger = IdentityLedger::parse(&retired).expect("retire successor parses");
        assert!(retired_ledger.is_retired(IdentityKind::Field, "Counter.value"));
    }

    #[test]
    fn checked_length_disagreement_is_typed_not_panicking() {
        let captured = CapturedLedger::capture(None).expect("absent capture");
        let plan = LedgerMutationPlan::mint(
            &captured,
            anchor(IdentityKind::Application, "."),
            Vec::new(),
        )
        .expect("plan admits");
        let mut admitted = plan
            .bind_candidates(vec![id(0x01)])
            .expect("candidate binds");
        admitted.projected_len += 1;
        assert!(matches!(
            admitted.publication(),
            Err(IdentityMutationError::CanonicalLengthMismatch { .. })
        ));
    }

    /// The row cap holds its chosen value (8192, tracking `marrow-image`'s member-tree
    /// total) so the full 4096 record-field width is reachable for a single wide resource.
    /// The `MAX_IDS_ROWS >= 4096 + overhead` decoupling invariant is enforced at compile
    /// time by the `const _` block. An artifact one row past the cap rejects as `Bound`.
    #[test]
    fn row_cap_holds_its_widened_value_and_rejects_one_past_it() {
        assert_eq!(super::MAX_IDS_ROWS, 8192, "durable-identity row cap");
        let mut out = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
        for row in 0..=super::MAX_IDS_ROWS {
            out.push_str(&format!("id field R.f{row} {:032x}\n", row + 1));
        }
        out.push_str("high-water 0\nend\n");
        assert_eq!(
            IdentityLedger::parse(out.as_bytes()).unwrap_err().kind,
            IdsErrorKind::Bound,
            "one row past the cap rejects",
        );
    }

    #[test]
    fn the_kind_tag_space_is_frozen_and_reserved() {
        // The frozen kind tag space: application/product/field/root/key, sum/member
        // (durable enum identity), group, and index (managed-index identity).
        let tags: Vec<u8> = IdentityKind::ALL.iter().map(|kind| kind.tag()).collect();
        assert_eq!(tags, vec![0, 1, 2, 3, 4, 5, 6, 7, 8]);
        for kind in IdentityKind::ALL {
            assert_eq!(
                IdentityKind::from_keyword(kind.keyword()),
                Some(*kind),
                "keyword round-trips"
            );
        }
    }
}
