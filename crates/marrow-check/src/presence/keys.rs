//! Canonical narrowing keys for presence proofs.
//!
//! A presence proof compares read targets for identity: two reads narrow the same
//! place when their key arguments are equal. The key is a span-stripped canonical
//! form of a [`CheckedExpr`] — `CheckedExpr` derives `Eq`, but two textually equal
//! reads carry different spans, so a structural comparison would treat them as
//! distinct. [`expression_key`] is the *sole* owner of that canonical form: every
//! variant maps to a tagged string (`lit:`, `binding:`, `call:`, `field:`,
//! `interp:`, …) that ignores spans and resolves a single-segment name to its
//! scope binding id, so the key is stable across read sites and a rebinding
//! invalidates dependent narrowings. No other layer reproduces this formatting;
//! `target.rs`, `effects.rs`, and the persisted `PresenceProofFact.keys` consume
//! the strings this module produces, they do not build their own.

use super::scope::NameScope;
use super::util::extend_unique;
use crate::{
    CheckedArg, CheckedCallTarget, CheckedExpr, CheckedInterpolationPart, CheckedSavedTerminal,
    MarrowType,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SavedPlaceKey {
    pub(super) root: String,
    pub(super) members: Vec<String>,
    pub(super) keys: Vec<String>,
    pub(super) key_types: Vec<Option<MarrowType>>,
    pub(super) key_bindings: Vec<u32>,
}

/// A canonical narrowing key (`text`) plus the scope binding ids it reads
/// (`bindings`), so reassigning any of those bindings can invalidate a narrowing
/// keyed on this expression. See the module docs for the canonical-form contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExprKey {
    pub(super) text: String,
    pub(super) ty: Option<MarrowType>,
    pub(super) bindings: Vec<u32>,
}

pub(super) fn saved_place_key(expr: &CheckedExpr, scope: &NameScope) -> Option<SavedPlaceKey> {
    let place = expr.saved_place()?;
    let mut path = SavedPlaceKey {
        root: place.root.clone(),
        members: place
            .layers
            .iter()
            .map(|layer| layer.name.clone())
            .collect(),
        keys: Vec::new(),
        key_types: Vec::new(),
        key_bindings: Vec::new(),
    };
    match &place.terminal {
        CheckedSavedTerminal::Record => {}
        CheckedSavedTerminal::Field { name, .. } | CheckedSavedTerminal::Index { name, .. } => {
            path.members.push(name.clone());
        }
    }
    append_args_to_key(&mut path, &place.identity_args, scope);
    for layer in &place.layers {
        append_args_to_key(&mut path, &layer.args, scope);
    }
    if let CheckedSavedTerminal::Index { args, .. } = &place.terminal {
        append_args_to_key(&mut path, args, scope);
    }
    Some(path)
}

fn append_args_to_key(path: &mut SavedPlaceKey, args: &[CheckedArg], scope: &NameScope) {
    for arg in args {
        let key = argument_key(arg, scope);
        path.keys.push(key.text);
        path.key_types.push(key.ty);
        extend_unique(&mut path.key_bindings, key.bindings);
    }
}

pub(super) fn binding_key(name: &str, scope: &NameScope) -> Option<ExprKey> {
    scoped_binding_key(name, scope)
}

fn scoped_binding_key(name: &str, scope: &NameScope) -> Option<ExprKey> {
    let binding = scope.lookup(name)?;
    Some(ExprKey {
        text: format!("binding:{binding}:{name}"),
        ty: scope.lookup_type(name).cloned(),
        bindings: vec![binding],
    })
}

pub(super) fn assigned_bindings(expr: &CheckedExpr, scope: &NameScope) -> Vec<u32> {
    expression_key(expr, scope).bindings
}

pub(super) fn argument_key(arg: &CheckedArg, scope: &NameScope) -> ExprKey {
    let mut text = String::new();
    if let Some(name) = &arg.name {
        text.push_str(name);
        text.push('=');
    }
    let value = expression_key(&arg.value, scope);
    text.push_str(&value.text);
    ExprKey {
        text,
        ty: value.ty,
        bindings: value.bindings,
    }
}

