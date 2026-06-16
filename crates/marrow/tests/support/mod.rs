use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_PATH_COUNTER: AtomicU64 = AtomicU64::new(0);
pub(crate) fn json(stdout: Vec<u8>) -> serde_json::Value {
    serde_json::from_slice(&stdout).expect("json output")
}
pub(crate) fn jsonl(stdout: Vec<u8>) -> Vec<serde_json::Value> {
    let text = String::from_utf8(stdout).expect("jsonl utf8");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("jsonl record"))
        .collect()
}
pub(crate) fn json_records_in_stderr(stderr: Vec<u8>) -> Vec<serde_json::Value> {
    let text = String::from_utf8(stderr).expect("stderr utf8");
    text.lines()
        .filter(|line| line.trim_start().starts_with('{'))
        .map(|line| serde_json::from_str(line).expect("json stderr record"))
        .collect()
}
/// The diagnostic records of a `--format jsonl` run: every record except the trailing
/// summary. Asserting against parsed records, not a rendered stderr blob, keeps the oracle
/// on typed codes and payload fields. Shared by every CLI surface that reads diagnostics
/// from the structured JSONL stream.
pub(crate) fn diagnostic_records(stdout: Vec<u8>) -> Vec<serde_json::Value> {
    jsonl(stdout)
        .into_iter()
        .filter(|record| record["kind"] != "summary")
        .collect()
}
pub(crate) fn codes(records: &[serde_json::Value]) -> Vec<&str> {
    records
        .iter()
        .filter_map(|record| record["code"].as_str())
        .collect()
}

/// A process-unique temporary path under the system temp dir. The directory is
/// not created; callers that need an existing directory go through
/// [`temp_dir`]. The timestamp, pid, and process-local serial keep names from
/// colliding when tests run in parallel or in quick succession.
pub(crate) fn unique_temp_path(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let serial = TEMP_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "marrow-{name}-{}-{nanos}-{serial}",
        std::process::id()
    ))
}

/// A freshly created, process-unique temporary directory wrapped in a guard that
/// removes it on drop (including on panic).
pub(crate) fn temp_dir(name: &str) -> TempProject {
    let root = unique_temp_path(name);
    fs::create_dir_all(&root).expect("create temp dir");
    TempProject { root }
}

/// Write a single `.mw` source file at a process-unique path and return it. The
/// file lives until the test process exits; single-file cases do not need a
/// directory guard.
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

/// The adjacent `.<file_name>.*.tmp` artifacts an atomic writer leaves beside `path`.
/// Atomic `fmt --write` and `backup` publish through a sibling temp file, so this is the
/// shared oracle for "did the failed write clean up after itself".
pub(crate) fn temp_artifacts_for(path: &Path) -> Vec<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .expect("artifact file name")
        .to_string_lossy();
    let prefix = format!(".{file_name}.");
    fs::read_dir(parent)
        .expect("read artifact parent")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|entry| {
            entry
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
        })
        .collect()
}

/// Write `contents` to `root/relative`, creating parent directories.
pub(crate) fn write(root: impl AsRef<Path>, relative: &str, contents: &str) {
    let path = root.as_ref().join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

/// Invoke the `marrow` binary with the given arguments.
pub(crate) fn marrow(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .output()
        .expect("run marrow")
}

/// Invoke the `marrow` binary from a chosen working directory.
pub(crate) fn marrow_in(cwd: impl AsRef<Path>, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("run marrow")
}
pub(crate) fn backup_artifact(root: impl AsRef<Path>, file_name: &str) -> PathBuf {
    let root = root.as_ref();
    commit_catalog_if_clean(root);
    let archive = root.join(file_name);
    let output = marrow(&[
        "backup",
        root.to_str().expect("project path utf8"),
        archive.to_str().expect("backup path utf8"),
    ]);
    assert_eq!(output.status.code(), Some(0), "backup: {output:?}");
    archive
}

/// Invoke the `marrow` binary with a leading subcommand followed by `args`.
pub(crate) fn marrow_sub(cmd: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .arg(cmd)
        .args(args)
        .output()
        .expect("run marrow subcommand")
}

/// Invoke the `marrow` binary with a leading subcommand from a chosen working directory.
pub(crate) fn marrow_sub_in(cwd: impl AsRef<Path>, cmd: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .current_dir(cwd)
        .arg(cmd)
        .args(args)
        .output()
        .expect("run marrow subcommand")
}

/// Whether `token` reads as a diagnostic code: a dotted lowercase identifier with no
/// spaces, so a path fragment or a prose word never parses as the code. This is the
/// one oracle for "is this segment the dotted code" shared by every CLI surface that
/// prints `... code: message` without a structured envelope.
pub(crate) fn is_code(token: &str) -> bool {
    token.contains('.')
        && !token.contains(' ')
        && token
            .chars()
            .all(|character| character.is_ascii_lowercase() || character == '.' || character == '_')
}

/// Locate the dotted code among `": "`-delimited `segments`, returning its index and
/// value. The code is the first segment that reads as a dotted code (see [`is_code`]);
/// every segment before it is the location, and every segment after it is the message.
/// This is the one position-finding contract shared by the located and bare fault
/// grammars.
pub(crate) fn find_code_segment<'a>(segments: &[&'a str]) -> (usize, &'a str) {
    let index = segments
        .iter()
        .position(|segment| is_code(segment))
        .expect("a dotted code segment");
    (index, segments[index])
}

