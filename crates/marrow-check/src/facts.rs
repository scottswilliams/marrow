//! Typed checked facts derived from the best-effort checked program.

use std::collections::HashMap;
use std::path::PathBuf;

use marrow_schema::stdlib::Capability;
use marrow_schema::{NodeKind, ScalarType, Type};
use marrow_store::key::SavedKey;
use marrow_store::tree::decode_tree_enum_member;
use marrow_store::value::decode_value;
use marrow_syntax::{ParsedSource, ResourceMember, SourceSpan, TypeRef};

use crate::catalog::{
    CatalogKey, enum_path, resource_member_path, resource_path, store_index_path, store_path,
};
use crate::executable::CheckedParamMode;
use crate::program::{CheckedModule, MarrowType};
use crate::{build_alias_map, expand_alias};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StoreId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StoreIndexId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceMemberId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumMemberId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalId(pub u32);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckedFacts {
    modules: Vec<ModuleFact>,
    functions: Vec<FunctionFact>,
    locals: Vec<LocalFact>,
    resources: Vec<ResourceFact>,
    stores: Vec<StoreFact>,
    store_indexes: Vec<StoreIndexFact>,
    resource_members: Vec<ResourceMemberFact>,
    enums: Vec<EnumFact>,
    enum_members: Vec<EnumMemberFact>,
    presence_proofs: Vec<PresenceProofFact>,
}

impl CheckedFacts {
    pub(crate) fn from_modules(
        modules: &[CheckedModule],
        sources: &HashMap<PathBuf, &ParsedSource>,
    ) -> Self {
        let mut facts = Self::default();

        for (module_index, module) in modules.iter().enumerate() {
            facts.modules.push(ModuleFact {
                id: ModuleId(module_index as u32),
                name: module.name.clone(),
                source_file: module.source_file.clone(),
                span: module.span,
            });
        }

        for (module_index, module) in modules.iter().enumerate() {
            let module_id = ModuleId(module_index as u32);
            let parsed = sources.get(&module.source_file);
            facts.collect_enum_facts(module_id, module, parsed.copied());
        }

        for (module_index, module) in modules.iter().enumerate() {
            let module_id = ModuleId(module_index as u32);
            let parsed = sources.get(&module.source_file);
            facts.collect_resource_facts(module_id, module, parsed.copied());
        }

        for (module_index, module) in modules.iter().enumerate() {
            let module_id = ModuleId(module_index as u32);
            let parsed = sources.get(&module.source_file);
            facts.collect_store_facts(module_id, module, parsed.copied());
        }

        for (module_index, module) in modules.iter().enumerate() {
            let module_id = ModuleId(module_index as u32);
            let parsed = sources.get(&module.source_file).copied();
            for function in &module.functions {
                if let Some(function) = facts.function_fact(module_id, module, function, parsed) {
                    facts.functions.push(function);
                }
            }
        }

        facts
    }

    pub fn modules(&self) -> &[ModuleFact] {
        &self.modules
    }

    pub fn functions(&self) -> &[FunctionFact] {
        &self.functions
    }

    pub fn locals(&self) -> &[LocalFact] {
        &self.locals
    }

    pub fn resources(&self) -> &[ResourceFact] {
        &self.resources
    }

    pub fn resource(&self, id: ResourceId) -> &ResourceFact {
        &self.resources[id.0 as usize]
    }

    pub fn stores(&self) -> &[StoreFact] {
        &self.stores
    }

    pub fn store(&self, id: StoreId) -> &StoreFact {
        &self.stores[id.0 as usize]
    }

    pub fn store_indexes(&self) -> &[StoreIndexFact] {
        &self.store_indexes
    }

    pub fn store_index(&self, id: StoreIndexId) -> &StoreIndexFact {
        &self.store_indexes[id.0 as usize]
    }

    pub fn resource_members(&self) -> &[ResourceMemberFact] {
        &self.resource_members
    }

    pub fn enums(&self) -> &[EnumFact] {
        &self.enums
    }

    pub fn enum_members(&self) -> &[EnumMemberFact] {
        &self.enum_members
    }

    pub fn enum_(&self, id: EnumId) -> Option<&EnumFact> {
        self.enums.get(id.0 as usize)
    }

    pub fn enum_member(&self, id: EnumMemberId) -> Option<&EnumMemberFact> {
        self.enum_members.get(id.0 as usize)
    }

    pub(crate) fn enum_member_by_source_order(
        &self,
        enum_id: EnumId,
        ordinal: u32,
    ) -> Option<&EnumMemberFact> {
        self.enum_members
            .iter()
            .filter(|member| member.enum_id == enum_id)
            .nth(ordinal as usize)
    }

