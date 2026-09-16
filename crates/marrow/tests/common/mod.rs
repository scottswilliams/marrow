#![allow(dead_code)]
//! The shared `.mw` fixture harness for the `marrow` crate's integration suites.
//!
//! A [`Project`] is an in-memory project — manifest, optional `.marrow/ids`, source
//! files — built inline or loaded from `crates/marrow/tests/fixtures/v01/<name>/`.
//! Drive it through the library path ([`Project::image`], [`Project::try_image`],
//! [`Project::session`]) or the CLI path ([`Project::materialize`] ->
//! [`Workspace::marrow`], or the one-shot [`Project::run_cli`]). Assertions read typed
//! outcomes, never rendered prose: a [`CallOutcome`] fault carries the stable
//! `marrow-codes` string and [`Diagnostics`] carries `(code, line, column)`.
//!
//! Every durable fixture ships a complete fixed-hex `.marrow/ids` with `high-water 0`,
//! covering every declaration that mints a row; omitting one fails the build. Only
//! `marrow run` mints, and it draws from OS entropy and rewrites the ledger, which
//! would make a fixture nondeterministic — every other path reports
//! `check.durable_identity` instead.

use std::borrow::Cow;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use marrow_codes::Code;
use marrow_compile::{CompileFailure, Compiled, SourceDiagnostic, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput, capture};
use marrow_verify::{VerifiedImage, verify};
use marrow_vm::{
    DurableRun, MemoryAttachment, MintOutcome, Value, mint_ephemeral, prepare, run_export,
};

/// The built `marrow` binary under test.
pub const MARROW_BIN: &str = env!("CARGO_BIN_EXE_marrow");

/// The default manifest: the sole supported edition, nothing else.
pub const DEFAULT_MANIFEST: &str = "edition = \"2026\"\n";

/// An identity ledger declaring no durable anchors, for a storeless project that
/// still needs an explicit `.marrow/ids` on disk.
pub const EMPTY_IDS: &str =
    "marrow ids v0\nmachine-written by marrow; do not edit\nhigh-water 0\nend\n";

/// The captured-project bounds every driver uses. The production defaults; a fixture
/// never needs to widen them.
const LIMITS: CaptureLimits = CaptureLimits::DEFAULT;

// ---------------------------------------------------------------------------
// Project scaffolding
// ---------------------------------------------------------------------------

/// An in-memory Marrow project: a manifest, an optional identity ledger, and the
/// source files. Build it inline or load it from an on-disk fixture, then drive it
/// through the library or CLI path.
#[derive(Clone)]
pub struct Project {
    manifest: Vec<u8>,
    ids: Option<Vec<u8>>,
    files: Vec<(String, Vec<u8>)>,
}

impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}

impl Project {
    /// A project with the default manifest, no identity ledger, and no source files.
    pub fn new() -> Self {
        Self {
            manifest: DEFAULT_MANIFEST.as_bytes().to_vec(),
            ids: None,
            files: Vec::new(),
        }
    }

    /// A single-source project with `source` at `src/main.mw`.
    pub fn single(source: &str) -> Self {
        Self::new().source("src/main.mw", source)
    }

