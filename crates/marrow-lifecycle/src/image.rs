//! Image-derived store shape, active binding and durable identity correspondence.
//! Lifecycle pairs the verified image with its exact projection; accepted Head metadata
//! supplies physical addresses. The kernel validates and retains the numbered projection.

use std::collections::HashMap;

use marrow_image::{IMAGE_FORMAT_VERSION, LedgerIdBytes, interface_fingerprint};
use marrow_kernel::codec::value::{ScalarKind, ValueShape, ValueShapeBuilder};
use marrow_kernel::durable::{
    BranchSchema, FieldSchema, IndexComponent, NumberingError, SiteTarget, StoreProjection,
    StoreProjectionBuilder, StoreSchema, StoreSchemaBuilder,
};
use marrow_verify::{
    CeilingDescriptor, ImageType, RootId, Scalar, SealedIndexComponent, SealedSite,
    SealedSiteTarget, SemanticNode, SemanticNodeKind, SemanticStep, VerifiedImage,
};

use crate::codec::FormatError;
use crate::head::ActiveBinding;
use crate::headmap::HeadMap;
use marrow_codes::Code;

/// Derive the active binding a store records for `image`: the active image's byte identity
/// plus the binding facts a binding-only rebind compares (the durable contract and the
/// export-set interface fingerprint). The interface fingerprint is a runner-free digest over
/// the image's export declaration identities (see [`interface_fingerprint`]) — blind to
/// signatures, so a resignatured export is not a binding-fact delta today; the durable
/// contract independently catches every durable-graph change. Authority is *not* a binding
/// fact; the accepted deployment ceiling is separately owned (see [`accepted_ceiling`] and
/// [`ActiveBinding`]).
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

/// The declaration identity of every durable node the store numbers, in the kernel's
/// canonical split pre-order (see [`split_order`]). This is the sole owner of head-map node
/// eligibility: a managed index carries a 16-byte identity in its cell keys rather than a
/// number, so the walk excludes it, and every caller that asks which nodes the map binds
/// asks here.
pub(crate) fn numbered_node_ids(image: &VerifiedImage) -> Vec<LedgerIdBytes> {
    let (nodes, order) = split_order(image);
    order.iter().map(|&i| nodes[i].path.node_id()).collect()
}

/// Build the head identity map for `image`: the ledger-id ↔ cell-number bijection, where
/// node `i` in the store-local cell-key numbering is the `i`-th durable node in
/// [`numbered_node_ids`].
///
/// Returns a [`FormatError`] when the node count exceeds the head map's bound, and when the
/// walk yields one ledger id twice — the map is a bijection over declaration identities, so
/// a program whose durable nodes do not carry distinct ids (two store roots of one resource
/// share their members' ids) has no head map and cannot be provisioned today.
pub fn head_map(image: &VerifiedImage) -> Result<HeadMap, FormatError> {
    HeadMap::assign(&numbered_node_ids(image))
}

/// A mismatch between the verified image, its projection and the accepted identity map.
/// The Head supplies addresses; these checks do not authenticate its historical authorship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinDisagreement {
    /// An image occurrence has no distinct accepted binding.
    Missing { ledger_id: LedgerIdBytes },
    /// The Head binds an identity that the projection does not reach.
    Unexpected {
        ledger_id: LedgerIdBytes,
        number: u32,
    },
    /// Physical address coverage, uniqueness or high-water validation failed.
    Numbering(NumberingError),
    /// A projection node has no distinct image occurrence at its `^root.member` spelling.
    Unnamed { place: String },
    /// The projection and image disagree on a node's kind at the same semantic path.
    /// Matching physical numbers cannot establish matching layout.
    Kind {
        place: String,
        image: SemanticNodeKind,
        store: SemanticNodeKind,
    },
    /// An image occurrence is absent from the projection. Coverage uses semantic paths,
    /// so another root sharing a declaration identity cannot satisfy this occurrence.
    Uncovered {
        ledger_id: LedgerIdBytes,
        place: Option<String>,
    },
}

