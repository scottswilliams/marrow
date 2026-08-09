//! The pending-journal protocol end to end: claim, append, replay, exact-prefix
//! truncation, terminal unlink, and the crash-debris matrix. Crash states are
//! kill-point fixtures: each on-disk prefix of the protocol is constructed
//! directly, then reopened and classified through the public API.

mod common;

use std::os::unix::fs::{MetadataExt, PermissionsExt};

use common::Scratch;
use marrow_fs_journal::{
    AdmittedDir, CacheLock, CorruptionReason, CustodyError, EntryName, EntryNameError,
    FrameCorruption, FsIdentity, JournalCommon, JournalError, JournalKind, NodeKind, PendingName,
    PendingState, TailState, claim, classify, encode_header, encode_record,
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

const PROVISION_HEADER: [u8; 8] = [0x77; 8];

/// The exact bytes a fresh Provision claim writes: header plus the
/// sequence-zero Prepared record.
fn provision_claim_bytes() -> Vec<u8> {
    let mut bytes =
        encode_header(JournalKind::Provision, &PROVISION_HEADER).expect("encode header");
    bytes.extend_from_slice(
        &encode_record(JournalKind::Provision, 0, 1, b"P").expect("encode Prepared"),
    );
    bytes
}

fn claim_provision<'d>(
    dir: &'d AdmittedDir,
    names: &PendingName,
) -> marrow_fs_journal::LiveJournal<'d> {
    claim(
        dir,
        names,
        JournalKind::Provision,
        |_witness| PROVISION_HEADER.to_vec(),
        b"P",
    )
    .expect("claim a provision journal")
}

#[test]
fn pending_names_are_derived_and_readmitted() {
    let names = pending_name("store");
    assert_eq!(names.base().as_str(), "store");
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

    let live = claim_provision(&dir, &names);
    assert_eq!(live.kind(), JournalKind::Provision);
    assert_eq!(live.next_sequence(), 1);
    assert_eq!(live.last_tag(), 1);
    assert!(!live.is_complete());

    // On disk: the pending name alone, one link, mode 0600, exact bytes.
    assert!(
        !scratch.path().join("store.pending.create").exists(),
        "the claim name is unlinked once the claim completes",
    );
    let written = std::fs::read(scratch.path().join("store.pending")).expect("read pending");
    assert_eq!(written, provision_claim_bytes());
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

    let live = claim(
        &dir,
        &names,
        JournalKind::Lineage,
        |witness| {
            assert_eq!(witness.parent, parent);
            let mut header = JournalCommon {
                generation: [0x51; 16],
                parent: witness.parent,
                journal_inode: witness.journal_inode,
            }
            .encode()
            .to_vec();
            header.extend_from_slice(&[0xEE; 136]);
            header
        },
        &[0x01],
    )
    .expect("claim a lineage journal");
    drop(live);

    let pending_path = scratch.path().join("pkg.pending");
    let written = std::fs::read(&pending_path).expect("read pending");
    let embedded = JournalCommon::decode(written[16..64].try_into().expect("48-byte common"));
    assert_eq!(embedded.generation, [0x51; 16]);
    assert_eq!(embedded.parent, parent);
    assert_eq!(
        embedded.journal_inode,
        identity_of(&pending_path),
        "the header witnesses the created inode",
    );
}

#[test]
fn a_kind_four_header_must_embed_the_offered_witness() {
    let scratch = Scratch::new("witness-refused");
    let dir = root(&scratch);
    let names = pending_name("pkg");

    let result = claim(
        &dir,
        &names,
        JournalKind::Lineage,
        |witness| {
            let mut header = JournalCommon {
                generation: [0x51; 16],
                parent: FsIdentity::new(1, 1), // not the admitted parent
                journal_inode: witness.journal_inode,
            }
            .encode()
            .to_vec();
            header.extend_from_slice(&[0xEE; 136]);
            header
        },
        &[0x01],
    );
    assert!(matches!(result, Err(JournalError::WitnessNotEmbedded)));
    assert!(
        !scratch.path().join("pkg.pending.create").exists()
            && !scratch.path().join("pkg.pending").exists(),
        "a producer-law refusal discards its witnessed preclaim file",
    );
}

