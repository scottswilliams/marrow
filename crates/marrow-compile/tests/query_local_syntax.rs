//! The query-local syntax policy: `completions` and `active_call` re-parse exactly one
//! already-admitted file's already-retained bytes per query.
//!
//! Two properties are pinned here.
//!
//! **Outcome agreement.** Parsing is a pure function of the source bytes, so an outcome
//! derived from a per-query parse equals the one a retained tree produced. This file
//! freezes a corpus of rendered outcomes covering the classification cases the retained
//! trees served — clean files, a recovered-broken file, a file that never decoded — so a
//! divergence is a failing assertion rather than an assumption.
//!
//! **Latency.** The trade this policy accepts is a lex-and-parse per query in exchange
//! for retaining no tree. The budget is pinned at the maximum file the project owner
//! admits, because that is the worst case an editor can reach.
//!
//! Parseability itself is never inferred from a query-local parse: `broken_files` stays
//! the snapshot's independent record, which is why a recovered-broken file still
//! classifies positions while its hover facts stay syntax-unavailable.

use std::sync::Arc;
use std::time::Instant;

use marrow_compile::{
    ActiveCallOutcome, AnalysisSnapshot, CompletionOutcome, Fact, InputRevision, PositionClass,
    QueryError, Unavailability, analyze,
};
use marrow_project::{CaptureLimits, CapturedFile, FileIdentity, Manifest, ProjectInput};

/// The largest file the project owner admits — the worst case a query-local parse can
/// be handed, since drive admission refuses anything larger before a snapshot exists.
const MAX_ADMITTED_FILE_BYTES: usize = 1 << 20;

/// The editor-latency budget for one query of a maximum admitted file, in the optimized
/// profile the language server ships in.
///
/// This is the cost the retention bound buys: the snapshot retains no tree, so a query
/// lexes and parses the one file it names, then classifies the position over the result.
/// The budget covers the whole query, not the parse alone. It is set above the measured
/// worst case with room for ordinary machine variation, and below the threshold at which
/// a response stops reading as immediate.
///
/// A parse cache is not the remedy if this fails: that would be a second retention owner
/// and would reopen the bound this policy closes.
const QUERY_BUDGET_MS: u128 = 100;

/// The same budget for a file of ordinary size, which is what an editor session actually
/// spends nearly all of its queries on.
const ORDINARY_FILE_BYTES: usize = 64 * 1024;
const ORDINARY_QUERY_BUDGET_MS: u128 = 10;

fn captured(files: Vec<(&str, Vec<u8>)>) -> Arc<ProjectInput> {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .into_iter()
        .map(|(path, bytes)| CapturedFile::new(path.to_string(), bytes))
        .collect();
    Arc::new(
        marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
            .expect("the fixture is inside the production admission envelope"),
    )
}

fn snapshot(files: Vec<(&str, Vec<u8>)>) -> Arc<AnalysisSnapshot> {
    match analyze(captured(files), InputRevision::new(1)) {
        Ok(snapshot) => snapshot,
        Err(failure) => panic!(
            "the fixture analyzes; failed at revision {}",
            failure.revision().get()
        ),
    }
}

fn identity(path: &str) -> FileIdentity {
    FileIdentity::validate(path)
        .expect("the fixture path is a canonical identity")
        .0
}

