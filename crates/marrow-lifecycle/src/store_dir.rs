//! The store directory: its on-disk layout, and its custody while an owner holds it.
//!
//! A provisioned store is a private owner-only directory with these artifacts:
//!
//! ```text
//! <dir>/store.redb   the ordered-byte engine database
//! <dir>/envelope     the StoreEnvelope bytes (store instance + writer/engine provenance)
//! <dir>/head         the LogicalHead bytes (active binding + reserved slots + head map)
//! <dir>/lock         optional ownership marker (records the last mutable holder)
//! ```
//!
//! A store is COMPLETE only when the directory holds all three of `store.redb`, `envelope`,
//! and `head` as regular files. The lock is not one of them and says nothing about
//! completeness: provision and inspection do not write it, mutable opening creates it, and it
//! persists — empty after a clean close, carrying the crashed holder's descriptor after an
//! unclean one. Completeness is asked twice with different questions:
//! [`artifacts_present`] before an owner is held, [`AdmittedStoreDir::is_complete`] under it.
//!
//! Once the physical owner is held, [`AdmittedStoreDir`] is how the `envelope` and the
//! `head` are read. It retains the directory as a descriptor and reads each of those two
//! children from it: the child is opened without following a link, must be a regular file
//! reachable under exactly one name, and the exact ceiling its own recorded version selects
//! is applied before its bytes are allocated for. The descriptor-rooted operations
//! themselves belong to `marrow-fs-journal`, the workspace's sole owner of them.
//!
//! Two paths into the same directory are outside that protocol and are resolved by path
//! instead: `store.redb`, which `marrow-store` opens as part of holding the engine, and the
//! `lock` entry it owns. Neither's bytes are admitted from the retained descriptor, so what
//! this module establishes about *content* covers the two artifacts it reads and no more.
//! The engine's node kind is the one exception, decided by the completeness verdict because
//! its opener resolves it by path and would otherwise follow a link out of the directory the
//! owner holds.

use std::path::{Path, PathBuf};

use marrow_codes::Code;
use marrow_fs_journal::{
    AdmittedDir, CustodyError, CustodyOp, EntryName, EntryStat, NodeKind, OpenedFile,
};

use crate::codec::{ARTIFACT_PREFIX_BYTES, FormatError};
use crate::seam::{Event, Seam, Step};

/// The engine database file name within a store directory.
pub const ENGINE_FILE: &str = marrow_kernel::durable::NATIVE_ENGINE_FILE;
/// The persisted envelope file name within a store directory.
pub const ENVELOPE_FILE: &str = "envelope";
/// The logical-head file name within a store directory.
pub const HEAD_FILE: &str = "head";
/// The owner lock file name within a store directory.
pub const LOCK_FILE: &str = marrow_kernel::durable::NATIVE_LOCK_FILE;

/// One entry of a store directory an owner-held admission names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreEntry {
    /// The store directory itself.
    Directory,
    /// The ordered-byte engine database. Admission decides only that the directory holds it
    /// as a regular file; its bytes belong to the storage layer, which opens it by path.
    Engine,
    /// The persisted envelope.
    Envelope,
    /// The persisted logical head.
    Head,
}

impl StoreEntry {
    /// The entry's name in prose. An artifact is named by the entry name it occupies, so a
    /// refusal a user reads names the file they can go and look at.
    pub fn label(self) -> &'static str {
        match self {
            StoreEntry::Directory => "directory",
            StoreEntry::Engine => ENGINE_FILE,
            StoreEntry::Envelope => ENVELOPE_FILE,
            StoreEntry::Head => HEAD_FILE,
        }
    }
}

/// A store directory that could not be examined at all.
///
/// This process cannot reach or traverse the directory, so nothing about the store it may
/// hold was observed: not whether it exists, not whether it is complete, not whether it is
/// held. A failure to look is reported as itself — never folded into absence, and never into
/// a claim that what could not be seen is missing or corrupt.
#[derive(Debug)]
pub struct StoreAccessError {
    path: PathBuf,
    source: std::io::Error,
}

