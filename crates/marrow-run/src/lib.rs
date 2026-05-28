//! The Marrow runtime: evaluate checked `.mw` functions.
//!
//! This first slice evaluates a pure function body over scalar values: integer
//! and boolean literals, locals (`let`/`var`), arithmetic, comparison, and
//! logical operators, and conditionals. Saved data, loops, string values,
//! structured errors, and calls between functions build on this spine.

use marrow_syntax::{
    BinaryOp, Block, Expression, FunctionDecl, LiteralKind, SourceSpan, Statement, UnaryOp,
};

/// A runtime value. This slice models the scalar shapes a pure function needs;
/// saved trees, identities, and error values arrive with the features that
/// produce them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
}

/// A runtime fault: a stable `run.*` code, a human-readable message, and the
/// source span of the construct that raised it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub code: &'static str,
    pub message: String,
    pub span: SourceSpan,
}

/// A value was used where another type was required (e.g. `+` on a non-integer,
/// a non-boolean condition, or assigning to an immutable binding).
pub const RUN_TYPE: &str = "run.type";
/// A name was read or assigned that is not bound in scope.
pub const RUN_UNBOUND_NAME: &str = "run.unbound_name";
/// Integer arithmetic overflowed the 64-bit range.
pub const RUN_OVERFLOW: &str = "run.overflow";
/// Integer division or remainder by zero.
pub const RUN_DIVIDE_BY_ZERO: &str = "run.divide_by_zero";
/// A construct this slice of the runtime does not yet evaluate.
pub const RUN_UNSUPPORTED: &str = "run.unsupported";

/// Evaluate `function` with positional `args`, returning its returned value, or
/// `None` if it returns without one. Parameters bind to `args` by position.
pub fn evaluate_function(
    function: &FunctionDecl,
    args: &[Value],
) -> Result<Option<Value>, RuntimeError> {
    if args.len() != function.params.len() {
        return Err(RuntimeError {
            code: RUN_TYPE,
            message: format!(
                "function `{}` expects {} argument(s), got {}",
                function.name,
                function.params.len(),
                args.len()
            ),
            span: function.span,
        });
    }
    let mut env = Env::new();
    env.push_scope();
    for (param, arg) in function.params.iter().zip(args) {
        env.bind(param.name.clone(), arg.clone(), false);
    }
    let flow = eval_block(&function.body, &mut env)?;
    env.pop_scope();
    Ok(match flow {
        Flow::Return(value) => value,
        Flow::Normal => None,
    })
}

/// Where control flow stands after a statement or block.
enum Flow {
    /// Fall through to the next statement.
    Normal,
    /// A `return`, carrying its value if it had one.
    Return(Option<Value>),
}

/// A name binding: its value and whether it may be reassigned (`var` vs `let`).
struct Binding {
    value: Value,
    mutable: bool,
}

/// A lexical environment: a stack of scopes, each a list of bindings. A resource
/// has few locals, so lookups are linear and innermost-first.
struct Env {
    scopes: Vec<Vec<(String, Binding)>>,
}

/// Why an assignment did not land.
enum AssignError {
    Unbound,
    Immutable,
}

impl Env {
    fn new() -> Self {
        Self { scopes: Vec::new() }
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Bind `name` in the innermost scope, shadowing any binding further out.
    fn bind(&mut self, name: String, value: Value, mutable: bool) {
        self.scopes
            .last_mut()
            .expect("a scope is open")
            .push((name, Binding { value, mutable }));
    }

    /// The value bound to `name`, searching innermost scope first.
    fn lookup(&self, name: &str) -> Option<&Value> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|scope| scope.iter().rev())
            .find(|(bound, _)| bound == name)
            .map(|(_, binding)| &binding.value)
    }

    /// Reassign an existing mutable binding.
    fn assign(&mut self, name: &str, value: Value) -> Result<(), AssignError> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some((_, binding)) = scope.iter_mut().rev().find(|(bound, _)| bound == name) {
                if !binding.mutable {
                    return Err(AssignError::Immutable);
                }
                binding.value = value;
                return Ok(());
            }
        }
        Err(AssignError::Unbound)
    }
}

/// Evaluate a block in its own scope, stopping at the first `return`. The scope
/// is popped on every exit, including when a statement raises an error, so the
/// environment is left balanced for reuse.
fn eval_block(block: &Block, env: &mut Env) -> Result<Flow, RuntimeError> {
    env.push_scope();
    let result = eval_statements(&block.statements, env);
    env.pop_scope();
    result
}

/// Evaluate statements in order until one returns or the block ends.
fn eval_statements(statements: &[Statement], env: &mut Env) -> Result<Flow, RuntimeError> {
    for statement in statements {
        let flow = eval_statement(statement, env)?;
        if !matches!(flow, Flow::Normal) {
            return Ok(flow);
        }
    }
    Ok(Flow::Normal)
}

