//! Structural rules over the parsed tree.
//!
//! These checks read only the parsed syntax tree: `catch` type annotations,
//! assignment targets, and `const` values that are not constant expressions.
//! They do not need type or effect facts, so they run directly on each
//! declaration.

use marrow_codes::Code;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use marrow_schema::Type;
use marrow_syntax::{
    Block, CatchClause, Expression, FunctionDecl, InterpolationPart, SourceSpan, Statement,
    format_expression,
};

use crate::checks::{check_const_int_overflow, check_range_value};
use crate::typerules::{LiteralSign, check_literal_range, negated_integer_literal};
use crate::walk::for_each_child_expr;
use crate::{CHECK_COMMIT_AMPLIFICATION, CheckDiagnostic};

/// A `break`/`continue` is outside any loop, so it could never resolve at runtime.
pub const CHECK_LOOP_CONTROL_FLOW: &str = Code::CheckLoopControlFlow.as_str();
/// A `catch` annotation must be `Error`.
pub const CHECK_CATCH_TYPE: &str = Code::CheckCatchType.as_str();
/// An assignment target is not a writable place.
pub const CHECK_INVALID_ASSIGN_TARGET: &str = Code::CheckInvalidAssignTarget.as_str();
/// A `const` value is not a constant expression.
pub const CHECK_NON_CONSTANT_CONST: &str = Code::CheckNonConstantConst.as_str();
/// A loop over a saved layer mutates that same layer. Collect the keys into a
/// local sequence first when a rewrite must change the traversed layer.
pub const CHECK_LOOP_MUTATES_TRAVERSED_LAYER: &str = Code::CheckLoopMutatesTraversedLayer.as_str();

/// Apply every structural statement rule to one function body. `module_consts` names
/// the module-level constants in scope, which are immutable like a local `const`
/// unless a local `var` of the same name shadows one inside the body.
pub(crate) fn check_function_body(
    file: &Path,
    function: &FunctionDecl,
    module_consts: &HashSet<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    let mut immutable: HashMap<String, ImmutableKind> = module_consts
        .iter()
        .map(|name| (name.clone(), ImmutableKind::Const))
        .collect();
    // A parameter shadows a like-named module constant, so it overrides the seed.
    for param in &function.params {
        immutable.insert(param.name.clone(), ImmutableKind::Parameter);
    }
    // A keyed-collection parameter is a local collection place, so a write through
    // it reads as a keyed assignment rather than an unknown target. The parameter
    // is read-only, so the immutability rule then rejects the write with a precise
    // message, exactly as it would for any read-only binding.
    let local_collections: HashSet<String> = function
        .params
        .iter()
        .filter(|param| !param.keys.is_empty())
        .map(|param| param.name.clone())
        .collect();
    walk_block(file, &function.body, &immutable, &local_collections, out);
    walk_loop_control_flow(file, &function.body, 0, out);
    walk_loop_layer_mutations(file, &function.body, &mut Vec::new(), out);
    walk_commit_amplification(file, &function.body, false, false, out);
}

/// Apply the structural body rules to an `evolve transform` block, which has no
/// function parameters and so no immutable bindings to start.
pub(crate) fn check_transform_body(file: &Path, body: &Block, out: &mut Vec<CheckDiagnostic>) {
    walk_block(file, body, &HashMap::new(), &HashSet::new(), out);
    walk_loop_control_flow(file, body, 0, out);
    walk_loop_layer_mutations(file, body, &mut Vec::new(), out);
}

/// An immutable local place: one that names a binding which assignment cannot
/// rewrite. The kind shapes the diagnostic message; the rule is the same for all.
#[derive(Clone, Copy)]
enum ImmutableKind {
    Parameter,
    Const,
    LoopVariable,
    IfConstBinding,
}

impl ImmutableKind {
    fn message(self, name: &str) -> String {
        match self {
            Self::Parameter => format!("parameter `{name}` is read-only"),
            Self::Const => format!("`{name}` is a constant and cannot be reassigned"),
            Self::LoopVariable => format!("loop variable `{name}` cannot be reassigned"),
            Self::IfConstBinding => {
                format!("`{name}` is an `if const` binding and cannot be reassigned")
            }
        }
    }
}

