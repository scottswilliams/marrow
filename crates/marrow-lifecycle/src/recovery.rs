//! Fresh validation and activation of the caller-selected current store binding.
//! No missing head write is replayed, no old acknowledgment is reconstructed,
//! and no application attachment escapes this operation.

use std::path::Path;

use marrow_codes::Code;
use marrow_image::{ImageId, StoreHeadDigest};
use marrow_kernel::durable::NativeOpenAccess;

use crate::actor::ImageAdmission;
use crate::audit::{self, AuditError, Names, StoreAudit};
use crate::envelope::{EnvelopeRecord, EnvelopeState};
use crate::provision::{AdmitError, LockedStore, OpenError};
use crate::store_dir::{AdmissionError, Artifact, StoreEntry};
use crate::{PreparedImage, StoreInstanceId};

/// The binding actually validated and activated at the current directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredStore {
    pub instance: StoreInstanceId,
    pub image_id: ImageId,
    /// Relative names moved by this attempt, at most one per metadata slot.
    pub preserved: Vec<String>,
}

/// Failure and the preservation moves already known to this attempt.
/// A lost reply cannot be reconstructed by a later invocation.
#[derive(Debug)]
pub struct RecoveryError {
    pub fault: RecoveryFault,
    pub preserved: Vec<String>,
}

impl RecoveryError {
    pub fn code(&self) -> &'static str {
        self.fault.code()
    }
}
impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fault.fmt(f)
    }
}
impl std::error::Error for RecoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.fault)
    }
}

/// One OS entropy draw for a filename, not a store identity or replay token.
fn preservation_nonce() -> std::io::Result<[u8; 16]> {
    use std::io::Read;
    let mut bytes = [0; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[derive(Debug)]
pub enum RecoveryFault {
    Validation(AuditError),
    /// The present head is not one of the exact heads recorded by Pending.
    HeadMismatch,
    Logical(Box<StoreAudit>),
    Metadata(AdmissionError),
    Io(std::io::Error),
    /// Required earlier barriers passed, but final activation was not confirmed.
    Completion {
        instance: StoreInstanceId,
        source: AuditError,
    },
}

impl RecoveryFault {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Validation(error) => error.code(),
            Self::HeadMismatch | Self::Logical(_) => Code::StoreCorruption.as_str(),
            Self::Metadata(error) => error.code(),
            Self::Io(_) => Code::StoreIo.as_str(),
            Self::Completion { .. } => Code::StoreActivationUncertain.as_str(),
        }
    }
}

impl std::fmt::Display for RecoveryFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(error) => write!(f, "{error}"),
            Self::HeadMismatch => write!(
                f,
                "the current head is not recorded by the pending transition"
            ),
            Self::Logical(report) => write!(
                f,
                "the store has {} logical integrity findings",
                report.summary.findings
            ),
            Self::Metadata(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "recovery failed: {error}"),
            Self::Completion { instance, source } => write!(
                f,
                "store {} activation is unconfirmed: {source}",
                instance.to_hex()
            ),
        }
    }
}

impl std::error::Error for RecoveryFault {}

fn validation(error: OpenError) -> RecoveryFault {
    RecoveryFault::Validation(AuditError::Open(error))
}

fn accepts_head(state: EnvelopeState, digest: StoreHeadDigest) -> bool {
    match state {
        EnvelopeState::Active => true,
        EnvelopeState::Provision { head } => head == digest,
        EnvelopeState::Rebind { old, new } => digest == old || digest == new,
    }
}

/// Validate physical and logical integrity under one owner, establish fresh
/// current-location barriers, and activate only the supplied exact stored image.
pub fn recover(dir: &Path, prepared: PreparedImage) -> Result<RecoveredStore, RecoveryError> {
    let mut preserved = Vec::new();
    match recover_inner(dir, prepared, &mut preserved) {
        Ok((instance, image_id)) => Ok(RecoveredStore {
            instance,
            image_id,
            preserved,
        }),
        Err(fault) => Err(RecoveryError { fault, preserved }),
    }
}

