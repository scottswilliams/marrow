//! The process-main coordinator and its reader/worker/writer threads.
//!
//! The reader frames stdin into a cap-1 ingress queue and backpressures without drop or
//! reorder. The coordinator owns lifecycle, admission, document versions, overlay
//! construction, latest-wins edit coalescing, and outbound ordering; it never blocks on
//! I/O, downstream sends, or joins — it idles only on a cap-1 lost-wakeup-safe wake
//! channel and drains receipts, then results, then ingress. One analysis worker owns all
//! capture/analyze work behind the single worker credit. One writer accepts immutable
//! framed bytes and returns a delivery receipt that frees the outbound credit it consumed.
//!
//! The coordinator is a *pure event machine*: [`Coordinator`] consumes typed events
//! (`on_frame`, `on_worker_result`, `on_receipt`, `on_terminal`) and produces outbound
//! frames into an [`Coordinator::outbox`] and at most one analysis job into
//! [`Coordinator::job_out`], which the thread driver drains. It touches no channel, so
//! the whole law matrix — request-ledger delivery states and terminal arbitration, the
//! shared live-entry budget, the capture-episode latch, publication exclusivity, and
//! `ContentModified` reauthorization — is driven deterministically in-crate without a
//! test-only production entry point or any timing dependence.

use std::collections::VecDeque;
use std::io::BufReader;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;

use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentFormattingParams, GotoDefinitionParams, HoverParams, InitializeParams,
    InitializeResult, OneOf, ServerCapabilities, ServerInfo, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions,
};
use marrow_compile::{AnalysisSnapshot, InputRevision};

use crate::analysis::{AnalysisOutcome, CaptureRejection, OverlayInput, run_analysis};
use crate::capacities::{
    MAX_ANONYMOUS_ERROR_SLOTS, MAX_LIVE_REQUEST_ENTRIES, OUTBOUND_QUEUE_CAPACITY,
    RECEIPT_QUEUE_CAPACITY, THREAD_STACK_BYTES,
};
use crate::credit::{CreditPool, OutboundCredit, PublicationPlanCredit};
use crate::document::{DocumentLedger, DocumentState, RevisionCounter, UnavailableEvidence};
use crate::facts;
use crate::lifecycle::{
    CONTENT_MODIFIED, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, Lifecycle, METHOD_NOT_FOUND,
    PARSE_ERROR, Phase, REQUEST_FAILED, RequestGate, SERVER_NOT_INITIALIZED,
};
use crate::outbound::{MessageType, Outbound, encode};
use crate::protocol::{Inbound, InvalidReason, Reject, RequestId, decode};
use crate::uri::{DocumentKey, SelectedRoot, UriError};

/// A completed analysis job returned by the worker.
struct WorkerResult {
    outcome: AnalysisOutcome,
}

/// A unit of capture/analyze work handed to the worker.
struct WorkerJob {
    root: SelectedRoot,
    revision: InputRevision,
    overlay: Vec<(String, Vec<u8>)>,
}

/// A writer delivery receipt: one completed and flushed frame.
struct Receipt;

/// Run the language server over stdio, returning the process exit code.
pub fn serve() -> u8 {
    // Install a payload-free no-I/O panic hook before any thread or input, so a producer
    // panic cannot deadlock the process on a formatting or I/O attempt.
    std::panic::set_hook(Box::new(|_| {}));

    let (ingress_tx, ingress_rx) = sync_channel::<ReaderEvent>(1);
    let (work_tx, work_rx) = sync_channel::<WorkerJob>(1);
    let (result_tx, result_rx) = sync_channel::<WorkerResult>(1);
    let (frame_tx, frame_rx) = sync_channel::<Vec<u8>>(OUTBOUND_QUEUE_CAPACITY);
    let (receipt_tx, receipt_rx) = sync_channel::<Receipt>(RECEIPT_QUEUE_CAPACITY);
    let (wake_tx, wake_rx) = sync_channel::<()>(1);

    let reader = spawn("marrow-lsp-reader", {
        let wake_tx = wake_tx.clone();
        move || reader_loop(ingress_tx, wake_tx)
    });
    let worker = spawn("marrow-lsp-worker", {
        let wake_tx = wake_tx.clone();
        move || worker_loop(&work_rx, &result_tx, &wake_tx)
    });
    let writer = spawn("marrow-lsp-writer", {
        let wake_tx = wake_tx.clone();
        move || writer_loop(&frame_rx, &receipt_tx, &wake_tx)
    });

    let mut coordinator = Coordinator::new();
    let exit = drive(
        &mut coordinator,
        &ingress_rx,
        &result_rx,
        &receipt_rx,
        &wake_rx,
        &work_tx,
        &frame_tx,
    );

    drop(work_tx);
    drop(frame_tx);
    for handle in [reader, worker, writer] {
        let _ = handle.join();
    }
    exit
}

fn spawn(name: &str, body: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .stack_size(THREAD_STACK_BYTES)
        .spawn(body)
        .expect("spawn server thread")
}

/// The thread driver: it moves coordinator outputs to the writer and worker channels and
/// feeds events back. It contains no protocol logic.
fn drive(
    coordinator: &mut Coordinator,
    ingress: &Receiver<ReaderEvent>,
    results: &Receiver<WorkerResult>,
    receipts: &Receiver<Receipt>,
    wake: &Receiver<()>,
    work_tx: &SyncSender<WorkerJob>,
    frame_tx: &SyncSender<Vec<u8>>,
) -> u8 {
    while coordinator.running {
        let mut progressed = false;
        while let Ok(Receipt) = receipts.try_recv() {
            coordinator.on_receipt();
            progressed = true;
        }
        while let Ok(result) = results.try_recv() {
            coordinator.on_worker_result(result);
            progressed = true;
        }
        if let Ok(event) = ingress.try_recv() {
            match event {
                ReaderEvent::Frame(body) => coordinator.on_frame(&body),
                ReaderEvent::Terminal => coordinator.on_terminal(),
            }
            progressed = true;
        }
        // Move coordinator outputs downstream without blocking on a full channel.
        if let Some(job) = coordinator.job_out.take()
            && work_tx.try_send(job).is_err()
        {
            coordinator.worker_busy = false;
            coordinator.pending_recompute = true;
        }
        while let Some(bytes) = coordinator.outbox.front() {
            match frame_tx.try_send(bytes.clone()) {
                Ok(()) => {
                    coordinator.outbox.pop_front();
                }
                Err(_) => break,
            }
        }
        if !coordinator.running {
            break;
        }
        if !progressed {
            let _ = wake.recv();
        }
    }
    coordinator.exit_code
}

/// What the reader hands the coordinator.
enum ReaderEvent {
    Frame(Vec<u8>),
    Terminal,
}

fn reader_loop(ingress: SyncSender<ReaderEvent>, wake: SyncSender<()>) {
    let stdin = std::io::stdin();
    let mut reader = crate::transport::FrameReader::new(BufReader::new(stdin.lock()));
    loop {
        let event = match reader.next_frame() {
            Ok(crate::transport::FrameEvent::Frame(body)) => ReaderEvent::Frame(body),
            Ok(crate::transport::FrameEvent::Eof) | Err(_) => {
                let _ = ingress.send(ReaderEvent::Terminal);
                let _ = wake.try_send(());
                return;
            }
        };
        if ingress.send(event).is_err() {
            return;
        }
        let _ = wake.try_send(());
    }
}

