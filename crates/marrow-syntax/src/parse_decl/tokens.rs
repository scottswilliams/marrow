//! Low-level helpers over a token slice shared across the declaration and
//! statement parsers: line and span bounds, top-level delimiter scanning, comment
//! construction, and the bridge into the expression parser.

use std::borrow::Cow;

use super::{ParseError, ParseResult};
use crate::NESTING_DEPTH_LIMIT;
use crate::ast::{
    Comment, CommentMarker, CommentPlacement, Expression, IdentityTypeExpr, NameSegment, TypeExpr,
};
use crate::diagnostic::{
    DiagnosticReason, DiscardingSyntaxErrorSink, ExpectedSyntax, ParseDiagnosticReason, SourceSpan,
    SyntaxError, SyntaxSink,
};
use crate::parse_expr::{ExprParser, ParseComplete, join_spans};
use crate::token::{Keyword, LexicalClass, Token, TokenKind, is_qualified_name};

/// The end byte of the physical line containing `start`, excluding the trailing
/// `\r`/`\n`. This matches `Line::end_byte` for a declaration's first line.
pub(super) fn first_line_end(source: &str, start: usize) -> usize {
    let tail = &source[start..];
    let break_at = tail
        .find('\n')
        .map(|index| {
            if tail[..index].ends_with('\r') {
                index - 1
            } else {
                index
            }
        })
        .unwrap_or(tail.len());
    start + break_at
}
/// Strip the `///` doc-comment marker and surrounding whitespace, matching
/// `Line::doc_comment`.
pub(super) fn doc_comment_text(text: &str) -> String {
    text.strip_prefix("///").unwrap_or(text).trim().to_string()
}
/// The segments of a `::`-qualified path, each carrying the span it was spelled at.
/// The whole spelling is checked against the path grammar first, so segments are only
/// ever produced for a path the grammar admits.
fn qualified_name(source: &str, tokens: &[Token]) -> Option<Box<[NameSegment]>> {
    let first = tokens.first()?;
    let last = tokens.last()?;
    let text = &source[first.span.start_byte..last.span.end_byte];
    is_qualified_name(text).then(|| {
        tokens
            .iter()
            .step_by(2)
            .map(|token| NameSegment::new(token.text(source), token.span))
            .collect()
    })
}
/// Why a `use`/`module` path failed to parse: a reserved word stands where a path
/// segment must be, or the tokens do not spell a `::`-qualified name at all.
pub(super) enum PathNameError {
    ReservedSegment(Token),
    NotQualified,
}
pub(super) fn module_name(
    source: &str,
    tokens: &[Token],
) -> Result<Box<[NameSegment]>, PathNameError> {
    if let Some(reserved) = reserved_segment(tokens) {
        return Err(PathNameError::ReservedSegment(*reserved));
    }
    qualified_name(source, tokens).ok_or(PathNameError::NotQualified)
}
pub(super) fn import_name(
    source: &str,
    tokens: &[Token],
) -> Result<Box<[NameSegment]>, PathNameError> {
    // A project may declare `module std::bytes`, so the reserved type word `bytes`
    // stays legal as that import's final segment; a reserved segment in any other
    // position is the path error.
    if let Some(reserved) =
        reserved_segment(tokens).filter(|_| !is_std_bytes_import(source, tokens))
    {
        return Err(PathNameError::ReservedSegment(*reserved));
    }
    qualified_name(source, tokens).ok_or(PathNameError::NotQualified)
}
fn reserved_segment(tokens: &[Token]) -> Option<&Token> {
    tokens
        .iter()
        .step_by(2)
        .find(|token| matches!(token.kind, TokenKind::Keyword(_)))
}
fn is_std_bytes_import(source: &str, tokens: &[Token]) -> bool {
    matches!(
        tokens,
        [std, sep, bytes]
            if std.kind == TokenKind::Identifier
                && std.text(source) == "std"
                && sep.kind == TokenKind::DoubleColon
                && bytes.kind == TokenKind::Keyword(Keyword::Bytes)
    )
}
pub(super) fn push_parse_error(sink: &mut SyntaxSink<'_>, fallback: SourceSpan, error: ParseError) {
    let (span, reason, message) = error.locate(fallback);
    sink.push(SyntaxError::new(
        DiagnosticReason::Parser(reason),
        message,
        None,
        span,
    ));
}
/// Drop comment tokens from a token slice. A `//` or `///` inside an open delimiter
/// lexes to a comment token with no newline; like a blank line it separates and closes
/// nothing, so a multi-line declaration list must read it as absent. The slice is
/// returned unchanged when it holds no comments, keeping the common case borrowed.
pub(super) fn strip_comment_tokens(tokens: &[Token]) -> Cow<'_, [Token]> {
    if tokens.iter().any(|token| is_line_comment(token.kind)) {
        Cow::Owned(
            tokens
                .iter()
                .copied()
                .filter(|token| !is_line_comment(token.kind))
                .collect(),
        )
    } else {
        Cow::Borrowed(tokens)
    }
}
/// Split tokens on top-level commas (depth 0), dropping a trailing empty group from a
/// trailing comma. Valid only over declaration and type slices, where `<`/`>` delimit a
/// generic argument list rather than comparing values, so a comma inside a nested
/// generic does not split its enclosing list.
pub(super) fn split_top_level_commas(tokens: &[Token]) -> Vec<&[Token]> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::Less => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket | TokenKind::Greater => {
                depth = depth.saturating_sub(1)
            }
            TokenKind::Comma if depth == 0 => {
                parts.push(&tokens[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < tokens.len() {
        parts.push(&tokens[start..]);
    }
    parts
}
/// Index of the first top-level `=` (assignment separator). Equality is spelled `==`,
/// so a depth-0 `=` is unambiguously the statement's assignment.
pub(super) fn find_top_level_equal(tokens: &[Token]) -> Option<usize> {
    find_top_level(tokens, TokenKind::Equal)
}

/// The split of a binding's `: TYPE [= VALUE]` tail into its type-annotation and
/// optional value token slices.
pub(super) struct BindingSplit<'t> {
    /// The type-annotation tokens. Owned only in the glued `>=` case, where a synthetic
    /// closing `>` is appended; borrowed otherwise.
    pub type_tokens: Cow<'t, [Token]>,
    /// The value tokens after the boundary, or `None` for a value-less binding
    /// (`var x: T`).
    pub value_tokens: Option<&'t [Token]>,
    /// The span of the boundary `=`/`>=` to anchor a value diagnostic at, or `None`
    /// for a value-less binding.
    pub equal_span: Option<SourceSpan>,
}

/// Split the tokens after a binding's `:` into the type annotation and the optional
/// value. The boundary is the first depth-0 `=`, or a `>=` that glues a generic close
/// to the assignment (`const m: Map<string, int>= m`) — the one token-split the angle
/// grammar needs, contributing a synthetic `>` to the type. Depth counts `(`/`[` only:
/// a generic's own `<`/`>` never wrap the top-level assignment.
pub(super) fn split_type_and_value(after_colon: &[Token]) -> BindingSplit<'_> {
    let mut depth = 0usize;
    for (index, token) in after_colon.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen | TokenKind::LeftBracket => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket => depth = depth.saturating_sub(1),
            TokenKind::Equal if depth == 0 => {
                return BindingSplit {
                    type_tokens: Cow::Borrowed(&after_colon[..index]),
                    value_tokens: Some(&after_colon[index + 1..]),
                    equal_span: Some(token.span),
                };
            }
            TokenKind::GreaterEqual if depth == 0 => {
                let close = Token {
                    kind: TokenKind::Greater,
                    span: SourceSpan {
                        start_byte: token.span.start_byte,
                        end_byte: token.span.start_byte + 1,
                        line: token.span.line,
                        column: token.span.column,
                    },
                };
                let mut type_tokens = after_colon[..index].to_vec();
                type_tokens.push(close);
                return BindingSplit {
                    type_tokens: Cow::Owned(type_tokens),
                    value_tokens: Some(&after_colon[index + 1..]),
                    equal_span: Some(token.span),
                };
            }
            _ => {}
        }
    }
    BindingSplit {
        type_tokens: Cow::Borrowed(after_colon),
        value_tokens: None,
        equal_span: None,
    }
}
/// Index of the first token satisfying `predicate` at parenthesis/bracket depth 0. The
/// predicate receives the candidate index and the full slice so it can peek at
/// neighbouring tokens.
fn find_at_top_level(
    tokens: &[Token],
    predicate: impl Fn(usize, &[Token]) -> bool,
) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::LeftParen | TokenKind::LeftBracket => depth += 1,
            TokenKind::RightParen | TokenKind::RightBracket => depth = depth.saturating_sub(1),
            _ if depth == 0 && predicate(index, tokens) => return Some(index),
            _ => {}
        }
    }
    None
}
/// Index of the first occurrence of `kind` at parenthesis/bracket depth 0.
pub(super) fn find_top_level(tokens: &[Token], kind: TokenKind) -> Option<usize> {
    find_at_top_level(tokens, |index, tokens| tokens[index].kind == kind)
}
/// Index of the first compound-assign operator (`+=`, `-=`, `*=`, `/=`, `%=`) at
/// depth 0, so one inside a call argument does not split the statement.
pub(super) fn find_top_level_compound_assign(tokens: &[Token]) -> Option<usize> {
    find_at_top_level(tokens, |index, tokens| {
        matches!(
            tokens[index].kind,
            TokenKind::PlusEqual
                | TokenKind::MinusEqual
                | TokenKind::StarEqual
                | TokenKind::SlashEqual
                | TokenKind::PercentEqual
        )
    })
}
/// The zero-width gap position just after `anchor`, where a missing operand that
/// follows a `=`/keyword/operator is reported.
pub(super) fn gap_after(anchor: SourceSpan) -> SourceSpan {
    SourceSpan {
        start_byte: anchor.end_byte,
        end_byte: anchor.end_byte,
        line: anchor.line,
        column: anchor.column,
    }
}

