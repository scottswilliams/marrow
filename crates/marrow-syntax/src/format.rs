//! Render the syntax tree back to canonical Marrow `.mw` source.
//!
//! Canonical style: binary operators are spaced
//! (`a + b`), ranges are not (`1..10`), unary is `-x` / `not x`, calls are
//! `f(a, b)`, dotted fields and `::` name paths have no surrounding spaces.
//! The syntax tree does not record parentheses, so the formatter re-inserts
//! the minimum needed to preserve operator precedence and associativity.

use crate::{
    ArgMode, Argument, BinaryOp, Block, Comment, CommentMarker, CommentPlacement, ConstDecl,
    Declaration, EnumDecl, EnumMember, EvolveDecl, EvolveStep, Expression, FunctionDecl,
    InterpolationPart, KeyParam, ParamDecl, ParamMode, ResourceDecl, ResourceMember, Statement,
    StoreDecl, TypeRef, UnaryOp,
};

/// Precedence of an expression, tightest-binding last. Used to decide where
/// parentheses are required. Atoms (literals, names, calls, fields, …) bind
/// tightest; `or` binds loosest.
const PREC_ATOM: u8 = 11;
const PREC_UNARY: u8 = 10;

/// One indentation level in canonical Marrow source.
const INDENT: &str = "    ";

