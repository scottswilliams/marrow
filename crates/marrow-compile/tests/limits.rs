//! Construction bounds under hostile width: the resource-limit totality gate, the
//! generic-issuance amplification corpora, and the wide-resource scale floor.
//!
//! Each fixture drives the production `compile` path over an at-or-past-bound project and
//! asserts the classified outcome, so a bound change moves the corpus with it.

use marrow_project::ProjectInput;

#[path = "common/ledger.rs"]
mod ledger_fixture;
#[path = "common/project.rs"]
mod project_capture;

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

/// A durable identity ledger over an ordered anchor list. The caller lists exactly
/// the anchors its shape declares; the format is written in one place.
fn ledger(anchors: &[String]) -> Vec<u8> {
    let borrowed: Vec<&str> = anchors.iter().map(String::as_str).collect();
    ledger_fixture::ledger(&borrowed)
}
