//! The analysis worker's work: capture the project through the shared physical adapter
//! with a version-selected overlay, then analyze it through the compiler fact floor.
//!
//! This module is the *only* consumer of the CAP01 capture allowlist. It constructs the
//! overlay from the open-document texts, captures, and calls
//! [`marrow_compile::analyze`]. It reclassifies no capture failure: it renders the
//! opaque [`CaptureFailure`] through the borrowed facade's operating-system-prose-free
//! operational writer into its own bounded sink and yields bounded typed evidence. It
//! never reads a raw path or error, matches a pure error/reason, or renders through any
//! other writer.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use marrow_compile::{AnalysisFailure, AnalysisSnapshot, InputRevision, analyze};
use marrow_project::ProjectInput;
use marrow_project_fs::{
    CaptureFailure, CapturePresentation, OverlayEntry, OverlaySnapshot, capture_project,
};

use crate::document::UnavailableEvidence;
use crate::uri::SelectedRoot;

/// The upper bound on a rendered operational message. The facade writer streams without
/// a cap; the server bounds its own sink and treats overflow as a rendering failure.
const MAX_OPERATIONAL_MESSAGE_BYTES: usize = 8 * 1024;

/// One borrowed overlay input: a canonical root-relative key and replacement bytes,
/// gathered from an open `OpenText` entry before capture.
pub struct OverlayInput<'a> {
    /// The canonical root-relative key, e.g. `src/foo.mw`.
    pub key: &'a str,
    /// The open-document body bytes.
    pub bytes: &'a [u8],
}

/// The outcome of one analysis job.
pub enum AnalysisOutcome {
    /// A complete immutable snapshot at the job's revision.
    Snapshot(Arc<AnalysisSnapshot>),
    /// Capture (or overlay admission before capture) was refused. The rendered bounded
    /// evidence is carried for the request-owned `-32803` and the background episode.
    Capture(CaptureRejection),
    /// The analysis floor exhausted a fixed resource bound. Recoverable.
    ResourceLimit { revision: InputRevision },
    /// The compiler was internally incoherent. Fail-stop class.
    Invariant { revision: InputRevision },
}

/// Bounded rendered capture-failure evidence: a stable code and an operational message.
/// The one shape the server presents, whether the failure came from overlay input or
/// physical capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureRejection {
    /// The revision the rejected capture belonged to.
    pub revision: InputRevision,
    /// The rendered bounded evidence, or `None` when the operational message overflowed
    /// its bounded sink (an outbound-encoding failure the coordinator handles).
    pub evidence: Option<UnavailableEvidence>,
}

impl SelectedRoot {
    /// The reconstructed absolute filesystem path of this root. No physical
    /// canonicalization — the caller-selected lexical spelling is retained.
    pub fn to_path(&self) -> PathBuf {
        let mut path = PathBuf::from("/");
        for component in self.components() {
            path.push(component);
        }
        path
    }
}

