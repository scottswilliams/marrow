//! Shared project-setup harness for the `marrow-check` integration tests.
//!
//! Every checker test drives the real `check_project` or `analyze_project`
//! pipeline over a throwaway on-disk project. This module is the single owner of
//! that setup: a uniquely named temp directory, a recursive file writer, and the
//! standard `src`-rooted config.
//!
//! [`TempProject`] removes its directory on drop, so a test never cleans up by
//! hand and a panicking assertion still releases the directory.
//!
//! Each test binary includes this module, so not every binary exercises every
//! helper; the crate-wide `dead_code` allowance keeps the shared surface intact.

#![allow(dead_code)]

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use marrow_check::{
    AnalysisSnapshot, CheckDiagnostic, CheckReport, ProjectSources, analyze_project, check_project,
};
use marrow_project::{ProjectConfig, parse_config};

static NEXT_PROJECT_SERIAL: AtomicU64 = AtomicU64::new(0);

/// A temporary project directory removed when the value is dropped.
///
/// Derefs to its root [`Path`], so it passes straight into `check_project`,
/// `analyze_project`, and any other `&Path` consumer without an explicit
/// accessor.
pub struct TempProject {
    root: PathBuf,
}

impl Deref for TempProject {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

/// Create an empty, uniquely named project root removed on drop.
///
/// The name is suffixed with the process id plus a nanosecond clock reading and
/// a process-unique serial, so parallel test threads never share a directory and
/// one test's cleanup cannot race another's read.
pub fn temp_root(name: &str) -> TempProject {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let serial = NEXT_PROJECT_SERIAL.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "marrow-{name}-{}-{nanos}-{serial}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("create project root");
    TempProject { root }
}

/// Create a uniquely named project root and let `build` populate its files.
pub fn temp_project(name: &str, build: impl FnOnce(&Path)) -> TempProject {
    let root = temp_root(name);
    build(&root);
    root
}

/// Write `contents` to `root/relative`, creating parent directories as needed.
pub fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

/// The standard project config: a single `src` source root.
pub fn config() -> ProjectConfig {
    parse_config(r#"{ "sourceRoots": ["src"] }"#).expect("config")
}

/// Assert `report` carries no errors, dumping every diagnostic on failure.
pub fn assert_clean(report: &CheckReport) {
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// The diagnostics in `report` whose code is `code`, borrowed in report order.
pub fn with_code<'a>(report: &'a CheckReport, code: &str) -> Vec<&'a CheckDiagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == code)
        .collect()
}

/// Check a single script `src` placed at `src/app.mw` and return its diagnostics
/// whose code is `code`.
pub fn check_script(name: &str, src: &str, code: &str) -> Vec<CheckDiagnostic> {
    with_code(&check_module_report_at(name, "src/app.mw", src), code)
        .into_iter()
        .cloned()
        .collect()
}

/// Check a single library module `src` (declaring `module m`, placed at `src/m.mw`)
/// and return its diagnostics whose code is `code`. Unlike [`check_script`], the file
/// declares a module, so its functions join the checked program as call targets.
pub fn check_module(name: &str, src: &str, code: &str) -> Vec<CheckDiagnostic> {
    with_code(&check_module_report(name, src), code)
        .into_iter()
        .cloned()
        .collect()
}

/// Check a single library module and return its whole report, for tests that assert a
/// program is clean rather than filtering for one code.
pub fn check_module_report(name: &str, src: &str) -> CheckReport {
    check_module_report_at(name, "src/m.mw", src)
}

fn check_module_report_at(name: &str, relative: &str, src: &str) -> CheckReport {
    let root = temp_project(name, |root| write(root, relative, src));
    let (report, _program) = check_project(&root, &config()).expect("check");
    report
}

/// Write each `(relative-path, source)` under a fresh project root, overlay the same
/// text, and run the editor-facing `analyze_project` path. Returns the snapshot and the
/// absolute paths in the given order, so a test can position into the buffer it wrote.
/// This is the single owner of the write-then-overlay setup the tooling queries use.
pub fn analyze_overlay(name: &str, files: &[(&str, &str)]) -> (AnalysisSnapshot, Vec<PathBuf>) {
    let root = temp_root(name);
    let mut sources = ProjectSources::new();
    let mut paths = Vec::new();
    for (relative, source) in files {
        let path = root.join(relative);
        write(&root, relative, source);
        sources.insert(&path, *source);
        paths.push(path);
    }
    let snapshot = analyze_project(&root, &config(), &sources).expect("analyze");
    (snapshot, paths)
}

/// Shared catalog-fixture plumbing for the discharge and presence suites: the
/// well-known catalog file location, the JSON writer, the bare `CatalogEntry`
/// constructor, the presence-suite catalog wrapper, and the deterministic
/// label-to-id minting the presence suite keys its fixtures on. The discharge
/// suite uses literal `cat_` ids; both layer their own structural fields
/// (`accepted_struct`, `accepted_key_shape`) on top of the bare entry.
pub mod catalog {
    use std::hash::{Hash, Hasher};
    use std::path::{Path, PathBuf};

    use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata};

    /// The well-known accepted-catalog file under a project root.
    pub fn catalog_path(root: &Path) -> PathBuf {
        root.join("marrow.catalog.json")
    }

    /// Write `metadata` to the accepted-catalog file at the project root.
    pub fn write_catalog(root: &Path, metadata: &CatalogMetadata) {
        std::fs::write(catalog_path(root), metadata.to_json_pretty()).expect("write catalog");
    }

    /// An `Active` catalog entry with the given kind, canonical path, stable id, and
    /// aliases and no recorded structural signature. Suites add their structural fields
    /// (`accepted_struct`, `accepted_key_shape`) on top of this.
    pub fn entry(
        kind: CatalogEntryKind,
        path: &str,
        stable_id: &str,
        aliases: &[&str],
    ) -> CatalogEntry {
        CatalogEntry {
            kind,
            path: path.to_string(),
            stable_id: stable_id.to_string(),
            aliases: aliases.iter().map(|alias| alias.to_string()).collect(),
            lifecycle: CatalogLifecycle::Active,
            accepted_key_shape: None,
            accepted_struct: None,
        }
    }

    /// Wrap `entries` in a presence-suite catalog at a fixed schema digest, so a
    /// fixture lists only the entries it cares about and the digest never varies.
    pub fn catalog(entries: Vec<CatalogEntry>) -> CatalogMetadata {
        CatalogMetadata::new(7, entries)
    }

    /// Mint a deterministic `cat_<32 hex>` stable id from a readable label, so a
    /// fixture names a member by a readable label and the assertions that look the
    /// id back up agree without sharing a literal constant.
    pub fn derived_id(label: &str) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        label.hash(&mut hasher);
        let first = hasher.finish();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (label, "catalog-presence-fixture").hash(&mut hasher);
        let second = hasher.finish();
        format!("cat_{first:016x}{second:016x}")
    }
}
