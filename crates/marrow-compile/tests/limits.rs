//! Construction bounds under hostile width: the resource-limit totality gate, the
//! generic-issuance amplification corpora, and the wide-resource scale floor.
//!
//! Each fixture drives the production `compile` path over an at-or-past-bound project and
//! asserts the classified outcome, so a bound change moves the corpus with it.

use marrow_project::ProjectInput;

use marrow_test_programs::ledger::ledger;
use marrow_test_programs::project as project_capture;

#[path = "limits/issuance_amplification.rs"]
mod issuance_amplification;
#[path = "limits/resource_limits.rs"]
mod resource_limits;
#[path = "limits/wide_resource.rs"]
mod wide_resource;

/// The single-file project every fixture here widens, captured through the production
/// project owner under the identity ledger `ids`.
fn project(source: &str, ids: Option<&[u8]>) -> ProjectInput {
    project_capture::project_with_ids(&[("src/main.mw", source)], ids)
}
