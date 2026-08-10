//! Module-level constants.
//!
//! A top-level `const NAME [: type] = <literal>` binds a compile-time scalar value
//! that is module-private: it is referenced by name only from within its own
//! module, and folded into the image as a constant load at each use. The compiled
//! subset restricts a constant's value to a scalar literal (optionally a negated
//! integer literal); richer constant expressions land in a later lane.

use marrow_codes::Code;
use marrow_project::FileIdentity;
use marrow_syntax::{
    ConstDecl, Expression, LiteralKind, SourceSpan, TypeExpr, UnaryOp, decode_string_literal,
};

use crate::analysis::FileRef;
use crate::decl::{
    Binding, DeclarationLedger, DeclarationLedgerFull, DeclarationNamespace, DeclarationOccurrence,
    DeclarationRefusalSummary, Declared, refuse, refuse_row,
};
use crate::diag::{DiagnosticCollector, SourceDiagnostic};
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

/// A constant's key: module-private, so the same name in two modules is two
/// constants.
type ConstKey = (String, String);

/// The module-private constants of a project, accepted and refused alike.
///
/// A refused constant stays in the ledger under its own name, so a later use is
/// steered to the cause the declaration already reported instead of being told the
/// name does not exist.
pub(crate) struct ConstRegistry {
    entries: DeclarationLedger<ConstKey, ConstScalar>,
}

impl Default for ConstRegistry {
    fn default() -> Self {
        Self {
            entries: DeclarationLedger::new(DeclarationNamespace::Constant),
        }
    }
}

impl ConstRegistry {
    /// What the constant named `name` binds in `module`: its folded value, the
    /// refusal that stands in its place, or a genuine absence.
    pub(crate) fn lookup(&self, module: &str, name: &str) -> Binding<'_, ConstScalar> {
        self.entries.lookup(&(module.to_string(), name.to_string()))
    }

    /// Every constant name declared in `module`, refused ones included — the
    /// did-you-mean corpus, so a near miss on a refused name still suggests it.
    pub(crate) fn names_in(&self, module: &str) -> impl Iterator<Item = &str> {
        self.entries
            .keys()
            .filter(move |(declared, _)| declared == module)
            .map(|(_, name)| name.as_str())
    }

    /// Evaluate every module constant to its folded scalar value, reporting a typed
    /// diagnostic for a non-literal value, a type-annotation mismatch, or a
    /// duplicate name within one module, and retaining every refused name.
    ///
    /// `consts` pairs each declaration with its dotted module and its file in both
    /// representations: the owned identity a diagnostic renders and the `Copy`
    /// coordinate the ledger retains.
    pub(crate) fn build(
        consts: &[(String, FileRef, FileIdentity, &ConstDecl)],
        types: &TypeRegistry,
        diagnostics: &mut DiagnosticCollector,
    ) -> Result<Self, DeclarationLedgerFull> {
        let mut entries: DeclarationLedger<ConstKey, ConstScalar> =
            DeclarationLedger::new(DeclarationNamespace::Constant);
        for (module, at, file, decl) in consts {
            let key = (module.clone(), decl.name.clone());
            let declared = Declared {
                name: &decl.name,
                file,
                at: *at,
                span: decl.span,
            };
            // A reserved built-in name is refused before evaluation: the constant
            // would be shadowed by the compiler's own intercept and never reached.
            if crate::lower::is_reserved_builtin_name(&decl.name) {
                let refusal = refuse_row(
                    diagnostics,
                    declared,
                    crate::lower::reserved_builtin_name(file, decl.span, &decl.name),
                );
                entries.declare(key, DeclarationOccurrence::Refused(refusal))?;
                continue;
            }
            // A refused declaration still occupies its name, so the redeclaration
            // conflicts whichever of the two the compiler could evaluate.
            if entries.declared(&key) {
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
            let occurrence = match evaluate(declared, decl, types, diagnostics) {
                Ok(value) => DeclarationOccurrence::Accepted(value),
                Err(refusal) => DeclarationOccurrence::Refused(refusal),
            };
            entries.declare(key, occurrence)?;
        }
        Ok(Self { entries })
    }
}

