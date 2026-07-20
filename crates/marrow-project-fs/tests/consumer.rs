//! Positive external consumer test for the capture entry point and presentation
//! facade, importing only `marrow_project_fs` and the standard library.

use std::fs;
use std::path::{Path, PathBuf};

use marrow_project_fs::{Code, OverlayEntry, OverlaySnapshot, capture_project};

/// A temporary directory removed on drop.
struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "marrow-project-fs-consumer-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create temp dir");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn write(&self, relative: &str, contents: &[u8]) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write fixture");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

#[test]
fn a_consumer_presents_a_capture_failure_through_the_public_facade() {
    // A readable project root with no manifest yields a presentable manifest read
    // failure through the public facade.
    let temp = TempDir::new("missing-manifest");
    let failure =
        capture_project(temp.path(), OverlaySnapshot::empty()).expect_err("the manifest is absent");

    let presentation = failure.presentation(temp.path());
    assert_eq!(presentation.code(), Code::IoRead);

    let mut message = String::new();
    presentation
        .write_operational_message(&mut message)
        .expect("string sink");
    assert_eq!(
        message,
        format!("failed to read {}/marrow.toml", temp.path().display()),
        "the operational writer omits operating-system prose"
    );
    assert!(presentation.position().is_none());
}

#[test]
fn a_nonmember_overlay_is_refused_and_presentable() {
    // An overlay whose key names no captured source is refused after a successful
    // pure capture, and the refusal is presentable through the facade.
    let temp = TempDir::new("nonmember-overlay");
    temp.write("marrow.toml", b"edition = \"2026\"\n");
    temp.write("src/main.mw", b"pub fn main()\n");

    let entries = [OverlayEntry::new("src/ghost.mw", b"fn main() {}")];
    let snapshot = OverlaySnapshot::try_new(&entries).expect("a valid overlay constructs");

    let failure =
        capture_project(temp.path(), snapshot).expect_err("a nonmember overlay is refused");
    let presentation = failure.presentation(temp.path());

    let mut message = String::new();
    presentation
        .write_operational_message(&mut message)
        .expect("string sink");
    assert!(!message.is_empty());
    assert!(presentation.position().is_none());
}
