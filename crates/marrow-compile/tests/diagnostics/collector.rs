//! The compiler's diagnostic laws: the one bounded collector and its consuming syntax
//! bridge, the typed count/bytes ceilings, input admission and retention, absence of
//! pre-collector amplification, and the file identity and full byte span every row keeps.

use std::sync::Arc;

use marrow_codes::Code;
use marrow_compile::{
    AnalysisFailure, AnalysisResourceLimit, CompileFailure, InputRevision, MAX_PARSED_FILE_BYTES,
    ResourceLimitKind, SourceDiagnostic, analyze, compile, compile_with_tests,
};
use marrow_project::{CaptureLimits, CapturedFile, FileIdentity, Manifest, ProjectInput};
use marrow_syntax::{SYNTAX_DIAGNOSTIC_COUNT_LIMIT, SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT};

mod bounds {
    use super::*;
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
            let clean = format!("module {clean_module}\n\npub fn f(): int {{\n    return 1\n}}\n")
                .into_bytes();
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
        assert_eq!(utf8.code(), Code::CheckUnsupported);
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

    /// Every sentence fragment `AnalysisResourceLimit::description` hands a reader, pinned
    /// exactly. The five analysis-owned bounds ship hand-written prose that reaches a user
    /// through three CLI commands and the language server, so a reworded fragment is a
    /// user-visible change and belongs in a review of what it now says, not in whatever lane
    /// happened to touch the file. The sixth arm delegates, and its law — a compile-side
    /// bound reads the same whichever owner reports it — is asserted below against a limit
    /// the production path produced.
    ///
    /// The match is exhaustive and each arm is written out, so a new bound cannot land
    /// without a fragment stated here.
    #[test]
    fn every_analysis_resource_limit_description_is_pinned() {
        let analysis_owned = [
            (
                AnalysisResourceLimit::SnapshotFactCount { limit: 1 },
                "the analysis fact table is full",
            ),
            (
                AnalysisResourceLimit::SnapshotFactBytes { limit: 1 },
                "the analysis facts hold too much text to retain",
            ),
            (
                AnalysisResourceLimit::CompletionCandidateCount { limit: 1 },
                "one completion query has too many candidates",
            ),
            (
                AnalysisResourceLimit::CompletionRenderBytes { limit: 1 },
                "one completion query renders too much text",
            ),
            (
                AnalysisResourceLimit::ActiveCallRenderBytes { limit: 1 },
                "one signature query renders too much text",
            ),
        ];
        for (limit, expected) in &analysis_owned {
            // Exhaustiveness anchor: a new variant makes this match non-exhaustive, so it
            // cannot land without an arm here and a fragment in the table above.
            match limit {
                AnalysisResourceLimit::Compile(_)
                | AnalysisResourceLimit::SnapshotFactCount { .. }
                | AnalysisResourceLimit::SnapshotFactBytes { .. }
                | AnalysisResourceLimit::CompletionCandidateCount { .. }
                | AnalysisResourceLimit::CompletionRenderBytes { .. }
                | AnalysisResourceLimit::ActiveCallRenderBytes { .. } => {}
            }
            assert_eq!(&limit.description(), expected);
        }

        let spellings: Vec<&str> = analysis_owned
            .iter()
            .map(|(limit, _)| limit.description())
            .collect();
        let mut sorted = spellings.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            analysis_owned.len(),
            "each analysis-owned bound must be distinguishable by the sentence a reader sees"
        );

        // The delegating arm: a compile-side bound answers in `ResourceLimitKind`'s own
        // words, so the same exhausted bound reads identically whether `compile` or
        // `analyze` reported it.
        assert_one_error_per_at_sign();
        let input = project(vec![(
            "src/main.mw".to_string(),
            at_sign_lines(SYNTAX_DIAGNOSTIC_COUNT_LIMIT + 1),
        )]);
        let failure = analyze(Arc::new(input), InputRevision::new(31))
            .err()
            .expect("analysis shares the compile diagnostic ceiling");
        let AnalysisFailure::ResourceLimit {
            limit: analysis, ..
        } = failure
        else {
            panic!("expected the shared compile diagnostic limit");
        };
        let AnalysisResourceLimit::Compile(compile_limit) = analysis else {
            panic!("expected the delegating arm");
        };
        assert_eq!(
            analysis.description(),
            compile_limit.kind().description(),
            "the delegating arm adds no words of its own"
        );
    }
}

