//! The statement/block type driver: a fresh scope frame per block, and the
//! `StatementCheck` dispatch that infers each expression, records bindings, and
//! routes each statement kind to its operator, range, condition, and saved-access
//! checks.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use marrow_codes::Code;
use marrow_syntax::SourceSpan;

use crate::diagnostics::{DiagnosticAnchor, UninitializedVarKind};
use crate::enums::{MatchCheck, check_match, resolve_diagnosed_annotation_type};
use crate::executable::{SavedPlaceResolver, lower_expr_for_file};
use crate::infer::{
    assignment_target_is_error_code, bind, infer_assignment_target_type_with_read_scope,
    infer_collection_subject_type_with_read_scope, infer_type_with_read_scope,
    local_binding_with_read_scope, reject_saved_access_with_suggested_index,
};
use crate::presence::{FlowCtx, Narrowing, ReadScope};
use crate::resolve::resolve_store_by_root;
use crate::typerules::is_optional_value;
use crate::{
    CHECK_CALL_ARGUMENT, CHECK_COLLECTION_UNSUPPORTED, CHECK_KEY_TYPE, CHECK_UNRESOLVED_NAME,
    CheckDiagnostic, CheckedProgram, ConditionTypeFault, DiagnosticPayload, MarrowType,
};

use super::calls::is_by_value_collection_slot;
use super::const_int::{ConstIntScope, fold_const_int};
use super::loop_head::{LoopHeadScope, check_for_head, for_frame};
use super::operators::{
    check_assignment, check_binary, check_condition, check_return_type, check_throw_type,
};
use super::ranges::{
    check_range_header, check_range_iterable_value_parts, check_range_value_guarded,
};
use super::required_fields::RequiredFieldAssignments;
use super::returns::check_return_values;
use super::saved_keys::{
    SequencePositionWrite, check_sequence_position_write, saved_root_args_address_record,
};
use super::saved_paths::{
    is_partial_key_layer_path, is_saved_collection_path, is_saved_index_branch_path,
    is_saved_key_range_path, is_saved_path_with_key_range_arg,
};

/// The scope frame a `catch` clause's block runs under: the caught error value
/// bound to its name. Shared by the type pass and cursor scope reconstruction so the
/// two cannot drift.
pub(crate) fn catch_frame(clause: &marrow_syntax::CatchClause) -> HashMap<String, MarrowType> {
    let mut frame = HashMap::new();
    frame.insert(clause.name.clone(), MarrowType::Error);
    frame
}

/// Type-check a function body, tracking the type of each in-scope binding and
/// inferring each expression. A check fires only when a type or signature is known
/// to be wrong, so an unresolved value — a saved-data read, a cross-module value,
/// an unresolved call — is never a false positive.
pub(crate) fn check_function_types(
    program: &CheckedProgram,
    file: &Path,
    function: &marrow_syntax::FunctionDecl,
    module_constants: &HashMap<String, MarrowType>,
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    // Module constants overlaid with the parameter list; a parameter shadows a
    // like-named constant.
    let mut base = module_constants.clone();
    for param in &function.params {
        base.insert(
            param.name.clone(),
            MarrowType::keyed(
                param
                    .keys
                    .iter()
                    .map(|key| resolve_diagnosed_annotation_type(&key.ty, program, aliases, file)),
                resolve_diagnosed_annotation_type(&param.ty, program, aliases, file),
            ),
        );
    }
    let mut scope: Vec<HashMap<String, MarrowType>> = vec![base];
    // A statically-known integer environment mirroring the type scope, seeded with
    // the module's integer constants. A parameter shadows a like-named constant, so
    // its name is masked as dynamic in this function.
    let mut const_base = module_const_ints(program, file);
    for param in &function.params {
        const_base.insert(param.name.clone(), None);
    }
    let mut const_ints: ConstIntScope = vec![const_base];
    let mut required_fields = RequiredFieldAssignments::new();
    let mut narrowing = Narrowing::new();
    let return_type = function
        .return_type
        .as_ref()
        .map_or(MarrowType::Unknown, |ty| {
            resolve_diagnosed_annotation_type(ty, program, aliases, file)
        });
    check_undeclared_saved_roots(program, file, &function.body, diagnostics);
    let body_start = diagnostics.len();
    check_block_types_with_read_scope(
        BlockTypeContext {
            program,
            file,
            return_type: &return_type,
            aliases,
            transform_old: None,
        },
        &function.body,
        &mut scope,
        &mut const_ints,
        diagnostics,
        &mut required_fields,
        &mut narrowing,
    );
    collapse_repeated_unresolved_names(diagnostics, body_start);
}

/// Reject every `^root` in a body that names no declared store. A saved root is the
/// only spelling of a saved address, so an undeclared or misspelled root is a static
/// resolution error at its span. Without this the path lowerer drops the whole body
/// as unresolvable and the runtime faults late with no source location.
fn check_undeclared_saved_roots(
    program: &CheckedProgram,
    file: &Path,
    block: &marrow_syntax::Block,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::Statement;
    for statement in &block.statements {
        match statement {
            Statement::Const { value, .. }
            | Statement::Throw { value, .. }
            | Statement::Expr { value, .. } => {
                reject_undeclared_roots_in_expr(program, file, value, diagnostics)
            }
            Statement::Var { value, .. } | Statement::Return { value, .. } => {
                if let Some(value) = value {
                    reject_undeclared_roots_in_expr(program, file, value, diagnostics);
                }
            }
            Statement::Assign { target, value, .. }
            | Statement::CompoundAssign { target, value, .. } => {
                reject_undeclared_roots_in_expr(program, file, target, diagnostics);
                reject_undeclared_roots_in_expr(program, file, value, diagnostics);
            }
            Statement::Delete { path, .. } => {
                reject_undeclared_roots_in_expr(program, file, path, diagnostics)
            }
            Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                reject_undeclared_roots_in_expr(program, file, condition, diagnostics);
                check_branch_saved_roots(
                    program,
                    file,
                    then_block,
                    else_ifs,
                    else_block,
                    diagnostics,
                );
            }
            Statement::IfConst {
                value,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                reject_undeclared_roots_in_expr(program, file, value, diagnostics);
                check_branch_saved_roots(
                    program,
                    file,
                    then_block,
                    else_ifs,
                    else_block,
                    diagnostics,
                );
            }
            Statement::While {
                condition, body, ..
            } => {
                reject_undeclared_roots_in_expr(program, file, condition, diagnostics);
                check_undeclared_saved_roots(program, file, body, diagnostics);
            }
            Statement::For {
                iterable,
                step,
                body,
                ..
            } => {
                reject_undeclared_roots_in_expr(program, file, iterable, diagnostics);
                if let Some(step) = step {
                    reject_undeclared_roots_in_expr(program, file, step, diagnostics);
                }
                check_undeclared_saved_roots(program, file, body, diagnostics);
            }
            Statement::Transaction { body, .. } => {
                check_undeclared_saved_roots(program, file, body, diagnostics)
            }
            Statement::Try { body, catch, .. } => {
                check_undeclared_saved_roots(program, file, body, diagnostics);
                if let Some(catch) = catch {
                    check_undeclared_saved_roots(program, file, &catch.block, diagnostics);
                }
            }
            Statement::Match {
                scrutinee, arms, ..
            } => {
                reject_undeclared_roots_in_expr(program, file, scrutinee, diagnostics);
                for arm in arms {
                    check_undeclared_saved_roots(program, file, &arm.block, diagnostics);
                }
            }
            Statement::Break { .. } | Statement::Continue { .. } | Statement::Error { .. } => {}
        }
    }
}

/// Walk the then-block, each else-if condition and block, and the else-block of a
/// branching statement for undeclared saved roots.
fn check_branch_saved_roots(
    program: &CheckedProgram,
    file: &Path,
    then_block: &marrow_syntax::Block,
    else_ifs: &[marrow_syntax::ElseIf],
    else_block: &Option<marrow_syntax::Block>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    check_undeclared_saved_roots(program, file, then_block, diagnostics);
    for else_if in else_ifs {
        reject_undeclared_roots_in_expr(program, file, &else_if.condition, diagnostics);
        check_undeclared_saved_roots(program, file, &else_if.block, diagnostics);
    }
    if let Some(else_block) = else_block {
        check_undeclared_saved_roots(program, file, else_block, diagnostics);
    }
}

fn reject_undeclared_roots_in_expr(
    program: &CheckedProgram,
    file: &Path,
    expr: &marrow_syntax::Expression,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    crate::walk::for_each_saved_root(expr, &mut |root, span| {
        if resolve_store_by_root(program, root).is_none() {
            diagnostics.push(CheckDiagnostic::new(
                Code::CheckUnknownRoot,
                DiagnosticAnchor::at(file, span),
                DiagnosticPayload::UnknownRoot {
                    root: root.to_string(),
                },
                &program.decl_ids(),
            ));
        }
    });
}

