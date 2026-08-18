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

use crate::custody::{AdmittedDir, CustodyError, EntryStat, FsIdentity, NodeKind, OpenedFile};
use crate::entry::{EntryName, EntryNameError};
use crate::frame::{
    DecodedFrame, FrameCorruption, FrameLawError, JOURNAL_COMMON_LEN, JournalCommon, JournalKind,
    PREFIX_LEN, RECORD_OVERHEAD, TailState, decode_frame, encode_header, encode_record,
};

/// The fixed claim-name suffix.
const CLAIM_SUFFIX: &str = ".pending.create";
/// The fixed pending-name suffix.
const PENDING_SUFFIX: &str = ".pending";
/// The fixed journal file mode.
const JOURNAL_MODE: u32 = 0o600;

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
        let claim = EntryName::admit(&format!("{base}{CLAIM_SUFFIX}"))?;
        let pending = EntryName::admit(&format!("{base}{PENDING_SUFFIX}"))?;
        Ok(Self {
            base: base.clone(),
            claim,
            pending,
        })
    }

    /// The base name.
    pub fn base(&self) -> &EntryName {
        &self.base
    }

    /// The claim entry name (`<base>.pending.create`).
    pub fn claim(&self) -> &EntryName {
        &self.claim
    }

    /// The two marker names alone, for a reader that may touch nothing else.
    pub fn markers(&self) -> MarkerNames<'_> {
        MarkerNames {
            claim: &self.claim,
            pending: &self.pending,
        }
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
    /// The journal file's size is not the exact expected byte length.
    UnexpectedLength { expected: u64, found: u64 },
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
            Self::UnexpectedLength { expected, found } => write!(
                formatter,
                "the journal is {found} bytes, not the expected {expected}"
            ),
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

/// A typed journal failure.
#[derive(Debug)]
pub enum JournalError {
    /// A custody operation refused.
    Custody(CustodyError),
    /// A kind whose header must be led by the shared common was offered a
    /// header with no common to lead it; nothing was written.
    WitnessNotEmbedded,
    /// The producer violated the kind's frame law; nothing was written.
    Law(FrameLawError),
    /// Corruption was found; no further mutation is authorized.
    Corrupt(CorruptionReason),
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
            Self::WitnessNotEmbedded => formatter
                .write_str("this journal kind's header must be led by the claim's own witness"),
            Self::Law(error) => write!(formatter, "frame law refused: {error}"),
            Self::Corrupt(reason) => write!(formatter, "retained corruption: {reason}"),
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
    Corrupt(CorruptionReason),
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
        discard_witnessed(self.dir, self.name.claim(), &self.file)
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

    /// Complete the claim: re-establish the durable claim sync (the crash may
    /// have preceded it), unlink the claim name, and return the live journal.
    pub fn adopt(self) -> Result<LiveJournal<'d>, JournalError> {
        let total = frame_total_len(&self.frame);
        self.dir.sync()?;
        recheck(self.dir, self.name.pending(), &self.file, 2, total)?;
        recheck(self.dir, self.name.claim(), &self.file, 2, total)?;
        self.dir.unlink(self.name.claim())?;
        self.dir.sync()?;
        recheck(self.dir, self.name.pending(), &self.file, 1, total)?;
        Ok(LiveJournal {
            dir: self.dir,
            kind: self.frame.kind(),
            witness: JournalWitness {
                parent: self.dir.identity(),
                journal_inode: self.file.identity(),
            },
            name: self.name,
            file: self.file,
            total_len: total,
            next_sequence: 1,
            last_tag: 1,
        })
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
        let TailState::IncompletePrefix { bytes } = self.frame.tail() else {
            return Err(JournalError::NoIncompleteTail);
        };
        let tail = bytes.clone();
        let valid_len = frame_total_len(&self.frame);
        let valid_bytes = self.file.read_prefix(valid_len)?;

        // The offered record must itself be the legal continuation: appended
        // to the valid frame it decodes as exactly one more complete record.
        let mut candidate = valid_bytes.clone();
        candidate.extend_from_slice(expected_next_record);
        match decode_frame(self.frame.kind(), &candidate) {
            Ok(frame)
                if frame.records().len() == self.frame.records().len() + 1
                    && frame.tail() == &TailState::Clean => {}
            Ok(_) => return Err(JournalError::TailNotPrefix),
            Err(corruption) => return Err(JournalError::ExpectedRecordIllegal(corruption)),
        }

