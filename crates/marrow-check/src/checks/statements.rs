//! The statement/block type driver: a fresh scope frame per block, and the
//! `StatementCheck` dispatch that infers each expression, records bindings, and
//! routes each statement kind to its operator, range, condition, and saved-access
//! checks.

use std::collections::HashMap;
use std::path::Path;

use marrow_syntax::SourceSpan;

use crate::enums::{MatchCheck, check_match, resolve_type};
use crate::infer::{bind, infer_type_with_read_scope, local_binding_with_read_scope};
use crate::resolve::resolve_store_by_root;
use crate::walk::for_each_child_expr;
use crate::{
    CHECK_COLLECTION_UNSUPPORTED, CHECK_CONDITION_TYPE, CHECK_LOSSY_ROUND_TRIP, CHECK_RANGE_VALUE,
    CheckDiagnostic, CheckedProgram, MarrowType,
};

use super::collections::{
    check_entries_value_position, check_for_collection_support, check_for_entries_support,
    for_frame, is_saved_index_branch_path, is_saved_index_range_path, is_saved_key_range_path,
};
use super::operators::{check_assignment, check_condition, check_return_type, check_throw_type};
use super::ranges::{check_range_header, check_range_iterable_value_parts};
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
            resolve_type(&param.ty, program, aliases, file),
        );
    }
    let mut scope: Vec<HashMap<String, MarrowType>> = vec![base];
    let return_type = function
        .return_type
        .as_ref()
        .map_or(MarrowType::Unknown, |ty| {
            resolve_type(ty, program, aliases, file)
        });
    check_block_types(
        program,
        file,
        &return_type,
        &function.body,
        &mut scope,
        aliases,
        diagnostics,
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
) {
    scope.push(HashMap::new());
    for statement in &block.statements {
        check_statement_types(context, statement, scope, diagnostics);
    }
    scope.pop();
}