fn worker_loop(
    work: &Receiver<WorkerJob>,
    result: &SyncSender<WorkerResult>,
    wake: &SyncSender<()>,
) {
    while let Ok(job) = work.recv() {
        let overlay: Vec<OverlayInput<'_>> = job
            .overlay
            .iter()
            .map(|(key, bytes)| OverlayInput {
                key: key.as_str(),
                bytes: bytes.as_slice(),
            })
            .collect();
        let outcome = run_analysis(&job.root, &overlay, job.revision);
        if result.send(WorkerResult { outcome }).is_err() {
            return;
        }
        let _ = wake.try_send(());
    }
}

fn writer_loop(frames: &Receiver<Vec<u8>>, receipts: &SyncSender<Receipt>, wake: &SyncSender<()>) {
    let stdout = std::io::stdout();
    while let Ok(body) = frames.recv() {
        let mut handle = stdout.lock();
        if crate::transport::write_frame(&mut handle, &body).is_err() {
            return;
        }
        drop(handle);
        if receipts.send(Receipt).is_err() {
            return;
        }
        let _ = wake.try_send(());
    }
}

// ---- request ledger ----

/// The delivery state of one live request-ledger entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ReqState {
    /// Admitted and routed; no response frame handed off yet (a held query, or a
    /// request whose response is queued behind a credit).
    Live,
    /// A response frame was handed off; awaiting its delivery receipt.
    AwaitingDelivery,
}

/// The terminal classification of a request that never delivered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TerminalClass {
    /// A terminal event before the response frame was handed off.
    AbandonedByTerminal,
    /// A terminal event after handoff but before the matching delivered receipt.
    DeliveryUnknown,
}

/// Which owner a handed-off outbound frame belongs to, so its delivery receipt retires
/// the right ledger entry and drives lifecycle/publication bookkeeping.
#[derive(Clone, Debug)]
enum FrameOwner {
    Request(RequestId),
    Anonymous,
    Publication,
    Initialize,
    Shutdown,
    ShowMessage,
}

/// The shared live-entry budget: ordinary requests and known-id error-only entries.
struct RequestLedger {
    entries: std::collections::HashMap<RequestId, ReqState>,
    capacity: usize,
}

impl RequestLedger {
    fn new(capacity: usize) -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            capacity,
        }
    }

    fn is_live(&self, id: &RequestId) -> bool {
        self.entries.contains_key(id)
    }

    /// Reserve one entry for a unique id, or `None` when the budget is exhausted.
    fn reserve(&mut self, id: RequestId) -> Option<()> {
        if self.entries.len() >= self.capacity {
            return None;
        }
        self.entries.insert(id, ReqState::Live);
        Some(())
    }

    fn set_awaiting(&mut self, id: &RequestId) {
        if let Some(state) = self.entries.get_mut(id) {
            *state = ReqState::AwaitingDelivery;
        }
    }

    fn retire(&mut self, id: &RequestId) {
        self.entries.remove(id);
    }
}

// ---- held queries ----

/// A semantic request held until the analysis snapshot for its revision is ready. It is
/// bound to the admission-time revision and document version, and reauthorized before its
/// success is encoded: a changed revision or document state replaces the success with
/// `-32801 ContentModified`.
struct HeldQuery {
    id: RequestId,
    method: String,
    params: Option<Box<serde_json::value::RawValue>>,
    revision: InputRevision,
    key: DocumentKey,
    version: i32,
}

// ---- capture episode ----

/// The background capture-failure episode latch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CaptureEpisode {
    /// No background failure is latched; the next exact-current failure may notify.
    Eligible,
    /// A background failure is latched; later failures are suppressed until a later
    /// exact-current successful set resets it.
    Latched {
        episode: u64,
        failed_revision: InputRevision,
    },
}

// ---- publication ----

/// The in-flight diagnostic publication set holding the exclusive plan credit until every
/// frame delivers. Records the capture-episode latch it observed at commit, so only a set
/// that observed the still-current latch resets it.
struct PublicationState {
    credit: PublicationPlanCredit,
    frames_remaining: usize,
    observed_episode: Option<u64>,
}

/// The process-main coordinator.
struct Coordinator {
    lifecycle: Lifecycle,
    root: Option<SelectedRoot>,
    ledger: DocumentLedger,
    revisions: RevisionCounter,
    current_revision: InputRevision,
    snapshot: Option<Arc<AnalysisSnapshot>>,
    snapshot_revision: Option<InputRevision>,
    published: Vec<DocumentKey>,

    requests: RequestLedger,
    anonymous_slots: usize,
    anonymous_capacity: usize,
    held_queries: Vec<HeldQuery>,

    outbound_credits: CreditPool<OutboundCredit>,
    /// Handed-off frames awaiting a delivery receipt, each carrying the affine outbound
    /// credit it consumed. FIFO: the front is the oldest, matching the single writer.
    in_flight: VecDeque<(FrameOwner, OutboundCredit)>,
    /// Frames waiting for an outbound credit, in order. They hold no credit yet.
    pending_frames: VecDeque<(Vec<u8>, FrameOwner)>,

    worker_busy: bool,
    pending_recompute: bool,

    episode: CaptureEpisode,
    next_episode: u64,

    publication_credits: CreditPool<PublicationPlanCredit>,
    publication: Option<PublicationState>,
    pending_publication: Option<Arc<AnalysisSnapshot>>,

    /// Outbound frame bytes for the driver to write, in order.
    outbox: VecDeque<Vec<u8>>,
    /// At most one analysis job for the driver to dispatch.
    job_out: Option<WorkerJob>,

    /// Terminal classifications, recorded for tests and for delivery accounting.
    terminal_classes: Vec<(RequestId, TerminalClass)>,

    exit_code: u8,
    running: bool,
}

impl Coordinator {
    fn new() -> Self {
        Self::with_capacities(MAX_LIVE_REQUEST_ENTRIES, MAX_ANONYMOUS_ERROR_SLOTS)
    }

    /// Construct with explicit ledger capacities. Production uses the frozen bounds;
    /// tests drive the N/N+1 overflow reds with small capacities.
    fn with_capacities(request_capacity: usize, anonymous_capacity: usize) -> Self {
        let (revisions, current_revision) = RevisionCounter::initial();
        Self {
            lifecycle: Lifecycle::new(),
            root: None,
            ledger: DocumentLedger::new(),
            revisions,
            current_revision,
            snapshot: None,
            snapshot_revision: None,
            published: Vec::new(),
            requests: RequestLedger::new(request_capacity),
            anonymous_slots: 0,
            anonymous_capacity,
            held_queries: Vec::new(),
            outbound_credits: CreditPool::outbound(),
            in_flight: VecDeque::new(),
            pending_frames: VecDeque::new(),
            worker_busy: false,
            pending_recompute: false,
            episode: CaptureEpisode::Eligible,
            next_episode: 0,
            publication_credits: CreditPool::publication(),
            publication: None,
            pending_publication: None,
            outbox: VecDeque::new(),
            job_out: None,
            terminal_classes: Vec::new(),
            exit_code: 1,
            running: true,
        }
    }

