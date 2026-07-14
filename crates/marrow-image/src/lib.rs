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
    ConstId, FieldDef, FuncId, FunctionDef, ImageBuildError, ImageDraft, RecordTypeDef, RootDef,
    SiteDef, SiteId, SiteTarget, SpanEntry, StrId, TypeId,
};
pub use encode::EncodedImage;
pub use export_id::{EXPORT_ID_KIND, ExportId};
pub use instr::{
    Instr, OP_BOOL_NOT, OP_BRANCH_PRESENT, OP_CALL, OP_CONST_LOAD, OP_DUR_CREATE_ENTRY,
    OP_DUR_ERASE_ENTRY, OP_DUR_ERASE_FIELD, OP_DUR_EXISTS, OP_DUR_NEXT_KEY, OP_DUR_READ_ENTRY,
    OP_DUR_READ_FIELD, OP_DUR_REPLACE_ENTRY, OP_DUR_SET_REQUIRED, OP_DUR_SET_SPARSE, OP_EQ_BOOL,
    OP_EQ_INT, OP_EQ_TEXT, OP_FIELD_GET, OP_INT_ADD, OP_INT_GE, OP_INT_GT, OP_INT_LE, OP_INT_LT,
    OP_INT_DIV, OP_INT_MUL, OP_INT_NEG, OP_INT_REM, OP_INT_SUB, OP_JUMP, OP_JUMP_IF_FALSE,
    OP_LOCAL_GET,
    OP_LOCAL_SET, OP_POP, OP_RECORD_NEW, OP_RETURN, OP_SOME_WRAP, OP_TEXT_CONCAT, OP_TXN_BEGIN,
    OP_TEXT_GE, OP_TEXT_GT, OP_TEXT_LE, OP_TEXT_LT, OP_TXN_COMMIT, OP_UNREACHABLE, OP_VACANT_LOAD,
};
pub use ty::{ImageType, OPTIONAL_FLAG, Scalar, TAG_BOOL, TAG_INT, TAG_RECORD, TAG_TEXT, TAG_UNIT};
