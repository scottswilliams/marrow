//! The atom-granular deployment-ceiling admission check (G03).
//!
//! A store records a single **accepted deployment ceiling** at provision — the separately
//! owned standing maximum authority it admits (`crate::image::accepted_ceiling`), persisted
//! in the head. At attach, before any engine call, the presented image's whole-program
//! demand is intersected with that ceiling: an image whose verified demand fits within the
//! accepted ceiling is admitted (even when its demand is *narrower* than a prior image's),
//! and an image whose demand exceeds it is **refused** — [`DemandExceedsCeiling`] — naming,
//! for each exceeding atom, the export that demands it, the new effect, and the durable
//! place, in the program's own source vocabulary, so the owner can consciously expand the
//! ceiling to admit exactly the new demand and nothing more.
//!
//! Demand never grants. This owner only checks: it computes `demand \ ceiling` over the
//! canonical atom set and refuses when it is nonempty. The refusal is the term-3 (D08)
//! effect-ceiling honesty guarantee — a broadened read-only export is refused until the
//! deployment authority covers it, rather than the write silently landing.
//!
//! Source vocabulary is a projection of published image facts. The exceeding atoms are the
//! *presented* image's own demand atoms, so the presented image spells them: this module
//! reconstructs a ledger-id → `^root.member` naming join from the verified image's sealed
//! roots, fields, groups, and branches (the same facts the schema derivation consumes),
//! degrading a step it cannot spell to an unnamed place rather than risking a wrong name.

use std::collections::HashMap;

use marrow_codes::Code;
use marrow_image::{
    CeilingDescriptor, DemandAtom, ExportId, LedgerIdBytes, OperationClass, SemanticNodeKind,
    SemanticPath, SemanticStep, SemanticStepKind,
};
use marrow_verify::{SealedBranch, SealedRoot, VerifiedImage};

/// One durable-access atom a presented image demands that the store's accepted ceiling does
/// not admit: which export demands it, the new effect, and the durable place it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExceedingDemand {
    /// The export whose reconstructed demand reaches this atom, by source name. When several
    /// exports demand the same exceeding atom, the alphabetically first names it (a stable,
    /// deterministic choice), so the refusal always points at a real caller.
    pub export: String,
    /// The operation class the atom performs — the new effect the ceiling does not admit,
    /// rendered in source vocabulary (`reads`, `writes`, `probes`, `erases`, `iterates`) at
    /// the display edge.
    pub effect: OperationClass,
    /// The durable place the atom names, spelled `^root.member`, or `None` when a step of its
    /// path cannot be spelled from the image (a defensive degrade — the export and effect
    /// still name the refused authority).
    pub place: Option<String>,
}

/// An attach refusal: the presented image's verified demand exceeds the store's accepted
/// deployment ceiling. A typed lifecycle refusal, never corruption — the store is intact, no
/// engine call occurred, and the prior program remains usable. The owner consciously expands
/// the accepted ceiling to admit exactly the named demand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemandExceedsCeiling {
    /// Every exceeding atom, in a stable order (by place spelling, then effect), so the
    /// rendered refusal is a deterministic function of the delta.
    pub exceeding: Vec<ExceedingDemand>,
}

impl DemandExceedsCeiling {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> &'static str {
        Code::StoreDemandExceedsCeiling.as_str()
    }
}

impl std::fmt::Display for DemandExceedsCeiling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the program image demands durable authority the store's accepted ceiling does not \
             admit, so it is refused before any store access and the store is intact: "
        )?;
        for (i, atom) in self.exceeding.iter().enumerate() {
            if i > 0 {
                write!(f, "; ")?;
            }
            let verb = effect_verb(atom.effect);
            match &atom.place {
                Some(place) => write!(f, "export `{}` {verb} {place}", atom.export)?,
                None => write!(f, "export `{}` {verb} a durable place", atom.export)?,
            }
        }
        write!(
            f,
            ". Consciously expand the store's accepted authority ceiling to admit this demand \
             before activating the new program against this store"
        )
    }
}

impl std::error::Error for DemandExceedsCeiling {}

