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
/// plus the binding facts a binding-only rebind compares (the durable contract and the
/// export-set interface fingerprint). The interface fingerprint is a runner-free digest over
/// the image's export declaration identities (see [`interface_fingerprint`]) — blind to
/// signatures, so a resignatured export is not a binding-fact delta today; the durable
/// contract independently catches every durable-graph change. Authority is *not* a binding fact — the accepted deployment ceiling is a
/// separately owned standing maximum recorded once at provision (see [`accepted_ceiling`])
/// and enforced atom-granularly at attach, so a demand change within the ceiling is not a
/// rebind delta and a demand change beyond it is a distinct, more actionable refusal.
pub fn active_binding(image: &VerifiedImage) -> ActiveBinding {
    let export_ids: Vec<[u8; 32]> = image
        .exports()
        .iter()
        .map(|export| *export.id().bytes())
        .collect();
    ActiveBinding {
        image_format_version: IMAGE_FORMAT_VERSION,
        image_id: image.image_id().0,
        durable_contract: *image.durable_contract().bytes(),
        interface: interface_fingerprint(&export_ids),
    }
}

/// The accepted deployment ceiling a store records for `image` at provision: the canonical
/// atom-set payload of the ceiling over the image's whole-program demand union — the
/// separately owned standing maximum authority the store admits. Persisted verbatim in the
/// head ([`crate::LogicalHead::accepted_ceiling`]) and reconstructed at attach with
/// [`marrow_image::CeilingDescriptor::from_payload`] for the atom-granular admission check.
/// The compiler describes demand; provision accepts it as the ceiling; neither grants — the
/// attach check intersects the presented image's demand with this bound.
pub fn accepted_ceiling(image: &VerifiedImage) -> Vec<u8> {
    CeilingDescriptor::from_demand_union(image.demand_union()).atom_set_payload()
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

/// The first point at which a persisted head-map pin and the numbering this toolchain
/// derives disagree. The payload is typed so a tool asserts the disagreement itself, not a
/// rendered sentence; the hex spelling exists only in [`std::fmt::Display`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinDisagreement {
    /// A durable node the two bijections bind differently: the number the persisted pin
    /// binds its ledger id to (`None`: the pin does not carry the id), and the number the
    /// derivation assigns (`None`: the derivation does not reach the id).
    Binding {
        ledger_id: LedgerIdBytes,
        persisted: Option<u32>,
        derived: Option<u32>,
    },
    /// Every binding agrees but the never-reuse high-water differs — a head claiming
    /// numbers were used and retired where the derivation retires nothing. No production
    /// path writes such a head today, so it is refused rather than tolerated.
    HighWater { persisted: u32, derived: u32 },
}

/// The head-map pin refusal: the store's persisted ledger-id ↔ cell-number bijection
/// (FR01 §3) disagrees with the numbering this toolchain derives for the store's active
/// durable contract. Fail-closed and recovery-shaped — serving the store would readdress
/// durable cells (ledger id X's bytes read as id Y's value), so the attach refuses before
/// any engine call and the store bytes are untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadMapPinMismatch {
    /// The first disagreement, in the derived walk order (then any pin-only binding, then
    /// the high-water), so the rendered refusal is a deterministic function of the delta.
    pub disagreement: PinDisagreement,
}

impl HeadMapPinMismatch {
    /// The stable dotted code a tool reports. The pin is part of the store's durable
    /// addressing, so a disagreement is recovery-shaped: whether the head bytes drifted or
    /// the toolchain's walk did, the store's cells cannot be addressed under this pairing.
    pub fn code(&self) -> &'static str {
        marrow_codes::Code::StoreCorruption.as_str()
    }
}

