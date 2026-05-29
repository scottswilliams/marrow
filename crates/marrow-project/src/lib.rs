//! The Marrow project configuration file, `marrow.json`, and the mapping from
//! source-root-relative paths to module names.
//!
//! A project is source plus an explicit storage selection. The file stays
//! small enough for the CLI, language services, and editors to agree on it: it
//! holds project choices only, never compiled schemas, index metadata, or
//! secrets.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

/// Stable error code for an invalid `marrow.json`.
pub const CONFIG_INVALID: &str = "config.invalid";

/// A validated Marrow project configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    /// Directories searched for `.mw` source, relative to the project root.
    pub source_roots: Vec<String>,
    /// Default entrypoint, a qualified `pub fn` name such as `shelf::sample::main`.
    pub default_entry: Option<String>,
    /// The selected storage backend, if the project pins one.
    pub store: Option<StoreConfig>,
    /// Test file glob patterns.
    pub tests: Vec<String>,
}

/// The storage selection: which backend, and where its data lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreConfig {
    pub backend: StoreBackend,
    pub data_dir: Option<String>,
}

/// A storage backend a project can select. Code checks capabilities, not
/// backend names; these are configuration and operator vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreBackend {
    Memory,
    Native,
}

impl StoreBackend {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "memory" => Some(Self::Memory),
            "native" => Some(Self::Native),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Native => "native",
        }
    }
}

/// An invalid `marrow.json`: malformed JSON, an unknown key, a missing required
/// field, or an unknown backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub code: &'static str,
    pub message: String,
}

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            code: CONFIG_INVALID,
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ConfigError {}

/// Parse and validate the contents of a `marrow.json` file.
pub fn parse_config(json: &str) -> Result<ProjectConfig, ConfigError> {
    let raw: RawConfig =
        serde_json::from_str(json).map_err(|error| ConfigError::new(error.to_string()))?;

    if raw.source_roots.is_empty() {
        return Err(ConfigError::new(
            "`sourceRoots` must list at least one source directory",
        ));
    }
    for source_root in &raw.source_roots {
        check_under_root("sourceRoots entry", source_root)?;
    }
    for pattern in &raw.tests {
        check_under_root("tests entry", pattern)?;
    }

    let store = match raw.store {
        Some(store) => {
            let backend = StoreBackend::parse(&store.backend).ok_or_else(|| {
                ConfigError::new(format!(
                    "unknown store backend `{}`; expected `memory` or `native`",
                    store.backend
                ))
            })?;
            // The native backend opens against a directory, so it cannot run
            // without one; reject the unrunnable config here rather than at open.
            if backend == StoreBackend::Native
                && store.data_dir.as_deref().unwrap_or("").is_empty()
            {
                return Err(ConfigError::new(
                    "the `native` store backend requires a non-empty `dataDir`",
                ));
            }
            if let Some(data_dir) = &store.data_dir {
                check_under_root("dataDir", data_dir)?;
            }
            Some(StoreConfig {
                backend,
                data_dir: store.data_dir,
            })
        }
        None => None,
    };

    Ok(ProjectConfig {
        source_roots: raw.source_roots,
        default_entry: raw.run.and_then(|run| run.default_entry),
        store,
        tests: raw.tests,
    })
}

/// Reject a configured path that would not stay under the project root: every
/// such value is joined onto the root, and `Path::join` discards the root for an
/// absolute argument, while a `..` component walks above it. `label` names the
/// field for the diagnostic.
fn check_under_root(label: &str, value: &str) -> Result<(), ConfigError> {
    if value.is_empty() {
        return Err(ConfigError::new(format!("`{label}` must not be empty")));
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(ConfigError::new(format!(
            "`{label}` `{value}` must be relative to the project root, not absolute"
        )));
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(ConfigError::new(format!(
            "`{label}` `{value}` must not contain a `..` component"
        )));
    }
    Ok(())
}

/// The module name a library file must declare, derived from its path relative
/// to a source root: `shelf/books.mw` → `shelf::books`, `books.mw` → `books`.
///
/// Returns `None` when the path is not a `.mw` file or steps outside the source
/// root (a `.`/`..`/absolute component), so it can never name a module.
pub fn expected_module_name(relative_path: &Path) -> Option<String> {
    if relative_path.extension().and_then(|ext| ext.to_str()) != Some("mw") {
        return None;
    }

    let mut segments = Vec::new();
    if let Some(parent) = relative_path.parent() {
        for component in parent.components() {
            match component {
                Component::Normal(name) => segments.push(name.to_str()?.to_string()),
                // Curdir is harmless (`./shelf/books.mw`); anything else escapes
                // the source root and cannot form a module path.
                Component::CurDir => {}
                _ => return None,
            }
        }
    }
    segments.push(relative_path.file_stem()?.to_str()?.to_string());
    Some(segments.join("::"))
}

/// A `.mw` file discovered under a source root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleFile {
    /// Absolute (or `project_root`-relative) path to the file on disk.
    pub path: PathBuf,
    /// Path relative to the source root it was found under.
    pub relative_path: PathBuf,
    /// The module name the file must declare, or `None` if its path cannot name
    /// a module (e.g. a dotted stem).
    pub module_name: Option<String>,
}

/// A source root that could not be read while discovering modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverError {
    pub code: &'static str,
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for DiscoverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}: {}",
            self.code,
            self.path.display(),
            self.message
        )
    }
}

impl std::error::Error for DiscoverError {}

