use std::path::Path;

use marrow_schema::{NodeKind, stdlib};
use stdlib::{ParamType, ReturnType};

use crate::analysis::AnalysisSnapshot;
use crate::diagnostics::ConversionTarget;
use crate::executable::{CheckedBuiltinCall, CheckedBuiltinReturnShape, CheckedBuiltinValueShape};
use crate::program::{CheckedFunction, CheckedModule, CheckedProgram, MarrowType, TypeNames};
use crate::resolve::{Def, DefItem, Resolution, ResolvableKind};
use marrow_syntax::{
    Declaration, FunctionDecl, LexedSource, ParsedSource, ResourceDecl, ResourceMember, SourceFile,
    SourceSpan,
};

pub use marrow_syntax::{
    ActiveCallableContext, CallableCalleeContext, active_callable_context, callable_callee_contexts,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallableSignature {
    pub path: Vec<String>,
    pub kind: CallableSignatureKind,
    pub argument_style: CallableArgumentStyle,
    pub docs: Vec<String>,
    pub params: Vec<CallableParameter>,
    pub return_shape: Option<CallableValueShape>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallableSignatureKind {
    Builtin,
    ScalarConversion,
    ErrorConstructor,
    IdentityConstructor,
    StandardLibrary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallableArgumentStyle {
    Positional,
    NamedFields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallableParameter {
    pub label: String,
    pub required: bool,
    pub repeat: bool,
    pub shape: CallableValueShape,
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallableValueShape {
    Type(MarrowType),
    Scalar,
    Value,
    Sequence,
    Collection,
    SavedPath,
    SavedLayer,
    SavedRoot,
    Identity,
    ErrorCode,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSignatureHelpFact {
    pub callable: SourceSignatureHelpCallable,
    pub active_argument: usize,
    pub named_argument: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSignatureHelpCallable {
    Intrinsic {
        path: Vec<String>,
        argument_style: CallableArgumentStyle,
        docs: Vec<String>,
        params: Vec<SourceSignatureHelpParameter>,
        return_shape: Option<CallableValueShape>,
    },
    ResourceConstructor {
        name: String,
        docs: Vec<String>,
        params: Vec<SourceSignatureHelpParameter>,
        return_type: MarrowType,
    },
    Function {
        name: String,
        docs: Vec<String>,
        params: Vec<SourceSignatureHelpParameter>,
        return_type: Option<MarrowType>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSignatureHelpParameter {
    pub name: Option<String>,
    pub label: String,
    pub required: bool,
    pub repeat: bool,
    pub ty: Option<MarrowType>,
    pub shape: Option<CallableValueShape>,
    pub docs: Vec<String>,
}

pub fn source_signature_help_fact_at(
    program: &CheckedProgram,
    snapshot: Option<&AnalysisSnapshot>,
    file: &Path,
    source: &str,
    lexed: &LexedSource,
    offset: usize,
) -> Option<SourceSignatureHelpFact> {
    if let Some(snapshot) = snapshot
        && !snapshot.files.iter().any(|analyzed| analyzed.path == file)
    {
        return None;
    }
    let parsed = marrow_syntax::parse_source(source);
    let context = active_callable_context(source, lexed, &parsed, offset)?;
    let callable = source_signature_help_callable(
        program,
        snapshot,
        file,
        &parsed,
        &context.callee_path_segments,
    )?;
    Some(SourceSignatureHelpFact {
        callable,
        active_argument: context.active_argument,
        named_argument: context.named_argument,
    })
}

pub(super) fn active_signature_help_parameter(
    fact: &SourceSignatureHelpFact,
) -> Option<&SourceSignatureHelpParameter> {
    match &fact.callable {
        SourceSignatureHelpCallable::Intrinsic {
            argument_style,
            params,
            ..
        } => active_parameter_for_style(
            params,
            *argument_style,
            fact.active_argument,
            fact.named_argument.as_deref(),
        ),
        SourceSignatureHelpCallable::ResourceConstructor { params, .. } => {
            named_active_parameter(params, fact.named_argument.as_deref())
        }
        SourceSignatureHelpCallable::Function { params, .. } => fact
            .named_argument
            .as_deref()
            .and_then(|name| named_active_parameter(params, Some(name)))
            .or_else(|| params.get(fact.active_argument)),
    }
}

fn active_parameter_for_style<'a>(
    params: &'a [SourceSignatureHelpParameter],
    style: CallableArgumentStyle,
    active_argument: usize,
    named_argument: Option<&str>,
) -> Option<&'a SourceSignatureHelpParameter> {
    match style {
        CallableArgumentStyle::Positional => params.get(active_argument),
        CallableArgumentStyle::NamedFields => named_active_parameter(params, named_argument),
    }
}

fn named_active_parameter<'a>(
    params: &'a [SourceSignatureHelpParameter],
    name: Option<&str>,
) -> Option<&'a SourceSignatureHelpParameter> {
    let name = name?;
    params
        .iter()
        .find(|parameter| parameter.name.as_deref() == Some(name))
}

pub fn intrinsic_callable_signature(segments: &[String]) -> Option<CallableSignature> {
    match segments {
        [first, module, op] if first == "std" => std_signature(module, op),
        [name] => identity_signature(name)
            .or_else(|| error_signature(name))
            .or_else(|| conversion_signature(name))
            .or_else(|| builtin_signature(name)),
        _ => None,
    }
}

pub fn intrinsic_completion_callables() -> Vec<CallableSignature> {
    CheckedBuiltinCall::descriptors()
        .iter()
        .map(builtin_signature_from_descriptor)
        .chain(ConversionTarget::all().map(conversion_signature_for_target))
        .chain(identity_signature("Id"))
        .chain(error_signature("Error"))
        .collect()
}

pub fn intrinsic_callable_signature_for_file(
    snapshot: &AnalysisSnapshot,
    file: &Path,
    segments: &[String],
) -> Option<CallableSignature> {
    let analyzed = snapshot
        .files
        .iter()
        .find(|analyzed| analyzed.path == file)?;
    intrinsic_callable_signature_for_source_file(&analyzed.parsed.file, segments)
}

pub fn resource_constructor_signature(
    program: &CheckedProgram,
    file: &Path,
    segments: &[String],
) -> Option<ResourceConstructorSignature> {
    let from_module = crate::module_of_file(program, file)?;
    resource_constructor_signature_from_module(program, from_module, segments)
}

fn resource_constructor_signature_from_module(
    program: &CheckedProgram,
    from_module: &str,
    segments: &[String],
) -> Option<ResourceConstructorSignature> {
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
    let NodeKind::Slot { ty, required, .. } = &member.kind else {
        return None;
    };
    Some(ResourceConstructorField {
        name: member.name.clone(),
        required: *required,
        ty: crate::enums::resolve_schema_type_for_module(ty, program, module),
        docs: member.docs.clone(),
    })
}

fn source_signature_help_callable(
    program: &CheckedProgram,
    snapshot: Option<&AnalysisSnapshot>,
    file: &Path,
    parsed: &ParsedSource,
    segments: &[String],
) -> Option<SourceSignatureHelpCallable> {
    let source_file = &parsed.file;
    let from_module = crate::module_of_file(program, file);
    let segments = signature_help_segments(source_file, segments)?;
    if let Some(from_module) = from_module {
        if let Some(callable) = intrinsic_callable_signature(&segments) {
            return Some(source_intrinsic_signature(callable));
        }
        if let Some(resource) =
            resource_constructor_signature_from_module(program, from_module, &segments)
        {
            return Some(source_resource_constructor_signature(resource));
        }
    }
    if let Some(resource) =
        source_file_resource_constructor_signature(program, file, parsed, &segments)
    {
        return Some(source_resource_constructor_signature(resource));
    }
    let from_module = from_module?;
    source_function_signature(program, snapshot, from_module, &segments)
}

fn intrinsic_callable_signature_for_source_file(
    source_file: &SourceFile,
    segments: &[String],
) -> Option<CallableSignature> {
    let expanded = signature_help_segments(source_file, segments)?;
    intrinsic_callable_signature(&expanded)
}

fn signature_help_segments(source_file: &SourceFile, segments: &[String]) -> Option<Vec<String>> {
    crate::expand_unique_import_alias(source_file, segments).ok()
}

fn source_file_resource_constructor_signature(
    program: &CheckedProgram,
    file: &Path,
    parsed: &ParsedSource,
    segments: &[String],
) -> Option<ResourceConstructorSignature> {
    let source_file = &parsed.file;
    let resource_name = current_source_resource_name(source_file, segments)?;
    let resource = unique_source_resource(source_file, resource_name)?;
    let module_name = &source_file.module.as_ref()?.name;
    let prelude = crate::checks::file_prelude(program, file, parsed);
    let ty = MarrowType::Resource(crate::resource_type_name(module_name, &resource.name));
    let fields = resource
        .members
        .iter()
        .filter_map(|member| {
            let ResourceMember::Field(field) = member else {
                return None;
            };
            if !field.keys.is_empty() {
                return None;
            }
            Some(ResourceConstructorField {
                name: field.name.clone(),
                required: field.required,
                ty: crate::enums::resolve_type(&field.ty, program, &prelude.aliases, file),
                docs: field.docs.clone(),
            })
        })
        .collect();

    Some(ResourceConstructorSignature {
        name: resource.name.clone(),
        ty,
        docs: resource.docs.clone(),
        fields,
    })
}

fn unique_source_resource<'a>(source_file: &'a SourceFile, name: &str) -> Option<&'a ResourceDecl> {
    let mut declarations = source_file
        .declarations
        .iter()
        .filter(|declaration| source_declaration_name(declaration) == Some(name));
    let declaration = declarations.next()?;
    if declarations.next().is_some() {
        return None;
    }
    match declaration {
        Declaration::Resource(resource) => Some(resource),
        _ => None,
    }
}

fn source_declaration_name(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::Const(decl) => Some(decl.name.as_str()),
        Declaration::Resource(decl) => Some(decl.name.as_str()),
        Declaration::Store(_) => None,
        Declaration::Surface(decl) => Some(decl.name.as_str()),
        Declaration::Function(decl) => Some(decl.name.as_str()),
        Declaration::Enum(decl) => Some(decl.name.as_str()),
        Declaration::Evolve(_) => None,
    }
}

fn current_source_resource_name<'a>(
    source_file: &SourceFile,
    segments: &'a [String],
) -> Option<&'a str> {
    match segments {
        [name] => Some(name.as_str()),
        [module_segments @ .., name] => {
            let module_name = source_file.module.as_ref()?.name.as_str();
            module_segments
                .iter()
                .map(String::as_str)
                .eq(module_name.split("::"))
                .then_some(name.as_str())
        }
        [] => None,
    }
}

