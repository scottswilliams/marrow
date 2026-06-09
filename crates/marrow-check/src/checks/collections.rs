//! Collection and saved-path loop typing: the scope frame a `for` body runs
//! under, the key/value types of saved paths and index branches, and the
//! collection-support rules for two-name loops over index branches.

use std::collections::HashMap;
use std::path::Path;

use marrow_schema::{IndexSchema, ResourceSchema, StoreSchema};
use marrow_syntax::Severity;

use crate::infer::{
    infer_type, layer_key_type, lift_member_type, saved_group_entry_type, saved_layer_chain,
    saved_leaf_type,
};
use crate::resolve::resolve_store_by_root;
use crate::{
    CHECK_COLLECTION_UNSUPPORTED, CheckDiagnostic, CheckedProgram, DiagnosticPayload, MarrowType,
    TypeNames, identity_type_for_store, resource_type_name,
};

use super::diagnostics::key_type_diagnostic;
use super::ranges::range_endpoint_type;

/// The scope frame a `for` loop's body runs under: its binding(s) typed against
/// the iterable. Collection loops bind the element, with `keys(...)` preserving
/// address-only traversal and two-name loops binding address plus element.
/// Inference here discards diagnostics; the type pass emits the iterable's.
pub(crate) fn for_frame(
    program: &CheckedProgram,
    binding: &marrow_syntax::ForBinding,
    iterable: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> HashMap<String, MarrowType> {
    let iterable_type = infer_type(program, iterable, scope, aliases, file, &mut Vec::new());
    if let Some((first_type, second_type)) =
        collection_loop_binding_types(program, binding.second.is_some(), iterable)
    {
        let mut frame = HashMap::new();
        frame.insert(binding.first.clone(), first_type);
        if let Some(second) = &binding.second {
            frame.insert(second.clone(), second_type.unwrap_or(MarrowType::Unknown));
        }
        return frame;
    }
    let first_type = match (&binding.second, &iterable_type) {
        (None, MarrowType::Sequence(element)) => (**element).clone(),
        // A range binds its variable to its endpoint scalar; only a same-typed
        // steppable-endpoint range types it, anything else stays unknown.
        (None, _) => range_endpoint_type(program, iterable, scope, aliases, file)
            .unwrap_or(MarrowType::Unknown),
        _ => MarrowType::Unknown,
    };
    let mut frame = HashMap::new();
    frame.insert(binding.first.clone(), first_type);
    if let Some(second) = &binding.second {
        frame.insert(second.clone(), MarrowType::Unknown);
    }
    frame
}

pub(super) fn collection_loop_binding_types(
    program: &CheckedProgram,
    two_name: bool,
    iterable: &marrow_syntax::Expression,
) -> Option<(MarrowType, Option<MarrowType>)> {
    let iterable = reversed_call_arg(iterable).unwrap_or(iterable);
    if let Some(path) = collection_wrapper_arg(iterable, "keys") {
        if two_name || is_saved_unique_index_branch_path(program, path) {
            return None;
        }
        return Some((saved_path_key_type(program, path)?, None));
    }
    if let Some(path) = collection_wrapper_arg(iterable, "values") {
        if two_name || is_saved_index_branch_path(program, path) {
            return None;
        }
        return Some((saved_path_value_type(program, path), None));
    }
    if let Some(path) = collection_wrapper_arg(iterable, "entries") {
        if !two_name || is_saved_index_branch_path(program, path) {
            return None;
        }
        return Some((
            saved_path_key_type(program, path)?,
            Some(saved_path_value_type(program, path)),
        ));
    }
    saved_path_key_type(program, iterable)?;
    if is_saved_index_branch_path(program, iterable) {
        if two_name {
            let (store, resource, index, module, arg_count) =
                saved_index_branch(program, iterable)?;
            if non_unique_index_branch_yields_identity(store, index, arg_count) {
                return Some((
                    saved_path_key_type(program, iterable)?,
                    Some(MarrowType::Resource(resource_type_name(
                        module,
                        &resource.name,
                    ))),
                ));
            }
            return None;
        }
        return Some((saved_path_key_type(program, iterable)?, None));
    }
    if two_name {
        return Some((
            saved_path_key_type(program, iterable)?,
            saved_path_direct_value_type(program, iterable),
        ));
    }
    Some((saved_path_key_type(program, iterable)?, None))
}

pub(super) fn check_for_collection_support(
    program: &CheckedProgram,
    file: &Path,
    binding: &marrow_syntax::ForBinding,
    iterable: &marrow_syntax::Expression,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let iterable = reversed_call_arg(iterable).unwrap_or(iterable);
    let Some((store, _resource, index, _module, arg_count)) = saved_index_branch(program, iterable)
    else {
        return;
    };
    if index.unique && arg_count != index.args.len() {
        diagnostics.push(key_type_diagnostic(
            file,
            iterable.span(),
            format!(
                "unique index `{}` expects {} key argument(s), but {} were given",
                index.name,
                index.args.len(),
                arg_count,
            ),
        ));
        return;
    }
    if binding.second.is_none() {
        return;
    }
    if non_unique_index_branch_yields_identity(store, index, arg_count) {
        return;
    }
    diagnostics.push(CheckDiagnostic {
        code: CHECK_COLLECTION_UNSUPPORTED,
        severity: Severity::Error,
        file: file.to_path_buf(),
        message: "a two-name loop over an index branch must yield identity values".to_string(),
        span: iterable.span(),
        payload: DiagnosticPayload::None,
    });
}

fn reversed_call_arg(expr: &marrow_syntax::Expression) -> Option<&marrow_syntax::Expression> {
    collection_wrapper_arg(expr, "reversed")
}

fn collection_wrapper_arg<'a>(
    expr: &'a marrow_syntax::Expression,
    wrapper: &str,
) -> Option<&'a marrow_syntax::Expression> {
    let marrow_syntax::Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    let marrow_syntax::Expression::Name { segments, .. } = callee.as_ref() else {
        return None;
    };
    if segments.as_slice() != [wrapper] {
        return None;
    }
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}

