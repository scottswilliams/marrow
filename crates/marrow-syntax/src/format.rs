//! Render the syntax tree back to canonical Marrow `.mw` source.
//!
//! Canonical style: binary operators are spaced (`a + b`), ranges are not
//! (`1..10`), unary is `-x` / `not x`, calls are `f(a, b)`, and dotted fields
//! and `::` name paths have no surrounding spaces.
//!
//! The syntax tree does not record parentheses, so the formatter re-inserts
//! the minimum needed to preserve operator precedence and associativity.

use crate::{
    AliasDecl, Argument, BinaryOp, Block, CatchClause, CheckedBind, Comment, CommentMarker,
    CommentPlacement, ConstDecl, Declaration, ElseIf, EnumDecl, EnumMember, EvolveDecl, EvolveStep,
    Expression, ForBinding, FunctionDecl, InterpolationPart, KeyParam, LoopOrder, MatchArm,
    NominalDecl, ParamDecl, ResourceDecl, ResourceMember, Statement, StoreDecl, TokenKind,
    TypeExpr, UnaryOp, encode_string_literal,
};

/// Precedence used to decide where parentheses are required, tightest-binding
/// last: atoms bind tightest, then unary, with binary operators below.
const PREC_ATOM: u8 = 12;
const PREC_UNARY: u8 = 11;

const INDENT: &str = "    ";

/// Format a whole `.mw` source file to canonical Marrow. The module
/// declaration, the `use` block, and each top-level declaration are separated by
/// a single blank line, and the result ends with a newline. Inside a body, a
/// single grouping blank line between statements or members is preserved
/// (see `format_block` and `format_body_lines`).
///
/// Normalizing layout makes the output a stable fixed point:
/// `format_source(format_source(s))` equals `format_source(s)`. Line comments
/// inside function bodies are retained as block trivia and re-emitted (see
/// `format_block`). A comment in the middle of a value that spans several lines
/// inside open delimiters is the one position the expression parser does not
/// carry through.
pub fn format_source(source: &str) -> String {
    let parsed = crate::parse_source(source);
    let file = &parsed.file;
    let mut sections: Vec<FormatSection> = Vec::new();

    if let Some(module) = &file.module {
        sections.push(FormatSection {
            span: module.span,
            leading_line: module.span.line,
            text: format!("module {}", module.name),
            kind: FormatSectionKind::Item,
            trailing_comment_line: TrailingCommentLine::Last,
        });
    }
    for comment in &file.comments {
        sections.push(FormatSection {
            span: comment.span,
            leading_line: comment.span.line,
            text: match comment.placement {
                CommentPlacement::OwnLine => format_block_comment(comment, 0),
                CommentPlacement::Trailing => format_trailing_comment(comment),
            },
            kind: FormatSectionKind::Comment(comment.placement),
            trailing_comment_line: TrailingCommentLine::Last,
        });
    }
    for use_decl in &file.uses {
        sections.push(FormatSection {
            span: use_decl.span,
            leading_line: use_decl.span.line,
            text: format!("use {}", use_decl.name),
            kind: FormatSectionKind::Use,
            trailing_comment_line: TrailingCommentLine::Last,
        });
    }
    for declaration in &file.declarations {
        let text = format_declaration(source, declaration);
        let span = declaration_span(declaration);
        sections.push(FormatSection {
            span,
            leading_line: span
                .line
                .saturating_sub(declaration_leading_doc_lines(declaration)),
            trailing_comment_line: declaration_trailing_comment_line(declaration),
            text,
            kind: FormatSectionKind::Item,
        });
    }

    if sections.is_empty() {
        return String::new();
    }
    sections.sort_by_key(|section| section.span.start_byte);
    let sections = merge_trailing_comment_sections(sections);
    let mut out = String::new();
    for (index, section) in sections.iter().enumerate() {
        if index > 0 {
            out.push_str(section_separator(&sections[index - 1], section));
        }
        out.push_str(&section.text);
    }
    out.push('\n');
    out
}

/// Whether replacing `source` with `formatted` would preserve every comment
/// token's marker and normalized text, leave both files parseable, and produce
/// stable formatter output.
pub fn format_preserves_comments(source: &str, formatted: &str) -> bool {
    let parsed_source = crate::parse_source(source);
    let parsed_formatted = crate::parse_source(formatted);
    !parsed_source.has_errors()
        && !parsed_formatted.has_errors()
        && format_source(formatted) == formatted
        && normalized_comment_tokens(source) == normalized_comment_tokens(formatted)
}

struct FormatSection {
    span: crate::SourceSpan,
    /// Source line of the section's first rendered line. For a declaration this
    /// steps above the header over its attached `;;` doc-comment lines, which
    /// render inline with the declaration but sit above its header span; the
    /// separator measures adjacency to a preceding comment against this line so a
    /// `;` comment stays glued to the doc comment that follows it.
    leading_line: u32,
    text: String,
    kind: FormatSectionKind,
    trailing_comment_line: TrailingCommentLine,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FormatSectionKind {
    Comment(CommentPlacement),
    Use,
    Item,
}

fn merge_trailing_comment_sections(sections: Vec<FormatSection>) -> Vec<FormatSection> {
    let mut merged: Vec<FormatSection> = Vec::new();
    for section in sections {
        if matches!(
            section.kind,
            FormatSectionKind::Comment(CommentPlacement::Trailing)
        ) && let Some(previous) = merged.last_mut()
        {
            append_trailing_comment_text(
                &mut previous.text,
                &section.text,
                previous.trailing_comment_line,
            );
        } else {
            merged.push(section);
        }
    }
    merged
}

fn declaration_trailing_comment_line(declaration: &Declaration) -> TrailingCommentLine {
    match declaration {
        Declaration::Alias(_) | Declaration::Nominal(_) | Declaration::Const(_) => {
            TrailingCommentLine::Last
        }
        Declaration::Resource(decl) => TrailingCommentLine::Line(decl.docs.len()),
        Declaration::Store(decl) => TrailingCommentLine::Line(decl.docs.len()),
        Declaration::Function(decl) => {
            TrailingCommentLine::Line(format_function_header_last_line(decl))
        }
        Declaration::Enum(decl) => TrailingCommentLine::Line(decl.docs.len()),
        Declaration::Evolve(_) => TrailingCommentLine::Line(0),
        Declaration::Test(decl) => TrailingCommentLine::Line(decl.docs.len()),
    }
}

fn format_function_header_last_line(decl: &FunctionDecl) -> usize {
    let param_lines = if decl.params.iter().any(|param| !param.docs.is_empty()) {
        1 + decl
            .params
            .iter()
            .map(|param| param.docs.len() + 1)
            .sum::<usize>()
    } else {
        0
    };
    decl.docs.len() + param_lines
}

fn line_count(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\n').count() + 1
}

fn section_separator(prev: &FormatSection, next: &FormatSection) -> &'static str {
    if prev.kind == FormatSectionKind::Use
        && next.kind == FormatSectionKind::Use
        && next.span.line == prev.span.line + 1
    {
        return "\n";
    }
    if matches!(prev.kind, FormatSectionKind::Comment(_)) && next.leading_line == prev.span.line + 1
    {
        return "\n";
    }
    "\n\n"
}

