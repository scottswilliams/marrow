//! The spawn half of the `marrow` crate's fixture harness: the built binary invoked over a
//! materialized [`Project`] ([`Project::materialize`] -> [`Workspace::marrow`], or the
//! one-shot [`Project::run_cli`]), the staged companion layout, and the conformance corpus
//! the suites drive in place.

use std::borrow::Cow;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use marrow_test_support::Scratch;

use super::project::Project;

/// The built `marrow` binary under test.
pub const MARROW_BIN: &str = env!("CARGO_BIN_EXE_marrow");

impl Project {
    /// Write the project to a fresh temporary directory. `label` names the directory
    /// for easier debugging; it need not be unique.
    pub fn materialize(&self, label: &str) -> Workspace {
        let root = Scratch::new(label);
        write(&root.path().join("marrow.toml"), &self.manifest);
        if let Some(ids) = &self.ids {
            write(&root.path().join(".marrow/ids"), ids);
        }
        for (path, bytes) in &self.files {
            write(&root.path().join(path), bytes);
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
// CLI path
// ---------------------------------------------------------------------------

/// A materialized project on disk. Invoke the `marrow` binary against it as many
/// times as a test needs; the directory is removed when the workspace drops.
pub struct Workspace {
    root: Scratch,
}

impl Workspace {
    pub fn dir(&self) -> &Path {
        self.root.path()
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    /// Read a project file back (for asserting a formatter or mint write).
    pub fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.root.path().join(relative))
            .unwrap_or_else(|error| panic!("read `{relative}`: {error}"))
    }

    /// Invoke the `marrow` binary in the project root with `args`, capturing the
    /// outcome.
    pub fn marrow(&self, args: &[&str]) -> CliOutcome {
        marrow_in(self.root.path(), args)
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

/// Write `contents` to `path`, creating parent directories.
pub fn write(path: &Path, contents: impl AsRef<[u8]>) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directory");
    }
    fs::write(path, contents).expect("write project file");
}

/// Stage a complete companion layout — the `marrow` and `marrow-runner` binaries
/// beside a `marrow-companions` release manifest — into a fresh directory, and
/// return it. The terminal locates the runner only beside itself, so every suite
/// that drives `--store` through the built CLI runs the staged copy rather than
/// `MARROW_BIN`. The stock runner is built next to the test binary by a workspace
/// build; its absence is a setup error, not a test failure.
pub fn stage_toolchain() -> Scratch {
    let runner = Path::new(MARROW_BIN)
        .parent()
        .expect("binary dir")
        .join("marrow-runner");
    assert!(
        runner.is_file(),
        "stock runner not built at {}; run a workspace build first",
        runner.display()
    );
    let dir = Scratch::new("toolchain");
    fs::copy(MARROW_BIN, dir.path().join("marrow")).expect("copy marrow");
    fs::copy(&runner, dir.path().join("marrow-runner")).expect("copy runner");
    let bytes = fs::read(&runner).expect("read runner");
    let id = marrow_image::companion_release_id(&bytes).to_hex();
    fs::write(
        dir.path().join("marrow-companions"),
        format!(
            "marrow companions v0\nrelease {}\nrunner marrow-runner {id}\nend\n",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .expect("write manifest");
    dir
}

/// Invoke the staged `marrow` binary in `dir` with `args`. Pair with
/// [`stage_toolchain`]; [`marrow_in`] runs the unstaged build, which finds no
/// companion.
pub fn staged_marrow_in(toolchain: &Path, dir: &Path, args: &[&str]) -> CliOutcome {
    let output = Command::new(toolchain.join("marrow"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .output()
        .expect("run the staged marrow binary");
    CliOutcome { output }
}
