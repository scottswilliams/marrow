//! The process-main coordinator and its reader/worker/writer threads.
//!
//! The reader frames stdin into a cap-1 ingress queue and backpressures without drop or
//! reorder. The coordinator owns lifecycle, admission, document versions, overlay
//! construction, latest-wins edit coalescing, and outbound ordering; it never blocks on
//! I/O, downstream sends, or joins — it idles only on a cap-1 lost-wakeup-safe wake
//! channel and drains receipts, then results, then ingress. One analysis worker owns all
//! capture/analyze work. One writer accepts immutable framed bytes and returns a delivery
//! receipt that frees the outbound credit it consumed.
//!
//! [`Coordinator`] is a pure event machine: it consumes typed events (`on_frame`,
//! `on_worker_result`, `on_receipt`, `on_terminal`) and produces outbound frames into
//! [`Coordinator::outbox`] and at most one analysis job into [`Coordinator::job_out`],
//! which the thread driver drains. It touches no channel, so its whole state machine is
//! exercised deterministically with no timing dependence.

use std::collections::VecDeque;
use std::io::{BufReader, Write};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::thread::JoinHandle;

use lsp_types::{
    CompletionOptions, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, InitializeParams, InitializeResult, OneOf, ServerCapabilities,
    ServerInfo, SignatureHelpOptions, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions,
};
use marrow_compile::{AnalysisResourceLimit, AnalysisSnapshot, InputRevision, ProjectFile};

use crate::analysis::{
    AnalysisOutcome, CaptureRejection, OverlayInput, run_analysis, validate_overlay,
};
use crate::capacities::{
    MAX_ANONYMOUS_ERROR_SLOTS, MAX_LIVE_REQUEST_ENTRIES, MAX_PUBLICATION_PLAN_BYTES,
    OUTBOUND_CREDITS, OUTBOUND_QUEUE_CAPACITY, RECEIPT_QUEUE_CAPACITY, THREAD_STACK_BYTES,
};
use crate::document::{DocumentLedger, DocumentState, RevisionCounter, UnavailableEvidence};
use crate::facts;
use crate::lifecycle::{IngressGate, Lifecycle, Phase, RequestGate};
use crate::outbound::{ErrorCode, MessageType, Outbound, ResponseResult, encode};
use crate::protocol::{Inbound, InvalidReason, Reject, RequestId, decode, decode_params};
use crate::query::{MalformedParams, QueryRefusal, SemanticQuery};
use crate::uri::{DocumentKey, OriginRoots, SelectedRoot, UriError};

/// A unit of capture/analyze work handed to the worker. The overlay shares the ledger's
/// document text, so dispatching a recompute copies no document body.
struct WorkerJob {
    root: SelectedRoot,
    revision: InputRevision,
    overlay: Vec<(String, Arc<str>)>,
}

/// A writer delivery receipt: one completed and flushed frame.
struct Receipt;