impl StoreAccessError {
    pub(crate) fn at(path: &Path, source: std::io::Error) -> Self {
        Self {
            path: path.to_path_buf(),
            source,
        }
    }

    /// The store directory that could not be examined.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The stable dotted code a tool reports.
    pub fn code(&self) -> Code {
        match self.source.kind() {
            std::io::ErrorKind::PermissionDenied => Code::StorePermissionDenied,
            _ => Code::StoreIo,
        }
    }
}

impl std::fmt::Display for StoreAccessError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the store directory at {} could not be examined: {}",
            self.path.display(),
            self.source,
        )
    }
}

impl std::error::Error for StoreAccessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// The artifacts an owner-held admission read admits. The store directory is not one of
/// them: it is the root a read is relative to, never a child a read resolves, so it cannot
/// reach a child-name lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Artifact {
    Envelope,
    Head,
}

/// A file a store-directory write creates: an artifact under its own name, or the
/// replacement it is staged in before the rename that installs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Body {
    Artifact(Artifact),
    Replacement(Artifact),
}

impl Body {
    fn name(self) -> EntryName {
        match self {
            Body::Artifact(artifact) => artifact.name(),
            Body::Replacement(artifact) => artifact.replacement_name(),
        }
    }
}

impl Artifact {
    fn replacement_name(self) -> EntryName {
        entry_name(match self {
            Self::Envelope => "envelope.replacing",
            Self::Head => "head.replacing",
        })
    }
    fn entry(self) -> StoreEntry {
        match self {
            Artifact::Envelope => StoreEntry::Envelope,
            Artifact::Head => StoreEntry::Head,
        }
    }

    /// The artifact's frozen directory-entry name. Each is one normal relative component,
    /// so admission of the name itself cannot fail for any store this crate reads.
    fn name(self) -> EntryName {
        entry_name(match self {
            Artifact::Envelope => ENVELOPE_FILE,
            Artifact::Head => HEAD_FILE,
        })
    }
}

/// One frozen store-directory entry name as a relative component. Every name this module
/// resolves is a fixed single component, so a name that cannot be admitted is a defect in
/// this module rather than a property of any store.
fn entry_name(name: &str) -> EntryName {
    EntryName::admit(name).expect("a store directory entry name is one normal component")
}

/// How a store artifact changed under the owner between the witness admission took of it
/// and the recheck admission made after reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instability {
    /// The opened node is no longer the node that was opened.
    Identity,
    /// The length disagrees with the length the read was bounded against.
    Length,
    /// The store directory no longer maps this name to the node that was read.
    ParentMapping,
}

impl Instability {
    fn describe(self) -> &'static str {
        match self {
            Instability::Identity => "changed identity while it was being read",
            Instability::Length => "changed length while it was being read",
            Instability::ParentMapping => "was replaced under its name while it was being read",
        }
    }
}

/// Why one store entry could not be admitted under the owner.
#[derive(Debug)]
pub enum AdmissionFault {
    /// The entry could not be admitted from the retained store directory: a link standing
    /// in for it, a node that is not a regular file, a platform this build's
    /// descriptor-rooted adapter does not qualify, or an I/O failure. An adapter that
    /// cannot perform the admission refuses here; it never falls back to a weaker open.
    Custody(CustodyError),
    /// The entry carries more than one link, so the bytes admission just checked remain
    /// reachable — and rewritable — under a name this owner does not hold.
    MultiplyLinked { links: u64 },
    /// The store directory maps the artifact's name to a node that is not a regular file,
    /// so the directory does not hold the artifact — whatever that node would resolve to if
    /// it were followed.
    NotAFile { found: NodeKind },
    /// The entry changed under the owner between its pre-read witness and its recheck.
    Unstable(Instability),
    /// The file is larger than the exact ceiling its own recorded version selects, so it is
    /// refused before its bytes are allocated for.
    OverCeiling { ceiling: u64 },
    /// The bytes are not a well-formed container.
    Format(FormatError),
}