/// Format a whole `.mw` source file as canonical Marrow. The module
/// declaration, the `use` block, and each top-level declaration are separated
/// by a single blank line; the result ends with a newline.
///
/// Formatting normalizes layout (indentation, blank lines, doc-comment
/// spacing), so the output is not byte-identical to arbitrary input but is a
/// stable fixed point: `format_source(format_source(s)) == format_source(s)`.
/// Line comments inside function bodies are retained as block trivia and
/// re-emitted (see `format_block`). A comment in the middle of a value that
/// spans several lines inside open delimiters is the one position the expression
/// parser does not carry through.
pub fn format_source(source: &str) -> String {
    let parsed = crate::parse_source(source);
    let file = &parsed.file;
    let mut sections: Vec<FormatSection> = Vec::new();

    if let Some(module) = &file.module {
        sections.push(FormatSection::item(
            module.span,
            format!("module {}", module.name),
        ));
    }
    for comment in &file.comments {
        sections.push(FormatSection::comment(comment));
    }
    for use_decl in &file.uses {
        sections.push(FormatSection::use_decl(
            use_decl.span,
            format!("use {}", use_decl.name),
        ));
    }
    for declaration in &file.declarations {
        sections.push(FormatSection::item(
            declaration_span(declaration),
            format_declaration(source, declaration),
        ));
    }

    if sections.is_empty() {
        return String::new();
    }
    sections.sort_by_key(|section| section.span.start_byte);
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

struct FormatSection {
    span: crate::SourceSpan,
    text: String,
    kind: FormatSectionKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FormatSectionKind {
    Comment,
    Use,
    Item,
}

impl FormatSection {
    fn item(span: crate::SourceSpan, text: String) -> Self {
        Self {
            span,
            text,
            kind: FormatSectionKind::Item,
        }
    }

    fn use_decl(span: crate::SourceSpan, text: String) -> Self {
        Self {
            span,
            text,
            kind: FormatSectionKind::Use,
        }
    }

    fn comment(comment: &Comment) -> Self {
        Self {
            span: comment.span,
            text: format_comment(comment),
            kind: FormatSectionKind::Comment,
        }
    }
}

fn section_separator(prev: &FormatSection, next: &FormatSection) -> &'static str {
    if prev.kind == FormatSectionKind::Use
        && next.kind == FormatSectionKind::Use
        && next.span.line == prev.span.line + 1
    {
        return "\n";
    }
    if prev.kind == FormatSectionKind::Comment && next.span.line == prev.span.line + 1 {
        return "\n";
    }
    "\n\n"
}

fn declaration_span(declaration: &Declaration) -> crate::SourceSpan {
    match declaration {
        Declaration::Const(decl) => decl.span,
        Declaration::Resource(decl) => decl.span,
        Declaration::Store(decl) => decl.span,
        Declaration::Function(decl) => decl.span,
        Declaration::Enum(decl) => decl.span,
        Declaration::Evolve(decl) => decl.span,
    }
}

/// Render one top-level declaration as canonical Marrow source, normalized to a
/// fixed layout. A drift anchor reads this as the declaration's stable shape: it
/// changes when the declaration changes and ignores the surrounding source layout,
/// so a digest built from it binds the whole declared surface with no field-by-field
/// enumeration gap. `source` supplies the original text any statement body the
/// declaration carries renders from.
pub fn format_declaration_normalized(source: &str, declaration: &Declaration) -> String {
    format_declaration(source, declaration)
}

/// Format a top-level declaration (const, resource, or function) as canonical
/// Marrow source, including its documentation comments. The returned text has
/// no trailing newline.
fn format_declaration(source: &str, declaration: &Declaration) -> String {
    match declaration {
        Declaration::Const(decl) => format_const(decl),
        Declaration::Resource(decl) => format_resource(decl),
        Declaration::Store(decl) => format_store(decl),
        Declaration::Function(decl) => format_function(source, decl),
        Declaration::Enum(decl) => format_enum(decl),
        Declaration::Evolve(decl) => format_evolve(source, decl),
    }
}

/// Format an `evolve` block: the bare header, then one step per line at one indent
/// level, matching the resource and store body shape. A `transform` step prints
/// its statement body one level deeper.
fn format_evolve(source: &str, decl: &EvolveDecl) -> String {
    let mut out = String::from("evolve");
    let step_pad = INDENT;
    for step in &decl.steps {
        out.push('\n');
        match step {
            EvolveStep::Rename { from, to, .. } => {
                out.push_str(&format!(
                    "{step_pad}rename {} -> {}",
                    format_expression(from),
                    format_expression(to)
                ));
            }
            EvolveStep::Default { target, value, .. } => {
                out.push_str(&format!(
                    "{step_pad}default {} = {}",
                    format_expression(target),
                    format_expression(value)
                ));
            }
            EvolveStep::Retire { target, .. } => {
                out.push_str(&format!("{step_pad}retire {}", format_expression(target)));
            }
            EvolveStep::Transform { target, body, .. } => {
                out.push_str(&format!(
                    "{step_pad}transform {}",
                    format_expression(target)
                ));
                let body = format_block(source, body, 2);
                if !body.is_empty() {
                    out.push('\n');
                    out.push_str(&body);
                }
            }
        }
    }
    out
}

fn format_const(decl: &ConstDecl) -> String {
    let mut out = format_docs(&decl.docs, 0);
    out.push_str(&format!(
        "const {}{} = {}",
        decl.name,
        format_type_annotation(&decl.ty),
        format_opt_expression(decl.value.as_ref())
    ));
    out
}

fn format_resource(decl: &ResourceDecl) -> String {
    let mut out = format_docs(&decl.docs, 0);
    out.push_str("resource ");
    out.push_str(&decl.name);
    let body = format_resource_body(&decl.members, &decl.comments, 1);
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    out
}

fn format_store(decl: &StoreDecl) -> String {
    let mut out = format_docs(&decl.docs, 0);
    out.push_str(&format!(
        "store ^{}{}: {}",
        decl.root.root,
        format_key_params(&decl.root.keys),
        decl.resource
    ));
    let body = format_store_body(&decl.indexes, &decl.comments, 1);
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    out
}

fn format_enum(decl: &EnumDecl) -> String {
    let mut out = format_docs(&decl.docs, 0);
    let visibility = if decl.public { "pub " } else { "" };
    out.push_str(&format!("{visibility}enum {}", decl.name));
    let body = format_enum_body(&decl.members, &decl.comments, 1);
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    out
}

/// Render one enum member and its nested members, each on its own line at the
/// given indent depth. A `category` member leads with the `category` word.
fn format_enum_member(member: &EnumMember, level: usize) -> String {
    let mut out = format_docs(&member.docs, level);
    let category = if member.category { "category " } else { "" };
    out.push_str(&format!(
        "{}{category}{}",
        INDENT.repeat(level),
        member.name
    ));
    let body = format_enum_body(&member.members, &member.comments, level + 1);
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    out
}

fn format_resource_body(members: &[ResourceMember], comments: &[Comment], level: usize) -> String {
    let mut lines = Vec::new();
    for comment in comments {
        lines.push(FormattedBodyLine {
            span: comment.span,
            text: format_comment(comment),
        });
    }
    for member in members {
        lines.push(FormattedBodyLine {
            span: resource_member_span(member),
            text: format_resource_member(member, level),
        });
    }
    lines.sort_by_key(|line| line.span.start_byte);
    lines
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_store_body(indexes: &[crate::IndexDecl], comments: &[Comment], level: usize) -> String {
    let mut lines = Vec::new();
    for comment in comments {
        lines.push(FormattedBodyLine {
            span: comment.span,
            text: format_comment(comment),
        });
    }
    for index in indexes {
        lines.push(FormattedBodyLine {
            span: index.span,
            text: format_index_decl(index, level),
        });
    }
    lines.sort_by_key(|line| line.span.start_byte);
    lines
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_enum_body(members: &[EnumMember], comments: &[Comment], level: usize) -> String {
    let mut lines = Vec::new();
    for comment in comments {
        lines.push(FormattedBodyLine {
            span: comment.span,
            text: format_comment(comment),
        });
    }
    for member in members {
        lines.push(FormattedBodyLine {
            span: member.span,
            text: format_enum_member(member, level),
        });
    }
    lines.sort_by_key(|line| line.span.start_byte);
    lines
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n")
}

struct FormattedBodyLine {
    span: crate::SourceSpan,
    text: String,
}

fn resource_member_span(member: &ResourceMember) -> crate::SourceSpan {
    match member {
        ResourceMember::Field(field) => field.span,
        ResourceMember::Group(group) => group.span,
    }
}

fn format_resource_member(member: &ResourceMember, level: usize) -> String {
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
            let body = format_resource_body(&group.members, &group.comments, level + 1);
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
        format_return_type(&decl.return_type)
    ));
    let body = format_block(source, &decl.body, 1);
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
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
    let mode = match param.mode {
        Some(ParamMode::Out) => "out ",
        Some(ParamMode::InOut) => "inout ",
        None => "",
    };
    format!("{mode}{}: {}", param.name, param.ty)
}

fn format_return_type(return_type: &Option<TypeRef>) -> String {
    match return_type {
        Some(ty) => format!(": {ty}"),
        None => String::new(),
    }
}

/// Render documentation comments as `;; ...` lines at `level`, each ending in a
/// newline so the declaration or member follows on its own line.
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

/// Format a block's statements at the given indentation level, one statement
/// per line, joined by newlines (no trailing newline). Nested blocks indent one
/// level deeper.
///
/// Line comments retained on the block are re-emitted so `parse -> format`
/// round-trips them: own-line comments appear on their own line at the block
/// indent, in source order between statements; a trailing comment is appended to
/// the line of the statement it sits on.
pub(crate) fn format_block(source: &str, block: &Block, level: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    // Comments are kept in source order; walk them in step with the statements.
    let mut comments = block.comments.iter().peekable();

    for (i, statement) in block.statements.iter().enumerate() {
        let stmt_span = statement.span();
        // Own-line comments that precede this statement.
        while let Some(comment) = comments.peek() {
            if comment.placement == CommentPlacement::OwnLine
                && comment.span.start_byte < stmt_span.start_byte
            {
                lines.push(format_comment(comments.next().expect("peeked")));
            } else {
                break;
            }
        }

        let mut text = format_statement(source, statement, level);
        // A trailing comment sits on this statement's line, so it appears after
        // the statement starts and before the next statement does. Append it to
        // the statement's last line.
        let next_start = block
            .statements
            .get(i + 1)
            .map_or(usize::MAX, |next| next.span().start_byte);
        if let Some(comment) = comments.peek()
            && comment.placement == CommentPlacement::Trailing
            && comment.span.start_byte > stmt_span.start_byte
            && comment.span.start_byte < next_start
        {
            text.push_str(&format!(" ; {}", comments.next().expect("peeked").text));
        }
        lines.push(text);
    }

    // Any remaining comments dangle after the last statement (or fill an
    // otherwise statement-less block); emit them on their own lines.
    for comment in comments {
        lines.push(format_comment(comment));
    }

    lines.join("\n")
}

/// Render an own-line `;` comment as a `; text` line, preserving the comment's
/// original indentation. Comments are indentation-exempt, so keeping the
/// author's column round-trips an outdented comment exactly rather than
/// re-indenting it to whichever block the lexer structurally attached it to.
fn format_comment(comment: &Comment) -> String {
    let pad = " ".repeat(comment.span.column.saturating_sub(1) as usize);
    let marker = match comment.marker {
        CommentMarker::Line => ";",
        CommentMarker::Doc => ";;",
    };
    if comment.text.is_empty() {
        format!("{pad}{marker}")
    } else {
        format!("{pad}{marker} {}", comment.text)
    }
}

/// Format one statement (and any nested blocks) at `level`. The returned text
/// has no trailing newline.
pub(crate) fn format_statement(source: &str, statement: &Statement, level: usize) -> String {
    let pad = INDENT.repeat(level);
    match statement {
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
        Statement::Delete { path, .. } => {
            format!("{pad}delete {}", format_expression_at(path, level))
        }
        Statement::Merge { target, value, .. } => format!(
            "{pad}merge {} = {}",
            format_expression_at(target, level),
            format_expression_at(value, level)
        ),
        Statement::Return { value, .. } => match value {
            Some(value) => format!("{pad}return {}", format_expression_at(value, level)),
            None => format!("{pad}return"),
        },
        Statement::Break { label, .. } => format!("{pad}break{}", format_label_suffix(label)),
        Statement::Continue { label, .. } => format!("{pad}continue{}", format_label_suffix(label)),
        Statement::Throw { value, .. } => {
            format!("{pad}throw {}", format_expression_at(value, level))
        }
        Statement::Expr { value, .. } => format!("{pad}{}", format_expression_at(value, level)),
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            let mut out = format!(
                "{pad}if {}\n{}",
                format_opt_expression_at(condition.as_ref(), level),
                format_block(source, then_block, level + 1)
            );
            for else_if in else_ifs {
                out.push_str(&format!(
                    "\n{pad}else if {}\n{}",
                    format_opt_expression_at(else_if.condition.as_ref(), level),
                    format_block(source, &else_if.block, level + 1)
                ));
            }
            if let Some(else_block) = else_block {
                out.push_str(&format!(
                    "\n{pad}else\n{}",
                    format_block(source, else_block, level + 1)
                ));
            }
            out
        }
        Statement::While {
            label,
            condition,
            body,
            ..
        } => format!(
            "{pad}{}while {}\n{}",
            format_label_prefix(label),
            format_opt_expression_at(condition.as_ref(), level),
            format_block(source, body, level + 1)
        ),
        Statement::For {
            label,
            binding,
            iterable,
            step,
            body,
            ..
        } => {
            let binding = match &binding.second {
                Some(second) => format!("{}, {second}", binding.first),
                None => binding.first.clone(),
            };
            let step = match step {
                Some(step) => format!(" by {}", format_expression_at(step, level)),
                None => String::new(),
            };
            format!(
                "{pad}{}for {binding} in {}{step}\n{}",
                format_label_prefix(label),
                format_expression_at(iterable, level),
                format_block(source, body, level + 1)
            )
        }
        Statement::Transaction { body, .. } => {
            format!(
                "{pad}transaction\n{}",
                format_block(source, body, level + 1)
            )
        }
        Statement::Lock { path, body, .. } => format!(
            "{pad}lock {}\n{}",
            format_opt_expression_at(path.as_ref(), level),
            format_block(source, body, level + 1)
        ),
        Statement::Try {
            body,
            catch,
            finally,
            ..
        } => {
            let mut out = format!("{pad}try\n{}", format_block(source, body, level + 1));
            if let Some(catch) = catch {
                out.push_str(&format!(
                    "\n{pad}catch {}{}\n{}",
                    catch.name,
                    format_type_annotation(&catch.ty),
                    format_block(source, &catch.block, level + 1)
                ));
            }
            if let Some(finally) = finally {
                out.push_str(&format!(
                    "\n{pad}finally\n{}",
                    format_block(source, finally, level + 1)
                ));
            }
            out
        }
        Statement::Match {
            scrutinee, arms, ..
        } => {
            let arm_pad = INDENT.repeat(level + 1);
            let mut out = format!(
                "{pad}match {}",
                format_opt_expression_at(scrutinee.as_ref(), level)
            );
            for arm in arms {
                out.push_str(&format!(
                    "\n{arm_pad}{}\n{}",
                    arm.path.join("::"),
                    format_block(source, &arm.block, level + 2)
                ));
            }
            out
        }
    }
}