    pub fn enum_member_is_selectable(&self, id: EnumMemberId) -> bool {
        self.enum_member(id)
            .is_some_and(|member| !member.category && !self.enum_member_has_children(id))
    }

    pub fn enum_member_is_descendant(
        &self,
        member_id: EnumMemberId,
        ancestor_id: EnumMemberId,
    ) -> bool {
        let Some(member) = self.enum_member(member_id) else {
            return false;
        };
        let Some(ancestor) = self.enum_member(ancestor_id) else {
            return false;
        };
        if member.enum_id != ancestor.enum_id {
            return false;
        }
        let mut current = Some(member_id);
        while let Some(id) = current {
            if id == ancestor_id {
                return true;
            }
            current = self.enum_member(id).and_then(|member| member.parent);
        }
        false
    }

    pub fn presence_proofs(&self) -> &[PresenceProofFact] {
        &self.presence_proofs
    }

    pub(crate) fn bind_catalog_ids(
        &mut self,
        modules: &[CheckedModule],
        ids: &HashMap<CatalogKey, String>,
    ) {
        self.bind_resource_catalog_ids(modules, ids);
        self.bind_store_catalog_ids(modules, ids);
        self.bind_store_index_catalog_ids(modules, ids);
        self.bind_resource_member_catalog_ids(modules, ids);
        self.bind_enum_catalog_ids(modules, ids);
        self.bind_enum_member_catalog_ids(modules, ids);
    }

    fn bind_resource_catalog_ids(
        &mut self,
        modules: &[CheckedModule],
        ids: &HashMap<CatalogKey, String>,
    ) {
        let resource_paths: Vec<String> = self
            .resources
            .iter()
            .map(|resource| {
                let module = &modules[resource.module.0 as usize];
                resource_path(&module.name, &resource.name)
            })
            .collect();
        for (resource, path) in self.resources.iter_mut().zip(resource_paths) {
            resource.catalog_id = catalog_id(ids, marrow_project::CatalogEntryKind::Resource, path);
        }
    }

    fn bind_store_catalog_ids(
        &mut self,
        modules: &[CheckedModule],
        ids: &HashMap<CatalogKey, String>,
    ) {
        let store_paths: Vec<String> = self
            .stores
            .iter()
            .map(|store| {
                let module = &modules[store.module.0 as usize];
                store_path(&module.name, &store.root)
            })
            .collect();
        for (store, path) in self.stores.iter_mut().zip(store_paths) {
            store.catalog_id = catalog_id(ids, marrow_project::CatalogEntryKind::Store, path);
        }
    }

    fn bind_store_index_catalog_ids(
        &mut self,
        modules: &[CheckedModule],
        ids: &HashMap<CatalogKey, String>,
    ) {
        let store_index_paths: Vec<String> = self
            .store_indexes
            .iter()
            .map(|index| {
                let store = &self.stores[index.store.0 as usize];
                let module = &modules[store.module.0 as usize];
                store_index_path(&module.name, &store.root, &index.name)
            })
            .collect();
        for (index, path) in self.store_indexes.iter_mut().zip(store_index_paths) {
            index.catalog_id = catalog_id(ids, marrow_project::CatalogEntryKind::StoreIndex, path);
        }
    }

    fn bind_resource_member_catalog_ids(
        &mut self,
        modules: &[CheckedModule],
        ids: &HashMap<CatalogKey, String>,
    ) {
        let resource_member_paths: Vec<String> = self
            .resource_members
            .iter()
            .map(|member| {
                let resource = &self.resources[member.resource.0 as usize];
                let module = &modules[resource.module.0 as usize];
                resource_member_path(
                    &module.name,
                    &resource.name,
                    &resource_member_name_path(&self.resource_members, member.id),
                )
            })
            .collect();
        for (member, path) in self.resource_members.iter_mut().zip(resource_member_paths) {
            member.catalog_id =
                catalog_id(ids, marrow_project::CatalogEntryKind::ResourceMember, path);
        }
    }

    fn bind_enum_catalog_ids(
        &mut self,
        modules: &[CheckedModule],
        ids: &HashMap<CatalogKey, String>,
    ) {
        let enum_paths: Vec<String> = self
            .enums
            .iter()
            .map(|enum_fact| {
                let module = &modules[enum_fact.module.0 as usize];
                enum_path(&module.name, &enum_fact.name)
            })
            .collect();
        for (enum_fact, path) in self.enums.iter_mut().zip(enum_paths) {
            enum_fact.catalog_id = catalog_id(ids, marrow_project::CatalogEntryKind::Enum, path);
        }
    }

