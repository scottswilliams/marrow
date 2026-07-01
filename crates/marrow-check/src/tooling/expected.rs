use std::collections::HashMap;
use std::path::Path;

use marrow_schema::EnumSchema;
use marrow_syntax::{
    Block, Declaration, ElseIf, Expression, Keyword, LexedSource, MatchArm, ParsedSource,
    SourceFile, SourceSpan, Statement, Token, TokenKind, TypeRef, active_callable_context,
};

use crate::analysis::{scope_at, span_covers};
use crate::checks::file_prelude;
use crate::enums::{enum_schema_in, resolve_type};
use crate::infer::{infer_assignment_target_type_with_read_scope, infer_only};
use crate::walk::for_each_child_expr;
use crate::{CheckedModule, CheckedProgram, MarrowType};

use super::signatures::{active_signature_help_parameter, source_signature_help_fact_at};

pub(super) struct ExpectedEnum<'a> {
    pub schema: &'a EnumSchema,
    pub context: ExpectedEnumContext,
}

pub(super) enum ExpectedEnumContext {
    Value { prefix: String },
    MatchArm,
}

enum ExpectedSourceContext<'a> {
    TypeRef(&'a TypeRef),
    AssignmentTarget(&'a Expression),
    MatchArmScrutinee(&'a Expression),
}

pub(super) fn expected_enum_at<'a>(
    program: &'a CheckedProgram,
    file: &Path,
    source: &str,
    parsed: &ParsedSource,
    lexed: &LexedSource,
    offset: usize,
) -> Option<ExpectedEnum<'a>> {
    if active_callable_context(source, lexed, parsed, offset).is_some() {
        let fact = source_signature_help_fact_at(program, None, file, source, lexed, offset)?;
        let ty =
            active_signature_help_parameter(&fact).and_then(|parameter| parameter.ty.clone())?;
        let prefix = enum_value_prefix_for_type(program, file, &parsed.file, &ty)?;
        return expected_enum_from_type(program, file, &ty, ExpectedEnumContext::Value { prefix });
    }

    let prelude = file_prelude(program, file, parsed);
    if let Some(context) = parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if span_covers(function.body.span, offset) => {
                expected_source_context_in_block(
                    &function.body,
                    function.return_type.as_ref(),
                    offset,
                )
            }
            _ => None,
        })
    {
        return match context {
            ExpectedSourceContext::TypeRef(ty) => {
                let resolved = resolve_type(ty, program, &prelude.aliases, file);
                expected_enum_from_type(
                    program,
                    file,
                    &resolved,
                    ExpectedEnumContext::Value {
                        prefix: ty.text.clone(),
                    },
                )
            }
            ExpectedSourceContext::AssignmentTarget(target) => {
                let scope = scope_stack_at(program, file, parsed, offset);
                let ty = infer_assignment_target_type_with_read_scope(
                    program,
                    target,
                    &scope,
                    &[],
                    &prelude.aliases,
                    file,
                    &mut Vec::new(),
                    crate::presence::ReadScope::none(),
                );
                let prefix = enum_value_prefix_for_type(program, file, &parsed.file, &ty)?;
                expected_enum_from_type(program, file, &ty, ExpectedEnumContext::Value { prefix })
            }
            ExpectedSourceContext::MatchArmScrutinee(scrutinee) => {
                let scope = scope_stack_at(program, file, parsed, offset);
                let ty = infer_only(program, scrutinee, &scope, &prelude.aliases, file);
                expected_enum_from_type(program, file, &ty, ExpectedEnumContext::MatchArm)
            }
        };
    }

    recovered_expected_enum_at(program, file, source, parsed, lexed, offset, &prelude)
}

fn expected_enum_from_type<'a>(
    program: &'a CheckedProgram,
    file: &Path,
    ty: &MarrowType,
    context: ExpectedEnumContext,
) -> Option<ExpectedEnum<'a>> {
    let MarrowType::Enum { module, name } = ty else {
        return None;
    };
    let schema = enum_schema_in(program, module, name)?;
    if !enum_visible_from_file(program, file, module, name) {
        return None;
    }
    Some(ExpectedEnum { schema, context })
}

fn scope_stack_at(
    program: &CheckedProgram,
    file: &Path,
    parsed: &ParsedSource,
    offset: usize,
) -> Vec<HashMap<String, MarrowType>> {
    vec![
        scope_at(program, file, parsed, offset)
            .into_iter()
            .collect(),
    ]
}

