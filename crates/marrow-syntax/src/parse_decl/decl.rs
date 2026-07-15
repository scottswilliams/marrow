//! The declaration parser: `DeclParser` and the top-level dispatch that frames
//! each declaration body. It owns the module, use, const, resource, store, enum,
//! and function declarations and delegates statement and expression parsing.

use super::FunctionHead;
use super::head::{parse_enum_head, parse_resource_head, parse_store_head};
use super::params::parse_function_head;
use super::stmt::StmtParser;
use super::tokens::{
    PathNameError, comment_from_token, doc_comment_text, find_top_level_equal, import_name,
    line_span_or, line_text_end_before, module_name, parse_type,
};
use crate::ast::{
    AliasDecl, Block, Comment, CommentMarker, CommentPlacement, ConstDecl, Declaration, EnumDecl,
    Expression, FunctionDecl, ModuleDecl, NominalDecl, ParsedSource, ResourceDecl, SavedRoot,
    SourceFile, StoreDecl, StructDecl, SupportSpelling, TestDecl, TypeExpr, UseDecl,
};
use crate::diagnostic::{Diagnostic, ExpectedSyntax, ParseDiagnosticReason, SourceSpan};
use crate::literal::decode_string_literal;
use crate::token::{Keyword, Token, TokenKind, is_identifier, keyword, tokens_in_range};

/// Recursive-descent parser for top-level declarations over the file-wide token
/// stream, the same stream `StmtParser`/`ExprParser` consume. It dispatches on
/// token shape, frames resource and function bodies by `INDENT`/`DEDENT` tokens,
/// and delegates statement and expression parsing to those parsers. A
/// declaration spans its whole first physical line at column 1.
pub(crate) struct DeclParser<'a> {
    pub(super) source: &'a str,
    pub(super) tokens: &'a [Token],
    pub(super) pos: usize,
    pub(super) diagnostics: Vec<Diagnostic>,
}

