//! Exact relation-work evidence for the production call-graph analyses.

use std::fmt::Write as _;

use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};
use marrow_syntax::SourceSpan;

use crate::compile::{CompileFailure, ResourceLimitKind, check, compile, compile_with_tests};
use crate::types::{CallGraphCounts, capture_call_graph_counts};
use crate::{test_ledger as ledger, test_project as project_capture};

fn project(source: String) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.into_bytes(),
    )];
    marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

/// A chain of `depth` functions, each calling the next, ending in a leaf. The
/// public driver adds the final edge, so the graph has exactly `depth` edges.
fn chain_source(depth: usize) -> String {
    let mut source = String::from("module main\n\n");
    for step in 0..depth {
        writeln!(source, "fn step{step:04}(n: int): int {{").expect("write");
        if step + 1 == depth {
            writeln!(source, "    return n").expect("write");
        } else {
            writeln!(source, "    return step{:04}(n)", step + 1).expect("write");
        }
        writeln!(source, "}}\n").expect("write");
    }
    writeln!(source, "pub fn driver(n: int): int {{").expect("write");
    writeln!(source, "    return step0000(n)").expect("write");
    writeln!(source, "}}").expect("write");
    source
}

fn chain_counts(depth: usize) -> crate::types::CallGraphCounts {
    let input = project(chain_source(depth));
    let (compiled, counts) = capture_call_graph_counts(|| compile(&input));
    compiled.expect("the acyclic chain compiles");
    assert_eq!(presence_counts(counts), CallGraphCounts::default());
    counts
}

#[test]
fn c2_algorithmic_work_is_linear_and_output_identical() {
    let mut observed_work = Vec::new();
    for (depth, expected_edge_work) in [(64usize, 320usize), (128, 640)] {
        let input = project(chain_source(depth));
        let ordinary = compile(&input).expect("the acyclic chain compiles");
        let (observed, counts) = capture_call_graph_counts(|| compile(&input));
        let observed = observed.expect("observation cannot change acceptance");

        assert_eq!(
            ordinary.image.bytes, observed.image.bytes,
            "the test-only observer cannot change image bytes at depth {depth}",
        );
        observed_work.push((depth, counts.total_edge_work(), expected_edge_work));
    }
    assert_eq!(
        observed_work,
        vec![(64, 320, 320), (128, 640, 640)],
        "SCC, eligibility and three semantic relations examine each edge once",
    );
}

#[test]
fn the_graph_analysis_takes_each_function_and_edge_exactly_once() {
    let observed: Vec<_> = [64usize, 128]
        .into_iter()
        .map(|depth| {
            let counts = chain_counts(depth);
            (depth, counts.graph_vertex_visits, counts.graph_edge_visits)
        })
        .collect();
    assert_eq!(observed, vec![(64, 65, 64), (128, 129, 128)]);
}

#[test]
fn each_propagated_relation_takes_each_function_and_edge_exactly_once() {
    let observed: Vec<_> = [64usize, 128]
        .into_iter()
        .map(|depth| {
            let counts = chain_counts(depth);
            (
                depth,
                counts.propagation_visits,
                counts.propagation_edge_visits,
            )
        })
        .collect();
    assert_eq!(observed, vec![(64, 195, 192), (128, 387, 384)]);
}

fn presence_counts(counts: CallGraphCounts) -> CallGraphCounts {
    CallGraphCounts {
        graph_vertex_visits: 0,
        graph_edge_visits: 0,
        closure_vertex_visits: 0,
        closure_edge_visits: 0,
        graph_scratch_bytes: 0,
        propagation_visits: 0,
        propagation_edge_visits: 0,
        ..counts
    }
}

fn durable_project(source: &str, roots: &[String]) -> ProjectInput {
    let mut anchors = vec![
        "application .".to_string(),
        "product R".to_string(),
        "field R.value".to_string(),
    ];
    for root in roots {
        anchors.push(format!("root {root}"));
        anchors.push(format!("key {root}.id"));
    }
    let borrowed: Vec<&str> = anchors.iter().map(String::as_str).collect();
    let ids = ledger::ledger(&borrowed);
    project_capture::project_with_ids(&[("src/main.mw", source)], Some(&ids))
}

fn diagnostic_rows(
    result: Result<impl std::fmt::Debug, CompileFailure>,
) -> Vec<(String, String, SourceSpan)> {
    let Err(CompileFailure::Diagnostics(rows)) = result else {
        panic!("expected source diagnostics, got {result:?}");
    };
    rows.iter()
        .map(|row| {
            (
                row.code().to_string(),
                row.file().as_str().to_string(),
                row.span(),
            )
        })
        .collect()
}

