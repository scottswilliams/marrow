use std::collections::HashSet;
use std::path::{Path, PathBuf};

use marrow_schema::{SchemaError, SchemaStoreInvalidation};
use marrow_syntax::StoreDecl;

use crate::CheckedProgram;
use crate::facts::{
    EnumId, ModuleId, ResourceId, ResourceMemberFact, ResourceMemberKind, StoreFact, StoreId,
    StoreIndexId, StoredValueMeaning,
};

#[derive(Debug, Clone, Default)]
pub(crate) struct PendingBackingInvalidations {
    entries: Vec<PendingBackingInvalidation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingBackingInvalidation {
    Resource {
        file: PathBuf,
        name: String,
    },
    Store {
        file: PathBuf,
        root: String,
    },
    Root {
        root: String,
    },
    Index {
        file: PathBuf,
        root: String,
        name: String,
    },
    Enum {
        file: PathBuf,
        name: String,
    },
}

impl PendingBackingInvalidations {
    pub(crate) fn extend(&mut self, other: Self) {
        for entry in other.entries {
            self.record(entry);
        }
    }

    pub(crate) fn record_resource_error(&mut self, file: &Path, resource: &str) {
        self.record_invalid_resource(file, resource);
    }

    pub(crate) fn record_invalid_resource(&mut self, file: &Path, resource: &str) {
        self.record(PendingBackingInvalidation::Resource {
            file: file.to_path_buf(),
            name: resource.to_string(),
        });
    }

    pub(crate) fn record_invalid_enum(&mut self, file: &Path, enum_name: &str) {
        self.record(PendingBackingInvalidation::Enum {
            file: file.to_path_buf(),
            name: enum_name.to_string(),
        });
    }

    pub(crate) fn record_invalid_root(&mut self, root: &str) {
        self.record(PendingBackingInvalidation::Root {
            root: root.to_string(),
        });
    }

    pub(crate) fn record_store_error(
        &mut self,
        file: &Path,
        store: &StoreDecl,
        error: &SchemaError,
    ) {
        match error.kind.store_invalidation() {
            Some(SchemaStoreInvalidation::Store) => {
                self.record(PendingBackingInvalidation::Store {
                    file: file.to_path_buf(),
                    root: store.root.root.clone(),
                });
            }
            Some(SchemaStoreInvalidation::Index { name }) => {
                self.record(PendingBackingInvalidation::Index {
                    file: file.to_path_buf(),
                    root: store.root.root.clone(),
                    name,
                });
            }
            None => {}
        }
    }

    pub(crate) fn resolve(&self, program: &CheckedProgram) -> BackingValidity {
        let mut validity = BackingValidity::default();
        for entry in &self.entries {
            validity.resolve_pending(program, entry);
        }
        validity
    }

