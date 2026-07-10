use marrow_schema::{Node, NodeKind, ScalarType, Type};

use crate::program::{CheckedModule, CheckedProgram, MarrowType};
use crate::resolve::resolve_store_by_root;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckedResourceRef {
    pub module: u32,
    pub resource: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedResourceConstructor {
    pub resource: CheckedResourceRef,
    pub name: String,
    pub fields: Vec<CheckedResourceConstructorField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedResourceConstructorField {
    pub name: String,
    pub required: bool,
    /// The field is declared `ErrorCode`, so a dynamic value bound to it must
    /// satisfy the dotted-lowercase grammar. A string literal is rejected at check.
    pub error_code: bool,
    pub ty: CheckedRuntimeValueType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedRuntimeValueType {
    Primitive(ScalarType),
    Error,
    Resource,
    GroupEntry,
    Identity {
        root: String,
        keys: Option<Vec<marrow_schema::KeyDef>>,
    },
    Enum {
        module: String,
        name: String,
        enum_id: Option<crate::facts::EnumId>,
        allowed_members: Vec<crate::facts::EnumMemberId>,
    },
    Sequence(Box<CheckedRuntimeValueType>),
    LocalTree {
        keys: Vec<CheckedRuntimeValueType>,
        value: Box<CheckedRuntimeValueType>,
    },
    Invalid,
    Unknown,
}

pub(super) fn resource_ref(
    program: &CheckedProgram,
    module: &CheckedModule,
    resource: &marrow_schema::ResourceSchema,
) -> Option<CheckedResourceRef> {
    let module_index = program
        .modules
        .iter()
        .position(|candidate| std::ptr::eq(candidate, module))?;
    let resource_index = module
        .resources
        .iter()
        .position(|candidate| std::ptr::eq(candidate, resource))?;
    Some(CheckedResourceRef {
        module: module_index as u32,
        resource: resource_index as u32,
    })
}

pub(super) fn checked_resource_constructor(
    program: &CheckedProgram,
    module: &CheckedModule,
    resource: &marrow_schema::ResourceSchema,
    resource_ref: CheckedResourceRef,
) -> CheckedResourceConstructor {
    CheckedResourceConstructor {
        resource: resource_ref,
        name: resource.name.clone(),
        fields: resource
            .members
            .iter()
            .filter_map(|node| checked_resource_constructor_field(program, module, node))
            .collect(),
    }
}

fn checked_resource_constructor_field(
    program: &CheckedProgram,
    module: &CheckedModule,
    node: &Node,
) -> Option<CheckedResourceConstructorField> {
    if !node.is_plain_field() {
        return None;
    }
    let NodeKind::Slot { required, ty, .. } = &node.kind else {
        return None;
    };
    Some(CheckedResourceConstructorField {
        name: node.name.clone(),
        required: *required,
        error_code: node.is_error_code(),
        ty: checked_runtime_value_type(
            program,
            checked_constructor_field_type(program, module, ty),
        ),
    })
}

fn checked_constructor_field_type(
    program: &CheckedProgram,
    module: &CheckedModule,
    ty: &Type,
) -> MarrowType {
    crate::enums::resolve_schema_type_for_module(ty, program, module)
}

pub(crate) fn checked_runtime_value_type(
    program: &CheckedProgram,
    ty: MarrowType,
) -> CheckedRuntimeValueType {
    match ty {
        MarrowType::Primitive(scalar) => CheckedRuntimeValueType::Primitive(scalar),
        MarrowType::Error => CheckedRuntimeValueType::Error,
        MarrowType::Resource(_) => CheckedRuntimeValueType::Resource,
        MarrowType::GroupEntry { .. } => CheckedRuntimeValueType::GroupEntry,
        MarrowType::Identity(root_id) => {
            let root = program
                .decl_ids()
                .root_spelling(root_id)
                .unwrap_or_default()
                .to_string();
            CheckedRuntimeValueType::Identity {
                keys: resolve_store_by_root(program, &root)
                    .map(|store| store.store.identity_keys.clone()),
                root,
            }
        }
        MarrowType::Enum(enum_id) => {
            let (module, name) = program
                .enum_by_id(enum_id)
                .map_or_else(Default::default, |(module, name)| {
                    (module.to_string(), name.to_string())
                });
            CheckedRuntimeValueType::Enum {
                enum_id: Some(enum_id),
                allowed_members: program
                    .facts
                    .enum_members()
                    .iter()
                    .filter(|member| {
                        member.enum_id == enum_id
                            && program.facts.enum_member_is_selectable(member.id)
                    })
                    .map(|member| member.id)
                    .collect(),
                module,
                name,
            }
        }
        MarrowType::Sequence(element) => CheckedRuntimeValueType::Sequence(Box::new(
            checked_runtime_value_type(program, *element),
        )),
        MarrowType::LocalTree { keys, value } => CheckedRuntimeValueType::LocalTree {
            keys: keys
                .into_iter()
                .map(|key| checked_runtime_value_type(program, key))
                .collect(),
            value: Box::new(checked_runtime_value_type(program, *value)),
        },
        // Presence rides the operation tag, not the runtime value type, so an
        // optional return types as its present arm; the empty optional has no
        // runtime value type of its own.
        MarrowType::Optional(inner) => checked_runtime_value_type(program, *inner),
        MarrowType::Absent => CheckedRuntimeValueType::Unknown,
        MarrowType::Invalid => CheckedRuntimeValueType::Invalid,
        // `Unknown` is the existing unsupported runtime shape used for dynamic
        // values. No-value and unresolved states map only to that fail-closed shape;
        // neither acquires a supported runtime value representation.
        MarrowType::Dynamic | MarrowType::NoValue | MarrowType::Unknown => {
            CheckedRuntimeValueType::Unknown
        }
    }
}