pub(super) fn saved_path_key_type(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    use marrow_syntax::Expression;
    match path {
        Expression::SavedRoot { name, .. } => {
            let store = resolve_store_by_root(program, name)?;
            if store.store.identity_keys.is_empty() {
                return None;
            }
            Some(identity_type_for_store(store.store))
        }
        Expression::Call { .. } => saved_index_branch_type(program, path),
        Expression::Field { .. } if is_saved_index_branch_path(program, path) => {
            saved_index_branch_type(program, path)
        }
        Expression::Field { .. } if saved_layer_chain(path).is_some() => {
            Some(layer_key_type(program, path))
        }
        Expression::Field { .. } => None,
        _ => None,
    }
}

fn saved_path_value_type(program: &CheckedProgram, path: &marrow_syntax::Expression) -> MarrowType {
    saved_path_direct_value_type(program, path).unwrap_or(MarrowType::Unknown)
}

fn saved_path_direct_value_type(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    use marrow_syntax::Expression;
    match path {
        Expression::SavedRoot { name, .. } => {
            let store = resolve_store_by_root(program, name)?;
            if store.store.identity_keys.is_empty() {
                return None;
            }
            Some(MarrowType::Resource(resource_type_name(
                &store.module.name,
                &store.resource.name,
            )))
        }
        Expression::Field { .. } => saved_leaf_type(program, path)
            .or_else(|| saved_group_entry_type(program, path))
            .or(Some(MarrowType::Unknown)),
        _ => None,
    }
}

fn saved_index_branch_type(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
) -> Option<MarrowType> {
    let (store, resource, index, module, arg_count) = saved_index_branch(program, path)?;
    if index.unique {
        return Some(identity_type_for_store(store));
    }
    let identity_arity = store.identity_keys.len();
    let identity_start = index.args.len().saturating_sub(identity_arity);
    if arg_count < identity_start {
        return index
            .args
            .get(arg_count)
            .map(|name| index_component_type(program, store, resource, module, name));
    }
    Some(identity_type_for_store(store))
}

pub(super) fn non_unique_index_branch_yields_identity(
    store: &StoreSchema,
    index: &IndexSchema,
    arg_count: usize,
) -> bool {
    if index.unique {
        return false;
    }
    let identity_arity = store.identity_keys.len();
    let identity_start = index.args.len().saturating_sub(identity_arity);
    arg_count >= identity_start
}

pub(super) fn index_component_type(
    program: &CheckedProgram,
    store: &StoreSchema,
    resource: &ResourceSchema,
    module: &str,
    name: &str,
) -> MarrowType {
    if let Some(key) = store.identity_keys.iter().find(|key| key.name == name) {
        return MarrowType::from_resolved(key.ty.clone(), TypeNames::default());
    }
    resource
        .field_type(&[name])
        .map(|ty| lift_member_type(program, ty.clone(), module))
        .unwrap_or(MarrowType::Unknown)
}

fn saved_index_branch<'p>(
    program: &'p CheckedProgram,
    path: &marrow_syntax::Expression,
) -> Option<(
    &'p StoreSchema,
    &'p ResourceSchema,
    &'p IndexSchema,
    &'p str,
    usize,
)> {
    match path {
        marrow_syntax::Expression::Call { callee, args, .. } => {
            if args
                .iter()
                .any(|arg| arg.mode.is_some() || arg.name.is_some())
            {
                return None;
            }
            let (store, resource, index, module) = saved_index_schema(program, callee)?;
            (args.len() <= index.args.len()).then_some((store, resource, index, module, args.len()))
        }
        marrow_syntax::Expression::Field { .. } => saved_index_schema(program, path)
            .map(|(store, resource, index, module)| (store, resource, index, module, 0)),
        _ => None,
    }
}

pub(super) fn saved_index_schema<'p>(
    program: &'p CheckedProgram,
    callee: &marrow_syntax::Expression,
) -> Option<(
    &'p StoreSchema,
    &'p ResourceSchema,
    &'p IndexSchema,
    &'p str,
)> {
    let marrow_syntax::Expression::Field { base, name, .. } = callee else {
        return None;
    };
    saved_index_schema_from_parts(program, base, name)
}

fn saved_index_schema_from_parts<'p>(
    program: &'p CheckedProgram,
    base: &marrow_syntax::Expression,
    name: &str,
) -> Option<(
    &'p StoreSchema,
    &'p ResourceSchema,
    &'p IndexSchema,
    &'p str,
)> {
    let marrow_syntax::Expression::SavedRoot { name: root, .. } = base else {
        return None;
    };
    let store = resolve_store_by_root(program, root)?;
    let index = store
        .store
        .indexes
        .iter()
        .find(|index| index.name == name)?;
    Some((store.store, store.resource, index, &store.module.name))
}

pub(crate) fn is_saved_index_branch_path(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
) -> bool {
    saved_index_branch(program, path).is_some()
}

fn is_saved_unique_index_branch_path(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
) -> bool {
    saved_index_branch(program, path).is_some_and(|(_, _, index, _, _)| index.unique)
}
