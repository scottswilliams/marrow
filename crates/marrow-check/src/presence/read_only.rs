use marrow_syntax::SourceSpan;

use crate::executable::{CheckedBodyVisitor, walk_checked_expr};
use crate::facts::{DirectEffectFacts, EffectClosureFacts};
use crate::{CheckedBuiltinCall, CheckedCallTarget, CheckedExpr, CheckedProgram};

use super::{direct_effects_for_expr, effect_closure_for_direct};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadOnlyExpressionEffects {
    pub(crate) direct: DirectEffectFacts,
    pub(crate) closure: EffectClosureFacts,
    pub(crate) saved_write_span: Option<SourceSpan>,
    pub(crate) unindexed_lookup_span: Option<SourceSpan>,
}

impl ReadOnlyExpressionEffects {
    pub(crate) fn writes_reachable(&self) -> bool {
        !self.direct.saved_writes.is_empty()
            || !self.direct.store_writes.is_empty()
            || !self.direct.saved_index_writes.is_empty()
            || self.direct.transactions
            || self.closure.transactions
            || self.closure.write_effects_reachable
    }

    pub(crate) fn host_effects_reachable(&self) -> bool {
        !self.direct.host_calls.is_empty() || !self.closure.host_calls.is_empty()
    }

    pub(crate) fn unindexed_collection_reads_reachable(&self) -> bool {
        self.direct.unindexed_collection_reads || self.closure.unindexed_collection_reads
    }

    /// Whether this expression carries any effect — a write, allocation
    /// (`append`/`nextId`), host call, throw, or an opaque user-function call —
    /// that disqualifies it from a presence guard's key or base position. A guard
    /// resolves a maybe-present read by catching the absent fault at the read
    /// site, so its sub-expressions must be effect-free pure reads; an effect
    /// smuggled in as a key would run every time the guard is evaluated.
    ///
    /// Only the direct effects of the expression itself are inspected, because the
    /// guard predicates run before per-function effect closures are computed. A
    /// user-function call is therefore opaque here: its body may write, so it is
    /// rejected on sight rather than admitted as a pure read.
    fn guard_effect_reachable(&self) -> bool {
        !self.direct.saved_writes.is_empty()
            || !self.direct.store_writes.is_empty()
            || !self.direct.saved_index_writes.is_empty()
            || self.direct.transactions
            || self.direct.throws
            || !self.direct.host_calls.is_empty()
            || !self.direct.user_function_calls.is_empty()
            || self.saved_write_span.is_some()
    }
}

/// Whether `expression` may appear as the key or base of a presence guard. The
/// read place itself is already known guardable; this screens its sub-expressions
/// so an effect — `nextId(^s)`, `append(...)`, or any user-function call whose
/// body the guard cannot yet see — can never ride into the guard.
pub(super) fn guard_subexpr_admissible(program: &CheckedProgram, expression: &CheckedExpr) -> bool {
    !read_only_expression_effects(program, expression).guard_effect_reachable()
}

pub(crate) fn read_only_expression_effects(
    program: &CheckedProgram,
    expression: &CheckedExpr,
) -> ReadOnlyExpressionEffects {
    let direct = direct_effects_for_expr(&program.facts, expression);
    let closure = effect_closure_for_direct(program, &direct);
    let mut spans = ReadOnlySpanCollector::default();
    spans.visit_expr(expression);
    ReadOnlyExpressionEffects {
        direct,
        closure,
        saved_write_span: spans.saved_write_span,
        unindexed_lookup_span: spans.unindexed_lookup_span,
    }
}

#[derive(Default)]
struct ReadOnlySpanCollector {
    saved_write_span: Option<SourceSpan>,
    unindexed_lookup_span: Option<SourceSpan>,
}

impl CheckedBodyVisitor for ReadOnlySpanCollector {
    fn visit_expr(&mut self, expression: &CheckedExpr) {
        if self.saved_write_span.is_none() {
            self.saved_write_span = direct_saved_write_span(expression);
        }
        if self.unindexed_lookup_span.is_none() {
            self.unindexed_lookup_span = direct_unindexed_lookup_span(expression);
        }
        walk_checked_expr(self, expression);
    }
}

fn direct_saved_write_span(expression: &CheckedExpr) -> Option<SourceSpan> {
    let CheckedExpr::Call { target, span, .. } = expression else {
        return None;
    };
    matches!(
        target,
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Append)
            | CheckedCallTarget::Builtin(CheckedBuiltinCall::NextId)
    )
    .then_some(*span)
}

fn direct_unindexed_lookup_span(expression: &CheckedExpr) -> Option<SourceSpan> {
    super::direct::unindexed_collection_lookup(expression).then_some(expression.span())
}
