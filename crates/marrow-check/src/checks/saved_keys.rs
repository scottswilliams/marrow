//! Key-argument typing for saved accesses: whole-record lookups, declared index
//! branches, and keyed layers, each checked against the keys it addresses. A
//! foreign identity spliced into a keyspace, or a scalar of the wrong type, is a
//! `check.key_type`.

use std::collections::HashMap;
use std::path::Path;

use marrow_schema::{ScalarType, StoreSchema};
use marrow_syntax::{Argument, SourceSpan};

use crate::diagnostics::CHECK_SEQUENCE_POSITION;
use crate::executable::{
    CheckedArg, CheckedExpr, CheckedLiteralKind, CheckedSavedLayer, SavedKeyParamTarget,
    SavedPlaceResolver, lower_expr_for_file,
};
use crate::typerules::{is_ordered, marrow_type_name, type_compatible};
use crate::{
    CheckDiagnostic, CheckedProgram, CheckedSavedIndexKey, CheckedSavedKeyParam, CheckedSavedPlace,
    CheckedSavedTerminal, MarrowType, TypeNames, identity_type_for_store,
};

use super::diagnostics::{call_diagnostic, key_type_diagnostic};

/// Reject a write to a sequence position the spec proves addresses no node. Every
/// single int-keyed layer is the canonical 1-based sequence shape — saved or local,
/// spelled `sequence[T]` or `name(k: int): V` — so a statically-known zero or
/// negative position can never be written to any of them. This is the static
/// counterpart of the absent fault a dynamic non-positive position raises at run
/// time, and it is a write-target rule only: a guarded read of such a position
/// resolves to absent at run time.
pub(crate) fn check_sequence_position_write(
    program: &CheckedProgram,
    target: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let Some(checked) = lower_expr_for_file(program, file, target, scope) else {
        return;
    };
    let Some(position) =
        static_non_positive_target_position(program, target, &checked, scope, aliases, file)
    else {
        return;
    };
    diagnostics.push(CheckDiagnostic::error(
        CHECK_SEQUENCE_POSITION,
        file,
        span,
        format!(
            "sequence positions are 1-based, so position `{position}` addresses no node and cannot be written",
        ),
    ));
}

