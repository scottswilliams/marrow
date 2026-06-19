use marrow_schema::ReturnPresence;
use marrow_store::cell::CatalogId;
use marrow_store::value::ScalarType;

use crate::{
    CheckedEntryFunction, CheckedFunctionRef, CheckedRuntimeFunction, CheckedRuntimeModule,
    CheckedRuntimeProgram, CheckedRuntimeValueType, StoredValueMeaning,
};

pub const ENTRY_PROTOCOL_TAG_VERSION: &str = "entry.invoke.v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryIdentity {
    pub requested_name: String,
    pub canonical_name: String,
    pub entry_tag: String,
    pub accepted_catalog_epoch: Option<u64>,
    pub source_digest: String,
    pub read_only_context_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryDescriptor {
    pub identity: EntryIdentity,
    pub parameters: Vec<EntryParameter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryParameter {
    pub name: String,
    pub shape: EntryArgumentShape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryArgumentShape {
    Scalar(ScalarType),
    Enum {
        render_label: String,
        catalog_id: CatalogId,
        members: Vec<EntryEnumMember>,
    },
    Identity {
        render_label: String,
        store_catalog_id: CatalogId,
        keys: Vec<EntryIdentityKey>,
    },
    Sequence(Box<EntryArgumentShape>),
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryEnumMember {
    pub render_label: String,
    pub catalog_id: CatalogId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryIdentityKey {
    pub render_label: String,
    pub scalar: ScalarType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryDescriptorError {
    Ambiguous,
    Private,
    Missing,
}

impl EntryDescriptor {
    pub fn resolve(
        program: &CheckedRuntimeProgram,
        entry: &str,
    ) -> Result<Self, EntryDescriptorError> {
        let target = match program.entry_function_ref(entry) {
            CheckedEntryFunction::Found(target) => target,
            CheckedEntryFunction::Ambiguous => return Err(EntryDescriptorError::Ambiguous),
            CheckedEntryFunction::Private => return Err(EntryDescriptorError::Private),
            CheckedEntryFunction::Missing => return Err(EntryDescriptorError::Missing),
        };
        Self::from_function_ref(program, entry, target).ok_or(EntryDescriptorError::Missing)
    }

    fn from_function_ref(
        program: &CheckedRuntimeProgram,
        requested_name: &str,
        target: CheckedFunctionRef,
    ) -> Option<Self> {
        let module = program.modules().get(target.module as usize)?;
        let function = module.functions().get(target.function as usize)?;
        let parameters = function
            .entry_params()
            .iter()
            .map(|param| EntryParameter {
                name: param.name.clone(),
                shape: argument_shape(program, &param.ty),
            })
            .collect();
        let canonical_name = canonical_entry_name(module, function);
        Some(Self {
            identity: EntryIdentity {
                requested_name: requested_name.to_string(),
                canonical_name: canonical_name.clone(),
                entry_tag: entry_tag(program, &canonical_name, function),
                accepted_catalog_epoch: program.accepted_catalog_epoch(),
                source_digest: program.source_digest().to_string(),
                read_only_context_digest: program.read_only_context_digest().to_string(),
            },
            parameters,
        })
    }
}

fn canonical_entry_name(
    module: &CheckedRuntimeModule,
    function: &CheckedRuntimeFunction,
) -> String {
    if module.name.is_empty() {
        function.name.clone()
    } else {
        format!("{}::{}", module.name, function.name)
    }
}

fn entry_tag(
    program: &CheckedRuntimeProgram,
    canonical_name: &str,
    function: &CheckedRuntimeFunction,
) -> String {
    let mut payload = String::new();
    push_part(&mut payload, "version", ENTRY_PROTOCOL_TAG_VERSION);
    push_part(&mut payload, "entry", canonical_name);
    push_part(
        &mut payload,
        "return_presence",
        return_presence_name(function.return_presence),
    );
    push_part(
        &mut payload,
        "return",
        if function.return_type.is_some() {
            "some"
        } else {
            "none"
        },
    );
    push_part(
        &mut payload,
        "params.len",
        &function.entry_params().len().to_string(),
    );
    for param in function.entry_params() {
        push_part(&mut payload, "param.name", &param.name);
        push_runtime_type(program, &mut payload, "param", &param.ty);
    }
    marrow_project::sha256_digest(payload.as_bytes())
}

fn argument_shape(
    program: &CheckedRuntimeProgram,
    ty: &CheckedRuntimeValueType,
) -> EntryArgumentShape {
    match ty {
        CheckedRuntimeValueType::Primitive(scalar) => EntryArgumentShape::Scalar(*scalar),
        CheckedRuntimeValueType::Enum {
            enum_id,
            allowed_members,
            ..
        } => enum_argument_shape(program, *enum_id, allowed_members),
        CheckedRuntimeValueType::Identity { root, .. } => identity_argument_shape(program, root),
        CheckedRuntimeValueType::Sequence(element) if entry_sequence_element_supported(element) => {
            EntryArgumentShape::Sequence(Box::new(argument_shape(program, element)))
        }
        CheckedRuntimeValueType::Sequence(_)
        | CheckedRuntimeValueType::Resource
        | CheckedRuntimeValueType::GroupEntry
        | CheckedRuntimeValueType::LocalTree { .. }
        | CheckedRuntimeValueType::Error
        | CheckedRuntimeValueType::Invalid
        | CheckedRuntimeValueType::Unknown => EntryArgumentShape::Unsupported,
    }
}

fn enum_argument_shape(
    program: &CheckedRuntimeProgram,
    enum_id: Option<crate::EnumId>,
    allowed_members: &[crate::EnumMemberId],
) -> EntryArgumentShape {
    let Some(enum_id) = enum_id else {
        return EntryArgumentShape::Unsupported;
    };
    let Some(enum_fact) = program.facts().enum_(enum_id) else {
        return EntryArgumentShape::Unsupported;
    };
    let Some(catalog_id) = accepted_catalog_id(program, enum_fact.catalog_id.as_deref()) else {
        return EntryArgumentShape::Unsupported;
    };
    let module_name = program
        .facts()
        .modules()
        .get(enum_fact.module.0 as usize)
        .map(|module| module.name.as_str())
        .unwrap_or_default();
    let name = if module_name.is_empty() {
        enum_fact.name.clone()
    } else {
        format!("{}::{}", module_name, enum_fact.name)
    };
    let Some(members) = allowed_members
        .iter()
        .map(|member_id| {
            let member = program.facts().enum_member(*member_id)?;
            Some(EntryEnumMember {
                render_label: member.name.clone(),
                catalog_id: accepted_catalog_id(program, member.catalog_id.as_deref())?,
            })
        })
        .collect::<Option<Vec<_>>>()
    else {
        return EntryArgumentShape::Unsupported;
    };
    EntryArgumentShape::Enum {
        render_label: name,
        catalog_id,
        members,
    }
}

fn identity_argument_shape(program: &CheckedRuntimeProgram, root: &str) -> EntryArgumentShape {
    let Some(store) = program.facts().store_by_root(root) else {
        return EntryArgumentShape::Unsupported;
    };
    let Some(store_catalog_id) = accepted_catalog_id(program, store.catalog_id.as_deref()) else {
        return EntryArgumentShape::Unsupported;
    };
    let Some(keys) = store
        .identity_keys
        .iter()
        .map(|key| match key.value_meaning {
            Some(StoredValueMeaning::Scalar(scalar)) => Some(EntryIdentityKey {
                render_label: key.name.clone(),
                scalar,
            }),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
    else {
        return EntryArgumentShape::Unsupported;
    };
    EntryArgumentShape::Identity {
        render_label: root.to_string(),
        store_catalog_id,
        keys,
    }
}

fn entry_sequence_element_supported(ty: &CheckedRuntimeValueType) -> bool {
    matches!(
        ty,
        CheckedRuntimeValueType::Primitive(_) | CheckedRuntimeValueType::Enum { .. }
    )
}

fn accepted_catalog_id(
    program: &CheckedRuntimeProgram,
    catalog_id: Option<&str>,
) -> Option<CatalogId> {
    let catalog_id = catalog_id?;
    let id = CatalogId::new(catalog_id.to_string()).ok()?;
    program.has_accepted_catalog_id(catalog_id).then_some(id)
}

fn push_runtime_type(
    program: &CheckedRuntimeProgram,
    payload: &mut String,
    prefix: &str,
    ty: &CheckedRuntimeValueType,
) {
    match ty {
        CheckedRuntimeValueType::Primitive(scalar) => {
            push_part(payload, prefix, "primitive");
            push_part(payload, prefix, scalar.name());
        }
        CheckedRuntimeValueType::Enum {
            enum_id,
            allowed_members,
            ..
        } => {
            push_part(payload, prefix, "enum");
            let enum_id = enum_id
                .and_then(|enum_id| program.facts().enum_(enum_id))
                .and_then(|fact| accepted_catalog_id(program, fact.catalog_id.as_deref()));
            push_optional_catalog_id(payload, prefix, enum_id.as_ref());
            push_part(payload, prefix, &allowed_members.len().to_string());
            for member_id in allowed_members {
                let catalog_id = program
                    .facts()
                    .enum_member(*member_id)
                    .and_then(|member| accepted_catalog_id(program, member.catalog_id.as_deref()));
                push_optional_catalog_id(payload, prefix, catalog_id.as_ref());
            }
        }
        CheckedRuntimeValueType::Identity { root, .. } => {
            push_part(payload, prefix, "identity");
            let store = program.facts().store_by_root(root);
            let store_catalog_id =
                store.and_then(|store| accepted_catalog_id(program, store.catalog_id.as_deref()));
            push_optional_catalog_id(payload, prefix, store_catalog_id.as_ref());
            let Some(store) = store else {
                push_part(payload, prefix, "keys.unavailable");
                return;
            };
            push_part(payload, prefix, &store.identity_keys.len().to_string());
            for key in &store.identity_keys {
                match key.value_meaning {
                    Some(StoredValueMeaning::Scalar(scalar)) => {
                        push_part(payload, prefix, scalar.name());
                    }
                    _ => push_part(payload, prefix, "unsupported"),
                }
            }
        }
        CheckedRuntimeValueType::Sequence(element) => {
            push_part(payload, prefix, "sequence");
            push_runtime_type(program, payload, prefix, element);
        }
        CheckedRuntimeValueType::Resource => push_part(payload, prefix, "resource"),
        CheckedRuntimeValueType::GroupEntry => push_part(payload, prefix, "group_entry"),
        CheckedRuntimeValueType::LocalTree { .. } => push_part(payload, prefix, "local_tree"),
        CheckedRuntimeValueType::Error => push_part(payload, prefix, "error"),
        CheckedRuntimeValueType::Invalid => push_part(payload, prefix, "invalid"),
        CheckedRuntimeValueType::Unknown => push_part(payload, prefix, "unknown"),
    }
}

fn return_presence_name(presence: ReturnPresence) -> &'static str {
    match presence {
        ReturnPresence::Always => "always",
        ReturnPresence::MaybePresent => "maybe_present",
    }
}

fn push_optional_catalog_id(payload: &mut String, prefix: &str, value: Option<&CatalogId>) {
    match value {
        Some(value) => {
            push_part(payload, prefix, "some");
            push_part(payload, prefix, value.as_str());
        }
        None => push_part(payload, prefix, "none"),
    }
}

fn push_part(payload: &mut String, label: &str, value: &str) {
    payload.push_str(label);
    payload.push('\0');
    payload.push_str(&value.len().to_string());
    payload.push('\0');
    payload.push_str(value);
    payload.push('\0');
}