fn format_type_annotation(ty: &Option<TypeRef>) -> String {
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

fn format_label_prefix(label: &Option<String>) -> String {
    match label {
        Some(label) => format!("{label}: "),
        None => String::new(),
    }
}

fn format_label_suffix(label: &Option<String>) -> String {
    match label {
        Some(label) => format!(" {label}"),
        None => String::new(),
    }
}

/// Format a single expression as canonical Marrow source.
pub fn format_expression(expression: &Expression) -> String {
    format_expression_at(expression, 0)
}

fn format_expression_at(expression: &Expression, level: usize) -> String {
    match expression {
        Expression::Literal { text, .. } => text.clone(),
        Expression::Name { segments, .. } => segments.join("::"),
        Expression::SavedRoot { name, .. } => format!("^{name}"),
        Expression::Call {
            callee,
            args,
            multiline,
            ..
        } => {
            let callee = format_child_at(callee, PREC_ATOM, level);
            if *multiline {
                let arg_pad = INDENT.repeat(level + 1);
                let close_pad = INDENT.repeat(level);
                let mut out = format!("{callee}(");
                for arg in args {
                    out.push('\n');
                    out.push_str(&arg_pad);
                    out.push_str(&format_argument_at(arg, level + 1));
                    out.push(',');
                }
                out.push('\n');
                out.push_str(&close_pad);
                out.push(')');
                out
            } else {
                let args = args
                    .iter()
                    .map(format_argument)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{callee}({args})")
            }
        }
        Expression::Field {
            base, name, quoted, ..
        } => {
            let segment = if *quoted {
                format!("\"{name}\"")
            } else {
                name.clone()
            };
            format!("{}.{}", format_child_at(base, PREC_ATOM, level), segment)
        }
        Expression::OptionalField {
            base, name, quoted, ..
        } => {
            let segment = if *quoted {
                format!("\"{name}\"")
            } else {
                name.clone()
            };
            format!("{}?.{}", format_child_at(base, PREC_ATOM, level), segment)
        }
        Expression::Unary { op, operand, .. } => {
            let operand = format_child_at(operand, PREC_UNARY, level);
            match op {
                UnaryOp::Neg => format!("-{operand}"),
                UnaryOp::Not => format!("not {operand}"),
            }
        }
        Expression::Binary {
            op, left, right, ..
        } => format_binary_at(*op, left, right, level),
        Expression::Interpolation { parts, .. } => format_interpolation_at(parts, level),
    }
}

/// Format an optional value-position expression, rendering nothing when the
/// value did not parse (a syntax error was already reported at parse time).
fn format_opt_expression(expression: Option<&Expression>) -> String {
    expression.map(format_expression).unwrap_or_default()
}

fn format_opt_expression_at(expression: Option<&Expression>, level: usize) -> String {
    expression
        .map(|expression| format_expression_at(expression, level))
        .unwrap_or_default()
}

fn format_binary_at(op: BinaryOp, left: &Expression, right: &Expression, level: usize) -> String {
    let precedence = binary_precedence(op);
    // Left-associative operators keep an equal-precedence left operand without
    // parentheses; the right operand needs them. Non-associative operators
    // (equality, comparison, range) require parentheses on either equal side.
    let (left_min, right_min) = if is_left_associative(op) {
        (precedence, precedence + 1)
    } else {
        (precedence + 1, precedence + 1)
    };
    let left = format_child_at(left, left_min, level);
    let right = format_child_at(right, right_min, level);
    match op {
        BinaryOp::RangeExclusive => format!("{left}..{right}"),
        BinaryOp::RangeInclusive => format!("{left}..={right}"),
        _ => format!("{left} {} {right}", binary_symbol(op)),
    }
}

fn format_child_at(child: &Expression, min_precedence: u8, level: usize) -> String {
    let rendered = format_expression_at(child, level);
    if precedence(child) < min_precedence {
        format!("({rendered})")
    } else {
        rendered
    }
}

fn format_argument(argument: &Argument) -> String {
    format_argument_at(argument, 0)
}

fn format_argument_at(argument: &Argument, level: usize) -> String {
    let mut out = String::new();
    match argument.mode {
        Some(ArgMode::Out) => out.push_str("out "),
        Some(ArgMode::InOut) => out.push_str("inout "),
        None => {}
    }
    if let Some(name) = &argument.name {
        out.push_str(name);
        out.push_str(": ");
    }
    out.push_str(&format_expression_at(&argument.value, level));
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
                out.push_str(&format_expression_at(expression, level));
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
        BinaryOp::Coalesce => 5,
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => 6,
        BinaryOp::RangeExclusive | BinaryOp::RangeInclusive => 7,
        BinaryOp::Concat => 8,
        BinaryOp::Add | BinaryOp::Subtract => 9,
        BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Remainder => 10,
    }
}

/// `is`, equality, `??`, comparison, and range are non-associative per the
/// grammar; all other binary operators are left-associative.
fn is_left_associative(op: BinaryOp) -> bool {
    !matches!(
        op,
        BinaryOp::Is
            | BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Coalesce
            | BinaryOp::Less
            | BinaryOp::LessEqual
            | BinaryOp::Greater
            | BinaryOp::GreaterEqual
            | BinaryOp::RangeExclusive
            | BinaryOp::RangeInclusive
    )
}

fn binary_symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Remainder => "%",
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Concat => "_",
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
        // Ranges are emitted without spaces by `format_binary`.
        BinaryOp::RangeExclusive => "..",
        BinaryOp::RangeInclusive => "..=",
    }
}
