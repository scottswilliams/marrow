//! Operation sites resolved against the reconstructed graph: each path names a node whose
//! kind agrees with the claimed target, and each executable node's coordinates are
//! re-derived rather than trusted.

use super::super::model::DecodedRoot;
use super::super::reject;
use super::project::is_flat_executable_root;
use crate::reader::Reader;
use crate::reject::{
    Bound, Duplicate, Region, RejectionKind as Kind, SiteFault, Tag, VerifyPhase, VerifyRejection,
};
use crate::sealed::{SealedSite, SealedSiteTarget};
use marrow_image::{
    LedgerIdBytes, RootId, SemanticNode, SemanticNodeKind, SemanticPath, SemanticStep,
    SemanticStepKind, SemanticTarget,
};
use std::collections::{HashMap, HashSet};

/// Decode the section's operation-site run, resolving each site against the graph's own
/// node set and refusing a repeated resolved identity.
pub(super) fn decode_sites(
    reader: &mut Reader<'_>,
    nodes: &[SemanticNode],
    roots: &[DecodedRoot],
) -> Result<(Vec<SealedSite>, Vec<SemanticPath>), VerifyRejection> {
    // Project the node set once into its keyed form: each node's executable coordinates,
    // keyed by its path. A site then resolves in one lookup.
    let projection = project_graph(nodes, roots);
    let site_count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
        as usize;
    if site_count > marrow_image::bounds::MAX_SITES {
        return Err(reject(VerifyPhase::Table, Kind::OverBound(Bound::Sites)));
    }
    let mut sites: Vec<SealedSite> = Vec::with_capacity(site_count);
    // Each site's resolved graph-node path, parallel to `sites` by index. The demand
    // reconstruction maps a durable opcode's site index to the semantic path of the
    // node it addresses; a flat site drops the path from its executable form, so it
    // is retained here rather than re-derived.
    let mut site_paths: Vec<SemanticPath> = Vec::with_capacity(site_count);
    // The resolved identities already claimed, keyed: a flat site by (root, target), a
    // parked site by (path, target), and the two can never collide. Keying keeps the claim
    // cost linear where comparing each fresh site against every retained one would be
    // quadratic in whole path chains, for the same first-duplicate wire ordinal.
    let mut claimed: HashSet<SealedSite> = HashSet::with_capacity(site_count);
    for _ in 0..site_count {
        let (site, path) = decode_site(reader, &projection)?;
        if !claimed.insert(site.clone()) {
            return Err(reject(VerifyPhase::Table, Kind::Duplicate(Duplicate::Site)));
        }
        sites.push(site);
        site_paths.push(path);
    }
    Ok((sites, site_paths))
}

