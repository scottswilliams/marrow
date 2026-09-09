//! One linear analysis of the direct-call graph and linear propagation over an
//! acyclic result.
//!
//! Recursion membership, the requires-ambient-transaction closure, and the
//! mutate/durable closures all consume the same direct-call relation. The graph is
//! analyzed once with iterative Tarjan: each function is discovered once and each
//! edge is read once. One further sweep closes body availability over that order.
//! The analysis mints an [`AcyclicCallOrder`] whose
//! reverse-topological order settles any subrelation in one pass, with each relevant
//! function and edge examined once.
//!
//! The traversal is iterative rather than recursive, so call depth does not become
//! native stack depth. SCC members are emitted into one flat vector; an ordinary
//! acyclic program does not allocate one nested vector per function. Cycle reporting
//! remains the caller's decision and order: this owner supplies membership only.

#[cfg(test)]
use crate::types::bump_call_graph;

/// Cycle membership and the flat SCC emission order of one direct-call graph.
pub(crate) struct CallGraphAnalysis {
    /// Available callee-closed vertices in callee-before-caller order.
    reverse_topological: Vec<usize>,
    /// Whether each function participates in a multi-function SCC or calls itself.
    on_cycle: Vec<bool>,
    eligible: Vec<bool>,
}

impl CallGraphAnalysis {
    /// Whether the function at `index` can reach itself through direct calls.
    pub(crate) fn on_cycle(&self, index: u16) -> bool {
        self.on_cycle
            .get(usize::from(index))
            .copied()
            .unwrap_or(false)
    }

    /// Retain the callee-closed, available, acyclic components. A cycle or missing
    /// body elsewhere does not invalidate an independent component's order.
    pub(crate) fn into_acyclic_order(self) -> AcyclicCallOrder {
        AcyclicCallOrder {
            reverse_topological: self.reverse_topological,
            eligible: self.eligible,
        }
    }

    #[cfg(test)]
    fn has_cycle(&self) -> bool {
        self.on_cycle.iter().any(|&on_cycle| on_cycle)
    }
}

/// The callee-closed available subset, proven acyclic. Mask positions retain the
/// full reserved function domain, including unavailable bodies and excluded callers.
pub(crate) struct AcyclicCallOrder {
    reverse_topological: Vec<usize>,
    eligible: Vec<bool>,
}

impl AcyclicCallOrder {
    pub(crate) fn domain_len(&self) -> usize {
        self.eligible.len()
    }

    pub(crate) fn contains(&self, function: usize) -> bool {
        self.eligible.get(function).copied().unwrap_or(false)
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.reverse_topological.len() == self.domain_len()
    }

    /// Eligible functions in callee-before-caller order. Their original indices
    /// remain in the full reserved domain, even when this order omits other slots.
    pub(crate) fn callee_before_caller(&self) -> &[usize] {
        &self.reverse_topological
    }

    /// Settle a monotone boolean property over an acyclic call subrelation.
    ///
    /// `base` decides the direct value at a function. `successors` visits the
    /// function's callees in the relation being propagated. Because the retained
    /// order came from the complete direct-call DAG, every in-domain callee already
    /// has its final value. Each function and each supplied edge is therefore
    /// examined exactly once.
    pub(crate) fn propagate<S: Fn(usize, &mut dyn FnMut(usize))>(
        &self,
        mut base: impl FnMut(usize) -> bool,
        successors: S,
    ) -> Vec<bool> {
        let mut value = vec![false; self.domain_len()];
        for &function in &self.reverse_topological {
            #[cfg(test)]
            bump_call_graph(|counts| counts.propagation_visits += 1);
            let mut settled = base(function);
            successors(function, &mut |callee| {
                #[cfg(test)]
                bump_call_graph(|counts| counts.propagation_edge_visits += 1);
                if value.get(callee).copied().unwrap_or(false) {
                    settled = true;
                }
            });
            value[function] = settled;
        }
        value
    }
}

