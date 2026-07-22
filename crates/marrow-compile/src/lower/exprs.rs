//! Expression lowering: literals, operators, calls, constructors, and try/field access.

use super::*;

impl<'a> FnLowerer<'a> {
    // --- expressions ---

    /// Lower `expr`, emitting code that pushes its value, then coerce that value to
    /// exactly `expected` (bare-to-optional via `SomeWrap`; `absent` becomes a vacant
    /// optional). Reports a diagnostic and returns `None` on mismatch.
    pub(super) fn lower_as(&mut self, expr: &Expression, expected: LTy) -> Option<()> {
        // A built-in constructor is directed by the expected type: it supplies the
        // exact `Option`/`Result` instantiation, so `none`/`some`/`ok`/`err` need no
        // annotation of their own here.
        if let Some(kind) = constructor_kind(expr) {
            return self.lower_ctor_as(kind, expr, expected);
        }
        // `List()` / `Map()` are empty-collection constructors directed by the
        // expected type, which supplies the exact instantiation.
        if let Some((head, args)) = collection_ctor_call(expr) {
            return self.lower_collection_ctor(head, args, expr.span(), expected);
        }
        if let Expression::Absent { span } = expr {
            // `absent` supplies the vacant value of any optional type, including an
            // optional generic parameter (`T?`) in a template body; the image vacant
            // carries the expected optional's image shape.
            if !expected.is_optional() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    *span,
                    format!(
                        "`absent` needs an optional type, found {}",
                        expected.spelling(self.records)
                    ),
                ));
                return None;
            }
            self.push(Instr::VacantLoad(expected.image()), *span);
            return Some(());
        }
        let got = self.lower_expr(expr)?;
        if got == expected {
            return Some(());
        }
        if !got.is_optional() && expected.is_optional() && got.to_optional() == expected {
            self.push(Instr::SomeWrap, expr.span());
            return Some(());
        }
        self.fail(type_mismatch(
            self.records,
            self.file,
            expr.span(),
            got,
            expected,
        ));
        None
    }

    /// Lower `expr`, emitting code that pushes its value and returning its type.
    pub(super) fn lower_expr(&mut self, expr: &Expression) -> Option<LTy> {
        // A read through a managed index `^root.index[keys]`: a unique index is an exact
        // complete-key lookup yielding the optional `Id(^root)`; a nonunique index is read
        // by scanning it with a `for` head, so naming one in value position is rejected.
        if let Some(read) = self.resolve_index_read(expr) {
            if read.index.unique {
                return self.lower_index_lookup(
                    read.index,
                    read.root.root_id,
                    read.keys,
                    expr.span(),
                );
            }
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                expr.span(),
                format!(
                    "read the non-unique index `{}` by scanning it with a `for` head, \
                     not as a value",
                    read.index.name
                ),
            ));
            return None;
        }
        // Inline `^root(key)` addresses read here. A place-rooted composed read — a field,
        // group, group leaf, or whole branch entry off a named `place`/pin — reads the same
        // way its inline `^root…` equivalent does. A bare place name is a durable
        // designation, not a value, and falls through to its own diagnostic below.
        if self.durable_shape_here(expr).is_some()
            || (!matches!(expr, Expression::Name { .. }) && self.durable_access(expr).is_some())
        {
            let place = self.resolve_durable(expr)?;
            return self.lower_durable_read(place);
        }
        match expr {
            Expression::Literal { kind, text, span } => self.lower_literal(*kind, text, *span),
            Expression::Name { segments, span, .. } => match segments.as_slice() {
                // `none` is a reserved Option constructor; it needs an expected type
                // (an annotation, argument, return, or coerced position) to know its
                // instantiation, so a bare `none` in value position is a type error.
                [name] if name == "none" => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *span,
                        "the Option type of `none` cannot be inferred here; use it where an Option is expected".to_string(),
                    ));
                    None
                }
                [name] => {
                    if let Some(local) = self.lookup(name) {
                        let (slot, ty) = (local.slot, local.ty);
                        // Record the resolved local/parameter type at this use site for
                        // editor hover, before emitting the load. A local use has no
                        // definition target. Rendered only for a body whose facts are
                        // retained: the type spelling is O(type depth), and a divergent
                        // monomorphization would otherwise render it for each of O(N)
                        // discarded instances (Σ = O(N²)).
                        if self.collect_hover {
                            let display = self.hover_type_display(ty);
                            self.record_hover(*span, display, None);
                        }
                        self.push(Instr::LocalGet(slot), *span);
                        return Some(ty);
                    }
                    // A place is a durable designation, not a first-class value:
                    // its bare name cannot be read, passed, or returned.
                    if self.lookup_place(name).is_some() {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            *span,
                            format!(
                                "`{name}` is a durable place, not a value; read a field with \
                                 `{name}.field`, guard the entry with `if const x = {name}`, \
                                 or test it with `exists({name})`"
                            ),
                        ));
                        return None;
                    }
                    // A module-private constant, folded to a constant load. Locals
                    // and parameters shadow it (checked first).
                    if let Some(value) = self.consts.get(self.module, name).cloned() {
                        return Some(self.lower_const_value(&value, *span));
                    }
                    // A binding whose initializer failed left this name unbound; the
                    // initializer already reported the cause, so a later use is silent.
                    if self.poisoned_bindings.contains(name.as_str()) {
                        self.failed = true;
                        return None;
                    }
                    let candidates = self
                        .locals
                        .iter()
                        .map(|local| local.name.as_str())
                        .chain(self.functions.module_function_names(self.module));
                    let suggestion = nearest_name(name, candidates);
                    self.fail(name_not_in_scope(
                        self.file,
                        *span,
                        name,
                        suggestion.as_deref(),
                        NameKind::Value,
                    ));
                    None
                }
                // `Enum::member` for a payloadless member is an enum value.
                [enum_name, variant] if self.records.enum_by_name(enum_name).is_some() => {
                    self.lower_enum_construct(enum_name, variant, &[], *span)
                }
                _ => {
                    self.fail(unsupported(self.file, *span, "a qualified name"));
                    None
                }
            },
            Expression::Absent { span } => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    *span,
                    "the type of `absent` cannot be inferred here".to_string(),
                ));
                None
            }
            Expression::Unary { op, operand, span } => self.lower_unary(*op, operand, *span),
            Expression::Binary {
                op, left, right, ..
            } => self.lower_binary(*op, left, right),
            Expression::Membership {
                value,
                range,
                negated,
                span,
            } => self.lower_membership(value, range, *negated, *span),
            Expression::Call {
                callee, args, span, ..
            } => match self.lower_call_core(callee, args, *span)? {
                CallResult::Value(ty) => Some(ty),
                CallResult::Unit => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *span,
                        "this call returns nothing and has no value here".to_string(),
                    ));
                    None
                }
                CallResult::Diverges => {
                    // A diverging builtin (`unreachable`/`todo`) is a statement, not a
                    // value; it is only valid in statement position.
                    let name = match callee.as_ref() {
                        Expression::Name { segments, .. } if segments.len() == 1 => {
                            segments[0].as_str()
                        }
                        _ => "unreachable",
                    };
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        *span,
                        format!("`{name}` is a statement and cannot be used as a value"),
                    ));
                    None
                }
            },
            Expression::Field {
                base, name, span, ..
            } => self.lower_field(base, name, *span),
            Expression::OptionalField {
                base, name, span, ..
            } => self.lower_optional_field(base, name, *span),
            Expression::Try { inner, span } => self.lower_try(inner, *span),
            Expression::Interpolation { parts, span } => self.lower_interpolation(parts, *span),
            // A `Keyed` on a durable base was handled above; here the base is a local
            // collection, so `xs[i]` / `m[k]` is a local bracket read yielding the optional.
            Expression::Keyed {
                base, keys, span, ..
            } => self.lower_local_bracket_read(base, keys, *span),
            other => {
                self.fail(unsupported(self.file, other.span(), "this expression"));
                None
            }
        }
    }

    /// Emit a folded module constant as a constant load of its scalar value.
    fn lower_const_value(&mut self, value: &ConstScalar, span: SourceSpan) -> LTy {
        let (scalar, const_id) = match value {
            ConstScalar::Int(value) => (ScalarType::Int, self.draft.intern_int(*value)),
            ConstScalar::Bool(value) => (ScalarType::Bool, self.draft.intern_bool(*value)),
            ConstScalar::Text(text) => (ScalarType::Text, self.draft.intern_text(text)),
        };
        self.push(Instr::ConstLoad(const_id.index()), span);
        LTy::bare_scalar(scalar)
    }

    fn lower_literal(&mut self, kind: LiteralKind, text: &str, span: SourceSpan) -> Option<LTy> {
        let (scalar, const_id) = match kind {
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
        self.push(Instr::ConstLoad(const_id.index()), span);
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
            let empty = self.draft.intern_text("");
            self.push(Instr::ConstLoad(empty.index()), span);
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
                let const_id = self.draft.intern_text(&decoded);
                self.push(Instr::ConstLoad(const_id.index()), *span);
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

    fn lower_unary(&mut self, op: UnaryOp, operand: &Expression, span: SourceSpan) -> Option<LTy> {
        let ty = self.lower_expr(operand)?;
        match op {
            UnaryOp::Neg => {
                if ty != LTy::bare_scalar(ScalarType::Int) {
                    self.fail(unary_error(
                        self.records,
                        self.file,
                        span,
                        "negate",
                        ty,
                        LTy::bare_scalar(ScalarType::Int),
                    ));
                    return None;
                }
                self.push(Instr::IntNeg, span);
                Some(LTy::bare_scalar(ScalarType::Int))
            }
            UnaryOp::Not => {
                if ty != LTy::bare_scalar(ScalarType::Bool) {
                    self.fail(unary_error(
                        self.records,
                        self.file,
                        span,
                        "apply `not` to",
                        ty,
                        LTy::bare_scalar(ScalarType::Bool),
                    ));
                    return None;
                }
                self.push(Instr::BoolNot, span);
                Some(LTy::bare_scalar(ScalarType::Bool))
            }
        }
    }

    fn lower_binary(&mut self, op: BinaryOp, left: &Expression, right: &Expression) -> Option<LTy> {
        match op {
            BinaryOp::And | BinaryOp::Or => self.lower_short_circuit(op, left, right),
            BinaryOp::Coalesce => self.lower_coalesce(left, right),
            _ => {
                // `absent` is not an equality operand: presence has one canonical vocabulary
                // (`if const` / `??` / `exists`), and a second equality spelling for the same
                // question is not admitted. Steer before generic operand typing, so the
                // message names the presence forms rather than the uninferable-`absent` error.
                // The left operand is lowered first so a genuinely ill-typed left still errors
                // at its own site.
                if matches!(op, BinaryOp::Equal | BinaryOp::NotEqual) {
                    if let Expression::Absent { span } = left {
                        self.fail(absent_not_operand(self.file, *span, op));
                        return None;
                    }
                    let left_ty = self.lower_expr(left)?;
                    if let Expression::Absent { span } = right {
                        self.fail(absent_not_operand(self.file, *span, op));
                        return None;
                    }
                    return self.lower_binary_op(op, left_ty, right);
                }
                let left_ty = self.lower_expr(left)?;
                self.lower_binary_op(op, left_ty, right)
            }
        }
    }

    /// Lower the right operand and the arithmetic/comparison operator, given the left
    /// operand's already-pushed type. Both operands must be bare scalars or bare
    /// nominals; a nominal operand routes to the capability-gated nominal table.
    pub(super) fn lower_binary_op(
        &mut self,
        op: BinaryOp,
        left_ty: LTy,
        right: &Expression,
    ) -> Option<LTy> {
        // The `step` capability admits only the literal `1`, so the right operand's
        // shape is read before it is lowered.
        let right_is_one = matches!(
            right,
            Expression::Literal {
                kind: LiteralKind::Integer,
                text,
                ..
            } if parse_int(text) == Some(1)
        );
        let right_ty = self.lower_expr(right)?;
        let span = right.span();
        // An abstract type parameter (template pass only) admits `==`/`!=` when it
        // supports equality and `<`/`<=`/`>`/`>=` when it supports order; every other
        // operator over it is rejected. An unconstrained parameter admits neither, so
        // it falls through to the standard operator error.
        if left_ty.bare_param().is_some() || right_ty.bare_param().is_some() {
            return self.lower_param_binary(op, left_ty, right_ty, span);
        }
        if left_ty.bare_nominal().is_some() || right_ty.bare_nominal().is_some() {
            return self.lower_nominal_binary(op, left_ty, right_ty, right_is_one, span);
        }
        if left_ty.bare_enum().is_some() || right_ty.bare_enum().is_some() {
            return self.lower_enum_binary(op, left_ty, right_ty, span);
        }
        if left_ty.bare_identity().is_some() || right_ty.bare_identity().is_some() {
            return self.lower_identity_binary(op, left_ty, right_ty, span);
        }
        let (Some(left), Some(right_scalar)) =
            (left_ty.bare_scalar_type(), right_ty.bare_scalar_type())
        else {
            self.fail(binary_error(
                self.records,
                self.file,
                span,
                op,
                left_ty,
                right_ty,
            ));
            return None;
        };
        use ScalarType::{Bool, Bytes, Date, Duration, Instant, Int, Text};
        let (instr, result): (Instr, ScalarType) = match (op, left, right_scalar) {
            (BinaryOp::Add, Int, Int) => (Instr::IntAdd, Int),
            (BinaryOp::Add, Text, Text) => (Instr::TextConcat, Text),
            (BinaryOp::Subtract, Int, Int) => (Instr::IntSub, Int),
            (BinaryOp::Multiply, Int, Int) => (Instr::IntMul, Int),
            (BinaryOp::Remainder, Int, Int) => (Instr::IntRem, Int),
            (BinaryOp::Divide, Int, Int) => (Instr::IntDiv, Int),
            #[expect(
                clippy::expect_used,
                reason = "match-arm narrowing: the arm guard already tested `int_comparison(op).is_some()`, so the same call in the body yields `Some`"
            )]
            (op, Int, Int) if int_comparison(op).is_some() => {
                (int_comparison(op).expect("guard matched"), Bool)
            }
            (BinaryOp::Less, Text, Text) => (Instr::TextLt, Bool),
            (BinaryOp::LessEqual, Text, Text) => (Instr::TextLe, Bool),
            (BinaryOp::Greater, Text, Text) => (Instr::TextGt, Bool),
            (BinaryOp::GreaterEqual, Text, Text) => (Instr::TextGe, Bool),
            (BinaryOp::Less, Bytes, Bytes) => (Instr::BytesLt, Bool),
            (BinaryOp::LessEqual, Bytes, Bytes) => (Instr::BytesLe, Bool),
            (BinaryOp::Greater, Bytes, Bytes) => (Instr::BytesGt, Bool),
            (BinaryOp::GreaterEqual, Bytes, Bytes) => (Instr::BytesGe, Bool),
            // Temporal order (same-type only). The closed arithmetic floor: a
            // duration sums/differences with a duration, and a duration shifts an
            // instant; there is no `date +/- int` operator (use `addDays`), no
            // `duration * int`, and no calendar-month arithmetic.
            #[expect(
                clippy::expect_used,
                reason = "match-arm narrowing: the arm guard tested `temporal_comparison(op).is_some()`, which holds exactly when `date_comparison(op)` is `Some`"
            )]
            (op, Date, Date) if temporal_comparison(op).is_some() => {
                (date_comparison(op).expect("guard matched"), Bool)
            }
            #[expect(
                clippy::expect_used,
                reason = "match-arm narrowing: the arm guard tested `temporal_comparison(op).is_some()`, which holds exactly when `instant_comparison(op)` is `Some`"
            )]
            (op, Instant, Instant) if temporal_comparison(op).is_some() => {
                (instant_comparison(op).expect("guard matched"), Bool)
            }
            #[expect(
                clippy::expect_used,
                reason = "match-arm narrowing: the arm guard tested `temporal_comparison(op).is_some()`, which holds exactly when `duration_comparison(op)` is `Some`"
            )]
            (op, Duration, Duration) if temporal_comparison(op).is_some() => {
                (duration_comparison(op).expect("guard matched"), Bool)
            }
            (BinaryOp::Add, Duration, Duration) => (Instr::DurationAdd, Duration),
            (BinaryOp::Subtract, Duration, Duration) => (Instr::DurationSub, Duration),
            (BinaryOp::Add, Instant, Duration) => (Instr::InstantAddDuration, Instant),
            (BinaryOp::Subtract, Instant, Duration) => (Instr::InstantSubDuration, Instant),
            (BinaryOp::Equal, a, b) if a == b => (eq_instr(a), Bool),
            (BinaryOp::NotEqual, a, b) if a == b => {
                self.push(eq_instr(a), span);
                self.push(Instr::BoolNot, span);
                return Some(LTy::bare_scalar(Bool));
            }
            _ => {
                self.fail(binary_error(
                    self.records,
                    self.file,
                    span,
                    op,
                    left_ty,
                    right_ty,
                ));
                return None;
            }
        };
        self.push(instr, span);
        Some(LTy::bare_scalar(result))
    }

    /// Lower a binary operator with a bare nominal operand. The capability table
    /// (documented in `docs/language/types-and-values.md`):
    ///
    /// - comparisons between two values of the same nominal are always admitted
    ///   (they construct nothing);
    /// - `add`: `N + int` and `int + N`, guarded to `N`;
    /// - `subtract`: `N - int` guarded to `N`; `N - N` to plain `int`, unguarded
    ///   (a difference is a count, not a value of the type);
    /// - `scale`: `N * int` and `int * N`, guarded to `N`;
    /// - `step`: `N + 1` and `N - 1` (the int literal `1`), guarded to `N`.
    ///
    /// Every operator that produces a nominal value re-guards the result, so no
    /// path constructs an out-of-interval value. A missing capability is a typed
    /// diagnostic naming it.
    fn lower_nominal_binary(
        &mut self,
        op: BinaryOp,
        left_ty: LTy,
        right_ty: LTy,
        right_is_one: bool,
        span: SourceSpan,
    ) -> Option<LTy> {
        let bool_ty = LTy::bare_scalar(ScalarType::Bool);
        let int_ty = LTy::bare_scalar(ScalarType::Int);
        let same_nominal = left_ty.bare_nominal().is_some() && left_ty == right_ty;
        if same_nominal {
            if let Some(instr) = int_comparison(op) {
                self.push(instr, span);
                return Some(bool_ty);
            }
            match op {
                BinaryOp::Equal => {
                    self.push(eq_instr(ScalarType::Int), span);
                    return Some(bool_ty);
                }
                BinaryOp::NotEqual => {
                    self.push(eq_instr(ScalarType::Int), span);
                    self.push(Instr::BoolNot, span);
                    return Some(bool_ty);
                }
                BinaryOp::Subtract => {
                    return if self.nominal_supports(left_ty).subtract {
                        self.push(Instr::IntSub, span);
                        Some(int_ty)
                    } else {
                        self.fail_missing_capability(left_ty, "subtract", op, span);
                        None
                    };
                }
                _ => {
                    self.fail(binary_error(
                        self.records,
                        self.file,
                        span,
                        op,
                        left_ty,
                        right_ty,
                    ));
                    return None;
                }
            }
        }
        // Mixed nominal/int arithmetic; the result is the nominal, re-guarded.
        let (nominal, nominal_on_left) = match (left_ty.bare_nominal(), right_ty.bare_nominal()) {
            (Some(_), None) if right_ty == int_ty => (left_ty, true),
            (None, Some(_)) if left_ty == int_ty => (right_ty, false),
            _ => {
                self.fail(binary_error(
                    self.records,
                    self.file,
                    span,
                    op,
                    left_ty,
                    right_ty,
                ));
                return None;
            }
        };
        let supports = self.nominal_supports(nominal);
        let stepped = supports.step && nominal_on_left && right_is_one;
        let instr = match op {
            BinaryOp::Add if supports.add || stepped => Instr::IntAdd,
            BinaryOp::Subtract if nominal_on_left && (supports.subtract || stepped) => {
                Instr::IntSub
            }
            BinaryOp::Multiply if supports.scale => Instr::IntMul,
            BinaryOp::Add => {
                self.fail_missing_capability(nominal, "add", op, span);
                return None;
            }
            BinaryOp::Subtract if nominal_on_left => {
                self.fail_missing_capability(nominal, "subtract", op, span);
                return None;
            }
            BinaryOp::Multiply => {
                self.fail_missing_capability(nominal, "scale", op, span);
                return None;
            }
            _ => {
                self.fail(binary_error(
                    self.records,
                    self.file,
                    span,
                    op,
                    left_ty,
                    right_ty,
                ));
                return None;
            }
        };
        self.push(instr, span);
        #[expect(
            clippy::expect_used,
            reason = "checker-classified type: this path runs only after the checker classified the receiver as a bare nominal type"
        )]
        let id = nominal.bare_nominal().expect("classified as a nominal");
        let info = self.records.nominal(id);
        self.push(
            Instr::RangeGuard {
                lo: info.lo,
                hi: info.hi,
            },
            span,
        );
        Some(nominal)
    }

    /// Lower `==`/`!=` on two values of the same enum to `EqEnum` (exact variant
    /// and payload equality). Any other operator, or two different enums, is a
    /// typed diagnostic — an enum has no ordering.
    fn lower_enum_binary(
        &mut self,
        op: BinaryOp,
        left_ty: LTy,
        right_ty: LTy,
        span: SourceSpan,
    ) -> Option<LTy> {
        let bool_ty = LTy::bare_scalar(ScalarType::Bool);
        let same_enum =
            left_ty.bare_enum().is_some() && left_ty.bare_enum() == right_ty.bare_enum();
        match op {
            BinaryOp::Equal if same_enum => {
                self.push(Instr::EqEnum, span);
                Some(bool_ty)
            }
            BinaryOp::NotEqual if same_enum => {
                self.push(Instr::EqEnum, span);
                self.push(Instr::BoolNot, span);
                Some(bool_ty)
            }
            _ => {
                self.fail(binary_error(
                    self.records,
                    self.file,
                    span,
                    op,
                    left_ty,
                    right_ty,
                ));
                None
            }
        }
    }

    /// Lower `==`/`!=` between two entry identities of the same store root — the only
    /// operators identities admit. Equality is key-tuple equality; a mismatched root
    /// (impossible with one declared root, but kept as the general rule) or any other
    /// operator is the standard binary error.
    fn lower_identity_binary(
        &mut self,
        op: BinaryOp,
        left_ty: LTy,
        right_ty: LTy,
        span: SourceSpan,
    ) -> Option<LTy> {
        let bool_ty = LTy::bare_scalar(ScalarType::Bool);
        let same_root = left_ty.bare_identity().is_some()
            && left_ty.bare_identity() == right_ty.bare_identity();
        match op {
            BinaryOp::Equal if same_root => {
                self.push(Instr::EqId, span);
                Some(bool_ty)
            }
            BinaryOp::NotEqual if same_root => {
                self.push(Instr::EqId, span);
                self.push(Instr::BoolNot, span);
                Some(bool_ty)
            }
            _ => {
                self.fail(binary_error(
                    self.records,
                    self.file,
                    span,
                    op,
                    left_ty,
                    right_ty,
                ));
                None
            }
        }
    }

    /// Lower `==`/`!=` and the ordering operators over an abstract type parameter,
    /// reached only in the template pass. Both operands must be the same type
    /// parameter (two distinct parameters are distinct opaque types). Equality is
    /// admitted when the parameter's constraint licenses it (`supports equality`, or
    /// `supports order`, which subsumes equality); ordering requires `supports
    /// order`. Any other operator, an unconstrained parameter, or a mismatch is the
    /// standard operator error. The emitted instruction is a stack-shape placeholder:
    /// the template pass discards its code, and a monomorphized instance re-lowers
    /// the body over the concrete type, emitting the real comparison.
    fn lower_param_binary(
        &mut self,
        op: BinaryOp,
        left_ty: LTy,
        right_ty: LTy,
        span: SourceSpan,
    ) -> Option<LTy> {
        let bool_ty = LTy::bare_scalar(ScalarType::Bool);
        let same_param = left_ty.bare_param().is_some() && left_ty == right_ty;
        let constraint = left_ty
            .bare_param()
            .and_then(|index| self.type_param_constraint(index));
        let admitted = match op {
            BinaryOp::Equal | BinaryOp::NotEqual => {
                constraint.is_some_and(TypeConstraint::admits_equality)
            }
            BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => {
                constraint.is_some_and(TypeConstraint::admits_order)
            }
            _ => false,
        };
        if same_param && admitted {
            // Placeholder with the right stack shape (pop two, push one bool); the
            // code is discarded by the template pass.
            self.push(Instr::EqInt, span);
            return Some(bool_ty);
        }
        if same_param {
            let want = match op {
                BinaryOp::Less
                | BinaryOp::LessEqual
                | BinaryOp::Greater
                | BinaryOp::GreaterEqual => "order",
                _ => "equality",
            };
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "operator `{}` needs the type parameter to `supports {want}`",
                    operator_symbol(op)
                ),
            ));
            return None;
        }
        self.fail(binary_error(
            self.records,
            self.file,
            span,
            op,
            left_ty,
            right_ty,
        ));
        None
    }

    /// The constraint on the abstract type parameter at `index`, in the template
    /// pass. `None` outside that pass or for an unconstrained parameter.
    fn type_param_constraint(&self, index: u16) -> Option<TypeConstraint> {
        let env = TypeEnv {
            params: &self.type_env,
        };
        env.constraint_at(index)
    }

    fn nominal_supports(&self, ty: LTy) -> SupportSet {
        #[expect(
            clippy::expect_used,
            reason = "checker-classified type: the caller passes a type it already classified as a bare nominal"
        )]
        let id = ty.bare_nominal().expect("caller classified a nominal");
        self.records.nominal(id).supports
    }

    fn fail_missing_capability(
        &mut self,
        ty: LTy,
        capability: &str,
        op: BinaryOp,
        span: SourceSpan,
    ) {
        let name = ty.spelling(self.records);
        self.fail(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            self.file,
            span,
            format!(
                "type `{name}` does not support `{capability}`, so `{}` is not defined for it",
                operator_symbol(op)
            ),
        ));
    }

    /// `left ?? right`: yield the present value of the optional `left`, else `right`.
    /// Lowered to the atomic present-branch (design §D), so no unchecked unwrap.
    fn lower_coalesce(&mut self, left: &Expression, right: &Expression) -> Option<LTy> {
        let left_ty = self.lower_expr(left)?;
        if !left_ty.is_optional() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                left.span(),
                format!(
                    "`??` needs an optional on the left, found {}",
                    left_ty.spelling(self.records)
                ),
            ));
            return None;
        }
        let bare = left_ty.to_bare();
        let bp = self.push_branch_present(left.span());
        let to_end = self.push_jump(left.span());
        let absent = self.here();
        self.patch(bp, absent);
        self.lower_as(right, bare)?;
        let end = self.here();
        self.patch(to_end, end);
        Some(bare)
    }

    fn lower_short_circuit(
        &mut self,
        op: BinaryOp,
        left: &Expression,
        right: &Expression,
    ) -> Option<LTy> {
        let bool_ty = LTy::bare_scalar(ScalarType::Bool);
        let left_ty = self.lower_expr(left)?;
        if left_ty != bool_ty {
            self.fail(logic_operand(
                self.records,
                self.file,
                left.span(),
                op,
                left_ty,
            ));
            return None;
        }
        match op {
            BinaryOp::And => {
                let jif = self.push_jif(left.span());
                let right_ty = self.lower_expr(right)?;
                if right_ty != bool_ty {
                    self.fail(logic_operand(
                        self.records,
                        self.file,
                        right.span(),
                        op,
                        right_ty,
                    ));
                    return None;
                }
                let to_end = self.push_jump(right.span());
                let false_at = self.here();
                self.patch(jif, false_at);
                let konst = self.draft.intern_bool(false);
                self.push(Instr::ConstLoad(konst.index()), left.span());
                let end = self.here();
                self.patch(to_end, end);
            }
            BinaryOp::Or => {
                let jif = self.push_jif(left.span());
                let konst = self.draft.intern_bool(true);
                self.push(Instr::ConstLoad(konst.index()), left.span());
                let to_end = self.push_jump(left.span());
                let rhs_at = self.here();
                self.patch(jif, rhs_at);
                let right_ty = self.lower_expr(right)?;
                if right_ty != bool_ty {
                    self.fail(logic_operand(
                        self.records,
                        self.file,
                        right.span(),
                        op,
                        right_ty,
                    ));
                    return None;
                }
                let end = self.here();
                self.patch(to_end, end);
            }
            #[expect(
                clippy::unreachable,
                reason = "match-arm narrowing: the caller reaches short-circuit lowering only for the `and`/`or` operators matched above"
            )]
            _ => unreachable!("only and/or reach short-circuit lowering"),
        }
        Some(bool_ty)
    }

    /// Lower interval membership `value in lo..hi` / `value not in lo..=hi` to a bool.
    /// The value is evaluated once into a slot and tested against both bounds:
    /// `lo <= value` and `value < hi` (exclusive) or `value <= hi` (inclusive), joined
    /// with the short-circuit `and`; `not in` negates the result. The range is over
    /// integers — a temporal range is not current behavior.
    fn lower_membership(
        &mut self,
        value: &Expression,
        range: &Expression,
        negated: bool,
        span: SourceSpan,
    ) -> Option<LTy> {
        let Some(range) = range_expr(range) else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                range.span(),
                "the right side of this `in` is not a range. Interval membership tests a \
                 range on the right. Write `value in lo..hi`."
                    .to_string(),
            ));
            return None;
        };
        if range.step.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                range.span,
                "an interval-membership range takes no `by` step".to_string(),
            ));
            return None;
        }
        let (Some(lo), Some(hi)) = (range.start, range.end) else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                range.span,
                "interval membership tests a range with both bounds; write `value in lo..hi`"
                    .to_string(),
            ));
            return None;
        };
        let int = LTy::bare_scalar(ScalarType::Int);
        // The value is evaluated once; both bound tests read it from the slot.
        self.lower_as(value, int)?;
        let value_slot = self.alloc_slot();
        self.push(Instr::LocalSet(value_slot), span);

        // lo <= value
        self.lower_as(lo, int)?;
        self.push(Instr::LocalGet(value_slot), span);
        self.push(Instr::IntLe, span);
        let jif = self.push_jif(span);

        // value <op> hi
        self.push(Instr::LocalGet(value_slot), span);
        self.lower_as(hi, int)?;
        self.push(
            if range.inclusive_end {
                Instr::IntLe
            } else {
                Instr::IntLt
            },
            span,
        );
        let to_end = self.push_jump(span);
        let false_at = self.here();
        self.patch(jif, false_at);
        let konst = self.draft.intern_bool(false);
        self.push(Instr::ConstLoad(konst.index()), span);
        let end = self.here();
        self.patch(to_end, end);

        if negated {
            self.push(Instr::BoolNot, span);
        }
        Some(LTy::bare_scalar(ScalarType::Bool))
    }

    /// A parenthesized application is a record constructor (`Note(title: t, ...)`)
    /// or a direct function call.
    pub(super) fn lower_call_core(
        &mut self,
        callee: &Expression,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<CallResult> {
        let Expression::Name {
            segments,
            segment_spans,
            ..
        } = callee
        else {
            // `Age.checked(n)`: the nominal range test, the one member call the
            // subset admits. Any other field-shaped callee stays unsupported.
            if let Expression::Field { base, name, .. } = callee {
                if name == "checked"
                    && let Expression::Name { segments, .. } = &**base
                    && let [type_name] = segments.as_slice()
                    && let Some((id, _)) = self.records.nominal_by_name(type_name)
                {
                    return self
                        .lower_checked_nominal(id, args, span)
                        .map(CallResult::Value);
                }
                // `Resource.branch.…(field: value, …)`: a keyed branch entry constructor at
                // any depth, symmetric with the root constructor `Resource(field: value, …)`
                // and resolved through the one type-namespace owner (the store's resource and
                // its executable branch tree).
                if let Some((resource, head_span, mut path)) = split_dotted_head(base) {
                    path.push(name.as_str());
                    if let Some(branch) = self.executable_branch_path(resource, &path) {
                        let display = branch_ctor_display(resource, &path);
                        return self
                            .lower_branch_constructor(
                                resource, &display, branch, head_span, args, span,
                            )
                            .map(CallResult::Value);
                    }
                    // `Resource.group(field: value, …)`: a group value constructor,
                    // symmetric with the branch entry constructor one level down. A
                    // group is an unkeyed single-level namespace, so its qualified head
                    // is the resource then the group name.
                    if let [group_name] = path.as_slice()
                        && self
                            .records
                            .by_name(resource)
                            .is_some_and(|record| record.group(group_name).is_some())
                    {
                        return self
                            .lower_group_constructor(resource, group_name, head_span, args, span)
                            .map(CallResult::Value);
                    }
                }
                // A method-shaped call on a value: `s.trim()`. Member syntax reaches
                // fields and constructor paths only, so this is not a call the subset
                // admits; the teaching form is the free-function spelling of the same
                // call, written with the receiver as the first argument.
                self.fail(SourceDiagnostic::at(
                    Code::CheckUnsupported.as_str(),
                    self.file,
                    span,
                    format!(
                        "`{name}` is written as a method call on `{receiver}`. A value has no \
                         methods; an operation on a value is an ordinary function call. Write \
                         `{name}({receiver})`.",
                        receiver = marrow_syntax::format_expression(base)
                    ),
                ));
                return None;
            }
            self.fail(unsupported(self.file, span, "this call"));
            return None;
        };
        let generic_enum_template = match segments.as_slice() {
            [enum_name, _] => self
                .records
                .type_template_by_name(enum_name)
                .filter(|template| self.records.template_is_enum(*template)),
            _ => None,
        };
        // The origin of a definition/hover fact is the callee's leaf name segment, not
        // the whole call. A degenerate empty path falls back to the call span.
        let callee_span = segment_spans.last().copied().unwrap_or(span);
        match (segments.as_slice(), generic_enum_template) {
            ([name], _) => self.lower_unqualified_call(name, args, span, callee_span),
            // `Enum::member(payload...)` constructs a payload-carrying enum value.
            ([enum_name, item], _) if self.records.enum_by_name(enum_name).is_some() => self
                .lower_enum_construct(enum_name, item, args, span)
                .map(CallResult::Value),
            // A generic enum template's variant infers its instantiation from the
            // payload values.
            ([_, item], Some(template)) => self
                .lower_generic_enum_construct(template, item, args, span)
                .map(CallResult::Value),
            ([prefix @ .., item], _) => {
                self.lower_qualified_call(prefix, item, args, span, callee_span)
            }
            ([], _) => {
                self.fail(unsupported(self.file, span, "this call"));
                None
            }
        }
    }

    /// An unqualified call: a builtin, a constructor, or a function in the same
    /// module. It never reaches another module — that requires a `::` qualifier.
    fn lower_unqualified_call(
        &mut self,
        name: &str,
        args: &[Argument],
        span: SourceSpan,
        callee_span: SourceSpan,
    ) -> Option<CallResult> {
        // The reserved built-ins are intercepted before any user resolution, so a
        // colliding declaration (rejected at its declaration site) can never reach
        // here. Dispatching on the shared classifier keeps interception and
        // declaration rejection reading the same fact.
        if let Some(builtin) = Builtin::from_name(name) {
            return match builtin {
                Builtin::Exists => self.lower_exists(args, span).map(CallResult::Value),
                Builtin::Unreachable => self.lower_unreachable(args, span),
                Builtin::Todo => self.lower_todo(args, span),
                // `some(v)` infers its Option from `v`; `ok`/`err` cannot infer the
                // whole Result, so they need an expected type (an annotation,
                // argument, return, or coerced position).
                Builtin::Some => self.lower_some_infer(args, span).map(CallResult::Value),
                Builtin::Ok | Builtin::Err => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!(
                            "the Result type of `{name}` cannot be inferred here; use it where a Result is expected"
                        ),
                    ));
                    None
                }
                // `isEmpty` accepts a string or a collection; the other two are
                // text-only.
                Builtin::IsEmpty => self.lower_is_empty(args, span).map(CallResult::Value),
                Builtin::Contains | Builtin::Trim => self
                    .lower_text_builtin(name, args, span)
                    .map(CallResult::Value),
                // `split`/`lines` return a `List[string]`; `join` consumes one.
                Builtin::Split | Builtin::Lines => self
                    .lower_text_split(name, args, span)
                    .map(CallResult::Value),
                Builtin::Join => self.lower_text_join(args, span).map(CallResult::Value),
                Builtin::DateAddDays | Builtin::DateDaysBetween => self
                    .lower_date_arith(builtin, args, span)
                    .map(CallResult::Value),
                // A variadic `List(a, b, c)` infers its element type from its arguments;
                // the empty `List()`/`Map()` infer nothing and need an expected type. A
                // `Map(...)` literal is deferred.
                Builtin::List if !args.is_empty() => self
                    .lower_list_literal_inferred(args, span)
                    .map(CallResult::Value),
                Builtin::Map if !args.is_empty() => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        "a map is constructed empty with `Map()` and filled with `m[k] = v`; \
                         a map literal is not yet available"
                            .to_string(),
                    ));
                    None
                }
                Builtin::List | Builtin::Map => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!(
                            "the type of `{name}()` cannot be inferred here; use it where a {name} type is expected"
                        ),
                    ));
                    None
                }
                Builtin::Id => self.lower_identity_ctor(args, span).map(CallResult::Value),
                // `none` is the payloadless Option constructor; it carries no
                // arguments, so a call form has no meaning.
                Builtin::None => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        "`none` takes no arguments; write `none` where an Option is expected"
                            .to_string(),
                    ));
                    None
                }
            };
        }
        // A scalar-type spelling in call position is a conversion (or, for a
        // temporal type, a compile-time-validated literal constructor), resolved
        // before records/functions so it is never shadowed. The admitted set is
        // closed; an unadmitted pair is a typed `check.unsupported`.
        if let Some(scalar) = ScalarType::from_spelling(name) {
            if scalar.is_temporal() {
                return self
                    .lower_temporal_construct(scalar, args, span)
                    .map(CallResult::Value);
            }
            return self
                .lower_conversion(name, args, span)
                .map(CallResult::Value);
        }
        if let Some((id, _)) = self.records.nominal_by_name(name) {
            return self
                .lower_nominal_construct(id, args, span)
                .map(CallResult::Value);
        }
        if self.records.struct_by_name(name).is_some() {
            return self
                .lower_struct_literal(name, args, span)
                .map(CallResult::Value);
        }
        // A generic struct template infers its instantiation from the field values.
        if let Some(template) = self.records.type_template_by_name(name)
            && !self.records.template_is_enum(template)
        {
            return self
                .lower_generic_struct_literal(template, args, span)
                .map(CallResult::Value);
        }
        if self.records.by_name(name).is_some() {
            return self
                .lower_constructor(name, args, span)
                .map(CallResult::Value);
        }
        if let Some(sig) = self.functions.same_module(self.module, name) {
            let (index, params, ret, target) = (
                sig.index,
                sig.params.clone(),
                sig.ret,
                sig.definition_target(),
            );
            if self.collect_hover {
                let display = signature_display(name, &params, ret, self.records);
                self.record_hover(callee_span, display, Some(target));
            }
            return self.lower_function_call(index, &params, ret, args, span);
        }
        // A same-module generic function is monomorphized at the call site (its type
        // arguments inferred from the arguments), resolved before the collection
        // fallbacks so a generic named `get`/`append`/... shadows them.
        if let Some(template) = self.generics.same_module(self.module, name) {
            self.record_generic_call(template, callee_span);
            return self.lower_generic_call(template, args, span);
        }
        // The procedural collection operations resolve last, so a same-module
        // function of the same common name shadows them.
        if let Some(result) = self.lower_collection_fallback(name, args, span) {
            return result;
        }
        let suggestion = nearest_name(name, self.functions.module_function_names(self.module));
        self.fail(name_not_in_scope(
            self.file,
            span,
            name,
            suggestion.as_deref(),
            NameKind::Function,
        ));
        None
    }

    /// Resolve `append`/`length` as collection operations, or `None` when `name` is not
    /// one of them (so the caller reports it as an unknown name). These are non-reserved
    /// fallbacks: a same-module function of the same name is resolved before this is
    /// reached. A map is read and written with bracket syntax (`m[k]`, `m[k] = v`), not
    /// a `get`/`insert` builtin.
    fn lower_collection_fallback(
        &mut self,
        name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<Option<CallResult>> {
        let value = match name {
            "append" => self.lower_append(args, span),
            "length" => self.lower_length(args, span),
            _ => return None,
        };
        Some(value.map(CallResult::Value))
    }

    /// A `::`-qualified call `prefix::item`: resolved against the calling module's
    /// `use` bindings and the project module set, to a `pub` function.
    fn lower_qualified_call(
        &mut self,
        prefix: &[String],
        item: &str,
        args: &[Argument],
        span: SourceSpan,
        callee_span: SourceSpan,
    ) -> Option<CallResult> {
        match self.functions.resolve_qualified(self.module, prefix, item) {
            CallResolution::Found(sig) => {
                let (index, params, ret, target) = (
                    sig.index,
                    sig.params.clone(),
                    sig.ret,
                    sig.definition_target(),
                );
                if self.collect_hover {
                    let display = signature_display(item, &params, ret, self.records);
                    self.record_hover(callee_span, display, Some(target));
                }
                self.lower_function_call(index, &params, ret, args, span)
            }
            CallResolution::NotPublic => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckVisibility.as_str(),
                    self.file,
                    span,
                    format!("`{item}` is not `pub`, so it cannot be called from another module"),
                ));
                None
            }
            CallResolution::NotFound => {
                // A qualified call whose target module did not parse is a dependency
                // gap, not a plain name error: the callee is unavailable because a
                // required owner is invalid. Record the gap at the callee leaf for
                // editor queries; the ordinary name diagnostic still reports it.
                if self.functions.names_broken_module(self.module, prefix) {
                    let file = self.file.clone();
                    self.dependency_gaps.push((file, callee_span));
                }
                // A qualified generic function is resolved through the same module
                // scope and monomorphized, after the monomorphic table misses.
                if let Some(module) = self.functions.resolved_module(self.module, prefix)
                    && let Some((template, public)) = self.generics.in_module(&module, item)
                {
                    if !public && module != self.module {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckVisibility.as_str(),
                            self.file,
                            span,
                            format!(
                                "`{item}` is not `pub`, so it cannot be called from another module"
                            ),
                        ));
                        return None;
                    }
                    self.record_generic_call(template, callee_span);
                    return self.lower_generic_call(template, args, span);
                }
                let path = prefix
                    .iter()
                    .map(String::as_str)
                    .chain(std::iter::once(item))
                    .collect::<Vec<_>>()
                    .join("::");
                // When the prefix names a real module, offer the nearest function in
                // that module as a did-you-mean for a misspelled cross-module callee.
                let suggestion = self
                    .functions
                    .resolved_module(self.module, prefix)
                    .and_then(|module| {
                        nearest_name(item, self.functions.module_function_names(&module))
                    });
                self.fail(name_not_in_scope(
                    self.file,
                    span,
                    &path,
                    suggestion.as_deref(),
                    NameKind::Function,
                ));
                None
            }
        }
    }

    /// Lower a direct function call: positional arguments matched to the callee's
    /// bare scalar params, then `Call`.
    fn lower_function_call(
        &mut self,
        index: u16,
        params: &[LTy],
        ret: RetType,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<CallResult> {
        if args.len() != params.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("expected {} arguments, found {}", params.len(), args.len()),
            ));
            return None;
        }
        for (argument, param) in args.iter().zip(params) {
            if argument.name.is_some() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    "function arguments are positional".to_string(),
                ));
                return None;
            }
            self.lower_as(&argument.value, *param)?;
        }
        self.push(Instr::Call(index), span);
        self.calls.push(index);
        Some(match ret {
            RetType::Unit => CallResult::Unit,
            RetType::Value(ty) => CallResult::Value(ty),
        })
    }

    /// Record the editor fact for a resolved generic call at the callee leaf span: the
    /// template's canonical signature display and its definition target. The target is
    /// the source template, never a minted instance.
    fn record_generic_call(&mut self, index: usize, callee_span: SourceSpan) {
        if !self.collect_hover {
            return;
        }
        let (display, target) = {
            let template = &self.generics.templates()[index];
            let decl = template.decl;
            (
                generic_signature_display(decl),
                DefinitionTarget {
                    file: template.file.clone(),
                    name_span: decl.name_span,
                    decl_range: decl_range(decl),
                },
            )
        };
        self.record_hover(callee_span, display, Some(target));
    }

    /// Lower a call to a generic function: infer each type argument from the call's
    /// arguments, revalidate the type-parameter constraints against the inferred
    /// concrete types, monomorphize one image function for the exact argument list,
    /// and emit a call to it. A type parameter that no argument determines, an
    /// argument whose type does not match the parameter shape, or an inferred type
    /// that violates a constraint is a typed `check.type`. Inference is exact: a
    /// generic argument matches the parameter type structurally with no implicit
    /// bare-to-optional widening.
    fn lower_generic_call(
        &mut self,
        template_index: usize,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<CallResult> {
        let template: &'a GenericTemplate<'a> = &self.generics.templates[template_index];
        let params = &template.decl.params;
        if args.len() != params.len() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!("expected {} arguments, found {}", params.len(), args.len()),
            ));
            return None;
        }
        let mut subst: Vec<Option<GArg>> = vec![None; template.type_params.len()];
        for (argument, param) in args.iter().zip(params) {
            if argument.name.is_some() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    "function arguments are positional".to_string(),
                ));
                return None;
            }
            let got = self.lower_expr(&argument.value)?;
            let expanded = self.records.expand(&param.ty);
            if let Err(error) = unify_type_param(
                self.records,
                &template.type_params,
                &expanded,
                got,
                &mut subst,
            ) {
                self.reject_unification(
                    error,
                    argument.value.span(),
                    "this generic call inference",
                );
                return None;
            }
        }
        // Every type parameter must be determined by an argument: there is no
        // explicit instantiation syntax, so an undetermined parameter cannot be
        // resolved and the call is rejected at its site.
        let mut concrete = Vec::with_capacity(subst.len());
        for (slot, (name, _)) in subst.iter().zip(&template.type_params) {
            match slot {
                Some(arg) => concrete.push(*arg),
                None => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!(
                            "cannot infer type parameter `{name}` of `{}`; \
                             pass an argument whose type determines it",
                            template.decl.name
                        ),
                    ));
                    return None;
                }
            }
        }
        // Per-application constraint revalidation: the concrete type substituted for
        // each constrained parameter must support the constraint's operators.
        for ((name, constraint), arg) in template.type_params.iter().zip(&concrete) {
            let Some(constraint) = constraint else {
                continue;
            };
            let satisfied = match arg {
                // In the template pass an argument may itself be an abstract
                // parameter; it satisfies the constraint when its own constraint does.
                GArg::Param(index) => {
                    self.type_param_constraint(*index)
                        .is_some_and(|outer| match constraint {
                            TypeConstraint::Equality => outer.admits_equality(),
                            TypeConstraint::Order => outer.admits_order(),
                        })
                }
                other => other.satisfies(*constraint),
            };
            if !satisfied {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!(
                        "type parameter `{name}` of `{}` is instantiated with `{}`, \
                         which does not `supports {}`",
                        template.decl.name,
                        garg_to_lty(*arg).spelling(self.records),
                        constraint.spelling(),
                    ),
                ));
                return None;
            }
        }
        // Resolve the return type against the concrete substitution, minting any
        // collection/enum instantiation the return shape needs into the draft (the
        // real draft for an instance, the throwaway draft for the template pass).
        let ret = match self.resolve_generic_return(template, &concrete) {
            Ok(ret) => ret,
            Err(ResolveError::Refusal(ResolveRefusal::Limit)) => {
                self.failed = true;
                return None;
            }
            Err(ResolveError::Refusal(ResolveRefusal::Unsupported)) => {
                let span = template
                    .decl
                    .return_type
                    .as_ref()
                    .map(TypeExpr::span)
                    .unwrap_or(template.decl.span);
                self.fail(unsupported(&template.file, span, "this return type"));
                return None;
            }
            Err(ResolveError::Invariant(invariant)) => {
                self.reject_resolution(
                    ResolveError::Invariant(invariant),
                    span,
                    "this return type",
                );
                return None;
            }
        };
        match self.mode {
            LowerMode::Template => {
                // The once-checked pass validates the call shape but cannot
                // monomorphize an abstract instantiation; a placeholder keeps the
                // discarded stream value-shaped.
                if let RetType::Value(_) = ret {
                    let zero = self.draft.intern_int(0);
                    self.push(Instr::ConstLoad(zero.index()), span);
                }
                Some(match ret {
                    RetType::Unit => CallResult::Unit,
                    RetType::Value(ty) => CallResult::Value(ty),
                })
            }
            LowerMode::Concrete => {
                let func = match self.records.reserve_fn_instance(
                    template_index,
                    concrete,
                    MintSite {
                        file: self.file,
                        span,
                    },
                ) {
                    Ok(func) => func,
                    Err(error) => {
                        self.reject_resolution(error, span, "this generic function call");
                        return None;
                    }
                };
                self.push(Instr::Call(func), span);
                self.calls.push(func);
                Some(match ret {
                    RetType::Unit => CallResult::Unit,
                    RetType::Value(ty) => CallResult::Value(ty),
                })
            }
        }
    }

    /// Resolve a generic template's return type under a concrete substitution,
    /// minting any instantiation it needs into the current draft.
    fn resolve_generic_return(
        &mut self,
        template: &GenericTemplate,
        concrete: &[GArg],
    ) -> Result<RetType, ResolveError> {
        let Some(annotation) = &template.decl.return_type else {
            return Ok(RetType::Unit);
        };
        let env: Vec<TypeParamSlot> = template
            .type_params
            .iter()
            .zip(concrete)
            .map(|((name, _), arg)| TypeParamSlot {
                name: name.clone(),
                binding: ParamBinding::Concrete(*arg),
            })
            .collect();
        let site = MintSite {
            file: &template.file,
            span: annotation.span(),
        };
        resolve_type(
            self.records,
            self.draft,
            self.durable,
            annotation,
            TypeEnv { params: &env },
            site,
        )
        .map(RetType::Value)
    }

    /// Lower a nominal construction `Age(n)`: coerce the one positional argument
    /// to the base int, then guard it against the type's inclusive interval. An
    /// out-of-interval value faults `run.range` at runtime; every path that
    /// produces a value of the type revalidates the interval this way.
    fn lower_nominal_construct(
        &mut self,
        id: NominalId,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let value = self.single_nominal_arg(id, args, span)?;
        self.lower_as(value, LTy::bare_scalar(ScalarType::Int))?;
        let info = self.records.nominal(id);
        self.push(
            Instr::RangeGuard {
                lo: info.lo,
                hi: info.hi,
            },
            span,
        );
        Some(LTy::Nominal {
            id,
            optional: false,
        })
    }

    /// Lower the nominal range test `Age.checked(n): Age?`: present exactly when
    /// the int lies in the interval, vacant otherwise, never a fault. Reuses the
    /// comparison and optional ops; no dedicated opcode.
    fn lower_checked_nominal(
        &mut self,
        id: NominalId,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let value = self.single_nominal_arg(id, args, span)?;
        self.lower_as(value, LTy::bare_scalar(ScalarType::Int))?;
        let slot = self.alloc_slot();
        self.push(Instr::LocalSet(slot), span);
        let (lo, hi) = {
            let info = self.records.nominal(id);
            (info.lo, info.hi)
        };
        // lo <= n && n <= hi, with each failed test jumping to the vacant edge.
        let lo_const = self.draft.intern_int(lo);
        self.push(Instr::LocalGet(slot), span);
        self.push(Instr::ConstLoad(lo_const.index()), span);
        let below = {
            self.push(Instr::IntGe, span);
            self.push_jif(span)
        };
        let hi_const = self.draft.intern_int(hi);
        self.push(Instr::LocalGet(slot), span);
        self.push(Instr::ConstLoad(hi_const.index()), span);
        let above = {
            self.push(Instr::IntLe, span);
            self.push_jif(span)
        };
        self.push(Instr::LocalGet(slot), span);
        self.push(Instr::SomeWrap, span);
        let to_end = self.push_jump(span);
        let vacant = self.here();
        self.patch(below, vacant);
        self.patch(above, vacant);
        self.push(Instr::VacantLoad(ImageType::opt_scalar(Scalar::Int)), span);
        let end = self.here();
        self.patch(to_end, end);
        Some(LTy::Nominal { id, optional: true })
    }

    /// The one positional argument of a nominal construction or range test.
    fn single_nominal_arg<'e>(
        &mut self,
        id: NominalId,
        args: &'e [Argument],
        span: SourceSpan,
    ) -> Option<&'e Expression> {
        match args {
            [arg] if arg.name.is_none() => Some(&arg.value),
            _ => {
                let name = self.records.nominal(id).name.clone();
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!("`{name}` takes one positional int value"),
                ));
                None
            }
        }
    }

    /// Lower a record constructor: each field's argument in declaration order.
    fn lower_constructor(
        &mut self,
        name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let record = self.accept_resolution(
            self.records
                .static_record_projection(name)
                .map_err(ResolveError::Invariant),
            span,
            "this record construction",
        )??;
        let record_type_id = record.type_id;

        // Resolve each named argument against a top-level field or a group before
        // emitting, so evaluation order is the declaration order (fields first, then
        // groups; f0 pushed first). A group argument is the group's value, built with
        // the qualified group constructor `Resource.group(…)`.
        for argument in args {
            let Some(arg_name) = &argument.name else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    "constructor arguments must be named".to_string(),
                ));
                return None;
            };
            if record.field(arg_name).is_none() && record.group(arg_name).is_none() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{name}` has no field `{arg_name}`"),
                ));
                return None;
            }
        }

        let field_plan: Vec<MemberPlan> = record
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty, field.required))
            .collect();
        for (field_name, field_ty, required) in field_plan {
            let arg = args
                .iter()
                .find(|a| a.name.as_deref() == Some(field_name.as_str()));
            let bare = garg_to_lty(field_ty);
            let expected = if required { bare } else { bare.to_optional() };
            match arg {
                Some(argument) => {
                    self.lower_as(&argument.value, expected)?;
                }
                None if required => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!("missing required field `{field_name}`"),
                    ));
                    return None;
                }
                None => {
                    // A sparse field defaults to vacant: a typed vacant optional of
                    // the field's value type.
                    self.push(Instr::VacantLoad(bare.to_optional().image()), span);
                }
            }
        }
        // Each unkeyed group slot follows the top-level fields, in group order. A
        // supplied `group: Resource.group(…)` argument carries the group value; an
        // omitted all-sparse group defaults to present with vacant leaves; an omitted
        // group with a required leaf cannot be auto-completed, so it is the
        // required-completeness rejection here rather than a silent incomplete value.
        let group_plan: Vec<GroupPlan> = record
            .groups
            .iter()
            .map(|group| {
                (
                    group.name.clone(),
                    group.type_id,
                    group.fields.iter().any(|leaf| leaf.required),
                    group
                        .fields
                        .iter()
                        .map(|leaf| (leaf.name.clone(), leaf.ty, leaf.required))
                        .collect(),
                )
            })
            .collect();
        for (group_name, group_type, has_required, leaves) in group_plan {
            let arg = args
                .iter()
                .find(|a| a.name.as_deref() == Some(group_name.as_str()));
            if let Some(argument) = arg {
                self.lower_as(
                    &argument.value,
                    LTy::Record {
                        ty: group_type,
                        optional: false,
                    },
                )?;
                continue;
            }
            if has_required {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!("missing required field `{group_name}`"),
                ));
                return None;
            }
            for (_leaf_name, leaf_ty, _required) in leaves {
                self.push(
                    Instr::VacantLoad(garg_to_lty(leaf_ty).to_optional().image()),
                    span,
                );
            }
            self.push(Instr::RecordNew(group_type.index()), span);
        }
        self.push(Instr::RecordNew(record_type_id.index()), span);
        Some(LTy::Record {
            ty: record_type_id,
            optional: false,
        })
    }

    /// The executable branch `resource.branch`, if `resource` is the store's executable
    /// resource and `branch` is one of its single-level keyed branches. The returned
    /// reference borrows the durable registry (lifetime `'a`), not `self`, so it stays
    /// valid across later mutating calls.
    /// The executable branch reached by the branch-name `path` from `resource`, if
    /// `resource` is the store's executable resource and each name is a keyed branch at its
    /// level. Walks the recursive branch tree so `Book.notes.tags` resolves the nested
    /// `tags` branch of `notes`. The returned reference borrows the durable registry
    /// (lifetime `'a`), not `self`, so it stays valid across later mutating calls.
    fn executable_branch_path(
        &self,
        resource: &str,
        path: &[&str],
    ) -> Option<&'a crate::durable::DurableBranch> {
        let root = self.durable.root_by_resource(resource)?;
        let (first, rest) = path.split_first()?;
        let mut branch = root.branch(first)?;
        for name in rest {
            branch = branch.branch(name)?;
        }
        Some(branch)
    }

    /// Lower a keyed branch entry constructor `Resource.branch(field: value, …)`. The
    /// branch's materialized record is built from its declared scalar fields in
    /// declaration order (f0 pushed first), each required field supplied and each sparse
    /// field defaulting to vacant — the same shape as the root constructor, one level
    /// down. The shadowing rule holds: a value binding may not shadow the resource type
    /// name in dotted-constructor head position.
    fn lower_branch_constructor(
        &mut self,
        resource: &str,
        display: &str,
        branch: &'a crate::durable::DurableBranch,
        head_span: SourceSpan,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        if self.lookup(resource).is_some() || self.lookup_place(resource).is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                head_span,
                format!(
                    "`{resource}` is a resource type here (the head of a branch constructor \
                     `{display}(…)`); a value binding may not shadow it"
                ),
            ));
            return None;
        }
        let record = branch.record;

        // Validate argument names against the branch's fields before emitting, so
        // evaluation order is the field declaration order.
        for argument in args {
            let Some(arg_name) = &argument.name else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    "constructor arguments must be named".to_string(),
                ));
                return None;
            };
            if branch.field(arg_name).is_none() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{display}` has no field `{arg_name}`"),
                ));
                return None;
            }
        }

        // `branch` borrows the registry (lifetime independent of `self`), so it stays
        // valid across the mutating `lower_as`/`push` calls below.
        for field in &branch.fields {
            let arg = args
                .iter()
                .find(|a| a.name.as_deref() == Some(field.name.as_str()));
            let bare = LTy::bare_scalar(field.scalar);
            let expected = if field.required {
                bare
            } else {
                bare.to_optional()
            };
            match arg {
                Some(argument) => {
                    self.lower_as(&argument.value, expected)?;
                }
                None if field.required => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!("missing required field `{}`", field.name),
                    ));
                    return None;
                }
                None => {
                    // A sparse field defaults to a typed vacant optional.
                    self.push(Instr::VacantLoad(bare.to_optional().image()), span);
                }
            }
        }
        self.push(Instr::RecordNew(record.index()), span);
        Some(LTy::Record {
            ty: record,
            optional: false,
        })
    }

    /// Lower a qualified group value constructor `Resource.group(field: value, …)`.
    /// The group's materialized record is built from its declared leaves in
    /// declaration order (f0 pushed first), each required leaf supplied and each
    /// sparse leaf defaulting to vacant — symmetric with the root and branch
    /// constructors. The shadowing rule holds: a value binding may not shadow the
    /// resource type name in dotted-constructor head position.
    fn lower_group_constructor(
        &mut self,
        resource: &str,
        group_name: &str,
        head_span: SourceSpan,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        if self.lookup(resource).is_some() || self.lookup_place(resource).is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                head_span,
                format!(
                    "`{resource}` is a resource type here (the head of a group constructor \
                     `{resource}.{group_name}(…)`); a value binding may not shadow it"
                ),
            ));
            return None;
        }
        let display = format!("{resource}.{group_name}");
        let group = self.accept_resolution(
            self.records
                .static_group_projection(resource, group_name)
                .map_err(ResolveError::Invariant),
            span,
            "this resource-group construction",
        )??;
        let group_type_id = group.type_id;
        let leaf_plan: Vec<MemberPlan> = group
            .fields
            .iter()
            .map(|leaf| (leaf.name.clone(), leaf.ty, leaf.required))
            .collect();

        for argument in args {
            let Some(arg_name) = &argument.name else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    "constructor arguments must be named".to_string(),
                ));
                return None;
            };
            if !leaf_plan.iter().any(|(name, _, _)| name == arg_name) {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{display}` has no field `{arg_name}`"),
                ));
                return None;
            }
        }

        for (leaf_name, leaf_ty, required) in leaf_plan {
            let arg = args
                .iter()
                .find(|a| a.name.as_deref() == Some(leaf_name.as_str()));
            let bare = garg_to_lty(leaf_ty);
            let expected = if required { bare } else { bare.to_optional() };
            match arg {
                Some(argument) => {
                    self.lower_as(&argument.value, expected)?;
                }
                None if required => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!("missing required field `{leaf_name}`"),
                    ));
                    return None;
                }
                None => {
                    self.push(Instr::VacantLoad(bare.to_optional().image()), span);
                }
            }
        }
        self.push(Instr::RecordNew(group_type_id.index()), span);
        Some(LTy::Record {
            ty: group_type_id,
            optional: false,
        })
    }

    /// Lower a dense struct literal `Point(x: a, y: b)`: named-only arguments, the
    /// exact field set with none missing, duplicated, or unknown, each coerced to
    /// its required field scalar in field declaration order (f0 pushed first) so
    /// the canonical product-leaf order owns evaluation. Emits `RecordNew` over the
    /// struct's shared image record def.
    fn lower_struct_literal(
        &mut self,
        name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let info = self.accept_resolution(
            self.records
                .static_struct_projection(name)
                .map_err(ResolveError::Invariant),
            span,
            "this struct construction",
        )??;
        let type_id = info.type_id;

        let mut ok = true;
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for argument in args {
            let Some(arg_name) = &argument.name else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{name}` fields are named, as `{name}(field: value, ...)`"),
                ));
                ok = false;
                continue;
            };
            if info.field(arg_name).is_none() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{name}` has no field `{arg_name}`"),
                ));
                ok = false;
                continue;
            }
            if !seen.insert(arg_name.as_str()) {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("field `{arg_name}` is set more than once"),
                ));
                ok = false;
            }
        }
        if !ok {
            return None;
        }

        let field_plan: Vec<(String, GArg)> = info
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty))
            .collect();
        for (field_name, field_ty) in field_plan {
            let arg = args
                .iter()
                .find(|a| a.name.as_deref() == Some(field_name.as_str()));
            match arg {
                Some(argument) => {
                    self.lower_as(&argument.value, garg_to_lty(field_ty))?;
                }
                None => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!("missing field `{field_name}`"),
                    ));
                    return None;
                }
            }
        }
        self.push(Instr::RecordNew(type_id.index()), span);
        Some(LTy::Struct {
            ty: type_id,
            optional: false,
        })
    }

    /// Lower a generic struct construction `Pair(first: v, second: w)`: infer each
    /// type parameter from the field values (there is no explicit `Pair<int, string>`
    /// construction syntax), monomorphize the instantiation, and construct the record.
    /// Field values are lowered in declaration order so evaluation order is stable.
    pub(super) fn lower_generic_struct_literal(
        &mut self,
        template: usize,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        if self.terminal_rejection() {
            return None;
        }
        let name = self.records.template_name(template).to_string();
        let fields = match self.records.template_struct_fields(template) {
            Ok(fields) => fields,
            Err(invariant) => {
                self.reject_resolution(
                    ResolveError::Invariant(invariant),
                    span,
                    "this generic struct construction",
                );
                return None;
            }
        };
        if !self.check_named_args(
            &name,
            args,
            &fields.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            span,
        ) {
            return None;
        }
        let params = self.records.template_type_params(template).to_vec();
        let mut subst: Vec<Option<GArg>> = vec![None; params.len()];
        for (field_name, field_ty) in &fields {
            let Some(argument) = args
                .iter()
                .find(|a| a.name.as_deref() == Some(field_name.as_str()))
            else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!("missing field `{field_name}`"),
                ));
                return None;
            };
            let got = self.lower_expr(&argument.value)?;
            let expanded = self.records.expand(field_ty);
            if let Err(error) = unify_type_param(self.records, &params, &expanded, got, &mut subst)
            {
                self.reject_unification(
                    error,
                    argument.value.span(),
                    "this generic struct inference",
                );
                return None;
            }
        }
        let concrete = self.determined_args(&name, &params, &subst, span)?;
        if !self.constraints_satisfied(template, &name, &concrete, span) {
            return None;
        }
        let site = MintSite {
            file: self.file,
            span,
        };
        let type_id = match self
            .records
            .mint_struct_instance(self.draft, template, &concrete, site)
        {
            Ok(type_id) => type_id,
            Err(error) => {
                self.reject_resolution(error, span, "this generic struct construction");
                return None;
            }
        };
        self.push(Instr::RecordNew(type_id.index()), span);
        Some(LTy::Struct {
            ty: type_id,
            optional: false,
        })
    }

    /// Lower a generic enum construction `Maybe::just(value: v)`: infer each type
    /// parameter from the variant's payload values, monomorphize the instantiation,
    /// and construct the variant. A payloadless variant or one that does not
    /// determine every parameter cannot be inferred at the construction site.
    pub(super) fn lower_generic_enum_construct(
        &mut self,
        template: usize,
        variant_name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        if self.terminal_rejection() {
            return None;
        }
        let name = self.records.template_name(template).to_string();
        let (template_variant, payload) = match self
            .records
            .template_variant_payload(template, variant_name)
        {
            Ok(Some(payload)) => payload,
            Ok(None) => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!("enum `{name}` has no member `{variant_name}`"),
                ));
                return None;
            }
            Err(invariant) => {
                self.reject_resolution(
                    ResolveError::Invariant(invariant),
                    span,
                    "this generic enum construction",
                );
                return None;
            }
        };
        if !self.check_named_args(
            &format!("{name}::{variant_name}"),
            args,
            &payload.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            span,
        ) {
            return None;
        }
        let params = self.records.template_type_params(template).to_vec();
        let mut subst: Vec<Option<GArg>> = vec![None; params.len()];
        for (field_name, field_ty) in &payload {
            let Some(argument) = args
                .iter()
                .find(|a| a.name.as_deref() == Some(field_name.as_str()))
            else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!("missing payload field `{field_name}`"),
                ));
                return None;
            };
            let got = self.lower_expr(&argument.value)?;
            let expanded = self.records.expand(field_ty);
            if let Err(error) = unify_type_param(self.records, &params, &expanded, got, &mut subst)
            {
                self.reject_unification(
                    error,
                    argument.value.span(),
                    "this generic enum inference",
                );
                return None;
            }
        }
        let concrete = self.determined_args(&name, &params, &subst, span)?;
        if !self.constraints_satisfied(template, &name, &concrete, span) {
            return None;
        }
        let site = MintSite {
            file: self.file,
            span,
        };
        let witness = match self.records.mint_enum_variant_instance(
            self.draft,
            template,
            &concrete,
            EnumVariantSelection {
                index: template_variant,
                name: variant_name,
            },
            site,
        ) {
            Ok(witness) => witness,
            Err(error) => {
                self.reject_resolution(error, span, "this generic enum construction");
                return None;
            }
        };
        self.push(
            Instr::EnumConstruct {
                enum_idx: witness.enum_id.index(),
                variant: witness.variant,
            },
            span,
        );
        Some(LTy::Enum {
            ty: witness.enum_id,
            optional: false,
        })
    }

    /// Validate that every argument is named, names a known field, and is set once.
    /// Shared by generic struct and enum construction. Returns whether the arguments
    /// are well-formed; each defect is reported.
    fn check_named_args(
        &mut self,
        subject: &str,
        args: &[Argument],
        field_names: &[String],
        _span: SourceSpan,
    ) -> bool {
        let mut ok = true;
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for argument in args {
            let Some(arg_name) = &argument.name else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{subject}` fields are named, as `{subject}(field: value, ...)`"),
                ));
                ok = false;
                continue;
            };
            if !field_names.iter().any(|name| name == arg_name) {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("`{subject}` has no field `{arg_name}`"),
                ));
                ok = false;
                continue;
            }
            if !seen.insert(arg_name.as_str()) {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    argument.value.span(),
                    format!("field `{arg_name}` is set more than once"),
                ));
                ok = false;
            }
        }
        ok
    }

    /// Per-application constraint revalidation for an inferred instantiation: every
    /// concrete argument must support its parameter's constraint. Construction always
    /// infers concrete arguments, so no abstract-parameter entailment applies here.
    fn constraints_satisfied(
        &mut self,
        template: usize,
        name: &str,
        concrete: &[GArg],
        span: SourceSpan,
    ) -> bool {
        for ((param_name, constraint), arg) in self
            .records
            .template_type_params(template)
            .iter()
            .zip(concrete)
        {
            if let Some(constraint) = constraint
                && !arg.satisfies(*constraint)
            {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!(
                        "type parameter `{param_name}` of `{name}` is instantiated with `{}`, \
                         which does not `supports {}`",
                        garg_to_lty(*arg).spelling(self.records),
                        constraint.spelling(),
                    ),
                ));
                return false;
            }
        }
        true
    }

    /// Turn an inference substitution into the concrete argument list, reporting an
    /// undetermined type parameter (which the construction site cannot resolve).
    fn determined_args(
        &mut self,
        name: &str,
        params: &[(String, Option<TypeConstraint>)],
        subst: &[Option<GArg>],
        span: SourceSpan,
    ) -> Option<Vec<GArg>> {
        let mut concrete = Vec::with_capacity(params.len());
        for (slot, (param_name, _)) in subst.iter().zip(params) {
            match slot {
                Some(arg) => concrete.push(*arg),
                None => {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        span,
                        format!(
                            "cannot infer type parameter `{param_name}` of `{name}`; \
                             a field value must determine it"
                        ),
                    ));
                    return None;
                }
            }
        }
        Some(concrete)
    }

    /// Lower an enum construction `Enum::member` or `Enum::member(field: v, ...)`.
    /// A payloadless member takes no arguments; a payload member takes the exact
    /// named payload set, each coerced to its declared scalar in payload
    /// declaration order (p0 pushed first), then `EnumConstruct`.
    fn lower_enum_construct(
        &mut self,
        enum_name: &str,
        variant_name: &str,
        args: &[Argument],
        span: SourceSpan,
    ) -> Option<LTy> {
        let info = self.accept_resolution(
            self.records
                .static_enum_projection(enum_name)
                .map_err(ResolveError::Invariant),
            span,
            "this enum construction",
        )??;
        let (enum_id, variant_index) = {
            let Some((index, _)) = info.variant(variant_name) else {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!("enum `{enum_name}` has no member `{variant_name}`"),
                ));
                return None;
            };
            (info.enum_id, index)
        };
        // The payload plan, resolved before emission so evaluation order is the
        // payload declaration order.
        let plan: Vec<(String, ScalarType)> = info
            .variant(variant_name)?
            .1
            .payload
            .iter()
            .map(|field| (field.name.clone(), field.scalar))
            .collect();

        if plan.is_empty() {
            if !args.is_empty() {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!("`{enum_name}::{variant_name}` carries no payload"),
                ));
                return None;
            }
        } else {
            let mut ok = true;
            let mut seen: BTreeSet<&str> = BTreeSet::new();
            for argument in args {
                let Some(arg_name) = &argument.name else {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        argument.value.span(),
                        format!(
                            "`{enum_name}::{variant_name}` payload fields are named, as \
                             `{variant_name}(field: value, ...)`"
                        ),
                    ));
                    ok = false;
                    continue;
                };
                if !plan.iter().any(|(name, _)| name == arg_name) {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        argument.value.span(),
                        format!("`{enum_name}::{variant_name}` has no payload field `{arg_name}`"),
                    ));
                    ok = false;
                    continue;
                }
                if !seen.insert(arg_name.as_str()) {
                    self.fail(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        self.file,
                        argument.value.span(),
                        format!("payload field `{arg_name}` is set more than once"),
                    ));
                    ok = false;
                }
            }
            if !ok {
                return None;
            }
            for (field_name, scalar) in &plan {
                let arg = args
                    .iter()
                    .find(|a| a.name.as_deref() == Some(field_name.as_str()));
                match arg {
                    Some(argument) => {
                        self.lower_as(&argument.value, LTy::bare_scalar(*scalar))?;
                    }
                    None => {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            span,
                            format!("missing payload field `{field_name}`"),
                        ));
                        return None;
                    }
                }
            }
        }
        self.push(
            Instr::EnumConstruct {
                enum_idx: enum_id.index(),
                variant: variant_index,
            },
            span,
        );
        Some(LTy::Enum {
            ty: enum_id,
            optional: false,
        })
    }

    /// The image enum index of the reserved `Option[inner]`, minting it on first use.
    fn opt_enum(&mut self, inner: GArg, span: SourceSpan) -> Option<EnumId> {
        let site = MintSite {
            file: self.file,
            span,
        };
        match self
            .records
            .instantiate_reserved_option(self.draft, inner, site)
        {
            Ok(id) => Some(id),
            Err(refusal) => {
                self.reject_resolution(refusal, span, "this inferred Option type");
                None
            }
        }
    }

    /// Lower a reserved `Option`/`Result` constructor directed by an expected type:
    /// `none`, `some(v)`, `ok(v)`, or `err(e)`. The expected type supplies the exact
    /// instantiation, so the argument (if any) is coerced to the matching member
    /// type. A constructor used where its reserved enum is not expected is a typed
    /// error. `Option`/`Result` are ordinary generic enums; these reserved spellings
    /// resolve to their variants recovered from the minting template.
    pub(super) fn lower_ctor_as(
        &mut self,
        kind: CtorKind,
        expr: &Expression,
        expected: LTy,
    ) -> Option<()> {
        if self.terminal_rejection() {
            return None;
        }
        let span = expr.span();
        // A sparse optional enum target (`Option<T>?`/`Result<T, E>?`) takes a bare
        // constructor wrapped present: lower against the bare enum, then `SomeWrap`.
        // This makes `= none`/`= some(v)` write a sparse optional-enum field or
        // local in one line — the present-value analogue of `= absent`.
        if let LTy::Enum { ty, optional: true } = expected {
            self.lower_ctor_as(
                kind,
                expr,
                LTy::Enum {
                    ty,
                    optional: false,
                },
            )?;
            self.push(Instr::SomeWrap, span);
            return Some(());
        }
        let LTy::Enum {
            ty: enum_id,
            optional: false,
        } = expected
        else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "`{}` needs an Option or Result type here, but the expected type is {}",
                    kind.name(),
                    expected.spelling(self.records)
                ),
            ));
            return None;
        };
        let reserved = self.accept_resolution(
            self.records
                .reserved_enum_args(enum_id)
                .map_err(ResolveError::Invariant),
            span,
            "this reserved constructor",
        )?;
        match (kind, reserved) {
            (CtorKind::None, Some(ReservedEnumArgs::Option(_))) => {
                self.push(
                    Instr::EnumConstruct {
                        enum_idx: enum_id.index(),
                        variant: OPTION_NONE,
                    },
                    span,
                );
                Some(())
            }
            (CtorKind::Some, Some(ReservedEnumArgs::Option(inner))) => {
                let arg = self.single_ctor_arg(expr, "some")?;
                self.lower_as(arg, garg_to_lty(inner))?;
                self.push(
                    Instr::EnumConstruct {
                        enum_idx: enum_id.index(),
                        variant: OPTION_SOME,
                    },
                    span,
                );
                Some(())
            }
            (CtorKind::Ok, Some(ReservedEnumArgs::Result(ok, _))) => {
                let arg = self.single_ctor_arg(expr, "ok")?;
                self.lower_as(arg, garg_to_lty(ok))?;
                self.push(
                    Instr::EnumConstruct {
                        enum_idx: enum_id.index(),
                        variant: RESULT_OK,
                    },
                    span,
                );
                Some(())
            }
            (CtorKind::Err, Some(ReservedEnumArgs::Result(_, err))) => {
                let arg = self.single_ctor_arg(expr, "err")?;
                self.lower_as(arg, garg_to_lty(err))?;
                self.push(
                    Instr::EnumConstruct {
                        enum_idx: enum_id.index(),
                        variant: RESULT_ERR,
                    },
                    span,
                );
                Some(())
            }
            _ => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    span,
                    format!(
                        "`{}` does not construct {}",
                        kind.name(),
                        expected.spelling(self.records)
                    ),
                ));
                None
            }
        }
    }

    /// Lower a bare `some(v)` whose Option type is inferred from `v`. `none`, `ok`,
    /// and `err` cannot infer their full type argument set, so they require an
    /// expected type and are rejected here.
    fn lower_some_infer(&mut self, args: &[Argument], span: SourceSpan) -> Option<LTy> {
        let [arg] = args else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`some` takes exactly one value, as `some(value)`".to_string(),
            ));
            return None;
        };
        if arg.name.is_some() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                "`some` takes a positional value".to_string(),
            ));
            return None;
        }
        let inner_ty = self.lower_expr(&arg.value)?;
        let Some(inner) = inner_ty.as_garg() else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                arg.value.span(),
                format!(
                    "{} cannot be the value of an Option",
                    inner_ty.spelling(self.records)
                ),
            ));
            return None;
        };
        let id = self.opt_enum(inner, arg.value.span())?;
        self.push(
            Instr::EnumConstruct {
                enum_idx: id.index(),
                variant: OPTION_SOME,
            },
            span,
        );
        Some(LTy::Enum {
            ty: id,
            optional: false,
        })
    }

    /// The single positional argument of a `some`/`ok`/`err` constructor call.
    fn single_ctor_arg<'e>(&mut self, expr: &'e Expression, name: &str) -> Option<&'e Expression> {
        let Expression::Call { args, .. } = expr else {
            return None;
        };
        match args.as_slice() {
            [arg] if arg.name.is_none() => Some(&arg.value),
            _ => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    expr.span(),
                    format!("`{name}` takes exactly one value, as `{name}(value)`"),
                ));
                None
            }
        }
    }

    /// Lower prefix `try <expr>`: propagate a `Result<T, E>`'s `err` out of the
    /// enclosing `Result[U, E]`-returning function (same `E`, no conversion),
    /// yielding the `ok` value `T`. Dispatches on the tag: on `err` it rebuilds the
    /// error in the return `Result` and returns; on `ok` it extracts the value.
    pub(super) fn lower_try(&mut self, inner: &Expression, span: SourceSpan) -> Option<LTy> {
        if self.terminal_rejection() {
            return None;
        }
        let inner_ty = self.lower_expr(inner)?;
        let Some(src_id) = inner_ty.bare_enum() else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                inner.span(),
                format!(
                    "`try` needs a Result value, found {}",
                    inner_ty.spelling(self.records)
                ),
            ));
            return None;
        };
        let ret_id = match self.ret {
            RetType::Value(ty) => ty.bare_enum(),
            RetType::Unit => None,
        };
        let classified = self.records.with_metadata_session(|session| {
            let source = session.reserved_instantiation(src_id)?;
            let ret = match (source, ret_id) {
                (Some(ReservedEnumArgs::Result(_, _)), Some(id)) => {
                    session.reserved_instantiation(id)?.map(|args| (id, args))
                }
                _ => None,
            };
            Ok::<_, LowerInvariant>((source, ret))
        });
        let (source, ret_result) = self.accept_resolution(
            classified.map_err(ResolveError::Invariant),
            inner.span(),
            "this try operand",
        )?;
        let Some(ReservedEnumArgs::Result(t_arg, e_arg)) = source else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                inner.span(),
                format!(
                    "`try` needs a Result value, found {}",
                    inner_ty.spelling(self.records)
                ),
            ));
            return None;
        };
        let Some((ret_id, ReservedEnumArgs::Result(_, ret_err))) = ret_result else {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                "`try` is only valid in a function that returns a Result".to_string(),
            ));
            return None;
        };
        if ret_err != e_arg {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                span,
                format!(
                    "`try` propagates the error type {}, but the function returns {}",
                    garg_spelling(e_arg, self.records),
                    garg_spelling(ret_err, self.records)
                ),
            ));
            return None;
        }
        let slot = self.alloc_slot();
        self.push(Instr::LocalSet(slot), span);
        self.push(Instr::LocalGet(slot), span);
        self.push(Instr::EnumTag, span);
        let err_tag = self.draft.intern_int(i64::from(RESULT_ERR));
        self.push(Instr::ConstLoad(err_tag.index()), span);
        self.push(Instr::EqInt, span);
        // False (not err, i.e. ok) jumps to the ok extraction; true (err) falls
        // through to rebuild the error in the return Result and return it.
        let to_ok = self.push_jif(span);
        self.push(Instr::LocalGet(slot), span);
        self.push(
            Instr::EnumPayloadGet {
                variant: RESULT_ERR,
                field: 0,
            },
            span,
        );
        self.push(
            Instr::EnumConstruct {
                enum_idx: ret_id.index(),
                variant: RESULT_ERR,
            },
            span,
        );
        self.push(Instr::Return, span);
        let ok_here = self.here();
        self.patch(to_ok, ok_here);
        self.push(Instr::LocalGet(slot), span);
        self.push(
            Instr::EnumPayloadGet {
                variant: RESULT_OK,
                field: 0,
            },
            span,
        );
        Some(garg_to_lty(t_arg))
    }

    fn lower_field(&mut self, base: &Expression, name: &str, span: SourceSpan) -> Option<LTy> {
        let base_ty = self.lower_expr(base)?;
        let (index, field_ty, required) =
            self.resolve_product_field(base_ty, name, base.span(), span)?;
        self.push(Instr::FieldGet(index), span);
        let bare = garg_to_lty(field_ty);
        Some(if required { bare } else { bare.to_optional() })
    }

    /// Lower `base?.name`: a member read through an *optional composite value*. The
    /// base is an optional record/struct value (local, or the value of a durable
    /// read); an absent base short-circuits the whole read to absent, and a present
    /// base yields the field wrapped optional. The result is always optional, so
    /// `?.` is the present-propagating analogue of `.` — its one meaning. This is a
    /// local-value operator: a durable address propagates absence structurally on
    /// its own and needs no `?.`.
    fn lower_optional_field(
        &mut self,
        base: &Expression,
        name: &str,
        span: SourceSpan,
    ) -> Option<LTy> {
        let base_ty = self.lower_expr(base)?;
        if !base_ty.is_optional() {
            self.fail(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                base.span(),
                format!(
                    "`?.` needs an optional value on the left, found {}; use `.` for a \
                     present value",
                    base_ty.spelling(self.records)
                ),
            ));
            return None;
        }
        let (index, field_ty, required) =
            self.resolve_product_field(base_ty.to_bare(), name, base.span(), span)?;
        let result = garg_to_lty(field_ty).to_optional();

        // Present: unwrap the optional composite to its bare record and read the
        // field; a required field is wrapped present, a sparse field already reads
        // optional. Absent: short-circuit to a vacant of the result type. Both paths
        // join at `result`.
        let to_absent = self.push_branch_present(base.span());
        self.push(Instr::FieldGet(index), span);
        if required {
            self.push(Instr::SomeWrap, span);
        }
        let to_end = self.push_jump(span);
        let absent = self.here();
        self.patch(to_absent, absent);
        self.push(Instr::VacantLoad(result.image()), span);
        let end = self.here();
        self.patch(to_end, end);
        Some(result)
    }

    /// Resolve `name` against a bare product (`record` or `struct`) value type to
    /// its slot index, bare value type, and required flag. The one owner of product
    /// field resolution, shared by field reads, assignments, and `unset`.
    /// `base_span` locates a non-product base; `field_span` locates an unknown field.
    pub(super) fn resolve_product_field(
        &mut self,
        base_ty: LTy,
        name: &str,
        base_span: SourceSpan,
        field_span: SourceSpan,
    ) -> Option<(u16, GArg, bool)> {
        match base_ty {
            LTy::Record {
                ty,
                optional: false,
            } => {
                let projection = self.accept_resolution(
                    self.records
                        .product_field_projection(ty, name)
                        .map_err(ResolveError::Invariant),
                    field_span,
                    "this record field access",
                )?;
                match projection {
                    ProductFieldProjection::Field {
                        index,
                        ty,
                        required,
                    } => return Some((index, ty, required)),
                    ProductFieldProjection::Group { index, ty } => {
                        return Some((index, GArg::Group(ty), true));
                    }
                    ProductFieldProjection::MissingRecordField => {
                        // A keyed branch of the resource this whole-entry record materializes
                        // from is not a projectable field: steer to the durable-path form
                        // rather than reporting a bare missing field.
                        if let Some(root) = self.durable.root_by_record(ty)
                            && root.branch(name).is_some()
                        {
                            self.fail(branch_not_a_field(
                                self.file,
                                field_span,
                                name,
                                &root.resource,
                                &root.name,
                            ));
                            return None;
                        }
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            field_span,
                            format!("record has no field `{name}`"),
                        ));
                        return None;
                    }
                    ProductFieldProjection::MissingGroupField => {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            field_span,
                            format!("group has no field `{name}`"),
                        ));
                        return None;
                    }
                    ProductFieldProjection::Absent => {}
                }
                // A materialized keyed branch entry value (from `if const n =
                // ^root(k).branch(bk)`) is an image record the resource registry does not
                // own; resolve its scalar fields against the branch's field layout.
                if let Some(branch) = self.durable.branch_by_record(ty) {
                    let Some((index, field)) = branch.field_index(name) else {
                        // A sub-branch of this materialized branch entry is a distinct durable
                        // node, not a field: steer to the durable-path form, the same as a
                        // top-level branch off a whole-entry record.
                        if branch.branch(name).is_some() {
                            self.fail(subbranch_not_a_field(self.file, field_span, name));
                            return None;
                        }
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            field_span,
                            format!("record has no field `{name}`"),
                        ));
                        return None;
                    };
                    return Some((index, GArg::Scalar(field.scalar), field.required));
                }
                self.fail(unsupported(self.file, field_span, "this field access"));
                None
            }
            LTy::Struct {
                ty,
                optional: false,
            } => {
                let projection = self.accept_resolution(
                    self.records
                        .struct_field_projection(ty, name)
                        .map_err(ResolveError::Invariant),
                    field_span,
                    "this struct field access",
                )?;
                match projection {
                    StructFieldProjection::Field { index, ty } => Some((index, ty, true)),
                    StructFieldProjection::Missing => {
                        self.fail(SourceDiagnostic::at(
                            Code::CheckType.as_str(),
                            self.file,
                            field_span,
                            format!("`{}` has no field `{name}`", base_ty.spelling(self.records)),
                        ));
                        None
                    }
                    StructFieldProjection::Absent => {
                        self.reject_resolution(
                            ResolveError::Invariant(LowerInvariant::ReadyBodyMissing(
                                TypeInstId::Record(ty),
                            )),
                            field_span,
                            "this struct field access",
                        );
                        None
                    }
                }
            }
            _ => {
                self.fail(SourceDiagnostic::at(
                    Code::CheckType.as_str(),
                    self.file,
                    base_span,
                    format!(
                        "field access needs a record or struct, found {}",
                        base_ty.spelling(self.records)
                    ),
                ));
                None
            }
        }
    }
}

