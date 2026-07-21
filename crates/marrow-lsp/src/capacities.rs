//! The frozen numeric capacities of the language server (the issue packet).
//!
//! Every bound the server enforces is spelled once here with its rationale. The
//! design requires these frozen before implementation: the transport framing
//! ceilings, the decoded-value bounds, the request/document/outbound ledger
//! capacities, and the retained-memory arithmetic `M_owned <= H_owned`.
//!
//! These bound *H00a-owned retained/reserved capacity only*. They are not a
//! whole-process out-of-memory policy, not an allocator or dependency-allocation
//! total, and not a resident-set-size authority; measured RSS and the A02a/GENPERF
//! latency clocks remain separate gates.
//!
//! # Retained-capacity inequality
//!
//! The server proves, at [`assert_capacity_budget`], the checked sum of every term
//! it retains stays under a fixed owned ceiling. Each dynamic term charges an
//! observed retained maximum, not a logical length, and a move-transfer inventory
//! (documented per credit type) proves no term is double-counted across affine
//! states.

/// The largest `Content-Length` header value the framed reader admits, and thus the
/// largest single message body. A JSON-RPC message over this bound is refused before
/// the body is allocated. 8 MiB clears the largest realistic `didOpen`/`didChange`
/// full-document body (bounded again by the capture adapter's own per-file policy)
/// while failing a hostile length closed.
pub const MAX_FRAME_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The largest header block (all header lines before the blank line) the reader
/// admits before the body length is known. A header block over this bound is a
/// framing fault. Fixed small: the LSP header grammar carries only `Content-Length`
/// and an optional `Content-Type`.
pub const MAX_HEADER_BLOCK_BYTES: usize = 8 * 1024;

/// The largest number of bytes the envelope decoder inspects for one message. Equal
/// to the frame body bound: the decoder never allocates a second copy beyond the
/// framed body it borrows.
pub const MAX_DECODE_BYTES: usize = MAX_FRAME_BODY_BYTES;

/// The largest UTF-8 byte length of a string request/response id the ledger admits.
/// A longer id is out of range and never enters the ledger. Fixed small: an editor
/// mints short correlation ids.
pub const MAX_REQUEST_ID_STRING_BYTES: usize = 256;

/// The largest number of simultaneously live request-ledger entries — ordinary
/// requests plus known-id error-only entries share this one budget. A unique valid
/// or recovered id that cannot reserve records `IngressOverload`. Sized to hold a
/// burst of in-flight editor requests with wide margin.
pub const MAX_LIVE_REQUEST_ENTRIES: usize = 512;

/// The largest number of simultaneously live null-id (anonymous) protocol-error
/// slots. A protocol error that cannot reserve one records the same zero-response
/// terminal outcome. Separate budget from the request ledger so a null-id flood
/// cannot starve known-id requests.
pub const MAX_ANONYMOUS_ERROR_SLOTS: usize = 64;

/// The largest number of open documents the ledger admits. A `didOpen` for a new key
/// that cannot reserve a slot records `OpenDocumentLedgerExhausted` and fail-stops.
/// A realistic editor session opens far fewer.
pub const MAX_OPEN_DOCUMENTS: usize = 4_096;

/// The largest UTF-8 byte length of a `file` URI the canonical owner admits. A longer
/// URI is rejected before decoding. Comfortably over the file-identity path bound.
pub const MAX_URI_BYTES: usize = 8 * 1024;

/// The outbound-frame queue capacity between the coordinator and the writer. Combined
/// with one active writer frame and the receipt-queue capacity it defines `W`, the
/// number of outbound credits.
pub const OUTBOUND_QUEUE_CAPACITY: usize = 8;

/// The writer receipt-queue capacity: completed-write receipts the writer sends back
/// to the coordinator.
pub const RECEIPT_QUEUE_CAPACITY: usize = 8;

/// The number of distinct revision-owned snapshot records the server retains at once,
/// bounded by the coordinator's current-plus-pending snapshot `Option`s. Two lets a newer
/// analysis land while an in-flight request still references the prior snapshot.
pub const MAX_RETAINED_SNAPSHOTS: usize = 2;

