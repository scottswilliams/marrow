use marrow_store::cell::CatalogId;
use marrow_store::value::ScalarType;

use crate::executable::checked_runtime_value_type;
use crate::{
    CheckedEntryFunction, CheckedFunctionRef, CheckedProgram, CheckedRuntimeFunction,
    CheckedRuntimeModule, CheckedRuntimeProgram, CheckedRuntimeValueType, MarrowType,
    ResourceMemberKind, StoredValueMeaning,
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
    pub return_value: Option<EntryArgumentShape>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryFunctionSurfaceDescriptor {
    pub identity: EntryIdentity,
    pub parameters: Vec<EntryParameter>,
    pub result: EntryResultShape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntrySurfaceProfile {
    Action,
    ComputedRead,
}

/// The surface result of an entry or computed read. Presence lives in this one
/// carrier — `Void` for a no-return action, `Present` for a definite `T`, `Optional`
/// for a `T?` — computed from the return type, so no parallel presence flag can
/// disagree with the value shape. The operation-tag presence component and the
/// client JSON presence enum are both derived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryResultShape {
    Void,
    Present(EntrySurfaceValueShape),
    Optional(EntrySurfaceValueShape),
}

impl EntryResultShape {
    /// Whether the result is maybe-present (`T?`) — the byte-stable tag presence
    /// component. A void result is definite, not maybe-present.
    pub fn maybe_present(&self) -> bool {
        matches!(self, EntryResultShape::Optional(_))
    }

    /// The present-arm value shape, or `None` for a void result.
    pub fn value(&self) -> Option<&EntrySurfaceValueShape> {
        match self {
            EntryResultShape::Void => None,
            EntryResultShape::Present(shape) | EntryResultShape::Optional(shape) => Some(shape),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryParameter {
    pub name: String,
    pub shape: EntryParameterShape,
}

/// A surface parameter's shape. Presence lives in this one carrier — `Present` for
/// a definite `T`, `Optional` for a `T?` — read off the parameter type, mirroring
/// the `EntryResultShape` return carrier so no parallel presence flag can disagree
/// with the value shape. The operation-tag parameter-optionality component is
/// derived from it, and the decoder binds an absent optional only where the carrier
/// says `Optional`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryParameterShape {
    Present(EntryArgumentShape),
    Optional(EntryArgumentShape),
}

impl EntryParameterShape {
    /// Whether the parameter is optional (`T?`) — the byte-stable tag optionality
    /// component.
    pub fn optional(&self) -> bool {
        matches!(self, EntryParameterShape::Optional(_))
    }

    /// The present-arm argument shape the value decodes into.
    pub fn shape(&self) -> &EntryArgumentShape {
        match self {
            EntryParameterShape::Present(shape) | EntryParameterShape::Optional(shape) => shape,
        }
    }

    /// The present-arm argument shape, consuming the carrier.
    pub fn into_shape(self) -> EntryArgumentShape {
        match self {
            EntryParameterShape::Present(shape) | EntryParameterShape::Optional(shape) => shape,
        }
    }
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
pub enum EntrySurfaceValueShape {
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
    Sequence(Box<EntrySurfaceValueShape>),
    Resource {
        render_label: String,
        resource_catalog_id: CatalogId,
        fields: Vec<EntryResourceResultField>,
    },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryResourceResultField {
    pub render_label: String,
    pub member_catalog_id: CatalogId,
    pub required: bool,
    pub shape: EntrySurfaceValueShape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EntrySignatureUnsupported {
    Parameter { name: String },
    ReturnValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComputedReadSignatureUnsupported {
    Parameter { name: String },
    ReturnValue,
}

pub(crate) fn surface_value_as_action_argument(
    shape: EntrySurfaceValueShape,
) -> Option<EntryArgumentShape> {
    match shape {
        EntrySurfaceValueShape::Scalar(scalar) => Some(EntryArgumentShape::Scalar(scalar)),
        EntrySurfaceValueShape::Enum {
            render_label,
            catalog_id,
            members,
        } => Some(EntryArgumentShape::Enum {
            render_label,
            catalog_id,
            members,
        }),
        EntrySurfaceValueShape::Identity {
            render_label,
            store_catalog_id,
            keys,
        } => Some(EntryArgumentShape::Identity {
            render_label,
            store_catalog_id,
            keys,
        }),
        EntrySurfaceValueShape::Sequence(element) => {
            let element = surface_value_as_action_argument(*element)?;
            Some(EntryArgumentShape::Sequence(Box::new(element)))
        }
        EntrySurfaceValueShape::Resource { .. } => None,
    }
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

    pub(crate) fn from_function_ref(
        program: &CheckedRuntimeProgram,
        requested_name: &str,
        target: CheckedFunctionRef,
    ) -> Option<Self> {
        let module = program.modules().get(target.module as usize)?;
        let function = module.functions().get(target.function as usize)?;
        let parameters: Vec<EntryParameter> = function
            .entry_params()
            .iter()
            .zip(&function.params)
            .map(|(runtime_param, checked_param)| {
                let shape = argument_shape(program, &runtime_param.ty);
                let shape = if matches!(checked_param.ty, MarrowType::Optional(_)) {
                    EntryParameterShape::Optional(shape)
                } else {
                    EntryParameterShape::Present(shape)
                };
                EntryParameter {
                    name: runtime_param.name.clone(),
                    shape,
                }
            })
            .collect();
        let canonical_name = canonical_entry_name(module, function);
        let return_value = function
            .entry_return_type()
            .map(|ty| argument_shape(program, ty));
        Some(Self {
            identity: EntryIdentity {
                requested_name: requested_name.to_string(),
                canonical_name: canonical_name.clone(),
                entry_tag: entry_tag(program, &canonical_name, function, &parameters),
                accepted_catalog_epoch: program.accepted_catalog_epoch(),
                source_digest: program.source_digest().to_string(),
                read_only_context_digest: program.read_only_context_digest().to_string(),
            },
            parameters,
            return_value,
        })
    }
}

impl EntryFunctionSurfaceDescriptor {
    pub fn from_function_ref(
        program: &CheckedProgram,
        requested_name: &str,
        target: CheckedFunctionRef,
        profile: EntrySurfaceProfile,
    ) -> Option<Self> {
        let runtime = program.runtime();
        let entry = EntryDescriptor::from_function_ref(&runtime, requested_name, target)?;
        if !entry
            .parameters
            .iter()
            .all(|parameter| callable_argument_shape_supported(parameter.shape.shape()))
        {
            return None;
        }

        let module = program.modules.get(target.module as usize)?;
        let function = module.functions.get(target.function as usize)?;
        if !function.public {
            return None;
        }
        let value = match function.return_type.as_ref() {
            Some(ty) => Some(surface_value_shape(program, ty, profile)?),
            None if profile == EntrySurfaceProfile::ComputedRead => return None,
            None => None,
        };
        if profile == EntrySurfaceProfile::ComputedRead
            && !value.as_ref().is_some_and(computed_read_shape_supported)
        {
            return None;
        }
        if profile == EntrySurfaceProfile::Action
            && value
                .clone()
                .is_some_and(|shape| surface_value_as_action_argument(shape).is_none())
        {
            return None;
        }

        let result = match value {
            Some(shape) if function.returns_maybe_present() => EntryResultShape::Optional(shape),
            Some(shape) => EntryResultShape::Present(shape),
            None => EntryResultShape::Void,
        };
        Some(Self {
            identity: entry.identity,
            parameters: entry.parameters,
            result,
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
    parameters: &[EntryParameter],
) -> String {
    let mut payload = String::new();
    push_part(&mut payload, "version", ENTRY_PROTOCOL_TAG_VERSION);
    push_part(&mut payload, "entry", canonical_name);
    push_part(
        &mut payload,
        "return_presence",
        return_presence_name(function.returns_maybe_present()),
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
    match function.entry_return_type() {
        Some(ty) => push_runtime_type(program, &mut payload, "return.type", ty),
        None => push_part(&mut payload, "return.type", "none"),
    }
    push_part(
        &mut payload,
        "params.len",
        &function.entry_params().len().to_string(),
    );
    for (param, parameter) in function.entry_params().iter().zip(parameters) {
        push_part(&mut payload, "param.name", &param.name);
        if parameter.shape.optional() {
            push_part(&mut payload, "param.optional", "true");
        }
        push_runtime_type(program, &mut payload, "param", &param.ty);
    }
    marrow_project::sha256_digest(payload.as_bytes())
}

pub(crate) fn function_ref_has_accepted_entry_catalog_ids(
    program: &CheckedProgram,
    target: CheckedFunctionRef,
) -> bool {
    let Some(module) = program.modules.get(target.module as usize) else {
        return false;
    };
    let Some(function) = module.functions.get(target.function as usize) else {
        return false;
    };
    function.params.iter().all(|param| {
        let ty = checked_runtime_value_type(program, param.ty.clone());
        runtime_type_has_accepted_catalog_ids(program, &ty)
    }) && function
        .return_type
        .as_ref()
        .map(|ty| checked_runtime_value_type(program, ty.clone()))
        .is_none_or(|ty| runtime_type_has_accepted_catalog_ids(program, &ty))
}

pub(crate) fn function_ref_has_accepted_computed_read_catalog_ids(
    program: &CheckedProgram,
    target: CheckedFunctionRef,
) -> bool {
    let Some(module) = program.modules.get(target.module as usize) else {
        return false;
    };
    let Some(function) = module.functions.get(target.function as usize) else {
        return false;
    };
    function.params.iter().all(|param| {
        let ty = checked_runtime_value_type(program, param.ty.clone());
        runtime_type_has_accepted_catalog_ids(program, &ty)
    }) && function
        .return_type
        .as_ref()
        .is_some_and(|ty| computed_read_type_has_accepted_catalog_ids(program, ty))
}

pub(crate) fn function_ref_has_supported_entry_signature(
    program: &CheckedProgram,
    target: CheckedFunctionRef,
) -> Result<(), EntrySignatureUnsupported> {
    let Some(module) = program.modules.get(target.module as usize) else {
        return Err(EntrySignatureUnsupported::ReturnValue);
    };
    let Some(function) = module.functions.get(target.function as usize) else {
        return Err(EntrySignatureUnsupported::ReturnValue);
    };
    for param in &function.params {
        let ty = checked_runtime_value_type(program, param.ty.clone());
        if !runtime_type_has_entry_shape(&ty) {
            return Err(EntrySignatureUnsupported::Parameter {
                name: param.name.clone(),
            });
        }
    }
    if let Some(ty) = function.return_type.as_ref() {
        let ty = checked_runtime_value_type(program, ty.clone());
        if !runtime_type_has_entry_shape(&ty) {
            return Err(EntrySignatureUnsupported::ReturnValue);
        }
    }
    Ok(())
}

pub(crate) fn function_ref_has_supported_computed_read_signature(
    program: &CheckedProgram,
    target: CheckedFunctionRef,
) -> Result<(), ComputedReadSignatureUnsupported> {
    let Some(module) = program.modules.get(target.module as usize) else {
        return Err(ComputedReadSignatureUnsupported::ReturnValue);
    };
    let Some(function) = module.functions.get(target.function as usize) else {
        return Err(ComputedReadSignatureUnsupported::ReturnValue);
    };
    for param in &function.params {
        let ty = checked_runtime_value_type(program, param.ty.clone());
        if !runtime_type_has_entry_shape(&ty) {
            return Err(ComputedReadSignatureUnsupported::Parameter {
                name: param.name.clone(),
            });
        }
    }
    let Some(return_type) = function.return_type.as_ref() else {
        return Err(ComputedReadSignatureUnsupported::ReturnValue);
    };
    if computed_read_type_has_surface_shape(program, return_type) {
        Ok(())
    } else {
        Err(ComputedReadSignatureUnsupported::ReturnValue)
    }
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

/// The present-arm type, one optional layer stripped. Presence rides the operation
/// tag, not the value/result shape, so an `Optional(T)` surface result resolves to
/// the same shape as a definite `T`. [`MarrowType::optional`] flattens, so one strip
/// reaches the present arm.
fn present_arm(ty: &MarrowType) -> &MarrowType {
    match ty {
        MarrowType::Optional(inner) => inner.as_ref(),
        other => other,
    }
}

fn surface_value_shape(
    program: &CheckedProgram,
    ty: &MarrowType,
    profile: EntrySurfaceProfile,
) -> Option<EntrySurfaceValueShape> {
    match present_arm(ty) {
        MarrowType::Resource(resource) if profile == EntrySurfaceProfile::ComputedRead => {
            resource_result_shape(program, resource)
        }
        MarrowType::Resource(_) => None,
        ty => {
            let runtime = program.runtime();
            let runtime_ty = checked_runtime_value_type(program, ty.clone());
            surface_value_shape_from_runtime(&runtime, &runtime_ty)
        }
    }
}

fn surface_value_shape_from_runtime(
    program: &CheckedRuntimeProgram,
    ty: &CheckedRuntimeValueType,
) -> Option<EntrySurfaceValueShape> {
    match argument_shape(program, ty) {
        EntryArgumentShape::Scalar(scalar) => Some(EntrySurfaceValueShape::Scalar(scalar)),
        EntryArgumentShape::Enum {
            render_label,
            catalog_id,
            members,
        } => Some(EntrySurfaceValueShape::Enum {
            render_label,
            catalog_id,
            members,
        }),
        EntryArgumentShape::Identity {
            render_label,
            store_catalog_id,
            keys,
        } => Some(EntrySurfaceValueShape::Identity {
            render_label,
            store_catalog_id,
            keys,
        }),
        EntryArgumentShape::Sequence(element) => {
            let element = argument_shape_to_surface_value(*element)?;
            Some(EntrySurfaceValueShape::Sequence(Box::new(element)))
        }
        EntryArgumentShape::Unsupported => None,
    }
}

fn argument_shape_to_surface_value(shape: EntryArgumentShape) -> Option<EntrySurfaceValueShape> {
    match shape {
        EntryArgumentShape::Scalar(scalar) => Some(EntrySurfaceValueShape::Scalar(scalar)),
        EntryArgumentShape::Enum {
            render_label,
            catalog_id,
            members,
        } => Some(EntrySurfaceValueShape::Enum {
            render_label,
            catalog_id,
            members,
        }),
        EntryArgumentShape::Identity {
            render_label,
            store_catalog_id,
            keys,
        } => Some(EntrySurfaceValueShape::Identity {
            render_label,
            store_catalog_id,
            keys,
        }),
        EntryArgumentShape::Sequence(element) => Some(EntrySurfaceValueShape::Sequence(Box::new(
            argument_shape_to_surface_value(*element)?,
        ))),
        EntryArgumentShape::Unsupported => None,
    }
}

fn resource_result_shape(
    program: &CheckedProgram,
    resource_name: &str,
) -> Option<EntrySurfaceValueShape> {
    let resource = resource_by_type_name(program, resource_name)?;
    let resource_catalog_id =
        checked_accepted_catalog_id_value(program, resource.catalog_id.as_deref())?;
    let mut fields = program
        .facts
        .resource_members()
        .iter()
        .filter(|member| member.resource == resource.id && member.parent.is_none())
        .map(|member| {
            if member.kind != ResourceMemberKind::Field || member.key_count != 0 {
                return None;
            }
            let member_catalog_id =
                checked_accepted_catalog_id_value(program, member.catalog_id.as_deref())?;
            let shape = stored_value_shape(program, member.value_meaning.as_ref()?)?;
            Some(EntryResourceResultField {
                render_label: member.name.clone(),
                member_catalog_id,
                required: member.plain_field_required?,
                shape,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    fields.sort_by(|left, right| left.member_catalog_id.cmp(&right.member_catalog_id));
    Some(EntrySurfaceValueShape::Resource {
        render_label: resource_name.to_string(),
        resource_catalog_id,
        fields,
    })
}

fn computed_read_type_has_accepted_catalog_ids(program: &CheckedProgram, ty: &MarrowType) -> bool {
    match present_arm(ty) {
        MarrowType::Resource(resource) => {
            computed_read_resource_type_has_accepted_catalog_ids(program, resource)
        }
        ty => {
            let runtime_ty = checked_runtime_value_type(program, ty.clone());
            runtime_type_has_accepted_catalog_ids(program, &runtime_ty)
        }
    }
}

fn computed_read_resource_type_has_accepted_catalog_ids(
    program: &CheckedProgram,
    resource_name: &str,
) -> bool {
    let Some(resource) = resource_by_type_name(program, resource_name) else {
        return false;
    };
    checked_accepted_catalog_id(program, resource.catalog_id.as_deref()).is_some()
        && program
            .facts
            .resource_members()
            .iter()
            .filter(|member| member.resource == resource.id && member.parent.is_none())
            .all(|member| {
                checked_accepted_catalog_id(program, member.catalog_id.as_deref()).is_some()
                    && stored_value_meaning_has_accepted_catalog_ids(
                        program,
                        member.value_meaning.as_ref(),
                    )
            })
}

fn computed_read_type_has_surface_shape(program: &CheckedProgram, ty: &MarrowType) -> bool {
    match present_arm(ty) {
        MarrowType::Resource(resource) => {
            computed_read_resource_type_has_surface_shape(program, resource)
        }
        ty => {
            let runtime_ty = checked_runtime_value_type(program, ty.clone());
            runtime_type_has_entry_shape(&runtime_ty)
        }
    }
}

fn computed_read_resource_type_has_surface_shape(
    program: &CheckedProgram,
    resource_name: &str,
) -> bool {
    let Some(resource) = resource_by_type_name(program, resource_name) else {
        return false;
    };
    program
        .facts
        .resource_members()
        .iter()
        .filter(|member| member.resource == resource.id && member.parent.is_none())
        .all(|member| {
            member.kind == ResourceMemberKind::Field
                && member.key_count == 0
                && member.plain_field_required.is_some()
                && stored_value_meaning_has_surface_shape(program, member.value_meaning.as_ref())
        })
}

fn stored_value_meaning_has_surface_shape(
    program: &CheckedProgram,
    meaning: Option<&StoredValueMeaning>,
) -> bool {
    match meaning {
        Some(StoredValueMeaning::Scalar(_)) => true,
        Some(StoredValueMeaning::Identity { store, .. }) => {
            program.facts.stores().get(store.0 as usize).is_some()
        }
        Some(StoredValueMeaning::Enum { enum_id, members }) => {
            program.facts.enum_(*enum_id).is_some()
                && members
                    .iter()
                    .all(|member| program.facts.enum_member(*member).is_some())
        }
        None => false,
    }
}

fn stored_value_meaning_has_accepted_catalog_ids(
    program: &CheckedProgram,
    meaning: Option<&StoredValueMeaning>,
) -> bool {
    match meaning {
        None | Some(StoredValueMeaning::Scalar(_)) => true,
        Some(StoredValueMeaning::Identity {
            store,
            store_catalog_id,
            ..
        }) => {
            checked_accepted_catalog_id(program, store_catalog_id.as_deref()).is_some()
                && checked_accepted_catalog_id(
                    program,
                    program.facts.store(*store).catalog_id.as_deref(),
                )
                .is_some()
        }
        Some(StoredValueMeaning::Enum { enum_id, members }) => {
            let Some(enum_fact) = program.facts.enum_(*enum_id) else {
                return false;
            };
            checked_accepted_catalog_id(program, enum_fact.catalog_id.as_deref()).is_some()
                && members.iter().all(|member| {
                    program.facts.enum_member(*member).is_some_and(|member| {
                        checked_accepted_catalog_id(program, member.catalog_id.as_deref()).is_some()
                    })
                })
        }
    }
}

fn resource_by_type_name<'a>(
    program: &'a CheckedProgram,
    resource_name: &str,
) -> Option<&'a crate::ResourceFact> {
    program.facts.resources().iter().find(|resource| {
        let module = program.facts.modules().get(resource.module.0 as usize);
        let qualified = module.map_or_else(
            || resource.name.clone(),
            |module| {
                if module.name.is_empty() {
                    resource.name.clone()
                } else {
                    format!("{}::{}", module.name, resource.name)
                }
            },
        );
        qualified == resource_name
    })
}

fn stored_value_shape(
    program: &CheckedProgram,
    meaning: &StoredValueMeaning,
) -> Option<EntrySurfaceValueShape> {
    match meaning {
        StoredValueMeaning::Scalar(scalar) => Some(EntrySurfaceValueShape::Scalar(*scalar)),
        StoredValueMeaning::Identity {
            root,
            store_catalog_id,
            key_scalars,
            ..
        } => {
            let store_catalog_id =
                checked_accepted_catalog_id_value(program, store_catalog_id.as_deref())?;
            let store = program.facts.store_by_root(root)?;
            let keys = store
                .identity_keys
                .iter()
                .zip(key_scalars)
                .map(|(key, scalar)| EntryIdentityKey {
                    render_label: key.name.clone(),
                    scalar: *scalar,
                })
                .collect();
            Some(EntrySurfaceValueShape::Identity {
                render_label: root.clone(),
                store_catalog_id,
                keys,
            })
        }
        StoredValueMeaning::Enum { enum_id, members } => {
            let enum_fact = program.facts.enum_(*enum_id)?;
            let catalog_id =
                checked_accepted_catalog_id_value(program, enum_fact.catalog_id.as_deref())?;
            let module_name = program
                .facts
                .modules()
                .get(enum_fact.module.0 as usize)
                .map(|module| module.name.as_str())
                .unwrap_or_default();
            let render_label = if module_name.is_empty() {
                enum_fact.name.clone()
            } else {
                format!("{}::{}", module_name, enum_fact.name)
            };
            let members = members
                .iter()
                .map(|member_id| {
                    let member = program.facts.enum_member(*member_id)?;
                    Some(EntryEnumMember {
                        render_label: member.name.clone(),
                        catalog_id: checked_accepted_catalog_id_value(
                            program,
                            member.catalog_id.as_deref(),
                        )?,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(EntrySurfaceValueShape::Enum {
                render_label,
                catalog_id,
                members,
            })
        }
    }
}

fn callable_argument_shape_supported(shape: &EntryArgumentShape) -> bool {
    match shape {
        EntryArgumentShape::Scalar(_)
        | EntryArgumentShape::Enum { .. }
        | EntryArgumentShape::Identity { .. } => true,
        EntryArgumentShape::Sequence(element) => {
            callable_sequence_argument_shape_supported(element)
        }
        EntryArgumentShape::Unsupported => false,
    }
}

fn computed_read_shape_supported(shape: &EntrySurfaceValueShape) -> bool {
    match shape {
        EntrySurfaceValueShape::Scalar(_)
        | EntrySurfaceValueShape::Enum { .. }
        | EntrySurfaceValueShape::Identity { .. } => true,
        EntrySurfaceValueShape::Sequence(element) => {
            computed_read_sequence_shape_supported(element)
        }
        EntrySurfaceValueShape::Resource { fields, .. } => fields
            .iter()
            .all(|field| computed_read_resource_field_shape_supported(&field.shape)),
    }
}

fn computed_read_sequence_shape_supported(shape: &EntrySurfaceValueShape) -> bool {
    matches!(
        shape,
        EntrySurfaceValueShape::Scalar(_) | EntrySurfaceValueShape::Enum { .. }
    )
}

fn computed_read_resource_field_shape_supported(shape: &EntrySurfaceValueShape) -> bool {
    matches!(
        shape,
        EntrySurfaceValueShape::Scalar(_)
            | EntrySurfaceValueShape::Enum { .. }
            | EntrySurfaceValueShape::Identity { .. }
    )
}

fn runtime_type_has_entry_shape(ty: &CheckedRuntimeValueType) -> bool {
    match ty {
        CheckedRuntimeValueType::Primitive(_) => true,
        CheckedRuntimeValueType::Enum { enum_id, .. } => enum_id.is_some(),
        CheckedRuntimeValueType::Identity { keys, .. } => keys
            .as_ref()
            .is_some_and(|keys| keys.iter().all(|key| key.ty.scalar().is_some())),
        CheckedRuntimeValueType::Sequence(element) => {
            runtime_sequence_element_has_entry_shape(element)
        }
        CheckedRuntimeValueType::Resource
        | CheckedRuntimeValueType::GroupEntry
        | CheckedRuntimeValueType::LocalTree { .. }
        | CheckedRuntimeValueType::Error
        | CheckedRuntimeValueType::Invalid
        | CheckedRuntimeValueType::Unknown => false,
    }
}

fn runtime_sequence_element_has_entry_shape(ty: &CheckedRuntimeValueType) -> bool {
    match ty {
        CheckedRuntimeValueType::Primitive(_) => true,
        CheckedRuntimeValueType::Enum { enum_id, .. } => enum_id.is_some(),
        _ => false,
    }
}

fn callable_sequence_argument_shape_supported(shape: &EntryArgumentShape) -> bool {
    matches!(
        shape,
        EntryArgumentShape::Scalar(_) | EntryArgumentShape::Enum { .. }
    )
}

fn runtime_type_has_accepted_catalog_ids(
    program: &CheckedProgram,
    ty: &CheckedRuntimeValueType,
) -> bool {
    match ty {
        CheckedRuntimeValueType::Primitive(_)
        | CheckedRuntimeValueType::Resource
        | CheckedRuntimeValueType::GroupEntry
        | CheckedRuntimeValueType::LocalTree { .. }
        | CheckedRuntimeValueType::Error
        | CheckedRuntimeValueType::Invalid
        | CheckedRuntimeValueType::Unknown => true,
        CheckedRuntimeValueType::Enum {
            enum_id,
            allowed_members,
            ..
        } => {
            let Some(enum_id) = enum_id else {
                return false;
            };
            let Some(enum_fact) = program.facts.enum_(*enum_id) else {
                return false;
            };
            checked_accepted_catalog_id(program, enum_fact.catalog_id.as_deref()).is_some()
                && allowed_members.iter().all(|member_id| {
                    program
                        .facts
                        .enum_member(*member_id)
                        .and_then(|member| {
                            checked_accepted_catalog_id(program, member.catalog_id.as_deref())
                        })
                        .is_some()
                })
        }
        CheckedRuntimeValueType::Identity { root, .. } => {
            let Some(store) = program.facts.store_by_root(root) else {
                return false;
            };
            checked_accepted_catalog_id(program, store.catalog_id.as_deref()).is_some()
                && store
                    .identity_keys
                    .iter()
                    .all(|key| matches!(key.value_meaning, Some(StoredValueMeaning::Scalar(_))))
        }
        CheckedRuntimeValueType::Sequence(element) => {
            runtime_type_has_accepted_catalog_ids(program, element)
        }
    }
}

fn checked_accepted_catalog_id(program: &CheckedProgram, catalog_id: Option<&str>) -> Option<()> {
    let catalog_id = catalog_id?;
    program
        .catalog
        .accepted_entries
        .iter()
        .any(|entry| entry.stable_id == catalog_id)
        .then_some(())
}

fn checked_accepted_catalog_id_value(
    program: &CheckedProgram,
    catalog_id: Option<&str>,
) -> Option<CatalogId> {
    let catalog_id = catalog_id?;
    checked_accepted_catalog_id(program, Some(catalog_id))?;
    CatalogId::new(catalog_id.to_string()).ok()
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

/// The byte-stable presence token for the operation tag. A maybe-present (`T?`)
/// return is a distinct tag from a definite one, preserving tag soundness.
fn return_presence_name(maybe_present: bool) -> &'static str {
    if maybe_present {
        "maybe_present"
    } else {
        "always"
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
