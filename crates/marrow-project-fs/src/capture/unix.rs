//! The Linux/macOS physical capture implementation: opened-handle admission, one
//! bounded iterative source traversal, and pure-owner composition.

use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use marrow_project::{CapturedFile, FileIdentity, Manifest, ProjectInput};

use crate::failure::{
    CaptureFailure, LinkPosition, PhysicalBound, PhysicalFailure, PhysicalIoError, PhysicalKind,
    PhysicalOperation, PhysicalRefusal, PhysicalRole,
};
use crate::limits::AdapterLimits;
use crate::overlay::OverlaySnapshot;
use crate::path::{
    CanonicalRoot, LiveNativePath, OperationalPath, PathBudget, ReserveError, SourceGuard,
    native_units,
};

const MANIFEST_FILE: &str = "marrow.toml";
const SOURCE_DIR: &str = "src";
const READ_CHUNK_BYTES: usize = 8 * 1024;

/// A bare admission refusal a primitive returns; the caller attaches the leased or
/// pathless evidence and the role.
type BareRefusal = (PhysicalOperation, PhysicalRefusal);

pub(super) fn capture(
    root: &Path,
    mut overlay: OverlaySnapshot<'_>,
    limits: &AdapterLimits,
) -> Result<ProjectInput, CaptureFailure> {
    let mut budget = PathBudget::new();

    let root_admission = admit_root(root, &mut budget, limits)?;
    let canonical = root_admission.canonical.as_path().to_path_buf();

    let manifest = manifest_stage(&canonical, &mut budget, limits, &mut overlay)?;
    let batch = source_stage(&canonical, &mut budget, limits, &mut overlay)?;
    let ids = ledger_stage(&canonical, &mut budget, limits, &mut overlay)?;

    root_admission.recheck()?;

    let (files, guards) = batch.into_parts();
    let input = marrow_project::capture(&manifest, files, ids.as_deref(), &limits.source)
        .map_err(CaptureFailure::from_project)?;
    // The guards keep every source native-path lease live across the pure-capture
    // call; only now may they drop.
    drop(guards);

    // Only after a successful pure capture: settle the lowest-original unmatched
    // overlay entry.
    overlay
        .settle()
        .map_err(CaptureFailure::from_overlay_input)?;

    Ok(input)
}

// ===== Composition stages =====================================================

fn manifest_stage(
    canonical: &Path,
    budget: &mut PathBudget,
    limits: &AdapterLimits,
    overlay: &mut OverlaySnapshot<'_>,
) -> Result<Manifest, CaptureFailure> {
    overlay.mark_wrong_role(MANIFEST_FILE);
    let live = reserve_fixed(
        budget,
        limits,
        PhysicalRole::Manifest,
        PathBuf::from(MANIFEST_FILE),
    )?;
    let mut admitted = admit_relative(
        canonical,
        live,
        PhysicalRole::Manifest,
        PhysicalKind::RegularFile,
    )?;
    let bytes = admitted.read_bounded(
        ReadBudget::new(PhysicalBound::ManifestBytes, limits.manifest_bytes),
        None,
    )?;
    let source = admitted.decode_utf8(bytes)?;
    Manifest::parse(&source).map_err(CaptureFailure::from_manifest)
}

fn ledger_stage(
    canonical: &Path,
    budget: &mut PathBudget,
    limits: &AdapterLimits,
    overlay: &mut OverlaySnapshot<'_>,
) -> Result<Option<Vec<u8>>, CaptureFailure> {
    overlay.mark_wrong_role(marrow_project::IDS_FILE);
    if optional_absent(&canonical.join(marrow_project::IDS_FILE)) {
        return Ok(None);
    }
    let live = reserve_fixed(
        budget,
        limits,
        PhysicalRole::IdentityLedger,
        PathBuf::from(marrow_project::IDS_FILE),
    )?;
    let mut admitted = admit_relative(
        canonical,
        live,
        PhysicalRole::IdentityLedger,
        PhysicalKind::RegularFile,
    )?;
    let bytes = admitted.read_bounded(
        ReadBudget::new(
            PhysicalBound::IdentityLedgerBytes,
            limits.identity_ledger_bytes,
        ),
        None,
    )?;
    Ok(Some(bytes))
}

