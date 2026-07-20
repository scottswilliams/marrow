//! Deterministic contained discovery and the immutable [`ProjectInput`].
//!
//! Discovery is pure: the caller (a physical adapter that walks the filesystem)
//! supplies the source file listing and bytes and the capture limits, and this
//! owner validates paths, derives module identities, rejects collisions,
//! rechecks the bounds the adapter already enforced, and produces an immutable
//! [`ProjectInput`] with modules in a canonical order. Because identities are
//! root-relative, capturing the same files yields a byte-identical result no
//! matter what order they arrive in or where the project lives on disk.

use marrow_codes::Code;

use crate::identity::{FileIdentity, ModuleName, SourcePathReason};
use crate::ids::{IdentityLedger, IdsError};
use crate::manifest::{Edition, Manifest};

/// A source file handed to [`capture`] by the physical adapter: a caller-supplied
/// root-relative path and the file's bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CapturedFile {
    relative_path: String,
    bytes: Vec<u8>,
}

impl CapturedFile {
    /// Pair a root-relative path with its bytes. Validation happens in
    /// [`capture`]; this constructor imposes no structure so the adapter can pass
    /// exactly what it read.
    pub fn new(relative_path: String, bytes: Vec<u8>) -> Self {
        Self {
            relative_path,
            bytes,
        }
    }

    /// Report whether a borrowed root-relative spelling is a syntactically valid
    /// identity that exceeds [`MAX_FILE_IDENTITY_BYTES`], mapping only that
    /// valid-overbound case to the sealed pathless
    /// [`CaptureErrorKind::SourcePathTooLong`]. Every other outcome — a valid
    /// in-bound identity or any syntax error — returns `Ok(())`, so an over-long
    /// identity is refused before any path copy while a syntactically invalid
    /// spelling stays deferred to [`capture`]'s ordinary raw-path selection. It
    /// borrows the spelling and copies no path.
    ///
    /// [`MAX_FILE_IDENTITY_BYTES`]: crate::MAX_FILE_IDENTITY_BYTES
    pub fn check_identity_bound(path: &str) -> Result<(), CaptureError> {
        match FileIdentity::check(path) {
            Err(SourcePathReason::TooLong { limit, actual }) => {
                Err(CaptureError::source_path_too_long(limit, actual))
            }
            _ => Ok(()),
        }
    }
}

/// The bounds a project capture may not exceed. The physical adapter enforces
/// them while walking so it never buffers an unbounded tree, and [`capture`]
/// rechecks them so the owner never trusts an adapter to have done so.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CaptureLimits {
    max_files: usize,
    max_file_bytes: usize,
    max_total_bytes: usize,
}

impl CaptureLimits {
    /// The production capture bounds.
    pub const DEFAULT: CaptureLimits = CaptureLimits {
        max_files: 4096,
        max_file_bytes: 1 << 20,
        max_total_bytes: 64 << 20,
    };

    /// Build explicit bounds. Used by the adapter's production default and by
    /// tests that exercise the boundary at small sizes.
    pub const fn new(max_files: usize, max_file_bytes: usize, max_total_bytes: usize) -> Self {
        Self {
            max_files,
            max_file_bytes,
            max_total_bytes,
        }
    }

    pub const fn max_files(self) -> usize {
        self.max_files
    }

    pub const fn max_file_bytes(self) -> usize {
        self.max_file_bytes
    }

    pub const fn max_total_bytes(self) -> usize {
        self.max_total_bytes
    }
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// One captured module: its canonical identity, the module name its path implies,
/// and its source bytes. Fields are private; a `ModuleInput` exists only inside a
/// [`ProjectInput`] built by [`capture`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModuleInput {
    identity: FileIdentity,
    module: ModuleName,
    source: Vec<u8>,
}

impl ModuleInput {
    /// The canonical root-relative identity.
    pub fn identity(&self) -> &FileIdentity {
        &self.identity
    }

