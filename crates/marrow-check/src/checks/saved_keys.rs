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
    SavedKeyParamTarget, SavedPlaceResolver, is_single_int_sequence, lower_expr_for_file,
};
use crate::model::decls::DeclIds;
use crate::typerules::{
    is_optional_value, is_ordered, marrow_type_name, type_compatible,
    unresolved_optional_diagnostic,
};
use crate::{
    CallArgumentFault, CheckDiagnostic, CheckedProgram, CheckedSavedIndexKey, CheckedSavedKeyParam,
    CheckedSavedPlace, CheckedSavedTerminal, MarrowType, identity_type_for_store,
};

use super::calls::call_argument;
use super::const_int::fold_const_int;
use super::diagnostics::key_type_diagnostic;

/// A sequence-position write target and the environment its position folds in.
pub(crate) struct SequencePositionWrite<'a, 'd> {
    pub(crate) program: &'a CheckedProgram,
    pub(crate) target: &'a marrow_syntax::Expression,
    pub(crate) scope: &'a [HashMap<String, MarrowType>],
    pub(crate) const_ints: &'a [HashMap<String, Option<i64>>],
    pub(crate) aliases: &'a HashMap<String, Vec<String>>,
    pub(crate) span: SourceSpan,
    pub(crate) file: &'a Path,
    pub(crate) diagnostics: &'d mut Vec<CheckDiagnostic>,
}

/// Reject a write to a sequence position the spec proves addresses no node. Every
/// single int-keyed layer is the canonical 1-based sequence shape — saved or local,
/// spelled `sequence[T]` or `name(k: int): V` — so a statically-known zero or
/// negative position can never be written to any of them. The position is folded
/// statically: a literal, a `const` binding, or integer arithmetic over either all
/// resolve at check, while a dynamic position folds to nothing and stays a run
/// fault. This is the static counterpart of the absent fault a dynamic non-positive
/// position raises at run time, and it is a write-target rule only: a guarded read
/// of such a position resolves to absent at run time.
pub(crate) fn check_sequence_position_write(check: SequencePositionWrite<'_, '_>) {
    for call in keyed_layer_calls_in_chain(check.target) {
        let Some(position) = static_non_positive_call_position(
            check.program,
            call,
            check.scope,
            check.const_ints,
            check.aliases,
            check.file,
        ) else {
            continue;
        };
        check.diagnostics.push(CheckDiagnostic::error(
            CHECK_SEQUENCE_POSITION,
            check.file,
            check.span,
            format!(
                "sequence positions are 1-based, so position `{position}` addresses no node and cannot be written",
            ),
        ));
    }
}

/// The keyed-layer calls in a write target's access chain, outermost first. A
/// history layer keys an entry whose fields are addressed beyond the call, so the
/// position-bearing call is nested under field accesses rather than being the
/// target itself; every such call along the chain is a sequence-position candidate.
fn keyed_layer_calls_in_chain(
    target: &marrow_syntax::Expression,
) -> Vec<&marrow_syntax::Expression> {
    let mut calls = Vec::new();
    let mut node = target;
    loop {
        match node {
            marrow_syntax::Expression::Call { callee, .. } => {
                calls.push(node);
                node = callee;
            }
            marrow_syntax::Expression::Field { base, .. }
            | marrow_syntax::Expression::OptionalField { base, .. } => node = base,
            _ => break,
        }
    }
    calls
}

