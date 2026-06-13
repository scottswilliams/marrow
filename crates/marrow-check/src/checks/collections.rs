//! Collection and saved-path loop typing: the scope frame a `for` body runs
//! under, the key/value types of saved paths and index branches, and the
//! collection-support rules for two-name loops over index branches.

use std::collections::HashMap;
use std::path::Path;

use marrow_schema::ScalarType;

use crate::executable::{SavedPlaceResolver, lower_expr_for_file};
use crate::infer::infer_type;
use crate::{
    CHECK_COLLECTION_UNSUPPORTED, CheckDiagnostic, CheckedExpr, CheckedProgram, CheckedSavedPlace,
    MarrowType, resource_type_name,
};

use super::diagnostics::key_type_diagnostic;
use super::ranges::range_endpoint_type;

/// The scope frame a `for` loop's body runs under: its binding(s) typed against
/// the iterable. Keyed collection loops bind the address, with `values(...)`
/// preserving value-only traversal and two-name loops binding address plus element.
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
    if let Some((first_type, second_type)) = local_collection_loop_binding_types(
        program,
        binding.second.is_some(),
        iterable,
        scope,
        aliases,
        file,
    ) {
        let mut frame = HashMap::new();
        frame.insert(binding.first.clone(), first_type);
        if let Some(second) = &binding.second {
            frame.insert(second.clone(), second_type.unwrap_or(MarrowType::Unknown));
        }
        return frame;
    }
    if let Some((first_type, second_type)) =
        collection_loop_binding_types(program, binding.second.is_some(), iterable, scope, file)
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
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> Option<(MarrowType, Option<MarrowType>)> {
    let iterable = reversed_call_arg(iterable).unwrap_or(iterable);
    if let Some(path) = collection_wrapper_arg(iterable, "keys") {
        if two_name || is_saved_unique_index_branch_path(program, path, scope, file) {
            return None;
        }
        return Some((saved_path_key_type(program, path, scope, file)?, None));
    }
    if let Some(path) = collection_wrapper_arg(iterable, "values") {
        if two_name || is_saved_index_branch_path(program, path, scope, file) {
            return None;
        }
        return Some((saved_path_value_type(program, path, scope, file), None));
    }
    if let Some(path) = collection_wrapper_arg(iterable, "entries") {
        if !two_name || is_saved_index_branch_path(program, path, scope, file) {
            return None;
        }
        return Some((
            saved_path_key_type(program, path, scope, file)?,
            Some(saved_path_value_type(program, path, scope, file)),
        ));
    }
    saved_path_key_type(program, iterable, scope, file)?;
    if is_saved_index_branch_path(program, iterable, scope, file) {
        if two_name {
            let checked = checked_saved_expr(program, iterable, scope, file)?;
            if SavedPlaceResolver::new(program).non_unique_index_branch_yields_identity(&checked) {
                let place = checked.saved_place()?;
                return Some((
                    saved_path_key_type(program, iterable, scope, file)?,
                    Some(saved_place_resource_type(program, place)),
                ));
            }
            return None;
        }
        return Some((saved_path_key_type(program, iterable, scope, file)?, None));
    }
    if two_name {
        return Some((
            saved_path_key_type(program, iterable, scope, file)?,
            saved_path_direct_value_type(program, iterable, scope, file),
        ));
    }
    Some((saved_path_key_type(program, iterable, scope, file)?, None))
}