/// The zero-width gap position just before `anchor`, where a missing assignment
/// target that precedes a `=` is reported.
fn gap_before(anchor: SourceSpan) -> SourceSpan {
    SourceSpan {
        start_byte: anchor.start_byte,
        end_byte: anchor.start_byte,
        line: anchor.line,
        column: anchor.column,
    }
}

/// Parse `tokens` as one complete expression anchored at `gap`. A failure is
/// reported once — at the failure token by the expression parser, or at the first
/// trailing token here when a complete expression is followed by tokens that are
/// not part of it — and yields `None`, so every `None` carries a diagnostic.
fn expr_slice(
    source: &str,
    tokens: &[Token],
    gap: SourceSpan,
    sink: &mut SyntaxSink<'_>,
) -> Option<Expression> {
    match ExprParser::new(source, tokens, gap, sink).parse_complete() {
        ParseComplete::Complete(expr) => Some(expr),
        ParseComplete::Reported => None,
        ParseComplete::Incomplete(span) => {
            let reason = ParseDiagnosticReason::Expected(ExpectedSyntax::Expression);
            sink.push(SyntaxError::new(
                DiagnosticReason::Parser(reason),
                "expected an expression",
                None,
                span,
            ));
            None
        }
    }
}

/// Parse `tokens` as one complete expression. An empty slice has no source bytes to
/// anchor a missing-expression diagnostic at, so the caller supplies a guaranteed-valid
/// `anchor` (the enclosing keyword, operator, or line); the line-0/column-0 default span
/// is never a valid source location.
pub(super) fn expr_of(
    source: &str,
    tokens: &[Token],
    anchor: SourceSpan,
    sink: &mut SyntaxSink<'_>,
) -> Option<Expression> {
    let gap = tokens.first().map_or(anchor, |token| token.span);
    expr_slice(source, tokens, gap, sink)
}