/// `W`: the number of non-`Clone` outbound credits. Equals outbound-queue capacity
/// plus one active writer plus receipt-queue capacity. Every response, error, null-id
/// protocol frame, and `showMessage` acquires one before construction.
pub const OUTBOUND_CREDITS: usize = OUTBOUND_QUEUE_CAPACITY + 1 + RECEIPT_QUEUE_CAPACITY;

/// The stack size for each spawned server thread. The analysis worker parses untrusted
/// source, whose recursion the parser bounds with a typed depth limit that trips far
/// inside this stack.
pub const THREAD_STACK_BYTES: usize = 256 * 1024 * 1024;

/// The fixed owned retained-capacity ceiling `H_owned`, in bytes. The checked term sum
/// `M_owned` must stay under this. 256 MiB is a generous editor-session ceiling that
/// still fails a retention avalanche closed; it excludes thread stacks, which the OS
/// maps lazily and which are charged separately below only as an accounting entry.
pub const H_OWNED_BYTES: u64 = 256 * 1024 * 1024;

/// A conservative fixed upper bound on the bytes one typed retained request/result
/// record occupies (`B_req`): the largest method payload the coordinator retains while
/// a request is in flight. Fixed generous.
pub const B_REQ_BYTES: u64 = 4 * 1024;

/// A conservative fixed upper bound on one anonymous-error slot record (`B_anon`).
pub const B_ANON_BYTES: u64 = 512;

/// A conservative fixed upper bound on one open-document ledger record excluding its
/// text body (`B_open`): the key, version, and maximum bounded failure evidence,
/// accounted whether the entry is text or unavailable so a later state replacement
/// cannot grow unaccounted retention.
pub const B_OPEN_RECORD_BYTES: u64 = 8 * 1024;

/// The aggregate open-document text ceiling (`B_open_text`): the total admitted source
/// bytes across every open `OpenText` entry, charged once (not a second per-document
/// copy). Bounded by the capture adapter's project-total policy; this is the server's
/// own accounting of the retained overlay text.
pub const B_OPEN_TEXT_BYTES: u64 = 32 * 1024 * 1024;

/// A conservative fixed upper bound on one retained project-input record (`B_project`)
/// excluding shared source bytes, charged per distinct retained snapshot revision.
pub const B_PROJECT_BYTES: u64 = 64 * 1024;

/// A conservative fixed upper bound on one retained analysis-snapshot record
/// (`B_snapshot`) — the diagnostic and fact collections — per distinct retained
/// revision. Bounded by the analysis floor's own fact/byte ceilings.
pub const B_SNAPSHOT_BYTES: u64 = 12 * 1024 * 1024;

/// The bytes reserved for coordinator transient state that is not otherwise itemized:
/// the pending-edit slot, the active job, the worker result, the pending result, and
/// the capture-transient evidence (`B_pending_edit + B_active_job + B_worker_result +
/// B_pending_result + B_cap_transient`).
pub const B_COORDINATOR_TRANSIENT_BYTES: u64 = 4 * 1024 * 1024;

/// The persistent delivered-diagnostic ledger ceiling (`B_diag_ledger`): the retained
/// per-file published-key state.
pub const B_DIAG_LEDGER_BYTES: u64 = 4 * 1024 * 1024;

/// The publication-plan ceiling (`B_publication_plan`): every fully encoded diagnostic
/// frame still owned by the in-flight plan, plus the retained old-ledger/new-snapshot
/// union keys and owned receipt mutations (`B_diag_union`).
pub const B_PUBLICATION_PLAN_BYTES: u64 = 24 * 1024 * 1024;

