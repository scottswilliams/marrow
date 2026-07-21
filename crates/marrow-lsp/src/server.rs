//! The process-main coordinator and its reader/worker/writer threads.
//!
//! The reader frames stdin into a cap-1 ingress queue and backpressures without drop or
//! reorder. The coordinator owns lifecycle, admission, document versions, overlay
//! construction, latest-wins edit coalescing, and outbound ordering; it never blocks on
//! I/O, downstream sends, or joins — it idles only on a cap-1 lost-wakeup-safe wake
//! channel and drains receipts, then results, then ingress. One analysis worker owns all
//! capture/analyze work behind the single [`WorkerCredit`]. One writer accepts immutable
//! framed bytes and returns a delivery receipt that frees the [`OutboundCredit`] it
//! consumed.
//!
//! # Scope
//!
//! This coordinator implements the primary journeys — initialize/initialized, full
//! document sync, whole-project recomputation, diagnostic publication with tombstones,
//! and hover/definition/formatting with revision reauthorization — faithfully to the
//! topology. The exhaustive terminal-arbitration, ingress-flood, and
//! publication-interleaving red matrix in the H00a design is not yet fully realized
//! here; see the crate `AGENTS.md` and the lane completion notes.

use std::io::BufReader;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;

use lsp_types::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentFormattingParams, GotoDefinitionParams, HoverParams, InitializeParams,
    InitializeResult, OneOf, ServerCapabilities, ServerInfo, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions,
};
use marrow_compile::{AnalysisSnapshot, InputRevision};
use std::sync::Arc;

use crate::analysis::{AnalysisOutcome, OverlayInput, run_analysis};
use crate::capacities::{
    MAX_ANONYMOUS_ERROR_SLOTS, MAX_LIVE_REQUEST_ENTRIES, OUTBOUND_QUEUE_CAPACITY,
    RECEIPT_QUEUE_CAPACITY, THREAD_STACK_BYTES,
};
use crate::credit::{CreditPool, OutboundCredit};
use crate::document::{DocumentLedger, DocumentState, RevisionCounter, UnavailableEvidence};
use crate::facts;
use crate::lifecycle::{
    CONTENT_MODIFIED, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, Lifecycle, METHOD_NOT_FOUND,
    PARSE_ERROR, REQUEST_FAILED, RequestGate, SERVER_NOT_INITIALIZED,
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

    let mut coordinator = Coordinator::new(work_tx, frame_tx, wake_tx);
    let exit = coordinator.run(&ingress_rx, &result_rx, &receipt_rx, &wake_rx);

    // Drop the coordinator's senders so the worker/writer loops observe disconnect and
    // return; the reader returns on EOF or when stdin closes.
    drop(coordinator);
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
        // Backpressure: a full cap-1 ingress blocks the reader here without drop.
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
            // A write or flush failure is terminal for delivery: stop without spin.
            return;
        }
        drop(handle);
        if receipts.send(Receipt).is_err() {
            return;
        }
        let _ = wake.try_send(());
    }
}

/// The published state of one file's diagnostics: the client-visible URI keys the
/// coordinator has delivered, for tombstone derivation.
struct Coordinator {
    lifecycle: Lifecycle,
    root: Option<SelectedRoot>,
    ledger: DocumentLedger,
    revisions: RevisionCounter,
    current_revision: InputRevision,
    snapshot: Option<Arc<AnalysisSnapshot>>,
    /// The document keys the server has a nonempty published diagnostic set for.
    published: Vec<DocumentKey>,
    outbound_credits: CreditPool<OutboundCredit>,
    /// Outbound credits currently in flight (awaiting a receipt), tracked by count.
    in_flight: Vec<OutboundCredit>,
    /// Frames waiting for an outbound credit, in order.
    pending_frames: std::collections::VecDeque<Vec<u8>>,
    worker_busy: bool,
    /// A coalesced pending recomputation: the latest edit supersedes an earlier one.
    pending_recompute: bool,
    request_entries: usize,
    anonymous_slots: usize,
    work_tx: SyncSender<WorkerJob>,
    frame_tx: SyncSender<Vec<u8>>,
    #[allow(dead_code)]
    wake_tx: SyncSender<()>,
    exit_code: u8,
    running: bool,
}

impl Coordinator {
    fn new(
        work_tx: SyncSender<WorkerJob>,
        frame_tx: SyncSender<Vec<u8>>,
        wake_tx: SyncSender<()>,
    ) -> Self {
        let (revisions, current_revision) = RevisionCounter::initial();
        Self {
            lifecycle: Lifecycle::new(),
            root: None,
            ledger: DocumentLedger::new(),
            revisions,
            current_revision,
            snapshot: None,
            published: Vec::new(),
            outbound_credits: CreditPool::outbound(),
            in_flight: Vec::new(),
            pending_frames: std::collections::VecDeque::new(),
            worker_busy: false,
            pending_recompute: false,
            request_entries: 0,
            anonymous_slots: 0,
            work_tx,
            frame_tx,
            wake_tx,
            exit_code: 1,
            running: true,
        }
    }

