use std::collections::{HashMap, HashSet};

use marrow_project::{CatalogEntry, CatalogEntryKind};

use crate::StoreLeafKind;
use crate::program::CheckedProgram;

/// Selectable member identities of each current enum, keyed by the enum id a
/// [`StoreLeafKind::Enum`] leaf carries. A stored value is valid only when its decoded
/// member is still selectable here, so a value naming a member the enum removed, marked
/// `category`, or gave children since the write fails closed.
pub(super) struct EnumMembers {
    by_enum: HashMap<crate::facts::EnumId, HashSet<String>>,
}

impl EnumMembers {
    pub(super) fn collect(program: &CheckedProgram) -> Self {
        let mut by_enum: HashMap<crate::facts::EnumId, HashSet<String>> = HashMap::new();
        for member in program.facts.enum_members() {
            let Some(catalog_id) = member.catalog_id.as_ref() else {
                continue;
            };
            if !program.facts.enum_member_is_selectable(member.id) {
                continue;
            }
            by_enum
                .entry(member.enum_id)
                .or_default()
                .insert(catalog_id.clone());
        }
        Self { by_enum }
    }

    /// Whether `member_id` is a current member of the enum. An enum with no recorded members
    /// (unbound first-run) admits any value: there is no accepted snapshot to contradict it.
    fn contains(&self, enum_id: crate::facts::EnumId, member_id: &str) -> bool {
        match self.by_enum.get(&enum_id) {
            Some(members) => members.contains(member_id),
            None => true,
        }
    }

    fn selectable(&self, enum_id: crate::facts::EnumId) -> Option<&HashSet<String>> {
        self.by_enum.get(&enum_id)
    }
}

/// Enum ids whose selectable-member set shrank since acceptance. Such an enum keeps its
/// stable identity, so the leaf token is unchanged and the change is not a retype; but a
/// stored value may name the now-gone member, so optional leaves referencing it must still
/// be scanned for validity. Required enum leaves are always scanned, so this drives only the
/// optional case.
pub(super) struct ShrunkEnums {
    pub(super) enums: HashSet<crate::facts::EnumId>,
}

impl ShrunkEnums {
    pub(super) fn collect(program: &CheckedProgram, current: &EnumMembers) -> Self {
        let enum_id_by_catalog: HashMap<&str, crate::facts::EnumId> = program
            .facts
            .enums()
            .iter()
            .filter_map(|enum_fact| {
                enum_fact
                    .catalog_id
                    .as_deref()
                    .map(|catalog_id| (catalog_id, enum_fact.id))
            })
            .collect();
        let mut enums = HashSet::new();
        for (enum_catalog_id, accepted_ids) in accepted_selectable_enum_members(program) {
            let Some(&enum_id) = enum_id_by_catalog.get(enum_catalog_id.as_str()) else {
                continue;
            };
            let empty = HashSet::new();
            let current_ids = current.selectable(enum_id).unwrap_or(&empty);
            if accepted_ids.iter().any(|id| !current_ids.contains(id)) {
                enums.insert(enum_id);
            }
        }
        Self { enums }
    }

    pub(super) fn shrank(&self, enum_id: crate::facts::EnumId) -> bool {
        self.enums.contains(&enum_id)
    }
}

/// Selectable members of each accepted enum, keyed by its stable catalog id. The accepted
/// catalog records the member tree only as paths, so selectability is read structurally: a
/// member is selectable iff no other member's path extends it.
fn accepted_selectable_enum_members(program: &CheckedProgram) -> HashMap<String, HashSet<String>> {
    let enum_paths: Vec<(&str, &str)> = program
        .catalog
        .accepted_entries
        .iter()
        .filter(|entry| entry.kind == CatalogEntryKind::Enum)
        .map(|entry| (entry.path.as_str(), entry.stable_id.as_str()))
        .collect();
    let members: Vec<&CatalogEntry> = program
        .catalog
        .accepted_entries
        .iter()
        .filter(|entry| entry.kind == CatalogEntryKind::EnumMember)
        .collect();
    let mut by_enum: HashMap<String, HashSet<String>> = HashMap::new();
    for member in &members {
        let Some((_, enum_catalog_id)) = enum_paths
            .iter()
            .find(|(enum_path, _)| is_member_path_of(&member.path, enum_path))
        else {
            continue;
        };
        if accepted_member_is_selectable(member, &members) {
            by_enum
                .entry((*enum_catalog_id).to_string())
                .or_default()
                .insert(member.stable_id.clone());
        }
    }
    by_enum
}

/// Whether an accepted member is a leaf of the member-path tree — no other member's path
/// extends it. This mirrors the source rule that a member is a category iff it has children,
/// and is the one home for the accepted-side selectability derivation.
fn accepted_member_is_selectable(member: &CatalogEntry, members: &[&CatalogEntry]) -> bool {
    !members
        .iter()
        .any(|other| !std::ptr::eq(*other, member) && is_member_path_of(&other.path, &member.path))
}

/// Whether `path` starts with `ancestor::` and adds at least one segment.
fn is_member_path_of(path: &str, ancestor: &str) -> bool {
    path.strip_prefix(ancestor)
        .and_then(|tail| tail.strip_prefix("::"))
        .is_some_and(|rest| !rest.is_empty())
}

/// Whether stored bytes are a valid value of a leaf's current type. The enum arm closes the
/// redefinition hole: bytes that structurally decode but name a member the current enum no
/// longer has are not a valid value, so they fail closed rather than decode silently.
pub(super) fn leaf_value_valid(
    leaf: &StoreLeafKind,
    bytes: &[u8],
    enum_members: &EnumMembers,
) -> bool {
    match leaf {
        StoreLeafKind::Scalar(scalar) => {
            marrow_store::value::decode_value(bytes, *scalar).is_some()
        }
        StoreLeafKind::Enum { enum_id } => marrow_store::tree::decode_tree_enum_member(bytes)
            .is_ok_and(|member| enum_members.contains(*enum_id, member.member_id().as_str())),
        StoreLeafKind::Identity { arity, .. } => {
            marrow_store::key::decode_identity_payload_arity(bytes, *arity).is_some()
        }
    }
}
