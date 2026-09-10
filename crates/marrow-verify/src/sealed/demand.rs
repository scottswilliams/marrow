//! One canonical atom owner and sparse selections in the complete function domain.

use marrow_image::{DemandSelection, DemandView, ExportDemand};

#[derive(Debug, Clone)]
pub(crate) struct FunctionDemands {
    universe: ExportDemand,
    pub(crate) rows: Vec<DemandSelection>,
}

impl FunctionDemands {
    pub(crate) fn new(universe: ExportDemand, rows: Vec<Vec<u32>>) -> Self {
        Self {
            universe,
            rows: rows
                .into_iter()
                .map(DemandSelection::from_ordinals)
                .collect(),
        }
    }

    /// The caller supplies an ordinal from this image's complete function domain.
    pub(crate) fn get(&self, function: usize) -> DemandView<'_> {
        self.universe
            .selected(&self.rows[function])
            .expect("verified demand ordinals")
    }

    pub(crate) fn union(&self, functions: impl IntoIterator<Item = usize>) -> ExportDemand {
        let selection = {
            let mut selection = DemandSelection::default();
            let mut scratch = Vec::new();
            for function in functions {
                selection.union_with(&self.rows[function], &mut scratch);
            }
            selection
        };
        self.universe
            .selected(&selection)
            .expect("verified demand union ordinals")
            .to_owned()
    }
}
