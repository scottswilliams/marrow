//! Enforcement artifacts for the total parser.
//!
//! Every parse yields a node: a failure is an [`Expression::Error`]/[`Statement::Error`]
//! carrying its span, and one diagnostic is reported at the failure token. Two
//! properties keep that discipline honest — the recovery-mechanism zoo it replaced
//! stays deleted, and an error node never appears without a diagnostic, so the
//! `has_errors` gate downstream crates rely on is sound.

use std::fs;
use std::path::{Path, PathBuf};

use marrow_syntax::{
    Block, Declaration, Expression, InterpolationPart, ParsedSource, SourceFile, Statement,
    parse_source,
};

use crate::common;

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory")
        .to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn scan(pattern: &str) -> Vec<String> {
    let crates_dir = crates_dir();
    let src = crates_dir.join("marrow-syntax").join("src");
    let mut sources = Vec::new();
    rust_sources(&src, &mut sources);
    let mut offenders = Vec::new();
    for source in sources {
        let text = fs::read_to_string(&source).expect("read source");
        for (line_index, line) in text.lines().enumerate() {
            if line.contains(pattern) {
                offenders.push(format!(
                    "{}:{}",
                    source
                        .strip_prefix(&crates_dir)
                        .unwrap_or(&source)
                        .display(),
                    line_index + 1
                ));
            }
        }
    }
    offenders
}

