//! Member trees, key tuples, value shapes, and managed indexes: the rows under one root,
//! each declaration id claimed once across the table and each shape tied to its record.
//!
//! `decode_members` drives an explicit stack. `decode_value_shape` and
//! `value_shape_matches` recurse to `MAX_DURABLE_VALUE_DEPTH`, which `VERIFY_STACK_BYTES`
//! budgets.

use super::super::reject;
use super::super::tables::decode_bare_scalar;
use crate::reader::Reader;
use crate::reject::{
    Bound, Duplicate, Flag, Projection, Ref, Region, RejectionKind as Kind, Tag, TieFault, TieNode,
    VerifyPhase, VerifyRejection,
};
use crate::sealed::{SealedEnumType, SealedField, SealedRecordType};
use marrow_image::{
    CanonicalValueShapeDag, DeclarationMemberDef, DeclarationMemberShape, DurableContractGraph,
    DurableIndexComponent, DurableIndexShape, DurableMemberViewKind, DurableMemberViews,
    DurableProductGraph, ImageType, KeyColumn, LedgerIdBytes, NamedLeaf, Scalar, StrId, TypeId,
    ValueShapeNodeId, ValueShapeView,
};
use std::collections::{BTreeMap, BTreeSet};

/// The ledger-id accounting for one durable table. `seen` holds every *declaration*
/// id — the application, each root's placement/product/key ids, each member's
/// field/group/branch id and branch key ids, each managed index id, and each durable
/// enum's sum and member ids on the enum's first durable occurrence — which must be
/// pairwise distinct because entropy-minted ids are distinct by construction. `enums`
/// records each durable enum identity by its sum id: the ordered member ids claimed at
/// its first occurrence, each with its payload leaf names in order. A later value shape
/// carrying an already-recorded sum id — the shape a second durable field of that enum
/// emits — is a *reference* to that one per-declaration identity, so it reclaims nothing
/// and must carry the identical member ids and payload names in order.
///
/// `products` records which durable Product identities a root has already declared. A
/// later root carrying an already-recorded Product id is a *reference* to that one
/// declaration, so it reclaims none of the declaration's ids; whether it states the same
/// graph and entry record is decided by the declaration table, which holds them.
/// `placements` records the root placement ids already occupied, so a repeated root
/// occurrence is refused as itself rather than as a generic duplicate id.
#[derive(Default)]
pub(super) struct LedgerScope {
    pub(super) seen: BTreeSet<LedgerIdBytes>,
    pub(super) enums: BTreeMap<LedgerIdBytes, Vec<RecordedMember>>,
    pub(super) products: BTreeSet<LedgerIdBytes>,
    pub(super) placements: BTreeSet<LedgerIdBytes>,
}

/// One member of a recorded durable enum identity: its member id and its payload leaf
/// names in order.
pub(super) type RecordedMember = (LedgerIdBytes, Vec<Box<str>>);

/// Whether a member tree is being decoded as a Product declaration's first occurrence,
/// which claims each declaration id it reads, or as a later reference to an already
/// accepted declaration, which claims nothing and is compared against it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MemberClaim {
    Declaration,
    Reference,
}

impl MemberClaim {
    /// Read one declaration id, claiming it as fresh when this occurrence declares it.
    pub(super) fn read(
        self,
        reader: &mut Reader<'_>,
        scope: &mut LedgerScope,
    ) -> Result<LedgerIdBytes, VerifyRejection> {
        match self {
            Self::Declaration => take_distinct_id(reader, scope),
            Self::Reference => read_id(reader),
        }
    }
}

/// Read one 16-byte ledger id from the reader without claiming it.
pub(super) fn read_id(reader: &mut Reader<'_>) -> Result<LedgerIdBytes, VerifyRejection> {
    let bytes: [u8; 16] = reader
        .take(16)
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
        .try_into()
        .expect("take(16) yields 16 bytes");
    Ok(LedgerIdBytes::from_bytes(bytes))
}

/// Claim `id` as a fresh declaration id, rejecting a duplicate against those already
/// seen in this durable table. Two equal declaration ids are a forged or corrupted
/// identity block.
pub(super) fn claim_distinct(
    scope: &mut LedgerScope,
    id: LedgerIdBytes,
) -> Result<(), VerifyRejection> {
    if !scope.seen.insert(id) {
        return Err(reject(
            VerifyPhase::Table,
            Kind::Duplicate(Duplicate::LedgerId),
        ));
    }
    Ok(())
}

