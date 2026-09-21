//! The compiler-owned durable-path naming join and the per-export demand sentence.
//!
//! A verifier-reconstructed [`DemandView`] names each durable node by a [`SemanticPath`]
//! — the stable chain of kind-tagged ledger ids from the application down — never by a
//! source name, since the image carries no demand and the verifier learns no spelling.
//! [`DurableNaming`] is the compiler's join from those ledger ids back to the program's
//! own `^root.member` spelling, so a demand set can be *described* in source spelling
//! without the verifier owning any name.
//!
//! The description never grants: rendering which durable places an export reads and
//! writes states access the compiler already reconstructed. Whether an invocation may
//! exercise that demand is a separate authority concern this owner does not touch.

use std::collections::{BTreeMap, BTreeSet};

use marrow_image::{DemandView, LedgerIdBytes, SemanticPath, SemanticStepKind};

/// Whether a named durable node opens a durable path (a store root, spelled `^name`) or
/// extends one (a field, index, group, or keyed branch, spelled `.name`). The sigil is
/// the node's rendered prefix, fixed when its identity is resolved.
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
/// wears elsewhere (an operation site, a verifier node, a physical key). Two nodes at
/// different ledger ids never collide, so the map is exact.
#[derive(Debug, Clone, Default)]
pub struct DurableNaming {
    by_id: BTreeMap<LedgerIdBytes, (PathSigil, Box<str>)>,
}

impl DurableNaming {
    /// Build the join from the durable registry's collected `(id, sigil, name)` entries.
    /// The registry commits entries only for an admitted durable graph, so every id
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
    /// spelling, so `[application, root, field]` renders `^root.field`.
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
    /// writes, each named by its durable path in source spelling, as prose:
    /// `reads ^a and ^a.b; writes ^a.b`. The places are those of
    /// [`Self::demand_places`], so the sentence and the place lists never disagree.
    pub fn demand_sentence(&self, demand: DemandView<'_>) -> Option<String> {
        if demand.is_empty() {
            return Some("reads or writes no durable data".to_string());
        }
        let places = self.demand_places(demand)?;
        let mut clauses: Vec<String> = Vec::new();
        if let Some(list) = joined(&places.reads) {
            clauses.push(format!("reads {list}"));
        }
        if let Some(list) = joined(&places.writes) {
            clauses.push(format!("writes {list}"));
        }
        Some(clauses.join("; "))
    }

    /// The per-export demand as exact places in source spelling, split by coverage.
    ///
    /// Access is grouped by read/write coverage — a presence probe, a field or entry
    /// read, and an ordered index or family traversal are all *reads*; a write and an
    /// erase are *writes* — the same projection the store ceiling checks. A place a
    /// read-modify-write export both reads and writes appears in both lists. Each list is
    /// ordered by spelling with each place once, so the result is a stable function of the
    /// demand set. `None` only if a demanded node is unspellable, which cannot happen for
    /// a demand reconstructed from an admitted graph.
    pub fn demand_places(&self, demand: DemandView<'_>) -> Option<DemandPlaces> {
        let mut reads = BTreeSet::new();
        let mut writes = BTreeSet::new();
        for atom in demand.atoms() {
            let spelled = self.spell(atom.path())?;
            if atom.class().mutates() {
                writes.insert(spelled);
            } else {
                reads.insert(spelled);
            }
        }
        Some(DemandPlaces {
            reads: reads.into_iter().collect(),
            writes: writes.into_iter().collect(),
        })
    }
}

/// An export's durable demand as exact places, split by read/write coverage: the same
/// facts [`DurableNaming::demand_sentence`] renders as prose, exposed as typed lists so
/// a renderer never re-derives spelling or coverage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemandPlaces {
    /// The places this export reads, in spelling order, each once.
    pub reads: Vec<String>,
    /// The places this export writes, in spelling order, each once.
    pub writes: Vec<String>,
}