    /// The module name the identity implies.
    pub fn module(&self) -> &ModuleName {
        &self.module
    }

    /// The captured source bytes.
    pub fn source(&self) -> &[u8] {
        &self.source
    }
}

/// The immutable input the rest of the pipeline consumes: the declared edition
/// and the project's modules in canonical identity order. Constructed only
/// through [`capture`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProjectInput {
    edition: Edition,
    modules: Vec<ModuleInput>,
    ledger: Option<IdentityLedger>,
}

impl ProjectInput {
    /// The declared language edition.
    pub fn edition(&self) -> Edition {
        self.edition
    }

    /// The captured modules, in canonical identity order.
    pub fn modules(&self) -> &[ModuleInput] {
        &self.modules
    }

    /// The parsed durable-identity ledger, when the project committed a
    /// `marrow.ids` artifact. `None` means the artifact is absent — the normal
    /// state of a storeless project, equivalent to an empty ledger.
    pub fn identity_ledger(&self) -> Option<&IdentityLedger> {
        self.ledger.as_ref()
    }
}

/// Capture a validated [`Manifest`], a caller-supplied source listing, and the
/// optional `marrow.ids` identity-artifact bytes into an immutable
/// [`ProjectInput`].
///
/// Checks apply in a fixed precedence so the reported fault is deterministic
/// regardless of input order: the identity artifact (rejected whole when
/// corrupt), then the file-count bound, then per-path validity, then
/// module-identity collisions, then the per-file and total byte bounds. Within a
/// family the offender is chosen by canonical identity (or, for an invalid path,
/// the smallest raw path), never by arrival order. A physical adapter that
/// enforces the same bounds while walking may stop at a different
/// (traversal-order) offender before this owner ever sees the listing; the
/// canonical selection here governs only faults this owner reports.
pub fn capture(
    manifest: &Manifest,
    files: Vec<CapturedFile>,
    ids: Option<&[u8]>,
    limits: &CaptureLimits,
) -> Result<ProjectInput, CaptureError> {
    let ledger = match ids {
        Some(bytes) => Some(IdentityLedger::parse(bytes).map_err(CaptureError::ids)?),
        None => None,
    };
    if files.len() > limits.max_files {
        return Err(CaptureError::limit(
            CaptureBound::FileCount,
            limits.max_files,
            files.len(),
        ));
    }

    // A syntactically valid identity longer than the maximum refuses first, before
    // the ordinary invalid-path collection. The offender is the lexicographically
    // smallest raw valid-overbound spelling; only its bounded limit/actual evidence
    // is retained — never the raw path — so the fault is input-order independent and
    // carries no path.
    let mut overbound: Option<(&str, usize, usize)> = None;
    for file in &files {
        if let Err(SourcePathReason::TooLong { limit, actual }) =
            FileIdentity::check(&file.relative_path)
        {
            let path = file.relative_path.as_str();
            if overbound.is_none_or(|(smallest, ..)| path < smallest) {
                overbound = Some((path, limit, actual));
            }
        }
    }
    if let Some((_, limit, actual)) = overbound {
        return Err(CaptureError::source_path_too_long(limit, actual));
    }

    // Validate every path before reporting, so an invalid path is chosen by its
    // sorted raw spelling rather than by arrival order.
    let mut invalid: Vec<(String, SourcePathReason)> = Vec::new();
    let mut valid: Vec<(FileIdentity, ModuleName, Vec<u8>)> = Vec::with_capacity(files.len());
    for file in files {
        match FileIdentity::validate(&file.relative_path) {
            Ok((identity, module)) => valid.push((identity, module, file.bytes)),
            Err(reason) => invalid.push((file.relative_path, reason)),
        }
    }
    if !invalid.is_empty() {
        invalid.sort_by(|a, b| a.0.cmp(&b.0));
        let (path, reason) = invalid.into_iter().next().expect("non-empty invalid set");
        return Err(CaptureError::source_path(path, reason));
    }

    valid.sort_by(|a, b| a.0.cmp(&b.0));

    if let Some(collision) = find_collision(&valid) {
        return Err(collision);
    }

    let mut total_bytes = 0usize;
    for (identity, _module, bytes) in &valid {
        if bytes.len() > limits.max_file_bytes {
            return Err(CaptureError::file_bytes(
                identity.clone(),
                limits.max_file_bytes,
                bytes.len(),
            ));
        }
        total_bytes = total_bytes.saturating_add(bytes.len());
    }
    if total_bytes > limits.max_total_bytes {
        return Err(CaptureError::limit(
            CaptureBound::TotalBytes,
            limits.max_total_bytes,
            total_bytes,
        ));
    }

    let modules = valid
        .into_iter()
        .map(|(identity, module, source)| ModuleInput {
            identity,
            module,
            source,
        })
        .collect();

    Ok(ProjectInput {
        edition: manifest.edition(),
        modules,
        ledger,
    })
}

