//! The pending-journal protocol end to end: claim, append, replay, exact-prefix
//! truncation, terminal unlink, and the crash-debris matrix. Crash states are
//! kill-point fixtures: each on-disk prefix of the protocol is constructed
//! directly, then reopened and classified through the public API.

use std::os::unix::fs::MetadataExt;

use crate::common::{Scratch, mode_of, require_mode_bits_bind, set_mode};
use marrow_fs_journal::{
    AdmittedDir, BuiltHeader, CacheLock, ClaimRefusal, CorruptionReason, CustodyError, CustodyOp,
    EntryName, EntryNameError, FrameCorruption, FsIdentity, JournalCommon, JournalError,
    JournalKind, LiveJournal, MarkerStats, NodeKind, PendingName, PendingState, TailState, claim,
    classify, encode_header, encode_record,
};

fn name(spelling: &str) -> EntryName {
    EntryName::admit(spelling).expect("test names are admissible")
}

fn root(scratch: &Scratch) -> AdmittedDir {
    AdmittedDir::admit_trusted_root(scratch.path()).expect("admit the scratch root")
}

fn pending_name(base: &str) -> PendingName {
    PendingName::derive(&name(base)).expect("derive pending names")
}

const GENERATION: [u8; 16] = [0x51; 16];
const HEADER_TAIL: [u8; 8] = [0x77; 8];
/// The kind-1 frame ceiling, as the frame known-answer tests freeze it.
const CEILING: usize = 2_101_248;

/// The row header as a caller builds it: the claim composes the leading
/// common from its own witness.
fn header() -> BuiltHeader {
    BuiltHeader::Witnessed {
        generation: GENERATION,
        tail: HEADER_TAIL.to_vec(),
    }
}

/// The row header bytes led by `common`.
fn header_bytes(common: JournalCommon) -> Vec<u8> {
    let mut bytes = common.encode().to_vec();
    bytes.extend_from_slice(&HEADER_TAIL);
    bytes
}

/// The exact bytes a claim writes: the header plus the sequence-zero Prepared
/// record.
fn claim_bytes(common: JournalCommon) -> Vec<u8> {
    let mut bytes = encode_header(JournalKind::Ids, &header_bytes(common)).expect("encode header");
    bytes.extend_from_slice(&encode_record(JournalKind::Ids, 0, 1, b"P").expect("encode Prepared"));
    bytes
}

/// The common a kill-point fixture plants. Its two identities are bytes to the
/// frame law, so a planted journal may carry any.
fn planted_common() -> JournalCommon {
    JournalCommon {
        generation: GENERATION,
        parent: FsIdentity::new(0, 0),
        journal_inode: FsIdentity::new(0, 0),
    }
}

fn planted_claim_bytes() -> Vec<u8> {
    claim_bytes(planted_common())
}

fn claim_ids<'d>(dir: &'d AdmittedDir, names: &PendingName) -> LiveJournal<'d> {
    claim(dir, names, JournalKind::Ids, header(), b"P").expect("claim an ids journal")
}

/// `claim` with its arm named and then dropped. These tests assert which refusal
/// a claim produces; which arm it fell on is asserted by the publication owner's
/// own kats. The match is spelled out rather than converted, so dropping the
/// distinction stays explicit.
fn claim_error<'d>(
    dir: &'d AdmittedDir,
    name: &PendingName,
    header: BuiltHeader,
    prepared_payload: &[u8],
) -> Result<LiveJournal<'d>, JournalError> {
    claim(dir, name, JournalKind::Ids, header, prepared_payload).map_err(|refusal| match refusal {
        ClaimRefusal::Preclaim(error) | ClaimRefusal::PossiblyDurable(error) => error,
    })
}

fn admit<'d>(dir: &'d AdmittedDir, names: &PendingName) -> Result<PendingState<'d>, JournalError> {
    classify(MarkerStats::read(dir, names.markers()).expect("stat the marker pair")).admit(
        dir,
        names,
        JournalKind::Ids,
    )
}

