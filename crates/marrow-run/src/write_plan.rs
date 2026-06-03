//! Explicit managed-write plans and commit behavior.

use marrow_store::StoreError;
use marrow_store::key::SavedKey;
use marrow_store::tree::{CommitMetadata, DataPathSegment, EngineProfile, TreeStore};

use crate::store::{DataAddress, IndexAddress};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlanStep {
    WriteData {
        address: DataAddress,
        value: Vec<u8>,
    },
    DeleteData {
        address: DataAddress,
    },
    DeleteRecordSubtree {
        address: DataAddress,
    },
    WriteIndex {
        address: IndexAddress,
        identity: Vec<SavedKey>,
        value: Vec<u8>,
    },
    DeleteIndex {
        address: IndexAddress,
        identity: Vec<SavedKey>,
    },
    DeleteIndexSubtree {
        address: IndexAddress,
    },
    /// Stamp the catalog epoch, engine profile, and commit metadata. This commits in
    /// the same transaction as the data steps so the store's epoch never advances
    /// without the data the new epoch describes. It is a metadata stamp, not a data or
    /// index write, so the read-only projection reports it as a [`WriteTarget::Meta`].
    StampMetadata {
        catalog_epoch: u64,
        profile: EngineProfile,
        commit: CommitMetadata,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOp {
    Write,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteTarget {
    Data {
        store: String,
        identity: Vec<SavedKey>,
        path: Vec<WriteDataSegment>,
    },
    Index {
        index: String,
        keys: Vec<SavedKey>,
        identity: Vec<SavedKey>,
    },
    /// A store-wide metadata stamp: the catalog epoch the commit advances to. It
    /// addresses no record or index cell, so a projection consumer (a dry-run summary)
    /// reports it as a metadata change rather than a data write.
    Meta { catalog_epoch: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteDataSegment {
    Member(String),
    Key(SavedKey),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WritePlan {
    pub(crate) steps: Vec<PlanStep>,
}

impl WritePlan {
    pub fn commit(self, store: &TreeStore, in_txn: bool) -> Result<(), StoreError> {
        if in_txn {
            return apply_steps(self.steps, store);
        }
        store.begin()?;
        match apply_steps(self.steps, store) {
            Ok(()) => store.commit(),
            Err(error) => {
                let _ = store.rollback();
                Err(error)
            }
        }
    }

    pub fn steps(&self) -> impl Iterator<Item = (WriteOp, WriteTarget, Option<&[u8]>)> {
        self.steps.iter().map(|step| match step {
            PlanStep::WriteData { address, value } => {
                (WriteOp::Write, data_target(address), Some(value.as_slice()))
            }
            PlanStep::DeleteData { address } | PlanStep::DeleteRecordSubtree { address } => {
                (WriteOp::Delete, data_target(address), None)
            }
            PlanStep::WriteIndex {
                address,
                identity,
                value,
            } => (
                WriteOp::Write,
                index_target(address, identity),
                Some(value.as_slice()),
            ),
            PlanStep::DeleteIndex { address, identity } => {
                (WriteOp::Delete, index_target(address, identity), None)
            }
            PlanStep::DeleteIndexSubtree { address } => {
                (WriteOp::Delete, index_target(address, &[]), None)
            }
            PlanStep::StampMetadata { catalog_epoch, .. } => (
                WriteOp::Write,
                WriteTarget::Meta {
                    catalog_epoch: *catalog_epoch,
                },
                None,
            ),
        })
    }
}

fn apply_steps(steps: Vec<PlanStep>, store: &TreeStore) -> Result<(), StoreError> {
    for step in steps {
        match step {
            PlanStep::WriteData { address, value } => {
                store.write_data_value(&address.store, &address.identity, &address.path, value)?
            }
            PlanStep::DeleteData { address } => {
                store.delete_data_subtree(&address.store, &address.identity, &address.path)?
            }
            PlanStep::DeleteRecordSubtree { address } => {
                store.delete_record_subtree(&address.store, &address.identity)?
            }
            PlanStep::WriteIndex {
                address,
                identity,
                value,
            } => store.write_index_entry(&address.index, &address.keys, &identity, value)?,
            PlanStep::DeleteIndex { address, identity } => {
                store.delete_index_entry(&address.index, &address.keys, &identity)?
            }
            PlanStep::DeleteIndexSubtree { address } => {
                store.delete_index_subtree(&address.index, &address.keys)?
            }
            PlanStep::StampMetadata {
                catalog_epoch,
                profile,
                commit,
            } => {
                store.write_catalog_epoch(catalog_epoch)?;
                store.write_engine_profile(&profile)?;
                store.write_commit_metadata(&commit)?;
            }
        }
    }
    Ok(())
}

fn data_target(address: &DataAddress) -> WriteTarget {
    WriteTarget::Data {
        store: address.store.as_str().to_string(),
        identity: address.identity.clone(),
        path: address
            .path
            .iter()
            .map(|segment| match segment {
                DataPathSegment::Member(member) => {
                    WriteDataSegment::Member(member.as_str().to_string())
                }
                DataPathSegment::Key(key) => WriteDataSegment::Key(key.clone()),
            })
            .collect(),
    }
}

fn index_target(address: &IndexAddress, identity: &[SavedKey]) -> WriteTarget {
    WriteTarget::Index {
        index: address.index.as_str().to_string(),
        keys: address.keys.clone(),
        identity: identity.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A metadata stamp projects to a write of a `Meta` target carrying the catalog
    /// epoch and no value, so a dry-run consumer reports it as a metadata change rather
    /// than a data or index write.
    #[test]
    fn stamp_metadata_projects_to_meta_target() {
        let profile = EngineProfile::new(3);
        let commit = CommitMetadata {
            commit_id: 7,
            catalog_epoch: 5,
            layout_epoch: 3,
            source_digest: "fnv1a64:0000000000000005".to_string(),
            engine_profile_digest: profile.digest_bytes(),
            changed_root_catalog_ids: Vec::new(),
            changed_index_catalog_ids: Vec::new(),
        };
        let plan = WritePlan {
            steps: vec![PlanStep::StampMetadata {
                catalog_epoch: 5,
                profile,
                commit,
            }],
        };

        let projected: Vec<_> = plan.steps().collect();
        assert_eq!(
            projected,
            vec![(WriteOp::Write, WriteTarget::Meta { catalog_epoch: 5 }, None)]
        );
    }
}