/// Find the first module-identity collision among the sorted entries: two files
/// that derive the same module name, or two identities that differ only in case
/// and would collide on a case-insensitive filesystem. Both offenders are named,
/// with the smaller identity first, so the reported collision is deterministic.
fn find_collision(sorted: &[(FileIdentity, ModuleName, Vec<u8>)]) -> Option<CaptureError> {
    for (i, (identity, module, _)) in sorted.iter().enumerate() {
        for (other_identity, other_module, _) in &sorted[i + 1..] {
            if module == other_module {
                return Some(CaptureError::module_collision(
                    module.clone(),
                    identity.clone(),
                    other_identity.clone(),
                    CollisionReason::DuplicateModule,
                ));
            }
            if identity.case_fold() == other_identity.case_fold() {
                return Some(CaptureError::module_collision(
                    module.clone(),
                    identity.clone(),
                    other_identity.clone(),
                    CollisionReason::CaseInsensitivePath,
                ));
            }
        }
    }
    None
}

/// A capture bound that a project exceeded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptureBound {
    /// Too many source files.
    FileCount,
    /// One source file is too large.
    FileBytes,
    /// The source files together are too large.
    TotalBytes,
}

/// Why two files collide on module identity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CollisionReason {
    /// Two distinct paths derive the same module name.
    DuplicateModule,
    /// Two paths differ only in case and would name the same file on a
    /// case-insensitive filesystem.
    CaseInsensitivePath,
}

/// The typed reason a capture failed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CaptureErrorKind {
    /// A caller-supplied path cannot name a contained source module.
    SourcePath {
        path: String,
        reason: SourcePathReason,
    },
    /// A caller-supplied path is a syntactically valid contained identity but
    /// exceeds the maximum identity byte length. Pathless: the offending raw path
    /// is never retained, so its bounded evidence is the limit and actual length
    /// alone.
    SourcePathTooLong { limit: usize, actual: usize },
    /// Two files collide on module identity.
    ModuleCollision {
        module: ModuleName,
        first: FileIdentity,
        second: FileIdentity,
        reason: CollisionReason,
    },
    /// A capture bound was exceeded.
    CaptureLimit {
        bound: CaptureBound,
        limit: usize,
        actual: usize,
    },
    /// The committed `marrow.ids` identity artifact is corrupt (rejected whole).
    IdsCorrupt { error: IdsError },
}

/// A capture failure. Carries a stable code, a typed [`CaptureErrorKind`], and a
/// human message. Path and manifest faults share [`Code::ConfigInvalid`] with the
/// manifest layer; discovery-specific faults use the `project.*` family.
///
/// Fields are private and every constructor is owner-private, so a `CaptureError`
/// is always the exact typed code/kind/message triple [`capture`] produced; a
/// hostile or inconsistent combination is unrepresentable outside this owner. The
/// read-only accessors expose the typed [`Code`] rather than a spelling.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CaptureError {
    code: Code,
    kind: CaptureErrorKind,
    message: String,
}