/// A stable rendering of one completion outcome, so an agreement assertion compares
/// exact classifications and candidate sets rather than a summary.
fn render_completions(outcome: Result<CompletionOutcome, QueryError>) -> String {
    match outcome {
        Err(QueryError::UnknownFile) => "error:unknown-file".to_string(),
        Err(QueryError::OffsetOutOfRange) => "error:offset".to_string(),
        Ok(CompletionOutcome::Refused(limit)) => format!("refused:{}", limit.description()),
        Ok(CompletionOutcome::Ready(Fact::Absent)) => "absent".to_string(),
        Ok(CompletionOutcome::Ready(Fact::Unavailable(Unavailability::Syntax))) => {
            "unavailable:syntax".to_string()
        }
        Ok(CompletionOutcome::Ready(Fact::Unavailable(Unavailability::Dependency))) => {
            "unavailable:dependency".to_string()
        }
        Ok(CompletionOutcome::Ready(Fact::Present(completions))) => {
            let class = match completions.class() {
                PositionClass::ExpressionName => "expr",
                PositionClass::Member => "member",
                PositionClass::EnumPath => "enum",
                PositionClass::TypeAnnotation => "type",
            };
            let labels: Vec<String> = completions
                .candidates()
                .iter()
                .map(|candidate| format!("{}:{}", candidate.label(), candidate.detail()))
                .collect();
            format!("{class}[{}]", labels.join(","))
        }
    }
}

fn render_active_call(outcome: Result<ActiveCallOutcome, QueryError>) -> String {
    match outcome {
        Err(QueryError::UnknownFile) => "error:unknown-file".to_string(),
        Err(QueryError::OffsetOutOfRange) => "error:offset".to_string(),
        Ok(ActiveCallOutcome::Refused(limit)) => format!("refused:{}", limit.description()),
        Ok(ActiveCallOutcome::Ready(Fact::Absent)) => "absent".to_string(),
        Ok(ActiveCallOutcome::Ready(Fact::Unavailable(Unavailability::Syntax))) => {
            "unavailable:syntax".to_string()
        }
        Ok(ActiveCallOutcome::Ready(Fact::Unavailable(Unavailability::Dependency))) => {
            "unavailable:dependency".to_string()
        }
        Ok(ActiveCallOutcome::Ready(Fact::Present(call))) => {
            let params: Vec<&str> = call.params().iter().map(|piece| piece.label()).collect();
            format!(
                "{}|{}|{:?}",
                call.signature(),
                params.join(","),
                call.active()
            )
        }
    }
}

const CLEAN: &str = "module main\n\n\
     struct Point {\n    x: int\n    y: int\n}\n\n\
     enum Colour {\n    red\n    green\n}\n\n\
     fn add(left: int, right: int): int {\n    return left + right\n}\n\n\
     pub fn run(): int {\n    var p: Point = Point(x: 1, y: 2)\n    return add(p.x, p.y)\n}\n";

/// A file with a real parse error whose recovered incomplete forms still classify.
const BROKEN: &str = "module broken\n\n\
     struct Item {\n    name: string\n}\n\n\
     pub fn go(): int {\n    var i: Item = Item(name: \"a\")\n    i.\n    return 0\n}\n";

fn corpus_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("src/main.mw", CLEAN.as_bytes().to_vec()),
        ("src/broken.mw", BROKEN.as_bytes().to_vec()),
        ("src/undecodable.mw", b"module bad\n\xFF\xFE".to_vec()),
    ]
}

/// Frozen `(file, offset)` probes with the exact outcome each must produce. The offsets
/// are derived from the source text so an edit to a fixture cannot silently move a probe
/// off its intended position.
fn probes() -> Vec<(&'static str, usize, &'static str)> {
    let after_dot = CLEAN.find("p.x").expect("the member probe exists") + 2;
    let in_call = CLEAN.find("add(p.x").expect("the call probe exists") + 4;
    let in_type = CLEAN.find("var p: Point").expect("the type probe exists") + 7;
    let broken_dot = BROKEN.find("    i.\n").expect("the recovered probe exists") + 6;
    vec![
        ("src/main.mw", after_dot, "member"),
        ("src/main.mw", in_call, "call"),
        ("src/main.mw", in_type, "type"),
        ("src/broken.mw", broken_dot, "recovered"),
        ("src/undecodable.mw", 0, "undecodable"),
    ]
}