/// The number of `;;` doc-comment lines that render above a declaration's header.
/// Doc comments attach directly above the header on contiguous lines, so this is
/// how far the section's first rendered line sits above its header span.
fn declaration_leading_doc_lines(declaration: &Declaration) -> u32 {
    let docs = match declaration {
        Declaration::Alias(decl) => decl.docs.len(),
        Declaration::Nominal(decl) => decl.docs.len(),
        Declaration::Const(decl) => decl.docs.len(),
        Declaration::Resource(decl) => decl.docs.len(),
        Declaration::Store(decl) => decl.docs.len(),
        Declaration::Function(decl) => decl.docs.len(),
        Declaration::Enum(decl) => decl.docs.len(),
        Declaration::Evolve(_) => 0,
        Declaration::Test(decl) => decl.docs.len(),
    };
    docs as u32
}

fn normalized_comment_tokens(source: &str) -> Vec<(CommentMarker, String)> {
    crate::lex_source(source)
        .tokens
        .iter()
        .filter_map(|token| {
            let marker = match token.kind {
                TokenKind::Comment => CommentMarker::Line,
                TokenKind::DocComment => CommentMarker::Doc,
                _ => return None,
            };
            let text = match marker {
                CommentMarker::Line => token.text(source).strip_prefix(';'),
                CommentMarker::Doc => token.text(source).strip_prefix(";;"),
            }
            .unwrap_or(token.text(source))
            .trim()
            .to_string();
            Some((marker, text))
        })
        .collect()
}

fn declaration_span(declaration: &Declaration) -> crate::SourceSpan {
    match declaration {
        Declaration::Alias(decl) => decl.span,
        Declaration::Nominal(decl) => decl.span,
        Declaration::Const(decl) => decl.span,
        Declaration::Resource(decl) => decl.span,
        Declaration::Store(decl) => decl.span,
        Declaration::Function(decl) => decl.span,
        Declaration::Enum(decl) => decl.span,
        Declaration::Evolve(decl) => decl.span,
        Declaration::Test(decl) => decl.span,
    }
}

/// Render one top-level declaration to canonical fixed layout, independent of
/// input style, with no trailing newline. `source` supplies the original text
/// for any statement body.
pub fn format_declaration(source: &str, declaration: &Declaration) -> String {
    match declaration {
        Declaration::Alias(decl) => format_alias(decl),
        Declaration::Nominal(decl) => format_nominal(decl),
        Declaration::Const(decl) => format_const(decl),
        Declaration::Resource(decl) => format_resource(source, decl),
        Declaration::Store(decl) => format_store(source, decl),
        Declaration::Function(decl) => format_function(source, decl),
        Declaration::Enum(decl) => format_enum(source, decl),
        Declaration::Evolve(decl) => format_evolve(source, decl),
        Declaration::Test(decl) => format_test(source, decl),
    }
}

fn format_evolve(source: &str, decl: &EvolveDecl) -> String {
    let mut out = String::from("evolve");
    let body = format_body_lines(
        source,
        &decl.comments,
        decl.steps.iter().map(|step| {
            let text = format_evolve_step(source, step);
            FormattedBodyLine {
                span: step.span(),
                trailing_comment_line: evolve_step_trailing_comment_line(step),
                text,
            }
        }),
    );
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    out
}

fn format_evolve_step(source: &str, step: &EvolveStep) -> String {
    let step_pad = INDENT;
    match step {
        EvolveStep::Rename { from, to, .. } => format!(
            "{step_pad}rename {} -> {}",
            format_expression_at(from, 1),
            format_expression_at(to, 1)
        ),
        EvolveStep::Default { target, value, .. } => format!(
            "{step_pad}default {} = {}",
            format_expression_at(target, 1),
            format_expression_at(value, 1)
        ),
        EvolveStep::Retire { target, .. } => {
            format!("{step_pad}retire {}", format_expression_at(target, 1))
        }
        EvolveStep::Transform { target, body, .. } => {
            let mut out = format!("{step_pad}transform {}", format_expression_at(target, 1));
            append_body_block(&mut out, &format_block(source, body, 2));
            out
        }
    }
}

fn evolve_step_trailing_comment_line(step: &EvolveStep) -> TrailingCommentLine {
    match step {
        EvolveStep::Transform { target, .. } => {
            let header = format!("{INDENT}transform {}", format_expression_at(target, 1));
            TrailingCommentLine::Line(line_count(&header).saturating_sub(1))
        }
        EvolveStep::Rename { .. } | EvolveStep::Default { .. } | EvolveStep::Retire { .. } => {
            TrailingCommentLine::Last
        }
    }
}

fn format_alias(decl: &AliasDecl) -> String {
    let mut out = format_docs(&decl.docs, 0);
    out.push_str(&format!("alias {} = ", decl.name));
    if let Some(ty) = &decl.ty {
        out.push_str(&ty.to_string());
    }
    out
}

fn format_nominal(decl: &NominalDecl) -> String {
    let mut out = format_docs(&decl.docs, 0);
    out.push_str(&format!("type {}", decl.name));
    if let Some(base) = &decl.base {
        out.push_str(&format!(": {base}"));
    }
    if let Some(interval) = &decl.interval {
        out.push_str(&format!(" in {}", format_expression_at(interval, 0)));
    }
    if !decl.supports.is_empty() {
        let list: Vec<&str> = decl
            .supports
            .iter()
            .map(|support| support.name.as_str())
            .collect();
        out.push_str(&format!(" supports {}", list.join(", ")));
    }
    out
}

fn format_const(decl: &ConstDecl) -> String {
    let mut out = format_docs(&decl.docs, 0);
    out.push_str(&format!(
        "const {}{} = {}",
        decl.name,
        format_type_annotation(&decl.ty),
        format_opt_expression_at(decl.value.as_ref(), 0)
    ));
    out
}

fn format_resource(source: &str, decl: &ResourceDecl) -> String {
    let mut out = format_docs(&decl.docs, 0);
    out.push_str("resource ");
    out.push_str(&decl.name);
    let body = format_resource_body(source, &decl.members, &decl.comments, 1);
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    out
}

