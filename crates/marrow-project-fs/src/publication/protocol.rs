//! The publication state machine: preflight, claim, install, settle, recover.
//!
//! One driver serves both entries. A fresh publication builds the header from
//! the admitted plan and drives from `Prepared`; recovery decodes the header a
//! crashed publication already made durable and drives from whatever phase the
//! journal reached. Neither enumerates a directory, and neither mutates a byte
//! of a state outside the closed map.

use std::fmt;

use marrow_fs_journal::{
    AdmittedDir, BuiltHeader, ClaimRefusal, CustodyError, EntryName, EntryStat, FsIdentity,
    JournalKind, LiveJournal, MarkerStats, OpenedFile, PendingState, PhaseRecord, TailState, claim,
    classify, encode_record,
};
use marrow_project::{LedgerExpectedArtifact, LedgerPublicationPlan, LedgerPublicationView};

use super::header::{PlannedHeader, RowHeader};
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

/// Which of the two artifact slots a byte run failed its comparison for.
/// Named for the slot rather than a role, because a *role* in this module is
/// which bound run an entry carries; these are the two names it can carry it
/// under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArtifactSlot {
    /// The committed `.marrow/ids`.
    Ledger,
    /// The staged successor.
    Stage,
}

impl fmt::Display for ArtifactSlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Ledger => "the committed ledger",
            Self::Stage => "the staged successor",
        })
    }
}

/// Which run the header binds a present entry to, resolved from the bound
/// inode number and the bound byte run together.
///
/// [`marrow_fs_journal::FsIdentity`] carries why a number alone cannot answer
/// this across a crash. The header is what makes the answer available here: it
/// records both exact byte runs, durable before the claim, so a resolution has
/// evidence that outlived every descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundRole {
    /// The staged successor: the bound number under the bound run.
    Next,
    /// The displaced generation: the bound number under the bound run.
    Base,
    /// A bound number under some other run — a recycled number, or an entry
    /// rewritten in place. It is neither bound run, and it is not a stranger
    /// either, so no arm may read it as one.
    Drifted,
    /// Neither bound number.
    Foreign,
}

impl BoundRole {
    /// Whether this resolution names a bound run at all.
    const fn is_bound_run(self) -> bool {
        matches!(self, Self::Next | Self::Base)
    }
}

/// Proof that the quarantine name has been reconciled against a
/// publication's own header and recorded terminal.
///
/// Reading the artifact map while an interrupted removal still holds an object
/// misreads the map: the name that object came from is absent, so a state with
/// a removal still owed looks like one already finished. The proof is produced
/// only by [`Session::reconcile`] and required by [`Session::read_map`] — every
/// map reading in this module, not the driver alone — so no path can reach one
/// without having reconciled first. Forgetting is a compile error rather than a
/// defect found in review.
///
/// The header-less callers — the arms that read the stage with no publication
/// to judge it against — reconcile without producing one, because they read no
/// map.
#[must_use = "the driver consumes this proof; producing it and dropping it reconciles nothing"]
pub(super) struct Reconciled(());

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
        /// Which slot's run failed.
        slot: ArtifactSlot,
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
            Self::BytesDrift { slot } => write!(formatter, "{slot} is not the exact bound run"),
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

/// The closed artifact map, read from name-to-entry resolutions and link
/// counts. Each present entry resolves against the runs the header bound —
/// inode number and exact bytes together — so no state here rests on a number
/// a crash may have left recycled. Each phase admits its own subset; every
/// other reading is retained corruption.
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
    // The journal classification below reads the two marker names only, so an
    // interrupted removal cannot affect it. Reconciliation therefore happens
    // after it, where the header and the recorded terminal are in hand and the
    // object in quarantine can be judged by the rule that authorized its
    // removal. The arms that read the stage without a header reconcile on their
    // own terms, where no terminal exists for a restore to contradict.
    let names = guard.journal_names();
    // The shape is bound rather than consumed, so the identity the header is
    // checked against is the one this classification acted on. Statting the
    // marker name a second time could witness a different object than the one
    // that was classified.
    let shape = classify(MarkerStats::read(meta, names.markers())?);
    let marker_witness = shape.marker_identity();
    match shape.admit(meta, names, JournalKind::Ids)? {
        PendingState::Absent => {
            reconcile_quarantine(guard, guard.stage_name())?;
            require_clear_stage(guard)?;
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
            let header = admit_header(meta, claimed.frame().row_header(), marker_witness)?;
            let reconciled = reconcile_for(guard, &header, None)?;
            let mut session = Session::new(guard, header, claimed.adopt()?, None);
            session.drive(&reconciled).map(Some)
        }
        PendingState::Pending(mut pending) => {
            let header = admit_header(meta, pending.frame().row_header(), marker_witness)?;
            let terminal = terminal_of(pending.frame().records())?;
            // Before the tail derivation below, which reads the artifact map
            // itself and would otherwise read it across an interrupted removal.
            let reconciled = reconcile_for(guard, &header, terminal)?;
            if let TailState::IncompletePrefix { .. } = pending.frame().tail() {
                let last = last_tag(pending.frame().records());
                let expected = expected_next_record(guard, &header, last, &reconciled)?;
                pending.truncate_tail(&expected)?;
            }
            let mut session = Session::new(guard, header, pending.resume()?, terminal);
            session.drive(&reconciled).map(Some)
        }
    }
}

