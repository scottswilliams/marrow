//! The decoded intermediate model: the plain records phase 1/2 build before sealing.

use crate::sealed::{RetShape, SealedCollectionType, SealedConst, SealedSite};
use marrow_image::{
    DurableContractId, DurableIndexShape, DurableProductGraph, ExportId, ImageId, ImageType,
    LedgerIdBytes, Scalar, SemanticNode, SemanticPath,
};
use std::rc::Rc;

pub(super) struct DecodedRecordType {
    #[allow(dead_code)]
    pub(super) name: u16,
    pub(super) fields: Vec<DecodedField>,
}

pub(super) struct DecodedField {
    pub(super) name: u16,
    /// A bare (non-optional) type: a scalar for a durable-storable field, or a
    /// closed enum for a local-only value field. The enum index is bounds-checked
    /// against the ENUMS table after it decodes (`validate_record_field_enums`).
    pub(super) ty: ImageType,
    pub(super) required: bool,
}

/// A decoded enum type: name string index and its ordered variants.
pub(super) struct DecodedEnum {
    pub(super) name: u16,
    pub(super) variants: Vec<DecodedVariant>,
}

/// A decoded enum variant: name string index, `category` flag, and dense payload
/// in declaration order. Each leaf is a bare (non-optional) [`ImageType`].
pub(super) struct DecodedVariant {
    pub(super) name: u16,
    pub(super) category: bool,
    pub(super) payload: Vec<ImageType>,
}

/// A decoded durable root: name string index, its ordered key tuple (each column a
/// scalar and its ledger id; empty for a singleton root), record type index, the
/// rest of the root's placement ledger identity, and the resource's durable member
/// graph. The Product identity is not among them: it names the declaration row, which the
/// one contract graph holds, so a root does not carry a second spelling of it.
pub(super) struct DecodedRoot {
    pub(super) name: u16,
    pub(super) keys: Vec<(Scalar, LedgerIdBytes)>,
    pub(super) record: u16,
    pub(super) placement: LedgerIdBytes,
    /// The Product declaration's canonical member graph, shared with the one durable
    /// contract graph this image decoded into. A Product is a declaration and a root is
    /// an occurrence of it, so repeated occurrences of one Product share the one graph
    /// accepted at its first occurrence rather than each retaining a copy.
    ///
    /// It is read as borrowed views, which carry no member vector, so nothing here can be
    /// turned back into an owned recursive tree.
    pub(super) members: DurableProductGraph,
    pub(super) indexes: Vec<DurableIndexShape>,
}

pub(super) struct DecodedFunction {
    pub(super) name: u16,
    pub(super) source: u16,
    pub(super) params: Vec<ImageType>,
    pub(super) ret: RetShape,
    pub(super) local_count: u16,
    pub(super) code: Vec<u8>,
    pub(super) spans: Vec<(u32, u32, u32)>,
}

pub(super) struct DecodedImage {
    pub(super) image_id: ImageId,
    pub(super) strings: Vec<Rc<str>>,
    pub(super) types: Vec<DecodedRecordType>,
    pub(super) enums: Vec<DecodedEnum>,
    pub(super) collections: Vec<SealedCollectionType>,
    pub(super) roots: Vec<DecodedRoot>,
    pub(super) sites: Vec<SealedSite>,
    /// Each site's resolved graph-node path, parallel to `sites` by index.
    pub(super) site_paths: Vec<SemanticPath>,
    pub(super) durable_contract: DurableContractId,
    /// The durable graph's node set, each paired with its derived [`SemanticPath`], as
    /// this verifier independently derived it from the decoded tables — the same
    /// derivation the recomputed contract id was taken over.
    pub(super) semantic_nodes: Vec<SemanticNode>,
    pub(super) consts: Vec<SealedConst>,
    pub(super) functions: Vec<DecodedFunction>,
    pub(super) exports: Vec<(ExportId, u16)>,
    /// Decoded TEST-ENTRY rows: `(name-string-index, function-index)`, ascending by
    /// name index.
    pub(super) test_entries: Vec<(u16, u16)>,
}
