//! Complete logical backup: the published artifact, occupied outputs, and cut barriers.

use super::*;
use crate::test_support::once;
use crate::{EngineKind, LogicalHead, ProvisionRequest, StoreEnvelope, StoreInstanceId};
use marrow_fs_journal::{CustodyError, CustodyOp};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Failure {
    FileSync,
    ReplacedStage,
    ParentSync,
}

fn storage_full(op: CustodyOp) -> CustodyError {
    CustodyError::Io {
        op,
        source: io::ErrorKind::StorageFull.into(),
    }
}

/// The seam that fails a backup at `failure`: the output file's sync, that sync after the
/// staged name was replaced underneath, or the publication's parent sync.
fn failing(failure: Failure) -> Seam {
    match failure {
        Failure::FileSync => once(
            |event| matches!(event, Event::OutputSync { .. }),
            |_| Err(storage_full(CustodyOp::Sync)),
        ),
        Failure::ReplacedStage => once(
            |event| matches!(event, Event::OutputSync { .. }),
            |event| {
                let Event::OutputSync { stage } = event else {
                    unreachable!("selected the output sync");
                };
                std::fs::rename(stage, stage.with_extension("preserved")).unwrap();
                std::fs::write(stage, b"replacement must survive").unwrap();
                Err(storage_full(CustodyOp::Sync))
            },
        ),
        Failure::ParentSync => once(
            |event| matches!(event, Event::ParentSync),
            |_| Err(storage_full(CustodyOp::Sync)),
        ),
    }
    .0
}

pub(crate) const SOURCE: &str = "resource Item { required value: int }\nstore ^items[key: int]: Item { index byValue[value] unique }\npub fn read(key: int): int { return ^items[key].value ?? 0 }\npub fn seed() { transaction { ^items[7] = Item(value: 42) } }\n";

pub(crate) fn image_bytes() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| compile_image(SOURCE))
}

/// The ledger for [`SOURCE`]: the application, the `Item` product and its field, the
/// `items` root with its key, and the `byValue` index.
const IDS: &str = "marrow ids v0\nmachine-written by marrow; do not edit\nid application . 01010101010101010101010101010101\nid product Item 02020202020202020202020202020202\nid field Item.value 03030303030303030303030303030303\nid root items 04040404040404040404040404040404\nid key items.key 05050505050505050505050505050505\nid index items.byValue 06060606060606060606060606060606\nhigh-water 0\nend\n";

pub(crate) fn compile_image(source: &str) -> Vec<u8> {
    marrow_test_programs::program::compile_bytes(source, IDS)
}

use marrow_test_support::Scratch;

/// Provision a store at `scratch.store()` under the [`SOURCE`] corpus, so the backup and
/// restore cases start from a real published store rather than a hand-built directory.
pub(crate) fn provision_fixture(scratch: &Scratch) {
    let image = marrow_verify::verify(image_bytes()).unwrap();
    crate::provision(
        scratch.store(),
        ProvisionRequest {
            envelope: StoreEnvelope {
                instance: StoreInstanceId::draw().unwrap(),
                writer_toolchain: env!("CARGO_PKG_VERSION").into(),
                engine_kind: EngineKind::Redb,
                engine_format_version: marrow_kernel::durable::NATIVE_ENGINE_FORMAT_VERSION,
            },
            head: LogicalHead::provision(
                crate::active_binding(&image),
                crate::accepted_ceiling(&image),
                crate::head_map(&image).unwrap(),
            ),
        },
    )
    .unwrap();
}