fn local_collection_loop_binding_types(
    program: &CheckedProgram,
    two_name: bool,
    iterable: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<(MarrowType, Option<MarrowType>)> {
    let iterable = reversed_call_arg(iterable).unwrap_or(iterable);
    if let Some(path) = collection_wrapper_arg(iterable, "keys") {
        if two_name {
            return None;
        }
        return local_key_binding_type(local_iterable_type(
            program, path, scope, aliases, file, true,
        ))
        .map(|key| (key, None));
    }
    if let Some(path) = collection_wrapper_arg(iterable, "values") {
        if two_name {
            return None;
        }
        return local_value_binding_type(local_iterable_type(
            program, path, scope, aliases, file, false,
        ))
        .map(|value| (value, None));
    }
    if let Some(path) = collection_wrapper_arg(iterable, "entries") {
        if !two_name {
            return None;
        }
        return local_entries_binding_types(local_iterable_type(
            program, path, scope, aliases, file, false,
        ));
    }
    local_tree_binding_types(
        two_name,
        local_iterable_type(program, iterable, scope, aliases, file, false),
    )
}

fn local_iterable_type(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
    peel_reversed: bool,
) -> MarrowType {
    let path = if peel_reversed {
        reversed_call_arg(path).unwrap_or(path)
    } else {
        path
    };
    infer_type(program, path, scope, aliases, file, &mut Vec::new())
}

fn local_key_binding_type(ty: MarrowType) -> Option<MarrowType> {
    match ty {
        MarrowType::LocalTree { keys, .. } => Some(first_key_type(keys)),
        MarrowType::Sequence(_) => Some(MarrowType::Primitive(ScalarType::Int)),
        _ => None,
    }
}

fn local_value_binding_type(ty: MarrowType) -> Option<MarrowType> {
    match ty {
        MarrowType::LocalTree { value, .. } | MarrowType::Sequence(value) => Some(*value),
        _ => None,
    }
}

fn local_tree_binding_types(
    two_name: bool,
    ty: MarrowType,
) -> Option<(MarrowType, Option<MarrowType>)> {
    let MarrowType::LocalTree { keys, value } = ty else {
        return None;
    };
    let key = first_key_type(keys);
    if two_name {
        Some((key, Some(*value)))
    } else {
        Some((key, None))
    }
}

fn first_key_type(keys: Vec<MarrowType>) -> MarrowType {
    keys.into_iter().next().unwrap_or(MarrowType::Unknown)
}

fn local_entries_binding_types(ty: MarrowType) -> Option<(MarrowType, Option<MarrowType>)> {
    match ty {
        MarrowType::LocalTree { keys, value } => Some((first_key_type(keys), Some(*value))),
        MarrowType::Sequence(element) => {
            Some((MarrowType::Primitive(ScalarType::Int), Some(*element)))
        }
        _ => None,
    }
}

pub(super) fn check_for_collection_support(
    program: &CheckedProgram,
    file: &Path,
    binding: &marrow_syntax::ForBinding,
    iterable: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if binding.second.is_some() && is_non_pair_collection_wrapper(iterable) {
        diagnostics.push(CheckDiagnostic::error(
            CHECK_COLLECTION_UNSUPPORTED,
            file,
            iterable.span(),
            "a two-name loop requires a pair iterable (use entries(...))",
        ));
        return;
    }

    if binding.second.is_some()
        && two_name_entries_loop_arg(binding, iterable).is_none()
        && local_collection_loop_binding_types(program, true, iterable, scope, aliases, file)
            .is_none()
        && collection_loop_binding_types(program, true, iterable, scope, file).is_none()
        && matches!(
            infer_type(program, iterable, scope, aliases, file, &mut Vec::new()),
            MarrowType::Sequence(_)
        )
    {
        diagnostics.push(CheckDiagnostic::error(
            CHECK_COLLECTION_UNSUPPORTED,
            file,
            iterable.span(),
            "a two-name loop requires a pair iterable (use entries(...))",
        ));
        return;
    }

    let iterable = reversed_call_arg(iterable).unwrap_or(iterable);
    let Some(checked_iterable) = checked_saved_expr(program, iterable, scope, file) else {
        return;
    };
    let resolver = SavedPlaceResolver::new(program);
    let Some(index) = resolver.index_branch_info(&checked_iterable) else {
        return;
    };
    if index.unique && index.arg_count != index.key_count {
        diagnostics.push(key_type_diagnostic(
            file,
            iterable.span(),
            format!(
                "unique index `{}` expects {} key argument(s), but {} were given",
                index.name, index.key_count, index.arg_count,
            ),
        ));
        return;
    }
    if binding.second.is_none() {
        return;
    }
    if resolver.non_unique_index_branch_yields_identity(&checked_iterable) {
        return;
    }
    diagnostics.push(CheckDiagnostic::error(
        CHECK_COLLECTION_UNSUPPORTED,
        file,
        iterable.span(),
        "a two-name loop over an index branch must yield identity values",
    ));
}

fn is_non_pair_collection_wrapper(iterable: &marrow_syntax::Expression) -> bool {
    let iterable = reversed_call_arg(iterable).unwrap_or(iterable);
    is_collection_wrapper(iterable, "keys") || is_collection_wrapper(iterable, "values")
}

pub(super) fn check_for_entries_support(
    file: &Path,
    binding: &marrow_syntax::ForBinding,
    iterable: &marrow_syntax::Expression,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let Some(arg) = two_name_entries_loop_arg(binding, iterable) else {
        check_entries_value_position(file, iterable, diagnostics);
        return;
    };
    check_entries_loop_arg(file, arg, diagnostics);
}

pub(super) fn check_entries_value_position(
    file: &Path,
    expr: &marrow_syntax::Expression,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    use marrow_syntax::{Expression, InterpolationPart};
    if is_collection_wrapper(expr, "entries")
        && !has_collection_unsupported(diagnostics, file, expr.span())
    {
        diagnostics.push(entries_unsupported(
            file,
            expr.span(),
            "`entries(...)` is only valid as a two-name loop iterable",
        ));
    }
    match expr {
        Expression::Call { callee, args, .. } => {
            check_entries_value_position(file, callee, diagnostics);
            for arg in args {
                check_entries_value_position(file, &arg.value, diagnostics);
            }
        }
        Expression::Field { base, .. }
        | Expression::OptionalField { base, .. }
        | Expression::Unary { operand: base, .. } => {
            check_entries_value_position(file, base, diagnostics);
        }
        Expression::Binary { left, right, .. } => {
            check_entries_value_position(file, left, diagnostics);
            check_entries_value_position(file, right, diagnostics);
        }
        Expression::Range {
            start, end, step, ..
        } => {
            for part in [start.as_deref(), end.as_deref(), step.as_deref()]
                .into_iter()
                .flatten()
            {
                check_entries_value_position(file, part, diagnostics);
            }
        }
        Expression::Interpolation { parts, .. } => {
            for part in parts {
                if let InterpolationPart::Expr(expr) = part {
                    check_entries_value_position(file, expr, diagnostics);
                }
            }
        }
        Expression::Literal { .. } | Expression::Name { .. } | Expression::SavedRoot { .. } => {}
    }
}

fn two_name_entries_loop_arg<'a>(
    binding: &marrow_syntax::ForBinding,
    iterable: &'a marrow_syntax::Expression,
) -> Option<&'a marrow_syntax::Expression> {
    binding.second.as_ref()?;
    collection_wrapper_arg(iterable, "entries").or_else(|| {
        let inner = collection_wrapper_arg(iterable, "reversed")?;
        collection_wrapper_arg(inner, "entries")
    })
}