fn recovered_expected_enum_at<'a>(
    program: &'a CheckedProgram,
    file: &Path,
    source: &str,
    parsed: &ParsedSource,
    lexed: &LexedSource,
    offset: usize,
    prelude: &crate::checks::FilePrelude,
) -> Option<ExpectedEnum<'a>> {
    let (line_start, line_end) = line_bounds(source, offset);
    let line = significant_line_tokens(lexed, line_start, line_end);

    if let Some(ty) = recovered_empty_binding_type(source, &line, offset) {
        let resolved = resolve_type(&ty, program, &prelude.aliases, file);
        return expected_enum_from_type(
            program,
            file,
            &resolved,
            ExpectedEnumContext::Value { prefix: ty.text },
        );
    }

    if line.first().is_some_and(|token| {
        token.kind == TokenKind::Keyword(Keyword::Return) && offset >= token.span.end_byte
    }) && let Some(ty) = enclosing_function_return_type(parsed, offset)
    {
        let resolved = resolve_type(ty, program, &prelude.aliases, file);
        return expected_enum_from_type(
            program,
            file,
            &resolved,
            ExpectedEnumContext::Value {
                prefix: ty.text.clone(),
            },
        );
    }

    if let Some(target) = recovered_assignment_target(source, &line, offset) {
        let scope = scope_stack_at(program, file, parsed, offset);
        let ty = infer_assignment_target_type_with_read_scope(
            program,
            &target,
            &scope,
            &[],
            &prelude.aliases,
            file,
            &mut Vec::new(),
            crate::presence::ReadScope::none(),
        );
        let prefix = enum_value_prefix_for_type(program, file, &parsed.file, &ty)?;
        return expected_enum_from_type(program, file, &ty, ExpectedEnumContext::Value { prefix });
    }

    if let Some(scrutinee) = recovered_match_arm_scrutinee(source, parsed, lexed, line_start) {
        let scope = scope_stack_at(program, file, parsed, offset);
        let ty = infer_only(program, scrutinee, &scope, &prelude.aliases, file);
        return expected_enum_from_type(program, file, &ty, ExpectedEnumContext::MatchArm);
    }

    None
}

fn line_bounds(source: &str, offset: usize) -> (usize, usize) {
    let clamped = offset.min(source.len());
    let start = source[..clamped].rfind('\n').map_or(0, |index| index + 1);
    let end = source[clamped..]
        .find('\n')
        .map_or(source.len(), |index| clamped + index);
    (start, end)
}

fn significant_line_tokens(lexed: &LexedSource, line_start: usize, line_end: usize) -> Vec<&Token> {
    lexed
        .tokens
        .iter()
        .filter(|token| line_start <= token.span.start_byte && token.span.start_byte < line_end)
        .filter(|token| {
            !matches!(
                token.kind,
                TokenKind::Indent
                    | TokenKind::Dedent
                    | TokenKind::Newline
                    | TokenKind::Eof
                    | TokenKind::Comment
                    | TokenKind::DocComment
            )
        })
        .collect()
}

fn recovered_empty_binding_type(source: &str, tokens: &[&Token], offset: usize) -> Option<TypeRef> {
    let first = tokens.first()?;
    if !matches!(
        first.kind,
        TokenKind::Keyword(Keyword::Const) | TokenKind::Keyword(Keyword::Var)
    ) {
        return None;
    }
    let equal = top_level_token(tokens, TokenKind::Equal)?;
    if offset < tokens[equal].span.end_byte {
        return None;
    }
    let colon = top_level_token(&tokens[..equal], TokenKind::Colon)?;
    let start = tokens[colon].span.end_byte;
    let end = tokens[equal].span.start_byte;
    let (text, span) = trimmed_slice(source, start, end)?;
    Some(TypeRef { text, span })
}

fn recovered_assignment_target(
    source: &str,
    tokens: &[&Token],
    offset: usize,
) -> Option<Expression> {
    if matches!(
        tokens.first().map(|token| token.kind),
        Some(TokenKind::Keyword(
            Keyword::Const | Keyword::Var | Keyword::Return
        ))
    ) {
        return None;
    }
    let equal = top_level_token(tokens, TokenKind::Equal)?;
    if offset < tokens[equal].span.end_byte {
        return None;
    }
    let first = tokens[..equal].first()?;
    let target_text = source[first.span.start_byte..tokens[equal].span.start_byte].trim();
    parse_recovered_expression(target_text)
}

fn top_level_token(tokens: &[&Token], kind: TokenKind) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen | TokenKind::LeftBracket => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket => depth = depth.saturating_sub(1),
            token_kind if token_kind == kind && depth == 0 => return Some(index),
            _ => {}
        }
    }
    None
}

