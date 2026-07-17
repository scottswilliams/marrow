//! Single-use corpus converter for the BS01 brace-surface migration.
//!
//! It parses an old layout `.mw` artifact with the frozen
//! [`crate::layout_legacy`] pipeline, rewrites the one representation the surface
//! migration changed at the AST level — paren-suffixed keyed access
//! (`^books(id)`, a `Call` over a place root) becomes bracketed keyed access
//! (`^books[id]`, an [`Expression::Keyed`]) — and re-prints the result with the
//! production brace formatter. Everything else the migration changed (block
//! delimiting, `;`→`//` comment leaders, `[..]`→`<..>` generics, `=>` arms,
//! newline enum members) is structural: the frozen parser and the shared AST
//! already agree, so the formatter alone renders the new spelling.
//!
//! Each conversion is gated on span-erased AST round-trip equality: the printed
//! output must re-parse (with the production parser) to the same tree the
//! transform produced, ignoring only source spans. A fidelity failure aborts the
//! run rather than emitting hand-patchable output. The module and its frozen
//! dependency are deleted whole once the corpus is migrated.

use crate::ast::{
    Argument, Block, Declaration, Expression, InterpolationPart, ParsedSource, Statement,
};

/// Convert one old-layout `.mw` source to canonical brace source, or report why
/// it could not be converted faithfully.
pub(crate) fn convert_source(source: &str) -> Result<String, String> {
    let legacy = crate::layout_legacy::parse_source_layout(source);
    if legacy.has_errors() {
        return Err(format!(
            "legacy parse reported diagnostics: {:?}",
            legacy
                .diagnostics
                .iter()
                .map(|d| (d.code, d.span.line, d.span.column))
                .collect::<Vec<_>>()
        ));
    }
    let mut transformed = legacy.file.clone();
    transform_file(&mut transformed);

    let expected = ParsedSource {
        file: transformed,
        diagnostics: Vec::new(),
    };
    let printed = crate::format::format_parsed(source, &expected);

    let reparsed = crate::parse_source(&printed);
    if reparsed.has_errors() {
        return Err(format!(
            "converted output does not re-parse cleanly: {:?}\n--- output ---\n{printed}",
            reparsed
                .diagnostics
                .iter()
                .map(|d| (d.code, d.span.line, d.span.column))
                .collect::<Vec<_>>()
        ));
    }

    let want = strip_spans(&format!("{:#?}", expected.file));
    let got = strip_spans(&format!("{:#?}", reparsed.file));
    if want != got {
        return Err(format!(
            "span-erased AST mismatch after round-trip\n--- output ---\n{printed}\n--- first divergence ---\n{}",
            first_divergence(&want, &got)
        ));
    }
    Ok(printed)
}

/// Remove every `SourceSpan { … }` block from a `{:#?}` AST rendering so two
/// trees compare on structure alone. `SourceSpan` holds only scalar fields, so
/// the first `}` after `SourceSpan {` closes it; there is no nesting to balance.
fn strip_spans(debug: &str) -> String {
    let mut out = String::with_capacity(debug.len());
    let mut rest = debug;
    while let Some(at) = rest.find("SourceSpan {") {
        out.push_str(&rest[..at]);
        out.push_str("SPAN");
        rest = &rest[at + "SourceSpan {".len()..];
        match rest.find('}') {
            Some(close) => rest = &rest[close + 1..],
            None => break,
        }
    }
    out.push_str(rest);
    out
}

fn first_divergence(a: &str, b: &str) -> String {
    for (la, lb) in a.lines().zip(b.lines()) {
        if la != lb {
            return format!("want: {la}\n got: {lb}");
        }
    }
    format!(
        "line counts differ: want {} got {}",
        a.lines().count(),
        b.lines().count()
    )
}

