//! The one image stager: a compiled image written where a companion runner can read
//! it and nowhere else.
//!
//! Every command that hands an image to a stock runner — the attached terminal, and
//! the CLI's store and import commands — stages it here. The directory is created
//! `0700` and the image `0600` with `create_new`, so the file is never an existing
//! path, a symlink target, or world-readable, and the directory name is drawn from OS
//! entropy so two stagers never collide.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::channel::mint_id;

/// A compiled image staged in a private directory for a companion to read and verify
/// independently. Dropping removes the directory; [`StagedImage::remove`] does the
/// same while reporting failure, and [`StagedImage::retain`] deliberately leaves it.
pub struct StagedImage {
    dir: PathBuf,
    image: PathBuf,
    armed: bool,
}

/// Stage `image_bytes` for a companion runner.
pub fn stage_image(image_bytes: &[u8]) -> io::Result<StagedImage> {
    let dir = stage_dir();
    create_private_dir(&dir)?;
    let image = dir.join("image.mwi");
    let staged = StagedImage {
        dir,
        image,
        armed: true,
    };
    write_private(&staged.image, image_bytes)?;
    Ok(staged)
}

impl StagedImage {
    /// The staged image file, passed to the companion as `--image`.
    pub fn path(&self) -> &Path {
        &self.image
    }

    /// The private directory holding the image.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Remove the directory now and report an I/O failure to the caller.
    pub(crate) fn remove(&mut self) -> io::Result<()> {
        self.armed = false;
        std::fs::remove_dir_all(&self.dir)
    }

    /// Leave the directory on disk and name it, for a caller that could not confirm
    /// the reader is gone.
    pub(crate) fn retain(&mut self) -> PathBuf {
        self.armed = false;
        self.dir.clone()
    }
}

impl Drop for StagedImage {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// A private staging directory named from OS entropy so two stagers never collide.
fn stage_dir() -> PathBuf {
    let suffix = mint_id().map(|id| id.to_hex()).unwrap_or_else(|_| {
        format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )
    });
    std::env::temp_dir().join(format!("marrow-run-{suffix}"))
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(dir)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}
