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

use crate::dependency::{DependencyAlias, DependencyAliasReason, DependencyPath};
use crate::identity::{FileIdentity, ModuleName, SourceOrigin, SourcePathReason};
use crate::ids::{CapturedLedger, IDS_FILE, IdentityLedger, IdsError};
use crate::manifest::{Edition, Manifest};

/// A source file handed to [`capture`] by the physical adapter: the tree it came
/// from, a caller-supplied path relative to *that* tree's root, and its bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CapturedFile {
    origin: SourceOrigin,
    relative_path: String,
    bytes: Vec<u8>,
}

impl CapturedFile {
    /// Pair a root-project-relative path with its bytes. Validation happens in
    /// [`capture`]; this constructor imposes no structure so the adapter can pass
    /// exactly what it read.
    pub fn new(relative_path: String, bytes: Vec<u8>) -> Self {
        Self {
            origin: SourceOrigin::Root,
            relative_path,
            bytes,
        }
    }

    /// Pair a path relative to the root of the dependency captured under `alias`
    /// with its bytes.
    pub fn in_dependency(alias: DependencyAlias, relative_path: String, bytes: Vec<u8>) -> Self {
        Self {
            origin: SourceOrigin::Dependency(alias),
            relative_path,
            bytes,
        }
    }

    /// Map a borrowed root-relative spelling that is a syntactically valid identity
    /// past [`MAX_FILE_IDENTITY_BYTES`] to the pathless
    /// [`CaptureErrorKind::SourcePathTooLong`]. Every other outcome — a valid
    /// in-bound identity or any syntax error — returns `Ok(())`, leaving a
    /// syntactically invalid spelling to [`capture`]'s raw-path selection. The
    /// spelling is borrowed and no path is copied.
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

/// One captured dependency tree: the alias it was captured under and the
/// `.marrow/ids` bytes the adapter read there, or `None` when that tree committed
/// none. A dependency's ledger is read, never written: the declaring tree owns the
/// identities it committed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CapturedDependency<'a> {
    alias: &'a DependencyAlias,
    ids: Option<&'a [u8]>,
}

impl<'a> CapturedDependency<'a> {
    /// Name the dependency captured under `alias` and the ledger bytes it
    /// committed.
    pub fn new(alias: &'a DependencyAlias, ids: Option<&'a [u8]>) -> Self {
        Self { alias, ids }
    }
}

/// One captured module: the tree it came from, its canonical identity in that
/// tree, the module name its path implies there, and its source bytes. Fields are
/// private; a `ModuleInput` exists only inside a [`ProjectInput`] built by
/// [`capture`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModuleInput {
    origin: SourceOrigin,
    identity: FileIdentity,
    module: ModuleName,
    source: Vec<u8>,
}

impl ModuleInput {
    /// The tree this module was captured from.
    pub fn origin(&self) -> &SourceOrigin {
        &self.origin
    }

    /// The canonical identity, relative to the root of the tree it came from.
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

/// The immutable input the rest of the pipeline consumes: the declared edition,
/// the captured origins in canonical order with each origin's committed identity
/// ledger, and every module in canonical `(origin, identity)` order. Constructed
/// only through [`capture`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProjectInput {
    edition: Edition,
    modules: Vec<ModuleInput>,
    /// Captured origins in canonical order (the root first), parallel to
    /// `ledgers`. The root is always present, so `ledgers[0]` is its ledger.
    origins: Vec<SourceOrigin>,
    ledgers: Vec<CapturedLedger>,
    /// The manifest-declared location of each dependency origin, parallel to
    /// `origins[1..]`. The spelling is the manifest's own — relative to the consuming
    /// root — so a `ProjectInput` says where a tree sits *relative to the project*
    /// without ever carrying an absolute path.
    paths: Vec<DependencyPath>,
}

impl ProjectInput {
    /// The declared language edition.
    pub fn edition(&self) -> Edition {
        self.edition
    }

    /// The captured modules, in canonical `(origin, identity)` order: the root
    /// project's modules first, then each dependency's in alias order.
    pub fn modules(&self) -> &[ModuleInput] {
        &self.modules
    }