#[test]
fn a_duplicate_test_hole_does_not_prevent_generic_presence_analysis() {
    let source = r#"module main
resource R {
    required value: int
}
store ^r[id: int]: R
pub fn write(id: int) {
    transaction {
        place p = ^r[id]
        if exists(p) {
            const queued = identity(1)
            p.value = queued
        }
    }
}
fn erase(id: int) { delete ^r[id] }
fn identity<T>(x: T): T { return x }
test "same" {}
test "same" {}
"#;
    let input = durable_project(source, &["r".to_string()]);
    let ordinary = diagnostic_rows(compile_with_tests(&input));
    let (observed, counts) = capture_call_graph_counts(|| compile_with_tests(&input));
    let observed = diagnostic_rows(observed);
    assert_eq!(observed, ordinary);
    assert_eq!(observed.len(), 1);
    let (code, file, span) = &observed[0];
    assert_eq!(code, "check.name_conflict");
    assert_eq!(file, "src/main.mw");
    assert_eq!((span.line, span.column), (18, 6));
    assert_eq!(diagnostic_rows(check(&input)), ordinary);
    compile(&input).expect("excluding the duplicate tests permits the generic drain");

    // The duplicate test's slot stays vacant; the generic body still fills slot 4.
    // Presence summaries retain all five IDs and visit the four available bodies.

    assert_eq!(
        (counts.graph_vertex_visits, counts.graph_edge_visits),
        (5, 1)
    );
    assert_eq!(
        presence_counts(counts),
        CallGraphCounts {
            presence_stripes: 1,
            presence_row_visits: 4,
            presence_edge_visits: 1,
            presence_query_positions: 1,
            presence_summary_lookups: 1,
            presence_queries_queued: 1,
            presence_summary_words: 5,
            presence_query_rows: 1,
            presence_erase_rows: 1,
            presence_next_slots: 1,
            presence_erased_families: 1,
            presence_obligations: 1,
            presence_families: 1,
            ..CallGraphCounts::default()
        },
    );
}

fn resource_source(roots: &[String]) -> String {
    let mut source = String::from("module main\nresource R { required value: int }\n");
    for root in roots {
        writeln!(source, "store ^{root}[id: int]: R").expect("write");
    }
    source
}

fn push_guarded_writes(source: &mut String, roots: &[String]) {
    for (index, root) in roots.iter().enumerate() {
        writeln!(source, "        place p{index} = ^{root}[id]").expect("write");
        writeln!(
            source,
            "        if exists(p{index}) {{ p{index}.value = noop(id) }}"
        )
        .expect("write");
    }
}

#[test]
fn presence_stripes_bound_work_and_preserve_image_bytes() {
    for (families, stripes, positions) in [(1usize, 1, 1), (64, 1, 64), (65, 2, 129)] {
        let roots: Vec<String> = (0..families).map(|index| format!("r{index}")).collect();
        let mut source = resource_source(&roots);
        source.push_str("fn noop(x: int): int { return x }\nfn erase_all(id: int) {\n");
        for root in &roots {
            writeln!(source, "    delete ^{root}[id]").expect("write");
        }
        source.push_str("}\npub fn write(id: int) {\n    transaction {\n");
        push_guarded_writes(&mut source, &roots);
        source.push_str("    }\n}\n");
        let input = durable_project(&source, &roots);
        let (compiled, counts) = capture_call_graph_counts(|| compile(&input));
        let compiled = compiled.expect("the RHS calls do not erase any guarded entry");
        if families == 65 {
            let ordinary = compile(&input).expect("observation cannot change acceptance");
            assert_eq!(ordinary.image.bytes, compiled.image.bytes);
        }
        assert_eq!(
            (counts.graph_vertex_visits, counts.graph_edge_visits),
            (3, families),
        );
        assert_eq!(
            presence_counts(counts),
            CallGraphCounts {
                presence_stripes: stripes,
                presence_row_visits: 3 * stripes,
                presence_edge_visits: families * stripes,
                presence_query_positions: positions,
                presence_summary_lookups: families,
                presence_queries_queued: families,
                presence_summary_words: 3,
                presence_query_rows: families,
                presence_erase_rows: families,
                presence_next_slots: families.min(64),
                presence_erased_families: families,
                presence_obligations: families,
                presence_families: families,
                ..CallGraphCounts::default()
            },
            "presence work for {families} families",
        );
    }
}

