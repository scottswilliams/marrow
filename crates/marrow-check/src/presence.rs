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
pub(crate) use writes::{effect_closure, effect_closure_for_direct};

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

/// Optionality lives in the type and is enforced by the one rule during type
/// inference, so this post-lowering pass owns only the `nextId` collision check —
/// the remaining structural fact the type pass cannot see.
pub(crate) fn check_next_id_collisions(
    program: &mut crate::CheckedProgram,
    diagnostics: &mut Vec<crate::CheckDiagnostic>,
) {
    nextid::check_next_id_collisions(program, diagnostics);
}