/// Discover every `.mw` file under the project's source roots, pairing each
/// with the module name its path implies. Results are sorted by path so callers
/// see a deterministic order. Symlinks are skipped, so the walk cannot cycle.
pub fn discover_modules(
    project_root: &Path,
    config: &ProjectConfig,
) -> Result<Vec<ModuleFile>, DiscoverError> {
    let mut files = Vec::new();
    for source_root in &config.source_roots {
        let root = project_root.join(source_root);
        collect_mw_files(&root, &root, &mut files)?;
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    // Overlapping source roots (e.g. "src" and "src/sub") reach the same file
    // under two relative paths; keep the first source root's entry so a
    // correctly-placed file is not also reported under a mismatching name.
    files.dedup_by(|a, b| a.path == b.path);
    Ok(files)
}

/// Discover the `.mw` test files a project's `tests` patterns select, pairing each
/// with the module name its project-root-relative path implies. Test files live
/// outside the source roots — they are scripts, not library modules — so their
/// names are relative to the project root (`tests/books_test.mw` →
/// `tests::books_test`).
///
/// Each pattern is the directory-walk subset of a glob, honoring glob recursion
/// convention: a trailing double-star (`/**/*.mw`, `/**`) walks the base
/// directory recursively, while a single-star (`/*.mw`) matches only its
/// immediate directory; a bare directory is walked recursively; a bare `.mw`
/// file is taken directly. A pattern that matches nothing is skipped (no tests),
/// not an error. Results are sorted by path with duplicates removed.
pub fn discover_test_modules(
    project_root: &Path,
    config: &ProjectConfig,
) -> Result<Vec<ModuleFile>, DiscoverError> {
    let mut files = Vec::new();
    for pattern in &config.tests {
        let (base, recursive) = test_pattern_base(pattern);
        let target = project_root.join(base);
        if target.is_file() {
            files.push(module_file(project_root, target));
        } else if target.is_dir() {
            if recursive {
                collect_mw_files(project_root, &target, &mut files)?;
            } else {
                collect_mw_files_shallow(project_root, &target, &mut files)?;
            }
        }
        // A pattern that resolves to nothing on disk contributes no tests.
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    Ok(files)
}

/// The base path of a `tests` pattern and whether its directory is walked
/// recursively, with a trailing glob tail removed. Honoring glob convention, a
/// double-star tail (`/**/*.mw`, `/**`) recurses while a single-star tail
/// (`/*.mw`) matches only the immediate directory. A bare directory walks
/// recursively; a bare `.mw` file is taken directly.
///
/// `tests/**/*.mw` → (`tests`, recursive), `tests/*.mw` → (`tests`, shallow),
/// `tests` → (`tests`, recursive), `tests/smoke.mw` → (`tests/smoke.mw`, _).
fn test_pattern_base(pattern: &str) -> (&str, bool) {
    for (suffix, recursive) in [("/**/*.mw", true), ("/**", true), ("/*.mw", false)] {
        if let Some(base) = pattern.strip_suffix(suffix) {
            return (base, recursive);
        }
    }
    (pattern, true)
}

/// Walk `dir` only, collecting its immediate `.mw` files (no recursion). Backs
/// the single-star (`/*.mw`) test pattern.
fn collect_mw_files_shallow(
    source_root: &Path,
    dir: &Path,
    out: &mut Vec<ModuleFile>,
) -> Result<(), DiscoverError> {
    walk_mw_files(source_root, dir, out, false)
}

/// Walk `dir` recursively, collecting every `.mw` file beneath it.
fn collect_mw_files(
    source_root: &Path,
    dir: &Path,
    out: &mut Vec<ModuleFile>,
) -> Result<(), DiscoverError> {
    walk_mw_files(source_root, dir, out, true)
}

fn walk_mw_files(
    source_root: &Path,
    dir: &Path,
    out: &mut Vec<ModuleFile>,
    recursive: bool,
) -> Result<(), DiscoverError> {
    let entries = std::fs::read_dir(dir).map_err(|error| DiscoverError {
        code: "project.source_root",
        path: dir.to_path_buf(),
        message: error.to_string(),
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| DiscoverError {
            code: "project.source_root",
            path: dir.to_path_buf(),
            message: error.to_string(),
        })?;
        // `file_type` does not follow symlinks, so symlinked directories are
        // neither recursed into nor treated as source files.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            if recursive {
                walk_mw_files(source_root, &path, out, true)?;
            }
        } else if file_type.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("mw")
        {
            out.push(module_file(source_root, path));
        }
    }
    Ok(())
}

/// Build a [`ModuleFile`] for `path`, deriving its path relative to `source_root`
/// and the module name that relative path implies.
fn module_file(source_root: &Path, path: PathBuf) -> ModuleFile {
    // `path` is always discovered by walking down from `source_root`, so it is
    // an under-root descendant and stripping the prefix cannot fail.
    let relative_path = path
        .strip_prefix(source_root)
        .expect("discovered path is under its source root")
        .to_path_buf();
    let module_name = expected_module_name(&relative_path);
    ModuleFile {
        path,
        relative_path,
        module_name,
    }
}

/// The on-disk JSON shape. `deny_unknown_fields` rejects typos and stray keys,
/// keeping the configuration a small, closed set.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    source_roots: Vec<String>,
    #[serde(default)]
    run: Option<RawRun>,
    #[serde(default)]
    store: Option<RawStore>,
    #[serde(default)]
    tests: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawRun {
    #[serde(default)]
    default_entry: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawStore {
    backend: String,
    #[serde(default)]
    data_dir: Option<String>,
}
