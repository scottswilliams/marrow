//! The Marrow project configuration file, `marrow.json`, and the mapping from
//! source-root-relative paths to module names.
//!
//! A project is source plus optional storage selection. The file stays small
//! enough for the CLI, language services, and editors to agree on it: it holds
//! project choices only, never compiled schemas, index metadata, or secrets.

use marrow_codes::Code;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

mod digest;
pub use digest::{Sha256Digest, sha256_digest};

/// Stable error code for an invalid `marrow.json`.
pub const CONFIG_INVALID: &str = Code::ConfigInvalid.as_str();

/// Fixed source-tree artifact name for the committed catalog lock: the generated,
/// committed projection that seeds a fresh empty store and reports staleness. It is
/// always subordinate to a valid live store and never repairs or overrides it.
pub const CATALOG_FILE_NAME: &str = "marrow.lock";

/// A validated Marrow project configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    /// Directories searched for `.mw` source, relative to the project root.
    pub source_roots: Vec<String>,
    /// Default entrypoint, a qualified `pub fn` name such as `shelf::sample::main`.
    pub default_entry: Option<String>,
    /// The selected storage backend.
    pub store: StoreConfig,
    /// Test file or directory paths, relative to the project root.
    pub tests: Vec<String>,
    /// Project-relative output path for the generated TypeScript surface client, when declared.
    /// `None` means the project emits no client. A bare string today; an object form is a
    /// forward-compatible extension reserved for the split-repo profile.
    pub client: Option<String>,
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
}

/// An invalid `marrow.json`: malformed JSON, an unknown key, a missing required
/// source root field, or an unknown backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub code: &'static str,
    pub kind: ConfigErrorKind,
    pub message: String,
    /// The 1-based line and column of a JSON syntax or unknown-field fault, taken
    /// from the serde parser. Present only for faults the parser locates to a
    /// single point; validation faults with no single source point leave it
    /// `None`. Carrying it here keeps the position a machine fact in the
    /// diagnostic span rather than only prose a client must parse.
    pub position: Option<ConfigPosition>,
}

/// A 1-based source position inside `marrow.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigPosition {
    pub line: u32,
    pub column: u32,
}

/// The typed reason a `marrow.json` failed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigErrorKind {
    InvalidJson,
    MissingSourceRoots,
    EmptySourceRoots,
    UnknownStoreBackend {
        backend: String,
    },
    NativeStoreMissingDataDir,
    NativeStoreEmptyDataDir,
    InvalidPath {
        field: ConfigPathField,
        value: String,
        reason: ConfigPathViolation,
    },
    /// A `tests` entry equals, sits under, or contains a source root. Test files
    /// live outside the source roots — they are scripts, not library modules — so
    /// an overlap would run library `pub fn`s as tests.
    TestsOverlapSourceRoot {
        test_entry: String,
        source_root: String,
    },
}

/// The config field that carried an invalid project-relative path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPathField {
    SourceRootsEntry,
    DataDir,
    TestsEntry,
    Client,
}

/// Why a configured project-relative path is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPathViolation {
    Empty,
    Absolute,
    ParentDir,
    GlobMetacharacter,
}

impl ConfigError {
    fn new(kind: ConfigErrorKind, message: impl Into<String>) -> Self {
        Self {
            code: CONFIG_INVALID,
            kind,
            message: message.into(),
            position: None,
        }
    }

    /// Build an `InvalidJson` fault from a serde error, lifting its located
    /// position into `position` and stripping the ` at line L column C` suffix
    /// serde appends so the position lives as a machine fact, not in the prose.
    fn invalid_json(error: &serde_json::Error) -> Self {
        let text = error.to_string();
        let (line, column) = (error.line(), error.column());
        if line == 0 {
            return Self::new(ConfigErrorKind::InvalidJson, text);
        }
        let suffix = format!(" at line {line} column {column}");
        let message = text.strip_suffix(&suffix).unwrap_or(&text).to_string();
        // serde reports column 0 at a line boundary or EOF; normalized to 1-based.
        let column = column.max(1);
        Self {
            code: CONFIG_INVALID,
            kind: ConfigErrorKind::InvalidJson,
            message,
            position: Some(ConfigPosition {
                line: line as u32,
                column: column as u32,
            }),
        }
    }
}

