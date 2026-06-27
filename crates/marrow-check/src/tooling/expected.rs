use std::path::Path;

use marrow_schema::EnumSchema;
use marrow_syntax::{
    Block, Declaration, ElseIf, Expression, LexedSource, ParsedSource, SourceFile, Statement,
    TypeRef,
};

use crate::analysis::span_covers;
use crate::checks::file_prelude;
use crate::enums::{enum_schema_in, resolve_type};
use crate::walk::for_each_child_expr;
use crate::{CheckedModule, CheckedProgram, MarrowType};

use super::signatures::{active_signature_help_parameter, source_signature_help_fact_at};

pub(super) struct ExpectedEnum<'a> {
    pub schema: &'a EnumSchema,
    pub value_prefix: String,
}

pub(super) fn expected_enum_at<'a>(
    program: &'a CheckedProgram,
    file: &Path,
    source: &str,
    parsed: &ParsedSource,
    lexed: &LexedSource,
    offset: usize,
) -> Option<ExpectedEnum<'a>> {
    let prelude = file_prelude(program, file, parsed);
    if let Some(ty) = parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if span_covers(function.body.span, offset) => {
                expected_type_ref_in_block(&function.body, function.return_type.as_ref(), offset)
            }
            _ => None,
        })
    {
        let resolved = resolve_type(ty, program, &prelude.aliases, file);
        return expected_enum_from_type(program, file, &resolved, ty.text.clone());
    }

    let ty = expected_call_argument_type(program, file, source, lexed, offset)?;
    let prefix = enum_value_prefix_for_type(program, file, &parsed.file, &ty)?;
    expected_enum_from_type(program, file, &ty, prefix)
}

fn expected_enum_from_type<'a>(
    program: &'a CheckedProgram,
    file: &Path,
    ty: &MarrowType,
    value_prefix: String,
) -> Option<ExpectedEnum<'a>> {
    let MarrowType::Enum { module, name } = ty else {
        return None;
    };
    let schema = enum_schema_in(program, module, name)?;
    if !enum_visible_from_file(program, file, module, name) {
        return None;
    }
    Some(ExpectedEnum {
        schema,
        value_prefix,
    })
}

fn expected_call_argument_type(
    program: &CheckedProgram,
    file: &Path,
    source: &str,
    lexed: &LexedSource,
    offset: usize,
) -> Option<MarrowType> {
    let fact = source_signature_help_fact_at(program, None, file, source, lexed, offset)?;
    active_signature_help_parameter(&fact).and_then(|parameter| parameter.ty.clone())
}

fn enum_value_prefix_for_type(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
    ty: &MarrowType,
) -> Option<String> {
    let MarrowType::Enum { module, name } = ty else {
        return None;
    };
    if current_module(program, file).is_some_and(|current| current.name == *module) {
        return Some(name.clone());
    }
    if bare_enum_path_resolves_to(program, file, source_file, module, name) {
        return Some(name.clone());
    }
    crate::unique_import_alias_for_module(source_file, module)
        .ok()
        .flatten()
        .map_or_else(
            || Some(format!("{module}::{name}")),
            |alias| Some(format!("{alias}::{name}")),
        )
}

fn bare_enum_path_resolves_to(
    program: &CheckedProgram,
    file: &Path,
    source_file: &SourceFile,
    target_module: &str,
    enum_name: &str,
) -> bool {
    if current_module(program, file).is_none_or(|current| current.name != target_module)
        && crate::source_declares_top_level_name(source_file, enum_name)
    {
        return false;
    }
    let mut matches = program.modules.iter().filter(|module| {
        module
            .enums
            .iter()
            .any(|enum_schema| enum_schema.name == enum_name)
            && module_enum_visible_from_file(module, enum_name, file)
    });
    let Some(module) = matches.next() else {
        return false;
    };
    matches.next().is_none() && module.name == target_module
}

fn enum_visible_from_file(
    program: &CheckedProgram,
    file: &Path,
    module_name: &str,
    enum_name: &str,
) -> bool {
    current_module(program, file).is_some_and(|module| module.name == module_name)
        || program
            .modules
            .iter()
            .find(|module| module.name == module_name)
            .is_some_and(|module| module_enum_visible_from_file(module, enum_name, file))
}

