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
//! A store is COMPLETE only when the directory exists and all three of `store.redb`,
//! `envelope`, and `head` are present; the lock is transient (held while a process owns the
//! store, left behind after an unclean shutdown).
//!
//! Once the physical owner is held, [`AdmittedStoreDir`] is how the directory's own
//! artifacts are read. It retains the directory as a descriptor and reads each child from
//! it: no path is resolved twice, no link is followed, a child reachable under a second
//! name is refused, and the exact ceiling the child's own recorded version selects is
//! applied before its bytes are allocated for. The descriptor-rooted operations themselves
//! belong to `marrow-fs-journal`, the workspace's sole owner of them; what lives here is
//! the admission protocol laid over them.

use std::path::Path;

use marrow_codes::Code;
use marrow_fs_journal::{AdmittedDir, CustodyError, EntryName, EntryStat, OpenedFile};

use crate::codec::{ARTIFACT_PREFIX_BYTES, FormatError};

/// The engine database file name within a store directory.
pub const ENGINE_FILE: &str = marrow_kernel::durable::NATIVE_ENGINE_FILE;
/// The persisted envelope file name within a store directory.
pub const ENVELOPE_FILE: &str = "envelope";
/// The logical-head file name within a store directory.
pub const HEAD_FILE: &str = "head";
/// The owner lock file name within a store directory.
pub const LOCK_FILE: &str = marrow_kernel::durable::NATIVE_LOCK_FILE;

/// One entry of a store directory an owner-held admission read names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreEntry {
    /// The store directory itself.
    Directory,
    /// The persisted envelope.
    Envelope,
    /// The persisted logical head.
    Head,
}

impl StoreEntry {
    /// The entry's name in prose.
    pub fn label(self) -> &'static str {
        match self {
            StoreEntry::Directory => "directory",
            StoreEntry::Envelope => "envelope",
            StoreEntry::Head => "head",
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            StoreEntry::Directory => ".",
            StoreEntry::Envelope => ENVELOPE_FILE,
            StoreEntry::Head => HEAD_FILE,
        }
    }
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
    /// The entry changed under the owner between its pre-read witness and its recheck.
    Unstable(Instability),
    /// The bytes are not a well-formed container, including a file beyond the exact ceiling
    /// its own recorded version selects.
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
            AdmissionFault::MultiplyLinked { .. } | AdmissionFault::Unstable(_) => {
                Code::StoreCorruption.as_str()
            }
            AdmissionFault::Custody(CustodyError::ModeDenied { .. }) => {
                Code::StorePermissionDenied.as_str()
            }
            AdmissionFault::Custody(_) => Code::StoreIo.as_str(),
        }
    }
}

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let entry = self.entry.label();
        match &self.fault {
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
            AdmissionFault::Unstable(instability) => {
                write!(formatter, "the store {entry} {}", instability.describe())
            }
            AdmissionFault::Format(error) => write!(formatter, "the store {entry} {error}"),
        }
    }
}

impl std::error::Error for AdmissionError {}

/// The store directory retained as a descriptor while its physical owner is held. Every
/// artifact read is relative to it, so the directory a read reaches is the one the owner
/// locked rather than whatever the original path spells at that instant.
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
        entry: StoreEntry,
        ceiling: impl FnOnce(&[u8; ARTIFACT_PREFIX_BYTES]) -> Result<u64, FormatError>,
    ) -> Result<Vec<u8>, AdmissionError> {
        self.read_bounded(entry, ceiling)
            .map_err(|fault| AdmissionError { entry, fault })
    }

    fn read_bounded(
        &self,
        entry: StoreEntry,
        ceiling: impl FnOnce(&[u8; ARTIFACT_PREFIX_BYTES]) -> Result<u64, FormatError>,
    ) -> Result<Vec<u8>, AdmissionFault> {
        let name = admitted_name(entry);
        let file = self.dir.open_file(&name).map_err(AdmissionFault::Custody)?;
        let opened = file.stat().map_err(AdmissionFault::Custody)?;
        require_single_link(&opened)?;

        let ceiling = ceiling(&read_prefix(&file)?).map_err(AdmissionFault::Format)?;
        if opened.size() > ceiling {
            return Err(over_ceiling(entry));
        }

        let bound = usize::try_from(ceiling.saturating_add(1)).map_err(|_| over_ceiling(entry))?;
        let bytes = file.read_prefix(bound).map_err(AdmissionFault::Custody)?;
        if bytes.len() as u64 > ceiling {
            return Err(over_ceiling(entry));
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

/// The artifact names are fixed single components, so admission of the name itself cannot
/// fail for any store this crate reads.
fn admitted_name(entry: StoreEntry) -> EntryName {
    EntryName::admit(entry.file_name()).expect("a store artifact name is one normal component")
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

fn over_ceiling(entry: StoreEntry) -> AdmissionFault {
    AdmissionFault::Format(FormatError::LengthOverflow {
        field: entry.label(),
    })
}

/// The engine database path within `dir`.
pub fn engine_path(dir: &Path) -> std::path::PathBuf {
    dir.join(ENGINE_FILE)
}

/// The envelope file path within `dir`.
pub fn envelope_path(dir: &Path) -> std::path::PathBuf {
    dir.join(ENVELOPE_FILE)
}

/// The logical-head file path within `dir`.
pub fn head_path(dir: &Path) -> std::path::PathBuf {
    dir.join(HEAD_FILE)
}

/// The owner lock file path within `dir`.
#[cfg(test)]
pub fn lock_path(dir: &Path) -> std::path::PathBuf {
    dir.join(LOCK_FILE)
}

/// Whether all three durable artifacts (engine, envelope, head) are present in `dir`. A
/// store missing any one is incomplete — never published as complete — so a crash mid-build
/// (which leaves a temp directory, never a partial destination) can never be mistaken for a
/// finished store. The lock is deliberately excluded: it is transient, not a completeness
/// signal.
pub fn artifacts_present(dir: &Path) -> bool {
    engine_path(dir).is_file() && envelope_path(dir).is_file() && head_path(dir).is_file()
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

        let dir = Path::new("/stores/app");
        assert_eq!(engine_path(dir), Path::new("/stores/app/store.redb"));
        assert_eq!(envelope_path(dir), Path::new("/stores/app/envelope"));
        assert_eq!(head_path(dir), Path::new("/stores/app/head"));
        assert_eq!(lock_path(dir), Path::new("/stores/app/lock"));
    }

    /// Each artifact an admission read names resolves to exactly the frozen entry name, and
    /// each is admissible as one normal relative component.
    #[test]
    fn every_admitted_artifact_name_is_the_frozen_entry_name() {
        for (entry, expected) in [
            (StoreEntry::Envelope, ENVELOPE_FILE),
            (StoreEntry::Head, HEAD_FILE),
        ] {
            assert_eq!(admitted_name(entry).as_str(), expected);
        }
    }
}
