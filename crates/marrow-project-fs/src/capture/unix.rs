//! The Linux/macOS physical capture implementation: opened-handle admission, one
//! bounded iterative source traversal, and pure-owner composition.

use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use marrow_project::{
    CapturedDependency, CapturedFile, Dependency, DependencyAlias, FileIdentity, Manifest,
    ProjectInput,
};

use crate::failure::{
    CaptureFailure, DependencyRefusal, LedgerHome, LinkPosition, PhysicalBound, PhysicalFailure,
    PhysicalIoError, PhysicalKind, PhysicalRefusal, PhysicalRole,
};
use crate::limits::AdapterLimits;
use crate::overlay::OverlaySnapshot;
use crate::path::{PathBudget, PathLease, ReserveError, native_units};

const MANIFEST_FILE: &str = "marrow.toml";
const SOURCE_DIR: &str = "src";
const READ_CHUNK_BYTES: usize = 8 * 1024;

pub(super) fn capture(
    root: &Path,
    overlay: OverlaySnapshot<'_>,
    limits: &AdapterLimits,
) -> Result<ProjectInput, CaptureFailure> {
    let mut walk = Walk::new(overlay, limits);

    let root_admission = walk.admit_root(root)?;
    let root_tree = Tree::root(root_admission.canonical.clone());

    let manifest = walk.manifest_stage(&root_tree)?;
    walk.source_stage(&root_tree)?;
    let root_ids = walk.ledger_stage(&root_tree)?;

    // Each declared dependency is admitted from the root's canonical path and
    // walked through the same stages, carrying the same accumulators.
    let mut dependencies = Vec::with_capacity(manifest.dependencies().len());
    for declared in manifest.dependencies() {
        let (tree, admission) = walk.admit_dependency(&root_tree, declared)?;
        walk.source_stage(&tree)?;
        let ids = walk.ledger_stage(&tree)?;
        dependencies.push((declared.alias().clone(), ids, admission, tree));
    }

    root_admission.recheck()?;
    for (_, _, admission, _) in &dependencies {
        admission.recheck()?;
    }
    let captured: Vec<CapturedDependency<'_>> = dependencies
        .iter()
        .map(|(alias, ids, ..)| CapturedDependency::new(alias, ids.as_deref()))
        .collect();

    let Walk { files, overlay, .. } = walk;
    let input = marrow_project::capture_origins(
        &manifest,
        files,
        root_ids.as_deref(),
        &captured,
        &limits.source,
    )
    .map_err(CaptureFailure::from_project)?;

    // Only after a successful pure capture: settle the lowest-original unmatched
    // overlay entry.
    overlay
        .settle()
        .map_err(CaptureFailure::from_overlay_input)?;

    Ok(input)
}

// ===== Admitted trees =========================================================

/// One admitted source tree: the canonical directory capture walks, the spelling
/// that locates it under the caller's root, and the alias every file it
/// contributes is captured under. The root project's prefix is empty and its alias
/// is `None`.
///
/// A path a failure carries is always relative to the *caller's* root, so a
/// dependency's evidence keeps its declared prefix and never renders as a path in
/// the consuming tree.
pub(crate) struct Tree {
    canonical: PathBuf,
    prefix: PathBuf,
    alias: Option<DependencyAlias>,
}

impl Tree {
    pub(crate) fn root(canonical: PathBuf) -> Self {
        Self {
            canonical,
            prefix: PathBuf::new(),
            alias: None,
        }
    }

    /// The caller-root-relative evidence spelling of an in-tree path.
    fn evidence(&self, relative: &Path) -> PathBuf {
        if self.prefix.as_os_str().is_empty() {
            relative.to_path_buf()
        } else {
            self.prefix.join(relative)
        }
    }

    fn manifest_role(&self) -> PhysicalRole {
        match self.alias {
            None => PhysicalRole::Manifest,
            Some(_) => PhysicalRole::Dependency,
        }
    }

    /// Whether this tree's bodies may be replaced from the overlay. Only the root
    /// project is edited; a dependency is read exactly as it is committed.
    fn overlaid(&self) -> bool {
        self.alias.is_none()
    }

    fn captured(&self, spelling: String, bytes: Vec<u8>) -> CapturedFile {
        match &self.alias {
            None => CapturedFile::new(spelling, bytes),
            Some(alias) => CapturedFile::in_dependency(alias.clone(), spelling, bytes),
        }
    }
}

/// An opened tree root whose identity is held through capture and rechecked before
/// the pure owner is entered.
struct TreeAdmission {
    canonical: PathBuf,
    role: PhysicalRole,
    handle: File,
    identity: ObjectIdentity,
    /// The tree's native-path lease — the root's canonical path or the
    /// dependency's declared prefix — held until the recheck.
    _lease: PathLease,
}