fn module_enum_visible_from_file(module: &CheckedModule, enum_name: &str, file: &Path) -> bool {
    module.source_file == file || module.enum_public.get(enum_name).copied().unwrap_or(false)
}

fn current_module<'a>(program: &'a CheckedProgram, file: &Path) -> Option<&'a CheckedModule> {
    program
        .modules
        .iter()
        .find(|module| module.source_file == file)
}

fn expected_type_ref_in_block<'a>(
    block: &'a Block,
    function_return_type: Option<&'a TypeRef>,
    offset: usize,
) -> Option<&'a TypeRef> {
    for statement in &block.statements {
        if !span_covers(statement.span(), offset) {
            continue;
        }
        if let Some(ty) = expected_type_ref_for_statement(statement, function_return_type, offset) {
            return Some(ty);
        }
        return expected_type_ref_in_nested_block(statement, function_return_type, offset);
    }
    None
}

fn expected_type_ref_for_statement<'a>(
    statement: &'a Statement,
    function_return_type: Option<&'a TypeRef>,
    offset: usize,
) -> Option<&'a TypeRef> {
    match statement {
        Statement::Const {
            ty: Some(ty),
            value,
            ..
        }
        | Statement::Var {
            ty: Some(ty),
            value: Some(value),
            ..
        } if cursor_on_value_expression(value, offset) => Some(ty),
        Statement::Return {
            value: Some(value), ..
        } if cursor_on_value_expression(value, offset) => function_return_type,
        _ => None,
    }
}

fn expected_type_ref_in_nested_block<'a>(
    statement: &'a Statement,
    function_return_type: Option<&'a TypeRef>,
    offset: usize,
) -> Option<&'a TypeRef> {
    match statement {
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        }
        | Statement::IfConst {
            then_block,
            else_ifs,
            else_block,
            ..
        } => expected_type_ref_in_conditional_blocks(
            then_block,
            else_ifs,
            else_block.as_ref(),
            function_return_type,
            offset,
        ),
        Statement::While { body, .. }
        | Statement::For { body, .. }
        | Statement::Transaction { body, .. } => {
            expected_type_ref_in_body(body, function_return_type, offset)
        }
        Statement::Try { body, catch, .. } => {
            if let Some(ty) = expected_type_ref_in_body(body, function_return_type, offset) {
                return Some(ty);
            }
            catch.as_ref().and_then(|catch| {
                expected_type_ref_in_body(&catch.block, function_return_type, offset)
            })
        }
        Statement::Match { arms, .. } => arms
            .iter()
            .find_map(|arm| expected_type_ref_in_body(&arm.block, function_return_type, offset)),
        _ => None,
    }
}

fn expected_type_ref_in_body<'a>(
    body: &'a Block,
    function_return_type: Option<&'a TypeRef>,
    offset: usize,
) -> Option<&'a TypeRef> {
    if span_covers(body.span, offset) {
        expected_type_ref_in_block(body, function_return_type, offset)
    } else {
        None
    }
}

fn expected_type_ref_in_conditional_blocks<'a>(
    then_block: &'a Block,
    else_ifs: &'a [ElseIf],
    else_block: Option<&'a Block>,
    function_return_type: Option<&'a TypeRef>,
    offset: usize,
) -> Option<&'a TypeRef> {
    if span_covers(then_block.span, offset) {
        return expected_type_ref_in_block(then_block, function_return_type, offset);
    }
    for else_if in else_ifs {
        if span_covers(else_if.block.span, offset) {
            return expected_type_ref_in_block(&else_if.block, function_return_type, offset);
        }
    }
    else_block.and_then(|block| expected_type_ref_in_body(block, function_return_type, offset))
}

fn cursor_on_value_expression(value: &Expression, offset: usize) -> bool {
    if !span_covers(value.span(), offset) {
        return false;
    }
    let mut child_covers = false;
    for_each_child_expr(value, |child| {
        child_covers |= span_covers(child.span(), offset);
    });
    !child_covers
}
