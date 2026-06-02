//! Debug/admin access to raw backend bytes.
//!
//! These APIs are not Marrow's production backup contract. They expose the raw
//! saved-path stream for inspection, conformance checks, and narrowly scoped
//! repair tooling. Production backup must use typed tree-cell facts.

use std::io::{Read, Write};

use crate::backend::{Backend, StoreError};

/// Write a raw saved-path archive for debug/admin inspection.
pub fn write_raw_saved_path_archive(
    backend: &dyn Backend,
    out: &mut dyn Write,
) -> Result<u64, StoreError> {
    crate::archive::write_raw_saved_path_archive(backend, out)
}

/// Read a raw saved-path archive for debug/admin repair tooling.
///
/// The replay runs in one transaction, so a failed read rolls the target back to
/// its prior state.
pub fn read_raw_saved_path_archive(
    input: &mut dyn Read,
    backend: &mut dyn Backend,
) -> Result<u64, StoreError> {
    crate::archive::read_raw_saved_path_archive(input, backend)
}
