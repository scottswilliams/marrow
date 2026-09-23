use super::*;
use crate::lower::BodyRole;
use marrow_codes::Code;
use marrow_image::{
    AdmittedGraphInputPlan, FunctionDef, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes,
    RecordTypeDef, RootOccurrenceDef, Scalar,
};
use marrow_syntax::SourceSpan;

#[cfg(test)]
fn family() -> Family {
    let mut owner = ImageDraft::new();
    let mut draft = crate::compile::admitted(&mut owner);
    let plan = AdmittedGraphInputPlan::admit(1, 1, 0);
    let name = draft.intern_string("R").expect("small name");
    let record = draft
        .add_record_type(RecordTypeDef {
            name,
            fields: Vec::new(),
        })
        .expect("empty record");
    let product = LedgerIdBytes::from_bytes([1; 16]);
    draft
        .declare_product(&plan, product, record, Vec::new())
        .expect("empty product");
    let root = draft
        .add_root_occurrence(
            &plan,
            product,
            RootOccurrenceDef {
                name,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes([2; 16]),
                }],
                placement: LedgerIdBytes::from_bytes([3; 16]),
                indexes: Vec::new().into(),
            },
        )
        .expect("one keyed root");
    Family {
        root: root.root_id(),
        branch: Vec::new(),
    }
}

fn obligations(paths: &[(Option<usize>, usize)]) -> Vec<PresenceObligation> {
    let family = family();
    paths
        .iter()
        .enumerate()
        .map(|(index, &(start, end))| PresenceObligation {
            family: family.clone(),
            span: SourceSpan {
                start_byte: index,
                end_byte: index + 1,
                line: 1,
                column: 1,
            },
            start,
            end,
        })
        .collect()
}

fn queries(obligations: &[PresenceObligation]) -> Vec<Query<'_>> {
    let mut queries: Vec<_> = obligations
        .iter()
        .enumerate()
        .map(|(original, obligation)| Query {
            function: 0,
            family: 0,
            original,
            obligation,
        })
        .collect();
    queries.sort_by_key(|query| query.obligation.end);
    queries
}

fn chain(count: usize) -> Vec<PresenceCallNode> {
    (0..count)
        .map(|call| PresenceCallNode {
            parent: call.checked_sub(1),
            calls: call..call + 1,
        })
        .collect()
}

#[test]
fn forks_exclude_skipped_arms_and_joins_restore_their_effects() {
    let nodes = [
        PresenceCallNode {
            parent: None,
            calls: 0..1,
        },
        PresenceCallNode {
            parent: Some(0),
            calls: 1..2,
        },
        PresenceCallNode {
            parent: Some(0),
            calls: 2..3,
        },
        PresenceCallNode {
            parent: Some(1),
            calls: 3..4,
        },
        PresenceCallNode {
            parent: Some(2),
            calls: 1..4,
        },
        PresenceCallNode {
            parent: None,
            calls: 4..5,
        },
    ];
    let obligations = obligations(&[(None, 2), (Some(1), 3), (None, 3), (Some(0), 4), (None, 5)]);
    let queries = queries(&obligations);
    let mut failures = Vec::new();
    settle(
        &[0, 1, 0, 0, 0],
        &nodes,
        &[0, 1],
        &queries,
        0,
        &mut failures,
    );
    failures.sort_by_key(|failure| failure.query);
    assert_eq!(
        failures
            .iter()
            .map(|f| (f.query, f.call))
            .collect::<Vec<_>>(),
        [(2, 1), (3, 1)]
    );
}

#[test]
fn an_unavailable_callee_does_not_hide_a_later_actual_eraser() {
    let obligations = obligations(&[(None, 1)]);
    let queries = queries(&obligations);
    let mut failures = Vec::new();
    settle(&[u16::MAX, 0], &chain(2), &[1], &queries, 7, &mut failures);
    assert_eq!(
        failures
            .iter()
            .map(|f| (f.query, f.call))
            .collect::<Vec<_>>(),
        [(7, 1)]
    );
}

