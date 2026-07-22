//! E07 Graph Report gate: the complete durable-graph report journey.
//!
//! The storeless P02a Graph Report (`graph_report.rs`) parses a directed graph out of a
//! text blob. This suite drives its *durable* analog — the `e07_graph_report` fixture —
//! through the shared harness: the graph lives in the store, the build API mutates it
//! under transactions, and the read-only `report` export harvests the whole graph with
//! bounded, presence-heavy traversal into a local working set before assembling the same
//! deterministic report. The library path (`Session`) captures store effects across
//! calls — a committed transaction is observable by a later read, a rolled-back one is
//! not — and the CLI path pins the `marrow test` and `marrow check` journeys.

mod common;

use common::{CallOutcome, Project, Session};
use marrow_vm::Value;

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn some_text(s: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(s.into())))))
}

fn some_int(v: i64) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Int(v)))))
}

fn absent() -> Option<Value> {
    Some(Value::Optional(None))
}

fn session() -> Session {
    Project::from_fixture("e07_graph_report").session()
}

fn report(session: &mut Session) -> String {
    match session.call("report", vec![]) {
        Some(Value::Text(s)) => s.to_string(),
        other => panic!("report did not return text: {other:?}"),
    }
}

/// The frozen report for the canonical rooted chain `a -(5)-> b -(7)-> c`, root `a`, with
/// `b` given a color. Degrees, edges, reachability, order, and cycle sections are all sorted by
/// the ascending durable key order of the traversal, so the text is deterministic.
const CHAIN: &str = "Graph Report\n\
     nodes=3 edges=2 root=a overflow=false\n\
     -- degrees --\n\
     \x20 a [a] out=1 in=0 role=source\n\
     \x20 b [b] out=1 in=1 role=internal color=red\n\
     \x20 c [c] out=0 in=1 role=sink\n\
     -- edges --\n\
     \x20 a -> b w=5\n\
     \x20 b -> c w=7\n\
     -- reachable --\n\
     \x20 from a: a, b, c (3/3)\n\
     -- order --\n\
     \x20 a\n  b\n  c\n\
     -- cycle --\n\
     \x20 none";

const EMPTY: &str = "Graph Report\n\
     nodes=0 edges=0 root=- overflow=false\n\
     -- degrees --\n\
     -- edges --\n\
     \x20 (none)\n\
     -- reachable --\n\
     \x20 (no root)\n\
     -- order --\n\
     \x20 (none)\n\
     -- cycle --\n\
     \x20 none";

const CYCLE: &str = "Graph Report\n\
     nodes=4 edges=4 root=- overflow=false\n\
     -- degrees --\n\
     \x20 a [a] out=1 in=2 role=internal\n\
     \x20 b [b] out=1 in=1 role=internal\n\
     \x20 c [c] out=1 in=1 role=internal\n\
     \x20 d [d] out=1 in=0 role=source\n\
     -- edges --\n\
     \x20 a -> b w=1\n\
     \x20 b -> c w=1\n\
     \x20 c -> a w=1\n\
     \x20 d -> a w=1\n\
     -- reachable --\n\
     \x20 (no root)\n\
     -- order --\n\
     \x20 d\n\
     -- cycle --\n\
     \x20 a, b, c";

const ISOLATED: &str = "Graph Report\n\
     nodes=3 edges=1 root=ghost overflow=false\n\
     -- degrees --\n\
     \x20 a [a] out=1 in=0 role=source\n\
     \x20 b [b] out=0 in=1 role=sink\n\
     \x20 solo [Solo] out=0 in=0 role=isolated\n\
     -- edges --\n\
     \x20 a -> b w=2\n\
     -- reachable --\n\
     \x20 root ghost is not a node\n\
     -- order --\n\
     \x20 a\n  solo\n  b\n\
     -- cycle --\n\
     \x20 none";

