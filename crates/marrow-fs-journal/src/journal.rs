//! The pending-journal claim, append, replay, truncate, and terminal-state
//! protocol, with crash-debris classification.
//!
//! A journal lives under two fixed names derived from one base entry name:
//! `<base>.pending.create` (the claim name) and `<base>.pending`. The claim
//! file is created `CREATE | EXCL` mode `0600`, self-witnesses its opened
//! inode, contains the complete header plus the sequence-zero Prepared record
//! before any link, and is same-handle synced, reread, and validated. It is
//! then hard-linked destination-refusing to the pending name and the parent is
//! synced: that parent sync is the durable claim.
//!
//! Create-only is preclaim; create-plus-pending must be one two-link inode;
//! normal pending is the same one-link inode. Wrong kind, a third inode or
//! link, a malformed self-witness, or an unexpected node is retained
//! corruption and authorizes no artifact mutation. After the final unlink both
//! names are absent and the retained journal handle is `nlink == 0`; the
//! parent sync alone commits marker absence and permits owner release.

use std::fmt;

use crate::custody::{AdmittedDir, CustodyError, FsIdentity, NodeKind, OpenedFile};
use crate::entry::{EntryName, EntryNameError};
use crate::frame::{DecodedFrame, FrameCorruption, FrameLawError, JournalKind};

/// The fixed claim-name suffix.
const CLAIM_SUFFIX: &str = ".pending.create";
/// The fixed pending-name suffix.
const PENDING_SUFFIX: &str = ".pending";

/// The two fixed names of one pending journal, derived from a base name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingName {
    base: EntryName,
    claim: EntryName,
    pending: EntryName,
}

impl PendingName {
    /// Derive `<base>.pending.create` and `<base>.pending`, re-admitting both
    /// derived spellings.
    pub fn derive(base: &EntryName) -> Result<Self, EntryNameError> {
        let _ = base;
        todo!("pending-name derivation")
    }

    /// The base name.
    pub fn base(&self) -> &EntryName {
        &self.base
    }

    /// The claim entry name (`<base>.pending.create`).
    pub fn claim(&self) -> &EntryName {
        &self.claim
    }

    /// The pending entry name (`<base>.pending`).
    pub fn pending(&self) -> &EntryName {
        &self.pending
    }
}

/// The identities a claim witnesses, offered to the row-header builder so
/// kinds 4 and 5 embed them as their self-witness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalWitness {
    /// The admitted parent directory's identity.
    pub parent: FsIdentity,
    /// The claim file's opened inode identity.
    pub journal_inode: FsIdentity,
}

/// Why a journal or its surroundings are retained corruption. Classification
/// never mutates corrupt state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorruptionReason {
    /// The frame bytes are corrupt.
    Frame(FrameCorruption),
    /// A claimed or pending journal lacks its sequence-zero Prepared record.
    MissingPrepared,
    /// A claimed journal carries bytes beyond the header and Prepared record,
    /// which only a completed claim may append.
    ClaimBeyondPrepared,
    /// A journal name maps to the wrong node kind.
    WrongNodeKind { found: NodeKind },
    /// A journal file does not carry the fixed `0600` mode.
    WrongMode { found: u32 },
    /// A journal inode carries an unexpected hard-link count.
    ExtraLinks { found: u64 },
    /// The claim and pending names map to different inodes.
    SplitInodes,
    /// A kind-4 or kind-5 header's parent identity is not the admitted
    /// parent.
    SelfWitnessParentMismatch,
    /// A kind-4 or kind-5 header's inode identity is not the journal's own.
    SelfWitnessInodeMismatch,
    /// A same-handle reread returned different bytes than were written.
    RereadMismatch,
}

impl fmt::Display for CorruptionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frame(corruption) => write!(formatter, "corrupt frame: {corruption}"),
            Self::MissingPrepared => {
                formatter.write_str("the journal lacks its sequence-zero Prepared record")
            }
            Self::ClaimBeyondPrepared => {
                formatter.write_str("a claimed journal carries bytes beyond its Prepared record")
            }
            Self::WrongNodeKind { found } => {
                write!(formatter, "a journal name maps to a {found}")
            }
            Self::WrongMode { found } => {
                write!(formatter, "the journal mode is {found:o}, not 600")
            }
            Self::ExtraLinks { found } => {
                write!(formatter, "the journal inode has {found} links")
            }
            Self::SplitInodes => {
                formatter.write_str("the claim and pending names map to different inodes")
            }
            Self::SelfWitnessParentMismatch => {
                formatter.write_str("the header's parent identity is not the admitted parent")
            }
            Self::SelfWitnessInodeMismatch => {
                formatter.write_str("the header's inode identity is not the journal's own")
            }
            Self::RereadMismatch => {
                formatter.write_str("a same-handle reread returned different bytes")
            }
        }
    }
}