/// Decode one operation site — its semantic path then its target-kind byte — and
/// resolve it against the reconstructed node set. The path is `u8(step_count) ‖
/// [u8(ledger_kind) ‖ 16 id bytes]*`; the target byte is `0x00` whole-payload or
/// `0x01` field-leaf. Nothing here is trusted: the path is resolved to a node and
/// its kind cross-checked, and the executable physical facts are re-derived, so a
/// forged path, a flipped target byte, or a mutated ledger id is refused.
fn decode_site(
    reader: &mut Reader<'_>,
    graph: &GraphProjection<'_>,
) -> Result<(SealedSite, SemanticPath), VerifyRejection> {
    let step_count = reader
        .u8()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
        as usize;
    if step_count < marrow_image::bounds::MIN_SITE_PATH_STEPS {
        return Err(reject(VerifyPhase::Table, Kind::Site(SiteFault::Empty)));
    }
    if step_count > marrow_image::bounds::MAX_SITE_PATH_STEPS {
        return Err(reject(
            VerifyPhase::Table,
            Kind::OverBound(Bound::SitePathSteps),
        ));
    }
    let mut steps = Vec::with_capacity(step_count);
    for _ in 0..step_count {
        let kind_byte = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?;
        let kind = SemanticStepKind::from_ledger_kind(kind_byte)
            .ok_or(reject(VerifyPhase::Table, Kind::Unknown(Tag::SiteStep)))?;
        let id_bytes: [u8; 16] = reader
            .take(16)
            .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
            .try_into()
            .expect("take(16) yields 16 bytes");
        steps.push(SemanticStep::new(kind, LedgerIdBytes::from_bytes(id_bytes)));
    }
    let path = SemanticPath::try_from_steps(steps)
        .map_err(|_| reject(VerifyPhase::Table, Kind::Site(SiteFault::Empty)))?;
    let target = match reader
        .u8()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
    {
        0x00 => SemanticTarget::WholePayload,
        0x01 => SemanticTarget::FieldLeaf,
        0x02 => SemanticTarget::IndexScan,
        0x03 => SemanticTarget::IndexLookup,
        0x04 => SemanticTarget::GroupEntry,
        _ => return Err(reject(VerifyPhase::Table, Kind::Unknown(Tag::SiteTarget))),
    };
    let site = resolve_site(&path, target, graph)?;
    // The site's node path is the chain it resolved against — retained parallel to
    // the sealed site so demand reconstruction can name the node a flat site
    // addresses without re-deriving it from the executable form.
    Ok((site, path))
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
    path: &SemanticPath,
    target: SemanticTarget,
    graph: &GraphProjection<'_>,
) -> Result<SealedSite, VerifyRejection> {
    let node = graph.nodes.get(path.steps()).ok_or(reject(
        VerifyPhase::Table,
        Kind::Site(SiteFault::Unresolved),
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
                Kind::Site(SiteFault::TargetKind),
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
    if let NodeCoordinates::Index { global, unique } = node.coordinates {
        let agrees = match target {
            SemanticTarget::IndexScan => !unique,
            SemanticTarget::IndexLookup => unique,
            _ => unreachable!("only an index node admits an index read target"),
        };
        if !agrees {
            return Err(reject(VerifyPhase::Table, Kind::IndexReadKind));
        }
        let sealed_target = match target {
            SemanticTarget::IndexScan => SealedSiteTarget::IndexScan(global),
            SemanticTarget::IndexLookup => SealedSiteTarget::IndexLookup(global),
            _ => unreachable!("only an index node admits an index read target"),
        };
        return Ok(SealedSite::Flat {
            root: node.root,
            target: sealed_target,
        });
    }
    // A flat-executable keyed root — keyed, with every member a field or a simple keyed
    // branch (no group at any level) — is kernel-executable: a whole-payload or
    // keyed-branch-entry site, or a field-leaf site (scalar or widened value), at any
    // branch depth. A site on a non-flat root — a singleton, or a group at any level —
    // seals as parked (identity complete, execution deferred), as does any node the flat
    // kernel cannot address: a node reached through a `group` namespace, or a group
    // nested below the root's own members.
    let parked = || SealedSite::Parked {
        path: path.clone(),
        target,
    };
    if !graph.flat_roots[node.root.index() as usize] {
        return Ok(parked());
    }
    let sealed = match &node.coordinates {
        NodeCoordinates::Root => SealedSite::Flat {
            root: node.root,
            target: SealedSiteTarget::WholePayload,
        },
        NodeCoordinates::Branch(branch) => SealedSite::Flat {
            root: node.root,
            target: SealedSiteTarget::BranchEntry(branch.clone()),
        },
        NodeCoordinates::Field { branch, field } if branch.is_empty() => SealedSite::Flat {
            root: node.root,
            target: SealedSiteTarget::FieldLeaf(*field),
        },
        NodeCoordinates::Field { branch, field } => SealedSite::Flat {
            root: node.root,
            target: SealedSiteTarget::BranchField {
                branch: branch.clone(),
                field: *field,
            },
        },
        NodeCoordinates::Group(group) => SealedSite::Flat {
            root: node.root,
            target: SealedSiteTarget::GroupEntry(*group),
        },
        NodeCoordinates::Unaddressable => parked(),
        NodeCoordinates::Index { .. } => {
            unreachable!("an index node sealed and returned before this point")
        }
    };
    Ok(sealed)
}

/// The keyed projection of one durable table's reconstructed graph: every node's kind,
/// enclosing root occurrence, and executable coordinates, keyed by the node's path.
///
/// It is derived in one pass from the same [`SemanticNode`] set the contract id is
/// computed over — this verifier's own reconstruction — so it introduces no second graph,
/// path, or identity owner. Resolving a site is then one keyed lookup, so sealing a table
/// costs its site count plus its graph size rather than their product.
struct GraphProjection<'a> {
    nodes: HashMap<&'a [SemanticStep], ProjectedNode>,
    /// Whether the root occurrence at each DURABLE-table position is the flat keyed root
    /// the kernel executes, by table position.
    flat_roots: Vec<bool>,
}

/// One reconstructed graph node's resolved facts.
struct ProjectedNode {
    kind: SemanticNodeKind,
    /// The DURABLE-table position of the root occurrence this node hangs under. Every
    /// node's path carries that root's placement as its second step.
    root: RootId,
    coordinates: NodeCoordinates,
}

/// A graph node's executable coordinates under its root occurrence — the positions the
/// path kernel addresses it by, re-derived from the graph rather than trusted from the
/// image.
enum NodeCoordinates {
    /// The root occurrence's own entry.
    Root,
    /// A keyed branch, by its per-level branch index from the root down.
    Branch(Box<[u16]>),
    /// A stored field, by its containing node's branch path and its index among that
    /// node's direct fields.
    Field { branch: Box<[u16]>, field: u16 },
    /// A root-level unkeyed group, by its index among the root's direct groups.
    Group(u16),
    /// A managed index, by its position in the image-wide index table and its `unique`
    /// flag.
    Index { global: u16, unique: bool },
    /// A node the flat kernel cannot address whatever its root: one reached through a
    /// `group` namespace, or a group below the root's own members. It parks.
    Unaddressable,
}

/// The per-parent ordinals a node takes among its same-kind siblings, in declaration
/// order: a field indexes its container's materialized record (the orders are tied during
/// decode), a branch the sealed branch list at its level, and a group its root's sealed
/// group list.
#[derive(Default)]
struct SiblingOrdinals {
    fields: u16,
    groups: u16,
    branches: u16,
}

/// The fixed prefix every managed-index row consumes, in the order [`decode_indexes`]
/// reads it: a 16-byte ledger id, a 1-byte `unique` flag, and a 2-byte component count.
/// Its components follow and only add to the cost, so this is the row's lower bound.
const MIN_DECODED_INDEX_ROW_BYTES: usize = 16 + 1 + 2;

/// An image-wide managed-index position fits the `u16` [`project_graph`] spells it with.
///
/// The per-root structural bounds do **not** establish this: `MAX_ROOTS * MAX_INDEXES`
/// is 4096 × 32 = 131,072, twice what a `u16` carries. The bound is the container
/// ceiling instead. Every index row is decoded from a reader over bytes already limited
/// to `MAX_IMAGE_BYTES` by `decode_container`, and each row consumes at least
/// [`MIN_DECODED_INDEX_ROW_BYTES`], so an image carries at most
/// `MAX_IMAGE_BYTES / MIN_DECODED_INDEX_ROW_BYTES` = 27,594 of them — comfortably
/// inside `u16::MAX`. An image stating more rows than that runs out of bytes and is
/// rejected before this projection is reached.
const _: () = assert!(
    marrow_image::bounds::MAX_IMAGE_BYTES / MIN_DECODED_INDEX_ROW_BYTES < u16::MAX as usize,
    "an image-wide managed-index position must fit the u16 the graph projection spells it with",
);

/// Project a reconstructed node set into its keyed form.
///
/// The node set is in pre-order — a node precedes its descendants — so each node's
/// coordinates are its parent's extended by its own ordinal among its same-kind siblings,
/// and both are already known when it is reached. Nothing is re-derived per site.
fn project_graph<'a>(nodes: &'a [SemanticNode], roots: &[DecodedRoot]) -> GraphProjection<'a> {
    let mut root_positions: HashMap<LedgerIdBytes, RootId> = HashMap::with_capacity(roots.len());
    for (position, root) in roots.iter().enumerate() {
        root_positions.insert(root.placement, RootId::from_index(position as u16));
    }
    // Each managed index by its ledger id: its position in the image-wide index table,
    // assembled by iterating the roots in order and each root's indexes in order — the
    // same order the sealed index list is built in, so the position indexes that list
    // directly — and its `unique` flag. The narrowing to `u16` cannot truncate: the
    // count is bounded by the container ceiling, not by the per-root structural bounds
    // (see the const assert above).
    let mut index_positions: HashMap<LedgerIdBytes, (u16, bool)> = HashMap::new();
    let mut global: usize = 0;
    for root in roots {
        for index in root.indexes.iter() {
            index_positions.insert(index.id, (global as u16, index.unique));
            global += 1;
        }
    }

    let mut ordinals: HashMap<&'a [SemanticStep], SiblingOrdinals> = HashMap::new();
    let mut projected: HashMap<&'a [SemanticStep], ProjectedNode> =
        HashMap::with_capacity(nodes.len());
    for node in nodes {
        let steps = node.path.steps();
        let (_, parent) = steps
            .split_last()
            .expect("a semantic path carries at least one step");
        // Every node hangs under a root occurrence whose placement is its second step:
        // a root's own path is `[application, placement]` and every descendant extends it.
        let root = *root_positions
            .get(&steps[marrow_image::bounds::MIN_SITE_PATH_STEPS - 1].id)
            .expect("a reconstructed node hangs under a decoded root");
        let sibling = ordinals.entry(parent).or_default();
        let coordinates = match node.kind {
            SemanticNodeKind::Root => NodeCoordinates::Root,
            SemanticNodeKind::Field => {
                let field = sibling.fields;
                sibling.fields += 1;
                match projected.get(parent).map(|node| &node.coordinates) {
                    Some(NodeCoordinates::Root) => NodeCoordinates::Field {
                        branch: Box::default(),
                        field,
                    },
                    Some(NodeCoordinates::Branch(branch)) => NodeCoordinates::Field {
                        branch: branch.clone(),
                        field,
                    },
                    _ => NodeCoordinates::Unaddressable,
                }
            }
            SemanticNodeKind::Group => {
                let group = sibling.groups;
                sibling.groups += 1;
                match projected.get(parent).map(|node| &node.coordinates) {
                    Some(NodeCoordinates::Root) => NodeCoordinates::Group(group),
                    _ => NodeCoordinates::Unaddressable,
                }
            }
            SemanticNodeKind::Branch => {
                let branch = sibling.branches;
                sibling.branches += 1;
                match projected.get(parent).map(|node| &node.coordinates) {
                    Some(NodeCoordinates::Root) => NodeCoordinates::Branch(Box::new([branch])),
                    Some(NodeCoordinates::Branch(above)) => {
                        let mut path = above.to_vec();
                        path.push(branch);
                        NodeCoordinates::Branch(path.into())
                    }
                    _ => NodeCoordinates::Unaddressable,
                }
            }
            SemanticNodeKind::Index => match index_positions.get(&node.path.node_id()) {
                Some(&(global, unique)) => NodeCoordinates::Index { global, unique },
                None => NodeCoordinates::Unaddressable,
            },
        };
        projected.insert(
            steps,
            ProjectedNode {
                kind: node.kind,
                root,
                coordinates,
            },
        );
    }

    GraphProjection {
        nodes: projected,
        flat_roots: roots.iter().map(is_flat_executable_root).collect(),
    }
}
