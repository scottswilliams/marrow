use std::collections::{HashMap, HashSet};

use marrow_catalog::{CatalogEntry, CatalogEntryKind};

use crate::{CheckedProgram, StoreLeafKind};

/// Selectable member identities of each current enum, keyed by the enum id a
/// [`StoreLeafKind::Enum`] leaf carries. A stored value is valid only when its decoded
/// member is still selectable here, so a value naming a member the enum removed, marked
/// `category`, or gave children since the write fails closed.
pub(super) struct EnumMembers {
    by_enum: HashMap<crate::facts::EnumId, HashSet<String>>,
    /// Enums whose current identity constrains a stored value: they carry an accepted snapshot
    /// and are not covered by an identity-preserving rename this cycle. A stored value naming a
    /// member absent from such an enum's current set is a known-orphaned identity, not an
    /// unconstrained first-run value or a value whose spelling a rename carries forward.
    constrained: HashSet<crate::facts::EnumId>,
}

impl EnumMembers {
    /// `rename_covered` names the enums an identity-preserving rename carries forward this cycle
    /// (an enum-type rename or one of its members renamed), whose stored values stay valid even
    /// though their member paths moved.
    pub(super) fn collect(
        program: &CheckedProgram,
        rename_covered: &HashSet<crate::facts::EnumId>,
    ) -> Self {
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
        let constrained = snapshotted_enum_ids(program)
            .difference(rename_covered)
            .copied()
            .collect();
        Self {
            by_enum,
            constrained,
        }
    }

    /// Whether `member_id` is a current selectable member of the enum. A re-parent under a fresh
    /// `category` keys the moved member's saved identity on its full ancestor path, so the proposal
    /// mints a new identity and the old one is never bound onto the facts: the enum's current set
    /// is missing it. For an enum whose identity is constrained — it has an accepted snapshot and
    /// no rename carries its members forward this cycle — a stored id absent from the current set
    /// is therefore an orphaned value and fails closed. An unconstrained enum (first-run, or one a
    /// rename re-addresses this cycle) admits the value, since no accepted state contradicts it.
    fn contains(&self, enum_id: crate::facts::EnumId, member_id: &str) -> bool {
        match self.by_enum.get(&enum_id) {
            Some(members) => members.contains(member_id),
            None => !self.constrained.contains(&enum_id),
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

/// Enum ids that carry an accepted snapshot, derived by mapping each accepted enum entry's
/// stable catalog id to the [`facts::EnumId`] the current facts bind it under. A stored value
/// whose member is absent from such an enum's current set is a known-orphaned identity, where
/// an enum with no accepted snapshot has nothing to contradict the value.
fn snapshotted_enum_ids(program: &CheckedProgram) -> HashSet<crate::facts::EnumId> {
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
    program
        .catalog
        .accepted_entries
        .iter()
        .filter(|entry| entry.kind == CatalogEntryKind::Enum)
        .filter_map(|entry| enum_id_by_catalog.get(entry.stable_id.as_str()).copied())
        .collect()
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
        if member_is_selectable(member, &members) {
            by_enum
                .entry((*enum_catalog_id).to_string())
                .or_default()
                .insert(member.stable_id.clone());
        }
    }
    by_enum
}

/// Whether an enum member is a leaf of the member-path tree — no other member's path
/// extends it. This mirrors the source rule that a member is a category iff it has children,
/// and is the one home for the enum-member selectability derivation: the shrink scan and the
/// scaffold-rename inference both read selectability through here.
pub(super) fn member_is_selectable(member: &CatalogEntry, members: &[&CatalogEntry]) -> bool {
    !members
        .iter()
        .any(|other| !std::ptr::eq(*other, member) && is_member_path_of(&other.path, &member.path))
}

/// Whether `path` starts with `ancestor::` and adds at least one segment.
pub(super) fn is_member_path_of(path: &str, ancestor: &str) -> bool {
    path.strip_prefix(ancestor)
        .and_then(|tail| tail.strip_prefix("::"))
        .is_some_and(|rest| !rest.is_empty())
}

/// Whether stored bytes are a valid value of a leaf's current type. The enum arm closes the
/// redefinition hole: bytes that structurally decode but name a member the current enum no
/// longer has are not a valid value, so they fail closed rather than decode silently.
pub(super) fn leaf_value_valid(
    program: &CheckedProgram,
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
        StoreLeafKind::Identity { store_root, arity } => {
            let Some(keys) = marrow_store::key::decode_identity_payload_arity(bytes, *arity) else {
                return false;
            };
            program
                .facts
                .store_by_root(store_root)
                .is_some_and(|store| store.identity_keys_match(&keys))
        }
    }
}
