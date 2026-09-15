//! The lifecycle actor and the contract graph it derives its facts from: binding-facts
//! derivation, the head-map/kernel-numbering agreement, the attach classifier, the commit
//! outcomes an open store reports, and the bounded-representation falsifier over the
//! durable contract graph those facts are read out of.

mod support;

#[path = "actor/commit_outcome.rs"]
mod commit_outcome;
#[path = "actor/graph_bounds.rs"]
mod graph_bounds;
#[path = "actor/lifecycle.rs"]
mod lifecycle;
