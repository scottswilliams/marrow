//! Borrowed canonical demand subsets and their sparse ordinal selections.

use super::{DEMAND_SET_KIND, DemandAtom, DemandSetId, ExportDemand, atom_set_payload, frame_id};

/// A sorted, unique set of canonical atom ordinals. It has no function or pool
/// identity; only selecting it against a canonical owner establishes its meaning.
#[derive(Debug, Clone, Default)]
pub struct DemandSelection {
    ordinals: Vec<u32>,
}

impl DemandSelection {
    /// Normalize discovery order and duplicates once, retaining the input buffer.
    pub fn from_ordinals(mut ordinals: Vec<u32>) -> Self {
        ordinals.sort_unstable();
        ordinals.dedup();
        Self { ordinals }
    }

    /// Union another selection while preserving order and uniqueness. Scratch is
    /// reusable across unions and empty on return; its capacity may be retained.
    pub fn union_with(&mut self, other: &Self, scratch: &mut Vec<u32>) {
        scratch.clear();
        if other.ordinals.is_empty() {
            return;
        }
        let (mut left, mut right) = (0, 0);
        while left < self.ordinals.len() && right < other.ordinals.len() {
            let a = self.ordinals[left];
            let b = other.ordinals[right];
            scratch.push(a.min(b));
            if a <= b {
                left += 1;
            }
            if b <= a {
                right += 1;
            }
        }
        scratch.extend_from_slice(&self.ordinals[left..]);
        scratch.extend_from_slice(&other.ordinals[right..]);
        std::mem::swap(&mut self.ordinals, scratch);
        scratch.clear();
    }
}

#[derive(Debug, Clone, Copy)]
enum Selection<'a> {
    All,
    Subset(&'a DemandSelection),
}

/// A canonical demand borrowed from its atom owner. A view grants no authority
/// and cannot outlive or mutate either its owner or its selection.
#[derive(Debug, Clone, Copy)]
pub struct DemandView<'a> {
    owner: &'a ExportDemand,
    selection: Selection<'a>,
}

impl ExportDemand {
    pub fn as_view(&self) -> DemandView<'_> {
        DemandView {
            owner: self,
            selection: Selection::All,
        }
    }

    /// Borrow a subset whose greatest ordinal is in range. Selection construction
    /// already established order/uniqueness, so this check takes constant time.
    pub fn selected<'a>(&'a self, selection: &'a DemandSelection) -> Option<DemandView<'a>> {
        if let Some(&last) = selection.ordinals.last()
            && usize::try_from(last).ok()? >= self.atoms.len()
        {
            return None;
        }
        Some(DemandView {
            owner: self,
            selection: Selection::Subset(selection),
        })
    }
}

impl<'a> DemandView<'a> {
    /// Selected atoms in canonical order, without copying or visiting unselected atoms.
    pub fn atoms(self) -> impl ExactSizeIterator<Item = &'a DemandAtom> + Clone {
        match self.selection {
            Selection::All => Atoms::All(self.owner.atoms.iter()),
            Selection::Subset(selection) => Atoms::Subset {
                atoms: &self.owner.atoms,
                ordinals: selection.ordinals.iter(),
            },
        }
    }

    pub fn is_empty(self) -> bool {
        self.atoms().len() == 0
    }

    pub fn reads(self) -> bool {
        self.atoms().any(|atom| !atom.class().mutates())
    }

    pub fn writes(self) -> bool {
        self.atoms().any(|atom| atom.class().mutates())
    }

    pub fn atom_set_payload(self) -> Vec<u8> {
        atom_set_payload(self.atoms())
    }

    pub fn demand_set_id(self) -> DemandSetId {
        DemandSetId(frame_id(DEMAND_SET_KIND, &self.atom_set_payload()))
    }

    /// Materialize an independent owned demand. Its selected atoms are already
    /// canonical, so no sorting or duplicate elimination is necessary.
    pub fn to_owned(self) -> ExportDemand {
        ExportDemand {
            atoms: self.atoms().cloned().collect(),
        }
    }
}

impl PartialEq<DemandView<'_>> for DemandView<'_> {
    fn eq(&self, other: &DemandView<'_>) -> bool {
        self.atoms().eq(other.atoms())
    }
}

impl Eq for DemandView<'_> {}

#[derive(Clone)]
enum Atoms<'a> {
    All(std::slice::Iter<'a, DemandAtom>),
    Subset {
        atoms: &'a [DemandAtom],
        ordinals: std::slice::Iter<'a, u32>,
    },
}

impl<'a> Iterator for Atoms<'a> {
    type Item = &'a DemandAtom;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::All(atoms) => atoms.next(),
            Self::Subset { atoms, ordinals } => {
                let atoms = *atoms;
                ordinals.next().map(|&ordinal| {
                    let index = usize::try_from(ordinal).expect("selected ordinal fits owner");
                    &atoms[index]
                })
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.len();
        (len, Some(len))
    }
}