/// The module's integer constants folded to their values, seeding the function or
/// block const-int environment.
fn module_const_ints(program: &CheckedProgram, file: &Path) -> HashMap<String, Option<i64>> {
    program
        .module_by_file(file)
        .map(|module| super::const_int::module_const_int_scope(&module.constants))
        .unwrap_or_default()
}

/// One undeclared name is one root cause however many times the function uses it. A
/// failed assignment leaves the read sites that follow still unresolved, so the bare
/// name reports at the first use and later uses of the same name are dropped. The
/// dedup is keyed by the typed name payload, not the span, and stays within this
/// function's diagnostics so distinct names and other functions' faults are untouched.
fn collapse_repeated_unresolved_names(diagnostics: &mut Vec<CheckDiagnostic>, body_start: usize) {
    let mut reported: HashSet<String> = HashSet::new();
    let mut index = body_start;
    while index < diagnostics.len() {
        if let DiagnosticPayload::UnresolvedName { name } = &diagnostics[index].payload
            && !reported.insert(name.clone())
        {
            diagnostics.remove(index);
            continue;
        }
        index += 1;
    }
}

/// Type-check a block under a fresh scope frame for its `const`/`var` bindings,
/// folding its integer keys in the caller's live const-int scope. The caller threads
/// the enclosing function- and block-local constants so a body nested in a `match`
/// arm folds a local `const` exactly as a top-level body does.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_block_types(
    program: &CheckedProgram,
    file: &Path,
    return_type: &MarrowType,
    block: &marrow_syntax::Block,
    scope: &mut Vec<HashMap<String, MarrowType>>,
    const_ints: &mut ConstIntScope,
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let mut required_fields = RequiredFieldAssignments::inactive();
    let mut narrowing = Narrowing::new();
    check_block_types_with_read_scope(
        BlockTypeContext {
            program,
            file,
            return_type,
            aliases,
            transform_old: None,
        },
        block,
        scope,
        const_ints,
        diagnostics,
        &mut required_fields,
        &mut narrowing,
    );
}

pub(crate) struct TransformBlockTypeCheck<'a> {
    pub(crate) program: &'a CheckedProgram,
    pub(crate) file: &'a Path,
    pub(crate) return_type: &'a MarrowType,
    pub(crate) block: &'a marrow_syntax::Block,
    pub(crate) scope: &'a mut Vec<HashMap<String, MarrowType>>,
    pub(crate) aliases: &'a HashMap<String, Vec<String>>,
    pub(crate) transform_old_resource: &'a str,
    pub(crate) diagnostics: &'a mut Vec<CheckDiagnostic>,
}

pub(crate) fn check_transform_block_types(check: TransformBlockTypeCheck<'_>) {
    let TransformBlockTypeCheck {
        program,
        file,
        return_type,
        block,
        scope,
        aliases,
        transform_old_resource,
        diagnostics,
    } = check;
    let transform_old =
        scope
            .len()
            .checked_sub(1)
            .map(|frame| crate::presence::TransformOldReadScope {
                resource: transform_old_resource,
                frame,
            });
    check_return_values(file, block, true, diagnostics);
    let mut required_fields = RequiredFieldAssignments::inactive();
    let mut narrowing = Narrowing::new();
    // Mirror the caller's scope: the module constants seed the fold, and every frame
    // the caller already bound (the transform's `old`) masks its names as dynamic, so
    // a binding that shadows a like-named module constant cannot fold to it.
    let mut const_ints: ConstIntScope = vec![module_const_ints(program, file)];
    for frame in scope.iter().skip(1) {
        const_ints.push(frame.keys().map(|name| (name.clone(), None)).collect());
    }
    check_block_types_with_read_scope(
        BlockTypeContext {
            program,
            file,
            return_type,
            aliases,
            transform_old,
        },
        block,
        scope,
        &mut const_ints,
        diagnostics,
        &mut required_fields,
        &mut narrowing,
    );
}

#[derive(Clone, Copy)]
struct BlockTypeContext<'a> {
    program: &'a CheckedProgram,
    file: &'a Path,
    return_type: &'a MarrowType,
    aliases: &'a HashMap<String, Vec<String>>,
    transform_old: Option<crate::presence::TransformOldReadScope<'a>>,
}

fn check_block_types_with_read_scope(
    context: BlockTypeContext<'_>,
    block: &marrow_syntax::Block,
    scope: &mut Vec<HashMap<String, MarrowType>>,
    const_ints: &mut ConstIntScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
    required_fields: &mut RequiredFieldAssignments,
    narrowing: &mut Narrowing,
) {
    scope.push(HashMap::new());
    const_ints.push(HashMap::new());
    required_fields.push_frame();
    let mut fresh_next_id = None;
    for statement in &block.statements {
        check_statement_types(
            context,
            statement,
            scope,
            const_ints,
            diagnostics,
            required_fields,
            narrowing,
            fresh_next_id.as_ref(),
        );
        fresh_next_id = adjacent_fresh_next_id_binding(context.program, statement);
    }
    required_fields.pop_frame();
    const_ints.pop();
    scope.pop();
}

/// Type-check one statement, recursing into nested blocks and recording the type
/// of any binding it introduces.
#[allow(clippy::too_many_arguments)]
fn check_statement_types(
    context: BlockTypeContext<'_>,
    statement: &marrow_syntax::Statement,
    scope: &mut Vec<HashMap<String, MarrowType>>,
    const_ints: &mut ConstIntScope,
    diagnostics: &mut Vec<CheckDiagnostic>,
    required_fields: &mut RequiredFieldAssignments,
    narrowing: &mut Narrowing,
    fresh_next_id: Option<&FreshNextId>,
) {
    StatementCheck {
        program: context.program,
        file: context.file,
        return_type: context.return_type,
        scope,
        const_ints,
        aliases: context.aliases,
        transform_old: context.transform_old,
        diagnostics,
        required_fields,
        narrowing,
        fresh_next_id,
    }
    .check(statement);
}

struct FreshNextId {
    binding: String,
    root: String,
}

struct StatementCheck<'a> {
    program: &'a CheckedProgram,
    file: &'a Path,
    return_type: &'a MarrowType,
    scope: &'a mut Vec<HashMap<String, MarrowType>>,
    const_ints: &'a mut ConstIntScope,
    aliases: &'a HashMap<String, Vec<String>>,
    transform_old: Option<crate::presence::TransformOldReadScope<'a>>,
    diagnostics: &'a mut Vec<CheckDiagnostic>,
    required_fields: &'a mut RequiredFieldAssignments,
    narrowing: &'a mut Narrowing,
    fresh_next_id: Option<&'a FreshNextId>,
}

