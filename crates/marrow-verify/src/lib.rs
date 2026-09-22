//! The independent Marrow program-image verifier: the only path from image bytes to a
//! sealed [`VerifiedImage`]. Every executable claim is rebuilt from the bytes, and a
//! malformed or hostile image is refused at the earliest phase whose invariant it
//! violates, with a typed [`VerifyRejection`].

mod interface;
mod reader;
mod reject;
mod sealed;
mod verify;
mod vtype;

pub use interface::interface_of;
pub use marrow_image::{
    CeilingDescriptor, CeilingId, CollTypeId, DemandAtom, DemandSetId, DemandView,
    DurableContractId, DurableGraphInputRefusal, DurableIndexComponent, ExportDemand, ExportId,
    ImageId, ImageType, LedgerIdBytes, OperationClass, RootId, Scalar, SealedInstr, SemanticNode,
    SemanticNodeKind, SemanticPath, SemanticStep, SemanticStepKind, SemanticTarget, SiteId, TypeId,
};
pub use reject::{
    Bound, Duplicate, Flag, Operand, Projection, Ref, Region, RejectionKind, SiteFault, SiteKind,
    Tag, TieFault, TieNode, TypePosition, TypeRefFault, VerifyPhase, VerifyRejection,
};
pub use sealed::{
    AtomIncidence, FunctionIndex, NodeIncidence, SealedBranch, SealedCollectionType, SealedConst,
    SealedEnumType, SealedExport, SealedField, SealedFunction, SealedGroup, SealedIndex,
    SealedIndexComponent, SealedRecordType, SealedRoot, SealedSite, SealedSiteTarget,
    SealedTestEntry, SealedVariant, VerifiedFunction, VerifiedImage,
};
pub use verify::verify;

/// The machine stack [`verify`] requires, whatever image it is given. Three walks recurse to
/// a declared image bound (value-shape decode and match, branch sealing); a frame count is
/// not a byte bound, so `tests/stack_budget.rs` verifies the deepest admitted image on a
/// thread of exactly this size.
pub const VERIFY_STACK_BYTES: usize = 128 * 1024;
