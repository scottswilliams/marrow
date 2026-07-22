//! The compiler-owned durable-path naming join and the per-export demand sentence.
//!
//! A verifier-reconstructed [`ExportDemand`] names each durable node it touches by a
//! [`SemanticPath`] — the stable chain of kind-tagged ledger ids from the application
//! down — and never by a source name (the image carries no demand and the verifier
//! learns no spelling). [`DurableNaming`] is the compiler's join from those ledger ids
//! back to the program's own `^root.member` spelling: the durable registry records one
//! entry per graph node as it resolves the node's identity, so a demand set can be
//! *described* in source spelling without the verifier owning any name.
//!
//! The description never grants: [`DurableNaming::demand_sentence`] renders which
//! durable places an export reads and writes, exactly the access the compiler already
//! reconstructed. Whether an invocation may exercise that demand is a separate authority
//! concern this owner does not touch.

use std::collections::{BTreeMap, BTreeSet};

use marrow_image::{ExportDemand, LedgerIdBytes, SemanticPath, SemanticStepKind};

/// Whether a named durable node opens a durable path (a store root, spelled `^name`) or
/// extends one (a field, index, group, or keyed branch, spelled `.name`). A typed state
/// rather than a bare flag: the sigil is the node's rendered prefix, fixed at the point
/// its identity is resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathSigil {
    /// A top-level store root: rendered `^name`.
    Root,
    /// A member below a root — a stored field, managed index, static group, or keyed
    /// branch: rendered `.name`.
    Child,
}

/// The compiler-owned join from a durable node's stable ledger id to its source
/// spelling.
///
/// The join is keyed by ledger id, so it survives every representation the same node
/// wears elsewhere (an operation site, a verifier node, a physical key). A demand atom's
/// [`SemanticPath`] is a chain of those ledger ids; [`Self::spell`] walks the chain and
/// renders it in the program's own `^root.member` spelling. Two nodes at different ledger
/// ids never collide, so the map is exact.
#[derive(Debug, Clone, Default)]
pub struct DurableNaming {
    by_id: BTreeMap<LedgerIdBytes, (PathSigil, Box<str>)>,
}

impl DurableNaming {
    /// Build the join from the durable registry's collected `(id, sigil, name)` entries.
    /// The registry commits entries only for an admitted durable graph, so every id here
    /// belongs to a node whose identity resolved completely.
    pub(crate) fn from_entries(entries: Vec<(LedgerIdBytes, PathSigil, String)>) -> Self {
        Self {
            by_id: entries
                .into_iter()
                .map(|(id, sigil, name)| (id, (sigil, name.into_boxed_str())))
                .collect(),
        }
    }

    /// Render one durable node's [`SemanticPath`] in source spelling, or `None` if any
    /// step names a node this join does not know. The application step carries no
    /// spelling (it is the shared root of every path); each remaining step contributes
    /// its sigil and name, so `[application, root, field]` renders `^root.field`.
    fn spell(&self, path: &SemanticPath) -> Option<String> {
        let mut out = String::new();
        for step in path.steps() {
            if step.kind == SemanticStepKind::Application {
                continue;
            }
            let (sigil, name) = self.by_id.get(&step.id)?;
            match sigil {
                PathSigil::Root => out.push('^'),
                PathSigil::Child => out.push('.'),
            }
            out.push_str(name);
        }
        (!out.is_empty()).then_some(out)
    }

    /// The per-export demand sentence: which durable places the export reads and which it
    /// writes, each named by its durable path in source spelling.
    ///
    /// Access is grouped by read/write coverage — a presence probe, a field or entry
    /// read, and an ordered index or family traversal are all *reads*; a write and an
    /// erase are *writes* — the same projection the store ceiling checks, and the same
    /// `read`/`write` coverage a durable place is described by. A place a read-modify-
    /// write export both reads and writes appears in both clauses. Paths are ordered by
    /// their spelling and de-duplicated, so the sentence is a stable function of the
    /// demand set. Returns `None` only if a demanded node is unspellable, which cannot
    /// happen for a demand reconstructed from an admitted graph.
    pub fn demand_sentence(&self, demand: &ExportDemand) -> Option<String> {
        if demand.is_empty() {
            return Some("reads or writes no durable data".to_string());
        }
        let mut reads: Vec<String> = Vec::new();
        let mut writes: Vec<String> = Vec::new();
        for atom in demand.atoms() {
            let spelled = self.spell(atom.path())?;
            if atom.class().mutates() {
                writes.push(spelled);
            } else {
                reads.push(spelled);
            }
        }
        let mut clauses: Vec<String> = Vec::new();
        if let Some(list) = joined(reads) {
            clauses.push(format!("reads {list}"));
        }
        if let Some(list) = joined(writes) {
            clauses.push(format!("writes {list}"));
        }
        Some(clauses.join("; "))
    }