impl TreeAdmission {
    fn recheck(&self) -> Result<(), CaptureFailure> {
        let handle = self
            .handle
            .metadata()
            .map_err(|error| pathless(self.role, io_refusal(error)))?;
        let path = fs::symlink_metadata(&self.canonical)
            .map_err(|error| pathless(self.role, io_refusal(error)))?;
        if ObjectIdentity::from_metadata(&handle) != self.identity
            || ObjectIdentity::from_metadata(&path) != self.identity
        {
            return Err(pathless(self.role, PhysicalRefusal::Changed));
        }
        Ok(())
    }
}

// ===== One bounded walk over every admitted tree ==============================

/// One capture's shared accumulators: the native-path budget, the visited-entry
/// and source-byte counters, the borrowed overlay, and the captured files. Every
/// admitted tree walks through this one value, so a second root
/// continues the bounds the first one spent rather than restarting them.
struct Walk<'a, 'o> {
    limits: &'a AdapterLimits,
    budget: PathBudget,
    overlay: OverlaySnapshot<'o>,
    visited: usize,
    total_bytes: usize,
    files: Vec<CapturedFile>,
}

impl<'a, 'o> Walk<'a, 'o> {
    fn new(overlay: OverlaySnapshot<'o>, limits: &'a AdapterLimits) -> Self {
        Self {
            limits,
            budget: PathBudget::new(),
            overlay,
            visited: 0,
            total_bytes: 0,
            files: Vec::new(),
        }
    }

    /// Reserve the native units of `path` against the live and aggregate bounds.
    fn lease(&mut self, role: PhysicalRole, path: &Path) -> Result<PathLease, CaptureFailure> {
        self.budget
            .reserve(
                native_units(path),
                self.limits.max_retained_path_units,
                self.limits.max_path_work_units,
            )
            .map_err(|error| pathless(role, reserve_refusal(error)))
    }

    /// Admit an object by its path inside one admitted tree: reserve its
    /// caller-root-relative evidence spelling, inspect every component without
    /// following links, then admit the terminal object with an opened handle whose
    /// identity matches. Any refusal carries the evidence spelling.
    fn admit(
        &mut self,
        tree: &Tree,
        relative: &Path,
        role: PhysicalRole,
        expected: PhysicalKind,
    ) -> Result<AdmittedObject, CaptureFailure> {
        let evidence = tree.evidence(relative);
        let lease = self.lease(role, &evidence)?;
        if let Err(refusal) = inspect_components(&tree.canonical, relative) {
            return Err(physical(role, evidence, refusal));
        }
        let absolute = tree.canonical.join(relative);
        match open_terminal(&absolute, expected) {
            Ok((file, identity)) => Ok(AdmittedObject {
                file,
                absolute,
                evidence,
                _lease: lease,
                role,
                identity,
            }),
            Err(refusal) => Err(physical(role, evidence, refusal)),
        }
    }

    fn admit_root(&mut self, root: &Path) -> Result<TreeAdmission, CaptureFailure> {
        // Caller-root work charge before canonicalization.
        self.budget
            .charge_work(native_units(root), self.limits.max_path_work_units)
            .map_err(|error| pathless(PhysicalRole::Root, reserve_refusal(error)))?;
        let canonical = fs::canonicalize(root).map_err(root_io)?;
        let lease = self
            .budget
            .reserve(
                native_units(&canonical),
                self.limits.max_retained_path_units,
                self.limits.max_path_work_units,
            )
            .map_err(|error| pathless(PhysicalRole::Root, reserve_refusal(error)))?;
        let (handle, identity) = open_terminal(&canonical, PhysicalKind::Directory)
            .map_err(|refusal| pathless(PhysicalRole::Root, refusal))?;
        Ok(TreeAdmission {
            canonical,
            role: PhysicalRole::Root,
            handle,
            identity,
            _lease: lease,
        })
    }

    /// Admit one declared dependency: resolve its relative path from the root's
    /// canonical path without following a link, refuse a target that is the
    /// consuming project itself or is not a project, and refuse a dependency that
    /// declares dependencies of its own.
    fn admit_dependency(
        &mut self,
        root: &Tree,
        declared: &Dependency,
    ) -> Result<(Tree, TreeAdmission), CaptureFailure> {
        let prefix = PathBuf::from(declared.path().as_str());
        let lease = self.lease(PhysicalRole::Dependency, &prefix)?;
        let located =
            |refusal: PhysicalRefusal| physical(PhysicalRole::Dependency, prefix.clone(), refusal);
        let canonical = resolve_dependency(&root.canonical, declared).map_err(located)?;
        let (handle, identity) =
            open_terminal(&canonical, PhysicalKind::Directory).map_err(located)?;
        let tree = Tree {
            canonical: canonical.clone(),
            prefix,
            alias: Some(declared.alias().clone()),
        };
        let admission = TreeAdmission {
            canonical,
            role: PhysicalRole::Dependency,
            handle,
            identity,
            _lease: lease,
        };

        // A directory with no manifest, or with no source root to contribute
        // modules from, is not a project; saying so once is clearer than a read
        // failure on a file the consumer never named.
        if optional_absent(&tree.canonical.join(MANIFEST_FILE))
            || optional_absent(&tree.canonical.join(SOURCE_DIR))
        {
            return Err(self.dependency_refusal(&tree, DependencyRefusal::NotAProject));
        }

        let manifest = self.manifest_stage(&tree)?;
        if !manifest.dependencies().is_empty() {
            return Err(self.dependency_refusal(&tree, DependencyRefusal::Transitive));
        }
        Ok((tree, admission))
    }