/// The recovery machinery the total parser replaced must stay deleted: the
/// mutable gap-anchor field, the suppression-depth counter and its caller-owns
/// scope, the header-suppressing entry, and the `diagnostics.len()` before/after
/// guard that drove the generic fallback. Each is a distinct way to reintroduce a
/// second, cascading diagnostic; a resurrection is caught here.
#[test]
fn the_recovery_zoo_is_gone() {
    let mut offenders = Vec::new();
    for pattern in [
        "gap_anchor",
        "gap_suppression_depth",
        "while_caller_owns_recovery",
        "parse_complete_in_header",
        ".len() == before",
    ] {
        for offender in scan(pattern) {
            offenders.push(format!("{pattern}\t{offender}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "the total parser reports one diagnostic at the failure token and collapses \
         a failed sub-expression to an error node; the deleted recovery machinery \
         must not return:\n{}",
        offenders.join("\n")
    );
}

fn expr_has_error(expr: &Expression) -> bool {
    match expr {
        Expression::Error { .. } => true,
        Expression::Call { callee, args, .. } => {
            expr_has_error(callee) || args.iter().any(|arg| expr_has_error(&arg.value))
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            expr_has_error(base)
        }
        Expression::Unary { operand, .. } => expr_has_error(operand),
        Expression::Try { inner, .. } => expr_has_error(inner),
        Expression::Binary { left, right, .. } => expr_has_error(left) || expr_has_error(right),
        Expression::Range {
            start, end, step, ..
        } => [start, end, step]
            .into_iter()
            .flatten()
            .any(|part| expr_has_error(part)),
        Expression::Interpolation { parts, .. } => parts.iter().any(|part| match part {
            InterpolationPart::Expr(inner) => expr_has_error(inner),
            InterpolationPart::Text { .. } => false,
        }),
        Expression::Literal { .. }
        | Expression::Name { .. }
        | Expression::SavedRoot { .. }
        | Expression::Absent { .. } => false,
    }
}

fn block_has_error(block: &Block) -> bool {
    block.statements.iter().any(stmt_has_error)
}

fn stmt_has_error(stmt: &Statement) -> bool {
    match stmt {
        Statement::Error { .. } => true,
        Statement::Const { value, .. }
        | Statement::Assert { value, .. }
        | Statement::Expr { value, .. } => expr_has_error(value),
        Statement::Var { value, .. } | Statement::Return { value, .. } => {
            value.as_ref().is_some_and(expr_has_error)
        }
        Statement::Assign { target, value, .. }
        | Statement::CompoundAssign { target, value, .. } => {
            expr_has_error(target) || expr_has_error(value)
        }
        Statement::Delete { path, .. } => expr_has_error(path),
        Statement::PlaceBinding { place, .. } => expr_has_error(place),
        Statement::Unset { place, .. } => expr_has_error(place),
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            expr_has_error(condition)
                || block_has_error(then_block)
                || else_ifs.iter().any(|else_if| {
                    expr_has_error(&else_if.condition) || block_has_error(&else_if.block)
                })
                || else_block.as_ref().is_some_and(block_has_error)
        }
        Statement::IfConst {
            value,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            expr_has_error(value)
                || block_has_error(then_block)
                || else_ifs.iter().any(|else_if| {
                    expr_has_error(&else_if.condition) || block_has_error(&else_if.block)
                })
                || else_block.as_ref().is_some_and(block_has_error)
        }
        Statement::While {
            condition, body, ..
        } => expr_has_error(condition) || block_has_error(body),
        Statement::For {
            iterable,
            step,
            body,
            ..
        } => {
            expr_has_error(iterable)
                || step.as_ref().is_some_and(expr_has_error)
                || block_has_error(body)
        }
        Statement::Transaction { body, .. } => block_has_error(body),
        Statement::Match {
            scrutinee, arms, ..
        } => expr_has_error(scrutinee) || arms.iter().any(|arm| block_has_error(&arm.block)),
        Statement::Checked {
            op,
            out_of_range,
            zero_divisor,
            ..
        } => {
            expr_has_error(op)
                || [out_of_range, zero_divisor]
                    .into_iter()
                    .flatten()
                    .any(block_has_error)
        }
        Statement::Break { .. } | Statement::Continue { .. } => false,
    }
}

fn file_has_error(file: &SourceFile) -> bool {
    file.declarations
        .iter()
        .any(|declaration| match declaration {
            Declaration::Function(function) => block_has_error(&function.body),
            Declaration::Const(decl) => decl.value.as_ref().is_some_and(expr_has_error),
            _ => false,
        })
}

/// The canonical library parses to a tree with no error nodes and no diagnostics:
/// a well-formed program never yields the error placeholder.
#[test]
fn valid_programs_yield_no_error_nodes() {
    for block in common::documented_module_blocks() {
        let parsed = parse_source(&block.source);
        assert!(
            !parsed.has_errors(),
            "documented block {} should parse cleanly: {:#?}",
            block.path,
            parsed.diagnostics
        );
        assert!(
            !file_has_error(&parsed.file),
            "documented block {} should hold no error nodes",
            block.path
        );
    }
}

/// Every prefix of every documented library parses without panicking, and any
/// error node it produces travels with a diagnostic. This is the soundness
/// foundation of the `has_errors` gate: an error node can never reach a downstream
/// crate that trusts a clean `has_errors` to mean a fully structured tree.
#[test]
fn every_error_node_travels_with_a_diagnostic() {
    let mut malformed_seen = false;
    for block in common::documented_module_blocks() {
        // Every byte-boundary truncation is a distinct partially-written program.
        for end in char_boundaries(&block.source) {
            let ParsedSource { file, diagnostics } = parse_source(&block.source[..end]);
            let has_error_node = file_has_error(&file);
            let has_diagnostic = diagnostics.iter().any(|d| d.severity == severity_error());
            if has_error_node {
                malformed_seen = true;
                assert!(
                    has_diagnostic,
                    "an error node appeared with no diagnostic for prefix {:?} of {}",
                    &block.source[..end],
                    block.path
                );
            }
        }
    }
    // A property that never exercised a single error node would be vacuous.
    assert!(
        malformed_seen,
        "expected some truncation to produce an error node"
    );
}

fn char_boundaries(source: &str) -> Vec<usize> {
    (0..=source.len())
        .filter(|&index| source.is_char_boundary(index))
        .collect()
}

fn severity_error() -> marrow_syntax::Severity {
    marrow_syntax::Severity::Error
}
