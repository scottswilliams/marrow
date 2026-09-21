//! Descriptor-rooted custody: entry-name admission, the custody operations themselves,
//! and the dependency conditions the syscall backend is pinned under.

#[path = "custody/dependency_conditions.rs"]
mod dependency_conditions;
#[path = "custody/descriptor.rs"]
mod descriptor;
#[path = "custody/entry_names.rs"]
mod entry_names;
