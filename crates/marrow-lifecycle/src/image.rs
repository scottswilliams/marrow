//! The projection from a verified program image to the lifecycle's persisted facts: the
//! active binding facts a binding-only rebind compares, and the head identity map that
//! pins each durable node's ledger id to its store-local cell-key number.
//!
//! The persisted facts derive purely from a [`VerifiedImage`] — the sole source of a valid
//! durable schema — so the store owner needs no dependency on the runner or the compiler.
//! The head-map pin verification additionally consumes the kernel's own cell numbering of
//! the projection an open installs, so the numbers it compares are the numbers that will
//! address cells.

use std::collections::HashMap;

use marrow_image::{LedgerIdBytes, interface_fingerprint};
use marrow_kernel::durable::{
    BranchNumbering, BranchSchema, FieldSchema, StoreProjection, number_store,
};
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
/// contract independently catches every durable-graph change. Authority is *not* a binding
/// fact — the accepted deployment ceiling is a separately owned standing maximum recorded
/// once at provision (see [`accepted_ceiling`])
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
/// Returns a [`FormatError`] when the node count exceeds the head map's bound, and when the
/// walk yields one ledger id twice — the map is a bijection over declaration identities, so
/// a program whose durable nodes do not carry distinct ids (two store roots of one resource
/// share their members' ids) has no head map and cannot be provisioned today.
pub fn head_map(image: &VerifiedImage) -> Result<HeadMap, FormatError> {
    let (nodes, order) = split_order(image);
    let ledger_ids: Vec<LedgerIdBytes> = order.iter().map(|&i| nodes[i].path.node_id()).collect();
    HeadMap::assign(&ledger_ids)
}

/// The first point at which a persisted head-map pin and the numbering this toolchain
/// derives disagree. The payload is typed so a tool asserts the disagreement itself, not a
/// rendered sentence; the hex spelling exists only in [`std::fmt::Display`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinDisagreement {
    /// A durable node the two bijections bind differently: the number the persisted pin
    /// binds its ledger id to (`None`: the pin does not carry the id), and the number the
    /// derivation assigns (`None`: the derivation does not reach the id). At least one side
    /// is always `Some`: a disagreement is reported only for a node that one of the two
    /// sides binds.
    Binding {
        ledger_id: LedgerIdBytes,
        persisted: Option<u32>,
        derived: Option<u32>,
    },
    /// Every binding agrees but the never-reuse high-water differs — a head claiming
    /// numbers were used and retired where the derivation retires nothing. No production
    /// path writes such a head today, so it is refused rather than tolerated.
    HighWater { persisted: u32, derived: u32 },
    /// A store-schema node the verified image does not name (or names ambiguously, or has
    /// already paired with another store node), so no distinct ledger identity can be
    /// paired with its cell number: the derivation cannot reproduce a pin at all, and the
    /// store is refused rather than attached under an unpaired numbering. Carries the
    /// node's `^root.member` spelling.
    Unnamed { place: String },
    /// A store-schema node and the image node at the same `^root.member` place are of
    /// different kinds — a group respelled as a keyed branch, say — so they share a name
    /// and a cell number but not a physical layout. Refused during derivation, independent
    /// of the persisted map, because same-kind numbering agreement says nothing about the
    /// layout the bytecode will address.
    Kind {
        place: String,
        image: SemanticNodeKind,
        store: SemanticNodeKind,
    },
    /// A durable node of the image the store shape never reaches, so the pairing consumed
    /// only part of the image: the numbering that was compared covers fewer nodes than the
    /// program addresses. Refused during derivation, independent of the persisted map, so
    /// a persisted map truncated to match cannot make the omission invisible. Coverage is
    /// decided over occurrence identity — the node's own semantic path — so a like-named
    /// member of a second root sharing the reported declaration identity is uncovered on
    /// its own account. Reported by ledger id, with its `^root.member` spelling when the
    /// image's sealed structure names the node (it always does for a flat-executable root).
    Uncovered {
        ledger_id: LedgerIdBytes,
        place: Option<String>,
    },
}

/// The head-map pin refusal: the store's persisted ledger-id ↔ cell-number bijection
/// (FR01 §3) disagrees with the (ledger id → cell number) binding this toolchain would
/// actually serve the store under. Fail-closed and recovery-shaped — serving the store
/// would readdress durable cells (ledger id X's bytes read as id Y's value), so the attach
/// refuses before any engine call. The head, envelope, and engine data are unchanged by the
/// refusal; only the lock's owner marker was rewritten by acquisition, so the next
/// successful open runs the unclean-open audit.
///
/// The pin protects the attach path: `crate::attach` derives the pin before the store is
/// touched and compares it inside the admission gate, after the single-owner lock and
/// before any engine call, whenever the incoming durable contract is the store's active
/// contract. A changed durable contract is instead classified as the typed
/// `store.contract_changed` refusal after the engine's physical open but before any session;
/// that path never attaches the store either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadMapPinMismatch {
    /// The first disagreement, in the derived walk order (then any pin-only binding, then
    /// the high-water), so the rendered refusal is a deterministic function of the delta.
    pub disagreement: PinDisagreement,
}

