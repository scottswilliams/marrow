//! Docs-honesty verification gate: every `mw` fence in the current reference is
//! a complete source file that travels the real production path — capture,
//! compile, independent image verification, and the source tests it declares.
//! The syntax corpus proves the same fences parse and format; this gate
//! additionally fails when a documented example no longer checks, its compiled
//! image is rejected, or one of its `test` declarations no longer passes.
//!
//! A fence is extracted to a correctly-pathed project — module identity is
//! path-derived, so a `module a::b` header sits at `src/a/b.mw`; a moduleless
//! script sits at `src/main.mw`. A storeless fence travels that path in process.
//! A durable fence needs a minted `.marrow/ids`, and `marrow run` is the one
//! convenience mint, so those fences take the CLI: mint, then compile and verify
//! over the minted ledger exactly as a caller's project would. Contextual
//! fragments and deliberately future examples use `text` fences and are skipped
//! by construction.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use marrow_compile::CompileFailure;
use marrow_project::{CaptureLimits, CapturedFile, Manifest};
use marrow_vm::{DurableExecutionFault, DurableRun, IncompleteDisposition};

mod common;

use common::{TempDir, marrow_in, write};
use marrow_codes::Code;

#[test]
fn scratch_projects_are_unique_within_the_test_process() {
    let first = TempDir::new("unique");
    let second = TempDir::new("unique");
    assert_ne!(&*first, &*second);
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

/// The `.md` files directly in `dir`, in sorted path order.
fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(dir)
        .expect("read markdown directory")
        .map(|entry| entry.expect("markdown entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    files.sort();
    files
}

/// The `.md` files recursively beneath `dir`, optionally excluding one complete
/// subtree.
fn markdown_files_recursively(dir: &Path, excluded: Option<&Path>) -> Vec<PathBuf> {
    fn collect(dir: &Path, excluded: Option<&Path>, files: &mut Vec<PathBuf>) {
        if excluded == Some(dir) {
            return;
        }
        let mut entries = fs::read_dir(dir)
            .expect("read markdown directory")
            .map(|entry| entry.expect("markdown entry").path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                collect(&path, excluded, files);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    collect(dir, excluded, &mut files);
    files
}

/// Current documentation sources: every Markdown page below `docs/` except the
/// explicitly future subtree, followed by repository-root Markdown front doors.
fn current_markdown_files(root: &Path) -> Vec<PathBuf> {
    let docs = root.join("docs");
    let future = docs.join("future");
    let mut files = markdown_files_recursively(&docs, Some(&future));
    files.extend(markdown_files(root));
    files
}

fn fences_in_document(doc: &str, text: &str) -> Vec<DocFence> {
    let mut fences = Vec::new();
    let mut in_block = false;
    let mut index = 0usize;
    let mut source = String::new();
    for line in text.lines() {
        if line.trim() == "```mw" {
            assert!(!in_block, "nested mw fence in {doc} block #{index}");
            in_block = true;
            index += 1;
            source.clear();
            continue;
        }
        if line.trim() == "```" && in_block {
            in_block = false;
            fences.push(DocFence::new(doc.to_string(), index, source.clone()));
            continue;
        }
        if in_block {
            source.push_str(line);
            source.push('\n');
        }
    }
    assert!(!in_block, "unterminated mw fence in {doc} block #{index}");
    fences
}

/// Every complete `mw` fence in current documentation, in the same corpus order
/// the syntax gates read. Contextual fragments use another fence language and
/// are absent; future pages are checked separately for `mw`-fence absence.
fn documented_fences() -> Vec<DocFence> {
    let root = repo_root();
    let files = current_markdown_files(&root);

    let mut fences = Vec::new();
    for path in files {
        let doc = path
            .strip_prefix(&root)
            .expect("documentation path beneath repository root")
            .to_string_lossy()
            .into_owned();
        let text = fs::read_to_string(&path).expect("read markdown doc");
        fences.extend(fences_in_document(&doc, &text));
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
    initial_records: Vec<FailureRecord>,
    records: Vec<FailureRecord>,
    stdout: String,
    stderr: String,
}

impl FenceFailure {
    fn from_output(output: Output) -> Self {
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        Self {
            status: output.status.code(),
            initial_records: Vec::new(),
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

    fn initially_has(&self, outcome: &str, code: &str) -> bool {
        self.initial_records
            .iter()
            .any(|record| record.outcome == outcome && record.code.as_deref() == Some(code))
    }

    fn describe(&self) -> String {
        format!(
            "status={:?}, initial_records={:?}, records={:?}, stdout={:?}, stderr={:?}",
            self.status, self.initial_records, self.records, self.stdout, self.stderr
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

/// One fence's verdict from the in-process production path.
enum InProcess {
    /// Compiled, verified, and every declared source test passed.
    Clean,
    /// The fence is durable and its ledger rows are unminted, which only the CLI
    /// mint publishes. Carries the pre-mint diagnostics.
    NeedsMint(Vec<FailureRecord>),
    /// The fence failed, carrying the same typed records the CLI would stream.
    Rejected(Vec<FailureRecord>),
}

fn record(outcome: &str, code: Option<&str>) -> FailureRecord {
    FailureRecord {
        outcome: outcome.to_string(),
        code: code.map(str::to_string),
    }
}

/// Capture, compile, verify, and run one fence's source tests without a process.
/// The records mirror the CLI's typed stream, so both paths report alike.
fn check_in_process(fence: &DocFence) -> InProcess {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![CapturedFile::new(
        fence.source_rel_path().to_string_lossy().into_owned(),
        fence.source.clone().into_bytes(),
    )];
    let project = match marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT) {
        Ok(project) => project,
        Err(_) => return InProcess::Rejected(vec![record("error", None)]),
    };
    // The test-inclusive compile is the one `marrow test` drives: the production
    // `compile` emits an empty TEST-ENTRY table, so a fence's `test` declarations
    // would never run.
    let compiled = match marrow_compile::compile_with_tests(&project) {
        Ok(compiled) => compiled,
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            let records: Vec<_> = diagnostics
                .as_slice()
                .iter()
                .map(|diagnostic| record("diagnostic", Some(diagnostic.code().as_str())))
                .collect();
            return if records
                .iter()
                .any(|entry| entry.code.as_deref() == Some("check.durable_identity"))
            {
                InProcess::NeedsMint(records)
            } else {
                InProcess::Rejected(records)
            };
        }
        Err(_) => return InProcess::Rejected(vec![record("error", None)]),
    };
    let image = match marrow_verify::verify(&compiled.image.bytes) {
        Ok(image) => image,
        Err(rejection) => {
            return InProcess::Rejected(vec![record(
                "artifact_rejected",
                Some(rejection.code().as_str()),
            )]);
        }
    };

    let prepared = marrow_vm::prepare(image);
    let mut records = Vec::new();
    for index in 0..prepared.image().test_entries().len() {
        let test = marrow_vm::fresh_test(&prepared, index)
            .expect("the entry index came from the prepared image's own test table");
        records.extend(test_record(marrow_vm::run_test(test)));
    }
    if records.is_empty() {
        InProcess::Clean
    } else {
        InProcess::Rejected(records)
    }
}

/// The failure record one source test leaves, or none when it passes. A false
/// `assert` fails; any other source-mapped fault, a park, or an operational mint
/// failure errors.
fn test_record(run: DurableRun) -> Option<FailureRecord> {
    let fault = match run {
        DurableRun::Ran(Ok(_)) => return None,
        DurableRun::Ran(Err(fault)) => fault,
        DurableRun::Parked => return Some(record("errored", None)),
        DurableRun::Failed(code) => return Some(record("errored", Some(code.as_str()))),
    };
    let fault = match fault {
        DurableExecutionFault::Runtime(fault) => fault,
        DurableExecutionFault::Incomplete(incomplete) => match incomplete.into_disposition() {
            IncompleteDisposition::Classified { fault, .. } => fault,
            IncompleteDisposition::Pending { fault, .. } => fault,
        },
    };
    let outcome = if fault.code() == Code::RunAssert {
        "failed"
    } else {
        "fault"
    };
    Some(record(outcome, Some(fault.code().as_str())))
}

/// Compile, independently verify, and run one fence's source tests. A storeless
/// fence takes the in-process path; a durable one is minted once (`marrow run` is
/// the sole mint owner) and then driven through the CLI, so a clean durable
/// example reaches verification rather than stopping at its missing
/// machine-written identity artifact.
fn verify_fence(fence: &DocFence) -> Result<(), FenceFailure> {
    match check_in_process(fence) {
        InProcess::Clean => Ok(()),
        InProcess::Rejected(records) => Err(FenceFailure {
            status: None,
            initial_records: Vec::new(),
            records,
            stdout: String::new(),
            stderr: String::new(),
        }),
        InProcess::NeedsMint(pre_mint) => mint_and_verify(fence, pre_mint),
    }
}

/// A durable fence is missing only its machine-written ids until the one
/// convenience mint publishes them. Mint, then require a fresh compile,
/// verification and source-test run over the minted ledger. The final result
/// remains authoritative if minting fails.
fn mint_and_verify(fence: &DocFence, pre_mint: Vec<FailureRecord>) -> Result<(), FenceFailure> {
    let temp = TempDir::new("fence");
    write(&temp.join("marrow.toml"), "edition = \"2026\"\n");
    write(&temp.join(fence.source_rel_path()), &fence.source);

    let _ = marrow_in(&temp, &["run", "__doc_fence_probe__"]);
    finish(marrow_in(&temp, &["test", "--format", "jsonl"]).output).map_err(|mut failure| {
        failure.initial_records = pre_mint;
        failure
    })
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

/// The gate: every complete `mw` fence in current documentation compiles and
/// independently verifies. A failure names the page, block, source kind, process
/// status, and typed JSONL failure records.
#[test]
fn every_documented_mw_fence_compiles_and_verifies() {
    let fences = documented_fences();
    assert!(
        fences.len() >= 60,
        "expected the current documentation corpus, found {} complete source fences",
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

/// A keyed scalar leaf is rejected with a typed source diagnostic. This probe
/// requires that diagnostic to make the documentation gate fail closed.
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

// A durable fence rejected only once its store identity is complete: a durable read
// follows the region's commit, an ownership law caught at check time
// (`check.durable_after_commit`). The initial compile stops at `check.durable_identity`
// (no ledger yet), so the rejection surfaces only after the gate mints identities and
// retries — the boundary this probe exercises. The compiler refuses this class of fault
// first; the independent verifier still rejects a tampered image at `image.flow`.
const POST_MINT_REJECTED_DURABLE_BODY: &str = "resource Item {\n    required value: string\n}\n\nstore ^items[id: int]: Item\n\npub fn setAndGet(id: int, value: string): string? {\n    transaction {\n        ^items[id] = Item(value: value)\n    }\n    return ^items[id].value\n}\n";

// The gate's `artifact_rejected` branch (a fence that compiles clean but the independent
// verifier rejects) is unreachable through an honest fence: the `agreement_gate` enforces
// that no checker-accepted source is verifier-rejected. Only a forged or tampered image
// reaches it, and that coverage lives in the `marrow-verify` hostiles.
fn assert_rejected_after_identity_mint(fence: &DocFence) {
    let failure = verify_fence(fence).expect_err("rejected fence must fail the gate");
    assert!(
        failure.initially_has("diagnostic", "check.durable_identity"),
        "the probe must cross the identity mint/retry boundary, got: {}",
        failure.describe(),
    );
    assert!(
        failure.has("diagnostic", "check.durable_after_commit"),
        "the gate must catch the fence once its identity is minted, got: {}",
        failure.describe(),
    );
}

/// The gate must fail after a durable project's identity mint when the retried
/// compile rejects the source. A durable read follows the region's commit, so the
/// post-mint attempt is refused with `check.durable_after_commit`.
#[test]
fn a_fence_rejected_after_identity_mint_is_caught() {
    let broken = DocFence::new(
        "in-test".to_string(),
        1,
        format!("module broken::verify\n\n{POST_MINT_REJECTED_DURABLE_BODY}"),
    );

    assert_rejected_after_identity_mint(&broken);
}

/// The moduleless branch must write the same rejected durable source to the
/// script path before crossing the identity-mint and retried-compile gates.
#[test]
fn a_moduleless_fence_rejected_after_identity_mint_is_caught() {
    let broken = DocFence::new(
        "in-test".to_string(),
        1,
        POST_MINT_REJECTED_DURABLE_BODY.to_string(),
    );

    assert!(matches!(&broken.kind, FenceKind::Script));
    assert_rejected_after_identity_mint(&broken);
    assert_eq!(broken.source_rel_path(), PathBuf::from("src/main.mw"));
}

#[test]
fn current_documentation_inventory_reaches_nested_sections() {
    let root = repo_root();
    let files = current_markdown_files(&root)
        .into_iter()
        .map(|path| {
            path.strip_prefix(&root)
                .expect("documentation beneath repository root")
                .to_path_buf()
        })
        .collect::<Vec<_>>();
    assert!(files.contains(&PathBuf::from("docs/implementation/testing.md")));
    assert!(files.contains(&PathBuf::from("docs/tools/cli.md")));
    assert!(!files.iter().any(|path| path.starts_with("docs/future")));
}

#[test]
#[should_panic(expected = "unterminated mw fence")]
fn an_unterminated_mw_fence_cannot_escape_the_gate() {
    let _ = fences_in_document("in-test.md", "```mw\nmodule broken\n");
}

#[test]
fn future_pages_have_no_current_source_fences() {
    let future = repo_root().join("docs").join("future");
    for path in markdown_files_recursively(&future, None) {
        let text = fs::read_to_string(&path).expect("read future page");
        assert!(
            !text.lines().any(|line| line.trim() == "```mw"),
            "future page {} must use a non-current fence language",
            path.display(),
        );
    }
}

/// A complete source file does not need a `module` header. A moduleless script
/// (the quickstart programs) belongs to the production-path corpus like a module.
#[test]
fn complete_moduleless_reference_fences_are_gated() {
    let fences = documented_fences();
    assert!(
        fences
            .iter()
            .any(|fence| matches!(fence.kind, FenceKind::Script)),
        "at least one complete moduleless script must be gated",
    );
}

/// The narrated walkthrough quotes the workshop fixture. Every declaration group
/// (blank-line separated) inside a `text` fence in `docs/walkthrough.md` is an exact
/// excerpt of that program's source, so the page cannot drift from the program whose
/// own tests prove its behavior; a fence may stitch declarations the fixture keeps apart.
#[test]
fn walkthrough_excerpts_are_verbatim_fixture_source() {
    let root = repo_root();
    let page = fs::read_to_string(root.join("docs/walkthrough.md")).expect("read walkthrough");
    let fixture = fs::read_to_string(root.join("fixtures/v01/conformance/workshop/src/main.mw"))
        .expect("read workshop fixture");

    let mut excerpts = 0usize;
    let mut in_block = false;
    let mut block = String::new();
    for line in page.lines() {
        if line.trim() == "```text" {
            in_block = true;
            block.clear();
            continue;
        }
        if line.trim() == "```" && in_block {
            in_block = false;
            excerpts += 1;
            for chunk in block.split("\n\n").filter(|chunk| !chunk.trim().is_empty()) {
                assert!(
                    fixture.contains(chunk.trim_end_matches('\n')),
                    "walkthrough excerpt #{excerpts} is not verbatim fixture source:\n{chunk}"
                );
            }
            continue;
        }
        if in_block {
            block.push_str(line);
            block.push('\n');
        }
    }
    assert!(
        excerpts >= 6,
        "the walkthrough quotes at least six fixture excerpts, found {excerpts}"
    );
}