impl StatementCheck<'_> {
    fn check(&mut self, statement: &marrow_syntax::Statement) {
        use marrow_syntax::Statement;
        match statement {
            Statement::Const { ty, value, .. } => {
                self.check_binding_statement(statement, ty.as_ref(), Some(value))
            }
            Statement::Var { ty, value, .. } => {
                self.check_binding_statement(statement, ty.as_ref(), value.as_ref())
            }
            Statement::Assign { target, value, .. } => {
                self.check_assignment_statement(target, value)
            }
            Statement::CompoundAssign {
                target,
                op,
                op_span,
                value,
                ..
            } => self.check_compound_assignment_statement(target, *op, *op_span, value),
            Statement::Delete { path, .. } => self.check_delete_statement(path),
            Statement::Return { value, .. } => {
                self.check_return(value.as_ref());
                self.required_fields.invalidate_all();
            }
            Statement::Throw { value, .. } => {
                self.check_throw(value);
                self.required_fields.invalidate_all();
            }
            Statement::Expr { value, .. } => {
                self.infer(value);
                self.check_range_value(value);
                self.narrow_invalidate_if_writes_saved(value);
            }
            Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => self.check_conditional(condition, then_block, else_ifs, else_block.as_ref()),
            Statement::IfConst { .. } => self.check_if_const(statement),
            Statement::While {
                condition, body, ..
            } => self.check_while(condition, body),
            Statement::For {
                binding,
                order,
                iterable,
                step,
                body,
                ..
            } => self.check_for(binding, *order, iterable, step.as_ref(), body),
            Statement::Transaction { body, .. } => {
                self.check_block(body);
            }
            Statement::Try { body, catch, .. } => self.check_try(body, catch.as_ref()),
            Statement::Match {
                scrutinee,
                arms,
                span,
                ..
            } => self.check_match_statement(scrutinee, arms, *span),
            Statement::Break { .. } | Statement::Continue { .. } => {
                self.required_fields.invalidate_all();
            }
            Statement::Error { .. } => {}
        }
    }

    fn infer(&mut self, expr: &marrow_syntax::Expression) -> MarrowType {
        infer_type_with_read_scope(
            self.program,
            expr,
            self.scope,
            self.aliases,
            self.file,
            self.diagnostics,
            self.const_ints,
            ReadScope::new(self.transform_old, self.narrowing.current()),
        )
    }

    /// The narrowings a guard condition proves for its then-block (built inline so
    /// the immutable scope read stays a disjoint field borrow from the later
    /// mutation of the narrowing state).
    fn condition_narrowings(
        &self,
        condition: &marrow_syntax::Expression,
    ) -> Vec<crate::presence::ReadTarget> {
        FlowCtx {
            program: self.program,
            file: self.file,
            type_scope: self.scope,
            transform_old: self.transform_old,
        }
        .condition_narrowings(condition)
    }

    fn negated_exists_narrowings(
        &self,
        condition: &marrow_syntax::Expression,
    ) -> Vec<crate::presence::ReadTarget> {
        FlowCtx {
            program: self.program,
            file: self.file,
            type_scope: self.scope,
            transform_old: self.transform_old,
        }
        .negated_exists_narrowings(condition)
    }

    fn traversal_narrowing(
        &self,
        iterable: &marrow_syntax::Expression,
        binding: &marrow_syntax::ForBinding,
    ) -> Option<crate::presence::ReadTarget> {
        FlowCtx {
            program: self.program,
            file: self.file,
            type_scope: self.scope,
            transform_old: self.transform_old,
        }
        .traversal_narrowing(iterable, binding)
    }

    fn if_const_subject_target(
        &self,
        value: &marrow_syntax::Expression,
    ) -> Option<crate::presence::ReadTarget> {
        FlowCtx {
            program: self.program,
            file: self.file,
            type_scope: self.scope,
            transform_old: self.transform_old,
        }
        .if_const_subject_target(value)
    }

    fn expr_writes_saved(&self, expr: &marrow_syntax::Expression) -> bool {
        FlowCtx {
            program: self.program,
            file: self.file,
            type_scope: self.scope,
            transform_old: self.transform_old,
        }
        .expr_writes_saved(expr)
    }

    /// Drop any narrowing a write to `target` could have cleared (a reassigned key
    /// binding, or an overlapping or alias-possible saved write).
    fn narrow_invalidate_write(&mut self, target: &marrow_syntax::Expression) {
        let ctx = FlowCtx {
            program: self.program,
            file: self.file,
            type_scope: self.scope,
            transform_old: self.transform_old,
        };
        self.narrowing.invalidate_write(&ctx, target);
    }

    /// Drop every saved narrowing when an evaluated expression may run a
    /// field-writing call. Skips the lowering when nothing is narrowed.
    fn narrow_invalidate_if_writes_saved(&mut self, expr: &marrow_syntax::Expression) {
        if !self.narrowing.current().is_empty() && self.expr_writes_saved(expr) {
            self.narrowing.invalidate_saved();
        }
    }

    /// Check a guarded block (an `if`/`else if` then-block) under the narrowings its
    /// condition proves, restoring the pre-guard set — minus anything the block
    /// invalidated — on exit.
    fn check_guarded_block(
        &mut self,
        condition: &marrow_syntax::Expression,
        block: &marrow_syntax::Block,
    ) {
        let augment = self.condition_narrowings(condition);
        let snapshot = self.narrowing.enter(augment);
        self.check_inconclusive_block(block);
        self.narrowing.exit(snapshot);
    }

    fn check_block(&mut self, block: &marrow_syntax::Block) {
        check_block_types_with_read_scope(
            BlockTypeContext {
                program: self.program,
                file: self.file,
                return_type: self.return_type,
                aliases: self.aliases,
                transform_old: self.transform_old,
            },
            block,
            self.scope,
            self.const_ints,
            self.diagnostics,
            self.required_fields,
            self.narrowing,
        );
    }

    fn check_inconclusive_block(&mut self, block: &marrow_syntax::Block) {
        let mut required_fields = RequiredFieldAssignments::inactive();
        check_block_types_with_read_scope(
            BlockTypeContext {
                program: self.program,
                file: self.file,
                return_type: self.return_type,
                aliases: self.aliases,
                transform_old: self.transform_old,
            },
            block,
            self.scope,
            self.const_ints,
            self.diagnostics,
            &mut required_fields,
            self.narrowing,
        );
    }

    fn check_binding_statement(
        &mut self,
        statement: &marrow_syntax::Statement,
        annotation: Option<&marrow_syntax::TypeExpr>,
        value: Option<&marrow_syntax::Expression>,
    ) {
        let value_type = match value {
            Some(value) => {
                let value_type = self.infer(value);
                self.check_range_value(value);
                if self.value_is_saved_collection(value) {
                    self.reject_saved_collection_materialization(value);
                }
                // A `const` value is a compile-time constant expression, so arithmetic
                // that overflows `i64` is out of range at check, like the value-equal
                // literal. A `var` value is evaluated at run, so it is excluded.
                if matches!(statement, marrow_syntax::Statement::Const { .. }) {
                    super::check_const_int_overflow(
                        self.file,
                        value,
                        self.const_ints,
                        self.diagnostics,
                    );
                }
                value_type
            }
            None => MarrowType::Unknown,
        };
        if let (Some(annotation), Some(value)) = (annotation, value) {
            check_assignment(
                &self.program.decl_ids(),
                self.file,
                value.span(),
                &resolve_diagnosed_annotation_type(
                    annotation,
                    self.program,
                    self.aliases,
                    self.file,
                ),
                &value_type,
                self.diagnostics,
            );
            if marrow_schema::is_error_code_annotation(annotation) {
                super::calls::check_error_code_literal(
                    &self.program.decl_ids(),
                    value,
                    "an `ErrorCode` binding",
                    self.file,
                    self.diagnostics,
                );
            }
        }
        if annotation.is_none()
            && matches!(value_type, MarrowType::Absent)
            && let Some(value) = value
        {
            let names = self.program.decl_ids();
            self.diagnostics.push(CheckDiagnostic::new(
                Code::CheckUnannotatedAbsent,
                DiagnosticAnchor::at(self.file, value.span()),
                DiagnosticPayload::None,
                &names,
            ));
        }
        if value.is_none() {
            self.check_uninitialized_binding(statement, annotation);
        }
        if let Some(value) = value {
            self.narrow_invalidate_if_writes_saved(value);
        }
        self.bind_local(statement);
    }

    /// Reject a `var` of a type with no buildable initial form — an enum or a store
    /// identity — declared without an initializer. A keyed `var` starts as an empty
    /// tree, a resource builds field by field, and a scalar defaults, so each is a
    /// legitimate uninitialized declaration; an enum and an identity have neither a
    /// default member nor incremental construction, so an uninitialized one would only
    /// fault at its first use.
    fn check_uninitialized_binding(
        &mut self,
        statement: &marrow_syntax::Statement,
        annotation: Option<&marrow_syntax::TypeExpr>,
    ) {
        let marrow_syntax::Statement::Var { keys, span, .. } = statement else {
            return;
        };
        if !keys.is_empty() {
            return;
        }
        let Some(annotation) = annotation else {
            return;
        };
        let ty =
            resolve_diagnosed_annotation_type(annotation, self.program, self.aliases, self.file);
        let kind = match ty {
            MarrowType::Enum(_) => UninitializedVarKind::Enum,
            MarrowType::Identity(_) => UninitializedVarKind::Identity,
            _ => return,
        };
        let names = self.program.decl_ids();
        self.diagnostics.push(CheckDiagnostic::new(
            Code::CheckUninitializedVar,
            DiagnosticAnchor::at(self.file, *span),
            DiagnosticPayload::UninitializedVar {
                kind,
                annotation: annotation.to_string(),
            },
            &names,
        ));
    }

    /// Whether `value` materializes a saved collection — a store root, saved keyed
    /// sub-layer, or index branch, bare or laundered through `keys`/`values` — which
    /// streams in place and has no local materialization. Such a value cannot fill a
    /// local: binding it to a `const`/`var` or assigning it to a local collection would
    /// materialize the un-materializable, and every downstream use of the laundered
    /// local then checks clean and faults at runtime. A single saved value (a scalar
    /// leaf, a whole record) is excluded, so a legitimate value copy is untouched. The
    /// classifier is shared with the by-value-argument boundary.
    fn value_is_saved_collection(&self, value: &marrow_syntax::Expression) -> bool {
        super::calls::materializes_saved_collection_by_value(
            self.program,
            value,
            self.scope,
            self.file,
        )
    }

    /// Whether `target` addresses a saved place. Assigning to one is a saved write, so
    /// a saved-collection value on the right is a whole-root replacement, not a local
    /// materialization.
    fn target_is_saved_place(&self, target: &marrow_syntax::Expression) -> bool {
        lower_expr_for_file(self.program, self.file, target, self.scope)
            .is_some_and(|checked| checked.saved_place().is_some())
    }

    /// Whether `target` addresses a clearable saved place — a sparse field or keyed
    /// leaf — so its write target presents as `Optional` (present-or-clear).
    fn target_is_clearable_saved_place(&self, target: &marrow_syntax::Expression) -> bool {
        lower_expr_for_file(self.program, self.file, target, self.scope).is_some_and(|checked| {
            SavedPlaceResolver::new(self.program).write_target_clearable(&checked)
        })
    }

    fn reject_saved_collection_materialization(&mut self, value: &marrow_syntax::Expression) {
        self.diagnostics.push(CheckDiagnostic::error(
            CHECK_COLLECTION_UNSUPPORTED,
            self.file,
            value.span(),
            "a saved collection is iterated in place, not materialized as a local; iterate it or build a local collection",
        ));
    }

    fn bind_local(&mut self, statement: &marrow_syntax::Statement) {
        if let Some((name, ty)) = local_binding_with_read_scope(
            self.program,
            statement,
            self.scope,
            self.aliases,
            self.file,
            ReadScope::new(self.transform_old, self.narrowing.current()),
        ) {
            self.required_fields
                .bind_statement(self.program, statement, &name, &ty);
            self.record_const_int(statement, &name);
            bind(self.scope, &name, ty);
        }
    }

    /// Record a binding in the const-int environment. A `const` whose value folds to a
    /// known integer maps to that value; every other binding masks any like-named outer
    /// constant as dynamic, so a shadowing `var` or non-constant `const` does not fold
    /// to the shadowed value.
    fn record_const_int(&mut self, statement: &marrow_syntax::Statement, name: &str) {
        let value = match statement {
            marrow_syntax::Statement::Const { value, .. } => fold_const_int(value, self.const_ints),
            _ => None,
        };
        if let Some(frame) = self.const_ints.last_mut() {
            frame.insert(name.to_string(), value);
        }
    }

    /// Check `body` under a scope frame that binds the names in `frame` — a loop or
    /// catch variable, or an `if const` unwrapped binding. The const-int environment
    /// masks every bound name as dynamic, so a binding that shadows an outer constant
    /// does not fold to the shadowed value.
    fn check_block_under_frame(
        &mut self,
        frame: HashMap<String, MarrowType>,
        body: &marrow_syntax::Block,
    ) {
        self.check_block_under_frame_narrowed(frame, Vec::new(), body);
    }

    /// Check `body` under a scope frame binding the loop or catch variables, and a
    /// narrowing scope: `augment` is re-imposed at the body header (a loop traversal),
    /// and any narrowing the body clears is dropped on exit so a post-body read
    /// re-triggers the one rule.
    fn check_block_under_frame_narrowed(
        &mut self,
        frame: HashMap<String, MarrowType>,
        augment: Vec<crate::presence::ReadTarget>,
        body: &marrow_syntax::Block,
    ) {
        let masked = frame.keys().map(|name| (name.clone(), None)).collect();
        self.scope.push(frame);
        self.const_ints.push(masked);
        let snapshot = self.narrowing.enter(augment);
        self.check_inconclusive_block(body);
        self.narrowing.exit(snapshot);
        self.const_ints.pop();
        self.scope.pop();
    }

    /// Check a `for` loop body with the iterated-entry traversal narrowing imposed
    /// for the body. The traversal target is resolved after the loop binding enters
    /// scope, so a read keyed on the binding matches the narrowed place.
    fn check_for_loop_body(
        &mut self,
        frame: HashMap<String, MarrowType>,
        binding: &marrow_syntax::ForBinding,
        iterable: &marrow_syntax::Expression,
        body: &marrow_syntax::Block,
    ) {
        let masked: HashMap<String, Option<i64>> =
            frame.keys().map(|name| (name.clone(), None)).collect();
        self.scope.push(frame);
        self.const_ints.push(masked);
        let augment = self
            .traversal_narrowing(iterable, binding)
            .into_iter()
            .collect();
        let snapshot = self.narrowing.enter(augment);
        self.rewiden_loop_header(body);
        self.check_inconclusive_block(body);
        self.narrowing.exit(snapshot);
        self.const_ints.pop();
        self.scope.pop();
    }

    /// Re-impose the one rule at a loop header before the body is typed. The forward
    /// pass already widens a narrowing *after* a write within one iteration, but a loop
    /// also carries iteration one's write back to iteration two's textually-earlier
    /// read, so a place the body can clear anywhere must read as `Optional` at the
    /// header. Invalidation only removes from the narrowed set, so gathering the body's
    /// whole write footprint up front yields the loop fixpoint in a single typing pass.
    fn rewiden_loop_header(&mut self, body: &marrow_syntax::Block) {
        self.invalidate_block_writes(body);
    }

    fn invalidate_block_writes(&mut self, block: &marrow_syntax::Block) {
        for statement in &block.statements {
            self.invalidate_statement_writes(statement);
        }
    }

    /// Drop every narrowing a statement (and its nested blocks) could clear, mirroring
    /// the per-statement invalidation the forward pass applies: a write target, a
    /// field-writing call, a delete, or a match arm.
    fn invalidate_statement_writes(&mut self, statement: &marrow_syntax::Statement) {
        use marrow_syntax::Statement;
        match statement {
            Statement::Const { value, .. }
            | Statement::Throw { value, .. }
            | Statement::Expr { value, .. } => self.narrow_invalidate_if_writes_saved(value),
            Statement::Var { value, .. } | Statement::Return { value, .. } => {
                if let Some(value) = value {
                    self.narrow_invalidate_if_writes_saved(value);
                }
            }
            Statement::Assign { target, value, .. }
            | Statement::CompoundAssign { target, value, .. } => {
                self.narrow_invalidate_if_writes_saved(value);
                self.narrow_invalidate_write(target);
            }
            Statement::Delete { path, .. } => self.narrow_invalidate_write(path),
            Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                self.narrow_invalidate_if_writes_saved(condition);
                self.invalidate_conditional_writes(then_block, else_ifs, else_block.as_ref());
            }
            Statement::IfConst {
                value,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                self.narrow_invalidate_if_writes_saved(value);
                self.invalidate_conditional_writes(then_block, else_ifs, else_block.as_ref());
            }
            Statement::While {
                condition, body, ..
            } => {
                self.narrow_invalidate_if_writes_saved(condition);
                self.invalidate_block_writes(body);
            }
            Statement::For {
                iterable,
                step,
                body,
                ..
            } => {
                self.narrow_invalidate_if_writes_saved(iterable);
                if let Some(step) = step {
                    self.narrow_invalidate_if_writes_saved(step);
                }
                self.invalidate_block_writes(body);
            }
            Statement::Transaction { body, .. } => self.invalidate_block_writes(body),
            Statement::Try { body, catch, .. } => {
                self.invalidate_block_writes(body);
                if let Some(catch) = catch {
                    self.invalidate_block_writes(&catch.block);
                }
            }
            Statement::Match {
                scrutinee, arms, ..
            } => {
                self.narrow_invalidate_if_writes_saved(scrutinee);
                self.narrowing.invalidate_saved();
                for arm in arms {
                    self.invalidate_block_writes(&arm.block);
                }
            }
            Statement::Break { .. } | Statement::Continue { .. } | Statement::Error { .. } => {}
        }
    }

    fn invalidate_conditional_writes(
        &mut self,
        then_block: &marrow_syntax::Block,
        else_ifs: &[marrow_syntax::ElseIf],
        else_block: Option<&marrow_syntax::Block>,
    ) {
        self.invalidate_block_writes(then_block);
        for else_if in else_ifs {
            self.narrow_invalidate_if_writes_saved(&else_if.condition);
            self.invalidate_block_writes(&else_if.block);
        }
        if let Some(else_block) = else_block {
            self.invalidate_block_writes(else_block);
        }
    }

    fn check_assignment_statement(
        &mut self,
        target: &marrow_syntax::Expression,
        value: &marrow_syntax::Expression,
    ) {
        let target_type = infer_assignment_target_type_with_read_scope(
            self.program,
            target,
            self.scope,
            self.const_ints,
            self.aliases,
            self.file,
            self.diagnostics,
            ReadScope::new(self.transform_old, self.narrowing.current()),
        );
        // A clearable saved place — a sparse field or keyed leaf — presents as
        // `Optional` so `absent` and a `T?` value write through present-or-clear, while
        // a present `T` still widens in. A required field or positional element keeps
        // its bare `T` and rejects an unresolved `T?` (the one rule).
        let target_type = if self.target_is_clearable_saved_place(target) {
            MarrowType::optional(target_type)
        } else {
            target_type
        };
        let value_type = self.infer(value);
        self.check_range_value(value);
        // Assigning a saved collection to a local target launders the same
        // un-materializable stream the binding case does. Assigning to a saved target is
        // a whole-root write, not a local materialization, so it is excluded.
        if self.value_is_saved_collection(value) && !self.target_is_saved_place(target) {
            self.reject_saved_collection_materialization(value);
        }
        self.check_assignment_target(target);
        if let Some(store) = saved_root_replacement(TargetCheck {
            program: self.program,
            target,
            scope: self.scope,
            aliases: self.aliases,
            file: self.file,
            diagnostics: self.diagnostics,
            transform_old: self.transform_old,
        }) {
            self.required_fields
                .check_whole_root_write(self.file, value, store, self.diagnostics);
        }
        self.check_lossy_round_trip_warning(target, value);
        check_assignment(
            &self.program.decl_ids(),
            self.file,
            value.span(),
            &target_type,
            &value_type,
            self.diagnostics,
        );
        if assignment_target_is_error_code(
            self.program,
            target,
            self.scope,
            self.aliases,
            self.file,
            ReadScope::new(self.transform_old, self.narrowing.current()),
        ) {
            super::calls::check_error_code_literal(
                &self.program.decl_ids(),
                value,
                "an `ErrorCode` field",
                self.file,
                self.diagnostics,
            );
        }
        self.narrow_invalidate_if_writes_saved(value);
        self.narrow_invalidate_write(target);
        self.required_fields.assign_target(target);
    }

    fn check_compound_assignment_statement(
        &mut self,
        target: &marrow_syntax::Expression,
        op: marrow_syntax::CompoundAssignOp,
        op_span: SourceSpan,
        value: &marrow_syntax::Expression,
    ) {
        let target_type = infer_assignment_target_type_with_read_scope(
            self.program,
            target,
            self.scope,
            self.const_ints,
            self.aliases,
            self.file,
            self.diagnostics,
            ReadScope::new(self.transform_old, self.narrowing.current()),
        );
        // The target is resolved once, by the write leg above, which owns its
        // resolution diagnostics. The read leg needs the target's read type (which
        // wraps a maybe-present place in `?`) to combine and to require a proof, so it
        // reinfers into a scratch sink rather than restating the same diagnostic.
        let left_type = infer_type_with_read_scope(
            self.program,
            target,
            self.scope,
            self.aliases,
            self.file,
            &mut Vec::new(),
            self.const_ints,
            ReadScope::new(self.transform_old, self.narrowing.current()),
        );
        let right_type = self.infer(value);
        self.check_range_value(value);
        self.check_assignment_target(target);
        // The target is read above (consulting narrowing) before it is written, so
        // invalidate after the read and before any later statement.
        self.narrow_invalidate_if_writes_saved(value);
        self.narrow_invalidate_write(target);
        // A compound assignment reads the target before combining it, so a
        // maybe-present target must be resolved first — the one rule on the read.
        if is_optional_value(&left_type) {
            self.diagnostics
                .push(crate::typerules::unresolved_optional_diagnostic(
                    self.file,
                    target.span(),
                ));
            self.required_fields.assign_target(target);
            return;
        }
        let computed_type = check_binary(
            &self.program.decl_ids(),
            op.binary(),
            &left_type,
            &right_type,
            op_span,
            self.file,
            self.diagnostics,
        );
        check_assignment(
            &self.program.decl_ids(),
            self.file,
            op_span,
            &target_type,
            &computed_type,
            self.diagnostics,
        );
        self.required_fields.assign_target(target);
    }

    fn check_assignment_target(&mut self, target: &marrow_syntax::Expression) {
        if is_saved_index_branch_path(self.program, target, self.scope, self.file) {
            self.diagnostics.push(CheckDiagnostic::error(
                crate::rules::CHECK_INVALID_ASSIGN_TARGET,
                self.file,
                target.span(),
                "generated index branches cannot be assigned",
            ));
        } else if is_saved_path_with_key_range_arg(self.program, target, self.scope, self.file) {
            self.diagnostics.push(CheckDiagnostic::error(
                crate::rules::CHECK_INVALID_ASSIGN_TARGET,
                self.file,
                target.span(),
                "saved key ranges cannot be assigned",
            ));
        } else if is_partial_key_layer_path(self.program, target, self.scope, self.file) {
            self.diagnostics.push(CheckDiagnostic::error(
                crate::rules::CHECK_INVALID_ASSIGN_TARGET,
                self.file,
                target.span(),
                "a partially keyed layer addresses an inner sub-layer, not a writable entry; supply every key column to write an entry",
            ));
        } else if is_saved_collection_path(self.program, target, self.scope, self.file) {
            self.diagnostics.push(CheckDiagnostic::error(
                crate::rules::CHECK_INVALID_ASSIGN_TARGET,
                self.file,
                target.span(),
                "a bare store root addresses the whole collection, not a writable record; supply every identity key to write an entry",
            ));
        }
        if self.local_resource_nested_write_is_unaddressable(target)
            && !has_invalid_assign_target(self.diagnostics, self.file, target.span())
        {
            self.diagnostics.push(CheckDiagnostic::error(
                crate::rules::CHECK_INVALID_ASSIGN_TARGET,
                self.file,
                target.span(),
                "a nested write on a local resource must descend its declared groups; a keyed layer, a scalar field, or an undeclared member is not a place to write through",
            ));
        }
        check_sequence_position_write(SequencePositionWrite {
            program: self.program,
            target,
            scope: self.scope,
            const_ints: self.const_ints,
            aliases: self.aliases,
            span: target.span(),
            file: self.file,
            diagnostics: self.diagnostics,
        });
    }

    fn check_lossy_round_trip_warning(
        &mut self,
        target: &marrow_syntax::Expression,
        value: &marrow_syntax::Expression,
    ) {
        if !saved_record_replacement_has_keyed_layer(TargetCheck {
            program: self.program,
            target,
            scope: self.scope,
            aliases: self.aliases,
            file: self.file,
            diagnostics: self.diagnostics,
            transform_old: self.transform_old,
        }) {
            return;
        }
        if self
            .fresh_next_id
            .is_some_and(|fresh| target_is_adjacent_fresh_next_id_insert(target, fresh))
            && self.value_is_local_resource_name(value)
        {
            return;
        }
        let names = self.program.decl_ids();
        self.diagnostics.push(CheckDiagnostic::new(
            Code::CheckLossyRoundTrip,
            DiagnosticAnchor::at(self.file, target.span()),
            DiagnosticPayload::None,
            &names,
        ));
    }

    fn value_is_local_resource_name(&self, value: &marrow_syntax::Expression) -> bool {
        let marrow_syntax::Expression::Name { segments, .. } = value else {
            return false;
        };
        let [name] = segments.as_slice() else {
            return false;
        };
        self.scope
            .iter()
            .rev()
            .find_map(|frame| frame.get(name))
            .is_some_and(|ty| matches!(ty, MarrowType::Resource(_)))
    }

    /// Whether `target` writes a nested field of a local resource through a path that
    /// addresses no writable place, e.g. `book.versions(1).title` (or its unkeyed
    /// spelling `book.versions.title`), `book.title.sub` descending a scalar field, or
    /// `book.bogus.deep` descending an undeclared member. A local resource builds its
    /// plain and grouped fields directly, so the only writable nested path descends
    /// declared unkeyed groups (`p.name.first`); a keyed layer, a non-group field, or an
    /// undeclared member is not a group to descend, and a write through one corrupts the
    /// local resource into a bogus nested value at runtime. The check keys on the schema,
    /// not on a key appearing in the source, so neither keyed spelling slips through.
    fn local_resource_nested_write_is_unaddressable(
        &self,
        target: &marrow_syntax::Expression,
    ) -> bool {
        let Some((root, members)) = local_field_write_path(target) else {
            return false;
        };
        let Some(MarrowType::Resource(id)) =
            self.scope.iter().rev().find_map(|frame| frame.get(root))
        else {
            return false;
        };
        self.program
            .resource_by_id(*id)
            .is_some_and(|(resource, _)| descent_leaves_local_resource(resource, &members))
    }

    fn check_delete_statement(&mut self, path: &marrow_syntax::Expression) {
        // A delete target is an address, not a value read. Inferring it through the
        // collection-subject position surfaces its key-argument and structural
        // diagnostics while leaving the value-read partial-key gate silent, so the
        // dedicated partial-key delete rejection below is the single root cause.
        let subject_type = infer_collection_subject_type_with_read_scope(
            self.program,
            path,
            self.scope,
            self.const_ints,
            self.aliases,
            self.file,
            self.diagnostics,
            ReadScope::new(self.transform_old, self.narrowing.current()),
        );
        if is_saved_index_branch_path(self.program, path, self.scope, self.file) {
            self.diagnostics.push(CheckDiagnostic::error(
                CHECK_COLLECTION_UNSUPPORTED,
                self.file,
                path.span(),
                "generated index branches cannot be deleted",
            ));
        } else if is_saved_path_with_key_range_arg(self.program, path, self.scope, self.file) {
            self.diagnostics.push(CheckDiagnostic::error(
                crate::rules::CHECK_INVALID_ASSIGN_TARGET,
                self.file,
                path.span(),
                "saved key ranges cannot be deleted",
            ));
        } else if is_partial_key_layer_path(self.program, path, self.scope, self.file) {
            self.diagnostics.push(CheckDiagnostic::error(
                crate::rules::CHECK_INVALID_ASSIGN_TARGET,
                self.file,
                path.span(),
                "a partially keyed layer addresses an inner sub-layer, not a deletable entry; supply every key column to delete an entry",
            ));
        } else if !target_already_blamed(&subject_type) && !self.delete_target_is_addressable(path)
        {
            self.diagnostics.push(CheckDiagnostic::error(
                crate::rules::CHECK_INVALID_ASSIGN_TARGET,
                self.file,
                path.span(),
                "a delete addresses a saved path or a local collection entry; this is neither a deletable place",
            ));
        }
        self.narrow_invalidate_write(path);
    }

    /// Whether `path` names a place a delete can remove: a saved path (a record, a
    /// keyed-layer entry, a saved field) or a positional/keyed delete on a local
    /// collection. A resolved-but-non-saved target — a bare scalar local, a parameter,
    /// or a literal — addresses no such place, so deleting it is a check error rather
    /// than a deferred runtime fault. A target whose read inference already failed
    /// (unresolved, unknown type or member) is gated out by the caller so it reports
    /// once, not here.
    ///
    /// A saved store root applied to keys (`^books(1)`) does not lower to a standalone
    /// executable expression, so a `None` lowering is treated as addressable; the
    /// preceding saved-path branches in the caller already reject the saved shapes that
    /// are not deletable places.
    fn delete_target_is_addressable(&self, path: &marrow_syntax::Expression) -> bool {
        let Some(checked) = lower_expr_for_file(self.program, self.file, path, self.scope) else {
            return true;
        };
        checked.saved_place().is_some()
            || matches!(
                &checked,
                crate::CheckedExpr::Call {
                    target: crate::CheckedCallTarget::LocalCollection { .. },
                    ..
                }
            )
    }

    fn check_return(&mut self, value: Option<&marrow_syntax::Expression>) {
        if let Some(value) = value {
            let value_type = self.infer(value);
            self.check_range_value(value);
            // Returning a saved collection as a declared by-value collection (a
            // `sequence[...]` return type) launders the same un-materializable stream a
            // binding or assignment does, then hands it to every caller that iterates the
            // result. Reject it at this boundary so no laundered value reaches the runtime.
            if is_by_value_collection_slot(self.return_type)
                && self.value_is_saved_collection(value)
            {
                self.reject_saved_collection_materialization(value);
            }
            check_return_type(
                &self.program.decl_ids(),
                self.file,
                value.span(),
                self.return_type,
                &value_type,
                self.diagnostics,
            );
            self.narrow_invalidate_if_writes_saved(value);
        }
    }

    fn check_throw(&mut self, value: &marrow_syntax::Expression) {
        let value_type = self.infer(value);
        self.check_range_value(value);
        check_throw_type(
            &self.program.decl_ids(),
            self.file,
            value.span(),
            &value_type,
            self.diagnostics,
        );
        self.narrow_invalidate_if_writes_saved(value);
    }

    fn check_condition_expr(&mut self, condition: &marrow_syntax::Expression) {
        check_condition(
            self.program,
            self.file,
            condition,
            self.scope,
            self.const_ints,
            self.aliases,
            ReadScope::new(self.transform_old, self.narrowing.current()),
            self.diagnostics,
        );
        self.check_range_value(condition);
    }

    fn check_conditional(
        &mut self,
        condition: &marrow_syntax::Expression,
        then_block: &marrow_syntax::Block,
        else_ifs: &[marrow_syntax::ElseIf],
        else_block: Option<&marrow_syntax::Block>,
    ) {
        self.check_condition_expr(condition);
        self.narrow_invalidate_if_writes_saved(condition);
        self.check_guarded_block(condition, then_block);
        for else_if in else_ifs {
            self.check_condition_expr(&else_if.condition);
            self.narrow_invalidate_if_writes_saved(&else_if.condition);
            self.check_guarded_block(&else_if.condition, &else_if.block);
        }
        if let Some(block) = else_block {
            self.check_inconclusive_block(block);
        }
        // A fall-through-preventing `if not exists(place)` proves the place present
        // for the statements that follow the guard.
        if else_ifs.is_empty() && else_block.is_none() && block_prevents_fallthrough(then_block) {
            let narrowings = self.negated_exists_narrowings(condition);
            self.narrowing.add(narrowings);
        }
        self.required_fields.invalidate_all();
    }

    fn check_if_const(&mut self, statement: &marrow_syntax::Statement) {
        let marrow_syntax::Statement::IfConst {
            name,
            ty: annotation,
            value,
            then_block,
            else_ifs,
            else_block,
            span: _,
        } = statement
        else {
            return;
        };
        let annotation = annotation.as_ref();
        let else_block = else_block.as_ref();
        let value_type = self.infer(value);
        self.check_range_value(value);
        self.require_optional_if_const_subject(value, &value_type);
        // The subject is maybe-present even when a key carries an effect, so `if const`
        // must refuse to run that effect rather than bind on it.
        if crate::presence::guard_subject_key_effect(self.program, value, self.scope, self.file) {
            let names = self.program.decl_ids();
            self.diagnostics.push(CheckDiagnostic::new(
                Code::CheckConditionType,
                DiagnosticAnchor::at(self.file, value.span()),
                DiagnosticPayload::ConditionType(ConditionTypeFault::IfConstEffectInKey),
                &names,
            ));
        }
        // `if const` binds the present arm of the maybe-present subject: one optional
        // layer is stripped, so the then-block sees `T` for a subject typed `T?`.
        let present_type = value_type.without_optional();
        // A written annotation names the bound (present) type, like the type on a
        // `const`/`var`: an unresolvable name is a `check.unknown_type` and a
        // disagreeing type a `check.assignment_type`, and it then types the binding.
        let binding_type = match annotation {
            Some(annotation) => {
                let annotated_type = resolve_diagnosed_annotation_type(
                    annotation,
                    self.program,
                    self.aliases,
                    self.file,
                );
                check_assignment(
                    &self.program.decl_ids(),
                    self.file,
                    value.span(),
                    &annotated_type,
                    &present_type,
                    self.diagnostics,
                );
                annotated_type
            }
            None => present_type,
        };
        // `if const name = place` proves `place` itself present in the then-block, so
        // a re-read of the same saved place there reads as bare `T`.
        let augment = self.if_const_subject_target(value).into_iter().collect();
        self.narrow_invalidate_if_writes_saved(value);
        let mut frame = HashMap::new();
        frame.insert(name.to_string(), binding_type);
        self.check_block_under_frame_narrowed(frame, augment, then_block);
        for else_if in else_ifs {
            self.check_condition_expr(&else_if.condition);
            self.narrow_invalidate_if_writes_saved(&else_if.condition);
            self.check_guarded_block(&else_if.condition, &else_if.block);
        }
        if let Some(block) = else_block {
            self.check_inconclusive_block(block);
        }
        self.required_fields.invalidate_all();
    }

    /// `if const` binds the present arm of a maybe-present subject, so the subject
    /// must type to `Optional(T)` (or the empty `absent`). A definitely-present value
    /// has nothing to bind conditionally; an unresolved or poisoned subject defers so
    /// its own diagnostic owns the mistake.
    fn require_optional_if_const_subject(
        &mut self,
        value: &marrow_syntax::Expression,
        value_type: &MarrowType,
    ) {
        // A poisoned subject already reported its fault; an optional one is exactly
        // what `if const` binds. Anything else — a definite value, a collection, an
        // unresolved name — has no single maybe-present value to bind.
        if is_optional_value(value_type) || matches!(value_type, MarrowType::Invalid) {
            return;
        }
        let names = self.program.decl_ids();
        self.diagnostics.push(CheckDiagnostic::new(
            Code::CheckConditionType,
            DiagnosticAnchor::at(self.file, value.span()),
            DiagnosticPayload::ConditionType(ConditionTypeFault::IfConstRequiresBindable),
            &names,
        ));
    }

    fn check_while(&mut self, condition: &marrow_syntax::Expression, body: &marrow_syntax::Block) {
        self.check_condition_expr(condition);
        self.narrow_invalidate_if_writes_saved(condition);
        let snapshot = self.narrowing.enter(Vec::new());
        self.rewiden_loop_header(body);
        self.check_inconclusive_block(body);
        self.narrowing.exit(snapshot);
        self.required_fields.invalidate_all();
    }

    fn check_for(
        &mut self,
        binding: &marrow_syntax::ForBinding,
        order: marrow_syntax::LoopOrder,
        iterable: &marrow_syntax::Expression,
        step: Option<&marrow_syntax::Expression>,
        body: &marrow_syntax::Block,
    ) {
        // A loop iterable is a collection read, so a `^root.member(args)` lookup whose
        // member is not a declared index is a missing-index lookup: report it with the
        // index that would admit it rather than a member-access key-type error, and skip
        // the iterable checks below that would re-report the same root cause.
        if reject_saved_access_with_suggested_index(
            self.program,
            iterable,
            self.scope,
            self.file,
            self.diagnostics,
        ) {
            let frame = for_frame(
                self.program,
                binding,
                iterable,
                self.scope,
                self.aliases,
                self.file,
            );
            self.check_block_under_frame(frame, body);
            self.required_fields.invalidate_all();
            return;
        }
        let subject_type = infer_collection_subject_type_with_read_scope(
            self.program,
            iterable,
            self.scope,
            self.const_ints,
            self.aliases,
            self.file,
            self.diagnostics,
            ReadScope::new(self.transform_old, self.narrowing.current()),
        );
        // A maybe-present collection (`sequence[T]?`) must be resolved before it is
        // iterated; the one rule owns it before the iterable-shape gates so the message
        // names the four resolution forms. The body still checks under the bound frame so
        // a body mistake is not masked by the unresolved iterable.
        if is_optional_value(&subject_type) {
            self.diagnostics
                .push(crate::typerules::unresolved_optional_diagnostic(
                    self.file,
                    iterable.span(),
                ));
            let frame = for_frame(
                self.program,
                binding,
                iterable,
                self.scope,
                self.aliases,
                self.file,
            );
            self.check_block_under_frame(frame, body);
            self.required_fields.invalidate_all();
            return;
        }
        if !is_saved_index_branch_path(self.program, iterable, self.scope, self.file)
            && !is_saved_key_range_path(self.program, iterable, self.scope, self.file)
        {
            check_range_iterable_value_parts(self.file, iterable, self.diagnostics);
        }
        if let Some(step) = step {
            self.check_range_value(step);
        }
        check_range_header(
            self.program,
            self.file,
            iterable,
            step,
            self.scope,
            self.aliases,
            self.diagnostics,
        );
        check_for_head(
            &LoopHeadScope {
                program: self.program,
                file: self.file,
                scope: self.scope,
                aliases: self.aliases,
            },
            binding,
            order,
            iterable,
            self.diagnostics,
        );
        let frame = for_frame(
            self.program,
            binding,
            iterable,
            self.scope,
            self.aliases,
            self.file,
        );
        self.check_for_loop_body(frame, binding, iterable, body);
        self.required_fields.invalidate_all();
    }

    fn check_try(
        &mut self,
        body: &marrow_syntax::Block,
        catch: Option<&marrow_syntax::CatchClause>,
    ) {
        // A try body may not run to completion, so its narrowings do not survive; a
        // place it clears is dropped for the catch and the code after.
        let snapshot = self.narrowing.enter(Vec::new());
        self.check_inconclusive_block(body);
        self.narrowing.exit(snapshot);
        if let Some(clause) = catch {
            self.check_block_under_frame(catch_frame(clause), &clause.block);
        }
        self.required_fields.invalidate_all();
    }

    fn check_match_statement(
        &mut self,
        scrutinee: &marrow_syntax::Expression,
        arms: &[marrow_syntax::MatchArm],
        span: SourceSpan,
    ) {
        self.check_range_value(scrutinee);
        check_match(MatchCheck {
            program: self.program,
            file: self.file,
            return_type: self.return_type,
            scrutinee,
            arms,
            span,
            scope: self.scope,
            const_ints: self.const_ints,
            aliases: self.aliases,
            diagnostics: self.diagnostics,
        });
        // A match arm is checked outside the narrowing flow, so conservatively drop
        // every saved narrowing in case an arm cleared a proven place.
        self.narrowing.invalidate_saved();
        self.required_fields.invalidate_all();
    }

    fn check_range_value(&mut self, value: &marrow_syntax::Expression) {
        let (program, scope, file) = (self.program, &*self.scope, self.file);
        check_range_value_guarded(
            file,
            value,
            &|expr| allowed_saved_key_range_value_context(program, expr, scope, file),
            self.diagnostics,
        );
    }
}