/// The frozen per-export demand report `marrow check` prints, one line per export in
/// `module.item` order. The read-only `report` reads both roots, the node's sparse and
/// required fields, and the edge branch and its field; the build exports write what they
/// mutate. Demand describes access and never grants it.
const DEMAND: &str = "graph_report.addEdge reads ^nodes and ^nodes.edges; writes ^nodes and ^nodes.edges\n\
     graph_report.addNode reads ^nodes; writes ^nodes\n\
     graph_report.colorOf reads ^nodes.color\n\
     graph_report.edgeWeight reads ^nodes.edges.weight\n\
     graph_report.nodeExists reads ^nodes\n\
     graph_report.outDegree reads ^nodes.edges\n\
     graph_report.removeEdge writes ^nodes.edges\n\
     graph_report.report reads ^config, ^nodes, ^nodes.color, ^nodes.edges, ^nodes.edges.weight, and ^nodes.label\n\
     graph_report.setColor reads ^nodes; writes ^nodes.color\n\
     graph_report.setRoot reads ^config; writes ^config\n\
     graph_report.tint writes ^nodes.color\n";

/// A rooted chain built across several committed transactions is observable by the
/// read-only report and the read probes: each `addEdge`/`setRoot`/`setColor` commits to
/// the attachment, and the later `report` and probe calls read the committed graph. The
/// report harvests degrees, roles, a present color, reachability, order, and cycle from
/// the durable graph alone.
#[test]
fn a_rooted_chain_reports_from_the_committed_durable_graph() {
    let mut s = session();
    s.call("setRoot", vec![text("a")]);
    s.call("addEdge", vec![text("a"), text("b"), Value::Int(5)]);
    s.call("addEdge", vec![text("b"), text("c"), Value::Int(7)]);
    s.call("setColor", vec![text("b"), text("red")]);

    assert_eq!(report(&mut s), CHAIN);

    // Typed read probes over the same committed graph.
    assert_eq!(
        s.call("nodeExists", vec![text("a")]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        s.call("nodeExists", vec![text("zzz")]),
        Some(Value::Bool(false))
    );
    assert_eq!(s.call("outDegree", vec![text("a")]), Some(Value::Int(1)));
    assert_eq!(s.call("outDegree", vec![text("c")]), Some(Value::Int(0)));
    assert_eq!(s.call("colorOf", vec![text("b")]), some_text("red"));
    assert_eq!(s.call("colorOf", vec![text("a")]), absent());
    assert_eq!(
        s.call("edgeWeight", vec![text("a"), text("b")]),
        some_int(5)
    );
    assert_eq!(s.call("edgeWeight", vec![text("a"), text("c")]), absent());
}

/// An empty graph reports empty sections and no root.
#[test]
fn an_empty_graph_reports_an_empty_graph() {
    let mut s = session();
    assert_eq!(report(&mut s), EMPTY);
}

/// A cycle is left out of the topological order and named on the cycle line; the one
/// acyclic node is emitted. The bounded Kahn traversal runs over the durable graph.
#[test]
fn a_cycle_is_detected_over_the_durable_graph() {
    let mut s = session();
    s.call("addEdge", vec![text("a"), text("b"), Value::Int(1)]);
    s.call("addEdge", vec![text("b"), text("c"), Value::Int(1)]);
    s.call("addEdge", vec![text("c"), text("a"), Value::Int(1)]);
    s.call("addEdge", vec![text("d"), text("a"), Value::Int(1)]);
    assert_eq!(report(&mut s), CYCLE);
}

/// An explicitly added isolated node is reported with the `isolated` role, and a root
/// designation naming no node is flagged rather than silently reaching nothing.
#[test]
fn an_isolated_node_and_a_non_node_root_are_reported() {
    let mut s = session();
    s.call("addNode", vec![text("solo"), text("Solo")]);
    s.call("addEdge", vec![text("a"), text("b"), Value::Int(2)]);
    s.call("setRoot", vec![text("ghost")]);
    assert_eq!(report(&mut s), ISOLATED);
}

/// Removing an edge is observable: the committed delete drops the edge from the
/// out-degree and the weight read, while the endpoint node it pointed at survives.
#[test]
fn removing_an_edge_is_observable_by_a_later_read() {
    let mut s = session();
    s.call("addEdge", vec![text("a"), text("b"), Value::Int(3)]);
    s.call("addEdge", vec![text("a"), text("c"), Value::Int(4)]);
    assert_eq!(s.call("outDegree", vec![text("a")]), Some(Value::Int(2)));

    s.call("removeEdge", vec![text("a"), text("b")]);

    assert_eq!(s.call("outDegree", vec![text("a")]), Some(Value::Int(1)));
    assert_eq!(s.call("edgeWeight", vec![text("a"), text("b")]), absent());
    assert_eq!(
        s.call("edgeWeight", vec![text("a"), text("c")]),
        some_int(4)
    );
    // The delete is payload-only for the edge; the endpoint node b persists.
    assert_eq!(
        s.call("nodeExists", vec![text("b")]),
        Some(Value::Bool(true))
    );
}

/// The guarded `setColor` and the unguarded `tint` are the blessed sparse-set pair. The
/// guard is a no-op on an absent node; the unguarded set over an absent node stages the
/// color alone and the commit rolls the whole transaction back with `run.required_missing`
/// because the required `label` is unset, so the graph is left unchanged. On a present
/// node both land the color.
#[test]
fn a_sparse_color_set_is_guarded_or_rolls_back_atomically() {
    let mut s = session();
    s.call("addNode", vec![text("a"), text("Alpha")]);

    // Guarded set on a present node lands.
    s.call("setColor", vec![text("a"), text("green")]);
    assert_eq!(s.call("colorOf", vec![text("a")]), some_text("green"));

    // Guarded set on an absent node is a silent no-op: no write, no fault, still absent.
    assert_eq!(
        s.try_call("setColor", vec![text("b"), text("blue")]),
        CallOutcome::Value(None)
    );
    assert_eq!(
        s.call("nodeExists", vec![text("b")]),
        Some(Value::Bool(false))
    );

    // Unguarded set on the absent node b faults at commit and rolls back atomically.
    assert_eq!(
        s.try_call("tint", vec![text("b"), text("blue")]),
        CallOutcome::Fault("run.required_missing".to_string())
    );
    assert_eq!(
        s.call("nodeExists", vec![text("b")]),
        Some(Value::Bool(false))
    );
    assert_eq!(s.call("colorOf", vec![text("b")]), absent());

    // Unguarded set on a present node updates it exactly like the guarded form.
    assert_eq!(
        s.try_call("tint", vec![text("a"), text("amber")]),
        CallOutcome::Value(None)
    );
    assert_eq!(s.call("colorOf", vec![text("a")]), some_text("amber"));
}

/// The fixture's in-source `test`s pass end to end through the built binary under
/// `marrow test`: driver tests that seed the durable graph through the build exports and
/// assert the report and probes through the reading exports.
#[test]
fn the_fixture_tests_pass_through_marrow_test() {
    let output = Project::from_fixture("e07_graph_report")
        .run_cli("e07-graph-report", &["test", "--format", "jsonl"]);
    let stdout = output.stdout_text();
    assert!(output.success(), "marrow test must pass: {stdout}");
    let summary = output
        .jsonl_lines()
        .into_iter()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""passed":7"#), "{summary}");
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""errored":0"#), "{summary}");
    assert!(summary.contains(r#""total":7"#), "{summary}");
}

/// `marrow check --demand` describes each export's verifier-reconstructed durable demand
/// in source spelling and exits 0 for the clean fixture. The sentence bytes are frozen.
#[test]
fn check_reports_the_frozen_per_export_demand() {
    let output =
        Project::from_fixture("e07_graph_report").run_cli("e07-graph-report-check", &["check", "--demand"]);
    assert!(
        output.success(),
        "check must succeed on the clean fixture: {}",
        output.stderr_text()
    );
    assert_eq!(output.stdout_text(), DEMAND);
}
