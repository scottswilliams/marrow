//! Erase closures and presence-use intervals over the existing ordered call logs.
//!
//! One word per minted function is reused for stripes of 64 queried families.
//! Pending interval chains are drained only by a matching eraser; each query is
//! queued and drained at most once, including queries that have already expired.

use std::collections::{HashMap, HashSet};

use super::{AcyclicCallGraph, CompleteLoweredFunctionSet, DiagnosticCollector, LoweredFn};
use crate::durable::Family;
use crate::lower::{PresenceObligation, requires_presence};
#[cfg(test)]
use crate::types::bump_call_graph;

struct Query<'a> {
    function: usize,
    family: usize,
    original: usize,
    obligation: &'a PresenceObligation,
}

impl Query<'_> {
    fn stripe(&self) -> usize {
        self.family / 64
    }

    fn use_key(&self) -> (usize, usize, usize, u32, u32, usize) {
        let span = self.obligation.span;
        (
            self.function,
            span.start_byte,
            span.end_byte,
            span.line,
            span.column,
            self.family,
        )
    }
}

struct Erase {
    stripe: usize,
    function: usize,
    bit: u64,
}

struct Failure {
    query: usize,
    call: usize,
}

/// Reject protected uses whose ordinary or crossed-loop interval contains an
/// entry-erasing call. No function body or graph analysis is repeated here.
pub(super) fn reject_unproven_uses(
    lowered: &CompleteLoweredFunctionSet,
    acyclic: &AcyclicCallGraph,
    diagnostics: &mut DiagnosticCollector,
) {
    let functions = lowered.functions();
    if !functions.iter().any(|function| {
        function
            .presence_obligations
            .iter()
            .any(|obligation| !obligation.calls.is_empty())
    }) {
        return;
    }
    let erased: HashSet<&Family> = functions
        .iter()
        .flat_map(|function| &function.erased_families)
        .collect();
    let (families, queries) = collect_queries(functions, &erased);
    #[cfg(test)]
    bump_call_graph(|counts| {
        counts.presence_erased_families = counts.presence_erased_families.max(erased.len());
        counts.presence_query_rows = counts.presence_query_rows.max(queries.len());
    });
    if queries.is_empty() {
        return;
    }
    let erases = collect_erases(functions, &families);
    let mut summary = vec![0u64; functions.len()];
    #[cfg(test)]
    bump_call_graph(|counts| {
        counts.presence_summary_words = counts.presence_summary_words.max(summary.len());
        counts.presence_erase_rows = counts.presence_erase_rows.max(erases.len());
        counts.presence_families += families.len();
    });
    let mut next = Vec::new();
    let mut failures = Vec::new();
    let mut erase_cursor = 0;
    let mut query_cursor = 0;
    for stripe in 0..families.len().div_ceil(64) {
        #[cfg(test)]
        bump_call_graph(|counts| counts.presence_stripes += 1);
        summary.fill(0);
        while let Some(erase) = erases
            .get(erase_cursor)
            .filter(|erase| erase.stripe == stripe)
        {
            summary[erase.function] |= erase.bit;
            erase_cursor += 1;
        }
        propagate(functions, acyclic, &mut summary);
        while let Some(query) = queries
            .get(query_cursor)
            .filter(|query| query.stripe() == stripe)
        {
            let function = query.function;
            let end = query_cursor
                + queries[query_cursor..].partition_point(|query| {
                    query.stripe() == stripe && query.function == function
                });
            settle(
                &functions[function].callees,
                &summary,
                &queries[query_cursor..end],
                query_cursor,
                &mut next,
                &mut failures,
            );
            query_cursor = end;
        }
    }
    report(functions, &queries, failures, diagnostics);
}

fn collect_queries<'a>(
    functions: &'a [LoweredFn],
    erased: &HashSet<&Family>,
) -> (HashMap<&'a Family, usize>, Vec<Query<'a>>) {
    let mut families = HashMap::new();
    let mut queries = Vec::new();
    for (function, lowered) in functions.iter().enumerate() {
        for (original, obligation) in lowered.presence_obligations.iter().enumerate() {
            if obligation.calls.is_empty() {
                continue;
            }
            #[cfg(test)]
            bump_call_graph(|counts| counts.presence_obligations += 1);
            if !erased.contains(&obligation.family) {
                continue;
            }
            let ordinal = families.len();
            let family = *families.entry(&obligation.family).or_insert(ordinal);
            queries.push(Query {
                function,
                family,
                original,
                obligation,
            });
        }
    }
    queries.sort_unstable_by_key(|query| {
        (
            query.stripe(),
            query.function,
            query.obligation.calls.start,
            query.obligation.calls.end,
            query.original,
        )
    });
    (families, queries)
}