    // ---- inbound ----

    fn on_frame(&mut self, body: &[u8]) {
        match decode(body) {
            Inbound::Request { id, method, params } => self.on_request(id, &method, params),
            Inbound::Notification { method, params } => self.on_notification(&method, params),
            Inbound::UnsolicitedResponse => {}
            Inbound::Reject(reject) => self.on_reject(reject),
        }
    }

    fn on_reject(&mut self, reject: Reject) {
        match reject {
            Reject::ParseError => self.send_null_error(PARSE_ERROR, "parse error"),
            Reject::InvalidRequest {
                recovered_id,
                reason,
            } => {
                let message = match reason {
                    InvalidReason::NoBatch => "batch requests are not supported",
                    InvalidReason::Structural => "invalid request",
                };
                match recovered_id {
                    // A recovered-id invalid request reserves a known-id error-only entry
                    // before its frame is handed off; a collision with a live entry uses
                    // the anonymous slot instead.
                    Some(id) if self.requests.is_live(&id) => {
                        self.send_null_error(INVALID_REQUEST, message)
                    }
                    Some(id) => self.reserve_and_error(id, INVALID_REQUEST, message),
                    None => self.send_null_error(INVALID_REQUEST, message),
                }
            }
        }
    }

    fn on_request(
        &mut self,
        id: RequestId,
        method: &str,
        params: Option<Box<serde_json::value::RawValue>>,
    ) {
        // Duplicate-live classification precedes reservation and consumes no new entry.
        if self.requests.is_live(&id) {
            self.send_null_error(INVALID_REQUEST, "duplicate request id");
            return;
        }
        // Reserve the shared live-entry budget before any lifecycle or method routing.
        if self.requests.reserve(id.clone()).is_none() {
            // Exhaustion is a fixed terminal: no response, no state mutation.
            self.terminate(1);
            return;
        }
        match method {
            "initialize" => self.on_initialize(id, params),
            "shutdown" => self.on_shutdown(id),
            "textDocument/hover" | "textDocument/definition" | "textDocument/formatting" => {
                self.on_semantic_request(id, method, params)
            }
            _ => match self.lifecycle.gate_request() {
                RequestGate::NotInitialized => {
                    self.answer_error(&id, SERVER_NOT_INITIALIZED, "server not initialized")
                }
                RequestGate::InvalidInPhase => {
                    self.answer_error(&id, INVALID_REQUEST, "invalid request in current state")
                }
                RequestGate::Route => self.answer_error(&id, METHOD_NOT_FOUND, "method not found"),
            },
        }
    }

    fn on_initialize(&mut self, id: RequestId, params: Option<Box<serde_json::value::RawValue>>) {
        if self.lifecycle.on_initialize() != RequestGate::Route {
            self.answer_error(&id, INVALID_REQUEST, "initialize already handled");
            return;
        }
        let root = match params.as_deref().and_then(parse::<InitializeParams>) {
            Some(params) => match select_root(&params) {
                Ok(root) => root,
                Err(_) => {
                    self.lifecycle = restore_after_rejected_initialize();
                    self.answer_error(&id, INVALID_PARAMS, "malformed workspace root");
                    return;
                }
            },
            None => {
                self.lifecycle = restore_after_rejected_initialize();
                self.answer_error(&id, INVALID_PARAMS, "malformed initialize params");
                return;
            }
        };
        self.root = root;
        // Receipt-gated delivery: hand off the response but do not advance the lifecycle
        // until the delivery receipt for this frame arrives.
        self.requests.retire(&id);
        self.send(
            Outbound::Initialize {
                id,
                result: Box::new(initialize_result()),
            },
            FrameOwner::Initialize,
        );
    }

    fn on_shutdown(&mut self, id: RequestId) {
        match self.lifecycle.on_shutdown() {
            RequestGate::Route => {
                self.requests.retire(&id);
                self.send(Outbound::Null { id }, FrameOwner::Shutdown);
            }
            RequestGate::NotInitialized => {
                self.answer_error(&id, SERVER_NOT_INITIALIZED, "server not initialized")
            }
            RequestGate::InvalidInPhase => {
                self.answer_error(&id, INVALID_REQUEST, "invalid request in current state")
            }
        }
    }

    fn on_semantic_request(
        &mut self,
        id: RequestId,
        method: &str,
        params: Option<Box<serde_json::value::RawValue>>,
    ) {
        if self.lifecycle.gate_request() != RequestGate::Route {
            self.answer_error(&id, SERVER_NOT_INITIALIZED, "server not initialized");
            return;
        }
        if self.root.is_none() {
            self.answer_error(&id, INVALID_PARAMS, "no selected root");
            return;
        }
        // An unavailable project makes every semantic request the same fixed -32803.
        if !self.ledger.all_available() {
            self.answer_error(&id, REQUEST_FAILED, "project capture unavailable");
            return;
        }
        // Resolve the target document version for the revision binding.
        let Some(root) = self.root.clone() else {
            self.answer_error(&id, INVALID_PARAMS, "no selected root");
            return;
        };
        let Some(key) = self.request_document_key(&root, method, params.as_deref()) else {
            self.answer_error(&id, INVALID_PARAMS, "malformed params");
            return;
        };
        let Some(DocumentState::OpenText { version, .. }) = self.ledger.get(&key) else {
            self.answer_error(&id, CONTENT_MODIFIED, "content modified");
            return;
        };
        let held = HeldQuery {
            id,
            method: method.to_owned(),
            params,
            revision: self.current_revision,
            key,
            version: *version,
        };
        // Answer now against a ready exact-current snapshot, else hold until analysis.
        if self.snapshot_revision == Some(self.current_revision) {
            self.answer_held(held);
        } else {
            self.held_queries.push(held);
        }
    }

    /// The document key a semantic request targets, for revision binding.
    fn request_document_key(
        &self,
        root: &SelectedRoot,
        method: &str,
        params: Option<&serde_json::value::RawValue>,
    ) -> Option<DocumentKey> {
        let uri = match method {
            "textDocument/hover" => {
                parse::<HoverParams>(params?)?
                    .text_document_position_params
                    .text_document
                    .uri
            }
            "textDocument/definition" => {
                parse::<GotoDefinitionParams>(params?)?
                    .text_document_position_params
                    .text_document
                    .uri
            }
            "textDocument/formatting" => {
                parse::<DocumentFormattingParams>(params?)?
                    .text_document
                    .uri
            }
            _ => return None,
        };
        DocumentKey::from_uri(uri.as_str(), root).ok()
    }

