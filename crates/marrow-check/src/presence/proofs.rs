use super::scope::NameScope;
use super::target::{
    ReadPlace, ReadTarget, ReadTargetValue, proof_place, read_file, read_target_with_scope,
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
    // A direct address-only saved read is a partial-key composite layer — an iterable
    // inner sub-tree, not a maybe-present value. Descending a field off it is already
    // owned by `check.layer_not_value`, so recording a presence proof here would only
    // pile a second `bare_maybe_present_read` on the same mistake. A neighbor read of
    // such a prefix still resolves a single edge value, and an index range is its own
    // store-index presence, so both keep their proofs.
    if matches!(target.place, ReadPlace::Saved { .. })
        && target.value == ReadTargetValue::AddressOnly
        && target.read == PresenceProofRead::Direct
    {
        return None;
    }
    let place = proof_place(&target)?;
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
    program: &CheckedProgram,
    expr: &CheckedExpr,
    proof: ReadProof,
    context: ReadContext,
    recorder: &mut PresenceRecorder<'_>,
) {
    if proof.status == PresenceProofStatus::PendingAttachedData && context == ReadContext::Bare {
        let file = read_file(program, &proof.place).unwrap_or_default();
        recorder.diagnostics.push(CheckDiagnostic::error(
            CHECK_BARE_MAYBE_PRESENT_READ,
            &file,
            expr.span(),
            "maybe-present saved read must be resolved at the read site",
        ));
    }
    recorder.proofs.push(PresenceProofDraft {
        place: proof.place,
        keys: proof.keys,
        read: proof.read,
        source: proof.source,
        status: proof.status,
        span: expr.span(),
    });
}

pub(super) struct PresenceRecorder<'a> {
    pub(super) proofs: &'a mut Vec<PresenceProofDraft>,
    pub(super) diagnostics: &'a mut Vec<CheckDiagnostic>,
}

pub(super) struct ReadProof {
    place: PresenceProofPlace,
    keys: Vec<String>,
    read: PresenceProofRead,
    source: PresenceProofSource,
    status: PresenceProofStatus,
}