// ===== Fresh publication =====================================================

/// Refuse before anything is staged when the project already carries a
/// publication state. A fresh publication starts only from a clear one.
///
/// The journal is classified first, and a project carrying a durable claim is
/// refused before anything is reconciled. That ordering is the point: a
/// recorded terminal is the only thing that makes an object at the quarantine
/// name judgeable, and only recovery has the header to judge it with. A
/// publication that reconciled first would put that object back under the stage
/// name, manufacturing a map the terminal record contradicts and leaving a
/// state the later recovery could no longer settle. Refusing touches nothing
/// and leaves the judgement to the owner that can make it.
///
/// Reconciliation therefore happens only where no marker exists, so no terminal
/// exists either and a restore can contradict nothing.
fn preflight(guard: &ProjectMetadataWriteGuard) -> Result<(), IdsPublicationError> {
    let meta = guard.meta();
    match classify(MarkerStats::read(meta, guard.journal_names().markers())?).admit(
        meta,
        guard.journal_names(),
        JournalKind::Ids,
    )? {
        PendingState::Absent => {}
        PendingState::Preclaim(_) => {
            return Err(IdsPublicationError::bare(IdsRefusal::UnclaimedIncomplete));
        }
        PendingState::Claimed(_) | PendingState::Pending(_) => {
            return Err(IdsPublicationError::bare(IdsRefusal::Interrupted));
        }
        PendingState::Corrupt(reason) => return Err(IdsPublicationError::corrupt(reason)),
    }
    reconcile_quarantine(guard, guard.stage_name())?;
    require_clear_stage(guard)
}

/// Refuse a stage entry left behind with no publication to own it. Reached
/// only where the journal names say nothing is claimed, so an entry there is a
/// manual state an operator clears rather than one recovery can settle.
fn require_clear_stage(guard: &ProjectMetadataWriteGuard) -> Result<(), IdsPublicationError> {
    if guard.meta().stat_entry(guard.stage_name())?.is_some() {
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

    // The creating descriptor is held across the claim. It keeps the staged
    // inode number out of circulation, so the cleanup below judges the object
    // it created rather than a number an outside writer could have been handed
    // after an unlink.
    let stage = stage_successor(guard, next)?;
    let stage_identity = stage.identity();

    // The plan decides every byte of the header but the two identities only
    // the claim can witness, so the whole of it is built here, before the
    // claim, and handed over as a value. The claim runs no code of this
    // module's while it is establishing the marker.
    let planned = PlannedHeader {
        base,
        next_inode: stage_identity,
        base_bytes: observed
            .as_ref()
            .map(|(_, bytes)| bytes.clone())
            .unwrap_or_default(),
        next_bytes: next.to_vec(),
    };
    let claimed = claim(
        meta,
        guard.journal_names(),
        JournalKind::Ids,
        planned.build(),
        &[],
    );
    let journal = match claimed {
        Ok(journal) => journal,
        Err(refusal) => {
            // The claim names which arm it refused on, so this one does not
            // guess. A preclaim refusal issued no link, so no marker can
            // exist: the stage this call created is removed and the cause
            // returned. A possibly-durable one may have left a marker, and the
            // contract for a marker that may exist is the affine pending value
            // — removing the stage there could leave a marker with no
            // successor to resume from, and returning a bare error would drop
            // the guard borrow and the drop glue that keeps this process
            // honest about it.
            match refusal {
                ClaimRefusal::Preclaim(error) => {
                    // The cause that stopped the claim outranks a refusal from
                    // the cleanup that follows it.
                    let _ = discard_stage(guard, &stage);
                    return Err(error.into());
                }
                ClaimRefusal::PossiblyDurable(error) => {
                    return Ok(IdsPublishOutcome::Pending(Box::new(
                        IdsPublicationPending::unclaimed(guard, error.into(), None),
                    )));
                }
            }
        }
    };
    let header = RowHeader::from_planned(planned, journal.witness());

    // From here the marker is durable: every interruption is affine, including
    // a reconciliation that refuses before the driver ever runs.
    let mut session = Session::new(guard, header, journal, None);
    let reconciled = match session.reconcile() {
        Ok(reconciled) => reconciled,
        Err(cause) => return Ok(interrupted(guard, session, cause)),
    };
    match session.drive(&reconciled) {
        Ok(publication) => Ok(IdsPublishOutcome::Settled(publication)),
        Err(cause) => Ok(interrupted(guard, session, cause)),
    }
}

/// The pending outcome for a publication interrupted after its claim.
///
/// Three shapes, and the third is the one a finish leaves. A session that still
/// holds its journal is handed back to be resumed through it. One whose journal
/// a refused finish already consumed can resume nothing, so it is dropped and
/// what remains on disk decides: a marker still present is recovery's to adopt,
/// and a marker already unlinked means the finish got far enough that the
/// publication happened — its closing checks or its final sync are what
/// refused. Recovery reads markers, so with none present it would report a
/// completed publication as one that was never claimed. The terminal this
/// session recorded is carried along for exactly that case.
fn interrupted<'a>(
    guard: &'a ProjectMetadataWriteGuard,
    session: Session<'a>,
    cause: IdsPublicationError,
) -> IdsPublishOutcome<'a> {
    if !session.is_spent() {
        return IdsPublishOutcome::Pending(Box::new(IdsPublicationPending::new(session, cause)));
    }
    IdsPublishOutcome::Pending(Box::new(IdsPublicationPending::unclaimed(
        guard,
        cause,
        session.recorded(),
    )))
}