/// Read one 16-byte ledger id and claim it as a fresh, pairwise-distinct declaration id.
pub(super) fn take_distinct_id(
    reader: &mut Reader<'_>,
    scope: &mut LedgerScope,
) -> Result<LedgerIdBytes, VerifyRejection> {
    let id = read_id(reader)?;
    claim_distinct(scope, id)?;
    Ok(id)
}

/// Decode a placement key tuple: `count` columns, each a bare orderable durable-key
/// scalar and a distinct ledger id. Shared by roots and branches; the caller has
/// already validated `count` against `MAX_KEY_COLUMNS`.
pub(super) fn decode_key_tuple(
    reader: &mut Reader<'_>,
    count: usize,
    scope: &mut LedgerScope,
    claim: MemberClaim,
) -> Result<Vec<(Scalar, LedgerIdBytes)>, VerifyRejection> {
    let mut keys = Vec::with_capacity(count);
    for _ in 0..count {
        let key_tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?;
        let scalar = match decode_bare_scalar(key_tag) {
            Some(
                scalar @ (Scalar::Int
                | Scalar::Text
                | Scalar::Bool
                | Scalar::Bytes
                | Scalar::Date
                | Scalar::Instant),
            ) => scalar,
            _ => {
                return Err(reject(VerifyPhase::Table, Kind::KeyNotOrderable));
            }
        };
        let key_id = claim.read(reader, scope)?;
        keys.push((scalar, key_id));
    }
    Ok(keys)
}

/// Tie a root's group-inclusive materialized record to its durable member tree. The
/// record's slots run in the member tree's own top-level order with keyed branches
/// dropped: each `Field` member matches the next slot by value shape and required flag,
/// and each `Group` member matches the next slot — a bare group record — by tying its own
/// fields to the group's direct fields one level down. A slot count that disagrees, a
/// group slot that is not a group record, or any field mismatch is refused, so a hostile
/// image cannot claim one identity while executing over a different field or group shape.
///
/// Field slots precede group slots: a `Field` member after any `Group` member is refused,
/// so the record's leading scalar/widened field slots and its trailing group slots occupy
/// disjoint contiguous ranges. Sealing relies on this — a group's slot is `field_count +
/// ordinal` — so the fields-first invariant is verifier-enforced here rather than trusted
/// from the compiler.
pub(super) fn tie_root_record(
    record_fields: &[SealedField],
    members: &DurableProductGraph,
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
    values: &CanonicalValueShapeDag,
) -> Result<(), VerifyRejection> {
    let mut slots = record_fields.iter();
    let mut seen_group = false;
    for member in members.iter() {
        match member.kind() {
            DurableMemberViewKind::Field(field) => {
                let (value, required) = (field.value(), field.required());
                if seen_group {
                    return Err(reject(
                        VerifyPhase::Table,
                        Kind::RecordTie {
                            node: TieNode::Root,
                            fault: TieFault::FieldAfterGroup,
                        },
                    ));
                }
                let Some(slot) = slots.next() else {
                    return Err(reject(
                        VerifyPhase::Table,
                        Kind::RecordTie {
                            node: TieNode::Root,
                            fault: TieFault::MoreMembers,
                        },
                    ));
                };
                if required != slot.required
                    || !value_shape_matches(values, value, slot.ty, types, enums)
                {
                    return Err(reject(
                        VerifyPhase::Table,
                        Kind::RecordTie {
                            node: TieNode::Root,
                            fault: TieFault::FieldMismatch,
                        },
                    ));
                }
            }
            DurableMemberViewKind::Group(_) => {
                seen_group = true;
                let Some(slot) = slots.next() else {
                    return Err(reject(
                        VerifyPhase::Table,
                        Kind::RecordTie {
                            node: TieNode::Root,
                            fault: TieFault::MoreMembers,
                        },
                    ));
                };
                tie_group_slot(slot, member.members(), types, enums, values)?;
            }
            // A keyed branch is a distinct durable node, not a materialized record slot.
            DurableMemberViewKind::Branch(_) => {}
        }
    }
    if slots.next().is_some() {
        return Err(reject(
            VerifyPhase::Table,
            Kind::RecordTie {
                node: TieNode::Root,
                fault: TieFault::FewerMembers,
            },
        ));
    }
    Ok(())
}

