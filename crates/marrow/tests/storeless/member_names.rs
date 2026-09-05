//! `marrow check` and `marrow run` agree on a repeated parameter name: the
//! declaration is refused with a located `check.name_conflict` before any image is
//! built, so `f(1, 2)` never executes with its second slot standing in for `a`.

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
            "marrow-member-names-{name}-{}-{nanos}",
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

const REPEATED_PARAMETER: &str = "module main\n\n\
fn f(a: int, a: int): int {\n    return a\n}\n\n\
pub fn main(): int {\n    return f(1, 2)\n}\n";

#[test]
fn check_and_run_refuse_a_repeated_parameter_at_its_name() {
    let temp = TempDir::new("repeated-parameter");
    write(&temp.join("marrow.toml"), "edition = \"2026\"\n");
    write(&temp.join("src").join("main.mw"), REPEATED_PARAMETER);

    let check = run_in(&temp, &["check"]);
    let report = String::from_utf8_lossy(&check.stderr);
    assert!(
        !check.status.success(),
        "`marrow check` accepted a repeated parameter: {report}"
    );
    assert!(
        report.contains("src/main.mw:3:14: check.name_conflict"),
        "`marrow check` locates the conflict at the second `a`: {report}"
    );

    let run = run_in(&temp, &["run", "main", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        !run.status.success(),
        "`marrow run` executed a repeated parameter: {stdout}"
    );
    assert!(
        stdout.contains(r#""code":"check.name_conflict""#)
            && stdout.contains(r#""span":{"column":14,"line":3}"#),
        "`marrow run` refuses at the same span, before any image is verified: {stdout}"
    );
    assert!(
        !stdout.contains("image.table"),
        "the repeat never reaches the verifier: {stdout}"
    );
}
