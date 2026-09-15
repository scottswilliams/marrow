//! Explicit metadata-only activation after exact OLD admission and a complete
//! logical audit. The held engine remains read-only under OLD's layout and is
//! dropped after publication; no service can escape with a stale projection.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use marrow_codes::Code;
use marrow_image::{
    CeilingDescriptor, CeilingId, DurableContractView, DurableMemberView, DurableMemberViewKind,
    DurableMemberViews, ExportDemand, ImageId, LedgerIdBytes, ValueShapeComparison, ValueShapeView,
};
use marrow_kernel::durable::NativeOpenAccess;
use marrow_verify::VerifiedImage;

use crate::actor::{ImageAdmission, rewrite_atomically};
use crate::codec::FormatError;
use crate::envelope::{EnvelopeRecord, EnvelopeState};
use crate::head::{ActiveBinding, MAX_ACCEPTED_CEILING_BYTES};
use crate::provision::{AdmitError, open_admitted};
use crate::{AuditError, LifecycleError, LogicalHead, PreparedImage, StoreInstanceId, audit};

/// A completed sparse apply. This record grants no store access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyReceipt {
    pub instance: StoreInstanceId,
    pub old_image: ImageId,
    pub new_image: ImageId,
    pub old_ceiling: CeilingId,
    pub ceiling: CeilingId,
}

/// Refusal or failure of explicit apply. Publication failures preserve the
/// lifecycle owner's distinction between failed metadata and uncertain activation.
#[derive(Debug)]
pub enum ApplyError {
    /// The change does not preserve all old representations with sparse scalar additions.
    Unsupported,
    /// The bounded comparison of old and new field values stopped before reaching a verdict.
    /// This is a resource stop, not a judgement about the change: apply must not report an
    /// examination it never finished as an unsupported change.
    ComparisonExhausted,
    /// The exact proposed standing ceiling requires explicit acceptance.
    CeilingUnaccepted {
        old: CeilingId,
        proposed: CeilingId,
        added: Vec<crate::ExceedingDemand>,
    },
    /// The proposed standing ceiling exceeds the head's persisted payload bound.
    CeilingTooLarge,
    /// The proposed head identity map is not a publishable artifact: an exhausted entry or
    /// lifetime-number bound, or a binding its bijection refuses.
    HeadMap(FormatError),
    Lifecycle(LifecycleError),
}