/// A `const` value must be a compile-time constant expression: literals and
/// other constants combined with operators, never a host call or saved-data
/// read.
pub(crate) fn check_const_value(
    file: &Path,
    value: &Expression,
    const_ints: &HashMap<String, Option<i64>>,
    out: &mut Vec<CheckDiagnostic>,
) {
    if !is_constant_expr(value) {
        out.push(diagnostic(
            CHECK_NON_CONSTANT_CONST,
            file,
            value,
            "a `const` value must be a constant expression, not a host call or saved-data read",
        ));
    }
    check_literal_ranges(file, value, out);
    check_const_int_overflow(file, value, std::slice::from_ref(const_ints), out);
    check_range_value(file, value, out);
}

/// Range-check every literal in a `const` value, mirroring the constant-expression
/// shape walked by `is_constant_expr`.
fn check_literal_ranges(file: &Path, expr: &Expression, out: &mut Vec<CheckDiagnostic>) {
    match expr {
        Expression::Literal { kind, text, span } => {
            check_literal_range(*kind, text, LiteralSign::Bare, *span, file, out);
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            check_literal_ranges(file, base, out)
        }
        Expression::Unary { op, operand, .. } => match negated_integer_literal(*op, operand) {
            Some((text, span)) => check_literal_range(
                marrow_syntax::LiteralKind::Integer,
                text,
                LiteralSign::Negated,
                span,
                file,
                out,
            ),
            None => check_literal_ranges(file, operand, out),
        },
        Expression::Binary { left, right, .. } => {
            check_literal_ranges(file, left, out);
            check_literal_ranges(file, right, out);
        }
        Expression::Range {
            start, end, step, ..
        } => {
            for part in [start.as_deref(), end.as_deref(), step.as_deref()]
                .into_iter()
                .flatten()
            {
                check_literal_ranges(file, part, out);
            }
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Expr(expr) = part {
                    check_literal_ranges(file, expr, out);
                }
            }
        }
        // A `Call` is a leaf here on purpose: a call is never constant, so its
        // arguments are not part of a `const` value and carry no literal to range-check.
        Expression::Name { .. }
        | Expression::SavedRoot { .. }
        | Expression::Absent { .. }
        | Expression::Call { .. }
        | Expression::Error { .. } => {}
    }
}

/// Walk a block applying the catch and assign-target rules to each statement,
/// recursing into nested blocks. The block-scoped clone of `immutable` means a
/// shadowing binding in an inner block does not leak its mutability out.
fn walk_block(
    file: &Path,
    block: &Block,
    immutable: &HashMap<String, ImmutableKind>,
    inherited_local_collections: &HashSet<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    let mut immutable = immutable.clone();
    let mut local_collections = inherited_local_collections.clone();
    // The first declaration of each local name in this block, so a second `const` or
    // `var` of the same name is reported as a same-block redeclaration. An inner block
    // gets a fresh map, so shadowing across blocks stays allowed.
    let mut declared: HashMap<&str, SourceSpan> = HashMap::new();
    for statement in &block.statements {
        walk_statement(file, statement, &immutable, &local_collections, out);
        if let Some((name, span)) = local_declaration(statement) {
            if let Some(first) = declared.get(name) {
                out.push(crate::driver::duplicate_declaration_diagnostic(
                    file, name, span, *first,
                ));
            } else {
                declared.insert(name, span);
            }
        }
        match statement {
            // A `const` binding is immutable; a `var` rebinding the same name makes it
            // mutable again, though a same-block redeclaration is already reported.
            Statement::Const { name, .. } => {
                immutable.insert(name.clone(), ImmutableKind::Const);
                local_collections.remove(name);
            }
            Statement::Var { name, .. } => {
                immutable.remove(name);
                local_collections.remove(name);
            }
            _ => {}
        }
        if let Some(name) = local_collection_binding_name(statement) {
            local_collections.insert(name);
        }
    }
}

/// The `(name, span)` a `const`/`var` statement declares in its block, or `None` for
/// any other statement.
fn local_declaration(statement: &Statement) -> Option<(&str, SourceSpan)> {
    match statement {
        Statement::Const { name, span, .. } | Statement::Var { name, span, .. } => {
            Some((name, *span))
        }
        _ => None,
    }
}

