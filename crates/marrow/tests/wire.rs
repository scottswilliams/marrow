//! The wire interface and generated client: known-answer traces, the published interface projection, and the generator's output end to end.

mod common;

#[path = "wire/client_e2e.rs"]
mod client_e2e;
#[path = "wire/client_generator.rs"]
mod client_generator;
#[path = "wire/wire_interface.rs"]
mod wire_interface;
#[path = "wire/wire_kat.rs"]
mod wire_kat;