/// Retained corruption: a typed classification that authorizes no mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedCorruption {
    reason: CorruptionReason,
}

impl RetainedCorruption {
    pub(crate) fn new(reason: CorruptionReason) -> Self {
        Self { reason }
    }

    /// Why the state is corrupt.
    pub fn reason(&self) -> &CorruptionReason {
        &self.reason
    }
}

/// A typed journal failure.
#[derive(Debug)]
pub enum JournalError {
    /// A custody operation refused.
    Custody(CustodyError),
    /// The producer violated the kind's frame law; nothing was written.
    Law(FrameLawError),
    /// Corruption was found; nothing was mutated.
    Corrupt(CorruptionReason),
    /// A kind-4 or kind-5 row header did not embed the offered witness.
    WitnessNotEmbedded,
    /// The append would exceed the kind's ceiling; nothing was written.
    CeilingExceeded { total: usize, limit: usize },
    /// The terminal registry phase is already recorded.
    AppendAfterComplete,
    /// The requested phase tag does not advance past the last recorded one.
    TagNotAdvancing { last: u8, requested: u8 },
    /// The terminal registry phase is not yet recorded.
    FinishBeforeComplete { last_tag: u8 },
    /// The journal has no incomplete tail to truncate.
    NoIncompleteTail,
    /// The journal's incomplete tail must be truncated before resuming.
    IncompleteTail,
    /// The incomplete tail is not an exact prefix of the offered unique next
    /// record; nothing was truncated.
    TailNotPrefix,
    /// The offered next record is not a legal continuation of the frame.
    ExpectedRecordIllegal(FrameCorruption),
}

impl fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Custody(error) => write!(formatter, "{error}"),
            Self::Law(error) => write!(formatter, "frame law refused: {error}"),
            Self::Corrupt(reason) => write!(formatter, "retained corruption: {reason}"),
            Self::WitnessNotEmbedded => {
                formatter.write_str("the row header did not embed the claim witness")
            }
            Self::CeilingExceeded { total, limit } => write!(
                formatter,
                "appending would grow the journal to {total} bytes, over its {limit}-byte ceiling"
            ),
            Self::AppendAfterComplete => {
                formatter.write_str("the terminal registry phase is already recorded")
            }
            Self::TagNotAdvancing { last, requested } => write!(
                formatter,
                "phase tag {requested} does not advance past the recorded {last}"
            ),
            Self::FinishBeforeComplete { last_tag } => write!(
                formatter,
                "the journal's last phase tag is {last_tag}, not the terminal registry phase"
            ),
            Self::NoIncompleteTail => formatter.write_str("the journal has no incomplete tail"),
            Self::IncompleteTail => {
                formatter.write_str("the incomplete tail must be truncated before resuming")
            }
            Self::TailNotPrefix => formatter
                .write_str("the incomplete tail is not an exact prefix of the unique next record"),
            Self::ExpectedRecordIllegal(corruption) => write!(
                formatter,
                "the offered next record is not a legal continuation: {corruption}"
            ),
        }
    }
}

impl std::error::Error for JournalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Custody(error) => Some(error),
            Self::Law(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CustodyError> for JournalError {
    fn from(error: CustodyError) -> Self {
        Self::Custody(error)
    }
}

impl From<FrameLawError> for JournalError {
    fn from(error: FrameLawError) -> Self {
        Self::Law(error)
    }
}

/// The classified state of one pending-journal name pair.
#[derive(Debug)]
pub enum PendingState<'d> {
    /// Neither name exists.
    Absent,
    /// Only the claim name exists: preclaim debris, never durably claimed,
    /// discardable under witness.
    Preclaim(PreclaimDebris<'d>),
    /// Both names exist as one two-link inode holding exactly the header and
    /// Prepared record: a durable (or durable-pending) claim to adopt.
    Claimed(ClaimedJournal<'d>),
    /// Only the pending name exists: a one-link claimed journal to replay.
    Pending(PendingJournal<'d>),
    /// Retained corruption; no mutation is authorized.
    Corrupt(RetainedCorruption),
}

/// Preclaim debris: a claim file that was never durably claimed. The only
/// permitted mutation is a witnessed discard.
#[derive(Debug)]
pub struct PreclaimDebris<'d> {
    dir: &'d AdmittedDir,
    name: PendingName,
    file: OpenedFile,
}

impl PreclaimDebris<'_> {
    /// The debris file's witnessed identity.
    pub fn identity(&self) -> FsIdentity {
        self.file.identity()
    }

    /// Discard the debris under witness: the name must still map to the
    /// witnessed inode immediately before the unlink, and the parent is
    /// synced afterward.
    pub fn discard(self) -> Result<(), JournalError> {
        todo!("witnessed preclaim discard")
    }
}

/// A claimed journal observed in its two-link state.
#[derive(Debug)]
pub struct ClaimedJournal<'d> {
    dir: &'d AdmittedDir,
    name: PendingName,
    file: OpenedFile,
    frame: DecodedFrame,
}

