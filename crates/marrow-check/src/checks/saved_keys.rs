//! Key-argument typing for saved accesses: whole-record lookups, declared index
//! branches, and keyed layers, each checked against the keys it addresses. A
//! foreign identity spliced into a keyspace, or a scalar of the wrong type, is a
//! `check.key_type`.

use std::path::Path;

use marrow_schema::{IndexSchema, KeyDef, ResourceSchema, StoreSchema};
use marrow_syntax::{Argument, SourceSpan};

use crate::infer::saved_layer_chain;
use crate::resolve::resolve_store_by_root;
use crate::typerules::{is_ordered, marrow_type_name, type_compatible};
use crate::{CheckDiagnostic, CheckedProgram, MarrowType, TypeNames, identity_type_for_store};

use super::collections::{index_component_type, saved_index_schema};
use super::diagnostics::{call_diagnostic, key_type_diagnostic};

/// Type-check the key arguments of a saved access against the keys it addresses.
/// A foreign identity spliced into a keyspace, or a scalar of the wrong type, is a
/// `check.key_type`. Non-saved callees and unresolved roots are left alone.
pub(crate) fn check_saved_key_args(
    program: &CheckedProgram,
    callee: &marrow_syntax::Expression,
    args: &[Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::Expression;
    // A whole-record lookup `^root(key…)`: a sole identity argument may be the
    // resource's own identity (a splice), checked nominally; otherwise the per-key
    // scalars are checked against the declared identity keys.
    if let Expression::SavedRoot { name: root, .. } = callee {
        let Some(store) = resolve_store_by_root(program, root) else {
            return;
        };
        check_saved_key_argument_names(args, file, diagnostics);
        if let [MarrowType::Identity(_)] = arg_types {
            let expected = identity_type_for_store(store.store);
            if type_compatible(&expected, &arg_types[0]) == Some(false) {
                diagnostics.push(key_type_diagnostic(
                    file,
                    span,
                    format!(
                        "`^{root}` is addressed by `{}`, but this value is `{}`",
                        marrow_type_name(&expected),
                        marrow_type_name(&arg_types[0]),
                    ),
                ));
            }
            return;
        }
        if range_arg_position(args).is_some() {
            check_declared_key_range_args(
                &store.store.identity_keys,
                args,
                arg_types,
                span,
                file,
                diagnostics,
            );
        } else {
            check_keys_against(
                &store.store.identity_keys,
                arg_types,
                span,
                file,
                diagnostics,
            );
        }
        return;
    }
    // A declared index access `^root.index(args...)`: unique indexes read a
    // single identity only at a complete lookup key, while non-unique branches
    // accept typed prefixes for traversal.
    if let Some((store, resource, index, module)) = saved_index_schema(program, callee) {
        check_saved_key_argument_names(args, file, diagnostics);
        check_index_args_against(
            IndexArgTarget {
                program,
                store,
                resource,
                index,
                module,
            },
            args,
            arg_types,
            span,
            file,
            diagnostics,
        );
        return;
    }
    // A keyed-layer access `^root(key…).layer(key…)`: check this layer's key
    // parameters.
    if let Some((root, layers)) = saved_layer_chain(callee)
        && let Some(store) = resolve_store_by_root(program, root)
        && let Some(node) = store.resource.descend_layers(&layers)
    {
        check_saved_key_argument_names(args, file, diagnostics);
        if range_arg_position(args).is_some() {
            check_declared_key_range_args(
                &node.key_params,
                args,
                arg_types,
                span,
                file,
                diagnostics,
            );
        } else {
            check_keys_against(&node.key_params, arg_types, span, file, diagnostics);
        }
    }
}

pub(crate) fn saved_root_args_address_record(
    store: &StoreSchema,
    args: &[Argument],
    arg_types: &[MarrowType],
) -> bool {
    if args.iter().any(|arg| arg.name.is_some()) {
        return false;
    }
    if let [MarrowType::Identity(_)] = arg_types {
        return type_compatible(&identity_type_for_store(store), &arg_types[0]) != Some(false);
    }
    if range_arg_position(args).is_some() || args.len() != store.identity_keys.len() {
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

struct IndexArgTarget<'a> {
    program: &'a CheckedProgram,
    store: &'a StoreSchema,
    resource: &'a ResourceSchema,
    index: &'a IndexSchema,
    module: &'a str,
}

fn check_index_args_against(
    target: IndexArgTarget<'_>,
    args: &[Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let IndexArgTarget {
        program,
        store,
        resource,
        index,
        module,
    } = target;
    let expected_len = index.args.len();
    let range_arg = range_arg_position(args);
    if index.unique {
        if expected_len != arg_types.len() {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                format!(
                    "unique index `{}` expects {} key argument(s), but {} were given",
                    index.name,
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
                format!(
                    "unique index `{}` does not accept range arguments",
                    index.name
                ),
            ));
            return;
        }
    } else if arg_types.len() > expected_len {
        diagnostics.push(key_type_diagnostic(
            file,
            span,
            format!(
                "index `{}` accepts at most {} key argument(s), but {} were given",
                index.name,
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
        let identity_start = expected_len.saturating_sub(store.identity_keys.len());
        if index + 1 < identity_start {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                "an index range argument must leave only identity components after it".to_string(),
            ));
            return;
        }
    }

    for (position, (component, arg_type)) in index.args.iter().zip(arg_types).enumerate() {
        let expected = index_component_type(program, store, resource, module, component);
        if range_arg == Some(position) {
            check_index_range_arg(
                &expected,
                arg_type,
                component,
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
                    "index component `{component}` expects `{}`, but this value is `{}`",
                    marrow_type_name(&expected),
                    marrow_type_name(arg_type),
                ),
            ));
        }
    }
}

fn check_declared_key_range_args(
    keys: &[KeyDef],
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
        let expected = MarrowType::from_resolved(key.ty.clone(), TypeNames::default());
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

fn saved_key_arg_matches(expected: &MarrowType, actual: &MarrowType) -> bool {
    if matches!(actual, MarrowType::Unknown) {
        return false;
    }
    type_compatible(expected, actual) != Some(false)
}
