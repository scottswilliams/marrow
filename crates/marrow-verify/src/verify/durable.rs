//! Phase 2 durable graph: root/branch/group/index decoding, sealing, and shape descriptors.

use super::model::{
    DecodedEnum, DecodedField, DecodedIndex, DecodedMember, DecodedRecordType, DecodedRoot,
};
use super::reject;
use super::tables::decode_bare_scalar;
use crate::reader::Reader;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::{
    SealedBranch, SealedGroup, SealedIndexComponent, SealedRecordType, SealedSite, SealedSiteTarget,
};
use marrow_image::{
    DurableBranchShape, DurableContractDescriptor, DurableContractId, DurableEnumMemberShape,
    DurableFieldShape, DurableGroupShape, DurableIndexComponent, DurableIndexShape,
    DurableKeyShape, DurableMemberShape, DurableRootShape, DurableValueShape, ImageType,
    LedgerIdBytes, Scalar, SemanticNode, SemanticNodeKind, SemanticPath, SemanticStep,
    SemanticStepKind, SemanticTarget,
};
use std::collections::BTreeMap;
use std::rc::Rc;

/// Decode the DURABLE table (design §C 0x03): up to `MAX_ROOTS` roots — preceded,
/// when any root is present, by the application's 16-byte ledger id — then the operation
/// sites, then the 32-byte durable-contract id closing the section. Each root
/// carries its ledger identity block (placement, product, and key ids plus one id
/// per record field). Every site is revalidated against the roots and record
/// types, every declaration ledger id in the section must be pairwise distinct
/// (a durable enum's sum and member ids are one per-declaration identity that
/// later fields of that enum reference rather than reclaim), and the
/// contract id is independently recomputed from the decoded graph and checked
/// against the carried bytes.
/// The decoded durable graph: the roots, the sealed operation sites, each site's
/// resolved graph-node path (parallel to the sites), the recomputed contract id, and
/// the canonical descriptor the paths and id were derived from.
type DecodedDurable = (
    Vec<DecodedRoot>,
    Vec<SealedSite>,
    Vec<SemanticPath>,
    DurableContractId,
    DurableContractDescriptor,
);

pub(super) fn decode_durable(
    body: &[u8],
    strings: &[Rc<str>],
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
) -> Result<DecodedDurable, VerifyRejection> {
    let string_count = strings.len();
    let mut reader = Reader::new(body);
    let root_count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short root count"))? as usize;
    if root_count > marrow_image::bounds::MAX_ROOTS {
        return Err(reject(VerifyPhase::Table, "too many durable roots"));
    }
    let mut scope = LedgerScope::default();
    let application = if root_count > 0 {
        Some(take_distinct_id(
            &mut reader,
            &mut scope,
            "short application identity",
        )?)
    } else {
        None
    };
    let mut roots = Vec::with_capacity(root_count);
    for _ in 0..root_count {
        let name = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short root name"))?;
        if name as usize >= string_count {
            return Err(reject(VerifyPhase::Table, "root name index out of range"));
        }
        // The key tuple: a count, then each column's scalar type and distinct
        // ledger id. Zero columns is a singleton root; the closed orderable
        // durable-key scalar set (frozen at C04) admits int, string, bool, bytes,
        // date, and instant per column (`duration` is a span, not an identity).
        let key_count = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short root key count"))?
            as usize;
        if key_count > marrow_image::bounds::MAX_KEY_COLUMNS {
            return Err(reject(VerifyPhase::Table, "too many root key columns"));
        }
        let keys = decode_key_tuple(&mut reader, key_count, &mut scope)?;
        let record = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short root record"))?;
        if record as usize >= types.len() {
            return Err(reject(
                VerifyPhase::Table,
                "root record type index out of range",
            ));
        }
        let placement = take_distinct_id(&mut reader, &mut scope, "short placement identity")?;
        let product = take_distinct_id(&mut reader, &mut scope, "short product identity")?;
        // The resource's durable member tree: top-level fields interleaved with
        // static `group` namespaces and keyed `branch` placements. A field's stored
        // value is drawn from the closed acyclic durable value set (a bare scalar, a
        // dense struct, or a closed enum with sum/member ids).
        let mut member_budget = marrow_image::bounds::MAX_DURABLE_MEMBERS;
        let members = decode_members(&mut reader, 1, &mut member_budget, &mut scope)?;
        // The member tree's top-level fields and groups are exactly the materialized
        // record's stored field slots followed by its trailing group slots, in order and
        // value shape: this ties the durable identity to the executable record so a
        // hostile image cannot claim one identity while executing over a different field
        // or group shape. A field slot's value-shape match recurses through the record and
        // enum tables, so a widened field (a nominal, struct, or enum) is checked as
        // thoroughly as a plain scalar; each group slot is a group record whose own fields
        // tie to its `Group` member's direct fields one level down.
        let record_fields = &types[record as usize].fields;
        tie_root_record(record_fields, &members, types, enums)?;
        // Every keyed `branch` nested in the tree ties its own materialized record to
        // its direct field members the same way, one level down, so a hostile image
        // cannot claim a branch identity while executing over a different record shape.
        validate_branch_records(&members, types, enums, string_count)?;
        // The root's managed indexes follow its member tree. Each index's `Index`
        // ledger id is a distinct id across the whole table; each projected component
        // must reference a real top-level field or identity key of this same root, so a
        // hostile image cannot forge a projection over a leaf that does not exist.
        let indexes = decode_indexes(&mut reader, &keys, &members, &mut scope)?;
        roots.push(DecodedRoot {
            name,
            keys,
            record,
            placement,
            product,
            members,
            indexes,
        });
    }

    // A root's name keys its physical cell family, so two roots that resolve to the same
    // name would share one family — a later write to one silently overwriting the other.
    // The escape encoding is injective, so distinct name strings never collide physically;
    // reject only an image whose roots resolve to the same name string. Placement/product/
    // key ledger ids are already distinct across the table (`take_distinct_id`), so this
    // closes the one remaining cross-root physical-collision axis.
    for (i, root) in roots.iter().enumerate() {
        for other in &roots[..i] {
            if strings[root.name as usize] == strings[other.name as usize] {
                return Err(reject(VerifyPhase::Table, "two durable roots share a name"));
            }
        }
    }

    // Reconstruct the durable graph's node set now, from the same descriptor the
    // contract id is computed over, so every operation site resolves against this
    // verifier's own derivation of the graph rather than a compiler-side summary.
    let descriptor = durable_descriptor(application, &roots);
    let nodes = descriptor.semantic_nodes();

    let site_count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short site count"))? as usize;
    if site_count > marrow_image::bounds::MAX_SITES {
        return Err(reject(VerifyPhase::Table, "too many durable sites"));
    }
    let mut sites: Vec<SealedSite> = Vec::with_capacity(site_count);
    // Each site's resolved graph-node path, parallel to `sites` by index. The demand
    // reconstruction maps a durable opcode's site index to the semantic path of the
    // node it addresses; a flat site drops the path from its executable form, so it
    // is retained here rather than re-derived.
    let mut site_paths: Vec<SemanticPath> = Vec::with_capacity(site_count);
    for _ in 0..site_count {
        let (site, path) = decode_site(&mut reader, &nodes, &roots)?;
        // Sites are unique by their resolved identity: a flat site by (root, target),
        // a parked site by (path, target). Full structural equality covers both, and a
        // flat and a parked site can never collide.
        if sites.contains(&site) {
            return Err(reject(VerifyPhase::Table, "duplicate durable site"));
        }
        sites.push(site);
        site_paths.push(path);
    }

    // The section closes with the 32-byte durable-contract id. Recompute it
    // independently from the decoded graph — never trust the carried bytes — and
    // reject a mismatch, so a hostile image that mutates a root or field shape
    // without re-minting the contract is refused here.
    let carried: [u8; 32] = reader
        .take(32)
        .ok_or(reject(VerifyPhase::Table, "short durable contract id"))?
        .try_into()
        .expect("take(32) yields 32 bytes");
    if !reader.is_empty() {
        return Err(reject(
            VerifyPhase::Table,
            "trailing bytes in durable table",
        ));
    }
    let recomputed = descriptor.contract_id();
    if recomputed.bytes() != &carried {
        return Err(reject(
            VerifyPhase::Table,
            "durable contract id does not match the durable graph",
        ));
    }
    Ok((roots, sites, site_paths, recomputed, descriptor))
}