    fn dependency_refusal(&mut self, tree: &Tree, reason: DependencyRefusal) -> CaptureFailure {
        physical(
            PhysicalRole::Dependency,
            tree.prefix.clone(),
            PhysicalRefusal::Dependency { reason },
        )
    }

    // ===== Composition stages =================================================

    fn manifest_stage(&mut self, tree: &Tree) -> Result<Manifest, CaptureFailure> {
        if tree.overlaid() {
            self.overlay.mark_wrong_role(MANIFEST_FILE);
        }
        let role = tree.manifest_role();
        let relative = Path::new(MANIFEST_FILE);
        let mut admitted = self.admit(tree, relative, role, PhysicalKind::RegularFile)?;
        let bytes = admitted.read_bounded(
            ReadBudget::new(PhysicalBound::ManifestBytes, self.limits.manifest_bytes),
            None,
        )?;
        let source = admitted.decode_utf8(bytes)?;
        match Manifest::parse(&source) {
            Ok(manifest) => Ok(manifest),
            // A dependency's manifest faults belong to the dependency's own
            // project; from here it is simply not a usable project.
            Err(_) if tree.alias.is_some() => {
                Err(self.dependency_refusal(tree, DependencyRefusal::InvalidManifest))
            }
            Err(error) => Err(CaptureFailure::from_manifest(error)),
        }
    }

    fn ledger_stage(&mut self, tree: &Tree) -> Result<Option<Vec<u8>>, CaptureFailure> {
        if tree.overlaid() {
            self.overlay.mark_wrong_role(marrow_project::IDS_FILE);
            self.overlay
                .mark_wrong_role(marrow_project::LEGACY_IDS_FILE);
        }
        // A live publication marker means the committed ledger is whichever
        // generation recovery settles on, so every read-only front door refuses
        // here rather than capturing a generation that is about to be replaced.
        if let Some(marker) = crate::publication::ids_publication_marker(&tree.canonical) {
            return Err(CaptureFailure::from_ids_publication_marker(marker));
        }
        let home_present = !optional_absent(&tree.canonical.join(marrow_project::IDS_FILE));
        // The ledger has one home. A file at the retired root path is refused with
        // a one-line steer rather than read, so two live ledger locations are
        // unrepresentable and no second read path exists.
        if !optional_absent(&tree.canonical.join(marrow_project::LEGACY_IDS_FILE)) {
            return Err(physical(
                PhysicalRole::IdentityLedger,
                tree.evidence(Path::new(marrow_project::LEGACY_IDS_FILE)),
                PhysicalRefusal::LegacyLedgerPath {
                    home: if home_present {
                        LedgerHome::Occupied
                    } else {
                        LedgerHome::Vacant
                    },
                },
            ));
        }
        if !home_present {
            return Ok(None);
        }
        let relative = Path::new(marrow_project::IDS_FILE);
        let mut admitted = self.admit(
            tree,
            relative,
            PhysicalRole::IdentityLedger,
            PhysicalKind::RegularFile,
        )?;
        let bytes = admitted.read_bounded(
            ReadBudget::new(
                PhysicalBound::IdentityLedgerBytes,
                self.limits.identity_ledger_bytes,
            ),
            None,
        )?;
        Ok(Some(bytes))
    }

    fn source_stage(&mut self, tree: &Tree) -> Result<(), CaptureFailure> {
        if tree.overlaid() {
            self.overlay.mark_wrong_role(SOURCE_DIR);
        }
        let relative = Path::new(SOURCE_DIR);
        if optional_absent(&tree.canonical.join(SOURCE_DIR)) {
            return Ok(());
        }
        let root_dir = self.admit(
            tree,
            relative,
            PhysicalRole::SourceRoot,
            PhysicalKind::Directory,
        )?;
        self.run(tree, root_dir)
    }
}

// ===== Iterative bounded source traversal =====================================

struct DirectoryFrame {
    depth: usize,
    children: Vec<Child>,
    cursor: usize,
    _dir: AdmittedObject,
}