/// Tie one trailing group slot of a root record to its `Group` member: the slot is a
/// bare group record whose fields match the member's direct `Field` members by value
/// shape and required flag, one level down — the same field tie the root and a branch
/// apply. A group holds only leaf fields on the executable line, so a non-record slot,
/// an optional record slot, an out-of-range record index, or a field/member mismatch is
/// refused.
fn tie_group_slot(
    slot: &SealedField,
    group_members: DurableMemberViews<'_>,
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
    values: &CanonicalValueShapeDag,
) -> Result<(), VerifyRejection> {
    let ImageType::Record { idx, optional } = slot.ty else {
        return Err(reject(
            VerifyPhase::Table,
            Kind::RecordTie {
                node: TieNode::Group,
                fault: TieFault::SlotNotGroupRecord,
            },
        ));
    };
    if optional {
        return Err(reject(
            VerifyPhase::Table,
            Kind::RecordTie {
                node: TieNode::Group,
                fault: TieFault::SlotNotGroupRecord,
            },
        ));
    }
    if idx.index() as usize >= types.len() {
        return Err(reject(
            VerifyPhase::Table,
            Kind::OutOfRange(Ref::RecordType),
        ));
    }
    let group_fields = &types[idx.index() as usize].fields;
    let mut direct_fields = group_members.filter_map(|member| match member.kind() {
        DurableMemberViewKind::Field(field) => Some((field.value(), field.required())),
        _ => None,
    });
    for field in group_fields {
        match direct_fields.next() {
            Some((value, member_required))
                if member_required == field.required
                    && value_shape_matches(values, value, field.ty, types, enums) => {}
            _ => {
                return Err(reject(
                    VerifyPhase::Table,
                    Kind::RecordTie {
                        node: TieNode::Group,
                        fault: TieFault::FieldMismatch,
                    },
                ));
            }
        }
    }
    if direct_fields.next().is_some() {
        return Err(reject(
            VerifyPhase::Table,
            Kind::RecordTie {
                node: TieNode::Group,
                fault: TieFault::MoreMembers,
            },
        ));
    }
    Ok(())
}

/// Validate every keyed `branch` in a decoded member tree: its surface name and
/// materialized record type indices are in range, and its record's fields match its
/// own direct scalar field members in order, value shape, and required flag — the
/// same tie the root's record has to its member tree, one level down. Recurses
/// through groups and branches. The name and record are surface (not identity), so
/// this is the only place they are checked; a hostile image that names a branch
/// record disagreeing with the branch's field shapes is refused here.
pub(super) fn validate_branch_records(
    members: DurableMemberViews<'_>,
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
    string_count: usize,
    values: &CanonicalValueShapeDag,
) -> Result<(), VerifyRejection> {
    // One explicit stack of member runs, in the same pre-order the recursive walk had, so
    // the first violation is refused at the same member as before while the walk's own
    // stack use stays independent of how deep the image nested its rows.
    let mut stack = vec![members];
    while let Some(run) = stack.last_mut() {
        let Some(member) = run.next() else {
            stack.pop();
            continue;
        };
        match member.kind() {
            DurableMemberViewKind::Field(_) => {}
            DurableMemberViewKind::Group(_) => stack.push(member.members()),
            DurableMemberViewKind::Branch(branch) => {
                if branch.name().index() as usize >= string_count {
                    return Err(reject(VerifyPhase::Table, Kind::OutOfRange(Ref::String)));
                }
                if branch.record().index() as usize >= types.len() {
                    return Err(reject(
                        VerifyPhase::Table,
                        Kind::OutOfRange(Ref::RecordType),
                    ));
                }
                let record_fields = &types[branch.record().index() as usize].fields;
                let mut direct_fields = member.members().filter_map(|inner| match inner.kind() {
                    DurableMemberViewKind::Field(field) => Some((field.value(), field.required())),
                    _ => None,
                });
                for field in record_fields {
                    match direct_fields.next() {
                        Some((value, member_required))
                            if member_required == field.required
                                && value_shape_matches(values, value, field.ty, types, enums) => {}
                        _ => {
                            return Err(reject(
                                VerifyPhase::Table,
                                Kind::RecordTie {
                                    node: TieNode::Branch,
                                    fault: TieFault::FieldMismatch,
                                },
                            ));
                        }
                    }
                }
                if direct_fields.next().is_some() {
                    return Err(reject(
                        VerifyPhase::Table,
                        Kind::RecordTie {
                            node: TieNode::Branch,
                            fault: TieFault::MoreMembers,
                        },
                    ));
                }
                stack.push(member.members());
            }
        }
    }
    Ok(())
}

