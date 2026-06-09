//! Key-argument typing for saved accesses: whole-record lookups, declared index
//! branches, and keyed layers, each checked against the keys it addresses. A
//! foreign identity spliced into a keyspace, or a scalar of the wrong type, is a
//! `check.key_type`.

use std::path::Path;

use marrow_schema::{IndexSchema, ResourceSchema, StoreSchema};
use marrow_syntax::{Argument, SourceSpan};

use crate::infer::saved_layer_chain;
use crate::resolve::resolve_store_by_root;
use crate::typerules::{marrow_type_name, type_compatible};
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
        check_keys_against(
            &store.store.identity_keys,
            arg_types,
            span,
            file,
            diagnostics,
        );
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
        check_keys_against(&node.key_params, arg_types, span, file, diagnostics);
    }
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

    for (component, arg_type) in index.args.iter().zip(arg_types) {
        let expected = index_component_type(program, store, resource, module, component);
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