pub(crate) struct Child {
    absolute: PathBuf,
    relative: PathBuf,
    _lease: PathLease,
}

impl Child {
    pub(crate) fn relative(&self) -> &Path {
        &self.relative
    }
}

/// The atomic order-independent directory admission owner: a stateless settle
/// function over one observation sequence. It is generic over that sequence so
/// tests drive synthetic yield orders while production feeds the real `read_dir`
/// entries. It counts at most the remaining visit allowance plus one, measures the
/// aggregate carrier units commutatively, settles the aggregate bounds once
/// (retained wins), reserves live-only carriers, commits the aggregate work and the
/// visit count once, and sorts the carriers in native lexical order.
///
/// A partially settled batch is unrepresentable: every refusal returns before any
/// commit, so a refused batch leaves `visited`, `work`, and the live counter at
/// their baseline.
pub(crate) struct DirectoryAdmission;

impl DirectoryAdmission {
    pub(crate) fn settle(
        entries: impl Iterator<Item = io::Result<PathBuf>>,
        tree: &Tree,
        relative: &Path,
        budget: &mut PathBudget,
        limits: &AdapterLimits,
        visited: &mut usize,
    ) -> Result<Vec<Child>, CaptureFailure> {
        let remaining = limits.visited_entries.saturating_sub(*visited);
        let mut candidates: Vec<PathBuf> = Vec::new();
        let mut aggregate = 0usize;
        let mut count = 0usize;
        for entry in entries {
            // Poll the entry first: the first iterator error stops polling and wins
            // over pending aggregate/allocation/name-order outcomes, unless an extra
            // successful entry was already observed.
            let absolute = entry.map_err(|error| {
                physical(
                    PhysicalRole::SourceDirectory,
                    tree.evidence(relative),
                    io_refusal(error),
                )
            })?;
            if count >= remaining {
                // The (remaining + 1)th successful entry: drop every provisional
                // carrier and report the pathless visited-entry bound. Count-first
                // applies only because this extra success was observed.
                return Err(pathless(
                    PhysicalRole::SourceDirectory,
                    bound_refusal(
                        PhysicalBound::VisitedEntries,
                        limits.visited_entries,
                        limits.visited_entries + 1,
                    ),
                ));
            }
            count += 1;
            aggregate = aggregate
                .checked_add(native_units(&absolute))
                .ok_or_else(dir_oom)?;
            candidates.push(absolute);
        }

        // Clean EOF: settle the commutative aggregate before any commit; retained wins
        // a simultaneous over-bound; both are pathless.
        let prospective_retained = budget
            .retained()
            .checked_add(aggregate)
            .ok_or_else(dir_oom)?;
        if prospective_retained > limits.max_retained_path_units {
            return Err(pathless(
                PhysicalRole::SourceDirectory,
                bound_refusal(
                    PhysicalBound::RetainedPathUnits,
                    limits.max_retained_path_units,
                    prospective_retained,
                ),
            ));
        }
        let prospective_work = budget.work().checked_add(aggregate).ok_or_else(dir_oom)?;
        if prospective_work > limits.max_path_work_units {
            return Err(pathless(
                PhysicalRole::SourceDirectory,
                bound_refusal(
                    PhysicalBound::PathWorkUnits,
                    limits.max_path_work_units,
                    prospective_work,
                ),
            ));
        }

        // Stage carriers with live-only reservations, then commit the aggregate work
        // and the visit count once. No ordered `reserve` runs inside this batch.
        let mut children: Vec<Child> = Vec::new();
        children.try_reserve_exact(count).map_err(|_| dir_oom())?;
        for absolute in candidates {
            let units = native_units(&absolute);
            let lease = budget
                .reserve_live(units, limits.max_retained_path_units)
                .map_err(|error| pathless(PhysicalRole::SourceDirectory, reserve_refusal(error)))?;
            let name = absolute.file_name().map(PathBuf::from).unwrap_or_default();
            children.push(Child {
                relative: relative.join(&name),
                absolute,
                _lease: lease,
            });
        }
        budget.commit_work(aggregate).map_err(|_| dir_oom())?;
        *visited += count;
        children.sort_unstable_by(|a, b| a.absolute.cmp(&b.absolute));
        Ok(children)
    }
}