/// Decode a durable member tree: `u16(count) ‖ member*`. A field is tag `0x00`; a
/// group is tag `0x01`; a branch is tag `0x02`. `budget` bounds the total member
/// records across the whole tree and `depth` bounds nesting, so a hostile image
/// cannot drive unbounded recursion or allocation before the bounds are rechecked
/// Every declaration ledger id is distinct across the table; a durable
/// enum's sum and member ids are the exception — one per-declaration identity a
/// later field of that enum references rather than reclaims.
pub(super) fn decode_members(
    reader: &mut Reader<'_>,
    mut budget: MemberBudget,
    scope: &mut LedgerScope,
    graph: &mut DurableContractGraph,
    claim: MemberClaim,
) -> Result<Vec<DeclarationMemberDef>, VerifyRejection> {
    let top = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
        as usize;
    let mut commands: Vec<DeclarationMemberDef> = Vec::with_capacity(top.min(budget.remaining()));
    // One explicit stack of the runs still being read. The wire order is unchanged — a
    // nested run is read where the tree wrote it — but nothing recurses and nothing owns a
    // child vector: each command names its parent by an earlier command's index, which is
    // the only shape the graph's construction accepts.
    let mut stack: Vec<PendingRun> = vec![PendingRun {
        parent: None,
        remaining: top,
    }];
    while let Some(run) = stack.last_mut() {
        if run.remaining == 0 {
            stack.pop();
            continue;
        }
        run.remaining -= 1;
        let parent = run.parent;
        budget.spend()?;
        let index = u32::try_from(commands.len())
            .map_err(|_| reject(VerifyPhase::Table, Kind::OverBound(Bound::DurableMembers)))?;
        let tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?;
        let shape = match tag {
            0x00 => {
                let id = claim.read(reader, scope)?;
                let required = match reader
                    .u8()
                    .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
                {
                    0 => false,
                    1 => true,
                    _ => {
                        return Err(reject(
                            VerifyPhase::Table,
                            Kind::Flag(Flag::DurableFieldRequired),
                        ));
                    }
                };
                let value = decode_value_shape(reader, 1, scope, graph.value_shapes_mut())?;
                DeclarationMemberShape::Field {
                    id,
                    required,
                    value,
                }
            }
            0x01 => {
                let id = claim.read(reader, scope)?;
                descend(&mut stack, reader, index)?;
                DeclarationMemberShape::Group { id }
            }
            0x02 => {
                let placement = claim.read(reader, scope)?;
                // The branch's surface name and materialized record type index follow
                // the placement. Their ranges (against the string and type tables) and
                // the record/member-field alignment are checked in
                // `validate_branch_records`, where the type and enum tables are in scope.
                let name = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?;
                let record = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?;
                let key_count = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
                    as usize;
                if key_count > marrow_image::bounds::MAX_KEY_COLUMNS {
                    return Err(reject(
                        VerifyPhase::Table,
                        Kind::OverBound(Bound::KeyColumns),
                    ));
                }
                let keys = decode_key_tuple(reader, key_count, scope, claim)?;
                descend(&mut stack, reader, index)?;
                DeclarationMemberShape::Branch {
                    placement,
                    name: StrId::from_index(name),
                    record: TypeId::from_index(record),
                    keys: keys
                        .into_iter()
                        .map(|(scalar, id)| KeyColumn { scalar, id })
                        .collect(),
                }
            }
            _ => {
                return Err(reject(
                    VerifyPhase::Table,
                    Kind::Unknown(Tag::DurableMember),
                ));
            }
        };
        commands.push(DeclarationMemberDef { parent, shape });
    }
    Ok(commands)
}

/// One member run still being read: the command that opened it, and how many of its
/// members the wire has yet to state.
struct PendingRun {
    /// The command index of the `group` or `branch` whose members this run states, or
    /// `None` for the declaration's own top-level run.
    parent: Option<u32>,
    remaining: usize,
}

/// The member rows one Product declaration may still admit.
///
/// A whole-declaration allowance that only descends, spelled as its own type so the count
/// a decode spends cannot be confused with the several other counts in scope, and so
/// spending it always answers with the same refusal.
pub(super) struct MemberBudget(usize);

impl MemberBudget {
    /// The allowance one Product's whole member tree is decoded under.
    pub(super) fn whole_declaration() -> Self {
        Self(marrow_image::bounds::MAX_DURABLE_MEMBERS)
    }

