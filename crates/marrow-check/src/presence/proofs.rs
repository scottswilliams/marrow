use marrow_syntax::Severity;

use super::scope::NameScope;
use super::target::{
    ReadTarget, declaration_proves_presence, proof_place, read_file, read_target_with_scope,
};
use crate::facts::{PresenceProofFact, PresenceProofPlace, PresenceProofRead, PresenceProofSource};
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
    let source = match context {
        ReadContext::Resolved => PresenceProofSource::Narrowing,
        ReadContext::AttachedData => PresenceProofSource::AttachedDataPending,
        ReadContext::Bare if narrowed.contains(&target) => PresenceProofSource::Narrowing,
        ReadContext::Bare if declaration_proves_presence(program, &target) => {
            PresenceProofSource::Declaration
        }
        ReadContext::Bare => PresenceProofSource::AttachedDataPending,
    };
    Some(ReadProof {
        place,
        keys: target.keys,
        read: target.read,
        source,
    })
}

pub(super) fn record_read(
    program: &mut CheckedProgram,
    expr: &CheckedExpr,
    proof: ReadProof,
    context: ReadContext,
    diagnostics: &mut Vec<CheckDiagnostic>,
) {
    if proof.source == PresenceProofSource::AttachedDataPending && context == ReadContext::Bare {
        diagnostics.push(CheckDiagnostic {
            code: CHECK_BARE_MAYBE_PRESENT_READ,
            severity: Severity::Error,
            file: read_file(program, &proof.place).unwrap_or_default(),
            message: "maybe-present saved read must be resolved at the read site".to_string(),
            span: expr.span(),
        });
    }
    program.facts.record_presence_proof(PresenceProofFact {
        place: proof.place,
        keys: proof.keys,
        read: proof.read,
        source: proof.source,
        span: expr.span(),
    });
}

pub(super) struct ReadProof {
    place: PresenceProofPlace,
    keys: Vec<String>,
    read: PresenceProofRead,
    source: PresenceProofSource,
}
