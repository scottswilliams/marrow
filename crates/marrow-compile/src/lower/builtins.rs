//! Built-in and constructor classification: the `Builtin`/`CtorKind` vocabulary and the free classifiers over call syntax.

use super::*;

/// The bare lowered type a built-in generic argument denotes (the inverse of
/// [`LTy::as_garg`] over the value cases).
pub(super) fn garg_to_lty(arg: GArg) -> LTy {
    match arg {
        GArg::Scalar(scalar) => LTy::bare_scalar(scalar),
        GArg::Nominal(id) => LTy::Nominal {
            id,
            optional: false,
        },
        GArg::Struct(ty) => LTy::Struct {
            ty,
            optional: false,
        },
        // A group value materializes as a nested record; its leaves resolve through
        // the group owner (`group_by_type`), reached from the record field path.
        GArg::Group(ty) => LTy::Record {
            ty,
            optional: false,
        },
        GArg::Enum(ty) => LTy::Enum {
            ty,
            optional: false,
        },
        GArg::Collection(idx) => LTy::Collection {
            idx,
            optional: false,
        },
        GArg::Param(index) => LTy::Param {
            index,
            optional: false,
        },
    }
}

/// One constructor member plan entry: a field or group leaf's name, value type, and
/// required flag, collected before emission so evaluation follows declaration order.
pub(super) type MemberPlan = (String, GArg, bool);

/// One group slot's constructor plan: the group's name, its materialized-value
/// record type, whether it has a required leaf (so an omitted argument cannot be
/// auto-completed), and the plan of its leaves.
pub(super) type GroupPlan = (String, TypeId, bool, Vec<MemberPlan>);

/// The source spelling of a built-in generic argument, recursing through nested
/// `Option`/`Result` arguments.
pub(super) fn garg_spelling(arg: GArg, records: &TypeRegistry) -> String {
    garg_to_lty(arg).spelling(records)
}

/// A built-in `Option`/`Result` constructor form in expression position. The
/// constructor names are reserved, so any `none`, `some(_)`, `ok(_)`, or `err(_)`
/// is this built-in rather than a name or call the surrounding scope resolves.
#[derive(Debug, Clone, Copy)]
pub(super) enum CtorKind {
    None,
    Some,
    Ok,
    Err,
}

impl CtorKind {
    pub(super) fn name(self) -> &'static str {
        match self {
            CtorKind::None => "none",
            CtorKind::Some => "some",
            CtorKind::Ok => "ok",
            CtorKind::Err => "err",
        }
    }
}

/// A value-level built-in the compiler intercepts before user resolution: the
/// `Option`/`Result` constructors (`none`/`some`/`ok`/`err`), the presence test
/// (`exists`), the divergence marker (`unreachable`), and the pure text floor
/// (`isEmpty`/`contains`/`trim`/`split`/`lines`/`join`). None of these spellings is
/// a keyword, so the parser admits them as identifiers; the reservation is enforced
/// here instead.
///
/// This enum is the single owner of that name set. Call interception dispatches
/// on `from_name` (see `lower_unqualified_call`), and declaration rejection
/// consults the same classifier through [`is_reserved_builtin_name`], so a name
/// that is intercepted at a use site can never be silently shadowed by a
/// colliding value declaration. Adding a built-in is a new variant, which the
/// exhaustive dispatch match forces every consumer to account for.
#[derive(Debug, Clone, Copy)]
pub(super) enum Builtin {
    None,
    Some,
    Ok,
    Err,
    Exists,
    Unreachable,
    Todo,
    IsEmpty,
    Contains,
    Trim,
    /// The collection-returning text floor: `split(text, sep): List[string]`,
    /// `lines(text): List[string]`, `join(List[string], sep): string`. Like the rest
    /// of the floor these are reserved, so a colliding value declaration is rejected;
    /// they mint the `List[string]` COLLTYPES instantiation their result or argument
    /// names.
    Split,
    Lines,
    Join,
    /// The named temporal arithmetic floor: `addDays(date, int): date` and
    /// `daysBetween(date, date): int`. Named rather than operators so a date
    /// offset never reads as an ambiguous `date + int`; they are reserved, so a
    /// colliding value declaration is rejected. `marrow-temporal` owns the checked
    /// operations, which fault `run.temporal_overflow` past the supported range.
    DateAddDays,
    DateDaysBetween,
    /// The empty-collection constructors `List()`/`Map()`, type-directed by the
    /// expected type. They are reserved (blocking a colliding value declaration)
    /// because a bare `List`/`Map` at a use site is always the built-in constructor.
    /// The procedural collection operations (`append`/`insert`/`get`/`length`) are
    /// deliberately *not* reserved: they are common verbs, so a same-module function
    /// of that name wins and the collection op is a fallback (see
    /// [`FnLowerer::lower_collection_fallback`]).
    List,
    Map,
    /// The entry-identity constructor `Id(^root, keys…)`: a nominal value constructor
    /// wrapping the explicit key tuple as an `Id(^root)`. Reserved so a colliding value
    /// declaration is rejected; the leading `^root` argument is a saved-root reference,
    /// not an ordinary value, so it is dispatched to its own lowering.
    Id,
}

