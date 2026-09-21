//! The independent Marrow program-image verifier.
//!
//! This crate owns the only path from image bytes to a checked, sealed
//! [`VerifiedImage`]: it is the single container decoder and the phased verifier
//! The compiler emits bytes but never constructs a `VerifiedImage`;
//! the VM accepts only one this crate produced. Verification reconstructs every
//! executable claim from the bytes — it trusts no serialized compiler summary —
//! and rejects a malformed or hostile image at the earliest phase whose invariant
//! it violates, with a typed [`VerifyRejection`].

mod interface;
mod reader;
mod reject;
mod sealed;
mod verify;
mod vtype;

pub use interface::interface_of;
pub use marrow_image::{
    CeilingDescriptor, CeilingId, DemandAtom, DemandSetId, DemandView, DurableContractId,
    DurableGraphInputRefusal, DurableIndexComponent, ExportDemand, ExportId, ImageId, ImageType,
    LedgerIdBytes, OperationClass, Scalar, SealedInstr, SemanticNode, SemanticNodeKind,
    SemanticPath, SemanticStep, SemanticStepKind, SemanticTarget,
};
pub use reject::{
    Bound, Duplicate, Flag, Operand, Projection, Ref, Region, RejectionKind, SiteFault, SiteKind,
    Tag, TieFault, TieNode, TypePosition, TypeRefFault, VerifyPhase, VerifyRejection,
};
pub use sealed::{
    AtomIncidence, FunctionIndex, NodeIncidence, SealedBranch, SealedCollectionType, SealedConst,
    SealedEnumType, SealedExport, SealedField, SealedFunction, SealedGroup, SealedIndex,
    SealedIndexComponent, SealedRecordType, SealedRoot, SealedSite, SealedSiteTarget,
    SealedTestEntry, SealedVariant, SpanRow, VerifiedFunction, VerifiedImage,
    VerifiedRootOccurrence,
};
pub use verify::verify;

/// The machine stack [`verify`] requires, whatever image it is given.
///
/// Verification's input is chosen by a hostile producer, so its frame use must be bounded
/// by the image bounds and nothing else. Every walk over decoded structure drives an
/// explicit stack except three, which recurse natively at a depth `marrow_image::bounds`
/// fixes: value-shape decoding and the value-shape/record-type match at
/// `MAX_DURABLE_VALUE_DEPTH` (32), and branch sealing at `MAX_DURABLE_DEPTH` (16). A frame
/// count is not a stack bound, so the cost of those depths is measured rather than argued:
/// `tests/stack_budget.rs` verifies the deepest image the bounds admit on a thread of
/// exactly this size, and that image needs between 80 and 88 KiB unoptimized. The budget
/// is set above the measurement with room for the frames a debug build spends, and the
/// test fails if verification ever needs more.
///
/// It is stated here because it is the verifier's requirement, not its callers': a caller
/// that spawns a thread for verification sizes it from this, and a caller that verifies on
/// a thread it did not size — a default 2 MiB Rust thread, say — can read whether that is
/// enough. It is.
pub const VERIFY_STACK_BYTES: usize = 128 * 1024;