fn collect_erases(functions: &[LoweredFn], families: &HashMap<&Family, usize>) -> Vec<Erase> {
    let mut erases = Vec::new();
    for (function, lowered) in functions.iter().enumerate() {
        for family in &lowered.erased_families {
            if let Some(&ordinal) = families.get(family) {
                erases.push(Erase {
                    stripe: ordinal / 64,
                    function,
                    bit: 1 << (ordinal % 64),
                });
            }
        }
    }
    erases.sort_unstable_by_key(|erase| (erase.stripe, erase.function, erase.bit));
    erases
}

fn propagate(functions: &[LoweredFn], acyclic: &AcyclicCallGraph, summary: &mut [u64]) {
    for &function in acyclic.order().callee_before_caller() {
        #[cfg(test)]
        bump_call_graph(|counts| counts.presence_row_visits += 1);
        let mut word = summary[function];
        for &callee in &functions[function].callees {
            #[cfg(test)]
            bump_call_graph(|counts| counts.presence_edge_visits += 1);
            // Diagnostic-only lowering may retain a reserved, undrained callee.
            word |= summary.get(usize::from(callee)).copied().unwrap_or(0);
        }
        summary[function] = word;
    }
}

fn settle(
    calls: &[u16],
    summary: &[u64],
    queries: &[Query<'_>],
    offset: usize,
    next: &mut Vec<Option<usize>>,
    failures: &mut Vec<Failure>,
) {
    let mut head = [None; 64];
    let mut pending = 0u64;
    let mut cursor = 0;
    next.resize(queries.len(), None);
    #[cfg(test)]
    bump_call_graph(|counts| {
        counts.presence_next_slots = counts.presence_next_slots.max(next.len())
    });
    let end = queries
        .iter()
        .map(|query| query.obligation.calls.end)
        .max()
        .unwrap_or(0);
    for (position, &callee) in calls[..end].iter().enumerate() {
        #[cfg(test)]
        bump_call_graph(|counts| counts.presence_query_positions += 1);
        while let Some(query) = queries
            .get(cursor)
            .filter(|query| query.obligation.calls.start <= position)
        {
            if position < query.obligation.calls.end {
                let bit = query.family % 64;
                next[cursor] = head[bit];
                head[bit] = Some(cursor);
                pending |= 1 << bit;
                #[cfg(test)]
                bump_call_graph(|counts| counts.presence_queries_queued += 1);
            }
            cursor += 1;
        }
        if pending == 0 {
            continue;
        }
        #[cfg(test)]
        bump_call_graph(|counts| counts.presence_summary_lookups += 1);
        let mut hits = pending & summary.get(usize::from(callee)).copied().unwrap_or(0);
        pending &= !hits;
        while hits != 0 {
            let bit = hits.trailing_zeros() as usize;
            hits &= hits - 1;
            let mut chain = head[bit].take();
            while let Some(index) = chain {
                #[cfg(test)]
                bump_call_graph(|counts| counts.presence_queries_drained += 1);
                if position < queries[index].obligation.calls.end {
                    failures.push(Failure {
                        query: offset + index,
                        call: position,
                    });
                }
                chain = next[index];
            }
        }
    }
    #[cfg(test)]
    bump_call_graph(|counts| {
        counts.presence_failure_rows = counts.presence_failure_rows.max(failures.len());
    });
}

fn report(
    functions: &[LoweredFn],
    queries: &[Query<'_>],
    mut failures: Vec<Failure>,
    diagnostics: &mut DiagnosticCollector,
) {
    failures.sort_unstable_by_key(|failure| (queries[failure.query].use_key(), failure.call));
    let mut previous = None;
    for failure in failures {
        let query = &queries[failure.query];
        let key = query.use_key();
        if previous == Some(key) {
            continue;
        }
        previous = Some(key);
        let function = &functions[query.function];
        let callee = function.callees[failure.call];
        let name = functions[usize::from(callee)].name.as_str();
        diagnostics.push(requires_presence(
            &function.file,
            query.obligation.span,
            &format!(
                "the call to `{name}` before it erases an entry of the family, so the \
                 proof ended at the call"
            ),
        ));
    }
}

#[cfg(test)]
mod tests;
