//! Low-level helpers over a token slice shared across the declaration and
//! statement parsers: line and span bounds, top-level delimiter scanning, comment
//! construction, and the bridge into the expression parser.

use std::borrow::Cow;

use super::{ParseError, ParseResult};
use crate::NESTING_DEPTH_LIMIT;
use crate::ast::{
    Comment, CommentMarker, CommentPlacement, Expression, IdentityTypeExpr, TypeExpr,
};
use crate::diagnostic::{
    Diagnostic, DiagnosticReason, ExpectedSyntax, ParseDiagnosticReason, Severity, SourceSpan,
};
use crate::parse_expr::{ExprParser, ParseComplete, join_spans};
use crate::token::{Keyword, Token, TokenKind, is_qualified_name};

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
fn qualified_name_text(source: &str, tokens: &[Token]) -> Option<String> {
    let first = tokens.first()?;
    let last = tokens.last()?;
    let text = &source[first.span.start_byte..last.span.end_byte];
    is_qualified_name(text).then(|| text.to_string())
}
/// Why a `use`/`module` path failed to parse: a reserved word stands where a
/// path segment must be (with the offending token), or the tokens do not spell a
/// dotted/`::`-qualified name at all.
pub(super) enum PathNameError {
    ReservedSegment(Token),
    NotQualified,
}
pub(super) fn module_name(source: &str, tokens: &[Token]) -> Result<String, PathNameError> {
    if let Some(reserved) = reserved_segment(tokens) {
        return Err(PathNameError::ReservedSegment(*reserved));
    }
    qualified_name_text(source, tokens).ok_or(PathNameError::NotQualified)
}
pub(super) fn import_name(source: &str, tokens: &[Token]) -> Result<String, PathNameError> {
    // A project may declare `module std::bytes`, so the reserved type word `bytes`
    // stays legal as that import's final segment; a reserved segment in any other
    // position is the path error. This is a path-shape allowance, not a shipped
    // module.
    if let Some(reserved) =
        reserved_segment(tokens).filter(|_| !is_std_bytes_import(source, tokens))
    {
        return Err(PathNameError::ReservedSegment(*reserved));
    }
    qualified_name_text(source, tokens).ok_or(PathNameError::NotQualified)
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
pub(super) fn push_parse_error(
    diagnostics: &mut Vec<Diagnostic>,
    fallback: SourceSpan,
    error: ParseError,
) {
    let (span, reason, message) = error.locate(fallback);
    diagnostics.push(Diagnostic {
        code: reason.code(),
        reason: DiagnosticReason::Parser(reason),
        severity: Severity::Error,
        message,
        help: None,
        span,
    });
}
/// Drop comment tokens from a token slice. A `;` or `;;` line inside an open
/// delimiter lexes to a `Comment`/`DocComment` token with no newline; like a
/// blank line, it does not separate or close anything, so a declaration list
/// that spans several physical lines reads it as absent. Returns the slice
/// unchanged when it holds no comments, so the common single-line list keeps its
/// borrow.
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
/// Split tokens on top-level commas (depth 0), dropping a trailing empty group
/// from a trailing comma. Runs over declaration and type slices only, where `<`/`>`
/// delimit a generic argument list (`Map<K, V>`) rather than comparing values, so a
/// comma inside a nested generic does not split its enclosing list.
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
/// Index of the first top-level `=` (assignment separator). Equality is `==`, so
/// a depth-0 `=` is unambiguously the assignment in a statement; the depth-0
/// restriction still keeps named-argument colons and nested forms from splitting.
pub(super) fn find_top_level_equal(tokens: &[Token]) -> Option<usize> {
    find_top_level(tokens, TokenKind::Equal)
}

/// The split of a binding's `: TYPE [= VALUE]` tail into its type-annotation and
/// optional value token slices.
pub(super) struct BindingSplit<'t> {
    /// The type-annotation tokens. Owned only in the unspaced `>=` case, where a
    /// synthetic closing `>` is appended to terminate the generic the glued `>=`
    /// left open; borrowed otherwise.
    pub type_tokens: Cow<'t, [Token]>,
    /// The value tokens after the boundary, or `None` for a value-less binding
    /// (`var x: T`).
    pub value_tokens: Option<&'t [Token]>,
    /// The span of the boundary `=`/`>=` to anchor a value diagnostic at, or `None`
    /// for a value-less binding.
    pub equal_span: Option<SourceSpan>,
}

