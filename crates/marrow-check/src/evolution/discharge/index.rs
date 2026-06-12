use std::collections::{HashMap, HashSet};

use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, encode_identity_payload};
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::evolution::witness::{DefaultValue, RepairReason, Verdict};
use crate::executable::{CheckedSavedIndex, CheckedSavedPlace};
use crate::facts::{StoreIndexKeySource, StoredValueMeaning};

use super::{Accumulator, catalog_id};

/// One unique-index obligation to probe during the record scan: the index catalog id and how to
/// read each key column. The scan keys its collision state by `catalog_id`.
pub(super) struct UniqueIndexProbe {
    pub(super) catalog_id: CatalogId,
    columns: Vec<IndexKeyColumn>,
}

/// A place's unique-index plan: the indexes the scan can probe for collisions, and the ids of
/// any whose key shape it cannot. An unprobeable unique index fails closed rather than rebuild
/// unchecked, so a uniqueness guarantee is never published without verification.
pub(super) struct UniqueIndexPlan {
    pub(super) probes: Vec<UniqueIndexProbe>,
    pub(super) unprobeable: Vec<CatalogId>,
}

/// How to read one index key column: an identity key by its tuple position, or a top-level
/// member cell decoded by its meaning.
enum IndexKeyColumn {
    Identity {
        position: usize,
    },
    Member {
        path: DataPathSegment,
        meaning: StoredValueMeaning,
        default: Option<DefaultValue>,
    },
}

/// Build the unique-index plan: a unique index whose every column resolves becomes a probe; one
/// with any unresolvable column is recorded unprobeable and fails closed.
pub(super) fn unique_index_plan(
    place: &CheckedSavedPlace,
    acc: &Accumulator,
) -> Result<UniqueIndexPlan, StoreError> {
    let mut probes = Vec::new();
    let mut unprobeable = Vec::new();
    for index in &place.indexes {
        if !index.unique {
            continue;
        }
        let Some(index_catalog_id) = index.catalog_id.as_deref() else {
            continue;
        };
        let index_id = catalog_id(index_catalog_id)?;
        match index_key_columns(place, index, acc)? {
            Some(columns) => probes.push(UniqueIndexProbe {
                catalog_id: index_id,
                columns,
            }),
            None => unprobeable.push(index_id),
        }
    }
    Ok(UniqueIndexPlan {
        probes,
        unprobeable,
    })
}

/// The key-column readers for one index, or `None` when any column resolves to neither an
/// identity key position nor a top-level plain field. Every v0.1 index key resolves here; a
/// future index over a nested or keyed-layer column would resolve to `None` and fail closed.
fn index_key_columns(
    place: &CheckedSavedPlace,
    index: &CheckedSavedIndex,
    acc: &Accumulator,
) -> Result<Option<Vec<IndexKeyColumn>>, StoreError> {
    let mut columns = Vec::with_capacity(index.keys.len());
    for key in &index.keys {
        match key.source {
            StoreIndexKeySource::IdentityKey => {
                let Some(position) = place
                    .identity_keys
                    .iter()
                    .position(|identity_key| identity_key.name == key.name)
                else {
                    return Ok(None);
                };
                columns.push(IndexKeyColumn::Identity { position });
            }
            StoreIndexKeySource::ResourceMember(_) => {
                let Some(member) = place
                    .root_members
                    .iter()
                    .find(|member| member.name == key.name && member.is_plain_field())
                else {
                    return Ok(None);
                };
                let Some(member_catalog_id) = member.catalog_id.as_deref() else {
                    return Ok(None);
                };
                if acc.is_transform(member_catalog_id) {
                    return Ok(None);
                }
                let default = acc
                    .default_value_for(member_catalog_id, member.leaf.as_ref())
                    .and_then(Result::ok);
                columns.push(IndexKeyColumn::Member {
                    path: DataPathSegment::Member(catalog_id(member_catalog_id)?),
                    meaning: key.value_meaning.clone(),
                    default,
                });
            }
        }
    }
    Ok(Some(columns))
}

/// The full prospective unique-index key tuple a record would publish, or `None` when any
/// column is absent, so the record contributes no entry and cannot collide.
pub(super) fn prospective_index_key(
    store: &TreeStore,
    store_id: &CatalogId,
    probe: &UniqueIndexProbe,
    identity: &[SavedKey],
) -> Result<Option<Vec<SavedKey>>, StoreError> {
    let mut tuple = Vec::with_capacity(probe.columns.len());
    for column in &probe.columns {
        match column {
            IndexKeyColumn::Identity { position } => {
                let Some(key) = identity.get(*position) else {
                    return Ok(None);
                };
                tuple.push(key.clone());
            }
            IndexKeyColumn::Member {
                path,
                meaning,
                default,
            } => {
                let stored =
                    store.read_data_value(store_id, identity, std::slice::from_ref(path))?;
                let Some(bytes) = stored
                    .as_deref()
                    .or_else(|| default.as_ref().map(|value| value.encoded.as_slice()))
                else {
                    return Ok(None);
                };
                let Some(key) = meaning.stored_key(bytes) else {
                    return Ok(None);
                };
                tuple.push(key);
            }
        }
    }
    Ok(Some(tuple))
}

