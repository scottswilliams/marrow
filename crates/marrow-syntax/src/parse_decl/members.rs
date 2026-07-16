//! The resource, store, and enum member bodies: the shared indented-block trivia
//! framing and the per-member parsing for fields, groups, indexes, and enum
//! members.

use super::body::BodyLine;
use super::head::{enum_member_name, parse_field_or_group_tokens, parse_index_tokens};
use super::{DeclParser, MemberHead, ParseError};
use crate::ast::{Comment, EnumMember, FieldDecl, GroupDecl, IndexDecl, ResourceMember};
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

        let stray = ParseError::new(
            ParseDiagnosticReason::UnexpectedIndentation,
            "unexpected indentation in resource body; only groups introduce nested resource members",
        );
        while self.peek().is_some() {
            match self.next_body_line(&mut docs, &mut comments, &stray) {
                BodyLine::End => break,
                BodyLine::Trivia => continue,
                BodyLine::Item => {
                    // The node carries the whole-line span (column 1); a member
                    // error points at the content after the indentation.
                    let is_index = matches!(self.peek(), Some(TokenKind::Keyword(Keyword::Index)));
                    let span = self.header_span();
                    let err = self.content_span();
                    let member_docs = self.take_docs_for_current_item(&mut docs, &mut comments);
                    let (header, trailing_comment) = self.take_header_line_with_trailing_comment();
                    comments.extend(trailing_comment);
                    if is_index {
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
                self.report(err, error);
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
                name_span,
                keys,
                ty,
            }) => Some(ResourceMember::Field(FieldDecl {
                docs,
                required,
                name,
                name_span,
                keys,
                ty,
                span,
            })),
            Ok(MemberHead::Group {
                name,
                name_span,
                keys,
            }) => {
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
                debug_assert!(child_indexes.is_empty());
                Some(ResourceMember::Group(GroupDecl {
                    docs,
                    name,
                    name_span,
                    keys,
                    members: children,
                    comments: child_comments,
                    span,
                }))
            }
            Err(error) => {
                self.report(err, error);
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
        let stray = ParseError::new(
            ParseDiagnosticReason::EnumMemberMustBeBareName,
            "an enum member has no nested body",
        );
        while self.peek().is_some() {
            match self.next_body_line(&mut docs, &mut comments, &stray) {
                BodyLine::End => break,
                BodyLine::Trivia => continue,
                BodyLine::Item => {
                    let span = self.header_span();
                    let err = self.content_span();
                    let member_docs = self.take_docs_for_current_item(&mut docs, &mut comments);
                    let (header, trailing_comment) = self.take_header_line_with_trailing_comment();
                    comments.extend(trailing_comment);
                    match enum_member_name(self.source, header) {
                        Ok(head) => {
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
                                name: head.name,
                                name_span: head.name_span,
                                category: head.category,
                                payload: head.payload,
                                members: nested,
                                comments: nested_comments,
                                span,
                            });
                        }
                        Err(error) => self.report(err, error),
                    }
                }
            }
        }
        self.flush_docs_as_comments(&mut docs, &mut comments);
        (members, comments)
    }
}
