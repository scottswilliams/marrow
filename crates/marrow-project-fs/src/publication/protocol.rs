//! The publication state machine: preflight, claim, install, settle, recover.
//!
//! One driver serves both entries. A fresh publication builds the header from
//! the admitted plan and drives from `Prepared`; recovery decodes the header a
//! crashed publication already made durable and drives from whatever phase the
//! journal reached. Neither enumerates a directory, and neither mutates a byte
//! of a state outside the closed map.

use std::cell::RefCell;
use std::fmt;

use marrow_fs_journal::{
    AdmittedDir, CustodyError, EntryName, EntryStat, FsIdentity, JournalKind, LiveJournal,
    PendingState, PhaseRecord, TailState, claim, classify, encode_record,
};
use marrow_project::{LedgerExpectedArtifact, LedgerPublicationPlan, LedgerPublicationView};

use super::header::RowHeader;
use super::{
    IdsPublication, IdsPublicationError, IdsPublicationPending, IdsPublishOutcome, IdsRefusal,
    LEDGER_BYTE_CEILING, ProjectMetadataWriteGuard,
};

/// The kind-1 phase registry. Tag 1 is the claim's own `Prepared` record.
const INSTALLING: u8 = 2;
/// The terminal phase. Its one payload byte names which terminal it is, which
/// is what lets the frame's single exit serve both a completed install and a
/// publication that settled without installing.
const SETTLED: u8 = 3;

/// Which retained byte run failed its exact comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArtifactRole {
    /// The committed `.marrow/ids`.
    Target,
    /// The staged successor.
    Stage,
}

impl fmt::Display for ArtifactRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Target => "the committed ledger",
            Self::Stage => "the staged successor",
        })
    }
}

/// Why a state is outside the closed publication map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MapFault {
    /// The artifact map is not a state this phase can have produced.
    OffMap {
        /// The phase tag the journal had reached.
        phase: u8,
    },
    /// The successor is installed and a third live inode holds the stage name.
    /// After process death that is indistinguishable from an install whose
    /// displaced generation was substituted, so it authorizes no exchange and
    /// no cleanup.
    ThirdInode,
    /// A retained byte run is not the exact run the header binds.
    BytesDrift {
        /// Which run failed.
        role: ArtifactRole,
    },
    /// The exchange did not land in a state the protocol can settle.
    ExchangeUncertain,
    /// The journal reached its terminal phase with no terminal record.
    MissingTerminal,
    /// The terminal record's payload names no terminal.
    UnknownTerminal,
    /// The header's parent identity is not the admitted metadata directory.
    ParentMismatch,
    /// The header's journal-inode identity is not the marker's own.
    InodeMismatch,
    /// The journal carries a phase tag outside the kind's registry.
    UnknownPhase {
        /// The tag found.
        found: u8,
    },
}

impl fmt::Display for MapFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OffMap { phase } => write!(
                formatter,
                "the `.marrow` artifact map is not a state phase {phase} can have produced"
            ),
            Self::ThirdInode => formatter
                .write_str("a third live inode holds the stage name beside an installed successor"),
            Self::BytesDrift { role } => write!(formatter, "{role} is not the exact bound run"),
            Self::ExchangeUncertain => {
                formatter.write_str("the exchange did not land in a settleable state")
            }
            Self::MissingTerminal => {
                formatter.write_str("the terminal phase is recorded with no terminal record")
            }
            Self::UnknownTerminal => {
                formatter.write_str("the terminal record's payload names no terminal")
            }
            Self::ParentMismatch => {
                formatter.write_str("the header's parent identity is not the admitted directory")
            }
            Self::InodeMismatch => {
                formatter.write_str("the header's inode identity is not the marker's own")
            }
            Self::UnknownPhase { found } => {
                write!(
                    formatter,
                    "phase tag {found} is outside the kind's registry"
                )
            }
        }
    }
}

impl From<MapFault> for IdsPublicationError {
    fn from(fault: MapFault) -> Self {
        Self {
            refusal: IdsRefusal::Corrupt,
            detail: super::Detail::Map(fault),
        }
    }
}

/// Which terminal the publication settled into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Terminal {
    /// The successor is installed.
    Installed,
    /// The successor was not installed; the artifact is the other writer's.
    Reverted,
}

