//! Erase closures and presence queries over shared, parent-linked call paths.
//!
//! One word per minted function is reused for stripes of 64 queried families.
//! Each path node and query is visited once per relevant stripe. A loop join also
//! visits its original call range: nested joins repeat a call at most once per
//! enclosing loop, bounded by the syntax nesting limit. Paths are never expanded
//! into a separate interval list for each protected use.

use std::collections::{HashMap, HashSet};

use super::{AcyclicCallOrder, DiagnosticCollector, LoweredFn, LoweredFunctionSet};
use crate::durable::Family;
use crate::lower::{PresenceCallNode, PresenceObligation, requires_presence};
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

/// Reject protected uses whose ordinary or crossed-loop path contains an
/// entry-erasing call. No function body or graph analysis is repeated here.
pub(super) fn reject_unproven_uses(
    lowered: &LoweredFunctionSet,
    acyclic: &AcyclicCallOrder,
    diagnostics: &mut DiagnosticCollector,
) {
    let functions = lowered.functions();
    if !lowered
        .eligible(acyclic)
        .any(|function| !function.presence_obligations.is_empty())
    {
        return;
    }
    let erased: HashSet<&Family> = lowered
        .eligible(acyclic)
        .flat_map(|function| &function.erased_families)
        .collect();
    let (families, queries) = collect_queries(functions, acyclic, &erased);
    if queries.is_empty() {
        return;
    }
    let erases = collect_erases(functions, acyclic, &families);
    let mut summary = vec![0u64; functions.len()];
    let mut failures = Vec::new();
    let mut erase_cursor = 0;
    let mut query_cursor = 0;
    for stripe in 0..families.len().div_ceil(64) {
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
            #[expect(
                clippy::expect_used,
                reason = "collect_queries retains only bodies in the closed eligible component"
            )]
            let body = functions[function]
                .as_ref()
                .expect("queries retain eligible functions");
            settle(
                &body.callees,
                &body.presence_calls,
                &summary,
                &queries[query_cursor..end],
                query_cursor,
                &mut failures,
            );
            query_cursor = end;
        }
    }
    report(functions, &queries, failures, diagnostics);
}

fn collect_queries<'a>(
    functions: &'a [Option<LoweredFn>],
    acyclic: &AcyclicCallOrder,
    erased: &HashSet<&Family>,
) -> (HashMap<&'a Family, usize>, Vec<Query<'a>>) {
    let mut families = HashMap::new();
    let mut queries = Vec::new();
    for (function, lowered) in functions.iter().enumerate() {
        let Some(lowered) = lowered.as_ref().filter(|_| acyclic.contains(function)) else {
            continue;
        };
        for (original, obligation) in lowered.presence_obligations.iter().enumerate() {
            if obligation.start == Some(obligation.end) {
                continue;
            }
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
            query.obligation.end,
            query.original,
        )
    });
    (families, queries)
}

fn collect_erases(
    functions: &[Option<LoweredFn>],
    acyclic: &AcyclicCallOrder,
    families: &HashMap<&Family, usize>,
) -> Vec<Erase> {
    let mut erases = Vec::new();
    for (function, lowered) in functions.iter().enumerate() {
        let Some(lowered) = lowered.as_ref().filter(|_| acyclic.contains(function)) else {
            continue;
        };
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

fn propagate(functions: &[Option<LoweredFn>], acyclic: &AcyclicCallOrder, summary: &mut [u64]) {
    for &function in acyclic.callee_before_caller() {
        let mut word = summary[function];
        #[expect(
            clippy::expect_used,
            reason = "the graph order contains only present bodies with eligible callees"
        )]
        let calls = &functions[function]
            .as_ref()
            .expect("eligible functions have bodies")
            .callees;
        for &callee in calls {
            // Eligibility is closed over every direct callee.
            word |= summary[usize::from(callee)];
        }
        summary[function] = word;
    }
}

