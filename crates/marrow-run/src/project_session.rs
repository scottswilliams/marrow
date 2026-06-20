use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use marrow_check::evolution::preview;
use marrow_check::{
    AnalysisIdentity, AnalysisSnapshot, CheckReport, CheckedProgram, CheckedRuntimeProgram,
    CheckedSavedPlace, ProjectConfig,
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
        }
    }

    pub fn test() -> Self {
        Self {
            mode: ProjectMode::Test,
            entry_override: None,
            run_store_policy: RunStorePolicy::Commit,
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
    source_analysis_identity: AnalysisIdentity,
    program: CheckedProgram,
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
    store_path: PathBuf,
    store: TreeStore,
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
    program: CheckedProgram,
    store_path: PathBuf,
    store: TreeStore,
}

#[derive(Clone)]
struct CheckedSourceProgram {
    source_analysis_identity: AnalysisIdentity,
    program: CheckedProgram,
}

impl CheckedSourceProgram {
    fn from_snapshot(snapshot: AnalysisSnapshot) -> Self {
        Self {
            source_analysis_identity: snapshot.content_identity,
            program: snapshot.program,
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
    Native {
        path: PathBuf,
        store: Option<TreeStore>,
    },
    Isolated(IsolatedStore),
}

impl fmt::Debug for RunStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Memory(_) => formatter.write_str("Memory"),
            Self::Native { path, store } => formatter
                .debug_struct("Native")
                .field("path", path)
                .field("open", &store.is_some())
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
    AutoApplied { from_epoch: u64, to_epoch: u64 },
    DryRunWouldFreeze,
    DryRunWouldApply { from_epoch: u64, to_epoch: u64 },
    DryRunWouldFence { message: String },
}

impl ProjectSessionNotice {
    pub fn message(&self) -> String {
        match self {
            Self::AutoApplied {
                from_epoch,
                to_epoch,
            } => {
                format!("auto-applied evolution: catalog epoch {from_epoch} -> {to_epoch}")
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
            // The `dataDir` directory-creation fault owns its `config.data_dir`
            // code and write-path message in `marrow-check`; surface those rather
            // than reclassifying I/O failures by errno here.
            ref create @ marrow_check::ProjectIoError::DataDirCreate { .. } => Self::Config {
                code: create.code(),
                message: create.message(),
            },
            marrow_check::ProjectIoError::Config { code, message } => {
                Self::Config { code, message }
            }
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
        let (config, checked) = match open.mode {
            ProjectMode::Run if open.run_store_policy == RunStorePolicy::FreshMemory => {
                load_checked_for_fresh_memory_session(&root)?
            }
            ProjectMode::Run | ProjectMode::Test => load_checked_for_session(&root)?,
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
        &self.source_analysis_identity
    }

    pub fn program(&self) -> &CheckedProgram {
        &self.program
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
                store:
                    RunStore::Native {
                        store: Some(store), ..
                    },
                ..
            } => store,
            _ => return Ok(None),
        };
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
                (
                    RunStore::Native {
                        store: Some(store), ..
                    },
                    SessionWrites::Commit,
                ) => invoke_store(store, &call, invocation),
                (RunStore::Native { path, .. }, SessionWrites::Isolate)
                | (RunStore::Native { path, store: None }, SessionWrites::Commit) => {
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

impl ProjectSurfaceReadSession {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ProjectSessionError> {
        let root = root.as_ref().to_path_buf();
        let (config, program) = load_checked_for_surface_session(&root)?;
        let opened = open_surface_session(root, config, program, SurfaceStoreAccess::ReadOnly)?;
        Ok(Self {
            root: opened.root,
            program: opened.program,
            store_path: opened.store_path,
            store: opened.store,
        })
    }

    pub fn program(&self) -> &CheckedProgram {
        &self.program
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

impl ProjectSurfaceSession {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ProjectSessionError> {
        let root = root.as_ref().to_path_buf();
        let (config, program) = load_checked_for_surface_session(&root)?;
        let opened = open_surface_session(root, config, program, SurfaceStoreAccess::Write)?;
        Ok(Self {
            root: opened.root,
            program: opened.program,
            store_path: opened.store_path,
            store: opened.store,
        })
    }

    pub fn program(&self) -> &CheckedProgram {
        &self.program
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
    let store = match open.run_store_policy {
        RunStorePolicy::Commit => open_run_store(&root, &config, checked, false, &mut notices),
        RunStorePolicy::Isolated => open_run_store(&root, &config, checked, true, &mut notices),
        RunStorePolicy::FreshMemory => open_fresh_memory_run_store(&root, &config, checked),
    }?;
    let CheckedSourceProgram {
        source_analysis_identity,
        program,
    } = store.checked;
    let runtime = program.runtime();
    Ok(ProjectSession {
        root,
        config,
        source_analysis_identity,
        program,
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
    let checked = bind_proposed_catalog_identity(&root, &config, checked)?;
    let source_analysis_identity = checked.source_analysis_identity;
    let program = checked.program;
    let source_module_count = program.modules.len();
    let (test_report, program) = marrow_check::check_tests_program(&root, &config, program)
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
    Ok(ProjectSession {
        root,
        config,
        source_analysis_identity,
        program,
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
}

#[derive(Clone, Copy)]
enum SurfaceStoreAccess {
    ReadOnly,
    Write,
}

fn open_surface_session(
    root: PathBuf,
    config: ProjectConfig,
    program: CheckedProgram,
    access: SurfaceStoreAccess,
) -> Result<OpenSurfaceSession, ProjectSessionError> {
    let store = open_existing_surface_store(&root, &config, access)?;
    if populated_unstamped_store(&program, &store.store)? {
        return Err(ProjectSessionError::UnstampedStore);
    }
    if program.catalog.accepted_epoch.is_none() {
        return Err(ProjectSessionError::DurableStoreRequired);
    }
    let accepted_digest =
        program
            .catalog
            .accepted_digest
            .as_deref()
            .ok_or_else(|| ProjectSessionError::Catalog {
                code: marrow_catalog::CATALOG_INVALID,
                message: "accepted catalog digest is missing from the checked program".to_string(),
            })?;
    if store.store.read_store_uid()?.is_none() || store.store.read_commit_metadata()?.is_none() {
        return Err(ProjectSessionError::DurableStoreRequired);
    }
    fence_run(&program, &store.store).map_err(ProjectSessionError::Fence)?;
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
    })
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
    let uid = store
        .read_store_uid()?
        .ok_or(ProjectSessionError::DurableStoreRequired)?;
    let commit = store
        .read_commit_metadata()?
        .ok_or(ProjectSessionError::DurableStoreRequired)?;
    Ok(StoreStamp {
        store_uid: uid.as_str().to_string(),
        catalog_epoch: commit.catalog_epoch,
        commit_id: commit.commit_id,
    })
}

struct OpenRunStore {
    checked: CheckedSourceProgram,
    store: RunStore,
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
    notices: &mut Vec<ProjectSessionNotice>,
) -> Result<OpenRunStore, ProjectSessionError> {
    let Some(store) = open_store_file(root, config, !isolate_writes)? else {
        if isolate_writes && pending_baseline(&checked.program) {
            notices.push(ProjectSessionNotice::DryRunWouldFreeze);
            return open_memory_preview_store(root, config, checked);
        }
        return open_memory_store(checked);
    };
    if isolate_writes && pending_baseline(&checked.program) {
        if populated_unstamped_store(&checked.program, &store.store)? {
            return Err(ProjectSessionError::UnstampedStore);
        }
        notices.push(ProjectSessionNotice::DryRunWouldFreeze);
        let checked = bind_proposed_catalog_identity(root, config, checked)?;
        return finish_open(checked, store, true);
    }
    let checked = if isolate_writes {
        checked
    } else {
        establish_store_baseline(root, config, &store.store, checked)?
    };
    match fence_run(&checked.program, &store.store) {
        Ok(()) => finish_open(checked, store, isolate_writes),
        Err(FenceError::SchemaDrift) => {
            if isolate_writes {
                return classify_dry_run_drift(root, config, &checked, store, notices);
            }
            auto_apply_then_reopen(root, config, checked, store, isolate_writes, notices)
        }
        Err(error) => Err(ProjectSessionError::Fence(error)),
    }
}

fn open_memory_preview_store(
    root: &Path,
    config: &ProjectConfig,
    checked: CheckedSourceProgram,
) -> Result<OpenRunStore, ProjectSessionError> {
    open_bound_memory_store(bind_proposed_catalog_identity(root, config, checked)?)
}

fn open_memory_store(checked: CheckedSourceProgram) -> Result<OpenRunStore, ProjectSessionError> {
    if pending_baseline(&checked.program) {
        return Err(ProjectSessionError::DurableStoreRequired);
    }
    open_bound_memory_store(checked)
}

fn open_fresh_memory_run_store(
    root: &Path,
    config: &ProjectConfig,
    checked: CheckedSourceProgram,
) -> Result<OpenRunStore, ProjectSessionError> {
    open_bound_memory_store(bind_proposed_catalog_identity(root, config, checked)?)
}

fn open_bound_memory_store(
    checked: CheckedSourceProgram,
) -> Result<OpenRunStore, ProjectSessionError> {
    Ok(OpenRunStore {
        checked,
        store: RunStore::Memory(TreeStore::memory()),
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
    Ok(Some(NativeRunStore { path, store }))
}

fn finish_open(
    checked: CheckedSourceProgram,
    store: NativeRunStore,
    isolate_writes: bool,
) -> Result<OpenRunStore, ProjectSessionError> {
    if populated_unstamped_store(&checked.program, &store.store)? {
        return Err(ProjectSessionError::UnstampedStore);
    }
    let NativeRunStore { path, store } = store;
    Ok(OpenRunStore {
        checked,
        store: RunStore::Native {
            path,
            store: (!isolate_writes).then_some(store),
        },
    })
}

fn auto_apply_then_reopen(
    root: &Path,
    config: &ProjectConfig,
    checked: CheckedSourceProgram,
    store: NativeRunStore,
    isolate_writes: bool,
    notices: &mut Vec<ProjectSessionNotice>,
) -> Result<OpenRunStore, ProjectSessionError> {
    let witness = preview(&checked.program, &store.store)
        .map_err(ProjectSessionError::Store)?
        .0;
    let (from_epoch, to_epoch) = witness.epoch_range();
    match try_auto_apply(&witness, &checked.program, &store.store) {
        Ok(AutoApplyOutcome::Applied) => {
            notices.push(ProjectSessionNotice::AutoApplied {
                from_epoch,
                to_epoch,
            });
        }
        Ok(AutoApplyOutcome::MustFence(obligation)) => {
            return Err(ProjectSessionError::SchemaDrift {
                message: fence_message(&obligation),
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
    fence_run(&checked.program, &store.store).map_err(ProjectSessionError::Fence)?;
    finish_open(checked, store, isolate_writes)
}

fn classify_dry_run_drift(
    root: &Path,
    config: &ProjectConfig,
    checked: &CheckedSourceProgram,
    store: NativeRunStore,
    notices: &mut Vec<ProjectSessionNotice>,
) -> Result<OpenRunStore, ProjectSessionError> {
    let witness = preview(&checked.program, &store.store)
        .map_err(ProjectSessionError::Store)?
        .0;
    let (from_epoch, to_epoch) = witness.epoch_range();
    let obligation = RunObligation::classify(&witness);
    match obligation {
        RunObligation::ZeroMutation { .. } => {
            notices.push(ProjectSessionNotice::DryRunWouldApply {
                from_epoch,
                to_epoch,
            });
            let isolated = isolated_store(&store.path)?;
            match try_auto_apply(&witness, &checked.program, &isolated.store) {
                Ok(AutoApplyOutcome::Applied) => {}
                Ok(AutoApplyOutcome::MustFence(obligation)) => {
                    notices.push(ProjectSessionNotice::DryRunWouldFence {
                        message: fence_message(&obligation),
                    });
                    return finish_open(checked.clone(), store, true);
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
            fence_run(&checked.program, &isolated.store).map_err(ProjectSessionError::Fence)?;
            return Ok(OpenRunStore {
                checked,
                store: RunStore::Isolated(isolated),
            });
        }
        obligation => {
            notices.push(ProjectSessionNotice::DryRunWouldFence {
                message: fence_message(&obligation),
            });
        }
    }
    finish_open(checked.clone(), store, true)
}

fn fence_message(obligation: &RunObligation) -> String {
    let base = "store was stamped under a different schema at this catalog epoch";
    let cause = match obligation {
        RunObligation::Backfill { records } => format!(
            "; the change backfills {records} record(s). Run `marrow evolve apply` to discharge it."
        ),
        RunObligation::Transform { records } => format!(
            "; the change rewrites {records} record(s). Run `marrow evolve apply` to discharge it."
        ),
        RunObligation::DestructiveDrop { populated } => format!(
            "; the change drops {populated} populated record(s). Run `marrow evolve apply --maintenance` and confirm the retire to discharge it."
        ),
        RunObligation::Repair => "; the change cannot be discharged against the stored data. Run `marrow evolve preview` to see the required repair.".to_string(),
        RunObligation::ZeroMutation { .. } => String::new(),
    };
    format!("{base}{cause}")
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
    let snapshot =
        marrow_check::check_source_project_analysis_against(root, config, accepted.as_ref())?;
    Ok(CheckedSourceProgram::from_snapshot(snapshot))
}

fn load_checked_for_surface_session(
    root: &Path,
) -> Result<(ProjectConfig, CheckedProgram), ProjectSessionError> {
    let config = marrow_check::load_config(root)?;
    let accepted = {
        let store = open_store_for_inspection(root, &config)?;
        marrow_check::read_accepted_catalog_with_store_read_only(root, store.as_ref())?
    };
    let program = marrow_check::check_project_against(root, &config, accepted.as_ref())?;
    Ok((config, program))
}

fn load_checked_for_fresh_memory_session(
    root: &Path,
) -> Result<(ProjectConfig, CheckedSourceProgram), ProjectSessionError> {
    let config = marrow_check::load_config(root)?;
    let accepted = marrow_check::read_accepted_catalog_artifact(root)?;
    let snapshot =
        marrow_check::check_source_project_analysis_against(root, &config, accepted.as_ref())?;
    Ok((config, CheckedSourceProgram::from_snapshot(snapshot)))
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

fn bind_proposed_catalog_identity(
    root: &Path,
    config: &ProjectConfig,
    checked: CheckedSourceProgram,
) -> Result<CheckedSourceProgram, ProjectSessionError> {
    if checked.program.catalog.accepted_epoch.is_some() {
        return Ok(checked);
    }
    let Some(proposal) = checked.program.catalog.proposal.clone() else {
        return Ok(checked);
    };
    if proposal.entries.is_empty() {
        return Ok(checked);
    }
    marrow_check::check_source_project_analysis_against(root, config, Some(&proposal))
        .map(CheckedSourceProgram::from_snapshot)
        .map_err(ProjectSessionError::from)
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
    if !commit_catalog_baseline(store, &checked.program)? {
        return Ok(checked);
    }
    marrow_check::recheck_source_project_analysis_against_store_catalog(root, config, store)
        .map(CheckedSourceProgram::from_snapshot)
        .map_err(ProjectSessionError::from)
}

fn fence_run(program: &CheckedProgram, store: &TreeStore) -> Result<(), FenceError> {
    fence(
        program.catalog.accepted_epoch,
        &program.source_digest(),
        store,
    )
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