impl Terminal {
    const INSTALLED: u8 = 0;
    const REVERTED: u8 = 1;

    const fn payload(self) -> u8 {
        match self {
            Self::Installed => Self::INSTALLED,
            Self::Reverted => Self::REVERTED,
        }
    }

    const fn decode(payload: u8) -> Option<Self> {
        match payload {
            Self::INSTALLED => Some(Self::Installed),
            Self::REVERTED => Some(Self::Reverted),
            _ => None,
        }
    }

    const fn publication(self) -> IdsPublication {
        match self {
            Self::Installed => IdsPublication::Published,
            Self::Reverted => IdsPublication::ConcurrentChange,
        }
    }
}

/// The closed artifact map, read from name-to-inode mappings, link counts, and
/// exact sizes. Each phase admits its own subset; every other reading is
/// retained corruption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MapState {
    /// `target=base` (or absent) and `stage=next`, each at one link.
    Prepared,
    /// `target=next`, with the displaced generation at the stage name — the
    /// same inode at two links when the plan displaced nothing.
    Installed,
    /// `target=next` at one link and the stage absent: cleanup is done.
    InstalledClean,
    /// `stage=next` and a target that is neither the successor nor the bound
    /// generation: the publication reverted and has not been cleaned.
    Reverted,
    /// The stage is absent and the target is not the successor: a reverted
    /// publication whose cleanup is done.
    RevertedClean,
}

/// One publication under one guard, live from its durable claim to its
/// terminal unlink.
pub(super) struct Session<'a> {
    guard: &'a ProjectMetadataWriteGuard,
    header: RowHeader,
    terminal: Option<Terminal>,
    journal: Option<LiveJournal<'a>>,
}

// ===== Entry points ==========================================================

/// Compare `plan` against the filesystem under `guard` and install it.
pub(super) fn publish(
    guard: &ProjectMetadataWriteGuard,
    plan: LedgerPublicationPlan,
) -> Result<IdsPublishOutcome<'_>, IdsPublicationError> {
    preflight(guard)?;
    plan.visit(|view| publish_admitted(guard, view))
}

/// Settle any durably claimed publication under `guard`.
pub(super) fn recover(
    guard: &ProjectMetadataWriteGuard,
) -> Result<Option<IdsPublication>, IdsPublicationError> {
    let meta = guard.meta();
    let names = guard.journal_names();
    let claim_witness = meta.stat_entry(names.claim())?.map(|stat| stat.identity());
    let pending_witness = meta
        .stat_entry(names.pending())?
        .map(|stat| stat.identity());
    match classify(meta, names, JournalKind::Ids)? {
        PendingState::Absent => {
            if meta.stat_entry(guard.stage_name())?.is_some() {
                return Err(IdsPublicationError::bare(IdsRefusal::UnclaimedIncomplete));
            }
            Ok(None)
        }
        // A claim file that was never durably claimed is retained, not swept:
        // its content is unconstrained by the journal's own law, so nothing
        // here can prove it belongs to this project's protocol run.
        PendingState::Preclaim(_) => {
            Err(IdsPublicationError::bare(IdsRefusal::UnclaimedIncomplete))
        }
        PendingState::Corrupt(reason) => Err(IdsPublicationError::corrupt(reason)),
        PendingState::Claimed(claimed) => {
            let header = admit_header(meta, claimed.frame().row_header(), claim_witness)?;
            let mut session = Session::new(guard, header, claimed.adopt()?, None);
            session.drive().map(Some)
        }
        PendingState::Pending(mut pending) => {
            let header = admit_header(meta, pending.frame().row_header(), pending_witness)?;
            let terminal = terminal_of(pending.frame().records())?;
            if let TailState::IncompletePrefix { .. } = pending.frame().tail() {
                let last = last_tag(pending.frame().records());
                let expected = expected_next_record(guard, &header, last)?;
                pending.truncate_tail(&expected)?;
            }
            let mut session = Session::new(guard, header, pending.resume()?, terminal);
            session.drive().map(Some)
        }
    }
}

// ===== Fresh publication =====================================================

