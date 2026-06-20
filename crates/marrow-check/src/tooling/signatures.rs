use std::path::Path;

use marrow_schema::NodeKind;

use crate::program::{CheckedModule, CheckedProgram, MarrowType};
use crate::resolve::{Def, DefItem, Resolution, ResolvableKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceConstructorSignature {
    pub name: String,
    pub ty: MarrowType,
    pub docs: Vec<String>,
    pub fields: Vec<ResourceConstructorField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceConstructorField {
    pub name: String,
    pub required: bool,
    pub ty: MarrowType,
    pub docs: Vec<String>,
}

pub fn resource_constructor_signature(
    program: &CheckedProgram,
    file: &Path,
    segments: &[String],
) -> Option<ResourceConstructorSignature> {
    let from_module = crate::module_of_file(program, file)?;
    let Resolution::Found(Def {
        module,
        item: DefItem::Resource(resource),
        ..
    }) = crate::resolve(program, from_module, segments, ResolvableKind::Resource)
    else {
        return None;
    };

    let ty = MarrowType::Resource(crate::resource_type_name(&module.name, &resource.name));
    let fields = resource
        .members
        .iter()
        .filter_map(|member| constructor_field(program, module, member))
        .collect();

    Some(ResourceConstructorSignature {
        name: resource.name.clone(),
        ty,
        docs: resource.docs.clone(),
        fields,
    })
}

fn constructor_field(
    program: &CheckedProgram,
    module: &CheckedModule,
    member: &marrow_schema::Node,
) -> Option<ResourceConstructorField> {
    if !member.is_plain_field() {
        return None;
    }
    let NodeKind::Slot { ty, required } = &member.kind else {
        return None;
    };
    Some(ResourceConstructorField {
        name: member.name.clone(),
        required: *required,
        ty: crate::enums::resolve_schema_type_for_module(ty, program, module),
        docs: member.docs.clone(),
    })
}
