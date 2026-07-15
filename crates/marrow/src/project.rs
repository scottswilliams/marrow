//! The physical project-capture adapter.
//!
//! This is the filesystem edge the pure [`marrow_project`] owner deliberately
//! lacks: it reads `marrow.toml`, walks the fixed `src` source tree skipping
//! symlinks, reads each `.mw` file's bytes while enforcing the capture limits, and
//! hands root-relative paths and bytes to [`marrow_project::capture`], which
//! validates them and rechecks the same bounds. The CLI consumes the resulting
//! immutable `ProjectInput`; it never rebuilds discovery or identity here.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use marrow_codes::Code;
use marrow_project::{
    CaptureLimits, CapturedFile, Manifest, ManifestError, ManifestErrorKind, ProjectInput,
};

/// The manifest file at a project root.
pub(crate) const MANIFEST_FILE: &str = "marrow.toml";

/// The fixed source directory, mirroring [`marrow_project::SOURCE_ROOT`].
const SOURCE_DIR: &str = "src";

/// A project-capture failure, rendered by the CLI as a typed `code: message`
/// line. `location` names the manifest and 1-based position when the fault is a
/// located manifest syntax error.
pub(crate) struct CaptureFailure {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) location: Option<ManifestLocation>,
}

/// A located manifest fault: the manifest path and its 1-based line and column.
pub(crate) struct ManifestLocation {
    pub(crate) file: String,
    pub(crate) line: u32,
    pub(crate) column: u32,
}

impl CaptureFailure {
    fn simple(code: &'static str, message: String) -> Self {
        Self {
            code,
            message,
            location: None,
        }
    }
}

/// Read and capture the project rooted at `root` into an immutable
/// [`ProjectInput`], enforcing [`CaptureLimits::DEFAULT`]. The optional
/// `marrow.ids` identity artifact is read here — the artifact's one read — and
/// its bytes are validated by the pure owner exactly like source bytes.
pub(crate) fn capture_project(root: &Path) -> Result<ProjectInput, CaptureFailure> {
    let limits = CaptureLimits::DEFAULT;
    let manifest = read_manifest(root)?;
    let files = walk_source(root, &limits)?;
    let ids = read_ids(root)?;
    marrow_project::capture(&manifest, files, ids.as_deref(), &limits)
        .map_err(|error| CaptureFailure::simple(error.code, error.message))
}

/// Read the raw bytes of the project's `marrow.ids`, or `None` when the
/// artifact is absent. A symlinked artifact is refused like a symlinked source
/// root, and the size bound is enforced before reading so a hostile file is
/// never buffered; the pure owner rechecks the same bound.
fn read_ids(root: &Path) -> Result<Option<Vec<u8>>, CaptureFailure> {
    let path = root.join(marrow_project::IDS_FILE);
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return Ok(None);
    };
    if metadata.file_type().is_symlink() {
        return Err(CaptureFailure::simple(
            Code::ProjectIdsCorrupt.as_str(),
            format!(
                "{} is a symlink; the identity artifact must be a real file inside the project",
                path.display()
            ),
        ));
    }
    if metadata.len() > marrow_project::MAX_IDS_BYTES as u64 {
        return Err(CaptureFailure::simple(
            Code::ProjectIdsCorrupt.as_str(),
            format!(
                "{} is {} bytes, over the {}-byte identity-artifact bound",
                path.display(),
                metadata.len(),
                marrow_project::MAX_IDS_BYTES
            ),
        ));
    }
    fs::read(&path)
        .map(Some)
        .map_err(|error| read_dir_failure(&path, &error))
}

fn read_manifest(root: &Path) -> Result<Manifest, CaptureFailure> {
    let path = root.join(MANIFEST_FILE);
    let source = fs::read_to_string(&path).map_err(|error| {
        CaptureFailure::simple(
            Code::IoRead.as_str(),
            format!("failed to read {}: {error}", path.display()),
        )
    })?;
    Manifest::parse(&source).map_err(|error| manifest_failure(&path, error))
}

fn manifest_failure(path: &Path, error: ManifestError) -> CaptureFailure {
    let location = match (&error.kind, error.position) {
        (ManifestErrorKind::Malformed, Some(position)) => Some(ManifestLocation {
            file: path.display().to_string(),
            line: position.line,
            column: position.column,
        }),
        _ => None,
    };
    CaptureFailure {
        code: error.code,
        message: error.message,
        location,
    }
}

