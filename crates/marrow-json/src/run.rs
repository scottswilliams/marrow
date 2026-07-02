use marrow_check::{
    AnalysisCatalogGeneration, AnalysisGeneration, CheckedProgram, CheckedRuntimeProgram,
    EntryCostShapeFact, EntryDescriptor, EntryFootprintFact, EntryStoreOpenMode, Severity,
    WorkShapeClass,
};
use marrow_run::{
    EntryIdentity, ExecutionBoundary, ExecutionBoundaryStoreKind, ExecutionSessionKind,
    ProjectInvokeError, ProjectSession, ProjectSessionError, RunOutput, RuntimeError, StoreStamp,
};
use serde::Serialize;

const RUN_OUTPUT_CAP: usize = 8 * 1024;
const RUN_DIAGNOSTIC_MESSAGE_CAP: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunEnvelopeJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<RunResultJson>,
    pub output: String,
    pub diagnostics: Vec<RunDiagnosticJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store_stamp: Option<Option<RunStoreStampJson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub committed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_applied: Option<RunAutoAppliedJson>,
}

impl RunEnvelopeJson {
    pub fn with_store_state(mut self, state: RunStoreStateJson) -> Self {
        self.store_stamp = Some(state.stamp);
        if state.committed {
            self.committed = Some(true);
        }
        self
    }