/// Type-check one statement, recursing into nested blocks and recording the type
/// of any binding it introduces.
fn check_statement_types(
    context: BlockTypeContext<'_>,
    statement: &marrow_syntax::Statement,
    scope: &mut Vec<HashMap<String, MarrowType>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    StatementCheck {
        program: context.program,
        file: context.file,
        return_type: context.return_type,
        scope,
        aliases: context.aliases,
        transform_old: context.transform_old,
        diagnostics,
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
            Statement::Return { value, span } => self.check_return(value.as_ref(), *span),
            Statement::ReturnAbsent { .. } => {}
            Statement::Throw { value, span } => self.check_throw(value, *span),
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
            Statement::IfConst {
                name,
                value,
                then_block,
                else_ifs,
                else_block,
                ..
            } => self.check_if_const(name, value, then_block, else_ifs, else_block.as_ref()),
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
            Statement::Break { .. } | Statement::Continue { .. } => {}
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
        if let (Some(annotation), Some(_)) = (annotation, value) {
            check_assignment(
                self.file,
                span,
                &resolve_type(annotation, self.program, self.aliases, self.file),
                &value_type,
                self.diagnostics,
            );
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
            bind(self.scope, &name, ty);
        }
    }

    fn check_assignment_statement(
        &mut self,
        target: &marrow_syntax::Expression,
        value: &marrow_syntax::Expression,
        span: SourceSpan,
    ) {
        let target_type = self.infer(target);
        let value_type = self.infer(value);
        self.check_range_value(value);
        if is_saved_index_branch_path(self.program, target) {
            self.diagnostics.push(CheckDiagnostic::error(
                crate::rules::CHECK_INVALID_ASSIGN_TARGET,
                self.file,
                target.span(),
                "generated index branches cannot be assigned",
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
        self.check_lossy_round_trip_warning(target);
        check_assignment(self.file, span, &target_type, &value_type, self.diagnostics);
    }

    fn check_lossy_round_trip_warning(&mut self, target: &marrow_syntax::Expression) {
        let Some(members) = saved_root_replacement_members(RootReplacementCheck {
            program: self.program,
            target,
            scope: self.scope,
            aliases: self.aliases,
            file: self.file,
            transform_old: self.transform_old,
        }) else {
            return;
        };
        if !members_contain_keyed_layer(members) {
            return;
        }
        self.diagnostics.push(CheckDiagnostic::warning(
            CHECK_LOSSY_ROUND_TRIP,
            self.file,
            target.span(),
            "whole saved-root replacement clears keyed child layers omitted from the value",
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
        self.infer(path);
        if is_saved_index_branch_path(self.program, path) {
            self.diagnostics.push(CheckDiagnostic::error(
                CHECK_COLLECTION_UNSUPPORTED,
                self.file,
                path.span(),
                "generated index branches cannot be deleted",
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
        self.check_block(then_block);
        for else_if in else_ifs {
            if let Some(condition) = &else_if.condition {
                self.check_condition_expr(condition);
            }
            self.check_block(&else_if.block);
        }
        if let Some(block) = else_block {
            self.check_block(block);
        }
    }

    fn check_if_const(
        &mut self,
        name: &str,
        value: &marrow_syntax::Expression,
        then_block: &marrow_syntax::Block,
        else_ifs: &[marrow_syntax::ElseIf],
        else_block: Option<&marrow_syntax::Block>,
    ) {
        let value_type = self.infer(value);
        self.check_range_value(value);
        self.check_if_const_value(value, &value_type);
        let mut frame = HashMap::new();
        frame.insert(name.to_string(), value_type);
        self.scope.push(frame);
        self.check_block(then_block);
        self.scope.pop();
        for else_if in else_ifs {
            if let Some(condition) = &else_if.condition {
                self.check_condition_expr(condition);
            }
            self.check_block(&else_if.block);
        }
        if let Some(block) = else_block {
            self.check_block(block);
        }
    }

    fn check_if_const_value(&mut self, value: &marrow_syntax::Expression, value_type: &MarrowType) {
        let Some(module_index) = self
            .program
            .modules
            .iter()
            .position(|module| module.source_file == self.file)
        else {
            return;
        };
        let context = crate::executable::CheckedExecutableContext::new(self.program, module_index);
        let mut lower_scope = self.scope.clone();
        let Some(value) = crate::CheckedExpr::lower(value, &context, &mut lower_scope) else {
            return;
        };
        let read_resolves = crate::presence::read_resolves_in_type_scope(
            self.program,
            &value,
            self.scope,
            self.transform_old,
        );
        let is_value_read = value
            .saved_place()
            .is_some_and(|place| match &place.terminal {
                crate::CheckedSavedTerminal::Record => {
                    let root_is_addressed =
                        place.identity_keys.is_empty() || !place.identity_args.is_empty();
                    let layers_are_addressed = place
                        .layers
                        .iter()
                        .all(|layer| layer.key_params.is_empty() || !layer.args.is_empty());
                    root_is_addressed && layers_are_addressed
                }
                crate::CheckedSavedTerminal::Field { .. } => true,
                crate::CheckedSavedTerminal::Index {
                    unique,
                    arg_count,
                    args,
                    ..
                } => *unique && args.len() == *arg_count,
            })
            || fixed_singleton_root(self.program, &value)
            || (read_resolves && !matches!(value_type, MarrowType::Unknown));
        if !is_value_read || !read_resolves {
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
        self.check_block(body);
    }

    fn check_for(
        &mut self,
        binding: &marrow_syntax::ForBinding,
        iterable: &marrow_syntax::Expression,
        step: Option<&marrow_syntax::Expression>,
        body: &marrow_syntax::Block,
    ) {
        infer_type_with_read_scope(
            self.program,
            iterable,
            self.scope,
            self.aliases,
            self.file,
            self.diagnostics,
            self.transform_old,
        );
        check_for_entries_support(self.file, binding, iterable, self.diagnostics);
        if !is_saved_index_branch_path(self.program, iterable)
            && !is_saved_key_range_path(self.program, iterable)
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
        check_for_collection_support(self.program, self.file, binding, iterable, self.diagnostics);
        let frame = for_frame(
            self.program,
            binding,
            iterable,
            self.scope,
            self.aliases,
            self.file,
        );
        self.scope.push(frame);
        self.check_block(body);
        self.scope.pop();
    }

    fn check_try(
        &mut self,
        body: &marrow_syntax::Block,
        catch: Option<&marrow_syntax::CatchClause>,
    ) {
        self.check_block(body);
        if let Some(clause) = catch {
            let mut frame = HashMap::new();
            frame.insert(clause.name.clone(), MarrowType::Error);
            self.scope.push(frame);
            self.check_block(&clause.block);
            self.scope.pop();
        }
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
    }

    fn check_range_value(&mut self, value: &marrow_syntax::Expression) {
        if allowed_saved_key_range_value_context(self.program, value) {
            return;
        }
        if let Some(range) = marrow_syntax::range_expr(value) {
            self.diagnostics.push(CheckDiagnostic::error(
                CHECK_RANGE_VALUE,
                self.file,
                range.span,
                "a range can only be used as a `for` iterable",
            ));
        }
        for_each_child_expr(value, |child| self.check_range_value(child));
    }
}

fn allowed_saved_key_range_value_context(
    program: &CheckedProgram,
    value: &marrow_syntax::Expression,
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
    if arg.name.is_some() || !is_saved_key_range_path(program, &arg.value) {
        return false;
    }
    match name.as_str() {
        "exists" => true,
        "count" => is_saved_index_range_path(program, &arg.value),
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

fn fixed_singleton_root(program: &CheckedProgram, expr: &crate::CheckedExpr) -> bool {
    let crate::CheckedExpr::SavedRoot { name, .. } = expr else {
        return false;
    };
    crate::resolve::resolve_store_by_root(program, name)
        .is_some_and(|store| store.store.identity_keys.is_empty())
}

struct RootReplacementCheck<'a> {
    program: &'a CheckedProgram,
    target: &'a marrow_syntax::Expression,
    scope: &'a [HashMap<String, MarrowType>],
    aliases: &'a HashMap<String, Vec<String>>,
    file: &'a Path,
    transform_old: Option<crate::presence::TransformOldReadScope<'a>>,
}

fn saved_root_replacement_members<'a>(
    check: RootReplacementCheck<'a>,
) -> Option<&'a [marrow_schema::Node]> {
    match check.target {
        marrow_syntax::Expression::SavedRoot { name, .. } => {
            let store = resolve_store_by_root(check.program, name)?;
            store
                .store
                .identity_keys
                .is_empty()
                .then_some(store.resource.members.as_slice())
        }
        marrow_syntax::Expression::Call { callee, args, .. } => {
            let marrow_syntax::Expression::SavedRoot { name, .. } = callee.as_ref() else {
                return None;
            };
            let store = resolve_store_by_root(check.program, name)?;
            let arg_types = saved_root_replacement_arg_types(&check, args);
            if saved_root_args_address_record(store.store, args, &arg_types) {
                return Some(store.resource.members.as_slice());
            }
            None
        }
        _ => None,
    }
}

fn saved_root_replacement_arg_types(
    check: &RootReplacementCheck<'_>,
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

fn members_contain_keyed_layer(members: &[marrow_schema::Node]) -> bool {
    members
        .iter()
        .any(|member| !member.key_params.is_empty() || members_contain_keyed_layer(&member.members))
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