/// Why one named store entry could not be admitted.
#[derive(Debug)]
pub struct AdmissionError {
    /// Which entry refused.
    pub entry: StoreEntry,
    /// What refused it.
    pub fault: AdmissionFault,
}

impl AdmissionError {
    pub(crate) fn format(entry: StoreEntry, error: FormatError) -> Self {
        Self {
            entry,
            fault: AdmissionFault::Format(error),
        }
    }

    /// A custody refusal of the store directory itself.
    pub(crate) fn directory(error: CustodyError) -> Self {
        Self::directory_fault(AdmissionFault::Custody(error))
    }

    fn directory_fault(fault: AdmissionFault) -> Self {
        Self {
            entry: StoreEntry::Directory,
            fault,
        }
    }

    /// The stable dotted code a tool reports.
    pub fn code(&self) -> Code {
        match &self.fault {
            AdmissionFault::Format(error) => error.code(),
            AdmissionFault::OverCeiling { .. } => Code::StoreLimit,
            AdmissionFault::MultiplyLinked { .. }
            | AdmissionFault::NotAFile { .. }
            | AdmissionFault::Unstable(_) => Code::StoreCorruption,
            AdmissionFault::Custody(error) => custody_code(error),
        }
    }
}

/// The code a custody refusal reports. A substitution — a link, a node of the wrong kind, a
/// name that no longer resolves, an identity that drifted — is the store directory not
/// holding the artifact admission requires, which is the verdict its multiply-linked and
/// unstable siblings already reach; owner bits that deny the open are a permission refusal;
/// and only an unclassified failure of the operation itself is I/O.
///
/// A platform this build's descriptor-rooted adapter does not qualify, and a filesystem that
/// cannot provide an operation's required semantics, are neither a property of the store nor
/// of this process's access to it: no store can be opened here at all. They report as I/O
/// because no dotted code distinguishes them today; the refusal names the platform in prose.
fn custody_code(error: &CustodyError) -> Code {
    match error {
        CustodyError::ModeDenied { .. } => Code::StorePermissionDenied,
        // The entry's own bits are one way an open is denied; the access this process has to
        // the path leading to it is the other, and both are the same refusal to a reader.
        CustodyError::Io { source, .. }
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            Code::StorePermissionDenied
        }
        CustodyError::SymlinkRefused { .. }
        | CustodyError::WrongNodeKind { .. }
        | CustodyError::NotADirectory { .. }
        | CustodyError::IdentityDrift { .. }
        | CustodyError::NotFound { .. }
        | CustodyError::AlreadyExists { .. } => Code::StoreCorruption,
        CustodyError::UnqualifiedPlatform { .. }
        | CustodyError::Unsupported { .. }
        | CustodyError::Io { .. } => Code::StoreIo,
    }
}

/// The typed refusal this build returns for every store-directory admission on a platform
/// its descriptor-rooted adapter does not qualify, or `Ok(())`.
///
/// An open asks before it creates anything. The admission that would refuse anyway runs
/// after the owner lock and the marker it writes, so refusing only there leaves an inherited
/// unclean obligation behind in a store this build could never have opened on this platform.
pub(crate) fn qualified_platform() -> Result<(), AdmissionError> {
    marrow_fs_journal::qualified_platform().map_err(AdmissionError::directory)
}

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let entry = self.entry.label();
        match &self.fault {
            AdmissionFault::Custody(CustodyError::UnqualifiedPlatform { os, arch }) => write!(
                formatter,
                "the store {entry} cannot be admitted on {os}/{arch}: this build admits a store \
                 directory on macOS, or on Linux for x86_64 and aarch64, only",
            ),
            AdmissionFault::Custody(error) => {
                write!(
                    formatter,
                    "the store {entry} could not be admitted: {error}"
                )
            }
            AdmissionFault::MultiplyLinked { links } => write!(
                formatter,
                "the store {entry} is reachable under {links} names; admission requires exactly one",
            ),
            AdmissionFault::NotAFile { found } => write!(
                formatter,
                "the store directory holds a {found} under the {entry} name, not a regular file",
            ),
            AdmissionFault::Unstable(instability) => {
                write!(formatter, "the store {entry} {}", instability.describe())
            }
            AdmissionFault::OverCeiling { ceiling } => write!(
                formatter,
                "the store {entry} is larger than the {ceiling}-byte ceiling the version it \
                 records allows",
            ),
            AdmissionFault::Format(error) => write!(formatter, "the store {entry} {error}"),
        }
    }
}

