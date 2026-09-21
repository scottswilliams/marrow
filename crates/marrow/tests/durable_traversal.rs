//! Durable traversal: place bases, traversal sources, and the rejections each refuses.

#[path = "common/project.rs"]
pub mod common;

#[path = "durable_traversal/durable_place_base_composition.rs"]
mod durable_place_base_composition;
#[path = "durable_traversal/durable_traversal_place_base.rs"]
mod durable_traversal_place_base;
#[path = "durable_traversal/durable_traversal_rejections.rs"]
mod durable_traversal_rejections;
#[path = "durable_traversal/durable_traversal_source.rs"]
mod durable_traversal_source;