/// Decode one operation site — its semantic path then its target-kind byte — and
/// resolve it against the reconstructed node set. The path is `u8(step_count) ‖
/// [u8(ledger_kind) ‖ 16 id bytes]*`; the target byte is `0x00` whole-payload or
/// `0x01` field-leaf. Nothing here is trusted: the path is resolved to a node and
/// its kind cross-checked, and the executable physical facts are re-derived, so a
/// forged path, a flipped target byte, or a mutated ledger id is refused.
fn decode_site(
    reader: &mut Reader<'_>,
    nodes: &[SemanticNode],
    roots: &[DecodedRoot],
) -> Result<(SealedSite, SemanticPath), VerifyRejection> {
    let step_count = reader
        .u8()
        .ok_or(reject(VerifyPhase::Table, "short site path length"))? as usize;
    if step_count < marrow_image::bounds::MIN_SITE_PATH_STEPS {
        return Err(reject(
            VerifyPhase::Table,
            "durable site path names no graph node",
        ));
    }
    if step_count > marrow_image::bounds::MAX_SITE_PATH_STEPS {
        return Err(reject(VerifyPhase::Table, "durable site path too deep"));
    }
    let mut steps = Vec::with_capacity(step_count);
    for _ in 0..step_count {
        let kind_byte = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short site path step kind"))?;
        let kind = SemanticStepKind::from_ledger_kind(kind_byte)
            .ok_or(reject(VerifyPhase::Table, "unknown site path step kind"))?;
        let id_bytes: [u8; 16] = reader
            .take(16)
            .ok_or(reject(VerifyPhase::Table, "short site path step id"))?
            .try_into()
            .expect("take(16) yields 16 bytes");
        steps.push(SemanticStep::new(kind, LedgerIdBytes::from_bytes(id_bytes)));
    }
    let target = match reader
        .u8()
        .ok_or(reject(VerifyPhase::Table, "short site target"))?
    {
        0x00 => SemanticTarget::WholePayload,
        0x01 => SemanticTarget::FieldLeaf,
        0x02 => SemanticTarget::IndexScan,
        0x03 => SemanticTarget::IndexLookup,
        0x04 => SemanticTarget::GroupEntry,
        _ => return Err(reject(VerifyPhase::Table, "unknown site target tag")),
    };
    let site = resolve_site(&steps, target, nodes, roots)?;
    // The site's node path is the chain it resolved against — retained parallel to
    // the sealed site so demand reconstruction can name the node a flat site
    // addresses without re-deriving it from the executable form.
    Ok((site, SemanticPath::from_steps(steps)))
}