impl HeadMapPinMismatch {
    /// The stable dotted code a tool reports. The pin is part of the store's durable
    /// addressing, so a disagreement is recovery-shaped: whether the head bytes drifted or
    /// the toolchain's numbering did, the store's cells cannot be addressed under this
    /// pairing. The remedy is conditional on which it was (see [`std::fmt::Display`]) and
    /// one code covers both today; the store on a numbering-drift disagreement is healthy
    /// and decoded, so whether that cause deserves its own code family distinct from
    /// `store.corruption` is a codes-registry question outside this crate.
    pub fn code(&self) -> &'static str {
        marrow_codes::Code::StoreCorruption.as_str()
    }
}

/// The noun a refusal uses for a durable node kind.
fn kind_noun(kind: SemanticNodeKind) -> &'static str {
    match kind {
        SemanticNodeKind::Root => "store root",
        SemanticNodeKind::Group => "group",
        SemanticNodeKind::Branch => "keyed branch",
        SemanticNodeKind::Field => "field",
        SemanticNodeKind::Index => "managed index",
    }
}

/// Write a ledger id as lowercase hex.
fn write_ledger_id(f: &mut std::fmt::Formatter<'_>, ledger_id: &LedgerIdBytes) -> std::fmt::Result {
    for byte in ledger_id.bytes() {
        write!(f, "{byte:02x}")?;
    }
    Ok(())
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
                write!(f, "ledger id ")?;
                write_ledger_id(f, ledger_id)?;
                match persisted {
                    Some(number) => write!(f, " is pinned to number {number}")?,
                    None => write!(f, " is not in the pin")?,
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
            PinDisagreement::Unnamed { place } => write!(
                f,
                "the store node {place} cannot be paired with a distinct durable identity from \
                 the program image"
            )?,
            PinDisagreement::Kind {
                place,
                image,
                store,
            } => write!(
                f,
                "the store shape declares {place} as a {} where the program image declares a {}",
                kind_noun(*store),
                kind_noun(*image),
            )?,
            PinDisagreement::Uncovered { ledger_id, place } => {
                match place {
                    Some(place) => write!(f, "the program's durable node {place} (ledger id ")?,
                    None => write!(f, "the program's durable node with ledger id ")?,
                }
                write_ledger_id(f, ledger_id)?;
                write!(f, ") is not reached by the store shape")?;
            }
        }
        write!(
            f,
            ". The store is refused before any engine call: attaching it would address durable \
             cells under the wrong identities. Retry with the exact provisioning toolchain and \
             an image-derived projection. If that same derivation also disagrees, treat the \
             persisted head map as corrupt and stop; recovery requires known-good state, \
             because Marrow has no repair or migration path today"
        )
    }
}

impl std::error::Error for HeadMapPinMismatch {}

/// The derived head-map pin: the exact (ledger id → cell number) binding this toolchain
/// would serve the store under. The cell numbers are the kernel's own — [`number_store`]
/// over the exact projection this open installs — read in lockstep with the projection's
/// schemas, the same lockstep the kernel's site resolver performs, so the numbers compared
/// are the numbers that will address cells. Each schema node is then paired with its ledger
/// identity by its path-qualified source name **and its node kind** through the image's
/// sealed↔semantic correspondence (`crate::authority::named_durable_nodes`), each image
/// node claimed at most once and every image node claimed. Pairing by name rather than by
/// walk position is what makes the pin bite on derivation drift: kernel numbers are dense
/// over any projection shape, so a positionally paired comparison would accept a projection
/// that orders or shapes the store differently than the provisioning toolchain did, while
/// the name pairing turns that drift into different (id, number) pairs and a refusal. The
/// kind and coverage checks close what a name-and-number match alone leaves open: a store
/// shape that respells a node as another kind, or reaches only part of the program's graph,
/// numbers identically and is refused for its layout rather than its numbers.
///
/// Not bound here: a node's key arity and key scalar kinds, its field value shapes, and its
/// required flags. The image carries those facts, but their projection into the kernel's
/// vocabulary is owned by the VM's schema derivation, and the one production attach derives
/// its projection from the same image through that owner.
pub(crate) struct DerivedPin {
    /// One (ledger id, kernel cell number) per durable node, in the kernel's structural
    /// walk order (each root: the root, its fields, its groups and their fields, its
    /// branches recursively) — the deterministic order disagreements report in.
    pairs: Vec<(LedgerIdBytes, u32)>,
    /// One past the highest kernel number — the high-water a fresh pin over this numbering
    /// records.
    next_number: u32,
}