    /// Load a project from `crates/marrow/tests/fixtures/v01/<name>/`: `marrow.toml`
    /// (required), `.marrow/ids` (optional), and every file under `src/` keyed by its
    /// `src`-relative path.
    pub fn from_fixture(name: &str) -> Self {
        let root = fixtures_root().join(name);
        let manifest = fs::read(root.join("marrow.toml"))
            .unwrap_or_else(|error| panic!("read fixture `{name}` marrow.toml: {error}"));
        let ids = match fs::read(root.join(".marrow/ids")) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("read fixture `{name}` .marrow/ids: {error}"),
        };
        let mut files = Vec::new();
        let src = root.join("src");
        collect_sources(&src, &src, &mut files);
        assert!(
            !files.is_empty(),
            "fixture `{name}` has no `src` source files"
        );
        files.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            manifest,
            ids,
            files,
        }
    }

    pub fn manifest(mut self, manifest: &str) -> Self {
        self.manifest = manifest.as_bytes().to_vec();
        self
    }

    /// Set the identity ledger (the `.marrow/ids` artifact). See the module doc's
    /// ids-minting trap: a durable project needs a complete ledger.
    pub fn ids(mut self, ids: &str) -> Self {
        self.ids = Some(ids.as_bytes().to_vec());
        self
    }

    /// Add or replace a source file at the `src`-relative canonical path `path`
    /// (for example `src/bookstore.mw`).
    pub fn source(mut self, path: &str, source: &str) -> Self {
        let path = path.to_string();
        let bytes = source.as_bytes().to_vec();
        if let Some(slot) = self
            .files
            .iter_mut()
            .find(|(existing, _)| *existing == path)
        {
            slot.1 = bytes;
        } else {
            self.files.push((path, bytes));
        }
        self
    }

    // --- library path ---

    /// Capture, compile, and verify through the production path, panicking with the
    /// diagnostic codes on any source-diagnostic failure.
    pub fn image(&self) -> VerifiedImage {
        self.try_image().unwrap_or_else(|diagnostics| {
            panic!("project did not compile: {:?}", diagnostics.all())
        })
    }

    /// Capture, compile, and verify, returning typed [`Diagnostics`] on a
    /// source-diagnostic failure. A non-diagnostic compile failure (an aggregate
    /// resource limit or a compiler invariant) and a verifier rejection panic — a
    /// fixture asserting a diagnostic wants the diagnostic path, and the others name
    /// a malformed fixture or a compiler defect.
    pub fn try_image(&self) -> Result<VerifiedImage, Diagnostics> {
        let project = self.capture();
        match compile(&project) {
            Ok(compiled) => Ok(verify(&compiled.image.bytes).expect("verify a compiled image")),
            Err(CompileFailure::Diagnostics(diagnostics)) => Err(Diagnostics {
                diagnostics: diagnostics.as_slice().to_vec(),
            }),
            Err(other) => panic!("compilation failed without source diagnostics: {other}"),
        }
    }

    /// Capture and compile without verifying, for a suite that pins the compiled image's
    /// bytes or identity directly.
    pub fn compiled(&self) -> Compiled {
        compile(&self.capture()).expect("project compiles")
    }

    /// Open a persistent ephemeral-memory session: compile, verify, and (for a
    /// durable project) mint one attachment that serves every export call in
    /// sequence.
    pub fn session(&self) -> Session {
        let image = self.image();
        let attachment = if image.roots().is_empty() {
            None
        } else {
            match mint_ephemeral(prepare(image.clone())).into_mint() {
                MintOutcome::Ready(attachment) => Some(attachment),
                MintOutcome::Storeless | MintOutcome::Parked => {
                    panic!("durable shape is not executable by the ephemeral kernel")
                }
                MintOutcome::Failed(cause) => {
                    panic!("minting the attachment failed: {}", cause.as_str())
                }
            }
        };
        Session { image, attachment }
    }

    fn capture(&self) -> ProjectInput {
        let manifest =
            Manifest::parse(std::str::from_utf8(&self.manifest).expect("utf-8 manifest"))
                .expect("parse manifest");
        let files = self
            .files
            .iter()
            .map(|(path, bytes)| CapturedFile::new(path.clone(), bytes.clone()))
            .collect();
        capture(&manifest, files, self.ids.as_deref(), &LIMITS).expect("capture project")
    }

    // --- CLI path ---

    /// Write the project to a fresh temporary directory. `label` names the directory
    /// for easier debugging; it need not be unique.
    pub fn materialize(&self, label: &str) -> Workspace {
        let root = TempDir::new(label);
        write(&root.join("marrow.toml"), &self.manifest);
        if let Some(ids) = &self.ids {
            write(&root.join(".marrow/ids"), ids);
        }
        for (path, bytes) in &self.files {
            write(&root.join(path), bytes);
        }
        Workspace { root }
    }

    /// Materialize and invoke the `marrow` binary once with `args`. For several
    /// invocations against one workspace, use [`Project::materialize`] and drive the
    /// returned [`Workspace`].
    pub fn run_cli(&self, label: &str, args: &[&str]) -> CliOutcome {
        self.materialize(label).marrow(args)
    }
}

// ---------------------------------------------------------------------------
// Library path: persistent ephemeral session
// ---------------------------------------------------------------------------

/// A verified image plus one persistent ephemeral-memory attachment. Export calls
/// run in sequence against the same attachment, so a committed `transaction` is
/// observable by a later read.
pub struct Session {
    image: VerifiedImage,
    attachment: Option<MemoryAttachment>,
}

