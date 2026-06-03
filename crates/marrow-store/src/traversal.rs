//! Prefix scan helpers shared by the private ordered-byte engines.

use crate::backend::{ScanPage, StoreError};

pub(crate) trait Entries<'a>:
    Iterator<Item = Result<(&'a [u8], &'a [u8]), StoreError>>
{
}

impl<'a, I> Entries<'a> for I where I: Iterator<Item = Result<(&'a [u8], &'a [u8]), StoreError>> {}

pub(crate) enum ScanStep {
    Done,
    Continue,
}

pub(crate) struct ScanAccumulator<'a> {
    prefix: &'a [u8],
    limit: usize,
    page: ScanPage,
}

impl<'a> ScanAccumulator<'a> {
    pub(crate) fn new(prefix: &'a [u8], limit: usize) -> Self {
        Self {
            prefix,
            limit,
            page: ScanPage::default(),
        }
    }

    pub(crate) fn step(&mut self, key: &[u8], value: &[u8]) -> ScanStep {
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

    pub(crate) fn into_page(self) -> ScanPage {
        self.page
    }
}

pub(crate) fn scan<'a>(
    entries: impl Entries<'a>,
    prefix: &[u8],
    limit: usize,
) -> Result<ScanPage, StoreError> {
    let mut scan = ScanAccumulator::new(prefix, limit);
    for entry in entries {
        let (key, value) = entry?;
        match scan.step(key, value) {
            ScanStep::Done => break,
            ScanStep::Continue => {}
        }
    }
    Ok(scan.into_page())
}