    /// Answer a held query, reauthorizing its revision and document version. A changed
    /// revision or document state is `-32801 ContentModified`; the success encoder is
    /// never invoked in that case.
    fn answer_held(&mut self, held: HeldQuery) {
        let current = self.current_revision;
        let doc_ok = matches!(
            self.ledger.get(&held.key),
            Some(DocumentState::OpenText { version, .. }) if *version == held.version
        );
        if held.revision != current || !doc_ok {
            self.answer_error(&held.id, CONTENT_MODIFIED, "content modified");
            return;
        }
        let Some(root) = self.root.clone() else {
            self.answer_error(&held.id, INTERNAL_ERROR, "internal error");
            return;
        };
        let Some(snapshot) = self.snapshot.clone() else {
            self.answer_error(&held.id, REQUEST_FAILED, "analysis not ready");
            return;
        };
        match self.answer_semantic(&snapshot, &root, &held.method, held.params.as_deref()) {
            SemanticAnswer::Reply(body) => {
                let id = held.id.clone();
                self.requests.retire(&id);
                self.send(body.into_outbound(id.clone()), FrameOwner::Request(id));
            }
            SemanticAnswer::ContentModified => {
                self.answer_error(&held.id, CONTENT_MODIFIED, "content modified")
            }
            SemanticAnswer::BadParams => {
                self.answer_error(&held.id, INVALID_PARAMS, "malformed params")
            }
            SemanticAnswer::Internal => {
                self.answer_error(&held.id, INTERNAL_ERROR, "internal error")
            }
        }
    }

    fn answer_semantic(
        &self,
        snapshot: &AnalysisSnapshot,
        root: &SelectedRoot,
        method: &str,
        params: Option<&serde_json::value::RawValue>,
    ) -> SemanticAnswer {
        match method {
            "textDocument/hover" => {
                let Some(params) = params.and_then(parse::<HoverParams>) else {
                    return SemanticAnswer::BadParams;
                };
                let position = params.text_document_position_params;
                let Some((identity, source)) =
                    self.resolve_document(root, position.text_document.uri.as_str())
                else {
                    return SemanticAnswer::ContentModified;
                };
                SemanticAnswer::Reply(OutboundBody::Hover(facts::hover(
                    snapshot,
                    &identity,
                    &source,
                    position.position,
                )))
            }
            "textDocument/definition" => {
                let Some(params) = params.and_then(parse::<GotoDefinitionParams>) else {
                    return SemanticAnswer::BadParams;
                };
                let position = params.text_document_position_params;
                let Some((identity, source)) =
                    self.resolve_document(root, position.text_document.uri.as_str())
                else {
                    return SemanticAnswer::ContentModified;
                };
                let source_lookup = |file: &marrow_project_fs::FileIdentity| self.file_source(file);
                match facts::definition(
                    snapshot,
                    root,
                    &identity,
                    &source,
                    source_lookup,
                    position.position,
                ) {
                    Ok(location) => SemanticAnswer::Reply(OutboundBody::Definition(location)),
                    Err(_) => SemanticAnswer::Internal,
                }
            }
            "textDocument/formatting" => {
                let Some(params) = params.and_then(parse::<DocumentFormattingParams>) else {
                    return SemanticAnswer::BadParams;
                };
                let Some((identity, source)) =
                    self.resolve_document(root, params.text_document.uri.as_str())
                else {
                    return SemanticAnswer::ContentModified;
                };
                SemanticAnswer::Reply(OutboundBody::Formatting(facts::formatting(
                    snapshot, &identity, &source,
                )))
            }
            _ => SemanticAnswer::Internal,
        }
    }

    fn resolve_document(
        &self,
        root: &SelectedRoot,
        uri: &str,
    ) -> Option<(marrow_project_fs::FileIdentity, String)> {
        let (key, source) = self.resolve_open_document(root, uri)?;
        let (identity, _) = marrow_project_fs::FileIdentity::validate(key.relative()).ok()?;
        Some((identity, source))
    }

    fn resolve_open_document(
        &self,
        root: &SelectedRoot,
        uri: &str,
    ) -> Option<(DocumentKey, String)> {
        let key = DocumentKey::from_uri(uri, root).ok()?;
        match self.ledger.get(&key) {
            Some(DocumentState::OpenText { text, .. }) => Some((key, text.clone())),
            _ => None,
        }
    }

    fn file_source(&self, file: &marrow_project_fs::FileIdentity) -> Option<String> {
        let key = DocumentKey::from_identity(file);
        self.ledger
            .text_entries()
            .find(|(open_key, _)| **open_key == key)
            .map(|(_, text)| text.to_owned())
    }

    fn on_notification(&mut self, method: &str, params: Option<Box<serde_json::value::RawValue>>) {
        match method {
            "initialized" => {
                if self.lifecycle.on_initialized() {
                    self.enter_running();
                }
            }
            "exit" => {
                self.exit_code = self.lifecycle.on_exit();
                self.running = false;
            }
            "textDocument/didOpen" => self.on_did_open(params),
            "textDocument/didChange" => self.on_did_change(params),
            "textDocument/didClose" => self.on_did_close(params),
            // `$/cancelRequest` and every unknown notification are discarded.
            _ => {}
        }
    }