/// Split the tokens after a binding's `:` into the type annotation and the optional
/// value. The boundary is the first top-level (paren/bracket depth 0) `=`, or a
/// `>=` that glues a generic close to the assignment (`const m: Map<string, int>= m`)
/// — the one token-split the angle grammar needs. A `>=` boundary contributes a
/// synthetic closing `>` to the type and consumes the assignment. Depth counts
/// `(`/`[` only: a generic's own `<`/`>` never wrap the top-level assignment, and a
/// glued `>=` must be seen at depth 0 to be split.
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
/// Index of the first token satisfying `predicate` at parenthesis/bracket depth 0.
/// The traversal tracks delimiter depth; the predicate receives each candidate
/// index and the full slice so it can peek at neighbouring tokens.
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
/// Index of the first top-level compound-assign operator token (`+=`, `-=`,
/// `*=`, `/=`, `%=`) at parenthesis/bracket depth 0, so a compound operator
/// inside a call argument does not split the statement.
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
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Expression> {
    match ExprParser::new(source, tokens, gap).parse_complete(diagnostics) {
        ParseComplete::Complete(expr) => Some(expr),
        ParseComplete::Reported => None,
        ParseComplete::Incomplete(span) => {
            diagnostics.push(Diagnostic {
                code: ParseDiagnosticReason::Expected(ExpectedSyntax::Expression).code(),
                reason: DiagnosticReason::Parser(ParseDiagnosticReason::Expected(
                    ExpectedSyntax::Expression,
                )),
                severity: Severity::Error,
                message: "expected an expression".to_string(),
                help: None,
                span,
            });
            None
        }
    }
}

