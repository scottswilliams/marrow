use crate::{
    Block, CheckedBind, ConstDecl, Declaration, DiagnosticReason, EnumMember, EvolveStep,
    ExpectedSyntax, KeyParam, Keyword, LexedSource, ParseDiagnosticReason, ParsedSource,
    ResourceMember, SourceSpan, Statement, Token, TokenKind, TypeExpr,
    is_expression_callable_keyword, is_expression_path_segment_keyword, token::is_trivia,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveCallableContext {
    pub callee_path_segments: Vec<String>,
    pub active_argument: usize,
    pub named_argument: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallableCalleeContext {
    pub callee_path_segments: Vec<String>,
    pub callee_span: SourceSpan,
    pub callee_leaf_span: SourceSpan,
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

pub fn callable_callee_contexts(
    source: &str,
    lexed: &LexedSource,
    parsed: &ParsedSource,
) -> Vec<CallableCalleeContext> {
    let tokens = context_tokens(lexed);
    let suppression = CallableSuppressionIndex::new(parsed);
    let parens = ParenFacts::new(source, &tokens);
    let mut stack: Vec<OpenParenContext> = Vec::new();
    let mut contexts = Vec::new();

    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen => {
                let parent_suppressed = stack.last().is_some_and(|open| open.suppressed);
                let state = if parent_suppressed {
                    CalleeLookup::None
                } else {
                    callee_lookup_before(
                        source,
                        &tokens,
                        &suppression,
                        &parens,
                        stack.last(),
                        index,
                    )
                };
                let accepts_named_arguments = matches!(
                    state,
                    CalleeLookup::Callable(_) | CalleeLookup::NamedArgumentHost
                );
                let suppresses_nested = matches!(state, CalleeLookup::SuppressesNested);
                stack.push(OpenParenContext {
                    suppressed: parent_suppressed || suppresses_nested,
                    accepts_named_arguments,
                });
                if let CalleeLookup::Callable(context) = state {
                    contexts.push(context);
                }
            }
            TokenKind::RightParen => {
                stack.pop();
            }
            _ => {}
        }
    }

    contexts
}

struct OpenParenContext {
    suppressed: bool,
    accepts_named_arguments: bool,
}

enum CalleeLookup {
    Callable(CallableCalleeContext),
    NamedArgumentHost,
    SuppressesNested,
    None,
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
    callee_context_before(source, tokens, parsed, open).map(|context| context.callee_path_segments)
}

fn callee_context_before(
    source: &str,
    tokens: &[Token],
    parsed: &ParsedSource,
    open: usize,
) -> Option<CallableCalleeContext> {
    let segment_indices = callee_segment_indices_before(tokens, open)?;
    let root = segment_indices[0];

    if starts_after_non_call_path_operator(tokens, root) {
        return None;
    }

    starts_at_callable_root(source, tokens, parsed, root, open).then(|| {
        let leaf = *segment_indices
            .last()
            .expect("callee segment indices are nonempty");
        CallableCalleeContext {
            callee_path_segments: segment_indices
                .iter()
                .map(|index| tokens[*index].text(source).to_string())
                .collect(),
            callee_span: SourceSpan {
                start_byte: tokens[root].span.start_byte,
                end_byte: tokens[leaf].span.end_byte,
                line: tokens[root].span.line,
                column: tokens[root].span.column,
            },
            callee_leaf_span: tokens[leaf].span,
        }
    })
}

fn callee_lookup_before(
    source: &str,
    tokens: &[Token],
    suppression: &CallableSuppressionIndex,
    parens: &ParenFacts,
    parent: Option<&OpenParenContext>,
    open: usize,
) -> CalleeLookup {
    let Some(segment_indices) = callee_segment_indices_before(tokens, open) else {
        return CalleeLookup::None;
    };
    let root = segment_indices[0];

    if !starts_at_callable_root_indexed(source, tokens, suppression, parens, parent, root, open) {
        return CalleeLookup::SuppressesNested;
    }

    if starts_after_non_call_path_operator(tokens, root) {
        return CalleeLookup::NamedArgumentHost;
    }

    let leaf = *segment_indices
        .last()
        .expect("callee segment indices are nonempty");
    CalleeLookup::Callable(CallableCalleeContext {
        callee_path_segments: segment_indices
            .iter()
            .map(|index| tokens[*index].text(source).to_string())
            .collect(),
        callee_span: SourceSpan {
            start_byte: tokens[root].span.start_byte,
            end_byte: tokens[leaf].span.end_byte,
            line: tokens[root].span.line,
            column: tokens[root].span.column,
        },
        callee_leaf_span: tokens[leaf].span,
    })
}

fn starts_at_callable_root_indexed(
    source: &str,
    tokens: &[Token],
    suppression: &CallableSuppressionIndex,
    parens: &ParenFacts,
    parent: Option<&OpenParenContext>,
    root: usize,
    open: usize,
) -> bool {
    !looks_like_type_annotation_indexed(source, tokens, suppression, parent, root)
        && !looks_like_declaration_syntax_indexed(source, tokens, suppression, parens, root, open)
}

fn looks_like_type_annotation_indexed(
    source: &str,
    tokens: &[Token],
    suppression: &CallableSuppressionIndex,
    parent: Option<&OpenParenContext>,
    root: usize,
) -> bool {
    if suppression.type_refs.contains(tokens[root].span.start_byte) {
        return true;
    }
    let Some(colon_index) = root.checked_sub(1) else {
        return false;
    };
    let colon = &tokens[colon_index];
    colon.kind == TokenKind::Colon
        && same_line_between(source, colon, &tokens[root])
        && !colon_is_named_argument_value_indexed(source, tokens, parent, colon_index)
}

fn colon_is_named_argument_value_indexed(
    source: &str,
    tokens: &[Token],
    parent: Option<&OpenParenContext>,
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

    parent.is_some_and(|open| open.accepts_named_arguments)
}

fn looks_like_declaration_syntax_indexed(
    source: &str,
    tokens: &[Token],
    suppression: &CallableSuppressionIndex,
    parens: &ParenFacts,
    root: usize,
    open: usize,
) -> bool {
    let root_byte = tokens[root].span.start_byte;
    suppression.declarations.contains(root_byte)
        || follows_local_declaration_keyword(source, tokens, root)
        || (is_first_significant_token_on_line(source, tokens, root)
            && parens.has_type_suffix(open))
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

struct ParenFacts {
    type_suffix_by_open: Vec<bool>,
}

impl ParenFacts {
    fn new(source: &str, tokens: &[Token]) -> Self {
        let mut type_suffix_by_open = vec![false; tokens.len()];
        let mut stack = Vec::new();

        for (index, token) in tokens.iter().enumerate() {
            match token.kind {
                TokenKind::LeftParen => stack.push(index),
                TokenKind::RightParen => {
                    let Some(open) = stack.pop() else {
                        continue;
                    };
                    if tokens.get(index + 1).is_some_and(|next| {
                        next.kind == TokenKind::Colon && same_line_between(source, token, next)
                    }) {
                        type_suffix_by_open[open] = true;
                    }
                }
                _ => {}
            }
        }

        Self {
            type_suffix_by_open,
        }
    }

    fn has_type_suffix(&self, open: usize) -> bool {
        self.type_suffix_by_open.get(open).copied().unwrap_or(false)
    }
}

struct CallableSuppressionIndex {
    type_refs: ByteRanges,
    declarations: ByteRanges,
}

impl CallableSuppressionIndex {
    fn new(parsed: &ParsedSource) -> Self {
        let mut type_refs = ByteRanges::default();
        let mut declarations = ByteRanges::default();

        if let Some(module) = &parsed.file.module {
            declarations.push(module.span);
        }
        for use_decl in &parsed.file.uses {
            declarations.push(use_decl.span);
        }
        for declaration in &parsed.file.declarations {
            collect_declaration_suppression(parsed, declaration, &mut type_refs, &mut declarations);
        }
        for diagnostic in &parsed.diagnostics {
            if diagnostic_suppresses_callable(diagnostic) {
                declarations.push(diagnostic.span);
            }
        }

        type_refs.finish();
        declarations.finish();
        Self {
            type_refs,
            declarations,
        }
    }
}

#[derive(Default)]
struct ByteRanges {
    ranges: Vec<ByteRange>,
}

#[derive(Clone, Copy)]
struct ByteRange {
    start: usize,
    end: usize,
}

impl ByteRanges {
    fn push(&mut self, span: SourceSpan) {
        self.ranges.push(ByteRange {
            start: span.start_byte,
            end: span.end_byte,
        });
    }

    fn finish(&mut self) {
        self.ranges.sort_by_key(|range| (range.start, range.end));
        let mut merged: Vec<ByteRange> = Vec::new();
        for range in self.ranges.drain(..) {
            if range.start >= range.end {
                continue;
            }
            if let Some(last) = merged.last_mut()
                && range.start <= last.end
            {
                last.end = last.end.max(range.end);
                continue;
            }
            merged.push(range);
        }
        self.ranges = merged;
    }

    fn contains(&self, byte: usize) -> bool {
        let index = self.ranges.partition_point(|range| range.start <= byte);
        index > 0 && byte < self.ranges[index - 1].end
    }
}

fn collect_declaration_suppression(
    parsed: &ParsedSource,
    declaration: &Declaration,
    type_refs: &mut ByteRanges,
    declarations: &mut ByteRanges,
) {
    match declaration {
        Declaration::Alias(decl) => {
            collect_optional_type_ref(decl.ty.as_ref(), type_refs);
            declarations.push(decl.span);
        }
        Declaration::Nominal(decl) => {
            collect_optional_type_ref(decl.base.as_ref(), type_refs);
            declarations.push(decl.span);
        }
        Declaration::Const(decl) => {
            collect_optional_type_ref(decl.ty.as_ref(), type_refs);
            declarations.push(const_header_suppression_span(parsed, decl));
        }
        Declaration::Resource(resource) => {
            declarations.push(resource.span);
            for member in &resource.members {
                collect_resource_member_suppression(member, type_refs, declarations);
            }
        }
        Declaration::Struct(decl) => {
            declarations.push(decl.span);
            for member in &decl.members {
                collect_resource_member_suppression(member, type_refs, declarations);
            }
        }
        Declaration::Store(store) => {
            declarations.push(store.span);
            collect_key_param_type_refs(&store.root.keys, type_refs);
            for index in &store.indexes {
                declarations.push(index.span);
            }
        }
        Declaration::Function(function) => {
            declarations.push(function.span);
            for param in &function.params {
                type_refs.push(param.ty.span());
            }
            collect_optional_type_ref(function.return_type.as_ref(), type_refs);
            collect_block_type_refs(&function.body, type_refs);
        }
        Declaration::Enum(enum_decl) => {
            declarations.push(enum_decl.span);
            for member in &enum_decl.members {
                collect_enum_member_suppression(member, type_refs, declarations);
            }
        }
        Declaration::Evolve(evolve) => {
            declarations.push(evolve.span);
            for step in &evolve.steps {
                collect_evolve_step_suppression(step, type_refs, declarations);
            }
        }
        Declaration::Test(test) => {
            declarations.push(test.span);
            collect_block_type_refs(&test.body, type_refs);
        }
    }
}

fn collect_resource_member_suppression(
    member: &ResourceMember,
    type_refs: &mut ByteRanges,
    declarations: &mut ByteRanges,
) {
    declarations.push(member.span());
    match member {
        ResourceMember::Field(field) => {
            collect_key_param_type_refs(&field.keys, type_refs);
            type_refs.push(field.ty.span());
        }
        ResourceMember::Group(group) => {
            collect_key_param_type_refs(&group.keys, type_refs);
            for member in &group.members {
                collect_resource_member_suppression(member, type_refs, declarations);
            }
        }
    }
}

fn collect_enum_member_suppression(
    member: &EnumMember,
    type_refs: &mut ByteRanges,
    declarations: &mut ByteRanges,
) {
    declarations.push(member.span);
    for field in &member.payload {
        type_refs.push(field.ty.span());
    }
    for member in &member.members {
        collect_enum_member_suppression(member, type_refs, declarations);
    }
}

fn collect_evolve_step_suppression(
    step: &EvolveStep,
    type_refs: &mut ByteRanges,
    declarations: &mut ByteRanges,
) {
    match step {
        EvolveStep::Rename { from, to, .. } => {
            declarations.push(from.span());
            declarations.push(to.span());
        }
        EvolveStep::Default { target, .. }
        | EvolveStep::Retire { target, .. }
        | EvolveStep::Transform { target, .. } => declarations.push(target.span()),
    }
    if let EvolveStep::Transform { body, .. } = step {
        collect_block_type_refs(body, type_refs);
    }
}

fn collect_block_type_refs(block: &Block, type_refs: &mut ByteRanges) {
    for statement in &block.statements {
        collect_statement_type_refs(statement, type_refs);
    }
}

fn collect_statement_type_refs(statement: &Statement, type_refs: &mut ByteRanges) {
    match statement {
        Statement::Const { ty, .. } => collect_optional_type_ref(ty.as_ref(), type_refs),
        Statement::Var { keys, ty, .. } => {
            collect_key_param_type_refs(keys, type_refs);
            collect_optional_type_ref(ty.as_ref(), type_refs);
        }
        Statement::IfConst {
            ty,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            collect_optional_type_ref(ty.as_ref(), type_refs);
            collect_block_type_refs(then_block, type_refs);
            for else_if in else_ifs {
                collect_block_type_refs(&else_if.block, type_refs);
            }
            if let Some(block) = else_block {
                collect_block_type_refs(block, type_refs);
            }
        }
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            collect_block_type_refs(then_block, type_refs);
            for else_if in else_ifs {
                collect_block_type_refs(&else_if.block, type_refs);
            }
            if let Some(block) = else_block {
                collect_block_type_refs(block, type_refs);
            }
        }
        Statement::While { body, .. }
        | Statement::For { body, .. }
        | Statement::Transaction { body, .. } => collect_block_type_refs(body, type_refs),
        Statement::Match { arms, .. } => {
            for arm in arms {
                collect_block_type_refs(&arm.block, type_refs);
            }
        }
        Statement::Checked {
            bind,
            out_of_range,
            zero_divisor,
            ..
        } => {
            if let CheckedBind::Const { ty, .. } | CheckedBind::Var { ty, .. } = bind {
                collect_optional_type_ref(ty.as_ref(), type_refs);
            }
            for block in [out_of_range, zero_divisor].into_iter().flatten() {
                collect_block_type_refs(block, type_refs);
            }
        }
        Statement::Assign { .. }
        | Statement::CompoundAssign { .. }
        | Statement::Delete { .. }
        | Statement::PlaceBinding { .. }
        | Statement::Unset { .. }
        | Statement::Return { .. }
        | Statement::Break { .. }
        | Statement::Continue { .. }
        | Statement::Assert { .. }
        | Statement::Expr { .. }
        | Statement::Error { .. } => {}
    }
}

fn collect_key_param_type_refs(keys: &[KeyParam], type_refs: &mut ByteRanges) {
    for key in keys {
        type_refs.push(key.ty.span());
    }
}

fn collect_optional_type_ref(ty: Option<&TypeExpr>, type_refs: &mut ByteRanges) {
    if let Some(ty) = ty {
        type_refs.push(ty.span());
    }
}

fn diagnostic_suppresses_callable(diagnostic: &crate::Diagnostic) -> bool {
    matches!(
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
                    | ExpectedSyntax::TransformBody,
            ))
    )
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
            TokenKind::Dot | TokenKind::QuestionDot | TokenKind::Caret
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
    let mut type_refs = ByteRanges::default();
    let mut declarations = ByteRanges::default();
    for declaration in &parsed.file.declarations {
        collect_declaration_suppression(parsed, declaration, &mut type_refs, &mut declarations);
    }
    type_refs.finish();
    type_refs.contains(byte)
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
        Declaration::Alias(decl) => span_contains(decl.span, byte),
        Declaration::Nominal(decl) => span_contains(decl.span, byte),
        Declaration::Const(decl) => const_header_span_contains(parsed, decl, byte),
        Declaration::Resource(decl) => span_contains(decl.span, byte),
        Declaration::Struct(decl) => span_contains(decl.span, byte),
        Declaration::Store(decl) => span_contains(decl.span, byte),
        Declaration::Function(decl) => span_contains(decl.span, byte),
        Declaration::Enum(decl) => span_contains(decl.span, byte),
        Declaration::Evolve(decl) => span_contains(decl.span, byte),
        Declaration::Test(decl) => span_contains(decl.span, byte),
    }
}

fn const_header_span_contains(parsed: &ParsedSource, decl: &ConstDecl, byte: usize) -> bool {
    span_contains(const_header_suppression_span(parsed, decl), byte)
}

fn const_header_suppression_span(parsed: &ParsedSource, decl: &ConstDecl) -> SourceSpan {
    SourceSpan {
        start_byte: decl.span.start_byte,
        end_byte: const_header_end(parsed, decl),
        line: decl.span.line,
        column: decl.span.column,
    }
}

fn const_header_end(parsed: &ParsedSource, decl: &ConstDecl) -> usize {
    if let Some(value) = &decl.value {
        return value.span().start_byte;
    }

    parsed
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            span_contains(decl.span, diagnostic.span.start_byte)
                && matches!(
                    diagnostic.reason,
                    DiagnosticReason::Parser(ParseDiagnosticReason::Expected(
                        ExpectedSyntax::Expression
                    ))
                )
        })
        .map(|diagnostic| diagnostic.span.start_byte)
        .min()
        .unwrap_or(decl.span.end_byte)
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
        span_contains(diagnostic.span, byte) && diagnostic_suppresses_callable(diagnostic)
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
