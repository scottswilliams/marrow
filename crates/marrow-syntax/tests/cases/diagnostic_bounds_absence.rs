//! Exact-symbol absence gates for the bounded syntax diagnostic substrate.
//!
//! These scans read the crate's production sources and reject the shapes the
//! DIAGBOUND01 design deletes: raw diagnostic vectors, post-submit collector
//! mutation, a public collector/sink surface, and the caller-less active-call
//! API (amendment A1). Each scan matches exact symbols, never spelling proxies;
//! the presence assertions keep the scans honest against a rename.

use std::fs;
use std::path::{Path, PathBuf};

/// Every production `.rs` file under `src/`, with its text, in sorted order.
fn production_sources() -> Vec<(PathBuf, String)> {
    fn collect(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        let mut entries: Vec<PathBuf> = fs::read_dir(dir)
            .expect("read src directory")
            .map(|entry| entry.expect("src entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                let text = fs::read_to_string(&path).expect("read production source");
                out.push((path, text));
            }
        }
    }
    let src = src_root();
    let mut out = Vec::new();
    collect(&src, &mut out);
    assert!(
        !out.is_empty(),
        "production source inventory must be non-empty"
    );
    out
}

fn src_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// A1: the caller-less active-call API is deleted, not migrated. The file is
/// gone and no production source names its functions or context types.
#[test]
fn active_call_surface_is_deleted() {
    assert!(
        !src_root().join("active_call.rs").exists(),
        "src/active_call.rs must be deleted (A1)"
    );
    for (path, text) in production_sources() {
        for symbol in [
            "active_callable_context",
            "callable_callee_contexts",
            "ActiveCallableContext",
            "CallableCalleeContext",
        ] {
            assert!(
                !text.contains(symbol),
                "{} still names the deleted active-call symbol `{symbol}`",
                path.display()
            );
        }
    }
}

/// Every syntax producer writes through the one live collector: no production
/// raw diagnostic vector — owned, borrowed, or as a signature — survives. The
/// only raw-row store is the collector's private state; the finished payload is
/// the boxed `CompleteSyntaxDiagnostics`. Tests may hold expected-value
/// vectors; production sources may not.
#[test]
fn no_production_raw_diagnostic_vectors() {
    for (path, text) in production_sources() {
        assert!(
            !text.contains("Vec<Diagnostic>"),
            "{} holds a raw diagnostic vector; produce through the collector sink instead",
            path.display()
        );
    }
}

/// A8: a diagnostic is finalized (message, help, reason) before submission.
/// No production site reaches back into submitted diagnostic state to mutate
/// it; the exact regressing shape is a `diagnostics` store handing out
/// `last_mut`.
#[test]
fn no_post_submit_diagnostic_mutation() {
    for (path, text) in production_sources() {
        assert!(
            !text.contains("diagnostics.last_mut"),
            "{} mutates a submitted diagnostic; finalize before submission (A8)",
            path.display()
        );
    }
}

/// The collector is one concrete private type: no generic collector or the
/// retired generic counter family reappears. A type parameter on the collector
/// would reopen per-policy configurability the design deletes.
#[test]
fn the_collector_is_concrete_not_generic() {
    let sources = production_sources();
    let all: String = sources.iter().map(|(_, text)| text.as_str()).collect();
    assert!(
        all.contains("struct SyntaxDiagnosticCollector"),
        "expected the concrete collector; if it was renamed, update this scan"
    );
    for (path, text) in &sources {
        for forbidden in ["SyntaxDiagnosticCollector<", "BoundedDiagnosticCounter"] {
            assert!(
                !text.contains(forbidden),
                "{} declares a generic collector shape: `{forbidden}`",
                path.display()
            );
        }
    }
}