/// Flat adjacency for the immutable paths. The extra parent slot is the forest's
/// synthetic root; depth zero precedes every recorded call.
struct PathIndex {
    first_child: Vec<Option<usize>>,
    next_sibling: Vec<Option<usize>>,
    depth: Vec<usize>,
}

impl PathIndex {
    fn new(nodes: &[PresenceCallNode]) -> Self {
        let mut index = Self {
            first_child: vec![None; nodes.len() + 1],
            next_sibling: vec![None; nodes.len()],
            depth: vec![0; nodes.len()],
        };
        for (node, path) in nodes.iter().enumerate() {
            debug_assert!(path.parent.is_none_or(|parent| parent < node));
            index.depth[node] = path.parent.map_or(1, |parent| index.depth[parent] + 1);
            let parent = path.parent.unwrap_or(nodes.len());
            index.next_sibling[node] = index.first_child[parent];
            index.first_child[parent] = Some(node);
        }
        index
    }
}

#[derive(Clone, Copy)]
struct Witness {
    depth: usize,
    call: usize,
}

enum Visit {
    Enter(usize),
    Restore(usize),
}

fn settle(
    calls: &[u16],
    nodes: &[PresenceCallNode],
    summary: &[u64],
    queries: &[Query<'_>],
    offset: usize,
    failures: &mut Vec<Failure>,
) {
    let tree = PathIndex::new(nodes);
    let mut first_query = vec![None; nodes.len()];
    for (index, query) in queries.iter().enumerate() {
        first_query[query.obligation.end].get_or_insert(index);
    }
    let mut visits = Vec::new();
    push_children(&tree, nodes.len(), &mut visits);
    let mut last = [None; 64];
    let mut undo = Vec::new();
    while let Some(visit) = visits.pop() {
        let node = match visit {
            Visit::Enter(node) => node,
            Visit::Restore(mark) => {
                for (bit, previous) in undo.drain(mark..).rev() {
                    last[bit] = previous;
                }
                continue;
            }
        };
        let mark = undo.len();
        let mut seen = 0u64;
        for call in nodes[node].calls.clone() {
            let mut hits = summary.get(usize::from(calls[call])).copied().unwrap_or(0) & !seen;
            seen |= hits;
            while hits != 0 {
                let bit = hits.trailing_zeros() as usize;
                hits &= hits - 1;
                undo.push((bit, last[bit]));
                last[bit] = Some(Witness {
                    depth: tree.depth[node],
                    call,
                });
            }
        }
        if let Some(start) = first_query[node] {
            for (index, query) in queries.iter().enumerate().skip(start) {
                if query.obligation.end != node {
                    break;
                }
                let depth = query.obligation.start.map_or(0, |start| tree.depth[start]);
                if let Some(witness) = last[query.family % 64]
                    && witness.depth > depth
                {
                    failures.push(Failure {
                        query: offset + index,
                        call: witness.call,
                    });
                }
            }
        }
        visits.push(Visit::Restore(mark));
        push_children(&tree, node, &mut visits);
    }
}

fn push_children(tree: &PathIndex, parent: usize, visits: &mut Vec<Visit>) {
    let mut child = tree.first_child[parent];
    while let Some(node) = child {
        visits.push(Visit::Enter(node));
        child = tree.next_sibling[node];
    }
}

fn report(
    functions: &[Option<LoweredFn>],
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
        #[expect(
            clippy::expect_used,
            reason = "collect_queries retains only bodies in the closed eligible component"
        )]
        let function = functions[query.function]
            .as_ref()
            .expect("queries retain eligible functions");
        let callee = function.callees[failure.call];
        #[expect(
            clippy::expect_used,
            reason = "eligibility is closed over every direct callee of the query function"
        )]
        let name = functions[usize::from(callee)]
            .as_ref()
            .expect("eligible functions have bodies")
            .name
            .as_str();
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