/// Resolve a decoded site path plus target kind to a [`SealedSite`]. A path that
/// names no reconstructed node, or a target whose kind disagrees with the resolved
/// node's kind, is refused. A whole-payload, keyed-branch-entry, or field-leaf site on
/// a flat-executable keyed root seals as [`SealedSite::Flat`] with its re-derived root
/// index and (for a field leaf) resolved field index — widened field values, composite
/// keys, and keyed branches nested to any depth all execute. A managed-index read also
/// seals [`SealedSite::Flat`] with its re-derived index position and read-kind target. A
/// site on a non-flat root — a singleton (keyless) root, or a group at any level — seals
/// as [`SealedSite::Parked`], carrying the resolved path and target. Both forms re-derive
/// everything from the reconstructed graph, never trusting the image.
fn resolve_site(
    steps: &[SemanticStep],
    target: SemanticTarget,
    nodes: &[SemanticNode],
    roots: &[DecodedRoot],
) -> Result<SealedSite, VerifyRejection> {
    let node = nodes
        .iter()
        .find(|node| node.path.steps() == steps)
        .ok_or(reject(
            VerifyPhase::Table,
            "durable site path does not resolve to a graph node",
        ))?;
    // The target kind must agree with the resolved node's kind: a whole-payload
    // target names a keyed placement, a field-leaf target names a stored field, and an
    // index scan/lookup target names a managed index node.
    match (target, node.kind) {
        (SemanticTarget::WholePayload, SemanticNodeKind::Root | SemanticNodeKind::Branch) => {}
        (SemanticTarget::FieldLeaf, SemanticNodeKind::Field) => {}
        (SemanticTarget::GroupEntry, SemanticNodeKind::Group) => {}
        (SemanticTarget::IndexScan | SemanticTarget::IndexLookup, SemanticNodeKind::Index) => {}
        _ => {
            return Err(reject(
                VerifyPhase::Table,
                "durable site target kind does not match its resolved graph node",
            ));
        }
    }
    // An index read site resolves to its managed index and seals flat-executable, carrying
    // the index's global position and read-kind target for the VM's bounded scan/lookup. The
    // read kind must agree with the index's `unique` flag: a nonunique index admits
    // only a progressive-prefix `IndexScan`, and a unique index admits only a
    // complete-key `IndexLookup`. This is where a site that claims to *traverse* a
    // unique index — or to exact-lookup a nonunique one — is refused, so source can
    // never observe siblings through a unique index.
    if matches!(
        target,
        SemanticTarget::IndexScan | SemanticTarget::IndexLookup
    ) {
        let placement = steps[1].id;
        let root_pos = roots
            .iter()
            .position(|root| root.placement == placement)
            .ok_or(reject(
                VerifyPhase::Table,
                "durable index site path is not rooted at a durable root",
            ))?;
        let root = &roots[root_pos];
        let index_id = steps.last().expect("an index path has an index step").id;
        let local = root
            .indexes
            .iter()
            .position(|index| index.id == index_id)
            .ok_or(reject(
                VerifyPhase::Table,
                "durable index site names no managed index of its root",
            ))?;
        let index = &root.indexes[local];
        let agrees = match target {
            SemanticTarget::IndexScan => !index.unique,
            SemanticTarget::IndexLookup => index.unique,
            _ => unreachable!("guarded to index targets"),
        };
        if !agrees {
            return Err(reject(
                VerifyPhase::Table,
                "durable index site read kind disagrees with the index's unique flag",
            ));
        }
        // The index's position in the image-wide index table, assembled by iterating the
        // roots in order and each root's indexes in order (the same order used when the
        // sealed index list is built), so this position indexes that list directly.
        let global: usize = roots[..root_pos]
            .iter()
            .map(|root| root.indexes.len())
            .sum::<usize>()
            + local;
        let global = global as u16;
        let sealed_target = match target {
            SemanticTarget::IndexScan => SealedSiteTarget::IndexScan(global),
            SemanticTarget::IndexLookup => SealedSiteTarget::IndexLookup(global),
            _ => unreachable!("guarded to index targets"),
        };
        return Ok(SealedSite::Flat {
            root: root_pos as u16,
            target: sealed_target,
        });
    }
    // Every node carries its enclosing root's placement as its second step, so the
    // root index is that placement's position. A flat-executable keyed root — keyed, with
    // every member a field or a simple keyed branch (no group at any level) — is
    // kernel-executable: a whole-payload or keyed-branch-entry site, or a field-leaf site
    // (scalar or widened value), at any branch depth. A site on a non-flat root — a
    // singleton, or a group at any level — seals as parked (identity complete, execution
    // deferred).
    let placement = steps[1].id;
    let root_index = roots
        .iter()
        .position(|root| root.placement == placement)
        .ok_or(reject(
            VerifyPhase::Table,
            "durable site path is not rooted at a durable root",
        ))? as u16;
    let root = &roots[root_index as usize];
    let parked = || SealedSite::Parked {
        path: SemanticPath::from_steps(steps.to_vec()),
        target,
    };
    if !is_flat_executable_root(root) {
        return Ok(parked());
    }
    // The root is flat-executable, so every intermediate placement step below the root is
    // a keyed-branch placement (no groups on the flat path). `steps[2..]` are the branch
    // placements from the root down; a field target's last step is the field id.
    let below_root = &steps[marrow_image::bounds::MIN_SITE_PATH_STEPS..];
    let sealed = match target {
        SemanticTarget::WholePayload => match node.kind {
            // The root's own whole entry: exactly the two root steps.
            SemanticNodeKind::Root => {
                if !below_root.is_empty() {
                    return Ok(parked());
                }
                SealedSite::Flat {
                    root: root_index,
                    target: SealedSiteTarget::WholePayload,
                }
            }
            // A keyed branch entry at any depth: every step below the root is a branch
            // placement. Walk the placement chain through the recursive member tree into a
            // per-level branch path; a step that names no branch at its level parks.
            SemanticNodeKind::Branch => match walk_branch_path(&root.members, below_root) {
                Some((path, _)) => SealedSite::Flat {
                    root: root_index,
                    target: SealedSiteTarget::BranchEntry(path.into()),
                },
                None => parked(),
            },
            _ => unreachable!("a whole-payload target resolved to a root or branch node"),
        },
        SemanticTarget::FieldLeaf => {
            // The last step is the field id; the steps before it are the branch placements
            // from the root down to the field's containing node (empty for a top-level
            // field). Walk the branch chain, then resolve the field within the reached
            // node's own members.
            let Some((&field_step, branch_steps)) = below_root.split_last() else {
                return Ok(parked());
            };
            match walk_branch_path(&root.members, branch_steps) {
                Some((path, node_members)) => {
                    match top_level_field_index(node_members, field_step.id) {
                        Some(field) if path.is_empty() => SealedSite::Flat {
                            root: root_index,
                            target: SealedSiteTarget::FieldLeaf(field),
                        },
                        Some(field) => SealedSite::Flat {
                            root: root_index,
                            target: SealedSiteTarget::BranchField {
                                branch: path.into(),
                                field,
                            },
                        },
                        None => parked(),
                    }
                }
                None => parked(),
            }
        }
        // A root-level unkeyed group is addressed by exactly one group step below the
        // root; the flat kernel serves only root-level groups, so a group nested in a
        // branch or another group (more or fewer steps, or a non-root-level group id)
        // parks.
        SemanticTarget::GroupEntry => match below_root {
            [group_step] => match root_group_index(&root.members, group_step.id) {
                Some(group) => SealedSite::Flat {
                    root: root_index,
                    target: SealedSiteTarget::GroupEntry(group),
                },
                None => parked(),
            },
            _ => parked(),
        },
        // Index scan/lookup targets returned parked above, before the flat/field logic.
        SemanticTarget::IndexScan | SemanticTarget::IndexLookup => {
            unreachable!("index read targets are sealed and returned before this point")
        }
    };
    Ok(sealed)
}

/// Whether a decoded root is the flat keyed root the kernel executes: at least one key
/// column and a member tree of top-level storable-value fields (scalar or widened) and
/// keyed branches of the same shape (no group). The key may be single-column or a composite
/// tuple, at the root and at every branch. Re-derived from the decoded graph, so the
/// flat/parked classification never trusts a compiler summary.
pub(super) fn is_flat_executable_root(root: &DecodedRoot) -> bool {
    !root.keys.is_empty() && root.members.iter().all(member_flat_at_root)
}

