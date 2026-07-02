use std::collections::{BTreeMap, BTreeSet};

use super::client_ts::CreateFieldPlan;
use super::{
    SurfaceAbiJson, SurfaceCallableParameterJson, SurfaceCallableParameterPresenceJson,
    SurfaceComputedReadPresenceJson, SurfaceCreateOperationDescriptorJson, SurfaceDescriptorJson,
    SurfaceOperationIdentityKeyJson, SurfaceOperationValueShapeJson,
    SurfaceReadOperationDescriptorJson, SurfaceReadOperationKindJson,
    SurfaceReadProjectionFieldJson, SurfaceRouteBinding, SurfaceRouteBindings,
};

/// The scalar leaf kinds a surface value can carry, parsed once from the descriptor's source
/// spelling so the renderer matches a typed variant instead of re-inspecting a raw string. The set
/// mirrors the language's scalar types; an unrecognized spelling is a checker/DTO contract break.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScalarKind {
    Int,
    Bool,
    String,
    Decimal,
    Date,
    Instant,
    Duration,
    Bytes,
}

impl ScalarKind {
    fn parse(scalar: &str) -> Self {
        match scalar {
            "int" => Self::Int,
            "bool" => Self::Bool,
            "string" => Self::String,
            "decimal" => Self::Decimal,
            "date" => Self::Date,
            "instant" => Self::Instant,
            "duration" => Self::Duration,
            "bytes" => Self::Bytes,
            other => panic!("surface descriptor carries an unknown scalar kind `{other}`"),
        }
    }

    /// The canonical wire `kind` tag for this scalar, identical to its source spelling.
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Bool => "bool",
            Self::String => "string",
            Self::Decimal => "decimal",
            Self::Date => "date",
            Self::Instant => "instant",
            Self::Duration => "duration",
            Self::Bytes => "bytes",
        }
    }
}

/// The shape a generated TypeScript record/argument field decodes to or encodes from. This is the
/// single owner of "what TS type and decode/encode strategy a surface value uses"; the renderer
/// reads it, never re-deriving the classification from raw descriptor JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SurfaceFieldType {
    Scalar(ScalarKind),
    Enum {
        type_name: String,
        enum_catalog_const: String,
        encode_table: String,
    },
    Identity {
        brand: String,
        decode_fn: String,
    },
    Sequence(Box<SurfaceFieldType>),
    Resource {
        type_name: String,
        decode_fn: String,
    },
}

#[derive(Debug, Clone)]
pub(super) struct SurfaceEnumModel {
    pub type_name: String,
    pub enum_catalog_id: String,
    pub enum_catalog_const: String,
    pub member_table_const: String,
    pub encode_table_const: String,
    pub members: Vec<SurfaceEnumMember>,
}

#[derive(Debug, Clone)]
pub(super) struct SurfaceEnumMember {
    pub label: String,
    pub member_catalog_id: String,
}

#[derive(Debug, Clone)]
pub(super) struct SurfaceClientStore {
    pub store_catalog_id: String,
    pub catalog_const: String,
    pub brand_type: String,
    pub constructor: String,
    pub decode_fn: String,
    pub key_scalars: Vec<ScalarKind>,
}

/// How a generated record type is consumed, which fixes whether and how a decoder is emitted for it.
/// A surface read record decodes from the read envelope (and reads its identity-derived `id` only
/// when the backing store is keyed); a resource record decodes from a computed-read resource value;
/// a create/update body is input-only and has no decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecordDecode {
    SurfaceRead,
    ResourceValue,
    InputOnly,
}

#[derive(Debug, Clone)]
pub(super) struct SurfaceClientRecord {
    pub type_name: String,
    pub fields: Vec<SurfaceRecordField>,
    pub decode: RecordDecode,
}

