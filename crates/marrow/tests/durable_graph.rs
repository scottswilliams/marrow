//! The durable declaration graph: demand, breadth, groups and branches, composite keys,
//! nested branches, subtree purge, and cross-module roots.

mod common;

#[path = "durable_graph/cross_module_roots.rs"]
mod cross_module_roots;
#[path = "durable_graph/durable_composite_keys.rs"]
mod durable_composite_keys;
#[path = "durable_graph/durable_composite_presence.rs"]
mod durable_composite_presence;
#[path = "durable_graph/durable_demand.rs"]
mod durable_demand;
#[path = "durable_graph/durable_graph_breadth.rs"]
mod durable_graph_breadth;
#[path = "durable_graph/durable_graph_groups_branches.rs"]
mod durable_graph_groups_branches;
#[path = "durable_graph/durable_nested_branches.rs"]
mod durable_nested_branches;
#[path = "durable_graph/durable_subtree_purge.rs"]
mod durable_subtree_purge;
