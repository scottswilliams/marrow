//! Prefix scan helpers shared by the private ordered-byte engines.

use crate::backend::{ScanPage, StoreError};

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

enum ScanStep {
    Done,
    Continue,
}

struct ScanAccumulator<'a> {
    prefix: &'a [u8],
    limit: usize,
    page: ScanPage,
}

impl<'a> ScanAccumulator<'a> {
    fn new(prefix: &'a [u8], limit: usize) -> Self {
        Self {
            prefix,
            limit,
            page: ScanPage::default(),
        }
    }

    fn step(&mut self, key: &[u8], value: &[u8]) -> ScanStep {
        if !key.starts_with(self.prefix) {
            return ScanStep::Done;
        }
        if self.page.entries.len() == self.limit {
            self.page.truncated = true;
            return ScanStep::Done;
        }
        self.page.entries.push((key.to_vec(), value.to_vec()));
        ScanStep::Continue
    }

    fn into_page(self) -> ScanPage {
        self.page
    }
}

pub(crate) fn scan<T: ScanEntry, E>(
    entries: impl IntoIterator<Item = Result<T, E>>,
    prefix: &[u8],
    limit: usize,
    mut map_error: impl FnMut(E) -> StoreError,
) -> Result<ScanPage, StoreError> {
    let mut scan = ScanAccumulator::new(prefix, limit);
    for entry in entries {
        let entry = entry.map_err(&mut map_error)?;
        match scan.step(entry.key(), entry.value()) {
            ScanStep::Done => break,
            ScanStep::Continue => {}
        }
    }
    Ok(scan.into_page())
}
