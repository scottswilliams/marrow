//! Module-level constants.
//!
//! A top-level `const NAME [: type] = <literal>` binds a compile-time scalar value
//! that is module-private: it is referenced by name only from within its own
//! module, and folded into the image as a constant load at each use. The compiled
//! subset restricts a constant's value to a scalar literal (optionally a negated
//! integer literal); richer constant expressions land in a later lane.

use std::collections::BTreeMap;

use marrow_codes::Code;
use marrow_project::FileIdentity;
use marrow_syntax::{ConstDecl, Expression, LiteralKind, TypeExpr, UnaryOp, decode_string_literal};

use crate::diag::SourceDiagnostic;
use crate::lower::parse_int;
use crate::scalar::ScalarType;
use crate::types::TypeRegistry;

/// A folded module-constant value: one scalar literal.
#[derive(Debug, Clone)]
pub(crate) enum ConstScalar {
    Int(i64),
    Bool(bool),
    Text(String),
}

impl ConstScalar {
    /// The language scalar type of this constant.
    pub(crate) fn scalar(&self) -> ScalarType {
        match self {
            ConstScalar::Int(_) => ScalarType::Int,
            ConstScalar::Bool(_) => ScalarType::Bool,
            ConstScalar::Text(_) => ScalarType::Text,
        }
    }
}

/// The module-private constants of a project: `module -> [(name, value)]`.
#[derive(Default)]
pub(crate) struct ConstRegistry {
    entries: BTreeMap<String, Vec<(String, ConstScalar)>>,
}

impl ConstRegistry {
    /// The constant named `name` visible in `module`, if any.
    pub(crate) fn get(&self, module: &str, name: &str) -> Option<&ConstScalar> {
        self.entries
            .get(module)?
            .iter()
            .find(|(entry_name, _)| entry_name == name)
            .map(|(_, value)| value)
    }

    /// Evaluate every module constant to its folded scalar value, reporting a typed
    /// diagnostic for a non-literal value, a type-annotation mismatch, or a
    /// duplicate name within one module. `consts` pairs each declaration with its
    /// dotted module and its file identity (for the diagnostic span).
    pub(crate) fn build(
        consts: &[(String, FileIdentity, &ConstDecl)],
        types: &TypeRegistry,
        diagnostics: &mut Vec<SourceDiagnostic>,
    ) -> Self {
        let mut entries: BTreeMap<String, Vec<(String, ConstScalar)>> = BTreeMap::new();
        for (module, file, decl) in consts {
            if crate::lower::is_reserved_builtin_name(&decl.name) {
                diagnostics.push(crate::lower::reserved_builtin_name(
                    file, decl.span, &decl.name,
                ));
                continue;
            }
            let module_entries = entries.entry(module.clone()).or_default();
            if module_entries.iter().any(|(name, _)| name == &decl.name) {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckNameConflict.as_str(),
                    file,
                    decl.span,
                    format!(
                        "a constant named `{}` is already declared in this module",
                        decl.name
                    ),
                ));
                continue;
            }
            let Some(value) = evaluate(file, decl, types, diagnostics) else {
                continue;
            };
            module_entries.push((decl.name.clone(), value));
        }
        Self { entries }
    }
}

/// Evaluate one constant declaration to its folded value, checking a type
/// annotation when present.
fn evaluate(
    file: &FileIdentity,
    decl: &ConstDecl,
    types: &TypeRegistry,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<ConstScalar> {
    let Some(expression) = &decl.value else {
        diagnostics.push(unsupported(file, decl, "a constant without a value"));
        return None;
    };
    let value = literal_value(file, expression, diagnostics)?;
    if let Some(annotation) = &decl.ty {
        let declared = match types.expand(annotation) {
            TypeExpr::Name { text, .. } => ScalarType::from_spelling(&text),
            _ => None,
        };
        match declared {
            Some(scalar) if scalar == value.scalar() => {}
            Some(scalar) => {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    file,
                    decl.span,
                    format!(
                        "constant `{}` is declared `{}` but its value is `{}`",
                        decl.name,
                        scalar.spelling(),
                        value.scalar().spelling()
                    ),
                ));
                return None;
            }
            None => {
                diagnostics.push(unsupported(file, decl, "this constant type"));
                return None;
            }
        }
    }
    Some(value)
}

/// Fold a scalar literal (or a negated integer literal) to its value.
fn literal_value(
    file: &FileIdentity,
    expression: &Expression,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<ConstScalar> {
    match expression {
        Expression::Literal { kind, text, span } => match kind {
            LiteralKind::Integer => match parse_int(text) {
                Some(value) => Some(ConstScalar::Int(value)),
                None => {
                    diagnostics.push(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        file,
                        *span,
                        "integer literal is out of the 64-bit range".to_string(),
                    ));
                    None
                }
            },
            LiteralKind::Bool => Some(ConstScalar::Bool(text == "true")),
            LiteralKind::String => match decode_string_literal(text) {
                Ok(decoded) => Some(ConstScalar::Text(decoded)),
                Err(_) => {
                    diagnostics.push(SourceDiagnostic::at(
                        Code::CheckUnsupported.as_str(),
                        file,
                        *span,
                        "this string literal is not yet supported on the beta line".to_string(),
                    ));
                    None
                }
            },
            _ => {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckUnsupported.as_str(),
                    file,
                    *span,
                    "this literal is not yet supported in a constant".to_string(),
                ));
                None
            }
        },
        // A negated integer literal is the one non-atomic constant form the subset
        // folds, so `const MIN = -1` is expressible.
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            span,
        } => match literal_value(file, operand, diagnostics)? {
            ConstScalar::Int(value) => value.checked_neg().map(ConstScalar::Int).or_else(|| {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    file,
                    *span,
                    "integer literal is out of the 64-bit range".to_string(),
                ));
                None
            }),
            other => {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    file,
                    *span,
                    format!("cannot negate a `{}` constant", other.scalar().spelling()),
                ));
                None
            }
        },
        other => {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckUnsupported.as_str(),
                file,
                other.span(),
                "a constant must be a scalar literal on the beta line".to_string(),
            ));
            None
        }
    }
}

fn unsupported(file: &FileIdentity, decl: &ConstDecl, subject: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        decl.span,
        format!("{subject} is not yet supported on the beta line"),
    )
}
