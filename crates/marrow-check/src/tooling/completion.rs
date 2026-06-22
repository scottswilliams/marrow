use std::path::Path;

use marrow_schema::{EnumSchema, ResourceSchema};
use marrow_syntax::{Declaration, FunctionDecl, SourceFile};

use crate::{CheckedFunction, CheckedModule, CheckedProgram, MarrowType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceNamespaceCompletionFact {
    Module(SourceModuleNamespaceCompletionFact),
    Enum(SourceEnumNamespaceCompletionFact),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceModuleNamespaceCompletionFact {
    pub module: String,
    pub resources: Vec<SourceNamespaceResourceCompletion>,
    pub enums: Vec<SourceNamespaceEnumCompletion>,
    pub functions: Vec<SourceNamespaceFunctionCompletion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceNamespaceResourceCompletion {
    pub name: String,
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceNamespaceEnumCompletion {
    pub name: String,
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceNamespaceFunctionCompletion {
    pub name: String,
    pub params: Vec<SourceNamespaceFunctionParamCompletion>,
    pub return_type: Option<MarrowType>,
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceNamespaceFunctionParamCompletion {
    pub name: String,
    pub ty: MarrowType,
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEnumNamespaceCompletionFact {
    pub enum_name: String,
    pub members: Vec<SourceNamespaceEnumMemberCompletion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceNamespaceEnumMemberCompletion {
    pub name: String,
    pub docs: Vec<String>,
    pub status: SourceNamespaceEnumMemberStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceNamespaceEnumMemberStatus {
    Selectable,
    Category,
    Group,
}

pub fn source_namespace_completion_fact(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
    qualifier: &[String],
) -> Option<SourceNamespaceCompletionFact> {
    let expanded = expand_namespace_qualifier(source_file, qualifier)?;
    namespace_completion_fact_for_expanded_qualifier(program, file, source_file, &expanded)
}

pub fn source_namespace_completion_file_fact(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
    qualifier: &[String],
) -> Option<SourceNamespaceCompletionFact> {
    let expanded = expand_file_namespace_qualifier(source_file, qualifier)?;
    namespace_completion_fact_for_expanded_qualifier(program, file, source_file, &expanded)
}

fn namespace_completion_fact_for_expanded_qualifier(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
    expanded: &[String],
) -> Option<SourceNamespaceCompletionFact> {
    if let Some(module) = module_for_segments(program, expanded) {
        return Some(SourceNamespaceCompletionFact::Module(
            module_completion_fact(program, file, source_file, module),
        ));
    }
    enum_for_segments(program, file, expanded)
        .map(enum_completion_fact)
        .map(SourceNamespaceCompletionFact::Enum)
}

fn expand_namespace_qualifier(
    source_file: &SourceFile,
    qualifier: &[String],
) -> Option<Vec<String>> {
    match qualifier {
        [] => None,
        [segment] => expand_unique_import_module_alias(source_file, segment),
        _ => crate::expand_unique_import_alias(source_file, qualifier).ok(),
    }
}

fn expand_file_namespace_qualifier(
    source_file: &SourceFile,
    qualifier: &[String],
) -> Option<Vec<String>> {
    match qualifier {
        [] => None,
        [segment] => expand_unique_file_import_module_alias(source_file, segment),
        _ => expand_unique_file_import_alias(source_file, qualifier),
    }
}

fn expand_unique_import_module_alias(
    source_file: &SourceFile,
    segment: &str,
) -> Option<Vec<String>> {
    let mut matches = source_file
        .uses
        .iter()
        .filter(|use_decl| crate::short_name(&use_decl.name) == segment);
    let Some(import) = matches.next() else {
        return Some(vec![segment.to_string()]);
    };
    if matches.next().is_some() || crate::source_declares_top_level_name(source_file, segment) {
        return None;
    }
    Some(crate::split_type_path(&import.name))
}

fn expand_unique_file_import_module_alias(
    source_file: &SourceFile,
    segment: &str,
) -> Option<Vec<String>> {
    let mut matches = source_file
        .uses
        .iter()
        .filter(|use_decl| crate::short_name(&use_decl.name) == segment);
    let Some(import) = matches.next() else {
        return Some(vec![segment.to_string()]);
    };
    if matches.next().is_some() || crate::import_alias_head_is_file_shadowed(source_file, segment) {
        return None;
    }
    Some(crate::split_type_path(&import.name))
}

fn expand_unique_file_import_alias(
    source_file: &SourceFile,
    qualifier: &[String],
) -> Option<Vec<String>> {
    let head = qualifier.first()?;
    let mut matches = source_file
        .uses
        .iter()
        .filter(|use_decl| crate::short_name(&use_decl.name) == head.as_str());
    let Some(import) = matches.next() else {
        return Some(qualifier.to_vec());
    };
    if matches.next().is_some() || crate::import_alias_head_is_file_shadowed(source_file, head) {
        return None;
    }
    Some(
        crate::split_type_path(&import.name)
            .into_iter()
            .chain(qualifier[1..].iter().cloned())
            .collect(),
    )
}

fn module_for_segments<'a>(
    program: &'a CheckedProgram,
    segments: &[String],
) -> Option<&'a CheckedModule> {
    let module_name = segments.join("::");
    program
        .modules
        .iter()
        .find(|module| module.name == module_name)
}

fn enum_for_segments<'a>(
    program: &'a CheckedProgram,
    file: &Path,
    segments: &[String],
) -> Option<&'a EnumSchema> {
    let (enum_name, module_segments) = segments.split_last()?;
    let module = if module_segments.is_empty() {
        current_module(program, file)?
    } else {
        module_for_segments(program, module_segments)?
    };
    module.enums.iter().find(|enum_schema| {
        enum_schema.name == *enum_name
            && enum_visible_from_file(module, enum_schema.name.as_str(), file)
    })
}

fn current_module<'a>(program: &'a CheckedProgram, file: &Path) -> Option<&'a CheckedModule> {
    program
        .modules
        .iter()
        .find(|module| module.source_file == file)
}

fn enum_visible_from_file(module: &CheckedModule, enum_name: &str, file: &Path) -> bool {
    module.source_file == file || module.enum_public.get(enum_name).copied().unwrap_or(false)
}

fn module_completion_fact(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
    module: &CheckedModule,
) -> SourceModuleNamespaceCompletionFact {
    SourceModuleNamespaceCompletionFact {
        module: module.name.clone(),
        resources: module.resources.iter().map(resource_completion).collect(),
        enums: module
            .enums
            .iter()
            .filter(|enum_schema| enum_visible_from_file(module, enum_schema.name.as_str(), file))
            .map(enum_completion)
            .collect(),
        functions: function_completions(program, file, source_file, module),
    }
}

fn resource_completion(resource: &ResourceSchema) -> SourceNamespaceResourceCompletion {
    SourceNamespaceResourceCompletion {
        name: resource.name.clone(),
        docs: resource.docs.clone(),
    }
}

fn enum_completion(enum_schema: &EnumSchema) -> SourceNamespaceEnumCompletion {
    SourceNamespaceEnumCompletion {
        name: enum_schema.name.clone(),
        docs: enum_schema.docs.clone(),
    }
}

fn function_completions(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
    module: &CheckedModule,
) -> Vec<SourceNamespaceFunctionCompletion> {
    let parsed_functions = parsed_functions_for_file(program, file, source_file, module);
    module
        .functions
        .iter()
        .enumerate()
        .filter(|(_, function)| module.source_file == file || function.public)
        .map(|(index, function)| {
            function_completion_fact(
                function,
                parsed_functions
                    .as_ref()
                    .and_then(|functions| functions.get(index)),
            )
        })
        .collect()
}

fn parsed_functions_for_file<'a>(
    program: &CheckedProgram,
    file: &Path,
    source_file: &'a SourceFile,
    module: &CheckedModule,
) -> Option<Vec<&'a FunctionDecl>> {
    if module.source_file != file || current_module(program, file)?.name != module.name {
        return None;
    }
    Some(
        source_file
            .declarations
            .iter()
            .filter_map(|declaration| match declaration {
                Declaration::Function(function) => Some(function),
                _ => None,
            })
            .collect(),
    )
}

fn function_completion_fact(
    function: &CheckedFunction,
    parsed: Option<&&FunctionDecl>,
) -> SourceNamespaceFunctionCompletion {
    SourceNamespaceFunctionCompletion {
        name: function.name.clone(),
        params: function
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| SourceNamespaceFunctionParamCompletion {
                name: param.name.clone(),
                ty: param.ty.clone(),
                docs: parsed
                    .and_then(|function| function.params.get(index))
                    .map(|param| param.docs.clone())
                    .unwrap_or_default(),
            })
            .collect(),
        return_type: function.return_type.clone(),
        docs: parsed
            .map(|function| function.docs.clone())
            .unwrap_or_default(),
    }
}

fn enum_completion_fact(enum_schema: &EnumSchema) -> SourceEnumNamespaceCompletionFact {
    SourceEnumNamespaceCompletionFact {
        enum_name: enum_schema.name.clone(),
        members: enum_schema
            .members
            .iter()
            .enumerate()
            .map(|(ordinal, member)| SourceNamespaceEnumMemberCompletion {
                name: member.name.clone(),
                docs: member.docs.clone(),
                status: enum_member_status(enum_schema, ordinal),
            })
            .collect(),
    }
}

fn enum_member_status(enum_schema: &EnumSchema, ordinal: usize) -> SourceNamespaceEnumMemberStatus {
    if enum_schema.is_selectable_leaf(ordinal) {
        SourceNamespaceEnumMemberStatus::Selectable
    } else if enum_schema.is_category(ordinal) {
        SourceNamespaceEnumMemberStatus::Category
    } else {
        SourceNamespaceEnumMemberStatus::Group
    }
}
