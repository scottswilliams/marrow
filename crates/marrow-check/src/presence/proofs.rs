use super::scope::NameScope;
use super::target::{
    ReadTarget, declaration_proves_presence, proof_place, read_file, read_target_with_scope,
};
use crate::facts::{
    PresenceProofDraft, PresenceProofPlace, PresenceProofRead, PresenceProofSource,
    PresenceProofStatus,
};
use crate::{CHECK_BARE_MAYBE_PRESENT_READ, CheckDiagnostic, CheckedExpr, CheckedProgram};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadContext {
    Bare,
    Resolved,
    AttachedData,
}

pub(super) fn read_proof(
    program: &CheckedProgram,
    expr: &CheckedExpr,
    context: ReadContext,
    narrowed: &[ReadTarget],
    scope: &NameScope,
) -> Option<ReadProof> {
    let target = read_target_with_scope(program, expr, scope)?;
    let place = proof_place(program, &target)?;
    let (source, status) = match context {
        ReadContext::Resolved => (
            PresenceProofSource::Narrowing,
            PresenceProofStatus::Discharged,
        ),
        ReadContext::AttachedData => (
            PresenceProofSource::AttachedData,
            PresenceProofStatus::PendingAttachedData,
        ),
        ReadContext::Bare if narrowed.contains(&target) => (
            PresenceProofSource::Narrowing,
            PresenceProofStatus::Discharged,
        ),
        ReadContext::Bare if declaration_proves_presence(program, &target) => (
            PresenceProofSource::Declaration,
            PresenceProofStatus::Discharged,
        ),
        ReadContext::Bare => (
            PresenceProofSource::AttachedData,
            PresenceProofStatus::PendingAttachedData,
        ),
    };
    Some(ReadProof {
        place,
        keys: target.keys,
        read: target.read,
        source,
        status,
    })
}

pub(super) fn record_read(
    program: &mut CheckedProgram,
    expr: &CheckedExpr,
    proof: ReadProof,
    context: ReadContext,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if proof.status == PresenceProofStatus::PendingAttachedData && context == ReadContext::Bare {
        let file = read_file(program, &proof.place).unwrap_or_default();
        diagnostics.push(CheckDiagnostic::error(
            CHECK_BARE_MAYBE_PRESENT_READ,
            &file,
            expr.span(),
            "maybe-present saved read must be resolved at the read site",
        ));
    }
    program.facts.record_presence_proof(PresenceProofDraft {
        place: proof.place,
        keys: proof.keys,
        read: proof.read,
        source: proof.source,
        status: proof.status,
        span: expr.span(),
    });
}

pub(super) struct ReadProof {
    place: PresenceProofPlace,
    keys: Vec<String>,
    read: PresenceProofRead,
    source: PresenceProofSource,
    status: PresenceProofStatus,
}
