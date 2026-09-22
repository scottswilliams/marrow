//! The durable half of the semantic model: stored value shapes, the minted identity
//! anchor set and its gaps, root admission and the steering a failed admission owes its
//! references, transaction ownership, multi-resource projects, and member namespaces.
//!
//! Every fixture drives the production `compile` path over a store declaration and
//! asserts the typed diagnostic, the minted anchors, or the encoded image bytes.

use marrow_compile::{CompileFailure, SourceDiagnostic, compile};
use marrow_project::ProjectInput;

#[path = "common/ids.rs"]
mod ids;
use marrow_test_support::project as project_capture;

#[path = "durable_model/durable_identity_gaps.rs"]
mod durable_identity_gaps;
#[path = "durable_model/durable_identity_stability.rs"]
mod durable_identity_stability;
#[path = "durable_model/durable_value_dag.rs"]
mod durable_value_dag;
#[path = "durable_model/member_namespaces.rs"]
mod member_namespaces;
#[path = "durable_model/multi_resource.rs"]
mod multi_resource;
#[path = "durable_model/root_admission_steering.rs"]
mod root_admission_steering;
#[path = "durable_model/transaction_ownership.rs"]
mod transaction_ownership;

/// A single-module project captured under the identity ledger `ids`.
fn project(source: &str, ids: Option<&[u8]>) -> ProjectInput {
    project_capture::project_with_ids(&[("src/main.mw", source)], ids)
}

/// The diagnostics of a project the fixture requires to be refused.
fn refused(input: &ProjectInput) -> Vec<SourceDiagnostic> {
    match compile(input) {
        Ok(compiled) => panic!("expected a refusal, compiled: {compiled:?}"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
        Err(other) => panic!("expected source diagnostics, got {other:?}"),
    }
}
