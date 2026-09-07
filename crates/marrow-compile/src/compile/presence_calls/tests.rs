use super::*;
use marrow_image::{
    AdmittedGraphInputPlan, FunctionDef, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes,
    RecordTypeDef, RootOccurrenceDef, Scalar,
};
use marrow_syntax::SourceSpan;
use std::ops::Range;

#[cfg(test)]
fn family() -> Family {
    let mut owner = ImageDraft::new();
    let mut draft = crate::compile::admitted(&mut owner);
    let plan = AdmittedGraphInputPlan::admit(1, 1, 0).expect("empty product census");
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

fn obligations(ranges: &[Range<usize>]) -> Vec<PresenceObligation> {
    let family = family();
    ranges
        .iter()
        .enumerate()
        .map(|(index, calls)| PresenceObligation {
            family: family.clone(),
            span: SourceSpan {
                start_byte: index,
                end_byte: index + 1,
                line: 1,
                column: 1,
            },
            calls: calls.clone(),
        })
        .collect()
}

fn queries(obligations: &[PresenceObligation]) -> Vec<Query<'_>> {
    obligations
        .iter()
        .enumerate()
        .map(|(original, obligation)| Query {
            function: 0,
            family: 0,
            original,
            obligation,
        })
        .collect()
}

#[test]
fn a_drain_distinguishes_expired_live_empty_and_future_intervals() {
    let obligations = obligations(&[0..1, 1..1, 1..2, 2..4]);
    let queries = queries(&obligations);
    let mut next = Vec::new();
    for _ in 0..2 {
        let mut failures = Vec::new();
        let (_, counts) = crate::types::capture_call_graph_counts(|| {
            settle(
                &[0, 1, 0, 1],
                &[0, 1],
                &queries,
                0,
                &mut next,
                &mut failures,
            );
        });
        assert_eq!(
            failures
                .iter()
                .map(|f| (f.query, f.call))
                .collect::<Vec<_>>(),
            [(2, 1), (3, 3)]
        );
        assert_eq!(counts.presence_query_positions, 4);
        assert_eq!(counts.presence_summary_lookups, 4);
        assert_eq!(counts.presence_queries_queued, 3);
        assert_eq!(counts.presence_queries_drained, 3);
        assert_eq!(counts.presence_next_slots, 4);
        assert_eq!(counts.presence_failure_rows, 2);
    }
}

#[test]
fn an_unavailable_callee_does_not_discard_a_pending_query() {
    let obligations = obligations(&[0..2]);
    let queries = queries(&obligations);
    let mut failures = Vec::new();
    settle(
        &[u16::MAX, 0],
        &[1],
        &queries,
        7,
        &mut Vec::new(),
        &mut failures,
    );
    assert_eq!(
        failures
            .iter()
            .map(|f| (f.query, f.call))
            .collect::<Vec<_>>(),
        [(7, 1)]
    );
}

#[test]
fn pending_links_are_reusable_for_a_different_family_and_shorter_group() {
    let obligations = obligations(&[0..2, 0..2, 0..2]);
    let mut queries = queries(&obligations);
    let mut next = Vec::new();
    let mut failures = Vec::new();
    settle(&[0, 0], &[1], &queries, 0, &mut next, &mut failures);
    assert_eq!(failures.len(), 3);
    failures.clear();
    queries[0].family = 63;
    settle(
        &[0, 1],
        &[1, 1 << 63],
        &queries[..1],
        5,
        &mut next,
        &mut failures,
    );
    assert_eq!(
        failures
            .iter()
            .map(|f| (f.query, f.call))
            .collect::<Vec<_>>(),
        [(5, 1)]
    );
}

fn functions() -> Vec<LoweredFn> {
    let mut owner = ImageDraft::new();
    let mut draft = crate::compile::admitted(&mut owner);
    let source = draft.intern_string("src/main.mw").expect("source name");
    ["first", "second"]
        .into_iter()
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
            LoweredFn {
                func,
                file: crate::test_main_file_identity().clone(),
                name: name.to_string(),
                span: SourceSpan::default(),
                callees: vec![0, 1],
                is_export: false,
                is_test: false,
                unwrapped_mutations: Vec::new(),
                unwrapped_calls: Vec::new(),
                erased_families: Vec::new(),
                presence_obligations: Vec::new(),
                has_direct_durable_op: false,
                owns_transaction: false,
                code_spans: Vec::new(),
            }
        })
        .collect()
}

#[test]
fn reporting_coalesces_only_the_exact_use_and_selects_its_earliest_call() {
    let mut obligations = obligations(&[0..2, 1..2, 0..2, 0..2, 0..2]);
    // The first two are the ordinary/loop intervals for one concrete source use.
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
    let rows = collector.finish().expect_complete();
    assert_eq!(rows.len(), 4);
    assert!(
        rows.iter()
            .all(|row| row.code() == "check.requires_presence")
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
