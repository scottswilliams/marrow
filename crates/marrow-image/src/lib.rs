//! The Marrow program-image container.
//!
//! This crate owns the program-image contract: the
//! sectioned, length-prefixed, big-endian container grammar; the opcode encoding;
//! the representational bounds; the typed validating [`ImageDraft`] the compiler
//! builds an image through; the canonical [`ImageDraft::encode`] encoder; and the
//! [`ImageId`] integrity digest. It holds no decoder — the independent verifier in
//! `marrow-verify` owns the only path from bytes back to a checked image, so the
//! compiler can emit bytes but can never mint a trusted image.

pub mod bounds;
mod ceiling;
mod demand;
mod digest;
mod draft;
mod durable_id;
mod encode;
mod export_id;
mod instr;
mod interface;
mod issuance;
mod measure;
mod product;
mod remap;
mod semantic;
mod site_plan;
mod store_digest;
mod ty;
mod value_dag;

pub use ceiling::{CeilingDescriptor, CeilingId};
pub use demand::{
    CeilingDecodeError, DemandAtom, DemandSelection, DemandSetId, DemandView, ExportDemand,
    OperationClass,
};
pub use digest::{CompanionReleaseId, ImageId, companion_release_id, image_id};
pub use draft::{
    AdmittedGraphInputPlan, AdmittedRoot, CollTypeId, CollectionTypeDef, ConstId, DraftStateError,
    DraftTxn, EnumId, EnumTypeDef, FieldDef, FuncId, FunctionDef, ImageBuildError, ImageDraft,
    KeyColumn, RecordTypeDef, ReferenceKind, RootId, RootOccurrenceDef, SpanEntry, StrId, TypeId,
    VariantDef,
};
pub use durable_id::{
    DurableBranchView, DurableContractId, DurableContractView, DurableFieldView,
    DurableGraphTooLarge, DurableGroupView, DurableIndexComponent, DurableIndexShape,
    DurableMemberView, DurableMemberViewKind, DurableMemberViews, DurableRootView, LedgerIdBytes,
};
pub use encode::{EncodedImage, IMAGE_FORMAT_VERSION};
pub use export_id::ExportId;
pub use instr::{
    Draft, Instr, Instruction, OP_ASSERT, OP_BOOL_NOT, OP_BRANCH_PRESENT, OP_BYTES_GE, OP_BYTES_GT,
    OP_BYTES_LE, OP_BYTES_LT, OP_CALL, OP_CONST_LOAD, OP_CONV_BYTES_TEXT, OP_CONV_STRING,
    OP_DATE_ADD_DAYS, OP_DATE_DAYS_BETWEEN, OP_DATE_GE, OP_DATE_GT, OP_DATE_LE, OP_DATE_LT,
    OP_DUR_CREATE_ENTRY, OP_DUR_ERASE_ENTRY, OP_DUR_ERASE_FIELD, OP_DUR_ERASE_GROUP, OP_DUR_EXISTS,
    OP_DUR_FAMILY_EXISTS, OP_DUR_INDEX_EXISTS, OP_DUR_INDEX_LOOKUP, OP_DUR_INDEX_SCAN,
    OP_DUR_ITERATE_BOUNDED, OP_DUR_READ_ENTRY, OP_DUR_READ_FIELD, OP_DUR_READ_FIELD_PRESENT,
    OP_DUR_READ_GROUP, OP_DUR_READ_GROUP_PRESENT, OP_DUR_REPLACE_ENTRY, OP_DUR_REPLACE_GROUP,
    OP_DUR_SET_FIELD, OP_DURATION_ADD, OP_DURATION_GE, OP_DURATION_GT, OP_DURATION_LE,
    OP_DURATION_LT, OP_DURATION_SUB, OP_ENUM_CONSTRUCT, OP_ENUM_PAYLOAD_GET, OP_ENUM_TAG,
    OP_EQ_BOOL, OP_EQ_BYTES, OP_EQ_DATE, OP_EQ_DURATION, OP_EQ_ENUM, OP_EQ_ID, OP_EQ_INSTANT,
    OP_EQ_INT, OP_EQ_TEXT, OP_FIELD_GET, OP_FIELD_SET, OP_FIELD_UNSET, OP_IDENTITY_KEY_PATH,
    OP_INSTANT_ADD_DURATION, OP_INSTANT_GE, OP_INSTANT_GT, OP_INSTANT_LE, OP_INSTANT_LT,
    OP_INSTANT_SUB_DURATION, OP_INT_ADD, OP_INT_ADD_CHECKED, OP_INT_DIV, OP_INT_DIV_CHECKED,
    OP_INT_GE, OP_INT_GT, OP_INT_LE, OP_INT_LT, OP_INT_MUL, OP_INT_MUL_CHECKED, OP_INT_NEG,
    OP_INT_NEG_CHECKED, OP_INT_REM, OP_INT_REM_CHECKED, OP_INT_SUB, OP_INT_SUB_CHECKED, OP_JUMP,
    OP_JUMP_IF_FALSE, OP_LIST_APPEND, OP_LIST_GET, OP_LIST_INDEX, OP_LIST_LEN, OP_LIST_NEW,
    OP_LOCAL_GET, OP_LOCAL_SET, OP_MAKE_IDENTITY, OP_MAP_GET, OP_MAP_INSERT, OP_MAP_KEY_AT,
    OP_MAP_LEN, OP_MAP_NEW, OP_MAP_REMOVE, OP_MAP_VALUE_AT, OP_POP, OP_RANGE_GUARD, OP_RECORD_NEW,
    OP_RETURN, OP_SOME_WRAP, OP_TEXT_CONCAT, OP_TEXT_CONTAINS, OP_TEXT_GE, OP_TEXT_GT,
    OP_TEXT_IS_EMPTY, OP_TEXT_JOIN, OP_TEXT_LE, OP_TEXT_LINES, OP_TEXT_LT, OP_TEXT_SPLIT,
    OP_TEXT_TRIM, OP_TODO, OP_TXN_BEGIN, OP_TXN_COMMIT, OP_UNREACHABLE, OP_VACANT_LOAD, OpClass,
    Operands, Sealed, SealedInstr,
};
pub use interface::{
    CollectionShape, EnumShape, ExportSignature, FieldShape, FunctionDescriptor, Interface,
    InterfaceError, InterfaceId, RecordShape, RootShape, TransferType, VariantShape,
};
pub use product::{
    CanonicalDeclarationPathSelector, DeclarationMember, DeclarationMemberDef,
    DeclarationMemberShape, DurableContractGraph, DurableGraphInputRefusal, DurableProductGraph,
    RootOccurrenceSelector,
};
pub use semantic::{
    SemanticNode, SemanticNodeKind, SemanticPath, SemanticPathRefusal, SemanticStep,
    SemanticStepKind, SemanticTarget,
};
pub use site_plan::{OccurrenceSiteHandle, PlannedSiteRef, SitePlanStateError};
pub use store_digest::{
    StoreBackupDigest, StoreDataDigest, StoreEnvelopeDigest, StoreHeadDigest, interface_fingerprint,
};
pub use ty::{
    ImageType, OPTIONAL_FLAG, Scalar, TAG_BOOL, TAG_BYTES, TAG_COLLECTION, TAG_DATE, TAG_DURATION,
    TAG_ENUM, TAG_IDENTITY, TAG_INSTANT, TAG_INT, TAG_RECORD, TAG_TEXT, TAG_UNIT,
};
pub use value_dag::{
    CanonicalValueShapeDag, ValueShapeComparison, ValueShapeEnumMember, ValueShapeNodeId,
    ValueShapeView,
};

#[cfg(test)]
mod fixtures {
    use crate::durable_id::LedgerIdBytes;

    /// The sixteen-byte ledger id every byte of which is `byte`: the fixed fixture id the
    /// inline tests spell.
    pub(crate) fn id(byte: u8) -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([byte; 16])
    }
}
