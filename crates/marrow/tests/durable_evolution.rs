//! Durable evolution: field widening, widened values, enum reuse, identity allocation,
//! and the identity ledger.

mod common;

#[path = "durable_evolution/durable_enum_reuse.rs"]
mod durable_enum_reuse;
#[path = "durable_evolution/durable_field_widening.rs"]
mod durable_field_widening;
#[path = "durable_evolution/durable_id_allocation.rs"]
mod durable_id_allocation;
#[path = "durable_evolution/durable_identity.rs"]
mod durable_identity;
#[path = "durable_evolution/durable_widened_values.rs"]
mod durable_widened_values;