        // The exact-prefix law: only a strict prefix of that unique record
        // may be truncated.
        if tail.len() >= expected_next_record.len()
            || expected_next_record[..tail.len()] != tail[..]
        {
            return Err(JournalError::TailNotPrefix);
        }

        recheck(
            self.dir,
            self.name.pending(),
            &self.file,
            1,
            valid_len + tail.len(),
        )?;
        self.file.truncate(valid_len as u64)?;
        self.file.sync()?;
        let reread = self.file.read_prefix(self.frame.kind().ceiling() + 1)?;
        if reread != valid_bytes {
            return Err(JournalError::Corrupt(CorruptionReason::RereadMismatch));
        }
        recheck(self.dir, self.name.pending(), &self.file, 1, valid_len)?;
        self.frame = decode_frame(self.frame.kind(), &valid_bytes)
            .map_err(|corruption| JournalError::Corrupt(CorruptionReason::Frame(corruption)))?;
        Ok(())
    }

    /// Resume appending. The tail must be clean.
    pub fn resume(self) -> Result<LiveJournal<'d>, JournalError> {
        if self.frame.tail() != &TailState::Clean {
            return Err(JournalError::IncompleteTail);
        }
        let total = frame_total_len(&self.frame);
        recheck(self.dir, self.name.pending(), &self.file, 1, total)?;
        let last = self
            .frame
            .records()
            .last()
            .expect("a pending journal carries at least its Prepared record");
        let next_sequence = u32::try_from(self.frame.records().len())
            .expect("a registry holds at most six records");
        Ok(LiveJournal {
            dir: self.dir,
            kind: self.frame.kind(),
            last_tag: last.phase_tag(),
            witness: JournalWitness {
                parent: self.dir.identity(),
                journal_inode: self.file.identity(),
            },
            name: self.name,
            file: self.file,
            total_len: total,
            next_sequence,
        })
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
    witness: JournalWitness,
    total_len: usize,
    next_sequence: u32,
    last_tag: u8,
}

impl LiveJournal<'_> {
    /// The journal's kind.
    pub fn kind(&self) -> JournalKind {
        self.kind
    }

    /// The identities this claim witnessed: the directory it was claimed under
    /// and the inode it was written into. A caller that built the row header as
    /// a value reads them back here rather than from inside the claim.
    pub fn witness(&self) -> JournalWitness {
        self.witness
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
        if self.is_complete() {
            return Err(JournalError::AppendAfterComplete);
        }
        if phase_tag <= self.last_tag {
            return Err(JournalError::TagNotAdvancing {
                last: self.last_tag,
                requested: phase_tag,
            });
        }
        let record = encode_record(self.kind, self.next_sequence, phase_tag, payload)?;
        let total = self.total_len + record.len();
        if total > self.kind.ceiling() {
            return Err(JournalError::CeilingExceeded {
                total,
                limit: self.kind.ceiling(),
            });
        }
        recheck(self.dir, self.name.pending(), &self.file, 1, self.total_len)?;
        self.file.append(&record)?;
        self.file.sync()?;
        recheck(self.dir, self.name.pending(), &self.file, 1, total)?;
        self.total_len = total;
        self.next_sequence += 1;
        self.last_tag = phase_tag;
        Ok(())
    }

    /// Terminally unlink the complete journal: after the unlink both names
    /// are absent and the retained handle is `nlink == 0`; the parent sync
    /// alone commits marker absence and permits owner release.
    pub fn finish(self) -> Result<(), JournalError> {
        if !self.is_complete() {
            return Err(JournalError::FinishBeforeComplete {
                last_tag: self.last_tag,
            });
        }
        recheck(self.dir, self.name.pending(), &self.file, 1, self.total_len)?;
        if self.dir.stat_entry(self.name.claim())?.is_some() {
            return Err(JournalError::Custody(CustodyError::IdentityDrift {
                op: "finish",
            }));
        }
        self.dir.unlink(self.name.pending())?;
        let stat = self.file.stat()?;
        if stat.nlink() != 0 {
            return Err(JournalError::Corrupt(CorruptionReason::ExtraLinks {
                found: stat.nlink(),
            }));
        }
        if self.dir.stat_entry(self.name.pending())?.is_some()
            || self.dir.stat_entry(self.name.claim())?.is_some()
        {
            return Err(JournalError::Custody(CustodyError::IdentityDrift {
                op: "finish",
            }));
        }
        self.dir.sync()?;
        Ok(())
    }
}

