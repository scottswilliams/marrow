//! Shared project-setup harness for the `marrow-check` integration tests.
//!
//! Every checker test drives the real `check_project` or `analyze_project`
//! pipeline over a throwaway on-disk project. This module is the single owner of
//! that setup: a uniquely named temp directory, a recursive file writer, and the
//! standard `src`-rooted config.
//!
//! [`TempProject`] removes its directory on drop, so a test never cleans up by
//! hand and a panicking assertion still releases the directory.
//!
//! Each test binary includes this module, so not every binary exercises every
//! helper; the crate-wide `dead_code` allowance keeps the shared surface intact.

#![allow(dead_code)]

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use marrow_project::{ProjectConfig, parse_config};

static NEXT_PROJECT_SERIAL: AtomicU64 = AtomicU64::new(0);

/// A temporary project directory removed when the value is dropped.
///
/// Derefs to its root [`Path`], so it passes straight into `check_project`,
/// `analyze_project`, and any other `&Path` consumer without an explicit
/// accessor.
pub struct TempProject {
    root: PathBuf,
}

impl Deref for TempProject {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

/// Create an empty, uniquely named project root removed on drop.
///
/// The name is suffixed with the process id plus a nanosecond clock reading and
/// a process-unique serial, so parallel test threads never share a directory and
/// one test's cleanup cannot race another's read.
pub fn temp_root(name: &str) -> TempProject {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let serial = NEXT_PROJECT_SERIAL.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "marrow-{name}-{}-{nanos}-{serial}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("create project root");
    TempProject { root }
}

/// Create a uniquely named project root and let `build` populate its files.
pub fn temp_project(name: &str, build: impl FnOnce(&Path)) -> TempProject {
    let root = temp_root(name);
    build(&root);
    root
}

/// Write `contents` to `root/relative`, creating parent directories as needed.
pub fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

/// The standard project config: a single `src` source root.
pub fn config() -> ProjectConfig {
    parse_config(r#"{ "sourceRoots": ["src"] }"#).expect("config")
}