/// Refuse before anything is staged when the project already carries a
/// publication state. A fresh publication starts only from a clear one.
fn preflight(guard: &ProjectMetadataWriteGuard) -> Result<(), IdsPublicationError> {
    let meta = guard.meta();
    match classify(meta, guard.journal_names(), JournalKind::Ids)? {
        PendingState::Absent => {}
        PendingState::Preclaim(_) => {
            return Err(IdsPublicationError::bare(IdsRefusal::UnclaimedIncomplete));
        }
        PendingState::Claimed(_) | PendingState::Pending(_) => {
            return Err(IdsPublicationError::bare(IdsRefusal::Interrupted));
        }
        PendingState::Corrupt(reason) => return Err(IdsPublicationError::corrupt(reason)),
    }
    if meta.stat_entry(guard.stage_name())?.is_some() {
        return Err(IdsPublicationError::bare(IdsRefusal::UnclaimedIncomplete));
    }
    Ok(())
}

fn publish_admitted<'a>(
    guard: &'a ProjectMetadataWriteGuard,
    view: LedgerPublicationView<'_>,
) -> Result<IdsPublishOutcome<'a>, IdsPublicationError> {
    let meta = guard.meta();
    let next = view.next();

    // Recapture under the guard: the plan may only be installed over the exact
    // state it was admitted against.
    let observed = read_entry(meta, guard.ledger_name())?;
    let base = match (view.expected(), &observed) {
        (LedgerExpectedArtifact::Absent, None) => None,
        (LedgerExpectedArtifact::Present(expected), Some((identity, seen)))
            if expected == seen.as_slice() =>
        {
            Some(*identity)
        }
        _ => return Ok(IdsPublishOutcome::Settled(IdsPublication::ConcurrentChange)),
    };

    let stage = stage_successor(guard, next)?;
    let built: RefCell<Option<RowHeader>> = RefCell::new(None);
    let claimed = claim(
        meta,
        guard.journal_names(),
        JournalKind::Ids,
        |witness| {
            let header = RowHeader {
                parent: witness.parent,
                journal_inode: witness.journal_inode,
                base,
                next_inode: stage,
                base_bytes: observed
                    .as_ref()
                    .map(|(_, bytes)| bytes.clone())
                    .unwrap_or_default(),
                next_bytes: next.to_vec(),
            };
            let bytes = header.encode();
            *built.borrow_mut() = Some(header);
            bytes
        },
        &[],
    );
    let journal = match claimed {
        Ok(journal) => journal,
        Err(refusal) => {
            // Nothing is durably claimed, so this is an ordinary refusal — but
            // only after the stage this call created is gone and its absence is
            // durable. A refusal that left the stage behind would leave the
            // project in the manual unclaimed state instead.
            discard_stage(guard, stage)?;
            return Err(refusal.into());
        }
    };
    let header = built
        .into_inner()
        .expect("the claim builds its header before it writes");

    // From here the marker is durable: every interruption is affine.
    let mut session = Session::new(guard, header, journal, None);
    match session.drive() {
        Ok(publication) => Ok(IdsPublishOutcome::Settled(publication)),
        Err(cause) => Ok(IdsPublishOutcome::Pending(Box::new(
            IdsPublicationPending::new(session, cause),
        ))),
    }
}

/// Create, fill, sync, and validate the fixed stage entry, returning the
/// staged inode's identity. Any refusal removes the stage under witness first.
fn stage_successor(
    guard: &ProjectMetadataWriteGuard,
    next: &[u8],
) -> Result<FsIdentity, IdsPublicationError> {
    let meta = guard.meta();
    let name = guard.stage_name();
    let mut file = meta.create_file_excl(name)?;
    let identity = file.identity();
    let filled = (|| -> Result<(), IdsPublicationError> {
        file.append(next)?;
        file.sync()?;
        if file.read_prefix(LEDGER_BYTE_CEILING + 1)? != next {
            return Err(MapFault::BytesDrift {
                role: ArtifactRole::Stage,
            }
            .into());
        }
        meta.sync()?;
        require_entry(meta, name, identity, 1, next.len())?;
        Ok(())
    })();
    match filled {
        Ok(()) => Ok(identity),
        Err(refusal) => {
            discard_stage(guard, identity)?;
            Err(refusal)
        }
    }
}

