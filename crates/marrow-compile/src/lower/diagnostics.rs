use super::*;

/// Whether `ty` is a value that renders to canonical text — a bare scalar, enum, or
/// entry identity. A record, collection, or optional is not renderable; those are not
/// interpolation holes and cannot ride `string(...)`.
pub(super) fn is_interpolable(ty: LTy) -> bool {
    matches!(
        ty,
        LTy::Scalar {
            optional: false,
            ..
        } | LTy::Enum {
            optional: false,
            ..
        } | LTy::Identity {
            optional: false,
            ..
        }
    )
}

/// Whether `expr` is an integer literal, possibly negated, whose value is provably
/// nonzero. A checked `/`/`%` with such a divisor cannot fault with a zero divisor, so
/// the `on zero_divisor` arm is dead. A non-literal divisor is assumed possibly zero.
pub(super) fn divisor_nonzero_literal(expr: &Expression) -> bool {
    let literal = match expr {
        Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } => Some(text),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => match operand.as_ref() {
            Expression::Literal {
                kind: LiteralKind::Integer,
                text,
                ..
            } => Some(text),
            _ => None,
        },
        _ => None,
    };
    literal
        .and_then(|text| parse_int(text))
        .is_some_and(|value| value != 0)
}

/// Fold a duration word literal `COUNT UNIT` to signed nanoseconds: the count times
/// the unit's whole seconds times a second in nanoseconds. Returns `None` when the
/// shape is unexpected or the product leaves the representable range.
pub(super) fn duration_words_nanos(text: &str) -> Option<i128> {
    let mut parts = text.split_whitespace();
    let (Some(count), Some(unit), None) = (parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    let count = i128::from(parse_int(count)?);
    let seconds = i128::from(duration_unit_seconds(unit)?);
    count.checked_mul(seconds)?.checked_mul(1_000_000_000)
}

/// The rendered index text of a statically dead list index literal — `0` or any
/// negative literal — or `None` when the key is not a dead-index literal. List
/// positions are 1-based, so a literal `0` or negative names no position and is
/// refused at check time; a positive literal past the length is not statically dead
/// (the length is a runtime fact) and reads absent instead.
pub(super) fn dead_list_index_literal(key: &Expression) -> Option<String> {
    match key {
        Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } if parse_int(text) == Some(0) => Some("0".to_string()),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => match operand.as_ref() {
            Expression::Literal {
                kind: LiteralKind::Integer,
                text,
                ..
            } => Some(format!("-{}", text.replace('_', ""))),
            _ => None,
        },
        _ => None,
    }
}

