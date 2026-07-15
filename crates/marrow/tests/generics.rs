//! End-to-end rank-1 generic function tests: generic definitions travel the real
//! production path (capture -> compile -> monomorphize -> encode -> verify -> VM)
//! through the built binary, via the `generics` conformance fixture and inline
//! projects that run a monomorphized generic and assert its rendered result.

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
            "marrow-c03-generics-{name}-{}-{nanos}",
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
    conformance_dir("generics")
}

fn conformance_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .join("fixtures/v01/conformance")
        .join(name)
}

/// The generic value-types conformance fixture passes end to end: user generic
/// `struct`/`enum` construction, field access, `match`, constrained instantiation,
/// nesting, generic-typed function parameters, and the refounded `Option`/`Result`
/// (ordinary generic enums) all report `passed` through the production path.
#[test]
fn generic_types_conformance_fixture_passes_on_the_production_path() {
    let output = Command::new(MARROW)
        .args(["test", "--format", "jsonl"])
        .current_dir(conformance_dir("generic_types"))
        .output()
        .expect("run marrow binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "generic_types fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":10"#), "{summary}");
}

/// The generics conformance fixture passes end to end: identity, first, swap-style
/// construction, and the `supports equality`/`supports order` constrained helpers
/// all report `passed` through the production path, each call monomorphized.
#[test]
fn generics_conformance_fixture_passes_on_the_production_path() {
    let output = Command::new(MARROW)
        .args(["test", "--format", "jsonl"])
        .current_dir(fixture_dir())
        .output()
        .expect("run marrow binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "generics fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":7"#), "{summary}");
}

/// A monomorphized generic runs through the VM: a `pub` export calls a generic
/// helper at a concrete type and returns its result, rendered by `marrow run`.
#[test]
fn a_monomorphized_generic_runs_through_the_vm() {
    let temp = TempDir::new("run");
    project(
        &temp,
        "module main\n\
         \n\
         fn firstOr[T](xs: List[T], fallback: T): T\n\
         \x20   for x in xs\n\
         \x20       return x\n\
         \x20   return fallback\n\
         \n\
         pub fn head(): int\n\
         \x20   var xs: List[int] = List()\n\
         \x20   xs = append(xs, 11)\n\
         \x20   xs = append(xs, 22)\n\
         \x20   return firstOr(xs, 0)\n",
    );
    let output = run_in(&temp, &["run", "head", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":11"#), "{stdout}");
}

/// A `pub` generic function is not itself an invocable export: it has no single
/// image entry, so `marrow run` on its name fails to resolve, evidencing that
/// monomorphized instances carry no stable export identity.
#[test]
fn a_generic_function_is_not_an_export() {
    let temp = TempDir::new("no-export");
    project(
        &temp,
        "module main\n\
         \n\
         pub fn identity[T](x: T): T\n\
         \x20   return x\n\
         \n\
         pub fn concrete(): int\n\
         \x20   return identity(1)\n",
    );
    let output = run_in(&temp, &["run", "identity", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "a generic function is not a runnable export: {stdout}"
    );
    // The monomorphic entry that calls it does run.
    let concrete = run_in(&temp, &["run", "concrete", "--format", "jsonl"]);
    assert!(
        concrete.status.success(),
        "{}",
        String::from_utf8_lossy(&concrete.stdout)
    );
}