fn transform_file(file: &mut crate::ast::SourceFile) {
    for decl in &mut file.declarations {
        match decl {
            Declaration::Const(decl) => {
                if let Some(value) = &mut decl.value {
                    transform_expr(value);
                }
            }
            Declaration::Function(decl) => transform_block(&mut decl.body),
            Declaration::Test(decl) => transform_block(&mut decl.body),
            Declaration::Alias(_)
            | Declaration::Nominal(_)
            | Declaration::Resource(_)
            | Declaration::Struct(_)
            | Declaration::Store(_)
            | Declaration::Enum(_) => {}
        }
    }
}

fn transform_block(block: &mut Block) {
    for statement in &mut block.statements {
        transform_statement(statement);
    }
}

fn transform_opt(expr: &mut Option<Expression>) {
    if let Some(expr) = expr {
        transform_expr(expr);
    }
}

fn transform_statement(statement: &mut Statement) {
    match statement {
        Statement::Const { value, .. }
        | Statement::Assert { value, .. }
        | Statement::Expr { value, .. } => transform_expr(value),
        Statement::Var { value, .. } | Statement::Return { value, .. } => transform_opt(value),
        Statement::Assign { target, value, .. }
        | Statement::CompoundAssign { target, value, .. } => {
            transform_expr(target);
            transform_expr(value);
        }
        Statement::Delete { path, .. } => transform_expr(path),
        Statement::PlaceBinding { place, .. } | Statement::Unset { place, .. } => {
            transform_expr(place)
        }
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        }
        | Statement::IfConst {
            value: condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            transform_expr(condition);
            transform_block(then_block);
            for else_if in else_ifs {
                transform_expr(&mut else_if.condition);
                transform_block(&mut else_if.block);
            }
            if let Some(else_block) = else_block {
                transform_block(else_block);
            }
        }
        Statement::IfConstChain {
            bindings,
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            for binding in bindings {
                transform_expr(&mut binding.value);
            }
            transform_opt(condition);
            transform_block(then_block);
            for else_if in else_ifs {
                transform_expr(&mut else_if.condition);
                transform_block(&mut else_if.block);
            }
            if let Some(else_block) = else_block {
                transform_block(else_block);
            }
        }
        Statement::LetElse {
            value, else_block, ..
        } => {
            transform_expr(value);
            transform_block(else_block);
        }
        Statement::While {
            condition, body, ..
        } => {
            transform_expr(condition);
            transform_block(body);
        }
        Statement::For {
            iterable,
            step,
            bound,
            body,
            ..
        } => {
            transform_expr(iterable);
            transform_opt(step);
            if let Some(bound) = bound {
                transform_expr(&mut bound.limit);
                transform_opt(&mut bound.from);
                if let Some(on_more) = &mut bound.on_more {
                    transform_block(on_more);
                }
            }
            transform_block(body);
        }
        Statement::Transaction { body, .. } => transform_block(body),
        Statement::Match {
            scrutinee, arms, ..
        } => {
            transform_expr(scrutinee);
            for arm in arms {
                transform_block(&mut arm.block);
            }
        }
        Statement::Checked {
            op,
            out_of_range,
            zero_divisor,
            ..
        } => {
            transform_expr(op);
            if let Some(block) = out_of_range {
                transform_block(block);
            }
            if let Some(block) = zero_divisor {
                transform_block(block);
            }
        }
        Statement::Break { .. } | Statement::Continue { .. } | Statement::Error { .. } => {}
    }
}

