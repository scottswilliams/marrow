//! The store directory: its on-disk layout, and its custody while an owner holds it.
//!
//! A provisioned store is a private owner-only directory holding four files:
//!
//! ```text
//! <dir>/store.redb   the ordered-byte engine database
//! <dir>/envelope     the StoreEnvelope bytes (store instance + writer/engine provenance)
//! <dir>/head         the LogicalHead bytes (active binding + reserved slots + head map)
//! <dir>/lock         the owner lock (advisory; its body names the live owner)
//! ```
//!
//! A store is COMPLETE only when the directory holds all three of `store.redb`, `envelope`,
//! and `head` as regular files. The lock is not one of them and says nothing about
//! completeness: provision does not write it, the first open creates it, and from then on it
//! persists — empty after a clean close, carrying the crashed holder's descriptor after an
//! unclean one.
//!
//! Completeness is asked twice, and the two questions are different. Before an owner is
//! held, [`artifacts_present`] asks only whether the directory maps the three names to
//! entries at all — enough to keep an open from creating a lock entry in a directory that is
//! not a store, and no more. Under the owner, [`AdmittedStoreDir::is_complete`] decides what
//! those names map to.
//!
//! Neither question resolves a failure to look into an observation. Only "no such entry" is
//! an observation of the directory's contents; a directory this process cannot traverse
//! yields [`StoreAccessError`], never a missing artifact.
//!
//! Once the physical owner is held, [`AdmittedStoreDir`] is how the `envelope` and the
//! `head` are read. It retains the directory as a descriptor and reads each of those two
//! children from it: the child is opened without following a link, must be a regular file
//! reachable under exactly one name, and the exact ceiling its own recorded version selects
//! is applied before its bytes are allocated for. The descriptor-rooted operations
//! themselves belong to `marrow-fs-journal`, the workspace's sole owner of them; what lives
//! here is the admission protocol laid over them.
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
use marrow_fs_journal::{AdmittedDir, CustodyError, EntryName, EntryStat, NodeKind, OpenedFile};