#[test]
fn pending_names_are_derived_and_readmitted() {
    let names = pending_name("store");
    assert_eq!(names.claim().as_str(), "store.pending.create");
    assert_eq!(names.pending().as_str(), "store.pending");

    let long = "a".repeat(250);
    assert_eq!(
        PendingName::derive(&name(&long)),
        Err(EntryNameError::TooLong { len: 265 })
    );
}

#[test]
fn a_claim_publishes_exactly_the_header_and_prepared_record() {
    let scratch = Scratch::new("claim");
    let dir = root(&scratch);
    let names = pending_name("store");

    let live = claim_ids(&dir, &names);
    assert_eq!(live.last_tag(), 1);

    // On disk: the pending name alone, one link, mode 0600, exact bytes.
    assert!(
        !scratch.path().join("store.pending.create").exists(),
        "the claim name is unlinked once the claim completes",
    );
    let written = std::fs::read(scratch.path().join("store.pending")).expect("read pending");
    let common = JournalCommon {
        generation: GENERATION,
        parent: dir.identity(),
        journal_inode: live.witness().journal_inode,
    };
    assert_eq!(written, claim_bytes(common));
    let metadata = std::fs::metadata(scratch.path().join("store.pending")).expect("stat");
    assert_eq!(metadata.nlink(), 1);
    assert_eq!(metadata.mode() & 0o7777, 0o600);
}

#[test]
fn the_claim_witness_carries_the_parent_and_fresh_inode() {
    let scratch = Scratch::new("witness");
    let dir = root(&scratch);
    let names = pending_name("pkg");
    let parent = dir.identity();

    let live = claim_ids(&dir, &names);
    assert_eq!(
        live.witness().parent,
        parent,
        "the claim witnesses the directory it was taken under"
    );
    drop(live);

    let pending_path = scratch.path().join("pkg.pending");
    let written = std::fs::read(&pending_path).expect("read pending");
    let embedded = JournalCommon::decode(written[16..64].try_into().expect("48-byte common"));
    assert_eq!(embedded.generation, GENERATION);
    assert_eq!(embedded.parent, parent);
    assert_eq!(
        embedded.journal_inode,
        identity_of(&pending_path),
        "the header witnesses the created inode",
    );
}

// A header embedding a directory or an inode other than this claim's own is
// unrepresentable: `BuiltHeader` carries the generation slot and the bytes
// after the common, and the claim composes the common from its own witness.
// There is no state here to test.

#[test]
fn a_frame_law_violation_in_the_header_is_refused_before_any_link() {
    let scratch = Scratch::new("law-refused");
    let dir = root(&scratch);
    let names = pending_name("pkg");

    let result = claim_error(
        &dir,
        &names,
        BuiltHeader::Witnessed {
            generation: GENERATION,
            tail: vec![0xEE; CEILING],
        },
        b"P",
    );
    assert!(matches!(result, Err(JournalError::Law(_))));
    assert!(
        !scratch.path().join("pkg.pending.create").exists()
            && !scratch.path().join("pkg.pending").exists(),
    );
}

// A pre-link recheck refusal discards the never-linked claim file it created.
// No caller code runs between the create and the recheck — that is what keeps
// a preclaim refusal from following a link — so the state cannot be induced
// from inside the process, and reaching it needs a fault seam engaging after
// `create_file_excl` succeeds.

#[test]
fn a_claim_collides_with_existing_journal_names() {
    let scratch = Scratch::new("collide");
    let dir = root(&scratch);
    let names = pending_name("store");

    std::fs::write(scratch.path().join("store.pending.create"), b"debris")
        .expect("plant claim debris");
    assert!(matches!(
        claim_error(&dir, &names, header(), b"P"),
        Err(JournalError::Custody(CustodyError::AlreadyExists { .. }))
    ));

    std::fs::remove_file(scratch.path().join("store.pending.create")).expect("clear debris");
    std::fs::write(scratch.path().join("store.pending"), b"debris").expect("plant pending");
    assert!(matches!(
        claim_error(&dir, &names, header(), b"P"),
        Err(JournalError::Custody(CustodyError::AlreadyExists { .. }))
    ));
}