/// Split a `file:line:col` location into its file path and 1-based line/column. The
/// trailing two `:`-delimited fields are the numeric line and column; the rest, which
/// may itself contain `:` on some paths, is the file. Callers that need only part of
/// the result discard the fields they do not assert on.
pub(crate) fn parse_location(location: &str) -> (String, u32, u32) {
    let mut fields = location.rsplitn(3, ':');
    let column: u32 = fields
        .next()
        .expect("column field")
        .parse()
        .expect("column");
    let line: u32 = fields.next().expect("line field").parse().expect("line");
    let file = fields.next().expect("file field").to_string();
    (file, line, column)
}

/// The fault line a failed `marrow` command prints: its last non-empty stderr line. A
/// leading `std::log` stream or other preamble, if any, precedes it, so the located code
/// cannot be displaced. This is the single owner of "which stderr line is the fault" for
/// every CLI surface that selects a code or typed slots from rendered stderr.
pub(crate) fn last_fault(stderr: &[u8]) -> String {
    let text = String::from_utf8(stderr.to_vec()).expect("stderr utf8");
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .expect("a fault line")
        .to_string()
}

/// One text-mode CLI fault line parsed into its typed slots. `marrow run` and the
/// text `marrow test` report share one rendered grammar — `file:line:col: code:
/// message` when located, `code: message` when bare. A located fault carries `file`
/// and `line`; a bare fault carries only the `code`. Domain-specific unpacking (a
/// thrown-code bracket payload, a per-test outcome label) belongs in the calling test,
/// not here.
pub(crate) struct ParsedResult {
    pub(crate) file: Option<String>,
    pub(crate) line: Option<u32>,
    pub(crate) code: String,
}

/// Parse one rendered fault line into its typed slots. The code is the first
/// `": "`-delimited segment that reads as a dotted code (see [`find_code_segment`]);
/// everything before it is the `file:line:col` location, and a line whose code leads
/// with no preceding segments is bare and carries no origin. This is the one grammar
/// contract `marrow run` and `marrow test` share.
pub(crate) fn parse_result_line(line: &str) -> ParsedResult {
    let segments: Vec<&str> = line.trim().split(": ").collect();
    let (code_index, code) = find_code_segment(&segments);
    let code = code.to_string();
    if code_index == 0 {
        return ParsedResult {
            file: None,
            line: None,
            code,
        };
    }
    let location = segments[..code_index].join(": ");
    let (file, line, _column) = parse_location(&location);
    ParsedResult {
        file: Some(file),
        line: Some(line),
        code,
    }
}

/// The canonical native-store config selecting `src` and a `.data` store, with
/// no default entry. Tests that need a default entry write their own config.
pub(crate) const fn native_config() -> &'static str {
    r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#
}

/// The canonical native-store seed source: a `Counter` resource whose `seed`
/// transaction writes one record. Pairs with [`native_config`] for tests that
/// need a stored root to read back.
pub(crate) const fn counter_source() -> &'static str {
    "module app\n\
     \n\
     resource Counter\n\
     \x20\x20\x20\x20required value: int\n\
     store ^counter(id: int): Counter\n\
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
pub(crate) fn temp_project(name: &str, build: impl FnOnce(&Path)) -> TempProject {
    let project = temp_dir(name);
    build(&project.root);
    commit_catalog_if_clean(&project.root);
    project
}

/// Build a project in a self-cleaning temp directory without committing its
/// catalog, leaving any proposed durable identity pending.
pub(crate) fn temp_project_uncommitted(name: &str, build: impl FnOnce(&Path)) -> TempProject {
    let project = temp_dir(name);
    build(&project.root);
    project
}

/// Freeze a fixture project's pending durable identity through the store transaction,
/// then render the committed catalog file the way a state-establishing run does. A
/// project that does not check cleanly, proposes no catalog change, or configures no
/// durable store is left untouched.
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
    let Some(store_path) = native_store_path(root, &config) else {
        return;
    };
    fs::create_dir_all(store_path.parent().expect("store parent")).expect("create data dir");
    let store = marrow_store::tree::TreeStore::open(&store_path).expect("open fixture store");
    store
        .write_store_uid(
            &marrow_store::tree::StoreUid::new(
                "store_00000000000000000000000000000001".to_string(),
            )
            .expect("valid fixture store uid"),
        )
        .expect("write fixture store uid");
    marrow_run::evolution::commit_catalog_baseline(&store, &program)
        .expect("commit fixture catalog baseline");
    if let Some(snapshot) = store.read_catalog_snapshot().expect("read fixture catalog") {
        fs::write(
            root.join("marrow.catalog.json"),
            snapshot.to_json_pretty().expect("catalog renders"),
        )
        .expect("render fixture catalog");
    }
}

/// The native store file path a project's config selects, or `None` for an explicit
/// memory store. Mirrors the CLI's own resolution so fixtures and the binary agree on where
/// the store lives.
pub(crate) fn native_store_path(
    root: &Path,
    config: &marrow_project::ProjectConfig,
) -> Option<PathBuf> {
    marrow_check::native_store_path(root, config).expect("valid fixture store config")
}

/// The accepted store catalog id of a checked root member, addressed by name. CLI
/// tests that write cells under the live store resolve member ids through the same
/// checked facts the runtime uses, never by spelling the id.
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