/// Whether `block` cannot fall through to the statement after it, so a guard whose
/// then-block ends here proves its negated condition for the code that follows.
fn block_prevents_fallthrough(block: &marrow_syntax::Block) -> bool {
    block
        .statements
        .last()
        .is_some_and(statement_prevents_fallthrough)
}

fn statement_prevents_fallthrough(statement: &marrow_syntax::Statement) -> bool {
    use marrow_syntax::Statement;
    match statement {
        Statement::Return { .. }
        | Statement::Throw { .. }
        | Statement::Break { .. }
        | Statement::Continue { .. } => true,
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
        } => else_block.as_ref().is_some_and(|else_block| {
            block_prevents_fallthrough(then_block)
                && else_ifs
                    .iter()
                    .all(|else_if| block_prevents_fallthrough(&else_if.block))
                && block_prevents_fallthrough(else_block)
        }),
        Statement::Transaction { body, .. } => block_prevents_fallthrough(body),
        Statement::Try { body, catch, .. } => {
            block_prevents_fallthrough(body)
                && catch
                    .as_ref()
                    .is_none_or(|clause| block_prevents_fallthrough(&clause.block))
        }
        Statement::Match { arms, .. } => {
            !arms.is_empty()
                && arms
                    .iter()
                    .all(|arm| block_prevents_fallthrough(&arm.block))
        }
        Statement::Const { .. }
        | Statement::Var { .. }
        | Statement::Assign { .. }
        | Statement::CompoundAssign { .. }
        | Statement::Delete { .. }
        | Statement::Expr { .. }
        | Statement::While { .. }
        | Statement::For { .. }
        | Statement::Error { .. } => false,
    }
}