/// The statically-known non-positive position a write target addresses, whether the
/// target is a saved single int-keyed layer or a local single int-keyed collection.
/// `None` for any other shape, an in-range position, or a non-literal position.
fn static_non_positive_target_position(
    program: &CheckedProgram,
    target: &marrow_syntax::Expression,
    checked: &CheckedExpr,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<i64> {
    if let Some(layer) = checked.saved_place().and_then(|place| place.layers.last())
        && single_int_key_layer(layer)
    {
        return static_non_positive_position(layer.args.as_slice());
    }
    if local_single_int_key_target(program, target, scope, aliases, file) {
        let CheckedExpr::Call { args, .. } = checked else {
            return None;
        };
        return static_non_positive_position(args.as_slice());
    }
    None
}

/// Whether a saved layer is a single int-keyed sequence position. A composite or
/// non-int layer is not a 1-based sequence, so a zero or negative key there carries
/// meaning in its own right.
fn single_int_key_layer(layer: &CheckedSavedLayer) -> bool {
    matches!(layer.key_params.as_slice(), [param] if param.scalar == Some(ScalarType::Int))
}

/// Whether a write target is a local single int-keyed collection — a `sequence[T]`
/// or a `name(k: int): V` tree. Both are 1-based sequences; a composite, string, or
/// other non-int key column is not.
fn local_single_int_key_target(
    program: &CheckedProgram,
    target: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> bool {
    let marrow_syntax::Expression::Call { callee, .. } = target else {
        return false;
    };
    match crate::infer::infer_type(program, callee, scope, aliases, file, &mut Vec::new()) {
        MarrowType::Sequence(_) => true,
        MarrowType::LocalTree { keys, .. } => {
            matches!(keys.as_slice(), [MarrowType::Primitive(ScalarType::Int)])
        }
        _ => false,
    }
}

/// The statically-known non-positive value of a single position argument, or `None`
/// when the layer takes other than one argument, the argument is not an integer
/// literal, or the position is in range. A negated integer literal is folded to a
/// signed literal at lowering, so it is covered here too.
fn static_non_positive_position(args: &[CheckedArg]) -> Option<i64> {
    let [arg] = args else {
        return None;
    };
    let CheckedExpr::Literal {
        kind: CheckedLiteralKind::Integer,
        text,
        ..
    } = &arg.value
    else {
        return None;
    };
    text.parse::<i64>().ok().filter(|position| *position < 1)
}

/// Type-check the key arguments of a saved access against the keys it addresses.
/// A foreign identity spliced into a keyspace, or a scalar of the wrong type, is a
/// `check.key_type`. Non-saved callees and unresolved roots are left alone.
pub(crate) struct SavedKeyArgCheck<'a, 'd> {
    pub(crate) program: &'a CheckedProgram,
    pub(crate) callee: &'a marrow_syntax::Expression,
    pub(crate) args: &'a [Argument],
    pub(crate) arg_types: &'a [MarrowType],
    pub(crate) scope: &'a [HashMap<String, MarrowType>],
    pub(crate) span: SourceSpan,
    pub(crate) file: &'a Path,
    pub(crate) diagnostics: &'d mut Vec<CheckDiagnostic>,
}

pub(crate) fn check_saved_key_args(check: SavedKeyArgCheck<'_, '_>) {
    let Some(callee) = lower_expr_for_file(check.program, check.file, check.callee, check.scope)
    else {
        return;
    };
    let Some(target) = SavedPlaceResolver::new(check.program).saved_key_params(&callee) else {
        return;
    };
    check_saved_key_argument_names(check.args, check.file, check.diagnostics);
    match target {
        SavedKeyParamTarget::Root(place) => {
            check_root_args_against(
                place,
                check.args,
                check.arg_types,
                check.span,
                check.file,
                check.diagnostics,
            );
        }
        SavedKeyParamTarget::Index(place) => {
            check_index_args_against(
                check.program,
                place,
                check.args,
                check.arg_types,
                check.span,
                check.file,
                check.diagnostics,
            );
        }
        SavedKeyParamTarget::Layer(layer) => {
            check_layer_key_args(
                &layer.key_params,
                check.args,
                check.arg_types,
                check.span,
                check.file,
                check.diagnostics,
            );
        }
    }
}

/// Check the key arguments of a keyed layer. A composite layer is a chain of
/// single-key sub-layers, so a partial prefix is a valid descent into the inner
/// sub-layer; only supplying more keys than the layer declares is an error. A
/// range argument ranges a declared column, so it fills every column. The per-key
/// matching is shared with [`check_checked_key_args`]; only the arity policy and
/// this no-trailing-column range guard differ.
fn check_layer_key_args(
    keys: &[CheckedSavedKeyParam],
    args: &[Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if arg_types.len() > keys.len() {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            format!(
                "this keyed access expects at most {} key argument(s), but {} were given",
                keys.len(),
                arg_types.len(),
            ),
        ));
        return;
    }
    if range_arg_position(args).is_some() && args.len() != keys.len() {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            "a ranged key argument must leave no further key columns".to_string(),
        ));
        return;
    }
    check_supplied_layer_keys(keys, args, arg_types, span, file, diagnostics);
}

/// Match each supplied key argument against the column it fills. A range argument,
/// when present, is checked in range position and must be the final argument; every
/// other argument is checked nominally. Shared by the exact-arity full-address
/// caller and the partial-prefix layer caller, which screen arity beforehand.
fn check_supplied_layer_keys(
    keys: &[CheckedSavedKeyParam],
    args: &[Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let range_arg = range_arg_position(args);
    if let Some(range_arg) = range_arg
        && range_arg + 1 != args.len()
    {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            "a key range argument must be the final key argument".to_string(),
        ));
        return;
    }
    for (position, (key, arg_type)) in keys.iter().zip(arg_types).enumerate() {
        let expected = SavedPlaceResolver::saved_key_param_type(key);
        if range_arg == Some(position) {
            check_range_key_arg(
                RangeKeyArg {
                    expected: &expected,
                    actual: arg_type,
                    component: format!("key `{}`", key.name),
                    arg: &args[position].value,
                    allow_enum: false,
                },
                span,
                file,
                diagnostics,
            );
            continue;
        }
        if !saved_key_arg_matches(&expected, arg_type) {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                format!(
                    "key `{}` expects `{}`, but this value is `{}`",
                    key.name,
                    marrow_type_name(&expected),
                    marrow_type_name(arg_type),
                ),
            ));
        }
    }
}