impl ApplyError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unsupported => Code::StoreApplyUnsupported.as_str(),
            Self::ComparisonExhausted | Self::CeilingTooLarge => Code::StoreLimit.as_str(),
            Self::CeilingUnaccepted { .. } => Code::StoreCeilingUnaccepted.as_str(),
            Self::HeadMap(error) => error.code(),
            Self::Lifecycle(error) => error.code(),
        }
    }
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => write!(
                f,
                "apply requires unchanged old durable representations and only sparse scalar field additions"
            ),
            Self::ComparisonExhausted => write!(
                f,
                "comparing the old and new durable value representations exhausted its bounded work allowance"
            ),
            Self::CeilingUnaccepted { proposed, .. } => write!(
                f,
                "accept the proposed standing ceiling with --accept-ceiling {}",
                proposed.to_hex()
            ),
            Self::CeilingTooLarge => write!(
                f,
                "the proposed standing ceiling exceeds the store head's persisted bound"
            ),
            Self::HeadMap(error) => {
                write!(f, "the proposed head identity map {error}")
            }
            Self::Lifecycle(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ApplyError {}

fn audit_failure(error: AuditError) -> ApplyError {
    ApplyError::Lifecycle(LifecycleError::Audit(error))
}

/// Activate NEW using explicit verified artifacts. OLD must be the exact active
/// image. Existing addresses and values remain in place; added sparse fields are
/// absent. Any standing-ceiling expansion requires its exact union identity.
pub fn apply(
    dir: &Path,
    old: PreparedImage,
    new: PreparedImage,
    accepted: Option<CeilingId>,
) -> Result<ApplyReceipt, ApplyError> {
    let (old, old_projection) = old.into_parts();
    let (new, new_projection) = new.into_parts();
    let (Some(old_projection), Some(new_projection)) = (old_projection, new_projection) else {
        return Err(ApplyError::Lifecycle(LifecycleError::NotExecutable));
    };
    match preserves(old.durable_graph(), new.durable_graph()) {
        Ok(true) => {}
        Ok(false) => return Err(ApplyError::Unsupported),
        Err(ComparisonExhausted) => return Err(ApplyError::ComparisonExhausted),
    }
    let admission = ImageAdmission::derive(&old, old_projection);
    let names = admission.audit_names();
    let opened = open_admitted(dir, NativeOpenAccess::ReadOnly, |head| {
        admission.admit_exact(head)
    })
    .map_err(|error| audit_failure(audit::open_error(error)))?;

    let demand = new.demand_union();
    let AcceptedCeiling {
        old: old_ceiling,
        proposed,
        payload,
    } = accept_ceiling(&opened.head, &new, &demand, accepted)?;
    let new_admission = ImageAdmission::derive(&new, new_projection);
    let next_head = extend_head(&opened.head, *new_admission.incoming(), &new, payload)?;
    // Consume NEW's one projection through the same exact Head/layout validator
    // used by normal opening, without opening another engine or constructing service.
    new_admission
        .admit_exact_with_demand(&next_head, &demand)
        .map_err(|error| audit_failure(audit::open_error(AdmitError::Refused(error))))?;
    let report = audit::inspect(&opened, &names, old.image_id()).map_err(audit_failure)?;
    if !report.is_clean() {
        return Err(ApplyError::Lifecycle(LifecycleError::Invalid(Box::new(
            report,
        ))));
    }
    let receipt = ApplyReceipt {
        instance: opened.envelope.instance,
        old_image: old.image_id(),
        new_image: new.image_id(),
        old_ceiling,
        ceiling: proposed,
    };
    let old_record = EnvelopeRecord {
        metadata: opened.envelope.clone(),
        state: EnvelopeState::Active,
    };
    audit::verify_published(&opened.directory, dir, &old_record, opened.head_digest)
        .map_err(audit_failure)?;
    let envelope = crate::StoreEnvelope {
        writer_toolchain: crate::actor::current_toolchain(),
        ..opened.envelope.clone()
    };
    rewrite_atomically(
        &opened.directory,
        dir,
        &envelope,
        &next_head,
        opened.head_digest,
    )
    .map_err(ApplyError::Lifecycle)?;
    Ok(receipt)
}

/// The standing ceiling apply would publish, and the ceiling it replaces.
struct AcceptedCeiling {
    old: CeilingId,
    proposed: CeilingId,
    payload: Vec<u8>,
}

/// Settle the store's standing authority for NEW. The store keeps its own ceiling when NEW
/// demands no more; otherwise the proposal is exactly the union of the two, which the owner
/// must accept by identity. NEW's own image ceiling is a different set and never stands in
/// for that union.
fn accept_ceiling(
    head: &LogicalHead,
    new: &VerifiedImage,
    demand: &ExportDemand,
    accepted: Option<CeilingId>,
) -> Result<AcceptedCeiling, ApplyError> {
    let ceiling = CeilingDescriptor::from_payload(&head.accepted_ceiling)
        .map_err(|_| audit_failure(AuditError::InconsistentBinding))?;
    let expanded = ceiling
        .expanded(demand, MAX_ACCEPTED_CEILING_BYTES as usize)
        .ok_or(ApplyError::CeilingTooLarge)?;
    let old = ceiling.ceiling_id();
    let proposed = expanded.ceiling_id();
    if accepted.is_some_and(|id| id != proposed) || (old != proposed && accepted.is_none()) {
        let added = crate::authority::admit_demand(new, demand, &ceiling)
            .err()
            .map_or_else(Vec::new, |refusal| refusal.exceeding);
        return Err(ApplyError::CeilingUnaccepted {
            old,
            proposed,
            added,
        });
    }
    Ok(AcceptedCeiling {
        old,
        proposed,
        payload: expanded.atom_set_payload(),
    })
}

/// The head apply would publish: every accepted binding at its existing number, one fresh
/// never-reused number for each durable node NEW adds, and the accepted ceiling. The commit
/// position and data digests carry over unchanged, since apply writes no data cell.
fn extend_head(
    head: &LogicalHead,
    binding: ActiveBinding,
    new: &VerifiedImage,
    accepted_ceiling: Vec<u8>,
) -> Result<LogicalHead, ApplyError> {
    let accepted: HashSet<_> = head
        .head_map
        .entries()
        .iter()
        .map(|entry| entry.ledger_id)
        .collect();
    let additions: Vec<_> = crate::image::numbered_node_ids(new)
        .into_iter()
        .filter(|id| !accepted.contains(id))
        .collect();
    Ok(LogicalHead {
        binding,
        head_map: head
            .head_map
            .extend(&additions)
            .map_err(ApplyError::HeadMap)?,
        accepted_ceiling,
        commit_position: head.commit_position,
        data_digest: head.data_digest,
        data_digest_position: head.data_digest_position,
    })
}

/// A bounded value comparison stopped before reaching a verdict.
struct ComparisonExhausted;

/// Whether NEW preserves every old durable representation, adding only absent sparse scalar
/// fields. `Err` means a bounded value comparison ran out of work, which is not a verdict.
fn preserves(
    old: DurableContractView<'_>,
    new: DurableContractView<'_>,
) -> Result<bool, ComparisonExhausted> {
    if old.application() != new.application() {
        return Ok(false);
    }
    let mut roots = BTreeMap::new();
    for root in new.roots() {
        if roots.insert(root.placement(), root).is_some() {
            return Ok(false);
        }
    }
    let mut values = old.value_shapes().compare_with(new.value_shapes());
    for before in old.roots() {
        let Some(after) = roots.remove(&before.placement()) else {
            return Ok(false);
        };
        if before.product() != after.product() || before.keys() != after.keys() {
            return Ok(false);
        }
        let mut indexes = BTreeMap::new();
        for index in after.indexes() {
            if indexes.insert(index.id, index).is_some() {
                return Ok(false);
            }
        }
        for index in before.indexes() {
            let Some(other) = indexes.remove(&index.id) else {
                return Ok(false);
            };
            if index.unique != other.unique || index.components != other.components {
                return Ok(false);
            }
        }
        if !indexes.is_empty() || !members(before.members(), after.members(), new, &mut values)? {
            return Ok(false);
        }
    }
    Ok(roots.is_empty())
}

fn identity(member: DurableMemberView<'_>) -> LedgerIdBytes {
    match member.kind() {
        DurableMemberViewKind::Field(field) => field.id(),
        DurableMemberViewKind::Group(group) => group.id(),
        DurableMemberViewKind::Branch(branch) => branch.placement(),
    }
}

fn members<'a, 'b>(
    old: DurableMemberViews<'a>,
    new: DurableMemberViews<'b>,
    graph: DurableContractView<'b>,
    values: &mut ValueShapeComparison<'a, 'b>,
) -> Result<bool, ComparisonExhausted> {
    let mut remaining = BTreeMap::new();
    for member in new {
        if remaining.insert(identity(member), member).is_some() {
            return Ok(false);
        }
    }
    for before in old {
        let Some(after) = remaining.remove(&identity(before)) else {
            return Ok(false);
        };
        match (before.kind(), after.kind()) {
            (DurableMemberViewKind::Field(left), DurableMemberViewKind::Field(right)) => {
                if left.required() != right.required() {
                    return Ok(false);
                }
                match values.same(left.value(), right.value()) {
                    Some(true) => {}
                    Some(false) => return Ok(false),
                    None => return Err(ComparisonExhausted),
                }
            }
            (DurableMemberViewKind::Group(_), DurableMemberViewKind::Group(_)) => {}
            (DurableMemberViewKind::Branch(left), DurableMemberViewKind::Branch(right)) => {
                if left.keys() != right.keys() {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
        // Verified declaration nesting is bounded before a view can reach here.
        if !members(before.members(), after.members(), graph, values)? {
            return Ok(false);
        }
    }
    Ok(remaining.values().all(|member| matches!(
        member.kind(),
        DurableMemberViewKind::Field(field)
            if !field.required()
                && matches!(graph.value_shapes().view(field.value()), Some(ValueShapeView::Scalar(_)))
    )))
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
