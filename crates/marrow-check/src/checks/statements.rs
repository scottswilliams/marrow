//! The statement/block type driver: a fresh scope frame per block, and the
//! `StatementCheck` dispatch that infers each expression, records bindings, and
//! routes each statement kind to its operator, range, condition, and saved-access
//! checks.

use std::collections::HashMap;
use std::path::Path;

use marrow_schema::ReturnPresence;
use marrow_syntax::SourceSpan;

use crate::enums::{MatchCheck, check_match, resolve_diagnosed_annotation_type};
use crate::executable::{SavedPlaceResolver, lower_expr_for_file};
use crate::infer::{
    assignment_target_is_error_code, bind, infer_assignment_target_type_with_read_scope,
    infer_collection_subject_type_with_read_scope, infer_type_with_read_scope,
    local_binding_with_read_scope, reject_saved_access_with_suggested_index,
};
use crate::resolve::resolve_store_by_root;
use crate::{
    CHECK_CALL_ARGUMENT, CHECK_COLLECTION_UNSUPPORTED, CHECK_CONDITION_TYPE, CHECK_KEY_TYPE,
    CHECK_LAYER_NOT_VALUE, CHECK_LOSSY_ROUND_TRIP, CHECK_UNRESOLVED_NAME, CheckDiagnostic,
    CheckedProgram, MarrowType,
};

use super::collections::{
    catch_frame, check_entries_value_position, check_for_collection_support,
    check_for_entries_support, check_for_scalar_iterable, for_frame, is_partial_key_layer_path,
    is_saved_index_branch_path, is_saved_index_range_path, is_saved_key_range_path,
    is_saved_path_with_key_range_arg,
};
use super::operators::{check_assignment, check_condition, check_return_type, check_throw_type};
use super::ranges::{
    check_range_header, check_range_iterable_value_parts, check_range_value_guarded,
};
use super::required_fields::RequiredFieldAssignments;
use super::returns::check_return_values;
use super::saved_keys::saved_root_args_address_record;

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
    let mut required_fields = RequiredFieldAssignments::new();
    let return_type = function
        .return_type
        .as_ref()
        .map_or(MarrowType::Unknown, |ty| {
            resolve_diagnosed_annotation_type(ty, program, aliases, file)
        });
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
        diagnostics,
        &mut required_fields,
    );
}

/// Type-check a block under a fresh scope frame for its `const`/`var` bindings.
pub(crate) fn check_block_types(
    program: &CheckedProgram,
    file: &Path,
    return_type: &MarrowType,
    block: &marrow_syntax::Block,
    scope: &mut Vec<HashMap<String, MarrowType>>,
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let mut required_fields = RequiredFieldAssignments::inactive();
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
        diagnostics,
        &mut required_fields,
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
    check_return_values(file, block, true, ReturnPresence::Always, diagnostics);
    let mut required_fields = RequiredFieldAssignments::inactive();
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
        diagnostics,
        &mut required_fields,
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
    diagnostics: &mut Vec<CheckDiagnostic>,
    required_fields: &mut RequiredFieldAssignments,
) {
    scope.push(HashMap::new());
    required_fields.push_frame();
    for statement in &block.statements {
        check_statement_types(context, statement, scope, diagnostics, required_fields);
    }
    required_fields.pop_frame();
    scope.pop();
}

/// Type-check one statement, recursing into nested blocks and recording the type
/// of any binding it introduces.
fn check_statement_types(
    context: BlockTypeContext<'_>,
    statement: &marrow_syntax::Statement,
    scope: &mut Vec<HashMap<String, MarrowType>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
    required_fields: &mut RequiredFieldAssignments,
) {
    StatementCheck {
        program: context.program,
        file: context.file,
        return_type: context.return_type,
        scope,
        aliases: context.aliases,
        transform_old: context.transform_old,
        diagnostics,
        required_fields,
    }
    .check(statement);
}

struct StatementCheck<'a> {
    program: &'a CheckedProgram,
    file: &'a Path,
    return_type: &'a MarrowType,
    scope: &'a mut Vec<HashMap<String, MarrowType>>,
    aliases: &'a HashMap<String, Vec<String>>,
    transform_old: Option<crate::presence::TransformOldReadScope<'a>>,
    diagnostics: &'a mut Vec<CheckDiagnostic>,
    required_fields: &'a mut RequiredFieldAssignments,
}

