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
    assert!(!out.is_empty(), "production source inventory must be non-empty");
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
