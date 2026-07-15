//! Deterministic contained capture into an immutable `ProjectInput`.

use marrow_project::{
    CaptureBound, CaptureErrorKind, CaptureLimits, CapturedFile, CollisionReason, Manifest,
    ProjectInput,
};

fn manifest() -> Manifest {
    Manifest::parse("edition = \"2026\"\n").expect("valid manifest")
}

fn file(path: &str, body: &str) -> CapturedFile {
    CapturedFile::new(path.to_string(), body.as_bytes().to_vec())
}

fn capture(files: Vec<CapturedFile>) -> Result<ProjectInput, marrow_project::CaptureError> {
    marrow_project::capture(&manifest(), files, None, &CaptureLimits::DEFAULT)
}

fn identities(input: &ProjectInput) -> Vec<String> {
    input
        .modules()
        .iter()
        .map(|m| m.identity().as_str().to_string())
        .collect()
}

fn module_names(input: &ProjectInput) -> Vec<String> {
    input
        .modules()
        .iter()
        .map(|m| m.module().as_str().to_string())
        .collect()
}

#[test]
fn captures_modules_in_canonical_order() {
    let input = capture(vec![
        file("src/main.mw", "pub fn main()\n    return\n"),
        file("src/shelf/books.mw", "pub fn a()\n    return\n"),
    ])
    .expect("valid project");
    assert_eq!(input.edition().as_str(), "2026");
    assert_eq!(identities(&input), ["src/main.mw", "src/shelf/books.mw"]);
    assert_eq!(module_names(&input), ["main", "shelf.books"]);
    assert_eq!(input.modules()[0].source(), b"pub fn main()\n    return\n");
}

#[test]
fn discovery_order_does_not_change_the_result() {
    let ordered = capture(vec![
        file("src/a.mw", "a"),
        file("src/b.mw", "b"),
        file("src/c.mw", "c"),
    ])
    .expect("valid");
    let shuffled = capture(vec![
        file("src/c.mw", "c"),
        file("src/a.mw", "a"),
        file("src/b.mw", "b"),
    ])
    .expect("valid");
    assert_eq!(ordered, shuffled);
}

#[test]
fn determinism_yields_identical_project_input() {
    // The owner sees only root-relative paths, so the same listing always
    // produces a byte-identical `ProjectInput`. The on-disk relocation evidence
    // is the CLI test `relocation_produces_identical_formatted_bytes`.
    let a = capture(vec![file("src/shelf/books.mw", "pub fn a()\n    return\n")]).expect("valid");
    let b = capture(vec![file("src/shelf/books.mw", "pub fn a()\n    return\n")]).expect("valid");
    assert_eq!(identities(&a), identities(&b));
    assert_eq!(a, b);
}

#[test]
fn empty_project_is_valid() {
    let input = capture(vec![]).expect("an empty project is valid");
    assert!(input.modules().is_empty());
}

#[test]
fn duplicate_module_identity_rejects() {
    let error = capture(vec![file("src/a/b.mw", "x"), file("src/a.b.mw", "y")])
        .expect_err("colliding module names reject");
    assert_eq!(error.code, "project.module_collision");
    match error.kind {
        CaptureErrorKind::ModuleCollision {
            module,
            first,
            second,
            reason,
        } => {
            assert_eq!(module.as_str(), "a.b");
            assert_eq!(reason, CollisionReason::DuplicateModule);
            // Offenders are named smaller-identity-first, deterministically.
            assert_eq!(first.as_str(), "src/a.b.mw");
            assert_eq!(second.as_str(), "src/a/b.mw");
        }
        other => panic!("expected a module collision, got {other:?}"),
    }
}

#[test]
fn case_insensitive_path_collision_rejects() {
    let error = capture(vec![file("src/Books.mw", "x"), file("src/books.mw", "y")])
        .expect_err("case-only difference rejects");
    assert_eq!(error.code, "project.module_collision");
    match error.kind {
        CaptureErrorKind::ModuleCollision { reason, .. } => {
            assert_eq!(reason, CollisionReason::CaseInsensitivePath);
        }
        other => panic!("expected a case collision, got {other:?}"),
    }
}

