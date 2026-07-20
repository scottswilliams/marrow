//! The physical project-capture entry point and its current-behavior walker.
//!
//! `capture_project` reads `marrow.toml`, walks the fixed `src` source tree
//! skipping symlinks, reads each `.mw` file's bytes while enforcing the shared
//! source limits, admits the optional `marrow.ids` ledger, and hands root-relative
//! paths and bytes to [`marrow_project::capture`]. It is the one filesystem owner
//! below the tool consumers.
//!
//! This baseline preserves the exact current behavior for the empty overlay every
//! CLI capture supplies: metadata-then-reopen reads, unbounded manifest/ids reads,
//! and a native recursive walk with no visited-entry, depth, opened-handle, or
//! path-budget enforcement. It routes that behavior through the typed failure and
//! seam owners the target adapter hardens: a canonical-root stage, per-role
//! admission stages, an always-successful [`PathBudget`], non-`Clone`
//! [`OperationalPath`]/[`SourceSpelling`] owners, and one directory
//! batch-observation seam. Those seams are deliberately insufficient against the
//! target law and are replaced in target hardening.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use marrow_project::{CapturedFile, Manifest, ProjectInput};

use crate::failure::{
    CaptureFailure, LinkPosition, PhysicalBound, PhysicalFailure, PhysicalIoError,
    PhysicalOperation, PhysicalRefusal, PhysicalRole,
};
use crate::limits::AdapterLimits;
use crate::overlay::{OverlayBound, OverlayFailure, OverlayReason, OverlaySnapshot};
use crate::path::{OperationalPath, PathBudget, SourceSpelling};

/// The required manifest file at a project root.
const MANIFEST_FILE: &str = "marrow.toml";

/// The fixed source directory, mirroring `marrow_project::SOURCE_ROOT`.
const SOURCE_DIR: &str = "src";

/// Read and capture the project rooted at `root` into an immutable
/// [`ProjectInput`], enforcing the frozen [`AdapterLimits`]. The overlay replaces
/// disk bodies for admitted members; every current CLI capture supplies the empty
/// overlay.
///
/// # Errors
///
/// Returns an opaque [`CaptureFailure`] for a manifest, source, ledger, physical,
/// or overlay-input refusal; present its message and code through
/// [`CaptureFailure::presentation`].
pub fn capture_project(
    root: &Path,
    overlay: OverlaySnapshot<'_>,
) -> Result<ProjectInput, CaptureFailure> {
    capture_project_with_limits(root, overlay, &AdapterLimits::DEFAULT)
}

/// The limit-parameterized capture seam. Production capture always uses
/// [`AdapterLimits::DEFAULT`]; small-policy behavior is a test-only concern of the
/// target adapter, never a configurable production API.
pub(crate) fn capture_project_with_limits(
    root: &Path,
    overlay: OverlaySnapshot<'_>,
    limits: &AdapterLimits,
) -> Result<ProjectInput, CaptureFailure> {
    // Baseline nonempty-overlay behavior: fail closed with one deliberately coarse
    // refusal before selecting any user bytes. The raw bounds, lexical
    // classification, membership settlement, and replacement land in target
    // hardening; this refusal cannot materialize or select overlay content.
    if !overlay.is_empty() {
        return Err(CaptureFailure::from_overlay_input(OverlayFailure::new(
            OverlayReason::Bound {
                bound: OverlayBound::Entries,
                limit: 0,
                actual: overlay.len(),
                entry: None,
            },
        )));
    }

    let mut budget = PathBudget::new();
    canonical_root_stage(root, &mut budget);

    let manifest = manifest_stage(root)?;
    let files = source_stage(root, limits, &mut budget)?;
    let ids = ledger_stage(root, limits)?;

    marrow_project::capture(&manifest, files, ids.as_deref(), &limits.source)
        .map_err(CaptureFailure::from_project)
}

/// The canonical-root stage. Baseline behavior uses the caller root directly and
/// charges only its spelling; the target adapter resolves a canonical physical
/// root here and charges it to both counters.
fn canonical_root_stage(root: &Path, budget: &mut PathBudget) {
    budget.charge(root.as_os_str().len());
}