impl Walk<'_, '_> {
    fn run(&mut self, tree: &Tree, root_dir: AdmittedObject) -> Result<(), CaptureFailure> {
        let mut stack: Vec<DirectoryFrame> =
            vec![self.enumerate(tree, PathBuf::from(SOURCE_DIR), 0, root_dir)?];

        loop {
            let next = match stack.last_mut() {
                None => break,
                Some(frame) if frame.cursor >= frame.children.len() => None,
                Some(frame) => {
                    let child = &frame.children[frame.cursor];
                    let step = (
                        child.absolute.clone(),
                        child.relative().to_path_buf(),
                        frame.depth,
                    );
                    frame.cursor += 1;
                    Some(step)
                }
            };
            let Some((absolute, relative, depth)) = next else {
                stack.pop();
                continue;
            };

            let metadata = fs::symlink_metadata(&absolute).map_err(|error| {
                physical(
                    PhysicalRole::SourceDirectory,
                    tree.evidence(&relative),
                    io_refusal(error),
                )
            })?;
            let file_type = metadata.file_type();
            // Capture admits exactly the objects it opens. Following a link below
            // `src` would admit bytes at a name capture never opened, and skipping
            // one would drop whatever it names with no cause a consumer could see.
            // Refusing keeps a traversal cycle and an escape from the project
            // unrepresentable rather than merely unreached.
            if file_type.is_symlink() {
                return Err(physical(
                    PhysicalRole::SourceDirectory,
                    tree.evidence(&relative),
                    PhysicalRefusal::Link {
                        position: LinkPosition::Terminal,
                    },
                ));
            }
            if file_type.is_dir() {
                let child_depth = depth + 1;
                if child_depth > self.limits.traversal_depth {
                    return Err(physical(
                        PhysicalRole::SourceDirectory,
                        tree.evidence(&relative),
                        bound_refusal(
                            PhysicalBound::TraversalDepth,
                            self.limits.traversal_depth,
                            child_depth,
                        ),
                    ));
                }
                let admitted = self.admit(
                    tree,
                    &relative,
                    PhysicalRole::SourceDirectory,
                    PhysicalKind::Directory,
                )?;
                let subframe = self.enumerate(tree, relative, child_depth, admitted)?;
                stack.push(subframe);
            } else if has_mw_extension(&relative) {
                // Every entry occupying a module identity reaches the one source
                // owner, which classifies the terminal kind before opening it. A
                // special file there refuses as a wrong kind instead of leaving
                // the module it names missing without a cause.
                self.admit_source(tree, &relative)?;
            } else if tree.overlaid() {
                // An ignored entry (special file, or non-`.mw` regular file): counted
                // but never opened, and a wrong-role overlay member if named.
                self.overlay
                    .mark_wrong_role(&forward_slash_lossy(&relative));
            }
        }
        Ok(())
    }

    /// One atomic order-independent directory admission batch; see
    /// [`DirectoryAdmission`] for the bounds it settles.
    fn enumerate(
        &mut self,
        tree: &Tree,
        relative: PathBuf,
        depth: usize,
        dir: AdmittedObject,
    ) -> Result<DirectoryFrame, CaptureFailure> {
        let read_dir = fs::read_dir(dir.absolute()).map_err(|error| {
            physical(
                PhysicalRole::SourceDirectory,
                tree.evidence(&relative),
                io_refusal(error),
            )
        })?;
        let children = DirectoryAdmission::settle(
            read_dir.map(|entry| entry.map(|entry| entry.path())),
            tree,
            &relative,
            &mut self.budget,
            self.limits,
            &mut self.visited,
        )?;
        Ok(DirectoryFrame {
            depth,
            children,
            cursor: 0,
            _dir: dir,
        })
    }

    /// Admit one selected `.mw` source: file-count check, opened-handle admission,
    /// borrowed spelling, allocation-free check, valid-only spelling bound, checked
    /// materialization, pure validation, then overlay or disk bytes.
    fn admit_source(&mut self, tree: &Tree, relative: &Path) -> Result<(), CaptureFailure> {
        if self.files.len() >= self.limits.source.max_files() {
            // The file-count bound fires before opening the next file; it joins the
            // caller root to the offending path. The count spans every admitted
            // tree, so the offender may sit in a dependency.
            return Err(physical(
                PhysicalRole::SourceFile,
                tree.evidence(relative),
                bound_refusal(
                    PhysicalBound::SourceFiles,
                    self.limits.source.max_files(),
                    self.files.len() + 1,
                ),
            ));
        }

        let Some(spelling) = forward_slash_checked(relative) else {
            let evidence = tree.evidence(relative);
            let _lease = self.lease(PhysicalRole::SourceFile, &evidence)?;
            return Err(physical(
                PhysicalRole::SourceFile,
                evidence,
                PhysicalRefusal::InvalidPathEncoding,
            ));
        };

        // The pure identity-byte maximum is enforced on the borrowed spelling before
        // any native-path lease or opened handle: a valid over-long spelling forwards
        // the sealed pathless pure Capture family and materializes no path. Only that
        // case forwards; a syntactically invalid spelling stays deferred to pure
        // capture, which keeps `project.source_path` precedence. The opaque error is
        // forwarded unmatched: this adapter neither inspects nor reclassifies it.
        CapturedFile::check_identity_bound(&spelling).map_err(CaptureFailure::from_project)?;

        let mut admitted = self.admit(
            tree,
            relative,
            PhysicalRole::SourceFile,
            PhysicalKind::RegularFile,
        )?;

        let valid_spelling = FileIdentity::check(&spelling).is_ok();

        // Overlay membership decides the body: an exact member replaces the disk body
        // and never reads it; a pure-invalid spelling always takes the disk path so
        // pure capture keeps `project.source_path` precedence. A dependency is never
        // overlaid: it is read exactly as it is committed.
        let overlay_bytes = if valid_spelling && tree.overlaid() {
            match FileIdentity::validate(&spelling).ok() {
                Some((identity, _module)) => self.overlay.accept_source(&identity)?,
                None => None,
            }
        } else {
            None
        };
        let bytes = match overlay_bytes {
            Some(bytes) => {
                admitted.recheck_identity()?;
                bytes
            }
            None => self.read_disk_body(&mut admitted)?,
        };

        self.files
            .try_reserve_exact(1)
            .map_err(|_| pathless(PhysicalRole::SourceFile, oom_refusal()))?;
        self.files.push(tree.captured(spelling, bytes));
        Ok(())
    }

