//! Durable state through the store: places, groups, branches, and transactions.

#[path = "durable_state/branches.rs"]
mod branches;
#[path = "durable_state/groups.rs"]
mod groups;
#[path = "durable_state/places.rs"]
mod places;
#[path = "durable_state/transactions.rs"]
mod transactions;
