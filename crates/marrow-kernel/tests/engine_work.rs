//! Engine-call work laws: the counting engine measures what the kernel costs the byte
//! engine for an audit walk, a rejected or permitted call, an exact field mutation, an
//! index scan, and a wide sparse entry read.

mod common;

#[path = "engine_work/audit_walk.rs"]
mod audit_walk;
#[path = "engine_work/call_witness.rs"]
mod call_witness;
#[path = "engine_work/exact_field.rs"]
mod exact_field;
#[path = "engine_work/index_read.rs"]
mod index_read;
#[path = "engine_work/wide_entry.rs"]
mod wide_entry;