/// The natural present-tense verb for one operation class in the refusal sentence — a write
/// reads as "writes", a presence probe as "probes", an ordered traversal as "iterates".
fn effect_verb(effect: OperationClass) -> &'static str {
    match effect {
        OperationClass::Read => "reads",
        OperationClass::Write => "writes",
        OperationClass::Presence => "probes",
        OperationClass::Erase => "erases",
        OperationClass::IndexRead => "iterates",
    }
}

/// Intersect the presented `image`'s whole-program demand with the store's `accepted`
/// deployment ceiling. `Ok(())` when every demanded atom is admitted (demand ⊆ ceiling);
/// otherwise [`DemandExceedsCeiling`] naming every atom the ceiling does not admit. No engine
/// call is made — this is a pure comparison over reconstructed demand and the persisted
/// ceiling. The `accepted` ceiling is the reconstruction of the head's persisted payload.
pub fn admit(
    image: &VerifiedImage,
    accepted: &CeilingDescriptor,
) -> Result<(), DemandExceedsCeiling> {
    let exceeding_atoms = image.demand_union().not_admitted_by(accepted.demand());
    if exceeding_atoms.is_empty() {
        return Ok(());
    }

    let naming = Naming::from_image(image);
    let by_atom = exports_by_atom(image);

    let mut exceeding: Vec<ExceedingDemand> = exceeding_atoms
        .iter()
        .map(|atom| ExceedingDemand {
            export: export_for(&by_atom, atom),
            effect: atom.class(),
            place: naming.spell(atom),
        })
        .collect();
    exceeding.sort_by(|a, b| {
        a.place
            .cmp(&b.place)
            .then_with(|| a.effect.cmp(&b.effect))
            .then_with(|| a.export.cmp(&b.export))
    });
    Err(DemandExceedsCeiling { exceeding })
}

/// The map from a demanded atom (its place and class) to the source names of the exports that
/// reach it, built from the verifier's demand incidence — the published fact of which export
/// touches which node with which class. Keyed by `(place, class)` so a match is exact.
fn exports_by_atom(image: &VerifiedImage) -> HashMap<(SemanticPath, OperationClass), Vec<String>> {
    let mut by_atom: HashMap<(SemanticPath, OperationClass), Vec<String>> = HashMap::new();
    for node in image.demand_incidence() {
        for incidence in &node.touched_by {
            by_atom
                .entry((node.path.clone(), incidence.class))
                .or_default()
                .push(export_name(image, incidence.export));
        }
    }
    by_atom
}

/// The source name of an export by its declaration identity, or a stable placeholder when the
/// image cannot resolve it (which does not happen for a demand reconstructed from the image's
/// own exports).
fn export_name(image: &VerifiedImage, id: ExportId) -> String {
    match image.export_by_id(id) {
        Some(export) => image.function(export.function()).name().to_string(),
        None => "an export".to_string(),
    }
}

/// The rendered prefix of a durable node: a store root opens a path (`^`), every member below
/// it extends one (`.`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sigil {
    Root,
    Child,
}

/// The compiler-free source-spelling join from a durable node's stable ledger id to its
/// `^root.member` spelling, reconstructed from the verified image's sealed structure. Every id
/// here belongs to a node whose name the image publishes; a step outside the join (for example
/// a managed-index step, whose name the image does not carry) makes the whole place unspellable
/// and the refusal degrades to naming the export and effect only.
struct Naming {
    by_id: HashMap<LedgerIdBytes, (Sigil, String)>,
}

/// One durable node of the verified image, named by its path-qualified source spelling: the
/// store root's name followed by each member name down to the node, beside the ledger id
/// and kind its semantic node carries. Produced only by [`named_durable_nodes`], the single
/// owner of the sealed↔semantic correspondence.
pub(crate) struct NamedDurableNode {
    pub(crate) ledger_id: LedgerIdBytes,
    /// The node's kind, which decides its physical layout (a group is part of its entry's
    /// payload; a branch is a keyed child node), so a consumer pairing store nodes with
    /// image nodes by name binds the kind too.
    pub(crate) kind: SemanticNodeKind,
    /// The node's index into [`VerifiedImage::semantic_nodes`] — its occurrence identity.
    /// A ledger id is the identity of a *declaration*, so two roots of one resource give
    /// their like-named members the same ledger id; only the whole kind-tagged semantic
    /// path distinguishes the occurrences, and this index stands for that path. A consumer
    /// that must be injective over occurrences (the head-map pin's coverage check) keys on
    /// this, never on the ledger id.
    pub(crate) semantic_index: usize,
    /// The node's name path: `["books"]` for a root, `["books", "notes", "body"]` for a
    /// member. Never empty.
    pub(crate) path: Vec<String>,
}