fn eval_statement(statement: &Statement, env: &mut Env) -> Result<Flow, RuntimeError> {
    match statement {
        Statement::Let { name, value, .. } => {
            let value = eval_expr(value, env)?;
            env.bind(name.clone(), value, false);
            Ok(Flow::Normal)
        }
        Statement::Var {
            name,
            keys,
            value,
            span,
            ..
        } => {
            if !keys.is_empty() {
                return Err(unsupported("a keyed local variable", *span));
            }
            let value = value
                .as_ref()
                .ok_or_else(|| unsupported("an uninitialized variable", *span))?;
            let value = eval_expr(value, env)?;
            env.bind(name.clone(), value, true);
            Ok(Flow::Normal)
        }
        Statement::Assign {
            target,
            value,
            span,
        } => {
            let name = local_target(target, *span)?;
            let value = eval_expr(value, env)?;
            env.assign(name, value).map_err(|error| match error {
                AssignError::Immutable => RuntimeError {
                    code: RUN_TYPE,
                    message: format!("cannot assign to immutable `{name}`"),
                    span: *span,
                },
                AssignError::Unbound => RuntimeError {
                    code: RUN_UNBOUND_NAME,
                    message: format!("`{name}` is not bound"),
                    span: *span,
                },
            })?;
            Ok(Flow::Normal)
        }
        Statement::Return { value, .. } => {
            let value = value
                .as_ref()
                .map(|expr| eval_expr(expr, env))
                .transpose()?;
            Ok(Flow::Return(value))
        }
        Statement::Expr { value, .. } => {
            eval_expr(value, env)?;
            Ok(Flow::Normal)
        }
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            if eval_bool(condition, env)? {
                return eval_block(then_block, env);
            }
            for else_if in else_ifs {
                if eval_bool(&else_if.condition, env)? {
                    return eval_block(&else_if.block, env);
                }
            }
            match else_block {
                Some(block) => eval_block(block, env),
                None => Ok(Flow::Normal),
            }
        }
        other => Err(unsupported("this statement", other.span())),
    }
}

/// The single local name an assignment targets, or an "unsupported" error for a
/// saved path or qualified name (those arrive with later slices).
fn local_target(target: &Expression, span: SourceSpan) -> Result<&str, RuntimeError> {
    match target {
        Expression::Name { segments, .. } if segments.len() == 1 => Ok(&segments[0]),
        _ => Err(unsupported("assignment to this target", span)),
    }
}

fn eval_expr(expr: &Expression, env: &mut Env) -> Result<Value, RuntimeError> {
    match expr {
        Expression::Literal { kind, text, span } => eval_literal(*kind, text, *span),
        Expression::Name { segments, span } => {
            if segments.len() != 1 {
                return Err(unsupported("a qualified name", *span));
            }
            env.lookup(&segments[0])
                .cloned()
                .ok_or_else(|| RuntimeError {
                    code: RUN_UNBOUND_NAME,
                    message: format!("`{}` is not bound", segments[0]),
                    span: *span,
                })
        }
        Expression::Unary { op, operand, span } => eval_unary(*op, operand, *span, env),
        Expression::Binary {
            op,
            left,
            right,
            span,
        } => eval_binary(*op, left, right, *span, env),
        other => Err(unsupported("this expression", other.span())),
    }
}

fn eval_literal(kind: LiteralKind, text: &str, span: SourceSpan) -> Result<Value, RuntimeError> {
    match kind {
        LiteralKind::Integer => text
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| RuntimeError {
                code: RUN_OVERFLOW,
                message: format!("integer literal `{text}` is out of range"),
                span,
            }),
        LiteralKind::Bool => Ok(Value::Bool(text == "true")),
        LiteralKind::String | LiteralKind::Decimal | LiteralKind::Bytes => {
            Err(unsupported("this literal type", span))
        }
    }
}