fn trimmed_slice(source: &str, start: usize, end: usize) -> Option<(String, SourceSpan)> {
    let raw = source.get(start..end)?;
    let leading = raw.len() - raw.trim_start().len();
    let trailing = raw.trim_end().len();
    if leading >= trailing {
        return None;
    }
    let trimmed_start = start + leading;
    let trimmed_end = start + trailing;
    Some((
        source[trimmed_start..trimmed_end].to_string(),
        SourceSpan {
            start_byte: trimmed_start,
            end_byte: trimmed_end,
            line: 0,
            column: 0,
        },
    ))
}

fn parse_recovered_expression(text: &str) -> Option<Expression> {
    if text.is_empty() {
        return None;
    }
    let source =
        format!("module __completion\nfn __completion()\n    const __completion = {text}\n");
    let parsed = marrow_syntax::parse_source(&source);
    parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) => {
                function
                    .body
                    .statements
                    .iter()
                    .find_map(|statement| match statement {
                        Statement::Const { value, .. } => Some(value.clone()),
                        _ => None,
                    })
            }
            _ => None,
        })
}

fn enclosing_function_return_type(parsed: &ParsedSource, offset: usize) -> Option<&TypeRef> {
    parsed
        .file
        .declarations
        .iter()
        .enumerate()
        .find_map(|(index, declaration)| match declaration {
            Declaration::Function(function)
                if function.span.start_byte <= offset
                    && next_declaration_start(&parsed.file.declarations, index)
                        .is_none_or(|next| offset < next) =>
            {
                function.return_type.as_ref()
            }
            _ => None,
        })
}

fn next_declaration_start(declarations: &[Declaration], index: usize) -> Option<usize> {
    declarations.get(index + 1).map(declaration_start)
}

fn declaration_start(declaration: &Declaration) -> usize {
    match declaration {
        Declaration::Const(declaration) => declaration.span.start_byte,
        Declaration::Resource(declaration) => declaration.span.start_byte,
        Declaration::Store(declaration) => declaration.span.start_byte,
        Declaration::Surface(declaration) => declaration.span.start_byte,
        Declaration::Function(declaration) => declaration.span.start_byte,
        Declaration::Enum(declaration) => declaration.span.start_byte,
        Declaration::Evolve(declaration) => declaration.span.start_byte,
    }
}

fn recovered_match_arm_scrutinee<'a>(
    source: &str,
    parsed: &'a ParsedSource,
    lexed: &LexedSource,
    line_start: usize,
) -> Option<&'a Expression> {
    let current_indent = line_indent(source, line_start)?;
    let match_start = nearest_enclosing_match_line(source, lexed, line_start, current_indent)?;
    parsed
        .file
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) => {
                match_scrutinee_with_start(&function.body, match_start)
            }
            _ => None,
        })
}

fn nearest_enclosing_match_line(
    source: &str,
    lexed: &LexedSource,
    mut line_start: usize,
    current_indent: usize,
) -> Option<usize> {
    while line_start > 0 {
        let previous_end = line_start - 1;
        let previous_start = source[..previous_end]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let indent = line_indent(source, previous_start)?;
        if indent < current_indent {
            let line = significant_line_tokens(lexed, previous_start, previous_end);
            if line.is_empty() {
                line_start = previous_start;
                continue;
            }
            return line
                .first()
                .filter(|token| token.kind == TokenKind::Keyword(Keyword::Match))
                .map(|token| token.span.start_byte);
        }
        line_start = previous_start;
    }
    None
}

fn line_indent(source: &str, line_start: usize) -> Option<usize> {
    let line = source.get(line_start..)?;
    let mut indent = 0usize;
    for byte in line.bytes() {
        match byte {
            b' ' => indent += 1,
            b'\n' => return Some(indent),
            _ => return Some(indent),
        }
    }
    Some(indent)
}

fn match_scrutinee_with_start(block: &Block, match_start: usize) -> Option<&Expression> {
    for statement in &block.statements {
        match statement {
            Statement::Match {
                scrutinee: Some(scrutinee),
                arms,
                span,
            } => {
                if span.start_byte == match_start {
                    return Some(scrutinee);
                }
                if let Some(scrutinee) = arms
                    .iter()
                    .find_map(|arm| match_scrutinee_with_start(&arm.block, match_start))
                {
                    return Some(scrutinee);
                }
            }
            _ => {
                if let Some(scrutinee) = nested_statement_blocks(statement)
                    .into_iter()
                    .find_map(|block| match_scrutinee_with_start(block, match_start))
                {
                    return Some(scrutinee);
                }
            }
        }
    }
    None
}