/// A block walked with one name bound immutably for its duration — a loop variable
/// over the loop body, or an `if const` binding over its then block.
fn walk_block_with_immutable(
    file: &Path,
    block: &Block,
    immutable: &HashMap<String, ImmutableKind>,
    local_collections: &HashSet<String>,
    bound: &[(&str, ImmutableKind)],
    out: &mut Vec<CheckDiagnostic>,
) {
    let mut immutable = immutable.clone();
    for (name, kind) in bound {
        immutable.insert((*name).to_string(), *kind);
    }
    walk_block(file, block, &immutable, local_collections, out);
}

fn walk_statement(
    file: &Path,
    statement: &Statement,
    immutable: &HashMap<String, ImmutableKind>,
    local_collections: &HashSet<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    match statement {
        Statement::Assign { target, .. } | Statement::CompoundAssign { target, .. } => {
            check_assignment_target(file, target, immutable, local_collections, out);
        }
        Statement::If {
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            walk_block(file, then_block, immutable, local_collections, out);
            for else_if in else_ifs {
                walk_block(file, &else_if.block, immutable, local_collections, out);
            }
            if let Some(block) = else_block {
                walk_block(file, block, immutable, local_collections, out);
            }
        }
        // The `if const` binding is immutable only inside the then block; the else
        // arms do not see it.
        Statement::IfConst {
            name,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            walk_block_with_immutable(
                file,
                then_block,
                immutable,
                local_collections,
                &[(name, ImmutableKind::IfConstBinding)],
                out,
            );
            for else_if in else_ifs {
                walk_block(file, &else_if.block, immutable, local_collections, out);
            }
            if let Some(block) = else_block {
                walk_block(file, block, immutable, local_collections, out);
            }
        }
        // Every loop variable is immutable across the loop body.
        Statement::For { binding, body, .. } => {
            let bound: Vec<(&str, ImmutableKind)> = binding
                .names
                .iter()
                .map(|name| (name.name.as_str(), ImmutableKind::LoopVariable))
                .collect();
            walk_block_with_immutable(file, body, immutable, local_collections, &bound, out);
        }
        Statement::While { body, .. } | Statement::Transaction { body, .. } => {
            walk_block(file, body, immutable, local_collections, out)
        }
        Statement::Try { body, catch, .. } => {
            walk_block(file, body, immutable, local_collections, out);
            if let Some(catch) = catch {
                check_catch(file, catch, out);
                walk_block(file, &catch.block, immutable, local_collections, out);
            }
        }
        Statement::Match { arms, .. } => {
            for arm in arms {
                walk_block(file, &arm.block, immutable, local_collections, out);
            }
        }
        Statement::Const { .. }
        | Statement::Var { .. }
        | Statement::Delete { .. }
        | Statement::Return { .. }
        | Statement::Break { .. }
        | Statement::Continue { .. }
        | Statement::Throw { .. }
        | Statement::Expr { .. }
        | Statement::Error { .. } => {}
    }
}

fn check_assignment_target(
    file: &Path,
    target: &Expression,
    immutable: &HashMap<String, ImmutableKind>,
    local_collections: &HashSet<String>,
    out: &mut Vec<CheckDiagnostic>,
) {
    if !is_assignable(target) && !is_local_collection_lookup(target, local_collections) {
        out.push(diagnostic(
            CHECK_INVALID_ASSIGN_TARGET,
            file,
            target,
            "assignment target is not a writable place",
        ));
        return;
    }

    if let Some(name) = place_root_name(target)
        && let Some(kind) = immutable.get(name)
    {
        out.push(diagnostic(
            CHECK_INVALID_ASSIGN_TARGET,
            file,
            target,
            &kind.message(name),
        ));
    }
}

fn local_collection_binding_name(statement: &Statement) -> Option<String> {
    let Statement::Var { name, keys, ty, .. } = statement else {
        return None;
    };
    if !keys.is_empty()
        || ty
            .as_ref()
            .is_some_and(|ty| matches!(Type::resolve(ty), Type::Sequence(_)))
    {
        Some(name.clone())
    } else {
        None
    }
}

fn is_local_collection_lookup(target: &Expression, local_collections: &HashSet<String>) -> bool {
    let Expression::Call { callee, .. } = target else {
        return false;
    };
    let Expression::Name { segments, .. } = callee.as_ref() else {
        return false;
    };
    let [name] = segments.as_slice() else {
        return false;
    };
    local_collections.contains(name)
}