/// Rewrite paren-suffixed keyed access (`Call` over a place root) into
/// [`Expression::Keyed`], recursing children first so nested access such as
/// `^books(id).notes(nid)` converts inside-out.
fn transform_expr(expr: &mut Expression) {
    match expr {
        Expression::Call { callee, args, .. } => {
            transform_expr(callee);
            for arg in args.iter_mut() {
                transform_expr(&mut arg.value);
            }
        }
        Expression::Keyed { base, keys, .. } => {
            transform_expr(base);
            for key in keys {
                transform_expr(key);
            }
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            transform_expr(base)
        }
        Expression::Unary { operand, .. } => transform_expr(operand),
        Expression::Binary { left, right, .. } => {
            transform_expr(left);
            transform_expr(right);
        }
        Expression::Range {
            start, end, step, ..
        } => {
            if let Some(start) = start {
                transform_expr(start);
            }
            if let Some(end) = end {
                transform_expr(end);
            }
            if let Some(step) = step {
                transform_expr(step);
            }
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Expr(expr) = part {
                    transform_expr(expr);
                }
            }
        }
        Expression::Try { inner, .. } => transform_expr(inner),
        Expression::Literal { .. }
        | Expression::Name { .. }
        | Expression::SavedRoot { .. }
        | Expression::Absent { .. }
        | Expression::Error { .. } => {}
    }

    // Post-order: after children are converted, a `Call` whose callee roots at a
    // durable place (or a field/keyed chain over one) is keyed access, never an
    // invocation — a place is not callable. A bare-name callee is a function,
    // constructor, or local keyed collection and stays a `Call`.
    if let Expression::Call {
        callee,
        args,
        multiline,
        span,
    } = expr
        && is_place_rooted(callee)
    {
        {
            let keys = std::mem::take(args)
                .into_iter()
                .map(|Argument { name, value }| {
                    assert!(
                        name.is_none(),
                        "keyed access over a place must not carry a named argument"
                    );
                    value
                })
                .collect();
            *expr = Expression::Keyed {
                base: std::mem::replace(callee, Box::new(Expression::Error { span: *span })),
                keys,
                multiline: *multiline,
                span: *span,
            };
        }
    }
}

/// Whether an expression's leftmost root is a durable place (`^root`). Field,
/// optional-field, and keyed suffixes preserve place-rootedness; a bare name or
/// literal does not.
fn is_place_rooted(expr: &Expression) -> bool {
    match expr {
        Expression::SavedRoot { .. } => true,
        Expression::Field { base, .. }
        | Expression::OptionalField { base, .. }
        | Expression::Keyed { base, .. } => is_place_rooted(base),
        Expression::Call { callee, .. } => is_place_rooted(callee),
        _ => false,
    }
}