/// Every probe's completion and active-call outcome, in probe order.
fn corpus_outcomes(snapshot: &AnalysisSnapshot) -> Vec<(String, String, String)> {
    probes()
        .into_iter()
        .map(|(path, offset, label)| {
            let file = identity(path);
            (
                label.to_string(),
                render_completions(snapshot.completions(&file, offset)),
                render_active_call(snapshot.active_call(&file, offset)),
            )
        })
        .collect()
}

/// The frozen corpus outcomes. Each is exactly what the retained-tree path produced
/// before the trees were deleted, so a query-local parse that classified differently —
/// a recovered-broken file that stopped classifying, an undecodable file that stopped
/// being syntax-unavailable, a candidate set that changed — fails here.
#[test]
fn query_local_outcomes_match_the_frozen_corpus() {
    let snapshot = snapshot(corpus_files());
    let outcomes = corpus_outcomes(&snapshot);
    let rendered: Vec<String> = outcomes
        .iter()
        .map(|(label, completions, active)| format!("{label} => {completions} :: {active}"))
        .collect();
    assert_eq!(
        rendered,
        vec![
            "member => member[x:int,y:int] :: \
             fn add(left: int, right: int): int|left: int,right: int|Some(0)"
                .to_string(),
            "call => expr[p:Point,Colour:,add:(left: int, right: int): int,run:(): int,\
             none:,some:,ok:,err:,exists:,unreachable:,todo:,isEmpty:,contains:,trim:,\
             split:,lines:,join:,addDays:,daysBetween:,List:,Map:,Id:,maxInt:,minInt:] :: \
             fn add(left: int, right: int): int|left: int,right: int|Some(0)"
                .to_string(),
            "type => type[Point:,Colour:,int:,bool:,string:,bytes:,date:,instant:,\
             duration:,Option:,Result:,List:,Map:,Id:] :: absent"
                .to_string(),
            "recovered => member[name:string] :: absent".to_string(),
            "undecodable => unavailable:syntax :: unavailable:syntax".to_string(),
        ]
    );
}

/// Repeating a query re-derives the same outcome: the parse is transient, so nothing
/// accumulates and no second query sees different state.
#[test]
fn repeating_a_query_is_stable() {
    let snapshot = snapshot(corpus_files());
    assert_eq!(corpus_outcomes(&snapshot), corpus_outcomes(&snapshot));
    assert_eq!(corpus_outcomes(&snapshot), corpus_outcomes(&snapshot));
}

/// A recovered-broken file classifies positions, but parseability is never inferred
/// from the query-local parse: the snapshot's independent `broken_files` record still
/// makes its hover and document-symbol facts syntax-unavailable.
#[test]
fn parseability_is_not_inferred_from_a_query_local_parse() {
    let snapshot = snapshot(corpus_files());
    let broken = identity("src/broken.mw");
    assert!(matches!(
        snapshot.completions(&broken, BROKEN.find("    i.\n").expect("probe") + 6),
        Ok(CompletionOutcome::Ready(Fact::Present(_)))
    ));
    assert!(matches!(
        snapshot.hover(&broken, 0),
        Ok(Fact::Unavailable(Unavailability::Syntax))
    ));
    assert!(matches!(
        snapshot.document_symbols(&broken),
        Ok(Fact::Unavailable(Unavailability::Syntax))
    ));
}

