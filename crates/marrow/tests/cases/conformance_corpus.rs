use std::fs;
use std::path::{Path, PathBuf};

use crate::support;
use serde_json::Value;

/// The conformance corpus root: every subdirectory is a complete fixture
/// project whose `.mw` tests run through native `marrow test`.
fn corpus_root() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/v01/conformance"
    ))
}

fn corpus_fixtures() -> Vec<PathBuf> {
    let mut fixtures: Vec<PathBuf> = fs::read_dir(corpus_root())
        .expect("read conformance corpus root")
        .map(|entry| entry.expect("corpus entry").path())
        .filter(|path| path.is_dir())
        .collect();
    fixtures.sort();
    assert!(!fixtures.is_empty(), "the conformance corpus has fixtures");
    fixtures
}

/// Every fixture test module opens with the corpus header naming what the
/// fixture exercises, how it asserts, and which Rust test it replaced. The
/// header keeps the corpus navigable as suites migrate onto it; a fixture
/// without one is unlanded work.
fn assert_corpus_header(fixture: &Path) {
    let tests_dir = fixture.join("tests");
    let mut test_files: Vec<PathBuf> = fs::read_dir(&tests_dir)
        .unwrap_or_else(|_| panic!("fixture {} has a tests directory", fixture.display()))
        .map(|entry| entry.expect("tests entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "mw"))
        .collect();
    test_files.sort();
    assert!(
        !test_files.is_empty(),
        "fixture {} has .mw test modules",
        fixture.display()
    );
    for file in test_files {
        let text = fs::read_to_string(&file).expect("read fixture test module");
        let header: Vec<&str> = text
            .lines()
            .take_while(|line| line.starts_with(';'))
            .collect();
        for field in ["; Layer:", "; Oracle:", "; Replaces:"] {
            assert!(
                header.iter().any(|line| line.starts_with(field)),
                "{} is missing the corpus header field `{field}`",
                file.display()
            );
        }
    }
}

/// Run one fixture through `marrow test --format jsonl` and assert the typed
/// report: at least one test selected, every outcome `passed`.
fn assert_fixture_passes(fixture: &Path) {
    let output = support::marrow(&[
        "test",
        "--format",
        "jsonl",
        fixture.to_str().expect("fixture path utf8"),
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "fixture {}: {output:?}",
        fixture.display()
    );
    let records = support::jsonl(output.stdout);
    let summary = records
        .iter()
        .find(|record| record["kind"] == "summary")
        .unwrap_or_else(|| panic!("fixture {} reports a summary", fixture.display()));
    assert!(
        summary["selected"].as_u64().is_some_and(|count| count > 0),
        "fixture {} selects tests: {summary}",
        fixture.display()
    );
    assert_eq!(
        (summary["failed"].clone(), summary["errored"].clone()),
        (Value::from(0), Value::from(0)),
        "fixture {} passes: {records:#?}",
        fixture.display()
    );
    for record in records.iter().filter(|record| record["kind"] == "test") {
        assert_eq!(
            record["outcome"],
            Value::from("passed"),
            "fixture {} test record: {record}",
            fixture.display()
        );
    }
}

#[test]
fn every_conformance_fixture_carries_its_corpus_header() {
    for fixture in corpus_fixtures() {
        assert_corpus_header(&fixture);
    }
}

#[test]
fn every_conformance_fixture_passes_through_native_marrow_test() {
    for fixture in corpus_fixtures() {
        assert_fixture_passes(&fixture);
    }
}