#[test]
fn a_frame_law_violation_in_the_header_is_refused_before_any_link() {
    let scratch = Scratch::new("law-refused");
    let dir = root(&scratch);
    let names = pending_name("pkg");

    let result = claim(
        &dir,
        &names,
        JournalKind::Lineage,
        |_witness| vec![0xEE; 10],
        &[0x01],
    );
    assert!(matches!(result, Err(JournalError::Law(_))));
    assert!(
        !scratch.path().join("pkg.pending.create").exists()
            && !scratch.path().join("pkg.pending").exists(),
    );
}

#[test]
fn a_pre_link_recheck_refusal_discards_the_never_linked_claim_file() {
    let scratch = Scratch::new("pre-link-discard");
    let dir = root(&scratch);
    let names = pending_name("store");
    let claim_path = scratch.path().join("store.pending.create");

    // The header builder runs between the create and the link, so it is the
    // one public injection point for hostile mid-claim state: force a wrong
    // mode onto the fresh claim file and the pre-link recheck must refuse.
    let result = claim(
        &dir,
        &names,
        JournalKind::Provision,
        |_witness| {
            set_mode(&claim_path, 0o644);
            PROVISION_HEADER.to_vec()
        },
        b"P",
    );
    assert!(matches!(
        result,
        Err(JournalError::Corrupt(CorruptionReason::WrongMode {
            found: 0o644
        }))
    ));
    assert!(
        !claim_path.exists() && !scratch.path().join("store.pending").exists(),
        "every pre-link refusal discards the never-linked claim file under witness",
    );
}

#[test]
fn a_claim_collides_with_existing_journal_names() {
    let scratch = Scratch::new("collide");
    let dir = root(&scratch);
    let names = pending_name("store");

    std::fs::write(scratch.path().join("store.pending.create"), b"debris")
        .expect("plant claim debris");
    assert!(matches!(
        claim(
            &dir,
            &names,
            JournalKind::Provision,
            |_witness| PROVISION_HEADER.to_vec(),
            b"P",
        ),
        Err(JournalError::Custody(CustodyError::AlreadyExists { .. }))
    ));

    std::fs::remove_file(scratch.path().join("store.pending.create")).expect("clear debris");
    std::fs::write(scratch.path().join("store.pending"), b"debris").expect("plant pending");
    assert!(matches!(
        claim(
            &dir,
            &names,
            JournalKind::Provision,
            |_witness| PROVISION_HEADER.to_vec(),
            b"P",
        ),
        Err(JournalError::Custody(CustodyError::AlreadyExists { .. }))
    ));
}