    /// The trees this input was captured from, in canonical order. The root is
    /// always first; a dependency appears whether or not it contributed a module.
    pub fn origins(&self) -> &[SourceOrigin] {
        &self.origins
    }

    /// The location the manifest declares for one captured dependency, relative to the
    /// consuming root. `None` for the root origin, which is the project itself, and for
    /// an origin this input did not capture.
    ///
    /// A consumer that must name a dependency's file on disk — an editor rebuilding a
    /// document URI, say — joins this relative spelling to the root it captured from.
    /// The relative spelling is what keeps a capture byte-identical wherever the pair
    /// sits.
    pub fn dependency_path(&self, origin: &SourceOrigin) -> Option<&DependencyPath> {
        let index = self.origins.iter().position(|held| held == origin)?;
        self.paths.get(index.checked_sub(1)?)
    }

    /// The parsed durable-identity ledger of one captured origin, or `None` when
    /// that tree committed no artifact. A durable declaration resolves against the
    /// ledger of the tree that declares it, so a dependency's declarations keep the
    /// identities the dependency committed.
    pub fn identity_ledger_for(&self, origin: &SourceOrigin) -> Option<&IdentityLedger> {
        let index = self.origins.iter().position(|held| held == origin)?;
        self.ledgers[index].present_ledger()
    }

    /// Admit one structurally nonempty identity-mint operation against the exact
    /// captured root ledger state, then invoke `supply` once with the exact
    /// positive candidate count. Minting is the root project's alone: a dependency
    /// commits its own ledger in its own tree.
    pub fn admit_identity_mints_with<E>(
        &self,
        first: crate::IdentityAnchor,
        rest: Vec<crate::IdentityAnchor>,
        supply: impl FnOnce(usize) -> Result<Vec<crate::DurableIdentityId>, E>,
    ) -> Result<crate::LedgerPublicationPlan, crate::IdentityMintFailure<E>> {
        self.root_ledger()
            .admit_identity_mints_with(first, rest, supply)
    }

    fn root_ledger(&self) -> &CapturedLedger {
        self.ledgers
            .first()
            .expect("every capture holds the root origin's ledger")
    }
}

/// Capture a validated [`Manifest`], a caller-supplied source listing, and the
/// optional `.marrow/ids` identity-artifact bytes into an immutable
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
    capture_origins(manifest, files, ids, &[], limits)
}