/// Parse the operand that follows `anchor` — a `=`, statement keyword, or operator the
/// caller stripped. An absent operand is reported at the gap just past `anchor` rather
/// than on the keyword itself.
pub(super) fn expr_of_after(
    source: &str,
    tokens: &[Token],
    anchor: SourceSpan,
    sink: &mut SyntaxSink<'_>,
) -> Option<Expression> {
    expr_slice(source, tokens, gap_after(anchor), sink)
}

/// Parse an assignment target that precedes `anchor` — the `=` that follows it.
/// An absent target reports the missing expression at the gap just before `=`.
pub(super) fn expr_of_before(
    source: &str,
    tokens: &[Token],
    anchor: SourceSpan,
    sink: &mut SyntaxSink<'_>,
) -> Option<Expression> {
    expr_slice(source, tokens, gap_before(anchor), sink)
}

/// Parse an operand inside a `for` header. A malformed or empty operand is
/// reported once against the whole header by the caller, so this silent probe
/// writes through a discarding sink and yields `None`.
pub(super) fn expr_of_in_header(
    source: &str,
    tokens: &[Token],
    anchor: SourceSpan,
) -> Option<Expression> {
    let gap = tokens.first().map_or(anchor, |token| token.span);
    let mut discarding = DiscardingSyntaxErrorSink;
    match ExprParser::new(source, tokens, gap, &mut discarding).parse_complete() {
        ParseComplete::Complete(expr) => Some(expr),
        ParseComplete::Reported | ParseComplete::Incomplete(_) => None,
    }
}
/// Parse a type annotation into the structural [`TypeExpr`]. This is the one owner of
/// type-spelling grammar: generic applications `Head<..>`, `Id(^root)`, and the `?`
/// suffix are classified here so no downstream crate re-reads the spelling. The slice
/// must be exactly one type production; a malformed or over-long spelling reports the
/// diagnostic the caller's `expected`/`message` name.
pub(super) fn parse_type(
    source: &str,
    tokens: &[Token],
    expected: ExpectedSyntax,
    message: &'static str,
) -> ParseResult<TypeExpr> {
    // Every later type walk recurses on this production, so nesting must fail closed
    // here against the same limit expressions use rather than overflowing the native
    // stack at resolution time.
    if let Some(span) = type_nesting_overflow(tokens) {
        return Err(ParseError::at(
            span,
            ParseDiagnosticReason::NestingLimit,
            format!("type nests deeper than the limit of {NESTING_DEPTH_LIMIT}"),
        ));
    }
    if let Some(equal) = tokens.iter().find(|token| token.kind == TokenKind::Equal) {
        return Err(ParseError::at(
            equal.span,
            ParseDiagnosticReason::Expected(expected),
            message,
        ));
    }
    // A type annotation is a single type production, so any depth-0 token past its end
    // (an `in`, `@`, `where`, or a second bare word) is not part of the type; reject it
    // where it begins rather than gluing it into the spelling.
    let end = type_token_len(tokens);
    if let Some(trailing) = tokens.get(end) {
        // A complete production already precedes this token, so naming the stray token
        // is accurate where the caller's "expected <type>" prose would falsely report
        // the type as missing. A doubled `??`/`?.` spells a double optional, which
        // optionality forbids, so it keeps its own guidance.
        let detail: Cow<str> = if matches!(
            trailing.kind,
            TokenKind::QuestionQuestion | TokenKind::QuestionDot
        ) {
            Cow::Borrowed("an optional type is written `T?`")
        } else {
            Cow::Owned(format!(
                "unexpected `{}` after the {}",
                trailing.text(source),
                type_context_noun(expected)
            ))
        };
        return Err(ParseError::at(
            trailing.span,
            ParseDiagnosticReason::Expected(expected),
            detail,
        ));
    }
    build_type_expr(source, tokens, expected)
}

