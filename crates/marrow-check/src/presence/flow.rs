//! Flow-sensitive `T?` → `T` narrowing as an inference refinement.
//!
//! Narrowing does not own *what* is optional — the type does. It only discharges
//! the one rule for a re-read of a saved place a guard has proven present, and
//! re-imposes optionality when that place could have been cleared. The state lives
//! beside the type pass as a sibling of `RequiredFieldAssignments`: the statement
//! checker enters and exits guarded, looped, and caught scopes, records writes and
//! field-writing calls, and the read inference consults [`Narrowing::current`] to
//! decide whether a maybe-present read still carries its `Optional`.
//!
//! Invalidation is conservative — alias-safe and effect-aware. A saved write whose
//! canonical key is not provably distinct from a narrowed key drops the narrowing,
//! a call whose effect footprint may write the field drops every saved narrowing,
//! and any place cleared anywhere in a loop body is re-imposed at the header.

use std::collections::HashMap;
use std::path::Path;

use super::TransformOldReadScope;
use super::effects::{
    condition_narrowings, negated_exists_narrowings, saved_targets,
    targets_invalidated_by_key_bindings, targets_invalidated_by_written_target,
    traversal_narrowing,
};
use super::keys::assigned_bindings;
use super::scope::NameScope;
use super::target::{ReadTarget, read_target_with_scope};
use super::util::extend_unique;
use super::writes::expr_calls_saved_writer;
use crate::executable::lower_expr_for_file;
use crate::{CheckedExpr, CheckedForBinding, CheckedProgram, MarrowType};

/// The scope a narrowing query resolves against: the live type scope plus the
/// optional `old` binding of an evolution transform. The flow primitives lower a
/// source expression to a [`CheckedExpr`] and resolve its read target against this.
#[derive(Clone, Copy)]
pub(crate) struct FlowCtx<'a> {
    pub(crate) program: &'a CheckedProgram,
    pub(crate) file: &'a Path,
    pub(crate) type_scope: &'a [HashMap<String, MarrowType>],
    pub(crate) transform_old: Option<TransformOldReadScope<'a>>,
}

impl FlowCtx<'_> {
    fn name_scope(&self) -> NameScope {
        NameScope::from_type_scope(self.type_scope, self.transform_old)
    }

    fn lower(&self, expr: &marrow_syntax::Expression) -> Option<CheckedExpr> {
        lower_expr_for_file(self.program, self.file, expr, self.type_scope)
    }

    fn read_target(&self, expr: &marrow_syntax::Expression) -> Option<ReadTarget> {
        let checked = self.lower(expr)?;
        read_target_with_scope(self.program, &checked, &self.name_scope())
    }

    /// The narrowings a guard condition proves in its then-block: an `exists(place)`
    /// (or `cond and exists(place)`) read target, with the whole condition's saved
    /// writes already screened off by [`condition_narrowings`].
    pub(crate) fn condition_narrowings(
        &self,
        condition: &marrow_syntax::Expression,
    ) -> Vec<ReadTarget> {
        match self.lower(condition) {
            Some(checked) => condition_narrowings(self.program, &checked, &self.name_scope()),
            None => Vec::new(),
        }
    }

    /// The narrowings a fall-through-preventing `if not exists(place)` proves for the
    /// statements that follow it.
    pub(crate) fn negated_exists_narrowings(
        &self,
        condition: &marrow_syntax::Expression,
    ) -> Vec<ReadTarget> {
        match self.lower(condition) {
            Some(checked) => negated_exists_narrowings(self.program, &checked, &self.name_scope()),
            None => Vec::new(),
        }
    }

    /// The narrowing a `for` loop proves for each iterated record read in its body: a
    /// composite root or index branch streams present record identities. A positional
    /// or keyed layer streams keys, not records, so the leaf read it indexes stays
    /// `T?` under the one rule.
    pub(crate) fn traversal_narrowing(
        &self,
        iterable: &marrow_syntax::Expression,
        binding: &marrow_syntax::ForBinding,
    ) -> Option<ReadTarget> {
        let checked = self.lower(iterable)?;
        let binding = CheckedForBinding {
            names: binding.names.iter().map(|n| n.name.clone()).collect(),
        };
        traversal_narrowing(self.program, &checked, &binding, &self.name_scope())
    }

    /// The read target a present `if const name = place` binds, unless the subject
    /// runs a saved write that would re-execute on every guarded read.
    pub(crate) fn if_const_subject_target(
        &self,
        value: &marrow_syntax::Expression,
    ) -> Option<ReadTarget> {
        if self.expr_writes_saved(value) {
            return None;
        }
        self.read_target(value)
    }

    /// Whether evaluating `expr` may run a function whose effect footprint writes
    /// saved data, so every saved narrowing must be dropped (fail-closed when the
    /// footprint is imprecise).
    pub(crate) fn expr_writes_saved(&self, expr: &marrow_syntax::Expression) -> bool {
        self.lower(expr)
            .is_some_and(|checked| expr_calls_saved_writer(self.program, &checked))
    }
}

