//! The `surface` declaration body: contextual surface item lines and
//! source-native collection targets.

use super::tokens::{comment_from_token, split_top_level_commas};
use super::{DeclParser, ParseError, ParseResult};
use crate::ast::{
    Comment, CommentMarker, CommentPlacement, SavedRoot, SurfaceDecl, SurfaceItem, SurfaceTarget,
};
use crate::diagnostic::{ExpectedSyntax, ParseDiagnosticReason, SourceSpan};
use crate::token::{Token, TokenKind};

impl<'a> DeclParser<'a> {
    pub(super) fn parse_surface(&mut self) -> SurfaceDecl {
        let span = self.header_span();
        let err = self.content_span();
        let header = self.take_header_line();
        let (name, store) = match parse_surface_head(self.source, &header[1..]) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.error_span(err, error.reason, error.message);
                (
                    String::new(),
                    SavedRoot {
                        root: String::new(),
                        keys: Vec::new(),
                        span: SourceSpan::default(),
                    },
                )
            }
        };
        let (items, comments) = if matches!(self.peek(), Some(TokenKind::Indent)) {
            let (items, comments, attempted_item) = self.parse_surface_items();
            if items.is_empty() && !attempted_item {
                self.error_span(
                    span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceBody),
                    "expected an indented surface body with at least one item",
                );
            }
            (items, comments)
        } else {
            self.error_span(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceBody),
                "expected an indented surface body",
            );
            (Vec::new(), Vec::new())
        };
        SurfaceDecl {
            name,
            store,
            items,
            comments,
            span,
        }
    }

    fn parse_surface_items(&mut self) -> (Vec<SurfaceItem>, Vec<Comment>, bool) {
        let mut items = Vec::new();
        let mut comments = Vec::new();
        let mut attempted_item = false;
        self.advance(); // INDENT
        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Dedent => {
                    self.advance();
                    break;
                }
                TokenKind::Newline => {
                    self.advance();
                }
                TokenKind::Comment | TokenKind::DocComment => {
                    let token = self.advance();
                    comments.push(comment_from_token(
                        self.source,
                        token,
                        CommentPlacement::OwnLine,
                        comment_marker(token.kind),
                    ));
                    if matches!(self.peek(), Some(TokenKind::Newline)) {
                        self.advance();
                    }
                }
                TokenKind::Indent => {
                    self.advance();
                    if self.peek().is_some_and(|kind| {
                        !matches!(
                            kind,
                            TokenKind::Dedent | TokenKind::Newline | TokenKind::Eof
                        )
                    }) {
                        let err = self.content_span();
                        self.error_span(
                            err,
                            ParseDiagnosticReason::UnexpectedIndentation,
                            "unexpected indented block in surface body",
                        );
                    }
                    self.skip_to_block_end();
                }
                _ => {
                    attempted_item = true;
                    if let Some(item) = self.parse_surface_item(&mut comments) {
                        items.push(item);
                    }
                }
            }
        }
        (items, comments, attempted_item)
    }

    fn parse_surface_item(&mut self, comments: &mut Vec<Comment>) -> Option<SurfaceItem> {
        let span = self.header_span();
        let err = self.content_span();
        let lead = self.tokens[self.pos];
        let lead_word = (lead.kind == TokenKind::Identifier).then(|| lead.text(self.source));
        let (header, trailing_comment) = self.take_header_line_with_trailing_comment();
        comments.extend(trailing_comment);
        match lead_word {
            Some("fields") => self.parse_surface_field_item(span, err, &header[1..]),
            Some("collection") => self.parse_surface_collection_item(span, err, &header[1..]),
            Some("create") => self.parse_surface_create_item(span, err, &header[1..]),
            Some("update") => self.parse_surface_update_item(span, err, &header[1..]),
            _ => {
                self.error_span(
                    err,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceItem),
                    "expected a surface item: `fields`, `collection`, `create`, or `update`",
                );
                None
            }
        }
    }

    fn parse_surface_field_item(
        &mut self,
        span: SourceSpan,
        err: SourceSpan,
        tokens: &[Token],
    ) -> Option<SurfaceItem> {
        surface_name_list(self.source, tokens)
            .map(|names| SurfaceItem::Fields { names, span })
            .map_err(|error| self.error_span(err, error.reason, error.message))
            .ok()
    }

    fn parse_surface_create_item(
        &mut self,
        span: SourceSpan,
        err: SourceSpan,
        tokens: &[Token],
    ) -> Option<SurfaceItem> {
        surface_name_list(self.source, tokens)
            .map(|names| SurfaceItem::Create { names, span })
            .map_err(|error| self.error_span(err, error.reason, error.message))
            .ok()
    }

    fn parse_surface_update_item(
        &mut self,
        span: SourceSpan,
        err: SourceSpan,
        tokens: &[Token],
    ) -> Option<SurfaceItem> {
        surface_name_list(self.source, tokens)
            .map(|names| SurfaceItem::Update { names, span })
            .map_err(|error| self.error_span(err, error.reason, error.message))
            .ok()
    }

    fn parse_surface_collection_item(
        &mut self,
        span: SourceSpan,
        err: SourceSpan,
        tokens: &[Token],
    ) -> Option<SurfaceItem> {
        match surface_collection(self.source, tokens) {
            Ok((target, alias)) => Some(SurfaceItem::Collection {
                target,
                alias,
                span,
            }),
            Err(error) => {
                self.error_span(err, error.reason, error.message);
                None
            }
        }
    }
}

