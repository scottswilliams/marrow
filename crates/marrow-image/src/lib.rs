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
mod digest;
mod draft;
mod encode;
mod export_id;
mod instr;
mod ty;

pub use digest::{IMAGE_DIGEST_KIND, ImageId, image_id};
pub use draft::{
    ConstId, EnumId, EnumTypeDef, FieldDef, FuncId, FunctionDef, ImageBuildError, ImageDraft,
    RecordTypeDef, RootDef, SiteDef, SiteId, SiteTarget, SpanEntry, StrId, TypeId, VariantDef,
};
pub use encode::EncodedImage;
pub use export_id::{EXPORT_ID_KIND, ExportId};
pub use instr::{
    Instr, OP_ASSERT, OP_BOOL_NOT, OP_BRANCH_PRESENT, OP_BYTES_GE, OP_BYTES_GT, OP_BYTES_LE,
    OP_BYTES_LT, OP_CALL, OP_CONST_LOAD, OP_CONV_BYTES_TEXT, OP_CONV_STRING_BOOL,
    OP_CONV_STRING_INT, OP_DUR_CREATE_ENTRY, OP_DUR_ERASE_ENTRY, OP_DUR_ERASE_FIELD, OP_DUR_EXISTS,
    OP_DUR_NEXT_KEY, OP_DUR_READ_ENTRY, OP_DUR_READ_FIELD, OP_DUR_REPLACE_ENTRY,
    OP_DUR_SET_REQUIRED, OP_DUR_SET_SPARSE, OP_ENUM_CONSTRUCT, OP_ENUM_PAYLOAD_GET, OP_ENUM_TAG,
    OP_EQ_BOOL, OP_EQ_BYTES, OP_EQ_ENUM, OP_EQ_INT, OP_EQ_TEXT, OP_FIELD_GET, OP_INT_ADD,
    OP_INT_ADD_CHECKED, OP_INT_DIV, OP_INT_DIV_CHECKED, OP_INT_GE, OP_INT_GT, OP_INT_LE, OP_INT_LT,
    OP_INT_MUL, OP_INT_MUL_CHECKED, OP_INT_NEG, OP_INT_NEG_CHECKED, OP_INT_REM, OP_INT_REM_CHECKED,
    OP_INT_SUB, OP_INT_SUB_CHECKED, OP_JUMP, OP_JUMP_IF_FALSE, OP_LOCAL_GET, OP_LOCAL_SET, OP_POP,
    OP_RANGE_GUARD, OP_RECORD_NEW, OP_RETURN, OP_SOME_WRAP, OP_TEXT_CONCAT, OP_TEXT_CONTAINS,
    OP_TEXT_GE, OP_TEXT_GT, OP_TEXT_IS_EMPTY, OP_TEXT_LE, OP_TEXT_LT, OP_TEXT_TRIM, OP_TXN_BEGIN,
    OP_TXN_COMMIT, OP_UNREACHABLE, OP_VACANT_LOAD,
};
pub use ty::{
    ImageType, OPTIONAL_FLAG, Scalar, TAG_BOOL, TAG_BYTES, TAG_ENUM, TAG_INT, TAG_RECORD, TAG_TEXT,
    TAG_UNIT,
};