/// Running collision state for one unique index, keyed by the canonical byte encoding of each
/// key tuple. Every tuple shares the index's arity, so the encoding is an injective identity for
/// the tuple. This scan keeps one seen key per distinct populated tuple and reports the number
/// of distinct tuples more than one record claims; it does not count duplicate records.
#[derive(Default)]
pub(super) struct IndexScan {
    seen: HashSet<Vec<u8>>,
    collisions: HashSet<Vec<u8>>,
}

impl IndexScan {
    pub(super) fn observe(&mut self, key: Vec<SavedKey>) {
        let encoded = encode_identity_payload(&key);
        if !self.seen.insert(encoded.clone()) {
            self.collisions.insert(encoded);
        }
    }

    fn collision_tuple_count(&self) -> usize {
        self.collisions.len()
    }
}

/// Classify each changed index the place declares. Non-unique indexes rebuild their derived cells
/// from the records they cover; unique indexes first consume the prospective tuple scan, and a
/// collision or unprobeable key shape fails closed so a uniqueness guarantee is never published
/// without verification.
pub(super) fn classify_indexes(
    place: &CheckedSavedPlace,
    collisions: &HashMap<CatalogId, IndexScan>,
    unprobeable: &HashSet<CatalogId>,
    acc: &mut Accumulator,
) -> Result<(), StoreError> {
    for index in &place.indexes {
        let Some(index_catalog_id) = index.catalog_id.as_deref() else {
            continue;
        };
        let index_id = catalog_id(index_catalog_id)?;
        if !acc.is_changed_index(&index_id) {
            continue;
        }
        let collision_tuples = collisions
            .get(&index_id)
            .map_or(0, IndexScan::collision_tuple_count);
        let verdict = if index.unique && collision_tuples > 0 {
            acc.counts.index_collisions += collision_tuples;
            acc.diagnostic(
                index_id.clone(),
                format!(
                    "unique index `{}` has {collision_tuples} colliding key tuple(s); resolve duplicates before activating",
                    index.name
                ),
            );
            Verdict::RepairRequired {
                reason: RepairReason::UniqueIndexCollision,
            }
        } else if unprobeable.contains(&index_id) {
            acc.diagnostic(
                index_id.clone(),
                format!(
                    "unique index `{}` has a key shape the uniqueness scan cannot probe; its collisions cannot be verified, so the change fails closed",
                    index.name
                ),
            );
            Verdict::RepairRequired {
                reason: RepairReason::UniqueIndexUnprobeable,
            }
        } else {
            Verdict::DerivedRebuild
        };
        acc.push_index(index_id, verdict)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap, HashSet};

    use super::{IndexScan, classify_indexes, unique_index_plan};
    use crate::StoreLeafKind;
    use crate::evolution::discharge::{Accumulator, catalog_id};
    use crate::evolution::{RepairReason, Verdict};
    use crate::executable::{
        CheckedSavedIndex, CheckedSavedIndexKey, CheckedSavedKeyParam, CheckedSavedMember,
        CheckedSavedMemberKind, CheckedSavedPlace, CheckedSavedTerminal,
    };
    use crate::facts::{
        ResourceMemberId, StoreId, StoreIndexId, StoreIndexKeySource, StoredValueMeaning,
    };
    use marrow_catalog::CatalogEntryKind;
    use marrow_store::cell::CatalogId;
    use marrow_store::key::SavedKey;
    use marrow_store::value::ScalarType;

    fn unique_index(name: &str, catalog_id: &str, key_name: &str) -> CheckedSavedIndex {
        CheckedSavedIndex {
            id: StoreIndexId(0),
            name: name.to_string(),
            catalog_id: Some(catalog_id.to_string()),
            unique: true,
            keys: vec![CheckedSavedIndexKey {
                name: key_name.to_string(),
                source: StoreIndexKeySource::ResourceMember(ResourceMemberId(0)),
                value_meaning: StoredValueMeaning::Scalar(ScalarType::Str),
            }],
        }
    }

    fn place_with_indexes(indexes: Vec<CheckedSavedIndex>) -> CheckedSavedPlace {
        CheckedSavedPlace {
            root: "books".to_string(),
            store_id: StoreId(0),
            store_catalog_id: Some("cat_000000000000000000000000000000aa".to_string()),
            resource_name: "Book".to_string(),
            root_members: vec![CheckedSavedMember {
                id: Some(ResourceMemberId(0)),
                name: "isbn".to_string(),
                key_params: Vec::new(),
                kind: CheckedSavedMemberKind::Field { required: true },
                catalog_id: Some("cat_000000000000000000000000000000bb".to_string()),
                leaf: Some(StoreLeafKind::Scalar(ScalarType::Str)),
                typed_entry: false,
                group_members: Vec::new(),
            }],
            members: Vec::new(),
            indexes,
            identity_args: Vec::new(),
            identity_keys: vec![CheckedSavedKeyParam {
                name: "id".to_string(),
                scalar: Some(ScalarType::Int),
            }],
            next_id_shape: String::new(),
            layers: Vec::new(),
            terminal: CheckedSavedTerminal::Record,
            span: marrow_syntax::SourceSpan::default(),
        }
    }

    fn empty_accumulator() -> Accumulator {
        Accumulator::new(Vec::new(), BTreeSet::new(), HashSet::new(), HashMap::new())
    }

    // A unique index whose key resolves to a top-level plain field is probeable; one whose
    // key names a member the place does not declare cannot be probed for collisions, so the
    // plan must route it to `unprobeable` rather than silently treat it as a clean rebuild.
    #[test]
    fn unique_index_with_unresolvable_key_is_unprobeable() {
        let place = place_with_indexes(vec![
            unique_index("byIsbn", "cat_000000000000000000000000000000c1", "isbn"),
            unique_index("byGhost", "cat_000000000000000000000000000000c2", "ghost"),
        ]);

        let acc = empty_accumulator();
        let plan = unique_index_plan(&place, &acc).expect("plan");

        let probed: Vec<&str> = plan
            .probes
            .iter()
            .map(|probe| probe.catalog_id.as_str())
            .collect();
        let unprobeable: Vec<&str> = plan.unprobeable.iter().map(CatalogId::as_str).collect();
        assert_eq!(
            probed,
            ["cat_000000000000000000000000000000c1"],
            "probed {probed:?} unprobeable {unprobeable:?}"
        );
        assert_eq!(
            unprobeable,
            ["cat_000000000000000000000000000000c2"],
            "probed {probed:?} unprobeable {unprobeable:?}"
        );
    }

    // An unprobeable unique index must fail closed: its uniqueness cannot be verified from
    // the snapshot, so the discharge blocks activation rather than rebuilding an unchecked
    // guarantee. A probeable index with no collisions still discharges to a derived rebuild.
    #[test]
    fn unprobeable_unique_index_fails_closed() {
        let place = place_with_indexes(vec![
            unique_index("byIsbn", "cat_000000000000000000000000000000c1", "isbn"),
            unique_index("byGhost", "cat_000000000000000000000000000000c2", "ghost"),
        ]);
        let unprobeable: HashSet<CatalogId> =
            [catalog_id("cat_000000000000000000000000000000c2").unwrap()]
                .into_iter()
                .collect();
        let mut acc = empty_accumulator();
        acc.insert_affected(
            "cat_000000000000000000000000000000c1",
            CatalogEntryKind::StoreIndex,
        )
        .expect("mark changed index");
        acc.insert_affected(
            "cat_000000000000000000000000000000c2",
            CatalogEntryKind::StoreIndex,
        )
        .expect("mark changed index");

        classify_indexes(&place, &HashMap::new(), &unprobeable, &mut acc).expect("classify");

        let ghost = acc
            .verdicts
            .iter()
            .find(|v| v.catalog_id.as_str() == "cat_000000000000000000000000000000c2")
            .expect("ghost verdict");
        assert!(
            matches!(
                ghost.verdict,
                Verdict::RepairRequired {
                    reason: RepairReason::UniqueIndexUnprobeable
                }
            ),
            "an unprobeable unique index must fail closed, got {:?}",
            ghost.verdict
        );
        let isbn = acc
            .verdicts
            .iter()
            .find(|v| v.catalog_id.as_str() == "cat_000000000000000000000000000000c1")
            .expect("isbn verdict");
        assert!(
            matches!(isbn.verdict, Verdict::DerivedRebuild),
            "a probeable collision-free unique index rebuilds, got {:?}",
            isbn.verdict
        );
    }

    #[test]
    fn index_scan_reports_distinct_colliding_tuples_not_duplicate_records() {
        let mut scan = IndexScan::default();

        scan.observe(vec![SavedKey::Str("dup".into())]);
        scan.observe(vec![SavedKey::Str("dup".into())]);
        scan.observe(vec![SavedKey::Str("dup".into())]);
        scan.observe(vec![SavedKey::Str("other".into())]);
        scan.observe(vec![SavedKey::Str("other".into())]);

        assert_eq!(scan.collision_tuple_count(), 2);
    }
}
