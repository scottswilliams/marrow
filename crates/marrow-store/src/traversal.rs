//! The bounded forward `scan_after` collector shared by the two engines.

use crate::engine::{Cell, limits};
use crate::error::StoreError;

pub(crate) trait ScanEntry {
    fn key(&self) -> &[u8];
    fn value(&self) -> &[u8];
}

impl ScanEntry for (&[u8], &[u8]) {
    fn key(&self) -> &[u8] {
        self.0
    }

    fn value(&self) -> &[u8] {
        self.1
    }
}

/// Collect the cells of `entries` that fall under `prefix`, stopping at the first
/// cell outside it and at the record and aggregate-byte limits. `entries` must
/// already be positioned strictly after the scan cursor and in ascending order.
/// At least one cell is always returned when one is available, so a caller that
/// resumes from the last key makes progress even past the aggregate limit.
pub(crate) fn collect_after<T, E>(
    entries: impl IntoIterator<Item = Result<T, E>>,
    prefix: &[u8],
    mut map_error: impl FnMut(E) -> StoreError,
) -> Result<Vec<Cell>, StoreError>
where
    T: ScanEntry,
{
    let mut out: Vec<Cell> = Vec::new();
    let mut aggregate = 0usize;
    for entry in entries {
        let entry = entry.map_err(&mut map_error)?;
        let key = entry.key();
        if !key.starts_with(prefix) {
            break;
        }
        if out.len() == limits::SCAN_MAX_RECORDS {
            break;
        }
        let value = entry.value();
        aggregate = aggregate.saturating_add(key.len() + value.len());
        if aggregate > limits::SCAN_MAX_AGGREGATE_BYTES && !out.is_empty() {
            break;
        }
        out.push((key.to_vec(), value.to_vec()));
    }
    Ok(out)
}