fn allowed_saved_key_range_value_context(
    program: &CheckedProgram,
    value: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> bool {
    let marrow_syntax::Expression::Call { callee, args, .. } = value else {
        return false;
    };
    let marrow_syntax::Expression::Name { segments, .. } = callee.as_ref() else {
        return false;
    };
    let [name] = segments.as_slice() else {
        return false;
    };
    let [arg] = args.as_slice() else {
        return false;
    };
    if arg.name.is_some() || !is_saved_key_range_path(program, &arg.value, scope, file) {
        return false;
    }
    // A saved key-range argument to a cardinality or presence call is a legitimate
    // traversal shape, not a range used outside a `for`. Whether the specific shape is
    // supported (a store-root or keyed-layer range counts as neither) is owned by the
    // call's own argument rule, which reports an accurate message there.
    matches!(name.as_str(), "exists" | "count")
}

/// The local root and ordered member chain of a field write, e.g.
/// `("book", ["versions", "title"])` for both `book.versions.title` and
/// `book.versions(1).title`. The chain is every member named between the root and the
/// written field, ending with the field itself; a key applied to a layer is dropped,
/// since the schema, not the source, decides whether a layer is keyed. `None` for any
/// target that is not a dotted write rooted at a bare local name.
fn local_field_write_path(target: &marrow_syntax::Expression) -> Option<(&str, Vec<&str>)> {
    let marrow_syntax::Expression::Field { base, name, .. } = target else {
        return None;
    };
    let mut members = vec![name.as_str()];
    let root = collect_field_members(base, &mut members)?;
    members.reverse();
    Some((root, members))
}

/// Walk a field-write base from the written field down to its root, pushing each
/// member name (a `.field` step or the callee of a `layer(key)` lookup) onto
/// `members` in reverse order and returning the bare root name. `None` for any base
/// shape that is not a chain of field accesses and keyed-layer lookups rooted at a
/// single bare name.
fn collect_field_members<'a>(
    base: &'a marrow_syntax::Expression,
    members: &mut Vec<&'a str>,
) -> Option<&'a str> {
    match base {
        marrow_syntax::Expression::Name { segments, .. } => match segments.as_slice() {
            [name] => Some(name),
            _ => None,
        },
        marrow_syntax::Expression::Field { base, name, .. } => {
            members.push(name);
            collect_field_members(base, members)
        }
        marrow_syntax::Expression::Call { callee, .. } => match callee.as_ref() {
            marrow_syntax::Expression::Field { base, name, .. } => {
                members.push(name);
                collect_field_members(base, members)
            }
            _ => None,
        },
        _ => None,
    }
}