impl StatementCheck<'_> {
    fn check(&mut self, statement: &marrow_syntax::Statement) {
        use marrow_syntax::Statement;
        match statement {
            Statement::Const {
                ty, value, span, ..
            } => self.check_binding_statement(statement, ty.as_ref(), Some(value), *span),
            Statement::Var {
                ty, value, span, ..
            } => self.check_binding_statement(statement, ty.as_ref(), value.as_ref(), *span),
            Statement::Assign {
                target,
                value,
                span,
            } => self.check_assignment_statement(target, value, *span),
            Statement::Delete { path, .. } => self.check_delete_statement(path),
            Statement::Return { value, span } => {
                self.check_return(value.as_ref(), *span);
                self.required_fields.invalidate_all();
            }
            Statement::ReturnAbsent { .. } => self.required_fields.invalidate_all(),
            Statement::Throw { value, span } => {
                self.check_throw(value, *span);
                self.required_fields.invalidate_all();
            }
            Statement::Expr { value, .. } => {
                self.infer(value);
                self.check_range_value(value);
            }
            Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => self.check_conditional(
                condition.as_ref(),
                then_block,
                else_ifs,
                else_block.as_ref(),
            ),
            Statement::IfConst { .. } => self.check_if_const(statement),
            Statement::While {
                condition, body, ..
            } => self.check_while(condition.as_ref(), body),
            Statement::For {
                binding,
                iterable,
                step,
                body,
                ..
            } => self.check_for(binding, iterable, step.as_ref(), body),
            Statement::Transaction { body, .. } => {
                self.check_block(body);
            }
            Statement::Try { body, catch, .. } => self.check_try(body, catch.as_ref()),
            Statement::Match {
                scrutinee,
                arms,
                span,
                ..
            } => self.check_match_statement(scrutinee.as_ref(), arms, *span),
            Statement::Break { .. } | Statement::Continue { .. } => {
                self.required_fields.invalidate_all();
            }
        }
    }

    fn infer(&mut self, expr: &marrow_syntax::Expression) -> MarrowType {
        let ty = infer_type_with_read_scope(
            self.program,
            expr,
            self.scope,
            self.aliases,
            self.file,
            self.diagnostics,
            self.transform_old,
        );
        check_entries_value_position(self.file, expr, self.diagnostics);
        ty
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
            self.diagnostics,
            self.required_fields,
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
            self.diagnostics,
            &mut required_fields,
        );
    }

    fn check_binding_statement(
        &mut self,
        statement: &marrow_syntax::Statement,
        annotation: Option<&marrow_syntax::TypeRef>,
        value: Option<&marrow_syntax::Expression>,
        span: SourceSpan,
    ) {
        let value_type = match value {
            Some(value) => {
                let value_type = self.infer(value);
                self.check_range_value(value);
                value_type
            }
            None => MarrowType::Unknown,
        };
        if let (Some(annotation), Some(value)) = (annotation, value) {
            check_assignment(
                self.file,
                span,
                &resolve_diagnosed_annotation_type(
                    annotation,
                    self.program,
                    self.aliases,
                    self.file,
                ),
                &value_type,
                self.diagnostics,
            );
            if marrow_schema::is_error_code_spelling(&annotation.text) {
                super::calls::check_error_code_literal(
                    value,
                    "an `ErrorCode` binding",
                    self.file,
                    self.diagnostics,
                );
            }
        }
        self.bind_local(statement);
    }

    fn bind_local(&mut self, statement: &marrow_syntax::Statement) {
        if let Some((name, ty)) = local_binding_with_read_scope(
            self.program,
            statement,
            self.scope,
            self.aliases,
            self.file,
            self.transform_old,
        ) {
            self.required_fields
                .bind_statement(self.program, statement, &name, &ty);
            bind(self.scope, &name, ty);
        }
    }

    fn check_assignment_statement(
        &mut self,
        target: &marrow_syntax::Expression,
        value: &marrow_syntax::Expression,
        span: SourceSpan,
    ) {
        let target_type = infer_assignment_target_type_with_read_scope(
            self.program,
            target,
            self.scope,
            self.aliases,
            self.file,
            self.diagnostics,
            self.transform_old,
        );
        let value_type = self.infer(value);
        self.check_range_value(value);
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
        }
        if self.is_nested_local_resource_field_write(target)
            && !has_invalid_assign_target(self.diagnostics, self.file, target.span())
        {
            self.diagnostics.push(CheckDiagnostic::error(
                crate::rules::CHECK_INVALID_ASSIGN_TARGET,
                self.file,
                target.span(),
                "nested local resource fields cannot be assigned",
            ));
        }
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
        self.check_lossy_round_trip_warning(target);
        check_assignment(self.file, span, &target_type, &value_type, self.diagnostics);
        if assignment_target_is_error_code(
            self.program,
            target,
            self.scope,
            self.aliases,
            self.file,
            self.transform_old,
        ) {
            super::calls::check_error_code_literal(
                value,
                "an `ErrorCode` field",
                self.file,
                self.diagnostics,
            );
        }
        self.required_fields.assign_target(target);
    }

    fn check_lossy_round_trip_warning(&mut self, target: &marrow_syntax::Expression) {
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
        self.diagnostics.push(CheckDiagnostic::warning(
            CHECK_LOSSY_ROUND_TRIP,
            self.file,
            target.span(),
            "whole saved-record replacement clears keyed child layers omitted from the value",
        ));
    }

    fn is_nested_local_resource_field_write(&self, target: &marrow_syntax::Expression) -> bool {
        let Some(root) = nested_local_field_root(target) else {
            return false;
        };
        self.scope
            .iter()
            .rev()
            .find_map(|frame| frame.get(root))
            .is_some_and(|ty| matches!(ty, MarrowType::Resource(_)))
    }

    fn check_delete_statement(&mut self, path: &marrow_syntax::Expression) {
        // A delete target is an address, not a value read. Inferring it through the
        // collection-subject position surfaces its key-argument and structural
        // diagnostics while leaving the value-read partial-key gate silent, so the
        // dedicated partial-key delete rejection below is the single root cause.
        infer_collection_subject_type_with_read_scope(
            self.program,
            path,
            self.scope,
            self.aliases,
            self.file,
            self.diagnostics,
            self.transform_old,
        );
        check_entries_value_position(self.file, path, self.diagnostics);
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
        }
    }

    fn check_return(&mut self, value: Option<&marrow_syntax::Expression>, span: SourceSpan) {
        if let Some(value) = value {
            let value_type = self.infer(value);
            self.check_range_value(value);
            check_return_type(
                self.file,
                span,
                self.return_type,
                &value_type,
                self.diagnostics,
            );
        }
    }

    fn check_throw(&mut self, value: &marrow_syntax::Expression, span: SourceSpan) {
        let value_type = self.infer(value);
        self.check_range_value(value);
        check_throw_type(self.file, span, &value_type, self.diagnostics);
    }

    fn check_condition_expr(&mut self, condition: &marrow_syntax::Expression) {
        check_condition(
            self.program,
            self.file,
            condition,
            self.scope,
            self.aliases,
            self.transform_old,
            self.diagnostics,
        );
        self.check_range_value(condition);
        check_entries_value_position(self.file, condition, self.diagnostics);
    }

    fn check_conditional(
        &mut self,
        condition: Option<&marrow_syntax::Expression>,
        then_block: &marrow_syntax::Block,
        else_ifs: &[marrow_syntax::ElseIf],
        else_block: Option<&marrow_syntax::Block>,
    ) {
        if let Some(condition) = condition {
            self.check_condition_expr(condition);
        }
        self.check_inconclusive_block(then_block);
        for else_if in else_ifs {
            if let Some(condition) = &else_if.condition {
                self.check_condition_expr(condition);
            }
            self.check_inconclusive_block(&else_if.block);
        }
        if let Some(block) = else_block {
            self.check_inconclusive_block(block);
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
            span,
        } = statement
        else {
            return;
        };
        let annotation = annotation.as_ref();
        let else_block = else_block.as_ref();
        let span = *span;
        let value_type = self.infer(value);
        self.check_range_value(value);
        self.check_if_const_value(value);
        // A written annotation carries the same contract as on `const`/`var`: it
        // must name the saved read's type. An unresolvable name is a
        // `check.unknown_type` and a disagreeing type a `check.assignment_type`;
        // the annotation then types the binding so the then-block sees the written
        // type rather than the read's inferred one.
        let binding_type = match annotation {
            Some(annotation) => {
                let annotated_type = resolve_diagnosed_annotation_type(
                    annotation,
                    self.program,
                    self.aliases,
                    self.file,
                );
                check_assignment(
                    self.file,
                    span,
                    &annotated_type,
                    &value_type,
                    self.diagnostics,
                );
                annotated_type
            }
            None => value_type,
        };
        let mut frame = HashMap::new();
        frame.insert(name.to_string(), binding_type);
        self.scope.push(frame);
        self.check_inconclusive_block(then_block);
        self.scope.pop();
        for else_if in else_ifs {
            if let Some(condition) = &else_if.condition {
                self.check_condition_expr(condition);
            }
            self.check_inconclusive_block(&else_if.block);
        }
        if let Some(block) = else_block {
            self.check_inconclusive_block(block);
        }
        self.required_fields.invalidate_all();
    }

    fn check_if_const_value(&mut self, value: &marrow_syntax::Expression) {
        let Some(value) = lower_expr_for_file(self.program, self.file, value, self.scope) else {
            return;
        };
        if !crate::presence::bindable_saved_value_read_in_type_scope(
            self.program,
            &value,
            self.scope,
            self.transform_old,
        ) {
            // A partial-key composite layer is the precise root cause when it is the
            // subject: the type pass already recorded `check.layer_not_value` on this
            // span, so the generic "requires a saved value read" message would only
            // stack a second diagnostic on the same mistake.
            if has_layer_not_value(self.diagnostics, self.file, value.span()) {
                return;
            }
            self.diagnostics.push(CheckDiagnostic::error(
                CHECK_CONDITION_TYPE,
                self.file,
                value.span(),
                "`if const` requires a saved value read such as `^root(id).field` or `^singleton`",
            ));
        }
    }

    fn check_while(
        &mut self,
        condition: Option<&marrow_syntax::Expression>,
        body: &marrow_syntax::Block,
    ) {
        if let Some(condition) = condition {
            self.check_condition_expr(condition);
        }
        self.check_inconclusive_block(body);
        self.required_fields.invalidate_all();
    }

    fn check_for(
        &mut self,
        binding: &marrow_syntax::ForBinding,
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
            self.scope.push(frame);
            self.check_inconclusive_block(body);
            self.scope.pop();
            self.required_fields.invalidate_all();
            return;
        }
        infer_collection_subject_type_with_read_scope(
            self.program,
            iterable,
            self.scope,
            self.aliases,
            self.file,
            self.diagnostics,
            self.transform_old,
        );
        check_for_entries_support(
            self.program,
            self.file,
            binding,
            iterable,
            self.scope,
            self.aliases,
            self.diagnostics,
        );
        if !is_saved_index_branch_path(self.program, iterable, self.scope, self.file)
            && !is_saved_key_range_path(self.program, iterable, self.scope, self.file)
        {
            check_range_iterable_value_parts(self.file, iterable, self.diagnostics);
        }
        if let Some(step) = step {
            self.check_range_value(step);
            check_entries_value_position(self.file, step, self.diagnostics);
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
        check_for_collection_support(
            self.program,
            self.file,
            binding,
            iterable,
            self.scope,
            self.aliases,
            self.diagnostics,
        );
        check_for_scalar_iterable(
            self.program,
            self.file,
            iterable,
            self.scope,
            self.aliases,
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
        self.scope.push(frame);
        self.check_inconclusive_block(body);
        self.scope.pop();
        self.required_fields.invalidate_all();
    }

    fn check_try(
        &mut self,
        body: &marrow_syntax::Block,
        catch: Option<&marrow_syntax::CatchClause>,
    ) {
        self.check_inconclusive_block(body);
        if let Some(clause) = catch {
            self.scope.push(catch_frame(clause));
            self.check_inconclusive_block(&clause.block);
            self.scope.pop();
        }
        self.required_fields.invalidate_all();
    }

    fn check_match_statement(
        &mut self,
        scrutinee: Option<&marrow_syntax::Expression>,
        arms: &[marrow_syntax::MatchArm],
        span: SourceSpan,
    ) {
        if let Some(scrutinee) = scrutinee {
            check_entries_value_position(self.file, scrutinee, self.diagnostics);
            self.check_range_value(scrutinee);
        }
        check_match(MatchCheck {
            program: self.program,
            file: self.file,
            return_type: self.return_type,
            scrutinee,
            arms,
            span,
            scope: self.scope,
            aliases: self.aliases,
            diagnostics: self.diagnostics,
        });
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
    match name.as_str() {
        "exists" => true,
        "count" => is_saved_index_range_path(program, &arg.value, scope, file),
        _ => false,
    }
}

fn nested_local_field_root(target: &marrow_syntax::Expression) -> Option<&str> {
    let marrow_syntax::Expression::Field { base, .. } = target else {
        return None;
    };
    if matches!(base.as_ref(), marrow_syntax::Expression::Name { .. }) {
        return None;
    }
    local_field_root(base)
}

fn local_field_root(expr: &marrow_syntax::Expression) -> Option<&str> {
    match expr {
        marrow_syntax::Expression::Name { segments, .. } => {
            let [name] = segments.as_slice() else {
                return None;
            };
            Some(name)
        }
        marrow_syntax::Expression::Field { base, .. } => local_field_root(base),
        marrow_syntax::Expression::Call { callee, .. } => match callee.as_ref() {
            marrow_syntax::Expression::Field { .. } => local_field_root(callee),
            _ => None,
        },
        _ => None,
    }
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
            if saved_root_args_address_record(store.store, args, &arg_types) {
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
                check.transform_old,
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

fn has_layer_not_value(diagnostics: &[CheckDiagnostic], file: &Path, span: SourceSpan) -> bool {
    diagnostics.iter().any(|diagnostic| {
        diagnostic.code == CHECK_LAYER_NOT_VALUE
            && diagnostic.file == file
            && diagnostic.span == span
    })
}
