use marrow_check::{
    CheckedModule, CheckedProgram, KeyDef, Node, NodeKind, ResourceFact, ResourceId,
    ResourceMemberFact, ResourceMemberId, ResourceMemberKind, ResourceSchema, ScalarType,
    StoreFact, StoreIdentityKeyFact, StoreIndexFact, StoreIndexKeySource, StoredValueMeaning, Type,
};
use serde::Serialize;

pub const RESOURCE_SCHEMA_PROFILE_VERSION: &str = "resource.schema.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceSchemaLookupJson {
    pub profile_version: String,
    pub resources: Vec<ResourceSchemaJson>,
    pub diagnostics: Vec<ResourceSchemaDiagnosticJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceSchemaDiagnosticJson {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSchemaJson {
    pub module: String,
    pub name: String,
    pub catalog_id: String,
    pub docs: Vec<String>,
    pub stores: Vec<ResourceStoreSchemaJson>,
    pub members: Vec<ResourceMemberSchemaJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceStoreSchemaJson {
    pub root: String,
    pub resource: String,
    pub catalog_id: String,
    pub docs: Vec<String>,
    pub identity_keys: Vec<ResourceSchemaKeyJson>,
    pub indexes: Vec<ResourceIndexSchemaJson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceIndexSchemaJson {
    pub name: String,
    pub catalog_id: String,
    pub args: Vec<String>,
    pub unique: bool,
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceSchemaKeyJson {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMemberSchemaJson {
    pub name: String,
    pub catalog_id: String,
    pub docs: Vec<String>,
    #[serde(flatten)]
    pub shape: ResourceMemberShapeJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ResourceMemberShapeJson {
    #[serde(rename = "field")]
    Field {
        #[serde(rename = "type")]
        type_name: String,
        required: bool,
        #[serde(rename = "errorCode")]
        error_code: bool,
    },
    #[serde(rename = "leaf")]
    Leaf {
        #[serde(rename = "type")]
        type_name: String,
        #[serde(rename = "keyParams")]
        key_params: Vec<ResourceSchemaKeyJson>,
        #[serde(rename = "errorCode")]
        error_code: bool,
    },
    #[serde(rename = "group")]
    Group {
        #[serde(rename = "keyParams")]
        key_params: Vec<ResourceSchemaKeyJson>,
        members: Vec<ResourceMemberSchemaJson>,
    },
}

pub fn resource_schema_for_name(program: &CheckedProgram, name: &str) -> ResourceSchemaLookupJson {
    let candidates: Vec<&ResourceFact> = program
        .facts
        .resources()
        .iter()
        .filter(|resource| resource.name == name)
        .collect();
    if candidates.len() > 1 {
        return ResourceSchemaLookupJson {
            profile_version: RESOURCE_SCHEMA_PROFILE_VERSION.to_string(),
            resources: Vec::new(),
            diagnostics: vec![ResourceSchemaDiagnosticJson {
                code: "resource.schema.identity".to_string(),
                message: format!(
                    "resource schema lookup for `{name}` is ambiguous without catalog-bound resource identity"
                ),
            }],
        };
    }

    match candidates.as_slice() {
        [] if source_resource_name_exists(program, name) => {
            empty_lookup(vec![incomplete_diagnostic(format!(
                "resource `{name}` has no complete checked resource fact"
            ))])
        }
        [] => empty_lookup(vec![missing_diagnostic(format!(
            "resource `{name}` is not present in the checked program"
        ))]),
        [resource] => lookup_from_result(resource_to_json(program, resource)),
        _ => unreachable!("resource candidate count checked above"),
    }
}

pub fn resource_schema_for_catalog_id(
    program: &CheckedProgram,
    catalog_id: &str,
) -> ResourceSchemaLookupJson {
    let candidates: Vec<&ResourceFact> = program
        .facts
        .resources()
        .iter()
        .filter(|resource| program.resource_catalog_id(resource.id) == Some(catalog_id))
        .collect();

    match candidates.as_slice() {
        [] => empty_lookup(vec![missing_diagnostic(format!(
            "resource catalog id `{catalog_id}` is not present in the checked program"
        ))]),
        [resource] => lookup_from_result(resource_to_json(program, resource)),
        _ => empty_lookup(vec![identity_diagnostic(format!(
            "resource catalog id `{catalog_id}` resolves to more than one resource"
        ))]),
    }
}

pub fn resource_schema_for_id(
    program: &CheckedProgram,
    resource_id: ResourceId,
) -> ResourceSchemaLookupJson {
    let Some(resource) = program.facts.resources().get(resource_id.0 as usize) else {
        return empty_lookup(vec![missing_diagnostic(format!(
            "resource id `{}` is not present in the checked program",
            resource_id.0
        ))]);
    };
    lookup_from_result(resource_to_json(program, resource))
}

fn empty_lookup(diagnostics: Vec<ResourceSchemaDiagnosticJson>) -> ResourceSchemaLookupJson {
    ResourceSchemaLookupJson {
        profile_version: RESOURCE_SCHEMA_PROFILE_VERSION.to_string(),
        resources: Vec::new(),
        diagnostics,
    }
}

fn lookup_from_result(
    result: Result<ResourceSchemaJson, ResourceSchemaDiagnosticJson>,
) -> ResourceSchemaLookupJson {
    match result {
        Ok(resource) => ResourceSchemaLookupJson {
            profile_version: RESOURCE_SCHEMA_PROFILE_VERSION.to_string(),
            resources: vec![resource],
            diagnostics: Vec::new(),
        },
        Err(diagnostic) => empty_lookup(vec![diagnostic]),
    }
}

fn resource_to_json(
    program: &CheckedProgram,
    fact: &ResourceFact,
) -> Result<ResourceSchemaJson, ResourceSchemaDiagnosticJson> {
    let module = module_for_resource(program, fact)?;
    let resource = resource_schema_for_fact(module, fact)?;
    Ok(ResourceSchemaJson {
        module: module.name.clone(),
        name: fact.name.clone(),
        catalog_id: required_catalog_id("resource", program.resource_catalog_id(fact.id))?,
        docs: resource.docs.clone(),
        stores: program
            .facts
            .stores()
            .iter()
            .filter(|store| store.resource == fact.id)
            .map(|store| store_to_json(program, fact, resource, store))
            .collect::<Result<Vec<_>, _>>()?,
        members: resource
            .members
            .iter()
            .map(|member| member_to_json(program, fact.id, None, member))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn store_to_json(
    program: &CheckedProgram,
    resource: &ResourceFact,
    source_resource: &ResourceSchema,
    store: &StoreFact,
) -> Result<ResourceStoreSchemaJson, ResourceSchemaDiagnosticJson> {
    let module = program
        .modules
        .get(store.module.0 as usize)
        .ok_or_else(|| incomplete_diagnostic(format!("store `{}` has no module", store.root)))?;
    let source_store = module
        .stores
        .iter()
        .find(|source| source.root == store.root)
        .ok_or_else(|| {
            incomplete_diagnostic(format!("store `{}` has no source schema", store.root))
        })?;
    validate_store_namespace(source_resource, store, source_store)?;
    Ok(ResourceStoreSchemaJson {
        root: store.root.clone(),
        resource: resource.name.clone(),
        catalog_id: required_catalog_id("store", program.store_catalog_id(store.id))?,
        docs: source_store.docs.clone(),
        identity_keys: store
            .identity_keys
            .iter()
            .map(identity_key_to_json)
            .collect::<Result<Vec<_>, _>>()?,
        indexes: program
            .facts
            .store_indexes()
            .iter()
            .filter(|index| index.store == store.id)
            .map(|index| index_to_json(program, store, source_store, index))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn validate_store_namespace(
    source_resource: &ResourceSchema,
    store: &StoreFact,
    source_store: &marrow_check::StoreSchema,
) -> Result<(), ResourceSchemaDiagnosticJson> {
    for key in &store.identity_keys {
        if source_resource
            .members
            .iter()
            .any(|member| member.name == key.name)
        {
            return Err(incomplete_diagnostic(format!(
                "store identity key `{}` collides with a top-level resource member",
                key.name
            )));
        }
        if source_store
            .indexes
            .iter()
            .any(|index| index.name == key.name)
        {
            return Err(incomplete_diagnostic(format!(
                "store identity key `{}` collides with an index",
                key.name
            )));
        }
    }
    Ok(())
}

fn identity_key_to_json(
    key: &StoreIdentityKeyFact,
) -> Result<ResourceSchemaKeyJson, ResourceSchemaDiagnosticJson> {
    let Some(StoredValueMeaning::Scalar(scalar)) = key.value_meaning.as_ref() else {
        return Err(incomplete_diagnostic(format!(
            "store identity key `{}` has no checked scalar meaning",
            key.name
        )));
    };
    Ok(ResourceSchemaKeyJson {
        name: key.name.clone(),
        type_name: key_scalar_type_name("store identity key", &key.name, *scalar)?,
    })
}

fn index_to_json(
    program: &CheckedProgram,
    store: &StoreFact,
    source_store: &marrow_check::StoreSchema,
    index: &StoreIndexFact,
) -> Result<ResourceIndexSchemaJson, ResourceSchemaDiagnosticJson> {
    validate_index_shape(store, index)?;
    let source_index = source_store
        .indexes
        .iter()
        .find(|source| source.name == index.name)
        .ok_or_else(|| {
            incomplete_diagnostic(format!("store index `{}` has no source schema", index.name))
        })?;
    Ok(ResourceIndexSchemaJson {
        name: index.name.clone(),
        catalog_id: required_catalog_id("store index", program.store_index_catalog_id(index.id))?,
        args: index.keys.iter().map(|key| key.name.clone()).collect(),
        unique: index.unique,
        docs: source_index.docs.clone(),
    })
}

fn validate_index_shape(
    store: &StoreFact,
    index: &StoreIndexFact,
) -> Result<(), ResourceSchemaDiagnosticJson> {
    if store.identity_keys.is_empty() {
        return Err(incomplete_diagnostic(format!(
            "store index `{}` is declared on keyless store `{}`",
            index.name, store.root
        )));
    }
    if index.keys.len() != index.declared_key_count {
        return Err(incomplete_diagnostic(format!(
            "store index `{}` has {} checked key(s) for {} declared argument(s)",
            index.name,
            index.keys.len(),
            index.declared_key_count
        )));
    }
    for key in &index.keys {
        validate_key_meaning("store index key", &key.name, &key.value_meaning)?;
    }
    if !index.unique && !index_ends_with_identity_keys(store, index) {
        return Err(incomplete_diagnostic(format!(
            "non-unique store index `{}` does not end with the store identity keys",
            index.name
        )));
    }
    Ok(())
}

fn index_ends_with_identity_keys(store: &StoreFact, index: &StoreIndexFact) -> bool {
    index.keys.len() >= store.identity_keys.len()
        && index.keys[index.keys.len() - store.identity_keys.len()..]
            .iter()
            .zip(&store.identity_keys)
            .all(|(index_key, identity_key)| {
                index_key.name == identity_key.name
                    && matches!(index_key.source, StoreIndexKeySource::IdentityKey)
            })
}

fn member_to_json(
    program: &CheckedProgram,
    resource_id: ResourceId,
    parent: Option<ResourceMemberId>,
    node: &Node,
) -> Result<ResourceMemberSchemaJson, ResourceSchemaDiagnosticJson> {
    let fact = member_fact(program, resource_id, parent, node)?;
    Ok(ResourceMemberSchemaJson {
        name: node.name.clone(),
        catalog_id: required_catalog_id(
            "resource member",
            program.resource_member_catalog_id(fact.id),
        )?,
        docs: node.docs.clone(),
        shape: member_shape_json(program, resource_id, fact, node)?,
    })
}

fn member_fact<'a>(
    program: &'a CheckedProgram,
    resource_id: ResourceId,
    parent: Option<ResourceMemberId>,
    node: &Node,
) -> Result<&'a ResourceMemberFact, ResourceSchemaDiagnosticJson> {
    program
        .facts
        .resource_members()
        .iter()
        .find(|fact| {
            fact.resource == resource_id && fact.parent == parent && fact.name == node.name
        })
        .ok_or_else(|| {
            incomplete_diagnostic(format!(
                "resource member `{}` has no checked fact",
                node.name
            ))
        })
}

fn member_shape_json(
    program: &CheckedProgram,
    resource_id: ResourceId,
    fact: &ResourceMemberFact,
    node: &Node,
) -> Result<ResourceMemberShapeJson, ResourceSchemaDiagnosticJson> {
    match (&node.kind, fact.kind) {
        (NodeKind::Slot { .. }, ResourceMemberKind::Field) if node.key_params.is_empty() => {
            let required = fact.plain_field_required.ok_or_else(|| {
                incomplete_diagnostic(format!(
                    "resource member `{}` has no checked requiredness",
                    fact.name
                ))
            })?;
            Ok(ResourceMemberShapeJson::Field {
                type_name: member_type_name(program, node, fact)?,
                required,
                error_code: node.is_error_code(),
            })
        }
        (NodeKind::Slot { .. }, ResourceMemberKind::Field) => Ok(ResourceMemberShapeJson::Leaf {
            type_name: member_type_name(program, node, fact)?,
            key_params: node
                .key_params
                .iter()
                .map(key_param_to_json)
                .collect::<Result<Vec<_>, _>>()?,
            error_code: node.is_error_code(),
        }),
        (NodeKind::Group, ResourceMemberKind::Group) => Ok(ResourceMemberShapeJson::Group {
            key_params: node
                .key_params
                .iter()
                .map(key_param_to_json)
                .collect::<Result<Vec<_>, _>>()?,
            members: node
                .members
                .iter()
                .map(|member| member_to_json(program, resource_id, Some(fact.id), member))
                .collect::<Result<Vec<_>, _>>()?,
        }),
        _ => Err(incomplete_diagnostic(format!(
            "resource member `{}` has mismatched source and checked shapes",
            fact.name
        ))),
    }
}

fn key_param_to_json(key: &KeyDef) -> Result<ResourceSchemaKeyJson, ResourceSchemaDiagnosticJson> {
    let Type::Scalar(scalar) = &key.ty else {
        return Err(incomplete_diagnostic(format!(
            "key parameter `{}` has no checked scalar type",
            key.name
        )));
    };
    Ok(ResourceSchemaKeyJson {
        name: key.name.clone(),
        type_name: key_scalar_type_name("key parameter", &key.name, *scalar)?,
    })
}

fn key_scalar_type_name(
    label: &str,
    name: &str,
    scalar: ScalarType,
) -> Result<String, ResourceSchemaDiagnosticJson> {
    if durable_key_scalar(scalar) {
        return Ok(scalar.name().to_string());
    }
    Err(incomplete_diagnostic(format!(
        "{label} `{name}` uses non-durable key scalar `{}`",
        scalar.name()
    )))
}

fn durable_key_scalar(scalar: ScalarType) -> bool {
    !matches!(scalar, ScalarType::Decimal)
}

fn validate_key_meaning(
    label: &str,
    name: &str,
    meaning: &StoredValueMeaning,
) -> Result<(), ResourceSchemaDiagnosticJson> {
    match meaning {
        StoredValueMeaning::Scalar(scalar) => {
            key_scalar_type_name(label, name, *scalar)?;
        }
        StoredValueMeaning::Enum { .. } => {}
        StoredValueMeaning::Identity {
            arity, key_scalars, ..
        } => {
            if *arity != key_scalars.len() {
                return Err(incomplete_diagnostic(format!(
                    "{label} `{name}` has mismatched identity key shape"
                )));
            }
            for scalar in key_scalars {
                key_scalar_type_name(label, name, *scalar)?;
            }
        }
    }
    Ok(())
}

fn member_type_name(
    program: &CheckedProgram,
    node: &Node,
    fact: &ResourceMemberFact,
) -> Result<String, ResourceSchemaDiagnosticJson> {
    let meaning = fact.value_meaning.as_ref().ok_or_else(|| {
        incomplete_diagnostic(format!(
            "resource member `{}` has no checked value meaning",
            fact.name
        ))
    })?;
    if node.is_error_code() {
        return match meaning {
            StoredValueMeaning::Scalar(ScalarType::Str) => Ok("ErrorCode".to_string()),
            _ => Err(incomplete_diagnostic(format!(
                "resource member `{}` is marked ErrorCode without checked string meaning",
                fact.name
            ))),
        };
    }
    match meaning {
        StoredValueMeaning::Scalar(scalar) => Ok(scalar.name().to_string()),
        StoredValueMeaning::Identity { root, .. } => Ok(format!("Id(^{root})")),
        StoredValueMeaning::Enum { enum_id, .. } => {
            let enum_fact = program.facts.enum_(*enum_id).ok_or_else(|| {
                incomplete_diagnostic(format!(
                    "resource member `{}` references a missing enum fact",
                    fact.name
                ))
            })?;
            let module = program
                .facts
                .modules()
                .get(enum_fact.module.0 as usize)
                .ok_or_else(|| {
                    incomplete_diagnostic(format!(
                        "enum `{}` has no checked module fact",
                        enum_fact.name
                    ))
                })?;
            Ok(format!("{}::{}", module.name, enum_fact.name))
        }
    }
}

fn module_for_resource<'a>(
    program: &'a CheckedProgram,
    resource: &ResourceFact,
) -> Result<&'a CheckedModule, ResourceSchemaDiagnosticJson> {
    program
        .modules
        .get(resource.module.0 as usize)
        .ok_or_else(|| {
            incomplete_diagnostic(format!(
                "resource `{}` has no checked module",
                resource.name
            ))
        })
}

fn resource_schema_for_fact<'a>(
    module: &'a CheckedModule,
    fact: &ResourceFact,
) -> Result<&'a ResourceSchema, ResourceSchemaDiagnosticJson> {
    module
        .resources
        .iter()
        .find(|resource| resource.name == fact.name)
        .ok_or_else(|| {
            incomplete_diagnostic(format!("resource `{}` has no source schema", fact.name))
        })
}

fn source_resource_name_exists(program: &CheckedProgram, name: &str) -> bool {
    program.modules.iter().any(|module| {
        module
            .resources
            .iter()
            .any(|resource| resource.name == name)
    })
}

fn required_catalog_id(
    label: &str,
    catalog_id: Option<&str>,
) -> Result<String, ResourceSchemaDiagnosticJson> {
    catalog_id
        .map(str::to_owned)
        .ok_or_else(|| incomplete_diagnostic(format!("{label} is missing a catalog id")))
}

fn identity_diagnostic(message: String) -> ResourceSchemaDiagnosticJson {
    ResourceSchemaDiagnosticJson {
        code: "resource.schema.identity".to_string(),
        message,
    }
}

fn incomplete_diagnostic(message: String) -> ResourceSchemaDiagnosticJson {
    ResourceSchemaDiagnosticJson {
        code: "resource.schema.incomplete".to_string(),
        message,
    }
}

fn missing_diagnostic(message: String) -> ResourceSchemaDiagnosticJson {
    ResourceSchemaDiagnosticJson {
        code: "resource.schema.missing".to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use marrow_check::{CheckReport, ProjectConfig, StoreBackend, StoreConfig, check_project};
    use serde_json::json;

    use super::{resource_schema_for_catalog_id, resource_schema_for_name};

    static TEMP_PROJECT_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempProject {
        path: PathBuf,
    }

    impl TempProject {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos();
            let counter = TEMP_PROJECT_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("{prefix}-{}-{nonce}-{counter}", std::process::id()));
            fs::create_dir(&path).expect("create temp project");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn checked_program(source: &str) -> (CheckReport, marrow_check::CheckedProgram) {
        checked_program_files(&[("shelf/books.mw", source)])
    }

    fn checked_clean_program(source: &str) -> marrow_check::CheckedProgram {
        let (report, program) = checked_program(source);
        assert!(
            !report.has_errors(),
            "resource schema fixture must check cleanly: {:#?}",
            report.diagnostics
        );
        program
    }

    fn checked_program_files(
        files: &[(&str, &str)],
    ) -> (CheckReport, marrow_check::CheckedProgram) {
        let root = TempProject::new("marrow-json-resource-schema-test");
        let source_dir = root.path().join("src");
        fs::create_dir(&source_dir).expect("create source dir");
        for (path, source) in files {
            let path = source_dir.join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create source parent");
            }
            fs::write(path, source).expect("write source");
        }
        let config = ProjectConfig {
            source_roots: vec!["src".into()],
            default_entry: None,
            store: StoreConfig {
                backend: StoreBackend::Native,
                data_dir: Some(".marrow/data".into()),
            },
            tests: Vec::new(),
            client: None,
        };
        check_project(root.path(), &config).expect("check project")
    }

    #[test]
    fn named_resource_schema_projects_catalog_bound_json_shape() {
        let program = checked_clean_program(
            "\
module shelf::books

;; Catalogued books.
resource Book
    ;; Display title.
    required title: string
    ;; Domain error code.
    code: ErrorCode
    ;; External ISBN.
    isbn: string
    ;; Ordered tag values.
    tags(pos: int): string
    ;; Edition-specific data.
    editions(edition: int)
        ;; Edition note.
        note: string

;; Primary book store.
store ^books(id: int): Book
    ;; ISBN lookup.
    index byIsbn(isbn) unique
",
        );

        let result = resource_schema_for_name(&program, "Book");
        let value = serde_json::to_value(result).expect("resource schema DTO serializes");
        let resource = &value["resources"][0];
        let resource_catalog_id = resource["catalogId"].as_str().expect("resource catalog id");
        let title_catalog_id = resource["members"][0]["catalogId"]
            .as_str()
            .expect("title catalog id");
        let code_catalog_id = resource["members"][1]["catalogId"]
            .as_str()
            .expect("code catalog id");
        let isbn_catalog_id = resource["members"][2]["catalogId"]
            .as_str()
            .expect("isbn catalog id");
        let tags_catalog_id = resource["members"][3]["catalogId"]
            .as_str()
            .expect("tags catalog id");
        let editions_catalog_id = resource["members"][4]["catalogId"]
            .as_str()
            .expect("editions catalog id");
        let note_catalog_id = resource["members"][4]["members"][0]["catalogId"]
            .as_str()
            .expect("note catalog id");
        let store_catalog_id = resource["stores"][0]["catalogId"]
            .as_str()
            .expect("store catalog id");
        let index_catalog_id = resource["stores"][0]["indexes"][0]["catalogId"]
            .as_str()
            .expect("index catalog id");

        assert_eq!(
            value,
            json!({
                "profile_version": "resource.schema.v1",
                "resources": [{
                    "module": "shelf::books",
                    "name": "Book",
                    "catalogId": resource_catalog_id,
                    "docs": ["Catalogued books."],
                    "stores": [{
                        "root": "books",
                        "resource": "Book",
                        "catalogId": store_catalog_id,
                        "docs": ["Primary book store."],
                        "identityKeys": [{"name": "id", "type": "int"}],
                        "indexes": [{
                            "name": "byIsbn",
                            "catalogId": index_catalog_id,
                            "args": ["isbn"],
                            "unique": true,
                            "docs": ["ISBN lookup."]
                        }]
                    }],
                    "members": [
                        {
                            "name": "title",
                            "catalogId": title_catalog_id,
                            "docs": ["Display title."],
                            "kind": "field",
                            "type": "string",
                            "required": true,
                            "errorCode": false
                        },
                        {
                            "name": "code",
                            "catalogId": code_catalog_id,
                            "docs": ["Domain error code."],
                            "kind": "field",
                            "type": "ErrorCode",
                            "required": false,
                            "errorCode": true
                        },
                        {
                            "name": "isbn",
                            "catalogId": isbn_catalog_id,
                            "docs": ["External ISBN."],
                            "kind": "field",
                            "type": "string",
                            "required": false,
                            "errorCode": false
                        },
                        {
                            "name": "tags",
                            "catalogId": tags_catalog_id,
                            "docs": ["Ordered tag values."],
                            "kind": "leaf",
                            "type": "string",
                            "keyParams": [{"name": "pos", "type": "int"}],
                            "errorCode": false
                        },
                        {
                            "name": "editions",
                            "catalogId": editions_catalog_id,
                            "docs": ["Edition-specific data."],
                            "kind": "group",
                            "keyParams": [{"name": "edition", "type": "int"}],
                            "members": [{
                                "name": "note",
                                "catalogId": note_catalog_id,
                                "docs": ["Edition note."],
                                "kind": "field",
                                "type": "string",
                                "required": false,
                                "errorCode": false
                            }]
                        }
                    ]
                }],
                "diagnostics": []
            })
        );
    }

    #[test]
    fn resource_schema_can_be_looked_up_by_catalog_id() {
        let program = checked_clean_program(
            "\
module shelf::books

resource Book
    title: string
",
        );
        let by_name = resource_schema_for_name(&program, "Book");
        let catalog_id = by_name.resources[0].catalog_id.as_str();

        let by_catalog_id = resource_schema_for_catalog_id(&program, catalog_id);

        assert_eq!(by_catalog_id, by_name);
    }

    #[test]
    fn absent_resource_name_returns_missing_diagnostic() {
        let program = checked_clean_program(
            "\
module shelf::books

resource Book
    title: string
",
        );

        let result = resource_schema_for_name(&program, "Author");
        let value = serde_json::to_value(result).expect("resource schema DTO serializes");

        assert_eq!(value["resources"], json!([]));
        assert_eq!(value["diagnostics"][0]["code"], "resource.schema.missing");
    }

    #[test]
    fn invalid_index_argument_does_not_export_successful_schema() {
        let (report, program) = checked_program(
            "\
module shelf::books

resource Book
    title: string

store ^books(id: int): Book
    index byGhost(ghost, id)
",
        );
        assert!(
            report.has_errors(),
            "fixture should carry index diagnostics"
        );

        let result = resource_schema_for_name(&program, "Book");
        let value = serde_json::to_value(result).expect("resource schema DTO serializes");

        assert_eq!(value["resources"], json!([]));
        assert_eq!(
            value["diagnostics"][0]["code"],
            "resource.schema.incomplete"
        );
    }

    #[test]
    fn decimal_identity_key_does_not_export_successful_schema() {
        let (report, program) = checked_program(
            "\
module shelf::books

resource Book
    title: string

store ^books(id: decimal): Book
",
        );
        assert!(
            report.has_errors(),
            "fixture should carry decimal identity-key diagnostics"
        );

        let result = resource_schema_for_name(&program, "Book");
        let value = serde_json::to_value(result).expect("resource schema DTO serializes");

        assert_eq!(value["resources"], json!([]));
        assert_eq!(
            value["diagnostics"][0]["code"],
            "resource.schema.incomplete"
        );
    }

    #[test]
    fn decimal_keyed_member_key_does_not_export_successful_schema() {
        let (report, program) = checked_program(
            "\
module shelf::books

resource Book
    tags(pos: decimal): string

store ^books(id: int): Book
",
        );
        assert!(
            report.has_errors(),
            "fixture should carry decimal keyed-member diagnostics"
        );

        let result = resource_schema_for_name(&program, "Book");
        let value = serde_json::to_value(result).expect("resource schema DTO serializes");

        assert_eq!(value["resources"], json!([]));
        assert_eq!(
            value["diagnostics"][0]["code"],
            "resource.schema.incomplete"
        );
    }

    #[test]
    fn decimal_index_member_arg_does_not_export_successful_schema() {
        let (report, program) = checked_program(
            "\
module shelf::books

resource Book
    price: decimal

store ^books(id: int): Book
    index byPrice(price, id)
",
        );
        assert!(
            report.has_errors(),
            "fixture should carry decimal index-argument diagnostics"
        );

        let result = resource_schema_for_name(&program, "Book");
        let value = serde_json::to_value(result).expect("resource schema DTO serializes");

        assert_eq!(value["resources"], json!([]));
        assert_eq!(
            value["diagnostics"][0]["code"],
            "resource.schema.incomplete"
        );
    }

    #[test]
    fn identity_index_member_with_decimal_key_does_not_export_successful_schema() {
        let (report, program) = checked_program(
            "\
module shelf::books

resource Author
    name: string

store ^authors(id: decimal): Author

resource Book
    author: Id(^authors)

store ^books(id: int): Book
    index byAuthor(author, id)
",
        );
        assert!(
            report.has_errors(),
            "fixture should carry decimal identity index diagnostics"
        );

        let result = resource_schema_for_name(&program, "Book");
        let value = serde_json::to_value(result).expect("resource schema DTO serializes");

        assert_eq!(value["resources"], json!([]));
        assert_eq!(
            value["diagnostics"][0]["code"],
            "resource.schema.incomplete"
        );
    }

    #[test]
    fn non_unique_index_missing_identity_suffix_does_not_export_successful_schema() {
        let (report, program) = checked_program(
            "\
module shelf::books

resource Book
    title: string

store ^books(id: int): Book
    index byTitle(title)
",
        );
        assert!(
            report.has_errors(),
            "fixture should carry non-unique identity suffix diagnostics"
        );

        let result = resource_schema_for_name(&program, "Book");
        let value = serde_json::to_value(result).expect("resource schema DTO serializes");

        assert_eq!(value["resources"], json!([]));
        assert_eq!(
            value["diagnostics"][0]["code"],
            "resource.schema.incomplete"
        );
    }

    #[test]
    fn keyless_root_index_does_not_export_successful_schema() {
        let (report, program) = checked_program(
            "\
module shelf::books

resource Setting
    name: string

store ^settings: Setting
    index byName(name)
",
        );
        assert!(
            report.has_errors(),
            "fixture should carry keyless-root index diagnostics"
        );

        let result = resource_schema_for_name(&program, "Setting");
        let value = serde_json::to_value(result).expect("resource schema DTO serializes");

        assert_eq!(value["resources"], json!([]));
        assert_eq!(
            value["diagnostics"][0]["code"],
            "resource.schema.incomplete"
        );
    }

    #[test]
    fn identity_key_member_collision_does_not_export_successful_schema() {
        let (report, program) = checked_program(
            "\
module shelf::books

resource Book
    id: string

store ^books(id: int): Book
",
        );
        assert_report_has_code(&report, "schema.key_member_collision");

        let result = resource_schema_for_name(&program, "Book");
        let value = serde_json::to_value(result).expect("resource schema DTO serializes");

        assert_eq!(value["resources"], json!([]));
        assert_eq!(
            value["diagnostics"][0]["code"],
            "resource.schema.incomplete"
        );
    }

    #[test]
    fn identity_key_index_collision_does_not_export_successful_schema() {
        let (report, program) = checked_program(
            "\
module shelf::books

resource Book
    title: string

store ^books(byTitle: int): Book
    index byTitle(title, byTitle)
",
        );
        assert_report_has_code(&report, "schema.key_member_collision");

        let result = resource_schema_for_name(&program, "Book");
        let value = serde_json::to_value(result).expect("resource schema DTO serializes");

        assert_eq!(value["resources"], json!([]));
        assert_eq!(
            value["diagnostics"][0]["code"],
            "resource.schema.incomplete"
        );
    }

    #[test]
    fn unknown_member_type_does_not_export_successful_schema() {
        let (report, program) = checked_program(
            "\
module shelf::books

resource Book
    mystery: MissingType
",
        );
        assert!(report.has_errors(), "fixture should carry type diagnostics");

        let result = resource_schema_for_name(&program, "Book");
        let value = serde_json::to_value(result).expect("resource schema DTO serializes");

        assert_eq!(value["resources"], json!([]));
        assert_eq!(
            value["diagnostics"][0]["code"],
            "resource.schema.incomplete"
        );
    }

    #[test]
    fn duplicate_resource_names_return_marrow_diagnostic() {
        let (report, program) = checked_program_files(&[
            (
                "shelf/books.mw",
                "\
module shelf::books
resource Book
    title: string
",
            ),
            (
                "shelf/archive.mw",
                "\
module shelf::archive
resource Book
    code: string
                ",
            ),
        ]);
        assert!(
            !report.has_errors(),
            "resource schema fixture must check cleanly: {:#?}",
            report.diagnostics
        );

        let result = resource_schema_for_name(&program, "Book");
        let value = serde_json::to_value(result).expect("resource schema DTO serializes");

        assert_eq!(value["resources"], json!([]));
        assert_eq!(value["diagnostics"][0]["code"], "resource.schema.identity");
    }

    fn assert_report_has_code(report: &CheckReport, code: &str) {
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == code),
            "expected diagnostic code `{code}`, got {:#?}",
            report.diagnostics
        );
    }
}