/// Whether descending the resource `members` chain (a field-write path from the
/// outermost member to the written field) leaves the local resource at some
/// intermediate step. Every intermediate name must resolve to a declared unkeyed group
/// to descend into: a keyed layer is populated only after the resource is saved, a
/// non-group field is a leaf with no members, and an undeclared name names nothing. Any
/// of those means the write addresses no writable place on the local value. The final
/// member is the field being written and is never itself descended into.
fn descent_leaves_local_resource(
    resource: &marrow_schema::ResourceSchema,
    members: &[&str],
) -> bool {
    let Some((_, descended)) = members.split_last() else {
        return false;
    };
    let mut current = &resource.members;
    for &name in descended {
        let Some(node) = current.iter().find(|node| node.name == name) else {
            return true;
        };
        if !node.key_params.is_empty() {
            return true;
        }
        if matches!(node.kind, marrow_schema::NodeKind::Group) {
            current = &node.members;
        } else {
            return true;
        }
    }
    false
}

fn adjacent_fresh_next_id_binding(
    program: &CheckedProgram,
    statement: &marrow_syntax::Statement,
) -> Option<FreshNextId> {
    let (binding, value) = match statement {
        marrow_syntax::Statement::Const { name, value, .. } => (name, value),
        marrow_syntax::Statement::Var {
            name,
            keys,
            value: Some(value),
            ..
        } if keys.is_empty() => (name, value),
        _ => return None,
    };
    let root = fresh_next_id_root(program, value)?;
    Some(FreshNextId {
        binding: binding.clone(),
        root: root.to_string(),
    })
}

