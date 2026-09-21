//! The persistent provision and open flow over real directories: one winner, complete or
//! not at all, and custody of the stage across its rename.

use std::sync::{Arc, Mutex};

use super::*;
use crate::head::ActiveBinding;
use crate::headmap::HeadMap;
use crate::seam::Observer;
use crate::store_dir::Body;
use crate::test_support::Scratch;
use marrow_fs_journal::CustodyOp;
use marrow_image::LedgerIdBytes;
use marrow_kernel::durable::StoreProjection;

#[test]
fn provision_never_replaces_an_existing_empty_directory() {
    let scratch = Scratch::new("occupied-empty");
    let destination = scratch.base().join("destination");
    std::fs::create_dir(&destination).unwrap();
    let (_, request) = compiled_request();
    let result = provision(&destination, request);
    assert!(matches!(
        result,
        Err(ProvisionError {
            fault: ProvisionFault::AlreadyProvisioned,
            ..
        })
    ));
    assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn occupied_files_and_dangling_links_refuse_without_replacement() {
    let scratch = Scratch::new("occupied-entries");
    for dangling in [false, true] {
        let destination = scratch.base().join(if dangling { "link" } else { "file" });
        if dangling {
            std::os::unix::fs::symlink("missing-target", &destination).unwrap();
        } else {
            std::fs::write(&destination, b"existing bytes").unwrap();
        }
        let (_, request) = compiled_request();
        assert!(matches!(
            provision(&destination, request),
            Err(ProvisionError {
                fault: ProvisionFault::AlreadyProvisioned,
                ..
            })
        ));
        if dangling {
            assert_eq!(
                std::fs::read_link(&destination).unwrap(),
                Path::new("missing-target")
            );
        } else {
            assert_eq!(std::fs::read(&destination).unwrap(), b"existing bytes");
        }
    }
}

/// A construction observer: optionally cuts the build at `cut`, records the stage and its
/// entries when the stage is about to be removed, and optionally refuses that removal.
struct Construction {
    cut: Option<Step>,
    removal: Option<std::io::ErrorKind>,
    observed: Mutex<Option<(PathBuf, Vec<std::ffi::OsString>)>>,
}

impl Observer for Construction {
    fn at(&self, event: Event<'_>) -> Result<(), marrow_fs_journal::CustodyError> {
        match event {
            Event::Step { step, .. } if Some(step) == self.cut => {
                Err(crate::test_support::io_fault(CustodyOp::Sync))
            }
            Event::Removal { stage } => {
                let mut names: Vec<_> = std::fs::read_dir(stage)
                    .expect("owned stage before cleanup")
                    .map(|entry| entry.expect("stage entry").file_name())
                    .collect();
                names.sort();
                *self.observed.lock().expect("observed stage") = Some((stage.to_path_buf(), names));
                match self.removal {
                    Some(kind) => Err(marrow_fs_journal::CustodyError::Io {
                        op: CustodyOp::Unlink,
                        source: kind.into(),
                    }),
                    None => Ok(()),
                }
            }
            _ => Ok(()),
        }
    }
}

fn construction(cut: Option<Step>, removal: Option<std::io::ErrorKind>) -> Arc<Construction> {
    Arc::new(Construction {
        cut,
        removal,
        observed: Mutex::new(None),
    })
}

#[test]
fn provision_retains_the_original_failure_and_failed_cleanup_location() {
    for point in [
        Some(Step::FileSync(Body::Artifact(Artifact::Envelope))),
        None,
    ] {
        let scratch = std::mem::ManuallyDrop::new(Scratch::new("cleanup-failure"));
        eprintln!("cleanup-failure fixture: {}", scratch.base().display());
        let destination = scratch.base().join("destination");
        if point.is_none() {
            std::fs::create_dir(&destination).expect("occupied destination");
            std::fs::write(destination.join("sentinel"), b"existing destination")
                .expect("sentinel");
        }
        let (_, request) = compiled_request();
        let observer = construction(point, Some(std::io::ErrorKind::PermissionDenied));
        let error = provision_observed(&destination, request, Seam::armed(observer.clone()))
            .expect_err("provision failed");
        let stage = observer
            .observed
            .lock()
            .expect("observed stage")
            .as_ref()
            .expect("observed stage")
            .0
            .clone();
        assert!(stage.exists(), "forced cleanup failure retains owned stage");
        let cleanup = error.cleanup.as_ref().expect("typed cleanup evidence");
        assert_eq!(cleanup.stage, stage);
        assert_eq!(cleanup.source.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(matches!(
            (&point, &error.fault),
            (Some(_), ProvisionFault::Admission(_)) | (None, ProvisionFault::AlreadyProvisioned)
        ));
        assert_eq!(
            error.code(),
            if point.is_some() {
                Code::StoreIo
            } else {
                Code::StoreLocked
            }
        );
        if point.is_none() {
            assert_eq!(
                std::fs::read(destination.join("sentinel")).expect("destination untouched"),
                b"existing destination"
            );
        } else {
            assert!(!destination.exists());
        }
        // The user-facing failure must identify the actual directory whose cleanup failed.
        assert!(
            error
                .to_string()
                .contains(stage.to_str().expect("fixture path")),
            "failure lost the owned stage location: {error}"
        );
        drop(std::mem::ManuallyDrop::into_inner(scratch));
    }
}

#[test]
fn construction_failures_remove_only_the_stage_this_invocation_created() {
    for point in [
        Step::FileSync(Body::Artifact(Artifact::Envelope)),
        Step::FileSync(Body::Artifact(Artifact::Head)),
        Step::ConstructionStage,
    ] {
        let scratch = std::mem::ManuallyDrop::new(Scratch::new("construction-prefix"));
        eprintln!("construction-prefix fixture: {}", scratch.base().display());
        let destination = scratch.base().join("destination");
        let unrelated = scratch.base().join("unrelated.provisioning");
        std::fs::create_dir(&unrelated).expect("unrelated sibling");
        std::fs::write(unrelated.join("sentinel"), b"keep me").expect("sentinel");
        let (_, request) = compiled_request();
        let observer = construction(Some(point), None);
        let error = provision_observed(&destination, request, Seam::armed(observer.clone()))
            .expect_err("failed construction cannot publish");
        assert_eq!(error.code(), Code::StoreIo);
        assert!(
            error.cleanup.is_none(),
            "successful cleanup has no failure evidence"
        );
        assert!(matches!(
            error.fault,
            ProvisionFault::Admission(AdmissionError {
                fault: store_dir::AdmissionFault::Custody(
                    marrow_fs_journal::CustodyError::Io { .. }
                ),
                ..
            })
        ));
        let (stage, names) = observer
            .observed
            .lock()
            .expect("observed stage")
            .take()
            .expect("actual stage observed");
        let mut expected: Vec<std::ffi::OsString> = vec![store_dir::ENVELOPE_FILE.into()];
        if point != Step::FileSync(Body::Artifact(Artifact::Envelope)) {
            expected.push(store_dir::HEAD_FILE.into());
        }
        if point == Step::ConstructionStage {
            expected.push(store_dir::ENGINE_FILE.into());
        }
        expected.sort();
        assert_eq!(names, expected);
        assert!(!destination.exists());
        assert!(!stage.exists(), "only the owned stage is removed");
        assert_eq!(
            std::fs::read(unrelated.join("sentinel")).expect("unrelated retained"),
            b"keep me"
        );
        assert_eq!(
            std::fs::read_dir(scratch.base()).expect("parent").count(),
            1
        );
        drop(std::mem::ManuallyDrop::into_inner(scratch));
    }
}

fn compiled_request() -> (marrow_verify::VerifiedImage, ProvisionRequest) {
    let source = "resource Item { required value: int }\nstore ^items[key: int]: Item\npub fn read(key: int): int { return ^items[key].value ?? 0 }\n";
    let ids = "marrow ids v0\nmachine-written by marrow; do not edit\nid application . 01010101010101010101010101010101\nid product Item 02020202020202020202020202020202\nid field Item.value 03030303030303030303030303030303\nid root items 04040404040404040404040404040404\nid key items.key 05050505050505050505050505050505\nhigh-water 0\nend\n";
    let image = crate::test_support::compile::compile(source, ids);
    let request = ProvisionRequest {
        envelope: StoreEnvelope {
            instance: StoreInstanceId::draw().expect("instance"),
            writer_toolchain: env!("CARGO_PKG_VERSION").into(),
            engine_kind: crate::EngineKind::Redb,
            engine_format_version: marrow_kernel::durable::NATIVE_ENGINE_FORMAT_VERSION,
        },
        head: LogicalHead::provision(
            crate::active_binding(&image),
            crate::accepted_ceiling(&image),
            crate::head_map(&image).expect("head map"),
        ),
    };
    (image, request)
}

#[test]
fn a_complete_unpublished_stage_is_adopted_at_its_current_location() {
    let scratch = Scratch::new("complete-stage");
    let destination = scratch.base().join("destination");
    let stage = temp_sibling(&destination);
    create_private_dir(&stage).expect("private stage");
    let (image, request) = compiled_request();
    let instance = request.envelope.instance;
    let (owner, admitted) =
        build_in_temp(&stage, &request, Seam::NONE).expect("complete production stage");
    let (head, digest) = decode_head(&admitted).expect("head");
    let head = head.encode();
    assert_eq!(
        decode_record(&admitted).expect("record").state,
        EnvelopeState::Provision { head: digest }
    );
    assert!(!destination.exists());
    let held =
        crate::recover(&stage, crate::prepare(image.clone())).expect_err("construction owner held");
    assert!(matches!(
        held.fault,
        crate::RecoveryFault::Validation(crate::AuditError::Open(OpenError::Lock(
            NativeLockError::StoreInUse { .. }
        )))
    ));
    drop(admitted);
    drop(owner);
    assert!(matches!(
        crate::attach(&stage, crate::prepare(image.clone())),
        Err(crate::LifecycleError::Open(
            OpenError::ActivationRequired { .. }
        ))
    ));
    let recovered =
        crate::recover(&stage, crate::prepare(image.clone())).expect("adopt current stage");
    assert_eq!(recovered.instance, instance);
    assert_eq!(recovered.image_id, image.image_id());
    assert_eq!(
        std::fs::read(stage.join(crate::HEAD_FILE)).expect("head unchanged"),
        head
    );
    assert!(matches!(
        crate::attach(&stage, crate::prepare(image)).expect("active at stage"),
        crate::AttachOutcome::AlreadyActive(_)
    ));
    assert!(
        !destination.exists(),
        "recovery must not reconstruct a former destination"
    );
}

#[cfg(unix)]
enum PublicationMutation {
    Occupy,
    Substitute(PathBuf),
}

/// A publication observer: at each stage point it checks the owner, the admitted
/// descriptor and the namespace agree on one directory node and that a contender is still
/// refused, records the node, and optionally mutates the namespace there.
#[cfg(unix)]
struct PublicationObservation {
    destination: PathBuf,
    seen: Mutex<Vec<(StagePoint, u64, u64)>>,
    mutation: Mutex<Option<(StagePoint, PublicationMutation)>>,
}

#[cfg(unix)]
impl Observer for PublicationObservation {
    fn at(&self, event: Event<'_>) -> Result<(), marrow_fs_journal::CustodyError> {
        use std::os::unix::fs::MetadataExt;
        let Event::Stage {
            at,
            location,
            owner,
            admitted,
        } = event
        else {
            return Ok(());
        };
        let held = owner
            .directory_metadata()
            .expect("retained owner directory");
        let actual = std::fs::metadata(location).expect("current location");
        assert_eq!((held.dev(), held.ino()), (actual.dev(), actual.ino()));
        admitted
            .verify_location(location)
            .expect("same admitted descriptor");
        assert!(matches!(
            NativeStore::acquire_existing(location),
            Err(NativeOwnerAcquireError::Lock(
                marrow_kernel::durable::NativeLockError::StoreInUse { .. }
            ))
        ));
        self.seen
            .lock()
            .expect("seen")
            .push((at, actual.dev(), actual.ino()));
        let armed = self
            .mutation
            .lock()
            .expect("mutation")
            .as_ref()
            .is_some_and(|(point, _)| *point == at);
        if armed {
            let taken = self.mutation.lock().expect("mutation").take();
            match taken.expect("armed mutation").1 {
                PublicationMutation::Occupy => std::fs::create_dir(&self.destination).unwrap(),
                PublicationMutation::Substitute(saved) => {
                    std::fs::rename(location, saved).unwrap();
                    std::fs::create_dir(location).unwrap();
                    std::fs::write(location.join("replacement"), b"do not delete").unwrap();
                }
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn publication_observation(
    destination: &Path,
    mutation: Option<(StagePoint, PublicationMutation)>,
) -> Arc<PublicationObservation> {
    Arc::new(PublicationObservation {
        destination: destination.to_path_buf(),
        seen: Mutex::new(Vec::new()),
        mutation: Mutex::new(mutation),
    })
}

#[cfg(unix)]
#[test]
fn public_provision_holds_the_same_directory_owner_across_rename() {
    let scratch = Scratch::new("rename-owner");
    let destination = scratch.base().join("store");
    let (image, request) = compiled_request();
    let observer = publication_observation(&destination, None);
    provision_observed(&destination, request, Seam::armed(observer.clone()))
        .expect("public provision");
    let seen = observer.seen.lock().expect("seen");
    assert_eq!(seen.len(), 2);
    let (point, device, inode) = seen[0];
    assert_eq!(point, StagePoint::Built);
    assert_eq!(seen[1], (StagePoint::Published, device, inode));
    drop(seen);
    assert!(matches!(
        crate::attach(&destination, crate::prepare(image)).expect("released completed provision"),
        crate::AttachOutcome::AlreadyActive(_)
    ));
}

#[cfg(unix)]
#[test]
fn late_collision_and_identity_changes_preserve_actual_custody() {
    for case in 0..3 {
        let scratch = std::mem::ManuallyDrop::new(Scratch::new("publication-custody"));
        eprintln!(
            "preserved publication custody fixture {case}: {}",
            scratch.base().display()
        );
        let destination = scratch.base().join("store");
        let saved = scratch.base().join("retained-original");
        let (at, action) = match case {
            0 => (StagePoint::Built, PublicationMutation::Occupy),
            1 => (
                StagePoint::Built,
                PublicationMutation::Substitute(saved.clone()),
            ),
            _ => (
                StagePoint::Published,
                PublicationMutation::Substitute(saved.clone()),
            ),
        };
        let (_, request) = compiled_request();
        let instance = request.envelope.instance;
        let observer = publication_observation(&destination, Some((at, action)));
        let error = provision_observed(&destination, request, Seam::armed(observer)).unwrap_err();
        match case {
            0 => {
                assert!(matches!(error.fault, ProvisionFault::AlreadyProvisioned));
                assert!(error.cleanup.is_none());
                assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);
            }
            1 => {
                assert!(matches!(error.fault, ProvisionFault::Io(_)));
                let cleanup = error.cleanup.expect("replacement must not be removed");
                assert_eq!(
                    std::fs::read(cleanup.stage.join("replacement")).unwrap(),
                    b"do not delete"
                );
                assert!(saved.join(crate::HEAD_FILE).is_file());
                assert!(!destination.exists());
            }
            _ => {
                assert!(
                    matches!(error.fault, ProvisionFault::PublicationUncertain { instance: found, .. } if found == instance)
                );
                assert!(error.cleanup.is_none());
                assert_eq!(
                    std::fs::read(destination.join("replacement")).unwrap(),
                    b"do not delete"
                );
                assert!(saved.join(crate::HEAD_FILE).is_file());
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn inadmissible_destination_names_refuse_before_stage_creation() {
    use std::os::unix::ffi::OsStringExt;
    let scratch = Scratch::new("publication-name-admission");
    for name in [
        std::ffi::OsString::from("a\\b"),
        std::ffi::OsString::from("a:store"),
        std::ffi::OsString::from("control\u{1}"),
        std::ffi::OsString::from_vec(vec![0xff]),
    ] {
        let (_, request) = compiled_request();
        let error = provision(&scratch.base().join(name), request).unwrap_err();
        assert!(
            matches!(error.fault, ProvisionFault::Io(source) if source.kind() == std::io::ErrorKind::InvalidInput)
        );
        assert!(error.cleanup.is_none());
        assert_eq!(std::fs::read_dir(scratch.base()).unwrap().count(), 0);
    }
}

#[cfg(unix)]
#[test]
fn symlink_parent_and_missing_owner_permissions_refuse_before_staging() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("publication-parent-admission");
    let parent = scratch.base().join("parent");
    std::fs::create_dir(&parent).unwrap();
    let link = scratch.base().join("link");
    std::os::unix::fs::symlink(&parent, &link).unwrap();
    let (_, request) = compiled_request();
    assert!(provision(&link.join("store"), request).is_err());
    assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).unwrap();
    let (_, request) = compiled_request();
    let result = provision(&parent.join("store"), request);
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
}

/// The empty store shape: no roots, so no site to resolve. These cases exercise the
/// directory lifecycle, not the store's own shape.
fn rootless_layout() -> NumberedProjection {
    let projection = StoreProjection::builder()
        .finish()
        .expect("a rootless projection has no site to resolve");
    NumberedProjection::accepted(projection, &[], 0).expect("empty address mapping")
}

/// The smallest complete provision request: one durable node, a two-byte accepted
/// ceiling payload, and no store shape.
fn test_request(instance: StoreInstanceId) -> ProvisionRequest {
    let head_map = HeadMap::assign(&[LedgerIdBytes::from_bytes([0x01; 16])]).expect("head map");
    ProvisionRequest {
        envelope: StoreEnvelope {
            instance,
            writer_toolchain: "0.1.0".into(),
            engine_kind: crate::envelope::EngineKind::Redb,
            engine_format_version: 1,
        },
        head: LogicalHead::provision(
            ActiveBinding {
                image_format_version: marrow_image::IMAGE_FORMAT_VERSION,
                image_id: [0x11; 32],
                durable_contract: [0x22; 32],
                interface: [0x33; 32],
            },
            vec![0x44, 0x45],
            head_map,
        ),
    }
}

/// The classification of a directory this test can examine. A preflight that cannot look
/// is a distinct outcome with its own coverage; nothing here should reach it.
fn classify(dir: &Path) -> Preflight {
    preflight(dir).expect("this test's directories are examinable")
}

#[test]
fn preflight_classifies_absent_incomplete_complete_without_creating() {
    let scratch = Scratch::new("preflight");
    // A path under the scratch base that does not itself exist: preflight must leave
    // both it and the store directory under it alone.
    let base = scratch.named_store("base");
    let dir = base.join("store");

    // Absent: no directory. Preflight creates nothing.
    assert_eq!(classify(&dir), Preflight::Absent);
    assert!(
        !base.exists(),
        "preflight must not create the base directory"
    );
    assert!(
        !dir.exists(),
        "preflight must not create the store directory"
    );

    // Incomplete: the directory exists but lacks artifacts.
    std::fs::create_dir_all(&dir).expect("create dir");
    assert_eq!(classify(&dir), Preflight::Incomplete);
    let before: Vec<_> = read_dir_names(&dir);
    assert_eq!(classify(&dir), Preflight::Incomplete);
    assert_eq!(
        read_dir_names(&dir),
        before,
        "preflight must not add a file"
    );
}

fn open_owner(dir: &Path, instance: [u8; 16]) -> NativeStore {
    NativeStore::acquire_existing(dir)
        .expect("acquire the owner")
        .bind_and_open_existing(NativeOpenAccess::ReadWrite, instance, || {
            Ok::<_, std::convert::Infallible>(rootless_layout())
        })
        .expect("bind and open")
}

#[test]
fn opaque_native_owner_holds_exclusion_and_clean_drop_releases_it() {
    let scratch = Scratch::new("opaque-owner");
    NativeStore::provision(scratch.base()).expect("provision native engine");
    let owner = open_owner(scratch.base(), [0x51; 16]);
    assert!(matches!(
        NativeStore::acquire_existing(scratch.base()),
        Err(NativeOwnerAcquireError::Lock(_)),
    ));
    drop(owner);
    drop(open_owner(scratch.base(), [0x52; 16]));
}

/// Every admission read and callback happens under one owner, and the pair of artifacts
/// the open reports is the snapshot taken under it. A competing open attempted from
/// inside the admission callback — the innermost point of the sequence — is refused as
/// contention, and an envelope rewritten at that same point does not reach the caller,
/// because the envelope is read once under this owner and never re-read behind the head.
#[test]
fn admission_reads_are_one_snapshot_under_one_owner() {
    let scratch = Scratch::new("one-snapshot");
    let store = scratch.base().join("store");
    let original = StoreInstanceId::draw().expect("entropy");
    provision(&store, test_request(original)).expect("provision");

    let mut contended = None;
    let opened = open_admitted(&store, NativeOpenAccess::ReadWrite, Seam::NONE, |head| {
        contended = Some(match LockedStore::acquire(&store, Seam::NONE) {
            Err(OpenError::Lock(error)) => error.code(),
            Ok(_) => panic!("a competing open ran inside the admission callback"),
            Err(other) => panic!("admission ran outside its owner: {other}"),
        });
        // Rewrite the envelope at the innermost point of the sequence. A second read
        // behind the head would pick this up; one snapshot cannot.
        let replacement = StoreEnvelope {
            instance: StoreInstanceId::draw().expect("entropy"),
            writer_toolchain: "9.9.9".into(),
            engine_kind: crate::envelope::EngineKind::Redb,
            engine_format_version: 1,
        };
        let replacement = EnvelopeRecord {
            metadata: replacement,
            state: EnvelopeState::Active,
        }
        .encode()
        .expect("encode replacement record");
        std::fs::write(store.join(store_dir::ENVELOPE_FILE), replacement)
            .expect("rewrite the envelope mid-admission");
        assert_eq!(head.binding.image_id, [0x11; 32]);
        Ok::<_, std::convert::Infallible>(rootless_layout())
    })
    .unwrap_or_else(|_| panic!("the open completes under its own owner"));

    assert_eq!(contended, Some(Code::StoreLocked));
    assert_eq!(
        opened.envelope.instance, original,
        "the open reports the envelope its owner admitted, not one rewritten behind it",
    );
}

fn read_dir_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}
