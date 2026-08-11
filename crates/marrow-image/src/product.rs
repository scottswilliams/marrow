//! The durable Product declaration table and the flat root-occurrence table.
//!
//! A durable **Product** is a declaration: the resource type a `store` root projects.
//! One Product declaration has one canonical member/value graph — its top-level fields,
//! static `group` namespaces, and nested keyed `branch` placements, each with its ledger
//! identity and value shape — and one runtime surface, however many roots occur over it.
//! A **root** is an occurrence: its own placement identity, spelling, key tuple, and
//! managed indexes, referencing the one declaration.
//!
//! The draft therefore holds two flat tables rather than one graph per root: a
//! [`ProductDeclarationTable`] keyed by [`DurableProductIdentity`], and a flat
//! [`RootOccurrence`] row per root that references its declaration. The wire format is
//! unchanged — v0 still carries the full member graph per root — so the encoder projects
//! each occurrence from its one retained declaration. Nothing is retained per
//! (root x member).

use std::collections::BTreeMap;

use crate::draft::{DurableMemberDef, KeyColumn, StrId, TypeId};
use crate::durable_id::{DurableIndexShape, DurableProductIdentity, RootPlacementIdentity};

/// The exact comparison boundary between two occurrences of one Product declaration:
/// its ledger identity and its canonical resource member/value graph.
///
/// The graph is the whole recursive member tree — every field id, `required` flag, and
/// value shape, every static `group`, and every nested keyed `branch` placement together
/// with its key tuple and its own members. Those are Product facts: they are declared by
/// the resource, not by the root that projects it. The *outer* root's placement identity,
/// spelling, key tuple, and managed indexes are occurrence facts and are deliberately not
/// in the claim, so two roots may share a Product while carrying different placements,
/// key tuples, and index shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DurableProductClaim {
    identity: DurableProductIdentity,
    members: Vec<DurableMemberDef>,
}

impl DurableProductClaim {
    pub(crate) fn new(identity: DurableProductIdentity, members: Vec<DurableMemberDef>) -> Self {
        Self { identity, members }
    }

    pub(crate) fn identity(&self) -> DurableProductIdentity {
        self.identity
    }

    pub(crate) fn members(&self) -> &[DurableMemberDef] {
        &self.members
    }
}

/// The runtime shape one Product declaration binds: the materialized entry record its
/// roots read and write.
///
/// A nested branch's runtime facts — its placement, interned name, and entry record type
/// — are carried by the declaration's own member graph ([`DurableMemberDef::Branch`]),
/// which is already part of [`DurableProductClaim`] and compared with it. They are not
/// copied here: the root entry record is the one such fact the member graph does not
/// carry, because it belongs to the resource rather than to any one member.
///
/// These facts are on the wire in the DURABLE section but are excluded from the durable
/// contract-id preimage, so binding them adds no contract bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProductEntryRecordClaim {
    root_entry_record: TypeId,
}

impl ProductEntryRecordClaim {
    pub(crate) fn new(root_entry_record: TypeId) -> Self {
        Self { root_entry_record }
    }

    pub(crate) fn root_entry_record(&self) -> TypeId {
        self.root_entry_record
    }
}

/// One admitted Product declaration: its claim and the runtime surface bound with it.
#[derive(Debug, Clone)]
pub(crate) struct ProductDeclaration {
    claim: DurableProductClaim,
    surface: ProductEntryRecordClaim,
}

impl ProductDeclaration {
    pub(crate) fn members(&self) -> &[DurableMemberDef] {
        self.claim.members()
    }

    pub(crate) fn identity(&self) -> DurableProductIdentity {
        self.claim.identity()
    }

    pub(crate) fn root_entry_record(&self) -> TypeId {
        self.surface.root_entry_record()
    }
}

/// Why a later occurrence of an already-declared Product was refused.
///
/// One Product ledger identity may serve many root occurrences exactly when every
/// occurrence's claim is identical. A divergent occurrence is two different declarations
/// wearing one identity; the draft records the first such conflict and the encoder
/// refuses to emit the image, so a divergent graph can never reach an accepted artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProductClaimConflict {
    /// The occurrence declares a different member/value graph.
    Graph(DurableProductIdentity),
    /// The occurrence declares the same graph with a different entry record.
    EntryRecord(DurableProductIdentity),
}

/// The flat table of durable Product declarations, keyed by Product ledger identity.
#[derive(Debug, Default, Clone)]
pub(crate) struct ProductDeclarationTable {
    rows: Vec<ProductDeclaration>,
    by_identity: BTreeMap<DurableProductIdentity, usize>,
}

impl ProductDeclarationTable {
    /// Admit one root's Product claim, returning the declaration row it references.
    ///
    /// The first occurrence of a Product identity claims the row and binds its graph and
    /// surface; a later occurrence is a reference that reclaims nothing and must match
    /// both exactly.
    pub(crate) fn admit(
        &mut self,
        claim: DurableProductClaim,
        surface: ProductEntryRecordClaim,
    ) -> Result<usize, ProductClaimConflict> {
        let identity = claim.identity();
        let Some(&row) = self.by_identity.get(&identity) else {
            let row = self.rows.len();
            self.by_identity.insert(identity, row);
            self.rows.push(ProductDeclaration { claim, surface });
            return Ok(row);
        };
        let declared = &self.rows[row];
        if declared.claim.members() != claim.members() {
            return Err(ProductClaimConflict::Graph(identity));
        }
        if declared.surface != surface {
            return Err(ProductClaimConflict::EntryRecord(identity));
        }
        Ok(row)
    }

    pub(crate) fn declaration(&self, row: usize) -> &ProductDeclaration {
        &self.rows[row]
    }

    pub(crate) fn by_identity(
        &self,
        identity: DurableProductIdentity,
    ) -> Option<&ProductDeclaration> {
        self.by_identity.get(&identity).map(|row| &self.rows[*row])
    }

    pub(crate) fn declarations(&self) -> &[ProductDeclaration] {
        &self.rows
    }

    /// Drop every declaration appended after `len`, restoring the table to the exact
    /// rows it held at that mark. The table is append-only, so a row's index — the
    /// occurrence rows' reference into it — is stable across the truncation.
    pub(crate) fn truncate(&mut self, len: usize) {
        self.rows.truncate(len);
        self.by_identity.retain(|_, row| *row < len);
    }
}

/// One flat root-occurrence row: the occurrence facts of one `store` root, referencing
/// the Product declaration it projects.
#[derive(Debug, Clone)]
pub(crate) struct RootOccurrence {
    declaration: usize,
    name: StrId,
    keys: Vec<KeyColumn>,
    placement: RootPlacementIdentity,
    indexes: Vec<DurableIndexShape>,
}

impl RootOccurrence {
    pub(crate) fn new(
        declaration: usize,
        name: StrId,
        keys: Vec<KeyColumn>,
        placement: RootPlacementIdentity,
        indexes: Vec<DurableIndexShape>,
    ) -> Self {
        Self {
            declaration,
            name,
            keys,
            placement,
            indexes,
        }
    }

    pub(crate) fn declaration(&self) -> usize {
        self.declaration
    }

    pub(crate) fn name(&self) -> StrId {
        self.name
    }

    pub(crate) fn keys(&self) -> &[KeyColumn] {
        &self.keys
    }

    pub(crate) fn placement(&self) -> RootPlacementIdentity {
        self.placement
    }

    pub(crate) fn indexes(&self) -> &[DurableIndexShape] {
        &self.indexes
    }
}
