//! Durable state through the store: references, groups, branches, and transactions.

#[path = "common/project.rs"]
pub mod common;

#[path = "durable_state/branches.rs"]
mod branches;
#[path = "durable_state/entry_references.rs"]
mod entry_references;
#[path = "durable_state/groups.rs"]
mod groups;
#[path = "durable_state/transactions.rs"]
mod transactions;