/// Whether a root's *direct* member keeps the root flat-executable. It admits one more
/// shape than [`DecodedMember::keeps_root_flat`]: a root-level unkeyed `group` whose own
/// members are all storable-value fields (a scalar or widened composite). A group is a
/// value unit of the root entry, executable at the root level, but a group nested in a
/// branch or in another group still parks — [`keeps_root_flat`] (used for branch
/// members) keeps `Group => false`, so a group below the root's direct members never
/// makes its enclosing branch flat.
pub(super) fn member_flat_at_root(member: &DecodedMember) -> bool {
    match member {
        DecodedMember::Field { .. } => true,
        DecodedMember::Group { members, .. } => members
            .iter()
            .all(|m| matches!(m, DecodedMember::Field { .. })),
        DecodedMember::Branch { .. } => member.is_simple_branch(),
    }
}

/// The index of the root-level unkeyed group with ledger id `id` among a member tree's
/// direct `Group` members, in declaration order. `None` when no direct group carries the
/// id — a nested or in-branch group is not a root-level group node.
fn root_group_index(members: &[DecodedMember], id: LedgerIdBytes) -> Option<u16> {
    members
        .iter()
        .filter(|member| matches!(member, DecodedMember::Group { .. }))
        .position(|member| matches!(member, DecodedMember::Group { id: gid, .. } if *gid == id))
        .map(|position| position as u16)
}

/// Seal a member tree's keyed branches into the recursive [`SealedBranch`] tree, in
/// declaration order, so a [`SealedSiteTarget::BranchEntry`] branch path indexes it level
/// by level. Called only for a flat-executable root, so every branch is a scalar-field
/// keyed branch (its `keys` are its ordered key columns) and its own members recurse
/// through the same rule.
pub(super) fn seal_branches(members: &[DecodedMember], strings: &[Rc<str>]) -> Vec<SealedBranch> {
    members
        .iter()
        .filter_map(|member| match member {
            DecodedMember::Branch {
                name,
                record,
                keys,
                members,
                ..
            } => Some(SealedBranch {
                name: strings[*name as usize].clone(),
                keys: keys.iter().map(|(scalar, _)| *scalar).collect(),
                record: *record,
                branches: seal_branches(members, strings),
            }),
            _ => None,
        })
        .collect()
}

/// Seal a flat-executable root's root-level unkeyed groups into [`SealedGroup`]s, in
/// declaration order, so a [`SealedSiteTarget::GroupEntry`] group index selects one.
/// Each group's name and materialized record come from the root's own record: the
/// verifier's record↔member tie (validated in the table phase) places one trailing
/// group slot per `Group` member, after the leading scalar/widened field slots, in
/// declaration order — so the group slot at `field_count + ordinal` is exactly this
/// group's slot. Called only for a flat-executable root, whose groups are all
/// storable-value-field groups.
pub(super) fn seal_groups(root: &DecodedRoot, types: &[SealedRecordType]) -> Vec<SealedGroup> {
    let record = &types[root.record as usize];
    let field_count = root
        .members
        .iter()
        .filter(|member| matches!(member, DecodedMember::Field { .. }))
        .count();
    root.members
        .iter()
        .filter(|member| matches!(member, DecodedMember::Group { .. }))
        .enumerate()
        .map(|(ordinal, _group)| {
            let slot = &record.fields[field_count + ordinal];
            let record = match slot.ty {
                ImageType::Record { idx, .. } => idx,
                _ => unreachable!("the record↔member tie places a Record slot per group member"),
            };
            SealedGroup {
                name: slot.name.clone(),
                record,
            }
        })
        .collect()
}

/// The index of the top-level field with ledger id `field_id` among a root's member
/// tree, counting only its direct field members in declaration order. This is the
/// field's index into the root's materialized record (their orders are tied during
/// root decode), so a resolved field-leaf site addresses the same field the record
/// types.
fn top_level_field_index(members: &[DecodedMember], field_id: LedgerIdBytes) -> Option<u16> {
    members
        .iter()
        .filter_map(|member| match member {
            DecodedMember::Field { id, .. } => Some(*id),
            _ => None,
        })
        .position(|id| id == field_id)
        .map(|index| index as u16)
}

/// Resolve an index's ledger-id projection to record/key positions the path kernel
/// maintains, against the same decoded root the components were re-resolved against in
/// `decode_indexes`. A field component names its position in the root's materialized
/// record (tied to the durable member order); a key component names its column in the
/// root's key tuple. Every component already resolved to a real leaf during decode, so a
/// miss here is an internal inconsistency the verifier refuses rather than mis-addressing
/// a maintained index cell.
pub(super) fn resolve_index_projection(
    root: &DecodedRoot,
    components: &[DurableIndexComponent],
) -> Result<Vec<SealedIndexComponent>, VerifyRejection> {
    components
        .iter()
        .map(|component| match component {
            DurableIndexComponent::Field(id) => top_level_field_index(&root.members, *id)
                .map(SealedIndexComponent::Field)
                .ok_or(reject(
                    VerifyPhase::Table,
                    "durable index field component resolves to no record position",
                )),
            DurableIndexComponent::Key(id) => root
                .keys
                .iter()
                .position(|(_, key_id)| key_id == id)
                .map(|column| SealedIndexComponent::Key(column as u16))
                .ok_or(reject(
                    VerifyPhase::Table,
                    "durable index key component resolves to no key column",
                )),
        })
        .collect()
}

/// Walk a chain of branch placement steps through a member tree, accumulating the
/// per-level branch index at each hop and descending into that branch's own members. The
/// returned path indexes the recursive sealed branch tree level by level, and the returned
/// member slice is the deepest reached node's own members (the whole tree when the chain is
/// empty), against which a field leaf resolves. `None` when a step names no branch at its
/// level — a group-scoped or otherwise non-branch step parks rather than mis-resolving.
/// Only branch steps appear here on the flat-executable path (no groups), so a resolved
/// walk is a pure branch chain.
fn walk_branch_path<'a>(
    mut members: &'a [DecodedMember],
    steps: &[SemanticStep],
) -> Option<(Vec<u16>, &'a [DecodedMember])> {
    let mut path = Vec::with_capacity(steps.len());
    for step in steps {
        let index = branch_index(members, step.id)?;
        path.push(index);
        members = members.iter().find_map(|member| match member {
            DecodedMember::Branch {
                placement, members, ..
            } if *placement == step.id => Some(members.as_slice()),
            _ => None,
        })?;
    }
    Some((path, members))
}

