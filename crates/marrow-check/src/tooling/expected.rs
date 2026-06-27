use std::path::Path;

use marrow_schema::EnumSchema;
use marrow_syntax::{Block, Declaration, ElseIf, Expression, ParsedSource, Statement, TypeRef};

use crate::analysis::span_covers;
use crate::checks::file_prelude;
use crate::enums::{enum_schema_in, resolve_type};
use crate::walk::for_each_child_expr;
use crate::{CheckedProgram, MarrowType};

pub(super) struct ExpectedEnum<'a> {
    pub schema: &'a EnumSchema,
    pub value_prefix: String,
}

pub(super) fn expected_enum_at<'a>(
    program: &'a CheckedProgram,
    file: &Path,
    parsed: &ParsedSource,
    offset: usize,
) -> Option<ExpectedEnum<'a>> {
    let prelude = file_prelude(program, file, parsed);
    let ty = parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if span_covers(function.body.span, offset) => {
                expected_type_ref_in_block(&function.body, function.return_type.as_ref(), offset)
            }
            _ => None,
        })?;
    let MarrowType::Enum { module, name } = resolve_type(ty, program, &prelude.aliases, file)
    else {
        return None;
    };
    Some(ExpectedEnum {
        schema: enum_schema_in(program, &module, &name)?,
        value_prefix: ty.text.clone(),
    })
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
