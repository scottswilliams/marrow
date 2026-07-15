//! End-to-end narrow-temporal tests (C04): `date`/`instant`/`duration` value types
//! built from canonical text literals travel the real production path (capture ->
//! compile -> encode -> verify -> VM) through the built binary, via the `temporal`
//! conformance fixture (a due-date scheduler). The language comparison order agrees
//! with the kernel key-codec byte order (pinned in `marrow-vm`'s
//! `temporal_order_agreement` test); these cases exercise the language verdicts and
//! the closed arithmetic floor.

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
        let root =
            std::env::temp_dir().join(format!("marrow-c04-{name}-{}-{nanos}", std::process::id()));
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

fn project(dir: &Path, source: &str) {
    fs::create_dir_all(dir.join("src")).expect("create src");
    fs::write(dir.join("marrow.toml"), "edition = \"2026\"\n").expect("write toml");
    fs::write(dir.join("src").join("main.mw"), source).expect("write source");
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
        .join("fixtures/v01/conformance/temporal")
}

#[test]
fn temporal_conformance_fixture_passes_on_the_production_path() {
    let output = Command::new(MARROW)
        .args(["test", "--format", "jsonl"])
        .current_dir(fixture_dir())
        .output()
        .expect("run marrow binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "temporal fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""errored":0"#), "{summary}");
    assert!(summary.contains(r#""total":10"#), "{summary}");
}

/// A malformed or out-of-range temporal literal is a compile-time `check.type`
/// diagnostic, not a runtime fault: the literal is validated and folded at compile
/// time.
#[test]
fn a_malformed_temporal_literal_is_a_check_type() {
    let bodies = [
        r#"const d: date = date("2026-13-01")"#, // impossible month
        r#"const d: date = date("2021-02-29")"#, // not a leap year
        r#"const d: date = date("0000-01-01")"#, // year below 0001
        r#"const i: instant = instant("2026-07-15T12:00:00")"#, // missing Z
        r#"const u: duration = duration("PT01S")"#, // leading-zero seconds
        r#"const u: duration = duration("-PT0S")"#, // negative zero
    ];
    for body in bodies {
        let temp = TempDir::new("bad-lit");
        project(
            &temp,
            &format!("module main\n\npub fn f(): int\n\x20   {body}\n\x20   return 0\n"),
        );
        let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{body} must fail: {stdout}");
        assert!(
            stdout.contains(r#""code":"check.type""#),
            "{body}: {stdout}"
        );
    }
}

/// A temporal constructor argument must be a static string literal; a non-literal
/// argument is a typed `check.unsupported` (there is no runtime temporal parse).
#[test]
fn a_non_literal_temporal_argument_is_a_check_unsupported() {
    let temp = TempDir::new("non-lit");
    project(
        &temp,
        "module main\n\npub fn f(s: string): date\n\x20   return date(s)\n",
    );
    let output = run_in(
        &temp,
        &["run", "f", "--format", "jsonl", "--", "2026-07-15"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.unsupported""#), "{stdout}");
}

/// The prototype's `1.second` duration-suffix literal is not in the beta floor; it
/// is a typed `check.unsupported` pointing at the canonical-text constructor.
#[test]
fn a_duration_suffix_literal_is_rejected() {
    let temp = TempDir::new("suffix");
    project(
        &temp,
        "module main\n\npub fn f(): duration\n\x20   return 1.second\n",
    );
    let output = run_in(&temp, &["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.unsupported""#), "{stdout}");
}

/// A `Map[date, V]` is admitted (temporal types are key scalars) and iterates in
/// ascending date order regardless of insertion order.
#[test]
fn a_date_keyed_map_iterates_in_date_order() {
    let temp = TempDir::new("date-map");
    project(
        &temp,
        "module main\n\
         \n\
         pub fn schedule(): Map[date, int]\n\
         \x20   var m: Map[date, int] = Map()\n\
         \x20   m = insert(m, date(\"2026-07-25\"), 2)\n\
         \x20   m = insert(m, date(\"2026-07-15\"), 1)\n\
         \x20   return m\n",
    );
    let jsonl = run_in(&temp, &["run", "schedule", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&jsonl.stdout);
    assert!(jsonl.status.success(), "{stdout}");
    // Keys render as canonical text in ascending date order (earlier date first).
    assert!(
        stdout.contains(r#""data":{"2026-07-15":1,"2026-07-25":2}"#),
        "{stdout}"
    );
}

/// A temporal export renders its result as canonical text (and JSONL string).
#[test]
fn a_temporal_result_renders_as_canonical_text() {
    let temp = TempDir::new("render");
    project(
        &temp,
        "module main\n\npub fn tomorrow(d: date): date\n\x20   return date_add_days(d, 1)\n",
    );
    let text = run_in(&temp, &["run", "tomorrow", "--", "2026-07-15"]);
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(text.status.success(), "{stdout}");
    assert!(stdout.contains("2026-07-16"), "{stdout}");

    let jsonl = run_in(
        &temp,
        &["run", "tomorrow", "--format", "jsonl", "--", "2026-07-15"],
    );
    let stdout = String::from_utf8_lossy(&jsonl.stdout);
    assert!(stdout.contains(r#""data":"2026-07-16""#), "{stdout}");
}

/// `date_add_days` past the supported range faults `run.temporal_overflow` at
/// runtime (the value is computed from arguments, not a compile-time literal).
#[test]
fn date_add_days_overflow_is_a_runtime_fault() {
    let temp = TempDir::new("overflow");
    project(
        &temp,
        "module main\n\npub fn f(d: date, n: int): date\n\x20   return date_add_days(d, n)\n",
    );
    let output = run_in(
        &temp,
        &["run", "f", "--format", "jsonl", "--", "9999-12-31", "1"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains(r#""code":"run.temporal_overflow""#),
        "{stdout}"
    );
}