    fn read_disk_body(&mut self, admitted: &mut AdmittedObject) -> Result<Vec<u8>, CaptureFailure> {
        let bytes = admitted.read_bounded(
            ReadBudget::new(
                PhysicalBound::SourceFileBytes,
                self.limits.source.max_file_bytes(),
            ),
            Some(ReadBudget::after(
                PhysicalBound::SourceTotalBytes,
                self.limits.source.max_total_bytes(),
                self.total_bytes,
            )),
        )?;
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        Ok(bytes)
    }
}

// ===== Physical admission =====================================================

#[derive(Clone, Copy, PartialEq, Eq)]
struct ObjectIdentity {
    dev: u64,
    ino: u64,
    kind: PhysicalKind,
    nlink: u64,
}

impl ObjectIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            kind: classify_kind(metadata),
            nlink: metadata.nlink(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DiskContentLength(u64);

/// One admitted opened object: its handle, canonical path, caller-root-relative
/// evidence spelling with its lease, role, and pre-observed identity separated from
/// disk-content length.
struct AdmittedObject {
    file: File,
    absolute: PathBuf,
    evidence: PathBuf,
    _lease: PathLease,
    role: PhysicalRole,
    identity: ObjectIdentity,
}

impl AdmittedObject {
    fn absolute(&self) -> &Path {
        &self.absolute
    }

    fn read_bounded(
        &mut self,
        primary: ReadBudget,
        aggregate: Option<ReadBudget>,
    ) -> Result<Vec<u8>, CaptureFailure> {
        let admitted_length = self
            .file
            .metadata()
            .map(|metadata| DiskContentLength(metadata.len()))
            .map_err(|error| self.io_failure(error))?;
        let bytes = read_bounded_loop(
            &mut self.file,
            self.role,
            &self.evidence,
            primary,
            aggregate,
        )?;
        self.recheck_disk_backed(admitted_length)?;
        Ok(bytes)
    }

    fn recheck_identity(&self) -> Result<(), CaptureFailure> {
        let (handle, path) = self.checkpoint()?;
        if ObjectIdentity::from_metadata(&handle) != self.identity
            || ObjectIdentity::from_metadata(&path) != self.identity
        {
            return Err(self.changed());
        }
        Ok(())
    }

    fn recheck_disk_backed(
        &self,
        admitted_length: DiskContentLength,
    ) -> Result<(), CaptureFailure> {
        let (handle, path) = self.checkpoint()?;
        if ObjectIdentity::from_metadata(&handle) != self.identity
            || ObjectIdentity::from_metadata(&path) != self.identity
            || DiskContentLength(handle.len()) != admitted_length
            || DiskContentLength(path.len()) != admitted_length
        {
            return Err(self.changed());
        }
        Ok(())
    }

    fn checkpoint(&self) -> Result<(Metadata, Metadata), CaptureFailure> {
        let handle = self
            .file
            .metadata()
            .map_err(|error| self.io_failure(error))?;
        let path = fs::symlink_metadata(&self.absolute).map_err(|error| self.io_failure(error))?;
        Ok((handle, path))
    }

    fn changed(&self) -> CaptureFailure {
        physical(self.role, self.evidence.clone(), PhysicalRefusal::Changed)
    }

    fn io_failure(&self, error: io::Error) -> CaptureFailure {
        physical(self.role, self.evidence.clone(), io_refusal(error))
    }

    fn decode_utf8(&self, bytes: Vec<u8>) -> Result<String, CaptureFailure> {
        String::from_utf8(bytes).map_err(|_| {
            physical(
                self.role,
                self.evidence.clone(),
                PhysicalRefusal::Io {
                    error: PhysicalIoError::new(io::Error::from(io::ErrorKind::InvalidData)),
                },
            )
        })
    }
}

/// Resolve one declared dependency path from the consuming project's canonical
/// root. A leading `..` run pops that canonical path, which is sound because it
/// holds no link; every descending segment is then inspected without following
/// links, so a dependency cannot be reached through one. The result is canonical by
/// construction and is never handed to `canonicalize`, which would follow a link
/// silently.
fn resolve_dependency(root: &Path, declared: &Dependency) -> Result<PathBuf, PhysicalRefusal> {
    let mut current = root.to_path_buf();
    let mut segments = declared.path().segments().peekable();
    while let Some(segment) = segments.next() {
        if segment == ".." {
            if !current.pop() {
                return Err(PhysicalRefusal::Missing {
                    error: PhysicalIoError::new(io::Error::from(io::ErrorKind::NotFound)),
                });
            }
            continue;
        }
        current.push(segment);
        let metadata = fs::symlink_metadata(&current).map_err(io_refusal)?;
        if metadata.file_type().is_symlink() {
            return Err(PhysicalRefusal::Link {
                position: if segments.peek().is_none() {
                    LinkPosition::Terminal
                } else {
                    LinkPosition::Intermediate
                },
            });
        }
        if !metadata.is_dir() {
            return Err(PhysicalRefusal::UnexpectedKind {
                expected: PhysicalKind::Directory,
            });
        }
    }
    if current == root {
        return Err(PhysicalRefusal::Dependency {
            reason: DependencyRefusal::SelfReference,
        });
    }
    Ok(current)
}

/// Inspect each relative component without following links: a symlink or wrong-kind
/// intermediate refuses, a missing component is `Missing`.
fn inspect_components(root: &Path, relative: &Path) -> Result<(), PhysicalRefusal> {
    let mut components = relative.components().peekable();
    let mut current = root.to_path_buf();
    while let Some(component) = components.next() {
        let terminal = components.peek().is_none();
        let Component::Normal(segment) = component else {
            return Err(PhysicalRefusal::Changed);
        };
        current.push(segment);
        let metadata = fs::symlink_metadata(&current).map_err(io_refusal)?;
        if metadata.file_type().is_symlink() {
            return Err(PhysicalRefusal::Link {
                position: if terminal {
                    LinkPosition::Terminal
                } else {
                    LinkPosition::Intermediate
                },
            });
        }
        if !terminal && !metadata.is_dir() {
            return Err(PhysicalRefusal::UnexpectedKind {
                expected: PhysicalKind::Directory,
            });
        }
    }
    Ok(())
}

/// Open and admit a terminal object: verify kind and (for a regular file) `nlink == 1`
/// before opening, then confirm the opened handle's identity matches.
fn open_terminal(
    absolute: &Path,
    expected: PhysicalKind,
) -> Result<(File, ObjectIdentity), PhysicalRefusal> {
    let before = fs::symlink_metadata(absolute).map_err(io_refusal)?;
    if before.file_type().is_symlink() {
        return Err(PhysicalRefusal::Link {
            position: LinkPosition::Terminal,
        });
    }
    let actual = classify_kind(&before);
    if actual != expected {
        return Err(PhysicalRefusal::UnexpectedKind { expected });
    }
    let identity = ObjectIdentity::from_metadata(&before);
    if expected == PhysicalKind::RegularFile && identity.nlink != 1 {
        return Err(PhysicalRefusal::Hardlink);
    }
    let file = File::open(absolute).map_err(io_refusal)?;
    let opened = file.metadata().map_err(io_refusal)?;
    if ObjectIdentity::from_metadata(&opened) != identity {
        return Err(PhysicalRefusal::Changed);
    }
    Ok((file, identity))
}

fn read_bounded_loop(
    reader: &mut impl Read,
    role: PhysicalRole,
    relative: &Path,
    primary: ReadBudget,
    aggregate: Option<ReadBudget>,
) -> Result<Vec<u8>, CaptureFailure> {
    let effective_remaining = aggregate.map_or(primary.remaining(), |budget| {
        primary.remaining().min(budget.remaining())
    });
    let mut bytes = Vec::new();
    let mut chunk = [0u8; READ_CHUNK_BYTES];
    loop {
        let remaining_plus_one = effective_remaining
            .saturating_add(1)
            .saturating_sub(bytes.len());
        if remaining_plus_one == 0 {
            return Err(read_bound(role, relative, primary, bytes.len()));
        }
        let request = remaining_plus_one.min(READ_CHUNK_BYTES);
        let read = loop {
            match reader.read(&mut chunk[..request]) {
                Ok(read) => break read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    return Err(physical(role, relative.to_path_buf(), io_refusal(error)));
                }
            }
        };
        if read == 0 {
            return Ok(bytes);
        }
        bytes
            .try_reserve_exact(read)
            .map_err(|_| physical(role, relative.to_path_buf(), oom_refusal()))?;
        bytes.extend_from_slice(&chunk[..read]);
        if primary.actual(bytes.len()) > primary.limit {
            return Err(read_bound(role, relative, primary, bytes.len()));
        }
        if let Some(aggregate) = aggregate
            && aggregate.actual(bytes.len()) > aggregate.limit
        {
            return Err(read_bound(role, relative, aggregate, bytes.len()));
        }
    }
}