fn format_store(source: &str, decl: &StoreDecl) -> String {
    let mut out = format_docs(&decl.docs, 0);
    out.push_str(&format!(
        "store ^{}{}: {}",
        decl.root.root,
        format_key_params(&decl.root.keys),
        decl.resource
    ));
    let body = format_store_body(source, &decl.indexes, &decl.comments, 1);
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    out
}

fn format_enum(source: &str, decl: &EnumDecl) -> String {
    let mut out = format_docs(&decl.docs, 0);
    let visibility = if decl.public { "pub " } else { "" };
    out.push_str(&format!("{visibility}enum {}", decl.name));
    let body = format_enum_body(source, &decl.members, &decl.comments, 1);
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    out
}

/// Render one enum member and its nested members, each on its own line at the
/// given indent depth. A `category` member leads with the `category` word.
fn format_enum_member(source: &str, member: &EnumMember, level: usize) -> String {
    let mut out = format_docs(&member.docs, level);
    let category = if member.category { "category " } else { "" };
    out.push_str(&format!(
        "{}{category}{}",
        INDENT.repeat(level),
        member.name
    ));
    let body = format_enum_body(source, &member.members, &member.comments, level + 1);
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    out
}

fn format_resource_body(
    source: &str,
    members: &[ResourceMember],
    comments: &[Comment],
    level: usize,
) -> String {
    format_body_lines(
        source,
        comments,
        members.iter().map(|member| FormattedBodyLine {
            span: member.span(),
            text: format_resource_member(source, member, level),
            trailing_comment_line: resource_member_trailing_comment_line(member),
        }),
    )
}

fn format_store_body(
    source: &str,
    indexes: &[crate::IndexDecl],
    comments: &[Comment],
    level: usize,
) -> String {
    format_body_lines(
        source,
        comments,
        indexes.iter().map(|index| FormattedBodyLine {
            span: index.span,
            text: format_index_decl(index, level),
            trailing_comment_line: TrailingCommentLine::Line(index.docs.len()),
        }),
    )
}

fn format_enum_body(
    source: &str,
    members: &[EnumMember],
    comments: &[Comment],
    level: usize,
) -> String {
    format_body_lines(
        source,
        comments,
        members.iter().map(|member| FormattedBodyLine {
            span: member.span,
            text: format_enum_member(source, member, level),
            trailing_comment_line: TrailingCommentLine::Line(member.docs.len()),
        }),
    )
}

fn resource_member_trailing_comment_line(member: &ResourceMember) -> TrailingCommentLine {
    match member {
        ResourceMember::Field(field) => TrailingCommentLine::Line(field.docs.len()),
        ResourceMember::Group(group) => TrailingCommentLine::Line(group.docs.len()),
    }
}

fn format_body_lines(
    source: &str,
    comments: &[Comment],
    items: impl Iterator<Item = FormattedBodyLine>,
) -> String {
    let mut lines = BlankAwareLines::new(source);
    let mut comments = comments.iter().peekable();
    let items: Vec<FormattedBodyLine> = items.collect();

    for (index, item) in items.iter().enumerate() {
        while let Some(comment) = comments.peek().copied() {
            if comment.placement == CommentPlacement::OwnLine
                && comment.span.start_byte < item.span.start_byte
            {
                lines.push(format_comment(comment), comment.span);
                comments.next();
            } else {
                break;
            }
        }

        let mut text = item.text.clone();
        let next_start = items
            .get(index + 1)
            .map_or(usize::MAX, |next| next.span.start_byte);
        if let Some(comment) = comments.peek().copied()
            && comment.placement == CommentPlacement::Trailing
            && comment.span.start_byte > item.span.start_byte
            && comment.span.start_byte < next_start
        {
            append_trailing_comment(&mut text, comment, item.trailing_comment_line);
            comments.next();
        }
        lines.push(text, item.span);
    }

    for comment in comments {
        lines.push(format_comment(comment), comment.span);
    }

    lines.finish()
}

/// Accumulates body lines, joining them with a single blank line wherever the
/// source held one or more blank lines immediately before a line, and with no
/// blank line otherwise. Two or more source blank lines collapse to one, and a
/// leading or trailing blank inside a body is dropped because the separator is
/// emitted only between two pushed lines.
///
/// The gap is measured from each line's own start byte rather than the previous
/// line's end byte: a block-bearing statement's span runs to its closing dedent,
/// which already swallows the very blank line we want to preserve, so the
/// reliable signal is whether the source immediately preceding the next line is
/// an empty line.
struct BlankAwareLines<'source> {
    source: &'source str,
    out: String,
    pushed_any: bool,
}

impl<'source> BlankAwareLines<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            source,
            out: String::new(),
            pushed_any: false,
        }
    }

    fn push(&mut self, text: String, span: crate::SourceSpan) {
        if self.pushed_any {
            self.out.push('\n');
            if blank_line_precedes(self.source, span.start_byte) {
                self.out.push('\n');
            }
        }
        self.pushed_any = true;
        self.out.push_str(&text);
    }

    fn finish(self) -> String {
        self.out
    }
}

/// Whether the line on which `start` sits is preceded by at least one blank
/// (whitespace-only) source line, treating the `;;` doc comments that attach to a
/// member as part of that member's visual group. A doc comment's text is rendered
/// inline with its member, so the member's own span begins at its header line; to
/// preserve the grouping blank line above a doc-commented member exactly as for a
/// plain one, the walk-back steps over any run of doc-comment lines first and then
/// looks for the blank above the comment.
fn blank_line_precedes(source: &str, start: usize) -> bool {
    let before = source.as_bytes();
    let mut line_start = start.min(before.len());
    loop {
        // Walk back over the current line's leading whitespace to its line break.
        let mut i = line_start;
        while i > 0 && before[i - 1] != b'\n' {
            if !before[i - 1].is_ascii_whitespace() {
                return false;
            }
            i -= 1;
        }
        if i == 0 {
            return false;
        }
        let above_end = i - 1; // index of the newline ending the line above
        let above_start = line_start_byte(before, above_end);
        if is_blank_line(&before[above_start..above_end]) {
            return true;
        }
        if is_doc_comment_line(&before[above_start..above_end]) {
            // Skip the doc comment and keep looking for the blank above the group.
            line_start = above_start;
            continue;
        }
        return false;
    }
}

/// Byte index of the start of the line containing `pos`.
fn line_start_byte(source: &[u8], pos: usize) -> usize {
    source[..pos]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |nl| nl + 1)
}

fn is_blank_line(line: &[u8]) -> bool {
    line.iter().all(u8::is_ascii_whitespace)
}

/// Whether `line` (its raw bytes, without the trailing newline) is a `;;` doc
/// comment: optional leading whitespace followed by two semicolons.
fn is_doc_comment_line(line: &[u8]) -> bool {
    let trimmed = line
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .map_or(&[][..], |start| &line[start..]);
    trimmed.starts_with(b";;")
}