/// The statically-known non-positive position a keyed-layer call addresses, whether
/// it lands on a saved single int-keyed layer or a local single int-keyed
/// collection. `None` for any other shape, an in-range position, or a position that
/// does not fold to a constant.
fn static_non_positive_call_position(
    program: &CheckedProgram,
    call: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    const_ints: &[HashMap<String, Option<i64>>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> Option<i64> {
    if !target_is_single_int_sequence(program, call, scope, aliases, file) {
        return None;
    }
    let [position] = sequence_position_args(call)? else {
        return None;
    };
    fold_const_int(&position.value, const_ints).filter(|position| *position < 1)
}

/// Whether a write target addresses a single int-keyed sequence position, saved or
/// local. A saved target is single int-keyed when its final keyed layer is, or — for
/// a whole-record store-root write — when the store's sole identity key is an integer;
/// a single-integer store key is itself a 1-based sequence. A local target's callee
/// must be a `sequence[T]` or a single int-keyed tree.
fn target_is_single_int_sequence(
    program: &CheckedProgram,
    target: &marrow_syntax::Expression,
    scope: &[HashMap<String, MarrowType>],
    aliases: &HashMap<String, Vec<String>>,
    file: &Path,
) -> bool {
    if let Some(checked) = lower_expr_for_file(program, file, target, scope)
        && let Some(place) = checked.saved_place()
    {
        return match place.layers.last() {
            Some(layer) => is_single_int_sequence(&layer.key_params),
            None => is_single_int_sequence(&place.identity_keys),
        };
    }
    local_single_int_key_target(program, target, scope, aliases, file)
}

/// The arguments of the outermost call of a write target — its sequence position.
/// `None` when the target is not a call.
fn sequence_position_args(target: &marrow_syntax::Expression) -> Option<&[Argument]> {
    let marrow_syntax::Expression::Call { args, .. } = target else {
        return None;
    };
    Some(args.as_slice())
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

/// Reject an `Id(^store, key)` whose statically-known single-int key names no record.
/// A store with one `int` identity key is a 1-based sequence, identical to the
/// `^store(0)` write address, so a folded zero or negative key addresses no record and
/// is caught here rather than faulting `run.absent_element`. The key folds in the same
/// const-int environment as the write address: a literal, a `const` binding, or integer
/// arithmetic over either resolves at check, while a dynamic key folds to nothing and
/// stays a run fault; a composite or non-`int` identity carries such keys with meaning
/// and is unaffected.
pub(crate) fn check_identity_sequence_position(
    store: &StoreSchema,
    key_args: &[Argument],
    const_ints: &[HashMap<String, Option<i64>>],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if !store.single_int_root() {
        return;
    }
    let [key] = key_args else {
        return;
    };
    let Some(position) = fold_const_int(&key.value, const_ints).filter(|position| *position < 1)
    else {
        return;
    };
    diagnostics.push(CheckDiagnostic::error(
        CHECK_SEQUENCE_POSITION,
        file,
        span,
        format!("sequence positions are 1-based, so position `{position}` addresses no node"),
    ));
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
    let names = check.program.decl_ids();
    check_saved_key_argument_names(&names, check.args, check.file, check.diagnostics);
    match target {
        SavedKeyParamTarget::Root(place) => {
            check_root_args_against(
                &names,
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
                &names,
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
    names: &DeclIds<'_>,
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
    check_supplied_layer_keys(names, keys, args, arg_types, span, file, diagnostics);
}

/// Match each supplied key argument against the column it fills. A range argument,
/// when present, is checked in range position and must be the final argument; every
/// other argument is checked nominally. Shared by the exact-arity full-address
/// caller and the partial-prefix layer caller, which screen arity beforehand.
fn check_supplied_layer_keys(
    names: &DeclIds<'_>,
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
                names,
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
        if reject_optional_key_arg(arg_type, span, file, diagnostics) {
            continue;
        }
        if !saved_key_arg_matches(&expected, arg_type) {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                format!(
                    "key `{}` expects `{}`, but this value is `{}`",
                    key.name,
                    marrow_type_name(names, &expected),
                    marrow_type_name(names, arg_type),
                ),
            ));
        }
    }
}

fn check_root_args_against(
    names: &DeclIds<'_>,
    place: &CheckedSavedPlace,
    args: &[Argument],
    arg_types: &[MarrowType],
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if range_arg_position(args).is_some() {
        check_checked_key_args(
            names,
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
        let expected = names
            .root_id(&place.root)
            .map_or(MarrowType::Unknown, MarrowType::Identity);
        if type_compatible(&expected, &arg_types[0]) == Some(false) {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                format!(
                    "`^{}` is addressed by `{}`, but this value is `{}`",
                    place.root,
                    marrow_type_name(names, &expected),
                    marrow_type_name(names, &arg_types[0]),
                ),
            ));
        }
        return;
    }
    check_checked_key_args(
        names,
        &place.identity_keys,
        args,
        arg_types,
        span,
        file,
        diagnostics,
    );
}

pub(crate) fn saved_root_args_address_record(
    program: &CheckedProgram,
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
        return type_compatible(&identity_type_for_store(program, store), &arg_types[0])
            != Some(false);
    }
    if args.len() != store.identity_keys.len() {
        return false;
    }
    store
        .identity_keys
        .iter()
        .zip(arg_types)
        .all(|(key, arg_type)| {
            let expected = MarrowType::from_resolved(key.ty.clone());
            saved_key_arg_matches(&expected, arg_type)
        })
}

fn check_saved_key_argument_names(
    names: &DeclIds<'_>,
    args: &[Argument],
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    for arg in args {
        if arg.name.is_some() {
            diagnostics.push(call_argument(
                names,
                file,
                arg.value.span(),
                CallArgumentFault::SavedKeyArgumentsPositional,
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

    let names = program.decl_ids();
    let resolver = SavedPlaceResolver::new(program);
    for (position, (component, arg_type)) in keys.iter().zip(arg_types).enumerate() {
        let expected = resolver.saved_index_key_type(component);
        if range_arg == Some(position) {
            check_range_key_arg(
                &names,
                RangeKeyArg {
                    expected: &expected,
                    actual: arg_type,
                    component: format!("index component `{}`", component.name),
                    arg: &args[position].value,
                    allow_enum: true,
                },
                span,
                file,
                diagnostics,
            );
            continue;
        }
        if reject_optional_key_arg(arg_type, span, file, diagnostics) {
            continue;
        }
        if !saved_key_arg_matches(&expected, arg_type) {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                format!(
                    "index component `{}` expects `{}`, but this value is `{}`",
                    component.name,
                    marrow_type_name(&names, &expected),
                    marrow_type_name(&names, arg_type),
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
    names: &DeclIds<'_>,
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
    check_supplied_layer_keys(names, keys, args, arg_types, span, file, diagnostics);
}

struct RangeKeyArg<'a> {
    expected: &'a MarrowType,
    actual: &'a MarrowType,
    component: String,
    arg: &'a marrow_syntax::Expression,
    allow_enum: bool,
}

fn check_range_key_arg(
    names: &DeclIds<'_>,
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
                marrow_type_name(names, check.expected),
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
                marrow_type_name(names, check.expected),
                marrow_type_name(names, check.actual),
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
    names: &DeclIds<'_>,
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
        let expected = MarrowType::from_resolved(key.ty.clone());
        if reject_optional_key_arg(arg_type, span, file, diagnostics) {
            continue;
        }
        if !saved_key_arg_matches(&expected, arg_type) {
            diagnostics.push(key_type_diagnostic(
                file,
                span,
                format!(
                    "key `{}` expects `{}`, but this value is `{}`",
                    key.name,
                    marrow_type_name(names, &expected),
                    marrow_type_name(names, arg_type),
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

/// A key must be present to address a node, so a maybe-present (`T?`) key argument is the
/// one rule. Routing it through the shared helper before the generic key-type mismatch
/// names the four resolution forms, consistent with the value slot sites. Returns whether
/// the argument was rejected as optional.
fn reject_optional_key_arg(
    arg_type: &MarrowType,
    span: SourceSpan,
    file: &Path,
    diagnostics: &mut Vec<CheckDiagnostic>,
) -> bool {
    if is_optional_value(arg_type) {
        diagnostics.push(unresolved_optional_diagnostic(file, span));
        return true;
    }
    false
}
