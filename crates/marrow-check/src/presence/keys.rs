use super::scope::NameScope;
use super::util::extend_unique;
use crate::{CheckedArg, CheckedArgMode, CheckedExpr, CheckedInterpolationPart};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SavedPathParts {
    pub(super) root: String,
    pub(super) members: Vec<String>,
    pub(super) keys: Vec<String>,
    pub(super) key_bindings: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExprKey {
    pub(super) text: String,
    pub(super) bindings: Vec<u32>,
}

pub(super) fn saved_path_parts(expr: &CheckedExpr, scope: &NameScope) -> Option<SavedPathParts> {
    match expr {
        CheckedExpr::SavedRoot { name, .. } => Some(SavedPathParts {
            root: name.clone(),
            members: Vec::new(),
            keys: Vec::new(),
            key_bindings: Vec::new(),
        }),
        CheckedExpr::Call { callee, args, .. } => {
            let mut path = saved_path_parts(callee, scope)?;
            for arg in args {
                let key = argument_key(arg, scope);
                path.keys.push(key.text);
                extend_unique(&mut path.key_bindings, key.bindings);
            }
            Some(path)
        }
        CheckedExpr::Field { base, name, .. } | CheckedExpr::OptionalField { base, name, .. } => {
            let mut path = saved_path_parts(base, scope)?;
            path.members.push(name.clone());
            Some(path)
        }
        CheckedExpr::Literal { .. }
        | CheckedExpr::Name { .. }
        | CheckedExpr::Unary { .. }
        | CheckedExpr::Binary { .. }
        | CheckedExpr::Interpolation { .. } => None,
    }
}

pub(super) fn binding_key(name: &str, scope: &NameScope) -> Option<ExprKey> {
    let binding = scope.lookup(name)?;
    Some(ExprKey {
        text: format!("binding:{binding}:{name}"),
        bindings: vec![binding],
    })
}

pub(super) fn assigned_bindings(expr: &CheckedExpr, scope: &NameScope) -> Vec<u32> {
    expression_key(expr, scope).bindings
}

pub(super) fn argument_key(arg: &CheckedArg, scope: &NameScope) -> ExprKey {
    let mut text = String::new();
    if let Some(mode) = arg.mode {
        text.push_str(match mode {
            CheckedArgMode::Out => "out:",
            CheckedArgMode::InOut => "inout:",
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

pub(super) fn expression_key(expr: &CheckedExpr, scope: &NameScope) -> ExprKey {
    match expr {
        CheckedExpr::Literal { kind, text, .. } => ExprKey {
            text: format!("lit:{kind:?}:{text}"),
            bindings: Vec::new(),
        },
        CheckedExpr::Name { segments, .. } if segments.len() == 1 => {
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
        CheckedExpr::Name { segments, .. } => ExprKey {
            text: format!("name:{}", segments.join("::")),
            bindings: Vec::new(),
        },
        CheckedExpr::SavedRoot { name, .. } => ExprKey {
            text: format!("root:{name}"),
            bindings: Vec::new(),
        },
        CheckedExpr::Call { callee, args, .. } => {
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
        CheckedExpr::Field {
            base, name, quoted, ..
        } => {
            let base = expression_key(base, scope);
            ExprKey {
                text: format!("field:{}:{quoted}:{name}", base.text),
                bindings: base.bindings,
            }
        }
        CheckedExpr::OptionalField {
            base, name, quoted, ..
        } => {
            let base = expression_key(base, scope);
            ExprKey {
                text: format!("optional:{}:{quoted}:{name}", base.text),
                bindings: base.bindings,
            }
        }
        CheckedExpr::Unary { op, operand, .. } => {
            let operand = expression_key(operand, scope);
            ExprKey {
                text: format!("unary:{op:?}:{}", operand.text),
                bindings: operand.bindings,
            }
        }
        CheckedExpr::Binary {
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
        CheckedExpr::Interpolation { parts, .. } => {
            let mut bindings = Vec::new();
            let text = parts
                .iter()
                .map(|part| match part {
                    CheckedInterpolationPart::Text { text, .. } => format!("text:{text}"),
                    CheckedInterpolationPart::Expr(expr) => {
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