struct FormattedBodyLine {
    span: crate::SourceSpan,
    text: String,
    trailing_comment_line: TrailingCommentLine,
}

#[derive(Clone, Copy)]
enum TrailingCommentLine {
    Last,
    Line(usize),
}

fn format_resource_member(source: &str, member: &ResourceMember, level: usize) -> String {
    let pad = INDENT.repeat(level);
    match member {
        ResourceMember::Field(field) => {
            let mut out = format_docs(&field.docs, level);
            let required = if field.required { "required " } else { "" };
            // A resource field always declares a type, so render it directly.
            out.push_str(&format!(
                "{pad}{required}{}{}: {}",
                field.name,
                format_key_params(&field.keys),
                field.ty,
            ));
            out
        }
        ResourceMember::Group(group) => {
            let mut out = format_docs(&group.docs, level);
            out.push_str(&format!(
                "{pad}{}{}",
                group.name,
                format_key_params(&group.keys)
            ));
            let body = format_resource_body(source, &group.members, &group.comments, level + 1);
            if !body.is_empty() {
                out.push('\n');
                out.push_str(&body);
            }
            out
        }
    }
}

fn format_index_decl(index: &crate::IndexDecl, level: usize) -> String {
    let pad = INDENT.repeat(level);
    let mut out = format_docs(&index.docs, level);
    let unique = if index.unique { " unique" } else { "" };
    out.push_str(&format!(
        "{pad}index {}({}){unique}",
        index.name,
        index.args.join(", ")
    ));
    out
}

fn format_function(source: &str, decl: &FunctionDecl) -> String {
    let mut out = format_docs(&decl.docs, 0);
    let visibility = if decl.public { "pub " } else { "" };
    out.push_str(&format!(
        "{visibility}fn {}({}){}",
        decl.name,
        format_params(&decl.params),
        format_type_annotation(&decl.return_type)
    ));
    append_body_block(&mut out, &format_block(source, &decl.body, 1));
    out
}

fn format_test(source: &str, decl: &crate::TestDecl) -> String {
    let mut out = format_docs(&decl.docs, 0);
    out.push_str(&format!("test {}", encode_string_literal(&decl.name)));
    append_body_block(&mut out, &format_block(source, &decl.body, 1));
    out
}

/// Render a parameter list. A list whose parameters carry documentation prints
/// one parameter per line so each doc sits on the line above its parameter, with
/// a trailing comma after the last so adding a parameter never edits the line
/// before it; any other list stays on the single `name: type, ...` line.
fn format_params(params: &[ParamDecl]) -> String {
    if params.iter().any(|param| !param.docs.is_empty()) {
        let mut out = String::from("\n");
        for param in params {
            out.push_str(&format_docs(&param.docs, 1));
            out.push_str(INDENT);
            out.push_str(&format_param(param));
            out.push_str(",\n");
        }
        return out;
    }
    params
        .iter()
        .map(format_param)
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_param(param: &ParamDecl) -> String {
    format!(
        "{}{}: {}",
        param.name,
        format_key_params(&param.keys),
        param.ty
    )
}

fn format_docs(docs: &[String], level: usize) -> String {
    let pad = INDENT.repeat(level);
    docs.iter()
        .map(|doc| {
            if doc.is_empty() {
                format!("{pad};;\n")
            } else {
                format!("{pad};; {doc}\n")
            }
        })
        .collect::<String>()
}

/// Format a block's statements one per line at `level`, joined without a
/// trailing newline.
///
/// Block comments are re-emitted so `parse -> format` round-trips them: own-line
/// comments appear on their own line, in source order between statements, each at
/// the block's canonical indent (see [`format_block_comment`]). A trailing comment
/// is appended to the line of the statement it sits on. Walking comments in step
/// with statements relies on both being in source order.
///
/// A comment in the middle of a value that spans several lines inside open
/// delimiters is not round-tripped: that is the one position the expression
/// parser does not carry through, a structural limitation inherited from the
/// parser rather than the formatter.
pub(crate) fn format_block(source: &str, block: &Block, level: usize) -> String {
    let mut lines = BlankAwareLines::new(source);
    let mut comments = block.comments.iter().peekable();

    for (i, statement) in block.statements.iter().enumerate() {
        let stmt_span = statement.span();
        while let Some(comment) = comments.peek().copied() {
            if comment.placement == CommentPlacement::OwnLine
                && comment.span.start_byte < stmt_span.start_byte
            {
                lines.push(format_block_comment(comment, level), comment.span);
                comments.next();
            } else {
                break;
            }
        }

        let next_start = block
            .statements
            .get(i + 1)
            .map_or(usize::MAX, |next| next.span().start_byte);
        let mut statement_comments = Vec::new();
        while let Some(comment) = comments.peek().copied() {
            if comment_belongs_to_statement(comment, stmt_span, next_start) {
                statement_comments.push(comment);
                comments.next();
            } else {
                break;
            }
        }
        let text = format_statement_with_comments(source, statement, &statement_comments, level);
        lines.push(text, stmt_span);
    }

    // Comments after the last statement, or an entirely statement-less block.
    for comment in comments {
        lines.push(format_block_comment(comment, level), comment.span);
    }

    lines.finish()
}

fn comment_belongs_to_statement(
    comment: &Comment,
    stmt_span: crate::SourceSpan,
    next_start: usize,
) -> bool {
    if comment.span.start_byte <= stmt_span.start_byte {
        return false;
    }
    match comment.placement {
        CommentPlacement::Trailing => comment.span.start_byte < next_start,
        CommentPlacement::OwnLine => comment.span.start_byte < stmt_span.end_byte,
    }
}

fn append_trailing_comment(text: &mut String, comment: &Comment, line: TrailingCommentLine) {
    let suffix = format_trailing_comment(comment);
    append_trailing_comment_text(text, &suffix, line);
}

fn format_trailing_comment(comment: &Comment) -> String {
    let mut suffix = String::new();
    suffix.push(' ');
    suffix.push_str(comment_marker_str(comment.marker));
    if !comment.text.is_empty() {
        suffix.push(' ');
        suffix.push_str(&comment.text);
    }
    suffix
}

fn append_trailing_comment_text(text: &mut String, suffix: &str, line: TrailingCommentLine) {
    match line {
        TrailingCommentLine::Last => text.push_str(suffix),
        TrailingCommentLine::Line(line) => {
            if let Some(index) = line_end_index(text, line) {
                text.insert_str(index, suffix);
            } else {
                text.push_str(suffix);
            }
        }
    }
}

fn line_end_index(text: &str, target_line: usize) -> Option<usize> {
    let mut line = 0usize;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            if line == target_line {
                return Some(index);
            }
            line += 1;
        }
    }
    None
}

