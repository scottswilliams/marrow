//! Docs-honesty verification gate: every `mw` fence in the current reference is
//! a complete source file that travels the real production path (`marrow test`)
//! through capture, compile, and independent image verification. The syntax
//! corpus proves the same fences parse and format; this gate additionally fails
//! when a documented example no longer checks or its compiled image is rejected.
//!
//! A fence is extracted to a correctly-pathed scratch project — module identity
//! is path-derived, so a `module a::b` header sits at `src/a/b.mw`; a moduleless
//! script sits at `src/main.mw`. Durable fences need a minted `marrow.ids`; the
//! one convenience mint action (`marrow run`) publishes it before the durable
//! export parks, so a durable fence verifies after the mint pre-pass exactly as a
//! caller's project would. Contextual fragments and deliberately future examples
//! use `text` fences and are skipped by construction.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MARROW: &str = env!("CARGO_BIN_EXE_marrow");

struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "marrow-doc-fences-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create temp dir");
        TempDir { root }
    }
}

impl Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write file");
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(MARROW)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run marrow binary")
}

/// How a complete source fence establishes its project identity.
enum FenceKind {
    /// A library file whose header declares a dotted module path.
    Module(String),
    /// A complete source file with no module header.
    Script,
}

/// One complete `mw` fence from a current reference page.
struct DocFence {
    doc: String,
    index: usize,
    kind: FenceKind,
    source: String,
}

impl DocFence {
    fn new(doc: String, index: usize, source: String) -> Self {
        let kind = match module_path_of(&source) {
            Some(path) => FenceKind::Module(path),
            None => FenceKind::Script,
        };
        Self {
            doc,
            index,
            kind,
            source,
        }
    }

    /// The project-relative source path derived by the real capture contract.
    fn source_rel_path(&self) -> PathBuf {
        match &self.kind {
            FenceKind::Module(module_path) => {
                let mut path = PathBuf::from("src");
                for segment in module_path.split('.') {
                    path.push(segment);
                }
                path.set_extension("mw");
                path
            }
            FenceKind::Script => PathBuf::from("src/main.mw"),
        }
    }

    fn source_label(&self) -> String {
        match &self.kind {
            FenceKind::Module(module_path) => {
                format!("module {}", module_path.replace('.', "::"))
            }
            FenceKind::Script => "moduleless script".to_string(),
        }
    }
}

/// The repository root (two levels above this crate's manifest).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonical repo root")
}