fn source_intrinsic_signature(callable: CallableSignature) -> SourceSignatureHelpCallable {
    SourceSignatureHelpCallable::Intrinsic {
        path: callable.path,
        argument_style: callable.argument_style,
        docs: callable.docs,
        params: callable
            .params
            .into_iter()
            .map(|param| source_intrinsic_parameter(param, callable.argument_style))
            .collect(),
        return_shape: callable.return_shape,
    }
}

fn source_intrinsic_parameter(
    param: CallableParameter,
    style: CallableArgumentStyle,
) -> SourceSignatureHelpParameter {
    SourceSignatureHelpParameter {
        name: match style {
            CallableArgumentStyle::Positional => None,
            CallableArgumentStyle::NamedFields => Some(param.label.clone()),
        },
        label: param.label,
        required: param.required,
        repeat: param.repeat,
        ty: shape_type(&param.shape).cloned(),
        shape: Some(param.shape),
        docs: param.docs,
    }
}

fn source_resource_constructor_signature(
    resource: ResourceConstructorSignature,
) -> SourceSignatureHelpCallable {
    SourceSignatureHelpCallable::ResourceConstructor {
        name: resource.name,
        docs: resource.docs,
        params: resource
            .fields
            .into_iter()
            .map(|field| SourceSignatureHelpParameter {
                name: Some(field.name.clone()),
                label: field.name,
                required: field.required,
                repeat: false,
                ty: Some(field.ty),
                shape: None,
                docs: field.docs,
            })
            .collect(),
        return_type: resource.ty,
    }
}