impl Session {
    /// Call `export` with `args`, returning its `Option<Value>` (`None` for a Unit
    /// return). Panics if the export faults, parks, or fails operationally — use
    /// [`Session::try_call`] to observe those.
    pub fn call(&mut self, export: &str, args: Vec<Value>) -> Option<Value> {
        match self.try_call(export, args) {
            CallOutcome::Value(value) => value,
            other => panic!("call to `{export}` did not return a value: {other:?}"),
        }
    }

    /// Call `export` with `args`, capturing the full outcome: a returned value, a
    /// source-mapped runtime fault (by stable code), a parked durable shape, or an
    /// operational failure (by stable code).
    pub fn try_call(&mut self, export: &str, args: Vec<Value>) -> CallOutcome {
        let (sealed, function) = self
            .image
            .exports()
            .iter()
            .find_map(|candidate| {
                let function = self
                    .image
                    .function(candidate.function())
                    .expect("verified function");
                (function.body().name() == export).then_some((candidate, function))
            })
            .unwrap_or_else(|| panic!("no export named `{export}`"));
        if function.demand().is_empty() {
            return match marrow_vm::run(function, args) {
                Ok(value) => CallOutcome::Value(value),
                Err(fault) => CallOutcome::Fault(fault.code()),
            };
        }
        let attachment = self
            .attachment
            .as_mut()
            .expect("a durable export requires a minted attachment");
        match run_export(attachment, sealed.id(), args).expect("the export is in the image") {
            DurableRun::Ran(Ok(value)) => CallOutcome::Value(value),
            DurableRun::Ran(Err(fault)) => CallOutcome::Fault(fault.code()),
            DurableRun::Parked => CallOutcome::Parked,
            DurableRun::Failed(code) => CallOutcome::Failed(code),
        }
    }

    /// The verified image, for a suite that inspects it directly.
    pub fn image(&self) -> &VerifiedImage {
        &self.image
    }
}

/// The captured outcome of one export call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallOutcome {
    /// The export returned; `None` for a Unit return.
    Value(Option<Value>),
    /// A source-mapped runtime fault, named by its registered code.
    Fault(Code),
    /// The image's durable shape is not executable by the ephemeral kernel.
    Parked,
    /// Minting or opening the session failed operationally, named by its registered code.
    Failed(Code),
}

// ---------------------------------------------------------------------------
// Library path: compile diagnostics
// ---------------------------------------------------------------------------

/// The typed source diagnostics from a failed compile. Assert stable codes and
/// spans, never message prose.
pub struct Diagnostics {
    diagnostics: Vec<SourceDiagnostic>,
}

impl Diagnostics {
    /// The diagnostics in compiler order, for a fixture asserting exact byte spans.
    pub fn iter(&self) -> std::slice::Iter<'_, SourceDiagnostic> {
        self.diagnostics.iter()
    }

    /// The diagnostic codes in compiler order.
    pub fn codes(&self) -> Vec<&str> {
        self.diagnostics.iter().map(|d| d.code().as_str()).collect()
    }

    pub fn has_code(&self, code: &str) -> bool {
        self.diagnostics.iter().any(|d| d.code().as_str() == code)
    }

    /// `(code, line, column)` for each diagnostic, in compiler order.
    pub fn all(&self) -> Vec<(&str, u32, u32)> {
        self.diagnostics
            .iter()
            .map(|d| (d.code().as_str(), d.line(), d.column()))
            .collect()
    }

    /// The number of diagnostics carrying `code` — the count a cascade-suppression
    /// fixture pins so one fault cannot re-report at every dependent site.
    pub fn count_code(&self, code: &str) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.code().as_str() == code)
            .count()
    }

    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// The single diagnostic carrying `code`, panicking unless exactly one does. The
    /// actionability suite asserts against one primary diagnostic per defect, so a
    /// second occurrence is a cascade regression the accessor surfaces immediately.
    pub fn only(&self, code: &str) -> &SourceDiagnostic {
        let mut matches = self
            .diagnostics
            .iter()
            .filter(|d| d.code().as_str() == code);
        let first = matches
            .next()
            .unwrap_or_else(|| panic!("no `{code}` diagnostic in {:?}", self.all()));
        assert!(
            matches.next().is_none(),
            "expected exactly one `{code}`, found several in {:?}",
            self.all()
        );
        first
    }

    /// The rendered messages, in compiler order, for asserting an actionable steer (a
    /// did-you-mean candidate, a named bound clause) that rides the diagnostic payload.
    pub fn messages(&self) -> Vec<&str> {
        self.diagnostics.iter().map(|d| d.message()).collect()
    }
}