/// Run one analysis job: build the overlay, capture, and analyze.
pub fn run_analysis(
    root: &SelectedRoot,
    overlay: &[OverlayInput<'_>],
    revision: InputRevision,
) -> AnalysisOutcome {
    let root_path = root.to_path();
    let entries: Vec<OverlayEntry<'_>> = overlay
        .iter()
        .map(|input| OverlayEntry::new(input.key, input.bytes))
        .collect();
    let snapshot = match OverlaySnapshot::try_new(&entries) {
        Ok(snapshot) => snapshot,
        Err(failure) => {
            // Overlay input was refused before capture: carry it through the opaque
            // boundary and render it exactly like a capture failure.
            let failure = CaptureFailure::from_overlay_input(failure);
            return AnalysisOutcome::Capture(render_rejection(&root_path, &failure, revision));
        }
    };
    match capture_project(&root_path, snapshot) {
        Ok(input) => run_analyze(input, revision),
        Err(failure) => AnalysisOutcome::Capture(render_rejection(&root_path, &failure, revision)),
    }
}

fn run_analyze(input: ProjectInput, revision: InputRevision) -> AnalysisOutcome {
    match analyze(Arc::new(input), revision) {
        Ok(snapshot) => AnalysisOutcome::Snapshot(snapshot),
        Err(AnalysisFailure::ResourceLimit { revision, .. }) => {
            AnalysisOutcome::ResourceLimit { revision }
        }
        Err(AnalysisFailure::Invariant { revision, .. }) => AnalysisOutcome::Invariant { revision },
    }
}

/// Render a capture failure through the borrowed facade into a bounded sink. The only
/// facade methods used are the allowlisted [`CapturePresentation::code`] and
/// [`CapturePresentation::write_operational_message`]; `position` and the location and
/// CLI writers are never called, so a capture failure is always unlocated.
fn render_rejection(
    root_path: &Path,
    failure: &CaptureFailure,
    revision: InputRevision,
) -> CaptureRejection {
    let presentation: CapturePresentation<'_> = failure.presentation(root_path);
    let code = presentation.code().as_str();
    let mut sink = BoundedSink::new(MAX_OPERATIONAL_MESSAGE_BYTES);
    let evidence = match presentation.write_operational_message(&mut sink) {
        Ok(()) => Some(UnavailableEvidence {
            code,
            message: sink.into_string(),
        }),
        // The bounded sink overflowed: discard the partial bytes; the coordinator maps
        // this to an outbound-encoding failure rather than a truncated message.
        Err(_) => None,
    };
    CaptureRejection { revision, evidence }
}

/// A `fmt::Write` sink that admits at most `limit` bytes and then fails, so a rendered
/// message can never exceed the server's bound. Partial bytes are discarded on overflow.
struct BoundedSink {
    buffer: String,
    limit: usize,
}

impl BoundedSink {
    fn new(limit: usize) -> Self {
        Self {
            buffer: String::new(),
            limit,
        }
    }

    fn into_string(self) -> String {
        self.buffer
    }
}

impl std::fmt::Write for BoundedSink {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        if self.buffer.len() + text.len() > self.limit {
            return Err(std::fmt::Error);
        }
        self.buffer.push_str(text);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Build a real on-disk project and return its root URI spelling.
    fn write_project(dir: &Path, files: &[(&str, &str)]) {
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("marrow.toml"), "edition = \"2026\"\n").unwrap();
        for (name, body) in files {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, body).unwrap();
        }
    }

    fn root_for(dir: &Path) -> SelectedRoot {
        let mut uri = String::from("file://");
        for component in dir.components() {
            use std::path::Component;
            if let Component::Normal(part) = component {
                uri.push('/');
                uri.push_str(part.to_str().unwrap());
            }
        }
        SelectedRoot::from_uri(&uri).unwrap()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "marrow-lsp-analysis-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn analyzes_clean_project_from_disk() {
        let dir = temp_dir("clean");
        write_project(
            &dir,
            &[("src/main.mw", "module main\n\npub fn add(a: int, b: int): int {\n    return a + b\n}\n")],
        );
        let root = root_for(&dir);
        let outcome = run_analysis(&root, &[], InputRevision::new(1));
        match outcome {
            AnalysisOutcome::Snapshot(snapshot) => {
                assert_eq!(snapshot.revision(), InputRevision::new(1));
                assert!(snapshot.diagnostics().is_empty(), "clean project has no diagnostics");
            }
            _ => panic!("expected a snapshot"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overlay_replaces_disk_body() {
        let dir = temp_dir("overlay");
        write_project(&dir, &[("src/main.mw", "module main\n\npub fn f(): int {\n    return 1\n}\n")]);
        let root = root_for(&dir);
        // Overlay an unparseable body; diagnostics must reflect the overlay, not disk.
        let bad = b"module main\n\npub fn f(): int {\n    return \n}\n";
        let overlay = vec![OverlayInput {
            key: "src/main.mw",
            bytes: bad,
        }];
        let outcome = run_analysis(&root, &overlay, InputRevision::new(2));
        match outcome {
            AnalysisOutcome::Snapshot(snapshot) => {
                assert!(
                    !snapshot.diagnostics().is_empty(),
                    "overlaid invalid body should produce diagnostics"
                );
            }
            other => panic!("expected snapshot, got {}", label(&other)),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_manifest_is_capture_rejection() {
        let dir = temp_dir("nomanifest");
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/main.mw"), "module main\n").unwrap();
        let root = root_for(&dir);
        let outcome = run_analysis(&root, &[], InputRevision::new(1));
        match outcome {
            AnalysisOutcome::Capture(rejection) => {
                let evidence = rejection.evidence.expect("rendered evidence");
                assert!(!evidence.message.is_empty());
                assert!(!evidence.code.is_empty());
            }
            other => panic!("expected capture rejection, got {}", label(&other)),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn overlay_noncanonical_key_is_capture_rejection() {
        let dir = temp_dir("badkey");
        write_project(&dir, &[("src/main.mw", "module main\n")]);
        let root = root_for(&dir);
        let overlay = vec![OverlayInput {
            key: "../escape.mw",
            bytes: b"module x\n",
        }];
        let outcome = run_analysis(&root, &overlay, InputRevision::new(1));
        assert!(matches!(outcome, AnalysisOutcome::Capture(_)));
        fs::remove_dir_all(&dir).ok();
    }

    fn label(outcome: &AnalysisOutcome) -> &'static str {
        match outcome {
            AnalysisOutcome::Snapshot(_) => "snapshot",
            AnalysisOutcome::Capture(_) => "capture",
            AnalysisOutcome::ResourceLimit { .. } => "resource",
            AnalysisOutcome::Invariant { .. } => "invariant",
        }
    }

    #[test]
    fn bounded_sink_overflows() {
        use std::fmt::Write;
        let mut sink = BoundedSink::new(4);
        assert!(sink.write_str("abc").is_ok());
        assert!(sink.write_str("de").is_err());
    }
}