/// A `catch` annotation, if present, must name `Error`. A bare catch is fine.
fn check_catch(file: &Path, catch: &CatchClause, out: &mut Vec<CheckDiagnostic>) {
    if let Some(ty) = &catch.ty
        && ty.to_string() != "Error"
    {
        out.push(CheckDiagnostic::error(
            CHECK_CATCH_TYPE,
            file,
            ty.span(),
            format!("catch type must be `Error`, found `{}`", ty),
        ));
    }
}

/// Whether a `break`/`continue` resolves to an enclosing loop.
fn jump_resolves_in_scope(loop_depth: usize) -> bool {
    loop_depth > 0
}

/// Walk a block reporting a `break`/`continue` that resolves to no enclosing loop,
/// which the runtime would otherwise only catch late with `run.no_enclosing_loop`.
fn walk_loop_control_flow(
    file: &Path,
    block: &Block,
    loop_depth: usize,
    out: &mut Vec<CheckDiagnostic>,
) {
    for statement in &block.statements {
        match statement {
            Statement::Break { .. } | Statement::Continue { .. }
                if !jump_resolves_in_scope(loop_depth) =>
            {
                out.push(diagnostic_at(
                    CHECK_LOOP_CONTROL_FLOW,
                    file,
                    statement,
                    "control flow statement is not inside a loop",
                ));
            }
            Statement::If {
                then_block,
                else_ifs,
                else_block,
                ..
            }
            | Statement::IfConst {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                walk_loop_control_flow(file, then_block, loop_depth, out);
                for else_if in else_ifs {
                    walk_loop_control_flow(file, &else_if.block, loop_depth, out);
                }
                if let Some(block) = else_block {
                    walk_loop_control_flow(file, block, loop_depth, out);
                }
            }
            Statement::While { body, .. } | Statement::For { body, .. } => {
                walk_loop_control_flow(file, body, loop_depth + 1, out);
            }
            Statement::Transaction { body, .. } => {
                walk_loop_control_flow(file, body, loop_depth, out);
            }
            Statement::Try { body, catch, .. } => {
                walk_loop_control_flow(file, body, loop_depth, out);
                if let Some(catch) = catch {
                    walk_loop_control_flow(file, &catch.block, loop_depth, out);
                }
            }
            Statement::Match { arms, .. } => {
                for arm in arms {
                    walk_loop_control_flow(file, &arm.block, loop_depth, out);
                }
            }
            Statement::Const { .. }
            | Statement::Var { .. }
            | Statement::Assign { .. }
            | Statement::CompoundAssign { .. }
            | Statement::Delete { .. }
            | Statement::Return { .. }
            | Statement::Break { .. }
            | Statement::Continue { .. }
            | Statement::Throw { .. }
            | Statement::Expr { .. }
            | Statement::Error { .. } => {}
        }
    }
}

/// A saved layer an enclosing `for` loop is traversing, with the loop's bound
/// key-column names, outermost-first. A field write into this layer at exactly the
/// loop's cursor — every key column bound positionally to its live loop name —
/// revisits the current entry and is safe; any other key set may insert or rewrite
/// a sibling mid-traversal.
///
/// The leaf value name of an (n+1)-name head is not a key and is not tracked. A slot
/// is cleared to `None` once its name is rebound in the body: a rebound name no
/// longer denotes the live loop key, so the whole exemption dies with it (failing
/// closed on shadowing).
struct TraversedLayer {
    text: String,
    column_keys: Vec<Option<String>>,
}

/// The bound key-column names of a `for` head, outermost-first: a 1-name head binds
/// its single key column; an (n+1)-name head binds all n key columns, and its final
/// name is the leaf value, which is not a key.
fn head_column_keys(binding: &marrow_syntax::ForBinding) -> Vec<Option<String>> {
    let names = &binding.names;
    let key_count = if names.len() == 1 { 1 } else { names.len() - 1 };
    names[..key_count]
        .iter()
        .map(|name| Some(name.name.clone()))
        .collect()
}