/// The sealed↔semantic correspondence, in declaration order: each sealed node's source name
/// path beside the ledger id its semantic node carries, matched per level by declaration
/// order and node kind. A count mismatch at a level omits that level's nodes rather than
/// risking a misaligned name; each consumer chooses its own posture toward an omission (the
/// authority refusal degrades that place to unnamed, the head-map pin refuses fail-closed).
pub(crate) fn named_durable_nodes(image: &VerifiedImage) -> Vec<NamedDurableNode> {
    let nodes = image.semantic_nodes();
    // Children of each container, keyed by the container's full step chain, in the order
    // semantic_nodes yields them (a node before its descendants, members in declaration
    // order) — the same structure image.rs::split_order relies on.
    let mut children: HashMap<Vec<SemanticStep>, Vec<usize>> = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        let steps = node.path.steps();
        if steps.len() >= 2 {
            children
                .entry(steps[..steps.len() - 1].to_vec())
                .or_default()
                .push(index);
        }
    }

    let mut named = Vec::new();
    let root_nodes: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.kind == SemanticNodeKind::Root)
        .map(|(index, _)| index)
        .collect();
    // Roots correlate with image.roots() by declaration order (both are declaration-ordered).
    if root_nodes.len() == image.roots().len() {
        for (root_index, &node_index) in root_nodes.iter().enumerate() {
            let sealed = &image.roots()[root_index];
            let mut path = vec![sealed.name().to_string()];
            named.push(NamedDurableNode {
                ledger_id: nodes[node_index].path.node_id(),
                kind: SemanticNodeKind::Root,
                semantic_index: node_index,
                path: path.clone(),
            });
            walk_members(
                nodes,
                &children,
                node_index,
                Members::root(image, sealed),
                &mut path,
                &mut named,
            );
        }
    }
    named
}

impl Naming {
    /// Build the join from the one sealed↔semantic correspondence: each named node's last
    /// path segment is its display name, and a one-segment path is a root.
    fn from_image(image: &VerifiedImage) -> Self {
        let mut by_id: HashMap<LedgerIdBytes, (Sigil, String)> = HashMap::new();
        for node in named_durable_nodes(image) {
            let sigil = if node.path.len() == 1 {
                Sigil::Root
            } else {
                Sigil::Child
            };
            let name = node
                .path
                .last()
                .cloned()
                .expect("a named durable node carries at least its root segment");
            by_id.insert(node.ledger_id, (sigil, name));
        }
        Self { by_id }
    }

    /// Spell one atom's path in source vocabulary, or `None` if any step is not in the join.
    /// The application step is the shared root of every path and carries no spelling.
    fn spell(&self, atom: &DemandAtom) -> Option<String> {
        let mut out = String::new();
        for step in atom.path().steps() {
            if step.kind == SemanticStepKind::Application {
                continue;
            }
            let (sigil, name) = self.by_id.get(&step.id)?;
            match sigil {
                Sigil::Root => out.push('^'),
                Sigil::Child => out.push('.'),
            }
            out.push_str(name);
        }
        (!out.is_empty()).then_some(out)
    }
}

/// The export that names an exceeding atom: the alphabetically first of the exports the
/// incidence records for it (a stable deterministic choice), or a placeholder when none is
/// recorded (which does not happen for a demand reconstructed from the image's own exports).
fn export_for(
    by_atom: &HashMap<(SemanticPath, OperationClass), Vec<String>>,
    atom: &DemandAtom,
) -> String {
    by_atom
        .get(&(atom.path().clone(), atom.class()))
        .and_then(|names| names.iter().min())
        .cloned()
        .unwrap_or_else(|| "an export".to_string())
}