/// Evaluate one constant declaration to its folded value, checking a type
/// annotation when present. Every failure arm reports its cause and hands back the
/// summary built from that same report.
fn evaluate(
    declared: Declared<'_>,
    decl: &ConstDecl,
    types: &TypeRegistry,
    diagnostics: &mut DiagnosticCollector,
) -> Result<ConstScalar, DeclarationRefusalSummary> {
    let Some(expression) = &decl.value else {
        return Err(unsupported(
            diagnostics,
            declared,
            declared.span,
            "a constant without a value",
        ));
    };
    let value = literal_value(declared, expression, diagnostics)?;
    if let Some(annotation) = &decl.ty {
        let annotated = match types.expand(annotation) {
            TypeExpr::Name { text, .. } => ScalarType::from_spelling(&text),
            _ => None,
        };
        match annotated {
            Some(scalar) if scalar == value.scalar() => {}
            Some(scalar) => {
                return Err(refuse(
                    diagnostics,
                    declared,
                    Code::CheckType.as_str(),
                    format!(
                        "constant `{}` is declared `{}` but its value is `{}`",
                        decl.name,
                        scalar.spelling(),
                        value.scalar().spelling()
                    ),
                ));
            }
            None => {
                return Err(unsupported(
                    diagnostics,
                    declared,
                    declared.span,
                    "this constant type",
                ));
            }
        }
    }
    Ok(value)
}

/// Fold a scalar literal (or a negated integer literal) to its value. The refusal
/// carries the span of the offending expression, which for a nested literal is
/// narrower than the declaration's.
fn literal_value(
    declared: Declared<'_>,
    expression: &Expression,
    diagnostics: &mut DiagnosticCollector,
) -> Result<ConstScalar, DeclarationRefusalSummary> {
    let out_of_range = |diagnostics: &mut DiagnosticCollector, span: SourceSpan| {
        refuse_at(
            diagnostics,
            declared,
            span,
            Code::CheckType.as_str(),
            "integer literal is out of the 64-bit range".to_string(),
        )
    };
    match expression {
        Expression::Literal { kind, text, span } => match kind {
            LiteralKind::Integer => match parse_int(text) {
                Some(value) => Ok(ConstScalar::Int(value)),
                None => Err(out_of_range(diagnostics, *span)),
            },
            LiteralKind::Bool => Ok(ConstScalar::Bool(&**text == "true")),
            LiteralKind::String => match decode_string_literal(text) {
                Ok(decoded) => Ok(ConstScalar::Text(decoded)),
                Err(_) => Err(unsupported(
                    diagnostics,
                    declared,
                    *span,
                    "this string literal",
                )),
            },
            _ => Err(refuse_at(
                diagnostics,
                declared,
                *span,
                Code::CheckUnsupported.as_str(),
                "this literal is not yet supported in a constant".to_string(),
            )),
        },
        // A negated integer literal is the one non-atomic constant form the subset
        // folds, so `const MIN = -1` is expressible.
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            span,
        } => match literal_value(declared, operand, diagnostics)? {
            ConstScalar::Int(value) => match value.checked_neg() {
                Some(negated) => Ok(ConstScalar::Int(negated)),
                None => Err(out_of_range(diagnostics, *span)),
            },
            other => Err(refuse_at(
                diagnostics,
                declared,
                *span,
                Code::CheckType.as_str(),
                format!("cannot negate a `{}` constant", other.scalar().spelling()),
            )),
        },
        // An integer-bound value built-in (`maxInt`/`minInt`) is a compile-time `int`
        // value, so `const CAP = maxInt` folds to that bound. Only these bare names are
        // admitted; every other name stays a non-literal that a later
        // constant-expression lane will handle.
        Expression::Name { segments, .. }
            if segments.len() == 1
                && crate::lower::builtin_const_int(segments[0].text()).is_some() =>
        {
            match crate::lower::builtin_const_int(segments[0].text()) {
                Some(value) => Ok(ConstScalar::Int(value)),
                None => Err(unsupported(
                    diagnostics,
                    declared,
                    segments[0].span(),
                    "this constant value",
                )),
            }
        }
        other => Err(refuse_at(
            diagnostics,
            declared,
            other.span(),
            Code::CheckUnsupported.as_str(),
            "a constant must be a scalar literal on the beta line".to_string(),
        )),
    }
}

/// Refuse `declared` at a span inside its declaration rather than at the
/// declaration's own.
fn refuse_at(
    diagnostics: &mut DiagnosticCollector,
    declared: Declared<'_>,
    span: SourceSpan,
    code: &'static str,
    message: String,
) -> DeclarationRefusalSummary {
    refuse(diagnostics, Declared { span, ..declared }, code, message)
}

fn unsupported(
    diagnostics: &mut DiagnosticCollector,
    declared: Declared<'_>,
    span: SourceSpan,
    subject: &str,
) -> DeclarationRefusalSummary {
    refuse_at(
        diagnostics,
        declared,
        span,
        Code::CheckUnsupported.as_str(),
        format!("{subject} is not yet supported on the beta line"),
    )
}
