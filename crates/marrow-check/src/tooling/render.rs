use super::signatures::{
    CallableArgumentStyle, CallableParameter, CallableSignature, CallableValueShape,
};
use crate::MarrowType;
use crate::model::decls::DeclIds;

pub fn render_callable_signature(names: &DeclIds<'_>, callable: &CallableSignature) -> String {
    let params = callable
        .params
        .iter()
        .map(|param| render_callable_parameter_label(names, param, callable.argument_style))
        .collect::<Vec<_>>()
        .join(", ");
    let path = callable.path.join("::");
    match callable
        .return_shape
        .as_ref()
        .map(|shape| render_callable_shape(names, shape))
    {
        Some(return_shape) => format!("{path}({params}): {return_shape}"),
        None => format!("{path}({params})"),
    }
}

fn render_callable_parameter_label(
    names: &DeclIds<'_>,
    param: &CallableParameter,
    style: CallableArgumentStyle,
) -> String {
    match style {
        CallableArgumentStyle::Positional => {
            let label = match param.shape {
                CallableValueShape::SavedRoot if param.label == "root" => "^root".to_string(),
                _ => param.label.clone(),
            };
            if param.repeat {
                format!("{label}...")
            } else {
                label
            }
        }
        CallableArgumentStyle::NamedFields => {
            format!(
                "{}: {}",
                param.label,
                render_callable_shape(names, &param.shape)
            )
        }
    }
}

pub fn render_callable_shape(names: &DeclIds<'_>, shape: &CallableValueShape) -> String {
    match shape {
        CallableValueShape::Type(ty) => render_marrow_type(names, ty),
        CallableValueShape::Scalar => "scalar".to_string(),
        CallableValueShape::Value => "value".to_string(),
        CallableValueShape::Sequence => "sequence".to_string(),
        CallableValueShape::Collection => "collection".to_string(),
        CallableValueShape::SavedPath => "path".to_string(),
        CallableValueShape::SavedLayer => "layer".to_string(),
        CallableValueShape::SavedRoot => "^root".to_string(),
        CallableValueShape::Identity => "Id".to_string(),
        CallableValueShape::ErrorCode => "ErrorCode".to_string(),
    }
}

pub fn render_marrow_type(names: &DeclIds<'_>, ty: &MarrowType) -> String {
    match ty {
        MarrowType::Primitive(scalar) => scalar.name().to_string(),
        MarrowType::Error => "Error".to_string(),
        MarrowType::Resource(id) => names.resource_display(*id),
        MarrowType::GroupEntry { resource, .. } => names.resource_display(*resource),
        MarrowType::Identity(root) => {
            format!("Id(^{})", names.root_spelling(*root).unwrap_or("?"))
        }
        MarrowType::Enum(id) => match names.enum_owner_and_name(*id) {
            Some(("", name)) => name.to_string(),
            Some((module, name)) => format!("{module}::{name}"),
            None => "unknown".to_string(),
        },
        MarrowType::Sequence(element) => {
            format!("sequence[{}]", render_marrow_type(names, element))
        }
        MarrowType::LocalTree { value, .. } => {
            format!("tree[{}]", render_marrow_type(names, value))
        }
        MarrowType::Optional(inner) => format!("{}?", render_marrow_type(names, inner)),
        MarrowType::Absent => "absent".to_string(),
        MarrowType::Invalid | MarrowType::Unknown => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::CheckedFacts;
    use crate::model::decls::StoreRootArena;
    use marrow_schema::ScalarType;

    #[test]
    fn local_tree_types_render_as_tree_of_value() {
        // These renderings exercise only structural leaves, whose spelling never
        // reads the recovery view, so an empty one suffices.
        let facts = CheckedFacts::default();
        let roots = StoreRootArena::default();
        let names = DeclIds::new(&facts, &roots);
        let ty = MarrowType::LocalTree {
            keys: vec![MarrowType::Primitive(ScalarType::Str)],
            value: Box::new(MarrowType::Primitive(ScalarType::Int)),
        };

        assert_eq!(render_marrow_type(&names, &ty), "tree[int]");
    }

    #[test]
    fn callable_signatures_use_marrow_type_rendering() {
        let facts = CheckedFacts::default();
        let roots = StoreRootArena::default();
        let names = DeclIds::new(&facts, &roots);
        let callable = CallableSignature {
            path: vec!["take".to_string()],
            kind: crate::tooling::CallableSignatureKind::Builtin,
            argument_style: CallableArgumentStyle::NamedFields,
            docs: Vec::new(),
            params: vec![CallableParameter {
                label: "items".to_string(),
                required: true,
                repeat: false,
                shape: CallableValueShape::Type(MarrowType::LocalTree {
                    keys: vec![MarrowType::Primitive(ScalarType::Str)],
                    value: Box::new(MarrowType::Primitive(ScalarType::Int)),
                }),
                docs: Vec::new(),
            }],
            return_shape: Some(CallableValueShape::Value),
        };

        assert_eq!(
            render_callable_signature(&names, &callable),
            "take(items: tree[int]): value"
        );
    }
}