fn nested_statement_blocks(statement: &Statement) -> Vec<&Block> {
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
        } => {
            let mut blocks = vec![then_block];
            blocks.extend(else_ifs.iter().map(|else_if| &else_if.block));
            blocks.extend(else_block.as_ref());
            blocks
        }
        Statement::While { body, .. }
        | Statement::For { body, .. }
        | Statement::Transaction { body, .. } => vec![body],
        Statement::Try { body, catch, .. } => {
            let mut blocks = vec![body];
            blocks.extend(catch.as_ref().map(|catch| &catch.block));
            blocks
        }
        Statement::Match { arms, .. } => arms.iter().map(|arm| &arm.block).collect(),
        _ => Vec::new(),
    }
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
            .module_by_name(module_name)
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

fn expected_source_context_in_block<'a>(
    block: &'a Block,
    function_return_type: Option<&'a TypeRef>,
    offset: usize,
) -> Option<ExpectedSourceContext<'a>> {
    for statement in &block.statements {
        if !span_covers(statement.span(), offset) {
            continue;
        }
        if let Some(context) =
            expected_source_context_for_statement(statement, function_return_type, offset)
        {
            return Some(context);
        }
        return expected_source_context_in_nested_block(statement, function_return_type, offset);
    }
    None
}

fn expected_source_context_for_statement<'a>(
    statement: &'a Statement,
    function_return_type: Option<&'a TypeRef>,
    offset: usize,
) -> Option<ExpectedSourceContext<'a>> {
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
        } if cursor_on_value_expression(value, offset) => Some(ExpectedSourceContext::TypeRef(ty)),
        Statement::Assign { target, value, .. } if cursor_on_value_expression(value, offset) => {
            Some(ExpectedSourceContext::AssignmentTarget(target))
        }
        Statement::Return {
            value: Some(value), ..
        } if cursor_on_value_expression(value, offset) => {
            function_return_type.map(ExpectedSourceContext::TypeRef)
        }
        Statement::Match {
            scrutinee: Some(scrutinee),
            arms,
            ..
        } if cursor_on_match_arm_path(arms, offset) => {
            Some(ExpectedSourceContext::MatchArmScrutinee(scrutinee))
        }
        _ => None,
    }
}

fn expected_source_context_in_nested_block<'a>(
    statement: &'a Statement,
    function_return_type: Option<&'a TypeRef>,
    offset: usize,
) -> Option<ExpectedSourceContext<'a>> {
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
        } => expected_source_context_in_conditional_blocks(
            then_block,
            else_ifs,
            else_block.as_ref(),
            function_return_type,
            offset,
        ),
        Statement::While { body, .. }
        | Statement::For { body, .. }
        | Statement::Transaction { body, .. } => {
            expected_source_context_in_body(body, function_return_type, offset)
        }
        Statement::Try { body, catch, .. } => {
            if let Some(context) =
                expected_source_context_in_body(body, function_return_type, offset)
            {
                return Some(context);
            }
            catch.as_ref().and_then(|catch| {
                expected_source_context_in_body(&catch.block, function_return_type, offset)
            })
        }
        Statement::Match { arms, .. } => arms.iter().find_map(|arm| {
            expected_source_context_in_body(&arm.block, function_return_type, offset)
        }),
        _ => None,
    }
}

fn expected_source_context_in_body<'a>(
    body: &'a Block,
    function_return_type: Option<&'a TypeRef>,
    offset: usize,
) -> Option<ExpectedSourceContext<'a>> {
    if span_covers(body.span, offset) {
        expected_source_context_in_block(body, function_return_type, offset)
    } else {
        None
    }
}

fn cursor_on_match_arm_path(arms: &[MatchArm], offset: usize) -> bool {
    arms.iter()
        .any(|arm| arm.path_spans.iter().any(|span| span_covers(*span, offset)))
}

fn expected_source_context_in_conditional_blocks<'a>(
    then_block: &'a Block,
    else_ifs: &'a [ElseIf],
    else_block: Option<&'a Block>,
    function_return_type: Option<&'a TypeRef>,
    offset: usize,
) -> Option<ExpectedSourceContext<'a>> {
    if span_covers(then_block.span, offset) {
        return expected_source_context_in_block(then_block, function_return_type, offset);
    }
    for else_if in else_ifs {
        if span_covers(else_if.block.span, offset) {
            return expected_source_context_in_block(&else_if.block, function_return_type, offset);
        }
    }
    else_block
        .and_then(|block| expected_source_context_in_body(block, function_return_type, offset))
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
