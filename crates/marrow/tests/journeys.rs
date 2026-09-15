//! Whole-program journeys over frozen on-disk fixtures: the workshop, the graph reports, the scale corpus, the source-test and formatter runs, and diagnostic actionability.

mod common;

#[path = "journeys/collections_temporal.rs"]
mod collections_temporal;
#[path = "journeys/diagnostic_actionability.rs"]
mod diagnostic_actionability;
#[path = "journeys/durable_graph_report.rs"]
mod durable_graph_report;
#[path = "journeys/graph_report.rs"]
mod graph_report;
#[path = "journeys/scale_corpus.rs"]
mod scale_corpus;
#[path = "journeys/tests_and_formatter.rs"]
mod tests_and_formatter;
#[path = "journeys/workshop.rs"]
mod workshop;
#[path = "journeys/workshop_e2e.rs"]
mod workshop_e2e;
#[path = "journeys/workshop_trusted_main.rs"]
mod workshop_trusted_main;