fn source_function_signature(
    program: &CheckedProgram,
    snapshot: Option<&AnalysisSnapshot>,
    from_module: &str,
    segments: &[String],
) -> Option<SourceSignatureHelpCallable> {
    let Resolution::Found(Def {
        module,
        item: DefItem::Function(function),
        ..
    }) = crate::resolve(program, from_module, segments, ResolvableKind::Function)
    else {
        return None;
    };
    let parsed = snapshot
        .and_then(|snapshot| parsed_function_decl(snapshot, &module.source_file, function.span));
    Some(source_checked_function_signature(function, parsed))
}

fn source_checked_function_signature(
    function: &CheckedFunction,
    parsed: Option<&FunctionDecl>,
) -> SourceSignatureHelpCallable {
    SourceSignatureHelpCallable::Function {
        name: function.name.clone(),
        docs: parsed
            .map(|function| function.docs.clone())
            .unwrap_or_default(),
        params: function
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let docs = parsed
                    .and_then(|function| function.params.get(index))
                    .filter(|decl| decl.name == param.name)
                    .map(|decl| decl.docs.clone())
                    .unwrap_or_default();
                SourceSignatureHelpParameter {
                    name: Some(param.name.clone()),
                    label: param.name.clone(),
                    required: true,
                    repeat: false,
                    ty: Some(param.ty.clone()),
                    shape: None,
                    docs,
                }
            })
            .collect(),
        return_type: function.return_type.clone(),
    }
}