    /// Spend one member row, refusing the declaration that would overrun the allowance.
    fn spend(&mut self) -> Result<(), VerifyRejection> {
        self.0 = self.0.checked_sub(1).ok_or(reject(
            VerifyPhase::Table,
            Kind::OverBound(Bound::DurableMembers),
        ))?;
        Ok(())
    }

    /// The rows still admissible, for sizing the command vector a hostile count must not
    /// be able to preallocate past.
    fn remaining(&self) -> usize {
        self.0
    }
}

/// Enter the nested member run a `group` or `branch` command opened: check the nesting
/// bound the level below would occupy, then read that level's count.
///
/// The bound is checked before the count is read, exactly where the recursive decode
/// checked it on entry, so an over-deep image is refused at the same byte.
fn descend(
    stack: &mut Vec<PendingRun>,
    reader: &mut Reader<'_>,
    parent: u32,
) -> Result<(), VerifyRejection> {
    if stack.len() + 1 > marrow_image::bounds::MAX_DURABLE_DEPTH {
        return Err(reject(
            VerifyPhase::Table,
            Kind::OverBound(Bound::DurableDepth),
        ));
    }
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
        as usize;
    stack.push(PendingRun {
        parent: Some(parent),
        remaining: count,
    });
    Ok(())
}

/// Decode a root's managed indexes: `u16(count) ‖ index*`. Each index is its distinct
/// `Index` ledger id, a `unique` flag byte, a `u16(component_count)`, and per component
/// a one-byte leaf kind (`0x02` field, `0x04` key) and the leaf's 16-byte ledger id.
/// Every component id is re-resolved against this root's own top-level field ids
/// (kind `0x02`) or identity key ids (kind `0x04`), so a projection over a leaf that
/// does not exist on the root is refused. The index id is distinct across the whole
/// durable table (via `seen`); component ids are references to already-seen leaf ids
/// and so are not added to `seen`.
pub(super) fn decode_indexes(
    reader: &mut Reader<'_>,
    keys: &[(Scalar, LedgerIdBytes)],
    members: &DurableProductGraph,
    scope: &mut LedgerScope,
    values: &CanonicalValueShapeDag,
) -> Result<Vec<DurableIndexShape>, VerifyRejection> {
    let field_ids: Vec<LedgerIdBytes> = members
        .iter()
        .filter_map(|member| match member.kind() {
            DurableMemberViewKind::Field(field) => Some(field.id()),
            _ => None,
        })
        .collect();
    // A managed-index field component must project one of the compiler's closed set of
    // orderable durable-key scalar shapes. Field executability is independent: Duration
    // and widened values can be stored but are not index-eligible.
    let index_eligible_field_ids: Vec<LedgerIdBytes> = members
        .iter()
        .filter_map(|member| match member.kind() {
            // A field whose shape names no node of this arena is not eligible. The
            // lookup is checked because the decoded image supplies both the id and the
            // arena, and refusing eligibility can only narrow what an index may name.
            DurableMemberViewKind::Field(field) => match values.view(field.value())? {
                ValueShapeView::Scalar(
                    Scalar::Int
                    | Scalar::Text
                    | Scalar::Bool
                    | Scalar::Bytes
                    | Scalar::Date
                    | Scalar::Instant,
                ) => Some(field.id()),
                ValueShapeView::Scalar(Scalar::Duration)
                | ValueShapeView::Struct(_)
                | ValueShapeView::Enum { .. } => None,
            },
            DurableMemberViewKind::Group(_) | DurableMemberViewKind::Branch(_) => None,
        })
        .collect();
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
        as usize;
    if count > marrow_image::bounds::MAX_INDEXES {
        return Err(reject(VerifyPhase::Table, Kind::OverBound(Bound::Indexes)));
    }
    let mut indexes = Vec::with_capacity(count);
    for _ in 0..count {
        let id = take_distinct_id(reader, scope)?;
        let unique = match reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
        {
            0 => false,
            1 => true,
            _ => {
                return Err(reject(VerifyPhase::Table, Kind::Flag(Flag::IndexUnique)));
            }
        };
        let component_count = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
            as usize;
        if component_count > marrow_image::bounds::MAX_INDEX_COMPONENTS {
            return Err(reject(
                VerifyPhase::Table,
                Kind::OverBound(Bound::IndexComponents),
            ));
        }
        let mut components = Vec::with_capacity(component_count);
        for _ in 0..component_count {
            let kind = reader
                .u8()
                .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?;
            let leaf: [u8; 16] = reader
                .take(16)
                .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
                .try_into()
                .expect("take(16) yields 16 bytes");
            let leaf = LedgerIdBytes::from_bytes(leaf);
            let component = match kind {
                0x02 => {
                    if !index_eligible_field_ids.contains(&leaf) {
                        let fault = if field_ids.contains(&leaf) {
                            Projection::FieldNotEligible
                        } else {
                            Projection::FieldUnknown
                        };
                        return Err(reject(VerifyPhase::Table, Kind::IndexProjection(fault)));
                    }
                    DurableIndexComponent::Field(leaf)
                }
                0x04 => {
                    if !keys.iter().any(|(_, key_id)| *key_id == leaf) {
                        return Err(reject(
                            VerifyPhase::Table,
                            Kind::IndexProjection(Projection::KeyUnknown),
                        ));
                    }
                    DurableIndexComponent::Key(leaf)
                }
                _ => {
                    return Err(reject(
                        VerifyPhase::Table,
                        Kind::Unknown(Tag::IndexComponent),
                    ));
                }
            };
            components.push(component);
        }
        // Re-enforce projection well-formedness the compiler owns: a reference-valid but
        // malformed projection (an empty projection, a repeated component, or a
        // non-unique index whose identity suffix is missing, misordered, or preceded by a
        // key) must never reach the sealed index model the runtime trusts to order rows.
        if let Err(fault) = validate_index_projection(unique, &components, keys) {
            return Err(reject(VerifyPhase::Table, Kind::IndexProjection(fault)));
        }
        indexes.push(DurableIndexShape {
            id,
            unique,
            components,
        });
    }
    Ok(indexes)
}