#[derive(Debug, Clone)]
pub(super) struct SurfaceRecordField {
    pub label: String,
    pub required: bool,
    /// Whether the field is an optional body key (`label?: T`). Sparse update bodies set this so a
    /// caller can omit any field; read records and exact-body create records leave it false.
    pub optional: bool,
    pub ty: SurfaceFieldType,
    /// The stable catalog id this field decodes against: the projection/resource member id. The
    /// synthetic identity `id` field carries no member id; it decodes from the record identity.
    pub member_catalog_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct SurfaceClientSurface {
    pub name: String,
    pub records: Vec<SurfaceClientRecord>,
    pub enums: Vec<String>,
    pub methods: Vec<SurfaceMethod>,
    /// The opaque page-cursor brand to declare for this surface, present only when the surface owns
    /// a paged read. Page method signatures reference it, so it must be emitted exactly once here.
    pub page_cursor_brand: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct SurfaceMethod {
    pub name: String,
    pub operation_tag: String,
    pub result_kind: &'static str,
    pub cursor_brand: String,
    pub input: SurfaceMethodInput,
    pub result: SurfaceMethodResult,
}

#[derive(Debug, Clone)]
pub(super) struct SurfaceMethodParam {
    pub name: String,
    pub ty: SurfaceFieldType,
}

/// An action or computed-read callable argument. `optional` carries the parameter's `T?` presence
/// so the renderer types it nullable and encodes an absent value as JSON `null`.
#[derive(Debug, Clone)]
pub(super) struct SurfaceCallableArg {
    pub name: String,
    pub ty: SurfaceFieldType,
    pub optional: bool,
}

#[derive(Debug, Clone)]
pub(super) enum SurfaceMethodInput {
    None,
    Identity {
        brand: String,
    },
    Delete {
        brand: String,
    },
    SingletonDelete,
    Create {
        brand: String,
        body_type: String,
        fields: Vec<CreateFieldPlan>,
    },
    SingletonCreate {
        body_type: String,
        fields: Vec<CreateFieldPlan>,
    },
    Update {
        brand: String,
        body_type: String,
        fields: Vec<CreateFieldPlan>,
    },
    SingletonUpdate {
        body_type: String,
        fields: Vec<CreateFieldPlan>,
    },
    Page {
        exact_keys: Vec<SurfaceFieldType>,
    },
    PageIterator {
        exact_keys: Vec<SurfaceMethodParam>,
    },
    /// A ranged index page: the exact keys that precede the ranged key, plus the ranged key's own
    /// type so the caller can pass a typed lower/upper bound over it.
    RangePage {
        exact_keys: Vec<SurfaceFieldType>,
        range_key: SurfaceFieldType,
    },
    /// A ranged index page iterator: the exact keys that precede the ranged key, plus the ranged
    /// key's own type so the caller can pass a typed lower/upper bound over it.
    RangePageIterator {
        exact_keys: Vec<SurfaceMethodParam>,
        range_key: SurfaceFieldType,
    },
    UniqueLookup {
        keys: Vec<SurfaceFieldType>,
    },
    Callable {
        params: Vec<SurfaceCallableArg>,
    },
}

#[derive(Debug, Clone)]
pub(super) enum SurfaceMethodResult {
    Record {
        record: String,
    },
    OptionalRecord {
        record: String,
    },
    Page {
        record: String,
    },
    PageIterator {
        record: String,
    },
    Created {
        record: String,
    },
    Updated,
    Deleted,
    Action {
        value: Option<SurfaceFieldType>,
    },
    ComputedRead {
        value: Option<SurfaceFieldType>,
        /// Whether the result is `T?`, so the renderer types it nullable and decodes an absent
        /// (`null`) result as `null` rather than a missing-value error.
        optional: bool,
    },
}

#[derive(Debug, Clone)]
pub(super) struct SurfaceOperationRoute {
    pub operation_tag: String,
    pub request_kind: &'static str,
    pub route_prefix: &'static str,
}

#[derive(Debug, Clone)]
pub(super) struct SurfaceClientModel {
    pub stores: Vec<SurfaceClientStore>,
    pub enums: Vec<SurfaceEnumModel>,
    pub resources: Vec<SurfaceClientRecord>,
    pub surfaces: Vec<SurfaceClientSurface>,
    pub routes: Vec<SurfaceOperationRoute>,
}

impl SurfaceClientModel {
    pub fn build(abi: &SurfaceAbiJson, bindings: &SurfaceRouteBindings) -> Self {
        let mut builder = ModelBuilder::default();
        builder.assign_store_names(abi);
        for surface in &abi.surfaces {
            builder.add_surface(surface, bindings);
        }
        let mut model = builder.finish();
        model.routes = bindings
            .iter()
            .map(|binding| SurfaceOperationRoute {
                operation_tag: binding.operation_tag.clone(),
                request_kind: binding.kind.operation_request_kind(),
                route_prefix: binding.kind.route_prefix(),
            })
            .collect();
        model
            .routes
            .sort_by(|left, right| left.operation_tag.cmp(&right.operation_tag));
        model
    }
}

#[derive(Default)]
struct ModelBuilder {
    /// A store's user-facing brand base (e.g. `Books` for the `Books` surface's store), assigned
    /// once from the surface that owns each store so identities read as `BooksId`, not a catalog id.
    store_names: BTreeMap<String, String>,
    used_names: std::collections::BTreeSet<String>,
    stores: BTreeMap<String, SurfaceClientStore>,
    enums: BTreeMap<String, SurfaceEnumModel>,
    resources: BTreeMap<String, SurfaceClientRecord>,
    /// The user-facing TypeScript type name chosen for each computed-read resource catalog id, so a
    /// resource reused across computed reads keeps one uniquified name instead of re-uniquifying.
    resource_names: BTreeMap<String, String>,
    surfaces: Vec<SurfaceClientSurface>,
}

impl ModelBuilder {
    /// Give every store its owning surface's name as a brand base before building methods, so a
    /// point identity decodes to `BooksId` rather than an opaque catalog suffix. A store reached
    /// only through an identity field, with no surface of its own, keeps a catalog-derived base.
    fn assign_store_names(&mut self, abi: &SurfaceAbiJson) {
        for surface in &abi.surfaces {
            let owner = surface
                .read
                .first()
                .map(|read| &read.store_catalog_id)
                .or(surface
                    .create
                    .as_ref()
                    .map(|create| &create.store_catalog_id))
                .or(surface
                    .update
                    .as_ref()
                    .map(|update| &update.store_catalog_id))
                .or(surface
                    .delete
                    .as_ref()
                    .map(|delete| &delete.store_catalog_id));
            if let Some(store_catalog_id) = owner {
                if self.store_names.contains_key(store_catalog_id) {
                    continue;
                }
                let base = self.unique_brand_base(&sanitize_type(&surface.name));
                self.store_names.insert(store_catalog_id.clone(), base);
            }
        }
    }

    fn unique_brand_base(&mut self, base: &str) -> String {
        let mut candidate = base.to_string();
        let mut counter = 2usize;
        while !self.used_names.insert(candidate.clone()) {
            candidate = format!("{base}{counter}");
            counter += 1;
        }
        candidate
    }

    /// Pick the user-facing TypeScript type name for an enum or computed-read resource: the sanitized
    /// source name when it is a usable identifier, the catalog-derived `fallback` otherwise. Either way
    /// the result is uniquified against every other emitted type, so a name clash across surfaces never
    /// produces a duplicate declaration. The catalog id stays a private const regardless.
    fn unique_type_name(&mut self, render_name: &str, fallback: &str) -> String {
        let sanitized = sanitize_type(render_name);
        let base = if sanitized == "_" {
            fallback
        } else {
            &sanitized
        };
        let mut candidate = base.to_string();
        let mut counter = 2usize;
        while !self.used_names.insert(candidate.clone()) {
            candidate = format!("{base}{counter}");
            counter += 1;
        }
        candidate
    }

    fn brand_base(&self, store_catalog_id: &str) -> String {
        self.store_names
            .get(store_catalog_id)
            .cloned()
            .unwrap_or_else(|| format!("Ref_{}", sanitize_catalog(store_catalog_id)))
    }

    /// Claim a brand base for a store reached only through an identity field. A store that owns a
    /// surface already holds its surface name (assigned up front), so this only fires for a target
    /// with no surface of its own: it brands the reference after the store's source name, keeping
    /// the catalog-id hash out of every user-facing symbol. The base is uniquified against all other
    /// emitted names, so two references to differently-pathed stores that share a leaf name stay
    /// collision-free.
    fn ensure_relation_brand(&mut self, store_catalog_id: &str, store_name: &str) {
        if self.store_names.contains_key(store_catalog_id) {
            return;
        }
        let base = self.unique_brand_base(&upper_first(&sanitize_type(store_name)));
        self.store_names.insert(store_catalog_id.to_string(), base);
    }

    fn finish(self) -> SurfaceClientModel {
        SurfaceClientModel {
            stores: self.stores.into_values().collect(),
            enums: self.enums.into_values().collect(),
            resources: self.resources.into_values().collect(),
            surfaces: self.surfaces,
            routes: Vec::new(),
        }
    }

    fn add_surface(&mut self, surface: &SurfaceDescriptorJson, bindings: &SurfaceRouteBindings) {
        let surface_name = sanitize_type(&surface.name);
        let record_type = format!("{surface_name}Record");
        let mut records = Vec::new();
        let mut enums = Vec::new();
        let mut methods = Vec::new();

        let projection_owner = surface
            .read
            .iter()
            .map(|read| {
                (
                    &read.store_catalog_id,
                    &read.identity_keys,
                    &read.projection,
                )
            })
            .chain(surface.create.iter().map(|create| {
                (
                    &create.store_catalog_id,
                    &create.identity_keys,
                    &create.projection,
                )
            }))
            .next();
        if let Some((store_catalog_id, identity_keys, projection)) = projection_owner {
            self.register_store(store_catalog_id, identity_keys);
            let fields =
                self.record_fields(store_catalog_id, identity_keys, projection, &mut enums);
            records.push(SurfaceClientRecord {
                type_name: record_type.clone(),
                fields,
                decode: RecordDecode::SurfaceRead,
            });
        }

        for read in &surface.read {
            self.register_store(&read.store_catalog_id, &read.identity_keys);
            let brand = self.store_brand(&read.store_catalog_id);
            let cursor_brand = format!("{surface_name}Cursor");
            let alias = method_alias(bindings, &read.operation_tag, &read.alias);
            let method = self.read_method(
                read,
                &record_type,
                &brand,
                &cursor_brand,
                &alias,
                &mut enums,
            );
            if let Some(helper) =
                self.page_iteration_method(read, &record_type, &cursor_brand, &alias, &mut enums)
            {
                methods.push(helper);
            }
            methods.push(method);
        }
        if let Some(create) = &surface.create {
            let (method, body) =
                self.create_method(create, &surface_name, &record_type, &mut enums);
            records.push(body);
            methods.push(method);
        }
        if let Some(update) = &surface.update {
            let (method, body) = self.update_method(update, &surface_name, &mut enums);
            records.push(body);
            methods.push(method);
        }
        if let Some(delete) = &surface.delete {
            self.register_store(&delete.store_catalog_id, &delete.identity_keys);
            let input = if matches!(
                delete.kind,
                super::SurfaceDeleteOperationKindJson::SingletonDelete
            ) {
                SurfaceMethodInput::SingletonDelete
            } else {
                SurfaceMethodInput::Delete {
                    brand: self.store_brand(&delete.store_catalog_id),
                }
            };
            methods.push(SurfaceMethod {
                name: "delete".into(),
                operation_tag: delete.operation_tag.clone(),
                result_kind: "deleted",
                cursor_brand: format!("{surface_name}Cursor"),
                input,
                result: SurfaceMethodResult::Deleted,
            });
        }
        for action in &surface.actions {
            let params = self.callable_args(&action.parameters, &mut enums);
            let value = action
                .return_value
                .as_ref()
                .map(|shape| self.callable_value_type(shape, &mut enums));
            let alias = method_alias(bindings, &action.operation_tag, &action.alias);
            methods.push(SurfaceMethod {
                name: alias,
                operation_tag: action.operation_tag.clone(),
                result_kind: "action",
                cursor_brand: format!("{surface_name}Cursor"),
                input: SurfaceMethodInput::Callable { params },
                result: SurfaceMethodResult::Action { value },
            });
        }
        for computed in &surface.computed_reads {
            let params = self.callable_args(&computed.callable.parameters, &mut enums);
            let value = computed
                .callable
                .result
                .value
                .as_ref()
                .map(|shape| self.computed_value_type(shape, &mut enums));
            let optional = matches!(
                computed.callable.result.presence,
                SurfaceComputedReadPresenceJson::MaybePresent
            );
            let alias = method_alias(bindings, &computed.operation_tag, &computed.alias);
            methods.push(SurfaceMethod {
                name: alias,
                operation_tag: computed.operation_tag.clone(),
                result_kind: "computed_read",
                cursor_brand: format!("{surface_name}Cursor"),
                input: SurfaceMethodInput::Callable { params },
                result: SurfaceMethodResult::ComputedRead { value, optional },
            });
        }

        methods.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.operation_tag.cmp(&right.operation_tag))
        });
        disambiguate_method_names(&mut methods);
        enums.sort();
        enums.dedup();
        let page_cursor_brand = methods
            .iter()
            .find(|method| matches!(method.result, SurfaceMethodResult::Page { .. }))
            .map(|method| method.cursor_brand.clone());
        self.surfaces.push(SurfaceClientSurface {
            name: surface.name.clone(),
            records,
            enums,
            methods,
            page_cursor_brand,
        });
    }

    fn read_method(
        &mut self,
        read: &SurfaceReadOperationDescriptorJson,
        record_type: &str,
        brand: &str,
        cursor_brand: &str,
        alias: &str,
        enums: &mut Vec<String>,
    ) -> SurfaceMethod {
        let (input, result) = match &read.kind {
            SurfaceReadOperationKindJson::SingletonRead => (
                SurfaceMethodInput::None,
                SurfaceMethodResult::Record {
                    record: record_type.into(),
                },
            ),
            SurfaceReadOperationKindJson::PointRead => (
                SurfaceMethodInput::Identity {
                    brand: brand.into(),
                },
                SurfaceMethodResult::Record {
                    record: record_type.into(),
                },
            ),
            SurfaceReadOperationKindJson::PagedRootCollection => (
                SurfaceMethodInput::Page {
                    exact_keys: Vec::new(),
                },
                SurfaceMethodResult::Page {
                    record: record_type.into(),
                },
            ),
            SurfaceReadOperationKindJson::PagedIndexCollection {
                exact_key_count, ..
            } => (
                SurfaceMethodInput::Page {
                    exact_keys: self.index_exact_key_types(read, *exact_key_count, enums),
                },
                SurfaceMethodResult::Page {
                    record: record_type.into(),
                },
            ),
            SurfaceReadOperationKindJson::PagedIndexRangeCollection {
                exact_key_count,
                range_key_index,
                ..
            } => (
                SurfaceMethodInput::RangePage {
                    exact_keys: self.index_exact_key_types(read, *exact_key_count, enums),
                    range_key: self.value_type(&read.index_keys[*range_key_index].value, enums),
                },
                SurfaceMethodResult::Page {
                    record: record_type.into(),
                },
            ),
            SurfaceReadOperationKindJson::UniqueIndexLookup { key_count, .. } => (
                SurfaceMethodInput::UniqueLookup {
                    keys: self.index_exact_key_types(read, *key_count, enums),
                },
                SurfaceMethodResult::OptionalRecord {
                    record: record_type.into(),
                },
            ),
        };
        let result_kind = match read.kind {
            SurfaceReadOperationKindJson::SingletonRead
            | SurfaceReadOperationKindJson::PointRead => "record",
            SurfaceReadOperationKindJson::PagedRootCollection
            | SurfaceReadOperationKindJson::PagedIndexCollection { .. }
            | SurfaceReadOperationKindJson::PagedIndexRangeCollection { .. } => "page",
            SurfaceReadOperationKindJson::UniqueIndexLookup { .. } => "optional_record",
        };
        SurfaceMethod {
            name: alias.into(),
            operation_tag: read.operation_tag.clone(),
            result_kind,
            cursor_brand: cursor_brand.into(),
            input,
            result,
        }
    }

    fn page_iteration_method(
        &mut self,
        read: &SurfaceReadOperationDescriptorJson,
        record_type: &str,
        cursor_brand: &str,
        alias: &str,
        enums: &mut Vec<String>,
    ) -> Option<SurfaceMethod> {
        let input = match &read.kind {
            SurfaceReadOperationKindJson::PagedRootCollection => SurfaceMethodInput::PageIterator {
                exact_keys: Vec::new(),
            },
            SurfaceReadOperationKindJson::PagedIndexCollection {
                exact_key_count, ..
            } => SurfaceMethodInput::PageIterator {
                exact_keys: self.index_exact_key_params(read, *exact_key_count, enums),
            },
            SurfaceReadOperationKindJson::PagedIndexRangeCollection {
                exact_key_count,
                range_key_index,
                ..
            } => SurfaceMethodInput::RangePageIterator {
                exact_keys: self.index_exact_key_params(read, *exact_key_count, enums),
                range_key: self.value_type(&read.index_keys[*range_key_index].value, enums),
            },
            SurfaceReadOperationKindJson::SingletonRead
            | SurfaceReadOperationKindJson::PointRead
            | SurfaceReadOperationKindJson::UniqueIndexLookup { .. } => return None,
        };
        Some(SurfaceMethod {
            name: format!("{alias}Pages"),
            operation_tag: read.operation_tag.clone(),
            result_kind: "page",
            cursor_brand: cursor_brand.into(),
            input,
            result: SurfaceMethodResult::PageIterator {
                record: record_type.into(),
            },
        })
    }

    /// The index exact-keys precede the identity keys in `index_keys`; take the leading `count` of
    /// them as the typed page/unique-lookup parameters. Each carries its real shape so an enum or
    /// identity exact-key is typed and encoded by that shape, not collapsed to a raw string.
    fn index_exact_key_types(
        &mut self,
        read: &SurfaceReadOperationDescriptorJson,
        count: usize,
        enums: &mut Vec<String>,
    ) -> Vec<SurfaceFieldType> {
        read.index_keys
            .iter()
            .take(count)
            .map(|key| self.value_type(&key.value, enums))
            .collect()
    }

    fn index_exact_key_params(
        &mut self,
        read: &SurfaceReadOperationDescriptorJson,
        count: usize,
        enums: &mut Vec<String>,
    ) -> Vec<SurfaceMethodParam> {
        let mut used = reserved_page_iterator_parameter_names();
        read.index_keys
            .iter()
            .take(count)
            .enumerate()
            .map(|(index, key)| {
                let base = page_iterator_parameter_base(&key.render_label, index);
                let name = unique_page_iterator_parameter_name(&base, &mut used);
                SurfaceMethodParam {
                    name,
                    ty: self.value_type(&key.value, enums),
                }
            })
            .collect()
    }

    fn create_method(
        &mut self,
        create: &SurfaceCreateOperationDescriptorJson,
        surface_name: &str,
        record_type: &str,
        enums: &mut Vec<String>,
    ) -> (SurfaceMethod, SurfaceClientRecord) {
        self.register_store(&create.store_catalog_id, &create.identity_keys);
        let brand = self.store_brand(&create.store_catalog_id);
        let body_type = format!("{surface_name}CreateBody");
        let fields = self.create_fields(&create.fields, enums);
        let body = self.body_record(&body_type, &fields, false);
        let input = if create_is_singleton(create) {
            SurfaceMethodInput::SingletonCreate {
                body_type: body_type.clone(),
                fields,
            }
        } else {
            SurfaceMethodInput::Create {
                brand,
                body_type: body_type.clone(),
                fields,
            }
        };
        let method = SurfaceMethod {
            name: "create".into(),
            operation_tag: create.operation_tag.clone(),
            result_kind: "created",
            cursor_brand: format!("{surface_name}Cursor"),
            input,
            result: SurfaceMethodResult::Created {
                record: record_type.into(),
            },
        };
        (method, body)
    }

    fn update_method(
        &mut self,
        update: &super::SurfaceUpdateOperationDescriptorJson,
        surface_name: &str,
        enums: &mut Vec<String>,
    ) -> (SurfaceMethod, SurfaceClientRecord) {
        self.register_store(&update.store_catalog_id, &update.identity_keys);
        let brand = self.store_brand(&update.store_catalog_id);
        let body_type = format!("{surface_name}UpdateBody");
        let fields = update
            .fields
            .iter()
            .map(|field| CreateFieldPlan {
                label: field.render_label.clone(),
                member_catalog_id: field.member_catalog_id.clone(),
                ty: self.value_type(&field.value, enums),
            })
            .collect::<Vec<_>>();
        let body = self.body_record(&body_type, &fields, true);
        let input = if matches!(
            update.kind,
            super::SurfaceUpdateOperationKindJson::SingletonUpdate
        ) {
            SurfaceMethodInput::SingletonUpdate { body_type, fields }
        } else {
            SurfaceMethodInput::Update {
                brand,
                body_type,
                fields,
            }
        };
        let method = SurfaceMethod {
            name: "update".into(),
            operation_tag: update.operation_tag.clone(),
            result_kind: "updated",
            cursor_brand: format!("{surface_name}Cursor"),
            input,
            result: SurfaceMethodResult::Updated,
        };
        (method, body)
    }

    /// The input record a create/update method binds: one field per write plan, keyed by render
    /// label. A create body takes the exact declared body, so every field is required; a sparse
    /// update body lets the caller omit any field, so every field is an optional key. Write bodies
    /// are input-only, so the fields carry no decode member id.
    fn body_record(
        &self,
        body_type: &str,
        fields: &[CreateFieldPlan],
        optional: bool,
    ) -> SurfaceClientRecord {
        SurfaceClientRecord {
            type_name: body_type.into(),
            fields: fields
                .iter()
                .map(|field| SurfaceRecordField {
                    label: field.label.clone(),
                    required: true,
                    optional,
                    ty: field.ty.clone(),
                    member_catalog_id: None,
                })
                .collect(),
            decode: RecordDecode::InputOnly,
        }
    }

    fn record_fields(
        &mut self,
        store_catalog_id: &str,
        identity_keys: &[SurfaceOperationIdentityKeyJson],
        projection: &[SurfaceReadProjectionFieldJson],
        enums: &mut Vec<String>,
    ) -> Vec<SurfaceRecordField> {
        let mut fields = Vec::new();
        // A keyless singleton record takes no identity, so it has no synthetic `id` field; the read
        // envelope carries `identity: null` and the client must not decode an absent identity.
        if !identity_keys.is_empty() {
            fields.push(SurfaceRecordField {
                label: "id".into(),
                required: true,
                optional: false,
                ty: SurfaceFieldType::Identity {
                    brand: self.store_brand(store_catalog_id),
                    decode_fn: self.store_decode_fn(store_catalog_id),
                },
                member_catalog_id: None,
            });
        }
        for field in projection {
            fields.push(SurfaceRecordField {
                label: field.render_label.clone(),
                required: field.required,
                optional: false,
                ty: self.value_type(&field.value, enums),
                member_catalog_id: Some(field.member_catalog_id.clone()),
            });
        }
        fields
    }

    fn create_fields(
        &mut self,
        fields: &[super::SurfaceCreateFieldDescriptorJson],
        enums: &mut Vec<String>,
    ) -> Vec<CreateFieldPlan> {
        fields
            .iter()
            .map(|field| CreateFieldPlan {
                label: field.render_label.clone(),
                member_catalog_id: field.member_catalog_id.clone(),
                ty: self.value_type(&field.value, enums),
            })
            .collect()
    }

    fn value_type(
        &mut self,
        shape: &SurfaceOperationValueShapeJson,
        enums: &mut Vec<String>,
    ) -> SurfaceFieldType {
        match shape {
            SurfaceOperationValueShapeJson::Scalar { scalar } => {
                SurfaceFieldType::Scalar(ScalarKind::parse(scalar))
            }
            SurfaceOperationValueShapeJson::Enum {
                render_name,
                enum_catalog_id,
                members,
            } => {
                let model = self.register_enum(
                    enum_catalog_id,
                    render_name,
                    members
                        .iter()
                        .map(|member| (member.render_label.clone(), member.catalog_id.clone())),
                );
                enums.push(model.member_table_const.clone());
                SurfaceFieldType::Enum {
                    type_name: model.type_name,
                    enum_catalog_const: model.enum_catalog_const,
                    encode_table: model.encode_table_const,
                }
            }
            SurfaceOperationValueShapeJson::Identity {
                store_name,
                store_catalog_id,
                key_scalars,
                ..
            } => {
                let scalars = key_scalars
                    .iter()
                    .map(|scalar| ScalarKind::parse(scalar))
                    .collect::<Vec<_>>();
                self.ensure_relation_brand(store_catalog_id, store_name);
                self.register_store_scalars(store_catalog_id, &scalars);
                SurfaceFieldType::Identity {
                    brand: self.store_brand(store_catalog_id),
                    decode_fn: self.store_decode_fn(store_catalog_id),
                }
            }
        }
    }

    fn callable_args(
        &mut self,
        parameters: &[SurfaceCallableParameterJson],
        enums: &mut Vec<String>,
    ) -> Vec<SurfaceCallableArg> {
        parameters
            .iter()
            .map(|parameter| SurfaceCallableArg {
                name: parameter.name.clone(),
                ty: self.callable_argument_type(&parameter.shape, enums),
                optional: matches!(
                    parameter.presence,
                    SurfaceCallableParameterPresenceJson::Optional
                ),
            })
            .collect()
    }

    fn callable_argument_type(
        &mut self,
        shape: &super::SurfaceCallableArgumentShapeJson,
        enums: &mut Vec<String>,
    ) -> SurfaceFieldType {
        use super::SurfaceCallableArgumentShapeJson as Shape;
        match shape {
            Shape::Scalar { scalar } => SurfaceFieldType::Scalar(ScalarKind::parse(scalar)),
            Shape::Enum {
                render_label,
                enum_catalog_id,
                members,
            } => {
                let model = self.register_enum(
                    enum_catalog_id,
                    render_label,
                    members
                        .iter()
                        .map(|member| (member.render_label.clone(), member.catalog_id.clone())),
                );
                enums.push(model.member_table_const.clone());
                SurfaceFieldType::Enum {
                    type_name: model.type_name,
                    enum_catalog_const: model.enum_catalog_const,
                    encode_table: model.encode_table_const,
                }
            }
            Shape::Identity {
                render_label,
                store_catalog_id,
                keys,
            } => {
                let scalars = keys
                    .iter()
                    .map(|key| ScalarKind::parse(&key.scalar))
                    .collect::<Vec<_>>();
                self.ensure_relation_brand(store_catalog_id, render_label);
                self.register_store_scalars(store_catalog_id, &scalars);
                SurfaceFieldType::Identity {
                    brand: self.store_brand(store_catalog_id),
                    decode_fn: self.store_decode_fn(store_catalog_id),
                }
            }
            Shape::Sequence { element } => {
                SurfaceFieldType::Sequence(Box::new(self.callable_argument_type(element, enums)))
            }
            Shape::Unsupported => SurfaceFieldType::Scalar(ScalarKind::String),
        }
    }

    fn callable_value_type(
        &mut self,
        shape: &super::SurfaceCallableArgumentShapeJson,
        enums: &mut Vec<String>,
    ) -> SurfaceFieldType {
        self.callable_argument_type(shape, enums)
    }

    fn computed_value_type(
        &mut self,
        shape: &super::SurfaceComputedReadValueShapeJson,
        enums: &mut Vec<String>,
    ) -> SurfaceFieldType {
        use super::SurfaceComputedReadValueShapeJson as Shape;
        match shape {
            Shape::Scalar { scalar } => SurfaceFieldType::Scalar(ScalarKind::parse(scalar)),
            Shape::Enum {
                render_label,
                enum_catalog_id,
                members,
            } => {
                let model = self.register_enum(
                    enum_catalog_id,
                    render_label,
                    members
                        .iter()
                        .map(|member| (member.render_label.clone(), member.catalog_id.clone())),
                );
                enums.push(model.member_table_const.clone());
                SurfaceFieldType::Enum {
                    type_name: model.type_name,
                    enum_catalog_const: model.enum_catalog_const,
                    encode_table: model.encode_table_const,
                }
            }
            Shape::Identity {
                render_label,
                store_catalog_id,
                keys,
            } => {
                let scalars = keys
                    .iter()
                    .map(|key| ScalarKind::parse(&key.scalar))
                    .collect::<Vec<_>>();
                self.ensure_relation_brand(store_catalog_id, render_label);
                self.register_store_scalars(store_catalog_id, &scalars);
                SurfaceFieldType::Identity {
                    brand: self.store_brand(store_catalog_id),
                    decode_fn: self.store_decode_fn(store_catalog_id),
                }
            }
            Shape::Sequence { element } => {
                SurfaceFieldType::Sequence(Box::new(self.computed_value_type(element, enums)))
            }
            Shape::Resource {
                render_label,
                resource_catalog_id,
                fields,
            } => {
                let type_name = self.resource_type_name(resource_catalog_id, render_label);
                let decode_fn = format!("decode{type_name}");
                let record_fields = fields
                    .iter()
                    .map(|field| SurfaceRecordField {
                        label: field.render_label.clone(),
                        required: field.required,
                        optional: false,
                        ty: self.computed_value_type(&field.value, enums),
                        member_catalog_id: Some(field.member_catalog_id.clone()),
                    })
                    .collect();
                self.resources
                    .entry(resource_catalog_id.clone())
                    .or_insert_with(|| SurfaceClientRecord {
                        type_name: type_name.clone(),
                        fields: record_fields,
                        decode: RecordDecode::ResourceValue,
                    });
                SurfaceFieldType::Resource {
                    type_name,
                    decode_fn,
                }
            }
        }
    }

    fn register_store(
        &mut self,
        store_catalog_id: &str,
        identity_keys: &[SurfaceOperationIdentityKeyJson],
    ) {
        let scalars = identity_keys
            .iter()
            .map(|key| match &key.value {
                SurfaceOperationValueShapeJson::Scalar { scalar } => ScalarKind::parse(scalar),
                _ => panic!("surface store identity keys must be scalar"),
            })
            .collect::<Vec<_>>();
        self.register_store_scalars(store_catalog_id, &scalars);
    }

    fn register_store_scalars(&mut self, store_catalog_id: &str, key_scalars: &[ScalarKind]) {
        let suffix = sanitize_catalog(store_catalog_id);
        let base = self.brand_base(store_catalog_id);
        self.stores
            .entry(store_catalog_id.to_string())
            .or_insert_with(|| SurfaceClientStore {
                store_catalog_id: store_catalog_id.to_string(),
                catalog_const: format!("STORE_{suffix}"),
                brand_type: format!("{base}Id"),
                constructor: format!("{}Id", lower_first(&base)),
                decode_fn: format!("decode{base}Id"),
                key_scalars: key_scalars.to_vec(),
            });
    }

    fn register_enum(
        &mut self,
        enum_catalog_id: &str,
        render_name: &str,
        members: impl Iterator<Item = (String, String)>,
    ) -> SurfaceEnumModel {
        if let Some(model) = self.enums.get(enum_catalog_id) {
            return model.clone();
        }
        let suffix = sanitize_catalog(enum_catalog_id);
        let type_name = self.unique_type_name(render_name, &format!("Enum_{suffix}"));
        let model = SurfaceEnumModel {
            type_name,
            enum_catalog_id: enum_catalog_id.to_string(),
            enum_catalog_const: format!("ENUM_{suffix}_CATALOG"),
            member_table_const: format!("ENUM_{suffix}"),
            encode_table_const: format!("ENUM_{suffix}_BY_LABEL"),
            members: members
                .map(|(label, member_catalog_id)| SurfaceEnumMember {
                    label,
                    member_catalog_id,
                })
                .collect(),
        };
        self.enums
            .insert(enum_catalog_id.to_string(), model.clone());
        model
    }

    fn resource_type_name(&mut self, resource_catalog_id: &str, render_label: &str) -> String {
        if let Some(name) = self.resource_names.get(resource_catalog_id) {
            return name.clone();
        }
        let fallback = format!("Resource_{}", sanitize_catalog(resource_catalog_id));
        let name = self.unique_type_name(render_label, &fallback);
        self.resource_names
            .insert(resource_catalog_id.to_string(), name.clone());
        name
    }

    fn store_brand(&self, store_catalog_id: &str) -> String {
        format!("{}Id", self.brand_base(store_catalog_id))
    }

    fn store_decode_fn(&self, store_catalog_id: &str) -> String {
        format!("decode{}Id", self.brand_base(store_catalog_id))
    }
}