/// Derive the pin this toolchain would serve `image` under `projection` with, or the typed
/// refusal when the store shape and the image do not pair node for node: a store node the
/// image does not name, a node the two declare with different kinds, or an image node the
/// store shape never reaches. Pure over its inputs: no store access, one `number_store`
/// call, one hash map over the image's named nodes, one pass over its semantic nodes.
pub(crate) fn derive_head_map_pin(
    image: &VerifiedImage,
    projection: &StoreProjection,
) -> Result<DerivedPin, HeadMapPinMismatch> {
    let named = crate::authority::named_durable_nodes(image);
    let mut by_path: HashMap<&[String], usize> = HashMap::with_capacity(named.len());
    for (index, node) in named.iter().enumerate() {
        if by_path.insert(&node.path, index).is_some() {
            // Two image nodes under one spelling: the name join is ambiguous, so no pairing
            // is trustworthy. The compiler rejects duplicate member names, so this is reachable
            // only through correspondence drift — refused, never guessed. Payload precision,
            // not the refusal: without this the second node stays unpaired and the coverage
            // check below refuses the same store as `Uncovered`, so removing it changes only
            // which typed disagreement is reported.
            return Err(unnamed(&node.path));
        }
    }

    // `number_store` mirrors the projection's schema structure node for node, so the zips
    // below pair each schema node with its own kernel number by construction; only the
    // resolution against the image can fail.
    let numbering = number_store(projection);
    let mut pairing = Pairing {
        named: &named,
        by_path,
        consumed: vec![false; named.len()],
        pairs: Vec::with_capacity(named.len()),
        path: Vec::new(),
    };
    for (schema, numbers) in projection.roots().iter().zip(&numbering) {
        pairing.enter(schema.root_name(), SemanticNodeKind::Root, numbers.root())?;
        pairing.fields(schema.fields(), numbers.fields())?;
        for (group, group_numbers) in schema.groups().iter().zip(numbers.groups()) {
            pairing.enter(
                group.name(),
                SemanticNodeKind::Group,
                group_numbers.number(),
            )?;
            pairing.fields(group.fields(), group_numbers.fields())?;
            pairing.leave();
        }
        pairing.branches(schema.branches(), numbers.branches())?;
        pairing.leave();
    }

    // Coverage: every durable node the image numbers (every semantic node but a managed
    // index, whose cell keys carry an identity rather than a number) was consumed by the
    // walk above. Checked over the image's own node list rather than the named join, so a
    // node the join could not name is uncovered too, never silently absent.
    //
    // Coverage is keyed on occurrence identity — the node's index into `semantic_nodes`,
    // standing for its whole kind-tagged semantic path — never on its ledger id. A ledger
    // id names a declaration, so two roots of one resource give their like-named members
    // one id; keying on the id would let a projection that covers `^a.v` silently cover
    // `^b.v` as well.
    let mut covered = vec![false; image.semantic_nodes().len()];
    for (index, &claimed) in pairing.consumed.iter().enumerate() {
        if claimed {
            covered[named[index].semantic_index] = true;
        }
    }
    if let Some((index, node)) = image
        .semantic_nodes()
        .iter()
        .enumerate()
        .filter(|(_, node)| node.kind != SemanticNodeKind::Index)
        .find(|&(index, _)| !covered[index])
    {
        let place = named
            .iter()
            .find(|named| named.semantic_index == index)
            .map(|named| spell_place(&named.path));
        return Err(HeadMapPinMismatch {
            disagreement: PinDisagreement::Uncovered {
                ledger_id: node.path.node_id(),
                place,
            },
        });
    }

    let next_number = pairing
        .pairs
        .iter()
        .map(|&(_, number)| number)
        .max()
        .map_or(0, |max| max + 1);
    Ok(DerivedPin {
        pairs: pairing.pairs,
        next_number,
    })
}

/// The in-progress pairing of store-schema nodes with image durable nodes: the store walk
/// descends the schema in the kernel's numbering order while `path` spells the node under
/// the cursor, and each store node claims exactly one image node of the same name and kind.
struct Pairing<'a> {
    named: &'a [crate::authority::NamedDurableNode],
    /// The image node at each name path, by index into `named`.
    by_path: HashMap<&'a [String], usize>,
    /// Which image nodes a store node has already claimed, so two store nodes can never
    /// share one identity.
    consumed: Vec<bool>,
    pairs: Vec<(LedgerIdBytes, u32)>,
    path: Vec<String>,
}