/// The required `marrow.toml` stage. Baseline behavior reads the whole file and
/// leaves bound/opened-handle/link admission to target hardening.
fn manifest_stage(root: &Path) -> Result<Manifest, CaptureFailure> {
    let path = root.join(MANIFEST_FILE);
    let source = fs::read_to_string(&path).map_err(|error| {
        physical(
            PhysicalRole::Manifest,
            PhysicalOperation::Read,
            fixed_role_path(MANIFEST_FILE),
            PhysicalRefusal::Io {
                error: PhysicalIoError::new(error),
            },
        )
    })?;
    Manifest::parse(&source).map_err(CaptureFailure::from_manifest)
}

/// The optional `src` source stage. A missing `src` root is an empty source set;
/// a symlinked `src` root is refused before descending.
fn source_stage(
    root: &Path,
    limits: &AdapterLimits,
    budget: &mut PathBudget,
) -> Result<Vec<CapturedFile>, CaptureFailure> {
    let source_root = root.join(SOURCE_DIR);
    let Ok(metadata) = fs::symlink_metadata(&source_root) else {
        return Ok(Vec::new());
    };
    if metadata.file_type().is_symlink() {
        return Err(physical(
            PhysicalRole::SourceRoot,
            PhysicalOperation::Inspect,
            fixed_role_path(SOURCE_DIR),
            PhysicalRefusal::Link {
                position: LinkPosition::Terminal,
            },
        ));
    }

    let mut files = Vec::new();
    let mut total_bytes = 0usize;
    collect(
        root,
        &source_root,
        limits,
        budget,
        &mut files,
        &mut total_bytes,
    )?;
    Ok(files)
}

/// Recursively collect every `.mw` file below `dir`, enforcing the shared source
/// bounds while reading. Baseline behavior is a native recursive walk skipping
/// symlinks; the bounded iterative traversal lands in target hardening.
fn collect(
    root: &Path,
    dir: &Path,
    limits: &AdapterLimits,
    budget: &mut PathBudget,
    files: &mut Vec<CapturedFile>,
    total_bytes: &mut usize,
) -> Result<(), CaptureFailure> {
    let entries = fs::read_dir(dir).map_err(|error| enumerate_failure(root, dir, error))?;
    let paths =
        observe_directory_batch(entries).map_err(|error| enumerate_failure(root, dir, error))?;

    for path in paths {
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            physical(
                PhysicalRole::SourceDirectory,
                PhysicalOperation::Inspect,
                root_relative(root, &path),
                PhysicalRefusal::Io {
                    error: PhysicalIoError::new(error),
                },
            )
        })?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect(root, &path, limits, budget, files, total_bytes)?;
        } else if file_type.is_file() && has_mw_extension(&path) {
            let spelling = source_spelling(root, &path, budget)?;
            let size = metadata.len() as usize;
            if size > limits.source.max_file_bytes() {
                return Err(source_bound(
                    &spelling,
                    PhysicalBound::SourceFileBytes,
                    limits.source.max_file_bytes(),
                    size,
                ));
            }
            let bytes = fs::read(&path).map_err(|error| {
                physical(
                    PhysicalRole::SourceFile,
                    PhysicalOperation::Read,
                    Some(spelling_path(&spelling)),
                    PhysicalRefusal::Io {
                        error: PhysicalIoError::new(error),
                    },
                )
            })?;
            *total_bytes = total_bytes.saturating_add(bytes.len());
            if *total_bytes > limits.source.max_total_bytes() {
                return Err(source_bound(
                    &spelling,
                    PhysicalBound::SourceTotalBytes,
                    limits.source.max_total_bytes(),
                    *total_bytes,
                ));
            }
            files.push(CapturedFile::new(spelling.into_string(), bytes));
            if files.len() > limits.source.max_files() {
                // The count bound joins the caller root to the offending path.
                return Err(physical(
                    PhysicalRole::SourceFile,
                    PhysicalOperation::Retain,
                    root_relative(root, &path),
                    PhysicalRefusal::Bound {
                        bound: PhysicalBound::SourceFiles,
                        limit: limits.source.max_files(),
                        actual: files.len(),
                    },
                ));
            }
        }
    }
    Ok(())
}

