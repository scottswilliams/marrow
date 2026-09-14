use super::*;

fn same_limit(actual: &AnalysisResourceLimit, expected: &AnalysisResourceLimit) {
    use AnalysisResourceLimit::*;
    match (actual, expected) {
        (CompletionCandidateCount { limit: a }, CompletionCandidateCount { limit: b })
        | (CompletionRenderBytes { limit: a }, CompletionRenderBytes { limit: b })
        | (ActiveCallRenderBytes { limit: a }, ActiveCallRenderBytes { limit: b }) => {
            assert_eq!(a, b)
        }
        _ => panic!("query refusal kind differs"),
    }
}

fn same_completions(actual: &CompletionOutcome, expected: &CompletionOutcome) {
    match (actual, expected) {
        (
            CompletionOutcome::Ready(Fact::Present(a)),
            CompletionOutcome::Ready(Fact::Present(b)),
        ) => {
            assert_eq!(a.class, b.class);
            assert_eq!(a.candidates.len(), b.candidates.len());
            for (a, b) in a.candidates.iter().zip(&b.candidates) {
                assert_eq!(a.label, b.label);
                assert_eq!(a.kind, b.kind);
                assert_eq!(a.detail, b.detail);
            }
        }
        (CompletionOutcome::Ready(Fact::Absent), CompletionOutcome::Ready(Fact::Absent)) => {}
        (
            CompletionOutcome::Ready(Fact::Unavailable(a)),
            CompletionOutcome::Ready(Fact::Unavailable(b)),
        ) => assert_eq!(std::mem::discriminant(a), std::mem::discriminant(b)),
        (CompletionOutcome::Refused(a), CompletionOutcome::Refused(b)) => same_limit(a, b),
        _ => panic!("completion outcome differs"),
    }
}

fn same_call(actual: &ActiveCallOutcome, expected: &ActiveCallOutcome) {
    match (actual, expected) {
        (
            ActiveCallOutcome::Ready(Fact::Present(a)),
            ActiveCallOutcome::Ready(Fact::Present(b)),
        ) => {
            assert_eq!(a.signature, b.signature);
            assert_eq!(a.active, b.active);
            assert_eq!(
                a.params.iter().map(|p| &p.label).collect::<Vec<_>>(),
                b.params.iter().map(|p| &p.label).collect::<Vec<_>>()
            );
        }
        (ActiveCallOutcome::Ready(Fact::Absent), ActiveCallOutcome::Ready(Fact::Absent)) => {}
        (
            ActiveCallOutcome::Ready(Fact::Unavailable(a)),
            ActiveCallOutcome::Ready(Fact::Unavailable(b)),
        ) => assert_eq!(std::mem::discriminant(a), std::mem::discriminant(b)),
        (ActiveCallOutcome::Refused(a), ActiveCallOutcome::Refused(b)) => same_limit(a, b),
        _ => panic!("active-call outcome differs"),
    }
}

fn check_at(
    source: &str,
    full: &marrow_syntax::SourceFile,
    offset: usize,
) -> (CompletionOutcome, ActiveCallOutcome) {
    let query = query_local_parse(source.as_bytes(), offset).expect("UTF-8 fixture");
    let view = QueryFile::new(&query);
    let oracle = QueryFile {
        declarations: &full.declarations,
        uses: &full.uses,
        offset: offset as u32,
    };
    let completion = completion::resolve(&view);
    same_completions(&completion, &completion::resolve(&oracle));
    let call = active_call::resolve(&view, source.as_bytes());
    same_call(&call, &active_call::resolve(&oracle, source.as_bytes()));
    (completion, call)
}

#[test]
fn position_queries_match_full_syntax_at_every_corpus_offset() {
    let sources = [
        "module main\nuse other\nstruct Point {\n x: int\n}\nenum Colour {\n red\n green\n}\nfn earlier<T>(p: Point, t: T): int {\n var q: Point = p\n q.x\n return later(q.x)\n}\nfn later(n: int): int {\n return n\n}\ntest \"calls\" {\n later(1)\n}\n",
        "const c: int = later(1)\nfn first(): int {\n return later(\n}\nfn later(n: int): int {\n return n\n}\n",
        "fn bad() {\n var\n @\n}\nfn selected(p: Missing) {\n p.\n E::\n}\nenum E {\n a\n b\n}\n",
        "fn unclosed() {\n call(\nfn swallowed(x: int) {\n x\n}\n",
        "fn first() { a }fn second() { b }\n",
        "// é\nfn text(p: string) {\n p\n}\n",
    ];
    for source in sources {
        let full = marrow_syntax::parse_source(source).file;
        for offset in 0..=source.len() {
            check_at(source, &full, offset);
        }
    }
}

#[test]
fn discarded_body_diagnostics_do_not_change_later_queries() {
    let source = format!(
        "fn bad() {{\n{}}}\nfn selected(n: int) {{\n later(n)\n}}\nfn later(n: int): int {{\n return n\n}}\n",
        "var\n".repeat(marrow_syntax::SYNTAX_DIAGNOSTIC_COUNT_LIMIT + 1)
    );
    let full = marrow_syntax::parse_source(&source);
    assert!(
        full.diagnostics.as_complete().is_err(),
        "fixture crosses diagnostic retention"
    );
    let offset = source.find("later(n)").expect("selected call") + "later(".len();
    let (_, call) = check_at(&source, &full.file, offset);
    assert!(matches!(call, ActiveCallOutcome::Ready(Fact::Present(_))));
}

#[test]
fn query_refusals_match_full_syntax() {
    let mut source = String::from("fn selected() {\n name\n}\n");
    for i in 0..MAX_COMPLETION_CANDIDATES {
        source.push_str(&format!("fn f{i}() {{\n 1\n}}\n"));
    }
    let full = marrow_syntax::parse_source(&source).file;
    let (outcome, _) = check_at(&source, &full, source.find("name").expect("selected name"));
    assert!(matches!(
        outcome,
        CompletionOutcome::Refused(AnalysisResourceLimit::CompletionCandidateCount { .. })
    ));

    let ty = "T".repeat(MAX_COMPLETION_RENDER_BYTES as usize);
    let source = format!("fn selected(n: {ty}) {{\n n\n}}\n");
    let full = marrow_syntax::parse_source(&source).file;
    let (outcome, _) = check_at(
        &source,
        &full,
        source.find("\n n").expect("selected name") + 2,
    );
    assert!(matches!(
        outcome,
        CompletionOutcome::Refused(AnalysisResourceLimit::CompletionRenderBytes { .. })
    ));

    let ty = "T".repeat(MAX_ACTIVE_CALL_RENDER_BYTES as usize);
    let source = format!("fn selected() {{\n target(1)\n}}\nfn target(n: {ty}) {{\n n\n}}\n");
    let full = marrow_syntax::parse_source(&source).file;
    let (_, outcome) = check_at(
        &source,
        &full,
        source.find("target(1)").expect("selected call") + "target(".len(),
    );
    assert!(matches!(
        outcome,
        ActiveCallOutcome::Refused(AnalysisResourceLimit::ActiveCallRenderBytes { .. })
    ));
}