impl std::error::Error for AdmissionError {}

/// A retained store directory. Artifact reads use this descriptor rather than resolving
/// the path again. Existing-store admission compares its identity with the physical owner's
/// retained directory before reading artifacts; private construction admits only a directory.
/// Every write through it passes the [`Seam`] the sequence was opened with.
pub(crate) struct AdmittedStoreDir {
    dir: AdmittedDir,
    seam: Seam,
}

impl AdmittedStoreDir {
    pub(crate) fn identity(&self) -> marrow_fs_journal::FsIdentity {
        self.dir.identity()
    }

    fn reach(&self, step: Step) -> Result<(), AdmissionFault> {
        self.seam
            .at(Event::Step { dir: self, step })
            .map_err(AdmissionFault::Custody)
    }

    /// Expose `step` of the sequence running over this directory to the seam.
    pub(crate) fn at(&self, step: Step) -> Result<(), AdmissionError> {
        self.reach(step).map_err(AdmissionError::directory_fault)
    }

    /// `fsync` this directory: the barrier `step` names.
    pub(crate) fn sync(&self, step: Step) -> Result<(), AdmissionError> {
        self.reach(step)
            .and_then(|()| self.dir.sync().map_err(AdmissionFault::Custody))
            .map_err(AdmissionError::directory_fault)
    }

    /// Create and sync a fresh metadata file without replacing an existing entry.
    /// The caller syncs the directory after constructing the complete stage.
    pub(crate) fn write_new(&self, artifact: Artifact, bytes: &[u8]) -> Result<(), AdmissionError> {
        self.create_synced(Body::Artifact(artifact), bytes)
            .map(|_| ())
            .map_err(|fault| AdmissionError {
                entry: artifact.entry(),
                fault,
            })
    }

    fn create_synced(&self, body: Body, bytes: &[u8]) -> Result<OpenedFile, AdmissionFault> {
        let name = body.name();
        let mut file = self
            .dir
            .create_file_excl(&name)
            .map_err(AdmissionFault::Custody)?;
        self.reach(Step::Append(body))?;
        file.append(bytes).map_err(AdmissionFault::Custody)?;
        self.reach(Step::FileSync(body))?;
        file.sync().map_err(AdmissionFault::Custody)?;
        self.check_file_mapping(&name, &file)?;
        Ok(file)
    }

    fn check_file_mapping(
        &self,
        name: &EntryName,
        file: &OpenedFile,
    ) -> Result<(), AdmissionFault> {
        let stat = file.stat().map_err(AdmissionFault::Custody)?;
        require_single_link(&stat)?;
        match self.dir.stat_entry(name).map_err(AdmissionFault::Custody)? {
            Some(mapped) if mapped.identity() == file.identity() => Ok(()),
            _ => Err(AdmissionFault::Unstable(Instability::ParentMapping)),
        }
    }

    /// Sync the replacement body and replace the existing metadata entry under
    /// this descriptor. Occupied reserved siblings refuse without modification.
    /// The caller must sync this directory before advancing the lifecycle state.
    pub(crate) fn replace(&self, artifact: Artifact, bytes: &[u8]) -> Result<(), AdmissionError> {
        self.replace_inner(artifact, bytes)
            .map_err(|fault| AdmissionError {
                entry: artifact.entry(),
                fault,
            })
    }