fn parse_surface_head(source: &str, tokens: &[Token]) -> ParseResult<(String, SavedRoot)> {
    let name = match tokens.first() {
        Some(token) if token.kind == TokenKind::Identifier => token.text(source).to_string(),
        _ => {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceName),
                "expected surface name",
            ));
        }
    };
    let rest = &tokens[1..];
    if !matches!(rest.first(), Some(token) if token.kind == TokenKind::Identifier && token.text(source) == "from")
    {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceHeader),
            "expected `surface Name from ^store`",
        ));
    }
    let store = match rest.get(1..) {
        Some([caret, root])
            if caret.kind == TokenKind::Caret && root.kind == TokenKind::Identifier =>
        {
            SavedRoot {
                root: root.text(source).to_string(),
                keys: Vec::new(),
                span: crate::parse_expr::join_spans(caret.span, root.span),
            }
        }
        _ => {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceStore),
                "expected saved root beginning with `^` after `from`",
            ));
        }
    };
    Ok((name, store))
}

fn surface_name_list(source: &str, tokens: &[Token]) -> ParseResult<Vec<String>> {
    if tokens.is_empty() {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceFieldList),
            "expected one or more field names",
        ));
    }
    let mut names = Vec::new();
    for part in split_top_level_commas(tokens) {
        match part {
            [token] if token.kind == TokenKind::Identifier => {
                names.push(token.text(source).to_string());
            }
            _ => {
                return Err(ParseError::new(
                    ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceFieldList),
                    "expected comma-separated field names",
                ));
            }
        }
    }
    if names.is_empty() {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceFieldList),
            "expected one or more field names",
        ));
    }
    Ok(names)
}

fn surface_collection(source: &str, tokens: &[Token]) -> ParseResult<(SurfaceTarget, String)> {
    let (target, rest) = surface_collection_target(source, tokens)?;
    let Some((as_token, alias_tokens)) = rest.split_first() else {
        return Err(surface_collection_error());
    };
    if as_token.kind == TokenKind::Dot {
        return Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceCollectionTarget),
            "expected collection target `^root` or `^root.index`",
        ));
    }
    if as_token.kind != TokenKind::Identifier || as_token.text(source) != "as" {
        return Err(surface_collection_error());
    }
    let alias = match alias_tokens {
        [token] if token.kind == TokenKind::Identifier => token.text(source).to_string(),
        _ => {
            return Err(ParseError::new(
                ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceCollection),
                "expected collection alias after `as`",
            ));
        }
    };
    Ok((target, alias))
}

fn surface_collection_target<'a>(
    source: &str,
    tokens: &'a [Token],
) -> ParseResult<(SurfaceTarget, &'a [Token])> {
    match tokens {
        [caret, root, dot, index, rest @ ..]
            if caret.kind == TokenKind::Caret
                && root.kind == TokenKind::Identifier
                && dot.kind == TokenKind::Dot
                && index.kind == TokenKind::Identifier =>
        {
            Ok((
                SurfaceTarget::Index {
                    root: root.text(source).to_string(),
                    index: index.text(source).to_string(),
                },
                rest,
            ))
        }
        [caret, root] if caret.kind == TokenKind::Caret && root.kind == TokenKind::Identifier => {
            Ok((
                SurfaceTarget::Root {
                    root: root.text(source).to_string(),
                },
                &[],
            ))
        }
        [caret, root, rest @ ..]
            if caret.kind == TokenKind::Caret && root.kind == TokenKind::Identifier =>
        {
            Ok((
                SurfaceTarget::Root {
                    root: root.text(source).to_string(),
                },
                rest,
            ))
        }
        _ => Err(ParseError::new(
            ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceCollectionTarget),
            "expected collection target `^root` or `^root.index`",
        )),
    }
}

fn surface_collection_error() -> ParseError {
    ParseError::new(
        ParseDiagnosticReason::Expected(ExpectedSyntax::SurfaceCollection),
        "expected `collection <target> as <alias>`",
    )
}

fn comment_marker(kind: TokenKind) -> CommentMarker {
    match kind {
        TokenKind::DocComment => CommentMarker::Doc,
        _ => CommentMarker::Line,
    }
}