fn fresh_next_id_root<'a>(
    program: &CheckedProgram,
    value: &'a marrow_syntax::Expression,
) -> Option<&'a str> {
    let marrow_syntax::Expression::Call { callee, args, .. } = value else {
        return None;
    };
    let marrow_syntax::Expression::Name { segments, .. } = callee.as_ref() else {
        return None;
    };
    if !matches!(segments.as_slice(), [name] if name == "nextId") {
        return None;
    }
    let [arg] = args.as_slice() else {
        return None;
    };
    if arg.name.is_some() {
        return None;
    }
    let marrow_syntax::Expression::SavedRoot { name, .. } = &arg.value else {
        return None;
    };
    resolve_store_by_root(program, name)
        .filter(|store| store.store.single_int_root())
        .map(|_| name.as_str())
}

fn target_is_adjacent_fresh_next_id_insert(
    target: &marrow_syntax::Expression,
    fresh: &FreshNextId,
) -> bool {
    let marrow_syntax::Expression::Call { callee, args, .. } = target else {
        return false;
    };
    let marrow_syntax::Expression::SavedRoot { name, .. } = callee.as_ref() else {
        return false;
    };
    if name != &fresh.root {
        return false;
    }
    let [arg] = args.as_slice() else {
        return false;
    };
    if arg.name.is_some() {
        return false;
    }
    matches!(
        &arg.value,
        marrow_syntax::Expression::Name { segments, .. }
            if matches!(segments.as_slice(), [binding] if binding == &fresh.binding)
    )
}

