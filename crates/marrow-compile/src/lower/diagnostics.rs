//! Typed diagnostic builders and the small literal-shape helpers lowering reports
//! through.

use super::*;
use crate::diag::{RefusedDeclaration, Steer, TypeMismatch, TypeSpelling, Unresolved};

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
/// negative literal — or `None` otherwise. List positions are 1-based, so those name no
/// position and are refused at check time; a positive literal past the length is not
/// statically dead (the length is a runtime fact) and reads absent instead.
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
        Expression::Name { segments, .. } => match &segments[..] {
            [name] => Some(name.text()),
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
        Expression::Name { segments, .. } => match &segments[..] {
            [name] => Some(name.text().to_string()),
            _ => None,
        },
        Expression::Literal {
            kind: LiteralKind::Integer,
            text,
            ..
        } => Some(text.to_string()),
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

/// The store-root name a durable address expression bottoms out at: the leftmost
/// `^name` leaf reached through keyed accesses and field/branch selectors, or `None`
/// when `expr` is not rooted at a `SavedRoot`. The one owner that extracts which store
/// an address names, so the resolvers dispatch against a single root lookup.
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
/// whose base is itself a field-of-an-entry-address. The resolver confirms the middle
/// selector names a root-level group; a stored field there is a clean resolution failure.
pub(super) fn is_group_leaf_address(expr: &Expression) -> bool {
    matches!(expr, Expression::Field { base, .. } if is_field_address(base))
}

/// Reject an operation on a root whose declared shape is not executable.
/// Binding diagnostics may independently refuse publication of the same root.
pub(super) fn not_yet_executable(
    file: &ProjectFile,
    span: SourceSpan,
    root: &str,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported,
        file,
        span,
        format!(
            "durable operations over `^{root}` are not yet executable for singleton roots, \
             nominal-bearing resources, or groups nested in a branch or another group"
        ),
    )
}

/// A keyed branch named where a field of a materialized entry record is expected — the
/// `b.notes[…]` chain off `if const b = ^root(k)`, or the `n.tags[…]` chain off a
/// materialized branch entry, which carries no declaring resource. A branch is a distinct
/// durable node, not a projection of the bound value, so the row steers to the
/// durable-path form.
pub(super) fn branch_not_a_field(
    file: &ProjectFile,
    span: SourceSpan,
    branch: &str,
    resource: Option<&str>,
) -> SourceDiagnostic {
    SourceDiagnostic::with_steer(
        Code::CheckType,
        file,
        span,
        Steer::KeyedBranch {
            branch: branch.to_string(),
            resource: resource.map(str::to_string),
        },
    )
}

/// `absent` used as an operand of `==`/`!=`. Presence has one canonical vocabulary
/// (`if const` / `??` / `exists`); no equality-shaped spelling is admitted, so the message
/// steers to the presence forms rather than the generic uninferable-`absent` type error.
pub(super) fn absent_not_operand(
    file: &ProjectFile,
    span: SourceSpan,
    op: BinaryOp,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType,
        file,
        span,
        format!(
            "`absent` is not an operand of `{}`. Presence is a distinct question, asked with a \
             presence form rather than equality: guard the value with `if const x = _`, \
             coalesce with `?? _`, or test a durable path with `exists(...)`.",
            operator_symbol(op)
        ),
    )
}

/// An unresolved name: no declaration of `family` answers `name` here. The nearest
/// declared identifier of the same family rides along as a typed [`Steer::DidYouMean`]
/// when one is an unambiguous close misspelling, so the fix is a single edit.
///
/// The one builder of an unresolved-name row: every site that fails to resolve a name
/// reports through here, and the sentence itself is rendered by [`Unresolved`].
pub(super) fn name_not_in_scope(
    file: &ProjectFile,
    span: SourceSpan,
    family: NameFamily,
    name: &str,
    suggestion: Option<&str>,
) -> SourceDiagnostic {
    SourceDiagnostic::with_unresolved(
        file,
        span,
        Unresolved {
            family,
            name: name.to_string(),
        },
        suggestion.map(|candidate| Steer::DidYouMean {
            family,
            candidate: candidate.to_string(),
        }),
    )
}