/// Narrow bodies reach presence checking beyond the final image function limit.
/// Two modules keep their declaration outlines below the separate per-file bound.
#[test]
fn presence_summaries_cover_functions_beyond_final_image_policy() {
    let functions = marrow_image::bounds::MAX_FUNCTIONS + 1;
    assert_eq!(functions, 4_097);
    let mut main = String::from(
        "module main\n\
         resource R { required value: int }\n\
         store ^r[id: int]: R\n\
         fn noop(x: int): int { return x }\n\
         fn erase(id: int) { delete ^r[id] }\n\
         pub fn write(id: int) {\n\
             transaction {\n\
                 place p = ^r[id]\n\
                 if exists(p) { p.value = noop(id) }\n\
             }\n\
         }\n",
    );
    let split = functions.div_ceil(2);
    for index in 3..split {
        writeln!(main, "fn pad{index}() {{}}").expect("write");
    }
    let mut extra = String::from("module extra\n");
    for index in split..functions {
        writeln!(extra, "fn pad{index}() {{}}").expect("write");
    }
    let ids = ledger::ledger(&[
        "application .",
        "product R",
        "field R.value",
        "root r",
        "key r.id",
    ]);
    let input = project_capture::project_with_ids(
        &[("src/main.mw", &main), ("src/extra.mw", &extra)],
        Some(&ids),
    );
    let (result, counts) = capture_call_graph_counts(|| compile(&input));
    let Err(CompileFailure::ResourceLimit(limit)) = result else {
        panic!("expected the final function limit, got {result:?}");
    };
    assert_eq!(limit.kind(), ResourceLimitKind::Functions);
    assert_eq!(limit.limit(), marrow_image::bounds::MAX_FUNCTIONS as u64);
    assert_eq!(
        CallGraphCounts {
            graph_scratch_bytes: 0,
            ..counts
        },
        CallGraphCounts {
            graph_vertex_visits: functions,
            graph_edge_visits: 1,
            closure_vertex_visits: functions,
            closure_edge_visits: 1,
            propagation_visits: 3 * functions,
            // The call is already inside a transaction, so ambient-transaction
            // propagation has no edge; mutation and durable closures each visit it.
            propagation_edge_visits: 2,
            presence_stripes: 1,
            presence_row_visits: functions,
            presence_edge_visits: 1,
            presence_query_positions: 1,
            presence_summary_lookups: 1,
            presence_queries_queued: 1,
            presence_summary_words: functions,
            presence_query_rows: 1,
            presence_erase_rows: 1,
            presence_next_slots: 1,
            presence_erased_families: 1,
            presence_obligations: 1,
            presence_families: 1,
            ..CallGraphCounts::default()
        },
    );
}

fn dense_presence_source(repetitions: usize, protected_root: &str) -> (ProjectInput, u32) {
    let mut roots: Vec<String> = (0..63).map(|index| format!("r{index}")).collect();
    roots.push("keep".to_string());
    let mut source = resource_source(&roots);
    source.push_str("fn noop(x: int): int { return x }\nfn erase_many(id: int) {\n");
    for root in &roots[..63] {
        writeln!(source, "    delete ^{root}[id]").expect("write");
    }
    source.push_str(
        "}\nfn erase_keep(id: int) { delete ^keep[id] }\n\
         pub fn probe(id: int) {\n    transaction {\n",
    );
    push_guarded_writes(&mut source, &roots);
    source.push_str("    }\n}\npub fn repeat(id: int) {\n    transaction {\n");
    writeln!(source, "        place p = ^{protected_root}[id]").expect("write");
    source.push_str("        if exists(p) {\n");
    for _ in 0..repetitions {
        source.push_str("            erase_many(id)\n");
    }
    let write_line = source.lines().count() as u32 + 1;
    source.push_str("            p.value = 1\n        }\n    }\n}\n");
    (durable_project(&source, &roots), write_line)
}

#[test]
fn dense_erasers_do_no_work_per_family_for_an_independent_pending_query() {
    for repetitions in [8usize, 16] {
        let (input, _) = dense_presence_source(repetitions, "keep");
        let (compiled, counts) = capture_call_graph_counts(|| compile(&input));
        compiled.expect("the dense eraser leaves the pending family present");
        let calls = 64 + repetitions;
        assert_eq!(
            (counts.graph_vertex_visits, counts.graph_edge_visits),
            (5, calls)
        );
        assert_eq!(
            presence_counts(counts),
            CallGraphCounts {
                presence_stripes: 1,
                presence_row_visits: 5,
                presence_edge_visits: calls,
                presence_query_positions: calls,
                presence_summary_lookups: calls,
                presence_queries_queued: 65,
                presence_summary_words: 5,
                presence_query_rows: 65,
                presence_erase_rows: 64,
                presence_next_slots: 64,
                presence_erased_families: 64,
                presence_obligations: 65,
                presence_families: 64,
                ..CallGraphCounts::default()
            },
            "presence work for {repetitions} repeated dense erasers",
        );
    }
}

#[test]
fn the_first_matching_eraser_drains_the_query_once() {
    let (input, write_line) = dense_presence_source(16, "r0");
    let (compiled, counts) = capture_call_graph_counts(|| compile(&input));
    let rows = diagnostic_rows(compiled);
    assert_eq!(rows.len(), 1);
    let (code, file, span) = &rows[0];
    assert_eq!(code, "check.requires_presence");
    assert_eq!(file, "src/main.mw");
    assert_eq!((span.line, span.column), (write_line, 13));
    assert_eq!(
        (counts.graph_vertex_visits, counts.graph_edge_visits),
        (5, 80)
    );
    assert_eq!(
        presence_counts(counts),
        CallGraphCounts {
            presence_stripes: 1,
            presence_row_visits: 5,
            presence_edge_visits: 80,
            presence_query_positions: 80,
            presence_summary_lookups: 65,
            presence_queries_queued: 65,
            presence_queries_drained: 1,
            presence_summary_words: 5,
            presence_query_rows: 65,
            presence_erase_rows: 64,
            presence_next_slots: 64,
            presence_failure_rows: 1,
            presence_erased_families: 64,
            presence_obligations: 65,
            presence_families: 64,
            ..CallGraphCounts::default()
        },
    );
}