/// The accepted Head cannot be paired with the image-derived store projection.
/// Admission refuses before opening the engine. This checks current correspondence,
/// not the provenance of a deliberately rewritten and resealed Head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadMapPinMismatch {
    /// The first disagreement in structural order, then any unmatched Head binding,
    /// then the kernel's physical-number validation.
    pub disagreement: PinDisagreement,
}

impl HeadMapPinMismatch {
    /// The typed store-corruption code for an unusable image/projection/Head pairing.
    pub fn code(&self) -> Code {
        marrow_codes::Code::StoreCorruption
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
             cannot describe the active program's store projection: "
        )?;
        match &self.disagreement {
            PinDisagreement::Missing { ledger_id } => {
                write!(f, "ledger id ")?;
                write_ledger_id(f, ledger_id)?;
                write!(f, " has no distinct accepted binding")?;
            }
            PinDisagreement::Unexpected { ledger_id, number } => {
                write!(f, "accepted number {number} names ledger id ")?;
                write_ledger_id(f, ledger_id)?;
                write!(f, " which the projection does not reach")?;
            }
            PinDisagreement::Numbering(error) => write!(f, "invalid physical numbering: {error}")?,
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
            ". The store is refused before engine opening. Use the matching image and intact accepted Head."
        )
    }
}

impl std::error::Error for HeadMapPinMismatch {}

/// Durable declaration identities in the kernel's structural order. The occurrence
/// join checks source paths, kinds and complete image coverage before any map lookup.
/// Indexes use stable identities directly and have no compact node number.
pub(crate) struct ProjectedNodes {
    ids: Vec<LedgerIdBytes>,
}

/// Join each projection occurrence with its verified image identity once.
/// The path-and-kind join detects missing, duplicate or differently shaped occurrences;
/// coverage is checked against all numbered image nodes. No physical numbers are minted.
pub(crate) fn derive_projection_nodes(
    image: &VerifiedImage,
    projection: &StoreProjection,
) -> Result<ProjectedNodes, HeadMapPinMismatch> {
    let named = crate::authority::named_durable_nodes(image);
    let mut by_path: HashMap<&[String], usize> = HashMap::with_capacity(named.len());
    for (index, node) in named.iter().enumerate() {
        if by_path.insert(&node.path, index).is_some() {
            // Two image nodes under one spelling: the name join is ambiguous, so no pairing
            // is trustworthy. The compiler rejects duplicate member names, so this is reachable
            // only through correspondence drift — refused, never guessed.
            return Err(unnamed(&node.path));
        }
    }

    let mut pairing = Pairing {
        named: &named,
        by_path,
        consumed: vec![false; named.len()],
        ids: Vec::with_capacity(named.len()),
        path: Vec::new(),
    };
    for schema in projection.roots() {
        pairing.enter(schema.root_name(), SemanticNodeKind::Root)?;
        pairing.fields(schema.fields())?;
        for group in schema.groups() {
            pairing.enter(group.name(), SemanticNodeKind::Group)?;
            pairing.fields(group.fields())?;
            pairing.leave();
        }
        pairing.branches(schema.branches())?;
        pairing.leave();
    }

    // Coverage: every durable node the image numbers (every semantic node but a managed
    // index, whose cell keys carry an identity rather than a number) was consumed by the
    // walk above. Checked over the image's own node list rather than the named join, so a
    // node the join could not name is uncovered too, never silently absent.
    //
    // Keyed on occurrence identity — the index into `semantic_nodes`, standing for the
    // whole kind-tagged semantic path — never on the ledger id: an id names a declaration,
    // so two roots of one resource share their like-named members' id, and keying on it
    // would let a projection covering `^a.v` silently cover `^b.v` too.
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

    Ok(ProjectedNodes { ids: pairing.ids })
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
    ids: Vec<LedgerIdBytes>,
    path: Vec<String>,
}