    fn run(
        &mut self,
        ingress: &Receiver<ReaderEvent>,
        results: &Receiver<WorkerResult>,
        receipts: &Receiver<Receipt>,
        wake: &Receiver<()>,
    ) -> u8 {
        while self.running {
            // Priority drain: receipts, then results, then ingress.
            let mut progressed = false;
            while let Ok(Receipt) = receipts.try_recv() {
                self.on_receipt();
                progressed = true;
            }
            while let Ok(result) = results.try_recv() {
                self.on_worker_result(result);
                progressed = true;
            }
            if let Ok(event) = ingress.try_recv() {
                self.on_reader_event(event);
                progressed = true;
            }
            if !self.running {
                break;
            }
            if !progressed {
                // Idle only on the wake barrier; a pending wake returns immediately.
                let _ = wake.recv();
            }
        }
        self.exit_code
    }

    fn on_reader_event(&mut self, event: ReaderEvent) {
        match event {
            ReaderEvent::Frame(body) => self.on_frame(&body),
            ReaderEvent::Terminal => {
                self.exit_code = self.lifecycle.on_terminal();
                self.running = false;
            }
        }
    }

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
                    Some(id) => self.send_error(&id, INVALID_REQUEST, message),
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
        // Reserve a live request-ledger entry; exhaustion drops with no response.
        if self.request_entries >= MAX_LIVE_REQUEST_ENTRIES {
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
                    self.send_error(&id, SERVER_NOT_INITIALIZED, "server not initialized")
                }
                RequestGate::InvalidInPhase => {
                    self.send_error(&id, INVALID_REQUEST, "invalid request in current state")
                }
                RequestGate::Route => self.send_error(&id, METHOD_NOT_FOUND, "method not found"),
            },
        }
    }

    fn on_initialize(&mut self, id: RequestId, params: Option<Box<serde_json::value::RawValue>>) {
        if self.lifecycle.on_initialize() != RequestGate::Route {
            self.send_error(&id, INVALID_REQUEST, "initialize already handled");
            return;
        }
        let root = match params
            .as_ref()
            .and_then(|raw| parse::<InitializeParams>(raw))
        {
            Some(params) => match select_root(&params) {
                Ok(root) => root,
                Err(RootError::TooMany) => {
                    // A malformed root candidate does not consume initialization.
                    self.rollback_initialize();
                    self.send_error(
                        &id,
                        INVALID_PARAMS,
                        "at most one workspace root is supported",
                    );
                    return;
                }
                Err(RootError::Malformed) => {
                    self.rollback_initialize();
                    self.send_error(&id, INVALID_PARAMS, "malformed root URI");
                    return;
                }
            },
            None => {
                self.rollback_initialize();
                self.send_error(&id, INVALID_PARAMS, "malformed initialize params");
                return;
            }
        };
        self.root = root;
        let result = Box::new(initialize_result());
        // The initialize response is delivered synchronously here for the coordinator's
        // purposes; delivery advances the lifecycle.
        self.send(Outbound::Initialize { id, result });
        if self.lifecycle.on_initialize_delivered() {
            self.enter_running();
        }
    }

    fn rollback_initialize(&mut self) {
        // A rejected initialize must leave the lifecycle in AwaitInitialize. The FSM
        // moved to InitializeReplyPending on on_initialize; reset it.
        self.lifecycle = Lifecycle::new();
    }

    fn on_shutdown(&mut self, id: RequestId) {
        match self.lifecycle.on_shutdown() {
            RequestGate::Route => {
                self.send(Outbound::Null { id });
                self.lifecycle.on_shutdown_delivered();
            }
            RequestGate::NotInitialized => {
                self.send_error(&id, SERVER_NOT_INITIALIZED, "server not initialized")
            }
            RequestGate::InvalidInPhase => {
                self.send_error(&id, INVALID_REQUEST, "invalid request in current state")
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
            self.send_error(&id, SERVER_NOT_INITIALIZED, "server not initialized");
            return;
        }
        let Some(root) = self.root.clone() else {
            self.send_error(&id, INVALID_PARAMS, "no selected root");
            return;
        };
        // Semantic requests require a ready snapshot at the current revision and an
        // available project. A missing snapshot or an unavailable project is -32803.
        let Some(snapshot) = self.snapshot.clone() else {
            self.send_error(&id, REQUEST_FAILED, "analysis not ready");
            return;
        };
        if !self.ledger.all_available() {
            self.send_error(&id, REQUEST_FAILED, "project capture unavailable");
            return;
        }
        let outcome = self.answer_semantic(&snapshot, &root, method, params.as_deref());
        match outcome {
            SemanticAnswer::Reply(outbound) => self.send_with_id(id, outbound),
            SemanticAnswer::ContentModified => {
                self.send_error(&id, CONTENT_MODIFIED, "content modified")
            }
            SemanticAnswer::BadParams => self.send_error(&id, INVALID_PARAMS, "malformed params"),
            SemanticAnswer::Internal => self.send_error(&id, INTERNAL_ERROR, "internal error"),
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
                let hover = facts::hover(snapshot, &identity, &source, position.position);
                SemanticAnswer::Reply(OutboundBody::Hover(hover))
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
                let Some(params) = params.and_then(parse::<DocumentFormattingParams>)
                else {
                    return SemanticAnswer::BadParams;
                };
                let Some((identity, source)) =
                    self.resolve_document(root, params.text_document.uri.as_str())
                else {
                    return SemanticAnswer::ContentModified;
                };
                let edits = facts::formatting(snapshot, &identity, &source);
                SemanticAnswer::Reply(OutboundBody::Formatting(edits))
            }
            _ => SemanticAnswer::Internal,
        }
    }

    /// The file identity and current open text for a client URI, if it is an open text
    /// document under the root. The identity derives from the canonical document key, so
    /// no client URI spelling is echoed.
    fn resolve_document(
        &self,
        root: &SelectedRoot,
        uri: &str,
    ) -> Option<(marrow_project_fs::FileIdentity, String)> {
        let (key, source) = self.resolve_open_document(root, uri)?;
        let (identity, _) = marrow_project_fs::FileIdentity::validate(key.relative()).ok()?;
        Some((identity, source))
    }

    /// The current open-document text for a client URI, plus its key, if it is an open
    /// text document under the root.
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
        for (open_key, text) in self.ledger.text_entries() {
            if open_key == &key {
                return Some(text.to_owned());
            }
        }
        None
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
            // $/cancelRequest is accepted and intentionally ignored; other unknown
            // notifications are discarded.
            _ => {}
        }
    }

    fn on_did_open(&mut self, params: Option<Box<serde_json::value::RawValue>>) {
        if self.lifecycle.phase() != crate::lifecycle::Phase::Running {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(params) = params.as_deref().and_then(parse::<DidOpenTextDocumentParams>) else {
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
        if self.lifecycle.phase() != crate::lifecycle::Phase::Running {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(params) = params.as_deref().and_then(parse::<DidChangeTextDocumentParams>) else {
            return;
        };
        let version = params.text_document.version;
        let Ok(key) = DocumentKey::from_uri(params.text_document.uri.as_str(), &root) else {
            return;
        };
        if self.ledger.validate_change(&key, version).is_err() {
            return;
        }
        // FULL sync: exactly one full-document change with no range.
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
        if self.lifecycle.phase() != crate::lifecycle::Phase::Running {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let Some(params) = params.as_deref().and_then(parse::<DidCloseTextDocumentParams>) else {
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

    /// Enqueue a recomputation when the worker is idle, else coalesce into the single
    /// pending slot (latest-wins).
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
        let job = WorkerJob {
            root: root.clone(),
            revision: self.current_revision,
            overlay,
        };
        match self.work_tx.try_send(job) {
            Ok(()) => {
                self.worker_busy = true;
                self.pending_recompute = false;
            }
            Err(_) => {
                // The worker is momentarily busy; coalesce.
                self.pending_recompute = true;
            }
        }
    }

    fn on_worker_result(&mut self, result: WorkerResult) {
        self.worker_busy = false;
        match result.outcome {
            AnalysisOutcome::Snapshot(snapshot) => {
                // Only an exact-current snapshot publishes.
                if snapshot.revision() == self.current_revision {
                    self.snapshot = Some(snapshot.clone());
                    self.publish_diagnostics(&snapshot);
                }
            }
            AnalysisOutcome::Capture(rejection) => {
                if rejection.revision == self.current_revision {
                    self.on_capture_failure(rejection.evidence);
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
        // Drain a coalesced recompute now that the worker is free.
        if let Some(root) = self.root.clone().filter(|_| self.pending_recompute) {
            self.dispatch_recompute(&root);
        }
    }

    fn on_capture_failure(&mut self, evidence: Option<UnavailableEvidence>) {
        // Background capture failure: publishes and clears no diagnostics. Report at
        // most one showMessage (the episode latch is simplified here to a single
        // background notification per failure).
        if let Some(evidence) = evidence {
            // The exact `<marrow-code>: <operational-message>` body, composed without a
            // rendering macro so the message is only the code and the facade-written text.
            let mut message =
                String::with_capacity(evidence.code.len() + 2 + evidence.message.len());
            message.push_str(evidence.code);
            message.push_str(": ");
            message.push_str(&evidence.message);
            self.send(Outbound::ShowMessage {
                typ: MessageType::Error,
                message,
            });
        }
    }

    /// Publish the complete diagnostic set for the current snapshot, plus an empty
    /// tombstone for every previously published file absent from the snapshot.
    fn publish_diagnostics(&mut self, snapshot: &AnalysisSnapshot) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let mut new_published = Vec::new();
        // One publication per snapshot file.
        for module in snapshot.input().modules() {
            let identity = module.identity();
            let key = DocumentKey::from_identity(identity);
            let source = std::str::from_utf8(module.source()).unwrap_or("");
            let version = self.version_for(&key);
            if let Ok(params) = facts::diagnostics_for_file(snapshot, &root, identity, source, version)
            {
                let has = !params.diagnostics.is_empty();
                self.send(Outbound::PublishDiagnostics(Box::new(params)));
                if has {
                    new_published.push(key);
                }
            }
        }
        // Tombstones: previously published files no longer in the snapshot.
        let snapshot_keys: Vec<DocumentKey> = snapshot
            .input()
            .modules()
            .iter()
            .map(|module| DocumentKey::from_identity(module.identity()))
            .collect();
        let previously = std::mem::take(&mut self.published);
        for key in &previously {
            if !snapshot_keys.contains(key)
                && let Ok(identity) = marrow_project_fs::FileIdentity::validate(key.relative())
                && let Some(uri) = lsp_uri(&root, &identity.0)
            {
                let tombstone = lsp_types::PublishDiagnosticsParams {
                    uri,
                    diagnostics: Vec::new(),
                    version: None,
                };
                self.send(Outbound::PublishDiagnostics(Box::new(tombstone)));
            }
        }
        self.published = new_published;
    }

    fn version_for(&self, key: &DocumentKey) -> Option<i32> {
        self.ledger.get(key).map(DocumentState::version)
    }

    // ---- outbound plumbing ----

    fn send_with_id(&mut self, id: RequestId, body: OutboundBody) {
        let outbound = match body {
            OutboundBody::Hover(result) => Outbound::Hover { id, result },
            OutboundBody::Definition(result) => Outbound::Definition { id, result },
            OutboundBody::Formatting(result) => Outbound::Formatting { id, result },
        };
        self.send(outbound);
    }

    fn send_error(&mut self, id: &RequestId, code: i32, message: &str) {
        self.send(Outbound::Error {
            id: Some(id.clone()),
            code,
            message: message.to_owned(),
        });
    }

    fn send_null_error(&mut self, code: i32, message: &str) {
        if self.anonymous_slots >= MAX_ANONYMOUS_ERROR_SLOTS {
            return;
        }
        self.anonymous_slots += 1;
        self.send(Outbound::Error {
            id: None,
            code,
            message: message.to_owned(),
        });
    }

    /// Acquire an outbound credit, encode, and hand the frame to the writer (or queue it
    /// when the outbound queue is full).
    fn send(&mut self, outbound: Outbound) {
        let bytes = match encode(&outbound) {
            Ok(bytes) => bytes,
            // A pre-handoff encoding failure emits zero bytes.
            Err(_) => return,
        };
        self.enqueue_frame(bytes);
    }

    fn enqueue_frame(&mut self, bytes: Vec<u8>) {
        // Acquire an outbound credit before handoff.
        let Some(credit) = self.outbound_credits.acquire() else {
            self.pending_frames.push_back(bytes);
            return;
        };
        match self.frame_tx.try_send(bytes) {
            Ok(()) => self.in_flight.push(credit),
            Err(std::sync::mpsc::TrySendError::Full(bytes)) => {
                self.outbound_credits.release(credit);
                self.pending_frames.push_back(bytes);
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                self.outbound_credits.release(credit);
            }
        }
    }

    fn on_receipt(&mut self) {
        if let Some(credit) = self.in_flight.pop() {
            self.outbound_credits.release(credit);
        }
        // A freed credit lets a queued frame proceed.
        if let Some(bytes) = self.pending_frames.pop_front() {
            self.enqueue_frame(bytes);
        }
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

enum SemanticAnswer {
    Reply(OutboundBody),
    ContentModified,
    BadParams,
    Internal,
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

fn lsp_uri(root: &SelectedRoot, identity: &marrow_project_fs::FileIdentity) -> Option<lsp_types::Uri> {
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
