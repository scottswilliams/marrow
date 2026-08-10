//! Production-path compiler diagnostic-bound laws: the one bounded compiler
//! collector, its consuming syntax bridge, the typed Count/OwnedBytes ceilings
//! (equal to the syntax ceilings, drift-pinned through the public limit values),
//! the invalid-UTF-8 contract, and the production/analysis stage tables.

use std::sync::Arc;

use marrow_compile::{
    AnalysisFailure, AnalysisResourceLimit, CompileFailure, InputRevision, ResourceLimitKind,
    SourceDiagnostic, analyze, compile, compile_with_tests,
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

fn diagnostics(input: &ProjectInput) -> Vec<SourceDiagnostic> {
    match compile(input) {
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        other => panic!("expected a diagnostics failure, got {other:?}"),
    }
}

fn assert_diagnostic_resource_limit(
    failure: CompileFailure,
    kind: ResourceLimitKind,
    limit: usize,
) {
    let CompileFailure::ResourceLimit(resource) = failure else {
        panic!("expected a diagnostic resource limit, got {failure:?}");
    };
    assert_eq!(resource.kind(), kind);
    assert_eq!(resource.limit(), limit as u64);
}

/// One deterministic syntax error per `@` character: the lexer reports each as
/// `unexpected character` and consumes it, and no token reaches the parser, so
/// the per-line diagnostic count is exact. The premise is asserted here through
/// the public syntax API so a lexer change fails this probe rather than
/// silently skewing every count fixture below.
fn assert_one_error_per_at_sign() {
    assert_eq!(
        marrow_syntax::parse_source("@\n")
            .diagnostics
            .summary()
            .count(),
        1
    );
    assert_eq!(
        marrow_syntax::parse_source("@\n@\n")
            .diagnostics
            .summary()
            .count(),
        2
    );
}

fn at_sign_lines(count: usize) -> Vec<u8> {
    "@\n".repeat(count).into_bytes()
}

/// A source of `use` declarations that each fail import resolution with a
/// deterministic `no module ...` diagnostic whose rendered message is
/// `name.len() + 28` bytes. Names embed `tag` so multi-file fixtures stay
/// distinct.
fn long_import_source(tag: usize, uses: usize, name_len: usize) -> Vec<u8> {
    let mut source = String::new();
    for index in 0..uses {
        let prefix = format!("u{tag}_{index}_");
        let name = format!("{prefix}{}", "a".repeat(name_len - prefix.len()));
        source.push_str(&format!("use {name}\n"));
    }
    source.into_bytes()
}

/// Exactly the count ceiling in one file is a complete bounded diagnostics
/// failure carrying every row: the N edge of the compiler count bound through
/// the consuming syntax bridge.
#[test]
fn exactly_the_count_ceiling_is_a_complete_diagnostics_failure() {
    assert_one_error_per_at_sign();
    let input = project(vec![(
        "src/main.mw".to_string(),
        at_sign_lines(SYNTAX_DIAGNOSTIC_COUNT_LIMIT),
    )]);
    let rows = diagnostics(&input);
    assert_eq!(rows.len(), SYNTAX_DIAGNOSTIC_COUNT_LIMIT);
    for row in &rows {
        assert_eq!(row.file().as_str(), "src/main.mw");
        assert!(row.reason().is_some(), "a syntax row retains its reason");
    }
}

/// One row past the count ceiling discards the whole collection (prefix
/// included) for the typed DiagnosticCount resource limit, whose public limit
/// value equals the syntax ceiling (A7 pin).
#[test]
fn one_past_the_count_ceiling_is_a_diagnostic_count_resource_limit() {
    assert_one_error_per_at_sign();
    let input = project(vec![(
        "src/main.mw".to_string(),
        at_sign_lines(SYNTAX_DIAGNOSTIC_COUNT_LIMIT + 1),
    )]);
    assert_diagnostic_resource_limit(
        compile(&input).expect_err("an over-ceiling diagnostic set must not compile"),
        ResourceLimitKind::DiagnosticCount,
        SYNTAX_DIAGNOSTIC_COUNT_LIMIT,
    );
    assert_diagnostic_resource_limit(
        compile_with_tests(&input).expect_err("test compilation shares the ceiling"),
        ResourceLimitKind::DiagnosticCount,
        SYNTAX_DIAGNOSTIC_COUNT_LIMIT,
    );
    let failure = analyze(Arc::new(input), InputRevision::new(9))
        .err()
        .expect("analysis shares the ceiling");
    let AnalysisFailure::ResourceLimit {
        limit: AnalysisResourceLimit::Compile(limit),
        ..
    } = failure
    else {
        panic!("expected the shared compile diagnostic limit");
    };
    assert_eq!(limit.kind(), ResourceLimitKind::DiagnosticCount);
}

/// A2: absorbing a Limited syntax terminal unconditionally leaves the compiler
/// collector Limited. A sibling clean file must not let the destroyed payload
/// disappear into a successful or partial compile — in either canonical order,
/// so a clean batch absorbed after the Limited one cannot restore a retaining
/// owner either.
#[test]
fn a_limited_syntax_file_forces_limited_beside_a_clean_sibling() {
    assert_one_error_per_at_sign();
    for (clean_module, dense_path) in [
        // The clean file is absorbed first, then the Limited one; then the
        // reverse, so a Complete batch absorbed into a Limited owner is covered.
        ("clean", "src/dense.mw"),
        ("zclean", "src/adense.mw"),
    ] {
        let clean =
            format!("module {clean_module}\n\npub fn f(): int {{\n    return 1\n}}\n").into_bytes();
        let input = project(vec![
            (format!("src/{clean_module}.mw"), clean),
            (
                dense_path.to_string(),
                at_sign_lines(SYNTAX_DIAGNOSTIC_COUNT_LIMIT + 1),
            ),
        ]);
        assert_diagnostic_resource_limit(
            compile(&input)
                .map(|_| ())
                .expect_err("a Limited syntax file must never vanish into a built image"),
            ResourceLimitKind::DiagnosticCount,
            SYNTAX_DIAGNOSTIC_COUNT_LIMIT,
        );
    }
}

/// The premise that keeps the collector's *unconditional* Limited guard
/// unobservable from production: a sealed syntax terminal always reports at
/// least one total past the ceiling it names, and the compiler ceilings equal
/// the syntax ceilings (A7), so every absorbed Limited terminal crosses a
/// compiler ceiling on the composition alone. The guard is what still holds A2
/// if this premise changes; its own red lives beside it in the collector.
#[test]
fn a_limited_syntax_terminal_always_crosses_a_compiler_ceiling_on_its_own() {
    let dense = "@\n".repeat(SYNTAX_DIAGNOSTIC_COUNT_LIMIT + 1);
    let summary = marrow_syntax::parse_source(&dense).diagnostics.summary();
    assert!(
        marrow_syntax::parse_source(&dense)
            .diagnostics
            .as_complete()
            .is_err(),
        "the fixture's syntax terminal is Limited"
    );
    assert!(
        summary.count() > SYNTAX_DIAGNOSTIC_COUNT_LIMIT
            || summary.owned_bytes() > SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT,
        "a limited summary reports a crossed total: {summary:?}"
    );
}

/// Crossing the retained-owned-byte ceiling discards the collection for the
/// typed DiagnosticBytes resource limit whose public value equals the syntax
/// byte ceiling (A7 pin); a set below the ceiling stays a complete
/// diagnostics failure with every row intact.
#[test]
fn crossing_the_byte_ceiling_is_a_diagnostic_bytes_resource_limit() {
    let over: Vec<(String, Vec<u8>)> = (0..8)
        .map(|tag| (format!("src/m{tag}.mw"), long_import_source(tag, 26, 5100)))
        .collect();
    assert_diagnostic_resource_limit(
        compile(&project(over)).expect_err("an over-byte diagnostic set must not compile"),
        ResourceLimitKind::DiagnosticBytes,
        SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT,
    );

    let under: Vec<(String, Vec<u8>)> = (0..4)
        .map(|tag| (format!("src/m{tag}.mw"), long_import_source(tag, 26, 5100)))
        .collect();
    let rows = diagnostics(&project(under));
    assert_eq!(rows.len(), 4 * 26, "every under-ceiling row is retained");
}

/// The pinned invalid-UTF-8 contract: the existing `check.unsupported` code,
/// the central static message, a zero-length 1:1 span at file start, no help,
/// no reason, no identity gap — and invalid-file rows form the canonical-order
/// prefix ahead of valid files' syntax rows.
#[test]
fn invalid_utf8_rows_form_the_canonical_prefix_with_the_pinned_contract() {
    let input = project(vec![
        ("src/a.mw".to_string(), b"@\n".to_vec()),
        ("src/b.mw".to_string(), vec![0xFF]),
    ]);
    let rows = diagnostics(&input);
    assert_eq!(rows.len(), 2);

    let utf8 = &rows[0];
    assert_eq!(utf8.file().as_str(), "src/b.mw");
    assert_eq!(utf8.code(), "check.unsupported");
    assert_eq!(utf8.message(), "source file is not valid UTF-8");
    let span = utf8.span();
    assert_eq!(
        (span.start_byte, span.end_byte, span.line, span.column),
        (0, 0, 1, 1)
    );
    assert_eq!(utf8.severity(), marrow_syntax::Severity::Error);
    assert!(utf8.help().is_none());
    assert!(utf8.reason().is_none());
    assert!(utf8.identity_gap().is_none());

    let syntax = &rows[1];
    assert_eq!(syntax.file().as_str(), "src/a.mw");
    assert!(syntax.reason().is_some());
    assert_eq!(syntax.severity(), marrow_syntax::Severity::Error);
}

/// 4096 invalid files — the admission maximum — retain exactly the count
/// ceiling of typed rows as a complete bounded set: the static invalid-UTF-8
/// message charges no owned bytes, so the byte ceiling is nowhere near.
#[test]
fn the_full_admission_width_of_invalid_files_stays_a_complete_set() {
    let files: Vec<(String, Vec<u8>)> = (0..CaptureLimits::DEFAULT.max_files())
        .map(|index| (format!("src/m{index:04}.mw"), vec![0xFF]))
        .collect();
    let rows = diagnostics(&project(files));
    assert_eq!(rows.len(), SYNTAX_DIAGNOSTIC_COUNT_LIMIT);
    assert!(
        rows.iter()
            .all(|row| row.message() == "source file is not valid UTF-8")
    );
}

/// Diagnostic order is file order with per-file position order: the bridge
/// absorbs one file at a time and each file's payload arrives position-sorted.
#[test]
fn absorbed_syntax_rows_keep_file_order_and_per_file_position_order() {
    let input = project(vec![
        ("src/a.mw".to_string(), b"@\n@\n".to_vec()),
        ("src/b.mw".to_string(), b"@\n".to_vec()),
    ]);
    let rows = diagnostics(&input);
    let order: Vec<(String, u32)> = rows
        .iter()
        .map(|row| (row.file().as_str().to_string(), row.line()))
        .collect();
    assert_eq!(
        order,
        vec![
            ("src/a.mw".to_string(), 1),
            ("src/a.mw".to_string(), 2),
            ("src/b.mw".to_string(), 1),
        ]
    );
}

/// The production compile projects the first non-empty stage; the analysis
/// union alone composes stages and may cross the shared ceiling. No production
/// cross-stage strengthening exists: the same project is a complete parse-stage
/// diagnostics failure for `compile` and a DiagnosticCount refusal for
/// `analyze`.
#[test]
fn production_projects_the_parse_stage_while_the_analysis_union_crosses() {
    assert_one_error_per_at_sign();
    let parse_rows = 3900;
    let semantic_rows = 300;
    let input = project(vec![
        ("src/broken.mw".to_string(), at_sign_lines(parse_rows)),
        (
            "src/valid.mw".to_string(),
            long_import_source(7, semantic_rows, 40),
        ),
    ]);

    let rows = diagnostics(&input);
    assert_eq!(
        rows.len(),
        parse_rows,
        "production reports the parse stage only"
    );
    assert!(
        rows.iter()
            .all(|row| row.file().as_str() == "src/broken.mw")
    );

    let failure = analyze(Arc::new(input), InputRevision::new(11))
        .err()
        .expect("the cross-stage union crosses the count ceiling");
    let AnalysisFailure::ResourceLimit {
        limit: AnalysisResourceLimit::Compile(limit),
        ..
    } = failure
    else {
        panic!("expected the union's diagnostic count limit");
    };
    assert_eq!(limit.kind(), ResourceLimitKind::DiagnosticCount);
    assert_eq!(limit.limit(), SYNTAX_DIAGNOSTIC_COUNT_LIMIT as u64);
}

/// Below the ceilings, the analysis union is the ordered cross-stage set:
/// parse rows first, then the semantic rows the production compile suppresses.
#[test]
fn the_analysis_union_orders_parse_rows_before_semantic_rows() {
    assert_one_error_per_at_sign();
    let input = project(vec![
        ("src/a.mw".to_string(), b"@\n@\n".to_vec()),
        ("src/b.mw".to_string(), long_import_source(3, 1, 40)),
    ]);

    let rows = diagnostics(&input);
    assert_eq!(rows.len(), 2, "production projects the parse stage");

    let Ok(snapshot) = analyze(Arc::new(input), InputRevision::new(12)) else {
        panic!("the union stays bounded");
    };
    let files: Vec<&str> = snapshot
        .diagnostics()
        .iter()
        .map(|row| row.file().as_str())
        .collect();
    assert_eq!(files, vec!["src/a.mw", "src/a.mw", "src/b.mw"]);
}
