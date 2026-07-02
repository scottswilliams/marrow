use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_check::evolution::{RepairDiagnostic, preview};
use marrow_check::tooling;
use marrow_check::{
    AnalysisGeneration, AnalysisIdentity, AnalysisSnapshot, CheckReport, CheckedProgram,
    CheckedRuntimeProgram, CheckedSavedPlace, ProjectConfig, StoreBackend,
};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{StoreUid, TreeStore};
use marrow_store::value::{ScalarType, scalar_key_matches_type, validate_scalar_key};
use marrow_syntax::SourceSpan;

use crate::entry::{
    CheckedEntryCall, EntryArgument, EntryInvocation, run_entry_with_debugger, run_entry_with_host,
};
use crate::evolution::{
    AutoApplyOutcome, BaselineError, FenceError, RunObligation, commit_catalog_baseline, fence,
    try_auto_apply,
};
use crate::host::{Host, Nondeterminism, StepHook, SystemNondeterminism};
use crate::surface::{
    SurfaceActionInvocation, SurfaceComputedReadInvocation, SurfaceCreate, SurfaceDelete,
    SurfaceReadError, SurfaceReadOperation, SurfaceUpdate,
};
use crate::value::{RunOutput, RunOutputSink};

const COMMITTED_CATALOG_LOCK_FILE: &str = "marrow.lock";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectMode {
    Run,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectOpen {
    mode: ProjectMode,
    entry_override: Option<String>,
    run_store_policy: RunStorePolicy,
    source_analysis_admission: Option<SourceAnalysisAdmission>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceAnalysisAdmission {
    source_analysis_identity: AnalysisIdentity,
    read_only_context_digest: String,
    accepted_catalog: marrow_catalog::CatalogMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunStorePolicy {
    Commit,
    Isolated,
    FreshMemory,
}

impl ProjectOpen {
    pub fn run() -> Self {
        Self {
            mode: ProjectMode::Run,
            entry_override: None,
            run_store_policy: RunStorePolicy::Commit,
            source_analysis_admission: None,
        }
    }

    pub fn test() -> Self {
        Self {
            mode: ProjectMode::Test,
            entry_override: None,
            run_store_policy: RunStorePolicy::Commit,
            source_analysis_admission: None,
        }
    }

    pub fn with_entry_override(mut self, entry: impl Into<String>) -> Self {
        self.entry_override = Some(entry.into());
        self
    }

    pub fn with_isolated_writes(mut self) -> Self {
        self.run_store_policy = RunStorePolicy::Isolated;
        self
    }

    pub fn with_fresh_memory_store(mut self) -> Self {
        self.run_store_policy = RunStorePolicy::FreshMemory;
        self
    }

    pub fn with_source_analysis_admission(mut self, admission: SourceAnalysisAdmission) -> Self {
        self.source_analysis_admission = Some(admission);
        self
    }
}

impl From<ProjectMode> for ProjectOpen {
    fn from(mode: ProjectMode) -> Self {
        match mode {
            ProjectMode::Run => Self::run(),
            ProjectMode::Test => Self::test(),
        }
    }
}

pub struct ProjectSession {
    root: PathBuf,
    config: ProjectConfig,
    analysis_snapshot: AnalysisSnapshot,
    execution_boundary: ExecutionBoundary,
    session_program: SessionProgram,
    runtime: CheckedRuntimeProgram,
    kind: SessionKind,
    notices: Vec<ProjectSessionNotice>,
}

/// Single-owner linked-Rust read session over a checked project surface.
///
/// The session owns one private store handle and is intended for sequential use
/// by its owner. It is not an `Arc`-shared web-server handle or a stable public
/// API surface.
pub struct ProjectSurfaceReadSession {
    root: PathBuf,
    program: CheckedProgram,
    surface_serve_boundary: SurfaceServeBoundary,
    store_path: PathBuf,
    store: TreeStore,
}

/// Checked project surface state captured from one source-analysis pass.
///
/// Opening sessions from this value reuses the captured program and config
/// while acquiring a fresh store handle for the operation.
pub struct ProjectSurfaceSnapshot {
    root: PathBuf,
    config: ProjectConfig,
    program: CheckedProgram,
    source_analysis_generation: AnalysisGeneration,
}

/// Single-owner linked-Rust read/write session over a checked project surface.
///
/// The session owns one private writable native-store handle. While it is open,
/// that handle is the process/session owner for admitted reads and sparse
/// updates; the native backend's locking excludes another writer or read-only
/// inspection handle. This type is not an `Arc`-shared multi-threaded web-server
/// handle and must not grow hidden open-time repair behavior.
pub struct ProjectSurfaceSession {
    root: PathBuf,
    config: ProjectConfig,
    program: CheckedProgram,
    source_analysis_generation: AnalysisGeneration,
    store_path: PathBuf,
    store: TreeStore,
    notices: Vec<ProjectSessionNotice>,
}

struct CheckedSourceProgram {
    snapshot: AnalysisSnapshot,
}

impl CheckedSourceProgram {
    fn from_snapshot(snapshot: AnalysisSnapshot) -> Self {
        Self { snapshot }
    }

    fn program(&self) -> &CheckedProgram {
        &self.snapshot.program
    }

    fn into_snapshot(self) -> AnalysisSnapshot {
        self.snapshot
    }
}

enum SessionProgram {
    Source,
    WithTests(Box<CheckedProgram>),
}

impl SessionProgram {
    fn checked<'a>(&'a self, snapshot: &'a AnalysisSnapshot) -> &'a CheckedProgram {
        match self {
            Self::Source => &snapshot.program,
            Self::WithTests(program) => program,
        }
    }
}

impl fmt::Debug for ProjectSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectSession")
            .field("root", &self.root)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for ProjectSurfaceReadSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectSurfaceReadSession")
            .field("root", &self.root)
            .field("store_path", &self.store_path)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for ProjectSurfaceSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectSurfaceSnapshot")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for ProjectSurfaceSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectSurfaceSession")
            .field("root", &self.root)
            .field("store_path", &self.store_path)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
enum SessionKind {
    Run { entry: String, store: RunStore },
    Test { cases: Vec<ProjectTestCase> },
}

enum RunStore {
    Memory(TreeStore),
    Native { path: PathBuf, store: TreeStore },
    Isolated(IsolatedStore),
}

impl fmt::Debug for RunStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Memory(_) => formatter.write_str("Memory"),
            Self::Native { path, .. } => formatter
                .debug_struct("Native")
                .field("path", path)
                .finish(),
            Self::Isolated(_) => formatter.write_str("Isolated"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTestCase {
    pub name: String,
    pub source_file: PathBuf,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectSessionNotice {
    AutoApplied {
        from_epoch: u64,
        to_epoch: u64,
    },
    /// A run wrote `marrow.lock` for the first time. The committed lock is otherwise invisible on
    /// the happy path, so a run announces its creation once to teach that the file exists and must
    /// be committed alongside source.
    LockCreated,
    /// A run rewrote an existing `marrow.lock` to match the current source. Announced so the
    /// developer knows the committed lock changed and must be re-committed.
    LockUpdated,
    /// A write-capable open found no store body on disk but a committed `marrow.lock` recording
    /// active roots, so it seeded an empty store from the committed identity. Announced loudly so
    /// a fresh checkout or a lost local store reconstructing from the lock is never silent.
    SeededFromCommittedLock,
    DryRunWouldFreeze,
    DryRunWouldApply {
        from_epoch: u64,
        to_epoch: u64,
    },
    DryRunWouldFence {
        message: String,
    },
}

impl ProjectSessionNotice {
    pub fn message(&self) -> String {
        match self {
            Self::AutoApplied { .. } => {
                "applied saved-data changes from your source and refreshed marrow.lock".to_string()
            }
            Self::LockCreated => {
                "wrote marrow.lock (commit this file alongside your source)".to_string()
            }
            Self::LockUpdated => {
                "updated marrow.lock to match current source; commit the change".to_string()
            }
            Self::SeededFromCommittedLock => {
                "initialized an empty store from marrow.lock".to_string()
            }
            Self::DryRunWouldFreeze => {
                "dry run: would freeze accepted catalog identity".to_string()
            }
            Self::DryRunWouldApply {
                from_epoch,
                to_epoch,
            } => {
                format!("dry run: would apply evolution: catalog epoch {from_epoch} -> {to_epoch}")
            }
            Self::DryRunWouldFence { message } => format!("dry run: would fence: {message}"),
        }
    }
}

#[derive(Debug)]
pub enum ProjectSessionError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    Entropy(std::io::Error),
    Config {
        code: &'static str,
        message: String,
        /// The located position of a `marrow.json` syntax or unknown-field fault,
        /// so the renderer anchors the diagnostic at `marrow.json:line:column`.
        position: Option<marrow_check::ConfigPosition>,
    },
    Catalog {
        code: &'static str,
        message: String,
    },
    Check {
        report: CheckReport,
    },
    CheckLoad {
        code: &'static str,
        path: PathBuf,
        message: String,
    },
    Store(StoreError),
    Fence(FenceError),
    NoEntry,
    DurableStoreRequired,
    UnstampedStore,
    SchemaDrift {
        message: String,
    },
    DryRunIsolationExhausted,
    DryRunIsolation {
        path: PathBuf,
        error: StoreError,
    },
}

impl ProjectSessionError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io { .. } => marrow_check::IO_READ,
            Self::Entropy(_) => "io.entropy",
            Self::Config { code, .. } => code,
            Self::Catalog { code, .. } => code,
            Self::Check { .. } => "check.failed",
            Self::CheckLoad { code, .. } => code,
            Self::Store(error) => error.code(),
            Self::Fence(error) => error.code(),
            Self::NoEntry => "run.no_entry",
            Self::DurableStoreRequired => "run.durable_store_required",
            Self::UnstampedStore => "run.store_unstamped",
            Self::SchemaDrift { .. } => "run.schema_drift",
            Self::DryRunIsolationExhausted => "run.dry_run_isolation",
            Self::DryRunIsolation { error, .. } => error.code(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Io { error, .. } => error.to_string(),
            Self::Entropy(error) => format!("OS entropy unavailable: {error}"),
            Self::Config { message, .. } => message.clone(),
            Self::Catalog { message, .. } => message.clone(),
            Self::Check { .. } => "project failed to check".to_string(),
            Self::CheckLoad { path, message, .. } => format!("{}: {message}", path.display()),
            Self::Store(error) => error.to_string(),
            Self::Fence(error) => error.message(),
            Self::NoEntry => {
                "no entry to run; pass --entry <name> or set `run.defaultEntry` in marrow.json"
                    .to_string()
            }
            Self::DurableStoreRequired => {
                "a durable store is required to establish accepted identity; configure a native store in marrow.json".to_string()
            }
            Self::UnstampedStore => {
                "store has saved records but no catalog activation stamp; run `marrow evolve preview` to inspect the required work and `marrow evolve apply` before running this accepted catalog".to_string()
            }
            Self::SchemaDrift { message } => message.clone(),
            Self::DryRunIsolationExhausted => {
                "could not allocate a temporary dry-run store directory".to_string()
            }
            Self::DryRunIsolation { error, .. } => error.to_string(),
        }
    }
}

impl From<marrow_check::ProjectIoError> for ProjectSessionError {
    fn from(error: marrow_check::ProjectIoError) -> Self {
        match error {
            marrow_check::ProjectIoError::Io { path, error } => Self::Io { path, error },
            // A missing `marrow.json` and a bare-file project path each own their typed code and
            // not-a-project message in `marrow-check`; surface those rather than a raw read fault.
            ref project @ (marrow_check::ProjectIoError::ConfigMissing { .. }
            | marrow_check::ProjectIoError::NotAProject { .. }) => Self::Config {
                code: project.code(),
                message: project.message(),
                position: None,
            },
            // The `dataDir` directory-creation fault owns its `config.data_dir`
            // code and write-path message in `marrow-check`; surface those rather
            // than reclassifying I/O failures by errno here.
            ref create @ marrow_check::ProjectIoError::DataDirCreate { .. } => Self::Config {
                code: create.code(),
                message: create.message(),
                position: None,
            },
            marrow_check::ProjectIoError::Config {
                code,
                message,
                position,
            } => Self::Config {
                code,
                message,
                position,
            },
            marrow_check::ProjectIoError::Catalog { code, message } => {
                Self::Catalog { code, message }
            }
            marrow_check::ProjectIoError::Check { report } => Self::Check { report },
            marrow_check::ProjectIoError::CheckLoad {
                code,
                path,
                message,
            } => Self::CheckLoad {
                code,
                path,
                message,
            },
            marrow_check::ProjectIoError::Store(error) => Self::Store(error),
        }
    }
}

impl From<StoreError> for ProjectSessionError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<BaselineError> for ProjectSessionError {
    fn from(error: BaselineError) -> Self {
        match error {
            BaselineError::Store(error) => Self::Store(error),
            BaselineError::Catalog(error) => Self::Catalog {
                code: error.code,
                message: error.message,
            },
        }
    }
}

#[derive(Debug)]
pub enum ProjectInvokeError {
    Runtime(crate::RuntimeError),
    Session(ProjectSessionError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreStamp {
    pub store_uid: String,
    pub catalog_epoch: u64,
    pub commit_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataViewBoundary {
    pub source_analysis_generation: AnalysisGeneration,
    pub store_snapshot: tooling::DataSnapshotStamp,
    pub watch_targets: Vec<DataViewWatchTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataViewUnavailableReason {
    MemoryStore,
    NativeStoreUnavailable,
    NativeStoreMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataViewWatchTarget {
    pub kind: DataViewWatchTargetKind,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataViewWatchTargetKind {
    StoreFile,
    CatalogLock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceServeBoundary {
    pub mode: SurfaceServeMode,
    pub data_view_boundary: DataViewBoundary,
    pub process_control: SurfaceServeProcessControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceServeMode {
    ReadOnly,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceServeProcessControl {
    NotExposed,
}

impl SurfaceServeBoundary {
    fn new(mode: SurfaceServeMode, data_view_boundary: DataViewBoundary) -> Self {
        Self {
            mode,
            data_view_boundary,
            process_control: SurfaceServeProcessControl::NotExposed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionBoundary {
    pub session_kind: ExecutionSessionKind,
    pub source_analysis_generation: AnalysisGeneration,
    pub store: ExecutionStoreBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionSessionKind {
    Run,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionStoreBoundary {
    pub kind: ExecutionBoundaryStoreKind,
    pub stamp: Option<StoreStamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionBoundaryStoreKind {
    FreshMemory,
    Isolated,
    NativeCommit,
    TestMemory,
    PlainMemory,
}

impl ProjectInvokeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Runtime(error) => error.code(),
            Self::Session(error) => error.code(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Runtime(error) => error.message.clone(),
            Self::Session(error) => error.message(),
        }
    }

    pub fn runtime(self) -> Option<crate::RuntimeError> {
        match self {
            Self::Runtime(error) => Some(error),
            Self::Session(_) => None,
        }
    }

    pub fn session(self) -> Option<ProjectSessionError> {
        match self {
            Self::Runtime(_) => None,
            Self::Session(error) => Some(error),
        }
    }
}

impl From<crate::RuntimeError> for ProjectInvokeError {
    fn from(error: crate::RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<ProjectSessionError> for ProjectInvokeError {
    fn from(error: ProjectSessionError) -> Self {
        Self::Session(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionWrites {
    Commit,
    Isolate,
}

pub struct SessionEntry<'a> {
    invocation: SessionInvocation<'a>,
    host: &'a Host,
    output: &'a mut dyn RunOutputSink,
    hook: Option<&'a mut dyn StepHook>,
    writes: SessionWrites,
}

enum SessionInvocation<'a> {
    Text {
        name: &'a str,
        args: Vec<(&'a str, &'a str)>,
    },
    Protocol(EntryInvocation),
}

impl<'a> SessionEntry<'a> {
    pub fn new(name: &'a str, host: &'a Host, output: &'a mut dyn RunOutputSink) -> Self {
        Self {
            invocation: SessionInvocation::Text {
                name,
                args: Vec::new(),
            },
            host,
            output,
            hook: None,
            writes: SessionWrites::Commit,
        }
    }

    pub fn text(
        name: &'a str,
        args: Vec<(&'a str, &'a str)>,
        host: &'a Host,
        output: &'a mut dyn RunOutputSink,
    ) -> Self {
        Self {
            invocation: SessionInvocation::Text { name, args },
            host,
            output,
            hook: None,
            writes: SessionWrites::Commit,
        }
    }

    pub fn protocol(
        invocation: EntryInvocation,
        host: &'a Host,
        output: &'a mut dyn RunOutputSink,
    ) -> Self {
        Self {
            invocation: SessionInvocation::Protocol(invocation),
            host,
            output,
            hook: None,
            writes: SessionWrites::Commit,
        }
    }

    pub fn with_hook(mut self, hook: &'a mut dyn StepHook) -> Self {
        self.hook = Some(hook);
        self
    }

    pub fn with_isolated_writes(mut self) -> Self {
        self.writes = SessionWrites::Isolate;
        self
    }
}

impl ProjectSession {
    pub fn open(
        root: impl AsRef<Path>,
        mode: impl Into<ProjectOpen>,
    ) -> Result<Self, ProjectSessionError> {
        let open = mode.into();
        let root = root.as_ref().to_path_buf();
        // A write-capable run replays an unclean shutdown before it inspects the store, so a
        // store left flagged for recovery by a signalled writer with no interrupted commit opens
        // clean rather than refusing the read-only catalog read it needs.
        if matches!(open.mode, ProjectMode::Run) && open.run_store_policy == RunStorePolicy::Commit
        {
            let config = marrow_check::load_config(&root)?;
            recover_store_for_write(&root, &config)?;
        }
        let (config, checked) = match open.mode {
            ProjectMode::Test => load_checked_for_fresh_memory_session(&root)?,
            ProjectMode::Run if open.run_store_policy == RunStorePolicy::FreshMemory => {
                load_checked_for_fresh_memory_session(&root)?
            }
            ProjectMode::Run => load_checked_for_session(&root)?,
        };
        match open.mode {
            ProjectMode::Run => open_run_session(root, config, checked, open),
            ProjectMode::Test => open_test_session(root, config, checked),
        }
    }

    pub fn config(&self) -> &ProjectConfig {
        &self.config
    }

    pub fn source_analysis_identity(&self) -> &AnalysisIdentity {
        self.analysis_snapshot.content_identity()
    }

    pub fn source_analysis_snapshot(&self) -> &AnalysisSnapshot {
        &self.analysis_snapshot
    }

    pub fn source_analysis_admission(
        &self,
    ) -> Result<Option<SourceAnalysisAdmission>, ProjectSessionError> {
        let Some(accepted_catalog) =
            accepted_catalog_for_admission(&self.analysis_snapshot.program)?
        else {
            return Ok(None);
        };
        Ok(Some(SourceAnalysisAdmission {
            source_analysis_identity: self.source_analysis_identity().clone(),
            read_only_context_digest: self.analysis_snapshot.program.read_only_context_digest(),
            accepted_catalog,
        }))
    }

    pub fn program(&self) -> &CheckedProgram {
        self.session_program.checked(&self.analysis_snapshot)
    }

    pub fn runtime_program(&self) -> &CheckedRuntimeProgram {
        &self.runtime
    }

    pub fn run_entry(&self) -> Option<&str> {
        match &self.kind {
            SessionKind::Run { entry, .. } => Some(entry),
            SessionKind::Test { .. } => None,
        }
    }

    pub fn test_cases(&self) -> &[ProjectTestCase] {
        match &self.kind {
            SessionKind::Run { .. } => &[],
            SessionKind::Test { cases } => cases,
        }
    }

    pub fn notices(&self) -> &[ProjectSessionNotice] {
        &self.notices
    }

    pub fn store_stamp(&self) -> Result<Option<StoreStamp>, ProjectSessionError> {
        let store = match &self.kind {
            SessionKind::Run {
                store: RunStore::Native { store, .. },
                ..
            } => store,
            _ => return Ok(None),
        };
        optional_store_stamp(store)
    }

    pub fn execution_boundary(&self) -> ExecutionBoundary {
        self.execution_boundary.clone()
    }

    pub fn surface_serve_boundary(&self) -> Result<SurfaceServeBoundary, ProjectSessionError> {
        let (store, watch_targets) = match &self.kind {
            SessionKind::Run {
                store: RunStore::Memory(store),
                ..
            }
            | SessionKind::Run {
                store: RunStore::Isolated(IsolatedStore { store, .. }),
                ..
            } => (store, Vec::new()),
            SessionKind::Run {
                store: RunStore::Native { store, .. },
                ..
            } => (
                store,
                data_view_watch_targets_for_config(&self.root, &self.config)?,
            ),
            SessionKind::Test { .. } => return Err(ProjectSessionError::DurableStoreRequired),
        };
        let data_view_boundary = DataViewBoundary {
            source_analysis_generation: self.analysis_snapshot.generation(),
            store_snapshot: tooling::data_snapshot_stamp(self.program(), store)?,
            watch_targets,
        };
        Ok(SurfaceServeBoundary::new(
            SurfaceServeMode::Write,
            data_view_boundary,
        ))
    }

    pub fn invoke(&self, invocation: SessionEntry<'_>) -> Result<RunOutput, ProjectInvokeError> {
        let call = match &invocation.invocation {
            SessionInvocation::Text { name, args } => {
                CheckedEntryCall::from_text_args(&self.runtime, name, args)?
            }
            SessionInvocation::Protocol(protocol) => {
                CheckedEntryCall::from_protocol_invocation(&self.runtime, protocol)?
            }
        };
        match &self.kind {
            SessionKind::Run { store, .. } => match (store, invocation.writes) {
                (RunStore::Memory(store), _) => invoke_store(store, &call, invocation),
                (RunStore::Isolated(isolated), _) => {
                    invoke_store(&isolated.store, &call, invocation)
                }
                (RunStore::Native { store, .. }, SessionWrites::Commit) => {
                    invoke_store(store, &call, invocation)
                }
                (RunStore::Native { path, .. }, SessionWrites::Isolate) => {
                    let isolated = isolated_store(path)?;
                    invoke_store(&isolated.store, &call, invocation)
                }
            },
            SessionKind::Test { .. } => {
                let store = TreeStore::memory();
                invoke_store(&store, &call, invocation)
            }
        }
    }
}

/// Whether the session admits a separately checked program against the same surface. A program is
/// admitted when its source and read-only context digests both match the session's. The read-only
/// context digest binds the canonical accepted catalog identity (epoch and order-independent
/// digest), evolution proof, and proposal, so a committed-lock checkout and a present (or
/// momentarily writer-locked) store bind the same digest for the same identity and admit each other
/// directly.
fn surface_session_admits_checked_program(
    session_program: &CheckedProgram,
    program: &CheckedProgram,
) -> bool {
    session_program.source_digest() == program.source_digest()
        && session_program.read_only_context_digest() == program.read_only_context_digest()
}

impl ProjectSurfaceReadSession {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ProjectSessionError> {
        let root = root.as_ref().to_path_buf();
        let (config, snapshot) = load_checked_for_surface_session(&root)?;
        ProjectSurfaceSnapshot::from_checked(root, config, snapshot).open_read_only()
    }

    pub fn program(&self) -> &CheckedProgram {
        &self.program
    }

    pub fn admits_checked_program(&self, program: &CheckedProgram) -> bool {
        surface_session_admits_checked_program(&self.program, program)
    }

    pub fn data_view_boundary(&self) -> &DataViewBoundary {
        &self.surface_serve_boundary.data_view_boundary
    }

    pub fn surface_serve_boundary(&self) -> Result<SurfaceServeBoundary, ProjectSessionError> {
        Ok(self.surface_serve_boundary.clone())
    }

    pub fn store_stamp(&self) -> Result<StoreStamp, ProjectSessionError> {
        store_stamp(&self.store)
    }

    pub fn saved_data_roots(
        &self,
    ) -> Result<tooling::StampedData<Vec<tooling::DataChildView>>, StoreError> {
        tooling::stamped_saved_data_root_views_in_store(&self.program, &self.store)
    }

    pub fn saved_data_children(
        &self,
        segments: &[tooling::SavedDataPathSegment],
        limit: usize,
        resume: Option<&SavedKey>,
    ) -> Result<tooling::StampedData<tooling::DataChildViewsPage>, tooling::ToolingError> {
        tooling::stamped_saved_data_child_views(&self.program, &self.store, segments, limit, resume)
    }

    pub fn saved_data_preview(
        &self,
        segments: &[tooling::SavedDataPathSegment],
        limit: usize,
    ) -> Result<Option<tooling::StampedData<tooling::DataPreviewReadResult>>, tooling::ToolingError>
    {
        let Some(path) = tooling::resolve_saved_data_path(&self.program, segments)? else {
            return Ok(None);
        };
        tooling::stamped_preview_data_path(&self.program, &self.store, &path, limit)
            .map(Some)
            .map_err(tooling::ToolingError::from)
    }

    pub fn saved_data_integrity_sample(
        &self,
        limit: usize,
    ) -> Result<tooling::StampedData<tooling::IntegrityProblemSample>, StoreError> {
        tooling::stamped_integrity_problem_details(&self.program, &self.store, limit)
    }

    pub fn admit_read_by_operation_tag(
        &self,
        operation_tag: &str,
    ) -> Result<SurfaceReadOperation<'_>, SurfaceReadError> {
        SurfaceReadOperation::admit_by_operation_tag(&self.program, &self.store, operation_tag)
    }

    pub fn admit_computed_read_by_operation_tag(
        &self,
        operation_tag: &str,
    ) -> Result<SurfaceComputedReadInvocation, SurfaceReadError> {
        SurfaceComputedReadInvocation::admit_by_operation_tag(&self.program, operation_tag)
    }

    pub fn invoke_computed_read(
        &self,
        computed_read: &SurfaceComputedReadInvocation,
        arguments: Vec<EntryArgument>,
        output: &mut dyn RunOutputSink,
    ) -> Result<RunOutput, ProjectInvokeError> {
        invoke_computed_read(&self.program, &self.store, computed_read, arguments, output)
    }
}

impl ProjectSurfaceSnapshot {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ProjectSessionError> {
        let root = root.as_ref().to_path_buf();
        let (config, snapshot) = load_checked_for_surface_session(&root)?;
        Ok(Self::from_checked(root, config, snapshot))
    }

    fn from_checked(root: PathBuf, config: ProjectConfig, snapshot: AnalysisSnapshot) -> Self {
        let source_analysis_generation = snapshot.generation();
        Self {
            root,
            config,
            program: snapshot.program,
            source_analysis_generation,
        }
    }

    pub fn program(&self) -> &CheckedProgram {
        &self.program
    }

    pub fn open_read_only(&self) -> Result<ProjectSurfaceReadSession, ProjectSessionError> {
        let opened = open_surface_session(
            self.root.clone(),
            &self.config,
            self.program.clone(),
            SurfaceStoreAccess::ReadOnly,
        )?;
        let data_view_boundary = data_view_boundary_for_opened_session(
            &opened.root,
            &self.config,
            &opened.program,
            &opened.store,
            self.source_analysis_generation.clone(),
        )?;
        Ok(ProjectSurfaceReadSession {
            root: opened.root,
            program: opened.program,
            surface_serve_boundary: SurfaceServeBoundary::new(
                SurfaceServeMode::ReadOnly,
                data_view_boundary,
            ),
            store_path: opened.store_path,
            store: opened.store,
        })
    }

    pub fn open_write(&self) -> Result<ProjectSurfaceSession, ProjectSessionError> {
        let opened = open_surface_session(
            self.root.clone(),
            &self.config,
            self.program.clone(),
            SurfaceStoreAccess::Write,
        )?;
        Ok(ProjectSurfaceSession {
            root: opened.root,
            config: self.config.clone(),
            program: opened.program,
            source_analysis_generation: self.source_analysis_generation.clone(),
            store_path: opened.store_path,
            store: opened.store,
            notices: opened.notices,
        })
    }
}

impl ProjectSurfaceSession {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ProjectSessionError> {
        let root = root.as_ref().to_path_buf();
        let (config, snapshot) = load_checked_for_surface_session(&root)?;
        ProjectSurfaceSnapshot::from_checked(root, config, snapshot).open_write()
    }

    pub fn program(&self) -> &CheckedProgram {
        &self.program
    }

    /// Notices raised while opening the write session, such as seeding an empty store from the
    /// committed lock on a fresh checkout. The serve startup prints these so a seed is never silent.
    pub fn notices(&self) -> &[ProjectSessionNotice] {
        &self.notices
    }

    pub fn surface_serve_boundary(&self) -> Result<SurfaceServeBoundary, ProjectSessionError> {
        let data_view_boundary = data_view_boundary_for_opened_session(
            &self.root,
            &self.config,
            &self.program,
            &self.store,
            self.source_analysis_generation.clone(),
        )?;
        Ok(SurfaceServeBoundary::new(
            SurfaceServeMode::Write,
            data_view_boundary,
        ))
    }

    pub fn store_stamp(&self) -> Result<StoreStamp, ProjectSessionError> {
        store_stamp(&self.store)
    }

    pub fn admit_read_by_operation_tag(
        &self,
        operation_tag: &str,
    ) -> Result<SurfaceReadOperation<'_>, SurfaceReadError> {
        SurfaceReadOperation::admit_by_operation_tag(&self.program, &self.store, operation_tag)
    }

    pub fn admit_update_by_operation_tag(
        &self,
        operation_tag: &str,
    ) -> Result<SurfaceUpdate<'_>, SurfaceReadError> {
        SurfaceUpdate::admit_by_operation_tag(&self.program, &self.store, operation_tag)
    }

    pub fn admit_create_by_operation_tag(
        &self,
        operation_tag: &str,
    ) -> Result<SurfaceCreate<'_>, SurfaceReadError> {
        SurfaceCreate::admit_by_operation_tag(&self.program, &self.store, operation_tag)
    }

    pub fn admit_delete_by_operation_tag(
        &self,
        operation_tag: &str,
    ) -> Result<SurfaceDelete<'_>, SurfaceReadError> {
        SurfaceDelete::admit_by_operation_tag(&self.program, &self.store, operation_tag)
    }

    pub fn admit_action_by_operation_tag(
        &self,
        operation_tag: &str,
    ) -> Result<SurfaceActionInvocation, SurfaceReadError> {
        SurfaceActionInvocation::admit_by_operation_tag(&self.program, operation_tag)
    }

    pub fn admit_computed_read_by_operation_tag(
        &self,
        operation_tag: &str,
    ) -> Result<SurfaceComputedReadInvocation, SurfaceReadError> {
        SurfaceComputedReadInvocation::admit_by_operation_tag(&self.program, operation_tag)
    }

    pub fn invoke_action(
        &self,
        action: &SurfaceActionInvocation,
        arguments: Vec<EntryArgument>,
        host: &Host,
        output: &mut dyn RunOutputSink,
    ) -> Result<RunOutput, ProjectInvokeError> {
        let action =
            SurfaceActionInvocation::admit_by_operation_tag(&self.program, action.operation_tag())
                .map_err(|error| ProjectInvokeError::Runtime(error.into_runtime_error()))?;
        let invocation = action.invocation(arguments);
        let runtime = self.program.runtime();
        let call = CheckedEntryCall::from_protocol_invocation(&runtime, &invocation)?;
        Ok(run_entry_with_host(&self.store, host, &call, output)?)
    }

    pub fn invoke_computed_read(
        &self,
        computed_read: &SurfaceComputedReadInvocation,
        arguments: Vec<EntryArgument>,
        output: &mut dyn RunOutputSink,
    ) -> Result<RunOutput, ProjectInvokeError> {
        invoke_computed_read(&self.program, &self.store, computed_read, arguments, output)
    }
}

fn invoke_computed_read(
    program: &CheckedProgram,
    store: &TreeStore,
    computed_read: &SurfaceComputedReadInvocation,
    arguments: Vec<EntryArgument>,
    output: &mut dyn RunOutputSink,
) -> Result<RunOutput, ProjectInvokeError> {
    let computed_read = SurfaceComputedReadInvocation::admit_by_operation_tag(
        program,
        computed_read.operation_tag(),
    )
    .map_err(|error| ProjectInvokeError::Runtime(error.into_runtime_error()))?;
    let invocation = computed_read.invocation(arguments);
    let runtime = program.runtime();
    let call = CheckedEntryCall::from_protocol_invocation(&runtime, &invocation)?;
    Ok(run_entry_with_host(store, &Host::new(), &call, output)?)
}

fn invoke_store(
    store: &TreeStore,
    call: &CheckedEntryCall<'_>,
    invocation: SessionEntry<'_>,
) -> Result<RunOutput, ProjectInvokeError> {
    if let Some(hook) = invocation.hook {
        Ok(run_entry_with_debugger(
            store,
            invocation.host,
            hook,
            call,
            invocation.output,
        )?)
    } else {
        Ok(run_entry_with_host(
            store,
            invocation.host,
            call,
            invocation.output,
        )?)
    }
}

fn open_run_session(
    root: PathBuf,
    config: ProjectConfig,
    checked: CheckedSourceProgram,
    open: ProjectOpen,
) -> Result<ProjectSession, ProjectSessionError> {
    let entry = open
        .entry_override
        .clone()
        .or_else(|| config.default_entry.clone())
        .ok_or(ProjectSessionError::NoEntry)?;
    let mut notices = Vec::new();
    let admission = open.source_analysis_admission.as_ref();
    if matches!(open.run_store_policy, RunStorePolicy::Commit) && admission.is_some() {
        return Err(ProjectSessionError::SchemaDrift {
            message:
                "source analysis admission is only valid for isolated or fresh-memory run sessions"
                    .to_string(),
        });
    }
    let store = match open.run_store_policy {
        RunStorePolicy::Commit => {
            open_run_store(&root, &config, checked, false, admission, &mut notices)
        }
        RunStorePolicy::Isolated => {
            open_run_store(&root, &config, checked, true, admission, &mut notices)
        }
        RunStorePolicy::FreshMemory => {
            open_fresh_memory_run_store(&root, &config, checked, admission)
        }
    }?;
    let analysis_snapshot = store.checked.into_snapshot();
    let execution_boundary = ExecutionBoundary {
        session_kind: ExecutionSessionKind::Run,
        source_analysis_generation: analysis_snapshot.generation(),
        store: store.store_boundary,
    };
    let runtime = analysis_snapshot.program.runtime();
    Ok(ProjectSession {
        root,
        config,
        analysis_snapshot,
        execution_boundary,
        session_program: SessionProgram::Source,
        runtime,
        kind: SessionKind::Run {
            entry,
            store: store.store,
        },
        notices,
    })
}

fn open_test_session(
    root: PathBuf,
    config: ProjectConfig,
    checked: CheckedSourceProgram,
) -> Result<ProjectSession, ProjectSessionError> {
    let checked = bind_proposed_catalog_identity(&root, &config, checked, None)?;
    let analysis_snapshot = checked.into_snapshot();
    let source_module_count = analysis_snapshot.program.modules.len();
    let (test_report, program) =
        marrow_check::check_tests_program(&root, &config, analysis_snapshot.program.clone())
            .map_err(|error| ProjectSessionError::CheckLoad {
                code: error.code,
                path: error.path,
                message: error.message,
            })?;
    if test_report.has_errors() {
        return Err(ProjectSessionError::Check {
            report: test_report,
        });
    }
    let cases = program.modules[source_module_count..]
        .iter()
        .flat_map(|module| {
            module
                .functions
                .iter()
                .filter(|function| function.public && function.params.is_empty())
                .map(|function| ProjectTestCase {
                    name: format!("{}::{}", module.name, function.name),
                    source_file: module.source_file.clone(),
                    span: function.span,
                })
        })
        .collect();
    let runtime = program.runtime();
    let execution_boundary = ExecutionBoundary {
        session_kind: ExecutionSessionKind::Test,
        source_analysis_generation: analysis_snapshot.generation(),
        store: ExecutionStoreBoundary {
            kind: ExecutionBoundaryStoreKind::TestMemory,
            stamp: None,
        },
    };
    Ok(ProjectSession {
        root,
        config,
        analysis_snapshot,
        execution_boundary,
        session_program: SessionProgram::WithTests(Box::new(program)),
        runtime,
        kind: SessionKind::Test { cases },
        notices: Vec::new(),
    })
}

struct OpenSurfaceSession {
    root: PathBuf,
    program: CheckedProgram,
    store_path: PathBuf,
    store: TreeStore,
    notices: Vec<ProjectSessionNotice>,
}

#[derive(Clone, Copy)]
enum SurfaceStoreAccess {
    ReadOnly,
    Write,
}

fn open_surface_session(
    root: PathBuf,
    config: &ProjectConfig,
    mut program: CheckedProgram,
    access: SurfaceStoreAccess,
) -> Result<OpenSurfaceSession, ProjectSessionError> {
    // The committed `marrow.lock` is the independent witness to durable identity for both access
    // modes: a PRESENT store presenting fewer committed roots than its lock recorded has lost
    // durable identity and fails closed with `store.corruption`, identically to run, evolve apply,
    // and the read-only inspection family, rather than admitting the loss. An ABSENT body is the
    // disposable-store case, not a loss, and passes here.
    let mut notices = Vec::new();
    guard_committed_lock_roots(&root, config)?;

    // An absent body under a roots-recording lock is the fresh-checkout (or lost-body) case: the
    // committed lock fully determines the surface ABI. A write-capable open seeds an on-disk store
    // from the committed identity so subsequent writes commit there; a read-only open materializes
    // that empty committed identity in memory and never writes the body, serving the empty
    // committed identity exactly as the read-only inspections report it. Either way the program is
    // re-derived from the resulting store so the open carries an accepted catalog rather than the
    // pre-seed proposal.
    let store = match access {
        SurfaceStoreAccess::Write => {
            if let Some(seeded) = seed_absent_store_from_committed_lock(&root, config, &program)? {
                program = seeded;
                notices.push(ProjectSessionNotice::SeededFromCommittedLock);
            }
            open_existing_surface_store(&root, config, access)?
        }
        SurfaceStoreAccess::ReadOnly => {
            match read_only_empty_committed_identity(&root, config, &program)? {
                Some((seeded, store)) => {
                    program = seeded;
                    store
                }
                None => open_existing_surface_store(&root, config, access)?,
            }
        }
    };
    if populated_unstamped_store(&program, &store.store)? {
        return Err(ProjectSessionError::UnstampedStore);
    }
    if store.store.read_store_uid()?.is_none() || store.store.read_commit_metadata()?.is_none() {
        return Err(ProjectSessionError::DurableStoreRequired);
    }
    // Serving saved data owes the same schema-driven completeness cross-check the runtime open,
    // inspection family, backup, and recover run: the structural witness the open ran proves the
    // store traversable but not that its index holds exactly the entries its records derive, so a
    // truncated index fails closed here rather than streaming an under-returning range.
    tooling::verify_store_completeness(&store.store, &program)?;
    // A write-capable open must not seize a store the binary is not admitted against. When the
    // committed lock records an epoch high-water a teammate already advanced past, the local store
    // is behind that committed activation, so a surface write would commit against an epoch the
    // shared source tree has left behind. Fail closed here exactly as run and evolve apply do; a
    // read cannot corrupt the store, so a behind read-only open is admitted at its own epoch.
    if matches!(access, SurfaceStoreAccess::Write)
        && let Some(behind) = store_behind_committed_lock(&root, &store.store)?
    {
        return Err(ProjectSessionError::Fence(behind));
    }
    // The fence owns the no-accepted-epoch decision for both open paths: a stamped store opened
    // by a program with no accepted epoch fails closed with `run.durable_store_required`. Once it
    // passes, the program carries an accepted epoch and digest to drift-check against.
    fence_run(&program, &store.store).map_err(ProjectSessionError::Fence)?;
    let accepted_digest =
        program
            .catalog
            .accepted_digest
            .as_deref()
            .ok_or_else(|| ProjectSessionError::Catalog {
                code: marrow_catalog::CATALOG_INVALID,
                message: "accepted catalog digest is missing from the checked program".to_string(),
            })?;
    let found = store.store.catalog_snapshot_digest()?;
    if found.as_deref() != Some(accepted_digest) {
        return Err(ProjectSessionError::SchemaDrift {
            message: "store catalog digest does not match the checked project catalog".to_string(),
        });
    }
    Ok(OpenSurfaceSession {
        root,
        program,
        store_path: store.path,
        store: store.store,
        notices,
    })
}

pub fn data_view_watch_targets(
    root: impl AsRef<Path>,
    config: &ProjectConfig,
) -> Result<Vec<DataViewWatchTarget>, ProjectSessionError> {
    data_view_watch_targets_for_config(root.as_ref(), config)
}

pub fn data_view_unavailable_reason_for_config(
    root: impl AsRef<Path>,
    config: &ProjectConfig,
) -> Result<Option<DataViewUnavailableReason>, ProjectSessionError> {
    if matches!(config.store.backend, StoreBackend::Memory) {
        return Ok(Some(DataViewUnavailableReason::MemoryStore));
    }
    let Some(store_path) = marrow_check::native_store_path(root.as_ref(), config)? else {
        return Ok(Some(DataViewUnavailableReason::NativeStoreUnavailable));
    };
    if !store_path.exists() {
        return Ok(Some(DataViewUnavailableReason::NativeStoreMissing));
    }
    Ok(None)
}

fn data_view_boundary_for_opened_session(
    root: &Path,
    config: &ProjectConfig,
    program: &CheckedProgram,
    store: &TreeStore,
    source_analysis_generation: AnalysisGeneration,
) -> Result<DataViewBoundary, ProjectSessionError> {
    Ok(DataViewBoundary {
        source_analysis_generation,
        store_snapshot: tooling::data_snapshot_stamp(program, store)?,
        watch_targets: data_view_watch_targets_for_config(root, config)?,
    })
}

fn data_view_watch_targets_for_config(
    root: &Path,
    config: &ProjectConfig,
) -> Result<Vec<DataViewWatchTarget>, ProjectSessionError> {
    let Some(store_path) = marrow_check::native_store_path(root, config)? else {
        return Ok(Vec::new());
    };
    Ok(vec![
        DataViewWatchTarget {
            kind: DataViewWatchTargetKind::StoreFile,
            path: store_path,
        },
        DataViewWatchTarget {
            kind: DataViewWatchTargetKind::CatalogLock,
            path: root.join(COMMITTED_CATALOG_LOCK_FILE),
        },
    ])
}

fn open_existing_surface_store(
    root: &Path,
    config: &ProjectConfig,
    access: SurfaceStoreAccess,
) -> Result<NativeRunStore, ProjectSessionError> {
    let Some(path) = marrow_check::native_store_path(root, config)? else {
        return Err(ProjectSessionError::DurableStoreRequired);
    };
    let store = match access {
        SurfaceStoreAccess::ReadOnly => TreeStore::open_read_only(&path)
            .map_err(|error| surface_store_open_error(&path, error))?,
        SurfaceStoreAccess::Write => TreeStore::open_existing(&path)
            .map_err(|error| surface_store_open_error(&path, error))?,
    };
    // Serving saved data is a store read, so it owes the same readability cross-check the runtime
    // and inspection families run: the data cells are their own derivation, so a backend page that
    // silently drops a cell or rewrites a stored value shifts every enumeration with no structural
    // fault until a read walks the damaged page. Verifying the per-root structural digest here fails
    // a btree-corrupt store closed at open rather than letting the server admit it and stream a
    // truncated prefix until a page reaches the corrupt cell.
    store.verify_readable()?;
    Ok(NativeRunStore { path, store })
}

fn surface_store_open_error(path: &Path, error: StoreError) -> ProjectSessionError {
    if let StoreError::Io { op: "open", .. } = &error
        && matches!(path.try_exists(), Ok(false))
    {
        return ProjectSessionError::DurableStoreRequired;
    }
    ProjectSessionError::Store(error)
}

fn store_stamp(store: &TreeStore) -> Result<StoreStamp, ProjectSessionError> {
    optional_store_stamp(store)?.ok_or(ProjectSessionError::DurableStoreRequired)
}

fn optional_store_stamp(store: &TreeStore) -> Result<Option<StoreStamp>, ProjectSessionError> {
    let Some(uid) = store.read_store_uid()? else {
        return Ok(None);
    };
    let Some(commit) = store.read_commit_metadata()? else {
        return Ok(None);
    };
    Ok(Some(StoreStamp {
        store_uid: uid.as_str().to_string(),
        catalog_epoch: commit.catalog_epoch,
        commit_id: commit.commit_id,
    }))
}

struct OpenRunStore {
    checked: CheckedSourceProgram,
    store: RunStore,
    store_boundary: ExecutionStoreBoundary,
}

struct NativeRunStore {
    path: PathBuf,
    store: TreeStore,
}

fn open_run_store(
    root: &Path,
    config: &ProjectConfig,
    checked: CheckedSourceProgram,
    isolate_writes: bool,
    admission: Option<&SourceAnalysisAdmission>,
    notices: &mut Vec<ProjectSessionNotice>,
) -> Result<OpenRunStore, ProjectSessionError> {
    if !isolate_writes && !commit_run_needs_native_store(root, config, checked.program())? {
        return open_memory_store(validate_checked_source_analysis_admission(
            checked, admission,
        )?);
    }
    if !isolate_writes {
        // A `dataDir` occupied by a non-directory is a configuration fault, not a store
        // failure. Classify it through the shared guard before any open — the lock-root
        // witness opens the store directly, so without this the stray file's `ENOTDIR`
        // would leak as a raw `store.io` whenever a committed lock records active roots.
        marrow_check::guard_data_dir(root, config)?;
        guard_committed_lock_roots(root, config)?;
        if store_absent_with_committed_lock(root, config)? {
            notices.push(ProjectSessionNotice::SeededFromCommittedLock);
        }
    }
    let Some(store) = open_store_file(root, config, !isolate_writes)? else {
        if isolate_writes && pending_baseline(checked.program()) {
            notices.push(ProjectSessionNotice::DryRunWouldFreeze);
            return open_memory_preview_store(root, config, checked, admission);
        }
        return open_memory_store(validate_checked_source_analysis_admission(
            checked, admission,
        )?);
    };
    if isolate_writes && pending_baseline(checked.program()) {
        if populated_unstamped_store(checked.program(), &store.store)? {
            return Err(ProjectSessionError::UnstampedStore);
        }
        notices.push(ProjectSessionNotice::DryRunWouldFreeze);
        let checked = bind_proposed_catalog_identity(root, config, checked, admission)?;
        return finish_open(checked, store, true);
    }
    let checked = if isolate_writes {
        checked
    } else {
        establish_store_baseline(root, config, &store.store, checked)?
    };
    if let Some(behind) = store_behind_committed_lock(root, &store.store)? {
        return Err(ProjectSessionError::Fence(behind));
    }
    match fence_run(checked.program(), &store.store) {
        // A shape-neutral in-place transform recomputes an already-accepted member, so it
        // moves neither the catalog epoch nor the source digest the fence reads. The fence
        // therefore agrees even though the migration is still pending. A pending evolution
        // blocks run until applied or withdrawn, so route a live in-place transform through
        // the discharge path the same way schema drift does: it auto-applies when the
        // affected store is empty and fences when it has records to rewrite.
        Ok(()) if marrow_check::evolution::has_pending_transform(checked.program()) => {
            divert_to_discharge(
                root,
                config,
                checked,
                store,
                isolate_writes,
                admission,
                notices,
            )
        }
        Ok(()) => {
            validate_source_analysis_admission(&checked.snapshot, admission)?;
            reproject_and_finish_open(root, checked, store, isolate_writes, notices)
        }
        Err(FenceError::SchemaDrift) => divert_to_discharge(
            root,
            config,
            checked,
            store,
            isolate_writes,
            admission,
            notices,
        ),
        Err(error) => Err(ProjectSessionError::Fence(error)),
    }
}

fn commit_run_needs_native_store(
    root: &Path,
    config: &ProjectConfig,
    program: &CheckedProgram,
) -> Result<bool, ProjectSessionError> {
    if config.store.backend == StoreBackend::Memory {
        return Ok(false);
    }
    if pending_baseline(program) {
        return Ok(true);
    }
    let Some(path) = marrow_check::native_store_path(root, config)? else {
        return Ok(false);
    };
    if marrow_check::read_committed_lock(root)?.is_some_and(|lock| lock.records_active_roots()) {
        return Ok(true);
    }
    Ok(!marrow_check::tooling::store_path_is_absent(&path))
}

/// Discharge a pending evolution on the run path: auto-apply it when it mutates no stored
/// record, otherwise fence with the obligation. This is the same gateway schema drift
/// takes, factored out so a shape-neutral in-place transform — which the fence cannot
/// see — reaches it too. An admitted session cannot serve a pending evolution.
fn divert_to_discharge(
    root: &Path,
    config: &ProjectConfig,
    checked: CheckedSourceProgram,
    store: NativeRunStore,
    isolate_writes: bool,
    admission: Option<&SourceAnalysisAdmission>,
    notices: &mut Vec<ProjectSessionNotice>,
) -> Result<OpenRunStore, ProjectSessionError> {
    validate_no_source_analysis_admission(admission)?;
    if isolate_writes {
        return classify_dry_run_drift(root, config, checked, store, notices);
    }
    auto_apply_then_reopen(root, config, checked, store, isolate_writes, notices)
}

fn open_memory_preview_store(
    root: &Path,
    config: &ProjectConfig,
    checked: CheckedSourceProgram,
    admission: Option<&SourceAnalysisAdmission>,
) -> Result<OpenRunStore, ProjectSessionError> {
    open_bound_memory_store(
        bind_proposed_catalog_identity(root, config, checked, admission)?,
        ExecutionBoundaryStoreKind::PlainMemory,
    )
}

fn open_memory_store(checked: CheckedSourceProgram) -> Result<OpenRunStore, ProjectSessionError> {
    if pending_baseline(checked.program()) {
        return Err(ProjectSessionError::DurableStoreRequired);
    }
    open_bound_memory_store(checked, ExecutionBoundaryStoreKind::PlainMemory)
}

fn open_fresh_memory_run_store(
    root: &Path,
    config: &ProjectConfig,
    checked: CheckedSourceProgram,
    admission: Option<&SourceAnalysisAdmission>,
) -> Result<OpenRunStore, ProjectSessionError> {
    open_bound_memory_store(
        bind_proposed_catalog_identity(root, config, checked, admission)?,
        ExecutionBoundaryStoreKind::FreshMemory,
    )
}

fn open_bound_memory_store(
    checked: CheckedSourceProgram,
    kind: ExecutionBoundaryStoreKind,
) -> Result<OpenRunStore, ProjectSessionError> {
    Ok(OpenRunStore {
        checked,
        store: RunStore::Memory(TreeStore::memory()),
        store_boundary: ExecutionStoreBoundary { kind, stamp: None },
    })
}

fn open_store_file(
    root: &Path,
    config: &ProjectConfig,
    write_uid: bool,
) -> Result<Option<NativeRunStore>, ProjectSessionError> {
    let path = if write_uid {
        resolve_store_path(root, config)?
    } else {
        marrow_check::native_store_path(root, config)?
    };
    let Some(path) = path else {
        return Ok(None);
    };
    let store = if write_uid {
        let store = TreeStore::open(&path)?;
        let mut nondeterminism = SystemNondeterminism::new();
        ensure_store_uid(&store, &mut nondeterminism)?;
        store
    } else if path.exists() {
        TreeStore::open_read_only(&path)?
    } else {
        return Ok(None);
    };
    // The runtime read of saved data is a store read, so it owes the same readability
    // cross-check the inspection family runs: the data cells are their own derivation, so a
    // backend page that silently drops a cell or truncates a record range shifts every
    // enumeration with no structural fault, and a torn page faults only when a read walks it.
    // Verifying the store readable here — the per-root structural digest plus the record and
    // index re-walks — fails a btree-corrupt store closed at open rather than letting a run
    // enumerate it truncated or fault mid-evaluation.
    store.verify_readable()?;
    Ok(Some(NativeRunStore { path, store }))
}

fn finish_open(
    checked: CheckedSourceProgram,
    store: NativeRunStore,
    isolate_writes: bool,
) -> Result<OpenRunStore, ProjectSessionError> {
    if populated_unstamped_store(checked.program(), &store.store)? {
        return Err(ProjectSessionError::UnstampedStore);
    }
    // Reading saved data owes the schema-driven completeness cross-check the inspection family,
    // backup, and recover run through their single owner. The structural witness the open already
    // ran proves the store traversable but cannot tell that the index holds exactly the entries its
    // data records derive: a truncated index reads under-returning with no structural fault. Running
    // it here fails such a store closed at open rather than letting a run enumerate it truncated or
    // accept a write onto it.
    tooling::verify_store_completeness(&store.store, checked.program())?;
    let NativeRunStore { path, store } = store;
    if isolate_writes {
        let isolated = isolated_store(&path)?;
        let store_boundary = ExecutionStoreBoundary {
            kind: ExecutionBoundaryStoreKind::Isolated,
            stamp: optional_store_stamp(&isolated.store)?,
        };
        return Ok(OpenRunStore {
            checked,
            store: RunStore::Isolated(isolated),
            store_boundary,
        });
    }
    let store_boundary = ExecutionStoreBoundary {
        kind: ExecutionBoundaryStoreKind::NativeCommit,
        stamp: optional_store_stamp(&store)?,
    };
    Ok(OpenRunStore {
        checked,
        store: RunStore::Native { path, store },
        store_boundary,
    })
}

/// Finish a fence-cleared native open, re-projecting the committed lock first on the writable
/// path. The store is the sole write authority and the lock is its committed source-tree
/// projection, so every commit-path open that the fence agrees matches this binary converges the
/// lock through this single owner — whether the store was already at this shape or an auto-apply
/// just advanced it. A `isolate_writes` (dry-run) open never re-projects, since it does not commit.
fn reproject_and_finish_open(
    root: &Path,
    checked: CheckedSourceProgram,
    store: NativeRunStore,
    isolate_writes: bool,
    notices: &mut Vec<ProjectSessionNotice>,
) -> Result<OpenRunStore, ProjectSessionError> {
    if !isolate_writes {
        reproject_committed_lock(root, &store.store, checked.program(), notices)?;
    }
    finish_open(checked, store, isolate_writes)
}

fn auto_apply_then_reopen(
    root: &Path,
    config: &ProjectConfig,
    checked: CheckedSourceProgram,
    store: NativeRunStore,
    isolate_writes: bool,
    notices: &mut Vec<ProjectSessionNotice>,
) -> Result<OpenRunStore, ProjectSessionError> {
    let (witness, diagnostics) =
        preview(checked.program(), &store.store).map_err(ProjectSessionError::Store)?;
    let (from_epoch, to_epoch) = witness.epoch_range();
    match try_auto_apply(&witness, checked.program(), &store.store) {
        Ok(AutoApplyOutcome::Applied) => {
            notices.push(ProjectSessionNotice::AutoApplied {
                from_epoch,
                to_epoch,
            });
        }
        Ok(AutoApplyOutcome::MustFence(obligation)) => {
            return Err(ProjectSessionError::SchemaDrift {
                message: fence_message(&obligation, &diagnostics),
            });
        }
        Err(_) => {
            return Err(ProjectSessionError::SchemaDrift {
                message: "store changed under the auto-apply probe; re-run to recompute the evolution against current data".to_string(),
            });
        }
    }
    drop(store);

    let checked = load_checked_for_config(root, config)?;
    let Some(store) = open_store_file(root, config, true)? else {
        return open_memory_store(checked);
    };
    fence_run(checked.program(), &store.store).map_err(ProjectSessionError::Fence)?;
    reproject_and_finish_open(root, checked, store, isolate_writes, notices)
}

fn classify_dry_run_drift(
    root: &Path,
    config: &ProjectConfig,
    checked: CheckedSourceProgram,
    store: NativeRunStore,
    notices: &mut Vec<ProjectSessionNotice>,
) -> Result<OpenRunStore, ProjectSessionError> {
    let (witness, diagnostics) =
        preview(checked.program(), &store.store).map_err(ProjectSessionError::Store)?;
    let (from_epoch, to_epoch) = witness.epoch_range();
    let obligation = RunObligation::classify(&witness);
    match obligation {
        RunObligation::ZeroMutation { .. } => {
            notices.push(ProjectSessionNotice::DryRunWouldApply {
                from_epoch,
                to_epoch,
            });
            let isolated = isolated_store(&store.path)?;
            match try_auto_apply(&witness, checked.program(), &isolated.store) {
                Ok(AutoApplyOutcome::Applied) => {}
                Ok(AutoApplyOutcome::MustFence(obligation)) => {
                    notices.push(ProjectSessionNotice::DryRunWouldFence {
                        message: fence_message(&obligation, &diagnostics),
                    });
                    return finish_open(checked, store, true);
                }
                Err(_) => {
                    return Err(ProjectSessionError::SchemaDrift {
                        message: "store changed under the auto-apply probe; re-run to recompute the evolution against current data".to_string(),
                    });
                }
            }
            let checked = CheckedSourceProgram::from_snapshot(
                marrow_check::recheck_source_project_analysis_against_store_catalog(
                    root,
                    config,
                    &isolated.store,
                )
                .map_err(ProjectSessionError::from)?,
            );
            fence_run(checked.program(), &isolated.store).map_err(ProjectSessionError::Fence)?;
            let store_boundary = ExecutionStoreBoundary {
                kind: ExecutionBoundaryStoreKind::Isolated,
                stamp: optional_store_stamp(&isolated.store)?,
            };
            return Ok(OpenRunStore {
                checked,
                store: RunStore::Isolated(isolated),
                store_boundary,
            });
        }
        obligation => {
            notices.push(ProjectSessionNotice::DryRunWouldFence {
                message: fence_message(&obligation, &diagnostics),
            });
        }
    }
    finish_open(checked, store, true)
}

fn fence_message(obligation: &RunObligation, diagnostics: &[RepairDiagnostic]) -> String {
    let base = "store was stamped under a different schema at this catalog epoch";
    let cause = match obligation {
        RunObligation::Backfill { records } => format!(
            "; the change backfills {records} record(s). Run `marrow evolve apply` to discharge it."
        ),
        RunObligation::Transform { records } => format!(
            "; the change rewrites {records} record(s). Run `marrow evolve apply` to discharge it."
        ),
        RunObligation::Rename { records } => format!(
            "; the change re-addresses {records} record(s) through a rename. Run `marrow evolve apply` to discharge it."
        ),
        RunObligation::DestructiveDrop { populated } => format!(
            "; the change drops {populated} populated record(s). Run `marrow evolve apply --maintenance` and confirm the retire to discharge it."
        ),
        RunObligation::Repair => repair_fence_message(diagnostics),
        RunObligation::ZeroMutation { .. } => String::new(),
    };
    format!("{base}{cause}")
}

fn repair_fence_message(diagnostics: &[RepairDiagnostic]) -> String {
    if diagnostics.is_empty() {
        return "; the change cannot be discharged against the stored data. Run `marrow evolve preview`, then `marrow evolve apply` after the required repair.".to_string();
    }
    let details = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "; the change cannot be discharged against the stored data: {details}. Run `marrow evolve preview`, then `marrow evolve apply` after the required repair."
    )
}

fn load_checked_for_session(
    root: &Path,
) -> Result<(ProjectConfig, CheckedSourceProgram), ProjectSessionError> {
    let config = marrow_check::load_config(root)?;
    let checked = load_checked_for_config(root, &config)?;
    Ok((config, checked))
}

fn load_checked_for_config(
    root: &Path,
    config: &ProjectConfig,
) -> Result<CheckedSourceProgram, ProjectSessionError> {
    let accepted = {
        let store = open_store_for_inspection(root, config)?;
        marrow_check::read_accepted_catalog_with_store(root, store.as_ref())?
    };
    let lock = lock_for_adoption(root, accepted.as_ref())?;
    let snapshot = marrow_check::check_source_project_analysis_against(
        root,
        config,
        accepted.as_ref(),
        lock.as_ref(),
    )?;
    Ok(CheckedSourceProgram::from_snapshot(snapshot))
}

fn accepted_catalog_for_admission(
    program: &CheckedProgram,
) -> Result<Option<marrow_catalog::CatalogMetadata>, ProjectSessionError> {
    let Some(epoch) = program.catalog.accepted_epoch else {
        return Ok(None);
    };
    let Some(digest) = program.catalog.accepted_digest.clone() else {
        return Err(ProjectSessionError::Catalog {
            code: marrow_catalog::CATALOG_INVALID,
            message: "accepted catalog digest is missing from the checked program".to_string(),
        });
    };
    let catalog = marrow_catalog::CatalogMetadata::from_stored_parts(
        epoch,
        digest,
        program.catalog.accepted_entries.clone(),
    )
    .map_err(|error| ProjectSessionError::Catalog {
        code: error.code,
        message: error.message,
    })?;
    Ok(Some(catalog))
}

fn load_checked_for_surface_session(
    root: &Path,
) -> Result<(ProjectConfig, AnalysisSnapshot), ProjectSessionError> {
    let config = marrow_check::load_config(root)?;
    let accepted = {
        let store = open_store_for_inspection(root, &config)?;
        marrow_check::read_accepted_catalog_with_store_read_only(root, store.as_ref())?
    };
    let lock = lock_for_adoption(root, accepted.as_ref())?;
    let snapshot = marrow_check::check_source_project_analysis_against(
        root,
        &config,
        accepted.as_ref(),
        lock.as_ref(),
    )?;
    Ok((config, snapshot))
}

/// The committed lock to drive first-run adoption, read only when no accepted store snapshot is
/// present. A valid store is the sole accepted authority, so when one is bound the lock is inert
/// and never read; on an empty store the committed lock seeds the fresh checkout with its
/// committed identity. A corrupt lock fails closed through the reader.
fn lock_for_adoption(
    root: &Path,
    accepted: Option<&marrow_catalog::CatalogMetadata>,
) -> Result<Option<marrow_catalog::CatalogLock>, ProjectSessionError> {
    if accepted.is_some() {
        return Ok(None);
    }
    marrow_check::read_committed_lock(root).map_err(ProjectSessionError::from)
}

fn load_checked_for_fresh_memory_session(
    root: &Path,
) -> Result<(ProjectConfig, CheckedSourceProgram), ProjectSessionError> {
    let config = marrow_check::load_config(root)?;
    let accepted = marrow_check::read_accepted_catalog_artifact(root)?;
    let snapshot = marrow_check::check_source_project_analysis_against(
        root,
        &config,
        accepted.as_ref(),
        None,
    )?;
    Ok((config, CheckedSourceProgram::from_snapshot(snapshot)))
}

fn validate_source_analysis_admission(
    snapshot: &AnalysisSnapshot,
    admission: Option<&SourceAnalysisAdmission>,
) -> Result<(), ProjectSessionError> {
    let Some(admission) = admission else {
        return Ok(());
    };
    if snapshot.content_identity() == &admission.source_analysis_identity {
        let digest = snapshot.program.read_only_context_digest();
        if digest == admission.read_only_context_digest {
            return Ok(());
        }
    }
    Err(ProjectSessionError::SchemaDrift {
        message: "source analysis changed while opening the admitted project session".to_string(),
    })
}

fn validate_checked_source_analysis_admission(
    checked: CheckedSourceProgram,
    admission: Option<&SourceAnalysisAdmission>,
) -> Result<CheckedSourceProgram, ProjectSessionError> {
    validate_source_analysis_admission(&checked.snapshot, admission)?;
    Ok(checked)
}

fn validate_no_source_analysis_admission(
    admission: Option<&SourceAnalysisAdmission>,
) -> Result<(), ProjectSessionError> {
    if admission.is_none() {
        return Ok(());
    }
    Err(ProjectSessionError::SchemaDrift {
        message: "source analysis admission does not apply after schema drift".to_string(),
    })
}

fn open_store_for_inspection(
    root: &Path,
    config: &ProjectConfig,
) -> Result<Option<TreeStore>, ProjectSessionError> {
    let Some(path) = marrow_check::native_store_path(root, config)? else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    TreeStore::open_read_only(&path)
        .map(Some)
        .map_err(ProjectSessionError::Store)
}

/// Replay an unclean shutdown before a write-capable command inspects the store.
///
/// A writable handle held for a process lifetime — an idle `serve --write`, or any
/// writer killed by a signal — leaves redb's on-disk recovery flag set even when no
/// commit was in flight, so the next read-only inspection the write command runs to
/// read the accepted catalog refuses the store as needing recovery. The backend
/// contract reserves that refusal for read-only opens: a write-capable open attempts
/// the replay. This runs that write-capable open up front, so a store with no genuine
/// interrupted commit opens clean for the rest of the command without a manual
/// `data recover`.
///
/// The replay is attempted only when a read-only probe reports the store needs it, so
/// a healthy store is never reopened for writing or modified. Genuine corruption is
/// not blessed: the replayed handle is proven structurally readable here, the
/// committed-lock witness runs in the lock-root guard, and the per-root structural
/// digest runs when the store-read open re-opens the store for the run.
pub fn recover_store_for_write(
    root: &Path,
    config: &ProjectConfig,
) -> Result<(), ProjectSessionError> {
    let Some(path) = marrow_check::native_store_path(root, config)? else {
        return Ok(());
    };
    if !path.exists() {
        return Ok(());
    }
    match TreeStore::open_read_only(&path) {
        Ok(_) => Ok(()),
        Err(StoreError::RecoveryRequired) => {
            let store = TreeStore::open_existing(&path).map_err(ProjectSessionError::Store)?;
            store
                .verify_readable()
                .map_err(ProjectSessionError::Store)?;
            Ok(())
        }
        Err(error) => Err(ProjectSessionError::Store(error)),
    }
}

fn bind_proposed_catalog_identity(
    root: &Path,
    config: &ProjectConfig,
    checked: CheckedSourceProgram,
    admission: Option<&SourceAnalysisAdmission>,
) -> Result<CheckedSourceProgram, ProjectSessionError> {
    if checked.program().catalog.accepted_epoch.is_some() {
        validate_source_analysis_admission(&checked.snapshot, admission)?;
        return Ok(checked);
    }
    let admitted = admission.map(|admission| &admission.accepted_catalog);
    let proposal = match admitted {
        Some(catalog) => catalog,
        None => {
            let Some(proposal) = checked.program().catalog.proposal.as_ref() else {
                validate_source_analysis_admission(&checked.snapshot, admission)?;
                return Ok(checked);
            };
            if proposal.entries.is_empty() {
                validate_source_analysis_admission(&checked.snapshot, admission)?;
                return Ok(checked);
            }
            proposal
        }
    };
    let snapshot =
        marrow_check::check_source_project_analysis_against(root, config, Some(proposal), None)
            .map_err(ProjectSessionError::from)?;
    validate_source_analysis_admission(&snapshot, admission)?;
    Ok(CheckedSourceProgram::from_snapshot(snapshot))
}

fn resolve_store_path(
    root: &Path,
    config: &ProjectConfig,
) -> Result<Option<PathBuf>, ProjectSessionError> {
    marrow_check::resolve_store_path(root, config).map_err(ProjectSessionError::from)
}

fn establish_store_baseline(
    root: &Path,
    config: &ProjectConfig,
    store: &TreeStore,
    checked: CheckedSourceProgram,
) -> Result<CheckedSourceProgram, ProjectSessionError> {
    if !commit_catalog_baseline(store, checked.program())? {
        return Ok(checked);
    }
    marrow_check::recheck_source_project_analysis_against_store_catalog(root, config, store)
        .map(CheckedSourceProgram::from_snapshot)
        .map_err(ProjectSessionError::from)
}

/// Re-project `marrow.lock` from the committed store baseline. The store is the sole write-time
/// authority; the lock is its committed source-tree projection. A writable commit-path open
/// re-projects after the fence agrees the store matches this binary, so a baseline this open just
/// established — or one a prior open committed before it could project the lock — converges to a
/// fresh lock. Re-projection rebuilds the accepted identity, shape, and the append-only id ledger
/// from the committed store snapshot alone (a retired id reserves its store entry), so a deleted
/// lock is recovered with its retired ids rather than re-projected empty. The projection is
/// idempotent on the bytes it would write, so a converged store re-projects nothing. It runs only
/// on the writable open path through the single lock-write owner, never as a read side effect.
fn reproject_committed_lock(
    root: &Path,
    store: &TreeStore,
    program: &CheckedProgram,
    notices: &mut Vec<ProjectSessionNotice>,
) -> Result<(), ProjectSessionError> {
    let Some(snapshot) = store.read_catalog_snapshot()? else {
        return Ok(());
    };
    match marrow_check::project_store_lock(root, &snapshot, &program.source_digest())? {
        marrow_check::LockProjection::Created => notices.push(ProjectSessionNotice::LockCreated),
        marrow_check::LockProjection::Updated => notices.push(ProjectSessionNotice::LockUpdated),
        marrow_check::LockProjection::Unchanged => {}
    }
    Ok(())
}

fn fence_run(program: &CheckedProgram, store: &TreeStore) -> Result<(), FenceError> {
    fence(
        program.catalog.accepted_epoch,
        &program.source_digest(),
        store,
    )
}

/// Whether the local store lags the committed lock. The committed lock records the epoch a write
/// path last activated against the shared source tree, so a stamped store whose epoch is below the
/// lock's high-water is a local checkout that has not caught up to an activation a teammate already
/// committed. This is reported as a store-behind fence (the operator runs `marrow evolve apply` to
/// advance the local store), distinct from same-epoch schema drift, which auto-applies a fresh
/// activation. The store remains the sole authority: the lock never rewinds or overrides it, it only
/// signals that the local store is behind a committed advance.
fn store_behind_committed_lock(
    root: &Path,
    store: &TreeStore,
) -> Result<Option<FenceError>, ProjectSessionError> {
    let Some(commit) = store.read_commit_metadata()? else {
        return Ok(None);
    };
    let Some(lock) = marrow_check::read_committed_lock(root)? else {
        return Ok(None);
    };
    if lock.epoch_high_water > commit.catalog_epoch {
        Ok(Some(FenceError::StoreBehind {
            stored: commit.catalog_epoch,
            accepted: lock.epoch_high_water,
        }))
    } else {
        Ok(None)
    }
}

/// Fail a write-capable run closed when a PRESENT store has lost the committed roots its
/// `marrow.lock` records, before establishing a fresh baseline would silently mask the loss. The
/// committed lock is the independent witness to durable identity: a present store that presents
/// fewer of the lock's active roots than the lock recorded — rolled back to its empty initial
/// commit, a partial root drop, or a uid-only store crashed mid-creation — has lost durable
/// identity and is `store.corruption`, not a clean store to re-baseline. An ABSENT store body is
/// the disposable-store case, not a loss: it passes here so the open seeds an empty store from the
/// committed identity. A genuine first run records no active root in the lock, so the witness never
/// fires either way.
///
/// A behind local checkout that keeps the same saved roots is not a loss: the witness keys on the
/// saved-root SET, so a store missing only members or indexes a later activation added passes and
/// the write path reaches the store-behind fence rather than a corruption verdict. A rolled-back,
/// crash-mid-creation, or partial-root-drop store presents fewer roots than the lock recorded, so
/// the witness fires whatever the store's epoch.
fn guard_committed_lock_roots(
    root: &Path,
    config: &ProjectConfig,
) -> Result<(), ProjectSessionError> {
    let Some(path) = marrow_check::native_store_path(root, config)? else {
        return Ok(());
    };
    let Some(lock) = marrow_check::read_committed_lock(root)? else {
        return Ok(());
    };
    if !lock.records_active_roots() {
        return Ok(());
    }
    let store = if marrow_check::tooling::store_path_is_absent(&path) {
        None
    } else {
        Some(TreeStore::open_read_only(&path)?)
    };
    marrow_check::tooling::verify_present_store_lock_roots(store.as_ref(), Some(&lock))
        .map_err(ProjectSessionError::Store)
}

/// Whether the store body is absent on disk while a committed lock records active roots — the
/// fresh-checkout or lost-store case the next write-capable open seeds from the committed identity.
/// The seed is announced loudly so the developer learns an empty store was reconstructed from the
/// lock rather than minted fresh.
fn store_absent_with_committed_lock(
    root: &Path,
    config: &ProjectConfig,
) -> Result<bool, ProjectSessionError> {
    let Some(path) = marrow_check::native_store_path(root, config)? else {
        return Ok(false);
    };
    if !marrow_check::tooling::store_path_is_absent(&path) {
        return Ok(false);
    }
    Ok(marrow_check::read_committed_lock(root)?.is_some_and(|lock| lock.records_active_roots()))
}

/// Seed an empty store from the committed identity when the store body is absent on disk while a
/// committed lock records active roots, returning the program re-derived from the seeded store. A
/// write-capable open — `serve --write` and `evolve apply` — expects a stamped, baselined store
/// carrying an accepted catalog, so a fresh checkout or a lost store body is materialized here
/// through the run write path's own uid mint and catalog baseline against the lock-adopted program,
/// then the analysis is re-projected against the seeded store's committed catalog. `Some` signals a
/// seed happened, so the caller announces it loudly. A present store is left untouched (`None`); the
/// run path already created it.
pub fn seed_absent_store_from_committed_lock(
    root: &Path,
    config: &ProjectConfig,
    program: &CheckedProgram,
) -> Result<Option<CheckedProgram>, ProjectSessionError> {
    if !store_absent_with_committed_lock(root, config)? {
        return Ok(None);
    }
    let Some(NativeRunStore { store, .. }) = open_store_file(root, config, true)? else {
        return Ok(None);
    };
    commit_catalog_baseline(&store, program)?;
    let snapshot =
        marrow_check::recheck_source_project_analysis_against_store_catalog(root, config, &store)?;
    Ok(Some(snapshot.program))
}

/// Materialize the empty committed identity in memory for a read-only open over an absent store
/// body. A fresh checkout (or a deleted body) carries no on-disk store, but the committed lock
/// fully determines the surface ABI, so a read-only serve presents the empty committed identity —
/// zero records — without ever writing the body. The write paths own the on-disk seed; a read must
/// not mutate the store. The in-memory store is minted, baselined from the lock-adopted program,
/// and the analysis re-projected against its committed catalog, so it satisfies the shared identity
/// and digest checks by construction exactly as an on-disk seed would. A present store, or a true
/// first run with no roots-recording lock, returns `None` and opens the on-disk body as usual.
fn read_only_empty_committed_identity(
    root: &Path,
    config: &ProjectConfig,
    program: &CheckedProgram,
) -> Result<Option<(CheckedProgram, NativeRunStore)>, ProjectSessionError> {
    if !store_absent_with_committed_lock(root, config)? {
        return Ok(None);
    }
    let Some(path) = marrow_check::native_store_path(root, config)? else {
        return Ok(None);
    };
    let store = TreeStore::memory();
    let mut nondeterminism = SystemNondeterminism::new();
    ensure_store_uid(&store, &mut nondeterminism)?;
    commit_catalog_baseline(&store, program)?;
    let snapshot =
        marrow_check::recheck_source_project_analysis_against_store_catalog(root, config, &store)?;
    Ok(Some((snapshot.program, NativeRunStore { path, store })))
}

fn pending_baseline(program: &CheckedProgram) -> bool {
    program.catalog.accepted_epoch.is_none()
        && program
            .catalog
            .proposal
            .as_ref()
            .is_some_and(|proposal| !proposal.entries.is_empty())
}

fn populated_unstamped_store(
    program: &CheckedProgram,
    store: &TreeStore,
) -> Result<bool, StoreError> {
    if pending_baseline(program) {
        return Ok(!store.is_empty()?);
    }
    if program.catalog.accepted_epoch.is_none() || store.read_commit_metadata()?.is_some() {
        return Ok(false);
    }
    for module in &program.modules {
        for saved in &module.stores {
            if saved_root_holds_records(program, store, &saved.root)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn saved_root_holds_records(
    program: &CheckedProgram,
    store: &TreeStore,
    root: &str,
) -> Result<bool, StoreError> {
    let Some(place) = marrow_check::checked_saved_root_place(program, root, SourceSpan::default())
    else {
        return Ok(false);
    };
    let Some(raw_store_id) = place.store_catalog_id.as_ref() else {
        return Ok(false);
    };
    let Ok(store_id) = CatalogId::new(raw_store_id.clone()) else {
        return Ok(false);
    };
    first_valid_record_identity_exists(store, &store_id, &place)
}

fn first_valid_record_identity_exists(
    store: &TreeStore,
    store_id: &CatalogId,
    place: &CheckedSavedPlace,
) -> Result<bool, StoreError> {
    if place.identity_keys.is_empty() {
        return store.data_subtree_exists(store_id, &[], &[]);
    }
    let mut identity = Vec::with_capacity(place.identity_keys.len());
    while identity.len() < place.identity_keys.len() {
        let Some(key) =
            store.record_first_child_at_arity(store_id, &identity, place.identity_keys.len())?
        else {
            return Ok(false);
        };
        validate_record_identity_key(place.identity_keys[identity.len()].scalar, &key)?;
        identity.push(key);
    }
    Ok(true)
}

fn validate_record_identity_key(
    expected: Option<ScalarType>,
    key: &SavedKey,
) -> Result<(), StoreError> {
    validate_scalar_key(key).map_err(|error| StoreError::Corruption {
        message: error.to_string(),
    })?;
    if let Some(expected) = expected
        && !scalar_key_matches_type(key, expected)
    {
        return Err(StoreError::Corruption {
            message: "stored record identity key does not match checked key type".to_string(),
        });
    }
    Ok(())
}

fn ensure_store_uid(
    store: &TreeStore,
    nondeterminism: &mut impl Nondeterminism,
) -> Result<StoreUid, ProjectSessionError> {
    if let Some(uid) = store.read_store_uid()? {
        return Ok(uid);
    }
    let uid = mint_store_uid(nondeterminism)?;
    store.write_store_uid(&uid)?;
    Ok(uid)
}

fn mint_store_uid(
    nondeterminism: &mut impl Nondeterminism,
) -> Result<StoreUid, ProjectSessionError> {
    let entropy = nondeterminism
        .entropy_u128()
        .map_err(ProjectSessionError::Entropy)?;
    Ok(StoreUid::from_entropy_bytes(entropy.to_be_bytes()))
}

struct IsolatedStore {
    store: TreeStore,
    _dir: TempStoreDir,
}

struct TempStoreDir {
    path: PathBuf,
}

impl Drop for TempStoreDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn isolated_store(source_path: &Path) -> Result<IsolatedStore, ProjectSessionError> {
    let temp_dir = create_temp_store_dir().map_err(|error| match error {
        TempStoreDirError::Io { path, error } => ProjectSessionError::Io { path, error },
        TempStoreDirError::Exhausted => ProjectSessionError::DryRunIsolationExhausted,
    })?;
    let isolated_path = temp_dir.path.join("marrow.redb");
    fs::copy(source_path, &isolated_path).map_err(|error| ProjectSessionError::Io {
        path: source_path.to_path_buf(),
        error,
    })?;
    let store =
        TreeStore::open(&isolated_path).map_err(|error| ProjectSessionError::DryRunIsolation {
            path: isolated_path.clone(),
            error,
        })?;
    Ok(IsolatedStore {
        store,
        _dir: temp_dir,
    })
}

enum TempStoreDirError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    Exhausted,
}

fn create_temp_store_dir() -> Result<TempStoreDir, TempStoreDirError> {
    let temp_root = std::env::temp_dir();
    for attempt in 0..100 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = temp_root.join(format!(
            "marrow-dry-run-store-{}-{nanos}-{attempt}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(TempStoreDir { path }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(TempStoreDirError::Io { path, error }),
        }
    }
    Err(TempStoreDirError::Exhausted)
}

#[cfg(test)]
mod tests {
    use std::io;

    use marrow_store::tree::TreeStore;

    use super::{Nondeterminism, ProjectSessionError, ensure_store_uid, mint_store_uid};

    struct FailingNondeterminism;

    impl Nondeterminism for FailingNondeterminism {
        fn now_nanos(&self) -> i128 {
            0
        }

        fn entropy_u128(&mut self) -> io::Result<u128> {
            Err(io::Error::other("entropy unavailable"))
        }
    }

    #[test]
    fn uid_minting_entropy_error_has_session_code_and_message() {
        let error = mint_store_uid(&mut FailingNondeterminism)
            .expect_err("entropy failure stops UID minting");

        assert!(matches!(error, ProjectSessionError::Entropy(_)));
        assert_eq!(error.code(), "io.entropy");
        assert_eq!(
            error.message(),
            "OS entropy unavailable: entropy unavailable"
        );
    }

    #[test]
    fn ensure_store_uid_propagates_entropy_error_without_stamping_store() {
        let store = TreeStore::memory();
        let error = ensure_store_uid(&store, &mut FailingNondeterminism)
            .expect_err("entropy failure stops store UID stamping");

        assert!(matches!(error, ProjectSessionError::Entropy(_)));
        assert!(
            store
                .read_store_uid()
                .expect("read missing store UID")
                .is_none()
        );
    }
}
