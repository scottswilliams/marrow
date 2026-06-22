mod calls;
mod direct;
mod effects;
mod keys;
mod nextid;
mod proofs;
mod read_only;
mod scope;
mod target;
mod util;
mod walk;
mod writes;

pub(crate) use direct::{direct_effects_for_block, direct_effects_for_expr};
pub(crate) use read_only::{
    ReadOnlyExpressionEffects, read_only_expression_effects, transitive_unindexed_lookup_span,
};
pub(crate) use target::{
    bindable_saved_value_read_in_type_scope, exists_target_in_type_scope,
    read_value_resolves_in_type_scope,
};
pub(crate) use writes::{effect_closure, effect_closure_for_direct};

#[derive(Clone, Copy)]
pub(crate) struct TransformOldReadScope<'a> {
    pub(crate) resource: &'a str,
    pub(crate) frame: usize,
}

pub(crate) fn check_presence(
    program: &mut crate::CheckedProgram,
    diagnostics: &mut Vec<crate::CheckDiagnostic>,
) {
    walk::check_presence(program, diagnostics);
    nextid::check_next_id_collisions(program, diagnostics);
}
