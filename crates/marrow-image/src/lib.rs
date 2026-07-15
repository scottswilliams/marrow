//! The Marrow program-image container.
//!
//! This crate owns the ProgramImage v0 contract (design §C/§D/§E): the frozen
//! sectioned, length-prefixed, big-endian container grammar; the opcode encoding;
//! the representational bounds; the typed validating [`ImageDraft`] the compiler
//! builds an image through; the canonical [`ImageDraft::encode`] encoder; and the
//! [`ImageId`] integrity digest. It holds no decoder — the independent verifier in
//! `marrow-verify` owns the only path from bytes back to a checked image, so the
//! compiler can emit bytes but can never mint a trusted image.

pub mod bounds;
mod demand;
mod digest;
mod draft;
mod durable_id;
mod encode;
mod export_id;
mod instr;
mod semantic;
mod ty;

pub use demand::{DEMAND_SET_KIND, DemandAtom, DemandSetId, ExportDemand, OperationClass};
pub use digest::{IMAGE_DIGEST_KIND, ImageId, image_id};
pub use draft::{
    CollTypeId, CollectionTypeDef, ConstId, DurableMemberDef, EnumId, EnumTypeDef, FieldDef,
    FuncId, FunctionDef, ImageBuildError, ImageDraft, KeyColumn, RecordTypeDef, RootDef,
    RootIdentity, SiteDef, SiteId, SpanEntry, StrId, TypeId, VariantDef,
};
pub use durable_id::{
    DURABLE_CONTRACT_KIND, DurableBranchShape, DurableContractDescriptor, DurableContractId,
    DurableEnumMemberShape, DurableFieldShape, DurableGroupShape, DurableIndexComponent,
    DurableIndexShape, DurableKeyShape, DurableMemberShape, DurableRootShape, DurableValueShape,
    LedgerIdBytes,
};
pub use encode::EncodedImage;
pub use export_id::{EXPORT_ID_KIND, ExportId};
pub use instr::{
    Instr, OP_ASSERT, OP_BOOL_NOT, OP_BRANCH_PRESENT, OP_BYTES_GE, OP_BYTES_GT, OP_BYTES_LE,
    OP_BYTES_LT, OP_CALL, OP_CONST_LOAD, OP_CONV_BYTES_TEXT, OP_CONV_STRING_BOOL,
    OP_CONV_STRING_INT, OP_DATE_ADD_DAYS, OP_DATE_DAYS_BETWEEN, OP_DATE_GE, OP_DATE_GT, OP_DATE_LE,
    OP_DATE_LT, OP_DUR_CREATE_ENTRY, OP_DUR_ERASE_ENTRY, OP_DUR_ERASE_FIELD, OP_DUR_EXISTS,
    OP_DUR_NEXT_KEY, OP_DUR_READ_ENTRY, OP_DUR_READ_FIELD, OP_DUR_REPLACE_ENTRY,
    OP_DUR_SET_REQUIRED, OP_DUR_SET_SPARSE, OP_DUR_SET_SPARSE_PRESENT, OP_DURATION_ADD,
    OP_DURATION_GE, OP_DURATION_GT, OP_DURATION_LE, OP_DURATION_LT, OP_DURATION_SUB,
    OP_ENUM_CONSTRUCT, OP_ENUM_PAYLOAD_GET, OP_ENUM_TAG, OP_EQ_BOOL, OP_EQ_BYTES, OP_EQ_DATE,
    OP_EQ_DURATION, OP_EQ_ENUM, OP_EQ_INSTANT, OP_EQ_INT, OP_EQ_TEXT, OP_FIELD_GET, OP_FIELD_SET,
    OP_FIELD_UNSET, OP_INSTANT_ADD_DURATION, OP_INSTANT_GE, OP_INSTANT_GT, OP_INSTANT_LE,
    OP_INSTANT_LT, OP_INSTANT_SUB_DURATION, OP_INT_ADD, OP_INT_ADD_CHECKED, OP_INT_DIV,
    OP_INT_DIV_CHECKED, OP_INT_GE, OP_INT_GT, OP_INT_LE, OP_INT_LT, OP_INT_MUL, OP_INT_MUL_CHECKED,
    OP_INT_NEG, OP_INT_NEG_CHECKED, OP_INT_REM, OP_INT_REM_CHECKED, OP_INT_SUB, OP_INT_SUB_CHECKED,
    OP_JUMP, OP_JUMP_IF_FALSE, OP_LIST_APPEND, OP_LIST_GET, OP_LIST_LEN, OP_LIST_NEW, OP_LOCAL_GET,
    OP_LOCAL_SET, OP_MAP_GET, OP_MAP_INSERT, OP_MAP_KEY_AT, OP_MAP_LEN, OP_MAP_NEW,
    OP_MAP_VALUE_AT, OP_POP, OP_RANGE_GUARD, OP_RECORD_NEW, OP_RETURN, OP_SOME_WRAP,
    OP_TEXT_CONCAT, OP_TEXT_CONTAINS, OP_TEXT_GE, OP_TEXT_GT, OP_TEXT_IS_EMPTY, OP_TEXT_JOIN,
    OP_TEXT_LE, OP_TEXT_LINES, OP_TEXT_LT, OP_TEXT_SPLIT, OP_TEXT_TRIM, OP_TXN_BEGIN,
    OP_TXN_COMMIT, OP_UNREACHABLE, OP_VACANT_LOAD,
};
pub use semantic::{
    SemanticNode, SemanticNodeKind, SemanticPath, SemanticStep, SemanticStepKind, SemanticTarget,
};
pub use ty::{
    ImageType, OPTIONAL_FLAG, Scalar, TAG_BOOL, TAG_BYTES, TAG_COLLECTION, TAG_DATE, TAG_DURATION,
    TAG_ENUM, TAG_INSTANT, TAG_INT, TAG_RECORD, TAG_TEXT, TAG_UNIT,
};
