//! The Marrow project configuration file, `marrow.json`, and the mapping from
//! source-root-relative paths to module names.
//!
//! A project is source plus an explicit storage selection. The file stays
//! small enough for the CLI, language services, and editors to agree on it: it
//! holds project choices only, never compiled schemas, index metadata, or
//! secrets.

use std::collections::HashMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

mod digest;
pub use digest::sha256_digest;

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
    /// Generated accepted catalog metadata, relative to the project root.
    pub accepted_catalog: String,
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
/// field, or an unknown backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub code: &'static str,
    pub message: String,
}

/// Stable error code for an invalid accepted catalog metadata file.
pub const CATALOG_INVALID: &str = "catalog.invalid";

/// A committed accepted catalog snapshot. Source checks may read it and propose
/// replacement contents, but they never write it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogMetadata {
    pub epoch: u64,
    pub digest: String,
    pub entries: Vec<CatalogEntry>,
}

impl CatalogMetadata {
    pub fn new(epoch: u64, entries: Vec<CatalogEntry>) -> Self {
        let digest = catalog_digest(epoch, &entries);
        Self {
            epoch,
            digest,
            entries,
        }
    }

    pub fn from_json(json: &str) -> Result<Self, CatalogError> {
        let catalog: Self =
            serde_json::from_str(json).map_err(|error| CatalogError::new(error.to_string()))?;
        let expected = catalog_digest(catalog.epoch, &catalog.entries);
        if catalog.digest != expected {
            return Err(CatalogError::new(format!(
                "catalog digest `{}` does not match computed digest `{expected}`",
                catalog.digest
            )));
        }
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("catalog metadata serializes")
    }

    /// Check the identity invariants a committed catalog must hold: non-empty
    /// paths and stable IDs, a unique stable ID per entry, and a unique
    /// `(kind, path)` across both canonical paths and aliases. A proposal built by
    /// the checker is validated through this so an identity collision fails closed
    /// at check time rather than at apply.
    pub fn validate(&self) -> Result<(), CatalogError> {
        let mut paths: HashMap<(CatalogEntryKind, &str), usize> = HashMap::new();
        let mut stable_ids: HashMap<&str, usize> = HashMap::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.path.is_empty() {
                return Err(CatalogError::new("catalog entry path must not be empty"));
            }
            if !is_catalog_stable_id(&entry.stable_id) {
                return Err(CatalogError::new(
                    "catalog stable ID must match cat_<32 lowercase hex>",
                ));
            }
            if let Some(first) = stable_ids.insert(entry.stable_id.as_str(), index) {
                return Err(CatalogError::new(format!(
                    "catalog stable ID `{}` is used by entries {first} and {index}",
                    entry.stable_id
                )));
            }
            insert_catalog_path(&mut paths, entry.kind, &entry.path, index)?;
            for alias in &entry.aliases {
                if alias.is_empty() {
                    return Err(CatalogError::new("catalog alias must not be empty"));
                }
                if alias == &entry.path {
                    return Err(CatalogError::new(format!(
                        "catalog alias `{alias}` repeats its canonical path"
                    )));
                }
                insert_catalog_path(&mut paths, entry.kind, alias, index)?;
            }
        }
        Ok(())
    }
}

/// One accepted durable identity binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogEntry {
    pub kind: CatalogEntryKind,
    pub path: String,
    pub stable_id: String,
    pub aliases: Vec<String>,
    pub lifecycle: CatalogLifecycle,
    /// The identity-key shape a store's durable records are keyed under: the comma-joined
    /// scalar type names of its identity keys in order (`int`, `int,string`), so the
    /// arity and each key type are both recorded. v0.1 has no graceful store-key migration,
    /// so a discharge compares this against the current declared shape and fails closed when
    /// they differ: re-keying would orphan every record addressed by the old key shape. Only
    /// a store entry records it; every other kind leaves it `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_key_shape: Option<String>,
    /// The identity-aware structural signature a resource member's durable data was accepted
    /// under: its member kind, plus its key shape if it is a keyed layer and its leaf token if
    /// it is a leaf. A leaf records `leaf:<token>`, where the token names the member's value
    /// type by referent identity rather than source spelling — a scalar by name (`int`,
    /// `string`, ...), an enum by the stable catalog id of the enum it refers to (`enum:<id>`),
    /// or a store identity by the referenced store's stable catalog id and arity
    /// (`id:<id>:<arity>`), prefixed by a keyed-leaf layer's key shape. An unkeyed group records
    /// `group`, and a keyed group records `keyed-group:[<shape>]`. The discharge fails closed
    /// when a member present in both the accepted snapshot and current source has a signature
    /// that changed and no explicit obligation already covers it, so any structural transition
    /// not handled by a targeted classifier cannot silently activate over existing data. A keyed
    /// layer's key shape lives here rather than in `accepted_key_shape`, which records only store
    /// identity keys. Only a resource-member entry records it; every other kind leaves it `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_struct: Option<String>,
}

