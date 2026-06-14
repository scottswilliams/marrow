//! The declaration parser: `DeclParser` and the top-level dispatch that frames
//! each declaration body. It owns the module, use, const, resource, store, enum,
//! and function declarations and delegates statement and expression parsing.

use super::FunctionHead;
use super::head::{parse_enum_head, parse_resource_head, parse_store_head};
use super::params::parse_function_head;
use super::stmt::StmtParser;
use super::tokens::{
    comment_from_token, doc_comment_text, find_top_level_equal, import_name, line_span,
    line_text_end_before, module_name, reject_structural_type_tokens, type_ref_from_tokens,
};
use crate::ast::{
    Block, Comment, CommentMarker, CommentPlacement, ConstDecl, Declaration, EnumDecl, Expression,
    FunctionDecl, ModuleDecl, ParsedSource, ResourceDecl, SavedRoot, SourceFile, StoreDecl,
    TypeRef, UseDecl,
};
use crate::diagnostic::{
    Diagnostic, ExpectedSyntax, ParseDiagnosticReason, SourceSpan, UnsupportedSyntax,
};
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
            Some(TokenKind::Keyword(Keyword::Resource)) if self.keyword_introduces_decl() => {
                let trailing_comment = self.peek_header_trailing_comment();
                let decl_docs = self.take_docs_for_current_item(docs, &mut file.comments);
                let resource = self.parse_resource(decl_docs);
                file.declarations.push(Declaration::Resource(resource));
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
            // `type` is not a keyword in Marrow; it lexes as an identifier.
            Some(TokenKind::Identifier)
                if self.identifier_is(self.pos, "type") && self.keyword_introduces_decl() =>
            {
                self.flush_docs_as_comments(docs, &mut file.comments);
                self.error_header(
                    ParseDiagnosticReason::Unsupported(UnsupportedSyntax::TypeAliases),
                    "type aliases are not used in Marrow; declare a resource or use a builtin type directly",
                );
            }
            _ => {
                self.flush_docs_as_comments(docs, &mut file.comments);
                self.error_header(
                    ParseDiagnosticReason::Expected(ExpectedSyntax::Declaration),
                    "expected module, use, const, resource, store, or fn declaration",
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
        &self,
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

    pub(super) fn flush_docs_as_comments(
        &self,
        docs: &mut Vec<Token>,
        comments: &mut Vec<Comment>,
    ) {
        comments.extend(docs.drain(..).map(|token| {
            comment_from_token(
                self.source,
                token,
                CommentPlacement::OwnLine,
                CommentMarker::Doc,
            )
        }));
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
        } else if let Some(name) = name {
            file.module = Some(ModuleDecl { name, span });
        } else {
            self.error_span(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::ModuleName),
                "expected qualified module name",
            );
        }
    }

    fn parse_use(&mut self, file: &mut SourceFile) {
        let span = self.header_span();
        let header = self.take_header_line();
        if let Some(name) = import_name(self.source, &header[1..]) {
            file.uses.push(UseDecl { name, span });
        } else {
            self.error_span(
                span,
                ParseDiagnosticReason::Expected(ExpectedSyntax::ImportName),
                "expected qualified import name",
            );
        }
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
    fn const_name_type(&mut self, span: SourceSpan, head: &[Token]) -> (String, Option<TypeRef>) {
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
                if let Err(error) = reject_structural_type_tokens(
                    tokens,
                    ExpectedSyntax::ConstType,
                    "expected const type annotation",
                ) {
                    self.error_span(span, error.reason, error.message);
                    return None;
                }
                return Some(type_ref_from_tokens(self.source, tokens));
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

    fn parse_resource(&mut self, docs: Vec<String>) -> ResourceDecl {
        let span = self.header_span();
        let header = self.take_header_line();
        let name = match parse_resource_head(self.source, &header[1..]) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.error_span(span, error.reason, error.message);
                String::new()
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
                self.error_span(span, error.reason, error.message);
                (
                    SavedRoot {
                        root: String::new(),
                        keys: Vec::new(),
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
        let (public, name) = match parse_enum_head(self.source, header) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.error_span(span, error.reason, error.message);
                (false, String::new())
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
                self.error_span(span, error.reason, error.message);
                FunctionHead {
                    public: false,
                    name: String::new(),
                    params: Vec::new(),
                    return_presence: crate::FunctionReturnPresence::Always,
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
            return_presence: head.return_presence,
            return_type: head.return_type,
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
        self.parse_expr_with_fallback(
            tokens,
            line_span(tokens),
            ParseDiagnosticReason::Expected(ExpectedSyntax::Expression),
            "expected an expression",
        )
    }
}
