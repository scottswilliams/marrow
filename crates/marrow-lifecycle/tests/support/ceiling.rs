//! The effect-ceiling corpus: one durable shape and two programs over it whose only
//! difference is the durable authority they demand.
//!
//! Both variants share [`IDS`], so the durable contract and the exported interface hold
//! still while the demand grows. That is what isolates an authority refusal from a
//! contract refusal: a case that varied the ledger too would not know which of the two
//! it had provoked.

pub use super::store::{attach_image, provision_approved as provision};
use marrow_verify::VerifiedImage;

/// The identity ledger shared by every variant: the application, the `Counter` product, its
/// two fields, the `counters` root, and its key column.
pub const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

/// The durable shape every variant starts from. The preemption case edits it, promoting
/// `label` to required.
pub const SHAPE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter
"#;

/// A read-only export. Its demand union is the accepted ceiling a store provisioned under
/// it records: it reads `^counters.value` and nothing more.
pub fn source_read_only() -> String {
    format!("{SHAPE}\npub fn readValue(n: int): int {{\n    return ^counters[n].value ?? 0\n}}\n")
}

/// The same export, same signature, broadened to also mutate — it now stamps the sparse
/// `label` of a present counter. The durable contract and interface are unchanged; only the
/// demand grows, by a write of `^counters.label` and the presence probe the guard makes.
pub fn source_broadened() -> String {
    format!(
        "{SHAPE}\npub fn readValue(n: int): int {{\n    var result = 0\n    \
         transaction {{\n        place slot = ^counters[n]\n        \
         if exists(slot) {{\n            slot.label = \"seen\"\n        }}\n        \
         result = ^counters[n].value ?? 0\n    }}\n    return result\n}}\n"
    )
}

/// A verified image of `source` under [`IDS`].
pub fn image(source: &str) -> VerifiedImage {
    marrow_test_programs::program::compile(source, IDS)
}