    fn replace_inner(&self, artifact: Artifact, bytes: &[u8]) -> Result<(), AdmissionFault> {
        let name = artifact.name();
        let original = self
            .dir
            .open_file_readonly(&name)
            .map_err(AdmissionFault::Custody)?;
        self.check_file_mapping(&name, &original)?;
        let _replacement_file = self.create_synced(Body::Replacement(artifact), bytes)?;
        self.check_file_mapping(&name, &original)?;
        self.reach(Step::Install(artifact))?;
        self.dir
            .rename_replace(&artifact.replacement_name(), &name)
            .map_err(AdmissionFault::Custody)
    }

    /// Preserve one reserved regular file without reading or interpreting its
    /// body. Record the known move before the directory barrier, so its name
    /// survives in the caller's failure report if that barrier fails.
    pub(crate) fn preserve_replacement(
        &self,
        artifact: Artifact,
        draw: impl FnOnce() -> std::io::Result<[u8; 16]>,
        preserved: &mut Vec<String>,
    ) -> Result<(), AdmissionError> {
        let preserve = || -> Result<(), AdmissionFault> {
            let source = artifact.replacement_name();
            if self
                .dir
                .stat_entry(&source)
                .map_err(AdmissionFault::Custody)?
                .is_none()
            {
                return Ok(());
            }
            let file = self
                .dir
                .open_file_readonly(&source)
                .map_err(AdmissionFault::Custody)?;
            self.check_file_mapping(&source, &file)?;
            file.sync().map_err(AdmissionFault::Custody)?;
            let nonce = draw().map_err(|source| {
                AdmissionFault::Custody(CustodyError::Io {
                    op: CustodyOp::Read,
                    source,
                })
            })?;
            let name = format!(
                "{}.preserved.{:032x}",
                source.as_str(),
                u128::from_be_bytes(nonce)
            );
            let destination = entry_name(&name);
            self.check_file_mapping(&source, &file)?;
            self.dir
                .rename_noreplace(&source, &destination)
                .map_err(AdmissionFault::Custody)?;
            preserved.push(name);
            self.reach(Step::Preservation)?;
            self.dir.sync().map_err(AdmissionFault::Custody)
        };
        preserve().map_err(|fault| AdmissionError {
            entry: artifact.entry(),
            fault,
        })
    }

    /// Flush existing artifact bytes while the owner has no transaction or
    /// application writer in flight. This does not commit engine transactions.
    pub(crate) fn sync_artifacts(&self) -> Result<(), AdmissionError> {
        for (entry, name) in [
            (StoreEntry::Envelope, ENVELOPE_FILE),
            (StoreEntry::Head, HEAD_FILE),
            (StoreEntry::Engine, ENGINE_FILE),
        ] {
            let name = entry_name(name);
            let sync = || -> Result<(), AdmissionFault> {
                let file = self
                    .dir
                    .open_file_readonly(&name)
                    .map_err(AdmissionFault::Custody)?;
                self.check_file_mapping(&name, &file)?;
                file.sync().map_err(AdmissionFault::Custody)?;
                self.check_file_mapping(&name, &file)
            };
            sync().map_err(|fault| AdmissionError { entry, fault })?;
        }
        self.sync(Step::RecoveryArtifacts)
    }

    /// Whether the directory at `path` is still the retained node.
    pub(crate) fn verify_location(&self, path: &Path) -> Result<(), AdmissionError> {
        let current = AdmittedDir::admit_trusted_root(path).map_err(AdmissionError::directory)?;
        if current.identity() != self.dir.identity() {
            return Err(AdmissionError::directory(CustodyError::IdentityDrift {
                op: CustodyOp::Stat,
            }));
        }
        Ok(())
    }

