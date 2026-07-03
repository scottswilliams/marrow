//! Soundness of the `has_errors` gate for total parsing.
//!
//! The parser yields an error node for anything it cannot structure, and every
//! error node travels with a `parse.syntax` diagnostic. Downstream, the checker
//! trusts that a parse error means the tree is not fully structured and must not
//! resolve an error node as if it were real syntax. These cases place an error
//! node in each expression position and drive the full check and analyze
//! pipelines: the run must not panic, must report the parse error, and must not
//! stack a semantic diagnostic on the error node.

use crate::support;
use support::{analyze_overlay, check_module_report};

/// One malformed function body per expression position an error node can occupy.
/// Each is otherwise a valid module, so the only fault is the unparsable operand.
const MALFORMED: &[(&str, &str)] = &[
    (
        "if-condition",
        "module m\nfn f()\n    if @\n        return\n",
    ),
    (
        "while-condition",
        "module m\nfn f()\n    while @\n        return\n",
    ),
    (
        "else-if-condition",
        "module m\nfn f()\n    if true\n        return\n    else if @\n        return\n",
    ),
    (
        "match-scrutinee",
        "module m\nfn f()\n    match @\n        x\n            return\n",
    ),
    ("return-value", "module m\nfn f(): int\n    return @\n"),
    ("throw-value", "module m\nfn f()\n    throw @\n"),
    ("delete-path", "module m\nfn f()\n    delete @\n"),
    (
        "expr-statement",
        "module m\nfn f()\n    show(@)\n\nfn show(x: int)\n    return\n",
    ),
    ("const-value", "module m\nconst C = @\n"),
    (
        "var-value",
        "module m\nfn f()\n    var x: int = @\n    return\n",
    ),
    (
        "interpolation-hole",
        "module m\nfn f()\n    var s: string = $\"a {@} b\"\n    return\n",
    ),
];

/// A parse error is reported for every malformed program, and no semantic
/// diagnostic is stacked on the error node: the checker sees only `parse.syntax`.
/// That is the observable form of "no error node reaches semantic processing" —
/// a `check.*` code here would mean the checker resolved the placeholder.
#[test]
fn error_nodes_reach_no_semantic_check() {
    for (label, source) in MALFORMED {
        let report = check_module_report(&format!("total-soundness-{label}"), source);
        assert!(
            report.diagnostics.iter().any(|d| d.code == "parse.syntax"),
            "{label}: the parse error must be reported: {:#?}",
            report.diagnostics
        );
        assert!(
            report.diagnostics.iter().all(|d| d.code == "parse.syntax"),
            "{label}: an error node must not reach a semantic check: {:#?}",
            report.diagnostics
        );
    }
}

/// The editor-facing analyze pipeline (binding index, document symbols, cursor
/// facts) runs over the same malformed programs without panicking and still
/// reports the parse error, so a partial-parse query never resolves an error node.
#[test]
fn analyze_pipeline_survives_error_nodes() {
    for (label, source) in MALFORMED {
        let (snapshot, paths) =
            analyze_overlay(&format!("total-analyze-{label}"), &[("src/m.mw", source)]);
        assert!(
            snapshot
                .report
                .diagnostics
                .iter()
                .any(|d| d.code == "parse.syntax"),
            "{label}: analyze must report the parse error: {:#?}",
            snapshot.report.diagnostics
        );
        // Document symbols are derived from the parsed tree; over a tree that holds
        // an error node the query must still return without faulting.
        let file = snapshot
            .files
            .iter()
            .find(|file| file.path == paths[0])
            .expect("analyzed file present");
        let _ = marrow_check::tooling::document_symbols(&file.parsed.file, source);
    }
}