// ---------------------------------------------------------------------------
// CLI path
// ---------------------------------------------------------------------------

/// A materialized project on disk. Invoke the `marrow` binary against it as many
/// times as a test needs; the directory is removed when the workspace drops.
pub struct Workspace {
    root: TempDir,
}

impl Workspace {
    pub fn dir(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Read a project file back (for asserting a formatter or mint write).
    pub fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.root.join(relative))
            .unwrap_or_else(|error| panic!("read `{relative}`: {error}"))
    }

    /// Invoke the `marrow` binary in the project root with `args`, capturing the
    /// outcome.
    pub fn marrow(&self, args: &[&str]) -> CliOutcome {
        marrow_in(&self.root, args)
    }
}

/// A captured CLI invocation. Derefs to the raw [`Output`] (so `.status`, `.stdout`,
/// and `.stderr` are available) and adds text and JSONL helpers.
#[derive(Debug)]
pub struct CliOutcome {
    pub output: Output,
}

impl Deref for CliOutcome {
    type Target = Output;
    fn deref(&self) -> &Output {
        &self.output
    }
}

impl CliOutcome {
    pub fn success(&self) -> bool {
        self.output.status.success()
    }

    /// The exit code, if the process exited normally.
    pub fn code(&self) -> Option<i32> {
        self.output.status.code()
    }

    /// Standard output as lossy UTF-8.
    pub fn stdout_text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.output.stdout)
    }

    /// Standard error as lossy UTF-8.
    pub fn stderr_text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.output.stderr)
    }

    /// The nonempty standard-output lines, for a `--format jsonl` run (one typed
    /// record per line).
    pub fn jsonl_lines(&self) -> Vec<String> {
        self.stdout_text()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string)
            .collect()
    }
}

/// The deployment ceiling id that `marrow image` names on standard error when the owner has
/// not accepted it. That refusal is the only place the id is published, so every suite that
/// needs one reads it here instead of restating the marker.
pub fn unaccepted_ceiling_id(stderr: &str) -> String {
    let marker = "deployment ceiling id is ";
    let start = stderr.find(marker).expect("stderr names the ceiling id") + marker.len();
    let rest = &stderr[start..];
    let end = rest.find(';').expect("ceiling id is delimited");
    rest[..end].trim().to_string()
}

// ---------------------------------------------------------------------------
// Support
// ---------------------------------------------------------------------------

/// Invoke the `marrow` binary with `args` in `dir` — the one spawn primitive. Runs with
/// `NO_COLOR=1`; the CLI emits no color to a pipe regardless, so this only guards a stray
/// terminal.
pub fn marrow_in(dir: &Path, args: &[&str]) -> CliOutcome {
    let output = Command::new(MARROW_BIN)
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("run the marrow binary");
    CliOutcome { output }
}

/// A conformance fixture directory in the repository-root corpus
/// (`fixtures/v01/conformance/<name>`), which suites drive in place through the CLI.
pub fn conformance_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .join("fixtures/v01/conformance")
        .join(name)
}

/// The fixture corpus root, resolved from the crate manifest directory so it is the
/// same regardless of the working directory a test runs in.
fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("v01")
}

/// Recursively collect `src` files, keyed by their path relative to the `src`
/// parent (so `src/a/b.mw`), matching the production capture identity.
fn collect_sources(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("read fixture src dir `{}`: {error}", dir.display()),
    };
    for entry in entries {
        let path = entry.expect("fixture dir entry").path();
        if path.is_dir() {
            collect_sources(base, &path, out);
        } else {
            let relative = path
                .strip_prefix(base.parent().expect("src has a parent"))
                .expect("fixture file under src");
            let key = relative.to_string_lossy().replace('\\', "/");
            let bytes = fs::read(&path).expect("read fixture source");
            out.push((key, bytes));
        }
    }
}

/// Write `contents` to `path`, creating parent directories.
pub fn write(path: &Path, contents: impl AsRef<[u8]>) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directory");
    }
    fs::write(path, contents).expect("write project file");
}

/// A temporary directory removed on drop, even through a failing assertion. The
/// per-process serial makes two directories minted in the same nanosecond distinct,
/// so a suite may scaffold scratch projects in a tight loop.
pub struct TempDir {
    root: PathBuf,
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

impl TempDir {
    /// A fresh directory named after `label`, which need not be unique.
    pub fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "marrow-test-{label}-{}-{serial}-{nanos}",
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