/// The parameter list of the `fn new(` declared in `text`, whitespace-collapsed onto
/// one line. rustfmt wraps this signature differently as its length changes, so the
/// shape is compared on a canonical form rather than on one formatting of it.
fn constructor_parameters(text: &str) -> String {
    let open = text.find("fn new(").expect("the constructor is declared") + "fn new(".len();
    let parameters = &text[open..];
    let mut depth = 1usize;
    let end = parameters
        .char_indices()
        .find_map(|(index, character)| {
            match character {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(index)
        })
        .expect("the parameter list is closed");
    let collapsed = parameters[..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // rustfmt writes a trailing comma when it wraps and omits it inline; the canonical
    // form has neither, so both spellings compare equal.
    collapsed.trim_end_matches(',').to_string()
}

/// The extraction reads the real parameter list, not one formatting of it: the same
/// signature wrapped by rustfmt and written inline collapse to one string, a nested
/// parenthesis does not truncate it, and a code parameter is visible in the result.
/// Without this the gate below could pass by matching nothing.
#[test]
fn the_constructor_parameter_scan_is_insensitive_to_formatting() {
    let wrapped = "fn new(\n    reason: DiagnosticReason,\n    span: SourceSpan,\n) -> Self {";
    let inline = "fn new(reason: DiagnosticReason, span: SourceSpan) -> Self {";
    assert_eq!(
        constructor_parameters(wrapped),
        constructor_parameters(inline)
    );
    assert_eq!(
        constructor_parameters(inline),
        "reason: DiagnosticReason, span: SourceSpan"
    );
    assert_eq!(
        constructor_parameters("fn new(make: fn(u8) -> u8, span: SourceSpan) -> Self {"),
        "make: fn(u8) -> u8, span: SourceSpan"
    );
    assert!(
        constructor_parameters("fn new(reason: DiagnosticReason, code: &'static str) -> Self {")
            .contains("code"),
        "a code parameter must be visible to the gate below"
    );
}

/// The code is derived from the typed reason at the one error constructor, so
/// a code/reason mismatch is unrepresentable rather than merely unwritten: the
/// constructor accepts the typed reason and no code beside it, and the stored code
/// is read back off that reason.
#[test]
fn the_error_constructor_derives_its_code_from_the_reason() {
    let sources = production_sources();
    let inherent = sources
        .iter()
        .find_map(|(_, text)| text.split_once("impl SyntaxError {"))
        .expect("the private error type declares an inherent impl")
        .1;
    let parameters = constructor_parameters(inherent);
    assert!(
        parameters.starts_with("reason: DiagnosticReason,"),
        "the error constructor must take the typed reason first: `{parameters}`"
    );
    assert!(
        !parameters.contains("code"),
        "the error constructor must accept no code beside the reason: `{parameters}`"
    );
    let all: String = sources.iter().map(|(_, text)| text.as_str()).collect();
    assert!(
        all.contains("code: reason.code(),"),
        "expected the derived code; if the mapping moved, update this scan"
    );
}

/// The compiler owns invalid UTF-8: every syntax entry point takes `&str`, so
/// non-UTF-8 input is unrepresentable to this crate by API shape. The pinned
/// signatures keep that contract conspicuous against a byte-slice entry point.
#[test]
fn entry_points_admit_only_utf8_by_signature() {
    let sources = production_sources();
    let all: String = sources.iter().map(|(_, text)| text.as_str()).collect();
    for signature in [
        "pub fn parse_source(source: &str)",
        "pub fn lex_source(source: &str)",
        "pub fn parse_expression(source: &str)",
    ] {
        assert!(
            all.contains(signature),
            "expected the `&str` entry point `{signature}`; syntax must never decode bytes"
        );
    }
}

/// The collector, sinks, and error constructor are private implementation:
/// no public collector, sink type, sink trait, or sink accessor exists. The
/// presence half asserts the private symbols under their current names so a
/// rename must update this scan rather than silently voiding it.
#[test]
fn collector_and_sinks_stay_private() {
    let sources = production_sources();
    let all: String = sources.iter().map(|(_, text)| text.as_str()).collect();
    for required in [
        "struct SyntaxDiagnosticCollector",
        "struct SyntaxSink",
        "trait SyntaxErrorSink",
        "struct DiscardingSyntaxErrorSink",
        "struct SyntaxError",
    ] {
        assert!(
            all.contains(required),
            "expected the private substrate symbol `{required}`; if it was renamed, update this scan"
        );
    }
    for (path, text) in &sources {
        for forbidden in [
            "pub struct SyntaxDiagnosticCollector",
            "pub struct SyntaxSink",
            "pub trait SyntaxErrorSink",
            "pub struct DiscardingSyntaxErrorSink",
            "pub struct SyntaxError",
            "pub fn lexer_sink",
            "pub fn parser_sink",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} exposes the diagnostic substrate publicly: `{forbidden}`",
                path.display()
            );
        }
    }
}