/// The canonical hover display of a resolved generic function callee, from its source
/// template: `fn name<T>(p: Ty): ret` with the declared type parameters and declared
/// parameter and return spellings. A generic call targets its source template, never a
/// minted instance.
fn generic_signature_display(decl: &FunctionDecl) -> String {
    let type_params = if decl.type_params.is_empty() {
        String::new()
    } else {
        let names = decl
            .type_params
            .iter()
            .map(|param| param.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        format!("<{names}>")
    };
    let params = decl
        .params
        .iter()
        .map(|param| param.ty.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    match &decl.return_type {
        None => format!("fn {}{type_params}({params})", decl.name),
        Some(ret) => format!("fn {}{type_params}({params}): {ret}", decl.name),
    }
}

/// The canonical hover display of a resolved function callee: `fn name(p1, p2): ret`
/// with the resolved concrete parameter and return types. A unit return omits the
/// `: ret`. Effects and demand are not shown.
fn signature_display(name: &str, params: &[LTy], ret: RetType, records: &TypeRegistry) -> String {
    let params = params
        .iter()
        .map(|param| param.spelling(records))
        .collect::<Vec<_>>()
        .join(", ");
    match ret {
        RetType::Unit => format!("fn {name}({params})"),
        RetType::Value(ty) => format!("fn {name}({params}): {}", ty.spelling(records)),
    }
}