    pub fn with_auto_applied(mut self, auto_applied: Option<RunAutoAppliedJson>) -> Self {
        self.auto_applied = auto_applied;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunResultJson {
    Value { value: EntryReturnJson },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryReturnJson {
    Int {
        value: i64,
    },
    Bool {
        value: bool,
    },
    String {
        value: String,
        truncated: bool,
        #[serde(rename = "originalBytes")]
        original_bytes: usize,
    },
    Decimal {
        value: String,
    },
    Date {
        value: i32,
    },
    Duration {
        value: String,
    },
    Instant {
        value: String,
    },
    Bytes {
        #[serde(rename = "value_b64")]
        value_b64: String,
        truncated: bool,
        #[serde(rename = "originalBytes")]
        original_bytes: usize,
    },
    Enum {
        member: String,
    },
    Identity {
        root: String,
        keys: Vec<EntryReturnSavedKeyJson>,
        #[serde(rename = "keysTruncated")]
        keys_truncated: bool,
    },
    Sequence {
        values: Vec<EntryReturnJson>,
        truncated: bool,
    },
    Truncated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntryReturnSavedKeyJson {
    Int {
        value: i64,
    },
    Bool {
        value: bool,
    },
    String {
        value: String,
        truncated: bool,
        #[serde(rename = "originalBytes")]
        original_bytes: usize,
    },
    Date {
        days_since_epoch: i32,
    },
    Duration {
        nanos: String,
    },
    Instant {
        nanos_since_epoch: String,
    },
    Bytes {
        #[serde(rename = "value_b64")]
        value_b64: String,
        truncated: bool,
        #[serde(rename = "originalBytes")]
        original_bytes: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunDiagnosticJson {
    pub code: String,
    pub kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<&'static str>,
    pub source_span: Option<RunSourceSpanJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<RunDiagnosticDataJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct RunDiagnosticDataJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_depth: Option<usize>,
}

/// A located diagnostic position. A fault without a file has no placeable
/// location, so the envelope carries `source_span: null` rather than a span
/// object with a null file — keeping `file` non-optional makes that
/// fabrication unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunSourceSpanJson {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunStoreStampJson {
    pub store_uid: String,
    pub catalog_epoch: u64,
    pub commit_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStoreStateJson {
    stamp: Option<RunStoreStampJson>,
    committed: bool,
}

impl RunStoreStateJson {
    pub fn from_stamps(stamp: Option<&StoreStamp>, before: Option<&StoreStamp>) -> Self {
        let committed = stamp
            .zip(before)
            .is_some_and(|(stamp, before)| stamp.commit_id != before.commit_id);
        Self {
            stamp: stamp.map(RunStoreStampJson::from),
            committed,
        }
    }
}

impl From<&StoreStamp> for RunStoreStampJson {
    fn from(stamp: &StoreStamp) -> Self {
        Self {
            store_uid: stamp.store_uid.clone(),
            catalog_epoch: stamp.catalog_epoch,
            commit_id: stamp.commit_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunAutoAppliedJson {
    pub from_epoch: u64,
    pub to_epoch: u64,
}

impl From<(u64, u64)> for RunAutoAppliedJson {
    fn from((from_epoch, to_epoch): (u64, u64)) -> Self {
        Self {
            from_epoch,
            to_epoch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryRunFactsJson {
    pub entry: EntryIdentityJson,
    #[serde(rename = "executionBoundary")]
    pub execution_boundary: ExecutionBoundaryJson,
    #[serde(rename = "storeOpenMode")]
    pub store_open_mode: &'static str,
    pub footprint: EntryFootprintJson,
    #[serde(rename = "costShape")]
    pub cost_shape: EntryCostShapeJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryRunAnalysisJson {
    #[serde(rename = "profileVersion")]
    pub profile_version: &'static str,
    #[serde(rename = "sourceIdentity")]
    pub source_identity: String,
    #[serde(rename = "configDigest")]
    pub config_digest: String,
    #[serde(rename = "checkedSourceDigest")]
    pub checked_source_digest: String,
    #[serde(rename = "readOnlyContextDigest")]
    pub read_only_context_digest: String,
    #[serde(rename = "acceptedCatalog")]
    pub accepted_catalog: Option<EntryRunAnalysisCatalogJson>,
    #[serde(rename = "proposalCatalog")]
    pub proposal_catalog: Option<EntryRunAnalysisCatalogJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryRunAnalysisCatalogJson {
    pub epoch: u64,
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecutionBoundaryJson {
    #[serde(rename = "sessionKind")]
    pub session_kind: &'static str,
    #[serde(rename = "sourceAnalysisGeneration")]
    pub source_analysis_generation: EntryRunAnalysisJson,
    pub store: ExecutionStoreBoundaryJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecutionStoreBoundaryJson {
    pub kind: &'static str,
    pub stamp: Option<RunStoreStampJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryIdentityJson {
    #[serde(rename = "requestedName")]
    pub requested_name: String,
    #[serde(rename = "canonicalName")]
    pub canonical_name: String,
    #[serde(rename = "entryTag")]
    pub entry_tag: String,
    #[serde(rename = "acceptedCatalogEpoch")]
    pub accepted_catalog_epoch: Option<u64>,
    #[serde(rename = "sourceDigest")]
    pub source_digest: String,
    #[serde(rename = "readOnlyContextDigest")]
    pub read_only_context_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryFootprintJson {
    pub entry: String,
    #[serde(rename = "writeEffectsReachable")]
    pub write_effects_reachable: bool,
    #[serde(rename = "storesRead")]
    pub stores_read: Vec<String>,
    #[serde(rename = "storesWritten")]
    pub stores_written: Vec<String>,
    #[serde(rename = "indexesTouched")]
    pub indexes_touched: Vec<String>,
    #[serde(rename = "workShape")]
    pub work_shape: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryCostShapeJson {
    pub entry: String,
    #[serde(rename = "workShape")]
    pub work_shape: &'static str,
    #[serde(rename = "pointReads")]
    pub point_reads: usize,
    #[serde(rename = "rangeScans")]
    pub range_scans: usize,
    pub writes: usize,
    #[serde(rename = "indexEntryTouches")]
    pub index_entry_touches: usize,
    #[serde(rename = "commitPoints")]
    pub commit_points: usize,
}

pub fn run_output_to_json(
    result: &RunOutput,
    output: String,
) -> Result<RunEnvelopeJson, RuntimeError> {
    let result = match result.value.as_ref() {
        Some(value) => RunResultJson::Value {
            value: crate::entry_return_to_json_bounded(value).map_err(|_| {
                RuntimeError::entry_surface(
                    "entry return value is outside the run JSON result surface",
                )
            })?,
        },
        None => RunResultJson::None,
    };
    Ok(RunEnvelopeJson {
        result: Some(result),
        output: truncate_output(output),
        diagnostics: Vec::new(),
        store_stamp: None,
        committed: None,
        auto_applied: None,
    })
}

pub fn run_error_to_json(
    program: &CheckedRuntimeProgram,
    error: &ProjectInvokeError,
    output: String,
) -> RunEnvelopeJson {
    match error {
        ProjectInvokeError::Runtime(error) => {
            runtime_error_to_json(error, runtime_error_source_file(program, error), output)
        }
        ProjectInvokeError::Session(error) => project_session_error_to_json(error, output),
    }
}

pub fn run_session_error_to_json(error: &ProjectSessionError, output: String) -> RunEnvelopeJson {
    project_session_error_to_json(error, output)
}

fn runtime_error_source_file(
    program: &CheckedRuntimeProgram,
    error: &RuntimeError,
) -> Option<String> {
    error
        .origin
        .and_then(|id| program.file_path(id))
        .map(|path| path.display().to_string())
}

pub fn entry_run_facts_to_json(session: &ProjectSession) -> Option<EntryRunFactsJson> {
    let entry = session.run_entry()?;
    let program = session.program();
    let runtime = session.runtime_program();
    let descriptor = EntryDescriptor::resolve(runtime, entry).ok()?;
    let identity = &descriptor.identity;
    let facts = program.entry_run_facts(&identity.canonical_name)?;
    let boundary = session.execution_boundary();
    if identity.canonical_name != facts.footprint.entry
        || facts.footprint.entry != facts.cost_shape.entry
    {
        return None;
    }
    Some(EntryRunFactsJson {
        entry: entry_identity_to_json(identity),
        execution_boundary: execution_boundary_facts_to_json(boundary),
        store_open_mode: store_open_mode_name(facts.store_open_mode),
        footprint: entry_footprint_to_json(program, &facts.footprint)?,
        cost_shape: entry_cost_shape_to_json(&facts.cost_shape),
    })
}

pub fn execution_boundary_to_json(session: &ProjectSession) -> ExecutionBoundaryJson {
    execution_boundary_facts_to_json(session.execution_boundary())
}

fn execution_boundary_facts_to_json(boundary: ExecutionBoundary) -> ExecutionBoundaryJson {
    ExecutionBoundaryJson {
        session_kind: execution_session_kind_name(boundary.session_kind),
        source_analysis_generation: EntryRunAnalysisJson::from(boundary.source_analysis_generation),
        store: ExecutionStoreBoundaryJson {
            kind: execution_store_kind_name(boundary.store.kind),
            stamp: boundary.store.stamp.as_ref().map(RunStoreStampJson::from),
        },
    }
}

fn execution_session_kind_name(kind: ExecutionSessionKind) -> &'static str {
    match kind {
        ExecutionSessionKind::Run => "run",
        ExecutionSessionKind::Test => "test",
    }
}

fn execution_store_kind_name(kind: ExecutionBoundaryStoreKind) -> &'static str {
    match kind {
        ExecutionBoundaryStoreKind::FreshMemory => "fresh_memory",
        ExecutionBoundaryStoreKind::Isolated => "isolated",
        ExecutionBoundaryStoreKind::NativeCommit => "native_commit",
        ExecutionBoundaryStoreKind::TestMemory => "test_memory",
        ExecutionBoundaryStoreKind::PlainMemory => "plain_memory",
    }
}

impl From<AnalysisGeneration> for EntryRunAnalysisJson {
    fn from(generation: AnalysisGeneration) -> Self {
        Self {
            profile_version: generation.profile_version,
            source_identity: generation.content_identity.as_str().to_string(),
            config_digest: generation.config_digest.as_str().to_string(),
            checked_source_digest: generation.checked_source_digest,
            read_only_context_digest: generation.read_only_context_digest,
            accepted_catalog: generation
                .accepted_catalog
                .map(EntryRunAnalysisCatalogJson::from),
            proposal_catalog: generation
                .proposal_catalog
                .map(EntryRunAnalysisCatalogJson::from),
        }
    }
}

impl From<AnalysisCatalogGeneration> for EntryRunAnalysisCatalogJson {
    fn from(catalog: AnalysisCatalogGeneration) -> Self {
        Self {
            epoch: catalog.epoch,
            digest: catalog.digest,
        }
    }
}

fn runtime_error_to_json(
    error: &RuntimeError,
    source_file: Option<String>,
    output: String,
) -> RunEnvelopeJson {
    RunEnvelopeJson {
        result: None,
        output: truncate_output(output),
        diagnostics: vec![runtime_diagnostic_to_json(error, source_file)],
        store_stamp: None,
        committed: None,
        auto_applied: None,
    }
}

fn project_session_error_to_json(error: &ProjectSessionError, output: String) -> RunEnvelopeJson {
    if let ProjectSessionError::Check { report } = error {
        let diagnostics = report
            .diagnostics
            .iter()
            .map(|diagnostic| RunDiagnosticJson {
                code: diagnostic.code.to_string(),
                kind: marrow_check::kind_for_code(diagnostic.code).to_string(),
                message: bounded_diagnostic_message(&diagnostic.message),
                severity: Some(severity_name(diagnostic.severity)),
                source_span: Some(RunSourceSpanJson {
                    file: diagnostic.file.display().to_string(),
                    line: diagnostic.span.line,
                    column: diagnostic.span.column,
                }),
                data: None,
            })
            .collect();
        return RunEnvelopeJson {
            result: None,
            output: truncate_output(output),
            diagnostics,
            store_stamp: None,
            committed: None,
            auto_applied: None,
        };
    }
    RunEnvelopeJson {
        result: None,
        output: truncate_output(output),
        diagnostics: vec![RunDiagnosticJson {
            code: error.code().to_string(),
            kind: marrow_check::kind_for_code(error.code()).to_string(),
            message: bounded_diagnostic_message(&error.message()),
            severity: None,
            source_span: None,
            data: None,
        }],
        store_stamp: None,
        committed: None,
        auto_applied: None,
    }
}

fn runtime_diagnostic_to_json(
    error: &RuntimeError,
    source_file: Option<String>,
) -> RunDiagnosticJson {
    RunDiagnosticJson {
        code: error.code().to_string(),
        data: Some(runtime_diagnostic_data(error)),
        kind: marrow_check::kind_for_code(error.code()).to_string(),
        message: bounded_diagnostic_message(&error.message),
        severity: None,
        source_span: source_file.map(|file| RunSourceSpanJson {
            file,
            line: error.span.line,
            column: error.span.column,
        }),
    }
}

fn runtime_diagnostic_data(error: &RuntimeError) -> RunDiagnosticDataJson {
    let mut data = RunDiagnosticDataJson::default();
    if let Some(code) = error.uncaught_throw_code() {
        data.code = Some(code.to_string());
    }
    if let Some(call_depth) = error.call_depth() {
        data.callee = Some(call_depth.function_name.clone());
        data.budget = Some(call_depth.budget);
        data.observed_depth = Some(call_depth.observed_depth);
    }
    data
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

fn entry_identity_to_json(identity: &EntryIdentity) -> EntryIdentityJson {
    EntryIdentityJson {
        requested_name: identity.requested_name.clone(),
        canonical_name: identity.canonical_name.clone(),
        entry_tag: identity.entry_tag.clone(),
        accepted_catalog_epoch: identity.accepted_catalog_epoch,
        source_digest: identity.source_digest.clone(),
        read_only_context_digest: identity.read_only_context_digest.clone(),
    }
}

fn entry_footprint_to_json(
    program: &CheckedProgram,
    footprint: &EntryFootprintFact,
) -> Option<EntryFootprintJson> {
    Some(EntryFootprintJson {
        entry: footprint.entry.clone(),
        write_effects_reachable: footprint.write_effects_reachable,
        stores_read: store_structural_paths(program, &footprint.stores_read)?,
        stores_written: store_structural_paths(program, &footprint.stores_written)?,
        indexes_touched: store_index_structural_paths(program, &footprint.indexes_touched)?,
        work_shape: work_shape_name(footprint.work_shape),
    })
}

fn entry_cost_shape_to_json(shape: &EntryCostShapeFact) -> EntryCostShapeJson {
    EntryCostShapeJson {
        entry: shape.entry.clone(),
        work_shape: work_shape_name(shape.work_shape),
        point_reads: shape.point_reads,
        range_scans: shape.range_scans,
        writes: shape.writes,
        index_entry_touches: shape.index_entry_touches,
        commit_points: shape.commit_points,
    }
}

fn store_structural_paths(
    program: &CheckedProgram,
    stores: &[marrow_check::StoreId],
) -> Option<Vec<String>> {
    stores
        .iter()
        .map(|store| program.store_structural_path(*store))
        .collect()
}

fn store_index_structural_paths(
    program: &CheckedProgram,
    indexes: &[marrow_check::StoreIndexId],
) -> Option<Vec<String>> {
    indexes
        .iter()
        .map(|index| program.store_index_structural_path(*index))
        .collect()
}

fn work_shape_name(shape: WorkShapeClass) -> &'static str {
    match shape {
        WorkShapeClass::ComputeOnly => "compute_only",
        WorkShapeClass::ReadOnly => "read_only",
        WorkShapeClass::WritesSavedData => "writes_saved_data",
    }
}

fn store_open_mode_name(mode: EntryStoreOpenMode) -> &'static str {
    match mode {
        EntryStoreOpenMode::ReadOnly => "read_only",
        EntryStoreOpenMode::WriteCapable => "write_capable",
    }
}

struct BoundedDiagnosticMessage {
    value: String,
}

fn bounded_diagnostic_message(value: &str) -> String {
    bounded_diagnostic_message_inner(value).value
}

fn bounded_diagnostic_message_inner(value: &str) -> BoundedDiagnosticMessage {
    if value.len() <= RUN_DIAGNOSTIC_MESSAGE_CAP {
        return BoundedDiagnosticMessage {
            value: value.to_string(),
        };
    }

    let mut end = RUN_DIAGNOSTIC_MESSAGE_CAP;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut value = value[..end].to_string();
    value.push('\u{2026}');
    BoundedDiagnosticMessage { value }
}

fn truncate_output(mut output: String) -> String {
    if output.len() > RUN_OUTPUT_CAP {
        let mut end = RUN_OUTPUT_CAP;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
        output.push_str("\n\u{2026}output truncated\u{2026}");
    }
    output
}