    fn record(&mut self, entry: PendingBackingInvalidation) {
        if !self.entries.contains(&entry) {
            self.entries.push(entry);
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BackingValidity {
    invalid_resources: HashSet<ResourceId>,
    invalid_stores: HashSet<StoreId>,
    duplicate_root_stores: HashSet<StoreId>,
    invalid_indexes: HashSet<StoreIndexId>,
    invalid_enums: HashSet<EnumId>,
}

impl BackingValidity {
    fn resolve_pending(&mut self, program: &CheckedProgram, entry: &PendingBackingInvalidation) {
        match entry {
            PendingBackingInvalidation::Resource { file, name } => {
                self.resolve_resource(program, file, name);
            }
            PendingBackingInvalidation::Store { file, root } => {
                self.resolve_store(program, file, root);
            }
            PendingBackingInvalidation::Root { root } => {
                self.resolve_root(program, root);
            }
            PendingBackingInvalidation::Index { file, root, name } => {
                self.resolve_index(program, file, root, name);
            }
            PendingBackingInvalidation::Enum { file, name } => {
                self.resolve_enum(program, file, name);
            }
        }
    }

    fn resolve_resource(&mut self, program: &CheckedProgram, file: &Path, name: &str) {
        self.invalid_resources.extend(
            program
                .facts
                .resources()
                .iter()
                .filter(|resource| {
                    module_file(program, resource.module) == Some(file) && resource.name == name
                })
                .map(|resource| resource.id),
        );
    }

    fn resolve_store(&mut self, program: &CheckedProgram, file: &Path, root: &str) {
        self.invalid_stores.extend(
            program
                .facts
                .stores()
                .iter()
                .filter(|store| {
                    module_file(program, store.module) == Some(file) && store.root == root
                })
                .map(|store| store.id),
        );
    }

    fn resolve_root(&mut self, program: &CheckedProgram, root: &str) {
        for store in program
            .facts
            .stores()
            .iter()
            .filter(|store| store.root == root)
        {
            self.invalid_stores.insert(store.id);
            self.duplicate_root_stores.insert(store.id);
        }
    }

    fn resolve_index(&mut self, program: &CheckedProgram, file: &Path, root: &str, name: &str) {
        for store_id in program
            .facts
            .stores()
            .iter()
            .filter(|store| module_file(program, store.module) == Some(file) && store.root == root)
            .map(|store| store.id)
        {
            self.invalid_indexes.extend(
                program
                    .facts
                    .store_indexes()
                    .iter()
                    .filter(|index| index.store == store_id && index.name == name)
                    .map(|index| index.id),
            );
        }
    }

    fn resolve_enum(&mut self, program: &CheckedProgram, file: &Path, name: &str) {
        self.invalid_enums.extend(
            program
                .facts
                .enums()
                .iter()
                .filter(|enum_| {
                    module_file(program, enum_.module) == Some(file) && enum_.name == name
                })
                .map(|enum_| enum_.id),
        );
    }

    pub(crate) fn store_is_invalid(&self, store: &StoreFact) -> bool {
        self.invalid_stores.contains(&store.id)
    }

    pub(crate) fn store_has_duplicate_root(&self, store: &StoreFact) -> bool {
        self.duplicate_root_stores.contains(&store.id)
    }

    pub(crate) fn resource_is_invalid(
        &self,
        program: &CheckedProgram,
        resource: ResourceId,
    ) -> bool {
        self.invalid_resources.contains(&resource)
            || self.resource_member_meanings_are_invalid(program, resource)
    }

    pub(crate) fn field_is_invalid(
        &self,
        program: &CheckedProgram,
        member: &ResourceMemberFact,
    ) -> bool {
        let mut visited = MeaningVisit::default();
        self.resource_member_is_invalid(program, member, &mut visited)
    }

    pub(crate) fn index_is_invalid(&self, program: &CheckedProgram, index: StoreIndexId) -> bool {
        let index = program.facts.store_index(index);
        self.invalid_indexes.contains(&index.id)
            || index.keys.len() != index.declared_key_count
            || index
                .keys
                .iter()
                .any(|key| self.stored_value_meaning_is_invalid(program, &key.value_meaning))
    }

    fn resource_member_meanings_are_invalid(
        &self,
        program: &CheckedProgram,
        resource: ResourceId,
    ) -> bool {
        let mut visited = MeaningVisit::default();
        self.resource_member_meanings_are_invalid_inner(program, resource, &mut visited)
    }

    fn resource_member_meanings_are_invalid_inner(
        &self,
        program: &CheckedProgram,
        resource: ResourceId,
        visited: &mut MeaningVisit,
    ) -> bool {
        if !visited.resources.insert(resource) {
            return false;
        }
        program
            .facts
            .resource_members()
            .iter()
            .filter(|member| member.resource == resource)
            .any(|member| self.resource_member_is_invalid(program, member, visited))
    }

    fn resource_member_is_invalid(
        &self,
        program: &CheckedProgram,
        member: &ResourceMemberFact,
        visited: &mut MeaningVisit,
    ) -> bool {
        match member.kind {
            ResourceMemberKind::Field => match member.value_meaning.as_ref() {
                Some(meaning) => {
                    self.stored_value_meaning_is_invalid_inner(program, Some(meaning), visited)
                }
                None => true,
            },
            ResourceMemberKind::Group => false,
        }
    }

    fn stored_value_meaning_is_invalid(
        &self,
        program: &CheckedProgram,
        meaning: &StoredValueMeaning,
    ) -> bool {
        let mut visited = MeaningVisit::default();
        self.stored_value_meaning_is_invalid_inner(program, Some(meaning), &mut visited)
    }

    fn stored_value_meaning_is_invalid_inner(
        &self,
        program: &CheckedProgram,
        meaning: Option<&StoredValueMeaning>,
        visited: &mut MeaningVisit,
    ) -> bool {
        match meaning {
            None | Some(StoredValueMeaning::Scalar(_)) => false,
            Some(StoredValueMeaning::Enum { enum_id, .. }) => {
                self.enum_is_invalid(program, *enum_id)
            }
            Some(StoredValueMeaning::Identity { store, .. }) => {
                self.identity_store_is_invalid(program, *store, visited)
            }
        }
    }

    fn identity_store_is_invalid(
        &self,
        program: &CheckedProgram,
        store: StoreId,
        visited: &mut MeaningVisit,
    ) -> bool {
        if !visited.stores.insert(store) {
            return false;
        }
        let store = program.facts.store(store);
        self.store_is_invalid(store)
            || self.invalid_resources.contains(&store.resource)
            || self.resource_member_meanings_are_invalid_inner(program, store.resource, visited)
    }

    fn enum_is_invalid(&self, program: &CheckedProgram, enum_id: EnumId) -> bool {
        program.facts.enum_(enum_id).is_none() || self.invalid_enums.contains(&enum_id)
    }
}

#[derive(Default)]
struct MeaningVisit {
    resources: HashSet<ResourceId>,
    stores: HashSet<StoreId>,
}

fn module_file(program: &CheckedProgram, module: ModuleId) -> Option<&Path> {
    program
        .facts
        .modules()
        .get(module.0 as usize)
        .map(|module| module.source_file.as_path())
}