    /// The per-export demand projected to durable **roots**, split by the same
    /// read/write coverage [`Self::demand_sentence`] uses, for a summary that names each
    /// touched root and how many distinct child places under it the export touches
    /// rather than listing every child atom. Derived from the same demand atoms and the
    /// same coverage classification as the sentence, so the two never disagree. Returns
    /// `None` only if a demanded node is unspellable, which cannot happen for a demand
    /// reconstructed from an admitted graph.
    pub fn demand_summary(&self, demand: &ExportDemand) -> Option<DemandSummary> {
        Some(DemandSummary {
            reads: self.roll_up(demand, false)?,
            writes: self.roll_up(demand, true)?,
        })
    }

    /// Collect one coverage's atoms into per-root [`RootDemand`]s: the roots this export
    /// reads (or writes) in spelling order, each carrying the count of distinct child
    /// places it touches under that root with that coverage. A root touched only as a
    /// whole entry carries a zero field count.
    fn roll_up(&self, demand: &ExportDemand, mutating: bool) -> Option<Vec<RootDemand>> {
        // Root spelling -> the distinct child extensions touched under it. The child key
        // is the atom's spelled remainder below the root (e.g. `.revision`), so two atoms
        // of the same field under different operation classes count once, matching the
        // sentence's own de-duplication.
        let mut roots: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for atom in demand.atoms() {
            if atom.class().mutates() != mutating {
                continue;
            }
            let (root, child) = self.split_root(atom.path())?;
            let children = roots.entry(root).or_default();
            if let Some(child) = child {
                children.insert(child);
            }
        }
        Some(
            roots
                .into_iter()
                .map(|(root, children)| RootDemand {
                    root,
                    field_count: children.len(),
                })
                .collect(),
        )
    }

    /// Split one durable node's [`SemanticPath`] into its root spelling (`^name`) and the
    /// spelled remainder below the root (`.a.b`, or `None` for a whole-entry atom).
    /// Reuses the same ledger-id join and sigils as [`Self::spell`], so a summary and a
    /// sentence spell the same node identically. `None` if any step is unknown to the
    /// join, mirroring [`Self::spell`].
    fn split_root(&self, path: &SemanticPath) -> Option<(String, Option<String>)> {
        let mut root: Option<String> = None;
        let mut child = String::new();
        for step in path.steps() {
            if step.kind == SemanticStepKind::Application {
                continue;
            }
            let (sigil, name) = self.by_id.get(&step.id)?;
            match sigil {
                PathSigil::Root => root = Some(format!("^{name}")),
                PathSigil::Child => {
                    child.push('.');
                    child.push_str(name);
                }
            }
        }
        Some((root?, (!child.is_empty()).then_some(child)))
    }
}

/// One durable root in an export's demand summary: the root in source spelling and how
/// many distinct child places under it the export touches with one coverage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootDemand {
    /// The store root in source spelling, e.g. `^patients`.
    pub root: String,
    /// Distinct demanded child places under this root with this coverage — stored
    /// fields, managed indexes, static groups, or keyed branches. Zero when the export
    /// touches only the root's whole entry.
    pub field_count: usize,
}

/// An export's durable demand projected to roots, split by read/write coverage. The same
/// projection [`DurableNaming::demand_sentence`] renders as prose, exposed as typed facts
/// so a renderer can group and summarize without re-deriving spelling or coverage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemandSummary {
    /// The roots this export reads, in spelling order.
    pub reads: Vec<RootDemand>,
    /// The roots this export writes, in spelling order.
    pub writes: Vec<RootDemand>,
}

/// Sort, de-duplicate, and join one clause's paths in the steady reference register:
/// `A`, `A and B`, or `A, B, and C`. `None` for an empty clause, so a clause with no
/// paths is dropped rather than rendered empty.
fn joined(mut paths: Vec<String>) -> Option<String> {
    paths.sort();
    paths.dedup();
    match paths.as_slice() {
        [] => None,
        [only] => Some(only.clone()),
        [first, second] => Some(format!("{first} and {second}")),
        [rest @ .., last] => Some(format!("{}, and {last}", rest.join(", "))),
    }
}