/// The single declared name within edit distance two of `target`, or `None` when none
/// is that close or two candidates tie for nearest: a did-you-mean earns its address only
/// as one unambiguous suggestion, never a list. A candidate must also be closer than a
/// full rewrite (`distance < target length`), so a short name matches nothing unrelated.
pub(super) fn nearest_name<'n>(
    target: &str,
    candidates: impl Iterator<Item = &'n str>,
) -> Option<String> {
    let target_len = target.chars().count();
    let mut best: Option<usize> = None;
    let mut best_name: Option<&str> = None;
    let mut tied = false;
    for candidate in candidates {
        if candidate == target {
            return None;
        }
        let distance = edit_distance(target, candidate);
        if distance > 2 || distance >= target_len {
            continue;
        }
        match best {
            Some(current) if distance < current => {
                best = Some(distance);
                best_name = Some(candidate);
                tied = false;
            }
            // A shadowed local can present the same name twice; only a *different* name
            // at the same distance is a real tie that suppresses the suggestion.
            Some(current) if distance == current => tied |= best_name != Some(candidate),
            Some(_) => {}
            None => {
                best = Some(distance);
                best_name = Some(candidate);
            }
        }
    }
    if tied {
        None
    } else {
        best_name.map(str::to_string)
    }
}

/// The Levenshtein edit distance between two identifiers. Names are short, so the plain
/// two-row dynamic program is the right cost.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = prev[j] + usize::from(ca != cb);
            curr[j + 1] = substitution.min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// A reference to a store root whose durable identity failed admission: declared, but
/// each identity gap was reported as `check.durable_identity`, so it dropped from the
/// registry. A bare not-in-scope name would misdirect toward a typo, so the reference
/// site names the admission failure instead. A genuinely undeclared root keeps
/// [`name_not_in_scope`].
pub(super) fn identity_admission_failed(
    file: &ProjectFile,
    span: SourceSpan,
    namespace: DeclarationNamespace,
    refusal: &DeclarationRefusalSummary,
) -> SourceDiagnostic {
    let name = refusal.name();
    SourceDiagnostic::with_refused_declaration(
        Code::CheckType,
        file,
        span,
        format!(
            "`{name}` was declared but failed identity admission; see the \
             `check.durable_identity` reports"
        ),
        RefusedDeclaration {
            namespace,
            declaring_code: refusal.code(),
            report: refusal.report(),
        },
    )
}

pub(super) fn checked_arm_error(
    file: &ProjectFile,
    span: SourceSpan,
    detail: &str,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType,
        file,
        span,
        format!("this checked form {detail}"),
    )
}

pub(super) fn loop_error(file: &ProjectFile, span: SourceSpan, keyword: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType,
        file,
        span,
        format!("`{keyword}` is not inside a loop"),
    )
}

/// The refusal of a presence-dependent use. `detail` names why no proof holds here.
pub(crate) fn requires_presence(
    file: &ProjectFile,
    span: SourceSpan,
    detail: &str,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckRequiresPresence,
        file,
        span,
        format!(
            "this use requires a present entry, but {detail}. Bind \
             `ref entry = ^root[key] else {{ … }}` with a diverging absence arm. \
             A proof ends at its block's end, at a `delete` of any entry in the same family, \
             and at a call that erases that family. After invalidation, bind a fresh \
             reference or recheck with `if exists(entry) {{ … }}`. To create an entry, \
             assign the whole record to a direct path."
        ),
    )
}

/// A value whose type does not fit the position it is written in.
pub(super) fn type_mismatch(
    records: &TypeRegistry,
    file: &ProjectFile,
    span: SourceSpan,
    found: LTy,
    want: LTy,
) -> SourceDiagnostic {
    SourceDiagnostic::with_type_mismatch(
        file,
        span,
        TypeMismatch::Value {
            found: TypeSpelling::new(found.spelling(records)),
            expected: TypeSpelling::new(want.spelling(records)),
        },
        (found.is_optional() && !want.is_optional() && found.to_bare() == want)
            .then_some(Steer::Presence),
    )
}

/// A unary operator applied to an operand type it is not defined for. The operator fixes
/// the type it wants — `-` an int, `not` a bool — so only that one operand is a fact.
pub(super) fn unary_error(
    records: &TypeRegistry,
    file: &ProjectFile,
    span: SourceSpan,
    op: UnaryOp,
    ty: LTy,
) -> SourceDiagnostic {
    let wanted = match op {
        UnaryOp::Neg => LTy::bare_scalar(ScalarType::Int),
        UnaryOp::Not => LTy::bare_scalar(ScalarType::Bool),
    };
    SourceDiagnostic::with_type_mismatch(
        file,
        span,
        TypeMismatch::Unary {
            op,
            found: TypeSpelling::new(ty.spelling(records)),
        },
        (ty.is_optional() && ty.to_bare() == wanted).then_some(Steer::Presence),
    )
}

