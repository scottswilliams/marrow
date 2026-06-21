use marrow_syntax::SourceSpan;

use crate::evolution::leaf_type::accepted_leaf_kind_in_facts;
use crate::{
    CheckedFacts, CheckedProgram, CheckedRuntimeProgram, CheckedSavedMember, CheckedSavedPlace,
    StoreLeafKind, checked_saved_root_place,
};

pub(crate) trait DataProgram {
    fn facts(&self) -> &CheckedFacts;
    fn source_digest(&self) -> String;
    fn root_place(&self, root: &str) -> Option<CheckedSavedPlace>;
    fn accepted_leaf_kind(&self, catalog_id: &str) -> Option<StoreLeafKind>;
}

impl DataProgram for CheckedProgram {
    fn facts(&self) -> &CheckedFacts {
        &self.facts
    }

    fn source_digest(&self) -> String {
        self.source_digest()
    }

    fn root_place(&self, root: &str) -> Option<CheckedSavedPlace> {
        checked_saved_root_place(self, root, SourceSpan::default())
    }

    fn accepted_leaf_kind(&self, catalog_id: &str) -> Option<StoreLeafKind> {
        self.catalog
            .accepted_entries
            .iter()
            .find(|entry| entry.stable_id == catalog_id)
            .and_then(|entry| entry.accepted_leaf_token())
            .and_then(|token| accepted_leaf_kind_in_facts(&self.facts, token))
    }
}

impl DataProgram for CheckedRuntimeProgram {
    fn facts(&self) -> &CheckedFacts {
        self.facts()
    }

    fn source_digest(&self) -> String {
        self.source_digest().to_string()
    }

    fn root_place(&self, root: &str) -> Option<CheckedSavedPlace> {
        self.debug_data_places()
            .iter()
            .find(|place| place.root == root)
            .cloned()
    }

    fn accepted_leaf_kind(&self, catalog_id: &str) -> Option<StoreLeafKind> {
        let token = self.accepted_leaf_token(catalog_id)?;
        accepted_leaf_kind_in_facts(self.facts(), token)
    }
}

/// A copy of the checked root place whose leaf members are retyped to the catalog
/// the data was accepted under. Inspection renders a stored value by the epoch it
/// was written under, so a blocked populated-leaf retype shows the stored type
/// rather than an uncommitted proposal type.
pub(crate) fn inspection_root_place(
    program: &(impl DataProgram + ?Sized),
    root: &str,
) -> Option<CheckedSavedPlace> {
    let mut place = program.root_place(root)?;
    retype_members_to_accepted(program, &mut place.root_members);
    retype_members_to_accepted(program, &mut place.members);
    Some(place)
}

pub(crate) fn checked_places(program: &(impl DataProgram + ?Sized)) -> Vec<CheckedSavedPlace> {
    program
        .facts()
        .stores()
        .iter()
        .filter_map(|store| inspection_root_place(program, &store.root))
        .collect()
}

fn retype_members_to_accepted(
    program: &(impl DataProgram + ?Sized),
    members: &mut [CheckedSavedMember],
) {
    for member in members {
        if let (Some(catalog_id), Some(_)) = (member.catalog_id.as_deref(), &member.leaf)
            && let Some(accepted) = program.accepted_leaf_kind(catalog_id)
        {
            member.leaf = Some(accepted);
        }
        retype_members_to_accepted(program, &mut member.group_members);
    }
}
