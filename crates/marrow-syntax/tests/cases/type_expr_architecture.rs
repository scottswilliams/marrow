//! Enforcement artifact for the one type-spelling grammar.
//!
//! Type annotations are parsed once into the structural `TypeExpr` node, and every
//! downstream crate matches on that node. Two shadow paths this replaced must stay
//! gone: the `TypeRef { text: String }` string carrier that shipped the spelling as
//! opaque text, and the string re-parse of type structure (`strip_prefix("sequence["`,
//! `strip_prefix("Id(^"`) that recovered `sequence[T]` and `Id(^root)` from that text.
//! A resurrection of either is caught here rather than silently reintroducing a second
//! grammar.
//!
//! Blind spot: this is a literal-pattern scan, so a respelled structure-parse — a
//! `contains`/`split`/re-lex of type text under a different shape — would slip past
//! it. The load-bearing enforcement is the typed node itself: `TypeExpr` exposes no
//! raw type-text accessor for downstream crates to re-parse, only `Name.text`, a
//! genuine leaf-name spelling. This scan guards the two historical regressions;
//! reviewers still block a new type-text re-parse the scan cannot name.

use std::fs;
use std::path::{Path, PathBuf};

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory")
        .to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn production_sources() -> Vec<PathBuf> {
    let crates_dir = crates_dir();
    let mut sources = Vec::new();
    for entry in fs::read_dir(&crates_dir)
        .expect("read crates dir")
        .flatten()
    {
        let src = entry.path().join("src");
        if src.is_dir() {
            rust_sources(&src, &mut sources);
        }
    }
    sources
}

fn scan(pattern: &str) -> Vec<String> {
    let crates_dir = crates_dir();
    let mut offenders = Vec::new();
    for source in production_sources() {
        let text = fs::read_to_string(&source).expect("read source");
        for (line_index, line) in text.lines().enumerate() {
            if line.contains(pattern) {
                offenders.push(format!(
                    "{}:{}",
                    source
                        .strip_prefix(&crates_dir)
                        .unwrap_or(&source)
                        .display(),
                    line_index + 1
                ));
            }
        }
    }
    offenders
}

#[test]
fn the_type_ref_string_carrier_is_gone() {
    let offenders = scan("TypeRef");
    assert!(
        offenders.is_empty(),
        "`TypeRef` is the deleted string-carrier type; a type annotation is the \
         structural `TypeExpr`. Reintroducing it would ship a spelling as opaque text \
         for downstream crates to re-parse:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn type_structure_is_never_recovered_by_string_parsing() {
    let mut offenders = Vec::new();
    for pattern in ["strip_prefix(\"sequence[", "strip_prefix(\"Id(^"] {
        offenders.extend(scan(pattern));
    }
    assert!(
        offenders.is_empty(),
        "`sequence[T]` and `Id(^root)` structure is classified once by the type parser; \
         re-parsing it from text is a second grammar:\n{}",
        offenders.join("\n")
    );
}