fn source_stage(
    canonical: &Path,
    budget: &mut PathBudget,
    limits: &AdapterLimits,
    overlay: &mut OverlaySnapshot<'_>,
) -> Result<CapturedBatch, CaptureFailure> {
    overlay.mark_wrong_role(SOURCE_DIR);
    if optional_absent(&canonical.join(SOURCE_DIR)) {
        return Ok(CapturedBatch::new());
    }
    let live = reserve_fixed(
        budget,
        limits,
        PhysicalRole::SourceRoot,
        PathBuf::from(SOURCE_DIR),
    )?;
    let root_dir = admit_relative(
        canonical,
        live,
        PhysicalRole::SourceRoot,
        PhysicalKind::Directory,
    )?;

    let mut traversal = Traversal {
        canonical,
        budget,
        limits,
        overlay,
        visited: 0,
        total_bytes: 0,
        batch: CapturedBatch::new(),
    };
    traversal.run(root_dir)?;
    Ok(traversal.batch)
}

// ===== Iterative bounded source traversal =====================================

struct Traversal<'a, 'o> {
    canonical: &'a Path,
    budget: &'a mut PathBudget,
    limits: &'a AdapterLimits,
    overlay: &'a mut OverlaySnapshot<'o>,
    visited: usize,
    total_bytes: usize,
    batch: CapturedBatch,
}

struct DirectoryFrame {
    depth: usize,
    children: Vec<Child>,
    cursor: usize,
    _dir: AdmittedObject,
}

pub(crate) struct Child {
    absolute: PathBuf,
    relative: PathBuf,
    _lease: LiveNativePath,
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
/// The settlement is atomic through the `Result` boundary rather than a mutable
/// typestate: a partially settled batch is unrepresentable because every refusal
/// returns before any commit, so a refused batch leaves `visited`, `work`, and the
/// live counter at their baseline and the caller receives either the fully staged
/// carriers or a pathless refusal.
pub(crate) struct DirectoryAdmission;

impl DirectoryAdmission {
    pub(crate) fn settle(
        entries: impl Iterator<Item = io::Result<PathBuf>>,
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
                    PhysicalOperation::Enumerate,
                    OperationalPath::new(relative.to_path_buf()),
                    io_refusal(error),
                )
            })?;
            if count >= remaining {
                // The (remaining + 1)th successful entry: drop every provisional
                // carrier and report the pathless visited-entry bound. Count-first
                // applies only because this extra success was observed.
                return Err(pathless(
                    PhysicalRole::SourceDirectory,
                    PhysicalOperation::Enumerate,
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
                PhysicalOperation::Retain,
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
                PhysicalOperation::Retain,
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
                .map_err(|error| {
                    pathless(
                        PhysicalRole::SourceDirectory,
                        PhysicalOperation::Retain,
                        reserve_refusal(error),
                    )
                })?;
            let name = absolute.file_name().map(PathBuf::from).unwrap_or_default();
            children.push(Child {
                relative: relative.join(&name),
                _lease: LiveNativePath::new(absolute.clone(), lease),
                absolute,
            });
        }
        budget.commit_work(aggregate).map_err(|_| dir_oom())?;
        *visited += count;
        children.sort_unstable_by(|a, b| a.absolute.cmp(&b.absolute));
        Ok(children)
    }
}