/// The index of the keyed `branch` with placement id `placement` among a root's
/// declaration-ordered branch members. This is the index into the root's sealed
/// branch list (both count only the direct branch members, in order), so a resolved
/// branch-entry site addresses the same branch the schema derives.
fn branch_index(members: &[DecodedMember], placement_id: LedgerIdBytes) -> Option<u16> {
    members
        .iter()
        .filter_map(|member| match member {
            DecodedMember::Branch { placement, .. } => Some(*placement),
            _ => None,
        })
        .position(|id| id == placement_id)
        .map(|index| index as u16)
}

/// The ledger-id accounting for one durable table. `seen` holds every *declaration*
/// id — the application, each root's placement/product/key ids, each member's
/// field/group/branch id and branch key ids, each managed index id, and each durable
/// enum's sum and member ids on the enum's first durable occurrence — which must be
/// pairwise distinct because entropy-minted ids are distinct by construction. `enums`
/// records each durable enum identity by its sum id: the ordered member ids claimed at
/// its first occurrence. A later value shape carrying an already-recorded sum id — the
/// shape a second durable field of that enum emits — is a *reference* to that one
/// per-declaration identity, so it reclaims nothing and must carry the identical member
/// ids in order.
#[derive(Default)]
struct LedgerScope {
    seen: Vec<LedgerIdBytes>,
    enums: BTreeMap<LedgerIdBytes, Vec<LedgerIdBytes>>,
}

/// Read one 16-byte ledger id from the reader without claiming it.
fn read_id(reader: &mut Reader<'_>, what: &'static str) -> Result<LedgerIdBytes, VerifyRejection> {
    let bytes: [u8; 16] = reader
        .take(16)
        .ok_or(reject(VerifyPhase::Table, what))?
        .try_into()
        .expect("take(16) yields 16 bytes");
    Ok(LedgerIdBytes::from_bytes(bytes))
}

/// Claim `id` as a fresh declaration id, rejecting a duplicate against those already
/// seen in this durable table. Two equal declaration ids are a forged or corrupted
/// identity block.
fn claim_distinct(scope: &mut LedgerScope, id: LedgerIdBytes) -> Result<(), VerifyRejection> {
    if scope.seen.contains(&id) {
        return Err(reject(VerifyPhase::Table, "duplicate durable ledger id"));
    }
    scope.seen.push(id);
    Ok(())
}

/// Read one 16-byte ledger id and claim it as a fresh, pairwise-distinct declaration id.
fn take_distinct_id(
    reader: &mut Reader<'_>,
    scope: &mut LedgerScope,
    what: &'static str,
) -> Result<LedgerIdBytes, VerifyRejection> {
    let id = read_id(reader, what)?;
    claim_distinct(scope, id)?;
    Ok(id)
}

