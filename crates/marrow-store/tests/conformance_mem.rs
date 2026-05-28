//! The in-memory store satisfies the shared backend conformance suite — the same
//! laws every persistent backend must pass.

use marrow_store::conformance;
use marrow_store::mem::MemStore;

#[test]
fn mem_store_passes_the_conformance_suite() {
    conformance::run_all(MemStore::new);
}