impl ExactSizeIterator for Atoms<'_> {
    fn len(&self) -> usize {
        match self {
            Self::All(atoms) => atoms.len(),
            Self::Subset { ordinals, .. } => ordinals.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CeilingDescriptor, LedgerIdBytes, OperationClass, SemanticPath};

    fn atom(placement: u8, class: OperationClass) -> DemandAtom {
        DemandAtom::new(
            SemanticPath::root(
                LedgerIdBytes::from_bytes([1; 16]),
                LedgerIdBytes::from_bytes([placement; 16]),
            ),
            class,
        )
    }

    #[test]
    fn selections_normalize_once_and_check_the_last_ordinal() {
        let owner = ExportDemand::from_atoms([
            atom(2, OperationClass::Read),
            atom(3, OperationClass::Read),
            atom(4, OperationClass::Read),
        ]);
        let full = DemandSelection::from_ordinals(vec![2, 0, 1, 2, 0]);
        assert_eq!(full.ordinals, [0, 1, 2]);
        let view = owner.selected(&full).expect("all ordinals fit");
        assert_eq!(view, owner.as_view());
        assert_eq!(view.atoms().len(), 3);
        assert!(!view.is_empty());
        let single = DemandSelection::from_ordinals(vec![1, 1]);
        assert_eq!(owner.selected(&single).expect("singleton").atoms().len(), 1);
        for invalid in [vec![3], vec![0, u32::MAX]] {
            assert!(
                owner
                    .selected(&DemandSelection::from_ordinals(invalid))
                    .is_none()
            );
        }
        let empty = DemandSelection::default();
        assert!(owner.selected(&empty).expect("empty subset").is_empty());
        let empty_owner = ExportDemand::from_atoms([]);
        assert_eq!(empty_owner.selected(&empty), Some(empty_owner.as_view()));
        assert!(empty_owner.selected(&single).is_none());
    }

    #[test]
    fn ordered_union_clears_dirty_scratch_and_reuses_it() {
        let mut left = DemandSelection::from_ordinals(vec![3, 1]);
        let mut scratch = vec![999];
        left.union_with(&DemandSelection::from_ordinals(vec![5, 3, 0]), &mut scratch);
        assert_eq!(left.ordinals, [0, 1, 3, 5]);
        assert!(scratch.is_empty());
        scratch.push(999);
        left.union_with(&DemandSelection::default(), &mut scratch);
        assert_eq!(left.ordinals, [0, 1, 3, 5]);
        assert!(scratch.is_empty());
        left.union_with(&DemandSelection::from_ordinals(vec![2, 4]), &mut scratch);
        assert_eq!(left.ordinals, [0, 1, 2, 3, 4, 5]);
        assert!(scratch.is_empty());
        let mut empty = DemandSelection::default();
        empty.union_with(&left, &mut scratch);
        assert_eq!(empty.ordinals, left.ordinals);
        assert!(scratch.is_empty());
    }

    #[test]
    fn canonicalization_maps_duplicate_tags_without_structural_order() {
        let read = atom(9, OperationClass::Read);
        let erase = atom(2, OperationClass::Erase);
        let mut mapped = std::collections::BTreeMap::new();
        let pool = ExportDemand::from_tagged_atoms(
            [(7, erase.clone()), (5, read.clone()), (9, erase.clone())],
            |tag, ordinal| {
                mapped.insert(tag, ordinal);
            },
        );
        assert_eq!(pool.atoms(), &[read, erase]);
        assert_eq!(
            mapped,
            std::collections::BTreeMap::from([(5, 0), (7, 1), (9, 1)])
        );
    }

    #[test]
    fn views_share_contents_and_payloads_across_different_pools() {
        let a = atom(4, OperationClass::Presence);
        let b = atom(8, OperationClass::Erase);
        let first = ExportDemand::from_atoms([a.clone(), b.clone()]);
        let second =
            ExportDemand::from_atoms([atom(2, OperationClass::Read), a.clone(), b.clone()]);
        let first_row = DemandSelection::from_ordinals(vec![0]);
        let second_row = DemandSelection::from_ordinals(vec![1]);
        let selected = first.selected(&first_row).expect("a in first pool");
        let other = second.selected(&second_row).expect("a in second pool");
        let owned = ExportDemand::from_atoms([a]);
        assert_eq!(selected, other);
        assert_eq!(selected, owned.as_view());
        assert_eq!(selected.to_owned(), owned);
        assert_eq!(selected.atom_set_payload(), owned.atom_set_payload());
        assert_eq!(selected.demand_set_id(), owned.demand_set_id());
        assert_eq!(selected.atoms().len(), 1);
        assert!(selected.reads());
        assert!(!selected.writes());
        assert_ne!(selected, first.as_view());
        assert!(first.as_view().writes());
        assert!(std::ptr::eq(
            selected.atoms().next().expect("a"),
            &first.atoms()[0]
        ));
        let ceiling = CeilingDescriptor::from_demand_union(selected.to_owned());
        assert_eq!(ceiling.atom_set_payload(), selected.atom_set_payload());
        let decoded =
            ExportDemand::decode_atom_set(&selected.atom_set_payload()).expect("canonical payload");
        assert_eq!(decoded.as_view(), selected);
        let empty = DemandSelection::default();
        assert_eq!(first.selected(&empty), second.selected(&empty));
        assert_eq!(
            first.selected(&empty).expect("empty").demand_set_id(),
            ExportDemand::from_atoms([]).demand_set_id()
        );
    }
}