fn parsed_function_decl<'a>(
    snapshot: &'a AnalysisSnapshot,
    file: &Path,
    span: SourceSpan,
) -> Option<&'a FunctionDecl> {
    let analyzed = snapshot
        .files
        .iter()
        .find(|file_info| file_info.path == file)?;
    analyzed
        .parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if function.span == span => Some(function),
            _ => None,
        })
}

fn shape_type(shape: &CallableValueShape) -> Option<&MarrowType> {
    match shape {
        CallableValueShape::Type(ty) => Some(ty),
        _ => None,
    }
}

fn builtin_signature(name: &str) -> Option<CallableSignature> {
    let descriptor = CheckedBuiltinCall::descriptor_for_name(name)?;
    Some(builtin_signature_from_descriptor(descriptor))
}

fn builtin_signature_from_descriptor(
    descriptor: &crate::executable::CheckedBuiltinCallDescriptor,
) -> CallableSignature {
    CallableSignature {
        path: vec![descriptor.spelling.to_string()],
        kind: CallableSignatureKind::Builtin,
        argument_style: CallableArgumentStyle::Positional,
        docs: vec![descriptor.docs.to_string()],
        params: descriptor
            .params
            .iter()
            .map(|param| CallableParameter {
                label: param.label.to_string(),
                required: true,
                repeat: false,
                shape: builtin_value_shape(param.shape),
                docs: Vec::new(),
            })
            .collect(),
        return_shape: builtin_return_shape(descriptor.return_shape),
    }
}

fn conversion_signature(name: &str) -> Option<CallableSignature> {
    Some(conversion_signature_for_target(
        ConversionTarget::from_name(name)?,
    ))
}

fn conversion_signature_for_target(target: ConversionTarget) -> CallableSignature {
    CallableSignature {
        path: vec![target.spelling().to_string()],
        kind: CallableSignatureKind::ScalarConversion,
        argument_style: CallableArgumentStyle::Positional,
        docs: Vec::new(),
        params: vec![param("value", CallableValueShape::Value)],
        return_shape: Some(match target {
            ConversionTarget::ErrorCode => CallableValueShape::ErrorCode,
            _ => CallableValueShape::Type(target.return_type()),
        }),
    }
}

fn error_signature(name: &str) -> Option<CallableSignature> {
    (name == "Error").then(|| CallableSignature {
        path: vec![name.to_string()],
        kind: CallableSignatureKind::ErrorConstructor,
        argument_style: CallableArgumentStyle::NamedFields,
        docs: Vec::new(),
        params: marrow_schema::error::fields()
            .iter()
            .map(|field| CallableParameter {
                label: field.name.to_string(),
                required: field.required,
                repeat: false,
                shape: error_field_shape(field),
                docs: Vec::new(),
            })
            .collect(),
        return_shape: Some(CallableValueShape::Type(MarrowType::Error)),
    })
}

fn std_signature(module: &str, op: &str) -> Option<CallableSignature> {
    let op = stdlib::lookup(module, op)?;
    Some(CallableSignature {
        path: vec!["std".to_string(), op.module.to_string(), op.op.to_string()],
        kind: CallableSignatureKind::StandardLibrary,
        argument_style: CallableArgumentStyle::Positional,
        docs: Vec::new(),
        params: op
            .params
            .iter()
            .map(|param| CallableParameter {
                label: std_param_label(param),
                required: true,
                repeat: false,
                shape: std_param_shape(param),
                docs: Vec::new(),
            })
            .collect(),
        return_shape: std_return_shape(&op.ret),
    })
}