impl<'a> DeclParser<'a> {
    pub(crate) fn new(source: &'a str, tokens: &'a [Token]) -> Self {
        Self {
            source,
            tokens,
            pos: 0,
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn parse(mut self) -> ParsedSource {
        let mut file = SourceFile::default();
        let mut docs: Vec<Token> = Vec::new();
        let mut saw_top_level_item = false;

        while let Some(kind) = self.peek() {
            match kind {
                TokenKind::Eof => break,
                TokenKind::Newline | TokenKind::Dedent => {
                    self.advance();
                }
                TokenKind::Comment => {
                    let token = self.advance();
                    file.comments.push(comment_from_token(
                        self.source,
                        token,
                        CommentPlacement::OwnLine,
                        CommentMarker::Line,
                    ));
                }
                TokenKind::DocComment => {
                    self.push_pending_doc(&mut docs, &mut file.comments);
                }
                _ => {
                    self.dispatch_top_level(&mut file, &mut docs, saw_top_level_item);
                    saw_top_level_item = true;
                }
            }
        }
        self.flush_docs_as_comments(&mut docs, &mut file.comments);

        ParsedSource {
            file,
            diagnostics: self.diagnostics,
        }
    }

    /// Parse one top-level construct at the current header line: a declaration
    /// keyword, an enum or function header, a stray indented region, or an
    /// unknown declaration. Each declaration keyword introduces its kind only
    /// when a space follows it, so a bare or glued keyword (such as `module::x`)
    /// falls through to the unknown-declaration arm.
    fn dispatch_top_level(
        &mut self,
        file: &mut SourceFile,
        docs: &mut Vec<Token>,
        saw_top_level_item: bool,
    ) {
        match self.peek() {
            // Indentation where a top-level declaration was expected: report each
            // stray indented line.
            Some(TokenKind::Indent) => self.report_stray_indented_lines(),
            Some(TokenKind::Keyword(Keyword::Module)) if self.keyword_introduces_decl() => {
                self.flush_docs_as_comments(docs, &mut file.comments);
                let trailing_comment = self.peek_header_trailing_comment();
                self.parse_module(file, saw_top_level_item);
                file.comments.extend(trailing_comment);
            }
            Some(TokenKind::Keyword(Keyword::Use)) if self.keyword_introduces_decl() => {
                self.flush_docs_as_comments(docs, &mut file.comments);
                let trailing_comment = self.peek_header_trailing_comment();
                self.parse_use(file);
                file.comments.extend(trailing_comment);
            }
            Some(TokenKind::Keyword(Keyword::Const)) if self.keyword_introduces_decl() => {
                let trailing_comment = self.peek_header_trailing_comment();
                let decl_docs = self.take_docs_for_current_item(docs, &mut file.comments);
                let decl = self.parse_const(decl_docs);
                file.declarations.push(Declaration::Const(decl));
                file.comments.extend(trailing_comment);
            }
            Some(TokenKind::Keyword(Keyword::Alias)) if self.keyword_introduces_decl() => {
                let trailing_comment = self.peek_header_trailing_comment();
                let decl_docs = self.take_docs_for_current_item(docs, &mut file.comments);
                let decl = self.parse_alias(decl_docs);
                file.declarations.push(Declaration::Alias(decl));
                file.comments.extend(trailing_comment);
            }
            Some(TokenKind::Keyword(Keyword::Type)) if self.keyword_introduces_decl() => {
                let trailing_comment = self.peek_header_trailing_comment();
                let decl_docs = self.take_docs_for_current_item(docs, &mut file.comments);
                let decl = self.parse_nominal(decl_docs);
                file.declarations.push(Declaration::Nominal(decl));
                file.comments.extend(trailing_comment);
            }
            Some(TokenKind::Keyword(Keyword::Resource)) if self.keyword_introduces_decl() => {
                let trailing_comment = self.peek_header_trailing_comment();
                let decl_docs = self.take_docs_for_current_item(docs, &mut file.comments);
                let resource = self.parse_resource(decl_docs);
                file.declarations.push(Declaration::Resource(resource));
                file.comments.extend(trailing_comment);
            }
            Some(TokenKind::Keyword(Keyword::Struct)) if self.keyword_introduces_decl() => {
                let trailing_comment = self.peek_header_trailing_comment();
                let decl_docs = self.take_docs_for_current_item(docs, &mut file.comments);
                let decl = self.parse_struct(decl_docs);
                file.declarations.push(Declaration::Struct(decl));
                file.comments.extend(trailing_comment);
            }
            Some(TokenKind::Keyword(Keyword::Store)) if self.keyword_introduces_decl() => {
                let trailing_comment = self.peek_header_trailing_comment();
                let decl_docs = self.take_docs_for_current_item(docs, &mut file.comments);
                let store = self.parse_store(decl_docs);
                file.declarations.push(Declaration::Store(store));
                file.comments.extend(trailing_comment);
            }
            // `evolve` needs no trailing-space gate: its header is the bare
            // keyword, with the steps in the indented block below.
            Some(TokenKind::Keyword(Keyword::Evolve)) => {
                self.flush_docs_as_comments(docs, &mut file.comments);
                let trailing_comment = self.peek_header_trailing_comment();
                let evolve = self.parse_evolve();
                file.declarations.push(Declaration::Evolve(evolve));
                file.comments.extend(trailing_comment);
            }
            Some(TokenKind::Keyword(Keyword::Test)) if self.keyword_introduces_decl() => {
                let trailing_comment = self.peek_header_trailing_comment();
                let decl_docs = self.take_docs_for_current_item(docs, &mut file.comments);
                let test = self.parse_test(decl_docs);
                file.declarations.push(Declaration::Test(test));
                file.comments.extend(trailing_comment);
            }
            _ if self.starts_enum_header() => {
                let trailing_comment = self.peek_header_trailing_comment();
                let decl_docs = self.take_docs_for_current_item(docs, &mut file.comments);
                let decl = self.parse_enum(decl_docs);
                file.declarations.push(Declaration::Enum(decl));
                file.comments.extend(trailing_comment);
            }
            _ if self.starts_function_header() => {
                let trailing_comment = self.peek_header_trailing_comment();
                let decl_docs = self.take_docs_for_current_item(docs, &mut file.comments);
                let function = self.parse_function(decl_docs);
                file.declarations.push(Declaration::Function(function));
                file.comments.extend(trailing_comment);
            }
            // `pub` gates only `fn` and `enum`; a `pub resource`/`pub store` is
            // reported at the `pub` token, which is then dropped so the rest of the
            // declaration parses and raises no follow-on cascade.
            Some(TokenKind::Keyword(Keyword::Pub)) if self.pub_precedes_ungated_decl() => {
                let pub_token = self.advance();
                self.error_span(
                    pub_token.span,
                    ParseDiagnosticReason::InvalidVisibility,
                    "resources and stores are not visibility-gated; remove `pub`",
                );
                self.dispatch_top_level(file, docs, saw_top_level_item);
            }
            _ => {
                self.flush_docs_as_comments(docs, &mut file.comments);
                self.error_header(
                    ParseDiagnosticReason::Expected(ExpectedSyntax::Declaration),
                    "expected module, use, alias, type, const, resource, store, or fn declaration",
                );
            }
        }
    }

    pub(super) fn push_pending_doc(&mut self, docs: &mut Vec<Token>, comments: &mut Vec<Comment>) {
        let token = self.advance();
        if docs
            .last()
            .is_some_and(|last| token.span.line > last.span.line + 1)
        {
            self.flush_docs_as_comments(docs, comments);
        }
        docs.push(token);
    }

    pub(super) fn take_docs_for_current_item(
        &mut self,
        docs: &mut Vec<Token>,
        comments: &mut Vec<Comment>,
    ) -> Vec<String> {
        let item_line = self.tokens[self.pos].span.line;
        if docs
            .last()
            .is_some_and(|last| item_line == last.span.line + 1)
        {
            return docs
                .drain(..)
                .map(|token| doc_comment_text(token.text(self.source)))
                .collect();
        }
        self.flush_docs_as_comments(docs, comments);
        Vec::new()
    }

    /// Drain accumulated `;;` doc comments that found no following declaration,
    /// member, or parameter to attach to. A dangling doc comment is a syntax error
    /// — a swallowed doc comment is one the formatter cannot place, breaking the
    /// check-run-format round trip — so each is reported and retained as trivia for
    /// the formatter to surface alongside the diagnostic.
    pub(super) fn flush_docs_as_comments(
        &mut self,
        docs: &mut Vec<Token>,
        comments: &mut Vec<Comment>,
    ) {
        for token in docs.drain(..) {
            self.error_span(
                token.span,
                ParseDiagnosticReason::DocCommentWithoutTarget,
                "a `;;` doc comment must precede a declaration, member, or parameter",
            );
            comments.push(comment_from_token(
                self.source,
                token,
                CommentPlacement::OwnLine,
                CommentMarker::Doc,
            ));
        }
    }

    fn parse_module(&mut self, file: &mut SourceFile, saw_top_level_item: bool) {
        let span = self.header_span();
        let header = self.take_header_line();
        let name = module_name(self.source, &header[1..]);
        if saw_top_level_item {
            self.error_span(
                span,
                ParseDiagnosticReason::LateModuleDeclaration,
                "module declaration must appear once at the start of the file",
            );
        } else {
            match name {
                Ok(name) => file.module = Some(ModuleDecl { name, span }),
                Err(PathNameError::ReservedSegment(reserved)) => {
                    self.report_reserved_path_segment(reserved);
                }
                Err(PathNameError::NotQualified) => self.error_span(
                    span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::ModuleName),
                    "expected qualified module name",
                ),
            }
        }
    }

    fn parse_use(&mut self, file: &mut SourceFile) {
        let span = self.header_span();
        let header = self.take_header_line();
        match import_name(self.source, &header[1..]) {
            Ok(name) => file.uses.push(UseDecl { name, span }),
            Err(PathNameError::ReservedSegment(reserved)) => {
                self.report_reserved_path_segment(reserved);
            }
            Err(PathNameError::NotQualified) => self.error_span(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::ImportName),
                "expected qualified import name",
            ),
        }
    }