#[test]
fn appends_grow_the_journal_by_exact_records_and_enforce_the_registry() {
    let scratch = Scratch::new("append");
    let dir = root(&scratch);
    let names = pending_name("store");
    let mut live = claim_ids(&dir, &names);
    let path = scratch.path().join("store.pending");
    let base_len = std::fs::metadata(&path).expect("stat").len();

    assert!(matches!(
        live.append(1, b"again"),
        Err(JournalError::TagNotAdvancing {
            last: 1,
            requested: 1,
        })
    ));
    assert!(matches!(
        live.append(4, b"beyond"),
        Err(JournalError::Law(_))
    ));

    live.append(2, b"installed").expect("append Installed");
    let record = encode_record(JournalKind::Ids, 1, 2, b"installed").expect("record");
    assert_eq!(
        std::fs::metadata(&path).expect("stat").len(),
        base_len + record.len() as u64
    );
    assert_eq!(live.last_tag(), 2);

    live.append(3, b"cleaned").expect("append Cleaned");
    assert!(matches!(
        live.append(3, b"more"),
        Err(JournalError::AppendAfterComplete)
    ));
}

#[test]
fn an_append_over_the_ceiling_is_refused_and_writes_nothing() {
    let scratch = Scratch::new("ceiling");
    let dir = root(&scratch);
    let names = pending_name("ids");

    let mut live = claim(
        &dir,
        &names,
        JournalKind::Ids,
        BuiltHeader::Witnessed {
            generation: GENERATION,
            tail: vec![0x88; 2_000_000],
        },
        b"",
    )
    .expect("claim a big ids journal");
    let path = scratch.path().join("ids.pending");
    let before = std::fs::metadata(&path).expect("stat").len();

    assert!(matches!(
        live.append(2, &[0u8; 102_000]),
        Err(JournalError::CeilingExceeded { .. })
    ));
    assert_eq!(
        std::fs::metadata(&path).expect("stat").len(),
        before,
        "a refused append writes nothing",
    );

    live.append(2, b"still-usable")
        .expect("the journal stays live");
}

#[test]
fn finish_requires_the_terminal_phase_then_removes_both_names() {
    let scratch = Scratch::new("finish");
    let dir = root(&scratch);
    let names = pending_name("store");
    let mut live = claim_ids(&dir, &names);

    live.append(2, b"installed").expect("append");
    let incomplete = live;
    let error = incomplete
        .finish()
        .expect_err("finish refuses before the terminal phase");
    assert!(matches!(
        error,
        JournalError::FinishBeforeComplete { last_tag: 2 }
    ));

    // Reopen the journal state and drive it to the terminal phase.
    let mut live = match admit(&dir, &names).expect("classify") {
        PendingState::Pending(pending) => pending.resume().expect("resume"),
        other => panic!("expected a pending journal, found {other:?}"),
    };
    live.append(3, b"cleaned").expect("append terminal");
    live.finish().expect("finish the complete journal");

    assert!(!scratch.path().join("store.pending").exists());
    assert!(!scratch.path().join("store.pending.create").exists());
    assert!(matches!(
        admit(&dir, &names).expect("classify after finish"),
        PendingState::Absent
    ));
}

#[test]
fn a_hidden_extra_link_fails_the_finish_recheck_closed() {
    let scratch = Scratch::new("finish-extra-link");
    let dir = root(&scratch);
    let names = pending_name("store");
    let mut live = claim_ids(&dir, &names);
    live.append(2, b"i").expect("append");
    live.append(3, b"c").expect("append terminal");

    std::fs::hard_link(
        scratch.path().join("store.pending"),
        scratch.path().join("shadow"),
    )
    .expect("plant a hidden extra link");
    assert!(matches!(
        live.finish(),
        Err(JournalError::Corrupt(CorruptionReason::ExtraLinks {
            found: 2
        }))
    ));
    assert!(
        scratch.path().join("store.pending").exists(),
        "a refused finish unlinks nothing",
    );
}

