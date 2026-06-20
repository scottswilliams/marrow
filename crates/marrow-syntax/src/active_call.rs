use crate::{
    Block, ConstDecl, Declaration, DiagnosticReason, EnumMember, EvolveStep, ExpectedSyntax,
    KeyParam, Keyword, LexedSource, ParseDiagnosticReason, ParsedSource, ResourceMember,
    SourceSpan, Statement, SurfaceItem, Token, TokenKind, TypeRef, is_expression_callable_keyword,
    is_expression_path_segment_keyword, token::is_trivia,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveCallableContext {
    pub callee_path_segments: Vec<String>,
    pub active_argument: usize,
    pub named_argument: Option<String>,
}

pub fn active_callable_context(
    source: &str,
    lexed: &LexedSource,
    parsed: &ParsedSource,
    cursor_byte_offset: usize,
) -> Option<ActiveCallableContext> {
    let tokens = context_tokens(lexed);
    let open_parens = open_parens_before_cursor(&tokens, cursor_byte_offset);
    let open = open_parens.last().copied()?;

    if enclosing_parens_suppress_context(source, &tokens, parsed, &open_parens) {
        return None;
    }

    let callee_path_segments = callee_path_before(source, &tokens, parsed, open)?;
    let (active_argument, argument_start, argument_end) =
        active_argument(&tokens, open, cursor_byte_offset);
    let named_argument = named_argument(source, &tokens, argument_start, argument_end);

    Some(ActiveCallableContext {
        callee_path_segments,
        active_argument,
        named_argument,
    })
}

fn open_parens_before_cursor(tokens: &[Token], cursor_byte_offset: usize) -> Vec<usize> {
    let mut stack = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if token.span.start_byte >= cursor_byte_offset {
            break;
        }
        match token.kind {
            TokenKind::LeftParen => stack.push(index),
            TokenKind::RightParen => {
                stack.pop();
            }
            _ => {}
        }
    }
    stack
}

fn callee_path_before(
    source: &str,
    tokens: &[Token],
    parsed: &ParsedSource,
    open: usize,
) -> Option<Vec<String>> {
    let segment_indices = callee_segment_indices_before(tokens, open)?;
    let root = segment_indices[0];

    if starts_after_non_call_path_operator(tokens, root) {
        return None;
    }

    starts_at_callable_root(source, tokens, parsed, root, open).then(|| {
        segment_indices
            .iter()
            .map(|index| tokens[*index].text(source).to_string())
            .collect()
    })
}

fn callee_segment_indices_before(tokens: &[Token], open: usize) -> Option<Vec<usize>> {
    let mut index = open.checked_sub(1)?;
    if !is_callable_path_segment_or_head(tokens[index].kind) {
        return None;
    }

    let mut segments = vec![index];
    while let Some(double_colon) = index.checked_sub(1) {
        if tokens[double_colon].kind != TokenKind::DoubleColon {
            break;
        }
        index = double_colon.checked_sub(1)?;
        if !is_callable_path_segment_or_head(tokens[index].kind) {
            return None;
        }
        segments.push(index);
    }
    segments.reverse();

    let [head, tail @ ..] = segments.as_slice() else {
        return None;
    };
    let valid_head = if tail.is_empty() {
        is_callable_path_head(tokens[*head].kind)
    } else {
        is_callable_path_segment(tokens[*head].kind)
    };
    (valid_head
        && tail
            .iter()
            .all(|index| is_callable_path_segment(tokens[*index].kind)))
    .then_some(segments)
}

fn starts_at_callable_root(
    source: &str,
    tokens: &[Token],
    parsed: &ParsedSource,
    root: usize,
    open: usize,
) -> bool {
    !looks_like_type_annotation(source, tokens, parsed, root)
        && !looks_like_declaration_syntax(source, tokens, parsed, root, open)
}

fn enclosing_parens_suppress_context(
    source: &str,
    tokens: &[Token],
    parsed: &ParsedSource,
    stack: &[usize],
) -> bool {
    stack
        .iter()
        .take(stack.len().saturating_sub(1))
        .copied()
        .any(|open| {
            callee_root_before(tokens, open)
                .is_some_and(|root| !starts_at_callable_root(source, tokens, parsed, root, open))
        })
}

fn callee_root_before(tokens: &[Token], open: usize) -> Option<usize> {
    callee_segment_indices_before(tokens, open).map(|segments| segments[0])
}

fn starts_after_non_call_path_operator(tokens: &[Token], root: usize) -> bool {
    previous_significant_token(tokens, root).is_some_and(|previous| {
        matches!(
            tokens[previous].kind,
            TokenKind::Dot | TokenKind::QuestionDot | TokenKind::Caret | TokenKind::At
        )
    })
}

