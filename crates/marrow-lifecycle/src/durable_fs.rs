//! Publication's parent-directory barrier. Store metadata writes and barriers
//! belong to the retained descriptor in `store_dir`.

use std::path::Path;

use marrow_fs_journal::{AdmittedDir, CustodyError, EntryName, FsIdentity};

pub(crate) fn custody_io(error: CustodyError) -> std::io::Error {
    let kind = match &error {
        CustodyError::Io { source, .. } => source.kind(),
        CustodyError::AlreadyExists { .. } => std::io::ErrorKind::AlreadyExists,
        CustodyError::NotFound { .. } => std::io::ErrorKind::NotFound,
        CustodyError::Unsupported { .. } | CustodyError::UnqualifiedPlatform { .. } => {
            std::io::ErrorKind::Unsupported
        }
        _ => std::io::ErrorKind::Other,
    };
    std::io::Error::new(kind, error)
}

/// One trusted parent and two admitted sibling names. Publication never replaces
/// an existing entry, including an empty directory or a dangling symbolic link.
pub(crate) struct Publication {
    parent: AdmittedDir,
    stage: EntryName,
    destination: EntryName,
}

impl Publication {
    pub(crate) fn admit(stage: &Path, destination: &Path) -> std::io::Result<Self> {
        fn parent(path: &Path) -> &Path {
            path.parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
        }
        fn name(path: &Path) -> std::io::Result<EntryName> {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "publication requires a UTF-8 entry name",
                    )
                })?;
            EntryName::admit(name)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
        }
        if parent(stage) != parent(destination) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "publication requires sibling entries",
            ));
        }
        let stage = name(stage)?;
        let destination_name = name(destination)?;
        if stage == destination_name {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "publication stage equals destination",
            ));
        }
        let parent = AdmittedDir::admit_trusted_root(parent(destination)).map_err(custody_io)?;
        Ok(Self {
            parent,
            stage,
            destination: destination_name,
        })
    }

    pub(crate) fn publish(&self, identity: FsIdentity) -> Result<(), CustodyError> {
        if self
            .parent
            .stat_entry(&self.stage)?
            .is_none_or(|entry| entry.identity() != identity)
        {
            return Err(CustodyError::IdentityDrift {
                op: "publish staged entry",
            });
        }
        self.parent.rename_noreplace(&self.stage, &self.destination)
    }

    pub(crate) fn sync(&self) -> std::io::Result<()> {
        self.parent.sync().map_err(custody_io)
    }
}

/// Flush the parent entry after publication renames the completed stage.
#[cfg(unix)]
pub(crate) fn sync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
pub(crate) fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}