/// Run the language server over stdio, returning the process exit code.
pub fn serve() -> u8 {
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
        let mut progressed = drain(receipts, coordinator, |coordinator, Receipt| {
            coordinator.on_receipt();
        });
        progressed |= drain(results, coordinator, Coordinator::on_worker_result);
        progressed |= receive_ingress(coordinator, ingress);
        // Move coordinator outputs downstream without blocking on a full channel.
        if let Some(job) = coordinator.job_out.take()
            && work_tx.try_send(job).is_err()
        {
            coordinator.worker_busy = false;
            coordinator.pending_recompute = true;
        }
        while let Some(bytes) = coordinator.outbound.outbox.front() {
            match frame_tx.try_send(bytes.clone()) {
                Ok(()) => {
                    coordinator.outbound.outbox.pop_front();
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

/// Drain one producer's channel into the coordinator. A disconnected channel means its
/// thread is gone — it panicked, or its stream closed without a terminal event — which
/// ends the server rather than leaving it idle forever.
fn drain<T>(
    events: &Receiver<T>,
    coordinator: &mut Coordinator,
    mut apply: impl FnMut(&mut Coordinator, T),
) -> bool {
    let mut progressed = false;
    loop {
        match events.try_recv() {
            Ok(event) => {
                apply(coordinator, event);
                progressed = true;
            }
            Err(TryRecvError::Empty) => return progressed,
            Err(TryRecvError::Disconnected) => {
                coordinator.terminate(1);
                return true;
            }
        }
    }
}

/// Receive at most one inbound event and apply it to the coordinator. A reader that is
/// gone without a terminal event is the same terminal.
fn receive_ingress(coordinator: &mut Coordinator, ingress: &Receiver<ReaderEvent>) -> bool {
    if coordinator.lifecycle.ingress_gate() == IngressGate::AwaitInitializeDelivery {
        return false;
    }
    match ingress.try_recv() {
        Ok(ReaderEvent::Frame(body)) => coordinator.on_frame(&body),
        Ok(ReaderEvent::Terminal) | Err(TryRecvError::Disconnected) => coordinator.on_terminal(),
        Err(TryRecvError::Empty) => return false,
    }
    true
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
            outcome => {
                // Either end of the stream is terminal; a framing fault is named on
                // stderr so an editor's log says why the server stopped reading.
                if let Err(crate::transport::FrameError::Fault(fault)) = outcome {
                    let _ = writeln!(std::io::stderr().lock(), "marrow-lsp: {fault:?}");
                }
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
            .map(|(key, text)| OverlayInput {
                key: key.as_str(),
                bytes: text.as_bytes(),
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
///
/// An entry retires on its delivery receipt, never at handoff, so no id can be reused
/// and answered twice inside the handoff-to-delivery window.
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
enum TerminalClass {
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
    /// delivery.
    Initialize(RequestId),
    /// The `shutdown` response.
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

// ---- outbound queue ----

/// The outbound path: encoded frames for the writer in order, the owners of frames
/// awaiting a delivery receipt, and frames waiting for credit. Every handoff passes
/// through this type under its capacity check, so `in_flight.len() <= OUTBOUND_CREDITS`
/// holds by construction.
struct OutboundQueue {
    /// Encoded frames for the driver to write, in order.
    outbox: VecDeque<Vec<u8>>,
    /// The owners of handed-off frames awaiting a delivery receipt. FIFO: the front is
    /// the oldest, matching the single writer. Its length is the spent credit.
    in_flight: VecDeque<FrameOwner>,
    /// Frames waiting for credit, in order.
    pending: VecDeque<(Vec<u8>, FrameOwner)>,
}

impl OutboundQueue {
    fn new() -> Self {
        Self {
            outbox: VecDeque::new(),
            in_flight: VecDeque::new(),
            pending: VecDeque::new(),
        }
    }

    /// The credit not yet spent: frames may be handed off while fewer than
    /// `OUTBOUND_CREDITS` await their delivery receipts.
    fn capacity(&self) -> usize {
        OUTBOUND_CREDITS - self.in_flight.len()
    }

    /// Hand a frame off if credit allows, else queue it behind the frames already
    /// waiting. Either way the frame is retained until its delivery.
    fn admit(&mut self, bytes: Vec<u8>, owner: FrameOwner) {
        if self.capacity() == 0 {
            self.pending.push_back((bytes, owner));
        } else {
            self.dispatch(bytes, owner);
        }
    }

    /// Hand off frames from `source` while credit allows, returning how many moved. A
    /// source frame never enters the waiting queue: it stays with its owner.
    fn feed(&mut self, source: &mut VecDeque<Vec<u8>>, owner: &FrameOwner) -> usize {
        let mut moved = 0;
        while self.capacity() > 0 {
            let Some(bytes) = source.pop_front() else {
                break;
            };
            self.dispatch(bytes, owner.clone());
            moved += 1;
        }
        moved
    }

    /// Hand off the oldest waiting frame if credit allows.
    fn admit_waiting(&mut self) {
        if self.capacity() > 0
            && let Some((bytes, owner)) = self.pending.pop_front()
        {
            self.dispatch(bytes, owner);
        }
    }

    /// Consume the oldest delivery receipt: the owner of the frame the writer completed.
    fn receipt(&mut self) -> Option<FrameOwner> {
        self.in_flight.pop_front()
    }

    /// The owners of every handed-off or waiting frame that has not delivered.
    fn undelivered_owners(&self) -> impl Iterator<Item = &FrameOwner> {
        self.in_flight
            .iter()
            .chain(self.pending.iter().map(|(_, owner)| owner))
    }

    fn dispatch(&mut self, bytes: Vec<u8>, owner: FrameOwner) {
        self.outbox.push_back(bytes);
        self.in_flight.push_back(owner);
    }
}

// ---- held queries ----

/// A semantic request held until current analysis completes and outbound credit is
/// available. It is bound to the admission-time revision and document version, and
/// reauthorized before either a success or resource refusal: changed input produces
/// `-32801 ContentModified`. Only fixed-size fields are retained — never the raw params —
/// so the held set is bounded by the request-ledger capacity, not the inbound frame size.
struct HeldQuery {
    id: RequestId,
    query: SemanticQuery,
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

/// The in-flight analysis publication set. It is exclusive: no other plan reads the
/// delivered ledger until every frame of this one delivers. Only a successful snapshot
/// records the capture-episode latch it observed at commit; a resource-stop publication
/// cannot reset that latch.
struct PublicationState {
    /// Pre-encoded frames not yet handed off, charged against the publication-plan bound.
    pending: VecDeque<Vec<u8>>,
    /// Handed-off frames awaiting a delivery receipt.
    in_flight_count: usize,
    observed_episode: Option<u64>,
}

/// The result for `current_revision`. An admitted edit invalidates it before capture
/// or analysis can run, so an older result can never answer a current request.
enum CurrentAnalysis {
    Pending,
    Ready(Arc<AnalysisSnapshot>),
    ResourceLimited(AnalysisResourceLimit),
}

/// The process-main coordinator.
struct Coordinator {
    lifecycle: Lifecycle,
    root: Option<SelectedRoot>,
    /// Where each captured dependency sits relative to `root`, taken from the most
    /// recent successful capture. Empty until one succeeds, which is why a document URI
    /// resolves against `root` alone: the workspace is one project and only its own
    /// source is ever opened.
    origins: OriginRoots,
    ledger: DocumentLedger,
    revisions: RevisionCounter,
    current_revision: InputRevision,
    analysis: CurrentAnalysis,
    published: Vec<DocumentKey>,

    requests: RequestLedger,
    anonymous_slots: usize,
    held_queries: Vec<HeldQuery>,

    outbound: OutboundQueue,

    worker_busy: bool,
    pending_recompute: bool,

    episode: CaptureEpisode,
    next_episode: u64,

    publication: Option<PublicationState>,
    pending_publication: Option<InputRevision>,

    /// At most one analysis job for the driver to dispatch.
    job_out: Option<WorkerJob>,

    /// Terminal classifications, recorded for tests and for delivery accounting.
    terminal_classes: Vec<(RequestId, TerminalClass)>,

    exit_code: u8,
    running: bool,
}

impl Coordinator {
    fn new() -> Self {
        let (revisions, current_revision) = RevisionCounter::initial();
        Self {
            lifecycle: Lifecycle::new(),
            root: None,
            origins: OriginRoots::default(),
            ledger: DocumentLedger::new(),
            revisions,
            current_revision,
            analysis: CurrentAnalysis::Pending,
            published: Vec::new(),
            requests: RequestLedger::new(MAX_LIVE_REQUEST_ENTRIES),
            anonymous_slots: 0,
            held_queries: Vec::new(),
            outbound: OutboundQueue::new(),
            worker_busy: false,
            pending_recompute: false,
            episode: CaptureEpisode::Eligible,
            next_episode: 0,
            publication: None,
            pending_publication: None,
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
            Reject::ParseError => self.send_null_error(ErrorCode::ParseError),
            Reject::InvalidRequest {
                recovered_id,
                reason,
            } => {
                let code = match reason {
                    InvalidReason::NoBatch => ErrorCode::BatchUnsupported,
                    InvalidReason::Structural => ErrorCode::InvalidRequest,
                };
                match recovered_id {
                    // A recovered-id invalid request reserves a known-id error-only entry
                    // before its frame is handed off; a collision with a live entry uses
                    // the anonymous slot instead.
                    Some(id) if !self.requests.is_live(&id) => self.reserve_and_error(id, code),
                    _ => self.send_null_error(code),
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
            self.send_null_error(ErrorCode::DuplicateRequestId);
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
            _ => match SemanticQuery::parse(method, params.as_deref()) {
                Some(parsed) => self.on_semantic_request(id, parsed),
                None => match self.lifecycle.gate_request() {
                    RequestGate::NotInitialized => {
                        self.respond(id, Err(ErrorCode::ServerNotInitialized))
                    }
                    RequestGate::InvalidInPhase => self.respond(id, Err(ErrorCode::InvalidInPhase)),
                    RequestGate::Route => self.respond(id, Err(ErrorCode::MethodNotFound)),
                },
            },
        }
    }

    fn on_initialize(&mut self, id: RequestId, params: Option<Box<serde_json::value::RawValue>>) {
        if self.lifecycle.on_initialize() != RequestGate::Route {
            self.respond(id, Err(ErrorCode::InitializeRepeated));
            return;
        }
        let root = match decode_params::<InitializeParams>(params.as_deref()) {
            Some(params) => match select_root(&params) {
                Ok(root) => root,
                Err(_) => {
                    self.lifecycle = restore_after_rejected_initialize();
                    self.respond(id, Err(ErrorCode::MalformedWorkspaceRoot));
                    return;
                }
            },
            None => {
                self.lifecycle = restore_after_rejected_initialize();
                self.respond(id, Err(ErrorCode::MalformedInitializeParams));
                return;
            }
        };
        self.root = root;
        // Receipt-gated: the lifecycle advances on the delivery receipt, not at handoff.
        // The initialize response is a fixed small frame, so a serialization failure is a
        // defect and fail-stops.
        let response = Outbound::Result {
            id: id.clone(),
            result: ResponseResult::Initialize(Box::new(initialize_result())),
        };
        if !self.hand_off(&response, FrameOwner::Initialize(id)) {
            self.terminate(1);
        }
    }

    fn on_shutdown(&mut self, id: RequestId) {
        match self.lifecycle.on_shutdown() {
            RequestGate::Route => {
                let response = Outbound::Result {
                    id: id.clone(),
                    result: ResponseResult::Null,
                };
                if !self.hand_off(&response, FrameOwner::Shutdown(id)) {
                    self.terminate(1);
                }
            }
            RequestGate::NotInitialized => self.respond(id, Err(ErrorCode::ServerNotInitialized)),
            RequestGate::InvalidInPhase => self.respond(id, Err(ErrorCode::InvalidInPhase)),
        }
    }

    fn on_semantic_request(
        &mut self,
        id: RequestId,
        parsed: Result<(SemanticQuery, lsp_types::Uri), MalformedParams>,
    ) {
        if self.lifecycle.gate_request() != RequestGate::Route {
            self.respond(id, Err(ErrorCode::ServerNotInitialized));
            return;
        }
        // An unavailable project makes every semantic request the same fixed -32803.
        if !self.ledger.all_available() {
            self.respond(id, Err(ErrorCode::CaptureUnavailable));
            return;
        }
        let Some(root) = self.root.clone() else {
            self.respond(id, Err(ErrorCode::NoWorkspaceRoot));
            return;
        };
        // The query was decoded to fixed-size fields at admission; binding it to a
        // document key here is what a held query retains, never the raw params.
        let key = parsed.ok().and_then(|(query, uri)| {
            Some((query, DocumentKey::from_uri(uri.as_str(), &root).ok()?))
        });
        let Some((query, key)) = key else {
            self.respond(id, Err(ErrorCode::MalformedParams));
            return;
        };
        let Some(DocumentState::OpenText { version, .. }) = self.ledger.get(&key) else {
            self.respond(id, Err(ErrorCode::ContentModified));
            return;
        };
        let held = HeldQuery {
            id,
            query,
            revision: self.current_revision,
            key,
            version: *version,
        };
        // Every semantic reply is deferred as a held query and served only against
        // available outbound credit, so a burst of requests cannot materialize a burst of
        // possibly large reply frames.
        self.held_queries.push(held);
        self.serve_ready_queries();
    }

    /// Answer a held query, reauthorizing its revision and document version. A changed
    /// revision or document state is `-32801 ContentModified`; the success encoder is
    /// never invoked in that case, so no reply can carry facts for text the client has
    /// already replaced.
    fn answer_held(&mut self, held: HeldQuery) {
        let doc_ok = matches!(
            self.ledger.get(&held.key),
            Some(DocumentState::OpenText { version, .. }) if *version == held.version
        );
        if held.revision != self.current_revision || !doc_ok {
            self.respond(held.id, Err(ErrorCode::ContentModified));
            return;
        }
        let answer = match &self.analysis {
            CurrentAnalysis::Ready(snapshot) => self.answer_ready(snapshot, &held),
            CurrentAnalysis::ResourceLimited(_) => Err(ErrorCode::AnalysisResourceLimit),
            CurrentAnalysis::Pending => {
                // No result for this revision yet: the query stays held until one lands.
                self.held_queries.push(held);
                return;
            }
        };
        self.respond(held.id, answer);
    }

    /// The reply for a held query against the ready snapshot. The document is resolved
    /// from the bound key, never a re-parsed URI; a document no longer open is
    /// `ContentModified`.
    fn answer_ready(
        &self,
        snapshot: &AnalysisSnapshot,
        held: &HeldQuery,
    ) -> Result<ResponseResult, ErrorCode> {
        let root = self.root.as_ref().ok_or(ErrorCode::InternalError)?;
        let (identity, source) = self
            .resolve_by_key(&held.key)
            .ok_or(ErrorCode::ContentModified)?;
        let target_uri = |file: &ProjectFile| lsp_uri(root, &self.origins, &key_of(file));
        held.query
            .answer(snapshot, &identity, source, target_uri)
            .map_err(|refusal| match refusal {
                QueryRefusal::ResourceLimit => ErrorCode::AnalysisResourceLimit,
                QueryRefusal::Internal => ErrorCode::InternalError,
            })
    }

    /// The file identity and current open text for a document key, if it is still an open
    /// text document.
    fn resolve_by_key(&self, key: &DocumentKey) -> Option<(ProjectFile, &str)> {
        let Some(DocumentState::OpenText { text, .. }) = self.ledger.get(key) else {
            return None;
        };
        let (identity, _) = marrow_project_fs::FileIdentity::validate(key.relative()).ok()?;
        Some((ProjectFile::new(key.origin().clone(), identity), text))
    }

    fn on_notification(&mut self, method: &str, params: Option<Box<serde_json::value::RawValue>>) {
        match method {
            "initialized" if self.lifecycle.on_initialized() => self.enter_running(),
            "initialized" => {}
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
        let Some(params) = decode_params::<DidOpenTextDocumentParams>(params.as_deref()) else {
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
        self.analysis = CurrentAnalysis::Pending;
        self.admit_document(&root, key, document.version, document.text);
    }

    fn on_did_change(&mut self, params: Option<Box<serde_json::value::RawValue>>) {
        if self.lifecycle.phase() != Phase::Running {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(params) = decode_params::<DidChangeTextDocumentParams>(params.as_deref()) else {
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
        self.analysis = CurrentAnalysis::Pending;
        self.admit_document(&root, key, version, change.text);
    }

    fn on_did_close(&mut self, params: Option<Box<serde_json::value::RawValue>>) {
        if self.lifecycle.phase() != Phase::Running {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(params) = decode_params::<DidCloseTextDocumentParams>(params.as_deref()) else {
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
        self.analysis = CurrentAnalysis::Pending;
        self.ledger.remove(&key);
        // Closing commits the removal and revision, but recapture waits until every
        // remaining open entry is available.
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
        let text: Arc<str> = Arc::from(text);
        let state = match validate_overlay(root, &self.candidate_overlay(&key, &text)) {
            Ok(()) => DocumentState::OpenText { version, text },
            Err(evidence) => DocumentState::OpenUnavailable {
                version,
                failure: evidence.unwrap_or_else(unrenderable_overlay_evidence),
            },
        };
        let available = state.is_text();
        self.ledger.insert(key, state);
        if available && self.ledger.all_available() {
            self.maybe_recompute(root);
        }
    }

    /// The full candidate overlay, borrowed: every other open text entry plus this key's
    /// new body. The changed key's prior state (text or unavailable) is excluded so the
    /// candidate body is the one under validation.
    fn candidate_overlay<'a>(
        &'a self,
        changed: &'a DocumentKey,
        body: &'a str,
    ) -> Vec<OverlayInput<'a>> {
        self.ledger
            .text_entries()
            .filter(|(key, _)| *key != changed)
            .map(|(key, text)| OverlayInput {
                key: key.relative(),
                bytes: text.as_bytes(),
            })
            .chain(std::iter::once(OverlayInput {
                key: changed.relative(),
                bytes: body.as_bytes(),
            }))
            .collect()
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
        let overlay = self
            .ledger
            .text_entries()
            .map(|(key, text)| (key.relative().to_owned(), Arc::clone(text)))
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
                    self.origins = OriginRoots::of(snapshot.input());
                    self.analysis = CurrentAnalysis::Ready(snapshot);
                    self.begin_publication();
                    self.serve_ready_queries();
                }
            }
            AnalysisOutcome::Capture(rejection) => {
                if rejection.revision == self.current_revision {
                    self.on_background_capture_failure(rejection);
                }
            }
            AnalysisOutcome::ResourceLimit { revision, limit } => {
                if revision == self.current_revision
                    && !matches!(self.analysis, CurrentAnalysis::ResourceLimited(_))
                {
                    self.analysis = CurrentAnalysis::ResourceLimited(limit);
                    self.begin_publication();
                    self.serve_ready_queries();
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

    /// Answer held queries while analysis for the current revision is complete and
    /// outbound credit is available. A potentially large reply is only ever materialized
    /// against free credit, so replies never enter the pending-frame queue; the
    /// unanswered remainder stays held at fixed-size cost, bounded by the request ledger.
    fn serve_ready_queries(&mut self) {
        while !matches!(self.analysis, CurrentAnalysis::Pending) && self.outbound.capacity() > 0 {
            let Some(query) = self.held_queries.pop() else {
                break;
            };
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
                    self.send_show_message(show_message(&evidence));
                }
            }
        }
    }

    // ---- publication (exclusive) ----

    fn begin_publication(&mut self) {
        // Publication exclusivity: only one plan is in flight at a time. A newer result
        // while a plan is in flight waits (latest-wins) and derives its tombstones only
        // after the prior final receipt.
        if self.publication.is_some() {
            self.pending_publication = Some(self.current_revision);
            return;
        }
        let (frames, new_published, observed_episode) = match &self.analysis {
            CurrentAnalysis::Ready(snapshot) => {
                let (frames, published) = self.plan_publication(snapshot);
                let observed = match self.episode {
                    CaptureEpisode::Latched { episode, .. } => Some(episode),
                    CaptureEpisode::Eligible => None,
                };
                (frames, published, observed)
            }
            CurrentAnalysis::ResourceLimited(limit) => {
                (self.plan_resource_stop(limit), Vec::new(), None)
            }
            CurrentAnalysis::Pending => return,
        };
        // Pre-encode the whole set fallibly before committing anything. A serialization
        // failure, or a plan whose retained bytes exceed the publication-plan bound,
        // fail-stops with the delivered ledger and capture episode unchanged.
        let mut encoded = VecDeque::new();
        let mut retained: u64 = 0;
        for outbound in &frames {
            let Ok(bytes) = encode(outbound) else {
                self.terminate(1);
                return;
            };
            retained = retained.saturating_add(bytes.len() as u64);
            if retained > MAX_PUBLICATION_PLAN_BYTES {
                self.terminate(1);
                return;
            }
            encoded.push_back(bytes);
        }
        // Commit the ledger only after every frame encodes.
        self.published = new_published;
        if encoded.is_empty() {
            // A zero-frame commit has no delivery to await: the episode resets now.
            self.reset_episode_if_observed(observed_episode);
            return;
        }
        // The in-flight plan keeps the next plan from reading the ledger until this
        // plan's final delivery receipt.
        self.publication = Some(PublicationState {
            pending: encoded,
            in_flight_count: 0,
            observed_episode,
        });
        self.feed_publication();
    }

    /// Compute the complete publication set without committing it: every current file's
    /// diagnostic list (including empties) plus an empty tombstone for every previously
    /// published file absent from the snapshot, and the new delivered-ledger key set. The
    /// delivered ledger is read but not mutated.
    fn plan_publication(&self, snapshot: &AnalysisSnapshot) -> (Vec<Outbound>, Vec<DocumentKey>) {
        let Some(root) = self.root.clone() else {
            return (Vec::new(), Vec::new());
        };
        let mut frames = Vec::new();
        let mut new_published = Vec::new();
        let origins = OriginRoots::of(snapshot.input());
        for module in snapshot.input().modules() {
            let identity = module.identity();
            let key = DocumentKey::captured(module.origin(), identity);
            let file = ProjectFile::from(module);
            // Only the root project's files are ever open, so a dependency file is
            // always published unversioned.
            let version = self.ledger.get(&key).map(DocumentState::version);
            let Some(uri) = lsp_uri(&root, &origins, &key) else {
                continue;
            };
            if let Ok(params) =
                facts::diagnostics_for_file(snapshot, uri, &file, module.source(), version)
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
            .map(|module| DocumentKey::captured(module.origin(), module.identity()))
            .collect();
        for key in &self.published {
            if !snapshot_keys.contains(key)
                && let Some(retraction) = Self::diagnostic_retraction(&root, &origins, key, None)
            {
                frames.push(retraction);
            }
        }
        (frames, new_published)
    }

    /// A stopped analysis has no source diagnostic set. Publish its canonical unlocated
    /// explanation and retract only the files whose prior diagnostics were nonempty.
    fn plan_resource_stop(&self, limit: &AnalysisResourceLimit) -> Vec<Outbound> {
        let mut frames = vec![Outbound::ShowMessage {
            typ: MessageType::Error,
            message: format!("Analysis stopped: {}.", limit.description()),
        }];
        if let Some(root) = &self.root {
            for key in &self.published {
                let version = self.ledger.get(key).map(DocumentState::version);
                if let Some(retraction) =
                    Self::diagnostic_retraction(root, &self.origins, key, version)
                {
                    frames.push(retraction);
                }
            }
        }
        frames
    }

    fn diagnostic_retraction(
        root: &SelectedRoot,
        origins: &OriginRoots,
        key: &DocumentKey,
        version: Option<i32>,
    ) -> Option<Outbound> {
        let uri = lsp_uri(root, origins, key)?;
        Some(Outbound::PublishDiagnostics(Box::new(
            lsp_types::PublishDiagnosticsParams {
                uri,
                diagnostics: Vec::new(),
                version,
            },
        )))
    }

    /// Move pre-encoded publication frames into the outbound path while credit allows,
    /// so publication retention stays bounded by the plan buffer and never floods the
    /// waiting queue past `W`.
    fn feed_publication(&mut self) {
        if let Some(state) = self.publication.as_mut() {
            state.in_flight_count += self
                .outbound
                .feed(&mut state.pending, &FrameOwner::Publication);
        }
    }

    fn on_publication_receipt(&mut self) {
        if let Some(state) = self.publication.as_mut() {
            state.in_flight_count = state.in_flight_count.saturating_sub(1);
        }
        self.feed_publication();
        let done = self
            .publication
            .as_ref()
            .is_some_and(|state| state.pending.is_empty() && state.in_flight_count == 0);
        if done {
            // The whole committed set is delivered: the next plan may build, and the
            // episode resets only if the commit observed the still-current latch.
            let state = self.publication.take().expect("publication present");
            self.reset_episode_if_observed(state.observed_episode);
            if self
                .pending_publication
                .take()
                .is_some_and(|revision| revision == self.current_revision)
            {
                self.begin_publication();
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

    /// Encode and hand off one frame against outbound credit. Returns whether the frame
    /// was handed off. A pre-handoff encode failure emits zero bytes and returns `false`,
    /// so the caller reconciles its own bookkeeping rather than stranding it. The frame is
    /// retained until its delivery receipt, whether written immediately or queued.
    #[must_use]
    fn hand_off(&mut self, outbound: &Outbound, owner: FrameOwner) -> bool {
        let Ok(bytes) = encode(outbound) else {
            return false;
        };
        if let Some(id) = owner.owned_id() {
            self.requests.set_awaiting(id);
        }
        self.outbound.admit(bytes, owner);
        true
    }

    /// Hand off a known-id response: a result, or the error named by its code. On a
    /// pre-handoff encode failure the entry gets exactly one fixed same-id `-32603`
    /// fallback — already internal-error class, so it takes no further fallback — and
    /// fail-stops if even that cannot encode. A dropped response is therefore never a
    /// silent no-reply.
    fn respond(&mut self, id: RequestId, body: Result<ResponseResult, ErrorCode>) {
        let outbound = match body {
            Ok(result) => Outbound::Result {
                id: id.clone(),
                result,
            },
            Err(code) => Outbound::Error {
                id: Some(id.clone()),
                code,
            },
        };
        if self.hand_off(&outbound, FrameOwner::Request(id.clone())) {
            return;
        }
        let fallback = Outbound::Error {
            id: Some(id.clone()),
            code: ErrorCode::InternalError,
        };
        if !self.hand_off(&fallback, FrameOwner::Request(id)) {
            self.terminate(1);
        }
    }

    fn reserve_and_error(&mut self, id: RequestId, code: ErrorCode) {
        if !self.requests.reserve(id.clone()) {
            self.terminate(1);
            return;
        }
        self.respond(id, Err(code));
    }

    fn send_null_error(&mut self, code: ErrorCode) {
        if self.anonymous_slots >= MAX_ANONYMOUS_ERROR_SLOTS {
            // Anonymous-slot exhaustion is the same zero-response terminal outcome.
            self.terminate(1);
            return;
        }
        self.anonymous_slots += 1;
        // A null-id protocol error has no recursive fallback: a pre-handoff encode failure
        // is a fixed `OutboundEncodingFailed` terminal, and the reserved slot is released.
        if !self.hand_off(&Outbound::Error { id: None, code }, FrameOwner::Anonymous) {
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
        let Some(owner) = self.outbound.receipt() else {
            return;
        };
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
        // The freed credit is offered in priority order: the in-flight publication set,
        // then a ready held query, then a queued small frame.
        self.feed_publication();
        self.serve_ready_queries();
        self.outbound.admit_waiting();
    }

    // ---- terminal ----

    fn on_terminal(&mut self) {
        // First-wins terminal: every unretired request is classified exactly once.
        let awaiting: Vec<RequestId> = self
            .outbound
            .undelivered_owners()
            .filter_map(|owner| owner.owned_id().cloned())
            .collect();
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

/// The fallback unavailable-evidence when even the bounded operational message overflows
/// its sink. Overlay refusal messages are short and cannot reach the cap, so this is a
/// defensive floor rather than a reachable path.
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

/// The document key naming one captured file.
fn key_of(file: &ProjectFile) -> DocumentKey {
    DocumentKey::captured(file.origin(), file.identity())
}

fn lsp_uri(
    root: &SelectedRoot,
    origins: &OriginRoots,
    key: &DocumentKey,
) -> Option<lsp_types::Uri> {
    use std::str::FromStr;
    lsp_types::Uri::from_str(&crate::uri::document_uri(root, origins, key)?).ok()
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
            // Trigger characters are editor ergonomics only: the checker classifies the
            // position from the source, never from the trigger character. The completion
            // surface is a complete list the client filters, so no `resolveProvider`.
            completion_provider: Some(CompletionOptions {
                trigger_characters: Some(vec![
                    ".".to_owned(),
                    ":".to_owned(),
                    "(".to_owned(),
                    ",".to_owned(),
                ]),
                ..Default::default()
            }),
            signature_help_provider: Some(SignatureHelpOptions {
                trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
                retrigger_characters: None,
                work_done_progress_options: Default::default(),
            }),
            document_symbol_provider: Some(OneOf::Left(true)),
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
    use crate::position::{LineMap, after};
    use crate::scratch::{self, TempDir};
    use std::fs;
    use std::path::Path;

    /// The advertised capabilities are pinned exactly, so any change to the wire contract
    /// is a reviewed decision rather than an incidental serialization.
    #[test]
    fn capabilities_advertisement_is_pinned() {
        let capabilities = initialize_result().capabilities;
        let json = serde_json::to_string(&capabilities).unwrap();
        assert_eq!(
            json,
            r#"{"textDocumentSync":{"openClose":true,"change":1},"hoverProvider":true,"completionProvider":{"triggerCharacters":[".",":","(",","]},"signatureHelpProvider":{"triggerCharacters":["(",","]},"definitionProvider":true,"documentSymbolProvider":true,"documentFormattingProvider":true}"#
        );
    }

    // ---- test scaffolding: drive the pure coordinator with deterministic events ----

    fn temp_project(tag: &str, main: &str) -> TempDir {
        TempDir::project(&format!("server-{tag}"), main)
    }

    fn root_uri(dir: &Path) -> String {
        scratch::uri_of(dir)
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
            .outbound
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

    /// The ledger key for the `src/main.mw` that `open_body` opens.
    fn main_key(dir: &Path) -> DocumentKey {
        let root = SelectedRoot::from_uri(&root_uri(dir)).expect("temp project root uri");
        DocumentKey::from_uri(&format!("{}/src/main.mw", root_uri(dir)), &root)
            .expect("main.mw is inside the selected root")
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

    /// A position-addressed semantic request against `src/main.mw`.
    fn position_body(dir: &Path, id: i64, method: &str, position: lsp_types::Position) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/{method}","params":{{"textDocument":{{"uri":"{}/src/main.mw"}},"position":{{"line":{},"character":{}}}}}}}"#,
            root_uri(dir),
            position.line,
            position.character
        )
    }

    fn hover_body(dir: &Path, id: i64, line: u32, character: u32) -> String {
        position_body(dir, id, "hover", lsp_types::Position::new(line, character))
    }

    fn completion_body(dir: &Path, id: i64, line: u32, character: u32) -> String {
        position_body(
            dir,
            id,
            "completion",
            lsp_types::Position::new(line, character),
        )
    }

    /// The library the dependency fixtures reach through a `[dependencies]` alias.
    const GRAPH_TEXT: &str =
        include_str!("../../../fixtures/v01/conformance/graph_report_lib/src/text.mw");

    /// A workspace whose project declares one local dependency nested inside it, so both
    /// trees sit under the one selected root and the server must tell them apart by
    /// origin rather than by containment.
    fn temp_dependency_project(tag: &str, main: &str) -> TempDir {
        let dir = temp_project(tag, main);
        dir.write("lib/graphtext/marrow.toml", "edition = \"2026\"\n");
        dir.write("lib/graphtext/src/text.mw", GRAPH_TEXT);
        dir.write(
            "marrow.toml",
            "edition = \"2026\"\n\n[dependencies]\ngraphtext = { path = \"lib/graphtext\" }\n",
        );
        dir
    }

    /// A running coordinator with its first analysis delivered and `main` open at version
    /// 1, whose analysis result has arrived but not yet been delivered.
    fn opened(dir: &Path, main: &str) -> Coordinator {
        let mut coordinator = running(dir);
        coordinator.outbound.outbox.clear();
        let initial = run_next_job(&mut coordinator);
        coordinator.on_worker_result(initial);
        deliver_frames(&mut coordinator);
        coordinator.on_frame(open_body(dir, 1, main).as_bytes());
        let outcome = run_next_job(&mut coordinator);
        assert!(matches!(outcome, AnalysisOutcome::Snapshot(_)));
        coordinator.on_worker_result(outcome);
        coordinator
    }

    fn cleanup(dir: &Path) {
        fs::remove_dir_all(dir).ok();
    }

    // ---- Law: receipt-gated initialize delivery ----

    #[test]
    fn initialize_response_delivery_gates_lifecycle_and_first_analysis() {
        let dir = temp_project("init", "module main\n");
        let mut coordinator = Coordinator::new();
        coordinator.on_frame(initialize_body(&root_uri(&dir)).as_bytes());
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

    #[test]
    fn latched_initialized_holds_followup_ingress_until_delivery() {
        let main = "module main\n\npub fn f(): int {\n    return 1\n}\n";
        let dir = temp_project("init-ingress", main);
        let mut coordinator = Coordinator::new();
        coordinator.on_frame(initialize_body(&root_uri(&dir)).as_bytes());
        let (ingress_tx, ingress_rx) = sync_channel(1);

        ingress_tx
            .send(ReaderEvent::Frame(
                br#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#.to_vec(),
            ))
            .unwrap();
        assert!(receive_ingress(&mut coordinator, &ingress_rx));
        assert_eq!(
            coordinator.lifecycle.phase(),
            Phase::InitializeReplyPending {
                initialized_latched: true
            }
        );

        ingress_tx
            .send(ReaderEvent::Frame(open_body(&dir, 1, main).into_bytes()))
            .unwrap();
        assert!(
            !receive_ingress(&mut coordinator, &ingress_rx),
            "follow-up ingress waits for initialize delivery"
        );
        assert!(
            coordinator.ledger.get(&main_key(&dir)).is_none(),
            "didOpen remains queued"
        );

        coordinator.on_receipt();
        assert_eq!(coordinator.lifecycle.phase(), Phase::Running);
        assert!(receive_ingress(&mut coordinator, &ingress_rx));
        assert!(
            coordinator.ledger.get(&main_key(&dir)).is_some(),
            "didOpen is admitted after delivery"
        );
        cleanup(&dir);
    }

    // ---- Law: a lost producer thread is terminal, never a hang ----

    #[test]
    fn a_lost_worker_is_terminal() {
        let mut coordinator = Coordinator::new();
        let (_ingress_tx, ingress_rx) = sync_channel(1);
        let (result_tx, result_rx) = sync_channel(1);
        let (_receipt_tx, receipt_rx) = sync_channel(1);
        let (_wake_tx, wake_rx) = sync_channel(1);
        let (work_tx, _work_rx) = sync_channel(1);
        let (frame_tx, _frame_rx) = sync_channel(1);
        // The worker unwound: its result sender is gone while every other thread lives.
        drop(result_tx);
        let exit = drive(
            &mut coordinator,
            &ingress_rx,
            &result_rx,
            &receipt_rx,
            &wake_rx,
            &work_tx,
            &frame_tx,
        );
        assert_eq!(exit, 1, "a lost worker ends the server nonzero");
        assert!(!coordinator.running);
    }

    // ---- Law: shared live-entry budget and IngressOverload N/N+1 ----

    #[test]
    fn live_entry_budget_admits_n_and_overloads_n_plus_1() {
        // At the shipped bound: N distinct requests reserve; N+1 fails closed with a
        // fixed terminal and no response.
        let mut coordinator = Coordinator::new();
        for id in 1..=MAX_LIVE_REQUEST_ENTRIES {
            coordinator.on_frame(
                format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"noSuchMethod"}}"#).as_bytes(),
            );
        }
        assert_eq!(coordinator.requests.entries.len(), MAX_LIVE_REQUEST_ENTRIES);
        assert!(coordinator.running, "still viable at N");
        let frames_before = coordinator.outbound.outbox.len();

        let over = MAX_LIVE_REQUEST_ENTRIES + 1;
        coordinator.on_frame(
            format!(r#"{{"jsonrpc":"2.0","id":{over},"method":"noSuchMethod"}}"#).as_bytes(),
        );
        assert!(!coordinator.running, "IngressOverload fail-stops at N+1");
        assert_eq!(
            coordinator.outbound.outbox.len(),
            frames_before,
            "the overloaded request emits no response"
        );
    }

    #[test]
    fn duplicate_live_id_consumes_no_entry_and_gets_null_error() {
        let mut coordinator = Coordinator::new();
        coordinator.on_frame(br#"{"jsonrpc":"2.0","id":7,"method":"noSuchMethod"}"#);
        assert_eq!(coordinator.requests.entries.len(), 1);
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
        let mut coordinator = Coordinator::new();
        for _ in 0..MAX_ANONYMOUS_ERROR_SLOTS {
            coordinator.on_frame(b"{ not json");
        }
        assert!(coordinator.running, "still viable at the anonymous bound");
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

        coordinator.on_frame(hover_body(&dir, 10, 3, 12).as_bytes());
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

        coordinator.on_frame(change_body(&dir, 2, main2).as_bytes());
        assert_ne!(coordinator.current_revision, rev_open);
        coordinator.job_out = None;

        // The snapshot lands for the new revision, so the hover held at the stale one
        // reauthorizes and fails.
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

    #[test]
    fn completion_held_across_edit_is_content_modified() {
        let main1 = "module main\n\npub fn f(): int {\n    return 1\n}\n";
        let main2 = "module main\n\npub fn f(): int {\n    return 2\n}\n";
        let dir = temp_project("cm-completion", main1);
        let mut coordinator = running(&dir);
        coordinator.job_out = None;
        coordinator.on_frame(open_body(&dir, 1, main1).as_bytes());
        coordinator.job_out = None;
        let rev_open = coordinator.current_revision;

        // Completion with no snapshot yet: held at rev_open, version 1.
        coordinator.on_frame(completion_body(&dir, 21, 3, 12).as_bytes());
        assert_eq!(coordinator.held_queries.len(), 1);

        coordinator.on_frame(change_body(&dir, 2, main2).as_bytes());
        assert_ne!(coordinator.current_revision, rev_open);
        coordinator.job_out = None;

        // The snapshot lands for the new revision, so the completion held at the stale one
        // reauthorizes and fails rather than answering with facts for the wrong text.
        let snapshot = snapshot_at(&dir, main2, coordinator.current_revision);
        coordinator.on_worker_result(AnalysisOutcome::Snapshot(snapshot));
        assert!(
            frames(&coordinator)
                .iter()
                .any(|f| f.contains(r#""id":21"#) && f.contains("-32801")),
            "the held completion is answered with ContentModified"
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

    // ---- Law: whole-analysis resource stops complete their exact revision ----

    const TYPE_ERROR: &str = "module main\n\npub fn f(): int {\n    return true\n}\n";

    fn run_next_job(coordinator: &mut Coordinator) -> AnalysisOutcome {
        let job = coordinator.job_out.take().expect("analysis job dispatched");
        let overlay: Vec<_> = job
            .overlay
            .iter()
            .map(|(key, text)| OverlayInput {
                key,
                bytes: text.as_bytes(),
            })
            .collect();
        run_analysis(&job.root, &overlay, job.revision)
    }

    fn deliver_frames(coordinator: &mut Coordinator) -> Vec<String> {
        let mut delivered = Vec::new();
        while let Some(frame) = coordinator.outbound.outbox.pop_front() {
            delivered.push(String::from_utf8(frame).expect("encoded UTF-8 frame"));
            coordinator.on_receipt();
        }
        delivered
    }

    fn notifications(messages: &[String], expected: &str) -> Vec<Box<serde_json::value::RawValue>> {
        messages
            .iter()
            .filter_map(|message| match decode(message.as_bytes()) {
                Inbound::Notification { method, params } if method == expected => {
                    Some(params.expect("notification parameters"))
                }
                _ => None,
            })
            .collect()
    }

    fn diagnostic_publications(messages: &[String]) -> Vec<lsp_types::PublishDiagnosticsParams> {
        notifications(messages, "textDocument/publishDiagnostics")
            .iter()
            .map(|params| serde_json::from_str(params.get()).expect("diagnostic parameters"))
            .collect()
    }

    fn diagnosed_coordinator(dir: &Path) -> Coordinator {
        let mut coordinator = running(dir);
        // `running` already delivered initialization; remove its retained test frame.
        coordinator.outbound.outbox.clear();
        let initial = run_next_job(&mut coordinator);
        coordinator.on_worker_result(initial);
        deliver_frames(&mut coordinator);

        coordinator.on_frame(open_body(dir, 1, TYPE_ERROR).as_bytes());
        let outcome = run_next_job(&mut coordinator);
        let AnalysisOutcome::Snapshot(snapshot) = &outcome else {
            panic!("ordinary type error produces a complete snapshot");
        };
        assert!(
            snapshot
                .diagnostics()
                .iter()
                .any(|d| d.code() == marrow_codes::Code::CheckType)
        );
        coordinator.on_worker_result(outcome);
        let delivered = deliver_frames(&mut coordinator);
        let publications = diagnostic_publications(&delivered);
        assert!(publications.iter().any(|params| {
            params.uri.as_str() == format!("{}/src/main.mw", root_uri(dir))
                && params.version == Some(1)
                && !params.diagnostics.is_empty()
        }));
        assert!(!coordinator.published.is_empty());
        assert!(
            coordinator.publication.is_none(),
            "prior diagnostics delivered"
        );
        coordinator
    }

    fn syntax_stop(coordinator: &mut Coordinator, dir: &Path, version: i64) -> AnalysisOutcome {
        let source = "@\n".repeat(marrow_syntax::SYNTAX_DIAGNOSTIC_COUNT_LIMIT + 1);
        coordinator.on_frame(change_body(dir, version, &source).as_bytes());
        let outcome = run_next_job(coordinator);
        assert!(matches!(
            outcome,
            AnalysisOutcome::ResourceLimit { revision, .. }
                if revision == coordinator.current_revision
        ));
        outcome
    }

    #[test]
    fn analysis_resource_limit_fails_held_queries() {
        let dir = temp_project("reslimit-held", TYPE_ERROR);
        let mut coordinator = diagnosed_coordinator(&dir);
        let outcome = syntax_stop(&mut coordinator, &dir, 2);

        coordinator.on_frame(hover_body(&dir, 8, 0, 0).as_bytes());
        assert_eq!(coordinator.held_queries.len(), 1);

        coordinator.on_worker_result(outcome);
        assert!(coordinator.held_queries.is_empty(), "held queries drain");
        assert!(
            frames(&coordinator)
                .iter()
                .any(|f| f.contains(r#""id":8"#) && f.contains("-32803")),
            "a held query at a resource-limited revision is -32803"
        );
        cleanup(&dir);
    }

    #[test]
    fn analysis_resource_limit_completes_late_current_queries() {
        let dir = temp_project("reslimit-late", TYPE_ERROR);
        let mut coordinator = diagnosed_coordinator(&dir);
        let outcome = syntax_stop(&mut coordinator, &dir, 2);
        coordinator.on_worker_result(outcome);
        deliver_frames(&mut coordinator);

        coordinator.on_frame(hover_body(&dir, 9, 0, 0).as_bytes());
        assert!(
            frames(&coordinator)
                .iter()
                .any(|frame| frame.contains(r#""id":9,"#) && frame.contains(r#""code":-32803,"#)),
            "a request admitted after the stop completes without another worker result"
        );
        assert!(coordinator.held_queries.is_empty());
        assert!(coordinator.requests.is_live(&RequestId::Integer(9)));
        deliver_frames(&mut coordinator);
        assert!(!coordinator.requests.is_live(&RequestId::Integer(9)));
        cleanup(&dir);
    }

    #[test]
    fn analysis_resource_limit_reauthorizes_queries_held_across_an_edit() {
        let dir = temp_project("reslimit-stale-query", TYPE_ERROR);
        let mut coordinator = diagnosed_coordinator(&dir);
        coordinator.on_frame(change_body(&dir, 2, TYPE_ERROR).as_bytes());
        let old_outcome = run_next_job(&mut coordinator);
        coordinator.on_frame(hover_body(&dir, 10, 3, 12).as_bytes());
        assert_eq!(coordinator.held_queries.len(), 1);

        let source = "@\n".repeat(marrow_syntax::SYNTAX_DIAGNOSTIC_COUNT_LIMIT + 1);
        coordinator.on_frame(change_body(&dir, 3, &source).as_bytes());
        coordinator.on_worker_result(old_outcome);
        let outcome = run_next_job(&mut coordinator);
        assert!(matches!(outcome, AnalysisOutcome::ResourceLimit { .. }));
        coordinator.on_worker_result(outcome);
        assert!(
            frames(&coordinator)
                .iter()
                .any(|frame| frame.contains(r#""id":10,"#) && frame.contains(r#""code":-32801,"#)),
            "the older request is ContentModified, not a failure for the new revision"
        );
        cleanup(&dir);
    }

    #[test]
    fn analysis_resource_limit_retracts_exactly_the_previously_published_files() {
        let dir = temp_project("reslimit-retract", TYPE_ERROR);
        fs::write(
            dir.join("src/other.mw"),
            "module other\npub fn g(): int { return false }\n",
        )
        .expect("second diagnostic file");
        let mut coordinator = diagnosed_coordinator(&dir);
        assert_eq!(coordinator.published.len(), 2);
        let outcome = syntax_stop(&mut coordinator, &dir, 2);
        coordinator.on_worker_result(outcome);
        let delivered = deliver_frames(&mut coordinator);
        let publications = diagnostic_publications(&delivered);
        assert_eq!(
            publications.len(),
            2,
            "every old diagnostic file is retracted"
        );
        assert!(
            publications
                .iter()
                .all(|params| params.diagnostics.is_empty())
        );
        let mut identities: Vec<_> = publications
            .iter()
            .map(|params| (params.uri.as_str().to_owned(), params.version))
            .collect();
        identities.sort();
        assert_eq!(
            identities,
            vec![
                (format!("{}/src/main.mw", root_uri(&dir)), Some(2)),
                (format!("{}/src/other.mw", root_uri(&dir)), None),
            ]
        );
        assert!(coordinator.published.is_empty());
        assert!(coordinator.publication.is_none());
        cleanup(&dir);
    }

    #[test]
    fn analysis_resource_limit_publishes_the_canonical_unlocated_explanation() {
        let dir = temp_project("reslimit-notice", TYPE_ERROR);
        let mut coordinator = diagnosed_coordinator(&dir);
        let outcome = syntax_stop(&mut coordinator, &dir, 2);
        coordinator.on_worker_result(outcome);
        let delivered = deliver_frames(&mut coordinator);
        let notices = notifications(&delivered, "window/showMessage");
        assert_eq!(notices.len(), 1, "the stop has one unlocated explanation");
        let params: lsp_types::ShowMessageParams =
            serde_json::from_str(notices[0].get()).expect("showMessage parameters");
        assert_eq!(params.typ, lsp_types::MessageType::ERROR);
        assert!(
            params
                .message
                .contains(marrow_compile::ResourceLimitKind::DiagnosticCount.description())
        );
        assert!(!notices[0].get().contains("\"uri\""));
        assert!(!notices[0].get().contains("\"range\""));
        cleanup(&dir);
    }

    #[test]
    fn analysis_resource_limit_exact_count_and_later_recovery_remain_complete() {
        let dir = temp_project("reslimit-boundary", TYPE_ERROR);
        let mut coordinator = diagnosed_coordinator(&dir);
        let exact = "@\n".repeat(marrow_syntax::SYNTAX_DIAGNOSTIC_COUNT_LIMIT);
        coordinator.on_frame(change_body(&dir, 2, &exact).as_bytes());
        let outcome = run_next_job(&mut coordinator);
        let AnalysisOutcome::Snapshot(snapshot) = &outcome else {
            panic!("the exact syntax diagnostic count retains a complete snapshot");
        };
        assert_eq!(
            snapshot.diagnostics().len(),
            marrow_syntax::SYNTAX_DIAGNOSTIC_COUNT_LIMIT
        );
        coordinator.on_worker_result(outcome);
        let delivered = deliver_frames(&mut coordinator);
        let publications = diagnostic_publications(&delivered);
        assert_eq!(publications.len(), 1);
        assert_eq!(publications[0].version, Some(2));
        assert_eq!(
            publications[0].diagnostics.len(),
            marrow_syntax::SYNTAX_DIAGNOSTIC_COUNT_LIMIT
        );
        assert!(notifications(&delivered, "window/showMessage").is_empty());

        let stopped = syntax_stop(&mut coordinator, &dir, 3);
        coordinator.on_worker_result(stopped);
        deliver_frames(&mut coordinator);
        let recovered = "module main\n\npub fn f(): int {\n    const n = 1\n    return n\n}\n";
        coordinator.on_frame(change_body(&dir, 4, recovered).as_bytes());
        coordinator.on_frame(hover_body(&dir, 11, 4, 11).as_bytes());
        assert_eq!(coordinator.held_queries.len(), 1);
        let outcome = run_next_job(&mut coordinator);
        let AnalysisOutcome::Snapshot(snapshot) = &outcome else {
            panic!("a later small revision recovers");
        };
        assert!(snapshot.diagnostics().is_empty());
        coordinator.on_worker_result(outcome);
        let delivered = deliver_frames(&mut coordinator);
        assert!(delivered.iter().any(|frame| {
            frame.contains(r#""id":11,"#)
                && frame.contains(r#""result":{"#)
                && frame.contains("int")
        }));
        let publications = diagnostic_publications(&delivered);
        assert_eq!(publications.len(), 1);
        assert_eq!(publications[0].version, Some(4));
        assert!(publications[0].diagnostics.is_empty());
        assert!(coordinator.held_queries.is_empty());
        cleanup(&dir);
    }

    #[test]
    fn analysis_resource_limit_from_an_older_job_cannot_replace_recovery() {
        let dir = temp_project("reslimit-old-result", TYPE_ERROR);
        let mut coordinator = diagnosed_coordinator(&dir);
        let stopped = syntax_stop(&mut coordinator, &dir, 2);
        let recovered = "module main\npub fn f(): int { return 1 }\n";
        coordinator.on_frame(change_body(&dir, 3, recovered).as_bytes());
        coordinator.on_worker_result(stopped);
        assert!(
            frames(&coordinator).is_empty(),
            "the older stop publishes nothing"
        );
        let outcome = run_next_job(&mut coordinator);
        assert!(matches!(outcome, AnalysisOutcome::Snapshot(_)));
        coordinator.on_worker_result(outcome);
        let delivered = deliver_frames(&mut coordinator);
        assert!(notifications(&delivered, "window/showMessage").is_empty());
        let publications = diagnostic_publications(&delivered);
        assert_eq!(publications.len(), 1);
        assert_eq!(publications[0].version, Some(3));
        assert!(publications[0].diagnostics.is_empty());
        cleanup(&dir);
    }

    #[test]
    fn analysis_resource_limit_waits_for_the_prior_publication_receipt() {
        let dir = temp_project("reslimit-pending", TYPE_ERROR);
        let mut coordinator = diagnosed_coordinator(&dir);
        let cleared = "module main\npub fn f(): int { return 1 }\n";
        coordinator.on_frame(change_body(&dir, 2, cleared).as_bytes());
        let outcome = run_next_job(&mut coordinator);
        coordinator.on_worker_result(outcome);
        assert!(coordinator.publication.is_some());
        let prior_frames = frames(&coordinator);

        let stopped = syntax_stop(&mut coordinator, &dir, 3);
        coordinator.on_worker_result(stopped);
        assert!(
            coordinator.pending_publication.is_some(),
            "the stop waits for delivery"
        );
        assert_eq!(frames(&coordinator), prior_frames);
        let delivered = deliver_frames(&mut coordinator);
        let publications = diagnostic_publications(&delivered);
        assert_eq!(
            publications.len(),
            1,
            "the prior plan already cleared the ledger"
        );
        assert_eq!(publications[0].version, Some(2));
        assert!(publications[0].diagnostics.is_empty());
        assert_eq!(notifications(&delivered, "window/showMessage").len(), 1);
        assert!(coordinator.publication.is_none());
        assert!(coordinator.pending_publication.is_none());
        cleanup(&dir);
    }

    #[test]
    fn analysis_resource_limit_pending_publication_is_dropped_after_an_edit() {
        let dir = temp_project("reslimit-pending-stale", TYPE_ERROR);
        let mut coordinator = diagnosed_coordinator(&dir);
        coordinator.on_frame(change_body(&dir, 2, TYPE_ERROR).as_bytes());
        let outcome = run_next_job(&mut coordinator);
        coordinator.on_worker_result(outcome);
        assert!(coordinator.publication.is_some());
        let prior_frames = frames(&coordinator);

        let stopped = syntax_stop(&mut coordinator, &dir, 3);
        coordinator.on_worker_result(stopped);
        assert!(
            coordinator.pending_publication.is_some(),
            "the stop waits for delivery"
        );
        coordinator.on_frame(change_body(&dir, 4, TYPE_ERROR).as_bytes());
        let delivered = deliver_frames(&mut coordinator);
        assert_eq!(
            delivered, prior_frames,
            "the stale stop never builds a plan"
        );
        assert!(coordinator.pending_publication.is_none());
        assert!(coordinator.publication.is_none());
        cleanup(&dir);
    }

    #[test]
    fn analysis_resource_limit_completes_all_six_semantic_request_kinds() {
        let dir = temp_project("reslimit-methods", TYPE_ERROR);
        let mut coordinator = diagnosed_coordinator(&dir);
        let mut stopped = Some(syntax_stop(&mut coordinator, &dir, 2));
        let position = r#", "position":{"line":0,"character":0}"#;
        let methods = [
            ("hover", position),
            ("definition", position),
            (
                "formatting",
                r#", "options":{"tabSize":4,"insertSpaces":true}"#,
            ),
            ("completion", position),
            ("signatureHelp", position),
            ("documentSymbol", ""),
        ];
        for batch in 0..2 {
            for (index, (method, extra)) in methods.iter().enumerate() {
                let id = 20 + batch * methods.len() + index;
                coordinator.on_frame(format!(
                    r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/{method}","params":{{"textDocument":{{"uri":"{}/src/main.mw"}}{extra}}}}}"#,
                    root_uri(&dir)
                ).as_bytes());
            }
            if let Some(outcome) = stopped.take() {
                assert_eq!(coordinator.held_queries.len(), methods.len());
                coordinator.on_worker_result(outcome);
            }
            let delivered = deliver_frames(&mut coordinator);
            for index in 0..methods.len() {
                let id = 20 + batch * methods.len() + index;
                assert!(delivered.iter().any(|frame| {
                    frame.contains(&format!("\"id\":{id},")) && frame.contains(r#""code":-32803,"#)
                }));
            }
            assert!(coordinator.held_queries.is_empty());
        }
        cleanup(&dir);
    }

    #[test]
    fn analysis_resource_limit_holds_credit_starved_replies_until_receipts() {
        let dir = temp_project("reslimit-credits", TYPE_ERROR);
        let mut coordinator = diagnosed_coordinator(&dir);
        let stopped = syntax_stop(&mut coordinator, &dir, 2);
        for id in 100..100 + crate::capacities::OUTBOUND_CREDITS {
            coordinator.on_frame(
                format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"noSuchMethod"}}"#).as_bytes(),
            );
        }
        assert_eq!(coordinator.outbound.capacity(), 0);
        coordinator.on_frame(hover_body(&dir, 20, 0, 0).as_bytes());
        coordinator.on_worker_result(stopped);
        coordinator.on_frame(hover_body(&dir, 21, 0, 0).as_bytes());
        assert_eq!(coordinator.held_queries.len(), 2);
        assert!(
            coordinator.outbound.pending.is_empty(),
            "no eager error frames"
        );
        let plan = coordinator.publication.as_ref().expect("bounded stop plan");
        assert_eq!(plan.in_flight_count, 0);
        assert_eq!(plan.pending.len(), 2);
        assert!(coordinator.requests.is_live(&RequestId::Integer(20)));
        assert!(coordinator.requests.is_live(&RequestId::Integer(21)));

        let delivered = deliver_frames(&mut coordinator);
        for id in [20, 21] {
            assert_eq!(
                delivered
                    .iter()
                    .filter(|frame| {
                        frame.contains(&format!("\"id\":{id},"))
                            && frame.contains(r#""code":-32803,"#)
                    })
                    .count(),
                1
            );
            assert!(!coordinator.requests.is_live(&RequestId::Integer(id)));
        }
        assert!(coordinator.held_queries.is_empty());
        assert!(coordinator.outbound.pending.is_empty());
        assert!(coordinator.publication.is_none());
        assert_eq!(notifications(&delivered, "window/showMessage").len(), 1);
        cleanup(&dir);
    }

    #[test]
    fn analysis_resource_limit_coalesces_repeated_and_pending_stops() {
        let dir = temp_project("reslimit-repeat", TYPE_ERROR);
        let mut coordinator = diagnosed_coordinator(&dir);
        let stopped = syntax_stop(&mut coordinator, &dir, 2);
        coordinator.on_worker_result(stopped);
        let source = "@\n".repeat(marrow_syntax::SYNTAX_DIAGNOSTIC_COUNT_LIMIT + 1);
        let root = selected_root(&dir);
        let overlay = [OverlayInput {
            key: "src/main.mw",
            bytes: source.as_bytes(),
        }];
        let duplicate = run_analysis(&root, &overlay, coordinator.current_revision);
        let first_frames = frames(&coordinator);
        coordinator.on_worker_result(duplicate);
        assert_eq!(frames(&coordinator), first_frames);
        assert!(coordinator.pending_publication.is_none());

        for version in [3, 4] {
            let stopped = syntax_stop(&mut coordinator, &dir, version);
            coordinator.on_worker_result(stopped);
        }
        assert_eq!(
            coordinator.pending_publication,
            Some(coordinator.current_revision)
        );
        let delivered = deliver_frames(&mut coordinator);
        assert_eq!(notifications(&delivered, "window/showMessage").len(), 2);
        let publications = diagnostic_publications(&delivered);
        assert_eq!(publications.len(), 1);
        assert_eq!(publications[0].version, Some(2));
        assert!(publications[0].diagnostics.is_empty());
        let duplicate = run_analysis(&root, &overlay, coordinator.current_revision);
        coordinator.on_worker_result(duplicate);
        assert!(
            frames(&coordinator).is_empty(),
            "a delivered stop is not republished"
        );
        let CurrentAnalysis::ResourceLimited(AnalysisResourceLimit::Compile(limit)) =
            &coordinator.analysis
        else {
            panic!("current typed stop retained")
        };
        assert_eq!(
            limit.kind(),
            marrow_compile::ResourceLimitKind::DiagnosticCount
        );
        cleanup(&dir);
    }

    #[test]
    fn analysis_resource_limit_preserves_the_capture_episode_until_success_delivers() {
        let dir = temp_project("reslimit-capture-episode", TYPE_ERROR);
        let mut coordinator = diagnosed_coordinator(&dir);
        fs::remove_file(dir.join("marrow.toml")).expect("remove disposable manifest");
        coordinator.on_frame(change_body(&dir, 2, TYPE_ERROR).as_bytes());
        let failure = run_next_job(&mut coordinator);
        assert!(matches!(failure, AnalysisOutcome::Capture(_)));
        coordinator.on_worker_result(failure);
        let episode = coordinator.episode;
        assert!(matches!(episode, CaptureEpisode::Latched { .. }));
        deliver_frames(&mut coordinator);
        fs::write(dir.join("marrow.toml"), "edition = \"2026\"\n")
            .expect("restore disposable manifest");

        let stopped = syntax_stop(&mut coordinator, &dir, 3);
        coordinator.on_worker_result(stopped);
        assert_eq!(
            coordinator
                .publication
                .as_ref()
                .expect("stop plan")
                .observed_episode,
            None
        );
        deliver_frames(&mut coordinator);
        assert_eq!(
            coordinator.episode, episode,
            "stop delivery does not reset capture"
        );
        coordinator.on_frame(change_body(&dir, 4, TYPE_ERROR).as_bytes());
        let recovered = run_next_job(&mut coordinator);
        coordinator.on_worker_result(recovered);
        assert_eq!(
            coordinator.episode, episode,
            "success still awaits delivery"
        );
        deliver_frames(&mut coordinator);
        assert_eq!(coordinator.episode, CaptureEpisode::Eligible);
        cleanup(&dir);
    }

    #[test]
    fn analysis_resource_limit_active_plan_finishes_before_newer_success() {
        let dir = temp_project("reslimit-active", TYPE_ERROR);
        let mut coordinator = diagnosed_coordinator(&dir);
        let stopped = syntax_stop(&mut coordinator, &dir, 2);
        coordinator.on_worker_result(stopped);
        let stop_frames = frames(&coordinator);
        let recovered = "module main\npub fn f(): int { return 1 }\n";
        coordinator.on_frame(change_body(&dir, 3, recovered).as_bytes());
        let outcome = run_next_job(&mut coordinator);
        coordinator.on_worker_result(outcome);
        assert_eq!(frames(&coordinator), stop_frames);
        assert_eq!(
            coordinator.pending_publication,
            Some(coordinator.current_revision)
        );
        let delivered = deliver_frames(&mut coordinator);
        let publications = diagnostic_publications(&delivered);
        assert_eq!(
            publications.iter().map(|p| p.version).collect::<Vec<_>>(),
            vec![Some(2), Some(3)]
        );
        assert!(publications.iter().all(|p| p.diagnostics.is_empty()));
        assert_eq!(notifications(&delivered, "window/showMessage").len(), 1);
        assert!(coordinator.publication.is_none());
        assert!(coordinator.pending_publication.is_none());
        cleanup(&dir);
    }

    #[test]
    fn query_local_completion_limit_preserves_ready_snapshot_and_diagnostics() {
        let mut main = String::from("module main\n\nstruct Big {\n");
        for index in 0..=marrow_compile::MAX_COMPLETION_CANDIDATES {
            main.push_str(&format!("    f{index:03}: int\n"));
        }
        main.push_str("}\n\nfn f(p: Big): int {\n    return p.\n}\n");
        let member_line = main
            .lines()
            .position(|line| line == "    return p.")
            .expect("incomplete member expression") as u32;
        let position = lsp_types::Position::new(member_line, "    return p.".len() as u32);
        let other = "module other\n\npub fn g(value: int): int {\n    return value\n}\n";
        let dir = temp_project("query-local-limit", &main);
        fs::write(dir.join("src/other.mw"), other).expect("valid sibling module");
        let mut coordinator = running(&dir);
        coordinator.outbound.outbox.clear();
        let initial = run_next_job(&mut coordinator);
        coordinator.on_worker_result(initial);
        deliver_frames(&mut coordinator);
        for open in [
            open_body(&dir, 1, &main),
            open_body(&dir, 1, other).replace("/src/main.mw", "/src/other.mw"),
        ] {
            coordinator.on_frame(open.as_bytes());
            let outcome = run_next_job(&mut coordinator);
            assert!(matches!(outcome, AnalysisOutcome::Snapshot(_)));
            coordinator.on_worker_result(outcome);
            let delivered = deliver_frames(&mut coordinator);
            assert!(
                diagnostic_publications(&delivered)
                    .iter()
                    .any(|params| !params.diagnostics.is_empty())
            );
        }
        let CurrentAnalysis::Ready(snapshot) = &coordinator.analysis else {
            panic!("incomplete member source retains a complete analysis snapshot");
        };
        let snapshot = Arc::clone(snapshot);
        let revision = coordinator.current_revision;
        let published = coordinator.published.clone();
        assert!(!published.is_empty());
        assert!(coordinator.publication.is_none());
        let (identity, _) = marrow_project_fs::FileIdentity::validate("src/main.mw")
            .expect("fixture file identity");
        assert!(matches!(
            facts::completion(&snapshot, &ProjectFile::root(identity), &main, position),
            Err(facts::ResourceLimited)
        ));

        // The sibling hover runs without an edit or another analysis result after the
        // real completion refusal. Neither query changes revision-wide publications.
        for (request, id, result) in [
            (
                completion_body(&dir, 20, position.line, position.character),
                20,
                r#""code":-32803"#,
            ),
            (
                hover_body(&dir, 21, 3, 11).replace("/src/main.mw", "/src/other.mw"),
                21,
                r#""value":"int""#,
            ),
        ] {
            coordinator.on_frame(request.as_bytes());
            let messages = frames(&coordinator);
            assert_eq!(messages.len(), 1, "exactly one response, no notifications");
            assert!(messages[0].contains(&format!(r#""id":{id}"#)));
            assert!(
                messages[0].contains(result),
                "request {id}: {}",
                messages[0]
            );
            assert!(matches!(
                &coordinator.analysis,
                CurrentAnalysis::Ready(current) if Arc::ptr_eq(current, &snapshot)
            ));
            assert_eq!(coordinator.current_revision, revision);
            assert_eq!(coordinator.published, published);
            assert!(coordinator.publication.is_none());
            assert!(coordinator.pending_publication.is_none());
            assert_eq!(
                coordinator.requests.entries.get(&RequestId::Integer(id)),
                Some(&ReqState::AwaitingDelivery)
            );
            assert_eq!(deliver_frames(&mut coordinator), messages);
            assert!(coordinator.requests.entries.is_empty());
            assert!(coordinator.job_out.is_none());
        }
        cleanup(&dir);
    }

    // ---- Law: dependency files are read-only and addressed at their own location ----

    /// A dependency's file is reported at its own location. The library sits at
    /// `lib/graphtext`, so its `src/text.mw` publishes under that directory rather than
    /// under the consuming project's `src`, where no such file exists. Identities are
    /// relative to their own tree; the origin is what places them.
    #[test]
    fn a_dependency_file_publishes_under_its_own_root() {
        let main = "module main\n\nuse graphtext::text\n\npub fn f(): int {\n    return 1\n}\n";
        let dir = temp_dependency_project("dependency-uri", main);
        let mut coordinator = opened(&dir, main);
        let publications = diagnostic_publications(&deliver_frames(&mut coordinator));
        let library = publications
            .iter()
            .find(|params| {
                params.uri.as_str() == format!("{}/lib/graphtext/src/text.mw", root_uri(&dir))
            })
            .expect("the library file publishes at its own location");
        assert_eq!(
            library.version, None,
            "a dependency file names no open document, so it publishes unversioned"
        );
        assert!(publications.iter().any(|params| {
            params.uri.as_str() == format!("{}/src/main.mw", root_uri(&dir))
                && params.version == Some(1)
        }));
    }

    /// A dependency file is read-only: opening one never places its body in the capture
    /// overlay, so the workspace keeps analysing. An overlay entry naming a file the root
    /// project does not declare refuses the whole capture, which is exactly what must not
    /// happen when a developer opens a library file to read it.
    #[test]
    fn opening_a_dependency_file_leaves_the_workspace_analysing() {
        let main = "module main\n\nuse graphtext::text\n\npub fn f(): int {\n    return 1\n}\n";
        let dir = temp_dependency_project("dependency-readonly", main);
        let mut coordinator = opened(&dir, main);
        deliver_frames(&mut coordinator);

        let library = open_body(&dir, 1, "module text\n\nthis is not Marrow source\n")
            .replace("/src/main.mw", "/lib/graphtext/src/text.mw");
        coordinator.on_frame(library.as_bytes());
        assert_eq!(
            coordinator.ledger.text_entries().count(),
            1,
            "the library file never enters the ledger"
        );
        assert!(
            coordinator.job_out.is_none(),
            "an ignored open dispatches nothing"
        );

        // The project's own edit still analyses, against the library's committed bytes.
        let edited = main.replace("return 1", "return 2");
        coordinator.on_frame(change_body(&dir, 2, &edited).as_bytes());
        let overlay: Vec<&str> = coordinator
            .job_out
            .as_ref()
            .expect("the edit dispatches a recompute")
            .overlay
            .iter()
            .map(|(key, _)| key.as_str())
            .collect();
        assert_eq!(overlay, ["src/main.mw"]);
        let outcome = run_next_job(&mut coordinator);
        assert!(matches!(outcome, AnalysisOutcome::Snapshot(_)));
        coordinator.on_worker_result(outcome);
        let publications = diagnostic_publications(&deliver_frames(&mut coordinator));
        assert!(publications.iter().any(|params| {
            params.uri.as_str() == format!("{}/src/main.mw", root_uri(&dir))
                && params.version == Some(2)
                && params.diagnostics.is_empty()
        }));
    }

    /// Definition from the application into a library function returns the library's
    /// own file URI. The target resolves through the existing definition fact: the
    /// boundary needs no new canonical fact, only the origin the snapshot carries.
    #[test]
    fn definition_across_a_dependency_boundary_names_the_library_file() {
        let main = "module main\n\nuse graphtext::text\n\npub fn f(): bool {\n    return text::startsWith(\"ab\", \"a\")\n}\n";
        let dir = temp_dependency_project("dependency-definition", main);
        let mut coordinator = opened(&dir, main);
        deliver_frames(&mut coordinator);

        let position = LineMap::new(main).position_at(after(main, "text::start"));
        coordinator.on_frame(position_body(&dir, 40, "definition", position).as_bytes());
        let delivered = deliver_frames(&mut coordinator);
        let reply = delivered
            .iter()
            .find(|frame| frame.contains(r#""id":40,"#))
            .expect("the definition is answered");
        assert!(
            reply.contains(&format!(
                r#""uri":"{}/lib/graphtext/src/text.mw""#,
                root_uri(&dir)
            )),
            "the definition names the library's own file: {reply}"
        );
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
        assert_eq!(coordinator.outbound.capacity(), 0);

        // A snapshot commits its publication plan but is credit-starved: the plan is in
        // flight with every frame still pending and none handed off.
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
        // starved publication rather than leaving the plan stalled forever.
        coordinator.on_receipt();
        assert!(
            coordinator
                .publication
                .as_ref()
                .is_some_and(|state| state.in_flight_count > 0),
            "a freed credit feeds the starved publication"
        );

        // Drain to completion: the plan leaves flight.
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
        assert_eq!(coordinator.outbound.capacity(), 0);
        let pending_before = coordinator.outbound.pending.len();

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
            coordinator.outbound.pending.len(),
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
        coordinator.on_frame(initialize_body(&root_uri(&dir)).as_bytes());
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

        // First snapshot: publication A builds and is the one plan in flight.
        let snapshot_a = snapshot_at(&dir, main1, rev1);
        coordinator.on_worker_result(AnalysisOutcome::Snapshot(snapshot_a));
        assert!(coordinator.publication.is_some());
        let frames_after_a = coordinator.outbound.outbox.len();

        // Advance the revision and deliver a newer snapshot while A is still in flight: it
        // must NOT build a second plan.
        coordinator.on_frame(change_body(&dir, 2, main2).as_bytes());
        coordinator.job_out = None;
        let rev2 = coordinator.current_revision;
        let snapshot_b = snapshot_at(&dir, main2, rev2);
        coordinator.on_worker_result(AnalysisOutcome::Snapshot(snapshot_b));
        assert!(
            coordinator.pending_publication.is_some(),
            "the newer set waits for the in-flight plan"
        );
        assert_eq!(
            coordinator.outbound.outbox.len(),
            frames_after_a,
            "no second plan frames while the first plan is in flight"
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

    #[test]
    fn pending_publication_is_dropped_after_a_newer_document_revision() {
        let main1 = "module main\n\npub fn f(): int {\n    return 1\n}\n";
        let main2 = "module main\n\npub fn f(): int {\n    return 2\n}\n";
        let main3 = "module main\n\npub fn f(): int {\n    return 3\n}\n";
        let dir = temp_project("pub-stale", main1);
        let mut coordinator = running(&dir);
        coordinator.job_out = None;
        coordinator.on_frame(open_body(&dir, 1, main1).as_bytes());
        coordinator.job_out = None;

        let snapshot1 = snapshot_at(&dir, main1, coordinator.current_revision);
        coordinator.on_worker_result(AnalysisOutcome::Snapshot(snapshot1));
        assert!(coordinator.publication.is_some());
        let first_plan_frames = coordinator.outbound.outbox.len();

        coordinator.on_frame(change_body(&dir, 2, main2).as_bytes());
        coordinator.job_out = None;
        let snapshot2 = snapshot_at(&dir, main2, coordinator.current_revision);
        coordinator.on_worker_result(AnalysisOutcome::Snapshot(snapshot2));
        assert!(coordinator.pending_publication.is_some());

        // Version 3 advances while version 2 is waiting behind version 1. Publishing the
        // waiting snapshot now would stamp version-2 facts with the version-3 ledger value.
        coordinator.on_frame(change_body(&dir, 3, main3).as_bytes());
        coordinator.job_out = None;
        while coordinator.pending_publication.is_some() {
            coordinator.on_receipt();
        }

        assert!(
            coordinator.publication.is_none(),
            "a pending snapshot from an older revision is not published"
        );
        assert_eq!(
            coordinator.outbound.outbox.len(),
            first_plan_frames,
            "no stale publication frame is encoded"
        );
        cleanup(&dir);
    }
}
