//! Typed checked facts derived from the best-effort checked program.

use std::collections::HashMap;
use std::path::PathBuf;

use marrow_schema::stdlib::Capability;
use marrow_schema::{NodeKind, ReturnPresence, ScalarType, Type};
use marrow_store::key::{SavedKey, decode_identity_payload_arity, encode_identity_index_key};
use marrow_store::tree::decode_tree_enum_member;
use marrow_store::value::{decode_value, scalar_key_matches_type};
use marrow_syntax::{ParsedSource, ResourceMember, SourceSpan, TypeRef};

use crate::catalog::{
    CatalogKey, DurableRendering, enum_path, resource_member_path, resource_path, store_index_path,
    store_path,
};
use crate::enums::{EnumAnnotationResolution, resolve_enum_annotation_type_for_module};
use crate::executable::CheckedFunctionRef;
use crate::program::{CheckedModule, MarrowType};
use crate::{build_alias_map, expand_alias, split_type_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StoreId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoreIndexId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SurfaceId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceMemberId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumMemberId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalId(pub u32);

/// The resource a store's index-fact collection reads, resolved once per store and threaded
/// in so the collector does not re-resolve it.
struct StoreIndexBinding<'a> {
    store_id: StoreId,
    resource: ResourceId,
    resource_schema: Option<&'a marrow_schema::ResourceSchema>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckedFacts {
    modules: Vec<ModuleFact>,
    functions: Vec<FunctionFact>,
    locals: Vec<LocalFact>,
    resources: Vec<ResourceFact>,
    stores: Vec<StoreFact>,
    store_indexes: Vec<StoreIndexFact>,
    surfaces: Vec<SurfaceFact>,
    resource_members: Vec<ResourceMemberFact>,
    enums: Vec<EnumFact>,
    enum_members: Vec<EnumMemberFact>,
    presence_proofs: Vec<PresenceProofFact>,
    durable_digest_captured_modules: Vec<u32>,
    durable_digest_renderings: Vec<DurableRendering>,
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

        // Resolve each module's id and parsed source once, then drive every collector over the
        // same bindings. Module facts must already exist so the collectors can resolve
        // cross-module references through `module_id`.
        let bindings: Vec<(ModuleId, &CheckedModule, Option<&ParsedSource>)> = modules
            .iter()
            .enumerate()
            .map(|(module_index, module)| {
                let parsed = sources.get(&module.source_file).copied();
                (ModuleId(module_index as u32), module, parsed)
            })
            .collect();

        for &(module_id, module, parsed) in &bindings {
            facts.collect_enum_facts(module_id, module, parsed);
        }
        for &(module_id, module, parsed) in &bindings {
            facts.collect_resource_facts(module_id, module, parsed);
        }
        for &(module_id, module, parsed) in &bindings {
            facts.collect_store_facts(modules, module_id, module, parsed);
        }
        for &(module_id, module, parsed) in &bindings {
            facts.collect_resource_member_facts_for_module(modules, module_id, module, parsed);
        }
        for &(module_id, module, parsed) in &bindings {
            facts.collect_store_index_facts_for_module(modules, module_id, module, parsed);
        }
        for &(module_id, module, parsed) in &bindings {
            for (source_index, function) in module.functions.iter().enumerate() {
                if let Some(function) =
                    facts.function_fact(module_id, module, function, source_index as u32, parsed)
                {
                    facts.functions.push(function);
                }
            }
        }

        facts
    }

    pub fn modules(&self) -> &[ModuleFact] {
        &self.modules
    }

    pub(crate) fn set_durable_digest_renderings(
        &mut self,
        captured_modules: Vec<u32>,
        renderings: Vec<DurableRendering>,
    ) {
        self.durable_digest_captured_modules = captured_modules;
        self.durable_digest_renderings = renderings;
    }

    pub(crate) fn extend_durable_digest_renderings(
        &mut self,
        captured_modules: Vec<u32>,
        renderings: Vec<DurableRendering>,
    ) {
        self.durable_digest_captured_modules
            .extend(captured_modules);
        self.durable_digest_renderings.extend(renderings);
    }

    pub(crate) fn has_captured_durable_digest_renderings_for_module_index(
        &self,
        module_index: u32,
    ) -> bool {
        self.durable_digest_captured_modules.contains(&module_index)
    }

    pub(crate) fn durable_digest_renderings_for_module_index(
        &self,
        module_index: u32,
    ) -> impl Iterator<Item = &DurableRendering> {
        self.durable_digest_renderings
            .iter()
            .filter(move |rendering| rendering.module_index() == module_index)
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

    pub fn store_by_root(&self, root: &str) -> Option<&StoreFact> {
        self.stores.iter().find(|store| store.root == root)
    }

    pub fn store_indexes(&self) -> &[StoreIndexFact] {
        &self.store_indexes
    }

    pub fn store_index(&self, id: StoreIndexId) -> &StoreIndexFact {
        &self.store_indexes[id.0 as usize]
    }

    pub fn surfaces(&self) -> &[SurfaceFact] {
        &self.surfaces
    }

    pub fn surface(&self, id: SurfaceId) -> &SurfaceFact {
        &self.surfaces[id.0 as usize]
    }

    pub(crate) fn set_surfaces(&mut self, surfaces: Vec<SurfaceFact>) {
        self.surfaces = surfaces;
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

    pub fn enum_member_catalog_path(&self, id: EnumMemberId) -> Option<String> {
        let member = self.enum_member(id)?;
        let enum_fact = self.enum_(member.enum_id)?;
        let module = self.modules.get(enum_fact.module.0 as usize)?;
        let path = member_name_path(&self.enum_members, id.0 as usize)?;
        Some(format!(
            "{}::{}",
            enum_path(&module.name, &enum_fact.name),
            path.join("::")
        ))
    }

    /// The `Enum::member` form a member renders as, dropping the module prefix so
    /// the text reads as the value's source spelling rather than its catalog path.
    pub fn enum_member_short_path(&self, id: EnumMemberId) -> Option<String> {
        let member = self.enum_member(id)?;
        let enum_fact = self.enum_(member.enum_id)?;
        let path = member_name_path(&self.enum_members, id.0 as usize)?;
        Some(format!("{}::{}", enum_fact.name, path.join("::")))
    }

    pub fn resource_member_catalog_path(&self, id: ResourceMemberId) -> Option<String> {
        let member = self.resource_members.get(id.0 as usize)?;
        let resource = self.resources.get(member.resource.0 as usize)?;
        let module = self.modules.get(resource.module.0 as usize)?;
        let path = member_name_path(&self.resource_members, id.0 as usize)?;
        Some(resource_member_path(&module.name, &resource.name, &path))
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
        self.enum_member(id).is_some_and(|member| member.selectable)
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
        self.bind_value_meaning_store_catalog_ids();
        self.bind_store_index_catalog_ids(modules, ids);
        self.bind_resource_member_catalog_ids(ids);
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
            resource.catalog_id = catalog_id(ids, marrow_catalog::CatalogEntryKind::Resource, path);
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
            store.catalog_id = catalog_id(ids, marrow_catalog::CatalogEntryKind::Store, path);
        }
    }

    fn bind_value_meaning_store_catalog_ids(&mut self) {
        let store_catalog_ids: Vec<Option<String>> = self
            .stores
            .iter()
            .map(|store| store.catalog_id.clone())
            .collect();
        for store in &mut self.stores {
            for key in &mut store.identity_keys {
                bind_value_meaning_store_catalog_id(key.value_meaning.as_mut(), &store_catalog_ids);
            }
        }
        for member in &mut self.resource_members {
            bind_value_meaning_store_catalog_id(member.value_meaning.as_mut(), &store_catalog_ids);
        }
        for index in &mut self.store_indexes {
            for key in &mut index.keys {
                bind_value_meaning_store_catalog_id(
                    Some(&mut key.value_meaning),
                    &store_catalog_ids,
                );
            }
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
            index.catalog_id = catalog_id(ids, marrow_catalog::CatalogEntryKind::StoreIndex, path);
        }
    }

    fn bind_resource_member_catalog_ids(&mut self, ids: &HashMap<CatalogKey, String>) {
        let resource_member_paths: Vec<Option<String>> = self
            .resource_members
            .iter()
            .map(|member| self.resource_member_catalog_path(member.id))
            .collect();
        for (member, path) in self.resource_members.iter_mut().zip(resource_member_paths) {
            member.catalog_id = path.and_then(|path| {
                catalog_id(ids, marrow_catalog::CatalogEntryKind::ResourceMember, path)
            });
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
            enum_fact.catalog_id = catalog_id(ids, marrow_catalog::CatalogEntryKind::Enum, path);
        }
    }

    fn bind_enum_member_catalog_ids(
        &mut self,
        _modules: &[CheckedModule],
        ids: &HashMap<CatalogKey, String>,
    ) {
        let enum_member_paths: Vec<Option<String>> = self
            .enum_members
            .iter()
            .map(|member| self.enum_member_catalog_path(member.id))
            .collect();
        for (member, path) in self.enum_members.iter_mut().zip(enum_member_paths) {
            member.catalog_id = path.and_then(|path| {
                catalog_id(ids, marrow_catalog::CatalogEntryKind::EnumMember, path)
            });
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
                    .and_then(|module| module.functions.get(fact.source_index as usize))
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
        self.surfaces = prefix.surfaces.clone();
        overwrite_prefix(&mut self.resource_members, &prefix.resource_members);
        overwrite_prefix(&mut self.enums, &prefix.enums);
        overwrite_prefix(&mut self.enum_members, &prefix.enum_members);
        self.durable_digest_captured_modules = prefix.durable_digest_captured_modules.clone();
        self.durable_digest_renderings = prefix.durable_digest_renderings.clone();
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
        self.member_path_ids(resource, path)
            .and_then(|ids| ids.last().copied())
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

    pub fn function_id_for_ref(&self, function_ref: CheckedFunctionRef) -> Option<FunctionId> {
        let module = ModuleId(function_ref.module);
        self.functions
            .iter()
            .find(|function| {
                function.module == module && function.source_index == function_ref.function
            })
            .map(|function| function.id)
    }

    pub fn function_for_ref(&self, function_ref: CheckedFunctionRef) -> Option<&FunctionFact> {
        self.function_id_for_ref(function_ref)
            .map(|function| self.function(function))
    }

    fn function_fact(
        &mut self,
        module_id: ModuleId,
        module: &CheckedModule,
        function: &crate::CheckedFunction,
        source_index: u32,
        parsed: Option<&ParsedSource>,
    ) -> Option<FunctionFact> {
        // The checked functions are built one per function declaration in source
        // order, so the declaration carrying this function's annotations is the
        // one at the same position — not the first declaration of this name, which
        // a by-name lookup would wrongly pick for a duplicate-named function.
        let declaration = parsed.and_then(|parsed| {
            parsed
                .file
                .declarations
                .iter()
                .filter_map(|declaration| match declaration {
                    marrow_syntax::Declaration::Function(function) => Some(function),
                    _ => None,
                })
                .nth(source_index as usize)
        });
        let aliases = build_alias_map(&module.imports);

        let params = function
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let declared = declaration.and_then(|declaration| declaration.params.get(index));
                // A keyed parameter's annotation text names only the leaf value
                // type, so the keyed shape lives in the resolved `MarrowType`; the
                // unkeyed annotation drives the type only for an ordinary parameter.
                let annotation = declared
                    .filter(|declared| declared.keys.is_empty())
                    .map(|declared| &declared.ty);
                let ty =
                    self.checked_type_for_signature(module_id, &param.ty, annotation, &aliases)?;
                Some((param.name.clone(), ty))
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
            .map(|(name, ty)| {
                let local = LocalFact {
                    id: LocalId(self.locals.len() as u32),
                    function: id,
                    name,
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
            return_presence: function.return_presence,
            direct_effects: DirectEffectFacts::default(),
            source_index,
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
                name_span: declaration.map_or(SourceSpan::default(), |resource| resource.name_span),
                span: declaration.map_or(SourceSpan::default(), |resource| resource.span),
            });
        }
    }

    fn collect_store_facts(
        &mut self,
        modules: &[CheckedModule],
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
            let identity_keys = store
                .identity_keys
                .iter()
                .map(|key| StoreIdentityKeyFact {
                    name: key.name.clone(),
                    value_meaning: self.stored_value_meaning(modules, module, &key.ty),
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
                name_span: declaration.map_or(SourceSpan::default(), |store| store.root.span),
                span: declaration.map_or(SourceSpan::default(), |store| store.span),
            });
        }
    }

    fn collect_resource_member_facts_for_module(
        &mut self,
        modules: &[CheckedModule],
        module_id: ModuleId,
        module: &CheckedModule,
        parsed: Option<&ParsedSource>,
    ) {
        for resource in &module.resources {
            let Some(resource_id) = self.resource_id(module_id, &resource.name) else {
                continue;
            };
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
            self.collect_resource_member_facts(
                modules,
                module,
                resource_id,
                None,
                &resource.members,
                declaration.map(|resource| resource.members.as_slice()),
            );
        }
    }

    fn collect_store_index_facts_for_module(
        &mut self,
        modules: &[CheckedModule],
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
            let Some(store_id) = self.store_id(module_id, &store.root) else {
                continue;
            };
            let Some(resource) = self.resource_id(module_id, &store.resource) else {
                continue;
            };
            let resource_schema = module
                .resources
                .iter()
                .find(|candidate| candidate.name == store.resource);
            let index_binding = StoreIndexBinding {
                store_id,
                resource,
                resource_schema,
            };
            self.collect_store_index_facts(modules, module, index_binding, store, declaration);
        }
    }

    fn collect_store_index_facts(
        &mut self,
        modules: &[CheckedModule],
        module: &CheckedModule,
        binding: StoreIndexBinding<'_>,
        store: &marrow_schema::StoreSchema,
        declaration: Option<&marrow_syntax::StoreDecl>,
    ) {
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
            let name_span = declaration
                .and_then(|store| {
                    store
                        .indexes
                        .iter()
                        .find(|candidate| candidate.name == index.name)
                        .map(|candidate| candidate.name_span)
                })
                .unwrap_or_default();
            let id = StoreIndexId(self.store_indexes.len() as u32);
            let keys = binding
                .resource_schema
                .map(|resource_schema| {
                    self.store_index_keys(
                        modules,
                        module,
                        binding.resource,
                        store,
                        resource_schema,
                        index,
                    )
                })
                .unwrap_or_default();
            self.store_indexes.push(StoreIndexFact {
                id,
                store: binding.store_id,
                name: index.name.clone(),
                unique: index.unique,
                declared_key_count: index.args.len(),
                keys,
                catalog_id: None,
                name_span,
                span,
            });
        }
    }

    fn collect_resource_member_facts(
        &mut self,
        modules: &[CheckedModule],
        module: &CheckedModule,
        resource_id: ResourceId,
        parent: Option<ResourceMemberId>,
        nodes: &[marrow_schema::Node],
        declarations: Option<&[ResourceMember]>,
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
            let name_span = declaration
                .map(|member| match member {
                    ResourceMember::Field(field) => field.name_span,
                    ResourceMember::Group(group) => group.name_span,
                })
                .unwrap_or_default();
            let kind = match node.kind {
                NodeKind::Slot { .. } => ResourceMemberKind::Field,
                NodeKind::Group => ResourceMemberKind::Group,
            };
            let plain_field_required = match &node.kind {
                NodeKind::Slot { required, .. } if node.key_params.is_empty() => Some(*required),
                _ => None,
            };
            let value_meaning = match &node.kind {
                NodeKind::Slot { ty, .. } => self.stored_value_meaning(modules, module, ty),
                NodeKind::Group => None,
            };
            let id = ResourceMemberId(self.resource_members.len() as u32);
            self.resource_members.push(ResourceMemberFact {
                id,
                resource: resource_id,
                parent,
                name: node.name.clone(),
                kind,
                key_count: node.key_params.len(),
                plain_field_required,
                value_meaning,
                catalog_id: None,
                name_span,
                span,
            });
            let nested = declaration.and_then(|member| match member {
                ResourceMember::Group(group) => Some(group.members.as_slice()),
                _ => None,
            });
            self.collect_resource_member_facts(
                modules,
                module,
                resource_id,
                Some(id),
                &node.members,
                nested,
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
                name_span: declaration.map_or(SourceSpan::default(), |decl| decl.name_span),
                span: declaration.map_or(SourceSpan::default(), |decl| decl.span),
            });
            self.collect_enum_member_facts(enum_id, enum_schema, declaration);
        }
    }

    fn collect_enum_member_facts(
        &mut self,
        enum_id: EnumId,
        enum_schema: &marrow_schema::EnumSchema,
        declaration: Option<&marrow_syntax::EnumDecl>,
    ) {
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
                selectable: enum_schema.is_selectable_leaf(index),
                catalog_id: None,
                name_span: member_spans
                    .get(index)
                    .map(|spans| spans.0)
                    .unwrap_or_else(SourceSpan::default),
                span: member_spans
                    .get(index)
                    .map(|spans| spans.1)
                    .unwrap_or_else(SourceSpan::default),
            });
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
        let (module, name) = self.resolve_named_module(module_id, segments, aliases)?;
        self.resource_id(module, &name)
    }

    /// Expand a namespace path through import aliases and resolve its module prefix,
    /// returning the owning module and the terminal name. An empty prefix means the
    /// name lives in `module_id`; a non-empty prefix must resolve to a known module.
    fn resolve_named_module(
        &self,
        module_id: ModuleId,
        segments: &[String],
        aliases: &HashMap<String, Vec<String>>,
    ) -> Option<(ModuleId, String)> {
        let expanded = expand_alias(segments, aliases);
        let (name, module) = expanded.split_last()?;
        let module = if module.is_empty() {
            module_id
        } else {
            self.module_id(&module.join("::"))?
        };
        Some((module, name.clone()))
    }

    fn stored_value_meaning(
        &self,
        modules: &[CheckedModule],
        module: &CheckedModule,
        ty: &Type,
    ) -> Option<StoredValueMeaning> {
        match ty {
            Type::Scalar(scalar) => Some(StoredValueMeaning::Scalar(*scalar)),
            // Schema validation rejects an `Identity`-typed store identity key as a
            // non-scalar key, so a store can never name another store as one of its
            // own identity keys. This branch therefore resolves only for stored
            // members and index arguments, where the referent store already exists,
            // and the forward-pass store collection order cannot strand the meaning.
            Type::Identity(identity) => {
                let store = self.store_for_root(identity)?;
                let key_scalars = self.store_identity_key_scalars(store)?;
                Some(StoredValueMeaning::Identity {
                    store,
                    root: identity.clone(),
                    store_catalog_id: self.stores[store.0 as usize].catalog_id.clone(),
                    arity: self.stores[store.0 as usize].identity_keys.len(),
                    key_scalars,
                })
            }
            Type::Named(name) => {
                let resolved = match resolve_enum_annotation_type_for_module(
                    &Type::Named(name.clone()),
                    modules,
                    module,
                ) {
                    EnumAnnotationResolution::Visible(resolved) => resolved,
                    EnumAnnotationResolution::Private(_)
                    | EnumAnnotationResolution::AmbiguousBareForeign(_)
                    | EnumAnnotationResolution::MissingOrNonEnum => return None,
                };
                let enum_module = self.module_id(&resolved.module)?;
                let enum_id = self.enum_id(enum_module, &resolved.name)?;
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
            .filter(|member| member.enum_id == enum_id && member.selectable)
            .map(|member| member.id)
            .collect()
    }

    fn store_index_keys(
        &self,
        modules: &[CheckedModule],
        module: &CheckedModule,
        resource: ResourceId,
        store: &marrow_schema::StoreSchema,
        resource_schema: &marrow_schema::ResourceSchema,
        index: &marrow_schema::IndexSchema,
    ) -> Vec<StoreIndexKeyFact> {
        index
            .args
            .iter()
            .filter_map(|arg| {
                self.identity_index_key(modules, module, store, arg)
                    .or_else(|| self.resource_member_index_key(resource, resource_schema, arg))
            })
            .collect()
    }

    fn identity_index_key(
        &self,
        modules: &[CheckedModule],
        module: &CheckedModule,
        store: &marrow_schema::StoreSchema,
        arg: &str,
    ) -> Option<StoreIndexKeyFact> {
        let key = store.identity_keys.iter().find(|key| key.name == arg)?;
        Some(StoreIndexKeyFact {
            name: arg.to_string(),
            source: StoreIndexKeySource::IdentityKey,
            value_meaning: self.stored_value_meaning(modules, module, &key.ty)?,
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
        self.store_by_root(root).map(|store| store.id)
    }

    fn store_identity_key_scalars(&self, store: StoreId) -> Option<Vec<ScalarType>> {
        self.stores
            .get(store.0 as usize)?
            .identity_keys
            .iter()
            .map(|key| key.value_meaning.as_ref()?.scalar())
            .collect()
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
    pub name_span: SourceSpan,
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
    pub name_span: SourceSpan,
    pub span: SourceSpan,
}

impl StoreFact {
    pub fn identity_keys_match(&self, keys: &[SavedKey]) -> bool {
        if self.identity_keys.len() != keys.len() {
            return false;
        }
        self.identity_keys.iter().zip(keys).all(|(expected, key)| {
            matches!(
                expected.value_meaning,
                Some(StoredValueMeaning::Scalar(scalar)) if scalar_key_matches_type(key, scalar)
            )
        })
    }
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
    pub declared_key_count: usize,
    pub keys: Vec<StoreIndexKeyFact>,
    pub catalog_id: Option<String>,
    pub name_span: SourceSpan,
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
pub struct SurfaceFact {
    pub id: SurfaceId,
    pub module: ModuleId,
    pub name: String,
    pub store: StoreId,
    pub fields: Vec<SurfaceFieldFact>,
    pub create: Vec<SurfaceFieldFact>,
    pub update: Vec<SurfaceFieldFact>,
    pub delete: Option<SurfaceDeleteFact>,
    pub collections: Vec<SurfaceCollectionFact>,
    pub actions: Vec<SurfaceActionFact>,
    pub computed_reads: Vec<SurfaceComputedReadFact>,
    pub read_operations: Vec<SurfaceReadOperationFact>,
    pub catalog_status: SurfaceCatalogStatus,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceCatalogStatus {
    Stable,
    /// A source-only surface always names at least one blocker, so consumers can
    /// explain why generated operations are not part of the stable ABI yet.
    SourceOnly(Vec<SurfaceCatalogBlocker>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceCatalogBlocker {
    PendingCatalogProposal,
    MissingAcceptedCatalogIds,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceFieldFact {
    pub name: String,
    pub member: ResourceMemberId,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceCollectionFact {
    pub alias: String,
    pub target: SurfaceCollectionTarget,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceActionFact {
    pub alias: String,
    pub function: CheckedFunctionRef,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceComputedReadFact {
    pub alias: String,
    pub path: String,
    pub function: CheckedFunctionRef,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceDeleteFact {
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceCollectionTarget {
    StoreRoot(StoreId),
    StoreIndex(StoreIndexId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceReadOperationFact {
    pub alias: String,
    pub kind: SurfaceReadOperationKind,
    pub footprint: SurfaceReadFootprint,
    pub projection: Vec<ResourceMemberId>,
    pub operation_tag: Option<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceReadOperationKind {
    SingletonRead {
        store: StoreId,
    },
    PointRead {
        store: StoreId,
    },
    PagedRootCollection {
        store: StoreId,
    },
    PagedIndexCollection {
        index: StoreIndexId,
        exact_key_count: usize,
        identity_key_count: usize,
    },
    UniqueIndexLookup {
        index: StoreIndexId,
        key_count: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceReadFootprint {
    FullRecord { resource: ResourceId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceMemberFact {
    pub id: ResourceMemberId,
    pub resource: ResourceId,
    pub parent: Option<ResourceMemberId>,
    pub name: String,
    pub kind: ResourceMemberKind,
    pub key_count: usize,
    pub plain_field_required: Option<bool>,
    pub value_meaning: Option<StoredValueMeaning>,
    pub catalog_id: Option<String>,
    pub name_span: SourceSpan,
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
    pub name_span: SourceSpan,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumMemberFact {
    pub id: EnumMemberId,
    pub enum_id: EnumId,
    pub parent: Option<EnumMemberId>,
    pub name: String,
    /// Whether this member is selectable as a value, as the [`marrow_schema::EnumSchema`]
    /// owner decides it: a concrete leaf with no children and no `category` marker. The fact
    /// records the schema's verdict so the selectability rule has one owner.
    pub selectable: bool,
    pub catalog_id: Option<String>,
    pub name_span: SourceSpan,
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
    pub return_presence: ReturnPresence,
    pub direct_effects: DirectEffectFacts,
    /// Position of the source function in its module's `functions`. A fact is
    /// built only when its signature resolves, so the facts are a subset of the
    /// module's functions; this stable index maps each fact back to its body
    /// without a by-name lookup, which would mis-attribute under a duplicate name.
    pub source_index: u32,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFact {
    pub id: LocalId,
    pub function: FunctionId,
    pub name: String,
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
    Identity {
        store: StoreId,
        root: String,
        store_catalog_id: Option<String>,
        arity: usize,
        key_scalars: Vec<ScalarType>,
    },
    Enum {
        enum_id: EnumId,
        members: Vec<EnumMemberId>,
    },
}

impl StoredValueMeaning {
    /// The scalar a value carries when its meaning is a plain scalar, and `None` for an
    /// identity or enum meaning. Callers that read an index or identity key column by its
    /// scalar type share this one extraction rather than re-matching the variant.
    pub fn scalar(&self) -> Option<ScalarType> {
        match self {
            Self::Scalar(scalar) => Some(*scalar),
            Self::Identity { .. } | Self::Enum { .. } => None,
        }
    }

    /// Decode a stored member value into the order-preserving key an index holds.
    /// This is the one place that turns a member's durable bytes into a [`SavedKey`],
    /// shared by the runtime that writes index entries and the evolution discharge
    /// that derives prospective entries; a single owner keeps the two from drifting. A
    /// scalar decodes by its type, an enum decodes to its member id, and an identity
    /// decodes to a store-prefixed canonical identity component.
    pub fn stored_key(&self, bytes: &[u8]) -> Option<SavedKey> {
        match self {
            Self::Scalar(scalar) => {
                decode_value(bytes, *scalar).and_then(|value| value.as_key().ok().flatten())
            }
            Self::Enum { .. } => decode_tree_enum_member(bytes)
                .ok()
                .map(|member| SavedKey::Str(member.member_id().as_str().to_string())),
            Self::Identity {
                store_catalog_id,
                arity,
                key_scalars,
                ..
            } => {
                let store_catalog_id = store_catalog_id.as_deref()?;
                let keys = decode_identity_payload_arity(bytes, *arity)?;
                if keys.len() != key_scalars.len()
                    || !keys
                        .iter()
                        .zip(key_scalars)
                        .all(|(key, scalar)| scalar_key_matches_type(key, *scalar))
                {
                    return None;
                }
                Some(SavedKey::Bytes(encode_identity_index_key(
                    store_catalog_id,
                    &keys,
                )))
            }
        }
    }
}

fn bind_value_meaning_store_catalog_id(
    meaning: Option<&mut StoredValueMeaning>,
    store_catalog_ids: &[Option<String>],
) {
    let Some(StoredValueMeaning::Identity {
        store,
        store_catalog_id,
        ..
    }) = meaning
    else {
        return;
    };
    *store_catalog_id = store_catalog_ids
        .get(store.0 as usize)
        .and_then(|catalog_id| catalog_id.clone());
}

/// Effects directly visible in a function body. Calls to user functions are not
/// expanded here; this summary is intentionally local to the function body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectEffectFacts {
    pub saved_reads: Vec<SavedPlaceEffect>,
    pub store_reads: Vec<StoreId>,
    /// Index branches read saved data but do not name a resource-member path.
    pub saved_index_reads: Vec<StoreIndexId>,
    pub saved_writes: Vec<SavedPlaceEffect>,
    pub store_writes: Vec<StoreId>,
    pub saved_index_writes: Vec<StoreIndexId>,
    pub transactions: bool,
    pub host_calls: Vec<HostEffect>,
    pub unindexed_collection_reads: bool,
    pub throws: bool,
    /// User-defined callees named directly by this body. Callee effects are not expanded into the
    /// direct summary, so callers that require a self-contained body read this list instead of
    /// re-walking source or resolving by name.
    pub user_function_calls: Vec<CheckedFunctionRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPlaceEffect {
    pub resource: ResourceId,
    pub members: Vec<ResourceMemberId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectClosureFacts {
    pub saved_reads: Vec<SavedPlaceEffect>,
    pub stores_read: Vec<StoreId>,
    pub saved_index_reads: Vec<StoreIndexId>,
    pub saved_writes: Vec<SavedPlaceEffect>,
    pub stores_written: Vec<StoreId>,
    pub saved_index_writes: Vec<StoreIndexId>,
    pub indexes_touched: Vec<StoreIndexId>,
    pub transactions: bool,
    pub host_calls: Vec<HostEffect>,
    pub unindexed_collection_reads: bool,
    pub throws: bool,
    pub write_effects_reachable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryFootprintFact {
    pub function: FunctionId,
    pub entry: String,
    pub write_effects_reachable: bool,
    pub stores_read: Vec<StoreId>,
    pub stores_written: Vec<StoreId>,
    pub indexes_touched: Vec<StoreIndexId>,
    pub work_shape: WorkShapeClass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryCostShapeFact {
    pub function: FunctionId,
    pub entry: String,
    pub work_shape: WorkShapeClass,
    pub point_reads: usize,
    pub range_scans: usize,
    pub writes: usize,
    pub index_entry_touches: usize,
    pub commit_points: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryRunFacts {
    pub footprint: EntryFootprintFact,
    pub cost_shape: EntryCostShapeFact,
    pub store_open_mode: EntryStoreOpenMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkShapeClass {
    ComputeOnly,
    ReadOnly,
    WritesSavedData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryStoreOpenMode {
    ReadOnly,
    WriteCapable,
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

fn flatten_enum_member_spans(
    members: &[marrow_syntax::EnumMember],
    spans: &mut Vec<(SourceSpan, SourceSpan)>,
) {
    for member in members {
        spans.push((member.name_span, member.span));
        flatten_enum_member_spans(&member.members, spans);
    }
}

fn catalog_id(
    ids: &HashMap<CatalogKey, String>,
    kind: marrow_catalog::CatalogEntryKind,
    path: String,
) -> Option<String> {
    ids.get(&CatalogKey::new(kind, path)).cloned()
}

/// A nested fact addressable by index whose ancestry forms a name path. Resource
/// members and enum members share this parent-chain shape over distinct id types.
trait MemberNode {
    fn parent_index(&self) -> Option<usize>;
    fn name(&self) -> &str;
}

impl MemberNode for ResourceMemberFact {
    fn parent_index(&self) -> Option<usize> {
        self.parent.map(|parent| parent.0 as usize)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl MemberNode for EnumMemberFact {
    fn parent_index(&self) -> Option<usize> {
        self.parent.map(|parent| parent.0 as usize)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

fn member_name_path<M: MemberNode>(members: &[M], index: usize) -> Option<Vec<String>> {
    let member = members.get(index)?;
    let mut path = match member.parent_index() {
        Some(parent) => member_name_path(members, parent)?,
        None => Vec::new(),
    };
    path.push(member.name().to_string());
    Some(path)
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
