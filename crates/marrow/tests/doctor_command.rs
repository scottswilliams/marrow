//! `marrow doctor` through the built binary: the terminal compiles the project, spawns the
//! release-verified companion, and relays its report and exit code.
//!
//! The companion layout is staged once into a private directory — the `marrow` and
//! `marrow-runner` binaries beside a `marrow-companions` manifest — because the terminal
//! locates the runner only beside itself. Provisioning and population go through the
//! staged runner and `marrow import`; neither binds a socket, so the whole suite runs
//! inside the sandbox. The stock runner is built into the same directory as the CLI test
//! binary by a workspace build.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MARROW: &str = env!("CARGO_BIN_EXE_marrow");

const SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn readValue(n: int): int {
    return ^counters[n].value ?? 0
}
"#;

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

/// The staged toolchain: a private directory holding `marrow`, `marrow-runner`, and the
/// manifest naming the runner by its release identity. Staged once per test binary.
fn toolchain() -> TempDir {
    let runner = Path::new(MARROW)
        .parent()
        .expect("binary dir")
        .join("marrow-runner");
    assert!(
        runner.is_file(),
        "stock runner not built at {}; run a workspace build first",
        runner.display()
    );
    let dir = TempDir::new("toolchain");
    fs::copy(MARROW, dir.root.join("marrow")).expect("copy marrow");
    fs::copy(&runner, dir.root.join("marrow-runner")).expect("copy runner");
    let bytes = fs::read(&runner).expect("read runner");
    let id = marrow_image::companion_release_id(&bytes).to_hex();
    fs::write(
        dir.root.join("marrow-companions"),
        format!(
            "marrow companions v0\nrelease {}\nrunner marrow-runner {id}\nend\n",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .expect("write manifest");
    dir
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos()
}

struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "marrow-doctor-{name}-{}-{}",
            std::process::id(),
            nanos()
        ));
        fs::create_dir(&root).expect("create temp dir");
        TempDir { root }
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

/// A durable project at `dir` with its ledger, and a provisioned store beside it
/// populated with two counters through `marrow import`.
fn project_with_store(toolchain: &Path, temp: &TempDir) -> (PathBuf, PathBuf) {
    let project = temp.root.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    write(&project.join("src/main.mw"), SOURCE);
    write(&project.join(".marrow/ids"), IDS);
    write(
        &project.join("seed.jsonl"),
        "{\"id\":1,\"value\":10,\"label\":\"ten\"}\n{\"id\":2,\"value\":20}\n",
    );
    let store = temp.root.join("store");
    let imported = marrow(
        toolchain,
        &project,
        &[
            "import",
            "--store",
            store.to_str().expect("store path"),
            "--jsonl",
            "seed.jsonl",
            "--root",
            "counters",
            "--keys",
            "id",
        ],
    );
    assert!(
        imported.status.success(),
        "import failed: {}",
        String::from_utf8_lossy(&imported.stderr)
    );
    (project, store)
}