/// Render an own-line comment, preserving its original column.
fn format_comment(comment: &Comment) -> String {
    let pad = " ".repeat(comment.span.column.saturating_sub(1) as usize);
    let marker = comment_marker_str(comment.marker);
    if comment.text.is_empty() {
        format!("{pad}{marker}")
    } else {
        format!("{pad}{marker} {}", comment.text)
    }
}

/// Render an own-line comment attached to a block at that block's canonical
/// indent. A comment belongs to exactly one block, so it renders at the same
/// indent as that block's statements or members regardless of its source column;
/// keeping a shallower source column would leave the comment misaligned from its
/// siblings when the block itself was written at a non-canonical indent, and a
/// reparse of that mismatched output would open a spurious deeper block and drop
/// the following statement (a non-idempotent, content-losing rewrite).
fn format_block_comment(comment: &Comment, level: usize) -> String {
    let block_column = (level * INDENT.len() + 1) as u32;
    let mut comment = comment.clone();
    comment.span.column = block_column;
    format_comment(&comment)
}

fn comment_marker_str(marker: CommentMarker) -> &'static str {
    match marker {
        CommentMarker::Line => ";",
        CommentMarker::Doc => ";;",
    }
}

fn append_first_trailing_comment(text: &mut String, comments: &[&Comment]) {
    if let Some(comment) = comments
        .iter()
        .copied()
        .find(|comment| comment.placement == CommentPlacement::Trailing)
    {
        append_trailing_comment(text, comment, TrailingCommentLine::Last);
    }
}

fn append_trailing_comment_between(
    text: &mut String,
    comments: &[&Comment],
    start_byte: usize,
    end_byte: usize,
) {
    if let Some(comment) = comments
        .iter()
        .copied()
        .find(|comment| trailing_comment_between(comment, start_byte, end_byte))
    {
        append_trailing_comment(text, comment, TrailingCommentLine::Last);
    }
}

fn trailing_comment_between(comment: &Comment, start_byte: usize, end_byte: usize) -> bool {
    comment.placement == CommentPlacement::Trailing
        && comment.span.start_byte > start_byte
        && comment.span.start_byte < end_byte
}

fn format_header_block(
    ctx: StatementFormatContext<'_, '_>,
    mut header: String,
    body: &Block,
) -> String {
    append_trailing_comment_between(
        &mut header,
        ctx.comments,
        ctx.start_byte,
        body.span.start_byte,
    );
    append_body_block(&mut header, &format_block(ctx.source, body, ctx.level + 1));
    header
}

/// Append a formatted body block under its header line. An empty body appends
/// nothing, leaving the header to stand alone: appending the usual newline would
/// leave a dangling blank line that the block-level blank accounting then counts
/// as a grouping blank and doubles, breaking formatter idempotence. Every
/// body-bearing construct — function and transform declarations, the compound
/// statements, `else`/`catch` clauses, and match arms — joins its body through
/// here, so an empty body is rendered uniformly.
fn append_body_block(out: &mut String, block: &str) {
    if !block.is_empty() {
        out.push('\n');
        out.push_str(block);
    }
}

#[derive(Clone, Copy)]
struct StatementFormatContext<'source, 'comments> {
    source: &'source str,
    comments: &'comments [&'comments Comment],
    start_byte: usize,
    level: usize,
}

fn format_statement_with_comments(
    source: &str,
    statement: &Statement,
    comments: &[&Comment],
    level: usize,
) -> String {
    let pad = INDENT.repeat(level);
    let mut text = match statement {
        Statement::Const {
            name, ty, value, ..
        } => format!(
            "{pad}const {name}{} = {}",
            format_type_annotation(ty),
            format_expression_at(value, level)
        ),
        Statement::Var {
            name,
            keys,
            ty,
            value,
            ..
        } => {
            let value = match value {
                Some(value) => format!(" = {}", format_expression_at(value, level)),
                None => String::new(),
            };
            format!(
                "{pad}var {name}{}{}{value}",
                format_key_params(keys),
                format_type_annotation(ty),
            )
        }
        Statement::Assign { target, value, .. } => format!(
            "{pad}{} = {}",
            format_expression_at(target, level),
            format_expression_at(value, level)
        ),
        Statement::CompoundAssign {
            target, op, value, ..
        } => format!(
            "{pad}{} {} {}",
            format_expression_at(target, level),
            op.symbol(),
            format_expression_at(value, level)
        ),
        Statement::Delete { path, .. } => {
            format!("{pad}delete {}", format_expression_at(path, level))
        }
        Statement::Return { value, .. } => match value {
            Some(value) => format!("{pad}return {}", format_expression_at(value, level)),
            None => format!("{pad}return"),
        },
        Statement::Break { .. } => format!("{pad}break"),
        Statement::Continue { .. } => format!("{pad}continue"),
        Statement::Throw { value, .. } => {
            format!("{pad}throw {}", format_expression_at(value, level))
        }
        Statement::Assert { value, .. } => {
            format!("{pad}assert {}", format_expression_at(value, level))
        }
        Statement::Expr { value, .. } => format!("{pad}{}", format_expression_at(value, level)),
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            span,
        } => {
            let ctx = StatementFormatContext {
                source,
                comments,
                start_byte: span.start_byte,
                level,
            };
            return format_if(ctx, condition, then_block, else_ifs, else_block.as_ref());
        }
        Statement::IfConst {
            name,
            ty,
            value,
            then_block,
            else_ifs,
            else_block,
            span,
        } => {
            let ctx = StatementFormatContext {
                source,
                comments,
                start_byte: span.start_byte,
                level,
            };
            return format_if_const(
                ctx,
                name,
                ty,
                value,
                then_block,
                else_ifs,
                else_block.as_ref(),
            );
        }
        Statement::While {
            condition,
            body,
            span,
        } => {
            let header = format!("{pad}while {}", format_expression_at(condition, level));
            let ctx = StatementFormatContext {
                source,
                comments,
                start_byte: span.start_byte,
                level,
            };
            return format_header_block(ctx, header, body);
        }
        Statement::For {
            binding,
            order,
            iterable,
            step,
            body,
            span,
        } => {
            let ctx = StatementFormatContext {
                source,
                comments,
                start_byte: span.start_byte,
                level,
            };
            return format_for(ctx, binding, *order, iterable, step.as_ref(), body);
        }
        Statement::Transaction { body, span } => {
            let ctx = StatementFormatContext {
                source,
                comments,
                start_byte: span.start_byte,
                level,
            };
            return format_header_block(ctx, format!("{pad}transaction"), body);
        }
        Statement::Try { body, catch, span } => {
            let ctx = StatementFormatContext {
                source,
                comments,
                start_byte: span.start_byte,
                level,
            };
            return format_try(ctx, body, catch.as_ref());
        }
        Statement::Match {
            scrutinee,
            arms,
            span,
        } => {
            let ctx = StatementFormatContext {
                source,
                comments,
                start_byte: span.start_byte,
                level,
            };
            return format_match(ctx, scrutinee, arms, *span);
        }
        Statement::Checked {
            bind,
            op,
            out_of_range,
            zero_divisor,
            span,
        } => {
            let ctx = StatementFormatContext {
                source,
                comments,
                start_byte: span.start_byte,
                level,
            };
            return format_checked(
                ctx,
                bind,
                op,
                out_of_range.as_ref(),
                zero_divisor.as_ref(),
                *span,
            );
        }
        // The formatter is invoked on parsed source and the CLI gates emission on
        // `!has_errors`, so this renders only in a best-effort `format_source` over
        // input that failed to parse. Echo the unstructured span verbatim rather
        // than dropping it, so no source is silently lost.
        Statement::Error { span } => {
            format!("{pad}{}", &source[span.start_byte..span.end_byte])
        }
    };
    append_first_trailing_comment(&mut text, comments);
    text
}