/// The bare local name of a bracket base for a teaching diagnostic (`xs` in `xs[0]`),
/// or `None` when the base is a compound expression that has no single-name spelling.
pub(super) fn simple_base_label(base: &Expression) -> Option<&str> {
    match base {
        Expression::Name { segments, .. } => match segments.as_slice() {
            [name] => Some(name.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// The source spelling of a simple assigned value, for a fix line that names the
/// user's own right-hand side (`v` in `xs[i] = v`, `9` in `xs[1] = 9`). Compound
/// expressions have no short spelling here; the caller falls back to the canonical
/// `_` placeholder.
pub(super) fn simple_value_spelling(value: &Expression) -> Option<String> {
    match value {
        Expression::Name { segments, .. } => match segments.as_slice() {
            [name] => Some(name.clone()),
            _ => None,
        },
        Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } => Some(text.clone()),
        Expression::Unary {
            op: UnaryOp::Neg,
            operand,
            ..
        } => match operand.as_ref() {
            Expression::Literal {
                kind: LiteralKind::Integer,
                text,
                ..
            } => Some(format!("-{text}")),
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn unsupported(file: &str, span: SourceSpan, subject: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        span,
        format!("{subject} is not yet supported on the beta line"),
    )
}

/// The store-root name a durable address expression bottoms out at: the leftmost
/// `^name` leaf reached through keyed accesses and field/branch selectors, or `None`
/// when `expr` is not rooted at a `SavedRoot` (not a `^root` durable access at all).
/// The one owner that extracts which store an address names, so the resolvers dispatch
/// against a single root lookup.
pub(super) fn saved_root_name(expr: &Expression) -> Option<&str> {
    match expr {
        Expression::SavedRoot { name, .. } => Some(name),
        Expression::Keyed { base, .. } | Expression::Field { base, .. } => saved_root_name(base),
        _ => None,
    }
}

/// Whether `expr` is a durable whole-entry address `^root[key]….b[bkey]` at any depth: a
/// keyed access whose base bottoms out at the store root, chained through branch
/// selectors. The single syntactic recognizer of a durable entry address; the resolver
/// rechecks the store and branch names.
pub(super) fn is_entry_address(expr: &Expression) -> bool {
    let Expression::Keyed { base, .. } = expr else {
        return false;
    };
    match base.as_ref() {
        Expression::SavedRoot { .. } => true,
        Expression::Field { base, .. } => is_entry_address(base),
        _ => false,
    }
}

/// Whether `expr` is a durable field-exact address `<entry-address>.field` at any depth: a
/// field selection on an entry address. A whole root-level group `^root(k).group` has the
/// same shape; the resolver tells a group from a field by name.
pub(super) fn is_field_address(expr: &Expression) -> bool {
    matches!(expr, Expression::Field { base, .. } if is_entry_address(base))
}

/// Whether `expr` is a durable group-leaf address `^root(k).group.leaf`: a field selection
/// whose base is itself a field-of-an-entry-address (the whole-group address). The resolver
/// confirms the middle selector names a root-level group; a base that turns out to be a
/// stored field is a clean resolution failure, not a group leaf.
pub(super) fn is_group_leaf_address(expr: &Expression) -> bool {
    matches!(expr, Expression::Field { base, .. } if is_field_address(base))
}

/// A durable operation over a declared-but-not-executable root (a singleton root, a root
/// whose resource declares a nominal-typed field, or one whose only durable content is a
/// group nested in a branch or another group): the shape's identity is complete and in the
/// image, but the kernel does not yet serve it, so the operation is rejected precisely
/// rather than silently dropped. Keyed roots — single-column or a composite tuple — whose
/// top-level fields are scalars or widened values (`struct`/`enum`/`Option`), their
/// root-level `group` members, and their `branch` placements, are executable.
pub(super) fn not_yet_executable(file: &str, span: SourceSpan, root: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        span,
        format!(
            "durable operations over `^{root}` are not yet executable: a singleton root, a root \
             whose resource declares a nominal-typed field, or a group nested in a branch or \
             another group, declares and verifies its identity but cannot yet be read or written"
        ),
    )
}

pub(super) fn name_error(file: &str, span: SourceSpan, name: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("`{name}` is not in scope"),
    )
}

pub(super) fn checked_arm_error(file: &str, span: SourceSpan, detail: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("this checked form {detail}"),
    )
}

pub(super) fn loop_error(file: &str, span: SourceSpan, keyword: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("`{keyword}` is not inside a loop"),
    )
}

pub(super) fn type_mismatch(
    records: &TypeRegistry,
    file: &str,
    span: SourceSpan,
    found: LTy,
    want: LTy,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!(
            "found {} where {} is required",
            found.spelling(records),
            want.spelling(records)
        ),
    )
}

pub(super) fn unary_error(
    records: &TypeRegistry,
    file: &str,
    span: SourceSpan,
    verb: &str,
    ty: LTy,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("cannot {verb} {}", ty.spelling(records)),
    )
}

pub(super) fn binary_error(
    records: &TypeRegistry,
    file: &str,
    span: SourceSpan,
    op: BinaryOp,
    left: LTy,
    right: LTy,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!(
            "`{}` is not defined for {} and {}",
            operator_symbol(op),
            left.spelling(records),
            right.spelling(records)
        ),
    )
}

pub(super) fn logic_operand(
    records: &TypeRegistry,
    file: &str,
    span: SourceSpan,
    op: BinaryOp,
    ty: LTy,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!(
            "`{}` operand must be bool, found {}",
            operator_symbol(op),
            ty.spelling(records)
        ),
    )
}