/// Decode a placement key tuple: `count` columns, each a bare orderable durable-key
/// scalar and a distinct ledger id. Shared by roots and branches; the caller has
/// already validated `count` against `MAX_KEY_COLUMNS`.
fn decode_key_tuple(
    reader: &mut Reader<'_>,
    count: usize,
    scope: &mut LedgerScope,
) -> Result<Vec<(Scalar, LedgerIdBytes)>, VerifyRejection> {
    let mut keys = Vec::with_capacity(count);
    for _ in 0..count {
        let key_tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short key type"))?;
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
                return Err(reject(
                    VerifyPhase::Table,
                    "key type must be an orderable durable-key scalar",
                ));
            }
        };
        let key_id = take_distinct_id(reader, scope, "short key identity")?;
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
fn tie_root_record(
    record_fields: &[DecodedField],
    members: &[DecodedMember],
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
) -> Result<(), VerifyRejection> {
    let mut slots = record_fields.iter();
    let mut seen_group = false;
    for member in members {
        match member {
            DecodedMember::Field {
                value, required, ..
            } => {
                if seen_group {
                    return Err(reject(
                        VerifyPhase::Table,
                        "root member tree places a field after a group",
                    ));
                }
                let Some(slot) = slots.next() else {
                    return Err(reject(
                        VerifyPhase::Table,
                        "root member tree has more top-level members than the record",
                    ));
                };
                if *required != slot.required || !value_shape_matches(value, slot.ty, types, enums)
                {
                    return Err(reject(
                        VerifyPhase::Table,
                        "root member tree fields do not match the record fields",
                    ));
                }
            }
            DecodedMember::Group {
                members: group_members,
                ..
            } => {
                seen_group = true;
                let Some(slot) = slots.next() else {
                    return Err(reject(
                        VerifyPhase::Table,
                        "root member tree has more top-level members than the record",
                    ));
                };
                tie_group_slot(slot, group_members, types, enums)?;
            }
            // A keyed branch is a distinct durable node, not a materialized record slot.
            DecodedMember::Branch { .. } => {}
        }
    }
    if slots.next().is_some() {
        return Err(reject(
            VerifyPhase::Table,
            "root member tree has fewer top-level members than the record",
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
    slot: &DecodedField,
    group_members: &[DecodedMember],
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
) -> Result<(), VerifyRejection> {
    let ImageType::Record { idx, optional } = slot.ty else {
        return Err(reject(
            VerifyPhase::Table,
            "a root group slot is not a group record",
        ));
    };
    if optional {
        return Err(reject(
            VerifyPhase::Table,
            "a root group slot must be a bare group record",
        ));
    }
    if idx as usize >= types.len() {
        return Err(reject(
            VerifyPhase::Table,
            "root group slot record index out of range",
        ));
    }
    let group_fields = &types[idx as usize].fields;
    let mut direct_fields = group_members.iter().filter_map(|member| match member {
        DecodedMember::Field {
            value, required, ..
        } => Some((value, *required)),
        _ => None,
    });
    for field in group_fields {
        match direct_fields.next() {
            Some((value, member_required))
                if member_required == field.required
                    && value_shape_matches(value, field.ty, types, enums) => {}
            _ => {
                return Err(reject(
                    VerifyPhase::Table,
                    "group member tree fields do not match its record fields",
                ));
            }
        }
    }
    if direct_fields.next().is_some() {
        return Err(reject(
            VerifyPhase::Table,
            "group member tree has more direct fields than its record",
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
fn validate_branch_records(
    members: &[DecodedMember],
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
    string_count: usize,
) -> Result<(), VerifyRejection> {
    for member in members {
        match member {
            DecodedMember::Field { .. } => {}
            DecodedMember::Group { members, .. } => {
                validate_branch_records(members, types, enums, string_count)?;
            }
            DecodedMember::Branch {
                name,
                record,
                members,
                ..
            } => {
                if *name as usize >= string_count {
                    return Err(reject(VerifyPhase::Table, "branch name index out of range"));
                }
                if *record as usize >= types.len() {
                    return Err(reject(
                        VerifyPhase::Table,
                        "branch record type index out of range",
                    ));
                }
                let record_fields = &types[*record as usize].fields;
                let mut direct_fields = members.iter().filter_map(|member| match member {
                    DecodedMember::Field {
                        value, required, ..
                    } => Some((value, *required)),
                    _ => None,
                });
                for field in record_fields {
                    match direct_fields.next() {
                        Some((value, member_required))
                            if member_required == field.required
                                && value_shape_matches(value, field.ty, types, enums) => {}
                        _ => {
                            return Err(reject(
                                VerifyPhase::Table,
                                "branch member tree fields do not match its record fields",
                            ));
                        }
                    }
                }
                if direct_fields.next().is_some() {
                    return Err(reject(
                        VerifyPhase::Table,
                        "branch member tree has more direct fields than its record",
                    ));
                }
                validate_branch_records(members, types, enums, string_count)?;
            }
        }
    }
    Ok(())
}

/// Decode a durable member tree: `u16(count) ‖ member*`. A field is tag `0x00`; a
/// group is tag `0x01`; a branch is tag `0x02`. `budget` bounds the total member
/// records across the whole tree and `depth` bounds nesting, so a hostile image
/// cannot drive unbounded recursion or allocation before the bounds are rechecked
/// (§ law 9). Every declaration ledger id is distinct across the table; a durable
/// enum's sum and member ids are the exception — one per-declaration identity a
/// later field of that enum references rather than reclaims.
fn decode_members(
    reader: &mut Reader<'_>,
    depth: usize,
    budget: &mut usize,
    scope: &mut LedgerScope,
) -> Result<Vec<DecodedMember>, VerifyRejection> {
    if depth > marrow_image::bounds::MAX_DURABLE_DEPTH {
        return Err(reject(VerifyPhase::Table, "durable member tree too deep"));
    }
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short durable member count"))? as usize;
    let mut members = Vec::with_capacity(count.min(*budget));
    for _ in 0..count {
        if *budget == 0 {
            return Err(reject(VerifyPhase::Table, "too many durable members"));
        }
        *budget -= 1;
        let tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short durable member tag"))?;
        let member = match tag {
            0x00 => {
                let id = take_distinct_id(reader, scope, "short durable field identity")?;
                let required = match reader.u8().ok_or(reject(
                    VerifyPhase::Table,
                    "short durable field required flag",
                ))? {
                    0 => false,
                    1 => true,
                    _ => {
                        return Err(reject(
                            VerifyPhase::Table,
                            "durable field required flag must be 0 or 1",
                        ));
                    }
                };
                let value = decode_value_shape(reader, 1, scope)?;
                DecodedMember::Field {
                    id,
                    required,
                    value,
                }
            }
            0x01 => {
                let id = take_distinct_id(reader, scope, "short durable group identity")?;
                let inner = decode_members(reader, depth + 1, budget, scope)?;
                DecodedMember::Group { id, members: inner }
            }
            0x02 => {
                let placement = take_distinct_id(reader, scope, "short durable branch identity")?;
                // The branch's surface name and materialized record type index follow
                // the placement. Their ranges (against the string and type tables) and
                // the record/member-field alignment are checked in
                // `validate_branch_records`, where the type and enum tables are in scope.
                let name = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, "short durable branch name"))?;
                let record = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, "short durable branch record"))?;
                let key_count = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, "short branch key count"))?
                    as usize;
                if key_count > marrow_image::bounds::MAX_KEY_COLUMNS {
                    return Err(reject(VerifyPhase::Table, "too many branch key columns"));
                }
                let keys = decode_key_tuple(reader, key_count, scope)?;
                let inner = decode_members(reader, depth + 1, budget, scope)?;
                DecodedMember::Branch {
                    placement,
                    name,
                    record,
                    keys,
                    members: inner,
                }
            }
            _ => return Err(reject(VerifyPhase::Table, "unknown durable member tag")),
        };
        members.push(member);
    }
    Ok(members)
}

/// Decode a root's managed indexes: `u16(count) ‖ index*`. Each index is its distinct
/// `Index` ledger id, a `unique` flag byte, a `u16(component_count)`, and per component
/// a one-byte leaf kind (`0x02` field, `0x04` key) and the leaf's 16-byte ledger id.
/// Every component id is re-resolved against this root's own top-level field ids
/// (kind `0x02`) or identity key ids (kind `0x04`), so a projection over a leaf that
/// does not exist on the root is refused. The index id is distinct across the whole
/// durable table (via `seen`); component ids are references to already-seen leaf ids
/// and so are not added to `seen`.
fn decode_indexes(
    reader: &mut Reader<'_>,
    keys: &[(Scalar, LedgerIdBytes)],
    members: &[DecodedMember],
    scope: &mut LedgerScope,
) -> Result<Vec<DecodedIndex>, VerifyRejection> {
    let field_ids: Vec<LedgerIdBytes> = members
        .iter()
        .filter_map(|member| match member {
            DecodedMember::Field { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    // A managed-index field component must project one of the compiler's closed set of
    // orderable durable-key scalar shapes. Field executability is independent: Duration
    // and widened values can be stored but are not index-eligible.
    let index_eligible_field_ids: Vec<LedgerIdBytes> = members
        .iter()
        .filter_map(|member| match member {
            DecodedMember::Field { id, value, .. } => match value {
                DurableValueShape::Scalar(
                    Scalar::Int
                    | Scalar::Text
                    | Scalar::Bool
                    | Scalar::Bytes
                    | Scalar::Date
                    | Scalar::Instant,
                ) => Some(*id),
                DurableValueShape::Scalar(Scalar::Duration)
                | DurableValueShape::Struct(_)
                | DurableValueShape::Enum { .. } => None,
            },
            DecodedMember::Group { .. } | DecodedMember::Branch { .. } => None,
        })
        .collect();
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short durable index count"))? as usize;
    if count > marrow_image::bounds::MAX_INDEXES {
        return Err(reject(VerifyPhase::Table, "too many durable indexes"));
    }
    let mut indexes = Vec::with_capacity(count);
    for _ in 0..count {
        let id = take_distinct_id(reader, scope, "short durable index identity")?;
        let unique = match reader.u8().ok_or(reject(
            VerifyPhase::Table,
            "short durable index unique flag",
        ))? {
            0 => false,
            1 => true,
            _ => {
                return Err(reject(
                    VerifyPhase::Table,
                    "durable index unique flag must be 0 or 1",
                ));
            }
        };
        let component_count = reader.u16().ok_or(reject(
            VerifyPhase::Table,
            "short durable index component count",
        ))? as usize;
        if component_count > marrow_image::bounds::MAX_INDEX_COMPONENTS {
            return Err(reject(
                VerifyPhase::Table,
                "too many durable index components",
            ));
        }
        let mut components = Vec::with_capacity(component_count);
        for _ in 0..component_count {
            let kind = reader.u8().ok_or(reject(
                VerifyPhase::Table,
                "short durable index component kind",
            ))?;
            let leaf: [u8; 16] = reader
                .take(16)
                .ok_or(reject(
                    VerifyPhase::Table,
                    "short durable index component identity",
                ))?
                .try_into()
                .expect("take(16) yields 16 bytes");
            let leaf = LedgerIdBytes::from_bytes(leaf);
            let component = match kind {
                0x02 => {
                    if !index_eligible_field_ids.contains(&leaf) {
                        return Err(reject(
                            VerifyPhase::Table,
                            if field_ids.contains(&leaf) {
                                "durable index field component names a field that is not \
                                 index-eligible"
                            } else {
                                "durable index field component names no top-level field of its root"
                            },
                        ));
                    }
                    DurableIndexComponent::Field(leaf)
                }
                0x04 => {
                    if !keys.iter().any(|(_, key_id)| *key_id == leaf) {
                        return Err(reject(
                            VerifyPhase::Table,
                            "durable index key component names no identity key of its root",
                        ));
                    }
                    DurableIndexComponent::Key(leaf)
                }
                _ => {
                    return Err(reject(
                        VerifyPhase::Table,
                        "unknown durable index component kind",
                    ));
                }
            };
            components.push(component);
        }
        // Re-enforce projection well-formedness the compiler owns: a reference-valid but
        // malformed projection (an empty projection, a repeated component, or a
        // non-unique index whose identity suffix is missing, misordered, or preceded by a
        // key) must never reach the sealed index model the runtime trusts to order rows.
        if let Err(detail) = validate_index_projection(unique, &components, keys) {
            return Err(reject(VerifyPhase::Table, detail));
        }
        indexes.push(DecodedIndex {
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
/// unique index carries no suffix obligation. Returns a static detail describing the first
/// violation.
///
/// The no-leading-key rule (a non-unique index carries no identity key before its suffix)
/// needs no separate branch: distinctness forbids any component from repeating, and the
/// suffix must already hold every identity key, so a leading identity key would duplicate
/// a suffix key and is rejected by the distinctness check.
fn validate_index_projection(
    unique: bool,
    components: &[DurableIndexComponent],
    keys: &[(Scalar, LedgerIdBytes)],
) -> Result<(), &'static str> {
    if components.is_empty() {
        return Err("durable index has an empty projection");
    }
    for (position, component) in components.iter().enumerate() {
        if components[..position]
            .iter()
            .any(|earlier| earlier.id() == component.id())
        {
            return Err("durable index repeats a projection component");
        }
    }
    if !unique {
        // The trailing `keys.len()` components must be exactly the identity keys in
        // declaration order.
        if components.len() < keys.len() {
            return Err("non-unique durable index does not end with the identity suffix");
        }
        let suffix_start = components.len() - keys.len();
        for (offset, (_, key_id)) in keys.iter().enumerate() {
            match components[suffix_start + offset] {
                DurableIndexComponent::Key(id) if id == *key_id => {}
                _ => {
                    return Err(
                        "non-unique durable index does not end with the identity keys in \
                         declaration order",
                    );
                }
            }
        }
    }
    Ok(())
}

/// Decode a durable field's stored value shape: `u8(value_tag) ‖ body`. A scalar is
/// tag `0x00` (a bare scalar); a dense struct is tag `0x01` (`u16(count) ‖ value*`);
/// a closed enum is tag `0x02` (`sum id ‖ u16(count) ‖ [member id ‖ u16(payload) ‖
/// value*]*`). An enum's sum and member ids are the identity of the enum *declaration*.
/// The first occurrence of a given sum id claims it and its member ids as fresh
/// pairwise-distinct ids; a later occurrence carrying an already-claimed sum id — the
/// shape a second durable field of that enum emits — is a reference that reclaims
/// nothing and must carry the identical member ids in order. `depth` bounds nesting so a
/// hostile image cannot drive unbounded recursion before the value shape is rechecked
/// (§ law 9).
fn decode_value_shape(
    reader: &mut Reader<'_>,
    depth: usize,
    scope: &mut LedgerScope,
) -> Result<DurableValueShape, VerifyRejection> {
    if depth > marrow_image::bounds::MAX_DURABLE_VALUE_DEPTH {
        return Err(reject(
            VerifyPhase::Table,
            "durable field value shape too deep",
        ));
    }
    let tag = reader
        .u8()
        .ok_or(reject(VerifyPhase::Table, "short durable value tag"))?;
    match tag {
        0x00 => {
            let scalar_tag = reader
                .u8()
                .ok_or(reject(VerifyPhase::Table, "short durable value scalar"))?;
            let scalar = decode_bare_scalar(scalar_tag).ok_or(reject(
                VerifyPhase::Table,
                "durable value scalar must be a bare scalar",
            ))?;
            Ok(DurableValueShape::Scalar(scalar))
        }
        0x01 => {
            let count = reader.u16().ok_or(reject(
                VerifyPhase::Table,
                "short durable struct leaf count",
            ))? as usize;
            if count > marrow_image::bounds::MAX_STRUCT_LEAVES {
                return Err(reject(VerifyPhase::Table, "too many durable struct leaves"));
            }
            let mut leaves = Vec::with_capacity(count);
            for _ in 0..count {
                leaves.push(decode_value_shape(reader, depth + 1, scope)?);
            }
            Ok(DurableValueShape::Struct(leaves))
        }
        0x02 => {
            let sum = read_id(reader, "short durable enum sum identity")?;
            let member_count = reader.u16().ok_or(reject(
                VerifyPhase::Table,
                "short durable enum member count",
            ))? as usize;
            if member_count > marrow_image::bounds::MAX_VARIANTS {
                return Err(reject(VerifyPhase::Table, "too many durable enum members"));
            }
            // An enum reached before (by its sum id) is a reference to that one
            // per-declaration identity: it reclaims neither the sum nor any member id,
            // and must present the identical member ids in the identical order. The
            // recorded identity is the ordered member ids only; each occurrence's payload
            // shapes are tied to the field's own enum-table entry by `value_shape_matches`,
            // so a payload divergence is caught there rather than against the first
            // occurrence.
            let recorded = scope.enums.get(&sum).cloned();
            match &recorded {
                Some(recorded_ids) if recorded_ids.len() != member_count => {
                    return Err(reject(
                        VerifyPhase::Table,
                        "durable enum identity reused with a different member set",
                    ));
                }
                Some(_) => {}
                None => claim_distinct(scope, sum)?,
            }
            let mut members = Vec::with_capacity(member_count);
            for index in 0..member_count {
                let id = read_id(reader, "short durable enum member identity")?;
                match &recorded {
                    Some(recorded_ids) if recorded_ids[index] != id => {
                        return Err(reject(
                            VerifyPhase::Table,
                            "durable enum identity reused with a different member set",
                        ));
                    }
                    Some(_) => {}
                    None => claim_distinct(scope, id)?,
                }
                let payload_count = reader.u16().ok_or(reject(
                    VerifyPhase::Table,
                    "short durable enum member payload count",
                ))? as usize;
                if payload_count > marrow_image::bounds::MAX_PAYLOAD_FIELDS {
                    return Err(reject(
                        VerifyPhase::Table,
                        "too many durable enum member payload leaves",
                    ));
                }
                let mut payload = Vec::with_capacity(payload_count);
                for _ in 0..payload_count {
                    payload.push(decode_value_shape(reader, depth + 1, scope)?);
                }
                members.push(DurableEnumMemberShape { id, payload });
            }
            if recorded.is_none() {
                scope
                    .enums
                    .insert(sum, members.iter().map(|member| member.id).collect());
            }
            Ok(DurableValueShape::Enum { sum, members })
        }
        _ => Err(reject(VerifyPhase::Table, "unknown durable value tag")),
    }
}

/// Whether a decoded durable field value shape structurally matches the materialized
/// record field type it claims, recursing through the record and enum tables. The
/// ledger ids a value shape carries (a struct records none; an enum a sum and per-
/// member id) are durable identity, verified by pairwise distinctness and the
/// contract-id recomputation — this match ties the *structure* to the executable
/// record so a hostile image cannot claim one durable identity while its record
/// carries a different value shape. A nominal field erases to its base scalar, so it
/// matches a bare scalar exactly like a plain scalar field.
fn value_shape_matches(
    shape: &DurableValueShape,
    ty: ImageType,
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
) -> bool {
    match (shape, ty) {
        (
            DurableValueShape::Scalar(shape_scalar),
            ImageType::Scalar {
                scalar,
                optional: false,
            },
        ) => *shape_scalar == scalar,
        (
            DurableValueShape::Struct(leaves),
            ImageType::Record {
                idx,
                optional: false,
            },
        ) => {
            let Some(record) = types.get(idx as usize) else {
                return false;
            };
            // A durable struct value is dense: every leaf is a required bare field,
            // matched positionally.
            record.fields.len() == leaves.len()
                && record.fields.iter().zip(leaves).all(|(field, leaf)| {
                    field.required && value_shape_matches(leaf, field.ty, types, enums)
                })
        }
        (
            DurableValueShape::Enum { members, .. },
            ImageType::Enum {
                idx,
                optional: false,
            },
        ) => {
            let Some(enum_def) = enums.get(idx as usize) else {
                return false;
            };
            enum_def.variants.len() == members.len()
                && enum_def
                    .variants
                    .iter()
                    .zip(members)
                    .all(|(variant, member)| {
                        variant.payload.len() == member.payload.len()
                            && variant
                                .payload
                                .iter()
                                .zip(&member.payload)
                                .all(|(leaf_ty, leaf)| {
                                    value_shape_matches(leaf, *leaf_ty, types, enums)
                                })
                    })
        }
        _ => false,
    }
}

/// Rebuild the canonical durable-graph descriptor from the decoded tables. This is
/// the verifier's independent reconstruction: it shares the canonical encoding owned
/// by `marrow-image` but reads only the decoded application id, roots, key tuples,
/// and member trees, so the recomputed id depends on nothing the compiler asserted
/// directly.
pub(super) fn durable_descriptor(
    application: Option<LedgerIdBytes>,
    roots: &[DecodedRoot],
) -> DurableContractDescriptor {
    let Some(application) = application else {
        return DurableContractDescriptor::empty();
    };
    let shapes = roots
        .iter()
        .map(|root| DurableRootShape {
            placement: root.placement,
            product: root.product,
            keys: key_shapes(&root.keys),
            members: member_shapes(&root.members),
            indexes: index_shapes(&root.indexes),
        })
        .collect();
    DurableContractDescriptor::new(application, shapes)
}

/// The descriptor index shapes for a decoded root's managed indexes.
fn index_shapes(indexes: &[DecodedIndex]) -> Vec<DurableIndexShape> {
    indexes
        .iter()
        .map(|index| DurableIndexShape {
            id: index.id,
            unique: index.unique,
            components: index.components.clone(),
        })
        .collect()
}

/// The descriptor key-tuple shapes for a decoded placement's key columns.
fn key_shapes(keys: &[(Scalar, LedgerIdBytes)]) -> Vec<DurableKeyShape> {
    keys.iter()
        .map(|(scalar, id)| DurableKeyShape {
            scalar: *scalar,
            id: *id,
        })
        .collect()
}

/// Convert a decoded member tree into the descriptor's member shapes, recursing
/// through groups and branches.
fn member_shapes(members: &[DecodedMember]) -> Vec<DurableMemberShape> {
    members
        .iter()
        .map(|member| match member {
            DecodedMember::Field {
                id,
                required,
                value,
            } => DurableMemberShape::Field(DurableFieldShape {
                id: *id,
                required: *required,
                value: value.clone(),
            }),
            DecodedMember::Group { id, members } => DurableMemberShape::Group(DurableGroupShape {
                id: *id,
                members: member_shapes(members),
            }),
            // Name and record are surface, not identity: the descriptor carries only
            // the branch's placement, key tuple, and member value shapes.
            DecodedMember::Branch {
                placement,
                keys,
                members,
                ..
            } => DurableMemberShape::Branch(DurableBranchShape {
                placement: *placement,
                keys: key_shapes(keys),
                members: member_shapes(members),
            }),
        })
        .collect()
}