#[cfg(test)]
mod driver {
    use super::convert_source;
    use std::path::{Path, PathBuf};

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .canonicalize()
            .expect("canonicalize repo root")
    }

    fn mw_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                mw_files(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("mw") {
                out.push(path);
            }
        }
    }

    /// Convert every `.mw` fixture in place. Ignored by default: it rewrites the
    /// corpus and is invoked explicitly in the converter flip.
    #[test]
    #[ignore = "BS01 converter: rewrites the .mw fixture corpus in place"]
    fn convert_fixtures() {
        let root = repo_root();
        let mut files = Vec::new();
        mw_files(&root.join("fixtures"), &mut files);
        files.sort();
        let mut converted = 0usize;
        let mut failures = Vec::new();
        for path in &files {
            let source = std::fs::read_to_string(path).expect("read fixture");
            match convert_source(&source) {
                Ok(new) => {
                    if new != source {
                        std::fs::write(path, &new).expect("write fixture");
                    }
                    converted += 1;
                }
                Err(why) => failures.push(format!("{}: {why}", path.display())),
            }
        }
        eprintln!("converted {converted}/{} fixtures", files.len());
        assert!(
            failures.is_empty(),
            "fidelity failures:\n{}",
            failures.join("\n\n")
        );
    }

    /// Verify (without writing) that every `.mw` fixture converts faithfully.
    /// Ignored: it depends on the pre-conversion corpus being present.
    #[test]
    #[ignore = "BS01 converter: fidelity gate over the layout fixture corpus"]
    fn gate_fixtures() {
        let root = repo_root();
        let mut files = Vec::new();
        mw_files(&root.join("fixtures"), &mut files);
        files.sort();
        let mut failures = Vec::new();
        for path in &files {
            let source = std::fs::read_to_string(path).expect("read fixture");
            if let Err(why) = convert_source(&source) {
                failures.push(format!("{}: {why}", path.display()));
            }
        }
        assert!(
            failures.is_empty(),
            "fidelity failures:\n{}",
            failures.join("\n\n")
        );
    }

    /// Convert every ` ```mw ` fence in `docs/**.md` in place, splicing the
    /// converted program back between the fence markers and leaving all
    /// surrounding prose untouched. Ignored by default.
    #[test]
    #[ignore = "BS01 converter: rewrites docs ```mw fences in place"]
    fn convert_doc_fences() {
        let root = repo_root();
        let mut mds = Vec::new();
        collect_md(&root.join("docs"), &mut mds);
        mds.sort();
        let mut fences = 0usize;
        let mut failures = Vec::new();
        for path in &mds {
            let text = std::fs::read_to_string(path).expect("read md");
            let (new_text, count, mut errs) = convert_fences(&text, path);
            fences += count;
            failures.append(&mut errs);
            if new_text != text {
                std::fs::write(path, &new_text).expect("write md");
            }
        }
        eprintln!("converted {fences} mw fences across {} docs", mds.len());
        assert!(
            failures.is_empty(),
            "fidelity failures:\n{}",
            failures.join("\n\n")
        );
    }

    /// Fidelity gate over every docs ` ```mw ` fence, without writing.
    #[test]
    #[ignore = "BS01 converter: fidelity gate over docs mw fences"]
    fn gate_doc_fences() {
        let root = repo_root();
        let mut mds = Vec::new();
        collect_md(&root.join("docs"), &mut mds);
        mds.sort();
        let mut fences = 0usize;
        let mut failures = Vec::new();
        for path in &mds {
            let text = std::fs::read_to_string(path).expect("read md");
            let (_new, count, mut errs) = convert_fences(&text, path);
            fences += count;
            failures.append(&mut errs);
        }
        eprintln!("gated {fences} mw fences across {} docs", mds.len());
        assert!(
            failures.is_empty(),
            "fidelity failures:\n{}",
            failures.join("\n\n")
        );
    }

    /// Print the converted form of a representative keyed-access fixture as
    /// human-checkable evidence that paren keyed access becomes bracket access.
    #[test]
    #[ignore = "BS01 converter: keyed-access conversion evidence"]
    fn show_keyed_access() {
        let root = repo_root();
        let mut files = Vec::new();
        mw_files(&root.join("fixtures"), &mut files);
        files.sort();
        for path in files {
            let source = std::fs::read_to_string(&path).expect("read fixture");
            if !source.contains("^") || !source.contains("(") {
                continue;
            }
            if let Ok(new) = convert_source(&source)
                && new.contains("[")
            {
                eprintln!("=== {}\n{new}\n", path.display());
                return;
            }
        }
        panic!("no keyed-access fixture found");
    }

    fn collect_md(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                collect_md(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                out.push(path);
            }
        }
    }

    fn convert_fences(text: &str, path: &Path) -> (String, usize, Vec<String>) {
        let mut out = String::with_capacity(text.len());
        let mut fences = 0usize;
        let mut failures = Vec::new();
        let mut lines = text.lines().peekable();
        let trailing_newline = text.ends_with('\n');
        let mut first = true;
        while let Some(line) = lines.next() {
            if !first {
                out.push('\n');
            }
            first = false;
            out.push_str(line);
            if line.trim_end() != "```mw" {
                continue;
            }
            // Collect the fence body up to the closing ```.
            let mut body = String::new();
            let mut closed = false;
            for inner in lines.by_ref() {
                if inner.trim_end() == "```" {
                    closed = true;
                    match convert_source(&body) {
                        Ok(new) => {
                            for out_line in new.trim_end_matches('\n').lines() {
                                out.push('\n');
                                out.push_str(out_line);
                            }
                            fences += 1;
                        }
                        Err(why) => {
                            // Leave the original body untouched, report it.
                            for out_line in body.trim_end_matches('\n').lines() {
                                out.push('\n');
                                out.push_str(out_line);
                            }
                            failures.push(format!("{}: fence failed: {why}", path.display()));
                        }
                    }
                    out.push('\n');
                    out.push_str(inner);
                    break;
                }
                body.push_str(inner);
                body.push('\n');
            }
            assert!(closed, "unterminated ```mw fence in {}", path.display());
        }
        if trailing_newline {
            out.push('\n');
        }
        (out, fences, failures)
    }
}