impl ConfigPathField {
    fn label(self) -> &'static str {
        match self {
            Self::SourceRootsEntry => "sourceRoots entry",
            Self::DataDir => "dataDir",
            Self::TestsEntry => "tests entry",
            Self::Client => "client",
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
///
/// The input is parsed both as a [`serde_json::Value`] and as [`RawConfig`]: the
/// tree pass rejects a non-object root and array-shaped `run`/`store` fields,
/// which a struct deserialize would otherwise map positionally, while the typed
/// pass carries the exact serde span in the unknown-field message.
pub fn parse_config(json: &str) -> Result<ProjectConfig, ConfigError> {
    let value: Value =
        serde_json::from_str(json).map_err(|error| ConfigError::invalid_json(&error))?;
    let object = config_object(&value)?;
    let has_source_roots = object.contains_key("sourceRoots");
    object_field(object, "run")?;
    object_field(object, "store")?;
    let raw: RawConfig =
        serde_json::from_str(json).map_err(|error| ConfigError::invalid_json(&error))?;

    if !has_source_roots {
        return Err(ConfigError::new(
            ConfigErrorKind::MissingSourceRoots,
            "`sourceRoots` must list at least one source directory",
        ));
    }
    if raw.source_roots.is_empty() {
        return Err(ConfigError::new(
            ConfigErrorKind::EmptySourceRoots,
            "`sourceRoots` must list at least one source directory",
        ));
    }
    if let Some(default_entry) = raw
        .run
        .as_ref()
        .and_then(|run| run.default_entry.as_deref())
    {
        check_no_nul("run.defaultEntry", default_entry)?;
    }
    for source_root in &raw.source_roots {
        check_under_root(ConfigPathField::SourceRootsEntry, source_root)?;
    }
    for test_path in &raw.tests {
        check_under_root(ConfigPathField::TestsEntry, test_path)?;
        check_plain_test_path(test_path)?;
        check_disjoint_from_source_roots(test_path, &raw.source_roots)?;
    }
    let store = match raw.store {
        Some(raw_store) => parse_store_config(raw_store)?,
        None => StoreConfig {
            backend: StoreBackend::Memory,
            data_dir: None,
        },
    };

    if let Some(client) = &raw.client {
        check_under_root(ConfigPathField::Client, client)?;
    }

    Ok(ProjectConfig {
        source_roots: raw.source_roots,
        default_entry: raw.run.and_then(|run| run.default_entry),
        store,
        tests: raw.tests,
        client: raw.client,
    })
}

fn parse_store_config(raw_store: RawStore) -> Result<StoreConfig, ConfigError> {
    check_no_nul("store.backend", &raw_store.backend)?;
    let backend = StoreBackend::parse(&raw_store.backend).ok_or_else(|| {
        ConfigError::new(
            ConfigErrorKind::UnknownStoreBackend {
                backend: raw_store.backend.clone(),
            },
            format!(
                "unknown store backend `{}`; expected `memory` or `native`",
                raw_store.backend
            ),
        )
    })?;
    if backend == StoreBackend::Native {
        match raw_store.data_dir.as_deref() {
            None => {
                return Err(ConfigError::new(
                    ConfigErrorKind::NativeStoreMissingDataDir,
                    "the `native` store backend requires a non-empty `dataDir`",
                ));
            }
            Some("") => {
                return Err(ConfigError::new(
                    ConfigErrorKind::NativeStoreEmptyDataDir,
                    "the `native` store backend requires a non-empty `dataDir`",
                ));
            }
            Some(_) => {}
        }
    }
    if let Some(data_dir) = &raw_store.data_dir {
        check_under_root(ConfigPathField::DataDir, data_dir)?;
    }
    let store = StoreConfig {
        backend,
        data_dir: raw_store.data_dir,
    };
    Ok(store)
}

/// Reject a configured path that would not stay under the project root: every
/// such value is joined onto the root, and `Path::join` discards the root for an
/// absolute argument, while a `..` component walks above it.
fn check_under_root(field: ConfigPathField, value: &str) -> Result<(), ConfigError> {
    if value.is_empty() {
        return Err(invalid_config_path(
            field,
            value,
            ConfigPathViolation::Empty,
        ));
    }
    check_no_nul(field.label(), value)?;
    let path = Path::new(value);
    if path.is_absolute() {
        return Err(invalid_config_path(
            field,
            value,
            ConfigPathViolation::Absolute,
        ));
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(invalid_config_path(
            field,
            value,
            ConfigPathViolation::ParentDir,
        ));
    }
    Ok(())
}

fn check_no_nul(label: &str, value: &str) -> Result<(), ConfigError> {
    if value.contains('\0') {
        return Err(ConfigError::new(
            ConfigErrorKind::InvalidJson,
            format!("`{label}` must not contain a NUL byte"),
        ));
    }
    Ok(())
}

fn check_plain_test_path(value: &str) -> Result<(), ConfigError> {
    if value
        .chars()
        .any(|character| matches!(character, '*' | '?' | '[' | ']' | '{' | '}'))
    {
        return Err(invalid_config_path(
            ConfigPathField::TestsEntry,
            value,
            ConfigPathViolation::GlobMetacharacter,
        ));
    }
    Ok(())
}

/// Reject a `tests` entry that overlaps any source root. Test files are scripts
/// that live outside the source roots; an entry that equals, descends from, or
/// contains a source root would load that root's library modules and run their
/// `pub fn`s as tests. Both paths are already validated as relative and
/// `..`-free, so a shared component prefix in either direction is a real overlap.
fn check_disjoint_from_source_roots(
    test_path: &str,
    source_roots: &[String],
) -> Result<(), ConfigError> {
    let test_components = path_components(test_path);
    for source_root in source_roots {
        let root_components = path_components(source_root);
        let overlaps = test_components
            .iter()
            .zip(&root_components)
            .all(|(test, root)| test == root);
        if overlaps {
            return Err(ConfigError::new(
                ConfigErrorKind::TestsOverlapSourceRoot {
                    test_entry: test_path.to_string(),
                    source_root: source_root.to_string(),
                },
                format!(
                    "`tests entry` `{test_path}` overlaps source root `{source_root}`; test files must live outside the source roots"
                ),
            ));
        }
    }
    Ok(())
}

/// The normal path segments of a project-relative path, dropping `.` components
/// so `./src/smoke.mw` and `src/smoke.mw` compare equal. The value is already
/// validated as relative and free of `..`, so only `Normal` and `CurDir` appear.
fn path_components(value: &str) -> Vec<&std::ffi::OsStr> {
    Path::new(value)
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect()
}

fn invalid_config_path(
    field: ConfigPathField,
    value: &str,
    reason: ConfigPathViolation,
) -> ConfigError {
    let label = field.label();
    let message = match reason {
        ConfigPathViolation::Empty => format!("`{label}` must not be empty"),
        ConfigPathViolation::Absolute => {
            format!("`{label}` `{value}` must be relative to the project root, not absolute")
        }
        ConfigPathViolation::ParentDir => {
            format!("`{label}` `{value}` must not contain a `..` component")
        }
        ConfigPathViolation::GlobMetacharacter => {
            format!("`{label}` `{value}` must not contain glob metacharacters")
        }
    };
    ConfigError::new(
        ConfigErrorKind::InvalidPath {
            field,
            value: value.to_string(),
            reason,
        },
        message,
    )
}

fn config_object(value: &Value) -> Result<&serde_json::Map<String, Value>, ConfigError> {
    value.as_object().ok_or_else(|| {
        ConfigError::new(
            ConfigErrorKind::InvalidJson,
            "config root must be a JSON object",
        )
    })
}

fn object_field(object: &serde_json::Map<String, Value>, field: &str) -> Result<(), ConfigError> {
    if object.get(field).is_some_and(|value| !value.is_object()) {
        return Err(ConfigError::new(
            ConfigErrorKind::InvalidJson,
            format!("`{field}` must be a JSON object"),
        ));
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

/// A project path that could not be read or made relative to its discovery root.
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

/// Discover the `.mw` test files a project's `tests` paths select, pairing each
/// with the module name its project-root-relative path implies. Test files live
/// outside the source roots — they are scripts, not library modules — so their
/// names are relative to the project root (`tests/books_test.mw` →
/// `tests::books_test`).
///
/// A `.mw` file entry selects that file. A directory entry is walked
/// recursively. A path that resolves to nothing on disk contributes no tests.
/// Results are sorted by path with duplicates removed.
pub fn discover_test_modules(
    project_root: &Path,
    config: &ProjectConfig,
) -> Result<Vec<ModuleFile>, DiscoverError> {
    let mut files = Vec::new();
    for test_path in &config.tests {
        let target = project_root.join(test_path);
        let Some(file_type) = std::fs::symlink_metadata(&target)
            .ok()
            .map(|metadata| metadata.file_type())
        else {
            continue;
        };
        if file_type.is_file() && target.extension().and_then(|ext| ext.to_str()) == Some("mw") {
            files.push(module_file(project_root, target)?);
        } else if file_type.is_dir() {
            collect_mw_files(project_root, &target, &mut files)?;
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    Ok(files)
}

/// Return the test module file for `path` when it is a `.mw` file selected by one
/// of the project's `tests` paths. This mirrors [`discover_test_modules`] for
/// overlay paths that may not exist on disk yet.
pub fn test_module_file(
    project_root: &Path,
    config: &ProjectConfig,
    path: &Path,
) -> Option<ModuleFile> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("mw") {
        return None;
    }
    if !path.starts_with(project_root) {
        return None;
    }
    for test_path in &config.tests {
        let target = project_root.join(test_path);
        if std::fs::symlink_metadata(&target)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_symlink())
        {
            continue;
        }
        let selected = if target.extension().and_then(|ext| ext.to_str()) == Some("mw") {
            path == target
        } else {
            path.starts_with(&target)
        };
        if selected {
            return module_file(project_root, path.to_path_buf()).ok();
        }
    }
    None
}

/// Collect the `.mw` files in `dir`, descending into subdirectories. Each file
/// is paired with the module name its path relative to `source_root` implies.
fn collect_mw_files(
    source_root: &Path,
    dir: &Path,
    out: &mut Vec<ModuleFile>,
) -> Result<(), DiscoverError> {
    let entries = std::fs::read_dir(dir).map_err(|error| DiscoverError {
        code: Code::ProjectSourceRoot.as_str(),
        path: dir.to_path_buf(),
        message: error.to_string(),
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| DiscoverError {
            code: Code::ProjectSourceRoot.as_str(),
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
            collect_mw_files(source_root, &path, out)?;
        } else if file_type.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("mw")
        {
            out.push(module_file(source_root, path)?);
        }
    }
    Ok(())
}

/// Build a [`ModuleFile`] for `path`, deriving its path relative to `source_root`
/// and the module name that relative path implies.
fn module_file(source_root: &Path, path: PathBuf) -> Result<ModuleFile, DiscoverError> {
    let relative_path = path
        .strip_prefix(source_root)
        .map_err(|_| DiscoverError {
            code: Code::ProjectSourceRoot.as_str(),
            path: path.clone(),
            message: "discovered module file is outside its discovery root".to_string(),
        })?
        .to_path_buf();
    let module_name = expected_module_name(&relative_path);
    Ok(ModuleFile {
        path,
        relative_path,
        module_name,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn module_file_rejects_paths_outside_the_discovery_root() {
        let error = super::module_file(
            &PathBuf::from("project/src"),
            PathBuf::from("project/tests/a.mw"),
        )
        .expect_err("outside-root file should fail closed");

        assert_eq!(error.code, "project.source_root");
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
    #[serde(default)]
    client: Option<String>,
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