/// Walk a block reporting a write, delete, or append that mutates the same saved
/// layer an enclosing `for` loop is traversing, forbidden because mutating a tree
/// layer while iterating it has undefined ordering. `traversed` holds a
/// [`TraversedLayer`] for each enclosing loop's traversed saved layer, carrying its
/// canonical text and live loop-key binding.
fn walk_loop_layer_mutations(
    file: &Path,
    block: &Block,
    traversed: &mut Vec<TraversedLayer>,
    out: &mut Vec<CheckDiagnostic>,
) {
    for statement in &block.statements {
        for rebound in rebound_names(statement) {
            clear_rebound_slot(traversed, rebound);
        }
        if loop_layer_mutation(statement, traversed) {
            out.push(diagnostic_at(
                CHECK_LOOP_MUTATES_TRAVERSED_LAYER,
                file,
                statement,
                "this write changes the saved layer the enclosing loop is traversing; \
                 collect the keys into a local sequence first",
            ));
        }
        match statement {
            Statement::If {
                then_block,
                else_ifs,
                else_block,
                ..
            }
            | Statement::IfConst {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                walk_loop_layer_mutations(file, then_block, traversed, out);
                for else_if in else_ifs {
                    walk_loop_layer_mutations(file, &else_if.block, traversed, out);
                }
                if let Some(block) = else_block {
                    walk_loop_layer_mutations(file, block, traversed, out);
                }
            }
            Statement::For {
                binding,
                iterable,
                body,
                ..
            } => {
                // A nested loop's own head names may shadow an outer loop key; binding
                // them permanently clears the matching outer slot, exactly as an
                // assignment or `const`/`var` rebind does (failing closed on shadowing).
                for name in &binding.names {
                    clear_rebound_slot(traversed, &name.name);
                }
                let pushed = traversed_layer(iterable).map(|text| TraversedLayer {
                    text,
                    column_keys: head_column_keys(binding),
                });
                let depth = traversed.len();
                if let Some(layer) = pushed {
                    traversed.push(layer);
                }
                walk_loop_layer_mutations(file, body, traversed, out);
                traversed.truncate(depth);
            }
            Statement::While { body, .. } | Statement::Transaction { body, .. } => {
                walk_loop_layer_mutations(file, body, traversed, out);
            }
            Statement::Try { body, catch, .. } => {
                walk_loop_layer_mutations(file, body, traversed, out);
                if let Some(catch) = catch {
                    // The caught-error name binds over the catch block; treat it like any
                    // other rebind of an outer loop key and clear the matching slot.
                    clear_rebound_slot(traversed, &catch.name);
                    walk_loop_layer_mutations(file, &catch.block, traversed, out);
                }
            }
            Statement::Match { arms, .. } => {
                for arm in arms {
                    walk_loop_layer_mutations(file, &arm.block, traversed, out);
                }
            }
            Statement::Const { .. }
            | Statement::Var { .. }
            | Statement::Assign { .. }
            | Statement::CompoundAssign { .. }
            | Statement::Delete { .. }
            | Statement::Return { .. }
            | Statement::Break { .. }
            | Statement::Continue { .. }
            | Statement::Throw { .. }
            | Statement::Expr { .. }
            | Statement::Error { .. } => {}
        }
    }
}

fn walk_commit_amplification(
    file: &Path,
    block: &Block,
    in_loop: bool,
    in_transaction: bool,
    out: &mut Vec<CheckDiagnostic>,
) {
    for statement in &block.statements {
        if in_loop && !in_transaction {
            push_commit_amplification_warnings(file, statement, out);
        }
        match statement {
            Statement::If {
                then_block,
                else_ifs,
                else_block,
                ..
            }
            | Statement::IfConst {
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                walk_commit_amplification(file, then_block, in_loop, in_transaction, out);
                for else_if in else_ifs {
                    if in_loop && !in_transaction {
                        push_append_write_warnings(file, &else_if.condition, out);
                    }
                    walk_commit_amplification(file, &else_if.block, in_loop, in_transaction, out);
                }
                if let Some(block) = else_block {
                    walk_commit_amplification(file, block, in_loop, in_transaction, out);
                }
            }
            Statement::While {
                condition, body, ..
            } => {
                if !in_transaction {
                    push_append_write_warnings(file, condition, out);
                }
                walk_commit_amplification(file, body, true, in_transaction, out);
            }
            Statement::For { body, .. } => {
                walk_commit_amplification(file, body, true, in_transaction, out);
            }
            Statement::Transaction { body, .. } => {
                walk_commit_amplification(file, body, in_loop, true, out);
            }
            Statement::Try { body, catch, .. } => {
                walk_commit_amplification(file, body, in_loop, in_transaction, out);
                if let Some(catch) = catch {
                    walk_commit_amplification(file, &catch.block, in_loop, in_transaction, out);
                }
            }
            Statement::Match { arms, .. } => {
                for arm in arms {
                    walk_commit_amplification(file, &arm.block, in_loop, in_transaction, out);
                }
            }
            Statement::Const { .. }
            | Statement::Var { .. }
            | Statement::Assign { .. }
            | Statement::CompoundAssign { .. }
            | Statement::Delete { .. }
            | Statement::Return { .. }
            | Statement::Break { .. }
            | Statement::Continue { .. }
            | Statement::Throw { .. }
            | Statement::Expr { .. }
            | Statement::Error { .. } => {}
        }
    }
}

