//! Production-path compiler-input admission and diagnostic-retention laws.

use std::sync::Arc;

use marrow_compile::{
    AnalysisFailure, AnalysisResourceLimit, CompileFailure, InputRevision, MAX_PARSED_FILE_BYTES,
    ResourceLimitKind, analyze, compile, compile_with_tests,
};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

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
    assert_exact_boundary_is_admitted(project_with_file_count(CaptureLimits::DEFAULT.max_files()));
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