/// Keep method keys unique within a surface. Aliases are checker-unique among reads, but the fixed
/// `create`/`update`/`delete` names can collide with a read aliased to one of them; suffix the later
/// collisions with a stable operation-tag fragment so the emitted object never has a duplicate key.
fn disambiguate_method_names(methods: &mut [SurfaceMethod]) {
    let mut used = std::collections::BTreeSet::new();
    for method in methods.iter_mut() {
        if used.insert(method.name.clone()) {
            continue;
        }
        let suffix = operation_tag_suffix(&method.operation_tag);
        let mut candidate = format!("{}_{suffix}", method.name);
        let mut counter = 2usize;
        while !used.insert(candidate.clone()) {
            candidate = format!("{}_{suffix}_{counter}", method.name);
            counter += 1;
        }
        method.name = candidate;
    }
}

fn operation_tag_suffix(operation_tag: &str) -> String {
    let suffix = operation_tag
        .strip_prefix("sha256:")
        .unwrap_or(operation_tag)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(8)
        .collect::<String>();
    if suffix.is_empty() {
        "op".into()
    } else {
        suffix
    }
}

fn lower_first(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_ascii_lowercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// Lift a store's lowercase source name into a PascalCase brand base, so a reference to `^projects`
/// reads as the `ProjectsId` type and `projectsId` constructor, matching the casing a surface name
/// already supplies.
fn upper_first(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

fn method_alias(bindings: &SurfaceRouteBindings, operation_tag: &str, fallback: &str) -> String {
    bindings
        .iter()
        .find(|binding: &&SurfaceRouteBinding| binding.operation_tag == operation_tag)
        .map(|binding| binding.alias.clone())
        .unwrap_or_else(|| fallback.to_string())
}

fn page_iterator_parameter_base(label: &str, index: usize) -> String {
    let base = lower_first(&sanitize_type(label));
    if base == "_" {
        format!("key{index}")
    } else if TS_RESERVED_PARAMETER_NAMES.contains(&base.as_str())
        || PAGE_ITERATOR_RESERVED_PARAMETER_NAMES.contains(&base.as_str())
    {
        format!("{base}Key")
    } else {
        base
    }
}

fn unique_page_iterator_parameter_name(base: &str, used: &mut BTreeSet<String>) -> String {
    let mut name = base.to_string();
    let mut counter = 2usize;
    while !used.insert(name.clone()) {
        name = format!("{base}{counter}");
        counter += 1;
    }
    name
}

fn reserved_page_iterator_parameter_names() -> BTreeSet<String> {
    TS_RESERVED_PARAMETER_NAMES
        .iter()
        .chain(PAGE_ITERATOR_RESERVED_PARAMETER_NAMES)
        .map(|name| (*name).to_string())
        .collect()
}

const PAGE_ITERATOR_RESERVED_PARAMETER_NAMES: &[&str] =
    &["cursor", "envelope", "options", "page", "transport"];

const TS_RESERVED_PARAMETER_NAMES: &[&str] = &[
    "arguments",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "eval",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

fn create_is_singleton(create: &SurfaceCreateOperationDescriptorJson) -> bool {
    matches!(
        create.kind,
        super::SurfaceCreateOperationKindJson::SingletonCreate
    )
}

/// Reduce a catalog id to a TypeScript-safe suffix. Catalog ids are already opaque ascii, so this
/// only guards against stray punctuation; collisions are impossible because catalog ids are unique.
fn sanitize_catalog(catalog_id: &str) -> String {
    catalog_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn sanitize_type(name: &str) -> String {
    let mut sanitized = String::new();
    for character in name.chars() {
        if sanitized.is_empty() {
            if character.is_ascii_alphabetic() || character == '_' {
                sanitized.push(character);
            } else {
                sanitized.push('_');
            }
        } else if character.is_ascii_alphanumeric() || character == '_' {
            sanitized.push(character);
        } else {
            sanitized.push('_');
        }
    }
    if sanitized.is_empty() {
        sanitized.push('_');
    }
    sanitized
}