/// Remove the stage entry under witness and make its absence durable.
fn discard_stage(
    guard: &ProjectMetadataWriteGuard,
    identity: FsIdentity,
) -> Result<(), IdsPublicationError> {
    let meta = guard.meta();
    let name = guard.stage_name();
    match meta.stat_entry(name)? {
        Some(stat) if stat.identity() == identity => {
            meta.unlink(name)?;
            meta.sync()?;
            Ok(())
        }
        None => Ok(()),
        Some(_) => Err(CustodyError::IdentityDrift {
            op: "stage discard",
        }
        .into()),
    }
}

// ===== The driver ============================================================

impl<'a> Session<'a> {
    fn new(
        guard: &'a ProjectMetadataWriteGuard,
        header: RowHeader,
        journal: LiveJournal<'a>,
        terminal: Option<Terminal>,
    ) -> Self {
        Self {
            guard,
            header,
            terminal,
            journal: Some(journal),
        }
    }

    /// Drive the publication to its terminal state from whatever phase the
    /// journal has reached. Re-entrant: every phase re-reads the artifact map,
    /// so a call after an interruption resumes rather than repeats.
    pub(super) fn drive(&mut self) -> Result<IdsPublication, IdsPublicationError> {
        loop {
            match self.phase() {
                1 => {
                    self.require_prepared()?;
                    self.append(INSTALLING, &[])?;
                }
                INSTALLING => {
                    let terminal = self.install()?;
                    self.append(SETTLED, &[terminal.payload()])?;
                    self.terminal = Some(terminal);
                }
                SETTLED => return self.settle(),
                found => return Err(MapFault::UnknownPhase { found }.into()),
            }
        }
    }

    fn phase(&self) -> u8 {
        self.journal
            .as_ref()
            .expect("the journal is live until it is finished")
            .last_tag()
    }

    fn append(&mut self, tag: u8, payload: &[u8]) -> Result<(), IdsPublicationError> {
        self.journal
            .as_mut()
            .expect("the journal is live until it is finished")
            .append(tag, payload)?;
        Ok(())
    }

    /// At `Prepared` the artifact must still be exactly what the header binds:
    /// nothing has been installed, so the map admits one reading only.
    fn require_prepared(&self) -> Result<(), IdsPublicationError> {
        match self.read_map(1)? {
            MapState::Prepared => self.require_prepared_bytes(),
            _ => Err(MapFault::OffMap { phase: 1 }.into()),
        }
    }

    /// Install the successor, or settle without installing it. The map decides:
    /// a crash between the `Installing` record and the mutation leaves the
    /// `Prepared` reading, and a crash after it leaves the installed reading.
    fn install(&self) -> Result<Terminal, IdsPublicationError> {
        match self.read_map(INSTALLING)? {
            MapState::Prepared => {
                self.require_prepared_bytes()?;
                match self.header.base {
                    None => self.link_absent(),
                    Some(base) => self.exchange_replace(base),
                }
            }
            settled => terminal_of_map(settled),
        }
    }

    /// Re-read the artifact map and name the terminal it settled into.
    ///
    /// The mutations call this instead of deciding a terminal from their own
    /// outcome: a refused destination and a returned exchange each leave a map,
    /// and the map is what recovery would read after a crash at the same point.
    fn terminal_from_map(&self) -> Result<Terminal, IdsPublicationError> {
        terminal_of_map(self.read_map(INSTALLING)?)
    }

    /// The absent arm: one destination-refusing hard link. A refused
    /// destination means the artifact this plan was admitted against as absent
    /// now exists, so the publication settles without installing.
    fn link_absent(&self) -> Result<Terminal, IdsPublicationError> {
        let meta = self.meta();
        match meta.link(self.guard.stage_name(), self.guard.ledger_name()) {
            Ok(()) => {
                let next = self.header.next_inode;
                require_entry(
                    meta,
                    self.guard.ledger_name(),
                    next,
                    2,
                    self.header.next_bytes.len(),
                )?;
                require_entry(
                    meta,
                    self.guard.stage_name(),
                    next,
                    2,
                    self.header.next_bytes.len(),
                )?;
                meta.sync()?;
                self.terminal_from_map()
            }
            // The artifact this plan was admitted against as absent now
            // exists, so the map — not this refusal — names the terminal.
            Err(CustodyError::AlreadyExists { .. }) => self.terminal_from_map(),
            Err(error) => Err(error.into()),
        }
    }