fn format_if(
    ctx: StatementFormatContext<'_, '_>,
    condition: &Expression,
    then_block: &Block,
    else_ifs: &[ElseIf],
    else_block: Option<&Block>,
) -> String {
    let pad = INDENT.repeat(ctx.level);
    let mut header = format!("{pad}if {}", format_expression_at(condition, ctx.level));
    append_trailing_comment_between(
        &mut header,
        ctx.comments,
        ctx.start_byte,
        then_block.span.start_byte,
    );
    let mut out = header;
    append_body_block(
        &mut out,
        &format_block(ctx.source, then_block, ctx.level + 1),
    );
    format_else_chain(
        ctx,
        &mut out,
        then_block.span.end_byte,
        else_ifs,
        else_block,
    );
    out
}

fn format_if_const(
    ctx: StatementFormatContext<'_, '_>,
    name: &str,
    ty: &Option<TypeExpr>,
    value: &Expression,
    then_block: &Block,
    else_ifs: &[ElseIf],
    else_block: Option<&Block>,
) -> String {
    let pad = INDENT.repeat(ctx.level);
    let mut header = format!(
        "{pad}if const {name}{} = {}",
        format_type_annotation(ty),
        format_expression_at(value, ctx.level)
    );
    append_trailing_comment_between(
        &mut header,
        ctx.comments,
        ctx.start_byte,
        then_block.span.start_byte,
    );
    let mut out = header;
    append_body_block(
        &mut out,
        &format_block(ctx.source, then_block, ctx.level + 1),
    );
    format_else_chain(
        ctx,
        &mut out,
        then_block.span.end_byte,
        else_ifs,
        else_block,
    );
    out
}

/// Append the `else if` chain and optional trailing `else` block shared by both
/// `if` forms. `previous_end` is the end byte of the preceding block, used to
/// place any trailing comment that sits before the next header.
fn format_else_chain(
    ctx: StatementFormatContext<'_, '_>,
    out: &mut String,
    mut previous_end: usize,
    else_ifs: &[ElseIf],
    else_block: Option<&Block>,
) {
    let pad = INDENT.repeat(ctx.level);
    for else_if in else_ifs {
        let mut header = format!(
            "{pad}else if {}",
            format_expression_at(&else_if.condition, ctx.level)
        );
        append_trailing_comment_between(
            &mut header,
            ctx.comments,
            previous_end,
            else_if.block.span.start_byte,
        );
        out.push('\n');
        out.push_str(&header);
        append_body_block(
            out,
            &format_block(ctx.source, &else_if.block, ctx.level + 1),
        );
        previous_end = else_if.block.span.end_byte;
    }
    if let Some(else_block) = else_block {
        let mut header = format!("{pad}else");
        append_trailing_comment_between(
            &mut header,
            ctx.comments,
            previous_end,
            else_block.span.start_byte,
        );
        out.push('\n');
        out.push_str(&header);
        append_body_block(out, &format_block(ctx.source, else_block, ctx.level + 1));
    }
}