fn previous_significant_token(tokens: &[Token], before: usize) -> Option<usize> {
    (0..before)
        .rev()
        .find(|index| !is_trivia(tokens[*index].kind))
}

fn looks_like_type_annotation(
    source: &str,
    tokens: &[Token],
    parsed: &ParsedSource,
    root: usize,
) -> bool {
    if parsed_type_ref_contains(parsed, tokens[root].span.start_byte) {
        return true;
    }
    let Some(colon_index) = root.checked_sub(1) else {
        return false;
    };
    let colon = &tokens[colon_index];
    colon.kind == TokenKind::Colon
        && same_line_between(source, colon, &tokens[root])
        && !colon_is_named_argument_value(source, tokens, parsed, colon_index)
}

fn parsed_type_ref_contains(parsed: &ParsedSource, byte: usize) -> bool {
    parsed
        .file
        .declarations
        .iter()
        .any(|declaration| declaration_type_ref_contains(declaration, byte))
}

fn declaration_type_ref_contains(declaration: &Declaration, byte: usize) -> bool {
    match declaration {
        Declaration::Const(decl) => optional_type_ref_contains(decl.ty.as_ref(), byte),
        Declaration::Resource(resource) => resource
            .members
            .iter()
            .any(|member| resource_member_type_ref_contains(member, byte)),
        Declaration::Store(store) => key_params_type_ref_contains(&store.root.keys, byte),
        Declaration::Surface(surface) => key_params_type_ref_contains(&surface.store.keys, byte),
        Declaration::Function(function) => {
            function
                .params
                .iter()
                .any(|param| type_ref_contains(&param.ty, byte))
                || optional_type_ref_contains(function.return_type.as_ref(), byte)
                || block_type_ref_contains(&function.body, byte)
        }
        Declaration::Enum(_) => false,
        Declaration::Evolve(evolve) => evolve
            .steps
            .iter()
            .any(|step| evolve_step_type_ref_contains(step, byte)),
    }
}

fn resource_member_type_ref_contains(member: &ResourceMember, byte: usize) -> bool {
    match member {
        ResourceMember::Field(field) => {
            key_params_type_ref_contains(&field.keys, byte) || type_ref_contains(&field.ty, byte)
        }
        ResourceMember::Group(group) => {
            key_params_type_ref_contains(&group.keys, byte)
                || group
                    .members
                    .iter()
                    .any(|member| resource_member_type_ref_contains(member, byte))
        }
    }
}

fn evolve_step_type_ref_contains(step: &EvolveStep, byte: usize) -> bool {
    match step {
        EvolveStep::Transform { body, .. } => block_type_ref_contains(body, byte),
        EvolveStep::Rename { .. } | EvolveStep::Default { .. } | EvolveStep::Retire { .. } => false,
    }
}

fn block_type_ref_contains(block: &Block, byte: usize) -> bool {
    block
        .statements
        .iter()
        .any(|statement| statement_type_ref_contains(statement, byte))
}

fn statement_type_ref_contains(statement: &Statement, byte: usize) -> bool {
    match statement {
        Statement::Const { ty, .. } => optional_type_ref_contains(ty.as_ref(), byte),
        Statement::Var { keys, ty, .. } => {
            key_params_type_ref_contains(keys, byte)
                || optional_type_ref_contains(ty.as_ref(), byte)
        }
        Statement::IfConst {
            ty,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            optional_type_ref_contains(ty.as_ref(), byte)
                || block_type_ref_contains(then_block, byte)
                || else_ifs
                    .iter()
                    .any(|else_if| block_type_ref_contains(&else_if.block, byte))
                || else_block
                    .as_ref()
                    .is_some_and(|block| block_type_ref_contains(block, byte))
        }
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            block_type_ref_contains(then_block, byte)
                || else_ifs
                    .iter()
                    .any(|else_if| block_type_ref_contains(&else_if.block, byte))
                || else_block
                    .as_ref()
                    .is_some_and(|block| block_type_ref_contains(block, byte))
        }
        Statement::While { body, .. }
        | Statement::For { body, .. }
        | Statement::Transaction { body, .. } => block_type_ref_contains(body, byte),
        Statement::Try { body, catch, .. } => {
            block_type_ref_contains(body, byte)
                || catch.as_ref().is_some_and(|catch| {
                    optional_type_ref_contains(catch.ty.as_ref(), byte)
                        || block_type_ref_contains(&catch.block, byte)
                })
        }
        Statement::Match { arms, .. } => arms
            .iter()
            .any(|arm| block_type_ref_contains(&arm.block, byte)),
        Statement::Assign { .. }
        | Statement::Delete { .. }
        | Statement::Return { .. }
        | Statement::ReturnAbsent { .. }
        | Statement::Break { .. }
        | Statement::Continue { .. }
        | Statement::Throw { .. }
        | Statement::Expr { .. } => false,
    }
}