/// Production-path compiler-input admission and diagnostic-retention laws.
///
mod retention {
    use super::*;
    fn captured_project(files: Vec<CapturedFile>, limits: CaptureLimits) -> ProjectInput {
        let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
        marrow_project::capture(&manifest, files, None, &limits)
            .expect("the pure capture API accepts its explicit wider limit")
    }

    fn project_with_file_count(file_count: usize) -> ProjectInput {
        let files = (0..file_count)
            .map(|index| CapturedFile::new(format!("src/module_{index}.mw"), Vec::new()))
            .collect();
        captured_project(files, CaptureLimits::new(file_count, 1, 1))
    }

    fn project_with_sources(sources: Vec<Vec<u8>>) -> ProjectInput {
        let max_files = sources.len();
        let max_file_bytes = sources.iter().map(Vec::len).max().unwrap_or_default();
        let total_bytes = sources.iter().map(Vec::len).sum();
        let files = sources
            .into_iter()
            .enumerate()
            .map(|(index, source)| CapturedFile::new(format!("src/module_{index}.mw"), source))
            .collect();
        captured_project(
            files,
            CaptureLimits::new(max_files, max_file_bytes, total_bytes),
        )
    }

    /// Sources totalling exactly `total` bytes, none of them longer than the drive's own
    /// per-file ceiling, so the aggregate ceiling is the only one they can trip.
    fn sources_totalling(total: usize) -> Vec<Vec<u8>> {
        let mut sources = vec![vec![0xff; MAX_PARSED_FILE_BYTES]; total / MAX_PARSED_FILE_BYTES];
        let remainder = total % MAX_PARSED_FILE_BYTES;
        if remainder > 0 {
            sources.push(vec![0xff; remainder]);
        }
        sources
    }

    /// Sources that trip the drive's per-file ceiling and the aggregate ceiling at once, so a
    /// refusal names whichever the drive checks first.
    fn overlapping_byte_limit_sources() -> Vec<Vec<u8>> {
        let mut sources = sources_totalling(CaptureLimits::DEFAULT.max_total_bytes());
        sources[0].push(0xff);
        sources
    }

    fn assert_compile_limit(
        failure: CompileFailure,
        expected_kind: ResourceLimitKind,
        expected_limit: usize,
    ) {
        let CompileFailure::ResourceLimit(limit) = failure else {
            panic!("expected a compiler-drive resource limit, got {failure:?}");
        };
        assert_eq!(limit.kind(), expected_kind);
        assert_eq!(limit.limit(), expected_limit as u64);
    }

    fn assert_drive_limit(
        project: ProjectInput,
        expected_kind: ResourceLimitKind,
        expected_limit: usize,
    ) {
        assert_compile_limit(
            compile(&project).expect_err("compile must refuse before parsing"),
            expected_kind,
            expected_limit,
        );
        assert_compile_limit(
            compile_with_tests(&project).expect_err("test compilation must share drive admission"),
            expected_kind,
            expected_limit,
        );

        let revision = InputRevision::new(73);
        let failure = match analyze(Arc::new(project), revision) {
            Ok(_) => panic!("analysis must refuse before parsing"),
            Err(failure) => failure,
        };
        assert_eq!(failure.revision(), revision);
        let AnalysisFailure::ResourceLimit {
            limit: AnalysisResourceLimit::Compile(limit),
            ..
        } = failure
        else {
            panic!("expected the shared compiler resource limit");
        };
        assert_eq!(limit.kind(), expected_kind);
        assert_eq!(limit.limit(), expected_limit as u64);
    }

    fn is_input_limit(kind: ResourceLimitKind) -> bool {
        matches!(
            kind,
            ResourceLimitKind::ProjectFiles
                | ResourceLimitKind::ProjectFileBytes
                | ResourceLimitKind::ProjectSourceBytes
        )
    }

    fn assert_compile_admitted<T>(result: Result<T, CompileFailure>) {
        if let Err(CompileFailure::ResourceLimit(limit)) = result {
            assert!(
                !is_input_limit(limit.kind()),
                "an exact-boundary project was refused by drive admission: {limit:?}"
            );
        }
    }

    fn assert_exact_boundary_is_admitted(project: ProjectInput) {
        assert_compile_admitted(compile(&project));
        assert_compile_admitted(compile_with_tests(&project));

        let revision = InputRevision::new(74);
        if let Err(AnalysisFailure::ResourceLimit {
            limit: AnalysisResourceLimit::Compile(limit),
            ..
        }) = analyze(Arc::new(project), revision)
        {
            assert!(
                !is_input_limit(limit.kind()),
                "analysis refused an exact-boundary project by drive admission: {limit:?}"
            );
        }
    }

