//! Saved-path shape predicates and layer key/value types. These answer "what
//! shape is this saved place" for callers outside the loop head — index-branch
//! and key-range classification, partial-layer detection, and the key/value types
//! a saved path streams — by delegating to the one `SavedPlaceResolver`.

use std::collections::HashMap;
use std::path::Path;

use crate::executable::{SavedPlaceResolver, lower_expr_for_file};
use crate::{CheckedExpr, CheckedProgram, CheckedSavedPlace, MarrowType};

pub(crate) fn checked_saved_expr(
    program: &CheckedProgram,
    path: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    file: &Path,
) -> Option<CheckedExpr> {
    lower_expr_for_file(program, file, path, scope)
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

pub(super) fn saved_path_direct_value_type(
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

pub(super) fn saved_place_resource_type(
    program: &CheckedProgram,
    place: &CheckedSavedPlace,
) -> MarrowType {
    SavedPlaceResolver::new(program).record_root_element_type(place)
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

/// Whether `expr` of type `ty` is a concrete non-iterable scalar — an `int`,
/// `string`, `bool`, enum value, or store identity. A range literal types to its
/// endpoint scalar but is genuinely iterable, so it is excluded syntactically.
/// `Unknown` defers, keeping cross-module unresolved values free of false positives.
pub(crate) fn is_concrete_scalar_value(expr: &marrow_syntax::Expression, ty: &MarrowType) -> bool {
    if marrow_syntax::range_expr(expr).is_some() || crate::rules::is_saved_path(expr) {
        return false;
    }
    matches!(
        ty,
        MarrowType::Primitive(_) | MarrowType::Enum(_) | MarrowType::Identity(_)
    )
}