fn push_commit_amplification_warnings(
    file: &Path,
    statement: &Statement,
    out: &mut Vec<CheckDiagnostic>,
) {
    match statement {
        Statement::Assign { target, value, .. }
        | Statement::CompoundAssign { target, value, .. } => {
            if is_saved_path(target) {
                push_commit_amplification_warning(file, statement.span(), out);
            }
            push_append_write_warnings(file, target, out);
            push_append_write_warnings(file, value, out);
        }
        Statement::Delete { path, .. } => {
            if is_saved_path(path) {
                push_commit_amplification_warning(file, statement.span(), out);
            }
            push_append_write_warnings(file, path, out);
        }
        Statement::Const { value, .. } => push_append_write_warnings(file, value, out),
        Statement::Var { value, .. } => {
            if let Some(value) = value {
                push_append_write_warnings(file, value, out);
            }
        }
        Statement::Return { value, .. } => {
            if let Some(value) = value {
                push_append_write_warnings(file, value, out);
            }
        }
        Statement::Throw { value, .. } | Statement::Expr { value, .. } => {
            push_append_write_warnings(file, value, out);
        }
        Statement::If { condition, .. } => {
            push_append_write_warnings(file, condition, out);
        }
        Statement::IfConst { value, .. } => push_append_write_warnings(file, value, out),
        Statement::For { iterable, step, .. } => {
            push_append_write_warnings(file, iterable, out);
            if let Some(step) = step {
                push_append_write_warnings(file, step, out);
            }
        }
        Statement::Match { scrutinee, .. } => {
            push_append_write_warnings(file, scrutinee, out);
        }
        Statement::Break { .. }
        | Statement::Continue { .. }
        | Statement::While { .. }
        | Statement::Transaction { .. }
        | Statement::Try { .. }
        | Statement::Error { .. } => {}
    }
}

fn push_commit_amplification_warning(
    file: &Path,
    span: marrow_syntax::SourceSpan,
    out: &mut Vec<CheckDiagnostic>,
) {
    out.push(CheckDiagnostic::warning(
        CHECK_COMMIT_AMPLIFICATION,
        file,
        span,
        "saved-data write inside a loop can amplify commits; use transaction",
    ));
}

fn push_append_write_warnings(file: &Path, expr: &Expression, out: &mut Vec<CheckDiagnostic>) {
    if let Expression::Call { callee, args, .. } = expr
        && append_target(callee, args).is_some()
    {
        push_commit_amplification_warning(file, expr.span(), out);
    }
    for_each_child_expr(expr, |child| push_append_write_warnings(file, child, out));
}

/// The saved layer a `for` loop traverses, as canonical text, or `None` for a loop
/// over a range or a local value (the "collect keys first" pattern). The head
/// iterable is a bare saved path — traversal direction is the head `reversed`
/// keyword, not a wrapper, so nothing is peeled.
fn traversed_layer(iterable: &Expression) -> Option<String> {
    is_saved_path(iterable).then(|| format_expression(iterable))
}

