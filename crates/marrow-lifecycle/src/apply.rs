//! Explicit metadata-only activation after exact OLD admission and a complete
//! logical audit. The held engine remains read-only under OLD's layout and is
//! dropped after publication; no service can escape with a stale projection.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use marrow_codes::Code;
use marrow_image::{
    CeilingDescriptor, CeilingId, DurableContractView, DurableMemberView, DurableMemberViewKind,
    DurableMemberViews, ImageId, LedgerIdBytes, ValueShapeComparison, ValueShapeView,
};
use marrow_kernel::durable::NativeOpenAccess;
use marrow_verify::SemanticNodeKind;

use crate::actor::{ImageAdmission, rewrite_atomically};
use crate::envelope::{EnvelopeRecord, EnvelopeState};
use crate::head::MAX_ACCEPTED_CEILING_BYTES;
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
    /// The exact proposed standing ceiling requires explicit acceptance.
    CeilingUnaccepted {
        old: CeilingId,
        proposed: CeilingId,
        added: Vec<crate::ExceedingDemand>,
    },
    /// The proposed map or ceiling exceeds an existing persisted bound.
    Limit,
    Lifecycle(LifecycleError),
}

impl ApplyError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unsupported => Code::StoreApplyUnsupported.as_str(),
            Self::CeilingUnaccepted { .. } => Code::StoreCeilingUnaccepted.as_str(),
            Self::Limit => Code::StoreLimit.as_str(),
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
            Self::CeilingUnaccepted { proposed, .. } => write!(
                f,
                "accept the proposed standing ceiling with --accept-ceiling {}",
                proposed.to_hex()
            ),
            Self::Limit => write!(f, "the proposed store metadata exceeds its persisted bound"),
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
    if !preserves(old.durable_graph(), new.durable_graph()) {
        return Err(ApplyError::Unsupported);
    }
    let admission = ImageAdmission::derive(&old, old_projection);
    let names = admission.audit_names();
    let opened = open_admitted(dir, NativeOpenAccess::ReadOnly, |head| {
        admission.admit_exact(head)
    })
    .map_err(|error| audit_failure(audit::open_error(error)))?;

    let ceiling = CeilingDescriptor::from_payload(&opened.head.accepted_ceiling)
        .map_err(|_| audit_failure(AuditError::InconsistentBinding))?;
    let demand = new.demand_union();
    let expanded = ceiling
        .expanded(&demand, MAX_ACCEPTED_CEILING_BYTES as usize)
        .ok_or(ApplyError::Limit)?;
    let old_ceiling = ceiling.ceiling_id();
    let proposed = expanded.ceiling_id();
    if accepted.is_some_and(|id| id != proposed) || (old_ceiling != proposed && accepted.is_none())
    {
        let added = crate::authority::admit_demand(&new, &demand, &ceiling)
            .err()
            .map_or_else(Vec::new, |refusal| refusal.exceeding);
        return Err(ApplyError::CeilingUnaccepted {
            old: old_ceiling,
            proposed,
            added,
        });
    }
    let old_ids: HashSet<_> = opened
        .head
        .head_map
        .entries()
        .iter()
        .map(|entry| entry.ledger_id)
        .collect();
    let additions: Vec<_> = new
        .semantic_nodes()
        .iter()
        .filter(|node| node.kind != SemanticNodeKind::Index)
        .map(|node| node.path.node_id())
        .filter(|id| !old_ids.contains(id))
        .collect();
    let head_map = opened
        .head
        .head_map
        .extend(&additions)
        .map_err(|_| ApplyError::Limit)?;
    drop(old_ids);
    drop(additions);
    let new_admission = ImageAdmission::derive(&new, new_projection);
    let next_head = LogicalHead {
        binding: *new_admission.incoming(),
        head_map,
        accepted_ceiling: expanded.atom_set_payload(),
        commit_position: opened.head.commit_position,
        data_digest: opened.head.data_digest,
        data_digest_position: opened.head.data_digest_position,
    };
    // Consume NEW's one projection through the same exact Head/layout validator
    // used by normal opening, without opening another engine or constructing service.
    new_admission
        .admit_exact_with_demand(&next_head, &demand)
        .map_err(|error| audit_failure(audit::open_error(AdmitError::Refused(error))))?;
    drop(demand);
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
    drop(report);
    drop(names);
    drop(old);
    drop(new);
    drop(ceiling);
    drop(expanded);
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

fn preserves(old: DurableContractView<'_>, new: DurableContractView<'_>) -> bool {
    if old.application() != new.application() {
        return false;
    }
    let mut roots = BTreeMap::new();
    for root in new.roots() {
        if roots.insert(root.placement(), root).is_some() {
            return false;
        }
    }
    let mut values = old.value_shapes().compare_with(new.value_shapes());
    for before in old.roots() {
        let Some(after) = roots.remove(&before.placement()) else {
            return false;
        };
        if before.product() != after.product() || before.keys() != after.keys() {
            return false;
        }
        let mut indexes = BTreeMap::new();
        for index in after.indexes() {
            if indexes.insert(index.id, index).is_some() {
                return false;
            }
        }
        for index in before.indexes() {
            let Some(other) = indexes.remove(&index.id) else {
                return false;
            };
            if index.unique != other.unique || index.components != other.components {
                return false;
            }
        }
        if !indexes.is_empty() || !members(before.members(), after.members(), new, &mut values) {
            return false;
        }
    }
    roots.is_empty()
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
) -> bool {
    let mut remaining = BTreeMap::new();
    for member in new {
        if remaining.insert(identity(member), member).is_some() {
            return false;
        }
    }
    for before in old {
        let Some(after) = remaining.remove(&identity(before)) else {
            return false;
        };
        match (before.kind(), after.kind()) {
            (DurableMemberViewKind::Field(left), DurableMemberViewKind::Field(right)) => {
                if left.required() != right.required()
                    || values.same(left.value(), right.value()) != Some(true)
                {
                    return false;
                }
            }
            (DurableMemberViewKind::Group(_), DurableMemberViewKind::Group(_)) => {}
            (DurableMemberViewKind::Branch(left), DurableMemberViewKind::Branch(right)) => {
                if left.keys() != right.keys() {
                    return false;
                }
            }
            _ => return false,
        }
        // Verified declaration nesting is bounded before a view can reach here.
        if !members(before.members(), after.members(), graph, values) {
            return false;
        }
    }
    remaining.values().all(|member| matches!(
        member.kind(),
        DurableMemberViewKind::Field(field)
            if !field.required()
                && matches!(graph.value_shapes().view(field.value()), Some(ValueShapeView::Scalar(_)))
    ))
}