impl Builtin {
    pub(super) fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "none" => Builtin::None,
            "some" => Builtin::Some,
            "ok" => Builtin::Ok,
            "err" => Builtin::Err,
            "exists" => Builtin::Exists,
            "unreachable" => Builtin::Unreachable,
            "todo" => Builtin::Todo,
            "isEmpty" => Builtin::IsEmpty,
            "contains" => Builtin::Contains,
            "trim" => Builtin::Trim,
            "split" => Builtin::Split,
            "lines" => Builtin::Lines,
            "join" => Builtin::Join,
            "addDays" => Builtin::DateAddDays,
            "daysBetween" => Builtin::DateDaysBetween,
            "List" => Builtin::List,
            "Map" => Builtin::Map,
            "Id" => Builtin::Id,
            _ => return None,
        })
    }
}

/// Whether `name` is a reserved value-level built-in that a `fn`, `const`,
/// parameter, or local binding may not redeclare. A colliding value declaration
/// would be admitted and then silently shadowed at every use site the compiler
/// intercepts (`some(v)`, bare `none`, `trim(s)`, ...), surfacing later as a
/// confusing type error; rejecting the declaration keeps the reserved name and
/// its interception the single fact.
///
/// Struct fields and enum variants are excluded: both are reached only through
/// member syntax (`r.none`, `Color::err`), never a bare or unqualified-call use,
/// so they cannot collide with an intercepted built-in.
pub(crate) fn is_reserved_builtin_name(name: &str) -> bool {
    Builtin::from_name(name).is_some()
}

/// The closed set of value-level built-in spellings, in a stable order, for the editor
/// completion namespace. This is the same set [`Builtin::from_name`] classifies; the
/// single owner exposes it here so the completion enumerator does not restate the list.
pub(crate) fn builtin_value_names() -> &'static [&'static str] {
    &[
        "none",
        "some",
        "ok",
        "err",
        "exists",
        "unreachable",
        "todo",
        "isEmpty",
        "contains",
        "trim",
        "split",
        "lines",
        "join",
        "addDays",
        "daysBetween",
        "List",
        "Map",
        "Id",
    ]
}

/// The diagnostic for a value declaration whose name is a reserved built-in.
pub(crate) fn reserved_builtin_name(
    file: &FileIdentity,
    span: SourceSpan,
    name: &str,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckNameConflict.as_str(),
        file,
        span,
        format!("`{name}` is a built-in and cannot be redeclared"),
    )
}

/// Classify an expression as a collection constructor call on the reserved type name
/// `List`/`Map`, returning the head and its positional arguments. An empty argument
/// list is the empty constructor; a non-empty one is variadic list construction (the
/// map literal is deferred, rejected by the ctor lowering).
pub(super) fn collection_ctor_call(expr: &Expression) -> Option<(&'static str, &[Argument])> {
    let Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    match &**callee {
        Expression::Name { segments, .. } => match segments.as_slice() {
            [n] if n == "List" => Some(("List", args)),
            [n] if n == "Map" => Some(("Map", args)),
            _ => None,
        },
        _ => None,
    }
}

/// The diagnostic for a built-in called with the wrong argument shape.
pub(super) fn builtin_arity(
    file: &FileIdentity,
    span: SourceSpan,
    name: &str,
    arity: usize,
) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckType.as_str(),
        file,
        span,
        format!("`{name}` takes {arity} positional argument(s)"),
    )
}

/// Classify an expression as a built-in constructor form: bare `none`, or a call
/// `some(..)`/`ok(..)`/`err(..)`. Returns `None` for anything else.
pub(super) fn constructor_kind(expr: &Expression) -> Option<CtorKind> {
    match expr {
        Expression::Name { segments, .. } if matches!(segments.as_slice(), [n] if n == "none") => {
            Some(CtorKind::None)
        }
        Expression::Call { callee, .. } => match &**callee {
            Expression::Name { segments, .. } => match segments.as_slice() {
                [n] if n == "some" => Some(CtorKind::Some),
                [n] if n == "ok" => Some(CtorKind::Ok),
                [n] if n == "err" => Some(CtorKind::Err),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// Split a dotted constructor base into its single-segment head name (with span) and the
/// branch-name chain before the final call segment. `Book` yields `("Book", span, [])`;
/// `Book.notes` yields `("Book", span, ["notes"])`; deeper chains accumulate. `None` for a
/// head that is not a single-segment name (a `::`-qualified or otherwise non-head base).
pub(super) fn split_dotted_head(expr: &Expression) -> Option<(&str, SourceSpan, Vec<&str>)> {
    match expr {
        Expression::Name { segments, span, .. } if segments.len() == 1 => {
            Some((segments[0].as_str(), *span, Vec::new()))
        }
        Expression::Field { base, name, .. } => {
            let (head, span, mut names) = split_dotted_head(base)?;
            names.push(name.as_str());
            Some((head, span, names))
        }
        _ => None,
    }
}

/// The source-shaped display of a branch constructor head, `Resource.b1.….bn`, for a
/// diagnostic.
pub(super) fn branch_ctor_display(resource: &str, path: &[&str]) -> String {
    std::iter::once(resource)
        .chain(path.iter().copied())
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::{Builtin, builtin_value_names};

    /// The editor completion namespace is exactly the set the classifier recognizes: every
    /// listed name classifies, so a new built-in cannot be added to the classifier without
    /// appearing in the completion namespace.
    #[test]
    fn completion_names_match_the_classifier() {
        for name in builtin_value_names() {
            assert!(
                Builtin::from_name(name).is_some(),
                "`{name}` is offered for completion but is not a classified built-in",
            );
        }
        assert_eq!(builtin_value_names().len(), 18);
    }
}