fn marrow(toolchain: &Path, dir: &Path, args: &[&str]) -> Output {
    Command::new(toolchain.join("marrow"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("run the staged marrow binary")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn a_clean_store_audits_with_a_stable_digest_and_exit_zero(toolchain: &Path) {
    let temp = TempDir::new("clean");
    let (project, store) = project_with_store(toolchain, &temp);
    let store_arg = store.to_str().expect("store path");

    let first = marrow(toolchain, &project, &["doctor", "--store", store_arg]);
    assert!(first.status.success(), "{}", text(&first.stderr));
    let out = text(&first.stdout);
    assert!(
        out.starts_with(&format!("Logical store audit: {store_arg}\n")),
        "{out}"
    );
    assert!(
        out.contains("Physical integrity was not checked.\n"),
        "{out}"
    );
    assert!(out.contains("entries 2, index cells 0, cells 6\n"), "{out}");
    assert!(out.contains("\nno findings\n"), "{out}");
    let digest = out
        .lines()
        .find_map(|line| line.strip_prefix("digest "))
        .expect("the report names the digest")
        .to_string();
    assert_eq!(digest.len(), 64);

    let second = marrow(
        toolchain,
        &project,
        &["doctor", "--store", store_arg, "--format", "jsonl"],
    );
    assert!(second.status.success(), "{}", text(&second.stderr));
    let out = text(&second.stdout);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1, "{out}");
    assert!(lines[0].starts_with("{\"cells\":6,\"digest\":\""), "{out}");
    assert!(
        lines[0].contains(&format!("\"digest\":\"{digest}\"")),
        "{out}"
    );
    assert!(
        lines[0].contains("\"entries\":2,\"findings\":0,\"image\":\""),
        "{out}"
    );
    assert!(
        lines[0].ends_with(&format!(
            "\"index_cells\":0,\"instance\":\"{}\",\"kind\":\"doctor\",\"listed\":0,\"outcome\":\"clean\",\"physical_integrity\":\"not_checked\",\"scope\":\"logical\",\"store\":\"{store_arg}\"}}",
            instance_of(lines[0])
        )),
        "{out}"
    );
}

fn instance_of(line: &str) -> &str {
    let start = line.find("\"instance\":\"").expect("instance") + "\"instance\":\"".len();
    &line[start..start + 32]
}

#[test]
#[ignore = "OPEN B4 physical-integrity obligation: logical doctor does not verify engine checksums"]
fn an_altered_engine_is_reported_as_corruption_with_exit_one() {
    let staged = toolchain();
    let toolchain = &staged.root;
    let temp = TempDir::new("flip");
    let (project, store) = project_with_store(toolchain, &temp);
    let engine = store.join("store.redb");
    let mut bytes = fs::read(&engine).expect("read engine");
    let at = bytes
        .windows(3)
        .position(|window| window == b"ten")
        .expect("the stored label is in the engine file");
    bytes[at] = b'T';
    fs::write(&engine, bytes).expect("write engine");

    let output = marrow(
        toolchain,
        &project,
        &["doctor", "--store", store.to_str().expect("store path")],
    );
    assert_eq!(output.status.code(), Some(1));
    let out = text(&output.stdout);
    assert!(out.contains("\nstore.corruption: "), "{out}");
}

fn a_code_only_edit_must_be_rebound_before_it_audits(toolchain: &Path) {
    let temp = TempDir::new("stale");
    let (project, store) = project_with_store(toolchain, &temp);
    write(
        &project.join("src/main.mw"),
        &SOURCE.replace("?? 0", "?? 1"),
    );
    let output = marrow(
        toolchain,
        &project,
        &["doctor", "--store", store.to_str().expect("store path")],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).starts_with("store.image_not_active: "),
        "{}",
        text(&output.stderr)
    );

    let output = marrow(
        toolchain,
        &project,
        &[
            "doctor",
            "--store",
            store.to_str().expect("store path"),
            "--format",
            "jsonl",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stdout).starts_with(
            "{\"code\":\"store.image_not_active\",\"kind\":\"doctor\",\"outcome\":\"error\",\"store\":\""
        ),
        "{}",
        text(&output.stdout)
    );
}

fn usage_and_absent_store_refusals_keep_their_codes(toolchain: &Path) {
    let temp = TempDir::new("usage");
    let (project, _) = project_with_store(toolchain, &temp);
    let output = marrow(toolchain, &project, &["doctor"]);
    assert_eq!(output.status.code(), Some(2));
    let output = marrow(
        toolchain,
        &project,
        &["doctor", "--store", "nowhere", "--format", "yaml"],
    );
    assert_eq!(output.status.code(), Some(2));
    let output = marrow(toolchain, &project, &["doctor", "--store", "nowhere"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).starts_with("store.io: "),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn a_compiler_resource_limit_keeps_its_typed_code_before_store_access() {
    let temp = TempDir::new("compiler-limit");
    let project = temp.root.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    let mut source = String::from("module main\n\n");
    for i in 0..257 {
        source.push_str(&format!("pub fn f{i}(): int {{\n    return 0\n}}\n\n"));
    }
    write(&project.join("src/main.mw"), &source);

    for format in ["text", "jsonl"] {
        let output = Command::new(MARROW)
            .args(["doctor", "--store", "nowhere", "--format", format])
            .current_dir(&project)
            .env("NO_COLOR", "1")
            .output()
            .expect("run doctor over the over-limit project");
        assert_eq!(output.status.code(), Some(1));
        let error = text(&output.stderr);
        assert!(
            error.starts_with(&format!(
                "{}: ",
                marrow_codes::Code::CliCompilerResourceLimit.as_str()
            )),
            "{format}: {error}"
        );
        assert!(output.stdout.is_empty(), "no audit report was produced");
        assert!(!project.join("nowhere").exists());
    }
}

fn a_storeless_program_has_nothing_to_audit(toolchain: &Path) {
    let temp = TempDir::new("storeless");
    let project = temp.root.join("app");
    write(&project.join("marrow.toml"), "edition = \"2026\"\n");
    write(
        &project.join("src/main.mw"),
        "pub fn answer(): int {\n    return 42\n}\n",
    );
    let output = marrow(toolchain, &project, &["doctor", "--store", "nowhere"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).starts_with("cli.durable_unsupported: "),
        "{}",
        text(&output.stderr)
    );
}

fn an_invalid_scalar_reports_a_logical_finding(toolchain: &Path) {
    let temp = TempDir::new("invalid-scalar");
    let (project, store) = project_with_store(toolchain, &temp);
    let engine = store.join("store.redb");
    let mut bytes = fs::read(&engine).expect("read engine");
    let at = bytes
        .windows(3)
        .position(|window| window == b"ten")
        .expect("stored label");
    bytes[at] = 0xff;
    fs::write(&engine, &bytes).expect("write malformed UTF-8");
    let code = marrow_codes::Code::StoreAuditUndecodable.as_str();
    let place = "^counters[1].label";
    for format in ["text", "jsonl"] {
        let output = marrow(
            toolchain,
            &project,
            &[
                "doctor",
                "--store",
                store.to_str().expect("store path"),
                "--format",
                format,
            ],
        );
        assert_eq!(output.status.code(), Some(1));
        let out = text(&output.stdout);
        if format == "text" {
            assert!(out.contains(&format!("  {code} at {place}\n")), "{out}");
            assert!(
                out.contains("Physical integrity was not checked.\n"),
                "{out}"
            );
        } else {
            let lines: Vec<_> = out.lines().collect();
            assert_eq!(lines.len(), 2, "{out}");
            assert!(lines[0].contains("\"outcome\":\"findings\""), "{out}");
            assert!(lines[0].contains("\"scope\":\"logical\""), "{out}");
            assert!(
                lines[0].contains("\"physical_integrity\":\"not_checked\""),
                "{out}"
            );
            assert_eq!(
                lines[1],
                format!("{{\"code\":\"{code}\",\"kind\":\"finding\",\"place\":\"{place}\"}}")
            );
        }
        assert_eq!(fs::read(&engine).expect("unchanged engine"), bytes);
    }
}

#[test]
fn doctor_reports_and_refusals_share_one_owned_toolchain() {
    let staged = toolchain();
    let path = staged.root.clone();
    a_clean_store_audits_with_a_stable_digest_and_exit_zero(&path);
    a_code_only_edit_must_be_rebound_before_it_audits(&path);
    usage_and_absent_store_refusals_keep_their_codes(&path);
    a_storeless_program_has_nothing_to_audit(&path);
    an_invalid_scalar_reports_a_logical_finding(&path);
    drop(staged);
    assert!(!path.exists(), "the suite removes its staged toolchain");
}