impl PlannedHeader {
    /// The header as the claim takes it: the generation slot and the bytes
    /// after the common, neither of which depends on a claim existing.
    fn build(&self) -> BuiltHeader {
        BuiltHeader::Witnessed {
            generation: self.generation(),
            tail: self.encode_tail(),
        }
    }
}

/// Create, fill, sync, and validate the fixed stage entry, returning the
/// descriptor that created it. Any refusal removes the stage under witness
/// first.
///
/// The descriptor is returned rather than dropped for its identity: it is what
/// keeps the staged inode number out of circulation, so every later judgement
/// about "the object this call created" is made against the object and not
/// against a number that an unlink could have freed for reuse.
fn stage_successor(
    guard: &ProjectMetadataWriteGuard,
    next: &[u8],
) -> Result<OpenedFile, IdsPublicationError> {
    let meta = guard.meta();
    let name = guard.stage_name();
    let mut file = meta.create_file_excl(name)?;
    let identity = file.identity();
    let filled = (|| -> Result<(), IdsPublicationError> {
        file.append(next)?;
        file.sync()?;
        if file.read_prefix(LEDGER_BYTE_CEILING + 1)? != next {
            return Err(MapFault::BytesDrift {
                slot: ArtifactSlot::Stage,
            }
            .into());
        }
        meta.sync()?;
        require_entry(meta, name, identity, 1, next.len())?;
        Ok(())
    })();
    match filled {
        Ok(()) => Ok(file),
        Err(refusal) => {
            // As above: the cause that refused the stage outranks a refusal
            // from the cleanup that follows it.
            let _ = discard_stage(guard, &file);
            Err(refusal)
        }
    }
}

/// Remove the stage entry this call created and make its absence durable.
///
/// The proof is the creating descriptor. While it lives the inode number it
/// holds cannot be handed to anything else, so within this call's own lifetime
/// an object carrying that number is the object this call created. That is the
/// one place in this protocol where a number alone is conclusive, and it is
/// conclusive only because the descriptor is still open.
fn discard_stage(
    guard: &ProjectMetadataWriteGuard,
    created: &OpenedFile,
) -> Result<(), IdsPublicationError> {
    remove_validated(guard, guard.stage_name(), "stage discard", |held| {
        Ok(held.identity() == created.identity())
    })
}

/// Remove the object at `name`, and only after proving through a descriptor on
/// that object that `accepts` admits it.
///
/// The unlink cannot itself be the validated act. `unlinkat` names a path, and
/// neither qualified platform offers an unlink through a descriptor, so between
/// any validation and any unlink the name can be repointed and the wrong object
/// deleted. What this owner does is move the object to a name no cooperating
/// writer ever touches before judging it: every name here but the ledger is a
/// protocol transient, gate-enforced never-tracked, and a cooperating Git
/// operation writes tracked paths only. A writer that reaches the quarantine
/// name is one deliberately writing untracked protocol names, which is outside
/// the cooperative contract. See [`marrow_fs_journal::FsIdentity`] for the
/// bound that leaves.
///
/// The move is a rename within one directory, which is the atomicity this
/// platform qualification already rests on everywhere else in the protocol, so
/// it adds no filesystem property to the envelope.
///
/// An object `accepts` rejects is never deleted: it is moved back under the
/// name it came from. A restore that cannot land leaves it at the quarantine
/// name, which the next command reconciles.
fn remove_validated(
    guard: &ProjectMetadataWriteGuard,
    name: &EntryName,
    op: &'static str,
    accepts: impl Fn(&OpenedFile) -> Result<bool, IdsPublicationError>,
) -> Result<(), IdsPublicationError> {
    let meta = guard.meta();
    let held = guard.quarantine_name();
    match meta.rename_noreplace(name, held) {
        Ok(()) => meta.sync()?,
        // Already gone: the absence this call wanted is the absence it found.
        Err(CustodyError::NotFound { .. }) => return Ok(()),
        Err(error) => return Err(error.into()),
    }

    let verdict = (|| -> Result<bool, IdsPublicationError> {
        let file = meta.open_file_readonly(held)?;
        accepts(&file)
    })();
    match verdict {
        Ok(true) => {
            meta.unlink(held)?;
            meta.sync()?;
            Ok(())
        }
        Ok(false) => restore_quarantined(meta, held, name)
            .and(Err(CustodyError::IdentityDrift { op }.into())),
        Err(refusal) => restore_quarantined(meta, held, name).and(Err(refusal)),
    }
}

