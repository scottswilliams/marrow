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