impl Pairing<'_> {
    /// Pair the store node with the image occurrence at this path and kind.
    /// The caller balances the descent with [`leave`].
    ///
    /// [`leave`]: Pairing::leave
    fn enter(&mut self, name: &str, kind: SemanticNodeKind) -> Result<(), HeadMapPinMismatch> {
        self.path.push(name.to_string());
        let Some(&index) = self.by_path.get(self.path.as_slice()) else {
            return Err(unnamed(&self.path));
        };
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
        self.ids.push(node.ledger_id);
        Ok(())
    }

    fn leave(&mut self) {
        self.path.pop();
    }

    /// Pair fields in declaration order.
    fn fields(&mut self, fields: &[FieldSchema]) -> Result<(), HeadMapPinMismatch> {
        for field in fields {
            self.enter(field.name(), SemanticNodeKind::Field)?;
            self.leave();
        }
        Ok(())
    }

    /// Pair keyed branches in declaration order, including nested branches.
    fn branches(&mut self, branches: &[BranchSchema]) -> Result<(), HeadMapPinMismatch> {
        for branch in branches {
            self.enter(branch.name(), SemanticNodeKind::Branch)?;
            self.fields(branch.fields())?;
            self.branches(branch.branches())?;
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

impl ProjectedNodes {
    /// Resolve exactly one accepted number per occurrence in structural order.
    /// Removing each matched ID also refuses repeated declaration identities.
    pub(crate) fn accepted_numbers(
        &self,
        persisted: &HeadMap,
    ) -> Result<Vec<u32>, HeadMapPinMismatch> {
        let mut pinned: HashMap<[u8; 16], u32> = persisted
            .entries()
            .iter()
            .map(|entry| (*entry.ledger_id.bytes(), entry.number))
            .collect();
        let mut numbers = Vec::with_capacity(self.ids.len());
        for &ledger_id in &self.ids {
            let number = pinned.remove(ledger_id.bytes()).ok_or(HeadMapPinMismatch {
                disagreement: PinDisagreement::Missing { ledger_id },
            })?;
            numbers.push(number);
        }
        // Every persisted binding must belong to this image, even when gaps below
        // the lifetime high-water are valid.
        if let Some(entry) = persisted
            .entries()
            .iter()
            .find(|entry| pinned.contains_key(entry.ledger_id.bytes()))
        {
            return Err(HeadMapPinMismatch {
                disagreement: PinDisagreement::Unexpected {
                    ledger_id: entry.ledger_id,
                    number: entry.number,
                },
            });
        }
        Ok(numbers)
    }
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;

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

/// Append `index` and its subtree in the split order [`split_order`] defines. The sequence
/// is consumed by a counter starting at zero and advancing one per node, so node `i` is
/// assigned number `i`, matching `number_store`.
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

/// Derive the store's root-indexed schema table and the index-aligned site table from a
/// verified image, or `None` when the image's durable shape is not executable by the flat
/// kernel (a storeless image, a singleton root, a nested group, or a nominal-typed field).
/// Every declared root must be flat-executable; if any one parks, the whole image parks,
/// since a partial store — some roots served, others silently absent — is never minted.
/// Derived once per [`crate::PreparedImage`]; the in-memory attachment, the persistent
/// provision, attach, and import all open their engine under this one table, so a store is
/// served under exactly the shape the running program expects.
pub(crate) fn derive_projection(image: &VerifiedImage) -> Option<StoreProjection> {
    // A durable image declares at least one root; a storeless image never reaches attach.
    if image.roots().is_empty() {
        return None;
    }

    // One StoreSchema per declared root, in declaration order, plus each root's offset into
    // the image-wide managed-index table. A site names its index by that image-wide
    // position; the kernel resolves it against its own root's schema, so the offset rebases
    // the position to root-local when the site table is built.
    let mut projection = StoreProjection::builder();
    let mut index_offsets = Vec::with_capacity(image.roots().len());
    let mut running_indexes = 0u16;
    for (root_index, root) in image.roots().iter().enumerate() {
        let schema = derive_root_schema(image, RootId::from_index(root_index as u16), root)?;
        index_offsets.push(running_indexes);
        running_indexes = running_indexes.checked_add(schema.indexes().len() as u16)?;
        projection.root(schema);
    }

    // The site table is index-aligned with the image's sites so `Durable::site` resolves by
    // image site index. A parked site is never referenced by a verified durable opcode (the
    // verifier refuses that in phase 3), so it keeps its slot as the kernel's typed absence
    // rather than as a semantically valid placeholder site.
    for site in image.sites() {
        emit_site(&mut projection, site, &index_offsets)?;
    }

    // Every position the sites name is resolved against the completed roots here. A verified
    // image cannot name a position its own roots do not declare, so a refusal is a divergence
    // between the verifier's shape and this projection, and the image parks rather than
    // attaching under a site table the resolver would have to trust.
    projection.finish().ok()
}

/// Derive one root's [`StoreSchema`] from the image, or `None` when the root is not
/// flat-executable (a singleton keyless root, or a group nested below its direct members).
///
/// The projection is a flat command stream into the kernel's schema builder over an
/// explicit stack, so a hostile or divergent branch tree costs the walk's own heap, not the
/// machine stack, and the builder returns a typed refusal that parks the root.
fn derive_root_schema(
    image: &VerifiedImage,
    root_index: RootId,
    root: &marrow_verify::SealedRoot,
) -> Option<StoreSchema> {
    // The executable layout is the keyed root (any key arity, its fields scalar or widened
    // composite) plus root-level unkeyed groups of storable-value fields plus field-only keyed
    // branches nested to any depth, including composite-keyed branches. A root with a group
    // nested below its direct members (or a nested/composite-keyed shape the flat kernel
    // cannot serve) is not yet executable (`has_extras`); a singleton (keyless) root has no
    // key columns and parks.
    if root.has_extras() || root.keys().is_empty() {
        return None;
    }
    let mut builder = StoreSchemaBuilder::root(root.name().to_string(), key_columns(root.keys()));

    // The unified root record is `[leading value fields][one Record slot per root-level
    // group]`, in declaration order. The typed split carries the two halves by name; the
    // kernel's flat field set is only the value fields, and the group slots become groups
    // below.
    let split = RecordSplit::of(image.record_type(root.record()), root.groups().len())?;
    emit_fields(image, &mut builder, split.value_fields)?;
    // A trailing group slot contributes no kernel field, but the root still parks unless
    // every slot of the unified record is a shape the durable codec stores, so each one is
    // derived and discarded.
    for slot in split.group_slots {
        value_shape(image, slot.ty())?;
    }

    // Each root-level group derives its own materialized record from the image; a group is a
    // value unit of the root entry, addressed by the root's key-path, so it carries a field
    // set but no key.
    for group in root.groups() {
        builder.open_group(group.name().to_string());
        emit_fields(
            image,
            &mut builder,
            image.record_type(group.record()).fields(),
        )?;
        builder.close_group();
    }

    // The sealed branch tree is in declaration order, so a `BranchEntry` branch path indexes
    // it level by level. The walk is an explicit stack over that tree, keeping a divergent
    // branch depth off the machine stack.
    let mut pending: Vec<BranchStep<'_>> = Vec::new();
    push_branches(&mut pending, root.branches());
    while let Some(step) = pending.pop() {
        match step {
            BranchStep::Open(branch) => {
                builder.open_branch(branch.name().to_string(), key_columns(branch.keys()));
                emit_fields(
                    image,
                    &mut builder,
                    image.record_type(branch.record()).fields(),
                )?;
                pending.push(BranchStep::Close);
                push_branches(&mut pending, branch.branches());
            }
            BranchStep::Close => {
                builder.close_branch();
            }
        }
        builder.refusal().map_or(Some(()), |_| None)?;
    }

    // This root's own managed indexes, in declaration order, each with a projection the
    // builder resolves against the completed root. An index over a parked root never reaches
    // here (the root parks above before its indexes are read).
    for index in image
        .indexes()
        .iter()
        .filter(|index| index.root() == root_index)
    {
        builder.index(
            *index.id().bytes(),
            index.unique(),
            index
                .projection()
                .iter()
                .map(|component| match component {
                    SealedIndexComponent::Key(column) => IndexComponent::key(*column),
                    SealedIndexComponent::Field(field) => IndexComponent::field(*field),
                })
                .collect(),
        );
    }

    builder.finish().ok()
}

/// The unified root record's two typed halves: the leading value fields the kernel
/// materializes directly, and the trailing per-group `Record` slots (one per root-level
/// group, in group declaration order) that become kernel groups instead. Splitting once,
/// by name, keeps the layout fact in one place rather than as positional truncation at
/// each use.
struct RecordSplit<'a> {
    value_fields: &'a [marrow_verify::SealedField],
    group_slots: &'a [marrow_verify::SealedField],
}

impl<'a> RecordSplit<'a> {
    /// Split `record` before its trailing `group_count` slots. `None` when the record
    /// holds fewer fields than the root holds groups — a divergence from the image's
    /// unified-record layout — so the caller parks the root.
    fn of(record: &'a marrow_verify::SealedRecordType, group_count: usize) -> Option<Self> {
        let values = record.fields().len().checked_sub(group_count)?;
        let (value_fields, group_slots) = record.fields().split_at(values);
        Some(Self {
            value_fields,
            group_slots,
        })
    }
}

/// One step of the explicit branch walk: open a sealed branch, or close the branch whose
/// subtree has been emitted.
enum BranchStep<'a> {
    Open(&'a marrow_verify::SealedBranch),
    Close,
}

/// Queue a level of sealed branches so they are opened in declaration order.
fn push_branches<'a>(
    pending: &mut Vec<BranchStep<'a>>,
    branches: &'a [marrow_verify::SealedBranch],
) {
    pending.extend(branches.iter().rev().map(BranchStep::Open));
}

/// The kernel key-column kinds of a sealed key tuple.
fn key_columns(keys: &[Scalar]) -> Vec<ScalarKind> {
    keys.iter().map(|scalar| scalar_kind(*scalar)).collect()
}

/// Emit one node's record fields into the schema builder, in declaration order, each with
/// its storable value shape. `None` when a field is a collection, unit, or identity — shapes
/// the durable field codec never stores inline — so the whole derivation parks. The verifier
/// proves an executable node's record fields are a scalar or a widened composite, so this is
/// defense in depth over that proof.
fn emit_fields(
    image: &VerifiedImage,
    builder: &mut StoreSchemaBuilder,
    fields: &[marrow_verify::SealedField],
) -> Option<()> {
    for field in fields {
        builder.field(
            field.name().to_string(),
            value_shape(image, field.ty())?,
            field.required(),
        );
    }
    builder.refusal().map_or(Some(()), |_| None)
}

/// Project one sealed site into the kernel's site table, tagging it with its root's
/// declaration position and rebasing an index-read position from image-wide to root-local.
/// A parked site — never referenced by a verified durable opcode — keeps its slot as the
/// table's typed absence. `None` when an index position does not project onto its own
/// root's table — a divergence from the verifier's shape — so the caller parks the image
/// rather than publishing a table it would have to trust.
fn emit_site(
    projection: &mut StoreProjectionBuilder,
    site: &SealedSite,
    index_offsets: &[u16],
) -> Option<()> {
    let (root, target) = match site {
        SealedSite::Flat { root, target, .. } => (root.wire_index(), target),
        SealedSite::Parked { .. } => {
            projection.parked_site();
            return Some(());
        }
    };
    let target = match target {
        SealedSiteTarget::WholePayload => SiteTarget::whole_payload(),
        SealedSiteTarget::FieldLeaf(field) => SiteTarget::field_leaf(*field),
        SealedSiteTarget::BranchEntry(branch) => SiteTarget::branch_entry(branch.clone()),
        SealedSiteTarget::BranchField { branch, field } => {
            SiteTarget::branch_field(branch.clone(), *field)
        }
        SealedSiteTarget::GroupEntry(group) => SiteTarget::group_entry(*group),
        // An index-read site names its index by image-wide position; the kernel resolves it
        // against this root's own schema, so the checked projection rebases it by the
        // root's index offset. Underflow or an unknown root is a shape divergence.
        SealedSiteTarget::IndexScan(index) => {
            SiteTarget::index_scan(root_local_index(*index, root, index_offsets)?)
        }
        SealedSiteTarget::IndexLookup(index) => {
            SiteTarget::index_lookup(root_local_index(*index, root, index_offsets)?)
        }
    };
    projection.site(root, target);
    Some(())
}

/// Rebase one image-wide managed-index position onto its root's own index table: the typed
/// root-local position the kernel's site targets carry. `None` when the position sits below
/// its root's first index or the root is unknown — either way the image's shape and the
/// derived projection disagree, and the caller parks.
fn root_local_index(index: u16, root: u16, index_offsets: &[u16]) -> Option<u16> {
    index.checked_sub(*index_offsets.get(root as usize)?)
}

/// Derive a field's kernel [`ValueShape`] from its image type: a scalar carries its kind; a
/// record becomes a product of its fields' shapes in declaration order; a closed enum
/// (`Option`/`Result`/a user `enum`) becomes a sum of its variants' dense payload shapes. A
/// collection, unit, or identity is not an inline field value, so it parks (`None`).
///
/// The walk is an explicit stack emitting flat builder commands, so the projection's own
/// depth is heap-bounded and the shape's depth is the builder's to refuse.
fn value_shape(image: &VerifiedImage, ty: ImageType) -> Option<ValueShape> {
    /// One step of the explicit shape walk.
    enum ShapeStep {
        /// Emit the shape of this image type.
        Ty(ImageType),
        /// Open the next variant of the sum at the top of the builder's stack.
        Variant { enum_idx: u16, variant: usize },
        /// Close the composite or variant whose members have been emitted.
        Close,
    }

    let mut builder = ValueShapeBuilder::new();
    let mut pending = vec![ShapeStep::Ty(ty)];
    while let Some(step) = pending.pop() {
        match step {
            ShapeStep::Ty(ImageType::Scalar { scalar, .. }) => {
                builder.scalar(scalar_kind(scalar));
            }
            ShapeStep::Ty(ImageType::Record { idx, .. }) => {
                builder.open_product(idx.wire_index());
                pending.push(ShapeStep::Close);
                pending.extend(
                    image
                        .record_type(idx)
                        .fields()
                        .iter()
                        .rev()
                        .map(|field| ShapeStep::Ty(field.ty())),
                );
            }
            ShapeStep::Ty(ImageType::Enum { idx, .. }) => {
                let sealed = image.enums().get(idx.index() as usize)?;
                builder.open_sum(idx.wire_index());
                pending.push(ShapeStep::Close);
                pending.extend((0..sealed.variants().len()).rev().map(|variant| {
                    ShapeStep::Variant {
                        enum_idx: idx.wire_index(),
                        variant,
                    }
                }));
            }
            // An entry identity is not an inline durable field value on this line, so it
            // parks like a collection or unit.
            ShapeStep::Ty(
                ImageType::Unit | ImageType::Collection { .. } | ImageType::Identity { .. },
            ) => return None,
            ShapeStep::Variant { enum_idx, variant } => {
                let sealed = image.enums().get(enum_idx as usize)?;
                let payload = sealed.variants().get(variant)?.payload();
                builder.open_variant();
                pending.push(ShapeStep::Close);
                pending.extend(payload.iter().rev().map(|leaf| ShapeStep::Ty(*leaf)));
            }
            ShapeStep::Close => {
                builder.close();
            }
        }
        // A latched refusal ends the walk: the remaining commands cannot change the verdict,
        // and stopping keeps a divergent type graph from driving the walk's own stack.
        builder.refusal().map_or(Some(()), |_| None)?;
    }
    builder.finish().ok()
}

/// Map an image scalar type to the runtime codec's scalar kind. Total over the
/// closed scalar domain the value/key codecs already support.
fn scalar_kind(scalar: Scalar) -> ScalarKind {
    match scalar {
        Scalar::Int => ScalarKind::Int,
        Scalar::Bool => ScalarKind::Bool,
        Scalar::Text => ScalarKind::Str,
        Scalar::Bytes => ScalarKind::Bytes,
        Scalar::Date => ScalarKind::Date,
        Scalar::Instant => ScalarKind::Instant,
        Scalar::Duration => ScalarKind::Duration,
    }
}