// ===== Read budget ============================================================

#[derive(Clone, Copy)]
struct ReadBudget {
    bound: PhysicalBound,
    limit: usize,
    already_used: usize,
}

impl ReadBudget {
    fn new(bound: PhysicalBound, limit: usize) -> Self {
        Self {
            bound,
            limit,
            already_used: 0,
        }
    }

    fn after(bound: PhysicalBound, limit: usize, already_used: usize) -> Self {
        Self {
            bound,
            limit,
            already_used,
        }
    }

    fn remaining(self) -> usize {
        self.limit.saturating_sub(self.already_used)
    }

    fn actual(self, additional: usize) -> usize {
        self.already_used.saturating_add(additional)
    }
}

// ===== Failure and classification helpers =====================================

fn classify_kind(metadata: &Metadata) -> PhysicalKind {
    if metadata.is_file() {
        PhysicalKind::RegularFile
    } else if metadata.is_dir() {
        PhysicalKind::Directory
    } else {
        PhysicalKind::Other
    }
}

fn io_refusal(error: io::Error) -> PhysicalRefusal {
    if error.kind() == io::ErrorKind::NotFound {
        PhysicalRefusal::Missing {
            error: PhysicalIoError::new(error),
        }
    } else {
        PhysicalRefusal::Io {
            error: PhysicalIoError::new(error),
        }
    }
}