fn check_root_args_against(
    place: &CheckedSavedPlace,
    args: &[Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if range_arg_position(args).is_some() {
        check_checked_key_args(
            &place.identity_keys,
            args,
            arg_types,
            span,
            file,
            diagnostics,
        );
        return;
    }
    if let [MarrowType::Identity(_)] = arg_types {
        let expected = MarrowType::Identity(place.root.clone());
        if type_compatible(&expected, &arg_types[0]) == Some(false) {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                format!(
                    "`^{}` is addressed by `{}`, but this value is `{}`",
                    place.root,
                    marrow_type_name(&expected),
                    marrow_type_name(&arg_types[0]),
                ),
            ));
        }
        return;
    }
    check_checked_key_args(
        &place.identity_keys,
        args,
        arg_types,
        span,
        file,
        diagnostics,
    );
}

pub(crate) fn saved_root_args_address_record(
    store: &StoreSchema,
    args: &[Argument],
    arg_types: &[MarrowType],
) -> bool {
    if args.iter().any(|arg| arg.name.is_some()) {
        return false;
    }
    if range_arg_position(args).is_some() {
        return false;
    }
    if let [MarrowType::Identity(_)] = arg_types {
        return type_compatible(&identity_type_for_store(store), &arg_types[0]) != Some(false);
    }
    if args.len() != store.identity_keys.len() {
        return false;
    }
    store
        .identity_keys
        .iter()
        .zip(arg_types)
        .all(|(key, arg_type)| {
            let expected = MarrowType::from_resolved(key.ty.clone(), TypeNames::default());
            saved_key_arg_matches(&expected, arg_type)
        })
}

fn check_saved_key_argument_names(
    args: &[Argument],
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for arg in args {
        if arg.name.is_some() {
            diagnostics.push(call_diagnostic(
                file,
                arg.value.span(),
                "saved key arguments must be positional".to_string(),
            ));
        }
    }
}

fn check_index_args_against(
    program: &CheckedProgram,
    place: &CheckedSavedPlace,
    args: &[Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let Some((name, unique, expected_len, keys)) = checked_index_target(place) else {
        return;
    };
    let range_arg = range_arg_position(args);
    if unique {
        if expected_len != arg_types.len() {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                format!(
                    "unique index `{}` expects {} key argument(s), but {} were given",
                    name,
                    expected_len,
                    arg_types.len(),
                ),
            ));
            return;
        }
        if range_arg.is_some() {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                format!("unique index `{}` does not accept range arguments", name),
            ));
            return;
        }
    } else if arg_types.len() > expected_len {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            format!(
                "index `{}` accepts at most {} key argument(s), but {} were given",
                name,
                expected_len,
                arg_types.len(),
            ),
        ));
        return;
    }

    if let Some(index) = range_arg
        && index + 1 != args.len()
    {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            "an index range argument must be the final key argument".to_string(),
        ));
        return;
    }
    if let Some(index) = range_arg {
        let identity_start = expected_len.saturating_sub(place.identity_keys.len());
        if index + 1 < identity_start {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                "an index range argument must leave only identity components after it".to_string(),
            ));
            return;
        }
    }

    let resolver = SavedPlaceResolver::new(program);
    for (position, (component, arg_type)) in keys.iter().zip(arg_types).enumerate() {
        let expected = resolver.saved_index_key_type(component);
        if range_arg == Some(position) {
            check_index_range_arg(
                &expected,
                arg_type,
                &component.name,
                &args[position].value,
                span,
                file,
                diagnostics,
            );
            continue;
        }
        if !saved_key_arg_matches(&expected, arg_type) {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                format!(
                    "index component `{}` expects `{}`, but this value is `{}`",
                    component.name,
                    marrow_type_name(&expected),
                    marrow_type_name(arg_type),
                ),
            ));
        }
    }
}

fn checked_index_target(
    place: &CheckedSavedPlace,
) -> Option<(&str, bool, usize, &[CheckedSavedIndexKey])> {
    let CheckedSavedTerminal::Index {
        name,
        unique,
        arg_count,
        ..
    } = &place.terminal
    else {
        return None;
    };
    let index = place.indexes.iter().find(|index| index.name == *name)?;
    Some((name, *unique, *arg_count, &index.keys))
}