/// The oracle walks each requested ancestor path directly. Production must answer
/// the same queries while sharing the path walk across all uses of an endpoint.
fn brute_force(
    nodes: &[PresenceCallNode],
    calls: &[u16],
    summary: &[u64],
    query: &Query<'_>,
) -> Option<usize> {
    let mut current = Some(query.obligation.end);
    while current != query.obligation.start {
        let node = current.expect("the start is an ancestor");
        for call in nodes[node].calls.clone() {
            if summary[usize::from(calls[call])] & (1 << (query.family % 64)) != 0 {
                return Some(call);
            }
        }
        current = nodes[node].parent;
    }
    None
}

#[test]
fn shared_path_queries_match_ancestor_walks_across_branches_and_union_nodes() {
    let calls: Vec<u16> = (0..24).map(|index| index % 4).collect();
    let summary = [0, 1, 1 << 63, 1 | (1 << 63)];
    for seed in 0..16usize {
        let nodes: Vec<_> = (0..24)
            .map(|node| PresenceCallNode {
                parent: (node != 0).then(|| (node + seed) % node),
                calls: if node % 5 == 0 {
                    0..node + 1
                } else {
                    node..node + 1
                },
            })
            .collect();
        let mut paths = Vec::new();
        for end in 0..nodes.len() {
            paths.push((None, end));
            let mut ancestor = Some(end);
            while let Some(start) = ancestor {
                paths.push((Some(start), end));
                ancestor = nodes[start].parent;
            }
        }
        let obligations = obligations(&paths);
        for family in [0, 63, 64, 127] {
            let mut queries = queries(&obligations);
            for query in &mut queries {
                query.family = family;
            }
            let expected: Vec<_> = queries
                .iter()
                .enumerate()
                .filter_map(|(index, query)| {
                    brute_force(&nodes, &calls, &summary, query).map(|call| (index, call))
                })
                .collect();
            let mut failures = Vec::new();
            settle(&calls, &nodes, &summary, &queries, 0, &mut failures);
            failures.sort_by_key(|failure| failure.query);
            assert_eq!(
                failures
                    .iter()
                    .map(|f| (f.query, f.call))
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }
}

#[test]
fn many_uses_share_one_path_and_each_has_one_failure_witness() {
    let nodes = chain(256);
    let obligations = obligations(&vec![(Some(0), 255); 2048]);
    let queries = queries(&obligations);
    let mut failures = Vec::new();
    settle(&vec![0; 256], &nodes, &[1], &queries, 0, &mut failures);
    assert_eq!(failures.len(), queries.len());
    assert!(failures.iter().all(|failure| failure.call == 255));
}

#[test]
fn many_uses_of_a_forked_history_keep_one_query_each() {
    const FORKS: usize = 128;
    const USES: usize = 2048;
    let mut nodes = chain(1);
    let mut calls = vec![0];
    let mut head = 0;
    for _ in 0..FORKS {
        let arm = calls.len();
        calls.push(1);
        nodes.push(PresenceCallNode {
            parent: Some(head),
            calls: arm..arm + 1,
        });
        let continuing = calls.len();
        calls.push(0);
        nodes.push(PresenceCallNode {
            parent: Some(head),
            calls: continuing..continuing + 1,
        });
        head = nodes.len() - 1;
    }
    let obligations = obligations(&vec![(Some(0), head); USES]);
    let queries = queries(&obligations);
    assert_eq!(nodes.len(), 1 + 2 * FORKS);
    assert_eq!(queries.len(), USES);
    let mut failures = Vec::new();
    settle(&calls, &nodes, &[0, 1], &queries, 0, &mut failures);
    assert!(
        failures.is_empty(),
        "erasers on skipped arms cannot leak into uses"
    );
}

fn functions() -> Vec<Option<LoweredFn>> {
    named_functions(&["first", "second"])
}

fn named_functions(names: &[&str]) -> Vec<Option<LoweredFn>> {
    let mut owner = ImageDraft::new();
    let mut draft = crate::compile::admitted(&mut owner);
    let source = draft.intern_string("src/main.mw").expect("source name");
    names
        .iter()
        .copied()
        .map(|name| {
            let function_name = draft.intern_string(name).expect("function name");
            let func = draft
                .add_function(FunctionDef {
                    name: function_name,
                    source,
                    params: Vec::new(),
                    ret: ImageType::Unit,
                    local_count: 0,
                    code: vec![Instr::Return],
                    spans: Vec::new(),
                })
                .expect("a body without sites");
            Some(LoweredFn {
                func,
                file: crate::test_file("src/main.mw").clone(),
                name: name.to_string(),
                span: SourceSpan::default(),
                callees: vec![0, 1],
                role: BodyRole::Helper,
                unwrapped_mutations: Vec::new(),
                unwrapped_calls: Vec::new(),
                erased_families: Vec::new(),
                presence_calls: Vec::new(),
                presence_obligations: Vec::new(),
                has_direct_durable_op: false,
                code_spans: Vec::new(),
            })
        })
        .collect()
}

#[test]
fn reporting_coalesces_only_the_exact_use_and_selects_its_earliest_call() {
    let mut obligations = obligations(&[(None, 1), (Some(0), 1), (None, 1), (None, 1), (None, 1)]);
    // The first two are the ordinary/loop paths for one concrete source use.
    // The other three differ by family, concrete function, and full source span.
    let span = obligations[0].span;
    for obligation in &mut obligations[1..4] {
        obligation.span = span;
    }
    obligations[2].family.branch.push(0);
    let mut queries = queries(&obligations);
    queries[2].family = 1;
    queries[3].function = 1;
    let failures = vec![
        Failure { query: 3, call: 1 },
        Failure { query: 1, call: 1 },
        Failure { query: 2, call: 1 },
        Failure { query: 4, call: 1 },
        Failure { query: 0, call: 0 },
    ];
    let functions = functions();
    let mut collector = DiagnosticCollector::new();
    report(&functions, &queries, failures, &mut collector);
    let rows = collector
        .finish()
        .into_complete()
        .expect("a complete terminal");
    assert_eq!(rows.len(), 4);
    assert!(
        rows.iter()
            .all(|row| row.code() == Code::CheckRequiresPresence)
    );
    assert!(
        rows[0].message().contains("`first`"),
        "earliest actual call survives coalescing"
    );
    assert!(
        rows[1..]
            .iter()
            .all(|row| row.message().contains("`second`"))
    );
    assert_eq!(
        rows.iter().map(|row| row.span()).collect::<Vec<_>>(),
        [span, span, obligations[4].span, span]
    );
}

#[test]
fn sparse_presence_reports_only_callee_closed_available_functions() {
    let mut functions = named_functions(&["missing", "eraser", "eligible", "excluded", "leaf"]);
    functions[0] = None;
    for function in functions.iter_mut().flatten() {
        function.callees.clear();
    }
    let family = family();
    functions[1]
        .as_mut()
        .expect("valid fixture construction")
        .erased_families
        .push(family.clone());
    let use_span = SourceSpan {
        start_byte: 20,
        end_byte: 21,
        line: 3,
        column: 4,
    };
    for (index, callees) in [(2, vec![1]), (3, vec![0, 1])] {
        let function = functions[index]
            .as_mut()
            .expect("the fixture populated this slot");
        function.presence_obligations.push(PresenceObligation {
            family: family.clone(),
            span: SourceSpan {
                start_byte: use_span.start_byte + index - 2,
                ..use_span
            },
            start: None,
            end: callees.len() - 1,
        });
        function.presence_calls = chain(callees.len());
        function.callees = callees;
    }
    let lowered = LoweredFunctionSet(functions);
    let mut diagnostics = DiagnosticCollector::new();
    let acyclic = crate::compile::reject_recursion(&lowered, &mut diagnostics);
    assert_eq!(
        (0..5).map(|id| acyclic.contains(id)).collect::<Vec<_>>(),
        vec![false, true, true, false, true]
    );
    let erased = HashSet::from([&family]);
    let (_, queries) = collect_queries(lowered.functions(), &acyclic, &erased);
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].function, 2);
    assert_eq!(
        lowered.functions()[queries[0].function]
            .as_ref()
            .expect("valid fixture construction")
            .callees,
        vec![1]
    );
    reject_unproven_uses(&lowered, &acyclic, &mut diagnostics);
    let rows = diagnostics
        .finish()
        .into_complete()
        .expect("a complete terminal");
    assert_eq!(
        rows.len(),
        1,
        "the caller of the missing body must not be reported"
    );
    assert_eq!(rows[0].code(), Code::CheckRequiresPresence);
    assert_eq!(rows[0].file(), crate::test_file("src/main.mw").identity());
    assert_eq!(rows[0].span(), use_span);
    assert!(
        rows[0].message().contains("`eraser`"),
        "the witness names the actual sparse callee"
    );
}