/// The `.md` files directly in `dir` (not recursive), in sorted path order.
fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(dir)
        .expect("read markdown directory")
        .map(|entry| entry.expect("markdown entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    files.sort();
    files
}

/// Every complete `mw` fence in the reference, in the same corpus order the
/// syntax gates read: `docs/language/*.md` (sorted), then top-level `*.md`
/// (sorted). Contextual fragments use another fence language and are absent.
fn documented_fences() -> Vec<DocFence> {
    let root = repo_root();
    let mut files = markdown_files(&root.join("docs").join("language"));
    files.extend(markdown_files(&root));

    let mut fences = Vec::new();
    for path in files {
        let doc = path
            .strip_prefix(&root)
            .expect("documentation path beneath repository root")
            .to_string_lossy()
            .into_owned();
        let text = fs::read_to_string(&path).expect("read markdown doc");
        let mut in_block = false;
        let mut index = 0usize;
        let mut source = String::new();
        for line in text.lines() {
            if line.trim() == "```mw" {
                in_block = true;
                index += 1;
                source.clear();
                continue;
            }
            if line.trim() == "```" && in_block {
                in_block = false;
                fences.push(DocFence::new(doc.clone(), index, source.clone()));
                continue;
            }
            if in_block {
                source.push_str(line);
                source.push('\n');
            }
        }
    }
    fences
}

/// The dotted module path a complete fence declares, or `None` for a script.
/// `module a::b` yields `a.b`.
fn module_path_of(source: &str) -> Option<String> {
    let header = source.trim_start().lines().next()?;
    let rest = header.strip_prefix("module ")?;
    Some(rest.trim().replace("::", "."))
}

#[derive(Debug, PartialEq, Eq)]
struct FailureRecord {
    outcome: String,
    code: Option<String>,
}

#[derive(Debug)]
struct FenceFailure {
    status: Option<i32>,
    records: Vec<FailureRecord>,
    stdout: String,
    stderr: String,
}

impl FenceFailure {
    fn from_output(output: Output) -> Self {
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        Self {
            status: output.status.code(),
            records: failure_records(&stdout),
            stdout,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn has(&self, outcome: &str, code: &str) -> bool {
        self.records
            .iter()
            .any(|record| record.outcome == outcome && record.code.as_deref() == Some(code))
    }

    fn has_code(&self, code: &str) -> bool {
        self.records
            .iter()
            .any(|record| record.code.as_deref() == Some(code))
    }

    fn describe(&self) -> String {
        format!(
            "status={:?}, records={:?}, stdout={:?}, stderr={:?}",
            self.status, self.records, self.stdout, self.stderr
        )
    }
}

/// Require one final production-path command to succeed without a typed failure
/// record. The status is the primary contract; checking the record stream too
/// makes a future exit-code regression fail closed.
fn finish(output: Output) -> Result<(), FenceFailure> {
    let success = output.status.success();
    let failure = FenceFailure::from_output(output);
    if success && failure.records.is_empty() {
        Ok(())
    } else {
        Err(failure)
    }
}

/// Compile and independently verify one fence on the production CLI path. A
/// durable fence is minted once (`marrow run` is the sole mint owner) and then
/// retried, so a clean durable example reaches verification rather than stopping
/// at its missing machine-written identity artifact.
fn verify_fence(fence: &DocFence) -> Result<(), FenceFailure> {
    let temp = TempDir::new("fence");
    write(&temp.join("marrow.toml"), "edition = \"2026\"\n");
    write(&temp.join(fence.source_rel_path()), &fence.source);

    let first = run_in(&temp, &["test", "--format", "jsonl"]);
    if first.status.success() {
        return finish(first);
    }
    let first_failure = FenceFailure::from_output(first);
    if !first_failure.has_code("check.durable_identity") {
        return Err(first_failure);
    }

    // A durable fence is missing only its machine-written ids until the one
    // convenience mint publishes them; require a fresh final compile+verify over
    // the minted ledger. The final result remains authoritative if minting fails.
    let _ = run_in(&temp, &["run", "__doc_fence_probe__"]);
    finish(run_in(&temp, &["test", "--format", "jsonl"]))
}

/// Typed failure records carried by the CLI's flat JSONL stream. Passing tests
/// and summaries are not failures and therefore do not appear here.
fn failure_records(stdout: &str) -> Vec<FailureRecord> {
    stdout
        .lines()
        .filter_map(|line| {
            let outcome = json_field(line, "outcome")?;
            matches!(
                outcome.as_str(),
                "diagnostic" | "artifact_rejected" | "fault" | "error" | "failed" | "errored"
            )
            .then(|| FailureRecord {
                outcome,
                code: json_field(line, "code"),
            })
        })
        .collect()
}

/// The value of a string field in one flat JSONL object (`"key":"value"`).
fn json_field(line: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let start = line.find(&needle)? + needle.len();
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

/// The gate: every complete `mw` fence in the current reference compiles and
/// independently verifies. A failure names the page, block, source kind, process
/// status, and typed JSONL failure records.
#[test]
fn every_documented_mw_fence_compiles_and_verifies() {
    let fences = documented_fences();
    assert!(
        fences.len() >= 60,
        "expected the reference corpus, found {} complete source fences",
        fences.len()
    );

    let mut failures = Vec::new();
    for fence in &fences {
        if let Err(failure) = verify_fence(fence) {
            failures.push(format!(
                "{} fence #{} [{}] failed compile/verify: {}",
                fence.doc,
                fence.index,
                fence.source_label(),
                failure.describe(),
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} documented source fence(s) fail the compile+verify gate:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

/// The gate is red-provable: a deliberately broken fence — a keyed scalar leaf,
/// the historical `check.unsupported` shape that shipped silently before this
/// gate — is caught, so a regression that reintroduces an unchecked fence cannot
/// pass green.
#[test]
fn a_source_rejected_fence_is_caught() {
    let broken = DocFence::new(
        "in-test".to_string(),
        1,
        "module broken::leaf\n\nresource Book {\n    required title: string\n    tags[pos: int]: string\n}\n".to_string(),
    );

    let failure = verify_fence(&broken).expect_err("broken source must fail the gate");
    assert!(
        failure.has("diagnostic", "check.unsupported"),
        "the gate must catch a keyed-scalar-leaf fence, got: {}",
        failure.describe(),
    );
}

/// The gate must fail after checking when the independent verifier rejects the
/// compiler's image. An empty durable region is the agreement ledger's smallest
/// checker-accepted `image.flow` case.
#[test]
fn a_verifier_rejected_fence_is_caught() {
    let broken = DocFence::new(
        "in-test".to_string(),
        1,
        "module broken::verify\n\npub fn emptyRegion() {\n    transaction {\n    }\n}\n"
            .to_string(),
    );

    let failure = verify_fence(&broken).expect_err("rejected image must fail the gate");
    assert!(
        failure.has("artifact_rejected", "image.flow"),
        "the gate must catch a verifier-rejected fence, got: {}",
        failure.describe(),
    );
}

/// A complete source file does not need a `module` header. The two standalone
/// declarations in the values reference are moduleless scripts and belong to
/// the production-path corpus.
#[test]
fn complete_moduleless_reference_fences_are_gated() {
    let fences = documented_fences();
    assert!(
        fences
            .iter()
            .any(|fence| fence.source.trim_start().starts_with("struct Point")),
        "the complete moduleless Point declaration must be gated",
    );
    assert!(
        fences
            .iter()
            .any(|fence| fence.source.trim_start().starts_with("struct Pair")),
        "the complete moduleless generic declarations must be gated",
    );
}