    /// The replace arm: one atomic exchange. The displaced generation lands at
    /// the stage name; anything else there is a third live inode that this
    /// process — and only this process, which has just proven the pre-exchange
    /// reading — may exchange back.
    fn exchange_replace(&self, base: FsIdentity) -> Result<Terminal, IdsPublicationError> {
        let meta = self.meta();
        let next = self.header.next_inode;
        meta.exchange(self.guard.ledger_name(), self.guard.stage_name())?;
        let target = self.stat(self.guard.ledger_name())?;
        let staged = self.stat(self.guard.stage_name())?;
        let (Some(target), Some(staged)) = (target, staged) else {
            return Err(MapFault::ExchangeUncertain.into());
        };
        if target.identity() != next {
            return Err(MapFault::ExchangeUncertain.into());
        }
        if staged.identity() == base {
            meta.sync()?;
            return self.terminal_from_map();
        }
        let third = staged.identity();
        meta.exchange(self.guard.ledger_name(), self.guard.stage_name())?;
        let restored = self.stat(self.guard.ledger_name())?;
        let returned = self.stat(self.guard.stage_name())?;
        match (restored, returned) {
            (Some(restored), Some(returned))
                if restored.identity() == third && returned.identity() == next =>
            {
                meta.sync()?;
                self.terminal_from_map()
            }
            _ => Err(MapFault::ExchangeUncertain.into()),
        }
    }

    /// Clean the exact stage alias, prove the terminal reading, then unlink the
    /// marker and sync. Only that final directory sync ends the publication.
    fn settle(&mut self) -> Result<IdsPublication, IdsPublicationError> {
        let terminal = self.terminal.ok_or(MapFault::MissingTerminal)?;
        let map = self.read_map(SETTLED)?;
        match (terminal, map) {
            (Terminal::Installed, MapState::Installed) => {
                self.clean_stage(self.displaced_identity())?;
            }
            (Terminal::Reverted, MapState::Reverted) => {
                self.clean_stage(self.header.next_inode)?;
            }
            (Terminal::Installed, MapState::InstalledClean)
            | (Terminal::Reverted, MapState::RevertedClean) => {}
            _ => return Err(MapFault::OffMap { phase: SETTLED }.into()),
        }
        self.prove_terminal(terminal)?;
        self.journal
            .take()
            .expect("the journal is live until it is finished")
            .finish()?;
        Ok(terminal.publication())
    }

    /// The identity the stage name carries once the successor is installed: the
    /// displaced generation, or the successor itself when the plan displaced
    /// nothing and the two names are one inode.
    fn displaced_identity(&self) -> FsIdentity {
        self.header.base.unwrap_or(self.header.next_inode)
    }

    fn clean_stage(&self, expected: FsIdentity) -> Result<(), IdsPublicationError> {
        let meta = self.meta();
        let name = self.guard.stage_name();
        match meta.stat_entry(name)? {
            Some(stat) if stat.identity() == expected => {
                meta.unlink(name)?;
                meta.sync()?;
                Ok(())
            }
            _ => Err(CustodyError::IdentityDrift {
                op: "stage cleanup",
            }
            .into()),
        }
    }

    /// Prove the terminal reading before the marker goes: an installed
    /// publication is the exact successor at one link with the stage absent; a
    /// reverted one leaves the artifact untouched and only proves the stage
    /// gone.
    fn prove_terminal(&self, terminal: Terminal) -> Result<(), IdsPublicationError> {
        let meta = self.meta();
        if meta.stat_entry(self.guard.stage_name())?.is_some() {
            return Err(MapFault::OffMap { phase: SETTLED }.into());
        }
        if terminal == Terminal::Installed {
            require_entry(
                meta,
                self.guard.ledger_name(),
                self.header.next_inode,
                1,
                self.header.next_bytes.len(),
            )?;
            self.require_bytes(
                self.guard.ledger_name(),
                &self.header.next_bytes,
                ArtifactRole::Target,
            )?;
        }
        Ok(())
    }

    // ----- map reading -------------------------------------------------------

