//! Multiple roots and their indexes: root presence, managed indexes, index reads, entry identity, and optional presence steering.

mod common;

#[path = "multi_root/entry_identity_value.rs"]
mod entry_identity_value;
#[path = "multi_root/index_read.rs"]
mod index_read;
#[path = "multi_root/managed_indexes.rs"]
mod managed_indexes;
#[path = "multi_root/optional_presence_steer.rs"]
mod optional_presence_steer;
#[path = "multi_root/root_index.rs"]
mod root_index;
#[path = "multi_root/root_presence.rs"]
mod root_presence;
#[path = "multi_root/roots.rs"]
mod roots;