/// A binary operator defined for no pair of these operand types.
pub(super) fn binary_error(
    records: &TypeRegistry,
    file: &ProjectFile,
    span: SourceSpan,
    op: BinaryOp,
    left: LTy,
    right: LTy,
) -> SourceDiagnostic {
    // The presence steer is carried when the operands differ solely in presence — the
    // same bare type, at least one optional — so binding or coalescing the value is the
    // whole fix. A different bare type survives making the value present.
    let same_bare_type =
        (left.is_optional() || right.is_optional()) && left.to_bare() == right.to_bare();
    SourceDiagnostic::with_type_mismatch(
        file,
        span,
        TypeMismatch::Binary {
            op,
            left: TypeSpelling::new(left.spelling(records)),
            right: TypeSpelling::new(right.spelling(records)),
        },
        same_bare_type.then_some(Steer::Presence),
    )
}

/// A branch or guard condition that is not `bool`.
pub(super) fn condition_not_bool(
    records: &TypeRegistry,
    file: &ProjectFile,
    span: SourceSpan,
    ty: LTy,
) -> SourceDiagnostic {
    SourceDiagnostic::with_type_mismatch(
        file,
        span,
        TypeMismatch::Condition {
            found: TypeSpelling::new(ty.spelling(records)),
        },
        None,
    )
}

/// A `try` whose propagated error type is not the one its enclosing function returns.
pub(super) fn try_propagation_error(
    records: &TypeRegistry,
    file: &ProjectFile,
    span: SourceSpan,
    propagated: GArg,
    returns: GArg,
) -> SourceDiagnostic {
    SourceDiagnostic::with_type_mismatch(
        file,
        span,
        TypeMismatch::TryPropagation {
            propagated: TypeSpelling::new(garg_spelling(propagated, records)),
            returns: TypeSpelling::new(garg_spelling(returns, records)),
        },
        None,
    )
}

/// An `and`/`or` operand that is not `bool`.
pub(super) fn logic_operand(
    records: &TypeRegistry,
    file: &ProjectFile,
    span: SourceSpan,
    op: BinaryOp,
    ty: LTy,
) -> SourceDiagnostic {
    SourceDiagnostic::with_type_mismatch(
        file,
        span,
        TypeMismatch::LogicOperand {
            op,
            found: TypeSpelling::new(ty.spelling(records)),
        },
        // `and`/`or` require bool, so only a `bool?` operand is presence-fixable.
        (ty.is_optional() && ty.to_bare() == LTy::bare_scalar(ScalarType::Bool))
            .then_some(Steer::Presence),
    )
}

#[cfg(test)]
mod nearest_name_tests {
    use super::nearest_name;

    #[test]
    fn a_single_close_candidate_is_suggested() {
        assert_eq!(
            nearest_name("membrs", ["members", "assets", "idseq"].into_iter()),
            Some("members".to_string()),
        );
    }

    #[test]
    fn two_distinct_equally_close_candidates_suppress_the_suggestion() {
        // `cat` is edit distance one from both `car` and `bat`: ambiguous, so silent.
        assert_eq!(nearest_name("cat", ["car", "bat"].into_iter()), None);
    }

    #[test]
    fn a_shadowed_name_repeated_at_the_same_distance_is_not_a_tie() {
        assert_eq!(
            nearest_name("cache", ["cache", "cache"].into_iter()),
            None,
            "an exact match is never suggested",
        );
        assert_eq!(
            nearest_name("chache", ["cache", "cache", "count"].into_iter()),
            Some("cache".to_string()),
            "the duplicate candidate is deduped, not read as a tie",
        );
    }

    #[test]
    fn a_far_or_short_name_earns_no_suggestion() {
        assert_eq!(nearest_name("ghosts", ["members"].into_iter()), None);
        // A two-character name is fully rewritten at distance two, so it never matches.
        assert_eq!(nearest_name("ab", ["cd"].into_iter()), None);
    }
}