fn eval_unary(
    op: UnaryOp,
    operand: &Expression,
    span: SourceSpan,
    env: &mut Env,
) -> Result<Value, RuntimeError> {
    match (op, eval_expr(operand, env)?) {
        (UnaryOp::Neg, Value::Int(n)) => n
            .checked_neg()
            .map(Value::Int)
            .ok_or_else(|| overflow(span)),
        (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (UnaryOp::Neg, _) => Err(type_error("negation expects an integer", span)),
        (UnaryOp::Not, _) => Err(type_error("`not` expects a boolean", span)),
    }
}

fn eval_binary(
    op: BinaryOp,
    left: &Expression,
    right: &Expression,
    span: SourceSpan,
    env: &mut Env,
) -> Result<Value, RuntimeError> {
    match op {
        // Logical operators short-circuit: the right side is evaluated only when
        // the left does not already decide the result.
        BinaryOp::And => Ok(Value::Bool(eval_bool(left, env)? && eval_bool(right, env)?)),
        BinaryOp::Or => Ok(Value::Bool(eval_bool(left, env)? || eval_bool(right, env)?)),
        BinaryOp::Add => int_op(left, right, env, span, i64::checked_add),
        BinaryOp::Subtract => int_op(left, right, env, span, i64::checked_sub),
        BinaryOp::Multiply => int_op(left, right, env, span, i64::checked_mul),
        BinaryOp::Divide => int_div(left, right, env, span, i64::checked_div),
        BinaryOp::Remainder => int_div(left, right, env, span, i64::checked_rem),
        BinaryOp::Less => int_cmp(left, right, env, |a, b| a < b),
        BinaryOp::LessEqual => int_cmp(left, right, env, |a, b| a <= b),
        BinaryOp::Greater => int_cmp(left, right, env, |a, b| a > b),
        BinaryOp::GreaterEqual => int_cmp(left, right, env, |a, b| a >= b),
        BinaryOp::Equal => Ok(Value::Bool(values_equal(left, right, env, span)?)),
        BinaryOp::NotEqual => Ok(Value::Bool(!values_equal(left, right, env, span)?)),
        BinaryOp::Concat | BinaryOp::RangeExclusive | BinaryOp::RangeInclusive => {
            Err(unsupported("this operator", span))
        }
    }
}

/// Apply a checked integer operation, mapping `None` (overflow) to `run.overflow`.
fn int_op(
    left: &Expression,
    right: &Expression,
    env: &mut Env,
    span: SourceSpan,
    op: fn(i64, i64) -> Option<i64>,
) -> Result<Value, RuntimeError> {
    let a = eval_int(left, env)?;
    let b = eval_int(right, env)?;
    op(a, b).map(Value::Int).ok_or_else(|| overflow(span))
}

/// Apply a checked division/remainder, rejecting a zero divisor and the
/// `i64::MIN / -1` overflow.
fn int_div(
    left: &Expression,
    right: &Expression,
    env: &mut Env,
    span: SourceSpan,
    op: fn(i64, i64) -> Option<i64>,
) -> Result<Value, RuntimeError> {
    let a = eval_int(left, env)?;
    let b = eval_int(right, env)?;
    if b == 0 {
        return Err(RuntimeError {
            code: RUN_DIVIDE_BY_ZERO,
            message: "integer division or remainder by zero".into(),
            span,
        });
    }
    op(a, b).map(Value::Int).ok_or_else(|| overflow(span))
}

fn int_cmp(
    left: &Expression,
    right: &Expression,
    env: &mut Env,
    op: fn(i64, i64) -> bool,
) -> Result<Value, RuntimeError> {
    let a = eval_int(left, env)?;
    let b = eval_int(right, env)?;
    Ok(Value::Bool(op(a, b)))
}

/// Whether two values are equal. They must share a scalar type; comparing across
/// types is a runtime type error (the checker rejects it statically).
fn values_equal(
    left: &Expression,
    right: &Expression,
    env: &mut Env,
    span: SourceSpan,
) -> Result<bool, RuntimeError> {
    match (eval_expr(left, env)?, eval_expr(right, env)?) {
        (Value::Int(a), Value::Int(b)) => Ok(a == b),
        (Value::Bool(a), Value::Bool(b)) => Ok(a == b),
        _ => Err(type_error("cannot compare values of different types", span)),
    }
}

fn eval_int(expr: &Expression, env: &mut Env) -> Result<i64, RuntimeError> {
    match eval_expr(expr, env)? {
        Value::Int(n) => Ok(n),
        _ => Err(type_error("expected an integer", expr.span())),
    }
}

fn eval_bool(expr: &Expression, env: &mut Env) -> Result<bool, RuntimeError> {
    match eval_expr(expr, env)? {
        Value::Bool(b) => Ok(b),
        _ => Err(type_error("expected a boolean", expr.span())),
    }
}

fn unsupported(what: &str, span: SourceSpan) -> RuntimeError {
    RuntimeError {
        code: RUN_UNSUPPORTED,
        message: format!("the runtime does not yet evaluate {what}"),
        span,
    }
}

fn type_error(message: &str, span: SourceSpan) -> RuntimeError {
    RuntimeError {
        code: RUN_TYPE,
        message: message.to_string(),
        span,
    }
}

fn overflow(span: SourceSpan) -> RuntimeError {
    RuntimeError {
        code: RUN_OVERFLOW,
        message: "integer arithmetic overflowed".into(),
        span,
    }
}