impl CaptureError {
    /// The stable diagnostic code this fault carries.
    pub fn code(&self) -> Code {
        self.code
    }

    /// The typed reason the capture failed.
    pub fn kind(&self) -> &CaptureErrorKind {
        &self.kind
    }

    /// The human-readable message.
    pub fn message(&self) -> &str {
        &self.message
    }

    fn source_path(path: String, reason: SourcePathReason) -> Self {
        let explanation = match reason {
            SourcePathReason::Absolute => "must be relative to the project root, not absolute",
            SourcePathReason::Escapes => "must not contain a `..` segment",
            SourcePathReason::NonCanonical => {
                "must be a canonical forward-slash path with no control character and no empty or `.` segment"
            }
            SourcePathReason::OutsideSourceRoot => "must live under the `src` source root",
            SourcePathReason::NotMarrowSource => "must be a `.mw` file with a non-empty name",
            // An over-long identity is always reported pathless; the raw path is
            // never retained, so it cannot ride the located source-path message.
            SourcePathReason::TooLong { limit, actual } => {
                return Self::source_path_too_long(limit, actual);
            }
        };
        Self {
            code: Code::ProjectSourcePath,
            kind: CaptureErrorKind::SourcePath {
                path: path.clone(),
                reason,
            },
            message: format!("source path `{path}` {explanation}"),
        }
    }

    fn source_path_too_long(limit: usize, actual: usize) -> Self {
        Self {
            code: Code::ProjectSourcePath,
            kind: CaptureErrorKind::SourcePathTooLong { limit, actual },
            message: format!("source path is {actual} bytes, over the {limit}-byte source-path limit"),
        }
    }

    fn module_collision(
        module: ModuleName,
        first: FileIdentity,
        second: FileIdentity,
        reason: CollisionReason,
    ) -> Self {
        let explanation = match reason {
            CollisionReason::DuplicateModule => format!(
                "`{}` and `{}` both name module `{}`",
                first.as_str(),
                second.as_str(),
                module.as_str()
            ),
            CollisionReason::CaseInsensitivePath => format!(
                "`{}` and `{}` differ only in case and collide on a case-insensitive filesystem",
                first.as_str(),
                second.as_str()
            ),
        };
        Self {
            code: Code::ProjectModuleCollision,
            kind: CaptureErrorKind::ModuleCollision {
                module,
                first,
                second,
                reason,
            },
            message: format!("colliding module identity: {explanation}"),
        }
    }

    fn limit(bound: CaptureBound, limit: usize, actual: usize) -> Self {
        Self::from_bound(bound, limit, actual)
    }

    fn ids(error: IdsError) -> Self {
        let message = format!("marrow.ids is corrupt: {}", error.message);
        let code = Code::from_code(error.code).expect("marrow.ids fault carries a registered code");
        Self {
            code,
            kind: CaptureErrorKind::IdsCorrupt { error },
            message,
        }
    }

    fn file_bytes(identity: FileIdentity, limit: usize, actual: usize) -> Self {
        let mut error = Self::from_bound(CaptureBound::FileBytes, limit, actual);
        error.message = format!(
            "source file `{}` is {actual} bytes, over the {limit}-byte per-file limit",
            identity.as_str()
        );
        error
    }

    fn from_bound(bound: CaptureBound, limit: usize, actual: usize) -> Self {
        let message = match bound {
            CaptureBound::FileCount => {
                format!("project has {actual} source files, over the {limit}-file limit")
            }
            CaptureBound::FileBytes => {
                format!("a source file is {actual} bytes, over the {limit}-byte per-file limit")
            }
            CaptureBound::TotalBytes => {
                format!("project source totals {actual} bytes, over the {limit}-byte project limit")
            }
        };
        Self {
            code: Code::ProjectCaptureLimit,
            kind: CaptureErrorKind::CaptureLimit {
                bound,
                limit,
                actual,
            },
            message,
        }
    }
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for CaptureError {}