struct TargetCheck<'p, 'a> {
    program: &'p CheckedProgram,
    target: &'a marrow_syntax::Expression,
    scope: &'a [HashMap<String, MarrowType>],
    aliases: &'a HashMap<String, Vec<String>>,
    file: &'a Path,
    diagnostics: &'a [CheckDiagnostic],
    transform_old: Option<crate::presence::TransformOldReadScope<'a>>,
}

fn saved_root_replacement<'p>(
    check: TargetCheck<'p, '_>,
) -> Option<crate::resolve::StoreResource<'p>> {
    match check.target {
        marrow_syntax::Expression::SavedRoot { name, .. } => {
            let store = resolve_store_by_root(check.program, name)?;
            store.store.identity_keys.is_empty().then_some(store)
        }
        marrow_syntax::Expression::Call { callee, args, .. } => {
            let marrow_syntax::Expression::SavedRoot { name, .. } = callee.as_ref() else {
                return None;
            };
            let store = resolve_store_by_root(check.program, name)?;
            let arg_types = target_arg_types(&check, args);
            if saved_root_args_address_record(check.program, store.store, args, &arg_types) {
                return Some(store);
            }
            None
        }
        _ => None,
    }
}

fn saved_record_replacement_has_keyed_layer(check: TargetCheck<'_, '_>) -> bool {
    let Some(target) = lower_expr_for_file(check.program, check.file, check.target, check.scope)
    else {
        return false;
    };
    let Some(place) = target.saved_place() else {
        return false;
    };
    if !saved_record_replacement_target_shape(check.target, !place.layers.is_empty()) {
        return false;
    }
    if target_has_saved_address_diagnostic(check.file, check.target.span(), check.diagnostics) {
        return false;
    }
    let resolver = SavedPlaceResolver::new(check.program);
    resolver
        .record_replacement_members(place)
        .is_some_and(|members| {
            members
                .iter()
                .any(|member| member.contains_keyed_descendant())
        })
}

fn saved_record_replacement_target_shape(
    target: &marrow_syntax::Expression,
    has_layers: bool,
) -> bool {
    if target_path_contains_optional_field(target) {
        return false;
    }
    if has_layers {
        return matches!(
            target,
            marrow_syntax::Expression::Call { callee, .. }
                if matches!(callee.as_ref(), marrow_syntax::Expression::Field { .. })
        );
    }
    match target {
        marrow_syntax::Expression::SavedRoot { .. } => true,
        marrow_syntax::Expression::Call { callee, .. } => {
            matches!(callee.as_ref(), marrow_syntax::Expression::SavedRoot { .. })
        }
        _ => false,
    }
}

fn target_path_contains_optional_field(target: &marrow_syntax::Expression) -> bool {
    match target {
        marrow_syntax::Expression::OptionalField { .. } => true,
        marrow_syntax::Expression::Call { callee, .. } => {
            target_path_contains_optional_field(callee)
        }
        marrow_syntax::Expression::Field { base, .. } => target_path_contains_optional_field(base),
        _ => false,
    }
}

fn target_arg_types(
    check: &TargetCheck<'_, '_>,
    args: &[marrow_syntax::Argument],
) -> Vec<MarrowType> {
    let mut diagnostics = Vec::new();
    args.iter()
        .map(|arg| {
            infer_type_with_read_scope(
                check.program,
                &arg.value,
                check.scope,
                check.aliases,
                check.file,
                &mut diagnostics,
                &[],
                ReadScope::transform(check.transform_old),
            )
        })
        .collect()
}

fn target_has_saved_address_diagnostic(
    file: &Path,
    span: SourceSpan,
    diagnostics: &[CheckDiagnostic],
) -> bool {
    diagnostics.iter().any(|diagnostic| {
        diagnostic.file == file
            && span_contains(span, diagnostic.span)
            && matches!(
                diagnostic.code,
                CHECK_CALL_ARGUMENT
                    | CHECK_KEY_TYPE
                    | CHECK_UNRESOLVED_NAME
                    | crate::rules::CHECK_INVALID_ASSIGN_TARGET
            )
    })
}

/// Whether inferring the delete target as a read already produced a primary
/// diagnostic for it. An unresolved name, an unknown declared type, or an unknown
/// member each yields `Unknown` or `Invalid` here, having already blamed the target;
/// the addressability rejection gates on this so an already-erroring target reports
/// once, not twice. A resolved, well-formed but non-saved target keeps a concrete
/// type and still earns the single addressability error.
fn target_already_blamed(subject_type: &MarrowType) -> bool {
    matches!(subject_type, MarrowType::Invalid | MarrowType::Unknown)
}

fn span_contains(outer: SourceSpan, inner: SourceSpan) -> bool {
    outer.start_byte <= inner.start_byte && inner.end_byte <= outer.end_byte
}

fn has_invalid_assign_target(
    diagnostics: &[CheckDiagnostic],
    file: &Path,
    span: SourceSpan,
) -> bool {
    diagnostics.iter().any(|diagnostic| {
        diagnostic.code == crate::rules::CHECK_INVALID_ASSIGN_TARGET
            && diagnostic.file == file
            && diagnostic.span == span
    })
}