impl Traversal<'_, '_> {
    fn run(&mut self, root_dir: AdmittedObject) -> Result<(), CaptureFailure> {
        let mut stack: Vec<DirectoryFrame> =
            vec![self.enumerate(PathBuf::from(SOURCE_DIR), 0, root_dir)?];

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
                    PhysicalOperation::Inspect,
                    OperationalPath::new(relative.clone()),
                    io_refusal(error),
                )
            })?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                let child_depth = depth + 1;
                if child_depth > self.limits.traversal_depth {
                    return Err(physical(
                        PhysicalRole::SourceDirectory,
                        PhysicalOperation::Enumerate,
                        OperationalPath::new(relative),
                        bound_refusal(
                            PhysicalBound::TraversalDepth,
                            self.limits.traversal_depth,
                            child_depth,
                        ),
                    ));
                }
                let live = reserve_fixed(
                    self.budget,
                    self.limits,
                    PhysicalRole::SourceDirectory,
                    relative.clone(),
                )?;
                let admitted = admit_relative(
                    self.canonical,
                    live,
                    PhysicalRole::SourceDirectory,
                    PhysicalKind::Directory,
                )?;
                let subframe = self.enumerate(relative, child_depth, admitted)?;
                stack.push(subframe);
            } else if file_type.is_file() && has_mw_extension(&relative) {
                self.admit_source(&relative)?;
            } else {
                // An ignored entry (special file, or non-`.mw` regular file): counted
                // but never opened, and a wrong-role overlay member if named.
                self.overlay
                    .mark_wrong_role(&forward_slash_lossy(&relative));
            }
        }
        Ok(())
    }

    /// One atomic order-independent directory admission batch: count at most the
    /// remaining visit allowance plus one, measure the aggregate carrier units
    /// commutatively, settle the aggregate bounds once (retained wins), commit
    /// visited/work once, and sort the carriers in native lexical order.
    fn enumerate(
        &mut self,
        relative: PathBuf,
        depth: usize,
        dir: AdmittedObject,
    ) -> Result<DirectoryFrame, CaptureFailure> {
        let read_dir = fs::read_dir(dir.absolute()).map_err(|error| {
            physical(
                PhysicalRole::SourceDirectory,
                PhysicalOperation::Enumerate,
                OperationalPath::new(relative.clone()),
                io_refusal(error),
            )
        })?;
        let children = DirectoryAdmission::settle(
            read_dir.map(|entry| entry.map(|entry| entry.path())),
            &relative,
            &mut *self.budget,
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
    fn admit_source(&mut self, relative: &Path) -> Result<(), CaptureFailure> {
        if self.batch.len() >= self.limits.source.max_files() {
            // The file-count bound fires before opening the next file; it joins the
            // caller root to the offending path.
            return Err(physical(
                PhysicalRole::SourceFile,
                PhysicalOperation::Retain,
                OperationalPath::new(relative.to_path_buf()),
                bound_refusal(
                    PhysicalBound::SourceFiles,
                    self.limits.source.max_files(),
                    self.batch.len() + 1,
                ),
            ));
        }

        let Some(spelling) = forward_slash_checked(relative) else {
            let live = reserve_fixed(
                self.budget,
                self.limits,
                PhysicalRole::SourceFile,
                relative.to_path_buf(),
            )?;
            return Err(physical(
                PhysicalRole::SourceFile,
                PhysicalOperation::Inspect,
                live.into_operational(),
                PhysicalRefusal::InvalidPathEncoding,
            ));
        };

        // The pure identity-byte maximum is enforced on the borrowed spelling before
        // any native-path lease or opened handle: a valid over-long spelling forwards
        // the sealed pathless pure Capture family and materializes no path. Only that
        // case forwards; a syntactically invalid spelling stays deferred to pure
        // capture, which keeps `project.source_path` precedence. The opaque error is
        // forwarded unmatched — CAP neither inspects the reason nor reclassifies it.
        CapturedFile::check_identity_bound(&spelling).map_err(CaptureFailure::from_project)?;

        let live = reserve_fixed(
            self.budget,
            self.limits,
            PhysicalRole::SourceFile,
            relative.to_path_buf(),
        )?;
        let mut admitted = admit_relative(
            self.canonical,
            live,
            PhysicalRole::SourceFile,
            PhysicalKind::RegularFile,
        )?;

        let valid_spelling = FileIdentity::check(&spelling).is_ok();

        // Overlay membership decides the body: an exact member replaces the disk body
        // and never reads it; a pure-invalid spelling always takes the disk path so
        // pure capture keeps `project.source_path` precedence.
        let overlay_bytes = if valid_spelling {
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

        let guard = admitted.into_guard();
        self.batch
            .push(CapturedFile::new(spelling, bytes), guard)
            .map_err(|error| {
                pathless(
                    PhysicalRole::SourceFile,
                    PhysicalOperation::Retain,
                    reserve_refusal(error),
                )
            })
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

/// One admitted opened object: its handle, canonical path, live leased path, role,
/// and pre-observed identity separated from disk-content length.
struct AdmittedObject {
    file: File,
    absolute: PathBuf,
    live: LiveNativePath,
    role: PhysicalRole,
    identity: ObjectIdentity,
}

impl AdmittedObject {
    fn absolute(&self) -> &Path {
        &self.absolute
    }

    fn relative(&self) -> &Path {
        self.live.as_path()
    }

    fn into_guard(self) -> SourceGuard {
        self.live.into_guard()
    }

    /// Consume into terminal failure evidence, transferring the live charge.
    fn into_operational(self) -> OperationalPath {
        self.live.into_operational()
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
            .map_err(|error| self.io_failure(PhysicalOperation::Recheck, error))?;
        // Disjoint field borrows: the read buffer borrows `file`, the evidence borrows
        // `live`.
        let bytes = read_bounded_loop(
            &mut self.file,
            self.role,
            self.live.as_path(),
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
            .map_err(|error| self.io_failure(PhysicalOperation::Recheck, error))?;
        let path = fs::symlink_metadata(&self.absolute)
            .map_err(|error| self.io_failure(PhysicalOperation::Recheck, error))?;
        Ok((handle, path))
    }

    fn changed(&self) -> CaptureFailure {
        physical(
            self.role,
            PhysicalOperation::Recheck,
            OperationalPath::new(self.relative().to_path_buf()),
            PhysicalRefusal::Changed,
        )
    }

    fn io_failure(&self, operation: PhysicalOperation, error: io::Error) -> CaptureFailure {
        physical(
            self.role,
            operation,
            OperationalPath::new(self.relative().to_path_buf()),
            io_refusal(error),
        )
    }

    fn decode_utf8(&self, bytes: Vec<u8>) -> Result<String, CaptureFailure> {
        String::from_utf8(bytes).map_err(|_| {
            physical(
                self.role,
                PhysicalOperation::Read,
                OperationalPath::new(self.relative().to_path_buf()),
                PhysicalRefusal::Io {
                    error: PhysicalIoError::new(io::Error::from(io::ErrorKind::InvalidData)),
                },
            )
        })
    }
}

/// The opened, rechecked project root: its handle and identity are held through
/// capture and rechecked before return; its charge stays live via `CanonicalRoot`.
struct RootAdmission {
    canonical: CanonicalRoot,
    handle: File,
    identity: ObjectIdentity,
}

impl RootAdmission {
    fn recheck(&self) -> Result<(), CaptureFailure> {
        let handle = self
            .handle
            .metadata()
            .map_err(|error| root_io(PhysicalOperation::Recheck, error))?;
        let path = fs::symlink_metadata(self.canonical.as_path())
            .map_err(|error| root_io(PhysicalOperation::Recheck, error))?;
        if ObjectIdentity::from_metadata(&handle) != self.identity
            || ObjectIdentity::from_metadata(&path) != self.identity
        {
            return Err(pathless(
                PhysicalRole::Root,
                PhysicalOperation::Recheck,
                PhysicalRefusal::Changed,
            ));
        }
        Ok(())
    }
}

fn admit_root(
    root: &Path,
    budget: &mut PathBudget,
    limits: &AdapterLimits,
) -> Result<RootAdmission, CaptureFailure> {
    // Caller-root work charge before canonicalization.
    budget
        .charge_work(native_units(root), limits.max_path_work_units)
        .map_err(|error| {
            pathless(
                PhysicalRole::Root,
                PhysicalOperation::Retain,
                reserve_refusal(error),
            )
        })?;
    let canonical =
        fs::canonicalize(root).map_err(|error| root_io(PhysicalOperation::Canonicalize, error))?;
    let lease = budget
        .reserve(
            native_units(&canonical),
            limits.max_retained_path_units,
            limits.max_path_work_units,
        )
        .map_err(|error| {
            pathless(
                PhysicalRole::Root,
                PhysicalOperation::Retain,
                reserve_refusal(error),
            )
        })?;
    let canonical_root = CanonicalRoot::new(canonical.clone(), lease);

    let (handle, identity) = open_terminal(&canonical, PhysicalKind::Directory)
        .map_err(|(operation, refusal)| pathless(PhysicalRole::Root, operation, refusal))?;
    Ok(RootAdmission {
        canonical: canonical_root,
        handle,
        identity,
    })
}

/// Admit a role-relative object: inspect every component without following links,
/// then admit the terminal object with an opened handle whose identity matches. On
/// any refusal the leased path terminalizes into the failure evidence.
fn admit_relative(
    canonical_root: &Path,
    live: LiveNativePath,
    role: PhysicalRole,
    expected: PhysicalKind,
) -> Result<AdmittedObject, CaptureFailure> {
    let relative = live.as_path().to_path_buf();
    if let Err((operation, refusal)) = inspect_components(canonical_root, &relative, expected) {
        return Err(physical(role, operation, live.into_operational(), refusal));
    }
    let absolute = canonical_root.join(&relative);
    match open_terminal(&absolute, expected) {
        Ok((file, identity)) => Ok(AdmittedObject {
            file,
            absolute,
            live,
            role,
            identity,
        }),
        Err((operation, refusal)) => {
            Err(physical(role, operation, live.into_operational(), refusal))
        }
    }
}

/// Inspect each relative component without following links: a symlink or wrong-kind
/// intermediate refuses, a missing component is `Missing`.
fn inspect_components(
    root: &Path,
    relative: &Path,
    _terminal_kind: PhysicalKind,
) -> Result<(), BareRefusal> {
    let mut components = relative.components().peekable();
    let mut current = root.to_path_buf();
    while let Some(component) = components.next() {
        let terminal = components.peek().is_none();
        let Component::Normal(segment) = component else {
            return Err((PhysicalOperation::Inspect, PhysicalRefusal::Changed));
        };
        current.push(segment);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| (PhysicalOperation::Inspect, io_refusal(error)))?;
        if metadata.file_type().is_symlink() {
            return Err((
                PhysicalOperation::Inspect,
                PhysicalRefusal::Link {
                    position: if terminal {
                        LinkPosition::Terminal
                    } else {
                        LinkPosition::Intermediate
                    },
                },
            ));
        }
        if !terminal && !metadata.is_dir() {
            return Err((
                PhysicalOperation::Inspect,
                PhysicalRefusal::UnexpectedKind {
                    expected: PhysicalKind::Directory,
                    actual: classify_kind(&metadata),
                },
            ));
        }
    }
    Ok(())
}

/// Open and admit a terminal object: verify kind and (for a regular file) `nlink == 1`
/// before opening, then confirm the opened handle's identity matches.
fn open_terminal(
    absolute: &Path,
    expected: PhysicalKind,
) -> Result<(File, ObjectIdentity), BareRefusal> {
    let before = fs::symlink_metadata(absolute)
        .map_err(|error| (PhysicalOperation::Inspect, io_refusal(error)))?;
    if before.file_type().is_symlink() {
        return Err((
            PhysicalOperation::Inspect,
            PhysicalRefusal::Link {
                position: LinkPosition::Terminal,
            },
        ));
    }
    let actual = classify_kind(&before);
    if actual != expected {
        return Err((
            PhysicalOperation::Inspect,
            PhysicalRefusal::UnexpectedKind { expected, actual },
        ));
    }
    let identity = ObjectIdentity::from_metadata(&before);
    if expected == PhysicalKind::RegularFile && identity.nlink != 1 {
        return Err((PhysicalOperation::Inspect, PhysicalRefusal::Hardlink));
    }
    let file =
        File::open(absolute).map_err(|error| (PhysicalOperation::Open, io_refusal(error)))?;
    let opened = file
        .metadata()
        .map_err(|error| (PhysicalOperation::Open, io_refusal(error)))?;
    if ObjectIdentity::from_metadata(&opened) != identity {
        return Err((PhysicalOperation::Open, PhysicalRefusal::Changed));
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
                    return Err(physical(
                        role,
                        PhysicalOperation::Read,
                        OperationalPath::new(relative.to_path_buf()),
                        io_refusal(error),
                    ));
                }
            }
        };
        if read == 0 {
            return Ok(bytes);
        }
        bytes.try_reserve_exact(read).map_err(|_| {
            physical(
                role,
                PhysicalOperation::Read,
                OperationalPath::new(relative.to_path_buf()),
                oom_refusal(),
            )
        })?;
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

// ===== Captured batch =========================================================

/// The one-to-one captured files and live source guards. Both vectors are checked
/// before a spelling relinquishes its owner, and the guards stay live across the
/// synchronous pure-capture call.
struct CapturedBatch {
    files: Vec<CapturedFile>,
    guards: Vec<SourceGuard>,
}

impl CapturedBatch {
    fn new() -> Self {
        Self {
            files: Vec::new(),
            guards: Vec::new(),
        }
    }

    fn len(&self) -> usize {
        self.files.len()
    }

    fn push(&mut self, file: CapturedFile, guard: SourceGuard) -> Result<(), ReserveError> {
        self.files
            .try_reserve_exact(1)
            .map_err(|_| ReserveError::Overflow)?;
        self.guards
            .try_reserve_exact(1)
            .map_err(|_| ReserveError::Overflow)?;
        self.files.push(file);
        self.guards.push(guard);
        Ok(())
    }

    fn into_parts(self) -> (Vec<CapturedFile>, Vec<SourceGuard>) {
        (self.files, self.guards)
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
    pathless(
        PhysicalRole::SourceDirectory,
        PhysicalOperation::Retain,
        oom_refusal(),
    )
}

fn physical(
    role: PhysicalRole,
    operation: PhysicalOperation,
    path: OperationalPath,
    refusal: PhysicalRefusal,
) -> CaptureFailure {
    CaptureFailure::from_physical(PhysicalFailure::new(role, operation, Some(path), refusal))
}

fn pathless(
    role: PhysicalRole,
    operation: PhysicalOperation,
    refusal: PhysicalRefusal,
) -> CaptureFailure {
    CaptureFailure::from_physical(PhysicalFailure::new(role, operation, None, refusal))
}

fn root_io(operation: PhysicalOperation, error: io::Error) -> CaptureFailure {
    pathless(PhysicalRole::Root, operation, io_refusal(error))
}

fn read_bound(
    role: PhysicalRole,
    relative: &Path,
    budget: ReadBudget,
    additional: usize,
) -> CaptureFailure {
    physical(
        role,
        PhysicalOperation::Read,
        OperationalPath::new(relative.to_path_buf()),
        bound_refusal(budget.bound, budget.limit, budget.actual(additional)),
    )
}

fn reserve_fixed(
    budget: &mut PathBudget,
    limits: &AdapterLimits,
    role: PhysicalRole,
    relative: PathBuf,
) -> Result<LiveNativePath, CaptureFailure> {
    let units = native_units(&relative);
    let lease = budget
        .reserve(
            units,
            limits.max_retained_path_units,
            limits.max_path_work_units,
        )
        .map_err(|error| pathless(role, PhysicalOperation::Retain, reserve_refusal(error)))?;
    Ok(LiveNativePath::new(relative, lease))
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