impl<'d> ClaimedJournal<'d> {
    /// The validated frame (header plus the Prepared record).
    pub fn frame(&self) -> &DecodedFrame {
        &self.frame
    }

    /// Complete the claim: re-sync the parent so the claim is durable,
    /// unlink the claim name, and return the live journal.
    pub fn adopt(self) -> Result<LiveJournal<'d>, JournalError> {
        todo!("claim adoption")
    }
}

/// A pending journal replayed from its one-link state.
#[derive(Debug)]
pub struct PendingJournal<'d> {
    dir: &'d AdmittedDir,
    name: PendingName,
    file: OpenedFile,
    frame: DecodedFrame,
}

impl<'d> PendingJournal<'d> {
    /// The replayed frame.
    pub fn frame(&self) -> &DecodedFrame {
        &self.frame
    }

    /// Truncate an incomplete tail that is an exact prefix of
    /// `expected_next_record`, the unique legal next record the caller
    /// derived from the header and admitted artifact state. Any other tail is
    /// corruption and nothing is mutated.
    pub fn truncate_tail(&mut self, expected_next_record: &[u8]) -> Result<(), JournalError> {
        let _ = expected_next_record;
        todo!("exact-prefix tail truncation")
    }

    /// Resume appending. The tail must be clean.
    pub fn resume(self) -> Result<LiveJournal<'d>, JournalError> {
        todo!("journal resumption")
    }
}

/// A live journal holding the retained claim-time handle. Appends are
/// bounded, synced, and rechecked; the terminal unlink is the only exit.
#[derive(Debug)]
pub struct LiveJournal<'d> {
    dir: &'d AdmittedDir,
    name: PendingName,
    kind: JournalKind,
    file: OpenedFile,
    total_len: usize,
    next_sequence: u32,
    last_tag: u8,
}

impl LiveJournal<'_> {
    /// The journal's kind.
    pub fn kind(&self) -> JournalKind {
        self.kind
    }

    /// The next record's sequence.
    pub fn next_sequence(&self) -> u32 {
        self.next_sequence
    }

    /// The last recorded phase tag.
    pub fn last_tag(&self) -> u8 {
        self.last_tag
    }

    /// Whether the terminal registry phase is recorded.
    pub fn is_complete(&self) -> bool {
        self.last_tag == self.kind.phase_count()
    }

    /// Append one record: validate the kind's law, recheck the mapping and
    /// witness, write, `fsync` the file, and recheck again.
    pub fn append(&mut self, phase_tag: u8, payload: &[u8]) -> Result<(), JournalError> {
        let _ = (phase_tag, payload);
        todo!("bounded journal append")
    }

    /// Terminally unlink the complete journal: after the unlink both names
    /// are absent and the retained handle is `nlink == 0`; the parent sync
    /// alone commits marker absence and permits owner release.
    pub fn finish(self) -> Result<(), JournalError> {
        todo!("terminal unlink")
    }
}

/// Claim a new pending journal in `dir` under `name`. `build_header` receives
/// the claim witness and returns the row-specific header; kinds 4 and 5 must
/// embed the witness as their leading `JournalCommon`. `prepared_payload` is
/// the sequence-zero Prepared record's payload.
pub fn claim<'d>(
    dir: &'d AdmittedDir,
    name: &PendingName,
    kind: JournalKind,
    build_header: impl FnOnce(&JournalWitness) -> Vec<u8>,
    prepared_payload: &[u8],
) -> Result<LiveJournal<'d>, JournalError> {
    let _ = (dir, name, kind, build_header, prepared_payload);
    todo!("journal claim")
}

/// Classify the state of the pending-journal name pair without mutating
/// anything.
pub fn classify<'d>(
    dir: &'d AdmittedDir,
    name: &PendingName,
    expected: JournalKind,
) -> Result<PendingState<'d>, JournalError> {
    let _ = (dir, name, expected);
    todo!("debris classification")
}
