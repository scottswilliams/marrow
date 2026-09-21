//! The durable-identity ledger and its committed artifact, `.marrow/ids`.
//!
//! The ledger is the source-side authority for entropy-minted durable identity:
//! each row binds a `(kind, path)` anchor to a random 128-bit id, and the
//! append-only tombstone list plus a monotonic retirement high-water keep a
//! retired id (and its retired anchor) from ever being reused — even across
//! store loss, because the artifact is committed with the source. No mutation
//! here writes a tombstone; retirement is a read-side grammar that publication
//! carries forward unchanged. Entropy ids
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

// The joined path and its two parts are one spelling, checked at compile time so a
// rename of either half cannot leave a second live ledger location behind.
const _: () = {
    let (joined, dir, entry) = (
        IDS_FILE.as_bytes(),
        META_DIR.as_bytes(),
        IDS_ENTRY.as_bytes(),
    );
    assert!(
        joined.len() == dir.len() + 1 + entry.len(),
        "the ledger path must be its directory and entry spellings joined"
    );
    let mut at = 0;
    while at < joined.len() {
        let expected = if at < dir.len() {
            dir[at]
        } else if at == dir.len() {
            b'/'
        } else {
            entry[at - dir.len() - 1]
        };
        assert!(
            joined[at] == expected,
            "the ledger path must be its directory and entry spellings joined"
        );
        at += 1;
    }
};

/// A ledger path no project may use. Nothing reads it: capture refuses a file
/// here with a one-line steer to the ledger's home, so a project never has two
/// live ledger locations.
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
/// The row cap tracks the durable member-tree scale so the record-field width
/// guard stays reachable for a single wide resource: a resource of
/// `marrow-image`'s `MAX_RECORD_FIELDS` (4096) declared fields anchors one `Field`
/// row per field plus a small fixed overhead, ~4100 rows. The cap matches
/// `marrow-image`'s `MAX_DURABLE_MEMBERS` (8192). At this width the binder is the
/// field-count guard, not `MAX_IDS_BYTES` (~4100 rows ≈ 250 KB « 1 MiB). A
/// multi-root project carrying several wide resources can still exceed the row
/// cap; sizing for that is a separate widen.
pub const MAX_IDS_BYTES: usize = 1 << 20;
pub const MAX_IDS_ROWS: usize = 8192;
/// The longest anchor path a row may carry.
const MAX_PATH_BYTES: usize = 512;

// This crate does not depend on `marrow-image`, so its `MAX_RECORD_FIELDS` (4096)
// is restated here as a cross-crate invariant; `marrow-image::bounds` carries the
// image-side half.
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

/// The canonical 32-digit lowercase-hex artifact spelling.
impl fmt::Display for DurableIdentityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut hex = [0; 32];
        for (index, byte) in self.0.into_iter().enumerate() {
            hex[index * 2] = ID_HEX_DIGITS[usize::from(byte >> 4)];
            hex[index * 2 + 1] = ID_HEX_DIGITS[usize::from(byte & 0x0f)];
        }
        f.write_str(std::str::from_utf8(&hex).expect("canonical identity hex is ASCII"))
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

fn compare_tombstone_anchor(
    tombstone: &IdentityTombstone,
    kind: IdentityKind,
    path: &str,
) -> Ordering {
    tombstone
        .anchor
        .kind
        .cmp(&kind)
        .then_with(|| tombstone.anchor.path.as_str().cmp(path))
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
                            format!("id `{id}` appears twice"),
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
                    ledger
                        .tombstones
                        .push(parse_retired_row(&mut fields, line, &mut ids)?);
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
        check_retirements(&ledger)?;
        // Only the admitted semantic state is normalized; the parser's input-order
        // rejection precedence above must survive unchanged.
        ledger.tombstones.sort_by(canonical_tombstone_order);
        Ok(ledger)
    }
}