fn param(name: &str, shape: CallableValueShape) -> CallableParameter {
    CallableParameter {
        label: name.to_string(),
        required: true,
        repeat: false,
        shape,
        docs: Vec::new(),
    }
}

fn identity_signature(name: &str) -> Option<CallableSignature> {
    (name == "Id").then(|| CallableSignature {
        path: vec![name.to_string()],
        kind: CallableSignatureKind::IdentityConstructor,
        argument_style: CallableArgumentStyle::Positional,
        docs: Vec::new(),
        params: vec![
            param("root", CallableValueShape::SavedRoot),
            CallableParameter {
                label: "key".to_string(),
                required: true,
                repeat: true,
                shape: CallableValueShape::Value,
                docs: Vec::new(),
            },
        ],
        return_shape: Some(CallableValueShape::Identity),
    })
}

fn builtin_return_shape(shape: CheckedBuiltinReturnShape) -> Option<CallableValueShape> {
    match shape {
        CheckedBuiltinReturnShape::Void => None,
        CheckedBuiltinReturnShape::Value(shape) => Some(builtin_value_shape(shape)),
    }
}

fn builtin_value_shape(shape: CheckedBuiltinValueShape) -> CallableValueShape {
    match shape {
        CheckedBuiltinValueShape::Value => CallableValueShape::Value,
        CheckedBuiltinValueShape::Collection => CallableValueShape::Collection,
        CheckedBuiltinValueShape::Sequence => CallableValueShape::Sequence,
        CheckedBuiltinValueShape::SavedPath => CallableValueShape::SavedPath,
        CheckedBuiltinValueShape::SavedLayer => CallableValueShape::SavedLayer,
        CheckedBuiltinValueShape::SavedRoot => CallableValueShape::SavedRoot,
        CheckedBuiltinValueShape::Identity => CallableValueShape::Identity,
        CheckedBuiltinValueShape::Scalar(scalar) => {
            CallableValueShape::Type(MarrowType::Primitive(scalar))
        }
    }
}

fn error_field_shape(field: &marrow_schema::error::ErrorField) -> CallableValueShape {
    if field.name == marrow_schema::error::CODE {
        return CallableValueShape::ErrorCode;
    }
    CallableValueShape::Type(MarrowType::from_resolved(
        field.ty.clone(),
        TypeNames::default(),
    ))
}

fn std_param_shape(param: &ParamType) -> CallableValueShape {
    match param {
        ParamType::Scalar(scalar) => CallableValueShape::Type(MarrowType::Primitive(*scalar)),
        ParamType::ScalarAny => CallableValueShape::Scalar,
        ParamType::Sequence(scalar) => CallableValueShape::Type(MarrowType::Sequence(Box::new(
            MarrowType::Primitive(*scalar),
        ))),
        ParamType::Error => CallableValueShape::Type(MarrowType::Error),
        ParamType::Path => CallableValueShape::SavedPath,
    }
}

fn std_param_label(param: &ParamType) -> String {
    match param {
        ParamType::Scalar(scalar) => scalar.name().to_string(),
        ParamType::ScalarAny => "scalar".to_string(),
        ParamType::Sequence(scalar) => format!("sequence[{}]", scalar.name()),
        ParamType::Error => "Error".to_string(),
        ParamType::Path => "path".to_string(),
    }
}

fn std_return_shape(ret: &ReturnType) -> Option<CallableValueShape> {
    match ret {
        ReturnType::Scalar(scalar) => {
            Some(CallableValueShape::Type(MarrowType::Primitive(*scalar)))
        }
        // A maybe-present op renders its result type as `T?` in a signature.
        ReturnType::OptionalScalar(scalar) => Some(CallableValueShape::Type(MarrowType::optional(
            MarrowType::Primitive(*scalar),
        ))),
        ReturnType::Sequence(scalar) => Some(CallableValueShape::Type(MarrowType::Sequence(
            Box::new(MarrowType::Primitive(*scalar)),
        ))),
        ReturnType::Void => None,
    }
}
