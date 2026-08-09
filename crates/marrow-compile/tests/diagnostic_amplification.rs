//! A5: no pre-collector amplification. An error-dense many-file project is
//! bounded by the one compiler collector's ceiling — the drive absorbs each
//! file's syntax terminal immediately after parsing it, so no un-absorbed
//! per-file diagnostic collection ever accumulates. The structural half of the
//! law (no collection of un-absorbed `ParsedSource` values can exist) is
//! enforced by the `ParsedSource` absence gate in `absence_gates.rs`.

use std::sync::Arc;

use marrow_compile::{
    AnalysisFailure, AnalysisResourceLimit, CompileFailure, InputRevision, ResourceLimitKind,
    analyze, compile, compile_with_tests,
};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};
use marrow_syntax::{SYNTAX_DIAGNOSTIC_COUNT_LIMIT, SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT};

fn project(files: Vec<(String, Vec<u8>)>) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .into_iter()
        .map(|(path, source)| CapturedFile::new(path, source))
        .collect();
    marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

fn assert_limit(failure: CompileFailure, kind: ResourceLimitKind, limit: usize) {
    let CompileFailure::ResourceLimit(resource) = failure else {
        panic!("expected the compiler collector's ceiling, got {failure:?}");
    };
    assert_eq!(resource.kind(), kind);
    assert_eq!(resource.limit(), limit as u64);
}

/// 512 files, each with 16 syntax errors (8192 rows — twice the ceiling),
/// resolve to exactly the typed DiagnosticCount ceiling on every public entry.
/// The retained outcome is the ceiling itself: no per-file collection survives
/// to amplify retention with the file count.
#[test]
fn an_error_dense_many_file_project_is_bounded_by_the_count_ceiling() {
    let files: Vec<(String, Vec<u8>)> = (0..512)
        .map(|index| (format!("src/m{index:03}.mw"), "@\n".repeat(16).into_bytes()))
        .collect();
    let input = project(files);

    assert_limit(
        compile(&input).expect_err("twice the ceiling must not compile"),
        ResourceLimitKind::DiagnosticCount,
        SYNTAX_DIAGNOSTIC_COUNT_LIMIT,
    );
    assert_limit(
        compile_with_tests(&input).expect_err("test compilation shares the ceiling"),
        ResourceLimitKind::DiagnosticCount,
        SYNTAX_DIAGNOSTIC_COUNT_LIMIT,
    );

    let revision = InputRevision::new(21);
    let failure = analyze(Arc::new(input), revision)
        .err()
        .expect("analysis shares the ceiling");
    assert_eq!(failure.revision(), revision);
    let AnalysisFailure::ResourceLimit {
        limit: AnalysisResourceLimit::Compile(limit),
        ..
    } = failure
    else {
        panic!("expected the shared compile diagnostic ceiling");
    };
    assert_eq!(limit.kind(), ResourceLimitKind::DiagnosticCount);
    assert_eq!(limit.limit(), SYNTAX_DIAGNOSTIC_COUNT_LIMIT as u64);
}

/// A byte-dense many-file project (about three times the byte ceiling of
/// rendered semantic diagnostics) resolves to the typed DiagnosticBytes
/// ceiling: retained bytes never scale with the file count past the one
/// collector's bound.
#[test]
fn a_byte_dense_many_file_project_is_bounded_by_the_byte_ceiling() {
    let files: Vec<(String, Vec<u8>)> = (0..24)
        .map(|tag| {
            let mut source = String::new();
            for index in 0..26 {
                let prefix = format!("u{tag}_{index}_");
                let name = format!("{prefix}{}", "a".repeat(5100 - prefix.len()));
                source.push_str(&format!("use {name}\n"));
            }
            (format!("src/m{tag:02}.mw"), source.into_bytes())
        })
        .collect();
    let input = project(files);

    assert_limit(
        compile(&input).expect_err("three times the byte ceiling must not compile"),
        ResourceLimitKind::DiagnosticBytes,
        SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT,
    );
}