fn check_entries_loop_arg(
    file: &Path,
    arg: &marrow_syntax::Expression,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if is_any_collection_wrapper(arg) && !has_collection_unsupported(diagnostics, file, arg.span())
    {
        diagnostics.push(entries_unsupported(
            file,
            arg.span(),
            "`entries(...)` loop heads require a path or local keyed tree",
        ));
        return;
    }
    check_entries_value_position(file, arg, diagnostics);
}

fn is_any_collection_wrapper(expr: &marrow_syntax::Expression) -> bool {
    ["keys", "values", "entries", "reversed"]
        .into_iter()
        .any(|name| is_collection_wrapper(expr, name))
}

fn is_collection_wrapper(expr: &marrow_syntax::Expression, wrapper: &str) -> bool {
    collection_wrapper_arg(expr, wrapper).is_some()
}

fn entries_unsupported(
    file: &Path,
    span: marrow_syntax::SourceSpan,
    message: &str,
) -> CheckDiagnostic {
    CheckDiagnostic::error(CHECK_COLLECTION_UNSUPPORTED, file, span, message)
}

fn has_collection_unsupported(
    diagnostics: &[CheckDiagnostic],
    file: &Path,
    span: marrow_syntax::SourceSpan,
) -> bool {
    diagnostics.iter().any(|diagnostic| {
        diagnostic.code == CHECK_COLLECTION_UNSUPPORTED
            && diagnostic.file == file
            && diagnostic.span == span
    })
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
        [arg] if arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}

pub(super) fn saved_path_key_type(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> Option<MarrowType> {
    let expr = checked_saved_expr(program, path, scope, file)?;
    SavedPlaceResolver::new(program).key_type(&expr)
}

fn saved_path_value_type(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> MarrowType {
    saved_path_direct_value_type(program, path, scope, file).unwrap_or(MarrowType::Unknown)
}

fn saved_path_direct_value_type(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> Option<MarrowType> {
    let expr = checked_saved_expr(program, path, scope, file)?;
    if let Some(place) = expr.saved_place()
        && place.layers.is_empty()
        && matches!(place.terminal, crate::CheckedSavedTerminal::Record)
    {
        return Some(saved_place_resource_type(program, place));
    }
    SavedPlaceResolver::new(program).value_type(&expr)
}

fn checked_saved_expr(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> Option<CheckedExpr> {
    lower_expr_for_file(program, file, path, scope)
}

fn saved_place_resource_type(program: &CheckedProgram, place: &CheckedSavedPlace) -> MarrowType {
    let store = program.facts.store(place.store_id);
    let module = program
        .facts
        .modules()
        .get(store.module.0 as usize)
        .map(|module| module.name.as_str())
        .unwrap_or_default();
    MarrowType::Resource(resource_type_name(module, &place.resource_name))
}

pub(crate) fn is_saved_index_branch_path(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> bool {
    checked_saved_expr(program, path, scope, file)
        .is_some_and(|expr| SavedPlaceResolver::new(program).is_index_branch(&expr))
}

pub(crate) fn is_saved_key_range_path(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> bool {
    let path = saved_key_range_subject(path);
    checked_saved_expr(program, path, scope, file)
        .is_some_and(|expr| SavedPlaceResolver::new(program).is_key_range_path(&expr))
}

pub(crate) fn is_saved_index_range_path(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> bool {
    checked_saved_expr(program, path, scope, file)
        .is_some_and(|expr| SavedPlaceResolver::new(program).is_index_range_path(&expr))
}

fn saved_key_range_subject(mut path: &marrow_syntax::Expression) -> &marrow_syntax::Expression {
    loop {
        if let Some(inner) = reversed_call_arg(path) {
            path = inner;
            continue;
        }
        if let Some(inner) = collection_wrapper_arg(path, "keys")
            .or_else(|| collection_wrapper_arg(path, "values"))
            .or_else(|| collection_wrapper_arg(path, "entries"))
        {
            path = inner;
            continue;
        }
        return path;
    }
}

fn is_saved_unique_index_branch_path(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> bool {
    checked_saved_expr(program, path, scope, file)
        .and_then(|expr| {
            SavedPlaceResolver::new(program)
                .index_branch_info(&expr)
                .map(|info| info.unique)
        })
        .unwrap_or(false)
}
