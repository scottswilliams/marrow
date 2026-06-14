//! Key-argument typing for saved accesses: whole-record lookups, declared index
//! branches, and keyed layers, each checked against the keys it addresses. A
//! foreign identity spliced into a keyspace, or a scalar of the wrong type, is a
//! `check.key_type`.

use std::collections::HashMap;
use std::path::Path;

use marrow_schema::StoreSchema;
use marrow_syntax::{Argument, SourceSpan};

use crate::executable::{SavedKeyParamTarget, SavedPlaceResolver, lower_expr_for_file};
use crate::typerules::{is_ordered, marrow_type_name, type_compatible};
use crate::{
    CheckDiagnostic, CheckedProgram, CheckedSavedIndexKey, CheckedSavedKeyParam, CheckedSavedPlace,
    CheckedSavedTerminal, MarrowType, TypeNames, identity_type_for_store,
};

use super::diagnostics::{call_diagnostic, key_type_diagnostic};

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
            if range_arg_position(check.args).is_some() {
                check_checked_key_range_args(
                    &layer.key_params,
                    check.args,
                    check.arg_types,
                    check.span,
                    check.file,
                    check.diagnostics,
                );
            } else {
                check_checked_keys_against(
                    &layer.key_params,
                    check.arg_types,
                    check.span,
                    check.file,
                    check.diagnostics,
                );
            }
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
        check_checked_key_range_args(
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
    check_checked_keys_against(&place.identity_keys, arg_types, span, file, diagnostics);
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

fn check_checked_key_range_args(
    keys: &[CheckedSavedKeyParam],
    args: &[Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let expected_len = keys.len();
    if arg_types.len() != expected_len {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            format!(
                "this keyed access expects {} key argument(s), but {} were given",
                expected_len,
                arg_types.len(),
            ),
        ));
        return;
    }
    let Some(range_arg) = range_arg_position(args) else {
        return;
    };
    if range_arg + 1 != args.len() {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            "a key range argument must be the final key argument".to_string(),
        ));
        return;
    }
    for (position, (key, arg_type)) in keys.iter().zip(arg_types).enumerate() {
        let expected = SavedPlaceResolver::saved_key_param_type(key);
        if range_arg == position {
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

fn ordered_range_component(expected: &MarrowType, allow_enum: bool) -> bool {
    match expected {
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

fn check_checked_keys_against(
    keys: &[CheckedSavedKeyParam],
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
        let expected = SavedPlaceResolver::saved_key_param_type(key);
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