fn format_for(
    ctx: StatementFormatContext<'_, '_>,
    binding: &ForBinding,
    order: LoopOrder,
    iterable: &Expression,
    step: Option<&Expression>,
    body: &Block,
) -> String {
    let pad = INDENT.repeat(ctx.level);
    let binding = binding
        .names
        .iter()
        .map(|name| name.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let order = match order {
        LoopOrder::Forward => "",
        LoopOrder::Reversed => "reversed ",
    };
    let step = match step {
        Some(step) => format!(" by {}", format_expression_at(step, ctx.level)),
        None => String::new(),
    };
    let header = format!(
        "{pad}for {binding} in {order}{}{step}",
        format_expression_at(iterable, ctx.level)
    );
    format_header_block(ctx, header, body)
}

fn format_try(
    ctx: StatementFormatContext<'_, '_>,
    body: &Block,
    catch: Option<&CatchClause>,
) -> String {
    let pad = INDENT.repeat(ctx.level);
    let mut header = format!("{pad}try");
    append_trailing_comment_between(
        &mut header,
        ctx.comments,
        ctx.start_byte,
        body.span.start_byte,
    );
    let mut out = header;
    append_body_block(&mut out, &format_block(ctx.source, body, ctx.level + 1));
    if let Some(catch) = catch {
        let mut header = format!(
            "{pad}catch {}{}",
            catch.name,
            format_type_annotation(&catch.ty)
        );
        append_trailing_comment_between(
            &mut header,
            ctx.comments,
            body.span.end_byte,
            catch.block.span.start_byte,
        );
        out.push('\n');
        out.push_str(&header);
        append_body_block(
            &mut out,
            &format_block(ctx.source, &catch.block, ctx.level + 1),
        );
    }
    out
}

fn format_match(
    ctx: StatementFormatContext<'_, '_>,
    scrutinee: &Expression,
    arms: &[MatchArm],
    span: crate::SourceSpan,
) -> String {
    let pad = INDENT.repeat(ctx.level);
    let arm_pad = INDENT.repeat(ctx.level + 1);
    let mut out = format!("{pad}match {}", format_expression_at(scrutinee, ctx.level));
    let first_arm_start = arms
        .first()
        .map_or(span.end_byte, |arm| arm.span.start_byte);
    append_trailing_comment_between(&mut out, ctx.comments, span.start_byte, first_arm_start);
    let arm_comments: Vec<&Comment> = ctx
        .comments
        .iter()
        .copied()
        .filter(|comment| !trailing_comment_between(comment, span.start_byte, first_arm_start))
        .collect();
    let mut comments = arm_comments.into_iter().peekable();
    for (i, arm) in arms.iter().enumerate() {
        let mut emitted_leading = false;
        while let Some(comment) = comments.peek().copied() {
            if comment.placement == CommentPlacement::OwnLine
                && comment.span.start_byte < arm.span.start_byte
            {
                out.push('\n');
                out.push_str(&format_block_comment(comment, ctx.level + 1));
                comments.next();
                emitted_leading = true;
            } else {
                break;
            }
        }
        // Preserve a source blank line that groups sibling arms, just as the body
        // emitter does for statements and members. A leading own-line comment
        // already carries its own blank, and the first arm sits directly under
        // the match header.
        if i > 0 && !emitted_leading && blank_line_precedes(ctx.source, arm.span.start_byte) {
            out.push('\n');
        }
        let mut header = format!("{arm_pad}{}", arm.path.join("::"));
        if let Some(comment) = comments.peek().copied()
            && trailing_comment_between(comment, arm.span.start_byte, arm.block.span.start_byte)
        {
            append_trailing_comment(&mut header, comment, TrailingCommentLine::Last);
            comments.next();
        }
        out.push('\n');
        out.push_str(&header);
        append_body_block(
            &mut out,
            &format_block(ctx.source, &arm.block, ctx.level + 2),
        );
    }
    for comment in comments {
        out.push('\n');
        out.push_str(&format_block_comment(comment, ctx.level + 1));
    }
    out
}

/// Format a checked-arithmetic form: the binding prefix and `checked <op>` on the
/// header line, then each present arm (`on out_of_range` before `on zero_divisor`,
/// regardless of source order) with its body. Idempotent for a comment-free parse.
fn format_checked(
    ctx: StatementFormatContext<'_, '_>,
    bind: &CheckedBind,
    op: &Expression,
    out_of_range: Option<&Block>,
    zero_divisor: Option<&Block>,
    span: crate::SourceSpan,
) -> String {
    let pad = INDENT.repeat(ctx.level);
    let prefix = match bind {
        CheckedBind::Const { name, ty } => {
            format!("const {name}{} = ", format_type_annotation(ty))
        }
        CheckedBind::Var { name, ty } => {
            format!("var {name}{} = ", format_type_annotation(ty))
        }
        CheckedBind::Return => "return ".to_string(),
    };
    let mut out = format!(
        "{pad}{prefix}checked {}",
        format_expression_at(op, ctx.level)
    );
    let first_arm_start = [out_of_range, zero_divisor]
        .into_iter()
        .flatten()
        .map(|block| block.span.start_byte)
        .min()
        .unwrap_or(span.end_byte);
    append_trailing_comment_between(&mut out, ctx.comments, span.start_byte, first_arm_start);
    let arm_pad = INDENT.repeat(ctx.level + 1);
    for (kind, block) in [
        ("out_of_range", out_of_range),
        ("zero_divisor", zero_divisor),
    ] {
        if let Some(block) = block {
            out.push('\n');
            out.push_str(&format!("{arm_pad}on {kind}"));
            append_body_block(&mut out, &format_block(ctx.source, block, ctx.level + 2));
        }
    }
    out
}

fn format_type_annotation(ty: &Option<TypeExpr>) -> String {
    match ty {
        Some(ty) => format!(": {ty}"),
        None => String::new(),
    }
}

fn format_key_params(keys: &[KeyParam]) -> String {
    if keys.is_empty() {
        return String::new();
    }
    let keys = keys
        .iter()
        .map(|key| format!("{}: {}", key.name, key.ty))
        .collect::<Vec<_>>()
        .join(", ");
    format!("({keys})")
}

/// Format a single expression as canonical Marrow source.
pub fn format_expression(expression: &Expression) -> String {
    format_expression_at(expression, 0)
}

fn format_expression_at(expression: &Expression, level: usize) -> String {
    format_expression_layout(expression, level, Layout::Block)
}

/// Where an expression is rendered. A statement body can host its own lines, so
/// a multiline call expands there. A string interpolation is lexed within a
/// single source line, so an embedded expression must stay on one physical
/// line; `Inline` forces nested calls into their single-line form regardless of
/// the multiline flag, so the formatted text re-parses to the same tree.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    Block,
    Inline,
}

fn format_expression_layout(expression: &Expression, level: usize, layout: Layout) -> String {
    match expression {
        Expression::Literal { text, .. } => text.clone(),
        Expression::Name { segments, .. } => segments.join("::"),
        Expression::SavedRoot { name, .. } => format!("^{name}"),
        Expression::Absent { .. } => "absent".to_string(),
        // Reachable only in a best-effort `format_source` over input that failed to
        // parse (emission is gated on `!has_errors`). The node carries no text, so
        // the unstructured fragment renders empty here; the surrounding statement
        // still formats.
        Expression::Error { .. } => String::new(),
        Expression::Call {
            callee,
            args,
            multiline,
            ..
        } => {
            let callee = format_child_at(callee, PREC_ATOM, level, layout);
            let rendered: Vec<String> = args
                .iter()
                .map(|arg| format_argument_at(arg, level + 1, layout))
                .collect();
            // A trailing comma requests the expanded form, and a wrapped argument
            // that itself spans lines forces it: the parser reads any call whose
            // parentheses span more than one line as multiline, so an inline
            // parent around a multiline child would not survive a re-parse. Inside
            // a string interpolation no newline can survive at all, so inline
            // layout overrides both and keeps the call on one line.
            let expand = matches!(layout, Layout::Block)
                && (*multiline || rendered.iter().any(|arg| arg.contains('\n')));
            if expand {
                let arg_pad = INDENT.repeat(level + 1);
                let close_pad = INDENT.repeat(level);
                let mut out = format!("{callee}(");
                for arg in &rendered {
                    out.push('\n');
                    out.push_str(&arg_pad);
                    out.push_str(arg);
                    out.push(',');
                }
                out.push('\n');
                out.push_str(&close_pad);
                out.push(')');
                out
            } else {
                format!("{callee}({})", rendered.join(", "))
            }
        }
        Expression::Field {
            base, name, quoted, ..
        } => format!(
            "{}.{}",
            format_child_at(base, PREC_ATOM, level, layout),
            field_segment(name, *quoted)
        ),
        Expression::OptionalField {
            base, name, quoted, ..
        } => format!(
            "{}?.{}",
            format_child_at(base, PREC_ATOM, level, layout),
            field_segment(name, *quoted)
        ),
        Expression::Unary { op, operand, .. } => {
            let operand = format_child_at(operand, PREC_UNARY, level, layout);
            match op {
                UnaryOp::Neg => format!("-{operand}"),
                UnaryOp::Not => format!("not {operand}"),
            }
        }
        Expression::Binary {
            op, left, right, ..
        } => format_binary_at(*op, left, right, level, layout),
        Expression::Range {
            start,
            end,
            inclusive_end,
            step,
            ..
        } => {
            let op = if *inclusive_end { "..=" } else { ".." };
            let mut out = format!(
                "{}{}{}",
                start
                    .as_ref()
                    .map(|start| format_child_at(start, PREC_ATOM, level, layout))
                    .unwrap_or_default(),
                op,
                end.as_ref()
                    .map(|end| format_child_at(end, PREC_ATOM, level, layout))
                    .unwrap_or_default()
            );
            if let Some(step) = step {
                out.push_str(" by ");
                out.push_str(&format_child_at(step, PREC_ATOM, level, layout));
            }
            out
        }
        Expression::Interpolation { parts, .. } => format_interpolation_at(parts, level),
    }
}

