//! The decoded intermediate model: the plain records phase 1/2 build before sealing.

use crate::sealed::{RetShape, SealedCollectionType, SealedConst, SealedSite};
use marrow_image::{
    DurableContractDescriptor, DurableContractId, DurableIndexComponent, DurableValueShape,
    ExportId, ImageId, ImageType, LedgerIdBytes, Scalar, SemanticPath,
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
/// rest of the root's placement/product ledger identity, and the resource's durable
/// member tree.
pub(super) struct DecodedRoot {
    pub(super) name: u16,
    pub(super) keys: Vec<(Scalar, LedgerIdBytes)>,
    pub(super) record: u16,
    pub(super) placement: LedgerIdBytes,
    pub(super) product: LedgerIdBytes,
    pub(super) members: Vec<DecodedMember>,
    pub(super) indexes: Vec<DecodedIndex>,
}

/// A decoded managed index of a root: its `Index` ledger id, its `unique` flag, and
/// its ordered projection of leaf references. Each component is re-resolved against
/// the root's own top-level fields and identity keys during decode, so a component
/// referencing no real leaf is refused.
pub(super) struct DecodedIndex {
    pub(super) id: LedgerIdBytes,
    pub(super) unique: bool,
    pub(super) components: Vec<DurableIndexComponent>,
}

/// One decoded durable member, in the image's declaration order: a stored scalar
/// field, a static `group` namespace, or a keyed `branch` placement. Groups and
/// branches recurse.
pub(super) enum DecodedMember {
    Field {
        id: LedgerIdBytes,
        required: bool,
        value: DurableValueShape,
    },
    Group {
        id: LedgerIdBytes,
        members: Vec<DecodedMember>,
    },
    Branch {
        placement: LedgerIdBytes,
        /// The branch's simple name (string index), for the physical layer.
        name: u16,
        /// The branch entry's materialized record type index.
        record: u16,
        keys: Vec<(Scalar, LedgerIdBytes)>,
        members: Vec<DecodedMember>,
    },
}

impl DecodedMember {
    /// Whether this member is a field-only keyed branch, recursively — the branch shape
    /// the kernel executes at any depth. Its key is one or more columns and every direct
    /// member itself keeps flat: a field (scalar or widened composite), or a nested branch
    /// that is itself a simple branch. A static `group` breaks it. The recursion admits an
    /// arbitrarily deep chain of field-only branches with composite keys, which the
    /// recursive physical layout and profile serve.
    pub(super) fn is_simple_branch(&self) -> bool {
        matches!(
            self,
            DecodedMember::Branch { keys, members, .. }
                if !keys.is_empty()
                    && members.iter().all(DecodedMember::keeps_root_flat)
        )
    }

    /// Whether this member keeps its containing root flat-executable: a field (scalar or
    /// widened composite — the durable field codec frames a composite inline in the one
    /// field-leaf cell) or a simple (recursively field-only, keyed) branch. A static
    /// `group` and a composite/nested branch still park the root.
    fn keeps_root_flat(&self) -> bool {
        match self {
            DecodedMember::Field { .. } => true,
            DecodedMember::Group { .. } => false,
            DecodedMember::Branch { .. } => self.is_simple_branch(),
        }
    }
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
    pub(super) durable_descriptor: DurableContractDescriptor,
    pub(super) consts: Vec<SealedConst>,
    pub(super) functions: Vec<DecodedFunction>,
    pub(super) exports: Vec<(ExportId, u16)>,
    /// Decoded TEST-ENTRY rows: `(name-string-index, function-index)`, ascending by
    /// name index.
    pub(super) test_entries: Vec<(u16, u16)>,
}
