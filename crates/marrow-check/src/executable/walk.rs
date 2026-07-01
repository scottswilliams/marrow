use super::{
    CheckedArg, CheckedBody, CheckedExpr, CheckedInterpolationPart, CheckedMatchArm, CheckedStmt,
};

pub(crate) trait CheckedBodyVisitor {
    fn visit_stmt(&mut self, statement: &CheckedStmt) {
        walk_checked_stmt(self, statement);
    }

    fn visit_expr(&mut self, expression: &CheckedExpr) {
        walk_checked_expr(self, expression);
    }

    fn visit_match_arm(&mut self, arm: &CheckedMatchArm) {
        walk_checked_match_arm(self, arm);
    }
}

pub(crate) fn walk_checked_body<V>(visitor: &mut V, body: &CheckedBody)
where
    V: CheckedBodyVisitor + ?Sized,
{
    for statement in body.statements() {
        visitor.visit_stmt(statement);
    }
}

pub(crate) fn walk_checked_stmt<V>(visitor: &mut V, statement: &CheckedStmt)
where
    V: CheckedBodyVisitor + ?Sized,
{
    match statement {
        CheckedStmt::Const { value, .. }
        | CheckedStmt::Expr { value, .. }
        | CheckedStmt::Throw { value, .. } => visitor.visit_expr(value),
        CheckedStmt::Var { value, .. } | CheckedStmt::Return { value, .. } => {
            if let Some(value) = value {
                visitor.visit_expr(value);
            }
        }
        CheckedStmt::Assign { target, value, .. }
        | CheckedStmt::CompoundAssign { target, value, .. } => {
            visitor.visit_expr(target);
            visitor.visit_expr(value);
        }
        CheckedStmt::Delete { path, .. } => visitor.visit_expr(path),
        CheckedStmt::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if let Some(condition) = condition {
                visitor.visit_expr(condition);
            }
            walk_checked_body(visitor, then_block);
            for else_if in else_ifs {
                if let Some(condition) = &else_if.condition {
                    visitor.visit_expr(condition);
                }
                walk_checked_body(visitor, &else_if.block);
            }
            if let Some(else_block) = else_block {
                walk_checked_body(visitor, else_block);
            }
        }
        CheckedStmt::IfConst {
            value,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            visitor.visit_expr(value);
            walk_checked_body(visitor, then_block);
            for else_if in else_ifs {
                if let Some(condition) = &else_if.condition {
                    visitor.visit_expr(condition);
                }
                walk_checked_body(visitor, &else_if.block);
            }
            if let Some(else_block) = else_block {
                walk_checked_body(visitor, else_block);
            }
        }
        CheckedStmt::While {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                visitor.visit_expr(condition);
            }
            walk_checked_body(visitor, body);
        }
        CheckedStmt::For {
            iterable,
            step,
            body,
            ..
        } => {
            visitor.visit_expr(iterable);
            if let Some(step) = step {
                visitor.visit_expr(step);
            }
            walk_checked_body(visitor, body);
        }
        CheckedStmt::Transaction { body, .. } => walk_checked_body(visitor, body),
        CheckedStmt::Try { body, catch, .. } => {
            walk_checked_body(visitor, body);
            if let Some(catch) = catch {
                walk_checked_body(visitor, &catch.block);
            }
        }
        CheckedStmt::Match {
            scrutinee, arms, ..
        } => {
            if let Some(scrutinee) = scrutinee {
                visitor.visit_expr(scrutinee);
            }
            for arm in arms {
                visitor.visit_match_arm(arm);
            }
        }
        CheckedStmt::Break { .. } | CheckedStmt::Continue { .. } => {}
    }
}

pub(crate) fn walk_checked_expr<V>(visitor: &mut V, expression: &CheckedExpr)
where
    V: CheckedBodyVisitor + ?Sized,
{
    match expression {
        CheckedExpr::Literal { .. }
        | CheckedExpr::Name { .. }
        | CheckedExpr::SavedRoot { .. }
        | CheckedExpr::Absent { .. } => {}
        CheckedExpr::Call { callee, args, .. } => {
            visitor.visit_expr(callee);
            for arg in args {
                walk_checked_arg(visitor, arg);
            }
        }
        CheckedExpr::Field { base, .. } | CheckedExpr::OptionalField { base, .. } => {
            visitor.visit_expr(base);
        }
        CheckedExpr::Unary { operand, .. } => visitor.visit_expr(operand),
        CheckedExpr::Binary { left, right, .. } => {
            visitor.visit_expr(left);
            visitor.visit_expr(right);
        }
        CheckedExpr::Range {
            start, end, step, ..
        } => {
            if let Some(start) = start {
                visitor.visit_expr(start);
            }
            if let Some(end) = end {
                visitor.visit_expr(end);
            }
            if let Some(step) = step {
                visitor.visit_expr(step);
            }
        }
        CheckedExpr::Interpolation { parts, .. } => {
            for part in parts {
                if let CheckedInterpolationPart::Expr(expr) = part {
                    visitor.visit_expr(expr);
                }
            }
        }
    }
}

pub(crate) fn walk_checked_match_arm<V>(visitor: &mut V, arm: &CheckedMatchArm)
where
    V: CheckedBodyVisitor + ?Sized,
{
    walk_checked_body(visitor, &arm.block);
}

fn walk_checked_arg<V>(visitor: &mut V, arg: &CheckedArg)
where
    V: CheckedBodyVisitor + ?Sized,
{
    visitor.visit_expr(&arg.value);
}