    fn bind_enum_member_catalog_ids(
        &mut self,
        modules: &[CheckedModule],
        ids: &HashMap<CatalogKey, String>,
    ) {
        let enum_member_paths: Vec<String> = self
            .enum_members
            .iter()
            .map(|member| {
                let enum_fact = &self.enums[member.enum_id.0 as usize];
                let module = &modules[enum_fact.module.0 as usize];
                let path = enum_member_name_path(&self.enum_members, member.id);
                format!(
                    "{}::{}",
                    enum_path(&module.name, &enum_fact.name),
                    path.join("::")
                )
            })
            .collect();
        for (member, path) in self.enum_members.iter_mut().zip(enum_member_paths) {
            member.catalog_id = catalog_id(ids, marrow_project::CatalogEntryKind::EnumMember, path);
        }
    }

    pub(crate) fn record_presence_proof(&mut self, proof: PresenceProofDraft) {
        if self.presence_proofs.iter().any(|existing| {
            existing.place == proof.place
                && existing.keys == proof.keys
                && existing.read == proof.read
                && existing.source == proof.source
                && existing.status == proof.status
                && existing.span == proof.span
        }) {
            return;
        }
        let id = PresenceProofId(self.presence_proofs.len() as u32);
        self.presence_proofs.push(PresenceProofFact {
            id,
            place: proof.place,
            keys: proof.keys,
            read: proof.read,
            source: proof.source,
            status: proof.status,
            span: proof.span,
        });
    }

    pub(crate) fn refresh_direct_effects(&mut self, modules: &[CheckedModule]) {
        let effects: Vec<DirectEffectFacts> = self
            .functions
            .iter()
            .map(|fact| {
                modules
                    .get(fact.module.0 as usize)
                    .and_then(|module| {
                        module
                            .functions
                            .iter()
                            .find(|function| function.name == fact.name)
                    })
                    .and_then(|function| function.runtime_body())
                    .map_or_else(DirectEffectFacts::default, |body| {
                        crate::presence::direct_effects_for_block(self, body)
                    })
            })
            .collect();
        for (function, effects) in self.functions.iter_mut().zip(effects) {
            function.direct_effects = effects;
        }
    }

    pub(crate) fn overwrite_prefix_from(&mut self, prefix: &Self) {
        overwrite_prefix(&mut self.modules, &prefix.modules);
        overwrite_prefix(&mut self.functions, &prefix.functions);
        overwrite_prefix(&mut self.locals, &prefix.locals);
        overwrite_prefix(&mut self.resources, &prefix.resources);
        overwrite_prefix(&mut self.stores, &prefix.stores);
        overwrite_prefix(&mut self.store_indexes, &prefix.store_indexes);
        overwrite_prefix(&mut self.resource_members, &prefix.resource_members);
        overwrite_prefix(&mut self.enums, &prefix.enums);
        overwrite_prefix(&mut self.enum_members, &prefix.enum_members);
    }

    pub fn module_id(&self, name: &str) -> Option<ModuleId> {
        self.modules
            .iter()
            .find(|module| module.name == name)
            .map(|module| module.id)
    }

    pub fn resource_id(&self, module: ModuleId, name: &str) -> Option<ResourceId> {
        self.resources
            .iter()
            .find(|resource| resource.module == module && resource.name == name)
            .map(|resource| resource.id)
    }

    pub fn store_id(&self, module: ModuleId, root: &str) -> Option<StoreId> {
        self.stores
            .iter()
            .find(|store| store.module == module && store.root == root)
            .map(|store| store.id)
    }

    pub fn resource_member_id(
        &self,
        resource: ResourceId,
        path: &[&str],
    ) -> Option<ResourceMemberId> {
        let mut parent = None;
        let mut current = None;
        for name in path {
            let member = self.resource_members.iter().find(|member| {
                member.resource == resource && member.parent == parent && member.name == *name
            })?;
            current = Some(member.id);
            parent = current;
        }
        current
    }

    pub fn enum_id(&self, module: ModuleId, name: &str) -> Option<EnumId> {
        self.enums
            .iter()
            .find(|enum_fact| enum_fact.module == module && enum_fact.name == name)
            .map(|enum_fact| enum_fact.id)
    }

    pub fn function_id(&self, module: ModuleId, name: &str) -> Option<FunctionId> {
        self.functions
            .iter()
            .find(|function| function.module == module && function.name == name)
            .map(|function| function.id)
    }

    pub fn function(&self, id: FunctionId) -> &FunctionFact {
        &self.functions[id.0 as usize]
    }