impl CatalogEntry {
    /// The identity-aware leaf token the member's durable bytes were accepted as, derived from
    /// the structural signature: a leaf member records `leaf:<token>`, so the token is the
    /// signature with that prefix stripped. The token records what type the bytes were last
    /// accepted as by referent identity rather than source spelling, so a later type change is
    /// detected across leaf kinds even when the new type's decoder would also accept the old
    /// bytes (an `int` stored as `1` reads as a `bool` `true`, or as an enum member), while a
    /// pure enum or store rename is correctly not a type change. `None` for any non-leaf
    /// member — a group, keyed group, or an entry that records no signature — since only a leaf
    /// position carries a single value cell with a leaf token.
    pub fn accepted_leaf_token(&self) -> Option<&str> {
        self.accepted_struct
            .as_deref()
            .and_then(|signature| signature.strip_prefix("leaf:"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CatalogEntryKind {
    Resource,
    Store,
    StoreIndex,
    ResourceMember,
    Enum,
    EnumMember,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CatalogLifecycle {
    Active,
    Deprecated,
    Reserved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogError {
    pub code: &'static str,
    pub message: String,
}

impl CatalogError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            code: CATALOG_INVALID,
            message: message.into(),
        }
    }
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CatalogError {}

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
            if backend == StoreBackend::Native && store.data_dir.as_deref().unwrap_or("").is_empty()
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

    let accepted_catalog = raw
        .accepted_catalog
        .unwrap_or_else(|| "marrow.catalog.json".to_string());
    check_under_root("acceptedCatalog", &accepted_catalog)?;

    Ok(ProjectConfig {
        source_roots: raw.source_roots,
        default_entry: raw.run.and_then(|run| run.default_entry),
        store,
        tests: raw.tests,
        accepted_catalog,
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
        collect_mw_files(&root, &root, &mut files, true)?;
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
/// A pattern that matches nothing is skipped (no tests), not an error. See
/// [`test_pattern_base`] for how each pattern's glob tail selects recursion.
/// Results are sorted by path with duplicates removed.
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
            collect_mw_files(project_root, &target, &mut files, recursive)?;
        }
        // A pattern that resolves to nothing on disk contributes no tests.
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    Ok(files)
}

/// Return the test module file for `path` when it is a `.mw` file selected by one
/// of the project's `tests` patterns. This mirrors [`discover_test_modules`] for
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
    for pattern in &config.tests {
        let (base, recursive) = test_pattern_base(pattern);
        let target = project_root.join(base);
        let selected = if target.extension().and_then(|ext| ext.to_str()) == Some("mw") {
            path == target
        } else if recursive {
            path.starts_with(&target)
        } else {
            path.parent() == Some(target.as_path())
        };
        if selected {
            return Some(module_file(project_root, path.to_path_buf()));
        }
    }
    None
}

/// The base path of a `tests` pattern and whether its directory is walked
/// recursively, with the trailing glob tail removed. A double-star tail
/// (`/**/*.mw`, `/**`) recurses; a single-star tail (`/*.mw`) matches only the
/// immediate directory; a bare directory recurses; a bare `.mw` file is taken
/// directly.
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

/// Collect the `.mw` files in `dir`, descending into subdirectories when
/// `recursive`. Each file is paired with the module name its path relative to
/// `source_root` implies.
fn collect_mw_files(
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
                collect_mw_files(source_root, &path, out, true)?;
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
    #[serde(default)]
    accepted_catalog: Option<String>,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DigestPayload<'a> {
    epoch: u64,
    entries: &'a [CatalogEntry],
}

fn catalog_digest(epoch: u64, entries: &[CatalogEntry]) -> String {
    let json = serde_json::to_string(&DigestPayload { epoch, entries })
        .expect("catalog digest payload serializes");
    sha256_digest(json.as_bytes())
}

fn is_catalog_stable_id(id: &str) -> bool {
    let Some(hex) = id.strip_prefix("cat_") else {
        return false;
    };
    hex.len() == 32
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn insert_catalog_path<'a>(
    paths: &mut HashMap<(CatalogEntryKind, &'a str), usize>,
    kind: CatalogEntryKind,
    path: &'a str,
    index: usize,
) -> Result<(), CatalogError> {
    if let Some(first) = paths.insert((kind, path), index) {
        return Err(CatalogError::new(format!(
            "catalog path `{path}` for `{kind:?}` is used by entries {first} and {index}"
        )));
    }
    Ok(())
}