/// Walk `root/src`, collecting every `.mw` file as a root-relative
/// [`CapturedFile`]. Symlinks are skipped so the walk cannot cycle or escape the
/// tree, and the capture limits are enforced while reading so the adapter never
/// buffers an unbounded project.
fn walk_source(root: &Path, limits: &CaptureLimits) -> Result<Vec<CapturedFile>, CaptureFailure> {
    let source_root = root.join(SOURCE_DIR);
    // A project with no `src` directory has no modules; that is valid, not an error.
    let Ok(metadata) = fs::symlink_metadata(&source_root) else {
        return Ok(Vec::new());
    };
    // The containment contract starts at the root itself: a symlinked `src`
    // would carry the whole walk outside the project tree (per-entry symlink
    // skipping never inspects the root), so it is refused before descending.
    if metadata.file_type().is_symlink() {
        return Err(CaptureFailure::simple(
            Code::ProjectSourcePath.as_str(),
            format!(
                "source root {} is a symlink; a project's `src` must be a real directory inside the project",
                source_root.display()
            ),
        ));
    }
    let mut files = Vec::new();
    let mut total_bytes = 0usize;
    collect(root, &source_root, limits, &mut files, &mut total_bytes)?;
    Ok(files)
}

fn collect(
    root: &Path,
    dir: &Path,
    limits: &CaptureLimits,
    files: &mut Vec<CapturedFile>,
    total_bytes: &mut usize,
) -> Result<(), CaptureFailure> {
    let entries = fs::read_dir(dir).map_err(|error| read_dir_failure(dir, &error))?;
    // Sort entries so the adapter's own traversal is deterministic even before the
    // owner sorts; this keeps a limit fault reproducible across runs.
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| read_dir_failure(dir, &error))?;
        paths.push(entry.path());
    }
    paths.sort();

    for path in paths {
        // `symlink_metadata` does not follow links, so a symlinked file or
        // directory is neither read nor descended into.
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| read_dir_failure(&path, &error))?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect(root, &path, limits, files, total_bytes)?;
        } else if file_type.is_file() && has_mw_extension(&path) {
            let relative = relative_path(root, &path)?;
            let size = metadata.len();
            if size > limits.max_file_bytes() as u64 {
                return Err(over_limit(
                    &relative,
                    size,
                    limits.max_file_bytes(),
                    "over the per-file byte limit",
                ));
            }
            let bytes = fs::read(&path).map_err(|error| read_dir_failure(&path, &error))?;
            *total_bytes = total_bytes.saturating_add(bytes.len());
            if *total_bytes > limits.max_total_bytes() {
                return Err(over_limit(
                    &relative,
                    *total_bytes as u64,
                    limits.max_total_bytes(),
                    "over the project byte limit",
                ));
            }
            files.push(CapturedFile::new(relative, bytes));
            if files.len() > limits.max_files() {
                return Err(over_limit(
                    &relative_display(&path),
                    files.len() as u64,
                    limits.max_files(),
                    "over the source-file limit",
                ));
            }
        }
    }
    Ok(())
}

fn has_mw_extension(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some(marrow_project::SOURCE_EXTENSION)
}

/// The forward-slash root-relative path a discovered file carries, or a typed
/// `project.source_path` fault when a path component is not valid UTF-8.
fn relative_path(root: &Path, path: &Path) -> Result<String, CaptureFailure> {
    let relative = path.strip_prefix(root).map_err(|_| {
        CaptureFailure::simple(
            Code::ProjectSourcePath.as_str(),
            format!(
                "discovered source file {} is outside the project root",
                path.display()
            ),
        )
    })?;
    let mut segments = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => {
                let text = name.to_str().ok_or_else(|| {
                    CaptureFailure::simple(
                        Code::ProjectSourcePath.as_str(),
                        format!("source path {} is not valid UTF-8", path.display()),
                    )
                })?;
                segments.push(text);
            }
            _ => {
                return Err(CaptureFailure::simple(
                    Code::ProjectSourcePath.as_str(),
                    format!(
                        "source path {} is not a canonical relative path",
                        path.display()
                    ),
                ));
            }
        }
    }
    Ok(segments.join("/"))
}

fn relative_display(path: &Path) -> String {
    path.display().to_string()
}

fn over_limit(subject: &str, actual: u64, limit: usize, explanation: &str) -> CaptureFailure {
    CaptureFailure::simple(
        Code::ProjectCaptureLimit.as_str(),
        format!("`{subject}` capture is {actual}, {explanation} ({limit})"),
    )
}

fn read_dir_failure(path: &Path, error: &io::Error) -> CaptureFailure {
    CaptureFailure::simple(
        Code::IoRead.as_str(),
        format!("failed to read {}: {error}", path.display()),
    )
}