    fn meta(&self) -> &'a AdmittedDir {
        self.guard.meta()
    }

    fn stat(&self, name: &EntryName) -> Result<Option<EntryStat>, IdsPublicationError> {
        Ok(self.meta().stat_entry(name)?)
    }

    /// Read the closed artifact map from name-to-inode mappings, link counts,
    /// and exact sizes. Byte runs are compared separately, at the points where
    /// an exact comparison decides a mutation. `phase` names the reader for an
    /// off-map refusal and is supplied rather than read from the journal,
    /// because the tail-derivation probe reads the map with no live journal.
    fn read_map(&self, phase: u8) -> Result<MapState, IdsPublicationError> {
        let target = self.stat(self.guard.ledger_name())?;
        let staged = self.stat(self.guard.stage_name())?;
        let next = self.header.next_inode;
        let next_len = self.header.next_bytes.len();
        let staged_is_next =
            |stat: &EntryStat| stat.identity() == next && stat.size() == next_len as u64;

        match (target, staged, self.header.base) {
            // Prepared, absent arm: the artifact this plan displaces nothing of
            // is still absent and the successor is staged.
            (None, Some(staged), None) if staged_is_next(&staged) && staged.nlink() == 1 => {
                Ok(MapState::Prepared)
            }
            // Prepared, replace arm: the bound generation is still committed.
            (Some(target), Some(staged), Some(base))
                if target.identity() == base
                    && target.nlink() == 1
                    && staged_is_next(&staged)
                    && staged.nlink() == 1 =>
            {
                Ok(MapState::Prepared)
            }
            // Installed, absent arm: one inode under both names.
            (Some(target), Some(staged), None)
                if target.identity() == next
                    && staged.identity() == next
                    && target.nlink() == 2 =>
            {
                Ok(MapState::Installed)
            }
            // Installed, replace arm: the displaced generation sits at the
            // stage name.
            (Some(target), Some(staged), Some(base))
                if target.identity() == next && staged.identity() == base =>
            {
                Ok(MapState::Installed)
            }
            // The one state a dead process cannot be given the benefit of: the
            // successor is installed and the stage holds neither the displaced
            // generation nor the successor itself.
            (Some(target), Some(_), _) if target.identity() == next => {
                Err(MapFault::ThirdInode.into())
            }
            (Some(target), None, _) if target.identity() == next && target.nlink() == 1 => {
                Ok(MapState::InstalledClean)
            }
            // Reverted: the successor is still only staged and the artifact is
            // neither it nor the generation the plan bound.
            (Some(target), Some(staged), base)
                if staged_is_next(&staged)
                    && staged.nlink() == 1
                    && target.identity() != next
                    && Some(target.identity()) != base =>
            {
                Ok(MapState::Reverted)
            }
            (Some(target), None, _) if target.identity() != next => Ok(MapState::RevertedClean),
            _ => Err(MapFault::OffMap { phase }.into()),
        }
    }

    fn require_prepared_bytes(&self) -> Result<(), IdsPublicationError> {
        self.require_bytes(
            self.guard.stage_name(),
            &self.header.next_bytes,
            ArtifactRole::Stage,
        )?;
        if self.header.base.is_some() {
            self.require_bytes(
                self.guard.ledger_name(),
                &self.header.base_bytes,
                ArtifactRole::Target,
            )?;
        }
        Ok(())
    }

    fn require_bytes(
        &self,
        name: &EntryName,
        expected: &[u8],
        role: ArtifactRole,
    ) -> Result<(), IdsPublicationError> {
        match read_entry(self.meta(), name)? {
            Some((_, bytes)) if bytes == expected => Ok(()),
            _ => Err(MapFault::BytesDrift { role }.into()),
        }
    }
}

// ===== Recovery helpers ======================================================

/// Decode a durable header and require its own witnesses: the directory it was
/// claimed under and the marker inode it was written into. A header that
/// witnesses another directory or another inode is substituted evidence.
fn admit_header(
    meta: &AdmittedDir,
    bytes: &[u8],
    marker: Option<FsIdentity>,
) -> Result<RowHeader, IdsPublicationError> {
    let header = RowHeader::decode(bytes)?;
    if header.parent != meta.identity() {
        return Err(MapFault::ParentMismatch.into());
    }
    if marker != Some(header.journal_inode) {
        return Err(MapFault::InodeMismatch.into());
    }
    Ok(header)
}

fn last_tag(records: &[PhaseRecord]) -> u8 {
    records.last().map_or(0, PhaseRecord::phase_tag)
}