/// A claim that refused, carrying whether a marker may exist because of it.
///
/// The two arms call for opposite handling, and the caller cannot tell them
/// apart from the error alone, so the distinction is a type rather than a
/// judgement made at the call site.
///
/// The boundary is the first attempt to link the marker into place, not the
/// parent sync that follows it. A link that has been issued may have taken
/// effect and been persisted whether or not the sync ran or returned: an
/// unsynced directory entry is not guaranteed absent after a crash, only
/// not guaranteed present. Treating an unsynced or outcome-uncertain link as
/// clean is exactly the mistake that lets a caller clean up under a marker
/// that survives.
///
/// There is no conversion into the bare error. Discarding this distinction is
/// what a caller must not do silently, so it must name an arm to reach the
/// refusal inside.
#[derive(Debug)]
pub enum ClaimRefusal {
    /// The refusal happened strictly before the first link attempt. No link
    /// was issued, so no marker can exist; the project carries at most a
    /// never-linked claim file, which classification reads as preclaim.
    Preclaim(JournalError),
    /// The refusal happened at or after the first link attempt. Whether a
    /// marker exists is not knowable here, so the caller must treat one as
    /// existing: hand the project to recovery rather than clean up under it.
    PossiblyDurable(JournalError),
}

/// A row header as a caller can build it: the generation slot and everything
/// after the shared leading common.
///
/// The claim composes the common itself, from the directory it is claiming
/// under and the inode it created, so a caller supplies no part of it and runs
/// no code inside the claim. That is the point of this being a value: a
/// callback invoked partway through the claim could reach the same directory
/// and link the marker itself, and every refusal after that would still be
/// reported as leaving no marker.
#[derive(Debug, Clone)]
pub enum BuiltHeader {
    /// A header led by the shared common. The caller supplies the generation
    /// slot and the bytes after it; the claim composes the common from the
    /// directory it is claiming under and the inode it created, so the two
    /// identities a caller cannot know are the two it does not supply.
    Witnessed {
        /// The row's 16-byte generation evidence.
        generation: [u8; 16],
        /// Everything after the common.
        tail: Vec<u8>,
    },
    /// A header with no shared common: the bytes are the whole of it. Kinds
    /// that carry a self-witness may not use this shape.
    Plain(Vec<u8>),
}

impl BuiltHeader {
    fn compose(&self, witness: &JournalWitness) -> Vec<u8> {
        match self {
            Self::Witnessed { generation, tail } => {
                let common = JournalCommon {
                    generation: *generation,
                    parent: witness.parent,
                    journal_inode: witness.journal_inode,
                };
                let mut bytes = Vec::with_capacity(JOURNAL_COMMON_LEN + tail.len());
                bytes.extend_from_slice(&common.encode());
                bytes.extend_from_slice(tail);
                bytes
            }
            Self::Plain(bytes) => bytes.clone(),
        }
    }
}

/// Claim a new pending journal in `dir` under `name`. `header` is the
/// row-specific header as a value; the claim composes the leading
/// `JournalCommon` from its own witness, so kinds 4 and 5 embed it by
/// construction. `prepared_payload` is the sequence-zero Prepared record's
/// payload.
///
/// # Errors
///
/// Returns a [`ClaimRefusal`] naming which side of the first link attempt the
/// refusal fell on. A [`ClaimRefusal::PossiblyDurable`] refusal may have left a
/// marker, so the caller hands the project to recovery rather than cleaning up
/// under it.
pub fn claim<'d>(
    dir: &'d AdmittedDir,
    name: &PendingName,
    kind: JournalKind,
    header: BuiltHeader,
    prepared_payload: &[u8],
) -> Result<LiveJournal<'d>, ClaimRefusal> {
    // The split is the enforcement. Everything whose refusal is genuinely
    // preclaim happens inside `claim_preflight`, which cannot reach the link;
    // everything from the link onward happens inside `claim_commit`, whose
    // every refusal is possibly-durable by construction. Neither arm is chosen
    // by inspecting an error or a flag, so a step that moves across the
    // boundary changes which function it is written in and nothing else.
    if kind.carries_self_witness() && matches!(header, BuiltHeader::Plain(_)) {
        return Err(ClaimRefusal::Preclaim(JournalError::WitnessNotEmbedded));
    }
    let prepared = claim_preflight(dir, name, kind, &header, prepared_payload)
        .map_err(ClaimRefusal::Preclaim)?;
    let total_len = prepared.bytes_len;
    let file = prepared.file;
    let witness = prepared.witness;
    claim_commit(dir, name, &file, total_len).map_err(ClaimRefusal::PossiblyDurable)?;
    Ok(LiveJournal {
        dir,
        name: name.clone(),
        kind,
        file,
        witness,
        total_len,
        next_sequence: 1,
        last_tag: 1,
    })
}

