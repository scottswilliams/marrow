//! Publication's parent-directory barrier. Store metadata writes and barriers
//! belong to the retained descriptor in `store_dir`.

use std::path::Path;

/// Flush the parent entry after publication renames the completed stage.
#[cfg(unix)]
pub(crate) fn sync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
pub(crate) fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}