#[test]
fn a_vanished_pending_name_fails_the_append_recheck_closed() {
    let scratch = Scratch::new("append-drift");
    let dir = root(&scratch);
    let names = pending_name("store");
    let mut live = claim_ids(&dir, &names);

    std::fs::remove_file(scratch.path().join("store.pending")).expect("steal the pending name");
    assert!(matches!(
        live.append(2, b"x"),
        Err(JournalError::Custody(CustodyError::IdentityDrift { .. }))
    ));
}

/// The hostile-umask leg: entry creation modes are restored on the creating
/// descriptor, so the claim law's exact `0600` (and the documented directory
/// `0700`) hold under any process umask. The outer test re-invokes this
/// binary's ignored helper through `sh` with `umask 0277`, which would
/// otherwise strip the owner-write bit from every created entry.
#[test]
fn creation_modes_are_umask_independent() {
    let exe = std::env::current_exe().expect("test binary path");
    let output = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(
            "umask 0277 && exec '{}' --exact lifecycle::hostile_umask_helper --ignored",
            exe.display()
        ))
        .output()
        .expect("run the hostile-umask child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "the hostile-umask child failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs only under `creation_modes_are_umask_independent`'s hostile umask.
#[test]
#[ignore = "re-invoked by creation_modes_are_umask_independent under umask 0277"]
fn hostile_umask_helper() {
    let scratch = Scratch::new("umask");
    // The scratch root itself was created under the hostile umask; restore
    // owner access so the subject under test is entry creation inside it.
    set_mode(scratch.path(), 0o700);

    // The control: an ordinary creation that restores no mode must show the
    // hostile umask actually applied. Without it a shell that ignored
    // `umask 0277` would make every assertion below vacuously green.
    let control = scratch.path().join("control");
    std::fs::File::create(&control).expect("create the umask control");
    assert_eq!(
        std::fs::metadata(&control).expect("stat control").mode() & 0o7777,
        0o400,
        "umask 0277 did not apply, so this leg proves nothing"
    );

    let dir = root(&scratch);

    dir.create_file_excl(&name("probe")).expect("create probe");
    assert_eq!(
        mode_of(&scratch.path().join("probe")),
        0o600,
        "file creation is umask-independent"
    );

    dir.create_child_dir(&name("inner")).expect("create child");
    let child_mode = std::fs::metadata(scratch.path().join("inner"))
        .expect("stat child")
        .mode()
        & 0o7777;
    assert_eq!(child_mode, 0o700, "directory creation is umask-independent");

    let lock = CacheLock::acquire(&dir, &name("lock")).expect("acquire lock");
    let lock_mode = std::fs::metadata(scratch.path().join("lock"))
        .expect("stat lock")
        .mode()
        & 0o7777;
    assert_eq!(lock_mode, 0o600, "lock creation is umask-independent");
    drop(lock);

    let names = pending_name("store");
    let mut live = claim_ids(&dir, &names);
    live.append(2, b"i").expect("append");
    live.append(3, b"c").expect("append terminal");
    live.finish().expect("finish under the hostile umask");
}

#[test]
fn classify_reports_absence() {
    let scratch = Scratch::new("absent");
    let dir = root(&scratch);
    assert!(matches!(
        admit(&dir, &pending_name("store")).expect("classify"),
        PendingState::Absent
    ));
}

#[test]
fn create_only_debris_is_preclaim_and_discardable_under_witness() {
    let scratch = Scratch::new("preclaim");
    let dir = root(&scratch);
    let names = pending_name("store");
    let claim_path = scratch.path().join("store.pending.create");

    // Kill points: an empty claim file, a partial header, and a complete
    // frame that never reached its link are all preclaim.
    let full = planted_claim_bytes();
    for cut in [0, 1, 15, 16, full.len()] {
        std::fs::write(&claim_path, &full[..cut]).expect("plant preclaim debris");
        match admit(&dir, &names).expect("classify") {
            PendingState::Preclaim(debris) => {
                debris.discard().expect("witnessed discard");
                assert!(
                    !claim_path.exists(),
                    "discard removes the debris at cut {cut}"
                );
            }
            other => panic!("cut {cut}: expected preclaim, found {other:?}"),
        }
    }
    assert!(matches!(
        admit(&dir, &names).expect("classify"),
        PendingState::Absent
    ));
}

#[test]
fn read_only_preclaim_debris_is_classified_and_discardable() {
    let scratch = Scratch::new("preclaim-readonly");
    let dir = root(&scratch);
    let names = pending_name("store");
    let claim_path = scratch.path().join("store.pending.create");

    // Kill point: the crash fell between the create and the mode-restoring
    // fchmod under a hostile umask, leaving mode-0400 debris. Classification
    // and discard must need only read access to the file.
    std::fs::write(&claim_path, b"partial").expect("plant preclaim debris");
    set_mode(&claim_path, 0o400);

    match admit(&dir, &names).expect("classify with read access alone") {
        PendingState::Preclaim(debris) => debris
            .discard()
            .expect("the witnessed discard needs only the directory"),
        other => panic!("expected preclaim, found {other:?}"),
    }
    assert!(!claim_path.exists());
}

/// The sibling kill point: the same crash under a umask that strips owner read
/// (`0477` leaves `0200`) leaves debris no open of this crate can reach, since
/// classification pins the inode it reports through a held descriptor.
/// Classification refuses it by name with the observed mode and the mode an
/// operator must restore, rather than an unclassified I/O error; restoring
/// that mode returns the debris to ordinary classification and discard.
#[test]
fn write_only_preclaim_debris_names_the_operator_action() {
    let scratch = Scratch::new("preclaim-writeonly");
    require_mode_bits_bind(&scratch);
    let dir = root(&scratch);
    let names = pending_name("store");
    let claim_path = scratch.path().join("store.pending.create");
    std::fs::write(&claim_path, b"partial").expect("plant preclaim debris");
    set_mode(&claim_path, 0o200);

    match admit(&dir, &names) {
        Err(JournalError::Custody(CustodyError::ModeDenied {
            op,
            found,
            required,
        })) => assert_eq!((op, found, required), (CustodyOp::OpenFile, 0o200, 0o400)),
        other => panic!("expected the typed mode refusal, found {other:?}"),
    }
    assert_eq!(
        std::fs::metadata(&claim_path).expect("stat debris").mode() & 0o7777,
        0o200,
        "the refusal writes no mode of its own",
    );

    set_mode(&claim_path, 0o400);
    match admit(&dir, &names).expect("classify after the restore") {
        PendingState::Preclaim(debris) => debris.discard().expect("witnessed discard"),
        other => panic!("expected preclaim, found {other:?}"),
    }
    assert!(!claim_path.exists());
}

#[test]
fn preclaim_debris_with_an_extra_link_is_retained_corruption() {
    let scratch = Scratch::new("preclaim-linked");
    let dir = root(&scratch);
    let names = pending_name("store");
    let claim_path = scratch.path().join("store.pending.create");
    std::fs::write(&claim_path, b"debris").expect("plant debris");
    std::fs::hard_link(&claim_path, scratch.path().join("elsewhere")).expect("extra link");

    match admit(&dir, &names).expect("classify") {
        PendingState::Corrupt(reason) => {
            assert_eq!(reason, CorruptionReason::ExtraLinks { found: 2 })
        }
        other => panic!("expected retained corruption, found {other:?}"),
    }
    assert!(claim_path.exists(), "classification never mutates");
}

#[test]
fn the_two_link_state_is_a_claim_to_adopt() {
    let scratch = Scratch::new("two-link");
    let dir = root(&scratch);
    let names = pending_name("store");
    let claim_path = scratch.path().join("store.pending.create");
    let pending_path = scratch.path().join("store.pending");

    // Kill point: the crash fell between the link and the claim-name unlink.
    std::fs::write(&claim_path, planted_claim_bytes()).expect("write claim file");
    set_mode(&claim_path, 0o600);
    std::fs::hard_link(&claim_path, &pending_path).expect("link to pending");

    let live = match admit(&dir, &names).expect("classify") {
        PendingState::Claimed(claimed) => {
            assert_eq!(claimed.frame().records().len(), 1);
            claimed.adopt().expect("adopt the claim")
        }
        other => panic!("expected a claimed journal, found {other:?}"),
    };
    assert!(!claim_path.exists(), "adoption unlinks the claim name");
    assert!(pending_path.exists());

    let mut live = live;
    live.append(2, b"i").expect("append after adoption");
    live.append(3, b"c").expect("append terminal");
    live.finish().expect("finish");
    assert!(!pending_path.exists());
}

#[test]
fn a_two_link_journal_with_appended_records_is_retained_corruption() {
    let scratch = Scratch::new("two-link-overfull");
    let dir = root(&scratch);
    let names = pending_name("store");
    let claim_path = scratch.path().join("store.pending.create");

    let mut bytes = planted_claim_bytes();
    bytes.extend_from_slice(&encode_record(JournalKind::Ids, 1, 2, b"x").expect("record"));
    std::fs::write(&claim_path, &bytes).expect("write overfull claim");
    set_mode(&claim_path, 0o600);
    std::fs::hard_link(&claim_path, scratch.path().join("store.pending")).expect("link");

    match admit(&dir, &names).expect("classify") {
        PendingState::Corrupt(reason) => {
            assert_eq!(reason, CorruptionReason::ClaimBeyondPrepared)
        }
        other => panic!("expected retained corruption, found {other:?}"),
    }
}

#[test]
fn split_inodes_under_both_names_are_retained_corruption() {
    let scratch = Scratch::new("split");
    let dir = root(&scratch);
    let names = pending_name("store");
    std::fs::write(scratch.path().join("store.pending.create"), b"one").expect("claim file");
    std::fs::write(scratch.path().join("store.pending"), b"two").expect("pending file");

    match admit(&dir, &names).expect("classify") {
        PendingState::Corrupt(reason) => assert_eq!(reason, CorruptionReason::SplitInodes),
        other => panic!("expected retained corruption, found {other:?}"),
    }
}

#[test]
fn a_pending_journal_replays_its_records_and_resumes() {
    let scratch = Scratch::new("replay");
    let dir = root(&scratch);
    let names = pending_name("store");
    let pending_path = scratch.path().join("store.pending");

    let mut bytes = planted_claim_bytes();
    bytes.extend_from_slice(&encode_record(JournalKind::Ids, 1, 2, b"installed").expect("record"));
    std::fs::write(&pending_path, &bytes).expect("write pending journal");
    set_mode(&pending_path, 0o600);

    let pending = match admit(&dir, &names).expect("classify") {
        PendingState::Pending(pending) => pending,
        other => panic!("expected a pending journal, found {other:?}"),
    };
    let records = pending.frame().records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].phase_tag(), 1);
    assert_eq!(records[0].payload(), b"P");
    assert_eq!(records[1].phase_tag(), 2);
    assert_eq!(records[1].payload(), b"installed");
    assert_eq!(pending.frame().tail(), &TailState::Clean);

    let mut live = pending.resume().expect("resume");
    assert_eq!(live.last_tag(), 2);
    live.append(3, b"cleaned").expect("append terminal");
    live.finish().expect("finish");
    assert!(!pending_path.exists());
}

