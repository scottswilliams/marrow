use std::collections::HashMap;

use marrow_schema::Type;
use marrow_syntax::{self as syntax, SourceSpan};

use crate::program::{CheckedProgram, MarrowType};
use crate::resolve::{Def, DefItem, Resolution, ResolvableKind, resolve};

use super::expr::{checked_enum_ref, lower_optional_expr};
use super::{
    CheckedElseIf, CheckedEnumRef, CheckedExecutableContext, CheckedExpr, CheckedForBinding,
    CheckedMatchArm,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedBody {
    statements: Vec<CheckedStmt>,
    span: SourceSpan,
}

impl CheckedBody {
    pub(crate) fn lower(
        block: &syntax::Block,
        context: &CheckedExecutableContext<'_>,
        mut scope: Vec<HashMap<String, MarrowType>>,
    ) -> Option<Self> {
        Self::lower_scoped(block, context, &mut scope)
    }

    pub(super) fn lower_scoped(
        block: &syntax::Block,
        context: &CheckedExecutableContext<'_>,
        scope: &mut Vec<HashMap<String, MarrowType>>,
    ) -> Option<Self> {
        let mut statements = Vec::new();
        scope.push(HashMap::new());
        for statement in &block.statements {
            let local_binding = crate::infer::local_binding(
                context.program,
                statement,
                scope,
                &context.aliases,
                context.source_file,
            );
            let binding_type = local_binding.as_ref().map(|(_, ty)| ty.clone());
            statements.push(CheckedStmt::lower(statement, context, scope, binding_type)?);
            if let Some((name, ty)) = local_binding {
                scope.last_mut()?.insert(name, ty);
            }
        }
        scope.pop();
        Some(Self {
            statements,
            span: block.span,
        })
    }

    pub fn statements(&self) -> &[CheckedStmt] {
        &self.statements
    }

    pub fn span(&self) -> SourceSpan {
        self.span
    }
}

fn lower_scoped_with_binding(
    block: &syntax::Block,
    context: &CheckedExecutableContext<'_>,
    scope: &mut Vec<HashMap<String, MarrowType>>,
    name: String,
    ty: MarrowType,
) -> Option<CheckedBody> {
    let mut frame = HashMap::new();
    frame.insert(name, ty);
    scope.push(frame);
    let body = CheckedBody::lower_scoped(block, context, scope);
    scope.pop();
    body
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedStmt {
    Const {
        name: String,
        binding_type: Option<MarrowType>,
        value: CheckedExpr,
        /// The binding is declared `ErrorCode`, so a dynamic value the runtime
        /// stores into it must satisfy the dotted-lowercase grammar.
        coerce_error_code: bool,
        span: SourceSpan,
    },
    Var {
        name: String,
        key_count: usize,
        ty: Option<Type>,
        binding_type: Option<MarrowType>,
        resource_default: bool,
        value: Option<CheckedExpr>,
        coerce_error_code: bool,
        span: SourceSpan,
    },
    Assign {
        target: CheckedExpr,
        value: CheckedExpr,
        /// The assignment target is an `ErrorCode` field, so a dynamic value the
        /// runtime writes into it must satisfy the dotted-lowercase grammar.
        coerce_error_code: bool,
        span: SourceSpan,
    },
    CompoundAssign {
        target: CheckedExpr,
        op: super::CheckedBinaryOp,
        value: CheckedExpr,
        coerce_error_code: bool,
        span: SourceSpan,
    },
    Delete {
        path: CheckedExpr,
        span: SourceSpan,
    },
    Return {
        value: Option<CheckedExpr>,
        span: SourceSpan,
    },
    Break {
        span: SourceSpan,
    },
    Continue {
        span: SourceSpan,
    },
    Throw {
        value: CheckedExpr,
        span: SourceSpan,
    },
    Expr {
        value: CheckedExpr,
        span: SourceSpan,
    },
    If {
        condition: Option<CheckedExpr>,
        then_block: CheckedBody,
        else_ifs: Vec<CheckedElseIf>,
        else_block: Option<CheckedBody>,
        span: SourceSpan,
    },
    IfConst {
        name: String,
        binding_type: Option<MarrowType>,
        value: CheckedExpr,
        then_block: CheckedBody,
        else_ifs: Vec<CheckedElseIf>,
        else_block: Option<CheckedBody>,
        span: SourceSpan,
    },
    While {
        condition: Option<CheckedExpr>,
        body: CheckedBody,
        span: SourceSpan,
    },
    For {
        binding: CheckedForBinding,
        order: marrow_syntax::LoopOrder,
        iterable: CheckedExpr,
        step: Option<CheckedExpr>,
        body: CheckedBody,
        span: SourceSpan,
    },
    Transaction {
        body: CheckedBody,
        span: SourceSpan,
    },
    Try {
        body: CheckedBody,
        catch: Option<super::CheckedCatchClause>,
        span: SourceSpan,
    },
    Match {
        scrutinee: Option<CheckedExpr>,
        arms: Vec<CheckedMatchArm>,
        enum_ref: Option<CheckedEnumRef>,
        span: SourceSpan,
    },
}

impl CheckedStmt {
    fn lower(
        statement: &syntax::Statement,
        context: &CheckedExecutableContext<'_>,
        scope: &mut Vec<HashMap<String, MarrowType>>,
        binding_type: Option<MarrowType>,
    ) -> Option<Self> {
        Self::lower_binding_or_write(statement, context, scope, binding_type)
            .or_else(|| Self::lower_control(statement, context, scope))
    }

    fn lower_binding_or_write(
        statement: &syntax::Statement,
        context: &CheckedExecutableContext<'_>,
        scope: &[HashMap<String, MarrowType>],
        binding_type: Option<MarrowType>,
    ) -> Option<Self> {
        Some(match statement {
            syntax::Statement::Const {
                name,
                ty,
                value,
                span,
            } => Self::Const {
                name: name.clone(),
                binding_type,
                value: CheckedExpr::lower(value, context, scope)?,
                coerce_error_code: annotation_is_error_code(ty.as_ref()),
                span: *span,
            },
            syntax::Statement::Var {
                name,
                keys,
                ty,
                value,
                span,
            } => Self::Var {
                name: name.clone(),
                key_count: keys.len(),
                ty: ty.as_ref().map(Type::resolve),
                binding_type,
                resource_default: ty.as_ref().is_some_and(|ty| {
                    let Type::Named(name) = Type::resolve(ty) else {
                        return false;
                    };
                    resolves_resource_type(context.program, context.from_module, &name)
                }),
                value: lower_optional_expr(value.as_ref(), context, scope)?,
                coerce_error_code: annotation_is_error_code(ty.as_ref()),
                span: *span,
            },
            syntax::Statement::Assign {
                target,
                value,
                span,
            } => Self::Assign {
                target: CheckedExpr::lower(target, context, scope)?,
                value: CheckedExpr::lower(value, context, scope)?,
                coerce_error_code: crate::infer::assignment_target_is_error_code(
                    context.program,
                    target,
                    scope,
                    &context.aliases,
                    context.source_file,
                    crate::presence::ReadScope::none(),
                ),
                span: *span,
            },
            syntax::Statement::CompoundAssign {
                target,
                op,
                value,
                span,
                ..
            } => Self::CompoundAssign {
                target: CheckedExpr::lower(target, context, scope)?,
                op: super::CheckedBinaryOp::lower(op.binary()),
                value: CheckedExpr::lower(value, context, scope)?,
                coerce_error_code: crate::infer::assignment_target_is_error_code(
                    context.program,
                    target,
                    scope,
                    &context.aliases,
                    context.source_file,
                    crate::presence::ReadScope::none(),
                ),
                span: *span,
            },
            syntax::Statement::Delete { path, span } => Self::Delete {
                path: CheckedExpr::lower(path, context, scope)?,
                span: *span,
            },
            syntax::Statement::Return { value, span } => Self::Return {
                value: lower_optional_expr(value.as_ref(), context, scope)?,
                span: *span,
            },
            syntax::Statement::Break { span } => Self::Break { span: *span },
            syntax::Statement::Continue { span } => Self::Continue { span: *span },
            syntax::Statement::Throw { value, span } => Self::Throw {
                value: CheckedExpr::lower(value, context, scope)?,
                span: *span,
            },
            syntax::Statement::Expr { value, span } => Self::Expr {
                value: CheckedExpr::lower(value, context, scope)?,
                span: *span,
            },
            _ => return None,
        })
    }

    fn lower_control(
        statement: &syntax::Statement,
        context: &CheckedExecutableContext<'_>,
        scope: &mut Vec<HashMap<String, MarrowType>>,
    ) -> Option<Self> {
        Self::lower_branch_control(statement, context, scope)
            .or_else(|| Self::lower_loop_control(statement, context, scope))
            .or_else(|| Self::lower_effect_control(statement, context, scope))
            .or_else(|| Self::lower_match_control(statement, context, scope))
    }

    fn lower_branch_control(
        statement: &syntax::Statement,
        context: &CheckedExecutableContext<'_>,
        scope: &mut Vec<HashMap<String, MarrowType>>,
    ) -> Option<Self> {
        Some(match statement {
            syntax::Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                span,
            } => Self::If {
                condition: lower_optional_expr(Some(condition), context, scope)?,
                then_block: CheckedBody::lower_scoped(then_block, context, scope)?,
                else_ifs: else_ifs
                    .iter()
                    .map(|else_if| CheckedElseIf::lower(else_if, context, scope))
                    .collect::<Option<Vec<_>>>()?,
                else_block: match else_block {
                    Some(block) => Some(CheckedBody::lower_scoped(block, context, scope)?),
                    None => None,
                },
                span: *span,
            },
            syntax::Statement::IfConst {
                name,
                ty,
                value,
                then_block,
                else_ifs,
                else_block,
                span,
            } => {
                // `if const` binds the present arm, so the name takes the subject's type
                // with one optional layer stripped. A written annotation already names
                // that present type (like the type on `const`/`var`); without one the
                // binding is the inferred subject type, `without_optional`, matching the
                // check pass.
                let binding_type = match ty {
                    Some(ty) => crate::enums::resolve_diagnosed_annotation_type(
                        ty,
                        context.program,
                        &context.aliases,
                        context.source_file,
                    ),
                    None => crate::infer::infer_only(
                        context.program,
                        value,
                        scope,
                        &context.aliases,
                        context.source_file,
                    )
                    .without_optional(),
                };
                Self::IfConst {
                    name: name.clone(),
                    binding_type: Some(binding_type.clone()),
                    value: CheckedExpr::lower(value, context, scope)?,
                    then_block: lower_scoped_with_binding(
                        then_block,
                        context,
                        scope,
                        name.clone(),
                        binding_type,
                    )?,
                    else_ifs: else_ifs
                        .iter()
                        .map(|else_if| CheckedElseIf::lower(else_if, context, scope))
                        .collect::<Option<Vec<_>>>()?,
                    else_block: match else_block {
                        Some(block) => Some(CheckedBody::lower_scoped(block, context, scope)?),
                        None => None,
                    },
                    span: *span,
                }
            }
            _ => return None,
        })
    }

    fn lower_loop_control(
        statement: &syntax::Statement,
        context: &CheckedExecutableContext<'_>,
        scope: &mut Vec<HashMap<String, MarrowType>>,
    ) -> Option<Self> {
        Some(match statement {
            syntax::Statement::While {
                condition,
                body,
                span,
            } => Self::While {
                condition: lower_optional_expr(Some(condition), context, scope)?,
                body: CheckedBody::lower_scoped(body, context, scope)?,
                span: *span,
            },
            syntax::Statement::For {
                binding,
                order,
                iterable,
                step,
                body,
                span,
            } => Self::For {
                binding: CheckedForBinding::lower(binding),
                order: *order,
                iterable: CheckedExpr::lower(iterable, context, scope)?,
                step: lower_optional_expr(step.as_ref(), context, scope)?,
                body: {
                    let mut body_scope = scope.clone();
                    body_scope.push(crate::checks::for_frame(
                        context.program,
                        binding,
                        iterable,
                        scope,
                        &context.aliases,
                        context.source_file,
                    ));
                    CheckedBody::lower_scoped(body, context, &mut body_scope)?
                },
                span: *span,
            },
            _ => return None,
        })
    }

    fn lower_effect_control(
        statement: &syntax::Statement,
        context: &CheckedExecutableContext<'_>,
        scope: &mut Vec<HashMap<String, MarrowType>>,
    ) -> Option<Self> {
        Some(match statement {
            syntax::Statement::Transaction { body, span } => Self::Transaction {
                body: CheckedBody::lower_scoped(body, context, scope)?,
                span: *span,
            },
            syntax::Statement::Try { body, catch, span } => Self::Try {
                body: CheckedBody::lower_scoped(body, context, scope)?,
                catch: match catch {
                    Some(catch) => Some(super::CheckedCatchClause::lower(catch, context, scope)?),
                    None => None,
                },
                span: *span,
            },
            _ => return None,
        })
    }

    fn lower_match_control(
        statement: &syntax::Statement,
        context: &CheckedExecutableContext<'_>,
        scope: &mut Vec<HashMap<String, MarrowType>>,
    ) -> Option<Self> {
        Some(match statement {
            syntax::Statement::Match {
                scrutinee,
                arms,
                span,
                ..
            } => {
                let match_enum = infer_match_enum(Some(scrutinee), context, scope);
                let match_enum_ref = match_enum
                    .as_ref()
                    .map(|(module, name)| (module.as_str(), name.as_str()));
                Self::Match {
                    scrutinee: lower_optional_expr(Some(scrutinee), context, scope)?,
                    arms: arms
                        .iter()
                        .map(|arm| CheckedMatchArm::lower(arm, match_enum_ref, context, scope))
                        .collect::<Option<Vec<_>>>()?,
                    enum_ref: match_enum_ref
                        .and_then(|(module, name)| checked_enum_ref(context.program, module, name)),
                    span: *span,
                }
            }
            _ => return None,
        })
    }

    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Const { span, .. }
            | Self::Var { span, .. }
            | Self::Assign { span, .. }
            | Self::CompoundAssign { span, .. }
            | Self::Delete { span, .. }
            | Self::Return { span, .. }
            | Self::Break { span, .. }
            | Self::Continue { span, .. }
            | Self::Throw { span, .. }
            | Self::Expr { span, .. }
            | Self::If { span, .. }
            | Self::IfConst { span, .. }
            | Self::While { span, .. }
            | Self::For { span, .. }
            | Self::Transaction { span, .. }
            | Self::Try { span, .. }
            | Self::Match { span, .. } => *span,
        }
    }
}

fn infer_match_enum(
    scrutinee: Option<&syntax::Expression>,
    context: &CheckedExecutableContext<'_>,
    scope: &[HashMap<String, MarrowType>],
) -> Option<(String, String)> {
    let MarrowType::Enum { module, name } = crate::infer::infer_only(
        context.program,
        scrutinee?,
        scope,
        &context.aliases,
        context.source_file,
    ) else {
        return None;
    };
    Some((module, name))
}

fn resolves_resource_type(program: &CheckedProgram, from_module: &str, name: &str) -> bool {
    let segments = crate::split_type_path(name);
    matches!(
        resolve(program, from_module, &segments, ResolvableKind::Resource),
        Resolution::Found(Def {
            item: DefItem::Resource(_),
            ..
        })
    )
}

/// Whether a binding annotation declares `ErrorCode`, so the runtime validates a
/// dynamic value the binding stores. A string literal is already rejected at check.
fn annotation_is_error_code(ty: Option<&syntax::TypeExpr>) -> bool {
    ty.is_some_and(marrow_schema::is_error_code_annotation)
}