fn key_params_type_ref_contains(keys: &[KeyParam], byte: usize) -> bool {
    keys.iter().any(|key| type_ref_contains(&key.ty, byte))
}

fn optional_type_ref_contains(ty: Option<&TypeRef>, byte: usize) -> bool {
    ty.is_some_and(|ty| type_ref_contains(ty, byte))
}

fn type_ref_contains(ty: &TypeRef, byte: usize) -> bool {
    span_contains(ty.span, byte)
}

fn colon_is_named_argument_value(
    source: &str,
    tokens: &[Token],
    parsed: &ParsedSource,
    colon_index: usize,
) -> bool {
    let Some(name_index) = colon_index.checked_sub(1) else {
        return false;
    };
    if tokens[name_index].kind != TokenKind::Identifier
        || !same_line_between(source, &tokens[name_index], &tokens[colon_index])
    {
        return false;
    }

    let Some(open) = innermost_open_paren_before(tokens, colon_index) else {
        return false;
    };
    let Some(root) = callee_root_before(tokens, open) else {
        return false;
    };

    starts_at_callable_root(source, tokens, parsed, root, open)
}

fn innermost_open_paren_before(tokens: &[Token], end: usize) -> Option<usize> {
    let mut stack = Vec::new();
    for (index, token) in tokens.iter().enumerate().take(end) {
        match token.kind {
            TokenKind::LeftParen => stack.push(index),
            TokenKind::RightParen => {
                stack.pop();
            }
            _ => {}
        }
    }
    stack.last().copied()
}

fn looks_like_declaration_syntax(
    source: &str,
    tokens: &[Token],
    parsed: &ParsedSource,
    root: usize,
    open: usize,
) -> bool {
    let root_byte = tokens[root].span.start_byte;
    declaration_header_contains(parsed, root_byte)
        || follows_local_declaration_keyword(source, tokens, root)
        || declaration_member_span_contains(parsed, root_byte)
        || declaration_syntax_diagnostic_contains(parsed, root_byte)
        || (is_first_significant_token_on_line(source, tokens, root)
            && key_list_has_type_suffix(source, tokens, open))
}

fn follows_local_declaration_keyword(source: &str, tokens: &[Token], root: usize) -> bool {
    let Some(previous) = previous_significant_token(tokens, root) else {
        return false;
    };
    matches!(
        tokens[previous].kind,
        TokenKind::Keyword(Keyword::Const | Keyword::Var)
    ) && same_line_between(source, &tokens[previous], &tokens[root])
}

fn key_list_has_type_suffix(source: &str, tokens: &[Token], open: usize) -> bool {
    let Some(close) = matching_right_paren(tokens, open) else {
        return false;
    };
    let Some(next) = tokens.get(close + 1) else {
        return false;
    };
    next.kind == TokenKind::Colon && same_line_between(source, &tokens[close], next)
}

