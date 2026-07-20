//! Deterministic contained capture into an immutable `ProjectInput`.

use marrow_codes::Code;
use marrow_project::{
    CaptureBound, CaptureErrorKind, CaptureLimits, CapturedFile, CollisionReason,
    MAX_FILE_IDENTITY_BYTES, Manifest, ProjectInput,
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
    assert_eq!(error.code(), Code::ProjectModuleCollision);
    match error.kind() {
        CaptureErrorKind::ModuleCollision {
            module,
            first,
            second,
            reason,
        } => {
            assert_eq!(module.as_str(), "a.b");
            assert_eq!(*reason, CollisionReason::DuplicateModule);
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
    assert_eq!(error.code(), Code::ProjectModuleCollision);
    match error.kind() {
        CaptureErrorKind::ModuleCollision { reason, .. } => {
            assert_eq!(*reason, CollisionReason::CaseInsensitivePath);
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
    assert_eq!(a.code(), Code::ProjectSourcePath);
    assert_eq!(a.kind(), b.kind());
    match a.kind() {
        CaptureErrorKind::SourcePath { path, .. } => assert_eq!(path.as_str(), "/etc/x.mw"),
        other => panic!("expected a source-path fault, got {other:?}"),
    }
}

/// A valid single-component `.mw` identity of exactly `bytes` UTF-8 bytes.
fn identity_of_len(lead: char, bytes: usize) -> String {
    format!("src/{}.mw", lead.to_string().repeat(bytes - 7))
}

#[test]
fn an_over_long_identity_refuses_pathless_in_the_source_path_family() {
    let over = identity_of_len('a', MAX_FILE_IDENTITY_BYTES + 1);
    let error = capture(vec![file(&over, "x")]).expect_err("an over-long identity refuses");
    assert_eq!(error.code(), Code::ProjectSourcePath);
    // The sealed pathless kind carries only the bounded evidence, never the path.
    assert_eq!(
        *error.kind(),
        CaptureErrorKind::SourcePathTooLong {
            limit: MAX_FILE_IDENTITY_BYTES,
            actual: MAX_FILE_IDENTITY_BYTES + 1,
        }
    );
    assert!(
        !error.message().contains("aaaa"),
        "the message retains no raw path"
    );
}

#[test]
fn the_over_long_offender_is_the_lexically_smallest_raw_path_not_the_shortest() {
    // The lexicographically smaller path (`a…`) is the longer of the two; selection
    // is by raw spelling, and the reported `actual` is that path's own length.
    let smaller_longer = identity_of_len('a', MAX_FILE_IDENTITY_BYTES + 100);
    let larger_shorter = identity_of_len('z', MAX_FILE_IDENTITY_BYTES + 1);
    let expected = CaptureErrorKind::SourcePathTooLong {
        limit: MAX_FILE_IDENTITY_BYTES,
        actual: MAX_FILE_IDENTITY_BYTES + 100,
    };

    let forward =
        capture(vec![file(&larger_shorter, "y"), file(&smaller_longer, "x")]).expect_err("refuses");
    let reverse =
        capture(vec![file(&smaller_longer, "x"), file(&larger_shorter, "y")]).expect_err("refuses");
    assert_eq!(*forward.kind(), expected);
    assert_eq!(forward.kind(), reverse.kind());
}

#[test]
fn an_over_long_identity_precedes_a_syntactically_invalid_path() {
    // A valid-overbound identity and a syntax-invalid path together: the pathless
    // `TooLong` refusal wins, before the ordinary invalid-path collection.
    let over = identity_of_len('m', MAX_FILE_IDENTITY_BYTES + 1);
    let error = capture(vec![file("/etc/x.mw", "x"), file(&over, "y")]).expect_err("refuses");
    assert!(matches!(
        error.kind(),
        CaptureErrorKind::SourcePathTooLong { .. }
    ));
}

#[test]
fn an_identity_at_the_maximum_captures() {
    let at_max = identity_of_len('a', MAX_FILE_IDENTITY_BYTES);
    let input = capture(vec![file(&at_max, "x")]).expect("4096 bytes captures");
    assert_eq!(input.modules().len(), 1);
    assert_eq!(
        input.modules()[0].identity().as_str().len(),
        MAX_FILE_IDENTITY_BYTES
    );
}

#[test]
fn check_identity_bound_maps_only_the_valid_overbound_case() {
    // A valid in-bound identity and every syntax error return `Ok`; only a valid
    // over-long identity maps to the sealed pathless too-long refusal.
    assert!(CapturedFile::check_identity_bound("src/main.mw").is_ok());
    assert!(CapturedFile::check_identity_bound("/etc/x.mw").is_ok());
    assert!(CapturedFile::check_identity_bound("src/../y.mw").is_ok());
    assert!(CapturedFile::check_identity_bound("lib/z.mw").is_ok());

    let over = identity_of_len('a', MAX_FILE_IDENTITY_BYTES + 1);
    let error = CapturedFile::check_identity_bound(&over).expect_err("valid-overbound maps");
    assert_eq!(error.code(), Code::ProjectSourcePath);
    assert_eq!(
        *error.kind(),
        CaptureErrorKind::SourcePathTooLong {
            limit: MAX_FILE_IDENTITY_BYTES,
            actual: MAX_FILE_IDENTITY_BYTES + 1,
        }
    );
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
    assert_eq!(error.code(), Code::ProjectCaptureLimit);
    match error.kind() {
        CaptureErrorKind::CaptureLimit {
            bound,
            limit,
            actual,
        } => {
            assert_eq!(*bound, CaptureBound::FileCount);
            assert_eq!(*limit, 3);
            assert_eq!(*actual, 4);
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
    match error.kind() {
        CaptureErrorKind::CaptureLimit {
            bound,
            limit,
            actual,
        } => {
            assert_eq!(*bound, CaptureBound::FileBytes);
            assert_eq!(*limit, 4);
            assert_eq!(*actual, 5);
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
    match error.kind() {
        CaptureErrorKind::CaptureLimit {
            bound,
            limit,
            actual,
        } => {
            assert_eq!(*bound, CaptureBound::TotalBytes);
            assert_eq!(*limit, 6);
            assert_eq!(*actual, 7);
        }
        other => panic!("expected a capture-limit fault, got {other:?}"),
    }
}