#[test]
fn published_empty_backup_contains_exact_image_and_accepted_head() {
    let scratch = Scratch::new("backup");
    provision_fixture(&scratch);
    let before_head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).unwrap();
    let before_envelope = std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).unwrap();
    let destination = scratch.path().join("backup");
    let result = backup(scratch.store(), image_bytes(), &destination).unwrap();
    assert!(result.audit.is_clean());
    assert_eq!(result.audit.summary.cells, 0);
    let bytes = std::fs::read(&destination).unwrap();
    let mut cursor = io::Cursor::new(&bytes);
    let (mut decoder, header) = crate::backup_stream::Decoder::new(&mut cursor).unwrap();
    assert_eq!(header.image, image_bytes());
    assert_eq!(header.head, before_head);
    assert!(decoder.next_cell().unwrap().is_none());
    assert_eq!(&bytes[bytes.len() - 32..], result.digest.bytes());
    assert_eq!(
        std::fs::read(scratch.store().join(crate::HEAD_FILE)).unwrap(),
        before_head
    );
    assert_eq!(
        std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).unwrap(),
        before_envelope
    );
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 2);
}

#[test]
fn occupied_output_is_unchanged_and_its_private_candidate_is_removed() {
    let scratch = Scratch::new("backup");
    provision_fixture(&scratch);
    let destination = scratch.path().join("backup");
    std::fs::write(&destination, b"existing backup").unwrap();
    let error = backup(scratch.store(), image_bytes(), &destination).unwrap_err();
    assert!(
        matches!(error.fault, BackupFault::Io(source) if source.kind() == io::ErrorKind::AlreadyExists)
    );
    assert!(error.unpublished.is_none());
    assert!(error.cleanup.is_none());
    assert_eq!(std::fs::read(&destination).unwrap(), b"existing backup");
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 2);
}

#[test]
fn held_source_and_invalid_image_refuse_before_output_creation() {
    let scratch = Scratch::new("backup");
    provision_fixture(&scratch);
    let destination = scratch.path().join("backup");
    let image = marrow_verify::verify(image_bytes()).unwrap();
    let held = crate::attach(scratch.store(), prepare(image)).unwrap();
    let error = backup(scratch.store(), image_bytes(), &destination).unwrap_err();
    assert!(matches!(error.fault, BackupFault::Audit(_)));
    assert!(error.unpublished.is_none());
    drop(held);
    assert!(matches!(
        backup(scratch.store(), b"invalid", &destination)
            .unwrap_err()
            .fault,
        BackupFault::Image(_)
    ));
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 1);
}

#[test]
fn failed_barriers_and_cleanup_identity_preserve_the_actual_outcome() {
    for failure in [
        Failure::FileSync,
        Failure::ReplacedStage,
        Failure::ParentSync,
    ] {
        let scratch = std::mem::ManuallyDrop::new(Scratch::new("backup"));
        eprintln!(
            "preserved backup failure fixture: {}",
            scratch.path().display()
        );
        provision_fixture(&scratch);
        let destination = scratch.path().join("backup");
        let error = backup_observed(
            scratch.store(),
            image_bytes(),
            &destination,
            failing(failure),
        )
        .unwrap_err();
        match failure {
            Failure::FileSync => {
                assert!(
                    matches!(error.fault, BackupFault::Io(source) if source.kind() == io::ErrorKind::StorageFull)
                );
                assert!(error.unpublished.is_none());
                assert!(error.cleanup.is_none());
                assert!(!destination.exists());
                assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 1);
            }
            Failure::ReplacedStage => {
                assert!(
                    matches!(error.fault, BackupFault::Io(source) if source.kind() == io::ErrorKind::StorageFull)
                );
                assert!(error.cleanup.is_some());
                let stage = error.unpublished.unwrap();
                assert_eq!(std::fs::read(&stage).unwrap(), b"replacement must survive");
                assert!(stage.with_extension("preserved").is_file());
                assert!(!destination.exists());
            }
            Failure::ParentSync => {
                assert!(
                    matches!(error.fault, BackupFault::PublicationUncertain { destination: ref found, ref source } if found == &destination && source.kind() == io::ErrorKind::StorageFull)
                );
                assert!(error.unpublished.is_none());
                assert!(error.cleanup.is_none());
                let bytes = std::fs::read(&destination).unwrap();
                let mut input = io::Cursor::new(bytes);
                let (mut decoder, _) = crate::backup_stream::Decoder::new(&mut input).unwrap();
                assert!(decoder.next_cell().unwrap().is_none());
                assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 2);
            }
        }
    }
}