#[test]
fn an_incomplete_tail_is_truncated_only_against_the_unique_next_record() {
    let scratch = Scratch::new("truncate");
    let dir = root(&scratch);
    let names = pending_name("store");
    let pending_path = scratch.path().join("store.pending");

    let valid = planted_claim_bytes();
    let expected_next = encode_record(JournalKind::Ids, 1, 2, b"final").expect("record");
    let mut bytes = valid.clone();
    bytes.extend_from_slice(&expected_next[..12]); // kill point: mid-record crash
    std::fs::write(&pending_path, &bytes).expect("write torn journal");
    set_mode(&pending_path, 0o600);

    let mut pending = match admit(&dir, &names).expect("classify") {
        PendingState::Pending(pending) => pending,
        other => panic!("expected a pending journal, found {other:?}"),
    };
    assert_eq!(
        pending.frame().tail(),
        &TailState::IncompletePrefix {
            bytes: expected_next[..12].to_vec(),
        }
    );

    // A resume before truncation is refused.
    let error = match admit(&dir, &names).expect("classify") {
        PendingState::Pending(other) => other.resume().expect_err("resume refuses a torn tail"),
        other => panic!("expected a pending journal, found {other:?}"),
    };
    assert!(matches!(error, JournalError::IncompleteTail));

    // A candidate that is not the tail's continuation truncates nothing.
    let wrong = encode_record(JournalKind::Ids, 1, 2, b"foxal").expect("record");
    assert!(matches!(
        pending.truncate_tail(&wrong),
        Err(JournalError::TailNotPrefix)
    ));
    assert_eq!(
        std::fs::metadata(&pending_path).expect("stat").len(),
        bytes.len() as u64,
        "a refused truncation mutates nothing",
    );

    // A structurally illegal candidate is refused as such.
    let illegal = encode_record(JournalKind::Ids, 2, 3, b"x").expect("record");
    assert!(matches!(
        pending.truncate_tail(&illegal),
        Err(JournalError::ExpectedRecordIllegal(_))
    ));

    // The unique next record truncates the torn tail exactly.
    pending.truncate_tail(&expected_next).expect("truncate");
    assert_eq!(
        std::fs::metadata(&pending_path).expect("stat").len(),
        valid.len() as u64
    );
    assert_eq!(pending.frame().tail(), &TailState::Clean);

    let mut live = pending.resume().expect("resume after truncation");
    live.append(2, b"final")
        .expect("re-append the truncated record");
    live.append(3, b"done").expect("append terminal");
    live.finish().expect("finish");
}