/// Capture a validated [`Manifest`], a caller-supplied origin-tagged source
/// listing, and each origin's optional `.marrow/ids` bytes into one immutable
/// [`ProjectInput`].
///
/// Every tree enters through this one call, so the file-count, per-file and total
/// byte bounds span all of them and no counter is reset per tree. The supplied
/// dependencies must be exactly the ones `manifest` declares, each once; anything
/// else is a `project.dependency_alias` fault, since the owner does not trust an
/// adapter to have matched the manifest it parsed.
///
/// The check precedence of [`capture`] is preserved and extended: the dependency
/// set, then each origin's identity artifact in canonical order, then the
/// file-count bound, per-path validity, alias-versus-root-module collisions,
/// module-identity collisions, and finally the per-file and total byte bounds.
pub fn capture_origins(
    manifest: &Manifest,
    files: Vec<CapturedFile>,
    ids: Option<&[u8]>,
    dependencies: &[CapturedDependency<'_>],
    limits: &CaptureLimits,
) -> Result<ProjectInput, CaptureError> {
    let ordered = admit_dependencies(manifest, dependencies)?;
    let mut captured_origins = vec![SourceOrigin::Root];
    let mut ledgers = vec![CapturedLedger::capture(ids).map_err(CaptureError::ids)?];
    let mut paths = Vec::with_capacity(ordered.len());
    // `admit_dependencies` returns the captured trees in manifest order, so the two
    // sequences agree entry by entry and a declared path lands beside its own origin.
    for (declared, dependency) in manifest.dependencies().iter().zip(ordered) {
        debug_assert_eq!(declared.alias(), dependency.alias);
        ledgers.push(CapturedLedger::capture(dependency.ids).map_err(CaptureError::ids)?);
        captured_origins.push(SourceOrigin::Dependency(dependency.alias.clone()));
        paths.push(declared.path().clone());
    }
    if files.len() > limits.max_files {
        return Err(CaptureError::limit(
            CaptureBound::FileCount,
            limits.max_files,
            files.len(),
        ));
    }

    // A valid identity past the maximum refuses before the ordinary invalid-path
    // collection. The offender is the smallest such `(origin, spelling)`, so the
    // fault is input-order independent.
    let mut overbound: Option<(&SourceOrigin, &str, usize, usize)> = None;
    for file in &files {
        if let Err(SourcePathReason::TooLong { limit, actual }) =
            FileIdentity::check(&file.relative_path)
        {
            let key = (&file.origin, file.relative_path.as_str());
            if overbound.is_none_or(|(origin, path, ..)| key < (origin, path)) {
                overbound = Some((key.0, key.1, limit, actual));
            }
        }
    }
    if let Some((.., limit, actual)) = overbound {
        return Err(CaptureError::source_path_too_long(limit, actual));
    }

    // Validate every path before reporting, so an invalid path is chosen by its
    // sorted `(origin, raw spelling)` rather than by arrival order.
    let mut invalid: Vec<(SourceOrigin, String, SourcePathReason)> = Vec::new();
    let mut valid: Vec<ModuleInput> = Vec::with_capacity(files.len());
    for file in files {
        match FileIdentity::validate_in(&file.relative_path, &file.origin) {
            Ok((identity, module)) => valid.push(ModuleInput {
                origin: file.origin,
                identity,
                module,
                source: file.bytes,
            }),
            Err(reason) => invalid.push((file.origin, file.relative_path, reason)),
        }
    }
    if !invalid.is_empty() {
        invalid.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        let (_, path, reason) = invalid.into_iter().next().expect("non-empty invalid set");
        return Err(CaptureError::source_path(path, reason));
    }

    valid.sort_by(|a, b| (&a.origin, &a.identity).cmp(&(&b.origin, &b.identity)));

    if let Some(fault) = find_alias_collision(&captured_origins, &valid) {
        return Err(fault);
    }
    if let Some(collision) = find_collision(&valid) {
        return Err(collision);
    }

    let mut total_bytes = 0usize;
    for module in &valid {
        if module.source.len() > limits.max_file_bytes {
            return Err(CaptureError::file_bytes(
                module.identity.clone(),
                limits.max_file_bytes,
                module.source.len(),
            ));
        }
        total_bytes = total_bytes.saturating_add(module.source.len());
    }
    if total_bytes > limits.max_total_bytes {
        return Err(CaptureError::limit(
            CaptureBound::TotalBytes,
            limits.max_total_bytes,
            total_bytes,
        ));
    }

    Ok(ProjectInput {
        edition: manifest.edition(),
        modules: valid,
        origins: captured_origins,
        ledgers,
        paths,
    })
}

/// Order the supplied dependencies into manifest (alias) order and check them
/// against the manifest: each declared dependency captured exactly once, and no
/// dependency the manifest does not declare.
fn admit_dependencies<'a>(
    manifest: &Manifest,
    captured: &[CapturedDependency<'a>],
) -> Result<Vec<CapturedDependency<'a>>, CaptureError> {
    let mut ordered = Vec::with_capacity(manifest.dependencies().len());
    for dependency in manifest.dependencies() {
        let mut matching = captured
            .iter()
            .filter(|supplied| supplied.alias == dependency.alias());
        let supplied = matching.next().ok_or_else(|| {
            CaptureError::dependency_alias(
                dependency.alias().clone(),
                DependencyAliasReason::Uncaptured,
            )
        })?;
        if matching.next().is_some() {
            return Err(CaptureError::dependency_alias(
                dependency.alias().clone(),
                DependencyAliasReason::Duplicate,
            ));
        }
        ordered.push(*supplied);
    }

    if ordered.len() != captured.len() {
        let undeclared = captured
            .iter()
            .map(|supplied| supplied.alias)
            .find(|alias| {
                !manifest
                    .dependencies()
                    .iter()
                    .any(|dependency| dependency.alias() == *alias)
            })
            .expect("a surplus captured tree names an undeclared alias");
        return Err(CaptureError::dependency_alias(
            undeclared.clone(),
            DependencyAliasReason::Undeclared,
        ));
    }
    Ok(ordered)
}

