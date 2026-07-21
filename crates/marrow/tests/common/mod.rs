#![allow(dead_code)]
//! The shared `.mw` fixture harness for the `marrow` crate's integration suites.
//!
//! One harness scaffolds a Marrow project, drives it through the production paths,
//! and captures typed outcomes, so a fixture suite is a thin file of assertions
//! rather than a fresh copy of the capture → compile → verify → run plumbing. The
//! five M1 fixture-authoring lanes and the existing embedded-`.mw` suites share this
//! one module (`mod common;`); it recompiles into each including test binary but is
//! authored, reviewed, and fixed in one place.
//!
//! # Scaffolding a project
//!
//! A [`Project`] is an in-memory project image: a manifest, an optional identity
//! ledger, and a set of source files. Build one inline, or load one from disk:
//!
//! ```ignore
//! // Inline, single source file at `src/main.mw`, default `edition = "2026"` manifest:
//! let project = Project::single("pub fn answer(): int {\n    return 42\n}\n");
//!
//! // Inline, several files and a durable identity ledger:
//! let project = Project::new()
//!     .source("src/bookstore.mw", BOOKSTORE_SOURCE)
//!     .ids(BOOKSTORE_IDS);
//!
//! // On disk, from `crates/marrow/tests/fixtures/v01/<name>/`:
//! let project = Project::from_fixture("counter_allocation");
//! ```
//!
//! # On-disk fixtures
//!
//! A fixture lives under `crates/marrow/tests/fixtures/v01/<name>/` as ordinary
//! source files, so new language behavior is authored as `.mw`, not as a Rust
//! string constant. The layout is a real project directory:
//!
//! ```text
//! fixtures/v01/<name>/
//!     marrow.toml        (required — the manifest)
//!     marrow.ids         (optional — the frozen identity ledger; see the trap below)
//!     src/<module>.mw    (one or more source files, any subtree depth)
//! ```
//!
//! [`Project::from_fixture`] reads `marrow.toml`, reads `marrow.ids` when present,
//! and walks `src/` recursively, keying each file by its `src`-relative canonical
//! path (`src/bookstore.mw`). The module name a fixture's exports carry is derived
//! from that path by the production owner, so an export in `src/bookstore.mw` is
//! `bookstore.<fn>`. `v01` is the identity version of the fixture corpus; a future
//! incompatible corpus is a new directory, never an edit that silently reinterprets
//! existing fixtures.
//!
//! # The ids-minting trap (read before authoring any durable fixture)
//!
//! The compiler never mints durable identities. On the library path
//! ([`Project::image`], [`Project::session`]) and on every CLI path except
//! `marrow run`, a durable declaration whose identity ledger is missing a row is a
//! hard `check.durable_identity` diagnostic at that declaration's span — not a
//! silent mint. Entropy minting is a `marrow run` convenience only, and it *rewrites
//! `marrow.ids` from OS entropy*, which would both dirty the repository and make the
//! fixture nondeterministic.
//!
//! So every durable fixture ships a complete, fixed-hex ledger with `high-water 0`
//! and never relies on the mint. Completeness is the trap: adding one durable
//! declaration usually adds *several* ledger rows, and omitting any one fails the
//! build. A declaration mints an anchor for each of:
//!
//! - the application itself (anchor path `.`), exactly once;
//! - each stored product (`resource`);
//! - each stored field, at its dotted path — including fields nested inside a
//!   `branch` or a `group` (`Book.notes.text`);
//! - each keyed placement: a `store` root *and* each keyed `branch`;
//! - each placement's key column (`books.id`, `Book.notes.noteId`);
//! - each compiler-maintained managed index (`books.byIsbn`);
//! - each durable-reachable closed enum and each of its variants;
//! - each unkeyed `group` namespace.
//!
//! Copy the shape of an existing ledger and extend it row by row. The two shipped
//! fixtures cover the common rows: `fixtures/v01/counter_allocation/marrow.ids`
//! (`application`/`product`/`field`/`root`/`key`) and `fixtures/v01/bookstore/marrow.ids`
//! (the same plus an `index` row). For the row kinds neither fixture demonstrates,
//! copy the exact spelling from a sibling suite in this directory: keyed `branch`
//! placements and their nested `id root`/`id key`/`id field` rows in
//! `durable_subtree_purge.rs`, an `id group` namespace in `durable_groups.rs`, and
//! `id sum`/`id member` enum rows in `durable_field_widening.rs`. A storeless project
//! needs no ledger; pass [`EMPTY_IDS`] only if a test needs an explicit empty
//! `marrow.ids` on disk.
//!
//! # Driving a project and capturing outcomes
//!
//! Two production paths, three outcome capture types:
//!
//! - **Library path — [`Project::session`] → [`Session::call`] / [`Session::try_call`].**
//!   Compiles, verifies, and mints one persistent ephemeral-memory attachment, then
//!   runs exports against it. The attachment persists across calls, so a mutating
//!   export's committed `transaction` is observable by a later reading export and a
//!   rolled-back one is not — this is how durable store effects are captured without
//!   a real store. Storeless exports run on the VM directly. [`Session::call`]
//!   returns the export's `Option<Value>` and panics on any fault; [`Session::try_call`]
//!   returns a [`CallOutcome`] that also captures faults, parks, and operational
//!   failures.
//!
//! - **Library path — [`Project::image`] / [`Project::try_image`].** The verified
//!   image, for a suite that inspects it directly. [`Project::try_image`] returns
//!   [`Diagnostics`] (typed codes and spans) on a source-diagnostic failure, so a
//!   diagnostics fixture can assert a code without spawning a subprocess.
//!
//! - **CLI path — [`Project::materialize`] → [`Workspace::marrow`], or the
//!   one-shot [`Project::run_cli`].** Writes the project to a fresh temporary
//!   directory and invokes the built `marrow` binary (`CARGO_BIN_EXE_marrow`) there
//!   with `NO_COLOR=1`, capturing a [`CliOutcome`]. `CliOutcome` derefs to the raw
//!   `std::process::Output` (so `.status`, `.stdout`, `.stderr` are available) and
//!   adds `stdout_text`/`stderr_text`/`jsonl_lines` helpers. The temporary directory
//!   is removed on drop, so a `marrow run` mint cannot dirty the repository.
//!
//! Outcome types never render prose for assertions: a [`CallOutcome`] fault carries
//! the stable `marrow-codes` string, and [`Diagnostics`] carries `(code, line,
//! column)` — assert those, not messages.

