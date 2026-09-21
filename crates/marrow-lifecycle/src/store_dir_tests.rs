//! The store directory under its owner: what one admission read admits and refuses.

use super::*;
use crate::seam::Seam;
use crate::test_support::Scratch;

#[test]
fn preservation_refuses_collision_and_entropy_failure_without_changing_files() {
    let scratch = Scratch::new("preservation-refusals");
    let root = scratch.base();
    let directory = AdmittedStoreDir::admit(root, Seam::NONE).expect("admit");
    let source = root.join("envelope.replacing");
    let destination = root.join("envelope.replacing.preserved.00000000000000000000000000000000");
    std::fs::write(&source, b"partial").expect("source");
    std::fs::write(&destination, b"previous preserved bytes").expect("collision");
    let mut preserved = Vec::new();
    let collision = directory
        .preserve_replacement(Artifact::Envelope, || Ok([0; 16]), &mut preserved)
        .expect_err("no replacement at collision");
    assert!(matches!(
        collision.fault,
        AdmissionFault::Custody(CustodyError::AlreadyExists { .. })
    ));
    let entropy = directory
        .preserve_replacement(
            Artifact::Envelope,
            || Err(std::io::Error::from(std::io::ErrorKind::Other)),
            &mut preserved,
        )
        .expect_err("entropy unavailable");
    assert!(matches!(
        entropy.fault,
        AdmissionFault::Custody(CustodyError::Io {
            op: CustodyOp::Read,
            ..
        })
    ));
    assert!(preserved.is_empty());
    assert_eq!(
        std::fs::read(&source).expect("source unchanged"),
        b"partial"
    );
    assert_eq!(
        std::fs::read(&destination).expect("collision unchanged"),
        b"previous preserved bytes"
    );
    drop(directory);
}

#[test]
fn store_directory_file_names_are_frozen() {
    // The store-directory layout is a durability contract; these names are frozen.
    assert_eq!(ENGINE_FILE, "store.redb");
    assert_eq!(ENVELOPE_FILE, "envelope");
    assert_eq!(HEAD_FILE, "head");
    assert_eq!(LOCK_FILE, "lock");
}

/// Each custody refusal is reported as itself. Two ways of substituting the same
/// artifact reach the same verdict, owner bits that deny the open are a permission
/// refusal rather than either, and a platform this build cannot admit a store directory
/// on names the operating system and architecture it refused on — the build is not
/// narrowed, so the refusal is the only place a user meets the narrowing.
#[test]
fn each_custody_refusal_is_reported_as_itself() {
    let refusal = |fault| AdmissionError {
        entry: StoreEntry::Envelope,
        fault,
    };
    for (fault, code) in [
        (
            CustodyError::SymlinkRefused {
                op: CustodyOp::OpenFile,
            },
            Code::StoreCorruption,
        ),
        (
            CustodyError::WrongNodeKind {
                op: CustodyOp::OpenFile,
                found: marrow_fs_journal::NodeKind::Directory,
            },
            Code::StoreCorruption,
        ),
        (
            CustodyError::NotFound {
                op: CustodyOp::OpenFile,
            },
            Code::StoreCorruption,
        ),
        (
            CustodyError::ModeDenied {
                op: CustodyOp::OpenFile,
                found: 0o400,
                required: 0o600,
            },
            Code::StorePermissionDenied,
        ),
    ] {
        assert_eq!(refusal(AdmissionFault::Custody(fault)).code(), code);
    }

    // The sibling that reaches the same verdict without going through custody: a second
    // link to the artifact is the same "the directory does not hold this artifact"
    // observation a substituted node makes.
    assert_eq!(
        refusal(AdmissionFault::MultiplyLinked { links: 2 }).code(),
        Code::StoreCorruption,
    );

    let unqualified = refusal(AdmissionFault::Custody(CustodyError::UnqualifiedPlatform {
        os: "freebsd",
        arch: "riscv64",
    }));
    assert_eq!(unqualified.code(), Code::StoreIo);
    let rendered = unqualified.to_string();
    assert!(
        rendered.contains("freebsd/riscv64"),
        "a platform refusal must name the platform it refused on: {rendered}",
    );
}

/// Each artifact an admission read names resolves to exactly the frozen entry name, is
/// admissible as one normal relative component, and reports itself under that name.
#[test]
fn every_admitted_artifact_name_is_the_frozen_entry_name() {
    for (artifact, expected) in [
        (Artifact::Envelope, ENVELOPE_FILE),
        (Artifact::Head, HEAD_FILE),
    ] {
        assert_eq!(artifact.name().as_str(), expected);
        assert_eq!(artifact.entry().label(), expected);
    }
}
