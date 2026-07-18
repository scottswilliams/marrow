//! Docs-honesty check gate: every complete `module` fence in the language
//! reference travels the real production path (`marrow test`) and must check
//! clean. The parse gate in `marrow-syntax` proves each fence parses and formats;
//! it cannot check, because checking needs the compiler. This gate closes that
//! gap: a documented `mw` example that no longer type-checks fails CI instead of
//! shipping a stale surface.
//!
//! A fence is extracted to a correctly-pathed scratch project — module identity
//! is path-derived, so a `module a::b` header sits at `src/a/b.mw` — and run
//! through the built binary. Durable fences need a minted `marrow.ids`; the one
//! convenience mint action (`marrow run`) publishes it before the durable export
//! parks, so a durable fence checks after the mint pre-pass exactly as a caller's
//! project would. Incomplete or deliberately future examples use `text` fences
//! and are skipped here by construction (only `module`-opening `mw` fences check).

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

/// One complete `module` fence from a reference page: where it lives, and its
/// source. `module_path` is the dotted path its header declares (`a::b` → `a.b`),
/// which fixes the file location the project capture derives its identity from.
struct ModuleFence {
    doc: String,
    index: usize,
    module_path: String,
    source: String,
}

impl ModuleFence {
    /// The correctly-pathed source file for this fence: `module a::b` → `src/a/b.mw`.
    fn source_rel_path(&self) -> PathBuf {
        let mut path = PathBuf::from("src");
        for segment in self.module_path.split('.') {
            path.push(segment);
        }
        path.set_extension("mw");
        path
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

/// Every complete `module` fence in the reference, in the same corpus order the
/// parse gate reads: `docs/language/*.md` (sorted), then top-level `*.md` (sorted).
/// A fence that opens with `module ` is a complete library file; a fragment or a
/// `text` fence is not a `mw` module and is not returned.
fn module_fences() -> Vec<ModuleFence> {
    let root = repo_root();
    let mut files = markdown_files(&root.join("docs").join("language"));
    files.extend(markdown_files(&root));

    let mut fences = Vec::new();
    for path in files {
        let doc = path.file_name().unwrap().to_string_lossy().into_owned();
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
                if let Some(module_path) = module_path_of(&source) {
                    fences.push(ModuleFence {
                        doc: doc.clone(),
                        index,
                        module_path,
                        source: source.clone(),
                    });
                }
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

/// The dotted module path a complete fence declares, or `None` when the fence is
/// a fragment (no `module ` header). `module a::b` yields `a.b`.
fn module_path_of(source: &str) -> Option<String> {
    let header = source.trim_start().lines().next()?;
    let rest = header.strip_prefix("module ")?;
    Some(rest.trim().replace("::", "."))
}

/// The `check.*` diagnostic codes a fence produces on the production check path,
/// empty when it checks clean. A durable fence is minted once (`marrow run` is the
/// sole mint owner) and re-checked, so a clean durable example is reported clean
/// rather than blocked on a machine-written identity artifact.
fn check_codes(fence: &ModuleFence) -> Vec<String> {
    let temp = TempDir::new("fence");
    write(&temp.join("marrow.toml"), "edition = \"2026\"\n");
    write(&temp.join(fence.source_rel_path()), &fence.source);

    let first = check_diagnostic_codes(&run_in(&temp, &["test", "--format", "jsonl"]));
    if !first.iter().any(|code| code == "check.durable_identity") {
        return first;
    }

    // A durable fence is missing only its machine-written ids until the one
    // convenience mint publishes them; re-check against the minted ledger.
    let _ = run_in(&temp, &["run", "__doc_fence_probe__"]);
    check_diagnostic_codes(&run_in(&temp, &["test", "--format", "jsonl"]))
}

/// The `check.*` codes carried by the `diagnostic` records in a JSONL run.
fn check_diagnostic_codes(output: &Output) -> Vec<String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter(|line| line.contains(r#""outcome":"diagnostic""#))
        .filter_map(|line| json_field(line, "code"))
        .filter(|code| code.starts_with("check."))
        .collect()
}

/// The value of a string field in one flat JSONL object (`"key":"value"`).
fn json_field(line: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let start = line.find(&needle)? + needle.len();
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

/// The gate: every complete `module` fence in the reference checks clean on the
/// production path. A failure names the page, the block, and its `check.*` codes.
#[test]
fn every_documented_module_fence_checks() {
    let fences = module_fences();
    assert!(
        fences.len() >= 40,
        "expected the reference corpus, found {} module fences",
        fences.len()
    );

    let mut failures = Vec::new();
    for fence in &fences {
        let codes = check_codes(fence);
        if !codes.is_empty() {
            failures.push(format!(
                "{} fence #{} [module {}] failed check: {:?}",
                fence.doc,
                fence.index,
                fence.module_path.replace('.', "::"),
                codes,
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} documented module fence(s) fail the check gate:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

/// The gate is red-provable: a deliberately broken fence — a keyed scalar leaf,
/// the historical `check.unsupported` shape that shipped silently before this
/// gate — is caught, so a regression that reintroduces an unchecked fence cannot
/// pass green.
#[test]
fn a_broken_module_fence_is_caught() {
    let broken = ModuleFence {
        doc: "in-test".to_string(),
        index: 1,
        module_path: "broken.leaf".to_string(),
        source: "module broken::leaf\n\nresource Book {\n    required title: string\n    tags[pos: int]: string\n}\n".to_string(),
    };

    let codes = check_codes(&broken);
    assert!(
        codes.iter().any(|code| code == "check.unsupported"),
        "the gate must catch a keyed-scalar-leaf fence, got: {codes:?}",
    );
}

/// The gate must fail after checking when the independent verifier rejects the
/// compiler's image. An empty durable region is the agreement ledger's smallest
/// checker-accepted `image.flow` case.
#[test]
fn a_verifier_rejected_module_fence_is_caught() {
    let broken = ModuleFence {
        doc: "in-test".to_string(),
        index: 1,
        module_path: "broken.verify".to_string(),
        source: "module broken::verify\n\npub fn emptyRegion() {\n    transaction {\n    }\n}\n"
            .to_string(),
    };

    let codes = check_codes(&broken);
    assert!(
        codes.iter().any(|code| code == "image.flow"),
        "the gate must catch a verifier-rejected fence, got: {codes:?}",
    );
}

/// A complete source file does not need a `module` header. The two standalone
/// declarations in the values reference are moduleless scripts and belong to
/// the production-path corpus.
#[test]
fn complete_moduleless_reference_fences_are_gated() {
    let fences = module_fences();
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
