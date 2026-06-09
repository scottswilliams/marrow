//! The resource, store, and enum member bodies: the shared indented-block trivia
//! framing and the per-member parsing for fields, groups, indexes, and enum
//! members.

use super::head::{enum_member_name, parse_field_or_group_tokens, parse_index_tokens};
use super::tokens::comment_from_token;
use super::{DeclParser, MemberBlockFrame, MemberHead, ParseError};
use crate::ast::{
    Comment, CommentMarker, CommentPlacement, EnumMember, FieldDecl, GroupDecl, IndexDecl,
    ResourceMember,
};
use crate::diagnostic::{ExpectedSyntax, ParseDiagnosticReason, SourceSpan};
use crate::token::{Keyword, Token, TokenKind};

impl<'a> DeclParser<'a> {
    pub(super) fn parse_store_members(&mut self) -> (Vec<IndexDecl>, Vec<Comment>) {
        let (members, indexes, comments) = self.parse_resource_members(true);
        for member in members {
            self.error_span(
                member.span(),
                ParseDiagnosticReason::ResourceMemberInStoreBody,
                "store bodies accept only index declarations",
            );
        }
        (indexes, comments)
    }

    /// Advance over the trivia at the head of an indented member block — a
    /// closing `DEDENT`, blank `NEWLINE`s, own-line comments, accumulated doc
    /// comments, and a stray deeper indent (reported as `stray_indent` and
    /// skipped) — and report what the next token is. A `Member` frame leaves the
    /// member header in place for the caller to parse; this owns only the layout
    /// shared by the resource and enum member loops.
    pub(super) fn next_member_block_frame(
        &mut self,
        docs: &mut Vec<Token>,
        comments: &mut Vec<Comment>,
        stray_indent: &ParseError,
    ) -> MemberBlockFrame {
        match self.peek() {
            Some(TokenKind::Dedent) => {
                self.advance();
                MemberBlockFrame::Done
            }
            Some(TokenKind::Newline) => {
                self.advance();
                MemberBlockFrame::Trivia
            }
            Some(TokenKind::Comment) => {
                let token = self.advance();
                comments.push(comment_from_token(
                    self.source,
                    token,
                    CommentPlacement::OwnLine,
                    CommentMarker::Line,
                ));
                MemberBlockFrame::Trivia
            }
            Some(TokenKind::DocComment) => {
                self.push_pending_doc(docs, comments);
                MemberBlockFrame::Trivia
            }
            Some(TokenKind::Indent) => {
                self.advance(); // INDENT
                if self.peek().is_some_and(|kind| {
                    !matches!(
                        kind,
                        TokenKind::Dedent | TokenKind::Newline | TokenKind::Eof
                    )
                }) {
                    let err = self.content_span();
                    self.error_span(err, stray_indent.reason.clone(), stray_indent.message);
                }
                self.skip_to_block_end();
                MemberBlockFrame::Trivia
            }
            _ => MemberBlockFrame::Member,
        }
    }

    /// Parse an `INDENT … DEDENT` block of resource members. Nested groups recurse
    /// on their own child block. Each member's span is its whole header line.
    pub(super) fn parse_resource_members(
        &mut self,
        allow_indexes: bool,
    ) -> (Vec<ResourceMember>, Vec<IndexDecl>, Vec<Comment>) {
        let mut members = Vec::new();
        let mut indexes = Vec::new();
        let mut comments = Vec::new();
        let mut docs: Vec<Token> = Vec::new();
        self.advance(); // INDENT

        let stray_indent = ParseError::new(
            ParseDiagnosticReason::UnexpectedIndentation,
            "unexpected indentation in resource body; only groups introduce nested resource members",
        );
        while let Some(kind) = self.peek() {
            match self.next_member_block_frame(&mut docs, &mut comments, &stray_indent) {
                MemberBlockFrame::Done => break,
                MemberBlockFrame::Trivia => continue,
                MemberBlockFrame::Member => {
                    // The node carries the whole-line span (column 1); a member
                    // error points at the content after the indentation.
                    let span = self.header_span();
                    let err = self.content_span();
                    let member_docs = self.take_docs_for_current_item(&mut docs, &mut comments);
                    let header = self.take_header_line();
                    if matches!(kind, TokenKind::Keyword(Keyword::Index)) {
                        if let Some(index) =
                            self.parse_index_member(allow_indexes, span, err, member_docs, header)
                        {
                            indexes.push(index);
                        }
                    } else if let Some(member) =
                        self.parse_field_or_group_member(span, err, member_docs, header)
                    {
                        members.push(member);
                    }
                }
            }
        }
        self.flush_docs_as_comments(&mut docs, &mut comments);
        (members, indexes, comments)
    }