/// The named members of a durable node in the sealed structure, split by kind so each is
/// correlated with the semantic children of the same kind.
struct Members {
    fields: Vec<String>,
    groups: Vec<(String, Vec<String>)>,
    branches: Vec<BranchMembers>,
}

/// A branch's members, recursively.
struct BranchMembers {
    name: String,
    members: Members,
}

impl Members {
    /// The members of a root: its leading value fields (the record minus its trailing group
    /// slots), its groups, and its branches.
    fn root(image: &VerifiedImage, root: &SealedRoot) -> Self {
        let group_count = root.groups().len();
        let record = image.record_type(root.record());
        let field_count = record.fields().len().saturating_sub(group_count);
        let fields = record.fields()[..field_count]
            .iter()
            .map(|field| field.name.to_string())
            .collect();
        let groups = root
            .groups()
            .iter()
            .map(|group| {
                let record = image.record_type(group.record());
                (
                    group.name().to_string(),
                    record.fields().iter().map(|f| f.name.to_string()).collect(),
                )
            })
            .collect();
        let branches = root
            .branches()
            .iter()
            .map(|branch| branch_members(image, branch))
            .collect();
        Self {
            fields,
            groups,
            branches,
        }
    }
}

/// One branch's members: its own record fields and, recursively, its sub-branches. A branch
/// carries no group (group-in-branch is not executable).
fn branch_members(image: &VerifiedImage, branch: &SealedBranch) -> BranchMembers {
    let record = image.record_type(branch.record());
    BranchMembers {
        name: branch.name().to_string(),
        members: Members {
            fields: record.fields().iter().map(|f| f.name.to_string()).collect(),
            groups: Vec::new(),
            branches: branch
                .branches()
                .iter()
                .map(|sub| branch_members(image, sub))
                .collect(),
        },
    }
}

/// Correlate a node's semantic children with its sealed members and emit each member's
/// path-qualified name beside its ledger id, recursing into groups and branches. A count
/// mismatch at a level degrades that level (no node emitted), so a misaligned walk never
/// invents a wrong name.
fn walk_members(
    nodes: &[marrow_image::SemanticNode],
    children: &HashMap<Vec<SemanticStep>, Vec<usize>>,
    node_index: usize,
    members: Members,
    path: &mut Vec<String>,
    named: &mut Vec<NamedDurableNode>,
) {
    let key = nodes[node_index].path.steps().to_vec();
    let kids = children.get(&key).cloned().unwrap_or_default();
    let kids_of = |kind: SemanticNodeKind| -> Vec<usize> {
        kids.iter()
            .copied()
            .filter(|&i| nodes[i].kind == kind)
            .collect()
    };
    let emit =
        |named: &mut Vec<NamedDurableNode>, path: &mut Vec<String>, index: usize, name: String| {
            path.push(name);
            named.push(NamedDurableNode {
                ledger_id: nodes[index].path.node_id(),
                kind: nodes[index].kind,
                semantic_index: index,
                path: path.clone(),
            });
        };

    let field_nodes = kids_of(SemanticNodeKind::Field);
    if field_nodes.len() == members.fields.len() {
        for (&fi, name) in field_nodes.iter().zip(members.fields) {
            emit(named, path, fi, name);
            path.pop();
        }
    }

    let group_nodes = kids_of(SemanticNodeKind::Group);
    if group_nodes.len() == members.groups.len() {
        for (&gi, (name, group_fields)) in group_nodes.iter().zip(members.groups) {
            emit(named, path, gi, name);
            let group_members = Members {
                fields: group_fields,
                groups: Vec::new(),
                branches: Vec::new(),
            };
            walk_members(nodes, children, gi, group_members, path, named);
            path.pop();
        }
    }

    let branch_nodes = kids_of(SemanticNodeKind::Branch);
    if branch_nodes.len() == members.branches.len() {
        for (&bi, branch) in branch_nodes.iter().zip(members.branches) {
            emit(named, path, bi, branch.name);
            walk_members(nodes, children, bi, branch.members, path, named);
            path.pop();
        }
    }
}