#[test]
fn an_invalid_path_rejects_deterministically() {
    // Two invalid paths in either order report the lexicographically-smaller one.
    let a = capture(vec![file("/etc/x.mw", "x"), file("src/../y.mw", "y")])
        .expect_err("invalid paths reject");
    let b = capture(vec![file("src/../y.mw", "y"), file("/etc/x.mw", "x")])
        .expect_err("invalid paths reject");
    assert_eq!(a.code, "project.source_path");
    assert_eq!(a.kind, b.kind);
    match a.kind {
        CaptureErrorKind::SourcePath { path, .. } => assert_eq!(path, "/etc/x.mw"),
        other => panic!("expected a source-path fault, got {other:?}"),
    }
}

/// The three files the capture-limit boundary probes use, so `N` = 3.
fn three_files() -> Vec<CapturedFile> {
    vec![
        file("src/a.mw", "a"),
        file("src/b.mw", "b"),
        file("src/c.mw", "c"),
    ]
}

#[test]
fn file_count_limit_boundary() {
    let limits = CaptureLimits::new(3, 1 << 20, 1 << 20);
    let manifest = manifest();

    // 0, 1, and N files all capture; N+1 rejects.
    assert!(marrow_project::capture(&manifest, vec![], None, &limits).is_ok());
    assert!(marrow_project::capture(&manifest, vec![file("src/a.mw", "a")], None, &limits).is_ok());
    assert!(marrow_project::capture(&manifest, three_files(), None, &limits).is_ok());

    let mut over = three_files();
    over.push(file("src/d.mw", "d"));
    let error = marrow_project::capture(&manifest, over, None, &limits).expect_err("N+1 rejects");
    assert_eq!(error.code, "project.capture_limit");
    match error.kind {
        CaptureErrorKind::CaptureLimit {
            bound,
            limit,
            actual,
        } => {
            assert_eq!(bound, CaptureBound::FileCount);
            assert_eq!(limit, 3);
            assert_eq!(actual, 4);
        }
        other => panic!("expected a capture-limit fault, got {other:?}"),
    }
}

#[test]
fn per_file_byte_limit_boundary() {
    let limits = CaptureLimits::new(16, 4, 1 << 20);
    let manifest = manifest();

    assert!(
        marrow_project::capture(&manifest, vec![file("src/a.mw", "abcd")], None, &limits).is_ok()
    );
    let error = marrow_project::capture(&manifest, vec![file("src/a.mw", "abcde")], None, &limits)
        .expect_err("oversize file rejects");
    match error.kind {
        CaptureErrorKind::CaptureLimit {
            bound,
            limit,
            actual,
        } => {
            assert_eq!(bound, CaptureBound::FileBytes);
            assert_eq!(limit, 4);
            assert_eq!(actual, 5);
        }
        other => panic!("expected a capture-limit fault, got {other:?}"),
    }
}

#[test]
fn total_byte_limit_boundary() {
    let limits = CaptureLimits::new(16, 1 << 20, 6);
    let manifest = manifest();

    // Two 3-byte files sit exactly at the 6-byte total.
    assert!(
        marrow_project::capture(
            &manifest,
            vec![file("src/a.mw", "aaa"), file("src/b.mw", "bbb")],
            None,
            &limits
        )
        .is_ok()
    );
    let error = marrow_project::capture(
        &manifest,
        vec![file("src/a.mw", "aaa"), file("src/b.mw", "bbbb")],
        None,
        &limits,
    )
    .expect_err("over-total rejects");
    match error.kind {
        CaptureErrorKind::CaptureLimit {
            bound,
            limit,
            actual,
        } => {
            assert_eq!(bound, CaptureBound::TotalBytes);
            assert_eq!(limit, 6);
            assert_eq!(actual, 7);
        }
        other => panic!("expected a capture-limit fault, got {other:?}"),
    }
}