    /// Report a reserved word used as a `use`/`module` path segment, at the
    /// offending segment rather than the declaration keyword.
    fn report_reserved_path_segment(&mut self, reserved: Token) {
        let word = reserved.text(self.source);
        self.error_span(
            reserved.span,
            ParseDiagnosticReason::KeywordPathSegment,
            format!("`{word}` is a keyword and cannot be a path segment"),
        );
    }

    fn parse_const(&mut self, docs: Vec<String>) -> ConstDecl {
        let span = self.header_span();
        let header = self.take_header_line();
        // `const Name [: type] = value`. The name is the identifier after the
        // keyword; the type runs from `:` to `=`; the value is everything after.
        let equal = find_top_level_equal(&header[1..]).map(|index| index + 1);
        let (name, ty, value) = match equal {
            Some(equal) => {
                let head = &header[1..equal];
                let value_tokens = &header[equal + 1..];
                // A missing value is reported before checking the name and
                // type, so its diagnostic sorts first on the line.
                if value_tokens.is_empty() {
                    self.error_span(
                        span,
                        ParseDiagnosticReason::ConstRequiresValue,
                        "const declarations require a value after `=`",
                    );
                }
                let (name, ty) = self.const_name_type(span, head);
                let value = self.value_expression(value_tokens);
                (name, ty, value)
            }
            None => {
                self.error_span(
                    span,
                    ParseDiagnosticReason::ConstRequiresValue,
                    "const declarations require `=` and a value",
                );
                let (name, ty) = self.const_name_type(span, &header[1..]);
                (name, ty, None)
            }
        };
        ConstDecl {
            docs,
            name,
            ty,
            value,
            span,
        }
    }

