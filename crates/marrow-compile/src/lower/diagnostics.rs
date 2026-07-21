//! Typed diagnostic builders and the small literal-shape helpers lowering reports through.

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

pub(super) fn unsupported(
    file: &FileIdentity,
    span: SourceSpan,
    subject: &str,
) -> SourceDiagnostic {
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
pub(super) fn not_yet_executable(
    file: &FileIdentity,
    span: SourceSpan,
    root: &str,
) -> SourceDiagnostic {
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

/// A keyed branch named where a field of a materialized entry record is expected — the
/// `b.notes[…]` chain off `if const b = ^root(k)`. A branch is a distinct durable node, not
/// a projection of the whole-entry value, so the message steers to the durable-path form.
/// `root` is the store-root source name (`books` for `^books`); `resource` is the resource
/// the record materializes; `branch` is the branch member in source spelling.
pub(super) fn branch_not_a_field(
    file: &FileIdentity,
    span: SourceSpan,
    branch: &str,
    resource: &str,
    root: &str,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!(
            "`{branch}` is a keyed branch of `{resource}`, not a field of the bound entry \
             value. A keyed branch is a distinct durable node reached through a store path, \
             not projected from a materialized record. Read it directly with \
             `^{root}[key].{branch}[branchKey]`, or bind the branch with a nested `if const`."
        ),
    )
}

/// A keyed sub-branch named on a materialized branch entry value (the `n.replies[…]` chain
/// off `if const n = ^root(k).notes(nk)`). Like a top-level branch it is a distinct durable
/// node, not a field; the concrete root spelling is not in hand here, so the message steers
/// to the durable-path form generically.
pub(super) fn subbranch_not_a_field(
    file: &FileIdentity,
    span: SourceSpan,
    branch: &str,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!(
            "`{branch}` is a keyed branch, not a field of the bound entry value. A keyed \
             branch is a distinct durable node reached through a store path, not projected \
             from a materialized record. Read it through its durable path, or bind it with a \
             nested `if const`."
        ),
    )
}

/// `absent` used as an operand of `==`/`!=`. Presence is a distinct question with one
/// canonical vocabulary (`if const` / `??` / `exists`); a second equality-shaped spelling
/// is not admitted, so the message steers to the presence forms rather than reporting the
/// generic uninferable-`absent` type error. Points at the `absent` operand span.
pub(super) fn absent_not_operand(
    file: &FileIdentity,
    span: SourceSpan,
    op: BinaryOp,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
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

pub(super) fn name_error(file: &FileIdentity, span: SourceSpan, name: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("`{name}` is not in scope"),
    )
}

/// Which family an unresolved name was looked up in, so a did-you-mean names the kind
/// of thing the suggested identifier is: a store root reads back with its `^` sigil, a
/// function or a value reads back plainly.
#[derive(Clone, Copy)]
pub(super) enum NameKind {
    Root,
    Function,
    Value,
}

/// An unresolved name, offering the nearest declared identifier of the same family when
/// one is a close misspelling. Without a suggestion this is exactly [`name_error`]; the
/// suggestion, when present, spells the candidate in its family's form so the fix is a
/// single edit the reader can apply directly.
pub(super) fn name_not_in_scope(
    file: &FileIdentity,
    span: SourceSpan,
    name: &str,
    suggestion: Option<&str>,
    kind: NameKind,
) -> SourceDiagnostic {
    let mut message = format!("`{name}` is not in scope");
    if let Some(candidate) = suggestion {
        let hint = match kind {
            NameKind::Root => format!(". Did you mean the store root `^{candidate}`?"),
            NameKind::Function => format!(". Did you mean the function `{candidate}`?"),
            NameKind::Value => format!(". Did you mean `{candidate}`?"),
        };
        message.push_str(&hint);
    }
    SourceDiagnostic::at(Code::CheckType.as_str(), file, span, message)
}

/// The single declared name within edit distance two of `target`, or `None` when none
/// is that close or two candidates tie for nearest. Deliberately conservative: a
/// did-you-mean earns its place only as one unambiguous suggestion, never a list. A
/// candidate must also be closer than a full rewrite (`distance < target length`), so a
/// short name does not match an unrelated one.
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

/// A reference to a store root whose durable identity failed admission: the root was
/// declared but each identity gap was reported as `check.durable_identity`, so it dropped
/// from the registry. Reporting a bare not-in-scope name here would misdirect toward a
/// typo; instead the reference site names the admission failure and points at the identity
/// reports. A genuinely undeclared root keeps the plain [`name_error`].
pub(super) fn identity_admission_failed(
    file: &FileIdentity,
    span: SourceSpan,
    name: &str,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!(
            "`{name}` was declared but failed identity admission; see the \
             `check.durable_identity` reports"
        ),
    )
}

pub(super) fn checked_arm_error(
    file: &FileIdentity,
    span: SourceSpan,
    detail: &str,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("this checked form {detail}"),
    )
}

pub(super) fn loop_error(file: &FileIdentity, span: SourceSpan, keyword: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("`{keyword}` is not inside a loop"),
    )
}

pub(super) fn type_mismatch(
    records: &TypeRegistry,
    file: &FileIdentity,
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
    file: &FileIdentity,
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
    file: &FileIdentity,
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
    file: &FileIdentity,
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
        // A shadowing local presents the same candidate name twice; that is one
        // unambiguous suggestion, not an ambiguous tie.
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
