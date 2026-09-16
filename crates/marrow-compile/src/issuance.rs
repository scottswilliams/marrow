//! The wide-ordinal population derivation: how many rows of each owned pre-seal
//! non-function kind an admitted `ProjectInput` can drive into the draft, and why that
//! count stays inside the private `u32` carrier. It lives here rather than beside the
//! carrier facts in `marrow-image` because it needs both the source-capture ceiling and
//! the compiler's own generic-instantiation bound.
//!
//! **Declared rows** — a string, constant, record, enum, collection, root, site, or type
//! parameter written in source — each occupy at least one distinct source byte, so they
//! cannot exceed [`MAX_ADMITTED_SOURCE_BYTES`]. **Generated rows** are not charged to
//! source bytes: one syntactic call can keep minting fresh instances when a generic
//! recurses over an ever-growing type, so the mint is refused at [`MAX_INSTANTIATIONS`].
//!
//! The two multiply. Interning is keyed, so instances lowering the same body share its
//! values, but filling an instance materializes the template's *declared* shape into
//! per-instance rows keying cannot absorb — one entry per declared field, one leaf per
//! declared variant and payload; `List<T>`/`Map<K,V>` dedup by source element type, and a
//! divergent instance carries a different element type at every step. One instance's
//! width is at most [`MAX_PARSED_FILE_BYTES`], since its template belongs to one admitted
//! file and materializes into syntax there, giving
//!
//! ```text
//! rows(kind) <= MAX_ADMITTED_SOURCE_BYTES
//!            + MAX_INSTANTIATIONS * MAX_PARSED_FILE_BYTES
//! ```
//!
//! The `const` assert below holds that against the `u32` domain using the live capture,
//! parse, and instantiation owners, so widening any one of them past the carrier breaks
//! the build. It bounds the carrier only: per-kind image-admission ceilings stay policy
//! verdicts applied after construction, and function instructions lie outside this
//! population — lowering checks each encoded width before retaining the crossing
//! instruction and reports `check.resource_limit` at that construct.

use marrow_project::CaptureLimits;

use crate::{MAX_PARSED_FILE_BYTES, types::MAX_INSTANTIATIONS};

/// The admitted whole-project source ceiling, read from the capture owner rather than
/// restated, so a capture widening cannot drift past this derivation silently.
const MAX_ADMITTED_SOURCE_BYTES: usize = CaptureLimits::DEFAULT.max_total_bytes();

/// The admitted file ceiling, used to keep file-fixed rows inside the declared-row term.
const MAX_ADMITTED_FILES: usize = CaptureLimits::DEFAULT.max_files();

/// The largest number of rows of one kind materialized by either fileless reserved
/// template (`Option` or `Result`) for one instance. Their Rust definitions are not const
/// data, so this is the named audit term a change to either definition must update.
const MAX_FILELESS_TEMPLATE_WIDTH: usize = 2;

/// The derived per-kind row maximum: declared rows charged to distinct project source
/// bytes, plus every generated instance carrying the widest template one admitted file can
/// contain.
const MAX_DERIVED_ROWS: usize =
    MAX_ADMITTED_SOURCE_BYTES + MAX_INSTANTIATIONS * MAX_PARSED_FILE_BYTES;

/// The derivation's conclusion: the population an admitted project can drive fits the
/// wide carrier, so a checked wide mint cannot refuse an admissible input.
const fn population_fits_the_wide_carrier() -> bool {
    MAX_ADMITTED_FILES <= MAX_ADMITTED_SOURCE_BYTES
        && MAX_FILELESS_TEMPLATE_WIDTH <= MAX_PARSED_FILE_BYTES
        && MAX_DERIVED_ROWS < u32::MAX as usize
}

const _: () = assert!(population_fits_the_wide_carrier());
