//! The independent Marrow program-image verifier.
//!
//! This crate owns the only path from image bytes to a checked, sealed
//! [`VerifiedImage`]: it is the single container decoder and the phased verifier
//! (design §E). The compiler emits bytes but never constructs a `VerifiedImage`;
//! the VM accepts only one this crate produced. Verification reconstructs every
//! executable claim from the bytes — it trusts no serialized compiler summary —
//! and rejects a malformed or hostile image at the earliest phase whose invariant
//! it violates, with a typed [`VerifyRejection`].

mod reader;
mod reject;
mod sealed;
mod verify;
mod vtype;

pub use marrow_image::{
    CeilingDescriptor, CeilingId, DemandAtom, DemandSetId, DurableContractId,
    DurableIndexComponent, ExportDemand, ExportId, ImageId, ImageType, LedgerIdBytes,
    OperationClass, Scalar, SemanticNode, SemanticNodeKind, SemanticPath, SemanticStep,
    SemanticStepKind, SemanticTarget,
};
pub use reject::{VerifyPhase, VerifyRejection};
pub use sealed::{
    AtomIncidence, NodeIncidence, RetShape, SealedConst, SealedEnumType, SealedExport, SealedField,
    SealedFunction, SealedIndex, SealedInstr, SealedRecordType, SealedRoot, SealedSite,
    SealedSiteTarget, SealedTestEntry, SealedVariant, SpanRow, VerifiedImage,
};
pub use verify::verify;
