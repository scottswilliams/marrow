use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[allow(dead_code)]
pub(crate) fn json(stdout: Vec<u8>) -> serde_json::Value {
    serde_json::from_slice(&stdout).expect("json output")
}

#[allow(dead_code)]
pub(crate) fn jsonl(stdout: Vec<u8>) -> Vec<serde_json::Value> {
    let text = String::from_utf8(stdout).expect("jsonl utf8");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("jsonl record"))
        .collect()
}

#[allow(dead_code)]
pub(crate) fn codes(records: &[serde_json::Value]) -> Vec<&str> {
    records
        .iter()
        .filter_map(|record| record["code"].as_str())
        .collect()
}

/// A process-unique temporary path under the system temp dir. The directory is
/// not created; callers that need an existing directory go through
/// [`temp_dir`]. The timestamp and pid keep names from colliding when tests run
/// in parallel or in quick succession.
#[allow(dead_code)]
pub(crate) fn unique_temp_path(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()))
}

/// A freshly created, process-unique temporary directory wrapped in a guard that
/// removes it on drop (including on panic).
#[allow(dead_code)]
pub(crate) fn temp_dir(name: &str) -> TempProject {
    let root = unique_temp_path(name);
    fs::create_dir_all(&root).expect("create temp dir");
    TempProject { root }
}

/// Write a single `.mw` source file at a process-unique path and return it. The
/// file lives until the test process exits; single-file cases do not need a
/// directory guard.
#[allow(dead_code)]
pub(crate) fn temp_source(name: &str, source: &str) -> PathBuf {
    let stem = unique_temp_path(name);
    // Append rather than replace the extension: a `name` containing a dot must
    // not truncate the unique stem the way `with_extension` would.
    let path = stem.with_file_name(format!(
        "{}.mw",
        stem.file_name().expect("temp stem").to_string_lossy()
    ));
    fs::write(&path, source).expect("write source");
    path
}

/// Write `contents` to `root/relative`, creating parent directories.
#[allow(dead_code)]
pub(crate) fn write(root: impl AsRef<Path>, relative: &str, contents: &str) {
    let path = root.as_ref().join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

/// Invoke the `marrow` binary with the given arguments.
#[allow(dead_code)]
pub(crate) fn marrow(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .output()
        .expect("run marrow")
}

/// Invoke the `marrow` binary with a leading subcommand followed by `args`.
#[allow(dead_code)]
pub(crate) fn marrow_sub(cmd: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg(cmd)
        .args(args)
        .output()
        .expect("run marrow subcommand")
}

/// The canonical native-store config selecting `src` and a `.data` store, with
/// no default entry. Tests that need a default entry write their own config.
#[allow(dead_code)]
pub(crate) const fn native_config() -> &'static str {
    r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#
}

/// The canonical native-store seed source: a `Counter` resource whose `seed`
/// transaction writes one record. Pairs with [`native_config`] for tests that
/// need a stored root to read back.
#[allow(dead_code)]
pub(crate) const fn counter_source() -> &'static str {
    "module app\n\
     \n\
     resource Counter at ^counter(id: int)\n\
     \x20\x20\x20\x20required value: int\n\
     \n\
     pub fn seed()\n\
     \x20\x20\x20\x20var c: Counter\n\
     \x20\x20\x20\x20c.value = 42\n\
     \x20\x20\x20\x20transaction\n\
     \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n"
}

/// A temporary project directory that removes itself on drop, including on a
/// panicking test. It derefs to its root [`Path`], so `.to_str()`, `.join(..)`,
/// and other path methods work directly on the guard.
pub(crate) struct TempProject {
    root: PathBuf,
}

impl TempProject {
    #[allow(dead_code)]
    pub(crate) fn path(&self) -> &Path {
        &self.root
    }
}

impl Deref for TempProject {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.root
    }
}

impl AsRef<Path> for TempProject {
    fn as_ref(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

/// Build a project in a self-cleaning temp directory and freeze its pending
/// catalog through the production writer, so read-only commands see a committed
/// catalog. Use [`temp_project_uncommitted`] when a test must observe the
/// pending state before a flow commits it.
#[allow(dead_code)]
pub(crate) fn temp_project(name: &str, build: impl FnOnce(&Path)) -> TempProject {
    let project = temp_dir(name);
    build(&project.root);
    commit_catalog_if_clean(&project.root);
    project
}

/// Build a project in a self-cleaning temp directory without committing its
/// catalog, leaving any proposed durable identity pending.
#[allow(dead_code)]
pub(crate) fn temp_project_uncommitted(name: &str, build: impl FnOnce(&Path)) -> TempProject {
    let project = temp_dir(name);
    build(&project.root);
    project
}

/// Freeze a fixture project's pending durable identity through the one production
/// catalog writer, so read-only commands (`data`, `serve`) and store-backed runs see
/// a committed catalog without re-implementing the write. A project that does not
/// check cleanly, or proposes no catalog change, is left untouched.
#[allow(dead_code)]
pub(crate) fn commit_catalog_if_clean(root: impl AsRef<Path>) {
    let root = root.as_ref();
    let Ok(config_text) = fs::read_to_string(root.join("marrow.json")) else {
        return;
    };
    let Ok(config) = marrow_project::parse_config(&config_text) else {
        return;
    };
    let Ok((report, program)) = marrow_check::check_project(root, &config) else {
        return;
    };
    if report.has_errors() {
        return;
    }
    if let Some((report, _program)) = marrow_check::commit_pending_identity(root, &config, &program)
        .expect("commit fixture catalog")
    {
        assert!(
            !report.has_errors(),
            "committed fixture catalog must check cleanly: {:#?}",
            report.diagnostics
        );
    }
}

/// The accepted store catalog id of a checked root member, addressed by name. CLI
/// tests that write cells under the live store resolve member ids through the same
/// checked facts the runtime uses, never by spelling the id.
#[allow(dead_code)]
pub(crate) fn member_catalog_id(
    members: &[marrow_check::CheckedSavedMember],
    name: &str,
) -> marrow_store::cell::CatalogId {
    let member = members
        .iter()
        .find(|member| member.name == name)
        .expect("checked member");
    marrow_store::cell::CatalogId::new(
        member
            .catalog_id
            .clone()
            .expect("accepted member catalog id"),
    )
    .expect("member catalog id")
}
