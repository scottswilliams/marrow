//! Deterministic production-owner accounting for compact alias normalization.
//! The counters are private test observers; elapsed time is not
//! part of the contract.

use std::fmt::Write as _;

use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

use super::capture_alias_cycle_counts;
use crate::compile::compile;

fn project(source: String) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.into_bytes(),
    )];
    marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

fn chain_source(signature_type: &str, count: usize) -> String {
    let mut source = String::new();
    for alias in 0..count - 1 {
        writeln!(source, "alias A{alias:03} = A{:03}", alias + 1).expect("write alias");
    }
    writeln!(source, "alias A{:03} = int", count - 1).expect("write terminal alias");
    writeln!(
        source,
        "\npub fn identity(value: {signature_type}): {signature_type} {{\n    return value\n}}"
    )
    .expect("write function");
    source
}

#[test]
fn alias_cycle_classification_is_linear() {
    for count in [8, 64, 256] {
        let (compiled, counts) =
            capture_alias_cycle_counts(|| compile(&project(chain_source("A000", count))));
        let compiled = compiled.expect("acyclic alias chain compiles");

        let direct =
            compile(&project(chain_source("int", count))).expect("direct int control compiles");
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
            (count, count - 1, 0),
            "cycle classification must visit each target once and resolve each chain edge once"
        );
        assert_eq!((counts.terminal_rows, counts.terminal_bytes), (1, 3));
        assert!(
            counts.node_entries <= count,
            "each node enters once: {counts:?}"
        );
        assert!(
            counts.edge_inspections < count,
            "each dependency is inspected once: {counts:?}"
        );
    }
}

#[test]
fn unsupported_alias_targets_do_not_traverse_application_arguments() {
    let mut source = String::from("alias A0 = int\n");
    for index in 1..=8 {
        writeln!(
            source,
            "alias A{index} = Pair<A{}, A{}>",
            index - 1,
            index - 1
        )
        .expect("write alias");
    }
    source.push_str("pub fn driver(): int { return 0 }\n");
    let (result, counts) = capture_alias_cycle_counts(|| compile(&project(source)));
    assert!(result.is_err());
    assert_eq!(
        counts.target_visits, 9,
        "only the written target heads are classified"
    );
    assert_eq!(
        counts.resolved_edges, 0,
        "unsupported applications own no alias dependency"
    );
}

#[test]
fn many_aliases_share_one_long_terminal() {
    let terminal = format!("Type{}", "x".repeat(1024));
    let mut source = format!("struct {terminal} {{ value: int }}\nalias Root = {terminal}\n");
    for index in 0..256 {
        writeln!(source, "alias A{index} = Root").expect("write alias");
    }
    source.push_str("pub fn driver(): int { return 0 }\n");
    let (result, counts) = capture_alias_cycle_counts(|| compile(&project(source)));
    result.expect("the shared named target compiles");
    assert_eq!(
        (counts.terminal_rows, counts.terminal_bytes),
        (1, terminal.len())
    );
    assert_eq!((counts.target_visits, counts.resolved_edges), (257, 256));
}

#[test]
fn composed_optional_aliases_refuse_without_allocating_more_terminals() {
    let source = "alias A = int?\nalias B = A?\nalias C = B\npub fn driver(): int { return 0 }\n";
    let (result, counts) = capture_alias_cycle_counts(|| compile(&project(source.into())));
    let Err(crate::CompileFailure::Diagnostics(diagnostics)) = result else {
        panic!("double optionality must be a source refusal");
    };
    let rows: Vec<_> = diagnostics
        .iter()
        .map(|row| (row.code(), row.line(), row.column()))
        .collect();
    assert_eq!(
        rows,
        [("check.unsupported", 2, 1), ("check.unsupported", 3, 1)]
    );
    assert!(
        diagnostics
            .iter()
            .last()
            .expect("dependent refusal")
            .refused_declaration()
            .is_some()
    );
    assert_eq!((counts.terminal_rows, counts.terminal_bytes), (1, 3));
}