use crate::codec::{ARTIFACT_PREFIX_BYTES, FormatError};

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
    pub fn code(&self) -> &'static str {
        match self.source.kind() {
            std::io::ErrorKind::PermissionDenied => Code::StorePermissionDenied.as_str(),
            _ => Code::StoreIo.as_str(),
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

impl Artifact {
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

    /// The stable dotted code a tool reports.
    pub fn code(&self) -> &'static str {
        match &self.fault {
            AdmissionFault::Format(error) => error.code(),
            AdmissionFault::OverCeiling { .. } => Code::StoreLimit.as_str(),
            AdmissionFault::MultiplyLinked { .. }
            | AdmissionFault::NotAFile { .. }
            | AdmissionFault::Unstable(_) => Code::StoreCorruption.as_str(),
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
fn custody_code(error: &CustodyError) -> &'static str {
    match error {
        CustodyError::ModeDenied { .. } => Code::StorePermissionDenied.as_str(),
        // The entry's own bits are one way an open is denied; the access this process has to
        // the path leading to it is the other, and both are the same refusal to a reader.
        CustodyError::Io { source, .. }
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            Code::StorePermissionDenied.as_str()
        }
        CustodyError::SymlinkRefused { .. }
        | CustodyError::WrongNodeKind { .. }
        | CustodyError::NotADirectory { .. }
        | CustodyError::IdentityDrift { .. }
        | CustodyError::NotFound { .. }
        | CustodyError::AlreadyExists { .. } => Code::StoreCorruption.as_str(),
        CustodyError::UnqualifiedPlatform { .. }
        | CustodyError::Unsupported { .. }
        | CustodyError::Io { .. } => Code::StoreIo.as_str(),
    }
}

/// The typed refusal this build returns for every store-directory admission on a platform
/// its descriptor-rooted adapter does not qualify, or `Ok(())`.
///
/// An open asks before it creates anything. The admission that would refuse anyway runs
/// after the owner lock and the marker it writes, so refusing only there leaves an inherited
/// unclean obligation behind in a store this build could never have opened on this platform.
pub(crate) fn qualified_platform() -> Result<(), AdmissionError> {
    marrow_fs_journal::qualified_platform().map_err(|error| AdmissionError {
        entry: StoreEntry::Directory,
        fault: AdmissionFault::Custody(error),
    })
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

/// The store directory retained as a descriptor while its physical owner is held. Each
/// artifact read is relative to that descriptor, so the whole admission snapshot reaches one
/// directory rather than re-resolving the path per read. The descriptor is obtained by
/// resolving the canonical path the physical owner reported, and no identity witness crosses
/// that boundary, so the directory it reaches is established to be one directory — not
/// established to be the same node the owner's lock is held over.
pub(crate) struct AdmittedStoreDir {
    dir: AdmittedDir,
}

impl AdmittedStoreDir {
    /// Retain the canonical store directory the physical owner already holds.
    pub(crate) fn admit(canonical_dir: &Path) -> Result<Self, AdmissionError> {
        AdmittedDir::admit_trusted_root(canonical_dir)
            .map(|dir| Self { dir })
            .map_err(|error| AdmissionError {
                entry: StoreEntry::Directory,
                fault: AdmissionFault::Custody(error),
            })
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
            let mapped =
                self.dir
                    .stat_entry(&entry_name(name))
                    .map_err(|error| AdmissionError {
                        entry: StoreEntry::Directory,
                        fault: AdmissionFault::Custody(error),
                    })?;
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

/// The envelope file path within `dir`.
pub fn envelope_path(dir: &Path) -> std::path::PathBuf {
    dir.join(ENVELOPE_FILE)
}

/// The logical-head file path within `dir`.
pub fn head_path(dir: &Path) -> std::path::PathBuf {
    dir.join(HEAD_FILE)
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
mod tests {
    use super::*;

    #[test]
    fn store_directory_file_names_are_frozen() {
        // The store-directory layout is a durability contract; these names are frozen.
        assert_eq!(ENGINE_FILE, "store.redb");
        assert_eq!(ENVELOPE_FILE, "envelope");
        assert_eq!(HEAD_FILE, "head");
        assert_eq!(LOCK_FILE, "lock");

        // The two artifacts provision writes are the only ones this module derives a path
        // for; the engine and the lock are reached under their own owners' paths.
        let dir = Path::new("/stores/app");
        assert_eq!(envelope_path(dir), Path::new("/stores/app/envelope"));
        assert_eq!(head_path(dir), Path::new("/stores/app/head"));
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
                CustodyError::SymlinkRefused { op: "open file" },
                Code::StoreCorruption.as_str(),
            ),
            (
                CustodyError::WrongNodeKind {
                    op: "open file",
                    found: marrow_fs_journal::NodeKind::Directory,
                },
                Code::StoreCorruption.as_str(),
            ),
            (
                CustodyError::NotFound { op: "open file" },
                Code::StoreCorruption.as_str(),
            ),
            (
                CustodyError::ModeDenied {
                    op: "open file",
                    found: 0o400,
                    required: 0o600,
                },
                Code::StorePermissionDenied.as_str(),
            ),
        ] {
            assert_eq!(refusal(AdmissionFault::Custody(fault)).code(), code);
        }

        // The sibling that reaches the same verdict without going through custody: a second
        // link to the artifact is the same "the directory does not hold this artifact"
        // observation a substituted node makes.
        assert_eq!(
            refusal(AdmissionFault::MultiplyLinked { links: 2 }).code(),
            Code::StoreCorruption.as_str(),
        );

        let unqualified = refusal(AdmissionFault::Custody(CustodyError::UnqualifiedPlatform {
            os: "freebsd",
            arch: "riscv64",
        }));
        assert_eq!(unqualified.code(), Code::StoreIo.as_str());
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
}