#[test]
fn truncating_a_clean_journal_is_refused() {
    let scratch = Scratch::new("truncate-clean");
    let dir = root(&scratch);
    let names = pending_name("store");
    let pending_path = scratch.path().join("store.pending");
    std::fs::write(&pending_path, planted_claim_bytes()).expect("write journal");
    set_mode(&pending_path, 0o600);

    let mut pending = match admit(&dir, &names).expect("classify") {
        PendingState::Pending(pending) => pending,
        other => panic!("expected a pending journal, found {other:?}"),
    };
    let next = encode_record(JournalKind::Ids, 1, 2, b"x").expect("record");
    assert!(matches!(
        pending.truncate_tail(&next),
        Err(JournalError::NoIncompleteTail)
    ));
}

#[test]
fn hostile_pending_states_are_retained_corruption() {
    let scratch = Scratch::new("hostile");
    let dir = root(&scratch);
    let names = pending_name("store");
    let pending_path = scratch.path().join("store.pending");

    // Interior damage: a corrupted record length echo.
    let mut torn = planted_claim_bytes();
    let last = torn.len() - 1;
    torn[last] ^= 0xFF;
    std::fs::write(&pending_path, &torn).expect("write");
    set_mode(&pending_path, 0o600);
    assert_corrupt(
        &dir,
        &names,
        &CorruptionReason::Frame(FrameCorruption::LengthEchoMismatch { sequence: 0 }),
    );

    // A header-only journal lost its durable Prepared record.
    let header_only =
        encode_header(JournalKind::Ids, &header_bytes(planted_common())).expect("header");
    std::fs::write(&pending_path, &header_only).expect("write");
    assert_corrupt(&dir, &names, &CorruptionReason::MissingPrepared);

    // A kind byte naming another kind under this journal's name.
    let mut other_kind = planted_claim_bytes();
    other_kind[9] = 2;
    std::fs::write(&pending_path, &other_kind).expect("write");
    assert_corrupt(
        &dir,
        &names,
        &CorruptionReason::Frame(FrameCorruption::BadKind { found: 2 }),
    );

    // Oversize: the ceiling plus one byte refuses before allocation.
    std::fs::write(&pending_path, vec![0xAA; CEILING + 1]).expect("write");
    assert_corrupt(
        &dir,
        &names,
        &CorruptionReason::Frame(FrameCorruption::Oversized { limit: CEILING }),
    );

    // The wrong mode is retained corruption.
    std::fs::write(&pending_path, planted_claim_bytes()).expect("write");
    set_mode(&pending_path, 0o644);
    assert_corrupt(&dir, &names, &CorruptionReason::WrongMode { found: 0o644 });
    set_mode(&pending_path, 0o600);

    // An extra hard link is retained corruption.
    std::fs::hard_link(&pending_path, scratch.path().join("shadow")).expect("link");
    assert_corrupt(&dir, &names, &CorruptionReason::ExtraLinks { found: 2 });
    std::fs::remove_file(scratch.path().join("shadow")).expect("unlink shadow");

    // A symbolic link under the pending name is retained corruption.
    std::fs::remove_file(&pending_path).expect("clear");
    std::os::unix::fs::symlink("elsewhere", &pending_path).expect("symlink");
    assert_corrupt(
        &dir,
        &names,
        &CorruptionReason::WrongNodeKind {
            found: NodeKind::Symlink,
        },
    );
}