fn recover_inner(
    dir: &Path,
    prepared: PreparedImage,
    preserved: &mut Vec<String>,
) -> Result<(StoreInstanceId, ImageId), RecoveryFault> {
    let (image, projection) = prepared.into_parts();
    let projection = projection.ok_or(RecoveryFault::Validation(AuditError::NotExecutable))?;
    let names = Names::new(&projection);
    let admission = ImageAdmission::derive(&image, projection);
    let locked = LockedStore::acquire(dir).map_err(validation)?;
    let location = locked.directory_path().to_path_buf();
    let state = locked.envelope.state;
    let opened = locked
        .open(NativeOpenAccess::Recovery, |head, digest| {
            if !accepts_head(state, digest) {
                return Err(RecoveryFault::HeadMismatch);
            }
            admission.admit_exact(head).map_err(|error| {
                RecoveryFault::Validation(audit::open_error(AdmitError::Refused(error)))
            })
        })
        .map_err(|error| match error {
            AdmitError::Open(error) => validation(error),
            AdmitError::Refused(error) => error,
        })?;
    let inspected =
        audit::inspect(&opened, &names, image.image_id()).map_err(RecoveryFault::Validation)?;
    if !inspected.is_clean() {
        return Err(RecoveryFault::Logical(Box::new(inspected)));
    }
    for artifact in [Artifact::Envelope, Artifact::Head] {
        opened
            .directory
            .preserve_replacement(artifact, preservation_nonce, preserved)
            .map_err(RecoveryFault::Metadata)?;
    }
    let instance = opened.envelope.instance;
    let record = EnvelopeRecord {
        metadata: crate::StoreEnvelope {
            writer_toolchain: env!("CARGO_PKG_VERSION").to_owned(),
            ..opened.envelope.clone()
        },
        state: EnvelopeState::Active,
    };
    opened
        .directory
        .sync_artifacts()
        .map_err(RecoveryFault::Metadata)?;
    opened
        .directory
        .verify_location(&location)
        .map_err(RecoveryFault::Metadata)?;
    let parent = location
        .parent()
        .ok_or_else(|| RecoveryFault::Io(std::io::Error::from(std::io::ErrorKind::InvalidInput)))?;
    #[cfg(test)]
    crate::store_dir::barrier_fault::check(
        &opened.directory,
        crate::store_dir::barrier_fault::Point::RecoveryParent,
    )
    .map_err(|error| RecoveryFault::Io(std::io::Error::other(error)))?;
    crate::durable_fs::sync_dir(parent).map_err(RecoveryFault::Io)?;
    opened
        .directory
        .verify_location(&location)
        .map_err(RecoveryFault::Metadata)?;

    let finish = || -> Result<(), AuditError> {
        let metadata_error = |error| AuditError::Open(OpenError::Admission(error));
        let bytes = record
            .encode()
            .map_err(|error| metadata_error(AdmissionError::format(StoreEntry::Envelope, error)))?;
        opened
            .directory
            .replace(Artifact::Envelope, &bytes)
            .map_err(metadata_error)?;
        #[cfg(test)]
        crate::store_dir::barrier_fault::check(
            &opened.directory,
            crate::store_dir::barrier_fault::Point::RecoveryActive,
        )
        .map_err(metadata_error)?;
        opened.directory.sync().map_err(metadata_error)?;
        #[cfg(test)]
        tests::replace_before_final_read(&opened.directory, &location).map_err(metadata_error)?;
        audit::verify_published(&opened.directory, &location, &record, opened.head_digest)
    };
    finish().map_err(|source| RecoveryFault::Completion { instance, source })?;
    Ok((instance, image.image_id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EngineKind, LogicalHead, ProvisionRequest, StoreEnvelope, accepted_ceiling, active_binding,
        head_map, prepare, provision,
    };
    use marrow_verify::VerifiedImage;
    use std::path::PathBuf;

    type FinalReadReplacement = (PathBuf, Artifact, Vec<u8>);
    thread_local! {
        static FINAL_READ_REPLACEMENT: std::cell::RefCell<Option<FinalReadReplacement>> = const { std::cell::RefCell::new(None) };
    }

    fn with_final_read_replacement<T>(
        path: &Path,
        artifact: Artifact,
        bytes: Vec<u8>,
        action: impl FnOnce() -> T,
    ) -> T {
        struct Restore(Option<FinalReadReplacement>);
        impl Drop for Restore {
            fn drop(&mut self) {
                FINAL_READ_REPLACEMENT.with(|slot| *slot.borrow_mut() = self.0.take());
            }
        }
        let path = std::fs::canonicalize(path).expect("test directory");
        let _restore = Restore(
            FINAL_READ_REPLACEMENT.with(|slot| slot.replace(Some((path, artifact, bytes)))),
        );
        action()
    }

    pub(super) fn replace_before_final_read(
        dir: &crate::store_dir::AdmittedStoreDir,
        location: &Path,
    ) -> Result<(), AdmissionError> {
        let replacement = FINAL_READ_REPLACEMENT.with(|slot| {
            let mut armed = slot.borrow_mut();
            if armed.as_ref().is_some_and(|(path, _, _)| path == location) {
                armed.take()
            } else {
                None
            }
        });
        if let Some((_, artifact, bytes)) = replacement {
            dir.replace(artifact, &bytes)?;
            dir.sync()?;
        }
        Ok(())
    }

    const SOURCE: &str = "resource Counter { required value: int }\nstore ^counters[id: int]: Counter\npub fn readValue(n: int): int { return ^counters[n].value ?? 0 }\n";
    const IDS: &str = "marrow ids v0\nmachine-written by marrow; do not edit\nid application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\nid product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\nid field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\nid root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\nid key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\nhigh-water 0\nend\n";

    fn compile_bytes(source: &str) -> Vec<u8> {
        compile_with_ids(source, IDS)
    }

    fn compile_with_ids(source: &str, ids: &str) -> Vec<u8> {
        let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
        let project = marrow_project::capture(
            &manifest,
            vec![marrow_project::CapturedFile::new(
                "src/main.mw".into(),
                source.as_bytes().to_vec(),
            )],
            Some(ids.as_bytes()),
            &marrow_project::CaptureLimits::DEFAULT,
        )
        .expect("capture");
        let compiled = marrow_compile::compile(&project).expect("compile");
        compiled.image.bytes
    }

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "marrow-explicit-recovery-{}-{nonce}-{sequence}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("scratch");
            Self(path)
        }
        fn store(&self) -> PathBuf {
            self.0.join("store")
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            if std::thread::panicking() {
                eprintln!("failed recovery fixture retained at {}", self.0.display());
                return;
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn request(image: &VerifiedImage, instance: StoreInstanceId) -> ProvisionRequest {
        ProvisionRequest {
            envelope: StoreEnvelope {
                instance,
                writer_toolchain: "0.1.0".into(),
                engine_kind: EngineKind::Redb,
                engine_format_version: marrow_kernel::durable::NATIVE_ENGINE_FORMAT_VERSION,
            },
            head: LogicalHead::provision(
                active_binding(image),
                accepted_ceiling(image),
                head_map(image).expect("head map"),
            ),
        }
    }

    fn populate_counter(dir: &Path, image: &VerifiedImage) {
        use marrow_kernel::codec::{key::KeyScalar, value::RuntimeScalar};
        use marrow_kernel::durable::{DemandCoverage, Durable, EntryValue, InvocationGrant};
        use marrow_kernel::equality::ValueDomain;

        let write_site = image
            .sites()
            .iter()
            .position(|site| {
                matches!(
                    site,
                    marrow_verify::SealedSite::Flat {
                        root: 0,
                        target: marrow_verify::SealedSiteTarget::WholePayload,
                    }
                )
            })
            .expect("compiled entry write") as u16;
        let crate::AttachOutcome::AlreadyActive(mut attachment) =
            crate::attach(dir, prepare(image.clone())).expect("attach")
        else {
            panic!("initial binding");
        };
        let (_, host) = attachment.bridge();
        let mut txn = host
            .txn_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: true,
                },
            )
            .expect("transaction");
        let site = txn.site(write_site);
        txn.create_entry(
            &site,
            &[KeyScalar::Int(7)],
            EntryValue {
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(42)))],
                groups: Vec::new(),
            },
        )
        .expect("write populated entry");
        assert!(matches!(
            txn.commit(),
            marrow_kernel::durable::CommitResult::Committed
        ));
    }

    #[test]
    fn logical_corruption_refuses_recovery_before_preserving_debris_or_activating() {
        let populated = Scratch::new();
        let target = Scratch::new();
        let integer_image = marrow_verify::verify(&compile_bytes(&format!(
            "{SOURCE}\npub fn setValue(n: int, v: int) {{ transaction {{ ^counters[n] = Counter(value: v) }} }}\n"
        ))).expect("verify");
        let boolean_image = marrow_verify::verify(&compile_bytes(
            "resource Counter { required value: bool }\nstore ^counters[id: int]: Counter\npub fn readValue(n: int): bool { return ^counters[n].value ?? false }\n",
        )).expect("verify");
        provision(
            &populated.store(),
            request(&integer_image, StoreInstanceId::draw().expect("instance")),
        )
        .expect("integer store");
        populate_counter(&populated.store(), &integer_image);
        provision(
            &target.store(),
            request(&boolean_image, StoreInstanceId::draw().expect("instance")),
        )
        .expect("boolean store");
        std::fs::copy(
            populated.store().join(crate::ENGINE_FILE),
            target.store().join(crate::ENGINE_FILE),
        )
        .expect("physically valid engine with incompatible saved values");
        let head = std::fs::read(target.store().join(crate::HEAD_FILE)).expect("head");
        let envelope = std::fs::read(target.store().join(crate::ENVELOPE_FILE)).expect("envelope");
        std::fs::write(target.store().join("envelope.replacing"), b"unexplained").expect("debris");
        let error =
            recover(&target.store(), prepare(boolean_image)).expect_err("logical corruption");
        assert!(error.preserved.is_empty());
        assert_eq!(error.code(), Code::StoreCorruption.as_str());
        let RecoveryFault::Logical(report) = error.fault else {
            panic!("physical validation must reach the logical refusal");
        };
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.code == Code::StoreAuditUndecodable)
        );
        assert_eq!(
            std::fs::read(target.store().join(crate::HEAD_FILE)).expect("head unchanged"),
            head
        );
        assert_eq!(
            std::fs::read(target.store().join(crate::ENVELOPE_FILE)).expect("no activation"),
            envelope
        );
        assert_eq!(
            std::fs::read(target.store().join("envelope.replacing")).expect("debris unmoved"),
            b"unexplained"
        );
    }

    #[test]
    fn recovery_activates_an_actual_uncertain_publication_without_replaying_the_head() {
        use crate::provision::publication_sync_fault::{self, Point};
        let scratch = Scratch::new();
        let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        let instance = StoreInstanceId::draw().expect("instance");
        assert!(matches!(
            publication_sync_fault::with_failure(&scratch.store(), Point::Publication, || {
                provision(&scratch.store(), request(&image, instance))
            }),
            Err(crate::ProvisionError {
                fault: crate::ProvisionFault::PublicationUncertain { .. },
                ..
            })
        ));
        let head_before = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head");
        assert!(matches!(
            crate::attach(&scratch.store(), prepare(image.clone())),
            Err(crate::LifecycleError::Open(
                OpenError::ActivationRequired { .. }
            ))
        ));
        let receipt =
            recover(&scratch.store(), prepare(image.clone())).expect("recover published store");
        assert_eq!(
            receipt,
            RecoveredStore {
                instance,
                image_id: image.image_id(),
                preserved: Vec::new(),
            }
        );
        assert_eq!(
            std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head"),
            head_before
        );
        assert!(
            crate::audit(&scratch.store(), prepare(image))
                .expect("ordinary audit after activation")
                .is_clean()
        );
    }

    #[test]
    fn failed_replacement_prefixes_preserve_pending_authority_and_actual_bytes() {
        use crate::provision::publication_sync_fault;
        use crate::store_dir::barrier_fault::{self, Point};
        for point in [
            Point::ReplacementPartial(Artifact::Envelope),
            Point::ReplacementBody(Artifact::Envelope),
            Point::ReplacementRename(Artifact::Envelope),
        ] {
            let scratch = std::mem::ManuallyDrop::new(Scratch::new());
            eprintln!("replacement-prefix fixture: {}", scratch.0.display());
            let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
            let instance = StoreInstanceId::draw().expect("instance");
            assert!(matches!(
                publication_sync_fault::with_failure(
                    &scratch.store(),
                    publication_sync_fault::Point::Publication,
                    || provision(&scratch.store(), request(&image, instance)),
                ),
                Err(crate::ProvisionError {
                    fault: crate::ProvisionFault::PublicationUncertain { .. },
                    ..
                })
            ));
            let envelope = std::fs::read(scratch.store().join(crate::ENVELOPE_FILE))
                .expect("pending envelope");
            let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head");
            let mut active = EnvelopeRecord::decode(&envelope).expect("pending record");
            assert!(matches!(active.state, EnvelopeState::Provision { .. }));
            active.state = EnvelopeState::Active;
            let mut active_bytes = active.encode().expect("expected active record");
            if point == Point::ReplacementPartial(Artifact::Envelope) {
                active_bytes.truncate(active_bytes.len() / 2);
            }
            std::fs::write(
                scratch.store().join("head.replacing"),
                b"prior interrupted head",
            )
            .expect("prior debris");
            let error = barrier_fault::with_failure(&scratch.store(), point, || {
                recover(&scratch.store(), prepare(image.clone()))
            })
            .expect_err("unsynced replacement cannot finish activation");
            assert_eq!(error.code(), Code::StoreActivationUncertain.as_str());
            assert!(
                matches!(error.fault, RecoveryFault::Completion { instance: found, .. } if found == instance)
            );
            assert_eq!(error.preserved.len(), 1);
            let prior = scratch.store().join(&error.preserved[0]);
            assert_eq!(
                std::fs::read(&prior).expect("earlier preservation retained"),
                b"prior interrupted head"
            );
            assert_eq!(
                std::fs::read(scratch.store().join(crate::ENVELOPE_FILE))
                    .expect("authoritative envelope"),
                envelope
            );
            assert_eq!(
                std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("authoritative head"),
                head
            );
            assert_eq!(
                std::fs::read(scratch.store().join("envelope.replacing"))
                    .expect("actual failed replacement"),
                active_bytes
            );
            assert!(matches!(
                crate::attach(&scratch.store(), prepare(image.clone())),
                Err(crate::LifecycleError::Open(
                    OpenError::ActivationRequired { .. }
                ))
            ));
            let receipt =
                recover(&scratch.store(), prepare(image.clone())).expect("fresh explicit recovery");
            assert_eq!(receipt.instance, instance);
            assert_eq!(receipt.image_id, image.image_id());
            assert_eq!(receipt.preserved.len(), 1);
            assert_ne!(receipt.preserved, error.preserved);
            assert_eq!(
                std::fs::read(scratch.store().join(&receipt.preserved[0]))
                    .expect("preserved actual replacement"),
                active_bytes
            );
            assert_eq!(
                std::fs::read(&prior).expect("earlier preservation unchanged"),
                b"prior interrupted head"
            );
            assert_eq!(
                std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head not replayed"),
                head
            );
            assert!(!scratch.store().join("envelope.replacing").exists());
            assert!(
                crate::audit(&scratch.store(), prepare(image))
                    .expect("ordinary audit after recovery")
                    .is_clean()
            );
            drop(std::mem::ManuallyDrop::into_inner(scratch));
        }
    }

    #[test]
    fn recovery_preserves_occupied_replacements_before_activation() {
        let scratch = Scratch::new();
        let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        provision(
            &scratch.store(),
            request(&image, StoreInstanceId::draw().expect("instance")),
        )
        .expect("provision");
        std::fs::write(
            scratch.store().join("envelope.replacing"),
            b"partial envelope",
        )
        .expect("envelope debris");
        std::fs::write(scratch.store().join("head.replacing"), b"partial head")
            .expect("head debris");
        let receipt = match recover(&scratch.store(), prepare(image)) {
            Ok(receipt) => receipt,
            Err(error) => {
                let original = scratch.0.clone();
                std::mem::forget(scratch);
                panic!(
                    "eligible recovery did not preserve replacement files: {error}; preserve {}",
                    original.display()
                );
            }
        };
        assert_eq!(receipt.preserved.len(), 2);
        for (name, bytes) in receipt
            .preserved
            .iter()
            .zip([b"partial envelope".as_slice(), b"partial head".as_slice()])
        {
            assert_eq!(
                std::fs::read(scratch.store().join(name)).expect("preserved bytes"),
                bytes
            );
        }
        assert!(!scratch.store().join("envelope.replacing").exists());
        assert!(!scratch.store().join("head.replacing").exists());
    }

    #[cfg(unix)]
    #[test]
    fn preservation_keeps_file_identity_permissions_and_uninterpreted_bytes() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        for slot in ["envelope.replacing", "head.replacing"] {
            for bytes in [b"".as_slice(), b"partial", &[0xff, 0, 0xfe, 0x80]] {
                let scratch = Scratch::new();
                provision(
                    &scratch.store(),
                    request(&image, StoreInstanceId::draw().expect("instance")),
                )
                .expect("provision");
                let path = scratch.store().join(slot);
                std::fs::write(&path, bytes).expect("debris");
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))
                    .expect("permissions");
                let before = std::fs::symlink_metadata(&path).expect("original metadata");
                let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head");
                let receipt = recover(&scratch.store(), prepare(image.clone())).expect("recover");
                assert_eq!(receipt.preserved.len(), 1);
                let moved = scratch.store().join(&receipt.preserved[0]);
                let after = std::fs::symlink_metadata(&moved).expect("preserved metadata");
                assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
                assert_eq!(after.mode(), before.mode());
                assert_eq!(after.nlink(), 1);
                assert_eq!(std::fs::read(&moved).expect("preserved bytes"), bytes);
                assert!(!path.exists());
                assert_eq!(
                    std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head unchanged"),
                    head
                );
                let repeated =
                    recover(&scratch.store(), prepare(image.clone())).expect("repeat recovery");
                assert!(repeated.preserved.is_empty());
                assert_eq!(
                    std::fs::read(&moved).expect("prior preservation retained"),
                    bytes
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn recovery_refuses_nonexclusive_replacement_shapes_without_moving_them() {
        use std::os::unix::fs::MetadataExt;

        #[derive(Clone, Copy)]
        enum Shape {
            Directory,
            Symlink,
            Hardlink,
        }
        let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        for slot in ["envelope.replacing", "head.replacing"] {
            for shape in [Shape::Directory, Shape::Symlink, Shape::Hardlink] {
                let scratch = Scratch::new();
                provision(
                    &scratch.store(),
                    request(&image, StoreInstanceId::draw().expect("instance")),
                )
                .expect("provision");
                let path = scratch.store().join(slot);
                let peer = scratch.0.join("peer");
                std::fs::write(&peer, b"peer bytes").expect("peer");
                match shape {
                    Shape::Directory => std::fs::create_dir(&path).expect("directory"),
                    Shape::Symlink => std::os::unix::fs::symlink(&peer, &path).expect("symlink"),
                    Shape::Hardlink => std::fs::hard_link(&peer, &path).expect("hard link"),
                }
                let before = std::fs::symlink_metadata(&path).expect("original metadata");
                let envelope =
                    std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope");
                let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head");
                let error = recover(&scratch.store(), prepare(image.clone())).expect_err("refuse");
                assert!(error.preserved.is_empty());
                let RecoveryFault::Metadata(error) = error.fault else {
                    panic!("expected metadata refusal");
                };
                match shape {
                    Shape::Directory => assert!(matches!(
                        error.fault,
                        crate::AdmissionFault::Custody(crate::CustodyError::WrongNodeKind { .. })
                    )),
                    Shape::Symlink => assert!(matches!(
                        error.fault,
                        crate::AdmissionFault::Custody(crate::CustodyError::SymlinkRefused { .. })
                    )),
                    Shape::Hardlink => assert!(matches!(
                        error.fault,
                        crate::AdmissionFault::MultiplyLinked { links: 2 }
                    )),
                }
                let after = std::fs::symlink_metadata(&path).expect("refused object retained");
                assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
                assert_eq!(after.mode(), before.mode());
                assert_eq!(after.nlink(), before.nlink());
                assert_eq!(std::fs::read(&peer).expect("peer unchanged"), b"peer bytes");
                assert_eq!(
                    std::fs::read(scratch.store().join(crate::ENVELOPE_FILE))
                        .expect("no activation"),
                    envelope
                );
                assert_eq!(
                    std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head unchanged"),
                    head
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_later_preservation_failure_reports_the_earlier_move() {
        let scratch = Scratch::new();
        let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        provision(
            &scratch.store(),
            request(&image, StoreInstanceId::draw().expect("instance")),
        )
        .expect("provision");
        let envelope = std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope");
        std::fs::write(scratch.store().join("envelope.replacing"), b"first partial")
            .expect("first slot");
        let peer = scratch.store().join("peer");
        std::fs::write(&peer, b"outside bytes").expect("peer");
        std::os::unix::fs::symlink(&peer, scratch.store().join("head.replacing"))
            .expect("refused slot");
        let error = recover(&scratch.store(), prepare(image)).expect_err("second slot refuses");
        assert!(matches!(
            error.fault,
            RecoveryFault::Metadata(AdmissionError {
                fault: crate::AdmissionFault::Custody(crate::CustodyError::SymlinkRefused { .. }),
                ..
            })
        ));
        assert_eq!(error.preserved.len(), 1);
        assert_eq!(
            std::fs::read(scratch.store().join(&error.preserved[0])).expect("known move"),
            b"first partial"
        );
        assert!(!scratch.store().join("envelope.replacing").exists());
        assert_eq!(
            std::fs::read_link(scratch.store().join("head.replacing")).expect("link retained"),
            peer
        );
        assert_eq!(
            std::fs::read(&peer).expect("peer unchanged"),
            b"outside bytes"
        );
        assert_eq!(
            std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("no activation"),
            envelope
        );
    }

    #[test]
    fn a_failed_preservation_barrier_reports_the_move_without_creating_a_replacement() {
        let scratch = Scratch::new();
        let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        provision(
            &scratch.store(),
            request(&image, StoreInstanceId::draw().expect("instance")),
        )
        .expect("provision");
        let before = std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope");
        std::fs::write(scratch.store().join("envelope.replacing"), b"partial").expect("debris");
        let error = crate::store_dir::barrier_fault::with_failure(
            &scratch.store(),
            crate::store_dir::barrier_fault::Point::Preservation,
            || recover(&scratch.store(), prepare(image)),
        )
        .expect_err("move barrier failed");
        assert!(matches!(
            error.fault,
            RecoveryFault::Metadata(AdmissionError {
                fault: crate::AdmissionFault::Custody(crate::CustodyError::Io {
                    op: "preservation directory sync",
                    ..
                }),
                ..
            })
        ));
        assert_eq!(error.preserved.len(), 1);
        assert_eq!(
            std::fs::read(scratch.store().join(&error.preserved[0])).expect("known move"),
            b"partial"
        );
        assert!(!scratch.store().join("envelope.replacing").exists());
        assert_eq!(
            std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("no activation write"),
            before
        );
    }

    #[test]
    fn failed_rebind_activation_reports_uncertainty_after_the_new_head_is_visible() {
        use crate::store_dir::barrier_fault::{self, Point};
        let scratch = Scratch::new();
        let old = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        let new =
            marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 1"))).expect("verify");
        let instance = StoreInstanceId::draw().expect("instance");
        provision(&scratch.store(), request(&old, instance)).expect("provision");
        let error = match barrier_fault::with_failure(&scratch.store(), Point::RebindActive, || {
            crate::attach(&scratch.store(), prepare(new.clone()))
        }) {
            Err(error) => error,
            Ok(_) => panic!("failed activation must not return an attachment"),
        };
        let head = crate::LogicalHead::decode(
            &std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("actual head"),
        )
        .expect("decode head");
        assert_eq!(head.binding, active_binding(&new));
        if error.code() != Code::StoreActivationUncertain.as_str() {
            let original = scratch.0.clone();
            std::mem::forget(scratch);
            panic!(
                "visible new binding lost activation uncertainty: {error:?}; preserve {}",
                original.display()
            );
        }
        assert!(matches!(
            error,
            crate::LifecycleError::ActivationUncertain { instance: found, .. } if found == instance
        ));
    }

    #[test]
    fn interrupted_rebind_adopts_actual_head_and_preserves_populated_data() {
        use crate::store_dir::barrier_fault::{self, Point};

        let source = format!(
            "{SOURCE}\npub fn setValue(n: int, v: int) {{ transaction {{ ^counters[n] = Counter(value: v) }} }}\n"
        );
        let old = marrow_verify::verify(&compile_bytes(&source)).expect("verify");
        let new =
            marrow_verify::verify(&compile_bytes(&source.replace("?? 0", "?? 1"))).expect("verify");
        for point in [
            Point::RebindPending,
            Point::ReplacementBody(Artifact::Head),
            Point::RebindHead,
        ] {
            let scratch = std::mem::ManuallyDrop::new(Scratch::new());
            eprintln!("rebind-prefix fixture: {}", scratch.0.display());
            let instance = StoreInstanceId::draw().expect("instance");
            provision(&scratch.store(), request(&old, instance)).expect("provision");
            populate_counter(&scratch.store(), &old);
            let before = crate::audit(&scratch.store(), prepare(old.clone())).expect("before");
            assert_eq!(before.summary.entries, 1);
            let error = match barrier_fault::with_failure(&scratch.store(), point, || {
                crate::attach(&scratch.store(), prepare(new.clone()))
            }) {
                Err(error) => error,
                Ok(_) => panic!("failed barrier must not return an attachment"),
            };
            assert!(matches!(error, crate::LifecycleError::Metadata(_)));
            let record = EnvelopeRecord::decode(
                &std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("pending"),
            )
            .expect("record");
            assert!(matches!(record.state, EnvelopeState::Rebind { .. }));
            let replacement = if point == Point::ReplacementBody(Artifact::Head) {
                let bytes = std::fs::read(scratch.store().join("head.replacing"))
                    .expect("unsynced replacement retained");
                assert_eq!(
                    LogicalHead::decode(&bytes)
                        .expect("real replacement head")
                        .binding,
                    active_binding(&new)
                );
                Some(bytes)
            } else {
                None
            };
            let (actual_image, other_image) = if point != Point::RebindHead {
                (&old, &new)
            } else {
                (&new, &old)
            };
            let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("actual head");
            assert_eq!(
                LogicalHead::decode(&head).expect("head").binding,
                active_binding(actual_image)
            );
            assert!(matches!(
                crate::attach(&scratch.store(), prepare(actual_image.clone())),
                Err(crate::LifecycleError::Open(
                    OpenError::ActivationRequired { .. }
                ))
            ));
            let rejected = recover(&scratch.store(), prepare(other_image.clone()))
                .expect_err("recovery cannot replay a different head");
            assert!(matches!(
                rejected.fault,
                RecoveryFault::Validation(AuditError::ImageNotActive)
            ));
            assert!(rejected.preserved.is_empty());
            if let Some(bytes) = &replacement {
                assert_eq!(
                    &std::fs::read(scratch.store().join("head.replacing"))
                        .expect("rejected recovery leaves replacement"),
                    bytes
                );
            }
            let receipt =
                recover(&scratch.store(), prepare(actual_image.clone())).expect("recover");
            assert_eq!(receipt.instance, instance);
            assert_eq!(receipt.image_id, actual_image.image_id());
            if let Some(bytes) = replacement {
                assert_eq!(receipt.preserved.len(), 1);
                assert_eq!(
                    std::fs::read(scratch.store().join(&receipt.preserved[0]))
                        .expect("fresh recovery preserves actual head bytes"),
                    bytes
                );
            } else {
                assert!(receipt.preserved.is_empty());
            }
            assert_eq!(
                std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head not replayed"),
                head
            );
            let after =
                crate::audit(&scratch.store(), prepare(actual_image.clone())).expect("after");
            assert!(after.is_clean());
            assert_eq!(after.summary.entries, 1);
            assert_eq!(after.digest, before.digest);
            assert!(matches!(
                crate::attach(&scratch.store(), prepare(actual_image.clone())).expect("recovered"),
                crate::AttachOutcome::AlreadyActive(_)
            ));
            drop(std::mem::ManuallyDrop::into_inner(scratch));
        }
    }

    fn sparse_images() -> (Vec<u8>, Vec<u8>) {
        let source = format!(
            "{SOURCE}\npub fn setValue(n: int, v: int) {{ transaction {{ ^counters[n] = Counter(value: v) }} }}\n"
        );
        let old = compile_bytes(&source);
        let ids = IDS.replace(
            "high-water",
            "id field Counter.extra 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\nhigh-water",
        );
        let source = SOURCE.replace("required value", "extra: int\nrequired value")
            + "\npub fn readExtra(n: int): int { return ^counters[n].extra ?? -1 }\n";
        let new = compile_with_ids(&source, &ids);
        (old, new)
    }

    fn store_bytes(dir: &Path) -> std::collections::BTreeMap<std::ffi::OsString, (Vec<u8>, u32)> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::read_dir(dir)
            .expect("store members")
            .map(|entry| {
                let entry = entry.expect("member");
                let metadata = std::fs::symlink_metadata(entry.path()).expect("metadata");
                assert!(
                    metadata.is_file(),
                    "fixture has only regular store artifacts"
                );
                (
                    entry.file_name(),
                    (
                        std::fs::read(entry.path()).expect("artifact"),
                        metadata.permissions().mode(),
                    ),
                )
            })
            .collect()
    }

    #[test]
    fn sparse_apply_refusals_preserve_populated_store_bytes() {
        use crate::ApplyError;
        use marrow_image::{CeilingDescriptor, CeilingId};
        let (old, new) = sparse_images();
        let old = marrow_verify::verify(&old).expect("old image");
        let new = marrow_verify::verify(&new).expect("new image");
        let scratch = Scratch::new();
        provision(
            &scratch.store(),
            request(&old, StoreInstanceId::draw().expect("instance")),
        )
        .expect("provision");
        populate_counter(&scratch.store(), &old);
        let before = store_bytes(&scratch.store());
        let ceiling =
            CeilingDescriptor::from_payload(&accepted_ceiling(&old)).expect("old ceiling");
        let proposed = ceiling
            .expanded(
                &new.demand_union(),
                crate::head::MAX_ACCEPTED_CEILING_BYTES as usize,
            )
            .expect("bounded union");
        assert_ne!(ceiling.ceiling_id(), proposed.ceiling_id());
        let image_ceiling = CeilingDescriptor::from_demand_union(new.demand_union());
        assert_ne!(image_ceiling.ceiling_id(), proposed.ceiling_id());
        for accepted in [
            None,
            Some(CeilingId::from_bytes([0; 32])),
            Some(image_ceiling.ceiling_id()),
        ] {
            let error = crate::apply(
                &scratch.store(),
                prepare(old.clone()),
                prepare(new.clone()),
                accepted,
            )
            .expect_err("explicit expansion required");
            let ApplyError::CeilingUnaccepted {
                old: prior,
                proposed: found,
                added,
            } = error
            else {
                panic!("wrong refusal: {error:?}")
            };
            assert_eq!(prior, ceiling.ceiling_id());
            assert_eq!(found, proposed.ceiling_id());
            assert!(
                added
                    .iter()
                    .any(|effect| effect.place.as_deref() == Some("^counters.extra"))
            );
            assert_eq!(store_bytes(&scratch.store()), before);
        }
        let error = crate::apply(
            &scratch.store(),
            prepare(old.clone()),
            prepare(old.clone()),
            Some(CeilingId::from_bytes([0; 32])),
        )
        .expect_err("wrong ID even without expansion");
        assert!(
            matches!(error, ApplyError::CeilingUnaccepted { old: prior, proposed, added } if prior == ceiling.ceiling_id() && proposed == prior && added.is_empty())
        );
        assert_eq!(store_bytes(&scratch.store()), before);
        let changed = marrow_verify::verify(&compile_bytes(&SOURCE.replace("required ", "")))
            .expect("changed requiredness");
        assert!(matches!(
            crate::apply(
                &scratch.store(),
                prepare(old.clone()),
                prepare(changed),
                None
            ),
            Err(ApplyError::Unsupported)
        ));
        assert_eq!(store_bytes(&scratch.store()), before);
        let receipt = crate::apply(
            &scratch.store(),
            prepare(old.clone()),
            prepare(new.clone()),
            Some(proposed.ceiling_id()),
        )
        .expect("exact union accepted");
        assert_eq!(receipt.old_ceiling, ceiling.ceiling_id());
        assert_eq!(receipt.ceiling, proposed.ceiling_id());
        let head = LogicalHead::decode(
            &std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("applied Head"),
        )
        .expect("Head");
        assert_eq!(head.accepted_ceiling, proposed.atom_set_payload());
        let applied = store_bytes(&scratch.store());
        assert!(matches!(
            crate::apply(
                &scratch.store(),
                prepare(old),
                prepare(new),
                Some(proposed.ceiling_id())
            ),
            Err(ApplyError::Lifecycle(crate::LifecycleError::Audit(
                AuditError::ContractChanged(_)
            )))
        ));
        assert_eq!(store_bytes(&scratch.store()), applied);
    }

    #[test]
    fn sparse_apply_rejects_incompatible_graphs_without_store_changes() {
        let (old, _) = sparse_images();
        let old = marrow_verify::verify(&old).expect("old image");
        let scratch = Scratch::new();
        provision(
            &scratch.store(),
            request(&old, StoreInstanceId::draw().expect("instance")),
        )
        .expect("provision");
        populate_counter(&scratch.store(), &old);
        let before = store_bytes(&scratch.store());
        let ids = IDS.replace(
            "high-water",
            "id field Counter.extra 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\nid index counters.byValue 10101010101010101010101010101010\nid sum Option[int] 11111111111111111111111111111111\nid member Option[int].none 12121212121212121212121212121212\nid member Option[int].some 13131313131313131313131313131313\nhigh-water",
        );
        for (label, fields, key, suffix) in [
            ("changed value", "required value: bool", "int", ""),
            ("removed field", "extra: int", "int", ""),
            ("changed requiredness", "value: int", "int", ""),
            (
                "new required field",
                "required value: int\nrequired extra: int",
                "int",
                "",
            ),
            (
                "new composite field",
                "required value: int\nextra: Option<int>",
                "int",
                "",
            ),
            ("changed key", "required value: int", "string", ""),
            (
                "new index",
                "required value: int",
                "int",
                "{ index byValue[value] unique }",
            ),
        ] {
            let source = format!(
                "resource Counter {{ {fields} }}\nstore ^counters[id: {key}]: Counter {suffix}\npub fn bootstrap(): int {{ return 0 }}\n"
            );
            let new = marrow_verify::verify(&compile_with_ids(&source, &ids))
                .unwrap_or_else(|error| panic!("{label}: {error:?}"));
            assert!(
                matches!(
                    crate::apply(&scratch.store(), prepare(old.clone()), prepare(new), None),
                    Err(crate::ApplyError::Unsupported)
                ),
                "{label}"
            );
            assert_eq!(store_bytes(&scratch.store()), before, "{label}");
        }
    }

    #[test]
    fn sparse_apply_rejects_changed_index_meaning_without_store_changes() {
        let source = format!(
            "{}\npub fn setValue(n: int, v: int) {{ transaction {{ ^counters[n] = Counter(value: v) }} }}\n",
            SOURCE.replace(
                ": Counter\n",
                ": Counter { index byValue[value, id] unique }\n"
            )
        );
        let ids = IDS.replace(
            "high-water",
            "id index counters.byValue 10101010101010101010101010101010\nhigh-water",
        );
        let old = marrow_verify::verify(&compile_with_ids(&source, &ids)).expect("indexed image");
        let scratch = Scratch::new();
        provision(
            &scratch.store(),
            request(&old, StoreInstanceId::draw().expect("instance")),
        )
        .expect("provision");
        populate_counter(&scratch.store(), &old);
        assert!(
            crate::audit(&scratch.store(), prepare(old.clone()))
                .expect("old audit")
                .is_clean()
        );
        let before = store_bytes(&scratch.store());
        for replacement in [
            "",
            "index byValue[id, value] unique",
            "index byValue[value, id]",
        ] {
            let changed = source.replace("index byValue[value, id] unique", replacement);
            let new =
                marrow_verify::verify(&compile_with_ids(&changed, &ids)).expect("changed index");
            assert!(
                matches!(
                    crate::apply(&scratch.store(), prepare(old.clone()), prepare(new), None),
                    Err(crate::ApplyError::Unsupported)
                ),
                "{replacement}"
            );
            assert_eq!(store_bytes(&scratch.store()), before, "{replacement}");
        }
    }

    #[test]
    fn sparse_apply_preserves_populated_irregular_addresses_and_refuses_exhaustion() {
        let (old, new) = sparse_images();
        let old = marrow_verify::verify(&old).expect("old image");
        let new = marrow_verify::verify(&new).expect("new image");
        let ceiling = marrow_image::CeilingDescriptor::from_payload(&accepted_ceiling(&old))
            .expect("old ceiling")
            .expanded(
                &new.demand_union(),
                crate::head::MAX_ACCEPTED_CEILING_BYTES as usize,
            )
            .expect("bounded union")
            .ceiling_id();
        for high_water in [90, u32::MAX] {
            let scratch = Scratch::new();
            let mut request = request(&old, StoreInstanceId::draw().expect("instance"));
            let mut encoded = Vec::new();
            request.head.head_map.encode(&mut encoded);
            assert_eq!(request.head.head_map.len(), 2);
            encoded[..4].copy_from_slice(&high_water.to_be_bytes());
            for (index, entry) in encoded[8..].chunks_exact_mut(20).enumerate() {
                entry[16..].copy_from_slice(&(17 - index as u32 * 5).to_be_bytes());
            }
            request.head.head_map =
                crate::HeadMap::decode(&mut crate::codec::Reader::new(&encoded))
                    .expect("valid irregular map before population");
            let old_map = request.head.head_map.clone();
            provision(&scratch.store(), request).expect("provision");
            populate_counter(&scratch.store(), &old);
            let before =
                crate::audit(&scratch.store(), prepare(old.clone())).expect("populated old");
            let bytes = store_bytes(&scratch.store());
            let result = crate::apply(
                &scratch.store(),
                prepare(old.clone()),
                prepare(new.clone()),
                Some(ceiling),
            );
            if high_water == u32::MAX {
                assert!(matches!(result, Err(crate::ApplyError::Limit)));
                assert_eq!(store_bytes(&scratch.store()), bytes);
                continue;
            }
            result.expect("apply at actual high-water");
            let head = LogicalHead::decode(
                &std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("Head"),
            )
            .expect("decode");
            for entry in old_map.entries() {
                assert_eq!(
                    head.head_map.number_of(&entry.ledger_id),
                    Some(entry.number)
                );
            }
            assert_eq!(head.head_map.len(), old_map.len() + 1);
            assert_eq!(head.head_map.next_number(), high_water + 1);
            let new_entry = head
                .head_map
                .entries()
                .iter()
                .find(|entry| old_map.number_of(&entry.ledger_id).is_none())
                .expect("added field");
            assert_eq!(new_entry.number, high_water);
            let after = crate::audit(&scratch.store(), prepare(new.clone())).expect("new layout");
            assert!(after.is_clean());
            assert_eq!(after.summary.entries, 1);
            assert_eq!(after.digest, before.digest);
        }
    }

    #[test]
    fn interrupted_sparse_apply_recovers_only_the_actual_head() {
        use crate::store_dir::barrier_fault::{self, Point};
        use crate::{ApplyError, LifecycleError};
        let (old, new) = sparse_images();
        let old = marrow_verify::verify(&old).expect("old image");
        let new = marrow_verify::verify(&new).expect("new image");
        let ceiling = marrow_image::CeilingDescriptor::from_payload(&accepted_ceiling(&old))
            .expect("old ceiling")
            .expanded(
                &new.demand_union(),
                crate::head::MAX_ACCEPTED_CEILING_BYTES as usize,
            )
            .expect("bounded union")
            .ceiling_id();
        for point in [
            Point::RebindPending,
            Point::ReplacementBody(Artifact::Head),
            Point::RebindHead,
            Point::RebindActive,
        ] {
            let scratch = Scratch::new();
            let instance = StoreInstanceId::draw().expect("instance");
            provision(&scratch.store(), request(&old, instance)).expect("provision");
            populate_counter(&scratch.store(), &old);
            let before =
                crate::audit(&scratch.store(), prepare(old.clone())).expect("old logical contents");
            let error = barrier_fault::with_failure(&scratch.store(), point, || {
                crate::apply(
                    &scratch.store(),
                    prepare(old.clone()),
                    prepare(new.clone()),
                    Some(ceiling),
                )
            })
            .expect_err("publication barrier fails");
            let record = EnvelopeRecord::decode(
                &std::fs::read(scratch.store().join(crate::ENVELOPE_FILE))
                    .expect("actual envelope"),
            )
            .expect("envelope");
            if point == Point::RebindActive {
                assert_eq!(record.state, EnvelopeState::Active);
                assert!(
                    matches!(error, ApplyError::Lifecycle(LifecycleError::ActivationUncertain { instance: found, .. }) if found == instance)
                );
            } else {
                assert!(matches!(record.state, EnvelopeState::Rebind { .. }));
                assert!(matches!(
                    error,
                    ApplyError::Lifecycle(LifecycleError::Metadata(_))
                ));
            }
            let (selected, other) = if matches!(
                point,
                Point::RebindPending | Point::ReplacementBody(Artifact::Head)
            ) {
                (&old, &new)
            } else {
                (&new, &old)
            };
            let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("actual Head");
            let replacement = if point == Point::ReplacementBody(Artifact::Head) {
                let bytes = std::fs::read(scratch.store().join("head.replacing"))
                    .expect("retained replacement");
                assert_eq!(
                    LogicalHead::decode(&bytes)
                        .expect("replacement Head")
                        .binding,
                    active_binding(&new)
                );
                Some(bytes)
            } else {
                None
            };
            assert_eq!(
                LogicalHead::decode(&head).expect("Head").binding,
                active_binding(selected)
            );
            let rejected = recover(&scratch.store(), prepare(other.clone()))
                .expect_err("wrong selected image");
            assert!(matches!(
                rejected.fault,
                RecoveryFault::Validation(AuditError::ContractChanged(_))
            ));
            assert!(rejected.preserved.is_empty());
            if let Some(bytes) = &replacement {
                assert_eq!(
                    &std::fs::read(scratch.store().join("head.replacing"))
                        .expect("wrong image preserves replacement"),
                    bytes
                );
            }
            let recovered =
                recover(&scratch.store(), prepare(selected.clone())).expect("recover actual image");
            assert_eq!(recovered.instance, instance);
            assert_eq!(recovered.image_id, selected.image_id());
            if let Some(bytes) = replacement {
                assert_eq!(recovered.preserved.len(), 1);
                assert_eq!(
                    std::fs::read(scratch.store().join(&recovered.preserved[0]))
                        .expect("preserved replacement"),
                    bytes
                );
            } else {
                assert!(recovered.preserved.is_empty());
            }
            assert_eq!(
                std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("Head not replayed"),
                head
            );
            let after = crate::audit(&scratch.store(), prepare(selected.clone()))
                .expect("recovered contents");
            assert!(after.is_clean());
            assert_eq!(after.summary.entries, 1);
            assert_eq!(after.digest, before.digest);
        }
    }

    #[test]
    fn sparse_apply_final_verification_preserves_uncertainty_and_instance() {
        use crate::actor::binding_fault::{self, Mutation, Point};
        let (old, new) = sparse_images();
        let old = marrow_verify::verify(&old).expect("old image");
        let new = marrow_verify::verify(&new).expect("new image");
        let ceiling = marrow_image::CeilingDescriptor::from_payload(&accepted_ceiling(&old))
            .expect("old ceiling")
            .expanded(
                &new.demand_union(),
                crate::head::MAX_ACCEPTED_CEILING_BYTES as usize,
            )
            .expect("union")
            .ceiling_id();
        for artifact in [Artifact::Head, Artifact::Envelope] {
            let scratch = Scratch::new();
            let instance = StoreInstanceId::draw().expect("instance");
            let req = request(&old, instance);
            let replacement = match artifact {
                Artifact::Head => req.head.encode(),
                Artifact::Envelope => {
                    let mut metadata = req.envelope.clone();
                    metadata.writer_toolchain = "substituted after publication".into();
                    EnvelopeRecord {
                        metadata,
                        state: EnvelopeState::Active,
                    }
                    .encode()
                    .expect("valid envelope")
                }
            };
            provision(&scratch.store(), req).expect("provision");
            populate_counter(&scratch.store(), &old);
            let error = binding_fault::with_mutation(
                &scratch.store(),
                Point::Activated,
                Mutation::Metadata(artifact, replacement.clone()),
                || {
                    crate::apply(
                        &scratch.store(),
                        prepare(old.clone()),
                        prepare(new.clone()),
                        Some(ceiling),
                    )
                },
            )
            .expect_err("changed final metadata cannot yield a receipt");
            assert!(
                matches!(error, crate::ApplyError::Lifecycle(crate::LifecycleError::ActivationUncertain { instance: found, .. }) if found == instance)
            );
            let name = match artifact {
                Artifact::Head => crate::HEAD_FILE,
                Artifact::Envelope => crate::ENVELOPE_FILE,
            };
            assert_eq!(
                std::fs::read(scratch.store().join(name)).expect("actual changed metadata"),
                replacement
            );
        }
    }

    #[test]
    fn sparse_apply_refuses_logical_corruption_before_publication() {
        let (integer, _) = sparse_images();
        let integer = marrow_verify::verify(&integer).expect("integer image");
        let source = "resource Counter { required value: bool }\nstore ^counters[id: int]: Counter\npub fn readValue(n: int): bool { return ^counters[n].value ?? false }\n";
        let old = marrow_verify::verify(&compile_bytes(source)).expect("boolean image");
        let ids = IDS.replace(
            "high-water",
            "id field Counter.extra 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\nhigh-water",
        );
        let new_source = source.replace("required value", "extra: int\nrequired value")
            + "\npub fn readExtra(n: int): int { return ^counters[n].extra ?? -1 }\n";
        let new =
            marrow_verify::verify(&compile_with_ids(&new_source, &ids)).expect("new boolean image");
        let populated = Scratch::new();
        let target = Scratch::new();
        provision(
            &populated.store(),
            request(&integer, StoreInstanceId::draw().expect("instance")),
        )
        .expect("integer store");
        populate_counter(&populated.store(), &integer);
        provision(
            &target.store(),
            request(&old, StoreInstanceId::draw().expect("instance")),
        )
        .expect("boolean store");
        std::fs::copy(
            populated.store().join(crate::ENGINE_FILE),
            target.store().join(crate::ENGINE_FILE),
        )
        .expect("physical store with wrong logical values");
        let before = store_bytes(&target.store());
        let ceiling = marrow_image::CeilingDescriptor::from_payload(&accepted_ceiling(&old))
            .expect("old ceiling")
            .expanded(
                &new.demand_union(),
                crate::head::MAX_ACCEPTED_CEILING_BYTES as usize,
            )
            .expect("union")
            .ceiling_id();
        let error = crate::apply(&target.store(), prepare(old), prepare(new), Some(ceiling))
            .expect_err("invalid OLD data");
        let crate::ApplyError::Lifecycle(crate::LifecycleError::Invalid(report)) = error else {
            panic!("logical audit must refuse: {error:?}")
        };
        assert!(!report.is_clean());
        assert_eq!(store_bytes(&target.store()), before);
    }

    #[test]
    fn interrupted_recovery_preserves_known_moves_and_requires_fresh_completion() {
        use crate::provision::publication_sync_fault;
        use crate::store_dir::barrier_fault::{self, Point};

        let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        for point in [
            Point::RecoveryArtifacts,
            Point::RecoveryParent,
            Point::RecoveryActive,
        ] {
            let scratch = Scratch::new();
            let instance = StoreInstanceId::draw().expect("instance");
            assert!(matches!(
                publication_sync_fault::with_failure(
                    &scratch.store(),
                    publication_sync_fault::Point::Publication,
                    || provision(&scratch.store(), request(&image, instance))
                ),
                Err(crate::ProvisionError {
                    fault: crate::ProvisionFault::PublicationUncertain { .. },
                    ..
                })
            ));
            let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head");
            std::fs::write(
                scratch.store().join("envelope.replacing"),
                b"first interrupted bytes",
            )
            .expect("first debris");
            let error = barrier_fault::with_failure(&scratch.store(), point, || {
                recover(&scratch.store(), prepare(image.clone()))
            })
            .expect_err("barrier failure cannot return recovery success");
            if point == Point::RecoveryActive {
                assert!(
                    matches!(error.fault, RecoveryFault::Completion { instance: found, .. } if found == instance)
                );
                assert_eq!(error.code(), Code::StoreActivationUncertain.as_str());
            } else if point == Point::RecoveryParent {
                assert!(matches!(error.fault, RecoveryFault::Io(_)));
                assert_eq!(error.code(), Code::StoreIo.as_str());
            } else {
                assert!(matches!(error.fault, RecoveryFault::Metadata(_)));
                assert_eq!(error.code(), Code::StoreIo.as_str());
            }
            assert_eq!(error.preserved.len(), 1);
            let first = scratch.store().join(&error.preserved[0]);
            assert_eq!(
                std::fs::read(&first).expect("known first move"),
                b"first interrupted bytes"
            );
            let record = EnvelopeRecord::decode(
                &std::fs::read(scratch.store().join(crate::ENVELOPE_FILE))
                    .expect("actual envelope"),
            )
            .expect("record");
            let digest = LogicalHead::decode_with_digest(&head).expect("head").1;
            assert_eq!(
                record.state,
                if point == Point::RecoveryActive {
                    EnvelopeState::Active
                } else {
                    EnvelopeState::Provision { head: digest }
                }
            );
            if point != Point::RecoveryActive {
                assert!(matches!(
                    crate::attach(&scratch.store(), prepare(image.clone())),
                    Err(crate::LifecycleError::Open(
                        OpenError::ActivationRequired { .. }
                    ))
                ));
            }
            std::fs::write(
                scratch.store().join("envelope.replacing"),
                b"second interrupted bytes",
            )
            .expect("new debris before fresh attempt");
            let receipt =
                recover(&scratch.store(), prepare(image.clone())).expect("fresh recovery");
            assert_eq!(receipt.instance, instance);
            assert_eq!(receipt.image_id, image.image_id());
            assert_eq!(receipt.preserved.len(), 1);
            assert_ne!(receipt.preserved, error.preserved);
            assert_eq!(
                std::fs::read(&first).expect("first preserved bytes retained"),
                b"first interrupted bytes"
            );
            assert_eq!(
                std::fs::read(scratch.store().join(&receipt.preserved[0]))
                    .expect("second preserved bytes"),
                b"second interrupted bytes"
            );
            assert_eq!(
                std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head never replayed"),
                head
            );
        }
    }

    #[test]
    fn recovery_refuses_a_held_owner_before_engine_access_or_preservation() {
        let scratch = Scratch::new();
        let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        provision(
            &scratch.store(),
            request(&image, StoreInstanceId::draw().expect("instance")),
        )
        .expect("provision");
        let held = LockedStore::acquire(&scratch.store()).expect("hold recovery admission owner");
        let envelope = std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope");
        std::fs::write(
            scratch.store().join(crate::ENGINE_FILE),
            b"invalid engine control",
        )
        .expect("engine control");
        std::fs::write(scratch.store().join("envelope.replacing"), b"unexplained")
            .expect("debris control");
        let error =
            recover(&scratch.store(), prepare(image)).expect_err("live owner refuses recovery");
        assert!(matches!(
            error.fault,
            RecoveryFault::Validation(AuditError::Open(OpenError::Lock(
                marrow_kernel::durable::NativeLockError::StoreInUse { .. }
            )))
        ));
        assert_eq!(error.code(), Code::StoreLocked.as_str());
        assert!(error.preserved.is_empty());
        assert_eq!(
            std::fs::read(scratch.store().join(crate::ENGINE_FILE)).expect("engine unchanged"),
            b"invalid engine control"
        );
        assert_eq!(
            std::fs::read(scratch.store().join("envelope.replacing")).expect("debris unmoved"),
            b"unexplained"
        );
        assert_eq!(
            std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("no activation"),
            envelope
        );
        drop(held);
    }

    #[test]
    fn final_admission_rereads_both_metadata_files_after_activation() {
        let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        let edited =
            marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 1"))).expect("verify");
        for artifact in [Artifact::Envelope, Artifact::Head] {
            let scratch = Scratch::new();
            let instance = StoreInstanceId::draw().expect("instance");
            let req = request(&image, instance);
            let mut changed_metadata = req.envelope.clone();
            changed_metadata.writer_toolchain = "changed before final read".into();
            let replacement = match artifact {
                Artifact::Envelope => EnvelopeRecord {
                    metadata: changed_metadata,
                    state: EnvelopeState::Active,
                }
                .encode()
                .expect("valid changed envelope"),
                Artifact::Head => request(&edited, instance).head.encode(),
            };
            provision(&scratch.store(), req).expect("provision");
            std::fs::write(
                scratch.store().join("envelope.replacing"),
                b"interrupted bytes",
            )
            .expect("debris");
            let error = with_final_read_replacement(
                &scratch.store(),
                artifact,
                replacement.clone(),
                || recover(&scratch.store(), prepare(image.clone())),
            )
            .expect_err("cached admission cannot certify changed metadata");
            assert_eq!(error.code(), Code::StoreActivationUncertain.as_str());
            assert!(matches!(error.fault,
                RecoveryFault::Completion {
                    instance: found,
                    source: AuditError::Open(OpenError::Corruption { .. }),
                } if found == instance));
            assert_eq!(error.preserved.len(), 1);
            assert_eq!(
                std::fs::read(scratch.store().join(&error.preserved[0])).expect("known move"),
                b"interrupted bytes"
            );
            let name = match artifact {
                Artifact::Envelope => crate::ENVELOPE_FILE,
                Artifact::Head => crate::HEAD_FILE,
            };
            assert_eq!(
                std::fs::read(scratch.store().join(name)).expect("actual changed metadata"),
                replacement
            );
        }
    }

    #[test]
    fn binding_preparation_refuses_a_replaced_engine_before_metadata_writes() {
        use crate::actor::binding_fault::{self, Mutation, Point};
        use marrow_kernel::durable::StoreError;
        let scratch = Scratch::new();
        let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        let edited =
            marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 1"))).expect("verify");
        let instance = StoreInstanceId::draw().expect("instance");
        provision(&scratch.store(), request(&image, instance)).expect("provision");
        let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head");
        let envelope = std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope");
        let result = binding_fault::with_mutation(
            &scratch.store(),
            Point::Admitted,
            Mutation::Engine(scratch.0.join("admitted-engine")),
            || crate::attach(&scratch.store(), prepare(edited)),
        );
        assert!(matches!(
            result,
            Err(crate::LifecycleError::Open(OpenError::Store(
                StoreError::Io {
                    op: "service preparation",
                    ..
                }
            )))
        ));
        assert_eq!(
            std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head"),
            head
        );
        assert_eq!(
            std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope"),
            envelope
        );
        assert!(!scratch.store().join(crate::LOCK_FILE).exists());
        assert_eq!(
            std::fs::read_dir(scratch.store()).expect("members").count(),
            3
        );
    }

    #[test]
    fn binding_rechecks_old_metadata_and_location_after_service_preparation() {
        use crate::actor::binding_fault::{self, Mutation, Point};
        let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        let edited =
            marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 1"))).expect("verify");
        for move_directory in [false, true] {
            let scratch = Scratch::new();
            let instance = StoreInstanceId::draw().expect("instance");
            let req = request(&image, instance);
            let old_head = req.head.encode();
            provision(&scratch.store(), req).expect("provision");
            let old_envelope =
                std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope");
            let changed_head = request(&edited, instance).head.encode();
            let (mutation, retained, expected_head) = if move_directory {
                let moved = scratch.0.join("moved");
                (Mutation::Directory(moved.clone()), moved, old_head)
            } else {
                (
                    Mutation::Metadata(Artifact::Head, changed_head.clone()),
                    scratch.store(),
                    changed_head,
                )
            };
            let result =
                binding_fault::with_mutation(&scratch.store(), Point::Prepared, mutation, || {
                    crate::attach(&scratch.store(), prepare(edited.clone()))
                });
            assert!(matches!(
                result,
                Err(crate::LifecycleError::Audit(AuditError::Open(_)))
            ));
            assert_eq!(
                std::fs::read(retained.join(crate::HEAD_FILE)).expect("retained head"),
                expected_head
            );
            assert_eq!(
                std::fs::read(retained.join(crate::ENVELOPE_FILE)).expect("retained envelope"),
                old_envelope
            );
            assert!(!retained.join("head.replacing").exists());
            assert!(!retained.join("envelope.replacing").exists());
        }
    }

    #[test]
    fn binding_final_verification_refuses_changed_metadata_with_instance() {
        use crate::actor::binding_fault::{self, Mutation, Point};
        let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        let edited =
            marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 1"))).expect("verify");
        for artifact in [Artifact::Head, Artifact::Envelope] {
            let scratch = Scratch::new();
            let instance = StoreInstanceId::draw().expect("instance");
            let req = request(&image, instance);
            let replacement = match artifact {
                Artifact::Head => req.head.encode(),
                Artifact::Envelope => {
                    let mut metadata = req.envelope.clone();
                    metadata.writer_toolchain = "changed during activation".into();
                    EnvelopeRecord {
                        metadata,
                        state: EnvelopeState::Active,
                    }
                    .encode()
                    .expect("envelope")
                }
            };
            provision(&scratch.store(), req).expect("provision");
            let result = binding_fault::with_mutation(
                &scratch.store(),
                Point::Activated,
                Mutation::Metadata(artifact, replacement.clone()),
                || crate::attach(&scratch.store(), prepare(edited.clone())),
            );
            assert!(
                matches!(result, Err(crate::LifecycleError::ActivationUncertain {
                instance: found,
                source: AuditError::Open(OpenError::Corruption { .. }),
            }) if found == instance)
            );
            let name = match artifact {
                Artifact::Head => crate::HEAD_FILE,
                Artifact::Envelope => crate::ENVELOPE_FILE,
            };
            assert_eq!(
                std::fs::read(scratch.store().join(name)).expect("retained changed metadata"),
                replacement
            );
        }
    }

    #[test]
    fn recovery_refuses_a_different_image_before_opening_the_engine() {
        let scratch = Scratch::new();
        let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        let edited =
            marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 1"))).expect("verify");
        let instance = StoreInstanceId::draw().expect("instance");
        provision(&scratch.store(), request(&image, instance)).expect("provision");
        let engine = scratch.store().join(crate::ENGINE_FILE);
        std::fs::write(&engine, b"not an engine").expect("engine refusal control");
        let envelope = std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope");
        assert!(matches!(
            recover(&scratch.store(), prepare(edited)).map_err(|error| error.fault),
            Err(RecoveryFault::Validation(AuditError::ImageNotActive))
        ));
        assert_eq!(
            std::fs::read(&engine).expect("engine unchanged"),
            b"not an engine"
        );
        assert_eq!(
            std::fs::read(scratch.store().join(crate::ENVELOPE_FILE)).expect("envelope unchanged"),
            envelope
        );
    }

    #[test]
    fn unsupported_stamp_refuses_admission_before_engine_or_preservation() {
        let image = marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify");
        for pending in [false, true] {
            let scratch = Scratch::new();
            let req = request(&image, StoreInstanceId::draw().expect("instance"));
            let digest = req.head.encode_with_digest().1;
            let mut metadata = req.envelope.clone();
            provision(&scratch.store(), req).expect("current provision");
            metadata.engine_format_version = u32::MAX;
            let record = EnvelopeRecord {
                metadata,
                state: if pending {
                    EnvelopeState::Provision { head: digest }
                } else {
                    EnvelopeState::Active
                },
            };
            let bytes = record.encode().expect("resealed unsupported stamp");
            std::fs::write(scratch.store().join(crate::ENVELOPE_FILE), &bytes).expect("envelope");
            std::fs::write(scratch.store().join(crate::ENGINE_FILE), b"invalid engine")
                .expect("engine control");
            std::fs::write(scratch.store().join("envelope.replacing"), b"unexplained")
                .expect("debris control");
            let error =
                recover(&scratch.store(), prepare(image.clone())).expect_err("unsupported stamp");
            assert!(error.preserved.is_empty());
            assert!(matches!(
                error.fault,
                RecoveryFault::Validation(AuditError::Open(OpenError::Store(
                    marrow_kernel::durable::StoreError::FormatVersion {
                        found: u32::MAX,
                        ..
                    }
                )))
            ));
            assert!(matches!(
                crate::attach(&scratch.store(), prepare(image.clone())),
                Err(crate::LifecycleError::Open(OpenError::Store(
                    marrow_kernel::durable::StoreError::FormatVersion {
                        found: u32::MAX,
                        ..
                    }
                )))
            ));
            assert_eq!(
                std::fs::read(scratch.store().join(crate::ENVELOPE_FILE))
                    .expect("envelope unchanged"),
                bytes
            );
            assert_eq!(
                std::fs::read(scratch.store().join(crate::ENGINE_FILE)).expect("engine unchanged"),
                b"invalid engine"
            );
            assert_eq!(
                std::fs::read(scratch.store().join("envelope.replacing")).expect("debris unmoved"),
                b"unexplained"
            );
        }
    }

    #[test]
    fn rebind_recovery_adopts_only_the_exact_recorded_old_or_new_head() {
        let images = [
            marrow_verify::verify(&compile_bytes(SOURCE)).expect("verify"),
            marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 1"))).expect("verify"),
            marrow_verify::verify(&compile_bytes(&SOURCE.replace("?? 0", "?? 2"))).expect("verify"),
        ];
        let instance = StoreInstanceId::draw().expect("instance");
        let old = request(&images[0], instance).head.encode_with_digest().1;
        let new = request(&images[1], instance).head.encode_with_digest().1;
        for (index, image) in images.into_iter().enumerate() {
            let scratch = Scratch::new();
            let req = request(&image, instance);
            let record = EnvelopeRecord {
                metadata: req.envelope.clone(),
                state: EnvelopeState::Rebind { old, new },
            };
            provision(&scratch.store(), req).expect("provision");
            let envelope = record.encode().expect("pending");
            std::fs::write(scratch.store().join(crate::ENVELOPE_FILE), &envelope).expect("pending");
            let head = std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head");
            let result =
                recover(&scratch.store(), prepare(image.clone())).map_err(|error| error.fault);
            if index < 2 {
                assert_eq!(
                    result.expect("recorded head is eligible").image_id,
                    image.image_id()
                );
            } else {
                assert!(matches!(result, Err(RecoveryFault::HeadMismatch)));
                assert_eq!(
                    std::fs::read(scratch.store().join(crate::ENVELOPE_FILE))
                        .expect("pending unchanged"),
                    envelope
                );
            }
            assert_eq!(
                std::fs::read(scratch.store().join(crate::HEAD_FILE)).expect("head unchanged"),
                head
            );
        }
    }
}