/// The optional `marrow.ids` ledger stage. Only `NotFound` means absence; a
/// symlink or an over-bound artifact is refused, and the raw bytes reach the pure
/// ledger owner unchanged.
fn ledger_stage(root: &Path, limits: &AdapterLimits) -> Result<Option<Vec<u8>>, CaptureFailure> {
    let path = root.join(marrow_project::IDS_FILE);
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return Ok(None);
    };
    if metadata.file_type().is_symlink() {
        return Err(physical(
            PhysicalRole::IdentityLedger,
            PhysicalOperation::Inspect,
            fixed_role_path(marrow_project::IDS_FILE),
            PhysicalRefusal::Link {
                position: LinkPosition::Terminal,
            },
        ));
    }
    if metadata.len() as usize > limits.identity_ledger_bytes {
        return Err(physical(
            PhysicalRole::IdentityLedger,
            PhysicalOperation::Retain,
            fixed_role_path(marrow_project::IDS_FILE),
            PhysicalRefusal::Bound {
                bound: PhysicalBound::IdentityLedgerBytes,
                limit: limits.identity_ledger_bytes,
                actual: metadata.len() as usize,
            },
        ));
    }
    fs::read(&path).map(Some).map_err(|error| {
        physical(
            PhysicalRole::IdentityLedger,
            PhysicalOperation::Read,
            root_relative(root, &path),
            PhysicalRefusal::Io {
                error: PhysicalIoError::new(error),
            },
        )
    })
}

/// The directory batch-observation seam: accumulate every entry path and sort in
/// native lexical order. Baseline behavior is yield-order-insensitive only through
/// this final sort and is otherwise infallible on a successful enumeration; the
/// atomic bounded admission batch replaces it in target hardening.
fn observe_directory_batch(entries: fs::ReadDir) -> io::Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        paths.push(entry?.path());
    }
    paths.sort();
    Ok(paths)
}

/// The forward-slash UTF-8 root-relative spelling of a selected source file, or an
/// `InvalidPathEncoding` refusal when a component is not valid UTF-8.
fn source_spelling(
    root: &Path,
    path: &Path,
    budget: &mut PathBudget,
) -> Result<SourceSpelling, CaptureFailure> {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let mut segments: Vec<&str> = Vec::new();
    for component in relative.components() {
        // A path built from `root.join(read_dir_entry)` has only `Normal`
        // components; a non-UTF-8 name is the one reachable fault.
        if let Component::Normal(name) = component {
            let text = name.to_str().ok_or_else(|| {
                physical(
                    PhysicalRole::SourceFile,
                    PhysicalOperation::Inspect,
                    Some(OperationalPath::new(relative.to_path_buf())),
                    PhysicalRefusal::InvalidPathEncoding,
                )
            })?;
            segments.push(text);
        }
    }
    let spelling = segments.join("/");
    budget.charge(spelling.len());
    Ok(SourceSpelling::new(spelling))
}

fn has_mw_extension(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some(marrow_project::SOURCE_EXTENSION)
}

/// A fixed-role path is retained as its literal root-relative spelling; the facade
/// joins the caller root to render it.
fn fixed_role_path(spelling: &str) -> Option<OperationalPath> {
    Some(OperationalPath::new(PathBuf::from(spelling)))
}

/// The root-relative path of a discovered entry, retained for the facade to join.
fn root_relative(root: &Path, path: &Path) -> Option<OperationalPath> {
    let relative = path.strip_prefix(root).unwrap_or(path);
    Some(OperationalPath::new(relative.to_path_buf()))
}

/// A selected source spelling as retained path evidence.
fn spelling_path(spelling: &SourceSpelling) -> OperationalPath {
    OperationalPath::new(PathBuf::from(spelling.as_str()))
}

/// A per-file or project-total source-byte bound, whose spelling the facade renders
/// directly (not joined to the caller root).
fn source_bound(
    spelling: &SourceSpelling,
    bound: PhysicalBound,
    limit: usize,
    actual: usize,
) -> CaptureFailure {
    physical(
        PhysicalRole::SourceFile,
        PhysicalOperation::Retain,
        Some(spelling_path(spelling)),
        PhysicalRefusal::Bound {
            bound,
            limit,
            actual,
        },
    )
}

fn enumerate_failure(root: &Path, dir: &Path, error: io::Error) -> CaptureFailure {
    physical(
        PhysicalRole::SourceDirectory,
        PhysicalOperation::Enumerate,
        root_relative(root, dir),
        PhysicalRefusal::Io {
            error: PhysicalIoError::new(error),
        },
    )
}

fn physical(
    role: PhysicalRole,
    operation: PhysicalOperation,
    path: Option<OperationalPath>,
    refusal: PhysicalRefusal,
) -> CaptureFailure {
    CaptureFailure::from_physical(PhysicalFailure::new(role, operation, path, refusal))
}
