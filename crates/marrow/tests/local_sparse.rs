//! End-to-end local-sparse-product tests: `resource` locals with field
//! assignment, `unset`, and an `Option`-typed sparse field travel the real
//! production path (capture -> compile -> encode -> verify -> VM) through the
//! built binary, via the `local_sparse` conformance fixture and inline
//! invalid-source projects asserting typed diagnostics.

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
            "marrow-c02-sparse-{name}-{}-{nanos}",
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

fn project(dir: &Path, source: &str) {
    write(&dir.join("marrow.toml"), "edition = \"2026\"\n");
    write(&dir.join("src").join("main.mw"), source);
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(MARROW)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run marrow binary")
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .join("fixtures/v01/conformance/local_sparse")
}

/// The local-sparse conformance fixture passes end to end: a fresh sparse field
/// reads absent, assignment sets it present, `unset` clears it, value/copy
/// semantics are independent, and an `Option[string]` sparse field keeps absent
/// distinct from a present `Option` none.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn local_sparse_conformance_fixture_passes_on_the_production_path() {
    let output = Command::new(MARROW)
        .args(["test", "--format", "jsonl"])
        .current_dir(fixture_dir())
        .output()
        .expect("run marrow binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "local_sparse fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":9"#), "{summary}");
}

/// A sparse field assignment flows through the VM: an export that builds a record,
/// assigns a sparse field, and reads it back yields the assigned value.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_sparse_field_assignment_flows_through_the_vm() {
    let temp = TempDir::new("assign");
    project(
        &temp,
        "resource Box\n\
         \x20   required id: int\n\
         \x20   note: string\n\
         \n\
         pub fn f(): string\n\
         \x20   var b = Box(id: 1)\n\
         \x20   b.note = \"hi\"\n\
         \x20   return b.note ?? \"absent\"\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":"hi""#), "{stdout}");
}

/// `unset` clears a present sparse field back to absent, observed through the VM.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn unset_clears_a_sparse_field_through_the_vm() {
    let temp = TempDir::new("unset");
    project(
        &temp,
        "resource Box\n\
         \x20   required id: int\n\
         \x20   note: string\n\
         \n\
         pub fn f(): string\n\
         \x20   var b = Box(id: 1, note: \"hi\")\n\
         \x20   unset b.note\n\
         \x20   return b.note ?? \"absent\"\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":"absent""#), "{stdout}");
}

/// A required field cannot be unset: it is a typed `check.type` at the field.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn unsetting_a_required_field_is_a_check_type_diagnostic() {
    let temp = TempDir::new("required-unset");
    project(
        &temp,
        "resource Box\n\
         \x20   required id: int\n\
         \n\
         pub fn f(): int\n\
         \x20   var b = Box(id: 1)\n\
         \x20   unset b.id\n\
         \x20   return 0\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.type""#), "{stdout}");
}

/// `unset` on a durable place is rejected: durable erasure uses `delete`.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn unsetting_a_durable_place_is_a_check_type_diagnostic() {
    let temp = TempDir::new("durable-unset");
    project(
        &temp,
        "resource Box\n\
         \x20   required id: int\n\
         \x20   note: string\n\
         \n\
         store ^boxes(id: int): Box\n\
         \n\
         pub fn f(k: int)\n\
         \x20   transaction\n\
         \x20       unset ^boxes(k).note\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl", "--", "1"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.type""#), "{stdout}");
}

/// A resource carrying an `Option` field is identity-complete: the store is
/// accepted and its durable identities are minted, including the sum and member ids
/// of the `Option[string]` reachable through it (`Option` is a closed enum). The
/// `Option`-valued field is not part of the kernel-executable flat scalar record, so
/// a durable operation over the store is a precise `check.unsupported`; it is no
/// longer a `check.type` on the declaration.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_store_over_an_option_field_resource_is_identity_complete() {
    let temp = TempDir::new("store-option");
    project(
        &temp,
        "resource Box\n\
         \x20   required id: int\n\
         \x20   tag: Option[string]\n\
         \n\
         store ^boxes(id: int): Box\n\
         \n\
         pub fn f(): int\n\
         \x20   return 0\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":0"#), "{stdout}");
    let ids = std::fs::read_to_string(temp.join("marrow.ids")).expect("marrow.ids written");
    assert!(ids.contains("sum Option[string] "), "{ids}");
    assert!(ids.contains("member Option[string].none "), "{ids}");
    assert!(ids.contains("member Option[string].some "), "{ids}");
}

/// An `Option[string]` sparse field keeps three states distinct through the VM:
/// absent, a present `Option` none, and a present `Option` some. No dedicated
/// absent runtime value is needed — vacancy is one representation, a present none
/// another.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn an_option_typed_sparse_field_keeps_absent_and_present_none_distinct() {
    let temp = TempDir::new("option-field");
    project(
        &temp,
        "resource Box\n\
         \x20   required id: int\n\
         \x20   tag: Option[string]\n\
         \n\
         pub fn classify(mode: int): string\n\
         \x20   var b = Box(id: 1)\n\
         \x20   if mode == 1\n\
         \x20       b.tag = none\n\
         \x20   if mode == 2\n\
         \x20       b.tag = some(\"hi\")\n\
         \x20   if const t = b.tag\n\
         \x20       match t\n\
         \x20           none\n\
         \x20               return \"present-none\"\n\
         \x20           some(v)\n\
         \x20               return v\n\
         \x20   return \"absent\"\n",
    );
    for (mode, expected) in [(0, "absent"), (1, "present-none"), (2, "hi")] {
        let output = run_in(
            &temp,
            &[
                "run",
                "classify",
                "--format",
                "jsonl",
                "--",
                &mode.to_string(),
            ],
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "mode {mode}: {stdout}");
        assert!(
            stdout.contains(&format!(r#""data":"{expected}""#)),
            "mode {mode}: {stdout}"
        );
    }
}

/// Assigning to a field of a `const`-bound record is rejected: the binding is
/// immutable, so the field cannot be reassigned.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn assigning_a_field_of_a_const_record_is_a_check_type_diagnostic() {
    let temp = TempDir::new("const-field");
    project(
        &temp,
        "resource Box\n\
         \x20   required id: int\n\
         \x20   note: string\n\
         \n\
         pub fn f(): int\n\
         \x20   const b = Box(id: 1)\n\
         \x20   b.note = \"x\"\n\
         \x20   return 0\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.type""#), "{stdout}");
}