#[test]
fn appends_grow_the_journal_by_exact_records_and_enforce_the_registry() {
    let scratch = Scratch::new("append");
    let dir = root(&scratch);
    let names = pending_name("store");
    let mut live = claim_provision(&dir, &names);
    let base_len = provision_claim_bytes().len() as u64;

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
    let record = encode_record(JournalKind::Provision, 1, 2, b"installed").expect("record");
    let path = scratch.path().join("store.pending");
    assert_eq!(
        std::fs::metadata(&path).expect("stat").len(),
        base_len + record.len() as u64
    );
    assert_eq!(live.next_sequence(), 2);
    assert_eq!(live.last_tag(), 2);

    live.append(3, b"cleaned").expect("append Cleaned");
    assert!(live.is_complete());
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

    let header = vec![0x88; 2_000_000];
    let mut live = claim(
        &dir,
        &names,
        JournalKind::Ids,
        |_witness| header.clone(),
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
    let mut live = claim_provision(&dir, &names);

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
    let mut live = match classify(&dir, &names, JournalKind::Provision).expect("classify") {
        PendingState::Pending(pending) => pending.resume().expect("resume"),
        other => panic!("expected a pending journal, found {other:?}"),
    };
    live.append(3, b"cleaned").expect("append terminal");
    live.finish().expect("finish the complete journal");

    assert!(!scratch.path().join("store.pending").exists());
    assert!(!scratch.path().join("store.pending.create").exists());
    assert!(matches!(
        classify(&dir, &names, JournalKind::Provision).expect("classify after finish"),
        PendingState::Absent
    ));
}

#[test]
fn a_hidden_extra_link_fails_the_finish_recheck_closed() {
    let scratch = Scratch::new("finish-extra-link");
    let dir = root(&scratch);
    let names = pending_name("store");
    let mut live = claim_provision(&dir, &names);
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
    let mut live = claim_provision(&dir, &names);

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
            "umask 0277 && exec '{}' --exact hostile_umask_helper --ignored",
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
    let dir = root(&scratch);

    let file = dir.create_file_excl(&name("probe")).expect("create probe");
    assert_eq!(
        file.stat().expect("stat probe").mode(),
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
    let mut live = claim_provision(&dir, &names);
    live.append(2, b"i").expect("append");
    live.append(3, b"c").expect("append terminal");
    live.finish().expect("finish under the hostile umask");
}

#[test]
fn classify_reports_absence() {
    let scratch = Scratch::new("absent");
    let dir = root(&scratch);
    assert!(matches!(
        classify(&dir, &pending_name("store"), JournalKind::Provision).expect("classify"),
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
    let full = provision_claim_bytes();
    for cut in [0, 1, 15, 16, full.len()] {
        std::fs::write(&claim_path, &full[..cut]).expect("plant preclaim debris");
        match classify(&dir, &names, JournalKind::Provision).expect("classify") {
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
        classify(&dir, &names, JournalKind::Provision).expect("classify"),
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

    match classify(&dir, &names, JournalKind::Provision).expect("classify with read access alone") {
        PendingState::Preclaim(debris) => debris
            .discard()
            .expect("the witnessed discard needs only the directory"),
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

    match classify(&dir, &names, JournalKind::Provision).expect("classify") {
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
    std::fs::write(&claim_path, provision_claim_bytes()).expect("write claim file");
    set_mode(&claim_path, 0o600);
    std::fs::hard_link(&claim_path, &pending_path).expect("link to pending");

    let live = match classify(&dir, &names, JournalKind::Provision).expect("classify") {
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

    let mut bytes = provision_claim_bytes();
    bytes.extend_from_slice(&encode_record(JournalKind::Provision, 1, 2, b"x").expect("record"));
    std::fs::write(&claim_path, &bytes).expect("write overfull claim");
    set_mode(&claim_path, 0o600);
    std::fs::hard_link(&claim_path, scratch.path().join("store.pending")).expect("link");

    match classify(&dir, &names, JournalKind::Provision).expect("classify") {
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

    match classify(&dir, &names, JournalKind::Provision).expect("classify") {
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

    let mut bytes = provision_claim_bytes();
    bytes.extend_from_slice(
        &encode_record(JournalKind::Provision, 1, 2, b"installed").expect("record"),
    );
    std::fs::write(&pending_path, &bytes).expect("write pending journal");
    set_mode(&pending_path, 0o600);

    let pending = match classify(&dir, &names, JournalKind::Provision).expect("classify") {
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
    assert_eq!(live.next_sequence(), 2);
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

    let valid = provision_claim_bytes();
    let expected_next = encode_record(JournalKind::Provision, 1, 2, b"final").expect("record");
    let mut bytes = valid.clone();
    bytes.extend_from_slice(&expected_next[..12]); // kill point: mid-record crash
    std::fs::write(&pending_path, &bytes).expect("write torn journal");
    set_mode(&pending_path, 0o600);

    let mut pending = match classify(&dir, &names, JournalKind::Provision).expect("classify") {
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
    let error = match classify(&dir, &names, JournalKind::Provision).expect("classify") {
        PendingState::Pending(other) => other.resume().expect_err("resume refuses a torn tail"),
        other => panic!("expected a pending journal, found {other:?}"),
    };
    assert!(matches!(error, JournalError::IncompleteTail));

    // A candidate that is not the tail's continuation truncates nothing.
    let wrong = encode_record(JournalKind::Provision, 1, 2, b"foxal").expect("record");
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
    let illegal = encode_record(JournalKind::Provision, 2, 3, b"x").expect("record");
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
    std::fs::write(&pending_path, provision_claim_bytes()).expect("write journal");
    set_mode(&pending_path, 0o600);

    let mut pending = match classify(&dir, &names, JournalKind::Provision).expect("classify") {
        PendingState::Pending(pending) => pending,
        other => panic!("expected a pending journal, found {other:?}"),
    };
    let next = encode_record(JournalKind::Provision, 1, 2, b"x").expect("record");
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
    let mut torn = provision_claim_bytes();
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
    let header_only = encode_header(JournalKind::Provision, &PROVISION_HEADER).expect("header");
    std::fs::write(&pending_path, &header_only).expect("write");
    assert_corrupt(&dir, &names, &CorruptionReason::MissingPrepared);

    // A different kind under this journal's name.
    let mut other_kind = encode_header(JournalKind::Rebind, &PROVISION_HEADER).expect("header");
    other_kind.extend_from_slice(&encode_record(JournalKind::Rebind, 0, 1, b"P").expect("record"));
    std::fs::write(&pending_path, &other_kind).expect("write");
    assert_corrupt(
        &dir,
        &names,
        &CorruptionReason::Frame(FrameCorruption::WrongKind {
            expected: JournalKind::Provision,
            found: JournalKind::Rebind,
        }),
    );

    // Oversize: the ceiling plus one byte refuses before allocation.
    std::fs::write(&pending_path, vec![0xAA; 4097]).expect("write");
    assert_corrupt(
        &dir,
        &names,
        &CorruptionReason::Frame(FrameCorruption::Oversized { limit: 4096 }),
    );

    // The wrong mode is retained corruption.
    std::fs::write(&pending_path, provision_claim_bytes()).expect("write");
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

#[test]
fn a_closed_kind_journal_verifies_its_self_witness_on_replay() {
    let scratch = Scratch::new("self-witness");
    let dir = root(&scratch);
    let names = pending_name("pkg");
    let pending_path = scratch.path().join("pkg.pending");

    // Create the journal file once so its inode identity is known before the
    // frames are planted; rewrites keep the same inode.
    std::fs::write(&pending_path, b"").expect("create pending file");
    set_mode(&pending_path, 0o600);
    let journal_inode = identity_of(&pending_path);

    let plant = |common: JournalCommon| {
        let mut header = common.encode().to_vec();
        header.extend_from_slice(&[0xEE; 136]);
        let mut bytes = encode_header(JournalKind::Lineage, &header).expect("header");
        bytes.extend_from_slice(
            &encode_record(JournalKind::Lineage, 0, 1, &[0x01]).expect("record"),
        );
        std::fs::write(&pending_path, &bytes).expect("write");
    };

    // Correct witness: the file's own identity and the admitted parent.
    let file = std::fs::metadata(scratch.path()).expect("parent metadata");
    plant(JournalCommon {
        generation: [0x51; 16],
        parent: FsIdentity::new(file.dev(), file.ino()),
        journal_inode,
    });
    assert!(matches!(
        classify(&dir, &names, JournalKind::Lineage).expect("classify"),
        PendingState::Pending(_)
    ));

    // A foreign parent identity is retained corruption.
    plant(JournalCommon {
        generation: [0x51; 16],
        parent: FsIdentity::new(0xDEAD, 0xBEEF),
        journal_inode,
    });
    assert_corrupt(&dir, &names, &CorruptionReason::SelfWitnessParentMismatch);

    // A foreign inode identity is retained corruption.
    plant(JournalCommon {
        generation: [0x51; 16],
        parent: FsIdentity::new(file.dev(), file.ino()),
        journal_inode: FsIdentity::new(0xDEAD, 0xBEEF),
    });
    assert_corrupt(&dir, &names, &CorruptionReason::SelfWitnessInodeMismatch);
}

fn identity_of(path: &std::path::Path) -> FsIdentity {
    let metadata = std::fs::metadata(path).expect("stat for identity");
    FsIdentity::new(metadata.dev(), metadata.ino())
}

fn set_mode(path: &std::path::Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("set mode");
}

fn assert_corrupt(dir: &AdmittedDir, names: &PendingName, expected: &CorruptionReason) {
    match classify(dir, names, names_kind(expected)).expect("classify") {
        PendingState::Corrupt(reason) => assert_eq!(&reason, expected),
        other => panic!("expected retained corruption {expected:?}, found {other:?}"),
    }
}

/// The kind each corruption fixture classifies against: lineage for the
/// self-witness cases, provision otherwise.
fn names_kind(expected: &CorruptionReason) -> JournalKind {
    match expected {
        CorruptionReason::SelfWitnessParentMismatch
        | CorruptionReason::SelfWitnessInodeMismatch => JournalKind::Lineage,
        _ => JournalKind::Provision,
    }
}
