//! File-identity validation and path-derived module names.

use marrow_project::{FileIdentity, SourcePathReason};

fn module_name(path: &str) -> Option<String> {
    FileIdentity::validate(path)
        .ok()
        .map(|(_, module)| module.as_str().to_string())
}

fn reason(path: &str) -> Option<SourcePathReason> {
    FileIdentity::validate(path).err()
}

#[test]
fn derives_module_name_from_nested_path() {
    assert_eq!(
        module_name("src/shelf/books.mw").as_deref(),
        Some("shelf.books")
    );
    assert_eq!(module_name("src/a/b/c.mw").as_deref(), Some("a.b.c"));
}

#[test]
fn derives_single_segment_for_a_root_file() {
    assert_eq!(module_name("src/main.mw").as_deref(), Some("main"));
}

#[test]
fn identity_is_the_root_relative_path() {
    let (identity, _) = FileIdentity::validate("src/shelf/books.mw").expect("valid");
    assert_eq!(identity.as_str(), "src/shelf/books.mw");
}

#[test]
fn rejects_paths_outside_the_source_root() {
    assert_eq!(
        reason("lib/books.mw"),
        Some(SourcePathReason::OutsideSourceRoot)
    );
    assert_eq!(
        reason("books.mw"),
        Some(SourcePathReason::OutsideSourceRoot)
    );
    // `src` alone names the root directory, not a file under it.
    assert_eq!(reason("src"), Some(SourcePathReason::OutsideSourceRoot));
}

#[test]
fn rejects_non_marrow_files() {
    assert_eq!(
        reason("src/notes.txt"),
        Some(SourcePathReason::NotMarrowSource)
    );
    assert_eq!(reason("src/books"), Some(SourcePathReason::NotMarrowSource));
    // A bare `.mw` with an empty stem cannot name a module.
    assert_eq!(reason("src/.mw"), Some(SourcePathReason::NotMarrowSource));
}

#[test]
fn rejects_absolute_paths() {
    assert_eq!(reason("/src/books.mw"), Some(SourcePathReason::Absolute));
}

#[test]
fn rejects_parent_segments() {
    assert_eq!(reason("src/../secret.mw"), Some(SourcePathReason::Escapes));
    assert_eq!(reason("../src/books.mw"), Some(SourcePathReason::Escapes));
}

#[test]
fn rejects_non_canonical_paths() {
    assert_eq!(reason(""), Some(SourcePathReason::NonCanonical));
    assert_eq!(
        reason("src//books.mw"),
        Some(SourcePathReason::NonCanonical)
    );
    assert_eq!(
        reason("src/./books.mw"),
        Some(SourcePathReason::NonCanonical)
    );
    assert_eq!(
        reason("src\\books.mw"),
        Some(SourcePathReason::NonCanonical)
    );
}

#[test]
fn a_dotted_stem_derives_a_name_that_collides_with_a_nested_path() {
    // `file_stem` strips only the final `.mw`, so `src/a.b.mw` derives module
    // `a.b` — the same name `src/a/b.mw` derives. Capture reports the collision.
    assert_eq!(module_name("src/a.b.mw").as_deref(), Some("a.b"));
    assert_eq!(module_name("src/a/b.mw").as_deref(), Some("a.b"));
}
