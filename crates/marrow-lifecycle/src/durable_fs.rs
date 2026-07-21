//! Durable filesystem primitives shared by provision and the lifecycle actor: the single
//! owner of "make a byte payload or a directory entry durable on disk".
//!
//! Two write shapes appear in the lifecycle path. Provision builds fresh files in a private
//! temporary directory ([`write_file`]); the actor replaces a live file in place under an
//! atomic rename ([`replace_file`]). Both flush the file body before it is observable, and
//! [`sync_dir`] flushes the directory entry so a newly created or renamed file survives a
//! crash. Keeping the three here means the fsync-before-rename discipline has one owner rather
//! than a copy per caller.

use std::path::Path;

/// Write `bytes` to `path` (creating it) and flush the body to disk. The caller flushes the
/// containing directory (via [`sync_dir`]) once all files are written, so a fresh file is
/// durable before the directory entry that names it.
pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Write `bytes` to a sibling temporary file, flush it, and atomically rename it over `path`.
/// A reader of `path` sees it wholly old or wholly new, never torn; the caller flushes the
/// directory (via [`sync_dir`]) afterward so the rename itself is durable.
pub(crate) fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let temp = path.with_extension("replacing");
    {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temp, path)
}

/// Flush a directory entry to disk so a newly created or renamed file within it is durable.
#[cfg(unix)]
pub(crate) fn sync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
pub(crate) fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}
