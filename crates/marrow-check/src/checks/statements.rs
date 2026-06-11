//! The statement/block type driver: a fresh scope frame per block, and the
//! `StatementCheck` dispatch that infers each expression, records bindings, and
//! routes each statement kind to its operator, range, condition, and saved-access
//! checks.

use std::collections::HashMap;
use std::path::Path;

use marrow_syntax::SourceSpan;

use crate::enums::{MatchCheck, check_match, resolve_type};
use crate::infer::{bind, infer_type, local_binding};
use crate::{
    CHECK_COLLECTION_UNSUPPORTED, CHECK_CONDITION_TYPE, CheckDiagnostic, CheckedProgram, MarrowType,
};

use super::collections::{check_for_collection_support, for_frame, is_saved_index_branch_path};
use super::operators::{check_assignment, check_condition, check_return_type, check_throw_type};
use super::ranges::{check_range_header, check_range_iterable_value_parts, check_range_value};

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
    scope.push(HashMap::new());
    for statement in &block.statements {
        check_statement_types(
            program,
            file,
            return_type,
            statement,
            scope,
            aliases,
            diagnostics,
        );
    }
    scope.pop();
}

/// Type-check one statement, recursing into nested blocks and recording the type
/// of any binding it introduces.
pub(crate) fn check_statement_types(
    program: &CheckedProgram,
    file: &Path,
    return_type: &MarrowType,
    statement: &marrow_syntax::Statement,
    scope: &mut Vec<HashMap<String, MarrowType>>,
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    StatementCheck {
        program,
        file,
        return_type,
        scope,
        aliases,
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
            Statement::Throw { value, span } => self.check_throw(value, *span),
            Statement::Expr { value, .. } => {
                self.infer(value);
                check_range_value(self.file, value, self.diagnostics);
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
            Statement::Try {
                body,
                catch,
                finally,
                ..
            } => self.check_try(body, catch.as_ref(), finally.as_ref()),
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
        infer_type(
            self.program,
            expr,
            self.scope,
            self.aliases,
            self.file,
            self.diagnostics,
        )
    }

    fn check_block(&mut self, block: &marrow_syntax::Block) {
        check_block_types(
            self.program,
            self.file,
            self.return_type,
            block,
            self.scope,
            self.aliases,
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
                check_range_value(self.file, value, self.diagnostics);
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
        if let Some((name, ty)) =
            local_binding(self.program, statement, self.scope, self.aliases, self.file)
        {
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
        check_range_value(self.file, value, self.diagnostics);
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
        check_assignment(self.file, span, &target_type, &value_type, self.diagnostics);
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
            check_range_value(self.file, value, self.diagnostics);
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
        check_range_value(self.file, value, self.diagnostics);
        check_throw_type(self.file, span, &value_type, self.diagnostics);
    }

    fn check_condition_expr(&mut self, condition: &marrow_syntax::Expression) {
        check_condition(
            self.program,
            self.file,
            condition,
            self.scope,
            self.aliases,
            self.diagnostics,
        );
        check_range_value(self.file, condition, self.diagnostics);
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
        check_range_value(self.file, value, self.diagnostics);
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
        let read_target = crate::presence::read_target(self.program, &value);
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
            || (read_target.is_some() && !matches!(value_type, MarrowType::Unknown));
        if !is_value_read || read_target.is_none() {
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
        self.infer(iterable);
        check_range_iterable_value_parts(self.file, iterable, self.diagnostics);
        if let Some(step) = step {
            check_range_value(self.file, step, self.diagnostics);
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
        finally: Option<&marrow_syntax::Block>,
    ) {
        self.check_block(body);
        if let Some(clause) = catch {
            let mut frame = HashMap::new();
            frame.insert(clause.name.clone(), MarrowType::Error);
            self.scope.push(frame);
            self.check_block(&clause.block);
            self.scope.pop();
        }
        if let Some(finally) = finally {
            self.check_block(finally);
        }
    }

    fn check_match_statement(
        &mut self,
        scrutinee: Option<&marrow_syntax::Expression>,
        arms: &[marrow_syntax::MatchArm],
        span: SourceSpan,
    ) {
        if let Some(scrutinee) = scrutinee {
            check_range_value(self.file, scrutinee, self.diagnostics);
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