/// The exhaustive kill sweep: every byte-position cut of the complete claim
/// bytes, planted as debris under each of the two single-name states,
/// classifies to a typed state — never an error, never a panic — and lands in
/// the class the representative fixtures pin.
#[test]
fn every_per_byte_cut_of_a_journal_classifies_deterministically() {
    let scratch = Scratch::new("cut-sweep");
    let dir = root(&scratch);
    let names = pending_name("store");
    let claim_path = scratch.path().join("store.pending.create");
    let pending_path = scratch.path().join("store.pending");
    let full = planted_claim_bytes();
    let header_len = header_bytes(planted_common()).len();
    let header_end = 16 + header_len;

    for cut in 0..=full.len() {
        // Create-only debris is preclaim at every cut: content and mode are
        // unconstrained before the durable claim.
        std::fs::write(&claim_path, &full[..cut]).expect("plant claim debris");
        match admit(&dir, &names).expect("classify claim debris") {
            PendingState::Preclaim(_) => {}
            other => panic!("claim cut {cut}: expected preclaim, found {other:?}"),
        }
        std::fs::remove_file(&claim_path).expect("clear claim debris");

        // Pending-name debris crosses the pinned class boundaries: inside the
        // fixed prefix, inside the row header, before the Prepared record's
        // completion, and the one complete frame.
        std::fs::write(&pending_path, &full[..cut]).expect("plant pending debris");
        set_mode(&pending_path, 0o600);
        match admit(&dir, &names).expect("classify pending debris") {
            PendingState::Corrupt(CorruptionReason::Frame(FrameCorruption::TooShort { found })) => {
                assert!(cut < 16, "pending cut {cut}: TooShort past the prefix");
                assert_eq!(found, cut);
            }
            PendingState::Corrupt(CorruptionReason::Frame(FrameCorruption::HeaderTruncated {
                expected,
                found,
            })) => {
                assert!(
                    (16..header_end).contains(&cut),
                    "pending cut {cut}: HeaderTruncated outside the row header"
                );
                assert_eq!((expected, found), (header_len, cut - 16));
            }
            PendingState::Corrupt(CorruptionReason::MissingPrepared) => {
                assert!(
                    (header_end..full.len()).contains(&cut),
                    "pending cut {cut}: MissingPrepared outside the record span"
                );
            }
            PendingState::Pending(_) => {
                assert_eq!(cut, full.len(), "pending cut {cut}: complete frame only");
            }
            other => panic!("pending cut {cut}: unclassified state {other:?}"),
        }
        std::fs::remove_file(&pending_path).expect("clear pending debris");
    }
}

fn identity_of(path: &std::path::Path) -> FsIdentity {
    let metadata = std::fs::metadata(path).expect("stat for identity");
    FsIdentity::new(metadata.dev(), metadata.ino())
}

fn assert_corrupt(dir: &AdmittedDir, names: &PendingName, expected: &CorruptionReason) {
    match admit(dir, names).expect("classify") {
        PendingState::Corrupt(reason) => assert_eq!(&reason, expected),
        other => panic!("expected retained corruption {expected:?}, found {other:?}"),
    }
}
