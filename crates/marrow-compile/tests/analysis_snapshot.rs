//! The revisioned `AnalysisSnapshot` as a projection: what it retains, what it agrees
//! with, and what a refusal does not suppress.
//!
//! `analyze`, `check`, and the production compile drive one pipeline, so their
//! diagnostics agree; a refused declaration leaves every independently-prerequisite
//! phase running; and a fact ceiling refuses a snapshot transactionally rather than
//! admitting a truncated fact set.

use marrow_project::FileIdentity;

#[path = "common/project.rs"]
mod project_capture;
use project_capture::{project, project_with_ids};

#[path = "analysis_snapshot/fact_settlement.rs"]
mod fact_settlement;
#[path = "analysis_snapshot/semantic_availability.rs"]
mod semantic_availability;
#[path = "analysis_snapshot/snapshot_agreement.rs"]
mod snapshot_agreement;
#[path = "analysis_snapshot/snapshot_retention.rs"]
mod snapshot_retention;

fn identity(path: &str) -> marrow_compile::ProjectFile {
    marrow_compile::ProjectFile::root(FileIdentity::validate(path).expect("canonical identity").0)
}