/// What the preflight leaves for the commit: the claim file it created and
/// filled, and the length it wrote.
struct PreparedClaim {
    file: OpenedFile,
    bytes_len: usize,
    witness: JournalWitness,
}

/// Everything strictly before the first link attempt.
///
/// No link is issued here, so every refusal this returns leaves no marker. A
/// refusal after the claim file exists discards it under witness first, which
/// is sound precisely because nothing has been linked: the entry is this
/// call's own, never-linked, and classification reads whatever a failed
/// discard leaves as preclaim.
fn claim_preflight(
    dir: &AdmittedDir,
    name: &PendingName,
    kind: JournalKind,
    header: &BuiltHeader,
    prepared_payload: &[u8],
) -> Result<PreparedClaim, JournalError> {
    if dir.stat_entry(name.claim())?.is_some() || dir.stat_entry(name.pending())?.is_some() {
        return Err(JournalError::Custody(CustodyError::AlreadyExists {
            op: "claim",
        }));
    }
    let mut file = dir.create_file_excl(name.claim())?;
    let witness = JournalWitness {
        parent: dir.identity(),
        journal_inode: file.identity(),
    };
    match write_claim_file(
        dir,
        name,
        &mut file,
        kind,
        &header.compose(&witness),
        prepared_payload,
    ) {
        Ok(bytes) => Ok(PreparedClaim {
            bytes_len: bytes.len(),
            file,
            witness,
        }),
        Err(refusal) => {
            // The refusal the caller must see is the one that stopped the
            // claim; a discard that itself fails leaves debris classification
            // owns, and must not replace a durability-relevant refusal with
            // its own unlink error.
            let _ = discard_witnessed(dir, name.claim(), &file);
            Err(refusal)
        }
    }
}

/// The first link attempt through the checks that follow it.
///
/// Every refusal from here is possibly-durable, and nothing here removes
/// anything. Once a link has been issued the marker may exist — an entry an
/// unsynced directory carries can still survive a crash, and an errored link
/// reports no outcome at all — so a cleanup issued on the strength of a
/// refusal here could make a marker's own successor absent underneath it.
/// Whatever state a refusal leaves is classification's to read and recovery's
/// to settle.
fn claim_commit(
    dir: &AdmittedDir,
    name: &PendingName,
    file: &OpenedFile,
    total_len: usize,
) -> Result<(), JournalError> {
    dir.link(name.claim(), name.pending())?;
    // This parent sync is what makes the claim durable in the ordinary case;
    // it is not what makes a refusal above or below it possibly-durable.
    dir.sync()?;
    recheck(dir, name.pending(), file, 2, total_len)?;
    recheck(dir, name.claim(), file, 2, total_len)?;
    dir.unlink(name.claim())?;
    dir.sync()?;
    recheck(dir, name.pending(), file, 1, total_len)?;
    if dir.stat_entry(name.claim())?.is_some() {
        return Err(JournalError::Custody(CustodyError::IdentityDrift {
            op: "claim",
        }));
    }
    Ok(())
}

/// The two marker stats a classification decides from, and nothing else.
///
/// Minted only by [`Self::read`], so a classification cannot be handed a shape
/// that was never on disk.
#[derive(Debug)]
pub struct MarkerStats {
    claim: Option<EntryStat>,
    pending: Option<EntryStat>,
}

impl MarkerStats {
    /// Stat the two marker names, and only those two.
    ///
    /// This is the pre-reconciliation read: a caller that classifies before
    /// reconciling other state depends on it touching no artifact. It receives
    /// the two names it may stat rather than the pair they came from, so the
    /// base name a `PendingName` also carries is not in scope here and an
    /// artifact read does not compile.
    ///
    /// # Errors
    ///
    /// Returns the custody refusal either stat produced.
    pub fn read(dir: &AdmittedDir, marker: MarkerNames<'_>) -> Result<Self, CustodyError> {
        Ok(Self {
            claim: dir.stat_entry(marker.claim)?,
            pending: dir.stat_entry(marker.pending)?,
        })
    }
}