/// A file of exactly the largest admitted size whose facts a snapshot can still hold.
///
/// Declaration density is capped by owners outside this row — the image function table
/// refuses a project of more than 4096 functions, and the snapshot fact ceilings refuse
/// one whose bodies produce more than 65 536 facts — so a maximum-size file that is
/// *uniformly* dense in declarations has no snapshot at all to query. The fixture
/// therefore carries the densest declaration set a snapshot admits and reaches the
/// admission ceiling with comment filler. The parse a query runs still lexes and parses
/// every one of the file's bytes, which is what the budget measures.
fn maximum_admitted_file() -> Vec<u8> {
    let mut source = String::from("module big\n\n");
    for index in 0..2_000usize {
        source.push_str(&format!(
            "fn f{index}(a: int, b: int): int {{\n    var t: int = a\n    t = t + b\n    return t\n}}\n\n"
        ));
    }
    let filler = "// filler line carrying ordinary comment text for the lexer to scan\n";
    while source.len() + filler.len() <= MAX_ADMITTED_FILE_BYTES {
        source.push_str(filler);
    }
    while source.len() < MAX_ADMITTED_FILE_BYTES {
        source.push('\n');
    }
    assert_eq!(source.len(), MAX_ADMITTED_FILE_BYTES);
    source.into_bytes()
}

/// The worst wall time of five completion queries over `path`, after one warm query.
fn worst_query_ms(snapshot: &AnalysisSnapshot, path: &str) -> u128 {
    let file = identity(path);
    let _ = snapshot.completions(&file, 64);
    let mut worst = 0u128;
    for _ in 0..5 {
        let started = Instant::now();
        let outcome = snapshot.completions(&file, 64);
        worst = worst.max(started.elapsed().as_millis());
        assert!(outcome.is_ok(), "the query resolves");
    }
    worst
}

/// A budget is a property of the optimized profile the language server ships in. The
/// unoptimized profile runs the same code an order of magnitude slower, so the
/// measurement is recorded there and enforced here.
#[track_caller]
fn assert_within_budget(measured: u128, budget: u128, bytes: usize) {
    eprintln!("query-local completion over {bytes} bytes: worst {measured} ms");
    if cfg!(debug_assertions) {
        return;
    }
    assert!(
        measured <= budget,
        "a query over a {bytes}-byte file took {measured} ms, over the {budget} ms budget"
    );
}

/// One query over a maximum admitted file stays inside the editor-latency budget.
#[test]
fn a_query_over_a_maximum_admitted_file_stays_in_budget() {
    let snapshot = snapshot(vec![("src/big.mw", maximum_admitted_file())]);
    assert_within_budget(
        worst_query_ms(&snapshot, "src/big.mw"),
        QUERY_BUDGET_MS,
        MAX_ADMITTED_FILE_BYTES,
    );
}

/// One query over an ordinary-sized file — what an editor session spends nearly all of
/// its queries on — stays far inside the budget, so the worst case above is the tail and
/// not the common cost.
#[test]
fn a_query_over_an_ordinary_file_stays_far_inside_the_budget() {
    let mut source = String::from("module ordinary\n\n");
    let filler = "// filler line carrying ordinary comment text for the lexer to scan\n";
    for index in 0..120usize {
        source.push_str(&format!(
            "fn f{index}(a: int, b: int): int {{\n    var t: int = a\n    t = t + b\n    return t\n}}\n\n"
        ));
    }
    while source.len() + filler.len() <= ORDINARY_FILE_BYTES {
        source.push_str(filler);
    }
    let snapshot = snapshot(vec![("src/ordinary.mw", source.into_bytes())]);
    assert_within_budget(
        worst_query_ms(&snapshot, "src/ordinary.mw"),
        ORDINARY_QUERY_BUDGET_MS,
        ORDINARY_FILE_BYTES,
    );
}

/// A maximum admitted file yields a snapshot, so the budgets above measure a reachable
/// worst case rather than a project the compiler would refuse.
///
/// Building the snapshot is also the measurement point for the analysis build transient:
/// running this test alone, against a run of the querying test above, separates what the
/// drive spends materializing every module's tree from what one query-local parse costs.
#[test]
fn a_maximum_admitted_file_yields_a_snapshot() {
    let snapshot = snapshot(vec![("src/big.mw", maximum_admitted_file())]);
    let file = identity("src/big.mw");
    assert!(matches!(
        snapshot.document_symbols(&file),
        Ok(Fact::Present(_))
    ));
}