    fn function_fact(
        &mut self,
        module_id: ModuleId,
        module: &CheckedModule,
        function: &crate::CheckedFunction,
        parsed: Option<&ParsedSource>,
    ) -> Option<FunctionFact> {
        let declaration = parsed.and_then(|parsed| parsed.file.function(&function.name));
        let aliases = build_alias_map(&module.imports);

        let params = function
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let annotation = declaration
                    .and_then(|declaration| declaration.params.get(index))
                    .map(|param| &param.ty);
                let ty =
                    self.checked_type_for_signature(module_id, &param.ty, annotation, &aliases)?;
                Some((param.name.clone(), param.mode, ty))
            })
            .collect::<Option<Vec<_>>>()?;
        let return_type = match function.return_type.as_ref() {
            Some(ty) => {
                let annotation =
                    declaration.and_then(|declaration| declaration.return_type.as_ref());
                Some(self.checked_type_for_signature(module_id, ty, annotation, &aliases)?)
            }
            None => None,
        };

        let id = FunctionId(self.functions.len() as u32);
        let params = params
            .into_iter()
            .map(|(name, mode, ty)| {
                let local = LocalFact {
                    id: LocalId(self.locals.len() as u32),
                    function: id,
                    name,
                    mode,
                    ty,
                };
                self.locals.push(local.clone());
                local
            })
            .collect();

        Some(FunctionFact {
            id,
            module: module_id,
            name: function.name.clone(),
            public: function.public,
            params,
            return_type,
            direct_effects: DirectEffectFacts::default(),
            span: function.span,
        })
    }

    fn collect_resource_facts(
        &mut self,
        module_id: ModuleId,
        module: &CheckedModule,
        parsed: Option<&ParsedSource>,
    ) {
        for resource in &module.resources {
            let declaration = parsed.and_then(|parsed| {
                parsed
                    .file
                    .declarations
                    .iter()
                    .find_map(|declaration| match declaration {
                        marrow_syntax::Declaration::Resource(candidate)
                            if candidate.name == resource.name =>
                        {
                            Some(candidate)
                        }
                        _ => None,
                    })
            });
            let resource_id = ResourceId(self.resources.len() as u32);
            self.resources.push(ResourceFact {
                id: resource_id,
                module: module_id,
                name: resource.name.clone(),
                catalog_id: None,
                span: declaration.map_or(SourceSpan::default(), |resource| resource.span),
            });
            let aliases = build_alias_map(&module.imports);
            self.collect_resource_member_facts(
                module_id,
                resource_id,
                None,
                &resource.members,
                declaration.map(|resource| resource.members.as_slice()),
                &aliases,
            );
        }
    }

    fn collect_store_facts(
        &mut self,
        module_id: ModuleId,
        module: &CheckedModule,
        parsed: Option<&ParsedSource>,
    ) {
        for store in &module.stores {
            let declaration = parsed.and_then(|parsed| {
                parsed
                    .file
                    .declarations
                    .iter()
                    .find_map(|declaration| match declaration {
                        marrow_syntax::Declaration::Store(candidate)
                            if candidate.root.root == store.root =>
                        {
                            Some(candidate)
                        }
                        _ => None,
                    })
            });
            let Some(resource) = self.resource_id(module_id, &store.resource) else {
                continue;
            };
            let store_id = StoreId(self.stores.len() as u32);
            let aliases = build_alias_map(&module.imports);
            let identity_keys = store
                .identity_keys
                .iter()
                .map(|key| StoreIdentityKeyFact {
                    name: key.name.clone(),
                    value_meaning: self.stored_value_meaning(module_id, &key.ty, &aliases),
                })
                .collect();
            self.stores.push(StoreFact {
                id: store_id,
                module: module_id,
                root: store.root.clone(),
                resource,
                identity_keys,
                next_id_shape: store.next_id_shape(),
                catalog_id: None,
                span: declaration.map_or(SourceSpan::default(), |store| store.span),
            });
            for index in &store.indexes {
                let span = declaration
                    .and_then(|store| {
                        store
                            .indexes
                            .iter()
                            .find(|candidate| candidate.name == index.name)
                            .map(|candidate| candidate.span)
                    })
                    .unwrap_or_default();
                let id = StoreIndexId(self.store_indexes.len() as u32);
                let keys = module
                    .resources
                    .iter()
                    .find(|resource| resource.name == store.resource)
                    .map(|resource_schema| {
                        self.store_index_keys(
                            module_id,
                            resource,
                            store,
                            resource_schema,
                            index,
                            &aliases,
                        )
                    })
                    .unwrap_or_else(Vec::new);
                self.store_indexes.push(StoreIndexFact {
                    id,
                    store: store_id,
                    name: index.name.clone(),
                    unique: index.unique,
                    keys,
                    catalog_id: None,
                    span,
                });
            }
        }
    }

    fn collect_resource_member_facts(
        &mut self,
        module_id: ModuleId,
        resource_id: ResourceId,
        parent: Option<ResourceMemberId>,
        nodes: &[marrow_schema::Node],
        declarations: Option<&[ResourceMember]>,
        aliases: &HashMap<String, Vec<String>>,
    ) {
        for node in nodes {
            let declaration = declarations.and_then(|declarations| {
                declarations.iter().find(|member| match member {
                    ResourceMember::Field(field) => field.name == node.name,
                    ResourceMember::Group(group) => group.name == node.name,
                })
            });
            let span = declaration
                .map(|member| match member {
                    ResourceMember::Field(field) => field.span,
                    ResourceMember::Group(group) => group.span,
                })
                .unwrap_or_default();
            let kind = match node.kind {
                NodeKind::Slot { .. } => ResourceMemberKind::Field,
                NodeKind::Group => ResourceMemberKind::Group,
            };
            let value_meaning = match &node.kind {
                NodeKind::Slot { ty, .. } => self.stored_value_meaning(module_id, ty, aliases),
                NodeKind::Group => None,
            };
            let id = ResourceMemberId(self.resource_members.len() as u32);
            self.resource_members.push(ResourceMemberFact {
                id,
                resource: resource_id,
                parent,
                name: node.name.clone(),
                kind,
                value_meaning,
                catalog_id: None,
                span,
            });
            let nested = declaration.and_then(|member| match member {
                ResourceMember::Group(group) => Some(group.members.as_slice()),
                _ => None,
            });
            self.collect_resource_member_facts(
                module_id,
                resource_id,
                Some(id),
                &node.members,
                nested,
                aliases,
            );
        }
    }

    fn collect_enum_facts(
        &mut self,
        module_id: ModuleId,
        module: &CheckedModule,
        parsed: Option<&ParsedSource>,
    ) {
        for enum_schema in &module.enums {
            let declaration = parsed.and_then(|parsed| {
                parsed
                    .file
                    .declarations
                    .iter()
                    .find_map(|declaration| match declaration {
                        marrow_syntax::Declaration::Enum(candidate)
                            if candidate.name == enum_schema.name =>
                        {
                            Some(candidate)
                        }
                        _ => None,
                    })
            });
            let enum_id = EnumId(self.enums.len() as u32);
            self.enums.push(EnumFact {
                id: enum_id,
                module: module_id,
                name: enum_schema.name.clone(),
                catalog_id: None,
                span: declaration.map_or(SourceSpan::default(), |decl| decl.span),
            });

            let mut member_spans = Vec::new();
            if let Some(declaration) = declaration {
                flatten_enum_member_spans(&declaration.members, &mut member_spans);
            }
            let member_start = self.enum_members.len() as u32;
            for (index, member) in enum_schema.members.iter().enumerate() {
                self.enum_members.push(EnumMemberFact {
                    id: EnumMemberId(member_start + index as u32),
                    enum_id,
                    parent: member
                        .parent
                        .map(|parent| EnumMemberId(member_start + parent as u32)),
                    name: member.name.clone(),
                    category: member.category,
                    catalog_id: None,
                    span: member_spans
                        .get(index)
                        .copied()
                        .unwrap_or_else(SourceSpan::default),
                });
            }
        }
    }

    fn checked_type_for_signature(
        &self,
        module_id: ModuleId,
        ty: &MarrowType,
        annotation: Option<&TypeRef>,
        aliases: &HashMap<String, Vec<String>>,
    ) -> Option<CheckedType> {
        annotation
            .and_then(|annotation| self.checked_type_from_type_ref(module_id, annotation, aliases))
            .or_else(|| self.checked_type(module_id, ty))
    }

    fn checked_type_from_type_ref(
        &self,
        module_id: ModuleId,
        ty: &TypeRef,
        aliases: &HashMap<String, Vec<String>>,
    ) -> Option<CheckedType> {
        self.checked_type_from_resolved_type(module_id, &Type::resolve(ty), aliases)
    }

    fn checked_type_from_resolved_type(
        &self,
        module_id: ModuleId,
        ty: &Type,
        aliases: &HashMap<String, Vec<String>>,
    ) -> Option<CheckedType> {
        match ty {
            Type::Scalar(scalar) => Some(CheckedType::Primitive(*scalar)),
            Type::Identity(identity) => self.store_for_root(identity).map(CheckedType::Identity),
            Type::Named(name) if name == "Error" => Some(CheckedType::Error),
            Type::Named(name) => {
                let segments = split_type_path(name);
                self.resolve_resource_segments(module_id, &segments, aliases)
                    .map(CheckedType::Resource)
            }
            Type::Sequence(element) => self
                .checked_type_from_resolved_type(module_id, element, aliases)
                .map(|element| CheckedType::Sequence(Box::new(element))),
            Type::Unknown => None,
        }
    }

    fn checked_type(&self, module_id: ModuleId, ty: &MarrowType) -> Option<CheckedType> {
        match ty {
            MarrowType::Primitive(scalar) => Some(CheckedType::Primitive(*scalar)),
            MarrowType::Error => Some(CheckedType::Error),
            MarrowType::Resource(name) => self
                .resolve_resource_type(module_id, name)
                .map(CheckedType::Resource),
            MarrowType::GroupEntry { resource, layers } => {
                let resource = self.resolve_resource_type(module_id, resource)?;
                let names: Vec<&str> = layers.iter().map(String::as_str).collect();
                Some(CheckedType::GroupEntry {
                    resource,
                    members: self.member_path_ids(resource, &names)?,
                })
            }
            MarrowType::Identity(root) => self.store_for_root(root).map(CheckedType::Identity),
            MarrowType::Enum { module, name } => {
                let module = self.module_id(module)?;
                self.enum_id(module, name).map(CheckedType::Enum)
            }
            MarrowType::Sequence(element) => self
                .checked_type(module_id, element)
                .map(|element| CheckedType::Sequence(Box::new(element))),
            MarrowType::LocalTree { keys, value } => {
                let keys = keys
                    .iter()
                    .map(|key| self.checked_type(module_id, key))
                    .collect::<Option<Vec<_>>>()?;
                let value = Box::new(self.checked_type(module_id, value)?);
                Some(CheckedType::LocalTree { keys, value })
            }
            MarrowType::Invalid | MarrowType::Unknown => None,
        }
    }

    fn resolve_resource_type(&self, module_id: ModuleId, name: &str) -> Option<ResourceId> {
        let segments = split_type_path(name);
        self.resolve_resource_segments(module_id, &segments, &HashMap::new())
    }

    fn resolve_resource_segments(
        &self,
        module_id: ModuleId,
        segments: &[String],
        aliases: &HashMap<String, Vec<String>>,
    ) -> Option<ResourceId> {
        let expanded = expand_alias(segments, aliases);
        let (resource, module) = expanded.split_last()?;
        if module.is_empty() {
            return self.resource_id(module_id, resource);
        }
        self.module_id(&module.join("::"))
            .and_then(|module_id| self.resource_id(module_id, resource))
    }

    fn resolve_enum_segments(
        &self,
        module_id: ModuleId,
        segments: &[String],
        aliases: &HashMap<String, Vec<String>>,
    ) -> Option<EnumId> {
        let expanded = expand_alias(segments, aliases);
        let (enum_name, module) = expanded.split_last()?;
        if module.is_empty() {
            return self.enum_id(module_id, enum_name);
        }
        self.module_id(&module.join("::"))
            .and_then(|module_id| self.enum_id(module_id, enum_name))
    }

    fn stored_value_meaning(
        &self,
        module_id: ModuleId,
        ty: &Type,
        aliases: &HashMap<String, Vec<String>>,
    ) -> Option<StoredValueMeaning> {
        match ty {
            Type::Scalar(scalar) => Some(StoredValueMeaning::Scalar(*scalar)),
            Type::Identity(identity) => self
                .store_for_root(identity)
                .map(StoredValueMeaning::Identity),
            Type::Named(name) => {
                let segments = split_type_path(name);
                let enum_id = self.resolve_enum_segments(module_id, &segments, aliases)?;
                Some(StoredValueMeaning::Enum {
                    enum_id,
                    members: self.selectable_enum_members(enum_id),
                })
            }
            Type::Sequence(_) | Type::Unknown => None,
        }
    }

    fn selectable_enum_members(&self, enum_id: EnumId) -> Vec<EnumMemberId> {
        self.enum_members
            .iter()
            .filter(|member| {
                member.enum_id == enum_id
                    && !member.category
                    && !self.enum_member_has_children(member.id)
            })
            .map(|member| member.id)
            .collect()
    }

    fn enum_member_has_children(&self, id: EnumMemberId) -> bool {
        self.enum_members
            .iter()
            .any(|member| member.parent == Some(id))
    }

    fn store_index_keys(
        &self,
        module_id: ModuleId,
        resource: ResourceId,
        store: &marrow_schema::StoreSchema,
        resource_schema: &marrow_schema::ResourceSchema,
        index: &marrow_schema::IndexSchema,
        aliases: &HashMap<String, Vec<String>>,
    ) -> Vec<StoreIndexKeyFact> {
        index
            .args
            .iter()
            .filter_map(|arg| {
                self.identity_index_key(module_id, store, arg, aliases)
                    .or_else(|| self.resource_member_index_key(resource, resource_schema, arg))
            })
            .collect()
    }

    fn identity_index_key(
        &self,
        module_id: ModuleId,
        store: &marrow_schema::StoreSchema,
        arg: &str,
        aliases: &HashMap<String, Vec<String>>,
    ) -> Option<StoreIndexKeyFact> {
        let key = store.identity_keys.iter().find(|key| key.name == arg)?;
        Some(StoreIndexKeyFact {
            name: arg.to_string(),
            source: StoreIndexKeySource::IdentityKey,
            value_meaning: self.stored_value_meaning(module_id, &key.ty, aliases)?,
        })
    }

    fn resource_member_index_key(
        &self,
        resource: ResourceId,
        resource_schema: &marrow_schema::ResourceSchema,
        arg: &str,
    ) -> Option<StoreIndexKeyFact> {
        let member = self.resource_member_id(resource, &[arg])?;
        let value_meaning = self.resource_members[member.0 as usize]
            .value_meaning
            .clone()?;
        resource_schema.field_type(&[arg])?;
        Some(StoreIndexKeyFact {
            name: arg.to_string(),
            source: StoreIndexKeySource::ResourceMember(member),
            value_meaning,
        })
    }

    fn store_for_root(&self, root: &str) -> Option<StoreId> {
        self.stores
            .iter()
            .find(|store| store.root == root)
            .map(|store| store.id)
    }

    fn member_path_ids(
        &self,
        resource: ResourceId,
        path: &[&str],
    ) -> Option<Vec<ResourceMemberId>> {
        let mut result = Vec::new();
        let mut parent = None;
        for name in path {
            let member = self.resource_members.iter().find(|member| {
                member.resource == resource && member.parent == parent && member.name == *name
            })?;
            result.push(member.id);
            parent = Some(member.id);
        }
        Some(result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleFact {
    pub id: ModuleId,
    pub name: String,
    pub source_file: PathBuf,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceFact {
    pub id: ResourceId,
    pub module: ModuleId,
    pub name: String,
    pub catalog_id: Option<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreFact {
    pub id: StoreId,
    pub module: ModuleId,
    pub root: String,
    pub resource: ResourceId,
    pub identity_keys: Vec<StoreIdentityKeyFact>,
    pub next_id_shape: String,
    pub catalog_id: Option<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreIdentityKeyFact {
    pub name: String,
    pub value_meaning: Option<StoredValueMeaning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreIndexFact {
    pub id: StoreIndexId,
    pub store: StoreId,
    pub name: String,
    pub unique: bool,
    pub keys: Vec<StoreIndexKeyFact>,
    pub catalog_id: Option<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreIndexKeyFact {
    pub name: String,
    pub source: StoreIndexKeySource,
    pub value_meaning: StoredValueMeaning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreIndexKeySource {
    IdentityKey,
    ResourceMember(ResourceMemberId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceMemberFact {
    pub id: ResourceMemberId,
    pub resource: ResourceId,
    pub parent: Option<ResourceMemberId>,
    pub name: String,
    pub kind: ResourceMemberKind,
    pub value_meaning: Option<StoredValueMeaning>,
    pub catalog_id: Option<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceMemberKind {
    Field,
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumFact {
    pub id: EnumId,
    pub module: ModuleId,
    pub name: String,
    pub catalog_id: Option<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumMemberFact {
    pub id: EnumMemberId,
    pub enum_id: EnumId,
    pub parent: Option<EnumMemberId>,
    pub name: String,
    pub category: bool,
    pub catalog_id: Option<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionFact {
    pub id: FunctionId,
    pub module: ModuleId,
    pub name: String,
    pub public: bool,
    pub params: Vec<LocalFact>,
    pub return_type: Option<CheckedType>,
    pub direct_effects: DirectEffectFacts,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFact {
    pub id: LocalId,
    pub function: FunctionId,
    pub name: String,
    pub mode: Option<CheckedParamMode>,
    pub ty: CheckedType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedType {
    Primitive(ScalarType),
    Error,
    Resource(ResourceId),
    GroupEntry {
        resource: ResourceId,
        members: Vec<ResourceMemberId>,
    },
    Identity(StoreId),
    Enum(EnumId),
    Sequence(Box<CheckedType>),
    LocalTree {
        keys: Vec<CheckedType>,
        value: Box<CheckedType>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredValueMeaning {
    Scalar(ScalarType),
    Identity(StoreId),
    Enum {
        enum_id: EnumId,
        members: Vec<EnumMemberId>,
    },
}

impl StoredValueMeaning {
    /// Decode a stored member value into the order-preserving key an index holds.
    /// This is the one place that turns a member's durable bytes into a [`SavedKey`],
    /// shared by the runtime that writes index entries and the evolution discharge
    /// that derives prospective entries; a single owner keeps the two from drifting. A
    /// scalar decodes by its type, an enum decodes to its member id, and an identity
    /// value is not a stored field, so it has no key.
    pub fn stored_key(&self, bytes: &[u8]) -> Option<SavedKey> {
        match self {
            Self::Scalar(scalar) => decode_value(bytes, *scalar).and_then(|value| value.as_key()),
            Self::Enum { .. } => decode_tree_enum_member(bytes)
                .ok()
                .map(|member| SavedKey::Str(member.member_id().as_str().to_string())),
            Self::Identity(_) => None,
        }
    }
}

/// Effects directly visible in a function body. Calls to user functions are not
/// expanded here; transitive summaries belong to the checked-executable lane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectEffectFacts {
    pub saved_reads: Vec<SavedPlaceEffect>,
    pub saved_writes: Vec<SavedPlaceEffect>,
    pub future_ephemeral_roots: FutureEphemeralRootEffects,
    pub transactions: bool,
    pub host_calls: Vec<HostEffect>,
    pub throws: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FutureEphemeralRootEffects {
    pub reads: Vec<FutureEphemeralRootEffect>,
    pub writes: Vec<FutureEphemeralRootEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FutureEphemeralRootEffect {
    pub root: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPlaceEffect {
    pub resource: ResourceId,
    pub members: Vec<ResourceMemberId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceProofFact {
    pub id: PresenceProofId,
    pub place: PresenceProofPlace,
    pub keys: Vec<String>,
    pub read: PresenceProofRead,
    pub source: PresenceProofSource,
    pub status: PresenceProofStatus,
    pub span: SourceSpan,
}

pub(crate) struct PresenceProofDraft {
    pub(crate) place: PresenceProofPlace,
    pub(crate) keys: Vec<String>,
    pub(crate) read: PresenceProofRead,
    pub(crate) source: PresenceProofSource,
    pub(crate) status: PresenceProofStatus,
    pub(crate) span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PresenceProofId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceProofPlace {
    Saved(SavedPlaceEffect),
    StoreIndex(StoreIndexId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceProofRead {
    Direct,
    Next,
    Prev,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceProofSource {
    Declaration,
    Narrowing,
    AttachedData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceProofStatus {
    Discharged,
    PendingAttachedData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEffect {
    Output,
    Capability(Capability),
}

fn flatten_enum_member_spans(members: &[marrow_syntax::EnumMember], spans: &mut Vec<SourceSpan>) {
    for member in members {
        spans.push(member.span);
        flatten_enum_member_spans(&member.members, spans);
    }
}

fn split_type_path(path: &str) -> Vec<String> {
    path.split("::").map(str::to_string).collect()
}

fn catalog_id(
    ids: &HashMap<CatalogKey, String>,
    kind: marrow_project::CatalogEntryKind,
    path: String,
) -> Option<String> {
    ids.get(&CatalogKey::new(kind, path)).cloned()
}

fn resource_member_name_path(members: &[ResourceMemberFact], id: ResourceMemberId) -> Vec<String> {
    let member = &members[id.0 as usize];
    let mut path = match member.parent {
        Some(parent) => resource_member_name_path(members, parent),
        None => Vec::new(),
    };
    path.push(member.name.clone());
    path
}

fn enum_member_name_path(members: &[EnumMemberFact], id: EnumMemberId) -> Vec<String> {
    let member = &members[id.0 as usize];
    let mut path = match member.parent {
        Some(parent) => enum_member_name_path(members, parent),
        None => Vec::new(),
    };
    path.push(member.name.clone());
    path
}

fn overwrite_prefix<T>(target: &mut [T], prefix: &[T])
where
    T: Clone,
{
    assert!(
        target.len() >= prefix.len(),
        "checked fact prefix longer than combined facts"
    );
    for (target, source) in target.iter_mut().zip(prefix) {
        *target = source.clone();
    }
}