/// Re-check one decoded index's projection against the closed well-formedness rules the
/// compiler owns, so a hostile image cannot smuggle a malformed projection past the
/// verifier. Every component id is already re-resolved to a real scalar field or identity
/// key of the root (the orderable-key predicate); this owns the ordering and cardinality
/// rules: the projection is non-empty, no component repeats, and a non-unique index ends
/// with exactly the identity keys in declaration order — the row-distinguishing suffix. A
/// unique index carries no suffix obligation.
///
/// The no-leading-key rule (a non-unique index carries no identity key before its suffix)
/// needs no separate branch: distinctness forbids any component from repeating, and the
/// suffix must already hold every identity key, so a leading identity key would duplicate
/// a suffix key and is rejected by the distinctness check.
fn validate_index_projection(
    unique: bool,
    components: &[DurableIndexComponent],
    keys: &[(Scalar, LedgerIdBytes)],
) -> Result<(), Projection> {
    if components.is_empty() {
        return Err(Projection::Empty);
    }
    for (position, component) in components.iter().enumerate() {
        if components[..position]
            .iter()
            .any(|earlier| earlier.id() == component.id())
        {
            return Err(Projection::RepeatedComponent);
        }
    }
    if !unique {
        // The trailing `keys.len()` components must be exactly the identity keys in
        // declaration order.
        if components.len() < keys.len() {
            return Err(Projection::MissingIdentitySuffix);
        }
        let suffix_start = components.len() - keys.len();
        for (offset, (_, key_id)) in keys.iter().enumerate() {
            match components[suffix_start + offset] {
                DurableIndexComponent::Key(id) if id == *key_id => {}
                _ => {
                    return Err(Projection::MissingIdentitySuffix);
                }
            }
        }
    }
    Ok(())
}

/// The arena's closed builder-domain refusal, as a table rejection. The verifier is the
/// image's only decoder and every image reaching it is untrusted, so an arena that
/// refuses a mint — a leaf outside it, or an arena at its carrier ceiling — is a
/// rejected image rather than an aborted verification.
fn mint(
    minted: Result<ValueShapeNodeId, marrow_image::DraftStateError>,
) -> Result<ValueShapeNodeId, VerifyRejection> {
    minted.map_err(|_| reject(VerifyPhase::Table, Kind::ValueArenaExhausted))
}