/// The largest single outbound frame body the server constructs (`F_out`): a response,
/// error, `showMessage`, or one diagnostic-publication frame. Bounded smaller than the
/// inbound frame ceiling because outbound bodies are the analysis floor's own
/// query-local ceilings (a 4 MiB format output, a 4 MiB fact-byte cap), never an
/// arbitrary editor `didChange` body.
pub const MAX_OUTBOUND_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// One complete bounded outbound frame (`F_out`).
pub const F_OUT_BYTES: u64 = MAX_OUTBOUND_FRAME_BYTES as u64;

/// One complete bounded inbound frame (`F_in`). Charged with factor three: reader-local,
/// ingress-queued, and coordinator-current.
pub const F_IN_BYTES: u64 = MAX_FRAME_BODY_BYTES as u64;

/// One writer receipt record (`B_receipt`).
pub const B_RECEIPT_BYTES: u64 = 256;

/// The retained selected-root URI spelling (`B_selected_root_uri`).
pub const B_SELECTED_ROOT_URI_BYTES: u64 = MAX_URI_BYTES as u64;

/// The fixed terminal-state record (`B_fixed_terminal`).
pub const B_FIXED_TERMINAL_BYTES: u64 = 4 * 1024;

/// The retained-capacity term sum `M_owned`. Every term charges an observed retained
/// maximum. Evaluated in a `const` context, so any `+`/`*` overflow is itself a
/// compile-time error (const evaluation panics on overflow) rather than a silent wrap —
/// the assertion cannot be proven on a wrapped sum.
///
/// Thread stacks are excluded from `M_owned`: they are OS-mapped lazily, are not
/// heap retention the server allocates, and are held to the separate measured-RSS
/// gate. The equation bounds heap retention the server itself reserves.
pub const fn m_owned() -> u64 {
    // Reader-local + ingress-queued + coordinator-current inbound frames.
    let inbound = 3 * F_IN_BYTES + MAX_DECODE_BYTES as u64;
    let requests = MAX_LIVE_REQUEST_ENTRIES as u64 * B_REQ_BYTES;
    let anonymous = MAX_ANONYMOUS_ERROR_SLOTS as u64 * B_ANON_BYTES;
    let open_docs = MAX_OPEN_DOCUMENTS as u64 * B_OPEN_RECORD_BYTES + B_OPEN_TEXT_BYTES;
    let snapshots = MAX_RETAINED_SNAPSHOTS as u64 * (B_PROJECT_BYTES + B_SNAPSHOT_BYTES);
    let coordinator = B_COORDINATOR_TRANSIENT_BYTES;
    let diagnostics = B_DIAG_LEDGER_BYTES + B_PUBLICATION_PLAN_BYTES;
    let outbound = OUTBOUND_CREDITS as u64 * (F_OUT_BYTES + B_RECEIPT_BYTES);
    let fixed = B_SELECTED_ROOT_URI_BYTES + B_FIXED_TERMINAL_BYTES;

    inbound
        + requests
        + anonymous
        + open_docs
        + snapshots
        + coordinator
        + diagnostics
        + outbound
        + fixed
}

/// Prove `M_owned <= H_owned` at compile time. A capacity change that would breach the
/// owned ceiling fails the build here rather than at runtime.
pub const fn assert_capacity_budget() {
    assert!(
        m_owned() <= H_OWNED_BYTES,
        "M_owned exceeds H_owned: the retained-capacity term sum must stay under the owned ceiling"
    );
}

const _: () = assert_capacity_budget();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_credits_equal_queue_plus_active_plus_receipts() {
        assert_eq!(
            OUTBOUND_CREDITS,
            OUTBOUND_QUEUE_CAPACITY + 1 + RECEIPT_QUEUE_CAPACITY
        );
    }

    #[test]
    fn retained_capacity_sum_is_under_owned_ceiling() {
        assert!(m_owned() <= H_OWNED_BYTES);
        // And not trivially under by being empty: the sum is a meaningful fraction.
        assert!(m_owned() > H_OWNED_BYTES / 4);
    }

    #[test]
    fn decode_bound_matches_frame_body() {
        assert_eq!(MAX_DECODE_BYTES, MAX_FRAME_BODY_BYTES);
    }
}
