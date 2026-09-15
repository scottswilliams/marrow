//! The numeric capacities of the language server.
//!
//! Every bound the server enforces is spelled once here with its rationale: the
//! transport framing ceilings, the request/document/outbound ledger capacities, and
//! the publication-plan retention ceiling. These bound what the server itself
//! reserves; they are not a whole-process memory policy or a resident-set authority.

/// The largest `Content-Length` header value the framed reader admits, and thus the
/// largest single message body. A JSON-RPC message over this bound is refused before
/// the body is allocated. 8 MiB clears the largest realistic `didOpen`/`didChange`
/// full-document body (bounded again by the capture adapter's own per-file policy)
/// while failing a hostile length closed.
pub(crate) const MAX_FRAME_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The largest header block (all header lines before the blank line) the reader
/// admits before the body length is known. A header block over this bound is a
/// framing fault. Fixed small: the LSP header grammar carries only `Content-Length`
/// and an optional `Content-Type`.
pub(crate) const MAX_HEADER_BLOCK_BYTES: usize = 8 * 1024;

/// The largest UTF-8 byte length of a string request/response id the ledger admits.
/// A longer id is out of range and never enters the ledger. Fixed small: an editor
/// mints short correlation ids.
pub(crate) const MAX_REQUEST_ID_STRING_BYTES: usize = 256;

/// The largest number of simultaneously live request-ledger entries — ordinary
/// requests plus known-id error-only entries share this one budget. A unique valid
/// or recovered id that cannot reserve records `IngressOverload`. Sized to hold a
/// burst of in-flight editor requests with wide margin.
pub(crate) const MAX_LIVE_REQUEST_ENTRIES: usize = 512;

/// The largest number of simultaneously live null-id (anonymous) protocol-error
/// slots. A protocol error that cannot reserve one records the same zero-response
/// terminal outcome. Separate budget from the request ledger so a null-id flood
/// cannot starve known-id requests.
pub(crate) const MAX_ANONYMOUS_ERROR_SLOTS: usize = 64;

/// The largest number of open documents the ledger admits. A `didOpen` for a new key
/// that cannot reserve a slot records `OpenDocumentLedgerExhausted` and fail-stops.
/// A realistic editor session opens far fewer.
pub(crate) const MAX_OPEN_DOCUMENTS: usize = 4_096;

/// The largest UTF-8 byte length of a `file` URI the canonical owner admits. A longer
/// URI is rejected before decoding. Comfortably over the file-identity path bound.
pub(crate) const MAX_URI_BYTES: usize = 8 * 1024;

/// The outbound-frame queue capacity between the coordinator and the writer. Combined
/// with one active writer frame and the receipt-queue capacity it defines `W`, the
/// number of outbound credits.
pub(crate) const OUTBOUND_QUEUE_CAPACITY: usize = 8;

/// The writer receipt-queue capacity: completed-write receipts the writer sends back
/// to the coordinator.
pub(crate) const RECEIPT_QUEUE_CAPACITY: usize = 8;

/// `W`: the number of non-`Clone` outbound credits. Equals outbound-queue capacity
/// plus one active writer plus receipt-queue capacity. Every frame acquires one
/// before handoff to the writer. Pre-encoded publication frames remain charged to
/// their exclusive plan until handoff; unanswered semantic queries remain held.
pub(crate) const OUTBOUND_CREDITS: usize = OUTBOUND_QUEUE_CAPACITY + 1 + RECEIPT_QUEUE_CAPACITY;

/// The stack size for each spawned server thread. The analysis worker parses untrusted
/// source, whose recursion the parser bounds by counting frames against a typed depth
/// limit that trips far inside this stack — on every recursive path, and at a depth that
/// does not move with the length of the file.
pub(crate) const THREAD_STACK_BYTES: usize = 256 * 1024 * 1024;

/// The largest total byte size of the fully encoded frames one in-flight publication
/// plan may own at once. A plan over this ceiling is refused rather than retained.
pub(crate) const MAX_PUBLICATION_PLAN_BYTES: u64 = 24 * 1024 * 1024;

/// The largest single outbound frame body the server constructs: a response,
/// error, `showMessage`, or one diagnostic-publication frame. Bounded smaller than the
/// inbound frame ceiling because outbound bodies are the analysis floor's own
/// query-local ceilings (a 4 MiB format output, a 4 MiB fact-byte cap), never an
/// arbitrary editor `didChange` body.
pub(crate) const MAX_OUTBOUND_FRAME_BYTES: usize = 4 * 1024 * 1024;