/// The two marker names, separated from the pair that also carries the base.
///
/// Minted only by [`PendingName::markers`], so the pair is still the one owner
/// of the spellings; what this removes is the reach back to the base name at
/// the point where reaching it would be a defect.
#[derive(Debug, Clone, Copy)]
pub struct MarkerNames<'n> {
    claim: &'n EntryName,
    pending: &'n EntryName,
}

/// Which of the four marker-name shapes the pair is in.
///
/// This is the whole of what the name pair decides, and it is decided from the
/// stats alone: [`classify`] receives no directory and no names, so a
/// classification cannot read an artifact however it is later edited. A caller
/// that classifies before reconciling other state depends on exactly that, and
/// it is now a property of the signature rather than of the body.
///
/// Turning a shape into a state that can act — discard, adopt, resume — reads
/// the marker file and needs the directory, so it is a separate, named step the
/// caller takes at the point of consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerShape {
    /// Neither name exists.
    Absent,
    /// Only the claim name exists.
    Preclaim(EntryStat),
    /// Both names exist.
    Claimed {
        /// The claim name's stat.
        claim: EntryStat,
        /// The pending name's stat.
        pending: EntryStat,
    },
    /// Only the pending name exists.
    Pending(EntryStat),
}

impl MarkerShape {
    /// The identity of the marker file this shape names, if it names one.
    ///
    /// This is the stat the classification itself acted on, so a caller that
    /// needs the marker's identity reads it from here rather than statting the
    /// name again. A second read could see a different object than the one
    /// classified; this cannot.
    #[must_use]
    pub fn marker_identity(&self) -> Option<FsIdentity> {
        match self {
            Self::Absent => None,
            // Both names exist as one two-link inode, so either stat names it;
            // the claim's is the one the claimed reading validated.
            Self::Preclaim(stat) | Self::Claimed { claim: stat, .. } | Self::Pending(stat) => {
                Some(stat.identity())
            }
        }
    }
}

/// The shape of the pending-journal name pair. Reads nothing and mutates
/// nothing: the stats are the whole input.
#[must_use]
pub fn classify(stats: MarkerStats) -> MarkerShape {
    match (stats.claim, stats.pending) {
        (None, None) => MarkerShape::Absent,
        (Some(claim), None) => MarkerShape::Preclaim(claim),
        (Some(claim), Some(pending)) => MarkerShape::Claimed { claim, pending },
        (None, Some(pending)) => MarkerShape::Pending(pending),
    }
}

impl MarkerShape {
    /// Turn this shape into the state that can act on it, reading the marker
    /// file where the shape says there is one.
    ///
    /// This runs after the shape is decided, so it is not the reader the
    /// classify-before-reconcile ordering depends on — that one is
    /// [`MarkerStats::read`], which is narrowed to the two names. Admission
    /// takes the full pair and directory deliberately: the states it returns
    /// act through both afterwards, a preclaim discard and a pending resume
    /// among them, and the base name is what a `PendingName` is for.
    ///
    /// # Errors
    ///
    /// Returns the journal refusal reading or validating the marker produced.
    pub fn admit<'d>(
        self,
        dir: &'d AdmittedDir,
        name: &PendingName,
        expected: JournalKind,
    ) -> Result<PendingState<'d>, JournalError> {
        match self {
            Self::Absent => Ok(PendingState::Absent),
            Self::Preclaim(claim) => classify_preclaim(dir, name, claim),
            Self::Claimed { claim, pending } => {
                classify_claimed(dir, name, expected, claim, pending)
            }
            Self::Pending(pending) => classify_pending(dir, name, expected, pending),
        }
    }
}

fn classify_preclaim<'d>(
    dir: &'d AdmittedDir,
    name: &PendingName,
    stat: EntryStat,
) -> Result<PendingState<'d>, JournalError> {
    if stat.kind() != NodeKind::Regular {
        return corrupt_state(CorruptionReason::WrongNodeKind { found: stat.kind() });
    }
    if stat.nlink() != 1 {
        return corrupt_state(CorruptionReason::ExtraLinks {
            found: stat.nlink(),
        });
    }
    // Preclaim content and mode are unconstrained: the crash may have fallen
    // anywhere before the durable claim, including before the mode-restoring
    // fchmod, so classification and discard must need only read access.
    let file = witnessed(dir.open_file_readonly(name.claim())?, stat.identity())?;
    Ok(PendingState::Preclaim(PreclaimDebris {
        dir,
        name: name.clone(),
        file,
    }))
}

