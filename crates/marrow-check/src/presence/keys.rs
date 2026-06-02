use marrow_syntax::{Argument, Expression, InterpolationPart};

use super::scope::NameScope;
use super::util::extend_unique;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SavedPathParts {
    pub(crate) root: String,
    pub(crate) members: Vec<String>,
    pub(crate) keys: Vec<String>,
    pub(crate) key_bindings: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExprKey {
    pub(super) text: String,
    pub(super) bindings: Vec<u32>,
}

pub(crate) fn saved_path_parts(expr: &Expression, scope: &NameScope) -> Option<SavedPathParts> {
    match expr {
        Expression::SavedRoot { name, .. } => Some(SavedPathParts {
            root: name.clone(),
            members: Vec::new(),
            keys: Vec::new(),
            key_bindings: Vec::new(),
        }),
        Expression::Call { callee, args, .. } => {
            let mut path = saved_path_parts(callee, scope)?;
            for arg in args {
                let key = argument_key(arg, scope);
                path.keys.push(key.text);
                extend_unique(&mut path.key_bindings, key.bindings);
            }
            Some(path)
        }
        Expression::Field { base, name, .. } | Expression::OptionalField { base, name, .. } => {
            let mut path = saved_path_parts(base, scope)?;
            path.members.push(name.clone());
            Some(path)
        }
        Expression::Literal { .. }
        | Expression::Name { .. }
        | Expression::Unary { .. }
        | Expression::Binary { .. }
        | Expression::Interpolation { .. } => None,
    }
}

pub(super) fn binding_key(name: &str, scope: &NameScope) -> Option<ExprKey> {
    let binding = scope.lookup(name)?;
    Some(ExprKey {
        text: format!("binding:{binding}:{name}"),
        bindings: vec![binding],
    })
}

pub(super) fn assigned_bindings(expr: &Expression, scope: &NameScope) -> Vec<u32> {
    expression_key(expr, scope).bindings
}

pub(super) fn argument_key(arg: &Argument, scope: &NameScope) -> ExprKey {
    let mut text = String::new();
    if let Some(mode) = arg.mode {
        text.push_str(match mode {
            marrow_syntax::ArgMode::Out => "out:",
            marrow_syntax::ArgMode::InOut => "inout:",
        });
    }
    if let Some(name) = &arg.name {
        text.push_str(name);
        text.push('=');
    }
    let value = expression_key(&arg.value, scope);
    text.push_str(&value.text);
    ExprKey {
        text,
        bindings: value.bindings,
    }
}

pub(super) fn expression_key(expr: &Expression, scope: &NameScope) -> ExprKey {
    match expr {
        Expression::Literal { kind, text, .. } => ExprKey {
            text: format!("lit:{kind:?}:{text}"),
            bindings: Vec::new(),
        },
        Expression::Name { segments, .. } if segments.len() == 1 => {
            let name = &segments[0];
            match scope.lookup(name) {
                Some(binding) => ExprKey {
                    text: format!("binding:{binding}:{name}"),
                    bindings: vec![binding],
                },
                None => ExprKey {
                    text: format!("name:{name}"),
                    bindings: Vec::new(),
                },
            }
        }
        Expression::Name { segments, .. } => ExprKey {
            text: format!("name:{}", segments.join("::")),
            bindings: Vec::new(),
        },
        Expression::SavedRoot { name, .. } => ExprKey {
            text: format!("root:{name}"),
            bindings: Vec::new(),
        },
        Expression::Call { callee, args, .. } => {
            let callee = expression_key(callee, scope);
            let mut bindings = callee.bindings;
            let mut args_text = Vec::new();
            for arg in args {
                let arg = argument_key(arg, scope);
                args_text.push(arg.text);
                extend_unique(&mut bindings, arg.bindings);
            }
            ExprKey {
                text: format!("call:{}({})", callee.text, args_text.join(",")),
                bindings,
            }
        }
        Expression::Field {
            base, name, quoted, ..
        } => {
            let base = expression_key(base, scope);
            ExprKey {
                text: format!("field:{}:{quoted}:{name}", base.text),
                bindings: base.bindings,
            }
        }
        Expression::OptionalField {
            base, name, quoted, ..
        } => {
            let base = expression_key(base, scope);
            ExprKey {
                text: format!("optional:{}:{quoted}:{name}", base.text),
                bindings: base.bindings,
            }
        }
        Expression::Unary { op, operand, .. } => {
            let operand = expression_key(operand, scope);
            ExprKey {
                text: format!("unary:{op:?}:{}", operand.text),
                bindings: operand.bindings,
            }
        }
        Expression::Binary {
            op, left, right, ..
        } => {
            let left = expression_key(left, scope);
            let right = expression_key(right, scope);
            let mut bindings = left.bindings;
            extend_unique(&mut bindings, right.bindings);
            ExprKey {
                text: format!("binary:{op:?}:{}:{}", left.text, right.text),
                bindings,
            }
        }
        Expression::Interpolation { parts, .. } => {
            let mut bindings = Vec::new();
            let text = parts
                .iter()
                .map(|part| match part {
                    InterpolationPart::Text { text, .. } => format!("text:{text}"),
                    InterpolationPart::Expr(expr) => {
                        let expr = expression_key(expr, scope);
                        extend_unique(&mut bindings, expr.bindings);
                        expr.text
                    }
                })
                .collect::<Vec<_>>()
                .join(",");
            ExprKey {
                text: format!("interp:{text}"),
                bindings,
            }
        }
    }
}
