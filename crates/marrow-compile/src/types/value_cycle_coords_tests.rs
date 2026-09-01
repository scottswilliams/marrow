//! The value-cycle report reads declaration coordinates the declare pass owns, not
//! the syntax tree.
//!
//! Driven through the production `compile` path. The observable is an exact
//! operation count, not a ratio: the pass examines zero syntax declarations, so a
//! reintroduced name scan fails the gate at one declaration rather than needing a
//! scale at which a linear term becomes visible.

use std::fmt::Write as _;

use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

use super::{ScalingCounts, capture_scaling_counts};
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

/// A corpus whose value cycles are reported: `pad` unrelated declarations ahead of
/// a cyclic struct and a cyclic resource, so a name scan over either declaration
/// list has something to walk before it matches.
fn cyclic_corpus(pad: usize) -> String {
    let mut source = String::new();
    for index in 0..pad {
        writeln!(source, "struct Pad{index} {{ value: int }}").expect("write pad struct");
    }
    for index in 0..pad {
        writeln!(source, "resource Spare{index} {{ value: int }}").expect("write pad resource");
    }
    source.push_str("struct Knot { me: Knot }\n");
    source.push_str("resource Coil { me: Coil }\n");
    source.push_str("fn main() {\n}\n");
    source
}

/// The counts of one compile, kept whether or not the program is admitted — this
/// corpus is deliberately refused, and its refusal is the thing being measured.
fn counts_of(source: String) -> (bool, ScalingCounts) {
    let (result, counts) = capture_scaling_counts(|| compile(&project(source)));
    (result.is_err(), counts)
}

/// The corpus must actually reach the report. A corpus that compiled cleanly, or
/// that stopped at an earlier phase, would make the zero below vacuous.
#[test]
fn the_cyclic_corpus_is_still_refused() {
    let (refused, _) = counts_of(cyclic_corpus(16));
    assert!(
        refused,
        "the value-cycle corpus must be refused; a clean compile never reaches the report"
    );
}

/// The gate: reporting a value cycle examines no syntax declaration.
#[test]
fn reporting_a_value_cycle_reads_no_syntax_declaration() {
    for pad in [1, 16, 64] {
        let (refused, counts) = counts_of(cyclic_corpus(pad));
        assert!(refused, "the corpus at pad {pad} must be refused");
        assert_eq!(
            counts.value_cycle_declaration_scan_steps, 0,
            "reporting a value cycle scanned {} syntax declarations at pad {pad}; the declare \
             pass owns the coordinate, so the report must read none",
            counts.value_cycle_declaration_scan_steps
        );
    }
}