/// Decode a durable field's stored value shape into `values`, returning a reference to
/// the node it minted: `u8(value_tag) ‖ body`. A scalar is tag `0x00` (a bare scalar); a
/// dense struct is tag `0x01` (`u16(count) ‖ leaf*`); a closed enum is tag `0x02`
/// (`sum id ‖ u16(count) ‖ [member id ‖ u16(payload) ‖ leaf*]*`); each leaf is
/// `0x03 ‖ u16(name_len) ‖ name ‖ value` (see [`decode_leaf`]). Leaves are minted
/// before the shape that references them, so the wire tree is consumed without ever
/// being materialized as one — a shape the image spells twice is decoded into the one
/// node the arena already holds. An enum's sum and member ids are the identity of the
/// enum *declaration*. The first occurrence of a given sum id claims it and its member
/// ids as fresh pairwise-distinct ids; a later occurrence carrying an already-claimed sum
/// id — the shape a second durable field of that enum emits — is a reference that
/// reclaims nothing and must carry the identical member ids and payload names in order.
/// `depth` bounds nesting so a hostile image cannot drive unbounded recursion before the
/// value shape is rechecked.
fn decode_value_shape(
    reader: &mut Reader<'_>,
    depth: usize,
    scope: &mut LedgerScope,
    values: &mut CanonicalValueShapeDag,
) -> Result<ValueShapeNodeId, VerifyRejection> {
    if depth > marrow_image::bounds::MAX_DURABLE_VALUE_DEPTH {
        return Err(reject(
            VerifyPhase::Table,
            Kind::OverBound(Bound::ValueDepth),
        ));
    }
    let tag = reader
        .u8()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?;
    match tag {
        0x00 => {
            let scalar_tag = reader
                .u8()
                .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?;
            let scalar = decode_bare_scalar(scalar_tag)
                .ok_or(reject(VerifyPhase::Table, Kind::Unknown(Tag::ValueScalar)))?;
            mint(values.scalar(scalar))
        }
        0x01 => {
            let count = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
                as usize;
            if count > marrow_image::bounds::MAX_STRUCT_LEAVES {
                return Err(reject(
                    VerifyPhase::Table,
                    Kind::OverBound(Bound::StructLeaves),
                ));
            }
            let mut leaves = Vec::with_capacity(count);
            for _ in 0..count {
                leaves.push(decode_leaf(reader, depth + 1, scope, values)?);
            }
            mint(values.struct_shape(leaves))
        }
        0x02 => {
            let sum = read_id(reader)?;
            let member_count = reader
                .u16()
                .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
                as usize;
            if member_count > marrow_image::bounds::MAX_VARIANTS {
                return Err(reject(VerifyPhase::Table, Kind::OverBound(Bound::Variants)));
            }
            // An enum reached before (by its sum id) is a reference to that one
            // per-declaration identity: it reclaims neither the sum nor any member id,
            // and must present the identical member ids and payload names in the
            // identical order. The recorded identity is the ordered member ids and each
            // member's payload names; each occurrence's payload shapes are tied to the
            // field's own enum-table entry by `value_shape_matches`, so a payload shape
            // divergence is caught there rather than against the first occurrence.
            let recorded = scope.enums.get(&sum).cloned();
            match &recorded {
                Some(recorded_ids) if recorded_ids.len() != member_count => {
                    return Err(reject(VerifyPhase::Table, Kind::EnumIdentityReused));
                }
                Some(_) => {}
                None => claim_distinct(scope, sum)?,
            }
            let mut members: Vec<(LedgerIdBytes, Vec<NamedLeaf>)> =
                Vec::with_capacity(member_count);
            for index in 0..member_count {
                let id = read_id(reader)?;
                match &recorded {
                    Some(recorded_ids) if recorded_ids[index].0 != id => {
                        return Err(reject(VerifyPhase::Table, Kind::EnumIdentityReused));
                    }
                    Some(_) => {}
                    None => claim_distinct(scope, id)?,
                }
                let payload_count = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
                    as usize;
                if payload_count > marrow_image::bounds::MAX_PAYLOAD_FIELDS {
                    return Err(reject(
                        VerifyPhase::Table,
                        Kind::OverBound(Bound::PayloadFields),
                    ));
                }
                let mut payload = Vec::with_capacity(payload_count);
                for _ in 0..payload_count {
                    payload.push(decode_leaf(reader, depth + 1, scope, values)?);
                }
                if let Some(recorded_ids) = &recorded
                    && !recorded_ids[index]
                        .1
                        .iter()
                        .eq(payload.iter().map(|(name, _)| name))
                {
                    return Err(reject(VerifyPhase::Table, Kind::EnumIdentityReused));
                }
                members.push((id, payload));
            }
            if recorded.is_none() {
                let identity = members
                    .iter()
                    .map(|(id, payload)| {
                        (*id, payload.iter().map(|(name, _)| name.clone()).collect())
                    })
                    .collect();
                scope.enums.insert(sum, identity);
            }
            mint(values.enum_shape(sum, members))
        }
        _ => Err(reject(VerifyPhase::Table, Kind::Unknown(Tag::DurableValue))),
    }
}

