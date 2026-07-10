//! Shared project-setup harness for the `marrow-check` integration tests.
//!
//! Every checker test drives the real `check_project` or `analyze_project`
//! pipeline over a throwaway on-disk project. This module is the single owner of
//! that setup: a uniquely named temp directory, a recursive file writer, and the
//! standard `src`-rooted config.
//!
//! [`TempProject`] removes its directory on drop, so a test never cleans up by
//! hand and a panicking assertion still releases the directory.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use marrow_check::{
    AnalysisSnapshot, CheckDiagnostic, CheckReport, CheckedProgram, DiagnosticPayload, EnumId,
    ProjectSources, ResourceId, analyze_project, check_project, check_project_with_catalog,
};
use marrow_project::{ProjectConfig, parse_config};
use marrow_schema::SchemaErrorKind;

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

/// The standard project config: a single `src` source root. The suites exercise durable
/// surfaces (stores, enums, resources), which require a native store to establish
/// committed identity, so the default backend is native.
pub fn config() -> ProjectConfig {
    parse_config(
        r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".marrow/data" } }"#,
    )
    .expect("config")
}

/// Assert `report` carries no errors, dumping every diagnostic on failure.
pub fn assert_clean(report: &CheckReport) {
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// Assert `diagnostic` carries the schema payload `expected`, dumping the diagnostic on
/// failure.
pub fn assert_schema_payload(diagnostic: &CheckDiagnostic, expected: SchemaErrorKind) {
    assert_eq!(
        diagnostic.payload,
        DiagnosticPayload::Schema(expected),
        "{diagnostic:#?}"
    );
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

/// Check a single library module and return the diagnostics whose code is `code`
/// alongside the checked program, so a test can recover an interned id (a resource
/// leaf carries its declaration id, not a spelling) from the program's facts.
pub fn check_module_program(
    name: &str,
    src: &str,
    code: &str,
) -> (Vec<CheckDiagnostic>, CheckedProgram) {
    check_program_at(name, "src/m.mw", src, code)
}

fn check_program_at(
    name: &str,
    relative: &str,
    src: &str,
    code: &str,
) -> (Vec<CheckDiagnostic>, CheckedProgram) {
    let root = temp_project(name, |root| write(root, relative, src));
    let (report, program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, code).into_iter().cloned().collect();
    (found, program)
}

/// Check a single library module and return its whole report alongside the checked
/// program, for a test that asserts across several codes and recovers an interned
/// id from the program's facts.
pub fn check_module_report_program(name: &str, src: &str) -> (CheckReport, CheckedProgram) {
    let root = temp_project(name, |root| write(root, "src/m.mw", src));
    check_project(&root, &config()).expect("check")
}

/// The interned id of the resource named `resource` in module `module` (empty for a
/// module-less script), for building an expected `MarrowType::Resource` without
/// hardcoding an arena index.
pub fn resource_id(program: &CheckedProgram, module: &str, resource: &str) -> ResourceId {
    let module_id = program
        .facts
        .module_id(module)
        .unwrap_or_else(|| panic!("no module `{module}`"));
    program
        .facts
        .resource_id(module_id, resource)
        .unwrap_or_else(|| panic!("no resource `{resource}` in `{module}`"))
}

/// The interned id of the enum named `enum_name` in module `module` (empty for a
/// module-less script), for building an expected `MarrowType::Enum` without
/// hardcoding an arena index. First-wins, matching the checker's aliasing.
pub fn enum_id(program: &CheckedProgram, module: &str, enum_name: &str) -> EnumId {
    let module_id = program
        .facts
        .module_id(module)
        .unwrap_or_else(|| panic!("no module `{module}`"));
    program
        .facts
        .enum_id(module_id, enum_name)
        .unwrap_or_else(|| panic!("no enum `{enum_name}` in `{module}`"))
}

/// The interned id of a declared store root, for building an expected
/// `MarrowType::Identity` without hardcoding an arena index.
pub fn identity_root_id(
    program: &CheckedProgram,
    root: &str,
) -> marrow_check::model::decls::StoreRootId {
    program
        .decl_roots
        .id(root)
        .unwrap_or_else(|| panic!("no store root `{root}`"))
}

/// Check a single library module and return its whole report, for tests that assert a
/// program is clean rather than filtering for one code.
pub fn check_module_report(name: &str, src: &str) -> CheckReport {
    check_module_report_at(name, "src/m.mw", src)
}

/// Check a single script `src` placed at `src/app.mw` and return its whole report, for
/// tests that assert across several codes rather than filtering for one.
pub fn check_script_report(name: &str, src: &str) -> CheckReport {
    check_module_report_at(name, "src/app.mw", src)
}

fn check_module_report_at(name: &str, relative: &str, src: &str) -> CheckReport {
    let root = temp_project(name, |root| write(root, relative, src));
    let (report, _program) = check_project(&root, &config()).expect("check");
    report
}

/// Write each `(relative-path, source)` under a fresh project root, overlay the same
/// text, and run the editor-facing `analyze_project` path. Returns the snapshot and the
/// absolute paths in the given order, so a test can position into the buffer it wrote.
/// This is the single owner of the write-then-overlay setup the tooling lookups use.
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
    let snapshot = analyze_project(&root, &config(), &sources, None, None).expect("analyze");
    (snapshot, paths)
}

/// Shared catalog-fixture plumbing for the discharge and presence suites: the
/// well-known catalog file location, the JSON writer, the bare `CatalogEntry`
/// constructor, the presence-suite catalog wrapper, and the deterministic
/// label-to-id minting the presence suite keys its fixtures on. The discharge
/// suite uses literal `cat_` ids; both layer their own structural fields
/// (`accepted_struct`, `accepted_key_shape`, `accepted_index_shape`) on top of the bare entry.
pub mod catalog {
    use std::hash::{Hash, Hasher};
    use std::path::{Path, PathBuf};

    use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata};
    use marrow_check::test_support::ACCEPTED_CATALOG_FIXTURE;

    /// The test-only accepted-catalog fixture file under a project root.
    pub fn catalog_path(root: &Path) -> PathBuf {
        root.join(ACCEPTED_CATALOG_FIXTURE)
    }

    /// Write `metadata` to the accepted-catalog fixture file at the project root.
    pub fn write_catalog(root: &Path, metadata: &CatalogMetadata) {
        std::fs::write(
            catalog_path(root),
            metadata.to_json_pretty().expect("catalog renders"),
        )
        .expect("write catalog");
    }

    /// Read the accepted-catalog fixture file at the project root, if one was written. A
    /// missing file is a first-run project. Suites bind this caller-supplied analysis input
    /// through [`super::check_with_accepted`] to pin a hand-built accepted catalog the source
    /// has moved away from.
    pub(super) fn read_catalog(root: &Path) -> Option<CatalogMetadata> {
        let json = std::fs::read_to_string(catalog_path(root)).ok()?;
        Some(CatalogMetadata::from_json(&json).expect("fixture catalog parses"))
    }

    /// An `Active` catalog entry with the given kind, canonical path, stable id, and
    /// aliases and no recorded structural signature. Suites add their structural fields
    /// (`accepted_struct`, `accepted_key_shape`, `accepted_index_shape`) on top of this.
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
            accepted_index_shape: None,
            accepted_struct: None,
            applied_transform: None,
        }
    }

    /// Wrap `entries` in a presence-suite catalog at a fixed epoch (7), so a fixture
    /// lists only the entries it cares about and the epoch is deterministic.
    pub fn catalog(entries: Vec<CatalogEntry>) -> CatalogMetadata {
        CatalogMetadata::new(7, entries).expect("catalog builds")
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

    /// An `Active` catalog entry whose stable id is minted deterministically from
    /// `label` via [`derived_id`], so a presence fixture names a member by a readable
    /// label and the assertions that look the id back up agree without sharing a literal
    /// constant. Tests that need a specific literal id call [`entry`] directly.
    pub fn entry_for_label(
        kind: CatalogEntryKind,
        canonical_path: &str,
        label: &str,
        aliases: &[&str],
    ) -> CatalogEntry {
        entry(kind, canonical_path, &derived_id(label), aliases)
    }

    /// A store-index presence entry that records `accepted_index_shape` over a
    /// label-derived stable id and no aliases, layering the shape onto
    /// [`entry_for_label`].
    pub fn store_index_entry_for_label(
        canonical_path: &str,
        label: &str,
        accepted_index_shape: &str,
    ) -> CatalogEntry {
        CatalogEntry {
            accepted_index_shape: Some(accepted_index_shape.to_string()),
            ..entry_for_label(CatalogEntryKind::StoreIndex, canonical_path, label, &[])
        }
    }
}

/// Check the project under `root`, binding any accepted catalog fixture the suite wrote
/// as caller-supplied analysis input. This is the catalog-aware replacement for a bare
/// `check_project` in suites that pin a hand-built accepted catalog the source has moved
/// away from.
pub fn check_with_accepted(root: &Path) -> (CheckReport, marrow_check::CheckedProgram) {
    let accepted = catalog::read_catalog(root);
    check_project_with_catalog(root, &config(), accepted.as_ref()).expect("check")
}