/// Put whatever an interrupted removal left at the quarantine name back under
/// `name`, with no publication to judge it against.
///
/// This is the header-less reconciliation: the callers that reach it have no
/// recorded terminal, so no cleanup was authorized and nothing here may finish
/// one. [`Session::reconcile`] is the form that runs with a terminal in hand.
fn reconcile_quarantine(
    guard: &ProjectMetadataWriteGuard,
    name: &EntryName,
) -> Result<(), IdsPublicationError> {
    let meta = guard.meta();
    let held = guard.quarantine_name();
    if meta.stat_entry(held)?.is_none() {
        return Ok(());
    }
    restore_quarantined(meta, held, name)
}

/// Move a quarantined object back under the name it came from. It is not this
/// protocol's object to delete, so a destination that has since been taken
/// refuses rather than being overwritten, and the object is left where it is.
fn restore_quarantined(
    meta: &AdmittedDir,
    held: &EntryName,
    name: &EntryName,
) -> Result<(), IdsPublicationError> {
    meta.rename_noreplace(held, name)?;
    meta.sync()?;
    Ok(())
}

// ===== The driver ============================================================

impl<'a> Session<'a> {
    /// A session with no live journal, for the readings that run before one is
    /// resumed — the tail derivation and the reconciliation beside it. It can
    /// read the artifact map and reconcile; it cannot drive.
    fn probe(
        guard: &'a ProjectMetadataWriteGuard,
        header: RowHeader,
        terminal: Option<Terminal>,
    ) -> Self {
        Self {
            guard,
            header,
            terminal,
            journal: None,
        }
    }

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
    pub(super) fn drive(
        &mut self,
        reconciled: &Reconciled,
    ) -> Result<IdsPublication, IdsPublicationError> {
        loop {
            match self.phase() {
                1 => {
                    self.require_pre_mutation_map(reconciled)?;
                    self.append(INSTALLING, &[])?;
                }
                INSTALLING => {
                    let terminal = self.install(reconciled)?;
                    self.append(SETTLED, &[terminal.payload()])?;
                    self.terminal = Some(terminal);
                }
                SETTLED => return self.settle(reconciled),
                found => return Err(MapFault::UnknownPhase { found }.into()),
            }
        }
    }

    /// Whether this session's journal is gone.
    ///
    /// `settle` takes the journal out to consume it, so a refusal from the
    /// finish it performs leaves a session that can drive nothing: every phase
    /// read expects the journal it no longer has. Such a session must not be
    /// handed back as something to retry.
    pub(super) fn is_spent(&self) -> bool {
        self.journal.is_none()
    }