use std::borrow::Cow;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use marrow_compile::{CompileFailure, SourceDiagnostic, compile};
use marrow_kernel::durable::EphemeralAttachment;
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput, capture};
use marrow_verify::{VerifiedImage, verify};
use marrow_vm::{DurableRun, Ephemeral, Value, mint_ephemeral, run_export};

/// The built `marrow` binary under test.
pub const MARROW_BIN: &str = env!("CARGO_BIN_EXE_marrow");

/// The default manifest: the sole supported edition, nothing else.
pub const DEFAULT_MANIFEST: &str = "edition = \"2026\"\n";

/// An identity ledger declaring no durable anchors, for a storeless project that
/// still needs an explicit `marrow.ids` on disk.
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
    /// (required), `marrow.ids` (optional), and every file under `src/` keyed by its
    /// `src`-relative path.
    pub fn from_fixture(name: &str) -> Self {
        let root = fixtures_root().join(name);
        let manifest = fs::read(root.join("marrow.toml"))
            .unwrap_or_else(|error| panic!("read fixture `{name}` marrow.toml: {error}"));
        let ids = match fs::read(root.join("marrow.ids")) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("read fixture `{name}` marrow.ids: {error}"),
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

    /// Replace the manifest.
    pub fn manifest(mut self, manifest: &str) -> Self {
        self.manifest = manifest.as_bytes().to_vec();
        self
    }

    /// Set the identity ledger (the `marrow.ids` artifact). See the module doc's
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

    /// Open a persistent ephemeral-memory session: compile, verify, and (for a
    /// durable project) mint one attachment that serves every export call in
    /// sequence.
    pub fn session(&self) -> Session {
        let image = self.image();
        let attachment = if image.roots().is_empty() {
            None
        } else {
            match mint_ephemeral(&image) {
                Ephemeral::Ready(attachment) => Some(attachment),
                Ephemeral::Parked => {
                    panic!("durable shape is not executable by the ephemeral kernel")
                }
                Ephemeral::Failed(code) => panic!("minting the attachment failed: {code}"),
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
        write_file(&root.join("marrow.toml"), &self.manifest);
        if let Some(ids) = &self.ids {
            write_file(&root.join("marrow.ids"), ids);
        }
        for (path, bytes) in &self.files {
            write_file(&root.join(path), bytes);
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
    attachment: Option<Box<EphemeralAttachment>>,
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
        let sealed = self
            .image
            .exports()
            .iter()
            .find(|candidate| self.image.function(candidate.function()).name() == export)
            .unwrap_or_else(|| panic!("no export named `{export}`"));
        if sealed.demand().is_empty() {
            return match marrow_vm::run(&self.image, sealed.function(), args) {
                Ok(value) => CallOutcome::Value(value),
                Err(fault) => CallOutcome::Fault(fault.code().to_string()),
            };
        }
        let attachment = self
            .attachment
            .as_deref_mut()
            .expect("a durable export requires a minted attachment");
        match run_export(&self.image, attachment, sealed, args) {
            DurableRun::Ran(Ok(value)) => CallOutcome::Value(value),
            DurableRun::Ran(Err(fault)) => CallOutcome::Fault(fault.code().to_string()),
            DurableRun::Parked => CallOutcome::Parked,
            DurableRun::Failed(code) => CallOutcome::Failed(code.to_string()),
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
    /// A source-mapped runtime fault, named by its stable `marrow-codes` string.
    Fault(String),
    /// The image's durable shape is not executable by the ephemeral kernel.
    Parked,
    /// Minting or opening the session failed operationally, named by stable code.
    Failed(String),
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
    /// The diagnostic codes in compiler order.
    pub fn codes(&self) -> Vec<&str> {
        self.diagnostics.iter().map(|d| d.code).collect()
    }

    /// Whether any diagnostic carries `code`.
    pub fn has_code(&self, code: &str) -> bool {
        self.diagnostics.iter().any(|d| d.code == code)
    }

    /// `(code, line, column)` for each diagnostic, in compiler order.
    pub fn all(&self) -> Vec<(&str, u32, u32)> {
        self.diagnostics
            .iter()
            .map(|d| (d.code, d.line(), d.column()))
            .collect()
    }

    /// The number of diagnostics carrying `code` — the count a cascade-suppression
    /// fixture pins so one fault cannot re-report at every dependent site.
    pub fn count_code(&self, code: &str) -> usize {
        self.diagnostics.iter().filter(|d| d.code == code).count()
    }

    /// The total number of diagnostics.
    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// The single diagnostic carrying `code`, panicking unless exactly one does. The
    /// actionability suite asserts against one primary diagnostic per defect, so a
    /// second occurrence is a cascade regression the accessor surfaces immediately.
    pub fn only(&self, code: &str) -> &SourceDiagnostic {
        let mut matches = self.diagnostics.iter().filter(|d| d.code == code);
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
        self.diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect()
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
    /// The project root.
    pub fn dir(&self) -> &Path {
        &self.root
    }

    /// A path inside the project root.
    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Read a project file back (for asserting a formatter or mint write).
    pub fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.root.join(relative))
            .unwrap_or_else(|error| panic!("read `{relative}`: {error}"))
    }

    /// Invoke the `marrow` binary in the project root with `args`, capturing the
    /// outcome. Runs with `NO_COLOR=1`; the CLI emits no color to a pipe regardless,
    /// so this is a no-op for piped output and only guards a stray terminal.
    pub fn marrow(&self, args: &[&str]) -> CliOutcome {
        let output = Command::new(MARROW_BIN)
            .args(args)
            .current_dir(&*self.root)
            .env("NO_COLOR", "1")
            .output()
            .expect("run the marrow binary");
        CliOutcome { output }
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
    /// Whether the command exited successfully.
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

// ---------------------------------------------------------------------------
// Support
// ---------------------------------------------------------------------------

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

fn write_file(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directory");
    }
    fs::write(path, bytes).expect("write project file");
}

/// A temporary directory removed on drop, even through a failing assertion.
struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "marrow-e07h-{label}-{}-{nanos}",
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