    /// Parse one `index` line in a resource or store body. Returns the index only
    /// when indexes are allowed here and the line parses; otherwise it reports the
    /// relevant diagnostic and yields nothing.
    fn parse_index_member(
        &mut self,
        allow_indexes: bool,
        span: SourceSpan,
        err: SourceSpan,
        docs: Vec<String>,
        header: &[Token],
    ) -> Option<IndexDecl> {
        match parse_index_tokens(self.source, &header[1..]) {
            Ok(index) if allow_indexes => Some(IndexDecl {
                docs,
                span,
                ..index
            }),
            Ok(_) => {
                self.error_span(
                    err,
                    ParseDiagnosticReason::IndexOutsideStoreBody,
                    "index declarations belong in a store body",
                );
                None
            }
            Err(error) => {
                self.error_span(err, error.reason, error.message);
                None
            }
        }
    }

    /// Parse one field or group header line and, for a group, its nested member
    /// block. Returns the member, or `None` when the header does not parse.
    fn parse_field_or_group_member(
        &mut self,
        span: SourceSpan,
        err: SourceSpan,
        docs: Vec<String>,
        header: &[Token],
    ) -> Option<ResourceMember> {
        match parse_field_or_group_tokens(self.source, header) {
            Ok(MemberHead::Field {
                required,
                name,
                keys,
                ty,
            }) => Some(ResourceMember::Field(FieldDecl {
                docs,
                required,
                name,
                keys,
                ty,
                span,
            })),
            Ok(MemberHead::Group { name, keys }) => {
                let (children, child_indexes, child_comments) =
                    if matches!(self.peek(), Some(TokenKind::Indent)) {
                        self.parse_resource_members(false)
                    } else {
                        self.error_span(
                            err,
                            ParseDiagnosticReason::Expected(ExpectedSyntax::ResourceBody),
                            "expected an indented resource group body",
                        );
                        (Vec::new(), Vec::new(), Vec::new())
                    };
                for index in child_indexes {
                    self.error_span(
                        index.span,
                        ParseDiagnosticReason::IndexOutsideStoreBody,
                        "index declarations belong in a store body",
                    );
                }
                Some(ResourceMember::Group(GroupDecl {
                    docs,
                    name,
                    keys,
                    members: children,
                    comments: child_comments,
                    span,
                }))
            }
            Err(error) => {
                self.error_span(err, error.reason, error.message);
                None
            }
        }
    }
    /// Parse an `INDENT … DEDENT` block of enum members. A member is a bare
    /// identifier on its own line; anything else (a type annotation, key
    /// parameters, or a deeper indent) is a parse error. This mirrors
    /// `parse_resource_members` but accepts only the bare-name form.
    pub(super) fn parse_enum_members(&mut self) -> (Vec<EnumMember>, Vec<Comment>) {
        let mut members = Vec::new();
        let mut comments = Vec::new();
        let mut docs: Vec<Token> = Vec::new();
        self.advance(); // INDENT

        // A stray indent here opens before any member header to nest under; a
        // member's own nested block is consumed right after its header, below.
        let stray_indent = ParseError::new(
            ParseDiagnosticReason::EnumMemberMustBeBareName,
            "an enum member has no nested body",
        );
        while self.peek().is_some() {
            match self.next_member_block_frame(&mut docs, &mut comments, &stray_indent) {
                MemberBlockFrame::Done => break,
                MemberBlockFrame::Trivia => continue,
                MemberBlockFrame::Member => {
                    let span = self.header_span();
                    let err = self.content_span();
                    let member_docs = self.take_docs_for_current_item(&mut docs, &mut comments);
                    let header = self.take_header_line();
                    match enum_member_name(self.source, header) {
                        Ok((name, category)) => {
                            // A member's children are the indented block that
                            // immediately follows its header, parsed by the same
                            // routine and attached, so members nest to any depth.
                            let (nested, nested_comments) =
                                if matches!(self.peek(), Some(TokenKind::Indent)) {
                                    self.parse_enum_members()
                                } else {
                                    (Vec::new(), Vec::new())
                                };
                            members.push(EnumMember {
                                docs: member_docs,
                                name,
                                category,
                                members: nested,
                                comments: nested_comments,
                                span,
                            });
                        }
                        Err(error) => self.error_span(err, error.reason, error.message),
                    }
                }
            }
        }
        self.flush_docs_as_comments(&mut docs, &mut comments);
        (members, comments)
    }
}
