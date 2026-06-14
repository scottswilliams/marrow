use std::path::Path;

use marrow_project::expected_module_name;

#[test]
fn derives_module_name_from_nested_path() {
    assert_eq!(
        expected_module_name(Path::new("shelf/books.mw")).as_deref(),
        Some("shelf::books")
    );
    assert_eq!(
        expected_module_name(Path::new("a/b/c.mw")).as_deref(),
        Some("a::b::c")
    );
}

#[test]
fn derives_single_segment_for_a_root_file() {
    assert_eq!(
        expected_module_name(Path::new("books.mw")).as_deref(),
        Some("books")
    );
}

#[test]
fn ignores_a_leading_current_directory() {
    assert_eq!(
        expected_module_name(Path::new("./shelf/books.mw")).as_deref(),
        Some("shelf::books")
    );
}

#[test]
fn rejects_non_mw_files() {
    assert_eq!(expected_module_name(Path::new("shelf/books.txt")), None);
    assert_eq!(expected_module_name(Path::new("shelf/books")), None);
}

#[test]
fn rejects_paths_that_escape_the_source_root() {
    assert_eq!(expected_module_name(Path::new("../shelf/books.mw")), None);
}

#[test]
fn dotted_stem_derives_a_name_that_can_never_match() {
    // `file_stem` strips only the final `.mw`, so `a.b.mw` derives `shelf::a.b`.
    // That segment is not a valid identifier, so no declaration can match it;
    // the mismatch surfaces as a path/declaration error in the checker.
    assert_eq!(
        expected_module_name(Path::new("shelf/a.b.mw")).as_deref(),
        Some("shelf::a.b")
    );
}