/// Whether `statement` mutates a layer the enclosing loop is traversing in a way
/// the loop cannot tolerate. A write or delete descends a chain of keyed entries
/// `^root(k0).layer(k1)…`; every keyed step is judged against each traversed layer,
/// not just the innermost, because an outer sibling key inserts a new entry into an
/// enclosing layer just as readily as the final step does. A *terminal* step — the
/// whole keyed entry the write replaces or the delete removes — is always unsafe at a
/// traversed layer: replacing an entry clears and rewrites its subtree (and a delete
/// removes the key), which invalidates the cursor even at the current key. An
/// *enclosing* step, descended into by a field or a further key, is safe only when it
/// addresses exactly the loop's cursor — every key column bound positionally to its
/// live loop name; any other key set may insert or rewrite a sibling mid-traversal, so
/// the conservative rule flags it (failing closed on a computed key, a swapped order,
/// an arity mismatch, or a rebound column). An `append(path, v)` adds a key to
/// `path`'s own layer (always unsafe at a matching traversed layer) and, by
/// auto-creating any absent enclosing entry, may insert a sibling into an enclosing
/// layer just like a write.
fn loop_layer_mutation(statement: &Statement, traversed: &[TraversedLayer]) -> bool {
    match statement {
        Statement::Assign { target, .. } | Statement::CompoundAssign { target, .. } => {
            place_inserts_into(target, true, traversed)
        }
        Statement::Delete { path, .. } => place_inserts_into(path, true, traversed),
        Statement::Expr {
            value: Expression::Call { callee, args, .. },
            ..
        } => append_target(callee, args).is_some_and(|path| {
            traversed.iter().any(|t| t.text == format_expression(path))
                || place_inserts_into(path, false, traversed)
        }),
        _ => false,
    }
}

/// Whether writing or deleting `place` changes the key set of any traversed layer.
/// Walks the keyed-entry spine from the place outward. `terminal` marks the outermost
/// keyed entry as the one the operation replaces or removes whole — that step has no
/// cursor exemption; every step reached by descending through it is an enclosing
/// entry whose exact-cursor address is exempt.
fn place_inserts_into(place: &Expression, terminal: bool, traversed: &[TraversedLayer]) -> bool {
    match place {
        Expression::Call { callee, args, .. } if is_saved_path(callee) => {
            keyed_step_unsafe(callee, args, terminal, traversed)
                || place_inserts_into(callee, false, traversed)
        }
        Expression::Field { base, .. } => place_inserts_into(base, false, traversed),
        _ => false,
    }
}

/// Whether a single keyed step `parent(args)` is an unsafe mutation of a traversed
/// layer. A terminal step clears or removes the entry, so any matching layer is
/// unsafe; an enclosing step is unsafe unless it addresses exactly the traversed
/// layer's live cursor.
fn keyed_step_unsafe(
    parent: &Expression,
    args: &[marrow_syntax::Argument],
    terminal: bool,
    traversed: &[TraversedLayer],
) -> bool {
    let layer = format_expression(parent);
    traversed
        .iter()
        .any(|t| t.text == layer && (terminal || !step_matches_cursor(args, &t.column_keys)))
}

/// Whether the keyed step's arguments address exactly the traversed layer's live
/// cursor: the same arity as the head's bound key columns, and every argument a bare
/// positional name equal to that column's still-live loop name. Anything else — a
/// computed key, a named argument, a swapped order, an arity mismatch, or a rebound
/// (cleared) column — fails the match, so the conservative rule flags the write.
fn step_matches_cursor(args: &[marrow_syntax::Argument], column_keys: &[Option<String>]) -> bool {
    if args.len() != column_keys.len() {
        return false;
    }
    args.iter().zip(column_keys).all(|(arg, column)| {
        let (Some(column), Expression::Name { segments, .. }) = (column, &arg.value) else {
            return false;
        };
        arg.name.is_none() && matches!(segments.as_slice(), [name] if name == column)
    })
}

/// Clear every traversed-layer column slot whose live loop name is `rebound`: once a
/// bound column name is reassigned, declared over, or shadowed, it no longer denotes
/// the live cursor, so the exemption for that column dies (and with it the layer's
/// exact-cursor exemption, since the match now requires every column).
fn clear_rebound_slot(traversed: &mut [TraversedLayer], rebound: &str) {
    for layer in traversed.iter_mut() {
        for column in layer.column_keys.iter_mut() {
            if column.as_deref() == Some(rebound) {
                *column = None;
            }
        }
    }
}

/// The local names a statement rebinds in the loop body — a `const`/`var`
/// declaration, an `if const` presence binding (which binds its name over the then
/// block to an arbitrary key), or an assignment whose target is a bare local name.
/// Once a loop column's name is rebound it no longer denotes the live loop key; the
/// clear is conservative and permanent so a name shadowed in one branch fails closed
/// everywhere after.
fn rebound_names(statement: &Statement) -> Vec<&str> {
    match statement {
        Statement::Const { name, .. }
        | Statement::Var { name, .. }
        | Statement::IfConst { name, .. } => vec![name],
        Statement::Assign { target, .. } | Statement::CompoundAssign { target, .. } => {
            place_root_name(target).into_iter().collect()
        }
        _ => Vec::new(),
    }
}