    /// The publication this session's own recorded terminal names, if it
    /// reached one.
    ///
    /// A finish removes the marker before its closing checks, so a refusal
    /// there can leave no marker at all. Recovery reads a marker, so it has
    /// nothing to read — but the publication it was finishing did happen, and
    /// this is what says which one. Without it a completed publication would be
    /// reported as a state that was never claimed.
    pub(super) fn recorded(&self) -> Option<IdsPublication> {
        self.terminal.map(Terminal::publication)
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

    /// The window before any artifact mutation: this publication has created
    /// its own stage and touched nothing else.
    ///
    /// Two readings are admitted. `Prepared` is the state the header binds,
    /// exact byte runs included: the map resolves every entry against the runs
    /// the header carries, so reaching this reading is already the comparison.
    /// `Reverted` — the stage still the exact successor at one link, the
    /// artifact neither the successor nor the generation the header binds —
    /// cannot be this publication's own work, because no mutation of the
    /// artifact has run. It is the outside writer the destination-refusing arms
    /// exist for, and the same fact the pre-claim recapture in
    /// `publish_admitted` reports as a concurrent change. The `Installing`
    /// window reads that writer identically, so which record an ordinary
    /// `git checkout` lands between does not decide whether the project settles
    /// or is retained.
    ///
    /// Admitting a reading names no terminal. The driver appends `Installing`
    /// and the map is read again there, where `terminal_of_map` decides; that
    /// window admits one reading more than this one, because the mutation
    /// between them can leave the successor installed.
    ///
    /// Every other reading is retained. An installed artifact names a mutation
    /// that has not run. An absent stage — which an outside writer sweeping the
    /// ignored transient can also produce — leaves nothing this publication may
    /// settle on or clean, and the `Installing` window refuses it too.
    fn require_pre_mutation_map(&self, reconciled: &Reconciled) -> Result<(), IdsPublicationError> {
        match self.read_map(1, reconciled)? {
            MapState::Prepared | MapState::Reverted => Ok(()),
            _ => Err(MapFault::OffMap { phase: 1 }.into()),
        }
    }

    /// Install the successor, or settle without installing it. The map decides:
    /// a crash between the `Installing` record and the mutation leaves the
    /// `Prepared` reading, and a crash after it leaves the installed reading.
    /// The two readings that reach here unmutated are the ones
    /// [`Self::require_pre_mutation_map`] admits, classified the same way.
    fn install(&self, reconciled: &Reconciled) -> Result<Terminal, IdsPublicationError> {
        match self.read_map(INSTALLING, reconciled)? {
            MapState::Prepared => match self.header.base {
                None => self.link_absent(reconciled),
                Some(base) => self.exchange_replace(base, reconciled),
            },
            settled => terminal_of_map(settled),
        }
    }

    /// Re-read the artifact map and name the terminal it settled into.
    ///
    /// The mutations call this instead of deciding a terminal from their own
    /// outcome: a refused destination and a returned exchange each leave a map,
    /// and the map is what recovery would read after a crash at the same point.
    fn terminal_from_map(&self, reconciled: &Reconciled) -> Result<Terminal, IdsPublicationError> {
        terminal_of_map(self.read_map(INSTALLING, reconciled)?)
    }

    /// The absent arm: one destination-refusing hard link. A refused
    /// destination means the artifact this plan was admitted against as absent
    /// now exists, so the publication settles without installing.
    fn link_absent(&self, reconciled: &Reconciled) -> Result<Terminal, IdsPublicationError> {
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
                self.terminal_from_map(reconciled)
            }
            // The artifact this plan was admitted against as absent now
            // exists, so the map — not this refusal — names the terminal. The
            // writer that put it there is outside the guard, because no
            // cooperating Marrow process can hold the write lock at the same
            // time. `.marrow/ids` is a committed file, so an ordinary `git
            // checkout`, `stash pop`, or `pull` landing between the map read
            // and this link is what reaches here.
            Err(CustodyError::AlreadyExists { .. }) => self.terminal_from_map(reconciled),
            Err(error) => Err(error.into()),
        }
    }

    /// The replace arm: one atomic exchange. The displaced generation lands at
    /// the stage name; anything else there is a third live inode that this
    /// process — and only this process, which has just proven the pre-exchange
    /// reading — may exchange back. Unlike the ledger, the stage name is one of
    /// the transients the write owner's ignore entry covers, so an ordinary Git
    /// operation neither tracks nor recreates it. The owner refuses to acquire
    /// a project whose ignore entry it cannot establish that for, so the only
    /// remaining way the name is tracked is one no library check can see: it
    /// was already in the index before this contract existed, or was added with
    /// `git add -f`. Reaching the exchange-back arm takes a writer editing the
    /// metadata directory outside the lock between this exchange and the stat
    /// below.
    fn exchange_replace(
        &self,
        base: FsIdentity,
        reconciled: &Reconciled,
    ) -> Result<Terminal, IdsPublicationError> {
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
            return self.terminal_from_map(reconciled);
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
                self.terminal_from_map(reconciled)
            }
            _ => Err(MapFault::ExchangeUncertain.into()),
        }
    }

    /// Clean the exact stage alias, prove the terminal reading, then unlink the
    /// marker and sync. Only that final directory sync ends the publication.
    fn settle(&mut self, reconciled: &Reconciled) -> Result<IdsPublication, IdsPublicationError> {
        let terminal = self.terminal.ok_or(MapFault::MissingTerminal)?;
        let map = self.read_map(SETTLED, reconciled)?;
        match (terminal, map) {
            (Terminal::Installed, MapState::Installed)
            | (Terminal::Reverted, MapState::Reverted) => {
                self.clean_stage(self.cleanup_role(terminal))?;
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

    /// Which bound run the cleanup a recorded terminal authorizes is entitled
    /// to remove from the stage name. The settle path and the reconciliation
    /// that resumes an interrupted settle read it from here, so the object a
    /// crash left mid-removal is judged by the same rule that authorized the
    /// removal.
    fn cleanup_role(&self, terminal: Terminal) -> BoundRole {
        match terminal {
            Terminal::Installed => self.displaced_role(),
            Terminal::Reverted => BoundRole::Next,
        }
    }

    /// Reconcile the quarantine with this publication's header and recorded
    /// terminal in hand.
    ///
    /// A blind restore is wrong here. When a terminal is already recorded, the
    /// cleanup it authorizes was running when the crash landed, and the object
    /// at the quarantine name is the one that cleanup had taken. Putting it
    /// back manufactures a pre-cleanup map the terminal record contradicts —
    /// and the target is independently mutable, so an outside writer can make
    /// that contradiction permanent. Finishing the removal instead is both what
    /// the terminal already committed to and the only reading that cannot be
    /// invalidated by what happened at the other name.
    ///
    /// Only an object resolving to the exact run that cleanup was entitled to
    /// remove is removed. Anything else is put back, and an object under no
    /// recorded terminal is put back too: no cleanup was authorized, so nothing
    /// here may finish one.
    pub(super) fn reconcile(&self) -> Result<Reconciled, IdsPublicationError> {
        let meta = self.meta();
        let held = self.guard.quarantine_name();
        let name = self.guard.stage_name();
        if meta.stat_entry(held)?.is_none() {
            return Ok(Reconciled(()));
        }
        let entitled = match self.terminal {
            Some(terminal) => {
                let file = meta.open_file_readonly(held)?;
                self.role_of_open(&file)? == self.cleanup_role(terminal)
            }
            None => false,
        };
        if entitled {
            meta.unlink(held)?;
            meta.sync()?;
        } else {
            restore_quarantined(meta, held, name)?;
        }
        Ok(Reconciled(()))
    }

    /// The run the stage name carries once the successor is installed: the
    /// displaced generation, or the successor itself when the plan displaced
    /// nothing and the two names are one inode.
    fn displaced_role(&self) -> BoundRole {
        if self.header.base.is_some() {
            BoundRole::Base
        } else {
            BoundRole::Next
        }
    }

    /// Unlink the stage alias, and only after the object there proves — through
    /// a descriptor on it — to carry the run the terminal says it carries. No
    /// descriptor survives the crash this cleanup may be resuming after, so the
    /// bound number and the bound run together are the whole proof available.
    fn clean_stage(&self, expected: BoundRole) -> Result<(), IdsPublicationError> {
        remove_validated(
            self.guard,
            self.guard.stage_name(),
            "stage cleanup",
            |held| Ok(self.role_of_open(held)? == expected),
        )
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
                ArtifactSlot::Ledger,
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

    /// The one resolver: which bound run an object of this identity carries,
    /// given a reader that answers whether it carries a candidate run.
    ///
    /// Named apart from the journal's `classify`, which answers an unrelated
    /// question about the marker pair.
    fn resolve_role(
        &self,
        identity: FsIdentity,
        carries: impl Fn(&[u8]) -> Result<bool, IdsPublicationError>,
    ) -> Result<BoundRole, IdsPublicationError> {
        if identity == self.header.next_inode {
            return Ok(if carries(&self.header.next_bytes)? {
                BoundRole::Next
            } else {
                BoundRole::Drifted
            });
        }
        if self.header.base == Some(identity) {
            return Ok(if carries(&self.header.base_bytes)? {
                BoundRole::Base
            } else {
                BoundRole::Drifted
            });
        }
        Ok(BoundRole::Foreign)
    }

    /// Resolve a present entry named in the map.
    ///
    /// The size comes from the stat already in hand, so an entry that cannot be
    /// a bound run is answered without an open; only a size-matching candidate
    /// is read, and the read is bounded by the ledger ceiling the header's own
    /// runs are bounded by. The read reopens by name after the stat, so this
    /// resolution rests on the bytes it returns rather than on a descriptor
    /// held across both — enough for a classification that authorizes no
    /// removal, and the reason every removal resolves through
    /// [`Self::role_of_open`] instead.
    fn role_of(
        &self,
        name: &EntryName,
        stat: &EntryStat,
    ) -> Result<BoundRole, IdsPublicationError> {
        self.resolve_role(stat.identity(), |run| {
            if stat.size() != run.len() as u64 {
                return Ok(false);
            }
            Ok(
                matches!(read_entry(self.meta(), name)?, Some((identity, bytes))
                    if identity == stat.identity() && bytes == run),
            )
        })
    }

    /// Resolve an object through a descriptor already open on it. The number,
    /// the size, and the bytes all come from that one descriptor, so nothing
    /// can substitute the object between the question and the answer. This is
    /// the resolution a removal is entitled to act on.
    fn role_of_open(&self, file: &OpenedFile) -> Result<BoundRole, IdsPublicationError> {
        let size = file.stat()?.size();
        self.resolve_role(file.identity(), |run| {
            if size != run.len() as u64 {
                return Ok(false);
            }
            Ok(file.read_prefix(LEDGER_BYTE_CEILING + 1)? == run)
        })
    }

    /// Read the closed artifact map from name-to-entry resolutions and link
    /// counts. Each present entry is resolved to the run the header bound it
    /// to — inode number and exact bytes together, never the number alone —
    /// because a number is not an identity across the crash window. `phase`
    /// names the reader for an off-map refusal and is supplied rather than read
    /// from the journal, because the tail-derivation probe reads the map with
    /// no live journal.
    fn read_map(
        &self,
        phase: u8,
        _reconciled: &Reconciled,
    ) -> Result<MapState, IdsPublicationError> {
        let ledger = self.guard.ledger_name();
        let stage = self.guard.stage_name();
        let target = self.stat(ledger)?;
        let staged = self.stat(stage)?;
        let target_role = match &target {
            Some(stat) => Some(self.role_of(ledger, stat)?),
            None => None,
        };
        let staged_role = match &staged {
            Some(stat) => Some(self.role_of(stage, stat)?),
            None => None,
        };
        let target_next = target_role == Some(BoundRole::Next);
        let target_base = target_role == Some(BoundRole::Base);
        let staged_next = staged_role == Some(BoundRole::Next);
        let staged_base = staged_role == Some(BoundRole::Base);
        // Present, but naming neither bound run: a stranger, or a bound number
        // under some other run.
        let target_neither = target_role.is_some_and(|role| !role.is_bound_run());
        let staged_neither = staged_role.is_some_and(|role| !role.is_bound_run());

        match (target, staged, self.header.base) {
            // Prepared, absent arm: the artifact this plan displaces nothing of
            // is still absent and the successor is staged.
            (None, Some(staged), None) if staged_next && staged.nlink() == 1 => {
                Ok(MapState::Prepared)
            }
            // Prepared, replace arm: the bound generation is still committed.
            (Some(target), Some(staged), Some(_))
                if target_base && target.nlink() == 1 && staged_next && staged.nlink() == 1 =>
            {
                Ok(MapState::Prepared)
            }
            // Installed, absent arm: one inode under both names.
            (Some(target), Some(_), None) if target_next && staged_next && target.nlink() == 2 => {
                Ok(MapState::Installed)
            }
            // Installed, replace arm: the displaced generation sits at the
            // stage name.
            (Some(_), Some(_), Some(_)) if target_next && staged_base => Ok(MapState::Installed),
            // The one state a dead process cannot be given the benefit of: the
            // successor is installed and the stage holds neither the displaced
            // generation nor the successor itself, so a third inode is live.
            // Any other unexpected reading beside an installed successor is
            // off-map rather than a third inode.
            (Some(_), Some(_), _) if target_next && staged_neither => {
                Err(MapFault::ThirdInode.into())
            }
            (Some(target), None, _) if target_next && target.nlink() == 1 => {
                Ok(MapState::InstalledClean)
            }
            // Reverted: the successor is still only staged and the artifact is
            // neither it nor the generation the plan bound.
            (Some(_), Some(staged), _) if staged_next && staged.nlink() == 1 && target_neither => {
                Ok(MapState::Reverted)
            }
            // The same reading with the stage already swept — including by the
            // cleanup this protocol runs itself, whose crash window leaves
            // exactly this state. A target naming neither bound run is admitted
            // here: which way the publication settled is what the terminal
            // record says, and `settle` pairs the two, so this reading needs
            // only to be one no mutation can still be owed. Nothing is unlinked
            // on it.
            (Some(_), None, _) if !target_next => Ok(MapState::RevertedClean),
            _ => Err(MapFault::OffMap { phase }.into()),
        }
    }

    fn require_bytes(
        &self,
        name: &EntryName,
        expected: &[u8],
        slot: ArtifactSlot,
    ) -> Result<(), IdsPublicationError> {
        match read_entry(self.meta(), name)? {
            Some((_, bytes)) if bytes == expected => Ok(()),
            _ => Err(MapFault::BytesDrift { slot }.into()),
        }
    }
}

// ===== Recovery helpers ======================================================

/// Decode a durable header and require its own witnesses: the directory it was
/// claimed under and the marker inode it was written into. A header that
/// witnesses another directory or another inode is substituted evidence.
///
/// Both comparisons are numbers recorded before a crash against numbers read
/// after it, and neither is descriptor-backed for the epoch it names: the
/// descriptors this process holds were opened after the crash, so they pin the
/// numbers only from that point on. The comparison is still worth making — it
/// refuses a header carried into another project or another marker — but it
/// establishes that the numbers agree, not that the objects are the ones the
/// crashed process saw. What makes the header safe to act on is that the states
/// it authorizes are resolved against its byte runs, not that these two numbers
/// matched. See [`marrow_fs_journal::FsIdentity`] for the bound.
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
    reconciled: &Reconciled,
) -> Result<Vec<u8>, IdsPublicationError> {
    let terminal = match last {
        1 => return Ok(record(1, INSTALLING, &[])),
        INSTALLING => pending_terminal(guard, header, reconciled)?,
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
    reconciled: &Reconciled,
) -> Result<Terminal, IdsPublicationError> {
    let probe = Session::probe(guard, header.clone(), None);
    probe.terminal_from_map(reconciled)
}

/// Reconcile with a header and recorded terminal but no live journal.
///
/// Tail derivation reads the artifact map before the journal is resumed, so it
/// needs the same proof the driver does and the same judgement behind it. The
/// probe carries no journal, exactly as [`pending_terminal`]'s does.
fn reconcile_for(
    guard: &ProjectMetadataWriteGuard,
    header: &RowHeader,
    terminal: Option<Terminal>,
) -> Result<Reconciled, IdsPublicationError> {
    Session::probe(guard, header.clone(), terminal).reconcile()
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
            slot: ArtifactSlot::Ledger,
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

#[cfg(test)]
mod tests {
    use marrow_fs_journal::{encode_header, encode_record};

    use super::*;
    use crate::publication::IdsPublication;

    /// A scratch project with a write owner, removed when the test ends.
    ///
    /// Teardown is a `Drop` guard rather than trailing statements, so a failing
    /// assertion leaves no directory behind for the next run to trip over. It
    /// removes only the fixed names this owner writes, because the owner
    /// enumerates no directory and neither does its fixture.
    struct Scratch {
        root: std::path::PathBuf,
        guard: Option<ProjectMetadataWriteGuard>,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "marrow-idpub01-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock after epoch")
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).expect("create the scratch project");
            let guard = ProjectMetadataWriteGuard::acquire(&root).expect("the write owner");
            Self {
                root,
                guard: Some(guard),
            }
        }

        fn guard(&self) -> &ProjectMetadataWriteGuard {
            self.guard.as_ref().expect("the guard lives until drop")
        }

        fn meta_path(&self) -> std::path::PathBuf {
            self.root.join(marrow_project::META_DIR)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let names: Vec<String> = {
                let guard = self.guard();
                [
                    guard.ledger_name().as_str(),
                    guard.stage_name().as_str(),
                    guard.quarantine_name().as_str(),
                    guard.journal_names().claim().as_str(),
                    guard.journal_names().pending().as_str(),
                    crate::publication::LOCK_NAME,
                    crate::publication::ignore::IGNORE_NAME,
                ]
                .iter()
                .map(|name| (*name).to_owned())
                .collect()
            };
            let meta = self.meta_path();
            self.guard = None;
            for name in names {
                std::fs::remove_file(meta.join(name)).ok();
            }
            std::fs::remove_dir(&meta).ok();
            std::fs::remove_dir(&self.root).ok();
        }
    }

    /// The shape a finish leaves once it has already removed the marker.
    ///
    /// `finish` unlinks the pending name and then performs its closing checks
    /// and its final directory sync. A refusal from those leaves no journal in
    /// the session and no marker on disk, but the publication itself happened:
    /// the artifact is installed and the marker is gone. Recovery reads
    /// markers, so it finds nothing and can only say that nothing was ever
    /// claimed. The terminal this session recorded is what makes the difference
    /// between reporting the publication and reporting a project that never
    /// published.
    #[test]
    fn a_finish_that_already_removed_the_marker_reports_the_terminal_it_recorded() {
        let scratch = Scratch::new("postunlink");
        let guard = scratch.guard();

        let meta_path = scratch.meta_path();
        let ledger_path = meta_path.join(guard.ledger_name().as_str());
        std::fs::write(&ledger_path, b"successor").expect("the installed successor");
        let next_inode = guard
            .meta()
            .stat_entry(guard.ledger_name())
            .expect("stat the successor")
            .expect("the successor exists")
            .identity();

        // Exactly what a finish leaves after its unlink: no marker anywhere,
        // and a session holding the terminal it recorded before removing it.
        let spent = Session::probe(
            guard,
            RowHeader {
                parent: guard.meta().identity(),
                journal_inode: FsIdentity::new(0, 0),
                base: None,
                next_inode,
                base_bytes: Vec::new(),
                next_bytes: b"successor".to_vec(),
            },
            Some(Terminal::Installed),
        );
        assert!(spent.is_spent());
        assert!(
            guard
                .meta()
                .stat_entry(guard.journal_names().pending())
                .expect("stat the marker name")
                .is_none(),
            "the fixture is the post-unlink shape",
        );

        let outcome = interrupted(guard, spent, IdsPublicationError::bare(IdsRefusal::Custody));
        let IdsPublishOutcome::Pending(pending) = outcome else {
            panic!("an interrupted publication is pending");
        };
        let settled = pending.recover().expect(
            "a publication whose marker the finish already removed is reported, not \
             called unclaimed",
        );
        assert_eq!(settled, IdsPublication::Published);
    }

    /// A session whose journal a refused finish already consumed must not be
    /// offered back as something to drive.
    ///
    /// The value this returns is what a caller consumes to try again, and the
    /// first thing a retry does is read a phase from the journal. A spent
    /// session has none, so handing one back offers a retry that cannot run.
    /// The marker it was finishing is still on disk, which is what the arm this
    /// routes to adopts — so the retry settles instead.
    #[test]
    fn a_spent_session_is_offered_as_a_marker_to_recover_not_a_journal_to_drive() {
        let scratch = Scratch::new("spent");
        let guard = scratch.guard();

        // An installed successor at one link with the stage already cleaned:
        // the reading a settled terminal finishes over without mutating
        // anything, so recovery reaches the marker unlink and nothing else.
        // Every spelling comes from the owners the gates protect: this module
        // names no entry itself, in test code either.
        let meta_path = scratch.meta_path();
        let ledger_path = meta_path.join(guard.ledger_name().as_str());
        std::fs::write(&ledger_path, b"successor").expect("the installed successor");
        let next_inode = guard
            .meta()
            .stat_entry(guard.ledger_name())
            .expect("stat the successor")
            .expect("the successor exists")
            .identity();

        let marker = meta_path.join(guard.journal_names().pending().as_str());
        std::fs::write(&marker, b"").expect("create the marker");
        let journal_inode = guard
            .meta()
            .stat_entry(guard.journal_names().pending())
            .expect("stat the marker")
            .expect("the marker exists")
            .identity();
        let header = RowHeader {
            parent: guard.meta().identity(),
            journal_inode,
            base: None,
            next_inode,
            base_bytes: Vec::new(),
            next_bytes: b"successor".to_vec(),
        };
        let mut bytes =
            encode_header(JournalKind::Ids, &header.encode()).expect("a lawful kind-1 header");
        for (sequence, (tag, payload)) in [
            (1u8, Vec::new()),
            (INSTALLING, Vec::new()),
            (SETTLED, vec![0]),
        ]
        .iter()
        .enumerate()
        {
            bytes.extend_from_slice(
                &encode_record(
                    JournalKind::Ids,
                    u32::try_from(sequence).expect("few records"),
                    *tag,
                    payload,
                )
                .expect("a lawful kind-1 record"),
            );
        }
        std::fs::write(&marker, &bytes).expect("fill the marker");
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600))
                .expect("the durable claim's fixed mode");
        }

        // Exactly what a finish that refused leaves behind: the journal gone
        // from the session, the marker still on disk.
        let spent = Session::probe(guard, header, Some(Terminal::Installed));
        assert!(spent.is_spent(), "the fixture is the state under test");

        let outcome = interrupted(
            guard,
            spent,
            IdsPublicationError::bare(IdsRefusal::Interrupted),
        );
        let IdsPublishOutcome::Pending(pending) = outcome else {
            panic!("an interrupted publication is pending");
        };
        let settled = pending
            .recover()
            .expect("the marker is adopted from disk rather than driven through a missing journal");
        assert_eq!(settled, IdsPublication::Published);
        assert!(!marker.exists(), "the publication finished");
    }
}