/// The noun for the type position a stray trailing token followed, so a rejection names
/// the context ("field type", "parameter type", ...) rather than a generic "type".
fn type_context_noun(expected: ExpectedSyntax) -> &'static str {
    match expected {
        ExpectedSyntax::FieldType => "field type",
        ExpectedSyntax::ParameterType => "parameter type",
        ExpectedSyntax::FunctionReturnType => "return type",
        _ => "type",
    }
}

/// The number of leading tokens making up one complete type production: the head token,
/// then each following `::` name segment, an attached `<...>` generic or `Id(...)`
/// group, then one optional trailing `?`. Group contents are spanned whole, so nested
/// types inside them do not end the type.
fn type_token_len(tokens: &[Token]) -> usize {
    let mut index = if tokens.is_empty() { 0 } else { 1 };
    while index < tokens.len() {
        match tokens[index].kind {
            TokenKind::DoubleColon => index += 2,
            TokenKind::Less | TokenKind::LeftParen => match balanced_group_end(tokens, index) {
                Some(close) => index = close + 1,
                None => return tokens.len(),
            },
            _ => break,
        }
    }
    if tokens.get(index).map(|token| token.kind) == Some(TokenKind::Question) {
        index += 1;
    }
    index.min(tokens.len())
}

/// The span of the delimiter that first opens a type nested deeper than
/// [`NESTING_DEPTH_LIMIT`], or `None` when the type stays within the limit. Counts
/// generic `<` and identity `(` opens, mirroring the limit the lexer and expression
/// parser enforce.
fn type_nesting_overflow(tokens: &[Token]) -> Option<SourceSpan> {
    let mut depth = 0usize;
    for token in tokens {
        match token.kind {
            TokenKind::Less | TokenKind::LeftParen => {
                depth += 1;
                if depth > NESTING_DEPTH_LIMIT {
                    return Some(token.span);
                }
            }
            TokenKind::Greater | TokenKind::RightParen => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

/// Index of the delimiter that closes the group opened at `open`, matching nested
/// generic `<...>` and identity `(...)` groups, or `None` when it never closes. Depth
/// tracking is exact because a nested generic close inside a type slice is always a bare
/// `>`: no `>>` token exists, and a `>=`-glued binding boundary is split off upstream.
fn balanced_group_end(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, token) in tokens[open..].iter().enumerate() {
        match token.kind {
            TokenKind::Less | TokenKind::LeftParen => depth += 1,
            TokenKind::Greater | TokenKind::RightParen => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}
/// Classify one validated type production: a trailing `?` is the optional suffix, a
/// generic application `Head<..>` recurses on its arguments, `Id(^root)` is a saved-store
/// identity, and everything else is a name resolved downstream. A malformed identity or
/// a `?` with no base is rejected here rather than deferred as a semantic error.
fn build_type_expr(
    source: &str,
    tokens: &[Token],
    expected: ExpectedSyntax,
) -> ParseResult<TypeExpr> {
    let span = join_spans(tokens[0].span, tokens[tokens.len() - 1].span);
    // A `?` with no base names no type to make optional.
    if let Some((last, base)) = tokens.split_last()
        && last.kind == TokenKind::Question
    {
        if base.is_empty() {
            return Err(ParseError::at(
                last.span,
                ParseDiagnosticReason::Expected(expected),
                "expected a type before `?`",
            ));
        }
        return Ok(TypeExpr::Optional {
            inner: Box::new(build_type_expr(source, base, expected)?),
            span,
        });
    }
    if opens_as_identity(tokens) {
        return build_identity(source, tokens, span, expected);
    }
    if let Some(apply) = build_apply(source, tokens, span, expected)? {
        return Ok(apply);
    }
    Ok(TypeExpr::Name {
        text: type_text(source, tokens),
        segment_spans: type_name_segment_spans(tokens),
        span,
    })
}

/// Exact token spans for a type spelling that is only a `::`-separated name.
/// Other unresolved spellings retain their whole span but deliberately expose no
/// identifier occurrence.
fn type_name_segment_spans(tokens: &[Token]) -> Vec<SourceSpan> {
    if tokens.is_empty() {
        return Vec::new();
    }
    for (index, token) in tokens.iter().enumerate() {
        let valid = if index.is_multiple_of(2) {
            match token.kind {
                TokenKind::Identifier => true,
                TokenKind::Keyword(keyword) => keyword.lexical_class() == LexicalClass::BuiltinType,
                _ => false,
            }
        } else {
            token.kind == TokenKind::DoubleColon
        };
        if !valid {
            return Vec::new();
        }
    }
    if tokens.len().is_multiple_of(2) {
        return Vec::new();
    }
    tokens.iter().step_by(2).map(|token| token.span).collect()
}

/// A generic type application `Head<Arg, ...>`: any identifier head whose `<...>` group
/// spans the whole tail, with comma-separated type arguments. The semantic owner resolves
/// the head (a reserved `Option`/`Result`/`List`/`Map` or a user-declared generic) and
/// owns argument arity, so an unknown head or wrong arity is a checker diagnostic.
fn build_apply(
    source: &str,
    tokens: &[Token],
    span: SourceSpan,
    expected: ExpectedSyntax,
) -> ParseResult<Option<TypeExpr>> {
    let open = name_run_len(tokens);
    let Some(last) = tokens.len().checked_sub(1) else {
        return Ok(None);
    };
    // A `::`-separated identifier run followed by `<` in type position opens a generic
    // application; `<` has no other meaning here. Anything not opening this way is a
    // plain name.
    if open == 0 || tokens.get(open).map(|token| token.kind) != Some(TokenKind::Less) {
        return Ok(None);
    }
    // An unclosed or short group is a targeted parse error, not a name absorbing the
    // stray `<` — reported at the opening `<` so the missing close is unambiguous.
    if tokens[last].kind != TokenKind::Greater || balanced_group_end(tokens, open) != Some(last) {
        return Err(ParseError::at(
            tokens[open].span,
            ParseDiagnosticReason::Expected(ExpectedSyntax::CloseTypeArguments),
            "expected `>` to close the type arguments",
        ));
    }
    let head = type_text(source, &tokens[..open]);
    let head_span = join_spans(tokens[0].span, tokens[open - 1].span);
    let inner = &tokens[open + 1..last];
    let mut args = Vec::new();
    for part in split_top_level_commas(inner) {
        if part.is_empty() {
            return Err(ParseError::at(
                span,
                ParseDiagnosticReason::Expected(expected),
                "a generic type argument is missing",
            ));
        }
        args.push(build_type_expr(source, part, expected)?);
    }
    Ok(Some(TypeExpr::Apply {
        head,
        head_span,
        args,
        span,
    }))
}

/// The number of leading tokens forming a `::`-separated identifier run, or 0 when
/// the slice does not open with an identifier. The run is the head of a generic
/// application; a qualified head names a type in the tree the first segment's alias
/// declares.
fn name_run_len(tokens: &[Token]) -> usize {
    if tokens.first().map(|token| token.kind) != Some(TokenKind::Identifier) {
        return 0;
    }
    let mut len = 1;
    while tokens.get(len).map(|token| token.kind) == Some(TokenKind::DoubleColon)
        && tokens.get(len + 1).map(|token| token.kind) == Some(TokenKind::Identifier)
    {
        len += 2;
    }
    len
}

/// Whether a token slice opens as an identity constructor `Id ( ^`. `Id` is reserved, so
/// this opening always intends a saved-store identity: the parser commits to that reading
/// and reports a malformed one rather than folding it into a name.
fn opens_as_identity(tokens: &[Token]) -> bool {
    matches!(
        tokens,
        [id, open, caret, ..]
            if id.kind == TokenKind::Keyword(Keyword::Id)
                && open.kind == TokenKind::LeftParen
                && caret.kind == TokenKind::Caret
    )
}

/// Build the saved-store identity named by a slice that opens `Id ( ^`. The only
/// well-formed spelling is `Id ( ^ root )` with a single saved-root name; a dotted or
/// empty root, or stray tokens after the close, is a targeted parse error rather than an
/// unresolvable name the checker would misreport.
fn build_identity(
    source: &str,
    tokens: &[Token],
    span: SourceSpan,
    expected: ExpectedSyntax,
) -> ParseResult<TypeExpr> {
    let malformed_root = |at: SourceSpan| {
        ParseError::at(
            at,
            ParseDiagnosticReason::Expected(expected),
            "the root of `Id(...)` must be a single saved-root name",
        )
    };
    let open = 1;
    let caret = 2;
    let Some(close) = balanced_group_end(tokens, open) else {
        return Err(malformed_root(tokens[open].span));
    };
    let root_tokens = &tokens[caret + 1..close];
    let [root] = root_tokens else {
        // An empty root points at the close paren where a name should be; a longer root
        // points at the first token past the name that breaks it up.
        let at = root_tokens
            .get(1)
            .map_or(tokens[close].span, |token| token.span);
        return Err(malformed_root(at));
    };
    if root.kind != TokenKind::Identifier {
        return Err(malformed_root(root.span));
    }
    if let Some(trailing) = tokens.get(close + 1) {
        return Err(ParseError::at(
            trailing.span,
            ParseDiagnosticReason::Expected(expected),
            "unexpected tokens after `Id(...)`",
        ));
    }
    Ok(TypeExpr::Identity(Box::new(IdentityTypeExpr {
        root: root.text(source).to_string(),
        keyword_span: tokens[0].span,
        caret_span: tokens[caret].span,
        root_span: root.span,
        span,
    })))
}

/// The whitespace-free source spelling of a type-token slice, so a wrapped annotation
/// formats as one line and its digest is stable across reformatting.
fn type_text(source: &str, tokens: &[Token]) -> String {
    let start = tokens[0].span.start_byte;
    let end = tokens[tokens.len() - 1].span.end_byte;
    source[start..end]
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}
/// The span of a token slice, falling back to `empty` when the slice holds no tokens.
/// There is deliberately no zero-argument form: every caller must supply a
/// guaranteed-valid anchor (the enclosing statement keyword or the line's first token) so
/// the line-0/column-0 default span stays unreachable from any diagnostic.
pub(super) fn line_span_or(tokens: &[Token], empty: SourceSpan) -> SourceSpan {
    match (tokens.first(), tokens.last()) {
        (Some(first), Some(last)) => join_spans(first.span, last.span),
        _ => empty,
    }
}
/// Index of the token that ends the line or header starting at `pos`
/// (`NEWLINE`/`{`/`}`/`EOF`), or `tokens.len()` if none follows. Newlines suppressed
/// inside open delimiters or after a continuation token never reach the token stream, so
/// the first newline seen here really does end the line.
pub(super) fn line_end(tokens: &[Token], pos: usize) -> usize {
    let mut index = pos;
    while index < tokens.len()
        && !matches!(
            tokens[index].kind,
            TokenKind::Newline | TokenKind::LeftBrace | TokenKind::RightBrace | TokenKind::Eof
        )
    {
        index += 1;
    }
    index
}
pub(super) fn is_line_comment(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::Comment | TokenKind::DocComment)
}
/// Build a `Comment` from a line comment token, stripping the leading marker and
/// surrounding whitespace so the formatter renders a canonical `// text` line.
pub(super) fn comment_from_token(
    source: &str,
    token: Token,
    placement: CommentPlacement,
    marker: CommentMarker,
) -> Comment {
    let text = token
        .text(source)
        .trim_start_matches('/')
        .trim()
        .to_string();
    Comment {
        text,
        placement,
        marker,
        span: token.span,
    }
}
