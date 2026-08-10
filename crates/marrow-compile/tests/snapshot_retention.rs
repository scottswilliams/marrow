//! The retained analysis-fact bounds, observed through the production `analyze`
//! entry point.
//!
//! Every law here is about what one [`marrow_compile::AnalysisSnapshot`] retains:
//! a project that crosses a snapshot fact ceiling is refused transactionally as a
//! typed resource limit, never admitted as a truncated or partial fact set, and
//! the refusal echoes the caller's revision unchanged.

use std::sync::Arc;

use marrow_compile::{
    AnalysisFailure, AnalysisResourceLimit, InputRevision, MAX_DOCUMENT_SYMBOLS_PER_FILE,
    MAX_SNAPSHOT_FACT_BYTES, MAX_SNAPSHOT_FACT_COUNT, MAX_SYMBOL_DEPTH, analyze,
};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

/// Capture a project at limits wide enough for the fixture, exactly as the pure
/// project owner admits one. The compiler's own drive admission still applies the
/// production envelope independently.
fn captured(files: Vec<(String, String)>) -> Arc<ProjectInput> {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let max_files = files.len().max(1);
    let max_file_bytes = files
        .iter()
        .map(|(_, source)| source.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let total = files
        .iter()
        .map(|(_, source)| source.len())
        .sum::<usize>()
        .max(1);
    let captured = files
        .into_iter()
        .map(|(path, source)| CapturedFile::new(path, source.into_bytes()))
        .collect();
    Arc::new(
        marrow_project::capture(
            &manifest,
            captured,
            None,
            &CaptureLimits::new(max_files, max_file_bytes, total),
        )
        .expect("the pure capture API accepts its explicit wider limit"),
    )
}

fn module_file(index: usize, source: String) -> (String, String) {
    (format!("src/module_{index}.mw"), source)
}

/// One module whose enum contributes `members` document-symbol nodes (the enum plus
/// each member), with each member name padded to `name_bytes`.
/// A module of `members` enum members, each name padded to `name_bytes`. Sized by its
/// callers to stay inside the admitted file length: a module over it is refused by drive
/// admission and never reaches the fact ceilings these fixtures are aimed at.
fn symbol_module(index: usize, members: usize, name_bytes: usize) -> (String, String) {
    let mut source = format!("module module_{index}\n\nenum E {{\n");
    for member in 0..members {
        let name = format!("m{member}");
        let padding = name_bytes.saturating_sub(name.len());
        source.push_str("    ");
        source.push_str(&name);
        for _ in 0..padding {
            source.push('x');
        }
        source.push('\n');
    }
    source.push_str("}\n");
    assert!(
        source.len() <= marrow_compile::MAX_PARSED_FILE_BYTES,
        "a fixture module has to be a file the drive admits: {} bytes against an admitted \
         {}",
        source.len(),
        marrow_compile::MAX_PARSED_FILE_BYTES
    );
    module_file(index, source)
}

fn analyze_files(files: Vec<(String, String)>, revision: u64) -> Result<(), AnalysisFailure> {
    analyze(captured(files), InputRevision::new(revision)).map(|_| ())
}

fn expect_snapshot(files: Vec<(String, String)>, revision: u64) {
    match analyze_files(files, revision) {
        Ok(()) => {}
        Err(failure) => panic!(
            "expected a snapshot, got a failure at revision {}",
            failure.revision().get()
        ),
    }
}

fn expect_limit(files: Vec<(String, String)>, revision: u64) -> AnalysisResourceLimit {
    match analyze_files(files, revision) {
        Err(AnalysisFailure::ResourceLimit {
            revision: echoed,
            limit,
        }) => {
            assert_eq!(
                echoed.get(),
                revision,
                "a refusal echoes the caller's revision unchanged"
            );
            limit
        }
        Err(AnalysisFailure::Invariant { .. }) => {
            panic!("expected a resource limit, got Invariant")
        }
        Ok(()) => panic!("expected a resource limit, got a snapshot"),
    }
}

/// Crossing the global retained fact count refuses the whole snapshot with the typed
/// count ceiling. Document-symbol nodes charge the same global count as hover facts,
/// so a wide declaration hierarchy spread over several files reaches it.
#[test]
fn crossing_the_fact_count_refuses_the_whole_snapshot() {
    let per_file = 4_000usize;
    let files_needed = (MAX_SNAPSHOT_FACT_COUNT as usize / per_file) + 2;
    let files = (0..files_needed)
        .map(|index| symbol_module(index, per_file, 1))
        .collect();
    match expect_limit(files, 11) {
        AnalysisResourceLimit::SnapshotFactCount { limit } => {
            assert_eq!(limit, MAX_SNAPSHOT_FACT_COUNT);
        }
        other => panic!("expected SnapshotFactCount, got {}", other.description()),
    }
}

/// Exactly at the count ceiling the snapshot is admitted: the bound refuses only a
/// crossing, never the last admissible fact.
#[test]
fn the_count_ceiling_itself_is_admitted() {
    // Each module contributes its enum plus `members` member nodes.
    let per_file = 4_001usize;
    let full_files = MAX_SNAPSHOT_FACT_COUNT as usize / per_file;
    let remainder = MAX_SNAPSHOT_FACT_COUNT as usize % per_file;
    let mut files: Vec<(String, String)> = (0..full_files)
        .map(|index| symbol_module(index, per_file - 1, 1))
        .collect();
    if remainder > 0 {
        files.push(symbol_module(full_files, remainder - 1, 1));
    }
    expect_snapshot(files, 12);
}

/// Crossing the retained fact byte ceiling without crossing the count ceiling refuses
/// with the typed byte limit. Each retained symbol name spelling charges its bytes.
#[test]
fn crossing_the_fact_bytes_refuses_with_the_byte_limit() {
    let name_bytes = 1_200usize;
    let per_file = 600usize;
    let bytes_per_file = (per_file * name_bytes) as u64;
    let files_needed = (MAX_SNAPSHOT_FACT_BYTES / bytes_per_file) as usize + 2;
    let files = (0..files_needed)
        .map(|index| symbol_module(index, per_file, name_bytes))
        .collect();
    assert!(
        (files_needed * (per_file + 1)) < MAX_SNAPSHOT_FACT_COUNT as usize,
        "the fixture crosses bytes without crossing the count ceiling"
    );
    match expect_limit(files, 13) {
        AnalysisResourceLimit::SnapshotFactBytes { limit } => {
            assert_eq!(limit, MAX_SNAPSHOT_FACT_BYTES);
        }
        other => panic!("expected SnapshotFactBytes, got {}", other.description()),
    }
}

/// Count wins a simultaneous crossing: a project that exceeds both ceilings reports
/// the count limit, never the byte limit.
#[test]
fn count_wins_a_simultaneous_crossing() {
    // Members per file sized to keep each module inside the admitted file length: the
    // fixture has to reach the fact ceilings, so it must not be refused before them.
    let per_file = 3_500usize;
    let files_needed = (MAX_SNAPSHOT_FACT_COUNT as usize / per_file) + 2;
    let files = (0..files_needed)
        .map(|index| symbol_module(index, per_file, 200))
        .collect();
    match expect_limit(files, 14) {
        AnalysisResourceLimit::SnapshotFactCount { limit } => {
            assert_eq!(limit, MAX_SNAPSHOT_FACT_COUNT);
        }
        other => panic!(
            "expected SnapshotFactCount to win, got {}",
            other.description()
        ),
    }
}

/// The per-file declaration-hierarchy bounds keep precedence over the global fact
/// bounds: a file over `MAX_DOCUMENT_SYMBOLS_PER_FILE` refuses with the per-file
/// count limit even when the project would also cross the global ceilings.
#[test]
fn the_per_file_symbol_bound_keeps_precedence() {
    let files = vec![symbol_module(
        0,
        MAX_DOCUMENT_SYMBOLS_PER_FILE as usize + 4,
        100,
    )];
    match expect_limit(files, 15) {
        AnalysisResourceLimit::DocumentSymbolCount { limit } => {
            assert_eq!(limit, MAX_DOCUMENT_SYMBOLS_PER_FILE);
        }
        other => panic!("expected DocumentSymbolCount, got {}", other.description()),
    }
}

/// A nesting overflow still transactionally refuses the whole snapshot; no partial
/// outline survives into a snapshot.
#[test]
fn the_symbol_depth_bound_refuses_transactionally() {
    let levels = MAX_SYMBOL_DEPTH as usize + 4;
    let mut source = String::from("module module_0\n\nenum Deep {\n");
    for level in 0..levels {
        source.push_str(&"    ".repeat(level + 1));
        source.push_str(&format!("category c{level} {{\n"));
    }
    source.push_str(&"    ".repeat(levels + 1));
    source.push_str("leaf\n");
    for level in (0..levels).rev() {
        source.push_str(&"    ".repeat(level + 1));
        source.push_str("}\n");
    }
    source.push_str("}\n");
    match expect_limit(vec![module_file(0, source)], 16) {
        AnalysisResourceLimit::DocumentSymbolDepth { limit } => {
            assert_eq!(limit, MAX_SYMBOL_DEPTH);
        }
        other => panic!("expected DocumentSymbolDepth, got {}", other.description()),
    }
}
