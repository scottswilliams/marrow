//! Render the syntax tree back to canonical Marrow `.mw` source.
//!
//! Canonical style: binary operators are spaced
//! (`a + b`), ranges are not (`1..10`), unary is `-x` / `not x`, calls are
//! `f(a, b)`, dotted fields and `::` name paths have no surrounding spaces.
//! The syntax tree does not record parentheses, so the formatter re-inserts
//! the minimum needed to preserve operator precedence and associativity.

use crate::{
    ArgMode, Argument, BinaryOp, Block, Comment, CommentPlacement, ConstDecl, Declaration,
    Expression, FunctionDecl, InterpolationPart, KeyParam, ParamDecl, ParamMode, ResourceDecl,
    ResourceMember, SavedRoot, Statement, TypeRef, UnaryOp,
};

/// Precedence of an expression, tightest-binding last. Used to decide where
/// parentheses are required. Atoms (literals, names, calls, fields, …) bind
/// tightest; `or` binds loosest.
const PREC_ATOM: u8 = 10;
const PREC_UNARY: u8 = 9;

/// One indentation level in canonical Marrow source.
const INDENT: &str = "    ";

/// Format a whole `.mw` source file as canonical Marrow. The module
/// declaration, the `use` block, and each top-level declaration are separated
/// by a single blank line; the result ends with a newline.
///
/// Formatting normalizes layout (indentation, blank lines, doc-comment
/// spacing), so the output is not byte-identical to arbitrary input but is a
/// stable fixed point: `format_source(format_source(s)) == format_source(s)`.
/// Ordinary `;` comments inside function bodies are retained as block trivia
/// and re-emitted (see `format_block`). A comment in the middle of a value that
/// spans several lines inside open delimiters is the one position the
/// expression parser does not carry through.
pub fn format_source(source: &str) -> String {
    let parsed = crate::parse_source(source);
    let file = &parsed.file;
    let mut sections: Vec<String> = Vec::new();

    if let Some(module) = &file.module {
        sections.push(format!("module {}", module.name));
    }
    if !file.uses.is_empty() {
        sections.push(
            file.uses
                .iter()
                .map(|use_decl| format!("use {}", use_decl.name))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    for declaration in &file.declarations {
        sections.push(format_declaration(source, declaration));
    }

    if sections.is_empty() {
        return String::new();
    }
    let mut out = sections.join("\n\n");
    out.push('\n');
    out
}

/// Format a top-level declaration (const, resource, or function) as canonical
/// Marrow source, including its documentation comments. The returned text has
/// no trailing newline.
fn format_declaration(source: &str, declaration: &Declaration) -> String {
    match declaration {
        Declaration::Const(decl) => format_const(decl),
        Declaration::Resource(decl) => format_resource(decl),
        Declaration::Function(decl) => format_function(source, decl),
    }
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
    if let Some(store) = &decl.store {
        out.push_str(&format_saved_root(store));
    }
    for member in &decl.members {
        out.push('\n');
        out.push_str(&format_resource_member(member, 1));
    }
    out
}

fn format_saved_root(store: &SavedRoot) -> String {
    format!(" at ^{}{}", store.root, format_key_params(&store.keys))
}

fn format_resource_member(member: &ResourceMember, level: usize) -> String {
    let pad = INDENT.repeat(level);
    match member {
        ResourceMember::Field(field) => {
            let mut out = format_member_meta(&field.docs, &field.stable_id, level);
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
            let mut out = format_member_meta(&group.docs, &group.stable_id, level);
            out.push_str(&format!(
                "{pad}{}{}",
                group.name,
                format_key_params(&group.keys)
            ));
            for child in &group.members {
                out.push('\n');
                out.push_str(&format_resource_member(child, level + 1));
            }
            out
        }
        ResourceMember::Index(index) => {
            let mut out = format_member_meta(&index.docs, &index.stable_id, level);
            let unique = if index.unique { " unique" } else { "" };
            out.push_str(&format!(
                "{pad}index {}({}){unique}",
                index.name,
                index.args.join(", ")
            ));
            out
        }
    }
}

/// Render a member's documentation comments and `@id(...)` metadata as the
/// lines preceding it, each indented to `level`.
fn format_member_meta(docs: &[String], stable_id: &Option<String>, level: usize) -> String {
    let pad = INDENT.repeat(level);
    let mut out = format_docs(docs, level);
    if let Some(stable_id) = stable_id {
        out.push_str(&format!("{pad}@id(\"{stable_id}\")\n"));
    }
    out
}

fn format_function(source: &str, decl: &FunctionDecl) -> String {
    let mut out = format_docs(&decl.docs, 0);
    let visibility = if decl.public { "pub " } else { "" };
    let params = decl
        .params
        .iter()
        .map(format_param)
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!(
        "{visibility}fn {}({params}){}",
        decl.name,
        format_return_type(&decl.return_type)
    ));
    let body = format_block(source, &decl.body, 1);
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    out
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
        .map(|doc| format!("{pad};; {doc}\n"))
        .collect::<String>()
}

/// Format a block's statements at the given indentation level, one statement
/// per line, joined by newlines (no trailing newline). Nested blocks indent one
/// level deeper.
///
/// Ordinary `;` comments retained on the block are re-emitted so `parse ->
/// format` round-trips them: own-line comments appear on their own line at the
/// block indent, in source order between statements; a trailing comment is
/// appended to the line of the statement it sits on.
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
    if comment.text.is_empty() {
        format!("{pad};")
    } else {
        format!("{pad}; {}", comment.text)
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
            format_expression(value)
        ),
        Statement::Var {
            name,
            keys,
            ty,
            value,
            ..
        } => {
            let value = match value {
                Some(value) => format!(" = {}", format_expression(value)),
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
            format_expression(target),
            format_expression(value)
        ),
        Statement::Delete { path, .. } => format!("{pad}delete {}", format_expression(path)),
        Statement::Merge { target, value, .. } => format!(
            "{pad}merge {} = {}",
            format_expression(target),
            format_expression(value)
        ),
        Statement::Return { value, .. } => match value {
            Some(value) => format!("{pad}return {}", format_expression(value)),
            None => format!("{pad}return"),
        },
        Statement::Break { label, .. } => format!("{pad}break{}", format_label_suffix(label)),
        Statement::Continue { label, .. } => format!("{pad}continue{}", format_label_suffix(label)),
        Statement::Throw { value, .. } => format!("{pad}throw {}", format_expression(value)),
        Statement::Expr { value, .. } => format!("{pad}{}", format_expression(value)),
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            let mut out = format!(
                "{pad}if {}\n{}",
                format_opt_expression(condition.as_ref()),
                format_block(source, then_block, level + 1)
            );
            for else_if in else_ifs {
                out.push_str(&format!(
                    "\n{pad}else if {}\n{}",
                    format_opt_expression(else_if.condition.as_ref()),
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
            format_opt_expression(condition.as_ref()),
            format_block(source, body, level + 1)
        ),
        Statement::For {
            label,
            binding,
            iterable,
            body,
            ..
        } => {
            let binding = match &binding.second {
                Some(second) => format!("{}, {second}", binding.first),
                None => binding.first.clone(),
            };
            format!(
                "{pad}{}for {binding} in {}\n{}",
                format_label_prefix(label),
                format_expression(iterable),
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
            format_opt_expression(path.as_ref()),
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
    match expression {
        Expression::Literal { text, .. } => text.clone(),
        Expression::Name { segments, .. } => segments.join("::"),
        Expression::SavedRoot { name, .. } => format!("^{name}"),
        Expression::Call { callee, args, .. } => {
            let args = args
                .iter()
                .map(format_argument)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({})", format_child(callee, PREC_ATOM), args)
        }
        Expression::Field {
            base, name, quoted, ..
        } => {
            let segment = if *quoted {
                format!("\"{name}\"")
            } else {
                name.clone()
            };
            format!("{}.{}", format_child(base, PREC_ATOM), segment)
        }
        Expression::OptionalField {
            base, name, quoted, ..
        } => {
            let segment = if *quoted {
                format!("\"{name}\"")
            } else {
                name.clone()
            };
            format!("{}?.{}", format_child(base, PREC_ATOM), segment)
        }
        Expression::Unary { op, operand, .. } => {
            let operand = format_child(operand, PREC_UNARY);
            match op {
                UnaryOp::Neg => format!("-{operand}"),
                UnaryOp::Not => format!("not {operand}"),
            }
        }
        Expression::Binary {
            op, left, right, ..
        } => format_binary(*op, left, right),
        Expression::Interpolation { parts, .. } => format_interpolation(parts),
    }
}

/// Format an optional value-position expression, rendering nothing when the
/// value did not parse (a syntax error was already reported at parse time).
fn format_opt_expression(expression: Option<&Expression>) -> String {
    expression.map(format_expression).unwrap_or_default()
}

fn format_binary(op: BinaryOp, left: &Expression, right: &Expression) -> String {
    let precedence = binary_precedence(op);
    // Left-associative operators keep an equal-precedence left operand without
    // parentheses; the right operand needs them. Non-associative operators
    // (equality, comparison, range) require parentheses on either equal side.
    let (left_min, right_min) = if is_left_associative(op) {
        (precedence, precedence + 1)
    } else {
        (precedence + 1, precedence + 1)
    };
    let left = format_child(left, left_min);
    let right = format_child(right, right_min);
    match op {
        BinaryOp::RangeExclusive => format!("{left}..{right}"),
        BinaryOp::RangeInclusive => format!("{left}..={right}"),
        _ => format!("{left} {} {right}", binary_symbol(op)),
    }
}

/// Format `child`, wrapping it in parentheses when it binds looser than
/// `min_precedence` requires.
fn format_child(child: &Expression, min_precedence: u8) -> String {
    let rendered = format_expression(child);
    if precedence(child) < min_precedence {
        format!("({rendered})")
    } else {
        rendered
    }
}

fn format_argument(argument: &Argument) -> String {
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
    out.push_str(&format_expression(&argument.value));
    out
}

fn format_interpolation(parts: &[InterpolationPart]) -> String {
    let mut out = String::from("$\"");
    for part in parts {
        match part {
            // Text keeps `{{`/`}}` escaped exactly as written.
            InterpolationPart::Text { text, .. } => out.push_str(text),
            InterpolationPart::Expr(expression) => {
                out.push('{');
                out.push_str(&format_expression(expression));
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
        BinaryOp::Equal | BinaryOp::NotEqual => 3,
        BinaryOp::Coalesce => 4,
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => 5,
        BinaryOp::RangeExclusive | BinaryOp::RangeInclusive => 6,
        BinaryOp::Concat => 7,
        BinaryOp::Add | BinaryOp::Subtract => 8,
        BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Remainder => 9,
    }
}

/// Equality, `??`, comparison, and range are non-associative per the grammar;
/// all other binary operators are left-associative.
fn is_left_associative(op: BinaryOp) -> bool {
    !matches!(
        op,
        BinaryOp::Equal
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
        // Ranges are emitted without spaces by `format_binary`.
        BinaryOp::RangeExclusive => "..",
        BinaryOp::RangeInclusive => "..=",
    }
}
