//! Deterministic production-owner accounting for transparent-alias cycle
//! classification. The counters are private test observers; elapsed time is not
//! part of the contract.

use std::fmt::Write as _;

use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

use super::capture_alias_cycle_counts;
use crate::compile::compile;

const ALIAS_COUNT: usize = 256;

fn project(source: String) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.into_bytes(),
    )];
    marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

fn chain_source(signature_type: &str) -> String {
    let mut source = String::new();
    for alias in 0..ALIAS_COUNT - 1 {
        writeln!(source, "alias A{alias:03} = A{:03}", alias + 1).expect("write alias");
    }
    writeln!(source, "alias A255 = int").expect("write terminal alias");
    writeln!(
        source,
        "\npub fn identity(value: {signature_type}): {signature_type} {{\n    return value\n}}"
    )
    .expect("write function");
    source
}

#[test]
fn alias_cycle_classification_is_linear() {
    let (compiled, counts) = capture_alias_cycle_counts(|| compile(&project(chain_source("A000"))));
    let compiled = compiled.expect("acyclic alias chain compiles");

    let direct = compile(&project(chain_source("int"))).expect("direct int control compiles");
    assert_eq!(
        compiled.image.bytes, direct.image.bytes,
        "every accepted alias in the chain expands to the same terminal int shape"
    );

    assert_eq!(
        (
            counts.target_visits,
            counts.resolved_edges,
            counts.cyclic_aliases,
        ),
        (ALIAS_COUNT, ALIAS_COUNT - 1, 0),
        "cycle classification must visit each target once and resolve each chain edge once"
    );
    assert!(
        counts.node_entries <= 2 * ALIAS_COUNT,
        "each node enters at most once in each iterative SCC pass: {counts:?}"
    );
    assert!(
        counts.edge_inspections <= 2 * (ALIAS_COUNT - 1),
        "each edge is inspected at most once in each iterative SCC pass: {counts:?}"
    );
}

#[test]
fn duplicate_alias_references_remain_distinct_graph_edges() {
    let source = "alias Loop = Pair<Loop, Loop>?\n\npub fn f(): int {\n    return 1\n}\n";
    let (result, counts) = capture_alias_cycle_counts(|| compile(&project(source.to_string())));

    assert!(result.is_err(), "the self-referential alias must reject");
    assert_eq!(
        (
            counts.target_visits,
            counts.resolved_edges,
            counts.node_entries,
            counts.edge_inspections,
            counts.cyclic_aliases,
        ),
        (4, 2, 2, 4, 1),
        "each resolved occurrence is retained while SCC membership remains singular"
    );
}
