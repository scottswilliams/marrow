//! Durable data through the lifecycle: the read-only store audit with backup and restore,
//! and staged bulk import.

mod support;

#[path = "durable_data/audit.rs"]
mod audit;
#[path = "durable_data/import.rs"]
mod import;
