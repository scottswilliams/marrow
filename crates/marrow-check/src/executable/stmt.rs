use std::collections::HashMap;

use marrow_schema::Type;
use marrow_syntax::{self as syntax, SourceSpan};

use crate::program::{CheckedProgram, MarrowType};
use crate::resolve::{Def, DefItem, Resolution, ResolvableKind, resolve};

use super::expr::{checked_enum_ref, lower_optional_expr};
use super::{
    CheckedElseIf, CheckedEnumRef, CheckedExecutableContext, CheckedExpr, CheckedForBinding,
    CheckedMatchArm, WriteFallibilityFact,
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
            statements.push(CheckedStmt::lower(statement, context, scope)?);
            if let Some((name, ty)) = crate::infer::local_binding(
                context.program,
                statement,
                scope,
                &context.aliases,
                context.source_file,
            ) {
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
        value: CheckedExpr,
        span: SourceSpan,
    },
    Var {
        name: String,
        key_count: usize,
        ty: Option<Type>,
        resource_default: bool,
        value: Option<CheckedExpr>,
        span: SourceSpan,
    },
    Assign {
        target: CheckedExpr,
        value: CheckedExpr,
        fallibility: WriteFallibilityFact,
        span: SourceSpan,
    },
    Delete {
        path: CheckedExpr,
        fallibility: WriteFallibilityFact,
        span: SourceSpan,
    },
    Return {
        value: Option<CheckedExpr>,
        span: SourceSpan,
    },
    ReturnAbsent {
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
    ) -> Option<Self> {
        Self::lower_binding_or_write(statement, context, scope)
            .or_else(|| Self::lower_control(statement, context, scope))
    }

    fn lower_binding_or_write(
        statement: &syntax::Statement,
        context: &CheckedExecutableContext<'_>,
        scope: &mut Vec<HashMap<String, MarrowType>>,
    ) -> Option<Self> {
        Some(match statement {
            syntax::Statement::Const {
                name, value, span, ..
            } => Self::Const {
                name: name.clone(),
                value: CheckedExpr::lower(value, context, scope)?,
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
                resource_default: ty.as_ref().is_some_and(|ty| {
                    let Type::Named(name) = Type::resolve(ty) else {
                        return false;
                    };
                    resolves_resource_type(context.program, context.from_module, &name)
                }),
                value: lower_optional_expr(value.as_ref(), context, scope)?,
                span: *span,
            },
            syntax::Statement::Assign {
                target,
                value,
                span,
            } => {
                let target = CheckedExpr::lower(target, context, scope)?;
                let fallibility = WriteFallibilityFact::for_assignment(target.saved_place());
                Self::Assign {
                    target,
                    value: CheckedExpr::lower(value, context, scope)?,
                    fallibility,
                    span: *span,
                }
            }
            syntax::Statement::Delete { path, span } => {
                let path = CheckedExpr::lower(path, context, scope)?;
                let fallibility = WriteFallibilityFact::for_delete(path.saved_place());
                Self::Delete {
                    path,
                    fallibility,
                    span: *span,
                }
            }
            syntax::Statement::Return { value, span } => Self::Return {
                value: lower_optional_expr(value.as_ref(), context, scope)?,
                span: *span,
            },
            syntax::Statement::ReturnAbsent { span } => Self::ReturnAbsent { span: *span },
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
                condition: lower_optional_expr(condition.as_ref(), context, scope)?,
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
                value,
                then_block,
                else_ifs,
                else_block,
                span,
            } => {
                let value_type = crate::infer::infer_only(
                    context.program,
                    value,
                    scope,
                    &context.aliases,
                    context.source_file,
                );
                Self::IfConst {
                    name: name.clone(),
                    value: CheckedExpr::lower(value, context, scope)?,
                    then_block: lower_scoped_with_binding(
                        then_block,
                        context,
                        scope,
                        name.clone(),
                        value_type,
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
                condition: lower_optional_expr(condition.as_ref(), context, scope)?,
                body: CheckedBody::lower_scoped(body, context, scope)?,
                span: *span,
            },
            syntax::Statement::For {
                binding,
                iterable,
                step,
                body,
                span,
            } => Self::For {
                binding: CheckedForBinding::lower(binding),
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
                let match_enum = infer_match_enum(scrutinee.as_ref(), context, scope);
                let match_enum_ref = match_enum
                    .as_ref()
                    .map(|(module, name)| (module.as_str(), name.as_str()));
                Self::Match {
                    scrutinee: lower_optional_expr(scrutinee.as_ref(), context, scope)?,
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
            | Self::Delete { span, .. }
            | Self::Return { span, .. }
            | Self::ReturnAbsent { span }
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