/// Analyze `callees`, whose slice position is the function domain.
///
/// Iterative Tarjan discovers every vertex once and reads every adjacency edge once.
/// A self-edge is recorded during that sole read; cycle classification performs no
/// hidden second adjacency scan. A second callee-first sweep excludes every missing
/// or cyclic body and its transitive callers; out-of-domain edges fail that closure.
pub(crate) fn analyze(callees: &[Option<&[u16]>]) -> CallGraphAnalysis {
    const UNVISITED: usize = usize::MAX;

    let count = callees.len();
    let mut index_of = vec![UNVISITED; count];
    let mut lowlink = vec![0usize; count];
    let mut on_stack = vec![false; count];
    let mut component_stack = Vec::with_capacity(count);
    let mut reverse_topological = Vec::with_capacity(count);
    let mut on_cycle = vec![false; count];
    let mut next_index = 0usize;

    // `(vertex, next edge)` replaces recursive Tarjan frames.
    let mut frames: Vec<(usize, usize)> = Vec::new();

    for root in 0..count {
        if index_of[root] != UNVISITED {
            continue;
        }

        index_of[root] = next_index;
        lowlink[root] = next_index;
        next_index += 1;
        component_stack.push(root);
        on_stack[root] = true;
        frames.push((root, 0));
        #[cfg(test)]
        bump_call_graph(|counts| counts.graph_vertex_visits += 1);

        while let Some(&mut (vertex, ref mut cursor)) = frames.last_mut() {
            let edges = callees[vertex].unwrap_or(&[]);
            if *cursor < edges.len() {
                let callee = usize::from(edges[*cursor]);
                *cursor += 1;
                #[cfg(test)]
                bump_call_graph(|counts| counts.graph_edge_visits += 1);

                if callee == vertex {
                    on_cycle[vertex] = true;
                }
                if callee >= count {
                    continue;
                }
                if index_of[callee] == UNVISITED {
                    index_of[callee] = next_index;
                    lowlink[callee] = next_index;
                    next_index += 1;
                    component_stack.push(callee);
                    on_stack[callee] = true;
                    frames.push((callee, 0));
                    #[cfg(test)]
                    bump_call_graph(|counts| counts.graph_vertex_visits += 1);
                } else if on_stack[callee] {
                    lowlink[vertex] = lowlink[vertex].min(index_of[callee]);
                }
                continue;
            }

            frames.pop();
            if lowlink[vertex] == index_of[vertex] {
                let component_start = reverse_topological.len();
                while let Some(member) = component_stack.pop() {
                    on_stack[member] = false;
                    reverse_topological.push(member);
                    if member == vertex {
                        break;
                    }
                }

                let component = &reverse_topological[component_start..];
                if component.len() > 1 {
                    for &member in component {
                        on_cycle[member] = true;
                    }
                }
            }

            if let Some(&mut (parent, _)) = frames.last_mut() {
                lowlink[parent] = lowlink[parent].min(lowlink[vertex]);
            }
        }
    }

    // Every SCC is classified before this sweep: an early member of a cycle must
    // never look like a settled callee. Tarjan emits callees before their callers,
    // so one pass closes availability transitively without another graph or fixpoint.
    // SCC traversal no longer needs its stack-membership scratch. Reuse that
    // allocation for the full-domain eligibility mask retained by the result.
    let mut eligible = on_stack;
    for ((eligible, body), &cycle) in eligible.iter_mut().zip(callees).zip(&on_cycle) {
        *eligible = body.is_some() && !cycle;
    }
    for &function in &reverse_topological {
        #[cfg(test)]
        bump_call_graph(|counts| counts.closure_vertex_visits += 1);
        if eligible[function] {
            #[expect(
                clippy::expect_used,
                reason = "eligibility is seeded only for present bodies and can only be removed"
            )]
            let closed = callees[function]
                .expect("eligible functions have bodies")
                .iter()
                .all(|&callee| {
                    #[cfg(test)]
                    bump_call_graph(|counts| counts.closure_edge_visits += 1);
                    eligible.get(usize::from(callee)).copied().unwrap_or(false)
                });
            eligible[function] = closed;
        }
    }
    reverse_topological.retain(|&function| eligible[function]);

    #[cfg(test)]
    bump_call_graph(|counts| {
        let bytes = (index_of.capacity()
            + lowlink.capacity()
            + component_stack.capacity()
            + reverse_topological.capacity())
            * size_of::<usize>()
            + (on_cycle.capacity() + eligible.capacity()) * size_of::<bool>()
            + frames.capacity() * size_of::<(usize, usize)>();
        counts.graph_scratch_bytes = counts.graph_scratch_bytes.max(bytes);
    });

    CallGraphAnalysis {
        reverse_topological,
        on_cycle,
        eligible,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acyclic_order_puts_every_callee_before_its_callers() {
        // 0 -> 1 -> 2, plus 3 -> 1.
        let edges: Vec<Option<&[u16]>> = vec![Some(&[1]), Some(&[2]), Some(&[]), Some(&[1])];
        let order = analyze(&edges).into_acyclic_order();
        let position = |target| {
            order
                .reverse_topological
                .iter()
                .position(|&member| member == target)
                .expect("every vertex is retained")
        };
        assert!(position(2) < position(1));
        assert!(position(1) < position(0));
        assert!(position(1) < position(3));
    }

    #[test]
    fn a_self_loop_is_a_cycle_and_a_lone_vertex_is_not() {
        let edges: Vec<Option<&[u16]>> = vec![Some(&[0]), Some(&[])];
        let analysis = analyze(&edges);
        assert!(analysis.on_cycle(0));
        assert!(!analysis.on_cycle(1));
        assert!(analysis.has_cycle());
    }

    #[test]
    fn disjoint_cycles_are_each_found_whole() {
        // 0 <-> 1, 2 alone, 3 -> 4 -> 5 -> 3.
        let edges: Vec<Option<&[u16]>> = vec![
            Some(&[1]),
            Some(&[0]),
            Some(&[]),
            Some(&[4]),
            Some(&[5]),
            Some(&[3]),
        ];
        let analysis = analyze(&edges);
        let flags: Vec<bool> = (0..6).map(|index| analysis.on_cycle(index)).collect();
        assert_eq!(flags, vec![true, true, false, true, true, true]);
    }

    #[test]
    fn empty_and_dangling_graphs_are_cycle_free() {
        assert!(!analyze(&[]).has_cycle());
        let edges: Vec<Option<&[u16]>> = vec![Some(&[9])];
        assert!(!analyze(&edges).has_cycle());
    }

    #[test]
    fn first_middle_and_last_holes_keep_the_reserved_domain() {
        for missing in 0..3 {
            let mut edges: Vec<Option<&[u16]>> = vec![Some(&[]); 3];
            edges[missing] = None;
            let order = analyze(&edges).into_acyclic_order();
            assert_eq!(order.domain_len(), 3);
            assert!(!order.is_complete());
            for index in 0..3 {
                assert_eq!(order.contains(index), index != missing);
            }
            assert!(!order.contains(3));
            let values = order.propagate(|_| true, |_, _| {});
            assert_eq!(
                values,
                (0..3).map(|index| index != missing).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn unavailable_callees_exclude_every_upstream_caller() {
        let cases: &[&[Option<&[u16]>]] = &[
            &[None, Some(&[0]), Some(&[1]), Some(&[])],
            &[Some(&[9]), Some(&[0]), Some(&[1]), Some(&[])],
            &[Some(&[1]), Some(&[0]), Some(&[1]), Some(&[])],
        ];
        for edges in cases {
            let order = analyze(edges).into_acyclic_order();
            assert_eq!(order.callee_before_caller(), &[3]);
            assert_eq!(order.domain_len(), 4);
            let values = order.propagate(
                |index| {
                    assert_eq!(index, 3);
                    true
                },
                |index, _| {
                    assert_eq!(index, 3);
                },
            );
            assert_eq!(values, vec![false, false, false, true]);
        }
        let order = analyze(&[None, Some(&[0]), Some(&[1])]).into_acyclic_order();
        assert!(order.callee_before_caller().is_empty());
        assert_eq!(
            order.propagate(|_| panic!("excluded body"), |_, _| panic!("excluded body")),
            vec![false; 3]
        );
    }

    #[test]
    fn the_highest_reserved_id_propagates_without_compacting_holes() {
        let count = usize::from(u16::MAX) + 1;
        let edge = [u16::MAX];
        let mut edges: Vec<Option<&[u16]>> = vec![None; count];
        edges[0] = Some(&edge);
        edges[count - 1] = Some(&[]);
        let order = analyze(&edges).into_acyclic_order();
        assert_eq!(order.domain_len(), count);
        assert_eq!(order.callee_before_caller(), &[count - 1, 0]);
        assert!(order.contains(count - 1));
        let value = order.propagate(
            |index| index == count - 1,
            |index, visit| {
                assert!(index == 0 || index == count - 1);
                if index == 0 {
                    visit(count - 1);
                }
            },
        );
        assert_eq!(value.len(), count);
        assert!(value[0] && value[count - 1]);
        assert!(value[1..count - 1].iter().all(|value| !value));
    }

    #[test]
    fn a_monotone_property_propagates_the_whole_depth_in_one_walk() {
        let edges: Vec<Vec<u16>> = vec![vec![1], vec![2], vec![3], vec![]];
        let slices: Vec<Option<&[u16]>> =
            edges.iter().map(|edges| Some(edges.as_slice())).collect();
        let order = analyze(&slices).into_acyclic_order();
        let value = order.propagate(
            |function| function == 3,
            |function, visit| {
                for &callee in &edges[function] {
                    visit(usize::from(callee));
                }
            },
        );
        assert_eq!(value, vec![true, true, true, true]);
    }

    #[test]
    fn cyclic_analysis_mints_an_empty_restricted_order() {
        let edges: Vec<Option<&[u16]>> = vec![Some(&[1]), Some(&[0])];
        let order = analyze(&edges).into_acyclic_order();
        assert!(!order.is_complete());
        assert!(order.callee_before_caller().is_empty());
        assert_eq!(order.domain_len(), 2);
    }
}