fn matching_right_paren(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open + 1) {
        match token.kind {
            TokenKind::LeftParen => depth += 1,
            TokenKind::RightParen => {
                if depth == 0 {
                    return Some(index);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

fn is_first_significant_token_on_line(source: &str, tokens: &[Token], index: usize) -> bool {
    let Some(previous) = index.checked_sub(1).and_then(|index| tokens.get(index)) else {
        return true;
    };
    !same_line_between(source, previous, &tokens[index])
}

fn same_line_between(source: &str, before: &Token, after: &Token) -> bool {
    before.span.end_byte <= after.span.start_byte
        && !source[before.span.end_byte..after.span.start_byte].contains('\n')
}

fn declaration_header_contains(parsed: &ParsedSource, byte: usize) -> bool {
    parsed
        .file
        .module
        .as_ref()
        .is_some_and(|module| span_contains(module.span, byte))
        || parsed
            .file
            .uses
            .iter()
            .any(|use_decl| span_contains(use_decl.span, byte))
        || parsed
            .file
            .declarations
            .iter()
            .any(|declaration| declaration_header_span_contains(parsed, declaration, byte))
}

fn declaration_header_span_contains(
    parsed: &ParsedSource,
    declaration: &Declaration,
    byte: usize,
) -> bool {
    match declaration {
        Declaration::Const(decl) => const_header_span_contains(parsed, decl, byte),
        Declaration::Resource(decl) => span_contains(decl.span, byte),
        Declaration::Store(decl) => span_contains(decl.span, byte),
        Declaration::Surface(decl) => span_contains(decl.span, byte),
        Declaration::Function(decl) => span_contains(decl.span, byte),
        Declaration::Enum(decl) => span_contains(decl.span, byte),
        Declaration::Evolve(decl) => span_contains(decl.span, byte),
    }
}

fn const_header_span_contains(parsed: &ParsedSource, decl: &ConstDecl, byte: usize) -> bool {
    span_contains(decl.span, byte) && !const_value_span_contains(parsed, decl, byte)
}

fn const_value_span_contains(parsed: &ParsedSource, decl: &ConstDecl, byte: usize) -> bool {
    decl.value
        .as_ref()
        .is_some_and(|value| span_contains(value.span(), byte))
        || parsed.diagnostics.iter().any(|diagnostic| {
            span_contains(decl.span, diagnostic.span.start_byte)
                && span_contains(diagnostic.span, byte)
                && matches!(
                    diagnostic.reason,
                    DiagnosticReason::Parser(ParseDiagnosticReason::Expected(
                        ExpectedSyntax::Expression
                    ))
                )
        })
}

fn declaration_member_span_contains(parsed: &ParsedSource, byte: usize) -> bool {
    parsed
        .file
        .declarations
        .iter()
        .any(|declaration| match declaration {
            Declaration::Resource(resource) => resource
                .members
                .iter()
                .any(|member| resource_member_span_contains(member, byte)),
            Declaration::Store(store) => store
                .indexes
                .iter()
                .any(|index| span_contains(index.span, byte)),
            Declaration::Surface(surface) => surface
                .items
                .iter()
                .any(|item| span_contains(surface_item_span(item), byte)),
            Declaration::Enum(enum_decl) => enum_decl
                .members
                .iter()
                .any(|member| enum_member_span_contains(member, byte)),
            Declaration::Evolve(evolve) => evolve
                .steps
                .iter()
                .any(|step| evolve_step_target_span_contains(step, byte)),
            _ => false,
        })
}

fn resource_member_span_contains(member: &ResourceMember, byte: usize) -> bool {
    span_contains(member.span(), byte)
        || match member {
            ResourceMember::Field(_) => false,
            ResourceMember::Group(group) => group
                .members
                .iter()
                .any(|member| resource_member_span_contains(member, byte)),
        }
}

fn surface_item_span(item: &SurfaceItem) -> SourceSpan {
    item.span()
}

fn enum_member_span_contains(member: &EnumMember, byte: usize) -> bool {
    span_contains(member.span, byte)
        || member
            .members
            .iter()
            .any(|member| enum_member_span_contains(member, byte))
}

fn evolve_step_target_span_contains(step: &EvolveStep, byte: usize) -> bool {
    match step {
        EvolveStep::Rename { from, to, .. } => {
            span_contains(from.span(), byte) || span_contains(to.span(), byte)
        }
        EvolveStep::Default { target, .. }
        | EvolveStep::Retire { target, .. }
        | EvolveStep::Transform { target, .. } => span_contains(target.span(), byte),
    }
}

fn declaration_syntax_diagnostic_contains(parsed: &ParsedSource, byte: usize) -> bool {
    parsed.diagnostics.iter().any(|diagnostic| {
        span_contains(diagnostic.span, byte)
            && matches!(
                diagnostic.reason,
                DiagnosticReason::Parser(ParseDiagnosticReason::MatchArmMemberPath)
                    | DiagnosticReason::Parser(ParseDiagnosticReason::EmptyIndexArguments)
                    | DiagnosticReason::Parser(ParseDiagnosticReason::EmptyKeyParameters)
                    | DiagnosticReason::Parser(ParseDiagnosticReason::EnumMemberMustBeBareName)
                    | DiagnosticReason::Parser(ParseDiagnosticReason::IndexOutsideStoreBody)
                    | DiagnosticReason::Parser(ParseDiagnosticReason::ResourceMemberInStoreBody)
                    | DiagnosticReason::Parser(ParseDiagnosticReason::Expected(
                        ExpectedSyntax::Declaration
                            | ExpectedSyntax::EnumBody
                            | ExpectedSyntax::EnumHeader
                            | ExpectedSyntax::EnumName
                            | ExpectedSyntax::EvolveStep
                            | ExpectedSyntax::EvolveTargetPath
                            | ExpectedSyntax::FieldType
                            | ExpectedSyntax::FunctionBody
                            | ExpectedSyntax::FunctionHeader
                            | ExpectedSyntax::FunctionName
                            | ExpectedSyntax::FunctionParameterList
                            | ExpectedSyntax::FunctionReturnType
                            | ExpectedSyntax::ImportName
                            | ExpectedSyntax::IndexArgumentList
                            | ExpectedSyntax::IndexFieldPath
                            | ExpectedSyntax::IndexName
                            | ExpectedSyntax::IndexTail
                            | ExpectedSyntax::KeyName
                            | ExpectedSyntax::KeyParameterList
                            | ExpectedSyntax::KeyType
                            | ExpectedSyntax::ModuleName
                            | ExpectedSyntax::ParameterName
                            | ExpectedSyntax::ParameterType
                            | ExpectedSyntax::ResourceBody
                            | ExpectedSyntax::ResourceHeader
                            | ExpectedSyntax::ResourceMemberName
                            | ExpectedSyntax::ResourceMemberSyntax
                            | ExpectedSyntax::ResourceName
                            | ExpectedSyntax::StoreResourceName
                            | ExpectedSyntax::StoreRoot
                            | ExpectedSyntax::SurfaceAction
                            | ExpectedSyntax::SurfaceBody
                            | ExpectedSyntax::SurfaceCollection
                            | ExpectedSyntax::SurfaceFieldList
                            | ExpectedSyntax::SurfaceCollectionTarget
                            | ExpectedSyntax::SurfaceHeader
                            | ExpectedSyntax::SurfaceItem
                            | ExpectedSyntax::SurfaceName
                            | ExpectedSyntax::SurfaceRead
                            | ExpectedSyntax::SurfaceStore
                            | ExpectedSyntax::TransformBody,
                    ))
            )
    })
}

fn span_contains(span: SourceSpan, byte: usize) -> bool {
    span.start_byte <= byte && byte < span.end_byte
}

fn active_argument(
    tokens: &[Token],
    open: usize,
    cursor_byte_offset: usize,
) -> (usize, usize, usize) {
    let mut active = 0usize;
    let mut argument_start = open + 1;
    let mut argument_end = argument_start;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;

    for (index, token) in tokens.iter().enumerate().skip(open + 1) {
        if token.span.start_byte >= cursor_byte_offset {
            break;
        }
        argument_end = index + 1;
        match token.kind {
            TokenKind::LeftParen => paren_depth += 1,
            TokenKind::RightParen => {
                if paren_depth == 0 {
                    argument_end = index;
                    break;
                }
                paren_depth -= 1;
            }
            TokenKind::LeftBracket => bracket_depth += 1,
            TokenKind::RightBracket => bracket_depth = bracket_depth.saturating_sub(1),
            TokenKind::Comma if paren_depth == 0 && bracket_depth == 0 => {
                active += 1;
                argument_start = index + 1;
                argument_end = argument_start;
            }
            _ => {}
        }
    }

    (active, argument_start, argument_end)
}

fn named_argument(
    source: &str,
    tokens: &[Token],
    argument_start: usize,
    argument_end: usize,
) -> Option<String> {
    let name_index = first_significant_token(tokens, argument_start, argument_end)?;
    let name = tokens.get(name_index)?;
    let colon = tokens.get(name_index + 1)?;
    (name_index + 1 < argument_end
        && name.kind == TokenKind::Identifier
        && colon.kind == TokenKind::Colon)
        .then(|| name.text(source).to_string())
}

fn first_significant_token(tokens: &[Token], start: usize, end: usize) -> Option<usize> {
    (start..end).find(|index| !is_trivia(tokens[*index].kind))
}

fn context_tokens(lexed: &LexedSource) -> Vec<Token> {
    lexed
        .tokens
        .iter()
        .filter(|token| {
            !matches!(
                token.kind,
                TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof
            )
        })
        .copied()
        .collect()
}

fn is_callable_path_segment_or_head(kind: TokenKind) -> bool {
    is_callable_path_head(kind) || is_callable_path_segment(kind)
}

fn is_callable_path_head(kind: TokenKind) -> bool {
    match kind {
        TokenKind::Identifier => true,
        TokenKind::Keyword(keyword) => is_expression_callable_keyword(keyword),
        _ => false,
    }
}

fn is_callable_path_segment(kind: TokenKind) -> bool {
    match kind {
        TokenKind::Identifier => true,
        TokenKind::Keyword(keyword) => is_expression_path_segment_keyword(keyword),
        _ => false,
    }
}
