//! Positive external consumer test for the capture entry point and presentation
//! facade, importing only `marrow_project_fs` and the standard library.

use std::path::Path;

use marrow_project_fs::{Code, OverlayEntry, OverlaySnapshot, capture_project};

/// A path that does not exist, so capture fails at the manifest with no fixture.
const ABSENT_ROOT: &str = "/marrow-project-fs-consumer-absent-root-zzz";

#[test]
fn a_consumer_presents_a_capture_failure_through_the_public_facade() {
    let root = Path::new(ABSENT_ROOT);
    let failure =
        capture_project(root, OverlaySnapshot::empty()).expect_err("the manifest is absent");

    let presentation = failure.presentation(root);
    assert_eq!(presentation.code(), Code::IoRead);

    let mut message = String::new();
    presentation
        .write_operational_message(&mut message)
        .expect("string sink");
    assert_eq!(
        message,
        format!("failed to read {ABSENT_ROOT}/marrow.toml"),
        "the operational writer omits operating-system prose"
    );
    assert!(presentation.position().is_none());
}

#[test]
fn a_nonempty_overlay_is_refused_before_any_filesystem_access_and_is_presentable() {
    let root = Path::new(ABSENT_ROOT);
    let entries = [OverlayEntry::new("src/main.mw", b"fn main() {}")];
    let snapshot = OverlaySnapshot::try_new(&entries).expect("baseline try_new is infallible");

    // The nonempty overlay is refused before the absent manifest is even read.
    let failure = capture_project(root, snapshot).expect_err("a nonempty overlay is refused");
    let presentation = failure.presentation(root);

    let mut message = String::new();
    presentation
        .write_operational_message(&mut message)
        .expect("string sink");
    assert!(!message.is_empty());
    assert!(presentation.position().is_none());
}