/// Parse `tokens` as one complete expression. An empty slice has no source bytes
/// to anchor a missing-expression diagnostic at, so the caller supplies a
/// guaranteed-valid `anchor` (the enclosing keyword, operator, or line) rather than
/// the invalid line-0/column-0 default span.
pub(super) fn expr_of(
    source: &str,
    tokens: &[Token],
    anchor: SourceSpan,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Expression> {
    let gap = tokens.first().map_or(anchor, |token| token.span);
    expr_slice(source, tokens, gap, diagnostics)
}

/// Parse the operand text that follows `anchor` — a `=`, statement keyword, or
/// operator the caller stripped. An absent operand reports the missing expression
/// at the gap just past `anchor`, so the diagnostic lands there rather than on the
/// statement keyword.
pub(super) fn expr_of_after(
    source: &str,
    tokens: &[Token],
    anchor: SourceSpan,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Expression> {
    expr_slice(source, tokens, gap_after(anchor), diagnostics)
}

/// Parse an assignment target that precedes `anchor` — the `=` that follows it.
/// An absent target reports the missing expression at the gap just before `=`.
pub(super) fn expr_of_before(
    source: &str,
    tokens: &[Token],
    anchor: SourceSpan,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Expression> {
    expr_slice(source, tokens, gap_before(anchor), diagnostics)
}

/// Parse an operand inside a `for` header. A malformed or empty operand is
/// reported once against the whole header by the caller, so this discards the
/// operand's own diagnostics and yields `None`.
pub(super) fn expr_of_in_header(
    source: &str,
    tokens: &[Token],
    anchor: SourceSpan,
) -> Option<Expression> {
    let gap = tokens.first().map_or(anchor, |token| token.span);
    let mut discarded = Vec::new();
    match ExprParser::new(source, tokens, gap).parse_complete(&mut discarded) {
        ParseComplete::Complete(expr) => Some(expr),
        ParseComplete::Reported | ParseComplete::Incomplete(_) => None,
    }
}
/// Parse a type annotation from its token slice into the structural [`TypeExpr`],
/// the one owner of type-spelling grammar. The slice must be exactly one type
/// production; a malformed or over-long spelling reports the same diagnostic the
/// caller's `expected`/`message` name. Generic applications `Head<..>`, `Id(^root)`,
/// and the `?` suffix are classified here so no downstream crate re-reads the spelling.
pub(super) fn parse_type(
    source: &str,
    tokens: &[Token],
    expected: ExpectedSyntax,
    message: &'static str,
) -> ParseResult<TypeExpr> {
    // A type production nests recursively downstream (generic-argument resolution
    // and every later type walk), so its bracket nesting must fail closed here
    // against the same limit expression and layout nesting do, rather than
    // overflowing the native stack at resolution time.
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
    // A type annotation is a single type production: one head word, optionally
    // extended by `::` name segments and an attached `<...>` generic or `Id(...)`
    // group, then an optional trailing `?`. Any depth-0 token past that end (an `in`, `@`,
    // `where`, or a second bare word) is not part of the type; reject it where it
    // begins rather than gluing it into the spelling. A doubled `??` or `?.` in
    // type position is the double-optional spelling, which optionality forbids.
    let end = type_token_len(tokens);
    if let Some(trailing) = tokens.get(end) {
        // A complete type production already precedes this token, so the type is
        // present; naming the stray token is accurate, where reusing the caller's
        // "expected <type>" prose would falsely report the type as missing. A
        // doubled `??`/`?.` is the double-optional spelling, which optionality
        // forbids, so it keeps its own guidance.
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

/// The noun for the type position a stray trailing token followed, so a
/// rejection names the context ("field type", "parameter type", ...) it was
/// parsing rather than a generic "type".
fn type_context_noun(expected: ExpectedSyntax) -> &'static str {
    match expected {
        ExpectedSyntax::FieldType => "field type",
        ExpectedSyntax::ParameterType => "parameter type",
        ExpectedSyntax::FunctionReturnType => "return type",
        _ => "type",
    }
}

/// The number of leading tokens that make up one complete type production: the
/// head token, then each following `::` name segment, an attached generic group
/// `<...>` at depth 0, or the `Id(...)` identity `(...)` group, then one optional
/// trailing `?` suffix. Group contents are spanned whole, so whitespace and nested
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
/// [`NESTING_DEPTH_LIMIT`], or `None` when the type stays within the limit.
/// Counts generic `<` and identity `(` opens, mirroring the limit the lexer and
/// expression parser enforce, so a deep type fails closed before any recursive
/// walk runs.
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
/// generic `<...>` and identity `(...)` groups. Within a delimited type slice a
/// nested generic close is always a bare `>` (no `>>` token exists and any
/// `>=`-glued binding boundary is split off by the statement parser before the
/// slice is formed), so tracking `<`/`>` and `(`/`)` depth is exact. `None` when
/// the group never closes.
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
/// Classify one validated type production into its structure, mirroring the
/// language's spelling grammar: a trailing `?` is the optional suffix,
/// a generic application `Head<..>` recurses on its arguments, `Id(^root)` is a
/// saved-store identity, and everything else is a name resolved downstream. As the sole owner of type
/// grammar, it rejects a structurally malformed identity or a `?` with no base
/// here rather than deferring a misleading semantic error downstream.
fn build_type_expr(
    source: &str,
    tokens: &[Token],
    expected: ExpectedSyntax,
) -> ParseResult<TypeExpr> {
    let span = join_spans(tokens[0].span, tokens[tokens.len() - 1].span);
    // Strip exactly one trailing `?` and wrap the base as an optional. A `?` with
    // no base names no type to make optional.
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
        span,
    })
}

/// A generic type application `Head<Arg, ...>`: any identifier head whose `<...>`
/// group spans the whole tail, with comma-separated type arguments. The head is
/// either a reserved toolchain generic (`Option`/`Result`/`List`/`Map`) or a
/// user-declared generic `struct`/`enum`; the semantic owner resolves it. The
/// applied argument arity is a checker concern, so a wrong arity structures and
/// reports semantically.
fn build_apply(
    source: &str,
    tokens: &[Token],
    span: SourceSpan,
    expected: ExpectedSyntax,
) -> ParseResult<Option<TypeExpr>> {
    let open = 1;
    let Some(last) = tokens.len().checked_sub(1) else {
        return Ok(None);
    };
    // An identifier followed by `<` in type position opens a generic application;
    // `<` has no other meaning here. Anything not opening this way is a plain name.
    if tokens.first().map(|token| token.kind) != Some(TokenKind::Identifier)
        || tokens.get(open).map(|token| token.kind) != Some(TokenKind::Less)
    {
        return Ok(None);
    }
    // The opened group must close with a matching `>` at the end of the production.
    // An unclosed or short group is a targeted parse error, not a name absorbing the
    // stray `<` — reported at the opening `<` so the missing close is unambiguous.
    if tokens[last].kind != TokenKind::Greater || balanced_group_end(tokens, open) != Some(last) {
        return Err(ParseError::at(
            tokens[open].span,
            ParseDiagnosticReason::Expected(ExpectedSyntax::CloseTypeArguments),
            "expected `>` to close the type arguments",
        ));
    }
    // Any identifier head introduces a generic type application: the reserved
    // `Option`/`Result`/`List`/`Map` or a user-declared generic `struct`/`enum`.
    // The semantic owner resolves the head; an unknown one is a checker diagnostic,
    // not a parse error.
    let head = tokens[0].text(source).to_string();
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
    Ok(Some(TypeExpr::Apply { head, args, span }))
}

/// Whether a token slice opens as an identity constructor `Id ( ^`. `Id` is a
/// reserved keyword, so this opening always intends a saved-store identity: the
/// parser commits to that reading and reports a malformed one rather than folding
/// it into a name.
fn opens_as_identity(tokens: &[Token]) -> bool {
    matches!(
        tokens,
        [id, open, caret, ..]
            if id.kind == TokenKind::Keyword(Keyword::Id)
                && open.kind == TokenKind::LeftParen
                && caret.kind == TokenKind::Caret
    )
}

/// Build the saved-store identity a token slice that opens `Id ( ^` names. The
/// only well-formed spelling is `Id ( ^ root )` with a single saved-root name; a
/// dotted or empty root, or stray tokens after the close, is a targeted parse
/// error rather than an unresolvable name the checker would misreport.
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
        // An empty root points at the close paren where a name should be; a longer
        // root points at the first token past the name that breaks it up.
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
    Ok(TypeExpr::Identity(IdentityTypeExpr {
        root: root.text(source).to_string(),
        keyword_span: tokens[0].span,
        caret_span: tokens[caret].span,
        root_span: root.span,
        span,
    }))
}

/// The whitespace-free source spelling of a type-token slice. The stored spelling
/// drops whitespace so a wrapped annotation formats as one line and its digest is
/// stable across reformatting.
fn type_text(source: &str, tokens: &[Token]) -> String {
    let start = tokens[0].span.start_byte;
    let end = tokens[tokens.len() - 1].span.end_byte;
    source[start..end]
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}
/// The span of a token slice, falling back to `empty` when the slice holds no
/// tokens. An empty slice has no source bytes to point at, so every caller must
/// supply a guaranteed-valid anchor (the enclosing statement keyword or the
/// line's first token) to keep a missing-operand diagnostic on a real 1-based
/// position. There is no zero-argument form: the line-0/column-0 default span is
/// never a valid source location, so it must be unreachable from any diagnostic.
pub(super) fn line_span_or(tokens: &[Token], empty: SourceSpan) -> SourceSpan {
    match (tokens.first(), tokens.last()) {
        (Some(first), Some(last)) => join_spans(first.span, last.span),
        _ => empty,
    }
}
/// Index of the token that ends the line or header starting at `pos`
/// (`NEWLINE`/`{`/`}`/`EOF`), or `tokens.len()` if none follows. A header line
/// continues across newlines suppressed inside open delimiters and after a
/// trailing continuation token, so this stops at the first block delimiter or
/// unsuppressed newline rather than any newline.
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