/// Join one clause's places in the steady reference register: `A`, `A and B`, or
/// `A, B, and C`. `None` for an empty clause, so a clause with no places is dropped
/// rather than rendered empty.
fn joined(paths: &[String]) -> Option<String> {
    match paths {
        [] => None,
        [only] => Some(only.clone()),
        [first, second] => Some(format!("{first} and {second}")),
        [rest @ .., last] => Some(format!("{}, and {last}", rest.join(", "))),
    }
}

#[cfg(test)]
mod tests {
    use super::{DemandPlaces, DurableNaming, PathSigil};
    use marrow_image::{
        DemandAtom, ExportDemand, OperationClass, SemanticPath, SemanticStep, SemanticStepKind,
    };

    use marrow_test_support::id;

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
        SemanticPath::root(id(APP), id(ROOT))
    }

    /// A field-leaf path `[application, root, field]`.
    fn field_path(field: u8) -> SemanticPath {
        root_path()
            .child(SemanticStep::new(SemanticStepKind::Field, id(field)))
            .expect("a three-step chain is in bounds")
    }

    /// An index path `[application, root, index]`.
    fn index_path() -> SemanticPath {
        root_path()
            .child(SemanticStep::new(SemanticStepKind::Index, id(INDEX)))
            .expect("a three-step chain is in bounds")
    }

    fn sentence(atoms: Vec<DemandAtom>) -> String {
        naming()
            .demand_sentence(ExportDemand::from_atoms(atoms).as_view())
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
        // The coverage projection, not the finer operation class, drives the clause.
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
                .demand_sentence(ExportDemand::from_atoms([]).as_view())
                .expect("the empty demand is nameable"),
            "reads or writes no durable data",
        );
    }

    #[test]
    fn demand_places_split_coverage_and_name_every_place_once() {
        // The same field read and written appears in both lists; a whole-entry read, a
        // field, and an index are each one place; a repeated atom is listed once.
        let places = naming()
            .demand_places(
                ExportDemand::from_atoms(vec![
                    DemandAtom::new(root_path(), OperationClass::Read),
                    DemandAtom::new(field_path(TITLE), OperationClass::Read),
                    DemandAtom::new(field_path(TITLE), OperationClass::Read),
                    DemandAtom::new(index_path(), OperationClass::IndexRead),
                    DemandAtom::new(field_path(TITLE), OperationClass::Write),
                    DemandAtom::new(field_path(SHELF), OperationClass::Erase),
                ])
                .as_view(),
            )
            .expect("every demanded node is nameable");
        assert_eq!(
            places,
            DemandPlaces {
                reads: vec![
                    "^books".to_string(),
                    "^books.byIsbn".to_string(),
                    "^books.title".to_string(),
                ],
                writes: vec!["^books.shelf".to_string(), "^books.title".to_string()],
            }
        );
    }

    #[test]
    fn a_demand_over_an_unknown_node_is_unspellable() {
        // The whole result is `None` rather than a partial or invented spelling, for
        // the place lists and the sentence alike.
        let unknown = SemanticPath::root(id(APP), id(0x77));
        let demand = ExportDemand::from_atoms([DemandAtom::new(unknown, OperationClass::Read)]);
        assert!(naming().demand_places(demand.as_view()).is_none());
        assert!(naming().demand_sentence(demand.as_view()).is_none());
    }

    #[test]
    fn naming_visits_only_selected_atoms() {
        let known = DemandAtom::new(root_path(), OperationClass::Read);
        let unknown = DemandAtom::new(SemanticPath::root(id(APP), id(0x77)), OperationClass::Read);
        let owner = ExportDemand::from_atoms([unknown, known.clone()]);
        let known_row = marrow_image::DemandSelection::from_ordinals(vec![0]);
        let unknown_row = marrow_image::DemandSelection::from_ordinals(vec![1]);
        let selected = owner.selected(&known_row).expect("known selection");
        let expected = ExportDemand::from_atoms([known]);
        assert_eq!(
            naming().demand_places(selected),
            naming().demand_places(expected.as_view())
        );
        let selected = owner
            .selected(&unknown_row)
            .expect("unknown selection is canonical");
        assert!(naming().demand_places(selected).is_none());
    }
}
