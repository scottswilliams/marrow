//! Literal, folded-constant, and string-interpolation lowering: the constant-pool
//! mints for every literal form and the interpolation walk that renders each hole
//! through the one text-conversion owner.

use super::*;

impl<'a, 'd> FnLowerer<'a, 'd> {
    /// Emit a folded module constant as a constant load of its scalar value.
    pub(super) fn lower_const_value(&mut self, value: &ConstScalar, span: SourceSpan) -> LTy {
        let (scalar, minted) = match value {
            ConstScalar::Int(value) => (ScalarType::Int, self.draft.intern_int(*value)),
            ConstScalar::Bool(value) => (ScalarType::Bool, self.draft.intern_bool(*value)),
            ConstScalar::Text(text) => (ScalarType::Text, self.draft.intern_text(text)),
        };
        let Some(const_id) = self.checked_mint(|_| minted) else {
            return LTy::bare_scalar(scalar);
        };
        self.push(Instr::ConstLoad(const_id), span);
        LTy::bare_scalar(scalar)
    }

    pub(super) fn lower_literal(
        &mut self,
        kind: LiteralKind,
        text: &str,
        span: SourceSpan,
    ) -> Option<LTy> {
        let (scalar, minted) = match kind {
            LiteralKind::Integer => {
                let Some(value) = parse_int(text) else {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        "integer literal is out of the 64-bit range".to_string(),
                    ));
                    return None;
                };
                (ScalarType::Int, self.draft.intern_int(value))
            }
            LiteralKind::Bool => (ScalarType::Bool, self.draft.intern_bool(text == "true")),
            LiteralKind::String => {
                let Ok(decoded) = decode_string_literal(text) else {
                    self.fail(unsupported(self.file, span, "this string literal"));
                    return None;
                };
                if decoded.len() > marrow_image::bounds::MAX_STRING_BYTES {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckResourceLimit.as_str(),
                        self.file,
                        span,
                        format!(
                            "a string literal is {} bytes; the fixed limit is {}",
                            decoded.len(),
                            marrow_image::bounds::MAX_STRING_BYTES
                        ),
                    ));
                    return None;
                }
                (ScalarType::Text, self.draft.intern_text(&decoded))
            }
            // The prototype's `1.second` duration-suffix literal is not in the beta
            // floor: a duration is constructed from a canonical text literal. Point
            // at the constructor rather than reporting a generic unsupported literal.
            LiteralKind::Duration => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckUnsupported.as_str(),
                    self.file,
                    span,
                    "duration suffix literals are not supported; construct a duration \
                     from canonical text, e.g. `duration(\"PT1S\")`"
                        .to_string(),
                ));
                return None;
            }
            // A duration word literal (`3 days`) folds at compile time to the canonical
            // temporal encoding: count times the unit's whole seconds times a second in
            // nanoseconds. The parser guarantees the `COUNT UNIT` shape with a fixed unit.
            LiteralKind::DurationWords => {
                let Some(nanos) = duration_words_nanos(text) else {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        "duration literal is out of the representable range".to_string(),
                    ));
                    return None;
                };
                (ScalarType::Duration, self.draft.intern_duration(nanos))
            }
            _ => {
                self.fail(unsupported(self.file, span, "this literal"));
                return None;
            }
        };
        let const_id = self.checked_mint(|_| minted)?;
        self.push(Instr::ConstLoad(const_id), span);
        Some(LTy::bare_scalar(scalar))
    }

    /// Lower an interpolated string `$"...{expr}..."` to a left-folded
    /// [`Instr::TextConcat`] over its parts. A literal text segment loads its
    /// decoded text; a hole admits any nonoptional scalar, enum, or identity accepted
    /// by [`is_interpolable`] and renders it through the canonical value-text owner.
    /// The whole expression is a `string`, and an empty interpolation is the empty
    /// string.
    pub(super) fn lower_interpolation(
        &mut self,
        parts: &[InterpolationPart],
        span: SourceSpan,
    ) -> Option<LTy> {
        if self.terminal_rejection() {
            return None;
        }
        let mut pushed = false;
        let mut ok = true;
        for part in parts {
            let part_ok = self.lower_interpolation_part(part);
            if self.terminal_rejection() {
                return None;
            }
            ok &= part_ok;
            if part_ok {
                if pushed {
                    self.push(Instr::TextConcat, span);
                } else {
                    pushed = true;
                }
            }
        }
        if !ok {
            return None;
        }
        if !pushed {
            let empty = self.checked_mint(|draft| draft.intern_text(""))?;
            self.push(Instr::ConstLoad(empty), span);
        }
        Some(LTy::bare_scalar(ScalarType::Text))
    }

    /// Push one interpolation part as a `string` value; return whether it lowered
    /// cleanly (a failed part has already reported its diagnostic).
    fn lower_interpolation_part(&mut self, part: &InterpolationPart) -> bool {
        match part {
            InterpolationPart::Text { text, span } => {
                let Ok(decoded) = decode_interpolation_text(text) else {
                    self.fail(unsupported(self.file, *span, "this interpolation text"));
                    return false;
                };
                let Some(const_id) = self.checked_mint(|draft| draft.intern_text(&decoded)) else {
                    return false;
                };
                self.push(Instr::ConstLoad(const_id), *span);
                true
            }
            InterpolationPart::Expr(expr) => {
                let Some(ty) = self.lower_expr(expr) else {
                    return false;
                };
                // A `string` hole is already text and needs no conversion; every other
                // interpolable value renders to canonical text through the one owner.
                if let LTy::Scalar {
                    scalar: ScalarType::Text,
                    optional: false,
                } = ty
                {
                    true
                } else if is_interpolable(ty) {
                    self.push(Instr::ConvString, expr.span());
                    true
                } else {
                    self.fail(unsupported(
                        self.file,
                        expr.span(),
                        &format!("interpolating a {} value", ty.spelling(self.records)),
                    ));
                    false
                }
            }
        }
    }
}