/// Refuse an alias that occupies the first segment of a root module's name: the
/// alias-rooted path `alias.rest` would otherwise name two different modules.
fn find_alias_collision(origins: &[SourceOrigin], sorted: &[ModuleInput]) -> Option<CaptureError> {
    for origin in origins {
        let Some(alias) = origin.alias() else {
            continue;
        };
        let collision = sorted.iter().find(|module| {
            module.origin == SourceOrigin::Root && module.module.first_segment() == alias.as_str()
        });
        if let Some(collision) = collision {
            return Some(CaptureError::dependency_alias(
                alias.clone(),
                DependencyAliasReason::RootModuleCollision {
                    module: collision.module.clone(),
                },
            ));
        }
    }
    None
}

/// Find the first module-identity collision among the sorted modules: two files
/// that derive the same module name — within one tree, or across trees once alias
/// prefixes are applied — or two identities in the same tree that differ only in
/// case and would collide on a case-insensitive filesystem. Two trees may hold the
/// same identity, since each is relative to its own root. Both offenders are named,
/// with the smaller `(origin, identity)` first, so the reported collision is
/// deterministic.
fn find_collision(sorted: &[ModuleInput]) -> Option<CaptureError> {
    for (index, module) in sorted.iter().enumerate() {
        for other in &sorted[index + 1..] {
            if module.module == other.module {
                return Some(CaptureError::module_collision(
                    module.module.clone(),
                    module.identity.clone(),
                    other.identity.clone(),
                    CollisionReason::DuplicateModule,
                ));
            }
            if module.origin == other.origin
                && module.identity.case_fold() == other.identity.case_fold()
            {
                return Some(CaptureError::module_collision(
                    module.module.clone(),
                    module.identity.clone(),
                    other.identity.clone(),
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
    /// The committed `.marrow/ids` identity artifact is corrupt (rejected whole).
    IdsCorrupt { error: IdsError },
    /// A declared dependency alias cannot root the modules it would contribute.
    DependencyAlias {
        /// The offending alias.
        alias: DependencyAlias,
        /// Why it was refused.
        reason: DependencyAliasReason,
    },
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
            // An over-long identity is always reported pathless.
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
            message: format!(
                "source path is {actual} bytes, over the {limit}-byte source-path limit"
            ),
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

    fn dependency_alias(alias: DependencyAlias, reason: DependencyAliasReason) -> Self {
        let explanation = match &reason {
            DependencyAliasReason::RootModuleCollision { module } => format!(
                "is the first segment of the root project's module `{}`",
                module.as_str()
            ),
            DependencyAliasReason::Duplicate => "was captured from more than one tree".to_string(),
            DependencyAliasReason::Undeclared => {
                "was captured but the manifest declares no such dependency".to_string()
            }
            DependencyAliasReason::Uncaptured => {
                "is declared but no tree was captured for it".to_string()
            }
            DependencyAliasReason::Placeholder => {
                "is the placeholder `_`, which names nothing".to_string()
            }
            DependencyAliasReason::NotIdentifier | DependencyAliasReason::TooLong { .. } => {
                "is not a usable identifier".to_string()
            }
        };
        Self {
            code: Code::ProjectDependencyAlias,
            kind: CaptureErrorKind::DependencyAlias {
                alias: alias.clone(),
                reason,
            },
            message: format!("dependency alias `{}` {explanation}", alias.as_str()),
        }
    }

    fn limit(bound: CaptureBound, limit: usize, actual: usize) -> Self {
        Self::from_bound(bound, limit, actual)
    }

    fn ids(error: IdsError) -> Self {
        Self {
            code: error.code(),
            message: format!("{IDS_FILE} is corrupt: {}", error.message()),
            kind: CaptureErrorKind::IdsCorrupt { error },
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
