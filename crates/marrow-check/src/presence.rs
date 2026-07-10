use std::collections::HashMap;
use std::path::Path;

mod calls;
mod direct;
mod effects;
mod flow;
mod keys;
mod nextid;
mod read_only;
mod scope;
mod target;
mod util;
mod writes;

pub(crate) use direct::{direct_effects_for_block, direct_effects_for_expr};
pub(crate) use flow::{FlowCtx, Narrowing, read_is_narrowed};
pub(crate) use read_only::{
    ReadOnlyExpressionEffects, read_only_expression_effects, transitive_unindexed_lookup_span,
};
pub(crate) use target::{
    ReadTarget, exists_target_in_type_scope, read_value_resolves_in_type_scope,
};
pub(crate) use writes::{build_function_closure, effect_closure, effect_closure_for_direct};

#[derive(Clone, Copy)]
pub(crate) struct TransformOldReadScope<'a> {
    pub(crate) resource: &'a str,
    pub(crate) frame: usize,
}

/// The read-time scope an inference query threads: the optional evolution-transform
/// `old` binding and the saved places flow narrowing has proven present. Bundled so
/// the single read-optionality site ([`crate::infer`]) consults both without a
/// second threaded parameter.
#[derive(Clone, Copy)]
pub(crate) struct ReadScope<'a> {
    pub(crate) transform_old: Option<TransformOldReadScope<'a>>,
    pub(crate) narrowed: &'a [ReadTarget],
}

impl<'a> ReadScope<'a> {
    pub(crate) fn none() -> Self {
        Self {
            transform_old: None,
            narrowed: &[],
        }
    }

    pub(crate) fn transform(transform_old: Option<TransformOldReadScope<'a>>) -> Self {
        Self {
            transform_old,
            narrowed: &[],
        }
    }

    pub(crate) fn new(
        transform_old: Option<TransformOldReadScope<'a>>,
        narrowed: &'a [ReadTarget],
    ) -> Self {
        Self {
            transform_old,
            narrowed,
        }
    }
}

/// Whether a presence guard's subject read (`??`, `if const`, `exists`) carries an
/// effect in a saved key position, which a guard may not run. The subject is lowered
/// and screened through the one guard-key owner; a subject that does not lower, or
/// names no saved place, carries no such effect. The guard sites reject an
/// effectful-key saved read rather than run its effect on every evaluation, while the
/// read still classifies maybe-present for the bare-read and compound-assign rules.
pub(crate) fn guard_subject_key_effect(
    program: &crate::CheckedProgram,
    expr: &marrow_syntax::Expression,
    scope: &[HashMap<String, crate::MarrowType>],
    file: &Path,
) -> bool {
    match crate::executable::lower_expr_for_file(program, file, expr, scope) {
        Some(checked) => target::guard_subject_key_effect_reachable(program, &checked),
        None => false,
    }
}

/// Optionality lives in the type and is enforced by the one rule during type
/// inference, so this post-lowering pass owns only the `nextId` collision check —
/// the remaining structural fact the type pass cannot see.
pub(crate) fn check_next_id_collisions(
    program: &mut crate::CheckedProgram,
    diagnostics: &mut Vec<crate::CheckDiagnostic>,
) {
    nextid::check_next_id_collisions(program, diagnostics);
}