    /// Admit the same directory node whose lock the pending owner retains. The owner must
    /// remain held through use of this descriptor. Later path-based engine opens still
    /// require the cooperating namespace to preserve the directory's name.
    pub(crate) fn admit_under_owner(
        owner: &marrow_kernel::durable::PendingNativeStoreOwner,
        seam: Seam,
    ) -> Result<Self, AdmissionError> {
        let admitted = Self::admit(owner.directory(), seam)?;
        let metadata = owner.directory_metadata().map_err(|source| {
            AdmissionError::directory(CustodyError::Io {
                op: CustodyOp::Stat,
                source,
            })
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let held = marrow_fs_journal::FsIdentity::new(metadata.dev(), metadata.ino());
            if admitted.dir.identity() != held {
                return Err(AdmissionError::directory(CustodyError::IdentityDrift {
                    op: CustodyOp::AdmitDirectory,
                }));
            }
            Ok(admitted)
        }
        #[cfg(not(unix))]
        {
            let _ = (admitted, metadata);
            Err(AdmissionError::directory(CustodyError::Unsupported {
                op: CustodyOp::Stat,
            }))
        }
    }

    /// Retain the canonical store directory the physical owner already holds.
    fn admit(canonical_dir: &Path, seam: Seam) -> Result<Self, AdmissionError> {
        AdmittedDir::admit_trusted_root(canonical_dir)
            .map(|dir| Self { dir, seam })
            .map_err(AdmissionError::directory)
    }

    /// Whether the retained directory holds all three durable artifacts. The completeness
    /// verdict is read through the same descriptor the artifact reads use, so it belongs to
    /// the one admission snapshot taken under the owner rather than to a separate resolution
    /// of three paths.
    ///
    /// A name that maps to nothing leaves the store incomplete. A name that maps to a node
    /// which is not a regular file is neither complete nor incomplete: the directory holds
    /// *something* under that name, and it is not the artifact. That is refused as itself,
    /// naming the entry. Deciding it here is what keeps this verdict and the opener of each
    /// artifact agreed on what the directory holds — the engine is opened by path, which
    /// follows a link, so counting a link as the engine present would admit a store whose
    /// engine bytes live outside the directory the owner holds.
    pub(crate) fn is_complete(&self) -> Result<bool, AdmissionError> {
        for (entry, name) in [
            (StoreEntry::Engine, ENGINE_FILE),
            (StoreEntry::Envelope, ENVELOPE_FILE),
            (StoreEntry::Head, HEAD_FILE),
        ] {
            let mapped = self
                .dir
                .stat_entry(&entry_name(name))
                .map_err(AdmissionError::directory)?;
            let Some(mapped) = mapped else {
                return Ok(false);
            };
            if mapped.kind() != NodeKind::Regular {
                return Err(AdmissionError {
                    entry,
                    fault: AdmissionFault::NotAFile {
                        found: mapped.kind(),
                    },
                });
            }
        }
        Ok(true)
    }

    /// Read one artifact's whole bytes under the owner.
    ///
    /// The child is opened from the retained directory without following a link and must be
    /// a regular file linked exactly once. Its fixed prefix is read first, because the
    /// version that prefix records is what selects the artifact's ceiling; the length
    /// witnessed at open is then checked against that ceiling before any body-sized
    /// allocation, and the body is read through the same handle at most one byte past it, so
    /// a file that grew after the check is caught rather than silently read as a decodable
    /// prefix. Identity, length, and the directory's mapping of the name are rechecked
    /// before the bytes are handed to a decoder.
    pub(crate) fn read(
        &self,
        artifact: Artifact,
        ceiling: impl FnOnce(&[u8; ARTIFACT_PREFIX_BYTES]) -> Result<u64, FormatError>,
    ) -> Result<Vec<u8>, AdmissionError> {
        self.read_bounded(artifact, ceiling)
            .map_err(|fault| AdmissionError {
                entry: artifact.entry(),
                fault,
            })
    }

    fn read_bounded(
        &self,
        artifact: Artifact,
        ceiling: impl FnOnce(&[u8; ARTIFACT_PREFIX_BYTES]) -> Result<u64, FormatError>,
    ) -> Result<Vec<u8>, AdmissionFault> {
        let name = artifact.name();
        let file = self.dir.open_file(&name).map_err(AdmissionFault::Custody)?;
        let opened = file.stat().map_err(AdmissionFault::Custody)?;
        require_single_link(&opened)?;

        let ceiling = ceiling(&read_prefix(&file)?).map_err(AdmissionFault::Format)?;
        let over_ceiling = || AdmissionFault::OverCeiling { ceiling };
        if opened.size() > ceiling {
            return Err(over_ceiling());
        }

        let bound = usize::try_from(ceiling.saturating_add(1)).map_err(|_| over_ceiling())?;
        let bytes = file.read_prefix(bound).map_err(AdmissionFault::Custody)?;
        if bytes.len() as u64 > ceiling {
            return Err(over_ceiling());
        }

        let reread = file.stat().map_err(AdmissionFault::Custody)?;
        if reread.identity() != opened.identity() {
            return Err(AdmissionFault::Unstable(Instability::Identity));
        }
        require_single_link(&reread)?;
        if reread.size() != opened.size() || bytes.len() as u64 != opened.size() {
            return Err(AdmissionFault::Unstable(Instability::Length));
        }
        match self
            .dir
            .stat_entry(&name)
            .map_err(AdmissionFault::Custody)?
        {
            Some(mapped) if mapped.identity() == file.identity() => Ok(bytes),
            _ => Err(AdmissionFault::Unstable(Instability::ParentMapping)),
        }
    }
}

fn read_prefix(file: &OpenedFile) -> Result<[u8; ARTIFACT_PREFIX_BYTES], AdmissionFault> {
    let prefix = file
        .read_prefix(ARTIFACT_PREFIX_BYTES)
        .map_err(AdmissionFault::Custody)?;
    <[u8; ARTIFACT_PREFIX_BYTES]>::try_from(prefix.as_slice())
        .map_err(|_| AdmissionFault::Format(FormatError::Truncated))
}

fn require_single_link(stat: &EntryStat) -> Result<(), AdmissionFault> {
    if stat.nlink() == 1 {
        Ok(())
    } else {
        Err(AdmissionFault::MultiplyLinked {
            links: stat.nlink(),
        })
    }
}

/// Whether `dir` maps `name` to an entry at all, following no link and creating nothing.
///
/// Only "no such entry" is an observation of the directory's contents. Every other failure
/// is a failure to look at the directory, and is returned as itself: a directory this
/// process cannot traverse is not a directory whose entries are missing.
fn entry_present(dir: &Path, name: &str) -> Result<bool, StoreAccessError> {
    match std::fs::symlink_metadata(dir.join(name)) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(StoreAccessError::at(dir, error)),
    }
}

/// Whether `dir` names an owner lock entry at all. This decides only whether the directory
/// has ever been opened as a store, never what the entry holds.
pub(crate) fn lock_entry_present(dir: &Path) -> Result<bool, StoreAccessError> {
    entry_present(dir, LOCK_FILE)
}

/// Whether `dir` maps all three durable artifact names (engine, envelope, head) to entries.
/// A store missing any one is incomplete — never published as complete — so a crash mid-build
/// (which leaves a temp directory, never a partial destination) can never be mistaken for a
/// finished store. The lock is deliberately excluded: provision does not write it, so a
/// complete store may carry no lock and its presence is no evidence of completeness.
///
/// This is the coarse question, asked before an owner is held and answered by whether the
/// name maps to anything. Whether what it maps to is the artifact — a regular file, not a
/// link out of the directory — is [`AdmittedStoreDir::is_complete`]'s to decide under the
/// owner, so this never reports a directory as holding an artifact the open would refuse.
pub(crate) fn artifacts_present(dir: &Path) -> Result<bool, StoreAccessError> {
    for name in [ENGINE_FILE, ENVELOPE_FILE, HEAD_FILE] {
        if !entry_present(dir, name)? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
#[path = "store_dir_tests.rs"]
mod tests;