    /// Split a const head (`Name` or `Name: type`) into the name and optional
    /// type, reporting a non-identifier name or malformed type annotation.
    fn const_name_type(&mut self, span: SourceSpan, head: &[Token]) -> (String, Option<TypeExpr>) {
        let colon = head.iter().position(|token| token.kind == TokenKind::Colon);
        let (name_tokens, type_tokens) = match colon {
            Some(index) => (&head[..index], Some(&head[index + 1..])),
            None => (head, None),
        };
        // The name is the verbatim text before any `:`, kept even when invalid so
        // the declaration still carries a name; only a non-identifier reports.
        let name = match name_tokens.first().zip(name_tokens.last()) {
            Some((first, last)) => self.source[first.span.start_byte..last.span.end_byte]
                .trim()
                .to_string(),
            None => String::new(),
        };
        // A reserved word is not an identifier (per the grammar), so it cannot name
        // a const any more than it can name a param, member, or key.
        if keyword(&name).is_some() {
            self.error_span(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::ConstName),
                format!("`{name}` is a keyword and cannot be used as a const name"),
            );
        } else if !is_identifier(&name) {
            self.error_span(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::ConstName),
                "expected const name before type annotation",
            );
        }
        // No `:` leaves the type absent; a `:` with no tokens after it reports
        // and yields no type. Semantic type resolution belongs downstream.
        let ty = type_tokens.and_then(|tokens| {
            if !tokens.is_empty() {
                match parse_type(
                    self.source,
                    tokens,
                    ExpectedSyntax::ConstType,
                    "expected const type annotation",
                ) {
                    Ok(parsed) => return Some(parsed),
                    Err(error) => {
                        self.report(span, error);
                        return None;
                    }
                }
            }
            self.error_span(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::ConstType),
                "expected const type annotation",
            );
            None
        });
        (name, ty)
    }

    /// Parse an `alias Name = Type` header line: a transparent type alias. The
    /// name is one identifier; the target type runs from `=` to end of line and
    /// is parsed by the shared type grammar. A missing `=`, keyword name, or
    /// malformed type reports at the header and keeps the declaration node so
    /// parsing stays total.
    fn parse_alias(&mut self, docs: Vec<String>) -> AliasDecl {
        let span = self.header_span();
        let header = self.take_header_line();
        let equal = find_top_level_equal(&header[1..]).map(|index| index + 1);
        let (name_tokens, type_tokens): (&[Token], Option<&[Token]>) = match equal {
            Some(equal) => (&header[1..equal], Some(&header[equal + 1..])),
            None => {
                self.error_span(
                    span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::AliasType),
                    "an alias declaration is `alias Name = Type`",
                );
                (&header[1..], None)
            }
        };
        let (name, name_span) = match name_tokens {
            [token] if token.kind == TokenKind::Identifier => {
                (token.text(self.source).to_string(), token.span)
            }
            _ => {
                let name = match name_tokens.first().zip(name_tokens.last()) {
                    Some((first, last)) => self.source[first.span.start_byte..last.span.end_byte]
                        .trim()
                        .to_string(),
                    None => String::new(),
                };
                let message = if keyword(&name).is_some() {
                    format!("`{name}` is a keyword and cannot be used as an alias name")
                } else {
                    "expected an alias name before `=`".to_string()
                };
                self.error_span(
                    span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::AliasName),
                    message,
                );
                (name, span)
            }
        };
        let ty = type_tokens.and_then(|tokens| {
            if tokens.is_empty() {
                self.error_span(
                    span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::AliasType),
                    "expected a target type after `=`",
                );
                return None;
            }
            match parse_type(
                self.source,
                tokens,
                ExpectedSyntax::AliasType,
                "expected an alias target type",
            ) {
                Ok(parsed) => Some(parsed),
                Err(error) => {
                    self.report(span, error);
                    None
                }
            }
        });
        AliasDecl {
            docs,
            name,
            name_span,
            ty,
            span,
        }
    }

    /// Parse a nominal type header line: `type Name: base in lo..hi` with an
    /// optional `supports cap, ...` tail. The name is one identifier; the base
    /// type runs from `:` to the `in` keyword; the interval is one range
    /// expression; the capabilities are comma-separated identifiers. Each missing
    /// or malformed piece reports once and leaves its slot empty so parsing stays
    /// total; base admission, the literal-range rule, and the closed capability
    /// set are checker rules.
    fn parse_nominal(&mut self, docs: Vec<String>) -> NominalDecl {
        let span = self.header_span();
        let header = self.take_header_line();
        let rest = &header[1..];

        let (name, name_span, rest) = match rest.first() {
            Some(token) if token.kind == TokenKind::Identifier => {
                (token.text(self.source).to_string(), token.span, &rest[1..])
            }
            Some(token) if matches!(token.kind, TokenKind::Keyword(_)) => {
                let text = token.text(self.source).to_string();
                self.error_span(
                    token.span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::NominalName),
                    format!("`{text}` is a keyword and cannot be used as a type name"),
                );
                (text, token.span, &rest[1..])
            }
            _ => {
                self.error_span(
                    span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::NominalName),
                    "a nominal type declaration is `type Name: base in lo..hi`",
                );
                return NominalDecl {
                    docs,
                    name: String::new(),
                    name_span: span,
                    base: None,
                    interval: None,
                    supports: Vec::new(),
                    span,
                };
            }
        };

        let rest = match rest.first() {
            Some(token) if token.kind == TokenKind::Colon => &rest[1..],
            _ => {
                self.error_span(
                    span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::NominalBase),
                    "expected `:` and a base type after the nominal type name",
                );
                rest
            }
        };

        // The base type runs to the `in` keyword; the interval to `supports` or
        // end of line. Neither keyword occurs inside a type or range spelling.
        let in_at = rest
            .iter()
            .position(|token| token.kind == TokenKind::Keyword(Keyword::In));
        let (base_tokens, tail) = match in_at {
            Some(index) => (&rest[..index], &rest[index + 1..]),
            None => {
                self.error_span(
                    span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::NominalInterval),
                    "a nominal type requires an `in lo..hi` interval",
                );
                (rest, &[][..])
            }
        };
        let base = if base_tokens.is_empty() {
            if in_at.is_some() {
                self.error_span(
                    span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::NominalBase),
                    "expected a base type between `:` and `in`",
                );
            }
            None
        } else {
            match parse_type(
                self.source,
                base_tokens,
                ExpectedSyntax::NominalBase,
                "expected a nominal base type",
            ) {
                Ok(parsed) => Some(parsed),
                Err(error) => {
                    self.report(span, error);
                    None
                }
            }
        };

        let supports_at = tail
            .iter()
            .position(|token| token.kind == TokenKind::Keyword(Keyword::Supports));
        let (interval_tokens, supports_tokens) = match supports_at {
            Some(index) => (&tail[..index], &tail[index + 1..]),
            None => (tail, &[][..]),
        };
        let interval = if interval_tokens.is_empty() {
            if in_at.is_some() {
                self.error_span(
                    span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::NominalInterval),
                    "expected an interval such as `0..150` after `in`",
                );
            }
            None
        } else {
            let interval_span = line_span_or(interval_tokens, span);
            self.parse_expr_with_fallback(
                interval_tokens,
                interval_span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::NominalInterval),
                "expected an interval such as `0..150` after `in`",
            )
        };

        let supports = self.parse_supports_list(span, supports_at.is_some(), supports_tokens);
        NominalDecl {
            docs,
            name,
            name_span,
            base,
            interval,
            supports,
            span,
        }
    }

    /// Parse the `supports` capability tail of a nominal header: identifiers
    /// separated by commas. A malformed tail reports once at the offending token.
    fn parse_supports_list(
        &mut self,
        header_span: SourceSpan,
        has_supports: bool,
        tokens: &[Token],
    ) -> Vec<SupportSpelling> {
        if !has_supports {
            return Vec::new();
        }
        if tokens.is_empty() {
            self.error_span(
                header_span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::NominalSupports),
                "expected a capability list after `supports`",
            );
            return Vec::new();
        }
        let mut supports = Vec::new();
        let mut expect_name = true;
        for token in tokens {
            match (expect_name, token.kind) {
                (true, TokenKind::Identifier) => {
                    supports.push(SupportSpelling {
                        name: token.text(self.source).to_string(),
                        span: token.span,
                    });
                    expect_name = false;
                }
                (false, TokenKind::Comma) => expect_name = true,
                _ => {
                    self.error_span(
                        token.span,
                        ParseDiagnosticReason::Expected(ExpectedSyntax::NominalSupports),
                        "a `supports` list is capability names separated by commas",
                    );
                    return supports;
                }
            }
        }
        if expect_name {
            self.error_span(
                header_span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::NominalSupports),
                "expected a capability name after `,`",
            );
        }
        supports
    }

    fn parse_resource(&mut self, docs: Vec<String>) -> ResourceDecl {
        let span = self.header_span();
        let header = self.take_header_line();
        let (name, name_span) = match parse_resource_head(self.source, &header[1..]) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.report(span, error);
                (String::new(), SourceSpan::default())
            }
        };
        let (members, indexes, comments) = if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.parse_resource_members(false)
        } else {
            self.error_span(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::ResourceBody),
                "expected an indented resource body",
            );
            (Vec::new(), Vec::new(), Vec::new())
        };
        debug_assert!(indexes.is_empty());
        ResourceDecl {
            docs,
            name,
            name_span,
            members,
            comments,
            span,
        }
    }

    /// Parse a `struct Name` declaration and its indented field body. The header
    /// and body reuse the resource machinery; a struct-specific restriction (no
    /// groups, keys, or `required` keyword) is a checker rule, so the shared parser
    /// stays one owner.
    fn parse_struct(&mut self, docs: Vec<String>) -> StructDecl {
        let span = self.header_span();
        let header = self.take_header_line();
        let (name, name_span) = match parse_resource_head(self.source, &header[1..]) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.report(span, error);
                (String::new(), SourceSpan::default())
            }
        };
        let (members, indexes, comments) = if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.parse_resource_members(false)
        } else {
            self.error_span(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::ResourceBody),
                "expected an indented struct body",
            );
            (Vec::new(), Vec::new(), Vec::new())
        };
        debug_assert!(indexes.is_empty());
        StructDecl {
            docs,
            name,
            name_span,
            members,
            comments,
            span,
        }
    }

    fn parse_store(&mut self, docs: Vec<String>) -> StoreDecl {
        let span = self.header_span();
        let header = self.take_header_line();
        let (root, resource) = match parse_store_head(self.source, &header[1..]) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.report(span, error);
                (
                    SavedRoot {
                        root: String::new(),
                        keys: Vec::new(),
                        span: SourceSpan::default(),
                    },
                    String::new(),
                )
            }
        };
        let (indexes, comments) = if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.parse_store_members()
        } else {
            (Vec::new(), Vec::new())
        };
        StoreDecl {
            docs,
            root,
            resource,
            indexes,
            comments,
            span,
        }
    }

    fn parse_enum(&mut self, docs: Vec<String>) -> EnumDecl {
        let span = self.header_span();
        let header = self.take_header_line();
        let (public, name, name_span) = match parse_enum_head(self.source, header) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.report(span, error);
                (false, String::new(), SourceSpan::default())
            }
        };
        let (members, comments) = if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.parse_enum_members()
        } else {
            self.error_span(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::EnumBody),
                "expected an indented enum body",
            );
            (Vec::new(), Vec::new())
        };
        if members.is_empty() {
            self.error_span(
                span,
                ParseDiagnosticReason::EnumNeedsMember,
                "an enum needs at least one member",
            );
        }
        EnumDecl {
            docs,
            public,
            name,
            name_span,
            members,
            comments,
            span,
        }
    }
    fn parse_function(&mut self, docs: Vec<String>) -> FunctionDecl {
        let span = self.header_span();
        let header = self.take_header_line();
        let head = match parse_function_head(self.source, header) {
            Ok(head) => head,
            Err(error) => {
                self.report(span, error);
                FunctionHead {
                    public: false,
                    name: String::new(),
                    params: Vec::new(),
                    return_type: None,
                }
            }
        };
        let body = if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.parse_function_body()
        } else {
            self.error_span(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::FunctionBody),
                "expected an indented function body",
            );
            Block {
                statements: Vec::new(),
                comments: Vec::new(),
                span,
            }
        };
        FunctionDecl {
            docs,
            public: head.public,
            name: head.name,
            params: head.params,
            return_type: head.return_type,
            body,
            span,
        }
    }

    /// Parse a `test "name"` declaration: the header is the `test` keyword followed
    /// by exactly one string literal (the report title), then an indented body of
    /// statements, where the owned `assert` is legal. A missing or non-string title
    /// reports `parse.syntax` and yields an empty name so parsing stays total.
    fn parse_test(&mut self, docs: Vec<String>) -> TestDecl {
        let span = self.header_span();
        let header = self.take_header_line();
        let (name, name_span) = match header.get(1) {
            Some(token) if token.kind == TokenKind::String => {
                let name = decode_string_literal(token.text(self.source)).unwrap_or_default();
                (name, token.span)
            }
            other => {
                let name_span = other.map_or(span, |token| token.span);
                self.error_span(
                    name_span,
                    ParseDiagnosticReason::Expected(ExpectedSyntax::TestName),
                    "a test declaration is `test \"name\"` with a string-literal title",
                );
                (String::new(), name_span)
            }
        };
        let body = if matches!(self.peek(), Some(TokenKind::Indent)) {
            self.parse_function_body()
        } else {
            self.error_span(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::TestBody),
                "expected an indented test body",
            );
            Block {
                statements: Vec::new(),
                comments: Vec::new(),
                span,
            }
        };
        TestDecl {
            docs,
            name,
            name_span,
            body,
            span,
        }
    }

    /// Parse a function body from its `INDENT … DEDENT` run via the statement
    /// parser. The body span runs from the first body line at column 1 to the end
    /// of the last physical line of the body.
    pub(super) fn parse_function_body(&mut self) -> Block {
        let indent = self.tokens[self.pos];
        let start = self.pos;
        let end = self.consume_block();
        // The block ends just before the line that closed it; that line is where
        // the matching `DEDENT` sits (or end-of-file for the final body).
        let closing = self.tokens[start..end]
            .iter()
            .rev()
            .find(|token| token.kind == TokenKind::Dedent)
            .map(|dedent| dedent.span.start_byte - (dedent.span.column as usize - 1))
            .unwrap_or_else(|| self.source.len());
        let span = SourceSpan {
            start_byte: indent.span.start_byte,
            end_byte: line_text_end_before(self.source, closing),
            line: indent.span.line,
            column: 1,
        };
        // Feed the statement parser a byte-bounded slice: tokens inside the body
        // span, so a `DEDENT` emitted past the last body line (at end of file) is
        // excluded and nested-block spans stay anchored to the source.
        let body_tokens = tokens_in_range(self.tokens, span.start_byte, span.end_byte);
        let (statements, comments, diagnostics) =
            StmtParser::new(self.source, body_tokens).parse_block();
        self.diagnostics.extend(diagnostics);
        Block {
            statements,
            comments,
            span,
        }
    }

    /// Parse a value-position expression. Returns `None` when the value text
    /// does not parse as a complete expression. An absent value is already
    /// reported by the caller, so only a present-but-malformed value raises a
    /// diagnostic here (a type spelling such as `int` in value position lands
    /// here, where it is a syntax error rather than a silent acceptance).
    fn value_expression(&mut self, tokens: &[Token]) -> Option<Expression> {
        if tokens.is_empty() {
            return None;
        }
        // A written-but-malformed value keeps its span as an error node rather than
        // vanishing, so tooling that locates the initializer (signature help, the
        // const header boundary) still sees where the value began.
        let span = line_span_or(tokens, tokens[0].span);
        Some(
            self.parse_expr_with_fallback(
                tokens,
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
                "expected an expression",
            )
            .unwrap_or(Expression::Error { span }),
        )
    }
}
