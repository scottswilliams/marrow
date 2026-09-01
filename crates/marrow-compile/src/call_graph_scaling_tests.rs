//! Exact relation-work evidence for the production call-graph analyses.

use std::fmt::Write as _;

use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

use crate::compile::compile;
use crate::types::capture_call_graph_counts;

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

#[test]
fn c2_algorithmic_work_is_linear_and_output_identical() {
    let mut observed_work = Vec::new();
    for (depth, expected_edge_work) in [(64usize, 256usize), (128, 512)] {
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
        vec![(64, 256, 256), (128, 512, 512)],
        "four semantic relations examine every edge exactly once",
    );
}