/// Check key arguments against declared key parameters under an exact-arity policy:
/// a full address fills every key column. The per-key matching is shared with
/// [`check_layer_key_args`] through [`check_supplied_layer_keys`].
fn check_checked_key_args(
    keys: &[CheckedSavedKeyParam],
    args: &[Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if arg_types.len() != keys.len() {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            format!(
                "this keyed access expects {} key argument(s), but {} were given",
                keys.len(),
                arg_types.len(),
            ),
        ));
        return;
    }
    check_supplied_layer_keys(keys, args, arg_types, span, file, diagnostics);
}

fn check_index_range_arg(
    expected: &MarrowType,
    actual: &MarrowType,
    component: &str,
    arg: &marrow_syntax::Expression,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    check_range_key_arg(
        RangeKeyArg {
            expected,
            actual,
            component: format!("index component `{component}`"),
            arg,
            allow_enum: true,
        },
        span,
        file,
        diagnostics,
    );
}

struct RangeKeyArg<'a> {
    expected: &'a MarrowType,
    actual: &'a MarrowType,
    component: String,
    arg: &'a marrow_syntax::Expression,
    allow_enum: bool,
}

fn check_range_key_arg(
    check: RangeKeyArg<'_>,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let Some(range) = marrow_syntax::range_expr(check.arg) else {
        return;
    };
    if range.start.is_none() && range.end.is_none() {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            "a bare range is not a valid key argument".to_string(),
        ));
        return;
    }
    if range.inclusive_end && range.end.is_none() {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            "an inclusive range key argument must have an upper bound".to_string(),
        ));
        return;
    }
    if range.step.is_some() {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            "key range arguments do not accept `by` steps".to_string(),
        ));
        return;
    }
    if !ordered_range_component(check.expected, check.allow_enum) {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            format!(
                "{} expects `{}`, which cannot be ranged",
                check.component,
                marrow_type_name(check.expected),
            ),
        ));
        return;
    }
    if !saved_key_arg_matches(check.expected, check.actual) {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            format!(
                "{} expects `{}`, but this range bound is `{}`",
                check.component,
                marrow_type_name(check.expected),
                marrow_type_name(check.actual),
            ),
        ));
    }
}

/// Whether a key or index component can carry a range bound. Saved keys are
/// orderable scalars — every scalar except `decimal`, which is not a key type — and,
/// for index components, enums. `bool` keys sort `false` before `true` through the
/// same order-preserving byte encoding, so a `bool` component ranges like any other
/// ordered key, distinct from value-comparison orderability.
fn ordered_range_component(expected: &MarrowType, allow_enum: bool) -> bool {
    match expected {
        MarrowType::Primitive(ScalarType::Bool) => true,
        MarrowType::Primitive(scalar) => is_ordered(*scalar),
        MarrowType::Enum { .. } => allow_enum,
        _ => false,
    }
}

fn range_arg_position(args: &[Argument]) -> Option<usize> {
    args.iter()
        .position(|arg| marrow_syntax::range_expr(&arg.value).is_some())
}

/// Check argument types against the declared key parameters they fill. A count
/// mismatch is reported once (the per-key mapping is then undefined); otherwise
/// each argument is checked nominally. An `unknown` argument is rejected: saved
/// keyspaces are nominal identity boundaries, so dynamic reentry must convert to
/// the declared key type instead of acting as `any`.
pub(crate) fn check_keys_against(
    keys: &[marrow_schema::KeyDef],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if keys.len() != arg_types.len() {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            format!(
                "this keyed access expects {} key argument(s), but {} were given",
                keys.len(),
                arg_types.len(),
            ),
        ));
        return;
    }
    for (key, arg_type) in keys.iter().zip(arg_types) {
        let expected = MarrowType::from_resolved(key.ty.clone(), TypeNames::default());
        if !saved_key_arg_matches(&expected, arg_type) {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                format!(
                    "key `{}` expects `{}`, but this value is `{}`",
                    key.name,
                    marrow_type_name(&expected),
                    marrow_type_name(arg_type),
                ),
            ));
        }
    }
}

fn saved_key_arg_matches(expected: &MarrowType, actual: &MarrowType) -> bool {
    if matches!(actual, MarrowType::Unknown) {
        return false;
    }
    type_compatible(expected, actual) != Some(false)
}