fn classify_claimed<'d>(
    dir: &'d AdmittedDir,
    name: &PendingName,
    expected: JournalKind,
    claim_stat: EntryStat,
    pending_stat: EntryStat,
) -> Result<PendingState<'d>, JournalError> {
    for stat in [&claim_stat, &pending_stat] {
        if stat.kind() != NodeKind::Regular {
            return corrupt_state(CorruptionReason::WrongNodeKind { found: stat.kind() });
        }
    }
    if claim_stat.identity() != pending_stat.identity() {
        return corrupt_state(CorruptionReason::SplitInodes);
    }
    if claim_stat.nlink() != 2 {
        return corrupt_state(CorruptionReason::ExtraLinks {
            found: claim_stat.nlink(),
        });
    }
    if claim_stat.mode() != JOURNAL_MODE {
        return corrupt_state(CorruptionReason::WrongMode {
            found: claim_stat.mode(),
        });
    }
    let file = witnessed(dir.open_file(name.claim())?, claim_stat.identity())?;
    let frame = match replay(expected, &file)? {
        Ok(frame) => frame,
        Err(corruption) => return corrupt_state(CorruptionReason::Frame(corruption)),
    };
    if frame.records().is_empty() {
        return corrupt_state(CorruptionReason::MissingPrepared);
    }
    if frame.records().len() > 1 || frame.tail() != &TailState::Clean {
        return corrupt_state(CorruptionReason::ClaimBeyondPrepared);
    }
    if let Some(reason) = witness_mismatch(&frame, dir, &file) {
        return corrupt_state(reason);
    }
    Ok(PendingState::Claimed(ClaimedJournal {
        dir,
        name: name.clone(),
        file,
        frame,
    }))
}

fn classify_pending<'d>(
    dir: &'d AdmittedDir,
    name: &PendingName,
    expected: JournalKind,
    stat: EntryStat,
) -> Result<PendingState<'d>, JournalError> {
    if stat.kind() != NodeKind::Regular {
        return corrupt_state(CorruptionReason::WrongNodeKind { found: stat.kind() });
    }
    if stat.nlink() != 1 {
        return corrupt_state(CorruptionReason::ExtraLinks {
            found: stat.nlink(),
        });
    }
    if stat.mode() != JOURNAL_MODE {
        return corrupt_state(CorruptionReason::WrongMode { found: stat.mode() });
    }
    let file = witnessed(dir.open_file(name.pending())?, stat.identity())?;
    let frame = match replay(expected, &file)? {
        Ok(frame) => frame,
        Err(corruption) => return corrupt_state(CorruptionReason::Frame(corruption)),
    };
    if frame.records().is_empty() {
        return corrupt_state(CorruptionReason::MissingPrepared);
    }
    if let Some(reason) = witness_mismatch(&frame, dir, &file) {
        return corrupt_state(reason);
    }
    Ok(PendingState::Pending(PendingJournal {
        dir,
        name: name.clone(),
        file,
        frame,
    }))
}

/// Require a freshly opened handle to still be the observed inode.
fn witnessed(file: OpenedFile, observed: FsIdentity) -> Result<OpenedFile, JournalError> {
    if file.identity() != observed {
        return Err(JournalError::Custody(CustodyError::IdentityDrift {
            op: "classify",
        }));
    }
    Ok(file)
}

/// Bounded replay: read at most ceiling-plus-one bytes through the retained
/// handle and decode. The inner result separates transient custody failures
/// (outer) from frame corruption (inner).
fn replay(
    expected: JournalKind,
    file: &OpenedFile,
) -> Result<Result<DecodedFrame, FrameCorruption>, JournalError> {
    let bytes = file.read_prefix(expected.ceiling() + 1)?;
    Ok(decode_frame(expected, &bytes))
}

fn witness_mismatch(
    frame: &DecodedFrame,
    dir: &AdmittedDir,
    file: &OpenedFile,
) -> Option<CorruptionReason> {
    let common = frame.journal_common()?;
    if common.parent != dir.identity() {
        return Some(CorruptionReason::SelfWitnessParentMismatch);
    }
    if common.journal_inode != file.identity() {
        return Some(CorruptionReason::SelfWitnessInodeMismatch);
    }
    None
}