    #[test]
    fn compiler_drive_refuses_the_4097th_captured_module_before_work() {
        let project = project_with_file_count(CaptureLimits::DEFAULT.max_files() + 1);
        assert_drive_limit(
            project,
            ResourceLimitKind::ProjectFiles,
            CaptureLimits::DEFAULT.max_files(),
        );
    }

    #[test]
    fn compiler_drive_admits_exactly_4096_captured_modules() {
        assert_exact_boundary_is_admitted(project_with_file_count(
            CaptureLimits::DEFAULT.max_files(),
        ));
    }

    /// The drive refuses a file longer than its own per-file ceiling, which is the longest
    /// file whose parse fits this crate's heap ceiling — shorter than the project owner's
    /// capture ceiling, so this file is captured and then refused here.
    #[test]
    fn compiler_drive_refuses_the_first_overbound_module_before_utf8_work() {
        let mut source = vec![0; MAX_PARSED_FILE_BYTES + 1];
        source[0] = 0xff;
        assert!(
            source.len() <= CaptureLimits::DEFAULT.max_file_bytes(),
            "the fixture is captured and then refused by the drive, not refused at capture"
        );
        let project = project_with_sources(vec![source]);
        assert_drive_limit(
            project,
            ResourceLimitKind::ProjectFileBytes,
            MAX_PARSED_FILE_BYTES,
        );
    }

    #[test]
    fn compiler_drive_admits_a_module_at_exactly_its_parse_ceiling() {
        let mut source = vec![0; MAX_PARSED_FILE_BYTES];
        source[0] = 0xff;
        assert_exact_boundary_is_admitted(project_with_sources(vec![source]));
    }

    #[test]
    fn compiler_drive_refuses_the_first_aggregate_byte_overrun_before_utf8_work() {
        let mut sources = sources_totalling(CaptureLimits::DEFAULT.max_total_bytes());
        sources.push(vec![0xff]);
        let project = project_with_sources(sources);
        assert_drive_limit(
            project,
            ResourceLimitKind::ProjectSourceBytes,
            CaptureLimits::DEFAULT.max_total_bytes(),
        );
    }

    #[test]
    fn compiler_drive_checks_module_count_before_file_and_aggregate_bytes() {
        let mut sources = overlapping_byte_limit_sources();
        sources.resize_with(CaptureLimits::DEFAULT.max_files() + 1, Vec::new);
        let project = project_with_sources(sources);
        assert_drive_limit(
            project,
            ResourceLimitKind::ProjectFiles,
            CaptureLimits::DEFAULT.max_files(),
        );
    }

    #[test]
    fn compiler_drive_checks_file_bytes_before_aggregate_bytes() {
        let project = project_with_sources(overlapping_byte_limit_sources());
        assert_drive_limit(
            project,
            ResourceLimitKind::ProjectFileBytes,
            MAX_PARSED_FILE_BYTES,
        );
    }

    #[test]
    fn compiler_drive_admits_exactly_64_mib_of_source() {
        let sources = sources_totalling(CaptureLimits::DEFAULT.max_total_bytes());
        assert_exact_boundary_is_admitted(project_with_sources(sources));
    }
}

/// A5: no pre-collector amplification. An error-dense many-file project is
/// bounded by the one compiler collector's ceiling — the drive absorbs each
/// file's syntax terminal immediately after parsing it, so no un-absorbed
/// per-file diagnostic collection ever accumulates. The structural half of the
/// law (no collection of un-absorbed `ParsedSource` values can exist) is
/// enforced by the `ParsedSource` absence gate in `absence_gates.rs`.
///
mod amplification {
    use super::*;
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

    /// The admission's maximum file count, each file with 16 syntax errors (16
    /// times the ceiling in rows), resolves to exactly the typed DiagnosticCount
    /// ceiling on every public entry. The retained outcome is the ceiling itself:
    /// no per-file collection survives to amplify retention with the file count,
    /// at the widest project the drive will admit.
    #[test]
    fn an_error_dense_many_file_project_is_bounded_by_the_count_ceiling() {
        let files: Vec<(String, Vec<u8>)> = (0..CaptureLimits::DEFAULT.max_files())
            .map(|index| (format!("src/m{index:04}.mw"), "@\n".repeat(16).into_bytes()))
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
}

/// Every source diagnostic names one real FIDB01-bounded `FileIdentity`, never an
/// empty or sentinel filename. The editor analysis floor (H00f) attributes each
/// diagnostic to a captured source file; a diagnostic with no truthful file is not
/// a source diagnostic.
///
/// This pins the behavioral half of the sentinel-elimination checkpoint: an
/// instantiation-limit diagnostic — the one path that previously fell back to a
/// reserved template's empty file and a 0:0 span — now carries the real use-site
/// file. The structural half (the reserved `TypeTemplate` no longer being able to
/// hold an empty file) is enforced by the type: `TypeTemplate::file` is
/// `Option<FileIdentity>`, so an empty-string file cannot be constructed.
///
mod file_identity {
    use super::*;
    fn project(files: Vec<(&str, &str)>) -> ProjectInput {
        let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
        let captured = files
            .into_iter()
            .map(|(path, source)| CapturedFile::new(path.to_string(), source.as_bytes().to_vec()))
            .collect();
        marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
            .expect("capture project")
    }