fn oom_refusal() -> PhysicalRefusal {
    PhysicalRefusal::Io {
        error: PhysicalIoError::new(io::Error::from(io::ErrorKind::OutOfMemory)),
    }
}

fn bound_refusal(bound: PhysicalBound, limit: usize, actual: usize) -> PhysicalRefusal {
    PhysicalRefusal::Bound {
        bound,
        limit,
        actual,
    }
}

fn reserve_refusal(error: ReserveError) -> PhysicalRefusal {
    match error {
        ReserveError::Retained { limit, actual } => {
            bound_refusal(PhysicalBound::RetainedPathUnits, limit, actual)
        }
        ReserveError::Work { limit, actual } => {
            bound_refusal(PhysicalBound::PathWorkUnits, limit, actual)
        }
        ReserveError::Overflow => oom_refusal(),
    }
}

fn dir_oom() -> CaptureFailure {
    pathless(PhysicalRole::SourceDirectory, oom_refusal())
}

fn physical(role: PhysicalRole, path: PathBuf, refusal: PhysicalRefusal) -> CaptureFailure {
    CaptureFailure::from_physical(PhysicalFailure {
        role,
        path: Some(path),
        refusal,
    })
}

fn pathless(role: PhysicalRole, refusal: PhysicalRefusal) -> CaptureFailure {
    CaptureFailure::from_physical(PhysicalFailure {
        role,
        path: None,
        refusal,
    })
}

fn root_io(error: io::Error) -> CaptureFailure {
    pathless(PhysicalRole::Root, io_refusal(error))
}

fn read_bound(
    role: PhysicalRole,
    relative: &Path,
    budget: ReadBudget,
    additional: usize,
) -> CaptureFailure {
    physical(
        role,
        relative.to_path_buf(),
        bound_refusal(budget.bound, budget.limit, budget.actual(additional)),
    )
}

/// Only `NotFound` means an absent optional role; any other error is admitted and
/// reclassified by the following admission.
fn optional_absent(absolute: &Path) -> bool {
    matches!(fs::symlink_metadata(absolute), Err(error) if error.kind() == io::ErrorKind::NotFound)
}

fn has_mw_extension(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str())
        == Some(marrow_project::SOURCE_EXTENSION)
}

/// The forward-slash root-relative spelling of a path whose components are all valid
/// UTF-8, or `None` when a component is not valid UTF-8.
fn forward_slash_checked(relative: &Path) -> Option<String> {
    let mut segments: Vec<&str> = Vec::new();
    for component in relative.components() {
        if let Component::Normal(name) = component {
            segments.push(name.to_str()?);
        }
    }
    Some(segments.join("/"))
}

/// The lossy forward-slash spelling for ignored-entry overlay marking.
fn forward_slash_lossy(relative: &Path) -> String {
    relative.to_string_lossy().replace('\\', "/")
}