impl std::fmt::Display for HeadMapPinMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the store's persisted head identity map (the ledger-id \u{2194} cell-number pin) \
             disagrees with the numbering this toolchain derives for the store's active \
             program: "
        )?;
        match &self.disagreement {
            PinDisagreement::Binding {
                ledger_id,
                persisted,
                derived,
            } => {
                let mut hex = String::with_capacity(32);
                for byte in ledger_id.bytes() {
                    use std::fmt::Write;
                    let _ = write!(hex, "{byte:02x}");
                }
                match persisted {
                    Some(number) => write!(f, "ledger id {hex} is pinned to number {number}")?,
                    None => write!(f, "ledger id {hex} is not in the pin")?,
                }
                match derived {
                    Some(number) => write!(f, " but derives number {number}")?,
                    None => write!(f, " but is not derived")?,
                }
            }
            PinDisagreement::HighWater { persisted, derived } => write!(
                f,
                "every binding agrees but the pinned high-water is {persisted} where the \
                 derivation's is {derived}"
            )?,
        }
        write!(
            f,
            ". The store is refused before any data access: serving it would address durable \
             cells under the wrong identities. Restore the store from a trusted backup, or \
             attach with the toolchain that provisioned it"
        )
    }
}

/// Verify the persisted head-map pin against the numbering this toolchain derives for
/// `image`: every ledger id must carry exactly the number position the canonical split
/// pre-order assigns (the same `number i = i`-th walked node mint [`head_map`] projects at
/// provision), no binding may exist on only one side, and the high-water must be the walked
/// node count. Total over any decoded [`HeadMap`] — the comparison consumes the persisted
/// bijection and the derived walk, allocating one hash map over the persisted entries, and
/// returns the first disagreement rather than panicking on any shape.
///
/// The attach actor runs this after the single-owner lock and before any engine call,
/// exactly when the incoming durable contract equals the store's active contract — the one
/// case where the persisted pin claims to describe the presented image's numbering.
pub fn verify_head_map_pin(
    image: &VerifiedImage,
    persisted: &HeadMap,
) -> Result<(), HeadMapPinMismatch> {
    let (nodes, order) = split_order(image);
    let mut pinned: HashMap<[u8; 16], u32> = persisted
        .entries()
        .iter()
        .map(|entry| (*entry.ledger_id.bytes(), entry.number))
        .collect();
    for (derived_number, &node) in order.iter().enumerate() {
        let ledger_id = nodes[node].path.node_id();
        let derived_number = derived_number as u32;
        match pinned.remove(ledger_id.bytes()) {
            Some(number) if number == derived_number => {}
            pinned_number => {
                return Err(HeadMapPinMismatch {
                    disagreement: PinDisagreement::Binding {
                        ledger_id,
                        persisted: pinned_number,
                        derived: Some(derived_number),
                    },
                });
            }
        }
    }
    // A pin-only binding: an id the persisted map carries that the derivation never walks.
    // Reported in the persisted encoding order, the map's own deterministic order.
    if let Some(entry) = persisted
        .entries()
        .iter()
        .find(|entry| pinned.contains_key(entry.ledger_id.bytes()))
    {
        return Err(HeadMapPinMismatch {
            disagreement: PinDisagreement::Binding {
                ledger_id: entry.ledger_id,
                persisted: Some(entry.number),
                derived: None,
            },
        });
    }
    let derived = order.len() as u32;
    if persisted.next_number() != derived {
        return Err(HeadMapPinMismatch {
            disagreement: PinDisagreement::HighWater {
                persisted: persisted.next_number(),
                derived,
            },
        });
    }
    Ok(())
}

/// The kind **and ledger identity** of each durable node in the same canonical split
/// pre-order the head map numbers, the other projection of [`split_order`]. This is the
/// cross-crate enforcement artifact: a test compares this sequence against the kernel's
/// [`number_store`](marrow_kernel::durable::number_store) structure flattened in the same
/// order, so a divergence in the two independent walks — the exact hazard of a two-owner
/// numbering — fails a build rather than silently binding ledger ids to the wrong cell
/// numbers. The identity travels with the kind because two same-kind siblings swapped in
/// only one walk keep the kind sequence identical; only their ids reveal the drift.
pub fn head_map_node_order(image: &VerifiedImage) -> Vec<(SemanticNodeKind, LedgerIdBytes)> {
    let (nodes, order) = split_order(image);
    order
        .iter()
        .map(|&i| (nodes[i].kind, nodes[i].path.node_id()))
        .collect()
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
    let nodes = image.semantic_nodes().to_vec();

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