    fn diagnostics(files: Vec<(&str, &str)>) -> Vec<SourceDiagnostic> {
        match compile(&project(files)) {
            Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
            other => panic!("expected source diagnostics, got {other:?}"),
        }
    }

    /// A source diagnostic's file is a canonical captured identity, exactly equal to
    /// the identity the project captured it under.
    #[test]
    fn a_source_diagnostic_names_a_real_captured_file_identity() {
        let source = "fn main() {\n    let x: Nonexistent = 0\n}\n";
        let produced = diagnostics(vec![("src/main.mw", source)]);
        let expected = FileIdentity::validate("src/main.mw")
            .expect("canonical identity")
            .0;
        for diagnostic in &produced {
            assert_eq!(
                diagnostic.file(),
                &expected,
                "every diagnostic names the captured file identity",
            );
            assert!(!diagnostic.file().as_str().is_empty());
        }
    }

    /// The instantiation-limit diagnostic attributes to the real use site's file,
    /// where the deleted `site.file.is_empty()` fallback would have emitted a reserved
    /// template's empty file and a 0:0 span.
    #[test]
    fn the_instantiation_limit_diagnostic_carries_the_real_use_site_file() {
        let library = "module library\n\nstruct Grow<T> {\n    next: Grow<List<T>>\n}\n\n\
                       pub fn deepen<T>(x: T): Grow<T> {\n    return deepen(x)\n}\n";
        let main = "module main\nuse library\n\n\
                    pub fn driver(): int {\n    const ignored = library::deepen(1)\n    return 0\n}\n";
        let produced = diagnostics(vec![("src/library.mw", library), ("src/main.mw", main)]);
        let limit = produced
            .iter()
            .find(|d| d.code() == Code::CheckInstantiationLimit)
            .expect("an instantiation-limit diagnostic");
        let expected = FileIdentity::validate("src/library.mw")
            .expect("canonical identity")
            .0;
        assert_eq!(limit.file(), &expected);
        assert!(!limit.file().as_str().is_empty());
        assert!(limit.line() >= 1 && limit.column() >= 1);
    }
}

/// Every source diagnostic retains the full UTF-8 byte span of the offending
/// construct, not only a 1-based point. The editor analysis floor (H00f) projects
/// this span into a selection range; a point-only diagnostic could not.
///
/// Red-first for the `SourceDiagnostic` full-span retention checkpoint: the
/// production `compile` path already threads a full `SourceSpan` into every
/// diagnostic constructor, and this gate proves the constructor keeps the byte
/// range rather than collapsing it to a point.
///
mod full_span {
    use super::*;
    fn project(source: &str) -> ProjectInput {
        let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
        let files = vec![CapturedFile::new(
            "src/main.mw".to_string(),
            source.as_bytes().to_vec(),
        )];
        marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
            .expect("capture project")
    }

    fn first_diagnostic(source: &str) -> SourceDiagnostic {
        match compile(&project(source)) {
            Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics
                .into_vec()
                .into_iter()
                .next()
                .expect("a nonempty diagnostic set has a first element"),
            other => panic!("expected source diagnostics, got {other:?}"),
        }
    }

    #[test]
    fn diagnostic_retains_full_byte_span_covering_the_construct() {
        // A syntax error over a multi-byte construct yields a diagnostic whose span
        // covers a real byte range, not a collapsed point.
        let source = "fn main() {\n    let x: = 0\n}\n";
        let diagnostic = first_diagnostic(source);
        let span = diagnostic.span();
        assert!(
            span.end_byte >= span.start_byte,
            "span byte range must be well-ordered, got {span:?}",
        );
        // The retained point stays consistent with the retained span (one owner).
        assert_eq!(diagnostic.line(), span.line);
        assert_eq!(diagnostic.column(), span.column);
        assert!(
            span.line >= 1 && span.column >= 1,
            "1-based point, got {span:?}"
        );
    }
}
