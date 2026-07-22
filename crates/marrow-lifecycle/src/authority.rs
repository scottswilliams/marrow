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
    /// The operation the atom performs, in source vocabulary (`read`, `write`, `presence`,
    /// `erase`, `iterate`) — the new effect the ceiling does not admit.
    pub effect: &'static str,
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

/// The natural present-tense verb for one effect word in the refusal sentence — `write` reads
/// as "writes", a presence probe as "probes", an ordered traversal as "iterates".
fn effect_verb(effect: &str) -> &'static str {
    match effect {
        "read" => "reads",
        "write" => "writes",
        "presence" => "probes",
        "erase" => "erases",
        "iterate" => "iterates",
        _ => "accesses",
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
            effect: atom.class().word(),
            place: naming.spell(atom),
        })
        .collect();
    exceeding.sort_by(|a, b| {
        a.place
            .cmp(&b.place)
            .then_with(|| a.effect.cmp(b.effect))
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

impl Naming {
    /// Build the join by co-walking the image's sealed roots with its semantic nodes: each
    /// sealed node's source name is paired with the ledger id its semantic node carries, matched
    /// by declaration order and node kind. A count mismatch at any level degrades that level to
    /// unnamed rather than risking a misaligned name.
    fn from_image(image: &VerifiedImage) -> Self {
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

        let mut by_id: HashMap<LedgerIdBytes, (Sigil, String)> = HashMap::new();
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
                by_id.insert(
                    nodes[node_index].path.node_id(),
                    (Sigil::Root, sealed.name().to_string()),
                );
                walk_members(
                    image,
                    &nodes,
                    &children,
                    node_index,
                    Members::root(image, sealed),
                    &mut by_id,
                );
            }
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

/// Correlate a node's semantic children with its sealed members and record each member's id →
/// name, recursing into groups and branches. A count mismatch at a level degrades that level
/// (no name recorded), so a misaligned walk never invents a wrong name.
fn walk_members(
    image: &VerifiedImage,
    nodes: &[marrow_image::SemanticNode],
    children: &HashMap<Vec<SemanticStep>, Vec<usize>>,
    node_index: usize,
    members: Members,
    by_id: &mut HashMap<LedgerIdBytes, (Sigil, String)>,
) {
    let key = nodes[node_index].path.steps().to_vec();
    let kids = children.get(&key).cloned().unwrap_or_default();
    let kids_of = |kind: SemanticNodeKind| -> Vec<usize> {
        kids.iter().copied().filter(|&i| nodes[i].kind == kind).collect()
    };

    let field_nodes = kids_of(SemanticNodeKind::Field);
    if field_nodes.len() == members.fields.len() {
        for (&fi, name) in field_nodes.iter().zip(members.fields) {
            by_id.insert(nodes[fi].path.node_id(), (Sigil::Child, name));
        }
    }

    let group_nodes = kids_of(SemanticNodeKind::Group);
    if group_nodes.len() == members.groups.len() {
        for (&gi, (name, group_fields)) in group_nodes.iter().zip(members.groups) {
            by_id.insert(nodes[gi].path.node_id(), (Sigil::Child, name));
            let group_members = Members {
                fields: group_fields,
                groups: Vec::new(),
                branches: Vec::new(),
            };
            walk_members(image, nodes, children, gi, group_members, by_id);
        }
    }

    let branch_nodes = kids_of(SemanticNodeKind::Branch);
    if branch_nodes.len() == members.branches.len() {
        for (&bi, branch) in branch_nodes.iter().zip(members.branches) {
            by_id.insert(nodes[bi].path.node_id(), (Sigil::Child, branch.name));
            walk_members(image, nodes, children, bi, branch.members, by_id);
        }
    }
}
