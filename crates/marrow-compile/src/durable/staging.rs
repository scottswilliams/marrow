//! Producer-bound custody for one durable store build.
//!
//! The parent durable builder can request only its exact `build_one` operation and a
//! consuming settlement. It cannot borrow, replace, or exchange the armed producer or
//! its diagnostic owner while another store transaction is live.

use marrow_image::{AdmittedGraphInputPlan, DraftTxn, ImageDraft};
use marrow_project::FileIdentity;
use marrow_syntax::ResourceDecl;

use super::{
    AdmittedDraft, DeclarationSite, DurableTypeMetadata, FileRef, GenericInvariant,
    IdentityBuildState, StoreBuild, StoreOccurrence, build_one,
};
use crate::diag::{BoundedDiagnostics, DiagnosticCollector};

/// One armed durable-store producer and the diagnostics written by that producer.
/// Private fields make moving the whole aggregate the only available reassignment.
pub(super) struct StagedStoreTxn<'d> {
    owner: DraftTxn<'d>,
    staged_diagnostics: DiagnosticCollector,
}

impl<'d> StagedStoreTxn<'d> {
    pub(super) fn new(draft: &'d mut ImageDraft) -> Self {
        Self {
            owner: super::admitted(draft),
            staged_diagnostics: DiagnosticCollector::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_one(
        self,
        plan: &AdmittedGraphInputPlan,
        type_metadata: &mut DurableTypeMetadata<'_, '_>,
        resources: &[(FileRef, FileIdentity, &ResourceDecl)],
        declared: DeclarationSite<'_>,
        store: StoreOccurrence<'_>,
        identity_build: &mut IdentityBuildState<'_, '_>,
    ) -> Result<(StoreBuild, BoundedDiagnostics), GenericInvariant> {
        let Self {
            mut owner,
            mut staged_diagnostics,
        } = self;
        let built = build_one(
            AdmittedDraft {
                draft: &mut owner,
                plan,
            },
            type_metadata,
            resources,
            declared,
            store,
            identity_build,
            &mut staged_diagnostics,
        )?;
        match &built {
            StoreBuild::Admitted(_) => {
                owner.commit();
            }
            StoreBuild::Refused(_) => {
                owner.rollback();
            }
        }
        Ok((built, staged_diagnostics.finish()))
    }
}
