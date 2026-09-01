//! One linear analysis of the direct-call graph and linear propagation over an
//! acyclic result.
//!
//! Recursion membership, the requires-ambient-transaction closure, and the
//! mutate/durable closures all consume the same direct-call relation. The graph is
//! analyzed once with iterative Tarjan: each function is discovered once and each
//! edge is read once. A successful analysis mints an [`AcyclicCallOrder`] whose
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
    /// Vertices in reverse topological order when the graph is acyclic: every callee
    /// precedes every caller. Cyclic SCC members are also present, but no propagation
    /// witness can be minted while any such component exists.
    reverse_topological: Vec<usize>,
    /// Whether each function participates in a multi-function SCC or calls itself.
    on_cycle: Vec<bool>,
    has_cycle: bool,
}

impl CallGraphAnalysis {
    /// Whether the function at `index` can reach itself through direct calls.
    pub(crate) fn on_cycle(&self, index: u16) -> bool {
        self.on_cycle
            .get(usize::from(index))
            .copied()
            .unwrap_or(false)
    }

    /// Consume a cycle-free analysis into the only owner that may propagate over its
    /// order. Removing edges from a DAG leaves the order valid for every subrelation.
    pub(crate) fn into_acyclic_order(self) -> Option<AcyclicCallOrder> {
        (!self.has_cycle).then_some(AcyclicCallOrder {
            reverse_topological: self.reverse_topological,
        })
    }

    #[cfg(test)]
    fn has_cycle(&self) -> bool {
        self.has_cycle
    }
}

/// A direct-call graph proven acyclic, in callee-before-caller order.
pub(crate) struct AcyclicCallOrder {
    reverse_topological: Vec<usize>,
}

impl AcyclicCallOrder {
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
        let mut value = vec![false; self.reverse_topological.len()];
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
            if let Some(slot) = value.get_mut(function) {
                *slot = settled;
            }
        }
        value
    }
}

/// Analyze `callees`, whose slice position is the function domain.
///
/// Iterative Tarjan discovers every vertex once and reads every adjacency edge once.
/// A self-edge is recorded during that sole read; cycle classification performs no
/// hidden second adjacency scan. Out-of-domain callees are ignored here because their
/// reference validity belongs to a different check.
pub(crate) fn analyze(callees: &[&[u16]]) -> CallGraphAnalysis {
    const UNVISITED: usize = usize::MAX;

    let count = callees.len();
    let mut index_of = vec![UNVISITED; count];
    let mut lowlink = vec![0usize; count];
    let mut on_stack = vec![false; count];
    let mut component_stack = Vec::with_capacity(count);
    let mut reverse_topological = Vec::with_capacity(count);
    let mut on_cycle = vec![false; count];
    let mut has_cycle = false;
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
            let edges = callees.get(vertex).copied().unwrap_or(&[]);
            if *cursor < edges.len() {
                let callee = usize::from(edges[*cursor]);
                *cursor += 1;
                #[cfg(test)]
                bump_call_graph(|counts| counts.graph_edge_visits += 1);

                if callee == vertex {
                    has_cycle = true;
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
                    has_cycle = true;
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

    CallGraphAnalysis {
        reverse_topological,
        on_cycle,
        has_cycle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acyclic_order_puts_every_callee_before_its_callers() {
        // 0 -> 1 -> 2, plus 3 -> 1.
        let edges: Vec<&[u16]> = vec![&[1], &[2], &[], &[1]];
        let order = analyze(&edges)
            .into_acyclic_order()
            .expect("the graph is acyclic");
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
        let edges: Vec<&[u16]> = vec![&[0], &[]];
        let analysis = analyze(&edges);
        assert!(analysis.on_cycle(0));
        assert!(!analysis.on_cycle(1));
        assert!(analysis.has_cycle());
    }

    #[test]
    fn disjoint_cycles_are_each_found_whole() {
        // 0 <-> 1, 2 alone, 3 -> 4 -> 5 -> 3.
        let edges: Vec<&[u16]> = vec![&[1], &[0], &[], &[4], &[5], &[3]];
        let analysis = analyze(&edges);
        let flags: Vec<bool> = (0..6).map(|index| analysis.on_cycle(index)).collect();
        assert_eq!(flags, vec![true, true, false, true, true, true]);
    }

    #[test]
    fn empty_and_dangling_graphs_are_cycle_free() {
        assert!(!analyze(&[]).has_cycle());
        let edges: Vec<&[u16]> = vec![&[9]];
        assert!(!analyze(&edges).has_cycle());
    }

    #[test]
    fn a_monotone_property_propagates_the_whole_depth_in_one_walk() {
        let edges: Vec<Vec<u16>> = vec![vec![1], vec![2], vec![3], vec![]];
        let slices: Vec<&[u16]> = edges.iter().map(Vec::as_slice).collect();
        let order = analyze(&slices)
            .into_acyclic_order()
            .expect("the graph is acyclic");
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
    fn cyclic_analysis_cannot_mint_a_propagation_order() {
        let edges: Vec<&[u16]> = vec![&[1], &[0]];
        assert!(analyze(&edges).into_acyclic_order().is_none());
    }
}