pub(super) fn expression_key(expr: &CheckedExpr, scope: &NameScope) -> ExprKey {
    match expr {
        CheckedExpr::Literal { kind, text, .. } => ExprKey {
            text: format!("lit:{kind:?}:{text}"),
            ty: None,
            bindings: Vec::new(),
        },
        CheckedExpr::Name { segments, .. } if segments.len() == 1 => {
            let name = &segments[0];
            scoped_binding_key(name, scope).unwrap_or_else(|| ExprKey {
                text: format!("name:{name}"),
                ty: None,
                bindings: Vec::new(),
            })
        }
        CheckedExpr::Name { segments, .. } => ExprKey {
            text: format!("name:{}", segments.join("::")),
            ty: None,
            bindings: Vec::new(),
        },
        CheckedExpr::SavedRoot { name, .. } => ExprKey {
            text: format!("root:{name}"),
            ty: None,
            bindings: Vec::new(),
        },
        CheckedExpr::Call {
            callee,
            args,
            target,
            ..
        } => {
            let callee = expression_key(callee, scope);
            let mut bindings = callee.bindings;
            let mut args_text = Vec::new();
            for arg in args {
                let arg = argument_key(arg, scope);
                args_text.push(arg.text);
                extend_unique(&mut bindings, arg.bindings);
            }
            let ty = match target {
                CheckedCallTarget::IdentityConstructor(constructor) => {
                    Some(MarrowType::Identity(constructor.root.clone()))
                }
                _ => None,
            };
            ExprKey {
                text: format!("call:{}({})", callee.text, args_text.join(",")),
                ty,
                bindings,
            }
        }
        CheckedExpr::Field {
            base, name, quoted, ..
        } => {
            let base = expression_key(base, scope);
            ExprKey {
                text: format!("field:{}:{quoted}:{name}", base.text),
                ty: None,
                bindings: base.bindings,
            }
        }
        CheckedExpr::OptionalField {
            base, name, quoted, ..
        } => {
            let base = expression_key(base, scope);
            ExprKey {
                text: format!("optional:{}:{quoted}:{name}", base.text),
                ty: None,
                bindings: base.bindings,
            }
        }
        CheckedExpr::Unary { op, operand, .. } => {
            let operand = expression_key(operand, scope);
            ExprKey {
                text: format!("unary:{op:?}:{}", operand.text),
                ty: None,
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
                ty: None,
                bindings,
            }
        }
        CheckedExpr::Range {
            start,
            end,
            inclusive_end,
            step,
            ..
        } => {
            let mut bindings = Vec::new();
            let mut parts = Vec::new();
            for part in [start.as_deref(), end.as_deref(), step.as_deref()] {
                if let Some(part) = part {
                    let key = expression_key(part, scope);
                    extend_unique(&mut bindings, key.bindings);
                    parts.push(key.text);
                } else {
                    parts.push(String::new());
                }
            }
            ExprKey {
                text: format!("range:{inclusive_end}:{}", parts.join(":")),
                ty: None,
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
                ty: None,
                bindings,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use marrow_syntax::SourceSpan;

    fn root_expr() -> CheckedExpr {
        CheckedExpr::SavedRoot {
            name: "books".to_string(),
            place: None,
            span: SourceSpan::default(),
        }
    }

    fn field_expr(quoted: bool) -> CheckedExpr {
        CheckedExpr::Field {
            base: Box::new(root_expr()),
            name: "title".to_string(),
            quoted,
            place: None,
            span: SourceSpan::default(),
        }
    }

    fn optional_field_expr(quoted: bool) -> CheckedExpr {
        CheckedExpr::OptionalField {
            base: Box::new(root_expr()),
            name: "title".to_string(),
            quoted,
            place: None,
            span: SourceSpan::default(),
        }
    }

    #[test]
    fn field_expression_keys_preserve_the_quoted_bit() {
        let scope = NameScope::default();

        assert_eq!(
            expression_key(&field_expr(false), &scope).text,
            "field:root:books:false:title"
        );
        assert_eq!(
            expression_key(&field_expr(true), &scope).text,
            "field:root:books:true:title"
        );
    }

    #[test]
    fn optional_field_expression_keys_preserve_the_quoted_bit() {
        let scope = NameScope::default();

        assert_eq!(
            expression_key(&optional_field_expr(false), &scope).text,
            "optional:root:books:false:title"
        );
        assert_eq!(
            expression_key(&optional_field_expr(true), &scope).text,
            "optional:root:books:true:title"
        );
    }
}