    fn on_did_open(&mut self, params: Option<Box<serde_json::value::RawValue>>) {
        if self.lifecycle.phase() != Phase::Running {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(params) = params
            .as_deref()
            .and_then(parse::<DidOpenTextDocumentParams>)
        else {
            return;
        };
        let document = params.text_document;
        let Ok(key) = DocumentKey::from_uri(document.uri.as_str(), &root) else {
            return;
        };
        if self.ledger.validate_open(&key).is_err() {
            return;
        }
        let Ok(revision) = self.revisions.advance() else {
            self.terminate(1);
            return;
        };
        self.current_revision = revision;
        self.ledger.insert(
            key,
            DocumentState::OpenText {
                version: document.version,
                text: document.text,
            },
        );
        self.maybe_recompute(&root);
    }

    fn on_did_change(&mut self, params: Option<Box<serde_json::value::RawValue>>) {
        if self.lifecycle.phase() != Phase::Running {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(params) = params
            .as_deref()
            .and_then(parse::<DidChangeTextDocumentParams>)
        else {
            return;
        };
        let version = params.text_document.version;
        let Ok(key) = DocumentKey::from_uri(params.text_document.uri.as_str(), &root) else {
            return;
        };
        if self.ledger.validate_change(&key, version).is_err() {
            return;
        }
        let Some(change) = params.content_changes.into_iter().next() else {
            return;
        };
        if change.range.is_some() {
            return;
        }
        let Ok(revision) = self.revisions.advance() else {
            self.terminate(1);
            return;
        };
        self.current_revision = revision;
        self.ledger.replace(
            &key,
            DocumentState::OpenText {
                version,
                text: change.text,
            },
        );
        self.maybe_recompute(&root);
    }

    fn on_did_close(&mut self, params: Option<Box<serde_json::value::RawValue>>) {
        if self.lifecycle.phase() != Phase::Running {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(params) = params
            .as_deref()
            .and_then(parse::<DidCloseTextDocumentParams>)
        else {
            return;
        };
        let Ok(key) = DocumentKey::from_uri(params.text_document.uri.as_str(), &root) else {
            return;
        };
        if self.ledger.validate_close(&key).is_err() {
            return;
        }
        let Ok(revision) = self.revisions.advance() else {
            self.terminate(1);
            return;
        };
        self.current_revision = revision;
        self.ledger.remove(&key);
        self.maybe_recompute(&root);
    }

    fn enter_running(&mut self) {
        if let Some(root) = self.root.clone() {
            self.maybe_recompute(&root);
        }
    }

    fn maybe_recompute(&mut self, root: &SelectedRoot) {
        if self.worker_busy {
            self.pending_recompute = true;
            return;
        }
        self.dispatch_recompute(root);
    }

    fn dispatch_recompute(&mut self, root: &SelectedRoot) {
        let overlay: Vec<(String, Vec<u8>)> = self
            .ledger
            .text_entries()
            .map(|(key, text)| (key.relative().to_owned(), text.as_bytes().to_vec()))
            .collect();
        self.job_out = Some(WorkerJob {
            root: root.clone(),
            revision: self.current_revision,
            overlay,
        });
        self.worker_busy = true;
        self.pending_recompute = false;
    }

    // ---- worker results ----

    fn on_worker_result(&mut self, result: WorkerResult) {
        self.worker_busy = false;
        match result.outcome {
            AnalysisOutcome::Snapshot(snapshot) => {
                if snapshot.revision() == self.current_revision {
                    self.snapshot = Some(snapshot.clone());
                    self.snapshot_revision = Some(snapshot.revision());
                    self.begin_publication(snapshot);
                    self.drain_held_queries();
                }
            }
            AnalysisOutcome::Capture(rejection) => {
                if rejection.revision == self.current_revision {
                    self.on_background_capture_failure(rejection);
                }
            }
            AnalysisOutcome::ResourceLimit { .. } => {
                // Recoverable: publishes and clears nothing.
            }
            AnalysisOutcome::Invariant { .. } => {
                self.terminate(1);
                return;
            }
        }
        if self.pending_recompute
            && let Some(root) = self.root.clone()
        {
            self.dispatch_recompute(&root);
        }
    }

    fn drain_held_queries(&mut self) {
        let held: Vec<HeldQuery> = std::mem::take(&mut self.held_queries);
        for query in held {
            self.answer_held(query);
        }
    }

    // ---- capture episode ----

    fn on_background_capture_failure(&mut self, rejection: CaptureRejection) {
        // A background capture failure publishes and clears no diagnostics. It notifies at
        // most once per episode: the first exact-current failure latches and shows a
        // message; later failures are suppressed while latched.
        match self.episode {
            CaptureEpisode::Latched { .. } => {}
            CaptureEpisode::Eligible => {
                let episode = self.next_episode;
                self.next_episode = self.next_episode.saturating_add(1);
                self.episode = CaptureEpisode::Latched {
                    episode,
                    failed_revision: rejection.revision,
                };
                if let Some(evidence) = rejection.evidence {
                    self.send(show_message(&evidence), FrameOwner::ShowMessage);
                }
            }
        }
    }

    // ---- publication (exclusive) ----

    fn begin_publication(&mut self, snapshot: Arc<AnalysisSnapshot>) {
        // Publication exclusivity: only one plan builds/commits at a time. A newer result
        // while a plan is in flight waits (latest-wins) and derives its tombstones only
        // after the prior final receipt.
        if self.publication.is_some() {
            self.pending_publication = Some(snapshot);
            return;
        }
        let Some(credit) = self.publication_credits.acquire() else {
            self.pending_publication = Some(snapshot);
            return;
        };
        let observed_episode = match self.episode {
            CaptureEpisode::Latched { episode, .. } => Some(episode),
            CaptureEpisode::Eligible => None,
        };
        let frames = self.build_publication_frames(&snapshot);
        let frames_remaining = frames.len();
        if frames_remaining == 0 {
            // A zero-frame commit resets the episode immediately (no frame to await).
            self.publication_credits.release(credit);
            self.reset_episode_if_observed(observed_episode);
            return;
        }
        self.publication = Some(PublicationState {
            credit,
            frames_remaining,
            observed_episode,
        });
        for outbound in frames {
            self.send(outbound, FrameOwner::Publication);
        }
    }

    /// Build the complete publication set: every current file's diagnostic list (including
    /// empties) plus an empty tombstone for every previously published file absent from the
    /// snapshot. The delivered-ledger update commits here (before frames deliver), matching
    /// the coordinator's single-writer FIFO ordering.
    fn build_publication_frames(&mut self, snapshot: &AnalysisSnapshot) -> Vec<Outbound> {
        let Some(root) = self.root.clone() else {
            return Vec::new();
        };
        let mut frames = Vec::new();
        let mut new_published = Vec::new();
        for module in snapshot.input().modules() {
            let identity = module.identity();
            let key = DocumentKey::from_identity(identity);
            let source = std::str::from_utf8(module.source()).unwrap_or("");
            let version = self.ledger.get(&key).map(DocumentState::version);
            if let Ok(params) =
                facts::diagnostics_for_file(snapshot, &root, identity, source, version)
            {
                let has = !params.diagnostics.is_empty();
                frames.push(Outbound::PublishDiagnostics(Box::new(params)));
                if has {
                    new_published.push(key);
                }
            }
        }
        let snapshot_keys: Vec<DocumentKey> = snapshot
            .input()
            .modules()
            .iter()
            .map(|module| DocumentKey::from_identity(module.identity()))
            .collect();
        let previously = std::mem::take(&mut self.published);
        for key in &previously {
            if !snapshot_keys.contains(key)
                && let Ok((identity, _)) = marrow_project_fs::FileIdentity::validate(key.relative())
                && let Some(uri) = lsp_uri(&root, &identity)
            {
                frames.push(Outbound::PublishDiagnostics(Box::new(
                    lsp_types::PublishDiagnosticsParams {
                        uri,
                        diagnostics: Vec::new(),
                        version: None,
                    },
                )));
            }
        }
        self.published = new_published;
        frames
    }

    fn on_publication_receipt(&mut self) {
        let Some(mut state) = self.publication.take() else {
            return;
        };
        state.frames_remaining = state.frames_remaining.saturating_sub(1);
        if state.frames_remaining == 0 {
            // The whole committed set is delivered: release the exclusive credit and reset
            // the episode only if the commit observed the still-current latch.
            let observed = state.observed_episode;
            self.publication_credits.release(state.credit);
            self.reset_episode_if_observed(observed);
            // A newer result that waited may now build its plan and derive tombstones from
            // the final ledger.
            if let Some(snapshot) = self.pending_publication.take() {
                self.begin_publication(snapshot);
            }
        } else {
            self.publication = Some(state);
        }
    }

    fn reset_episode_if_observed(&mut self, observed: Option<u64>) {
        if let CaptureEpisode::Latched { episode, .. } = self.episode
            && observed == Some(episode)
        {
            self.episode = CaptureEpisode::Eligible;
        }
    }

    // ---- outbound plumbing ----

    fn answer_error(&mut self, id: &RequestId, code: i32, message: &str) {
        // A request whose response is a known-id error retires its own ledger entry on
        // delivery, so keep the entry live through handoff.
        self.send(
            Outbound::Error {
                id: Some(id.clone()),
                code,
                message: message.to_owned(),
            },
            FrameOwner::Request(id.clone()),
        );
    }

    fn reserve_and_error(&mut self, id: RequestId, code: i32, message: &str) {
        if self.requests.reserve(id.clone()).is_none() {
            self.terminate(1);
            return;
        }
        self.answer_error(&id, code, message);
    }

    fn send_null_error(&mut self, code: i32, message: &str) {
        if self.anonymous_slots >= self.anonymous_capacity {
            // Anonymous-slot exhaustion is the same zero-response terminal outcome.
            self.terminate(1);
            return;
        }
        self.anonymous_slots += 1;
        self.send(
            Outbound::Error {
                id: None,
                code,
                message: message.to_owned(),
            },
            FrameOwner::Anonymous,
        );
    }

    /// Encode and hand off one frame, acquiring an outbound credit. A request-owned frame
    /// moves its ledger entry to `AwaitingDelivery`. When no credit is available the frame
    /// queues, still credited on acquisition, so `W` bounds total outstanding frames.
    fn send(&mut self, outbound: Outbound, owner: FrameOwner) {
        let bytes = match encode(&outbound) {
            Ok(bytes) => bytes,
            Err(_) => {
                // A pre-handoff encoding failure emits zero bytes; the ledger entry stays
                // live and is classified at terminal.
                return;
            }
        };
        if let FrameOwner::Request(id) = &owner {
            self.requests.set_awaiting(id);
        }
        match self.outbound_credits.acquire() {
            Some(credit) => {
                self.outbox.push_back(bytes);
                self.in_flight.push_back((owner, credit));
            }
            None => self.pending_frames.push_back((bytes, owner)),
        }
    }

    fn on_receipt(&mut self) {
        let Some((owner, credit)) = self.in_flight.pop_front() else {
            return;
        };
        // Return the affine credit for the delivered frame.
        self.outbound_credits.release(credit);
        match owner {
            FrameOwner::Request(id) => self.requests.retire(&id),
            FrameOwner::Anonymous => self.anonymous_slots = self.anonymous_slots.saturating_sub(1),
            FrameOwner::Publication => self.on_publication_receipt(),
            FrameOwner::Initialize => {
                if self.lifecycle.on_initialize_delivered() {
                    self.enter_running();
                }
            }
            FrameOwner::Shutdown => self.lifecycle.on_shutdown_delivered(),
            FrameOwner::ShowMessage => {}
        }
        // A freed credit lets a queued frame proceed.
        if let Some((bytes, owner)) = self.pending_frames.pop_front() {
            match self.outbound_credits.acquire() {
                Some(credit) => {
                    self.outbox.push_back(bytes);
                    self.in_flight.push_back((owner, credit));
                }
                None => self.pending_frames.push_front((bytes, owner)),
            }
        }
    }

    // ---- terminal ----

    fn on_terminal(&mut self) {
        // First-wins terminal. Classify every unretired request: a request whose frame was
        // handed off (in flight or queued behind a credit) is DeliveryUnknown; a request
        // with no handed-off frame (a held query, or a still-Live entry) is
        // AbandonedByTerminal.
        let mut awaiting: Vec<RequestId> = Vec::new();
        for (owner, _credit) in &self.in_flight {
            if let FrameOwner::Request(id) = owner {
                awaiting.push(id.clone());
            }
        }
        for (bytes, owner) in &self.pending_frames {
            let _ = bytes;
            if let FrameOwner::Request(id) = owner {
                awaiting.push(id.clone());
            }
        }
        for id in &awaiting {
            self.terminal_classes
                .push((id.clone(), TerminalClass::DeliveryUnknown));
            self.requests.retire(id);
        }
        for query in std::mem::take(&mut self.held_queries) {
            self.terminal_classes
                .push((query.id.clone(), TerminalClass::AbandonedByTerminal));
            self.requests.retire(&query.id);
        }
        // Any remaining live entries (reserved, no frame) are abandoned.
        let remaining: Vec<RequestId> = self.requests.entries.keys().cloned().collect();
        for id in remaining {
            self.terminal_classes
                .push((id.clone(), TerminalClass::AbandonedByTerminal));
            self.requests.retire(&id);
        }
        self.exit_code = self.lifecycle.on_terminal();
        self.running = false;
    }

    fn terminate(&mut self, code: u8) {
        self.exit_code = code;
        self.running = false;
    }
}

/// The body of a semantic reply before it is bound to an id.
enum OutboundBody {
    Hover(Option<lsp_types::Hover>),
    Definition(Option<lsp_types::Location>),
    Formatting(Option<Vec<lsp_types::TextEdit>>),
}

impl OutboundBody {
    fn into_outbound(self, id: RequestId) -> Outbound {
        match self {
            OutboundBody::Hover(result) => Outbound::Hover { id, result },
            OutboundBody::Definition(result) => Outbound::Definition { id, result },
            OutboundBody::Formatting(result) => Outbound::Formatting { id, result },
        }
    }
}

enum SemanticAnswer {
    Reply(OutboundBody),
    ContentModified,
    BadParams,
    Internal,
}

fn show_message(evidence: &UnavailableEvidence) -> Outbound {
    // The exact `<marrow-code>: <operational-message>` body, composed without a rendering
    // macro so the message is only the code and the facade-written text.
    let mut message = String::with_capacity(evidence.code.len() + 2 + evidence.message.len());
    message.push_str(evidence.code);
    message.push_str(": ");
    message.push_str(&evidence.message);
    Outbound::ShowMessage {
        typ: MessageType::Error,
        message,
    }
}

enum RootError {
    TooMany,
    Malformed,
}

fn select_root(params: &InitializeParams) -> Result<Option<SelectedRoot>, RootError> {
    if let Some(folders) = &params.workspace_folders {
        match folders.as_slice() {
            [] => {}
            [folder] => {
                return SelectedRoot::from_uri(folder.uri.as_str())
                    .map(Some)
                    .map_err(uri_to_root_error);
            }
            _ => return Err(RootError::TooMany),
        }
    }
    #[allow(deprecated)]
    match &params.root_uri {
        Some(uri) => SelectedRoot::from_uri(uri.as_str())
            .map(Some)
            .map_err(uri_to_root_error),
        None => Ok(None),
    }
}

fn uri_to_root_error(_: UriError) -> RootError {
    RootError::Malformed
}

/// A rejected initialize leaves the lifecycle in its initial `AwaitInitialize` phase.
fn restore_after_rejected_initialize() -> Lifecycle {
    Lifecycle::new()
}

fn lsp_uri(
    root: &SelectedRoot,
    identity: &marrow_project_fs::FileIdentity,
) -> Option<lsp_types::Uri> {
    use std::str::FromStr;
    lsp_types::Uri::from_str(&crate::uri::diagnostic_uri(root, identity)).ok()
}

fn parse<T: serde::de::DeserializeOwned>(raw: &serde_json::value::RawValue) -> Option<T> {
    serde_json::from_str(raw.get()).ok()
}

fn initialize_result() -> InitializeResult {
    InitializeResult {
        capabilities: ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Options(
                TextDocumentSyncOptions {
                    open_close: Some(true),
                    change: Some(TextDocumentSyncKind::FULL),
                    ..Default::default()
                },
            )),
            hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
            definition_provider: Some(OneOf::Left(true)),
            document_formatting_provider: Some(OneOf::Left(true)),
            ..Default::default()
        },
        server_info: Some(ServerInfo {
            name: "marrow-lsp".to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    // ---- test scaffolding: drive the pure coordinator with deterministic events ----

    fn temp_project(tag: &str, main: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "marrow-lsp-server-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(base.join("src")).unwrap();
        fs::write(base.join("marrow.toml"), "edition = \"2026\"\n").unwrap();
        fs::write(base.join("src/main.mw"), main).unwrap();
        base
    }

    fn root_uri(dir: &Path) -> String {
        let mut uri = String::from("file://");
        for component in dir.components() {
            if let std::path::Component::Normal(part) = component {
                uri.push('/');
                uri.push_str(part.to_str().unwrap());
            }
        }
        uri
    }

    fn selected_root(dir: &Path) -> SelectedRoot {
        SelectedRoot::from_uri(&root_uri(dir)).unwrap()
    }

    fn initialize_body(root: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"processId":null,"rootUri":"{root}","capabilities":{{}}}}}}"#
        )
    }

    fn snapshot_at(dir: &Path, main: &str, revision: InputRevision) -> Arc<AnalysisSnapshot> {
        let root = selected_root(dir);
        let overlay = vec![OverlayInput {
            key: "src/main.mw",
            bytes: main.as_bytes(),
        }];
        match run_analysis(&root, &overlay, revision) {
            AnalysisOutcome::Snapshot(snapshot) => snapshot,
            _ => panic!("expected snapshot"),
        }
    }

    /// The outbox frames as UTF-8 strings, for wire-level assertions.
    fn frames(coordinator: &Coordinator) -> Vec<String> {
        coordinator
            .outbox
            .iter()
            .map(|bytes| String::from_utf8(bytes.clone()).unwrap())
            .collect()
    }

    /// Drive a coordinator to `Running` with a selected root, delivering the initialize
    /// response receipt (receipt-gated) and the `initialized` notification.
    fn running(dir: &Path) -> Coordinator {
        let mut coordinator = Coordinator::new();
        coordinator.on_frame(initialize_body(&root_uri(dir)).as_bytes());
        coordinator.on_receipt(); // deliver the initialize response
        coordinator.on_frame(br#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
        coordinator
    }

    fn open_body(dir: &Path, version: i64, text: &str) -> String {
        let escaped = text
            .replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('"', "\\\"");
        format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{}/src/main.mw","languageId":"marrow","version":{version},"text":"{escaped}"}}}}}}"#,
            root_uri(dir)
        )
    }

    fn change_body(dir: &Path, version: i64, text: &str) -> String {
        let escaped = text
            .replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('"', "\\\"");
        format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didChange","params":{{"textDocument":{{"uri":"{}/src/main.mw","version":{version}}},"contentChanges":[{{"text":"{escaped}"}}]}}}}"#,
            root_uri(dir)
        )
    }

    fn hover_body(dir: &Path, id: i64, line: u32, character: u32) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/hover","params":{{"textDocument":{{"uri":"{}/src/main.mw"}},"position":{{"line":{line},"character":{character}}}}}}}"#,
            root_uri(dir)
        )
    }

    fn cleanup(dir: &Path) {
        fs::remove_dir_all(dir).ok();
    }

    // ---- Law: receipt-gated initialize delivery ----

    #[test]
    fn initialize_response_delivery_gates_lifecycle_and_first_analysis() {
        let dir = temp_project("init", "module main\n");
        let mut coordinator = Coordinator::new();
        coordinator.on_frame(initialize_body(&root_uri(dir.as_path())).as_bytes());
        // The response is handed off, but the lifecycle has NOT advanced and no analysis
        // job is enqueued yet.
        assert!(matches!(
            coordinator.lifecycle.phase(),
            Phase::InitializeReplyPending { .. }
        ));
        assert!(coordinator.job_out.is_none(), "no analysis before delivery");
        assert_eq!(frames(&coordinator).len(), 1, "one initialize response");

        // An `initialized` before delivery latches but cannot advance.
        coordinator.on_frame(br#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
        assert_eq!(
            coordinator.lifecycle.phase(),
            Phase::InitializeReplyPending {
                initialized_latched: true
            }
        );
        assert!(coordinator.job_out.is_none());

        // Delivering the initialize response advances to Running and enqueues exactly one
        // analysis job.
        coordinator.on_receipt();
        assert_eq!(coordinator.lifecycle.phase(), Phase::Running);
        assert!(
            coordinator.job_out.is_some(),
            "first analysis enqueued on delivery"
        );
        cleanup(&dir);
    }

    // ---- Law: shared live-entry budget and IngressOverload N/N+1 ----

    #[test]
    fn live_entry_budget_admits_n_and_overloads_n_plus_1() {
        // Capacity two: two distinct requests reserve; a third fails closed with a fixed
        // terminal and no response.
        let mut coordinator = Coordinator::with_capacities(2, 8);
        coordinator.on_frame(br#"{"jsonrpc":"2.0","id":1,"method":"noSuchMethod"}"#);
        coordinator.on_frame(br#"{"jsonrpc":"2.0","id":2,"method":"noSuchMethod"}"#);
        assert_eq!(coordinator.requests.entries.len(), 2);
        assert!(coordinator.running, "still viable at N");
        let frames_before = coordinator.outbox.len();

        coordinator.on_frame(br#"{"jsonrpc":"2.0","id":3,"method":"noSuchMethod"}"#);
        assert!(!coordinator.running, "IngressOverload fail-stops at N+1");
        assert_eq!(
            coordinator.outbox.len(),
            frames_before,
            "the overloaded request emits no response"
        );
    }

    #[test]
    fn duplicate_live_id_consumes_no_entry_and_gets_null_error() {
        let mut coordinator = Coordinator::with_capacities(4, 8);
        coordinator.on_frame(br#"{"jsonrpc":"2.0","id":7,"method":"noSuchMethod"}"#);
        assert_eq!(coordinator.requests.entries.len(), 1);
        // A second request with the same live id consumes no new entry and is a null-id
        // -32600 through the anonymous slot.
        coordinator.on_frame(br#"{"jsonrpc":"2.0","id":7,"method":"noSuchMethod"}"#);
        assert_eq!(
            coordinator.requests.entries.len(),
            1,
            "no new entry for a duplicate"
        );
        assert!(
            frames(&coordinator)
                .iter()
                .any(|f| f.contains(r#""id":null"#) && f.contains("-32600"))
        );
    }

    #[test]
    fn anonymous_slot_exhaustion_is_terminal() {
        // Capacity zero anonymous slots: the first null-id protocol error fail-stops.
        let mut coordinator = Coordinator::with_capacities(4, 0);
        coordinator.on_frame(b"{ not json");
        assert!(
            !coordinator.running,
            "anonymous exhaustion is a fixed terminal"
        );
    }

    // ---- Law: terminal arbitration ----

    #[test]
    fn terminal_classifies_awaiting_delivery_and_abandoned() {
        let dir = temp_project(
            "term",
            "module main\n\npub fn f(): int {\n    return 1\n}\n",
        );
        let mut coordinator = running(&dir);
        // Drop the initial analysis job and its outputs; open a doc so a hover can be held.
        coordinator.job_out = None;
        coordinator.on_frame(
            open_body(
                &dir,
                1,
                "module main\n\npub fn f(): int {\n    return 1\n}\n",
            )
            .as_bytes(),
        );
        coordinator.job_out = None; // ignore the recompute job; no snapshot arrives

        // A hover with no ready snapshot is held (Live, no frame).
        coordinator.on_frame(hover_body(&dir, 10, 3, 12).as_bytes());
        // An unknown request is answered immediately (frame handed off, awaiting delivery).
        coordinator.on_frame(br#"{"jsonrpc":"2.0","id":11,"method":"noSuchMethod"}"#);

        coordinator.on_terminal();
        let held = &coordinator.terminal_classes;
        assert!(
            held.iter().any(|(id, class)| *id == RequestId::Integer(10)
                && *class == TerminalClass::AbandonedByTerminal),
            "held query with no handed-off frame is AbandonedByTerminal"
        );
        assert!(
            held.iter().any(|(id, class)| *id == RequestId::Integer(11)
                && *class == TerminalClass::DeliveryUnknown),
            "handed-off-but-unreceipted request is DeliveryUnknown"
        );
        cleanup(&dir);
    }

    // ---- Law: ContentModified for a query held across an edit ----

    #[test]
    fn query_held_across_edit_is_content_modified() {
        let main1 = "module main\n\npub fn f(): int {\n    return 1\n}\n";
        let main2 = "module main\n\npub fn f(): int {\n    return 2\n}\n";
        let dir = temp_project("cm", main1);
        let mut coordinator = running(&dir);
        coordinator.job_out = None;
        coordinator.on_frame(open_body(&dir, 1, main1).as_bytes());
        coordinator.job_out = None;
        let rev_open = coordinator.current_revision;

        // Hover with no snapshot yet: held at rev_open, version 1.
        coordinator.on_frame(hover_body(&dir, 20, 3, 12).as_bytes());
        assert_eq!(coordinator.held_queries.len(), 1);

        // Edit advances the revision.
        coordinator.on_frame(change_body(&dir, 2, main2).as_bytes());
        assert_ne!(coordinator.current_revision, rev_open);
        coordinator.job_out = None;

        // The snapshot for the new revision arrives; the held hover reauthorizes against
        // the stale revision and is replaced with -32801 ContentModified.
        let snapshot = snapshot_at(&dir, main2, coordinator.current_revision);
        coordinator.on_worker_result(WorkerResult {
            outcome: AnalysisOutcome::Snapshot(snapshot),
        });
        assert!(
            frames(&coordinator)
                .iter()
                .any(|f| f.contains(r#""id":20"#) && f.contains("-32801")),
            "the held query is answered with ContentModified"
        );
        cleanup(&dir);
    }

    // ---- Law: capture-episode latch + publication reset ----

    #[test]
    fn capture_failure_latches_once_and_resets_on_successful_delivery() {
        let main = "module main\n\npub fn f(): int {\n    return 1\n}\n";
        let dir = temp_project("episode", main);
        let mut coordinator = running(&dir);
        coordinator.job_out = None;
        coordinator.on_frame(open_body(&dir, 1, main).as_bytes());
        coordinator.job_out = None;
        let revision = coordinator.current_revision;

        // First background capture failure: latch + one showMessage.
        coordinator.on_worker_result(WorkerResult {
            outcome: AnalysisOutcome::Capture(CaptureRejection {
                revision,
                evidence: Some(UnavailableEvidence {
                    code: "project.source_path",
                    message: "broken".to_owned(),
                }),
            }),
        });
        assert!(matches!(
            coordinator.episode,
            CaptureEpisode::Latched { .. }
        ));
        let show_count = frames(&coordinator)
            .iter()
            .filter(|f| f.contains("window/showMessage"))
            .count();
        assert_eq!(show_count, 1, "exactly one showMessage on latch");

        // Second failure while latched: suppressed.
        coordinator.on_worker_result(WorkerResult {
            outcome: AnalysisOutcome::Capture(CaptureRejection {
                revision,
                evidence: Some(UnavailableEvidence {
                    code: "project.source_path",
                    message: "still broken".to_owned(),
                }),
            }),
        });
        let show_count2 = frames(&coordinator)
            .iter()
            .filter(|f| f.contains("window/showMessage"))
            .count();
        assert_eq!(show_count2, 1, "second failure is suppressed while latched");

        // A successful publication set that observed the latch resets it on full delivery.
        let snapshot = snapshot_at(&dir, main, revision);
        coordinator.on_worker_result(WorkerResult {
            outcome: AnalysisOutcome::Snapshot(snapshot),
        });
        assert!(coordinator.publication.is_some(), "publication in flight");
        // Deliver every publication frame.
        while coordinator.publication.is_some() {
            coordinator.on_receipt();
        }
        assert_eq!(
            coordinator.episode,
            CaptureEpisode::Eligible,
            "the latch resets after the observing set fully delivers"
        );
        cleanup(&dir);
    }

    // ---- Law: publication exclusivity across receipts ----

    #[test]
    fn only_one_publication_plan_builds_at_a_time() {
        let main1 = "module main\n\npub fn f(): int {\n    return 1\n}\n";
        let main2 = "module main\n\npub fn f(): int {\n    return 2\n}\n";
        let dir = temp_project("pubexcl", main1);
        let mut coordinator = running(&dir);
        coordinator.job_out = None;
        coordinator.on_frame(open_body(&dir, 1, main1).as_bytes());
        coordinator.job_out = None;
        let rev1 = coordinator.current_revision;

        // First snapshot: publication A builds and holds the exclusive credit.
        let snapshot_a = snapshot_at(&dir, main1, rev1);
        coordinator.on_worker_result(WorkerResult {
            outcome: AnalysisOutcome::Snapshot(snapshot_a),
        });
        assert!(coordinator.publication.is_some());
        let frames_after_a = coordinator.outbox.len();

        // Advance the revision and deliver a newer snapshot while A is still in flight: it
        // must NOT build a second plan.
        coordinator.on_frame(change_body(&dir, 2, main2).as_bytes());
        coordinator.job_out = None;
        let rev2 = coordinator.current_revision;
        let snapshot_b = snapshot_at(&dir, main2, rev2);
        coordinator.on_worker_result(WorkerResult {
            outcome: AnalysisOutcome::Snapshot(snapshot_b),
        });
        assert!(
            coordinator.pending_publication.is_some(),
            "the newer set waits for the exclusive credit"
        );
        assert_eq!(
            coordinator.outbox.len(),
            frames_after_a,
            "no second plan frames while the first plan holds the credit"
        );

        // Deliver A's frames: on the final receipt, B builds from the final ledger.
        while coordinator.pending_publication.is_some() {
            coordinator.on_receipt();
        }
        assert!(
            coordinator.publication.is_some(),
            "B builds after A's final receipt"
        );
        cleanup(&dir);
    }
}