/// Whether a maybe-present read has been proven present at this point in the flow,
/// so its inferred type drops the `Optional` layer. The read's canonical target must
/// equal a narrowed one — the same span-stripped, binding-id-keyed identity the
/// guard recorded — so an aliasing key or a reassigned binding no longer matches.
pub(crate) fn read_is_narrowed(
    program: &CheckedProgram,
    checked: &CheckedExpr,
    type_scope: &[HashMap<String, MarrowType>],
    transform_old: Option<TransformOldReadScope<'_>>,
    narrowed: &[ReadTarget],
) -> bool {
    if narrowed.is_empty() {
        return false;
    }
    let scope = NameScope::from_type_scope(type_scope, transform_old);
    read_target_with_scope(program, checked, &scope)
        .is_some_and(|target| narrowed.contains(&target))
}

/// The set of saved places proven present at the current point in a function body.
/// Threaded through the statement type pass beside `RequiredFieldAssignments`: a
/// guard, loop, or negated-exists adds to it; a write, reassignment, or
/// field-writing call removes from it; a loop body re-imposes any place it cleared.
#[derive(Default)]
pub(crate) struct Narrowing {
    narrowed: Vec<ReadTarget>,
    tracking: Vec<TrackLevel>,
}

/// A scope (loop body, try/catch body, or guarded block) whose entry narrowings are
/// re-imposed on exit: any of them cleared inside is dropped from the surviving set,
/// even if the body later re-proved it, so the next iteration or the fall-through
/// re-triggers the one rule.
struct TrackLevel {
    entry: Vec<ReadTarget>,
    invalidated: Vec<ReadTarget>,
}

/// A saved snapshot of the narrowed set, restored when its scope exits.
pub(crate) struct Snapshot(Vec<ReadTarget>);

impl Narrowing {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn current(&self) -> &[ReadTarget] {
        &self.narrowed
    }

    /// Add proven-present narrowings to the live set (a negated-exists guard that
    /// holds for the remaining statements, or a guard condition's narrowings).
    pub(crate) fn add(&mut self, targets: Vec<ReadTarget>) {
        extend_unique(&mut self.narrowed, targets);
    }

    /// Enter a nested scope, augmenting the live set with the scope's own
    /// narrowings (a guard condition or a loop traversal). The returned snapshot
    /// restores the pre-scope set on exit, minus anything the scope invalidated.
    pub(crate) fn enter(&mut self, augment: Vec<ReadTarget>) -> Snapshot {
        let entry = self.narrowed.clone();
        self.tracking.push(TrackLevel {
            entry: entry.clone(),
            invalidated: Vec::new(),
        });
        extend_unique(&mut self.narrowed, augment);
        Snapshot(entry)
    }

    pub(crate) fn exit(&mut self, snapshot: Snapshot) {
        let level = self.tracking.pop().expect("balanced narrowing scope");
        self.narrowed = snapshot.0;
        self.narrowed
            .retain(|target| !level.invalidated.contains(target));
    }

    /// Drop any narrowing a write to `target` could have cleared: a reassigned key
    /// binding, or an overlapping or alias-possible saved write.
    pub(crate) fn invalidate_write(&mut self, ctx: &FlowCtx, target: &marrow_syntax::Expression) {
        let Some(checked) = ctx.lower(target) else {
            return;
        };
        let scope = ctx.name_scope();
        // A saved-path write addresses one node, so it invalidates only the
        // member-precise written target, which already covers same-field
        // alias-possible keys. A saved path merely *uses* its key bindings to
        // address that node; dropping every narrowing those bindings appear in
        // would expire unrelated sibling-field and other-resource narrowings. A
        // local reassignment — a bare name or a record field a narrowed key reads —
        // instead rebinds that key, so every narrowing keyed on it must drop.
        if checked.saved_place().is_none() {
            let assigned = assigned_bindings(&checked, &scope);
            let by_binding = targets_invalidated_by_key_bindings(&self.narrowed, &assigned);
            self.invalidate(by_binding);
        }
        if let Some(written) = read_target_with_scope(ctx.program, &checked, &scope) {
            let by_write = targets_invalidated_by_written_target(&self.narrowed, &written);
            self.invalidate(by_write);
        }
    }

    /// Drop every saved narrowing, the fail-closed response to a call whose effect
    /// footprint may write saved data.
    pub(crate) fn invalidate_saved(&mut self) {
        let invalidated = saved_targets(&self.narrowed);
        self.invalidate(invalidated);
    }

    fn invalidate(&mut self, invalidated: Vec<ReadTarget>) {
        if invalidated.is_empty() {
            return;
        }
        self.narrowed.retain(|target| !invalidated.contains(target));
        for level in &mut self.tracking {
            let tracked: Vec<ReadTarget> = invalidated
                .iter()
                .filter(|target| level.entry.contains(target))
                .cloned()
                .collect();
            extend_unique(&mut level.invalidated, tracked);
        }
    }
}