/// The terminal a replayed journal already recorded, if it reached one.
fn terminal_of(records: &[PhaseRecord]) -> Result<Option<Terminal>, IdsPublicationError> {
    let Some(record) = records.iter().find(|record| record.phase_tag() == SETTLED) else {
        return Ok(None);
    };
    match record.payload() {
        [payload] => Terminal::decode(*payload)
            .map(Some)
            .ok_or_else(|| MapFault::UnknownTerminal.into()),
        _ => Err(MapFault::UnknownTerminal.into()),
    }
}

/// The unique legal next record, derived from the durable header and the
/// admitted artifact state. It is the only byte run an incomplete tail may be
/// a prefix of.
fn expected_next_record(
    guard: &ProjectMetadataWriteGuard,
    header: &RowHeader,
    last: u8,
) -> Result<Vec<u8>, IdsPublicationError> {
    let terminal = match last {
        1 => return Ok(record(1, INSTALLING, &[])),
        INSTALLING => pending_terminal(guard, header)?,
        found => return Err(MapFault::UnknownPhase { found }.into()),
    };
    Ok(record(2, SETTLED, &[terminal.payload()]))
}

/// Encode one kind-1 record. The kind's registry admits exactly these
/// sequence/tag/payload shapes, so the frame law cannot refuse them.
fn record(sequence: u32, tag: u8, payload: &[u8]) -> Vec<u8> {
    encode_record(JournalKind::Ids, sequence, tag, payload)
        .expect("a kind-1 phase record is lawful by construction")
}

/// Which terminal record a crash could have been appending. The artifact map
/// answers it: a `Prepared` reading means no terminal was in flight at all.
fn pending_terminal(
    guard: &ProjectMetadataWriteGuard,
    header: &RowHeader,
) -> Result<Terminal, IdsPublicationError> {
    let probe = Session {
        guard,
        header: header.clone(),
        terminal: None,
        journal: None,
    };
    probe.terminal_from_map()
}

/// The one place a settled artifact map is turned into a terminal.
///
/// Every reader shares it: the phase driver, a mutation that found the
/// destination taken or exchanged a successor back out, and the tail derivation
/// that must reproduce the exact record a crash was appending. A second answer
/// anywhere could record an installed successor over a map that says nothing
/// was installed, and the `(terminal, map)` check in `settle` would then retain
/// the project as corrupt instead of publishing.
fn terminal_of_map(map: MapState) -> Result<Terminal, IdsPublicationError> {
    match map {
        MapState::Installed => Ok(Terminal::Installed),
        MapState::Reverted => Ok(Terminal::Reverted),
        MapState::Prepared | MapState::InstalledClean | MapState::RevertedClean => {
            Err(MapFault::OffMap { phase: INSTALLING }.into())
        }
    }
}

// ===== Custody helpers =======================================================

/// Read one entry's exact bytes through an opened handle, witnessing the inode
/// the name mapped to. `None` when the entry is absent.
fn read_entry(
    dir: &AdmittedDir,
    name: &EntryName,
) -> Result<Option<(FsIdentity, Vec<u8>)>, IdsPublicationError> {
    let Some(stat) = dir.stat_entry(name)? else {
        return Ok(None);
    };
    let file = dir.open_file(name)?;
    if file.identity() != stat.identity() {
        return Err(CustodyError::IdentityDrift { op: "ledger read" }.into());
    }
    let bytes = file.read_prefix(LEDGER_BYTE_CEILING + 1)?;
    if bytes.len() > LEDGER_BYTE_CEILING {
        return Err(MapFault::BytesDrift {
            role: ArtifactRole::Target,
        }
        .into());
    }
    Ok(Some((stat.identity(), bytes)))
}

/// Require a name to map to `identity` as a regular file at `nlink` links and
/// exactly `len` bytes.
fn require_entry(
    dir: &AdmittedDir,
    name: &EntryName,
    identity: FsIdentity,
    nlink: u64,
    len: usize,
) -> Result<(), IdsPublicationError> {
    match dir.stat_entry(name)? {
        Some(stat)
            if stat.identity() == identity
                && stat.nlink() == nlink
                && stat.size() == len as u64 => {}
        _ => {
            return Err(CustodyError::IdentityDrift {
                op: "publication recheck",
            }
            .into());
        }
    }
    Ok(())
}
