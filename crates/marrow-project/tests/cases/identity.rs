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
fn rejects_control_characters() {
    // NUL and ASCII control characters are wrong under any future module-name
    // character domain, so the owner rejects them today; the full character and
    // Unicode-normalization domain lands with the module-name semantic owner.
    assert_eq!(
        reason("src/bo\0oks.mw"),
        Some(SourcePathReason::NonCanonical)
    );
    assert_eq!(
        reason("src/bo\toks.mw"),
        Some(SourcePathReason::NonCanonical)
    );
    assert_eq!(
        reason("src/bo\noks.mw"),
        Some(SourcePathReason::NonCanonical)
    );
    assert_eq!(
        reason("src/bo\u{1b}oks.mw"),
        Some(SourcePathReason::NonCanonical)
    );
}

/// Every path `validate` accepts or rejects, `check` must classify identically.
/// `check` is the one allocation-free reason owner; `validate` delegates to it,
/// so they can never disagree on acceptance or on the exact reason.
const REASON_PARITY_PATHS: &[&str] = &[
    "src/main.mw",
    "src/shelf/books.mw",
    "src/a/b/c.mw",
    "src/a.b.mw",
    "lib/books.mw",
    "books.mw",
    "src",
    "src/notes.txt",
    "src/books",
    "src/.mw",
    "/src/books.mw",
    "src/../secret.mw",
    "../src/books.mw",
    "",
    "src//books.mw",
    "src/./books.mw",
    "src\\books.mw",
    "src/bo\0oks.mw",
    "src/bo\toks.mw",
    "src/bo\noks.mw",
    "src/bo\u{1b}oks.mw",
];

#[test]
fn check_and_validate_agree_on_every_reason() {
    for &path in REASON_PARITY_PATHS {
        let checked = FileIdentity::check(path);
        let validated = FileIdentity::validate(path);
        assert_eq!(
            checked.is_ok(),
            validated.is_ok(),
            "check and validate disagree on acceptance of {path:?}"
        );
        assert_eq!(
            checked.err(),
            validated.err(),
            "check and validate disagree on the reason for {path:?}"
        );
    }
}

#[test]
fn check_does_not_change_the_identities_validate_builds() {
    // Delegation to `check` leaves the exact identity and module bytes unchanged.
    let (identity, module) = FileIdentity::validate("src/shelf/books.mw").expect("valid");
    assert_eq!(identity.as_str(), "src/shelf/books.mw");
    assert_eq!(module.as_str(), "shelf.books");
    assert!(FileIdentity::check("src/shelf/books.mw").is_ok());
}

/// A syntactically valid single-component `.mw` identity of exactly `bytes`
/// UTF-8 bytes: `src/` (4) plus an ASCII stem plus `.mw` (3).
fn ascii_identity(bytes: usize) -> String {
    format!("src/{}.mw", "a".repeat(bytes - 7))
}

#[test]
fn a_valid_identity_at_the_maximum_is_accepted_and_one_over_is_refused() {
    let at_max = ascii_identity(4096);
    assert_eq!(at_max.len(), 4096);
    assert!(FileIdentity::check(&at_max).is_ok(), "4096 bytes is accepted");
    assert!(FileIdentity::validate(&at_max).is_ok());

    let over = ascii_identity(4097);
    assert_eq!(over.len(), 4097);
    assert!(FileIdentity::check(&over).is_err(), "4097 bytes is refused");
    assert!(FileIdentity::validate(&over).is_err());
}

#[test]
fn a_dotted_stem_derives_a_name_that_collides_with_a_nested_path() {
    // `file_stem` strips only the final `.mw`, so `src/a.b.mw` derives module
    // `a.b` — the same name `src/a/b.mw` derives. Capture reports the collision.
    assert_eq!(module_name("src/a.b.mw").as_deref(), Some("a.b"));
    assert_eq!(module_name("src/a/b.mw").as_deref(), Some("a.b"));
}