/// Cross-row invariants: a retired anchor or id must not also be live,
/// and no tombstone can record a retirement past the ledger high-water.
fn check_retirements(ledger: &IdentityLedger) -> Result<(), IdsError> {
    let mut reserved_anchors: BTreeSet<&IdentityAnchor> = BTreeSet::new();
    for tombstone in &ledger.tombstones {
        if tombstone.high_water > ledger.high_water {
            return Err(IdsError::new(
                IdsErrorKind::HighWater,
                format!(
                    "retired id `{}` records high-water {} past the ledger's {}",
                    tombstone.id, tombstone.high_water, ledger.high_water
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
    Ok(())
}

/// One `retired` row: its anchor, id, and the high-water the retirement witnessed.
/// The id joins `ids` so a reissue anywhere in the artifact is caught.
fn parse_retired_row(
    fields: &mut std::str::Split<'_, char>,
    line: &str,
    ids: &mut BTreeMap<DurableIdentityId, ()>,
) -> Result<IdentityTombstone, IdsError> {
    let (anchor, id) = parse_row(fields, line)?;
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
            format!("retired id `{id}` appears twice"),
        ));
    }
    Ok(IdentityTombstone {
        anchor,
        id,
        high_water: row_water,
    })
}

fn canonical_tombstone_order(left: &IdentityTombstone, right: &IdentityTombstone) -> Ordering {
    (&left.anchor, left.id).cmp(&(&right.anchor, right.id))
}

/// A validated captured identity artifact and its read-only semantic ledger.
///
/// Only project capture constructs this proof. The artifact is the exact
/// parser-validated bytes, or `None` when the tree committed no artifact, in
/// which case the empty ledger stands in.
#[derive(Clone)]
pub(crate) struct CapturedLedger {
    ledger: IdentityLedger,
    artifact: Option<Arc<[u8]>>,
}

impl CapturedLedger {
    pub(crate) fn capture(bytes: Option<&[u8]>) -> Result<Self, IdsError> {
        let ledger = match bytes {
            Some(bytes) => IdentityLedger::parse(bytes)?,
            None => IdentityLedger::default(),
        };
        Ok(Self {
            ledger,
            artifact: bytes.map(Arc::from),
        })
    }

    pub(crate) fn present_ledger(&self) -> Option<&IdentityLedger> {
        self.artifact.as_ref().map(|_| &self.ledger)
    }

    pub(crate) fn admit_identity_mints_with<E>(
        &self,
        first: IdentityAnchor,
        rest: Vec<IdentityAnchor>,
        supply: impl FnOnce(usize) -> Result<Vec<DurableIdentityId>, E>,
    ) -> Result<LedgerPublicationPlan, IdentityMintFailure<E>> {
        let plan =
            LedgerMutationPlan::mint(self, first, rest).map_err(IdentityMintFailure::Mutation)?;
        let candidates = supply(plan.change_count()).map_err(IdentityMintFailure::Supply)?;
        plan.bind_candidates(candidates)
            .map_err(IdentityMintFailure::Mutation)
    }
}

impl PartialEq for CapturedLedger {
    fn eq(&self, other: &Self) -> bool {
        self.artifact.is_some() == other.artifact.is_some() && self.ledger == other.ledger
    }
}

impl Eq for CapturedLedger {}

impl fmt::Debug for CapturedLedger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let artifact = match self.artifact {
            None => "Absent",
            Some(_) => "Present",
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
    /// The successor would exceed the fixed live-plus-tombstone row ceiling.
    RowLimit { projected: usize, limit: usize },
    /// The canonical successor would exceed the fixed artifact-byte ceiling.
    ByteLimit { projected: usize, limit: usize },
    /// The supplier returned a different number of candidates from the admitted request.
    CandidateCount { expected: usize, actual: usize },
    /// A candidate collides with a live, retired, or earlier candidate id.
    IdCollision(DurableIdentityId),
}

impl IdentityMutationError {
    /// The stable outward code for every planning or binding refusal.
    pub const fn code(&self) -> Code {
        Code::ProjectIdsMint
    }
}

/// Why a mint produced no publication plan. The two arms are distinct failures
/// with distinct outward codes: the caller owns the supply's, the ledger owns
/// the refusal's.
#[derive(Debug)]
pub enum IdentityMintFailure<E> {
    /// The caller's candidate supplier failed.
    Supply(E),
    /// Grammar, state, capacity, or candidate admission refused the request.
    Mutation(IdentityMutationError),
}

/// One canonical admitted successor bound to the exact captured artifact state
/// it was admitted against. Only project admission constructs it; the
/// publication owner installs `next` over a filesystem that still holds
/// `expected`.
#[derive(Debug)]
#[must_use = "a ledger publication plan must be consumed by the publication owner"]
pub struct LedgerPublicationPlan {
    expected: Option<Arc<[u8]>>,
    next: Vec<u8>,
}

impl LedgerPublicationPlan {
    /// The exact captured artifact bytes, or `None` when capture found no artifact.
    pub fn expected(&self) -> Option<&[u8]> {
        self.expected.as_deref()
    }

    /// The canonical admitted successor bytes.
    pub fn next(&self) -> &[u8] {
        &self.next
    }
}

/// The private borrowing owner of one structurally nonempty mint request set.
struct LedgerMutationPlan<'a> {
    captured: &'a CapturedLedger,
    requests: Vec<IdentityAnchor>,
    canonical_len: usize,
}

impl<'a> LedgerMutationPlan<'a> {
    fn mint(
        captured: &'a CapturedLedger,
        first: IdentityAnchor,
        mut rest: Vec<IdentityAnchor>,
    ) -> Result<Self, IdentityMutationError> {
        rest.push(first);
        rest.sort();
        let requests = validate_requests(rest)?;
        let ledger = &captured.ledger;
        for anchor in &requests {
            if ledger.entries.contains_key(anchor) {
                return Err(IdentityMutationError::AnchorActive(anchor.clone()));
            }
            if ledger.is_retired(anchor.kind, &anchor.path) {
                return Err(IdentityMutationError::AnchorRetired(anchor.clone()));
            }
        }
        let projected_rows = ledger.entries.len() + ledger.tombstones.len() + requests.len();
        if projected_rows > MAX_IDS_ROWS {
            return Err(IdentityMutationError::RowLimit {
                projected: projected_rows,
                limit: MAX_IDS_ROWS,
            });
        }
        let canonical_len = projected_canonical_len(ledger, &requests);
        if canonical_len > MAX_IDS_BYTES {
            return Err(IdentityMutationError::ByteLimit {
                projected: canonical_len,
                limit: MAX_IDS_BYTES,
            });
        }
        Ok(Self {
            captured,
            requests,
            canonical_len,
        })
    }

    fn change_count(&self) -> usize {
        self.requests.len()
    }

    /// Bind one candidate to each admitted request and serialize the successor.
    fn bind_candidates(
        self,
        candidates: Vec<DurableIdentityId>,
    ) -> Result<LedgerPublicationPlan, IdentityMutationError> {
        if candidates.len() != self.requests.len() {
            return Err(IdentityMutationError::CandidateCount {
                expected: self.requests.len(),
                actual: candidates.len(),
            });
        }
        let base = &self.captured.ledger;
        let mut used: BTreeSet<DurableIdentityId> = base.entries.values().copied().collect();
        used.extend(base.tombstones.iter().map(|tombstone| tombstone.id));
        for candidate in &candidates {
            if !used.insert(*candidate) {
                return Err(IdentityMutationError::IdCollision(*candidate));
            }
        }
        // Admission refused every live, retired, or repeated request anchor, so
        // each request lands on a fresh key.
        let mut ledger = base.clone();
        ledger
            .entries
            .extend(self.requests.into_iter().zip(candidates));
        let mut next = String::with_capacity(self.canonical_len);
        write_artifact(
            &mut next,
            ledger.entries(),
            &ledger.tombstones,
            ledger.high_water,
        )
        .expect("writing into a String cannot fail");
        Ok(LedgerPublicationPlan {
            expected: self.captured.artifact.clone(),
            next: next.into_bytes(),
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

/// The successor's exact canonical byte length, measured by running the one
/// serializer into a counting sink. An id renders at a fixed width, so the
/// requests are counted under a placeholder id before any candidate exists.
fn projected_canonical_len(ledger: &IdentityLedger, requests: &[IdentityAnchor]) -> usize {
    let placeholder = DurableIdentityId([0; 16]);
    let mut sink = ByteCount(0);
    write_artifact(
        &mut sink,
        ledger
            .entries()
            .chain(requests.iter().map(|anchor| (anchor, placeholder))),
        &ledger.tombstones,
        ledger.high_water,
    )
    .expect("a counting sink accepts every write");
    sink.0
}

struct ByteCount(usize);

impl fmt::Write for ByteCount {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.0 += text.len();
        Ok(())
    }
}

/// The one canonical serializer. `live` arrives in canonical anchor order from
/// the successor's map and `tombstones` in their canonical order; the counting
/// pass may pass rows in any order because every row's length is order-blind.
fn write_artifact<'a>(
    out: &mut impl fmt::Write,
    live: impl IntoIterator<Item = (&'a IdentityAnchor, DurableIdentityId)>,
    tombstones: &[IdentityTombstone],
    high_water: u64,
) -> fmt::Result {
    writeln!(out, "{IDS_HEADER}")?;
    writeln!(out, "{IDS_NOTICE}")?;
    for (anchor, id) in live {
        writeln!(out, "id {} {} {id}", anchor.kind.keyword(), anchor.path)?;
    }
    for tombstone in tombstones {
        writeln!(
            out,
            "retired {} {} {} {}",
            tombstone.anchor.kind.keyword(),
            tombstone.anchor.path,
            tombstone.id,
            tombstone.high_water
        )?;
    }
    writeln!(out, "high-water {high_water}")?;
    writeln!(out, "{IDS_END}")
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
    kind: IdsErrorKind,
    message: String,
}

impl IdsError {
    fn new(kind: IdsErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// The stable outward code: every ledger fault is `project.ids_corrupt`.
    pub const fn code(&self) -> Code {
        Code::ProjectIdsCorrupt
    }

    /// The typed reason the artifact was rejected.
    pub fn kind(&self) -> IdsErrorKind {
        self.kind
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for IdsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code().as_str(), self.message)
    }
}

impl std::error::Error for IdsError {}

#[cfg(test)]
mod tests {
    use super::{
        CapturedLedger, DurableIdentityId, IdentityAnchor, IdentityKind, IdentityLedger,
        IdentityMintFailure, IdentityMutationError, IdsErrorKind, LedgerPublicationPlan,
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
        plan.next().to_vec()
    }

    fn plan_parts(plan: LedgerPublicationPlan) -> (Option<Vec<u8>>, Vec<u8>) {
        (plan.expected().map(<[u8]>::to_vec), plan.next().to_vec())
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
            out.push_str(&format!("id field {path} {}\n", id_number(row).to_string()));
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
                id_number(row).to_string(),
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
            out.push_str(&format!("id field {path} {}\n", id_number(row).to_string()));
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
            out.push_str(&format!("id field {path} {}\n", id_number(row).to_string()));
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

    /// The counter ledger with `Counter.label` recorded as a tombstone. Nothing
    /// in this crate writes a tombstone, so the fixture is canonical bytes fed
    /// to the production parser.
    fn retired_counter_bytes() -> Vec<u8> {
        let live = String::from_utf8(counter_bytes()).expect("counter artifact is UTF-8");
        let hex = id(0x0f).to_string();
        let retired_line = format!("id field Counter.label {hex}\n");
        assert!(live.contains(&retired_line));
        let text = live.replace(&retired_line, "").replace(
            "high-water 0\n",
            &format!("retired field Counter.label {hex} 1\nhigh-water 1\n"),
        );
        let bytes = text.into_bytes();
        let parsed = IdentityLedger::parse(&bytes).expect("retired counter artifact parses");
        assert!(parsed.is_retired(IdentityKind::Field, "Counter.label"));
        bytes
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
        assert_eq!(identity.to_string(), expected);

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
        let target_row = format!("id field Counter.label {}\n", id(0x0f).to_string());
        let target_without_newline = target_row
            .strip_suffix('\n')
            .expect("the target row has its artifact newline");

        for suffix in [" ", "  ", " extra", " extra more", " extra "] {
            let replacement = format!("{target_without_newline}{suffix}\n");
            let malformed = text.replacen(&target_row, &replacement, 1);
            assert_eq!(
                IdentityLedger::parse(malformed.as_bytes())
                    .unwrap_err()
                    .kind(),
                IdsErrorKind::Malformed,
                "suffix {suffix:?} must reject before the row is inserted"
            );
        }

        let duplicate_with_suffix = text.replacen(
            &target_row,
            &format!("id application . {} extra\n", id(0x0a).to_string()),
            1,
        );
        assert_eq!(
            IdentityLedger::parse(duplicate_with_suffix.as_bytes())
                .unwrap_err()
                .kind(),
            IdsErrorKind::Malformed,
            "row grammar wins before duplicate-id or duplicate-anchor classification"
        );

        let suffix_and_conflict = text
            .replacen(&target_row, &format!("{target_without_newline} extra\n"), 1)
            .replacen("high-water ", "<<<<<<< ours\nhigh-water ", 1);
        assert_eq!(
            IdentityLedger::parse(suffix_and_conflict.as_bytes())
                .unwrap_err()
                .kind(),
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
        assert_eq!(error.kind(), IdsErrorKind::DuplicateAnchor);

        let dup_id = base.replace(
            "id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n",
            "id product Counter 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n",
        );
        let error = IdentityLedger::parse(dup_id.as_bytes()).unwrap_err();
        assert_eq!(error.kind(), IdsErrorKind::DuplicateId);
    }

    #[test]
    fn conflict_markers_and_torn_files_reject_whole() {
        let base = String::from_utf8(counter_bytes()).unwrap();
        for marker in ["<<<<<<< ours", "=======", ">>>>>>> theirs"] {
            let conflicted = base.replace("high-water", &format!("{marker}\nhigh-water"));
            assert_eq!(
                IdentityLedger::parse(conflicted.as_bytes())
                    .unwrap_err()
                    .kind(),
                IdsErrorKind::ConflictMarker,
                "marker {marker:?} must reject as a conflict"
            );
        }

        // A torn write loses the tail: no end marker, rejected whole.
        let torn = base.replace("end\n", "");
        assert_eq!(
            IdentityLedger::parse(torn.as_bytes()).unwrap_err().kind(),
            IdsErrorKind::Torn
        );

        // Content after the end marker is equally torn state.
        let trailing = format!("{base}id root extra 2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a\n");
        assert_eq!(
            IdentityLedger::parse(trailing.as_bytes())
                .unwrap_err()
                .kind(),
            IdsErrorKind::Malformed
        );
    }

    #[test]
    fn retire_then_re_add_cannot_reuse_the_anchor_or_id() {
        let retired_bytes = retired_counter_bytes();
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
        let base = String::from_utf8(retired_counter_bytes()).unwrap();

        // The retired anchor also live.
        let live_anchor = base.replace(
            "high-water 1",
            "id field Counter.label 2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b\nhigh-water 1",
        );
        assert_eq!(
            IdentityLedger::parse(live_anchor.as_bytes())
                .unwrap_err()
                .kind(),
            IdsErrorKind::RetiredReuse
        );

        // The retired id reissued on a live row.
        let live_id = base.replace(
            "high-water 1",
            "id field Counter.note 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\nhigh-water 1",
        );
        assert_eq!(
            IdentityLedger::parse(live_id.as_bytes())
                .unwrap_err()
                .kind(),
            IdsErrorKind::DuplicateId
        );

        // A tombstone past the ledger high-water is inconsistent history.
        let bad_water = base.replace("high-water 1", "high-water 0");
        assert_eq!(
            IdentityLedger::parse(bad_water.as_bytes())
                .unwrap_err()
                .kind(),
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
            IdentityLedger::parse(b"nonsense\n").unwrap_err().kind(),
            IdsErrorKind::Header
        );
        let missing_notice = "marrow ids v0\nhigh-water 0\nend\n";
        assert_eq!(
            IdentityLedger::parse(missing_notice.as_bytes())
                .unwrap_err()
                .kind(),
            IdsErrorKind::Header
        );
        let bad_row = "marrow ids v0\nmachine-written by marrow; do not edit\n\
                       id widget thing 00000000000000000000000000000000\nhigh-water 0\nend\n";
        assert_eq!(
            IdentityLedger::parse(bad_row.as_bytes())
                .unwrap_err()
                .kind(),
            IdsErrorKind::Malformed
        );
        let short_id = "marrow ids v0\nmachine-written by marrow; do not edit\n\
                        id root counters 0000\nhigh-water 0\nend\n";
        assert_eq!(
            IdentityLedger::parse(short_id.as_bytes())
                .unwrap_err()
                .kind(),
            IdsErrorKind::Malformed
        );
        let no_water = "marrow ids v0\nmachine-written by marrow; do not edit\nend\n";
        assert_eq!(
            IdentityLedger::parse(no_water.as_bytes())
                .unwrap_err()
                .kind(),
            IdsErrorKind::Malformed
        );
    }

    #[test]
    fn artifact_bounds_reject_oversize_input() {
        let huge = vec![b'a'; super::MAX_IDS_BYTES + 1];
        assert_eq!(
            IdentityLedger::parse(&huge).unwrap_err().kind(),
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
    fn near_cap_retired_base_refuses_rows_before_candidate_supply() {
        let retired_rows = super::MAX_IDS_ROWS / 2;
        let bytes = retired_artifact(retired_rows);
        let captured =
            CapturedLedger::capture(Some(&bytes)).expect("capture retired near-cap base");
        let request_count = super::MAX_IDS_ROWS - retired_rows + 1;
        let mut requests = (0..request_count)
            .map(|row| IdentityAnchor::new(IdentityKind::Field, format!("New.f{row:04}")));
        let first = requests.next().expect("nonempty near-cap request");
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
                    limit: 8192,
                }
            ))
        ));
        assert_eq!(calls.get(), 0, "row refusal precedes candidate supply");
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

        let retired_bytes = retired_counter_bytes();
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
        assert_eq!(expected, Some(crlf));
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

        let retired_bytes = retired_counter_bytes();
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
        assert_eq!(absent_expected, None);
        assert_eq!(present_expected, Some(empty_bytes.clone()));
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
        assert_eq!(canonical_expected, Some(canonical));
        assert_eq!(shuffled_expected, Some(shuffled));
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
        let first = format!("retired field Old.a {} 1\n", id(0x11).to_string(),);
        let second = format!("retired field Old.b {} 2\n", id(0x22).to_string(),);
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
        assert_eq!(canonical_expected, Some(canonical));
        assert_eq!(reversed_expected, Some(reversed));
        assert_ne!(canonical_expected, reversed_expected);
        assert_eq!(canonical_next, reversed_next);
        IdentityLedger::parse(&canonical_next).expect("canonical successor parses");
    }

    /// A mint over a base that already carries tombstones re-serializes those
    /// tombstones unchanged, so a retired anchor stays dead across publication.
    #[test]
    fn a_mint_over_a_tombstoned_base_carries_the_tombstones_forward() {
        let retired_bytes = retired_counter_bytes();
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
        assert_eq!(minted_ledger.high_water(), 1);
        assert_eq!(
            minted_ledger.lookup(IdentityKind::Field, "Counter.note"),
            Some(id(0x20)),
        );
    }

    /// An artifact one row past the cap rejects as `Bound`. The cap's relation to the
    /// record-field width is enforced at compile time by the `const _` block above.
    #[test]
    fn row_cap_holds_its_widened_value_and_rejects_one_past_it() {
        assert_eq!(super::MAX_IDS_ROWS, 8192, "durable-identity row cap");
        let mut out = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
        for row in 0..=super::MAX_IDS_ROWS {
            out.push_str(&format!("id field R.f{row} {:032x}\n", row + 1));
        }
        out.push_str("high-water 0\nend\n");
        assert_eq!(
            IdentityLedger::parse(out.as_bytes()).unwrap_err().kind(),
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