/// Assemble, write, sync, and validate the claim file's exact bytes through
/// the creating handle. Every refusal returns to the caller, which still
/// holds the never-linked file's witness for the discard.
fn write_claim_file(
    dir: &AdmittedDir,
    name: &PendingName,
    file: &mut OpenedFile,
    kind: JournalKind,
    header: &[u8],
    prepared_payload: &[u8],
) -> Result<Vec<u8>, JournalError> {
    let bytes = claim_bytes(kind, header, prepared_payload)?;
    file.append(&bytes)?;
    file.sync()?;
    let reread = file.read_prefix(kind.ceiling() + 1)?;
    if reread != bytes {
        return Err(JournalError::Corrupt(CorruptionReason::RereadMismatch));
    }
    recheck(dir, name.claim(), file, 1, bytes.len())?;
    Ok(bytes)
}

/// Assemble and law-check the claim bytes: the header plus the sequence-zero
/// Prepared record.
///
/// The header arrives already composed against this claim's own witness, so
/// nothing here re-checks that it embeds one.
fn claim_bytes(
    kind: JournalKind,
    header: &[u8],
    prepared_payload: &[u8],
) -> Result<Vec<u8>, JournalError> {
    let mut bytes = encode_header(kind, header)?;
    bytes.extend_from_slice(&encode_record(kind, 0, 1, prepared_payload)?);
    if bytes.len() > kind.ceiling() {
        return Err(JournalError::CeilingExceeded {
            total: bytes.len(),
            limit: kind.ceiling(),
        });
    }
    Ok(bytes)
}

/// The exact byte length of a decoded frame's durable content.
fn frame_total_len(frame: &DecodedFrame) -> usize {
    PREFIX_LEN
        + frame.row_header().len()
        + frame
            .records()
            .iter()
            .map(|record| RECORD_OVERHEAD + record.payload().len())
            .sum::<usize>()
}

fn corrupt_state<'d>(reason: CorruptionReason) -> Result<PendingState<'d>, JournalError> {
    Ok(PendingState::Corrupt(reason))
}

/// Recheck the retained handle and its path mapping before and after every
/// mutation: node kind, the exact `0600` mode, link count, exact byte length,
/// and name-to-inode mapping. Any mismatch fails closed.
fn recheck(
    dir: &AdmittedDir,
    name: &EntryName,
    file: &OpenedFile,
    nlink: u64,
    len: usize,
) -> Result<(), JournalError> {
    // Path mapping first: a stolen or vanished name is identity drift, not a
    // link-count artifact of the retained handle.
    match dir.stat_entry(name)? {
        Some(entry) if entry.identity() == file.identity() => {}
        _ => {
            return Err(JournalError::Custody(CustodyError::IdentityDrift {
                op: "journal recheck",
            }));
        }
    }
    let stat = file.stat()?;
    if stat.kind() != NodeKind::Regular {
        return Err(JournalError::Corrupt(CorruptionReason::WrongNodeKind {
            found: stat.kind(),
        }));
    }
    if stat.mode() != JOURNAL_MODE {
        return Err(JournalError::Corrupt(CorruptionReason::WrongMode {
            found: stat.mode(),
        }));
    }
    if stat.nlink() != nlink {
        return Err(JournalError::Corrupt(CorruptionReason::ExtraLinks {
            found: stat.nlink(),
        }));
    }
    let expected = len as u64;
    if stat.size() != expected {
        return Err(JournalError::Corrupt(CorruptionReason::UnexpectedLength {
            expected,
            found: stat.size(),
        }));
    }
    Ok(())
}

/// Unlink `name` after a stat witnesses that it still maps to the file's
/// inode, then sync the parent: the one permitted cleanup, and nothing is
/// removed on the evidence of the name alone. The witness and the unlink are
/// separate calls, so a replacement landing between them is still removed —
/// the safety claim needs an exclusive or private admitted parent for exactly
/// this reason.
fn discard_witnessed(
    dir: &AdmittedDir,
    name: &EntryName,
    file: &OpenedFile,
) -> Result<(), JournalError> {
    match dir.stat_entry(name)? {
        Some(entry) if entry.identity() == file.identity() => {
            dir.unlink(name)?;
            dir.sync()?;
            Ok(())
        }
        _ => Err(JournalError::Custody(CustodyError::IdentityDrift {
            op: "discard",
        })),
    }
}
