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
    MarrowType,
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
    // Any recognized collection (saved or local, sequence or keyed) bound above; a
    // single variable here is a range, whose binding is its endpoint scalar. A `for`
    // over a concrete scalar is rejected, but its binding recovers to that scalar's
    // type so a body use does not stack a `check.untyped_value` on the one root-cause
    // error.
    let first_type = match &binding.second {
        None => range_endpoint_type(program, iterable, scope, aliases, file)
            .or_else(|| scalar_iterable_recovery_type(program, iterable, scope, aliases, file))
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

/// The scope frame a `catch` clause's block runs under: the caught error value
/// bound to its name. Shared by the type pass and cursor scope reconstruction so
/// the two cannot drift.
pub(crate) fn catch_frame(clause: &marrow_syntax::CatchClause) -> HashMap<String, MarrowType> {
    let mut frame = HashMap::new();
    frame.insert(clause.name.clone(), MarrowType::Error);
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
        if two_name {
            return None;
        }
        // A non-unique index branch streams the store identity, so `values(...)`
        // materializes the whole record at it, exactly as the bare two-name form
        // does. A unique branch is a single-identity lookup, not a stream.
        if let Some(resource) = non_unique_index_branch_resource_type(program, path, scope, file) {
            return Some((resource, None));
        }
        if is_saved_index_branch_path(program, path, scope, file) {
            return None;
        }
        return Some((saved_path_value_type(program, path, scope, file), None));
    }
    if let Some(path) = collection_wrapper_arg(iterable, "entries") {
        if !two_name {
            return None;
        }
        if let Some(resource) = non_unique_index_branch_resource_type(program, path, scope, file) {
            return Some((
                saved_path_key_type(program, path, scope, file)?,
                Some(resource),
            ));
        }
        if is_saved_index_branch_path(program, path, scope, file) {
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
            if let Some(resource) =
                non_unique_index_branch_resource_type(program, iterable, scope, file)
            {
                return Some((
                    saved_path_key_type(program, iterable, scope, file)?,
                    Some(resource),
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
    local_collection_binding_types(
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

/// A direct local-collection loop binds the key being streamed: a keyed tree's
/// first key column, or a sequence's 1-based integer position. A two-name head
/// also binds the value at that key. A sequence is a 1-based integer-keyed tree,
/// so it follows the same shape as a keyed tree rather than yielding raw values.
fn local_collection_binding_types(
    two_name: bool,
    ty: MarrowType,
) -> Option<(MarrowType, Option<MarrowType>)> {
    let (key, value) = match ty {
        MarrowType::LocalTree { keys, value } => (first_key_type(keys), *value),
        MarrowType::Sequence(element) => (MarrowType::Primitive(ScalarType::Int), *element),
        _ => return None,
    };
    if two_name {
        Some((key, Some(value)))
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

    // Diagnostics report at the loop's written iterable; the checked place is
    // derived from the path under any `reversed(...)` wrapper.
    let span = iterable.span();
    let iterable = reversed_call_arg(iterable).unwrap_or(iterable);
    let resolver = SavedPlaceResolver::new(program);

    // A value-reading loop head pairs each streamed key with the value at it: a
    // two-name binding, or a `values(...)`/`entries(...)` wrapper. When the value
    // position is itself a sub-layer (a composite layer with more than one column
    // still to fill), there is no leaf to pair, so the head must descend one column
    // first. The wrapper's inner path carries that shape, so unwrap to it.
    let value_head = binding.second.is_some()
        || collection_wrapper_arg(iterable, "values").is_some()
        || collection_wrapper_arg(iterable, "entries").is_some();
    if value_head {
        let inner = collection_wrapper_arg(iterable, "values")
            .or_else(|| collection_wrapper_arg(iterable, "entries"))
            .unwrap_or(iterable);
        if checked_saved_expr(program, inner, scope, file)
            .is_some_and(|checked| resolver.value_position_is_sublayer(&checked))
        {
            diagnostics.push(CheckDiagnostic::error(
                CHECK_COLLECTION_UNSUPPORTED,
                file,
                span,
                "a value loop over a composite keyed layer must descend one key at a time: iterate the outer key, then descend the layer at that key for the inner key",
            ));
            return;
        }
    }

    let Some(checked_iterable) = checked_saved_expr(program, iterable, scope, file) else {
        return;
    };
    // A path that names one stored value — a fully-keyed leaf, a scalar field, a
    // single-key full entry, or a whole record — has no key to stream, so a `for`
    // over it is a clean check error rather than an accepted-then-faulted iteration.
    if resolver.addresses_single_value(&checked_iterable) {
        diagnostics.push(CheckDiagnostic::error(
            CHECK_COLLECTION_UNSUPPORTED,
            file,
            span,
            "this saved path names a single value, which cannot be iterated",
        ));
        return;
    }
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

/// Whether `expr` of type `ty` is a concrete non-iterable scalar — an `int`,
/// `string`, `bool`, enum value, or store identity. A range literal types to its
/// endpoint scalar but is genuinely iterable, so it is excluded syntactically.
/// `Unknown` defers, keeping cross-module unresolved values free of false positives.
/// Shared by the collection combinators (`for`, `count`, `reversed`) that need a
/// collection, not a scalar.
/// The recovery type a `for` loop's binding takes when the iterable is a concrete
/// non-iterable scalar: the scalar's own type. The loop is rejected, but its binding
/// is still typed so a body use is not a second untyped cascade.
fn scalar_iterable_recovery_type(
    program: &CheckedProgram,
    iterable: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<MarrowType> {
    let ty = infer_type(program, iterable, scope, aliases, file, &mut Vec::new());
    is_concrete_scalar_value(iterable, &ty).then_some(ty)
}

pub(crate) fn is_concrete_scalar_value(expr: &marrow_syntax::Expression, ty: &MarrowType) -> bool {
    // A range is iterable, and a saved path carries its own presence/traversal
    // semantics (a saved scalar's `count` is its presence), so neither is a local
    // non-iterable scalar.
    if marrow_syntax::range_expr(expr).is_some() || crate::rules::is_saved_path(expr) {
        return false;
    }
    matches!(
        ty,
        MarrowType::Primitive(_) | MarrowType::Enum { .. } | MarrowType::Identity(_)
    )
}

/// Whether `expr` binds through the saved or local collection-loop paths — that is,
/// it is a recognized iterable (a saved layer/index branch or a local sequence/map),
/// whose leaf scalar type must not be mistaken for a non-iterable scalar.
pub(crate) fn is_recognized_collection(
    program: &CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> bool {
    [false, true].into_iter().any(|two_name| {
        local_collection_loop_binding_types(program, two_name, expr, scope, aliases, file).is_some()
            || collection_loop_binding_types(program, two_name, expr, scope, file).is_some()
    })
}

/// Reject a `for` loop whose iterable is a concrete non-iterable scalar. A recognized
/// collection binds through the collection-loop paths, so its leaf scalar type is not
/// mistaken for a non-iterable. When the iterable is a combinator call whose argument
/// rule already reported an unusable argument at this span, the loop defers to that
/// single root-cause diagnostic instead of re-flagging the combinator's result.
pub(super) fn check_for_scalar_iterable(
    program: &CheckedProgram,
    file: &Path,
    iterable: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if is_recognized_collection(program, iterable, scope, aliases, file)
        || has_collection_unsupported(diagnostics, file, iterable.span())
    {
        return;
    }
    let iterable_type = infer_type(program, iterable, scope, aliases, file, &mut Vec::new());
    if is_concrete_scalar_value(iterable, &iterable_type) {
        diagnostics.push(CheckDiagnostic::error(
            CHECK_COLLECTION_UNSUPPORTED,
            file,
            iterable.span(),
            "this value is a scalar, which cannot be iterated",
        ));
    }
}

pub(super) fn check_for_entries_support(
    program: &CheckedProgram,
    file: &Path,
    binding: &marrow_syntax::ForBinding,
    iterable: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    let Some(arg) = two_name_entries_loop_arg(binding, iterable) else {
        check_entries_value_position(file, iterable, diagnostics);
        return;
    };
    check_entries_loop_arg(program, file, arg, scope, aliases, diagnostics);
}

pub(crate) fn check_entries_value_position(
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
        Expression::Literal { .. }
        | Expression::Name { .. }
        | Expression::SavedRoot { .. }
        | Expression::Absent { .. } => {}
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
    program: &CheckedProgram,
    file: &Path,
    arg: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if is_any_collection_wrapper(arg) && !has_collection_unsupported(diagnostics, file, arg.span())
    {
        diagnostics.push(entries_unsupported(
            file,
            arg.span(),
            "`entries(...)` loop heads require a path or local collection",
        ));
        return;
    }
    check_entries_value_position(file, arg, diagnostics);
    match entries_loop_arg_status(program, arg, scope, aliases, file) {
        EntriesLoopArgStatus::Supported | EntriesLoopArgStatus::Unknown => {}
        EntriesLoopArgStatus::Unsupported => {
            if !has_collection_unsupported(diagnostics, file, arg.span()) {
                diagnostics.push(entries_unsupported(
                    file,
                    arg.span(),
                    "`entries(...)` loop heads require a path or local collection",
                ));
            }
        }
    }
}

enum EntriesLoopArgStatus {
    Supported,
    Unsupported,
    Unknown,
}

fn entries_loop_arg_status(
    program: &CheckedProgram,
    arg: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> EntriesLoopArgStatus {
    if saved_path_key_type(program, arg, scope, file).is_some() {
        // A non-unique index branch pairs each streamed identity with its
        // materialized record, so `entries(...)` is supported over it; only a
        // unique branch — a single-identity lookup — has no entry stream.
        return if is_saved_unique_index_branch_path(program, arg, scope, file) {
            EntriesLoopArgStatus::Unsupported
        } else {
            EntriesLoopArgStatus::Supported
        };
    }
    match local_iterable_type(program, arg, scope, aliases, file, false) {
        MarrowType::LocalTree { .. } | MarrowType::Sequence(_) => EntriesLoopArgStatus::Supported,
        MarrowType::Unknown => EntriesLoopArgStatus::Unknown,
        _ => EntriesLoopArgStatus::Unsupported,
    }
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

pub(super) fn has_collection_unsupported(
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

pub(super) fn saved_path_value_type(
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
    SavedPlaceResolver::new(program).record_root_element_type(place)
}

/// The store's resource type when `path` names a non-unique index branch, whose
/// streamed store identity materializes the whole record. `None` for a unique
/// index branch (a single-identity lookup) or any non-index path, so the caller
/// keeps its own value-typing for those shapes.
fn non_unique_index_branch_resource_type(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> Option<MarrowType> {
    let checked = checked_saved_expr(program, path, scope, file)?;
    let resolver = SavedPlaceResolver::new(program);
    if !resolver.non_unique_index_branch_yields_identity(&checked) {
        return None;
    }
    Some(saved_place_resource_type(program, checked.saved_place()?))
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

pub(crate) fn is_saved_path_with_key_range_arg(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> bool {
    checked_saved_expr(program, path, scope, file)
        .is_some_and(|expr| SavedPlaceResolver::new(program).has_key_range_arg(&expr))
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

pub(crate) fn is_partial_key_layer_path(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> bool {
    checked_saved_expr(program, path, scope, file)
        .is_some_and(|expr| SavedPlaceResolver::new(program).is_partial_key_layer_path(&expr))
}

pub(crate) fn is_saved_collection_path(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> bool {
    checked_saved_expr(program, path, scope, file)
        .is_some_and(|expr| SavedPlaceResolver::new(program).is_saved_collection(&expr))
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

pub(crate) fn is_saved_unique_index_branch_path(
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