impl Pairing<'_> {
    /// Descend into the store node `name` of `kind`, numbered `number` by the kernel, and
    /// pair it with the image node at that place. The caller balances it with [`leave`].
    ///
    /// [`leave`]: Pairing::leave
    fn enter(
        &mut self,
        name: &str,
        kind: SemanticNodeKind,
        number: u32,
    ) -> Result<(), HeadMapPinMismatch> {
        self.path.push(name.to_string());
        let Some(&index) = self.by_path.get(self.path.as_slice()) else {
            return Err(unnamed(&self.path));
        };
        // Payload precision, not the refusal: a second claim would pair one ledger id with
        // two numbers, and `DerivedPin::verify` refuses that pair anyway (the persisted
        // entry is removed by the first, so the second reports a `Binding` disagreement).
        // Removing this guard changes only which typed disagreement is reported.
        if std::mem::replace(&mut self.consumed[index], true) {
            return Err(unnamed(&self.path));
        }
        let node = &self.named[index];
        if node.kind != kind {
            return Err(HeadMapPinMismatch {
                disagreement: PinDisagreement::Kind {
                    place: spell_place(&self.path),
                    image: node.kind,
                    store: kind,
                },
            });
        }
        self.pairs.push((node.ledger_id, number));
        Ok(())
    }

    fn leave(&mut self) {
        self.path.pop();
    }

    /// Pair the fields of the node under the cursor with their mirrored numbers.
    fn fields(
        &mut self,
        fields: &[FieldSchema],
        numbers: &[u32],
    ) -> Result<(), HeadMapPinMismatch> {
        for (field, &number) in fields.iter().zip(numbers) {
            self.enter(field.name(), SemanticNodeKind::Field, number)?;
            self.leave();
        }
        Ok(())
    }

    /// Pair one level of keyed branches with its mirrored numbering, recursively.
    fn branches(
        &mut self,
        branches: &[BranchSchema],
        numbering: &[BranchNumbering],
    ) -> Result<(), HeadMapPinMismatch> {
        for (branch, numbers) in branches.iter().zip(numbering) {
            self.enter(branch.name(), SemanticNodeKind::Branch, numbers.number())?;
            self.fields(branch.fields(), numbers.fields())?;
            self.branches(branch.branches(), numbers.branches())?;
            self.leave();
        }
        Ok(())
    }
}

/// The refusal for a store node at `path` that pairs with no distinct image identity.
fn unnamed(path: &[String]) -> HeadMapPinMismatch {
    HeadMapPinMismatch {
        disagreement: PinDisagreement::Unnamed {
            place: spell_place(path),
        },
    }
}

/// A durable node's `^root.member` source spelling from its name path.
fn spell_place(path: &[String]) -> String {
    let mut out = String::new();
    for (index, segment) in path.iter().enumerate() {
        out.push(if index == 0 { '^' } else { '.' });
        out.push_str(segment);
    }
    out
}

impl DerivedPin {
    /// Compare this derivation against the persisted pin. Total over any decoded
    /// [`HeadMap`]: one hash map over the persisted entries, the first disagreement
    /// reported in the derived walk order, then any pin-only binding in the persisted
    /// encoding order, then the high-water — never a panic.
    pub(crate) fn verify(&self, persisted: &HeadMap) -> Result<(), HeadMapPinMismatch> {
        let mut pinned: HashMap<[u8; 16], u32> = persisted
            .entries()
            .iter()
            .map(|entry| (*entry.ledger_id.bytes(), entry.number))
            .collect();
        for &(ledger_id, derived_number) in &self.pairs {
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
        // A pin-only binding: an id the persisted map carries that the derivation never
        // pairs. Payload precision, not the refusal: a decoded map forbids a duplicate
        // number and one at or above its high-water, so a map with an extra entry after
        // every derived pair matched necessarily carries a higher high-water and the check
        // below refuses it. Removing this branch reports `HighWater` instead, never `Ok`.
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
        if persisted.next_number() != self.next_number {
            return Err(HeadMapPinMismatch {
                disagreement: PinDisagreement::HighWater {
                    persisted: persisted.next_number(),
                    derived: self.next_number,
                },
            });
        }
        Ok(())
    }
}

/// Verify the store's persisted head-map pin against the numbering this toolchain would
/// actually serve it under: the kernel's [`number_store`] numbers over the exact
/// `projection` the open installs, paired to the image's durable identities by source name
/// and kind (see [`DerivedPin`]). The attach actor runs this after the single-owner lock and
/// before any engine call, exactly when the incoming durable contract equals the store's
/// active contract — the one case where the persisted pin claims to describe the presented
/// image's numbering.
pub fn verify_head_map_pin(
    image: &VerifiedImage,
    projection: &StoreProjection,
    persisted: &HeadMap,
) -> Result<(), HeadMapPinMismatch> {
    derive_head_map_pin(image, projection)?.verify(persisted)
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
fn split_order(image: &VerifiedImage) -> (&[SemanticNode], Vec<usize>) {
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
            walk_split_order(index, nodes, &children, &mut order);
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