/// Render nothing when the value did not parse; the syntax error was already
/// reported at parse time.
fn format_opt_expression_at(expression: Option<&Expression>, level: usize) -> String {
    expression
        .map(|expression| format_expression_at(expression, level))
        .unwrap_or_default()
}

fn format_binary_at(
    op: BinaryOp,
    left: &Expression,
    right: &Expression,
    level: usize,
    layout: Layout,
) -> String {
    let precedence = binary_precedence(op);
    // The associative side keeps an equal-precedence operand bare; the other side
    // parenthesizes one. A non-associative operator parenthesizes either equal side.
    let (left_min, right_min) = match associativity(op) {
        Associativity::Left => (precedence, precedence + 1),
        Associativity::Right => (precedence + 1, precedence),
        Associativity::None => (precedence + 1, precedence + 1),
    };
    let left = format_child_at(left, left_min, level, layout);
    let right = format_child_at(right, right_min, level, layout);
    match op {
        BinaryOp::RangeExclusive => format!("{left}..{right}"),
        BinaryOp::RangeInclusive => format!("{left}..={right}"),
        _ => format!("{left} {} {right}", binary_symbol(op)),
    }
}

fn format_child_at(child: &Expression, min_precedence: u8, level: usize, layout: Layout) -> String {
    let rendered = format_expression_layout(child, level, layout);
    if precedence(child) < min_precedence {
        format!("({rendered})")
    } else {
        rendered
    }
}

/// Render a field-access segment, quoting a name that was written quoted (a
/// segment that is not a bare identifier, such as `"old-title"`).
fn field_segment(name: &str, quoted: bool) -> String {
    if quoted {
        format!("\"{name}\"")
    } else {
        name.to_string()
    }
}

fn format_argument_at(argument: &Argument, level: usize, layout: Layout) -> String {
    let mut out = String::new();
    if let Some(name) = &argument.name {
        out.push_str(name);
        out.push_str(": ");
    }
    out.push_str(&format_expression_layout(&argument.value, level, layout));
    out
}

fn format_interpolation_at(parts: &[InterpolationPart], level: usize) -> String {
    let mut out = String::from("$\"");
    for part in parts {
        match part {
            // Text keeps `{{`/`}}` escaped exactly as written.
            InterpolationPart::Text { text, .. } => out.push_str(text),
            InterpolationPart::Expr(expression) => {
                out.push('{');
                out.push_str(&format_expression_layout(expression, level, Layout::Inline));
                out.push('}');
            }
        }
    }
    out.push('"');
    out
}

fn precedence(expression: &Expression) -> u8 {
    match expression {
        Expression::Binary { op, .. } => binary_precedence(*op),
        Expression::Unary { .. } => PREC_UNARY,
        _ => PREC_ATOM,
    }
}

fn binary_precedence(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Or => 1,
        BinaryOp::And => 2,
        BinaryOp::Is => 3,
        BinaryOp::Equal | BinaryOp::NotEqual => 4,
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => 5,
        BinaryOp::RangeExclusive | BinaryOp::RangeInclusive => 6,
        BinaryOp::Coalesce => 7,
        BinaryOp::Add | BinaryOp::Subtract => 9,
        BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Remainder => 10,
    }
}

enum Associativity {
    Left,
    Right,
    None,
}

/// `??` is right-associative (`a ?? b ?? c` is `a ?? (b ?? c)`). Equality, `is`,
/// comparison, and range are non-associative per the grammar and need parentheses
/// on either equal-precedence side; every other binary operator is left-associative.
fn associativity(op: BinaryOp) -> Associativity {
    match op {
        BinaryOp::Coalesce => Associativity::Right,
        BinaryOp::Is
        | BinaryOp::Equal
        | BinaryOp::NotEqual
        | BinaryOp::Less
        | BinaryOp::LessEqual
        | BinaryOp::Greater
        | BinaryOp::GreaterEqual
        | BinaryOp::RangeExclusive
        | BinaryOp::RangeInclusive => Associativity::None,
        _ => Associativity::Left,
    }
}

fn binary_symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Remainder => "%",
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Less => "<",
        BinaryOp::LessEqual => "<=",
        BinaryOp::Greater => ">",
        BinaryOp::GreaterEqual => ">=",
        BinaryOp::Equal => "==",
        BinaryOp::NotEqual => "!=",
        BinaryOp::Coalesce => "??",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
        BinaryOp::Is => "is",
        // Ranges are emitted unspaced, so these symbols are only for exhaustiveness.
        BinaryOp::RangeExclusive => "..",
        BinaryOp::RangeInclusive => "..=",
    }
}

#[cfg(test)]
mod tests {
    use crate::{Expression, parse_expression};

    fn parse(source: &str) -> Expression {
        let (expression, diagnostics) = parse_expression(source);
        assert!(
            diagnostics.is_empty(),
            "unexpected diagnostics: {diagnostics:?}"
        );
        expression.expect("expression")
    }

    /// A multiplicative operand under a unary must stay parenthesized: unary binds
    /// tighter than `*`/`/`/`%`, so dropping the parentheses would re-associate the
    /// negation onto the left factor and change the value.
    #[test]
    fn unary_over_multiplicative_keeps_parentheses() {
        for source in ["-(a * b)", "-(a / b)", "-(a % b)"] {
            let formatted = super::format_expression(&parse(source));
            assert_eq!(formatted, source);
        }
    }

    /// Parsing the formatted output yields a node of the same top-level shape:
    /// the negation stays on the whole product, not just the left factor.
    #[test]
    fn unary_over_multiplicative_round_trips_to_unary_root() {
        let reparsed = parse(&super::format_expression(&parse("-(a * b)")));
        match reparsed {
            Expression::Unary { operand, .. } => assert!(matches!(
                *operand,
                Expression::Binary {
                    op: crate::BinaryOp::Multiply,
                    ..
                }
            )),
            other => panic!("expected Unary over Multiply, got {other:?}"),
        }
    }
}