#[cfg(test)]
mod tests {
    use super::{DurableNaming, PathSigil, RootDemand};
    use marrow_image::{
        DemandAtom, ExportDemand, LedgerIdBytes, OperationClass, SemanticPath, SemanticStep,
        SemanticStepKind,
    };

    fn id(byte: u8) -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([byte; 16])
    }

    const APP: u8 = 0x0a;
    const ROOT: u8 = 0x0b;
    const TITLE: u8 = 0x0e;
    const SHELF: u8 = 0x1e;
    const INDEX: u8 = 0x4b;

    /// A `^books` root with a `title`/`shelf` field and a `byIsbn` index, mirroring the
    /// spellings the durable registry records for such a graph.
    fn naming() -> DurableNaming {
        DurableNaming::from_entries(vec![
            (id(ROOT), PathSigil::Root, "books".to_string()),
            (id(TITLE), PathSigil::Child, "title".to_string()),
            (id(SHELF), PathSigil::Child, "shelf".to_string()),
            (id(INDEX), PathSigil::Child, "byIsbn".to_string()),
        ])
    }

    /// The whole-entry path `[application, root]`.
    fn root_path() -> SemanticPath {
        SemanticPath::from_steps(vec![
            SemanticStep::new(SemanticStepKind::Application, id(APP)),
            SemanticStep::new(SemanticStepKind::Placement, id(ROOT)),
        ])
    }

    /// A field-leaf path `[application, root, field]`.
    fn field_path(field: u8) -> SemanticPath {
        SemanticPath::from_steps(vec![
            SemanticStep::new(SemanticStepKind::Application, id(APP)),
            SemanticStep::new(SemanticStepKind::Placement, id(ROOT)),
            SemanticStep::new(SemanticStepKind::Field, id(field)),
        ])
    }

    /// An index path `[application, root, index]`.
    fn index_path() -> SemanticPath {
        SemanticPath::from_steps(vec![
            SemanticStep::new(SemanticStepKind::Application, id(APP)),
            SemanticStep::new(SemanticStepKind::Placement, id(ROOT)),
            SemanticStep::new(SemanticStepKind::Index, id(INDEX)),
        ])
    }

    fn sentence(atoms: Vec<DemandAtom>) -> String {
        naming()
            .demand_sentence(&ExportDemand::from_atoms(atoms))
            .expect("every demanded node is nameable")
    }

    #[test]
    fn a_read_only_export_reads_the_entry_and_the_index() {
        assert_eq!(
            sentence(vec![
                DemandAtom::new(root_path(), OperationClass::Read),
                DemandAtom::new(index_path(), OperationClass::IndexRead),
            ]),
            "reads ^books and ^books.byIsbn",
        );
    }

    #[test]
    fn a_writer_writes_the_entry() {
        assert_eq!(
            sentence(vec![DemandAtom::new(root_path(), OperationClass::Write)]),
            "writes ^books",
        );
    }

    #[test]
    fn a_read_modify_write_names_the_place_in_both_clauses() {
        assert_eq!(
            sentence(vec![
                DemandAtom::new(field_path(TITLE), OperationClass::Read),
                DemandAtom::new(field_path(TITLE), OperationClass::Write),
            ]),
            "reads ^books.title; writes ^books.title",
        );
    }

    #[test]
    fn presence_and_a_family_traversal_read_and_an_erase_writes() {
        // Presence and an index/family traversal are non-mutating reads; an erase is a
        // mutating write. The coverage projection, not the finer class, drives the clause.
        assert_eq!(
            sentence(vec![DemandAtom::new(root_path(), OperationClass::Presence)]),
            "reads ^books",
        );
        assert_eq!(
            sentence(vec![DemandAtom::new(
                field_path(TITLE),
                OperationClass::Erase
            )]),
            "writes ^books.title",
        );
    }

    #[test]
    fn three_or_more_paths_join_with_an_oxford_list() {
        assert_eq!(
            sentence(vec![
                DemandAtom::new(root_path(), OperationClass::Read),
                DemandAtom::new(field_path(TITLE), OperationClass::Read),
                DemandAtom::new(field_path(SHELF), OperationClass::Read),
            ]),
            "reads ^books, ^books.shelf, and ^books.title",
        );
    }

    #[test]
    fn an_empty_demand_reads_or_writes_no_durable_data() {
        assert_eq!(
            naming()
                .demand_sentence(&ExportDemand::from_atoms([]))
                .expect("the empty demand is nameable"),
            "reads or writes no durable data",
        );
    }

    #[test]
    fn demand_summary_rolls_child_reads_up_to_their_root_with_a_field_count() {
        // A whole-entry read plus two field reads under the same root roll up to one
        // read root carrying a field count of two; the index read under `books` also
        // counts as a child. The projection uses the same coverage split as the sentence.
        let summary = naming()
            .demand_summary(&ExportDemand::from_atoms(vec![
                DemandAtom::new(root_path(), OperationClass::Read),
                DemandAtom::new(field_path(TITLE), OperationClass::Read),
                DemandAtom::new(field_path(SHELF), OperationClass::Read),
                DemandAtom::new(index_path(), OperationClass::IndexRead),
            ]))
            .expect("every demanded node is nameable");
        assert_eq!(
            summary.reads,
            vec![RootDemand {
                root: "^books".to_string(),
                field_count: 3,
            }],
        );
        assert!(summary.writes.is_empty());
    }

    #[test]
    fn demand_summary_splits_read_and_write_coverage_per_root() {
        // A read of one field and a write of another under the same root produce one read
        // root and one write root, each counting only its own coverage's children.
        let summary = naming()
            .demand_summary(&ExportDemand::from_atoms(vec![
                DemandAtom::new(field_path(TITLE), OperationClass::Read),
                DemandAtom::new(field_path(SHELF), OperationClass::Write),
            ]))
            .expect("every demanded node is nameable");
        assert_eq!(
            summary.reads,
            vec![RootDemand {
                root: "^books".to_string(),
                field_count: 1,
            }],
        );
        assert_eq!(
            summary.writes,
            vec![RootDemand {
                root: "^books".to_string(),
                field_count: 1,
            }],
        );
    }

    #[test]
    fn demand_summary_counts_a_whole_entry_root_with_no_fields_as_zero() {
        let summary = naming()
            .demand_summary(&ExportDemand::from_atoms(vec![DemandAtom::new(
                root_path(),
                OperationClass::Write,
            )]))
            .expect("every demanded node is nameable");
        assert_eq!(
            summary.writes,
            vec![RootDemand {
                root: "^books".to_string(),
                field_count: 0,
            }],
        );
    }

    #[test]
    fn demand_summary_de_duplicates_a_field_touched_under_two_classes() {
        // A presence probe and a read of the same field are both non-mutating reads of
        // one child; the field counts once, matching the sentence's de-duplication.
        let summary = naming()
            .demand_summary(&ExportDemand::from_atoms(vec![
                DemandAtom::new(field_path(TITLE), OperationClass::Presence),
                DemandAtom::new(field_path(TITLE), OperationClass::Read),
            ]))
            .expect("every demanded node is nameable");
        assert_eq!(
            summary.reads,
            vec![RootDemand {
                root: "^books".to_string(),
                field_count: 1,
            }],
        );
    }

    #[test]
    fn an_empty_demand_summarizes_to_no_roots() {
        let summary = naming()
            .demand_summary(&ExportDemand::from_atoms([]))
            .expect("the empty demand is nameable");
        assert!(summary.reads.is_empty());
        assert!(summary.writes.is_empty());
    }

    #[test]
    fn a_demand_summary_over_an_unknown_node_is_unnameable() {
        let unknown = SemanticPath::from_steps(vec![
            SemanticStep::new(SemanticStepKind::Application, id(APP)),
            SemanticStep::new(SemanticStepKind::Placement, id(0x77)),
        ]);
        let demand = ExportDemand::from_atoms([DemandAtom::new(unknown, OperationClass::Read)]);
        assert!(naming().demand_summary(&demand).is_none());
    }

    #[test]
    fn a_demand_over_an_unknown_node_is_unspellable() {
        // A node the join does not know cannot be rendered, so the whole sentence is
        // `None` rather than a partial or invented spelling. This never happens for a
        // demand reconstructed from an admitted graph.
        let unknown = SemanticPath::from_steps(vec![
            SemanticStep::new(SemanticStepKind::Application, id(APP)),
            SemanticStep::new(SemanticStepKind::Placement, id(0x77)),
        ]);
        let demand = ExportDemand::from_atoms([DemandAtom::new(unknown, OperationClass::Read)]);
        assert!(naming().demand_sentence(&demand).is_none());
    }
}