/// Decode one struct or payload leaf: `0x03 ‖ u16(name_len) ‖ name ‖ value`. The name is
/// read like a string-table entry — bounded by `MAX_STRING_BYTES` and valid UTF-8 — and
/// must not be empty; each fault is refused with its own kind before anything is minted,
/// so the arena's own refusal of an empty name is never reported as exhaustion.
fn decode_leaf(
    reader: &mut Reader<'_>,
    depth: usize,
    scope: &mut LedgerScope,
    values: &mut CanonicalValueShapeDag,
) -> Result<NamedLeaf, VerifyRejection> {
    let truncated = || reject(VerifyPhase::Table, Kind::Truncated(Region::Durable));
    if reader.u8().ok_or_else(truncated)? != 0x03 {
        return Err(reject(VerifyPhase::Table, Kind::Unknown(Tag::DurableLeaf)));
    }
    let len = reader.u16().ok_or_else(truncated)? as usize;
    if len > marrow_image::bounds::MAX_STRING_BYTES {
        return Err(reject(
            VerifyPhase::Table,
            Kind::OverBound(Bound::StringBytes),
        ));
    }
    if len == 0 {
        return Err(reject(VerifyPhase::Table, Kind::EmptyLeafName));
    }
    let raw = reader.take(len).ok_or_else(truncated)?;
    let name =
        std::str::from_utf8(raw).map_err(|_| reject(VerifyPhase::Table, Kind::InvalidUtf8))?;
    let name = Box::from(name);
    Ok((name, decode_value_shape(reader, depth, scope, values)?))
}

/// Whether a decoded durable field value shape structurally matches the materialized
/// record field type it claims, recursing through the record and enum tables. The
/// ledger ids a value shape carries (a struct records none; an enum a sum and per-
/// member id) are durable identity, verified by pairwise distinctness and the
/// contract-id recomputation — this match ties the *structure*, and each struct leaf's
/// declared name, to the executable record so a hostile image cannot claim one durable
/// identity while its record carries a different value shape. A nominal field erases to
/// its base scalar, so it matches a bare scalar exactly like a plain scalar field.
fn value_shape_matches(
    values: &CanonicalValueShapeDag,
    shape: ValueShapeNodeId,
    ty: ImageType,
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
) -> bool {
    // A shape naming no node of this arena matches no record type: the decoded image
    // supplies both the id and the arena, so a dangling reference is a mismatch rather
    // than an abort inside the only decoder a hostile image reaches.
    let Some(view) = values.view(shape) else {
        return false;
    };
    match (view, ty) {
        (
            ValueShapeView::Scalar(shape_scalar),
            ImageType::Scalar {
                scalar,
                optional: false,
            },
        ) => shape_scalar == scalar,
        (
            ValueShapeView::Struct(leaves),
            ImageType::Record {
                idx,
                optional: false,
            },
        ) => {
            let Some(record) = types.get(idx.index() as usize) else {
                return false;
            };
            // A durable struct value is dense: every leaf is a required bare field, and
            // leaf and field agree on name and shape position by position.
            record.fields.len() == leaves.len()
                && record.fields.iter().zip(leaves).all(|(field, leaf)| {
                    field.required
                        && *field.name == *leaf.name()
                        && value_shape_matches(values, leaf.shape(), field.ty, types, enums)
                })
        }
        (
            ValueShapeView::Enum { members, .. },
            ImageType::Enum {
                idx,
                optional: false,
            },
        ) => {
            let Some(enum_def) = enums.get(idx.index() as usize) else {
                return false;
            };
            enum_def.variants.len() == members.len()
                && enum_def
                    .variants
                    .iter()
                    .zip(members)
                    .all(|(variant, member)| {
                        variant.payload.len() == member.payload().len()
                            && variant.payload.iter().zip(member.payload()).all(
                                |(leaf_ty, leaf)| {
                                    value_shape_matches(
                                        values,
                                        leaf.shape(),
                                        *leaf_ty,
                                        types,
                                        enums,
                                    )
                                },
                            )
                    })
        }
        _ => false,
    }
}
