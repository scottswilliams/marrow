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

use crate::analysis::{
    AnalysisOutcome, CaptureRejection, OverlayInput, run_analysis, validate_overlay,
};
use crate::capacities::{
    B_PUBLICATION_PLAN_BYTES, MAX_ANONYMOUS_ERROR_SLOTS, MAX_LIVE_REQUEST_ENTRIES,
    OUTBOUND_QUEUE_CAPACITY, RECEIPT_QUEUE_CAPACITY, THREAD_STACK_BYTES,
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
    let (result_tx, result_rx) = sync_channel::<AnalysisOutcome>(1);
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
    results: &Receiver<AnalysisOutcome>,
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
    result: &SyncSender<AnalysisOutcome>,
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
        if result.send(outcome).is_err() {
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
    /// The `initialize` response: a known-id frame that also advances the lifecycle on
    /// delivery. Its entry is retired on its receipt, like an ordinary request, so the
    /// initialize id cannot be reused in the handoff-to-delivery window.
    Initialize(RequestId),
    /// The `shutdown` response, the same known-id / receipt-retired discipline.
    Shutdown(RequestId),
    ShowMessage,
}

impl FrameOwner {
    /// The request id whose ledger entry this frame owns and retires on delivery.
    fn owned_id(&self) -> Option<&RequestId> {
        match self {
            FrameOwner::Request(id) | FrameOwner::Initialize(id) | FrameOwner::Shutdown(id) => {
                Some(id)
            }
            FrameOwner::Anonymous | FrameOwner::Publication | FrameOwner::ShowMessage => None,
        }
    }
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

    /// Reserve one entry for a unique id. Returns `false` when the budget is exhausted.
    #[must_use]
    fn reserve(&mut self, id: RequestId) -> bool {
        if self.entries.len() >= self.capacity {
            return false;
        }
        self.entries.insert(id, ReqState::Live);
        true
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

/// The kind of a held semantic query, parsed to fixed-size fields at admission so a held
/// query never retains unbounded raw parameters.
enum HeldKind {
    Hover(lsp_types::Position),
    Definition(lsp_types::Position),
    Formatting,
}

/// A semantic request held until the analysis snapshot for its revision is ready. It is
/// bound to the admission-time revision and document version, and reauthorized before its
/// success is encoded: a changed revision or document state replaces the success with
/// `-32801 ContentModified`. Only fixed-size fields are retained — never the raw params —
/// so the held set is bounded by the request-ledger capacity, not the inbound frame size.
struct HeldQuery {
    id: RequestId,
    kind: HeldKind,
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
    /// Pre-encoded frames not yet handed off, charged against the publication-plan bound.
    pending: VecDeque<Vec<u8>>,
    /// Handed-off frames awaiting a delivery receipt.
    in_flight_count: usize,
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
        if !self.requests.reserve(id.clone()) {
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
        // until the delivery receipt for this frame arrives. The entry retires on that
        // receipt (not at handoff), so the initialize id cannot be reused in the window.
        // The initialize response is a fixed small frame; a serialization failure fail-stops.
        if !self.hand_off(
            &Outbound::Initialize {
                id: id.clone(),
                result: Box::new(initialize_result()),
            },
            FrameOwner::Initialize(id),
        ) {
            self.terminate(1);
        }
    }

    fn on_shutdown(&mut self, id: RequestId) {
        match self.lifecycle.on_shutdown() {
            RequestGate::Route => {
                if !self.hand_off(&Outbound::Null { id: id.clone() }, FrameOwner::Shutdown(id)) {
                    self.terminate(1);
                }
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
        // An unavailable project makes every semantic request the same fixed -32803.
        if !self.ledger.all_available() {
            self.answer_error(&id, REQUEST_FAILED, "project capture unavailable");
            return;
        }
        let Some(root) = self.root.clone() else {
            self.answer_error(&id, INVALID_PARAMS, "no selected root");
            return;
        };
        // Parse the request into a minimal typed kind and target key at admission, so a
        // held query retains only fixed-size fields — never the unbounded raw params.
        let Some((kind, key)) = parse_semantic(&root, method, params.as_deref()) else {
            self.answer_error(&id, INVALID_PARAMS, "malformed params");
            return;
        };
        let Some(DocumentState::OpenText { version, .. }) = self.ledger.get(&key) else {
            self.answer_error(&id, CONTENT_MODIFIED, "content modified");
            return;
        };
        let held = HeldQuery {
            id,
            kind,
            revision: self.current_revision,
            key,
            version: *version,
        };
        // Every semantic reply is deferred as a held query and served only against an
        // available outbound credit, so a burst of requests cannot materialize a burst of
        // (possibly large) reply frames. When a snapshot is ready and a credit is free the
        // query is answered synchronously here; otherwise it waits.
        self.held_queries.push(held);
        self.serve_ready_queries();
    }

    /// Answer a held query, reauthorizing its revision and document version. A changed
    /// revision or document state is `-32801 ContentModified`; the success encoder is
    /// never invoked in that case. The ledger entry rides as `AwaitingDelivery` and is
    /// retired only by its delivery receipt (never at handoff), so no duplicate-id or
    /// double-response window opens.
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
        match self.answer_kind(&snapshot, &root, &held.kind, &held.key) {
            SemanticAnswer::Reply(body) => {
                let outbound = body.into_outbound(held.id.clone());
                self.respond(held.id, outbound);
            }
            SemanticAnswer::ContentModified => {
                self.answer_error(&held.id, CONTENT_MODIFIED, "content modified")
            }
            SemanticAnswer::Internal => {
                self.answer_error(&held.id, INTERNAL_ERROR, "internal error")
            }
        }
    }

    /// Build the reply for a resolved held-query kind against the current snapshot. The
    /// document is resolved from the bound key (not a re-parsed URI); a closed document is
    /// `ContentModified`.
    fn answer_kind(
        &self,
        snapshot: &AnalysisSnapshot,
        root: &SelectedRoot,
        kind: &HeldKind,
        key: &DocumentKey,
    ) -> SemanticAnswer {
        let Some((identity, source)) = self.resolve_by_key(key) else {
            return SemanticAnswer::ContentModified;
        };
        match kind {
            HeldKind::Hover(position) => SemanticAnswer::Reply(OutboundBody::Hover(facts::hover(
                snapshot, &identity, &source, *position,
            ))),
            HeldKind::Definition(position) => {
                let source_lookup = |file: &marrow_project_fs::FileIdentity| self.file_source(file);
                match facts::definition(
                    snapshot,
                    root,
                    &identity,
                    &source,
                    source_lookup,
                    *position,
                ) {
                    Ok(location) => SemanticAnswer::Reply(OutboundBody::Definition(location)),
                    Err(_) => SemanticAnswer::Internal,
                }
            }
            HeldKind::Formatting => SemanticAnswer::Reply(OutboundBody::Formatting(
                facts::formatting(snapshot, &identity, &source),
            )),
        }
    }

    /// The file identity and current open text for a document key, if it is still an open
    /// text document.
    fn resolve_by_key(
        &self,
        key: &DocumentKey,
    ) -> Option<(marrow_project_fs::FileIdentity, String)> {
        match self.ledger.get(key) {
            Some(DocumentState::OpenText { text, .. }) => {
                let (identity, _) =
                    marrow_project_fs::FileIdentity::validate(key.relative()).ok()?;
                Some((identity, text.clone()))
            }
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
        self.admit_document(&root, key, document.version, document.text);
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
        self.admit_document(&root, key, version, change.text);
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
        // Closing commits the removal and revision, but disk recapture waits until every
        // remaining open entry is available (design: no recompute while an entry is
        // unavailable).
        if self.ledger.all_available() {
            self.maybe_recompute(&root);
        }
    }

    /// Admit a new or changed document body: validate the full candidate overlay, store
    /// `OpenText` on success or `OpenUnavailable` (with rendered evidence) on refusal, and
    /// enqueue a recompute only when every open entry is available.
    fn admit_document(
        &mut self,
        root: &SelectedRoot,
        key: DocumentKey,
        version: i32,
        text: String,
    ) {
        let candidate = self.candidate_overlay(&key, text.as_bytes());
        let inputs: Vec<OverlayInput<'_>> = candidate
            .iter()
            .map(|(key, bytes)| OverlayInput {
                key: key.as_str(),
                bytes: bytes.as_slice(),
            })
            .collect();
        let state = match validate_overlay(root, &inputs) {
            Ok(()) => DocumentState::OpenText { version, text },
            Err(evidence) => DocumentState::OpenUnavailable {
                version,
                failure: evidence.unwrap_or_else(unrenderable_overlay_evidence),
            },
        };
        drop(inputs);
        drop(candidate);
        let available = state.is_text();
        self.ledger.insert(key, state);
        if available && self.ledger.all_available() {
            self.maybe_recompute(root);
        }
    }

    /// Build the full candidate overlay: every other open text entry plus this key's new
    /// body. The changed key's prior state (text or unavailable) is excluded so the
    /// candidate body is the one under validation.
    fn candidate_overlay(&self, changed: &DocumentKey, body: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut overlay: Vec<(String, Vec<u8>)> = self
            .ledger
            .text_entries()
            .filter(|(key, _)| *key != changed)
            .map(|(key, text)| (key.relative().to_owned(), text.as_bytes().to_vec()))
            .collect();
        overlay.push((changed.relative().to_owned(), body.to_vec()));
        overlay
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

    fn on_worker_result(&mut self, outcome: AnalysisOutcome) {
        self.worker_busy = false;
        match outcome {
            AnalysisOutcome::Snapshot(snapshot) => {
                if snapshot.revision() == self.current_revision {
                    self.snapshot = Some(snapshot.clone());
                    self.snapshot_revision = Some(snapshot.revision());
                    self.begin_publication(snapshot);
                    self.serve_ready_queries();
                }
            }
            AnalysisOutcome::Capture(rejection) => {
                if rejection.revision == self.current_revision {
                    self.on_background_capture_failure(rejection);
                }
            }
            AnalysisOutcome::ResourceLimit { revision } => {
                // Recoverable: publishes and clears nothing. A request whose bound analysis
                // reached the resource limit gets a fixed `-32803`; the delivered
                // diagnostic ledger is unchanged.
                if revision == self.current_revision {
                    self.fail_held_queries();
                }
            }
            AnalysisOutcome::Invariant => {
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

    /// Answer held queries while a snapshot for the current revision is ready and an
    /// outbound credit is available. A potentially large reply (a whole-document format) is
    /// thus only ever materialized against an available credit, so replies never enter the
    /// pending-frame queue; the unanswered remainder stays held — bounded by the request
    /// ledger, at fixed-size cost — and is served as credits free on later receipts.
    fn serve_ready_queries(&mut self) {
        while self.snapshot_revision == Some(self.current_revision)
            && self.outbound_credits.available() > 0
        {
            let Some(query) = self.held_queries.pop() else {
                break;
            };
            self.answer_held(query);
        }
    }

    /// Answer every held query with a fixed `-32803`: the analysis for their revision could
    /// not produce a snapshot (a resource limit), so no success is possible.
    fn fail_held_queries(&mut self) {
        let held: Vec<HeldQuery> = std::mem::take(&mut self.held_queries);
        for query in held {
            self.answer_error(&query.id, REQUEST_FAILED, "analysis resource limit");
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
                    self.send_show_message(show_message(&evidence));
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
        let (frames, new_published) = self.plan_publication(&snapshot);
        // Pre-encode the whole set fallibly before committing anything. A serialization
        // failure or a plan whose retained bytes exceed the publication-plan bound is a
        // fixed `PublicationPlanFailed`: the credit is released, the delivered ledger and
        // capture episode are unchanged, and the server fail-stops. This is why a single
        // oversized diagnostic frame can never strand the exclusive credit.
        let mut encoded = VecDeque::new();
        let mut retained: u64 = 0;
        for outbound in &frames {
            let Ok(bytes) = encode(outbound) else {
                self.publication_credits.release(credit);
                self.terminate(1);
                return;
            };
            retained = retained.saturating_add(bytes.len() as u64);
            if retained > B_PUBLICATION_PLAN_BYTES {
                self.publication_credits.release(credit);
                self.terminate(1);
                return;
            }
            encoded.push_back(bytes);
        }
        if encoded.is_empty() {
            // A zero-frame commit resets the episode immediately (no frame to await), and
            // still commits the (empty) ledger transition.
            self.published = new_published;
            self.publication_credits.release(credit);
            self.reset_episode_if_observed(observed_episode);
            return;
        }
        // Commit: the delivered-ledger update happens only now, after every frame encoded.
        self.published = new_published;
        self.publication = Some(PublicationState {
            credit,
            pending: encoded,
            in_flight_count: 0,
            observed_episode,
        });
        self.feed_publication();
    }

    /// Compute the complete publication set without committing it: every current file's
    /// diagnostic list (including empties) plus an empty tombstone for every previously
    /// published file absent from the snapshot, and the new delivered-ledger key set. The
    /// delivered ledger is read but not mutated; `begin_publication` commits it only after
    /// the whole set encodes.
    fn plan_publication(&self, snapshot: &AnalysisSnapshot) -> (Vec<Outbound>, Vec<DocumentKey>) {
        let Some(root) = self.root.clone() else {
            return (Vec::new(), Vec::new());
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
        for key in &self.published {
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
        (frames, new_published)
    }

    /// Move pre-encoded publication frames into the outbound path one per available credit,
    /// so publication retention stays bounded by the plan buffer and never floods the
    /// pending-frame queue past `W`.
    fn feed_publication(&mut self) {
        loop {
            let has_pending = self
                .publication
                .as_ref()
                .is_some_and(|state| !state.pending.is_empty());
            if !has_pending {
                break;
            }
            let Some(credit) = self.outbound_credits.acquire() else {
                break;
            };
            let state = self.publication.as_mut().expect("publication present");
            let bytes = state.pending.pop_front().expect("pending frame present");
            state.in_flight_count += 1;
            self.outbox.push_back(bytes);
            self.in_flight.push_back((FrameOwner::Publication, credit));
        }
    }

    fn on_publication_receipt(&mut self) {
        if let Some(state) = self.publication.as_mut() {
            state.in_flight_count = state.in_flight_count.saturating_sub(1);
        }
        // The freed credit lets the next pre-encoded frame proceed.
        self.feed_publication();
        let done = self
            .publication
            .as_ref()
            .is_some_and(|state| state.pending.is_empty() && state.in_flight_count == 0);
        if done {
            // The whole committed set is delivered: release the exclusive credit and reset
            // the episode only if the commit observed the still-current latch.
            let state = self.publication.take().expect("publication present");
            self.publication_credits.release(state.credit);
            self.reset_episode_if_observed(state.observed_episode);
            // A newer result that waited may now build its plan and derive tombstones from
            // the final ledger.
            if let Some(snapshot) = self.pending_publication.take() {
                self.begin_publication(snapshot);
            }
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

    /// Encode and hand off one frame, acquiring an outbound credit. Returns whether the
    /// frame was handed off. A pre-handoff encode failure emits zero bytes and returns
    /// `false`, so the caller reconciles its own bookkeeping rather than stranding it. A
    /// request-owned handoff moves its ledger entry to `AwaitingDelivery`; the credit is
    /// held until the delivery receipt whether the frame is written immediately or queued.
    #[must_use]
    fn hand_off(&mut self, outbound: &Outbound, owner: FrameOwner) -> bool {
        let Ok(bytes) = encode(outbound) else {
            return false;
        };
        if let Some(id) = owner.owned_id() {
            self.requests.set_awaiting(id);
        }
        match self.outbound_credits.acquire() {
            Some(credit) => {
                self.outbox.push_back(bytes);
                self.in_flight.push_back((owner, credit));
            }
            None => self.pending_frames.push_back((bytes, owner)),
        }
        true
    }

    /// Hand off a known-id response. On a pre-handoff encode failure (an oversized success,
    /// or a serialization defect) the entry gets exactly one fixed same-id `-32603`
    /// fallback — already internal-error class, so it takes no further fallback — and
    /// fail-stops if even that cannot encode. The entry stays owned and retires on its
    /// delivery receipt, so a dropped response is never a silent no-reply.
    fn respond(&mut self, id: RequestId, outbound: Outbound) {
        if self.hand_off(&outbound, FrameOwner::Request(id.clone())) {
            return;
        }
        let fallback = Outbound::Error {
            id: Some(id.clone()),
            code: INTERNAL_ERROR,
            message: "internal error".to_owned(),
        };
        if !self.hand_off(&fallback, FrameOwner::Request(id)) {
            self.terminate(1);
        }
    }

    fn answer_error(&mut self, id: &RequestId, code: i32, message: &str) {
        self.respond(
            id.clone(),
            Outbound::Error {
                id: Some(id.clone()),
                code,
                message: message.to_owned(),
            },
        );
    }

    fn reserve_and_error(&mut self, id: RequestId, code: i32, message: &str) {
        if !self.requests.reserve(id.clone()) {
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
        // A null-id protocol error has no recursive fallback: a pre-handoff encode failure
        // is a fixed `OutboundEncodingFailed` terminal, and the reserved slot is released.
        if !self.hand_off(
            &Outbound::Error {
                id: None,
                code,
                message: message.to_owned(),
            },
            FrameOwner::Anonymous,
        ) {
            self.anonymous_slots -= 1;
            self.terminate(1);
        }
    }

    /// Hand off a background `showMessage`. A pre-handoff encode failure fail-stops with no
    /// substitute frame (no recursive protocol fallback).
    fn send_show_message(&mut self, message: Outbound) {
        if !self.hand_off(&message, FrameOwner::ShowMessage) {
            self.terminate(1);
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
            FrameOwner::Initialize(id) => {
                self.requests.retire(&id);
                if self.lifecycle.on_initialize_delivered() {
                    self.enter_running();
                }
            }
            FrameOwner::Shutdown(id) => {
                self.requests.retire(&id);
                self.lifecycle.on_shutdown_delivered();
            }
            FrameOwner::ShowMessage => {}
        }
        // The freed credit makes progress on outstanding work, in priority order: the
        // in-flight publication set, then a ready held query (its potentially large reply
        // is only ever materialized against an available credit, so it never enters the
        // pending-frame queue), then a queued small frame.
        self.feed_publication();
        self.serve_ready_queries();
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
            if let Some(id) = owner.owned_id() {
                awaiting.push(id.clone());
            }
        }
        for (_, owner) in &self.pending_frames {
            if let Some(id) = owner.owned_id() {
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
    Internal,
}

/// Parse a semantic request into its minimal typed kind and target document key. Returns
/// `None` for a malformed request, an unknown method, or a URI outside the selected root.
fn parse_semantic(
    root: &SelectedRoot,
    method: &str,
    params: Option<&serde_json::value::RawValue>,
) -> Option<(HeldKind, DocumentKey)> {
    let (kind, uri) = match method {
        "textDocument/hover" => {
            let params = parse::<HoverParams>(params?)?.text_document_position_params;
            (HeldKind::Hover(params.position), params.text_document.uri)
        }
        "textDocument/definition" => {
            let params = parse::<GotoDefinitionParams>(params?)?.text_document_position_params;
            (
                HeldKind::Definition(params.position),
                params.text_document.uri,
            )
        }
        "textDocument/formatting" => {
            let params = parse::<DocumentFormattingParams>(params?)?;
            (HeldKind::Formatting, params.text_document.uri)
        }
        _ => return None,
    };
    let key = DocumentKey::from_uri(uri.as_str(), root).ok()?;
    Some((kind, key))
}

/// The fallback unavailable-evidence when even the bounded operational message overflows
/// its sink (defensive: overlay refusal messages are short and cannot reach the cap).
fn unrenderable_overlay_evidence() -> UnavailableEvidence {
    UnavailableEvidence {
        code: marrow_codes::Code::ProjectSourcePath.as_str(),
        message: String::new(),
    }
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
    let admit = |uri: &str| {
        SelectedRoot::from_uri(uri)
            .map(Some)
            .map_err(|_: UriError| RootError::Malformed)
    };
    if let Some(folders) = &params.workspace_folders {
        match folders.as_slice() {
            [] => {}
            [folder] => return admit(folder.uri.as_str()),
            _ => return Err(RootError::TooMany),
        }
    }
    #[allow(deprecated)]
    match &params.root_uri {
        Some(uri) => admit(uri.as_str()),
        None => Ok(None),
    }
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
        coordinator.on_worker_result(AnalysisOutcome::Snapshot(snapshot));
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
        coordinator.on_worker_result(AnalysisOutcome::Capture(CaptureRejection {
            revision,
            evidence: Some(UnavailableEvidence {
                code: "project.source_path",
                message: "broken".to_owned(),
            }),
        }));
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
        coordinator.on_worker_result(AnalysisOutcome::Capture(CaptureRejection {
            revision,
            evidence: Some(UnavailableEvidence {
                code: "project.source_path",
                message: "still broken".to_owned(),
            }),
        }));
        let show_count2 = frames(&coordinator)
            .iter()
            .filter(|f| f.contains("window/showMessage"))
            .count();
        assert_eq!(show_count2, 1, "second failure is suppressed while latched");

        // A successful publication set that observed the latch resets it on full delivery.
        let snapshot = snapshot_at(&dir, main, revision);
        coordinator.on_worker_result(AnalysisOutcome::Snapshot(snapshot));
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

    // ---- Soundness: a reply retires its entry only on delivery, not at handoff ----

    #[test]
    fn duplicate_id_after_reply_handoff_is_rejected_not_answered_twice() {
        let main = "module main\n\npub fn f(): int {\n    return 1\n}\n";
        let dir = temp_project("dupreply", main);
        let mut coordinator = running(&dir);
        coordinator.job_out = None;
        coordinator.on_frame(open_body(&dir, 1, main).as_bytes());
        coordinator.job_out = None;

        // Hover id=5 held (no snapshot yet), then the snapshot lands and it is answered.
        coordinator.on_frame(hover_body(&dir, 5, 3, 12).as_bytes());
        let snapshot = snapshot_at(&dir, main, coordinator.current_revision);
        coordinator.on_worker_result(AnalysisOutcome::Snapshot(snapshot));
        // The reply frame was handed off; its ledger entry is still live (AwaitingDelivery),
        // not retired at handoff.
        assert!(coordinator.requests.is_live(&RequestId::Integer(5)));

        // A second request reusing the in-flight id is caught as a duplicate — no second
        // response for id 5 — rather than being admitted and answered again.
        coordinator.on_frame(hover_body(&dir, 5, 3, 12).as_bytes());
        let id5_results = frames(&coordinator)
            .iter()
            .filter(|f| f.contains(r#""id":5"#) && f.contains(r#""result""#))
            .count();
        assert_eq!(id5_results, 1, "the reused id is answered exactly once");
        assert!(
            frames(&coordinator)
                .iter()
                .any(|f| f.contains(r#""id":null"#) && f.contains("-32600")),
            "the duplicate id gets a null-id -32600"
        );
    }

    // ---- Law: per-document overlay refusal -> OpenUnavailable -> -32803, then recovery ----

    #[test]
    fn oversized_change_marks_document_unavailable_then_recovers() {
        let main = "module main\n\npub fn f(): int {\n    return 1\n}\n";
        let dir = temp_project("unavail", main);
        let mut coordinator = running(&dir);
        coordinator.job_out = None;
        coordinator.on_frame(open_body(&dir, 1, main).as_bytes());
        coordinator.job_out = None;

        // A change whose body exceeds the 1 MiB per-file overlay bound is refused: the
        // document becomes OpenUnavailable and no recompute is enqueued.
        let huge = "x".repeat((1 << 20) + 1);
        coordinator.on_frame(change_body(&dir, 2, &huge).as_bytes());
        assert!(
            !coordinator.ledger.all_available(),
            "the oversized document is unavailable"
        );
        assert!(
            coordinator.job_out.is_none(),
            "no recompute while unavailable"
        );

        // Every semantic request is the same fixed -32803 while a document is unavailable.
        coordinator.on_frame(hover_body(&dir, 7, 3, 12).as_bytes());
        assert!(
            frames(&coordinator)
                .iter()
                .any(|f| f.contains(r#""id":7"#) && f.contains("-32803")),
            "semantic requests are -32803 while unavailable"
        );

        // A later valid change recovers the document and re-enables recomputation. Model
        // the worker as idle (it consumed the earlier job) so recovery dispatches rather
        // than coalescing.
        coordinator.worker_busy = false;
        coordinator.on_frame(change_body(&dir, 3, main).as_bytes());
        assert!(
            coordinator.ledger.all_available(),
            "a valid change recovers the document"
        );
        assert!(
            coordinator.job_out.is_some(),
            "recompute re-enqueues on recovery"
        );
        cleanup(&dir);
    }

    // ---- Law: a resource-limited analysis fails held queries with -32803 ----

    #[test]
    fn analysis_resource_limit_fails_held_queries() {
        let main = "module main\n\npub fn f(): int {\n    return 1\n}\n";
        let dir = temp_project("reslimit", main);
        let mut coordinator = running(&dir);
        coordinator.job_out = None;
        coordinator.on_frame(open_body(&dir, 1, main).as_bytes());
        coordinator.job_out = None;

        coordinator.on_frame(hover_body(&dir, 8, 3, 12).as_bytes());
        assert_eq!(coordinator.held_queries.len(), 1);

        coordinator.on_worker_result(AnalysisOutcome::ResourceLimit {
            revision: coordinator.current_revision,
        });
        assert!(coordinator.held_queries.is_empty(), "held queries drain");
        assert!(
            frames(&coordinator)
                .iter()
                .any(|f| f.contains(r#""id":8"#) && f.contains("-32803")),
            "a held query at a resource-limited revision is -32803"
        );
        cleanup(&dir);
    }

    // ---- Soundness: a credit freed by a non-publication receipt feeds a starved plan ----

    #[test]
    fn freed_credit_feeds_a_credit_starved_publication() {
        let main = "module main\n\npub fn f(): int {\n    return 1\n}\n";
        let dir = temp_project("starve", main);
        let mut coordinator = running(&dir);
        coordinator.job_out = None;
        coordinator.on_frame(open_body(&dir, 1, main).as_bytes());
        coordinator.job_out = None;

        // Exhaust every outbound credit with in-flight (unreceipted) error frames.
        for id in 0..(crate::capacities::OUTBOUND_CREDITS as i64) {
            coordinator.on_frame(
                format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"noSuchMethod"}}"#).as_bytes(),
            );
        }
        assert_eq!(coordinator.outbound_credits.available(), 0);

        // A snapshot commits its publication plan but is credit-starved: the plan holds the
        // exclusive credit with frames pending and nothing in flight.
        let snapshot = snapshot_at(&dir, main, coordinator.current_revision);
        coordinator.on_worker_result(AnalysisOutcome::Snapshot(snapshot));
        assert!(coordinator.publication.is_some(), "plan committed");
        let starved = coordinator
            .publication
            .as_ref()
            .expect("publication present");
        assert!(
            !starved.pending.is_empty(),
            "frames pending under starvation"
        );
        assert_eq!(starved.in_flight_count, 0, "nothing fed yet");

        // Delivering a non-publication frame frees a credit; the receipt must feed the
        // starved publication rather than leaving its credit held forever.
        coordinator.on_receipt();
        assert!(
            coordinator
                .publication
                .as_ref()
                .is_some_and(|state| state.in_flight_count > 0),
            "a freed credit feeds the starved publication"
        );

        // Drain to completion: the exclusive credit is released.
        while coordinator.publication.is_some() {
            coordinator.on_receipt();
        }
        cleanup(&dir);
    }

    // ---- Soundness: replies never flood the pending-frame queue past W ----

    #[test]
    fn credit_starved_replies_stay_held_not_queued() {
        let main = "module main\n\npub fn f(): int {\n    return 1\n}\n";
        let dir = temp_project("noflood", main);
        let mut coordinator = running(&dir);
        coordinator.job_out = None;
        coordinator.on_frame(open_body(&dir, 1, main).as_bytes());
        coordinator.job_out = None;

        // Exhaust every outbound credit.
        for id in 0..(crate::capacities::OUTBOUND_CREDITS as i64) {
            coordinator.on_frame(
                format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"noSuchMethod"}}"#).as_bytes(),
            );
        }
        assert_eq!(coordinator.outbound_credits.available(), 0);
        let pending_before = coordinator.pending_frames.len();

        // A burst of semantic requests with no free credit: they are held, not materialized.
        for id in 100..140 {
            coordinator.on_frame(hover_body(&dir, id, 3, 12).as_bytes());
        }
        assert!(
            coordinator.held_queries.len() >= 40,
            "the burst is held, not answered"
        );

        // A snapshot lands while credits are exhausted: no reply materializes, so the
        // pending-frame queue does not grow with reply frames.
        let snapshot = snapshot_at(&dir, main, coordinator.current_revision);
        coordinator.on_worker_result(AnalysisOutcome::Snapshot(snapshot));
        assert_eq!(
            coordinator.pending_frames.len(),
            pending_before,
            "replies never enter the pending-frame queue"
        );
        assert!(
            !coordinator.held_queries.is_empty(),
            "unanswered replies stay held, bounded by the request ledger"
        );
        cleanup(&dir);
    }

    // ---- Soundness: initialize/shutdown ids retire on receipt, not at handoff ----

    #[test]
    fn initialize_id_reuse_in_delivery_window_is_rejected() {
        let dir = temp_project("initreuse", "module main\n");
        let mut coordinator = Coordinator::new();
        coordinator.on_frame(initialize_body(&root_uri(dir.as_path())).as_bytes());
        // The initialize id rides AwaitingDelivery until its receipt (not retired at handoff).
        assert!(coordinator.requests.is_live(&RequestId::Integer(1)));

        // A frame reusing the in-flight initialize id is caught as a duplicate, not answered
        // a second time.
        coordinator.on_frame(br#"{"jsonrpc":"2.0","id":1,"method":"noSuchMethod"}"#);
        assert!(
            frames(&coordinator)
                .iter()
                .any(|f| f.contains(r#""id":null"#) && f.contains("-32600")),
            "the reused initialize id gets a null-id -32600"
        );

        // The receipt retires the id and advances the lifecycle.
        coordinator.on_receipt();
        assert!(!coordinator.requests.is_live(&RequestId::Integer(1)));
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
        coordinator.on_worker_result(AnalysisOutcome::Snapshot(snapshot_a));
        assert!(coordinator.publication.is_some());
        let frames_after_a = coordinator.outbox.len();

        // Advance the revision and deliver a newer snapshot while A is still in flight: it
        // must NOT build a second plan.
        coordinator.on_frame(change_body(&dir, 2, main2).as_bytes());
        coordinator.job_out = None;
        let rev2 = coordinator.current_revision;
        let snapshot_b = snapshot_at(&dir, main2, rev2);
        coordinator.on_worker_result(AnalysisOutcome::Snapshot(snapshot_b));
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
