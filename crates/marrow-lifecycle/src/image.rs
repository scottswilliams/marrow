//! The projection from a verified program image to the lifecycle's persisted facts: the
//! active binding facts a binding-only rebind compares, and the head identity map that
//! pins each durable node's ledger id to its store-local cell-key number.
//!
//! Every fact here is derived purely from a [`VerifiedImage`] — the sole source of a valid
//! durable schema — so the store owner needs no dependency on the runner or the compiler.

use std::collections::HashMap;

use marrow_image::{LedgerIdBytes, interface_fingerprint};
use marrow_verify::{
    CeilingDescriptor, SemanticNode, SemanticNodeKind, SemanticStep, VerifiedImage,
};

use crate::codec::FormatError;
use crate::head::ActiveBinding;
use crate::headmap::HeadMap;

/// The container format version of the images this build reads and writes.
const IMAGE_FORMAT_VERSION: u8 = 0;

/// Derive the active binding a store records for `image`: the active image's byte identity
/// plus the binding facts a binding-only rebind compares (the durable contract, the
/// interface fingerprint over the exported call surface, and the deployment ceiling over the
/// demand union). The interface fingerprint is a runner-free digest over the image's export
/// declaration identities (see [`interface_fingerprint`]); the durable contract and ceiling
/// independently catch every durable-graph and demand/effect change, so the three facts
/// together are a conservative, sound binding-identity for the rebind classifier.
pub fn active_binding(image: &VerifiedImage) -> ActiveBinding {
    let export_ids: Vec<[u8; 32]> = image
        .exports()
        .iter()
        .map(|export| *export.id().bytes())
        .collect();
    let ceiling = CeilingDescriptor::from_demand_union(image.demand_union());
    ActiveBinding {
        image_format_version: IMAGE_FORMAT_VERSION,
        image_id: image.image_id().0,
        durable_contract: *image.durable_contract().bytes(),
        interface: interface_fingerprint(&export_ids),
        ceiling: *ceiling.ceiling_id().bytes(),
    }
}

/// Build the head identity map for `image`: the ledger-id ↔ cell-number bijection (FR01 §3),
/// where node `i` in the store-local cell-key numbering is the `i`-th durable node in the
/// kernel's canonical split pre-order (see [`split_order`]). A projection of that one walk:
/// number `i` binds to the ledger id of the `i`-th walked node.
///
/// Returns a [`FormatError`] only if the node count exceeds the head map's bound.
pub fn head_map(image: &VerifiedImage) -> Result<HeadMap, FormatError> {
    let (nodes, order) = split_order(image);
    let ledger_ids: Vec<LedgerIdBytes> = order.iter().map(|&i| nodes[i].path.node_id()).collect();
    HeadMap::assign(&ledger_ids)
}

/// The kind of each durable node in the same canonical split pre-order the head map numbers,
/// the other projection of [`split_order`]. This is the cross-crate enforcement artifact: a
/// test compares this sequence against the kernel's [`number_store`](marrow_kernel::durable::number_store)
/// structure flattened in the same order, so a divergence in the two independent walks — the
/// exact hazard of a two-owner numbering — fails a build rather than silently binding ledger
/// ids to the wrong cell numbers after a rename.
pub fn head_map_node_order(image: &VerifiedImage) -> Vec<SemanticNodeKind> {
    let (nodes, order) = split_order(image);
    order.iter().map(|&i| nodes[i].kind).collect()
}

/// The single owner of the durable graph's canonical split pre-order over the image's
/// [`semantic_nodes`](VerifiedImage::semantic_nodes): the node indices in the order the
/// kernel's `number_store` numbers them — each root in declaration order, then per node its
/// fields (in order), then its groups (each group node followed by its own members,
/// recursively), then its branches (each branch node followed by its members, recursively).
/// Managed-index nodes carry a 16-byte identity in their cell keys, not a number, so they are
/// excluded. Both [`head_map`] and [`head_map_node_order`] project this one walk, so they
/// cannot disagree.
fn split_order(image: &VerifiedImage) -> (Vec<SemanticNode>, Vec<usize>) {
    let nodes = image.semantic_nodes();

    // Children of each container, keyed by the container's full step chain, in the
    // declaration order `semantic_nodes` yields (a node before its descendants, members in
    // declaration order). A node's parent chain is its own chain minus the last step.
    let mut children: HashMap<Vec<SemanticStep>, Vec<usize>> = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        let steps = node.path.steps();
        if steps.len() >= 2 {
            let parent = steps[..steps.len() - 1].to_vec();
            children.entry(parent).or_default().push(index);
        }
    }

    let mut order: Vec<usize> = Vec::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        if node.kind == SemanticNodeKind::Root {
            walk_split_order(index, &nodes, &children, &mut order);
        }
    }
    (nodes, order)
}

/// Append `index`, then — in the kernel's split order — its field children, its group
/// children (each recursively, so a group node precedes its own members), and its branch
/// children (each recursively). A field is a cell-key leaf, so it is appended without
/// recursion. Because the shared counter that later consumes this sequence starts at zero and
/// advances one per node, node `i` is assigned number `i`, matching `number_store`.
fn walk_split_order(
    index: usize,
    nodes: &[SemanticNode],
    children: &HashMap<Vec<SemanticStep>, Vec<usize>>,
    out: &mut Vec<usize>,
) {
    out.push(index);
    let key = nodes[index].path.steps().to_vec();
    let Some(kids) = children.get(&key) else {
        return;
    };
    for &kid in kids {
        if nodes[kid].kind == SemanticNodeKind::Field {
            out.push(kid);
        }
    }
    for &kid in kids {
        if nodes[kid].kind == SemanticNodeKind::Group {
            walk_split_order(kid, nodes, children, out);
        }
    }
    for &kid in kids {
        if nodes[kid].kind == SemanticNodeKind::Branch {
            walk_split_order(kid, nodes, children, out);
        }
    }
}