/// The saved layer argument of `append(path, value)`, or `None` for any other
/// call. `append` adds a key to its first argument's layer.
fn append_target<'a>(
    callee: &Expression,
    args: &'a [marrow_syntax::Argument],
) -> Option<&'a Expression> {
    let Expression::Name { segments, .. } = callee else {
        return None;
    };
    if segments.len() != 1 || segments[0] != "append" {
        return None;
    }
    match args {
        [path, _] if path.name.is_none() && is_saved_path(&path.value) => Some(&path.value),
        _ => None,
    }
}

/// A saved-data path: a `^root`, a key lookup on a saved path, or a field of one.
pub(crate) fn is_saved_path(expr: &Expression) -> bool {
    match expr {
        Expression::SavedRoot { .. } => true,
        Expression::Field { base, .. } => is_saved_path(base),
        Expression::Call { callee, .. } => is_saved_path(callee),
        _ => false,
    }
}

/// A writable place: a bare name, a saved root, a field of a place, or a key
/// lookup on a saved place. Local collection lookups are scope-sensitive and are
/// handled by `check_assignment_target`.
pub(crate) fn is_assignable(target: &Expression) -> bool {
    match target {
        Expression::Name { segments, .. } => segments.len() == 1,
        Expression::SavedRoot { .. } => true,
        Expression::Field { base, .. } => is_assignable(base),
        Expression::Call { callee, .. } => is_key_lookup_target(callee),
        _ => false,
    }
}

fn place_root_name(expr: &Expression) -> Option<&str> {
    match expr {
        Expression::Name { segments, .. } if segments.len() == 1 => Some(&segments[0]),
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            place_root_name(base)
        }
        Expression::Call { callee, .. } => place_root_name(callee),
        _ => None,
    }
}

/// The callee of a key-lookup place: a saved root, a field of a place, or a
/// further key lookup.
fn is_key_lookup_target(callee: &Expression) -> bool {
    match callee {
        Expression::SavedRoot { .. } => true,
        Expression::Field { base, .. } => is_assignable(base),
        Expression::Call { callee, .. } => is_key_lookup_target(callee),
        _ => false,
    }
}

/// A constant expression: a literal or a name (another constant) combined with
/// field access, operators, or interpolation. A saved-data read or a call is
/// never constant, so neither is any expression containing one.
fn is_constant_expr(expr: &Expression) -> bool {
    match expr {
        // `absent` is the empty optional: a primary value with no runtime
        // computation, so a `const`/`var` annotated `T?` may initialize to it.
        Expression::Literal { .. } | Expression::Name { .. } | Expression::Absent { .. } => true,
        // `?.` is a possibly-absent read, never a compile-time constant.
        Expression::SavedRoot { .. }
        | Expression::Call { .. }
        | Expression::OptionalField { .. } => false,
        Expression::Field { base, .. } => is_constant_expr(base),
        Expression::Unary { operand, .. } => is_constant_expr(operand),
        Expression::Binary { left, right, .. } => is_constant_expr(left) && is_constant_expr(right),
        Expression::Range {
            start, end, step, ..
        } => [start.as_deref(), end.as_deref(), step.as_deref()]
            .into_iter()
            .flatten()
            .all(is_constant_expr),
        Expression::Interpolation { parts, .. } => parts.iter().all(|part| match part {
            InterpolationPart::Text { .. } => true,
            InterpolationPart::Expr(expr) => is_constant_expr(expr),
        }),
        // A parse-error placeholder is not a constant.
        Expression::Error { .. } => false,
    }
}

fn diagnostic(
    code: &'static str,
    file: &Path,
    expr: &Expression,
    message: &str,
) -> CheckDiagnostic {
    CheckDiagnostic::error(code, file, expr.span(), message)
}

fn diagnostic_at(
    code: &'static str,
    file: &Path,
    statement: &Statement,
    message: &str,
) -> CheckDiagnostic {
    CheckDiagnostic::error(code, file, statement.span(), message)
}
