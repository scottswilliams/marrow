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

use crate::actor::{AdmissionRefusal, BindingStrictness, ImageAdmission, rewrite_atomically};
use crate::codec::FormatError;
use crate::envelope::{EnvelopeRecord, EnvelopeState};
use crate::head::{ActiveBinding, MAX_ACCEPTED_CEILING_BYTES};
use crate::provision::{AdmitError, open_admitted};
use crate::seam::Seam;
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

/// Why apply refuses a change: the first difference between OLD's and NEW's durable
/// graphs that apply cannot publish without rewriting or reinterpreting stored data.
/// Apply adds sparse scalar fields and nothing else, so every other difference is one of
/// these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedChange {
    /// The application identity changed.
    Application,
    /// A root was added or removed, or its product or key columns changed.
    Root,
    /// A managed index was added, removed or changed.
    Index,
    /// An old field, group or branch is absent from NEW.
    MemberRemoved,
    /// A field, group or branch changed kind, requiredness or branch keys.
    MemberChanged,
    /// An old field's stored value representation changed: a scalar was retyped, a stored
    /// struct or enum payload leaf was reordered, renamed, retyped, added or removed, or an
    /// enum member changed. Existing cells would be read with a different meaning.
    StoredValue,
    /// An added member is not a sparse scalar field.
    MemberAdded,
}

impl UnsupportedChange {
    /// The stable receipt word for this reason.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Root => "root",
            Self::Index => "index",
            Self::MemberRemoved => "member_removed",
            Self::MemberChanged => "member_changed",
            Self::StoredValue => "stored_value",
            Self::MemberAdded => "member_added",
        }
    }
}

impl std::fmt::Display for UnsupportedChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Application => {
                "apply cannot change the application identity; provision a fresh store"
            }
            Self::Root => {
                "apply cannot add or remove a root or change its product or key columns; keep \
                 the old roots, or provision a fresh store"
            }
            Self::Index => {
                "apply cannot add, remove or change a managed index; keep the old indexes, or \
                 provision a fresh store"
            }
            Self::MemberRemoved => {
                "apply cannot remove a stored field, group or branch; keep it, or provision a \
                 fresh store"
            }
            Self::MemberChanged => {
                "apply cannot change a stored field's requiredness, a member's kind or a \
                 branch's keys; keep the old declaration, or provision a fresh store"
            }
            Self::StoredValue => {
                "apply does not convert stored values, and this change would read existing \
                 values with a different meaning; keep the old field type and the old order \
                 and names of its struct and enum payload fields, or provision a fresh store"
            }
            Self::MemberAdded => {
                "apply adds only optional scalar fields; declare the new field optional and \
                 scalar, or provision a fresh store"
            }
        })
    }
}

/// Refusal or failure of explicit apply. Publication failures preserve the
/// lifecycle owner's distinction between failed metadata and uncertain activation.
#[derive(Debug)]
pub enum ApplyError {
    /// NEW changes the durable graph in a way apply cannot publish over existing data.
    Unsupported(UnsupportedChange),
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
    pub fn code(&self) -> Code {
        match self {
            Self::Unsupported(_) => Code::StoreApplyUnsupported,
            Self::ComparisonExhausted | Self::CeilingTooLarge => Code::StoreLimit,
            Self::CeilingUnaccepted { .. } => Code::StoreCeilingUnaccepted,
            Self::HeadMap(error) => error.code(),
            Self::Lifecycle(error) => error.code(),
        }
    }
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(change) => change.fmt(f),
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
    apply_observed(dir, old, new, accepted, Seam::NONE)
}

/// [`apply`] over `seam`: the production seam observes nothing; a test's cuts or mutates
/// the sequence at a named step.
pub(crate) fn apply_observed(
    dir: &Path,
    old: PreparedImage,
    new: PreparedImage,
    accepted: Option<CeilingId>,
    seam: Seam,
) -> Result<ApplyReceipt, ApplyError> {
    let (old, old_projection) = old.into_parts();
    let (new, new_projection) = new.into_parts();
    let (Some(old_projection), Some(new_projection)) = (old_projection, new_projection) else {
        return Err(ApplyError::Lifecycle(LifecycleError::NotExecutable));
    };
    match preserves(old.durable_graph(), new.durable_graph()) {
        Ok(()) => {}
        Err(Refusal::Unsupported(change)) => return Err(ApplyError::Unsupported(change)),
        Err(Refusal::Exhausted) => return Err(ApplyError::ComparisonExhausted),
    }
    let admission = ImageAdmission::derive(&old, old_projection);
    let names = admission.audit_names();
    let opened = open_admitted(dir, NativeOpenAccess::ReadOnly, seam, |head| {
        admission.admit(head, BindingStrictness::Exact)
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
        .map_err(|_| audit_failure(AuditError::Refused(AdmissionRefusal::CeilingCorrupt)))?;
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

/// Why the durable graph comparison stopped without accepting NEW.
enum Refusal {
    /// NEW changes something apply cannot publish.
    Unsupported(UnsupportedChange),
    /// A bounded value comparison ran out of work, which is not a verdict.
    Exhausted,
}

/// Refuse with `change` unless `preserved` holds.
fn require(preserved: bool, change: UnsupportedChange) -> Result<(), Refusal> {
    if preserved {
        Ok(())
    } else {
        Err(Refusal::Unsupported(change))
    }
}

/// Accept NEW when it preserves every old durable representation, adding only absent
/// sparse scalar fields; otherwise name the first change it makes. The duplicate-identity
/// arms are unreachable for a verified image, whose placement, member and index ids are
/// pairwise distinct, so they fold into their family's reason.
fn preserves(old: DurableContractView<'_>, new: DurableContractView<'_>) -> Result<(), Refusal> {
    use UnsupportedChange::{Application, Index, Root};
    require(old.application() == new.application(), Application)?;
    let mut roots = BTreeMap::new();
    for root in new.roots() {
        require(roots.insert(root.placement(), root).is_none(), Root)?;
    }
    let mut values = old.value_shapes().compare_with(new.value_shapes());
    for before in old.roots() {
        let Some(after) = roots.remove(&before.placement()) else {
            return Err(Refusal::Unsupported(Root));
        };
        require(
            before.product() == after.product() && before.keys() == after.keys(),
            Root,
        )?;
        let mut indexes = BTreeMap::new();
        for index in after.indexes() {
            require(indexes.insert(index.id, index).is_none(), Index)?;
        }
        for index in before.indexes() {
            let Some(other) = indexes.remove(&index.id) else {
                return Err(Refusal::Unsupported(Index));
            };
            require(
                index.unique == other.unique && index.components == other.components,
                Index,
            )?;
        }
        require(indexes.is_empty(), Index)?;
        members(before.members(), after.members(), new, &mut values)?;
    }
    require(roots.is_empty(), Root)
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
) -> Result<(), Refusal> {
    use UnsupportedChange::{MemberAdded, MemberChanged, MemberRemoved, StoredValue};
    let mut remaining = BTreeMap::new();
    for member in new {
        require(
            remaining.insert(identity(member), member).is_none(),
            MemberChanged,
        )?;
    }
    for before in old {
        let Some(after) = remaining.remove(&identity(before)) else {
            return Err(Refusal::Unsupported(MemberRemoved));
        };
        match (before.kind(), after.kind()) {
            (DurableMemberViewKind::Field(left), DurableMemberViewKind::Field(right)) => {
                require(left.required() == right.required(), MemberChanged)?;
                let same = values
                    .same(left.value(), right.value())
                    .ok_or(Refusal::Exhausted)?;
                require(same, StoredValue)?;
            }
            (DurableMemberViewKind::Group(_), DurableMemberViewKind::Group(_)) => {}
            (DurableMemberViewKind::Branch(left), DurableMemberViewKind::Branch(right)) => {
                require(left.keys() == right.keys(), MemberChanged)?;
            }
            _ => return Err(Refusal::Unsupported(MemberChanged)),
        }
        // Verified declaration nesting is bounded before a view can reach here.
        members(before.members(), after.members(), graph, values)?;
    }
    require(
        remaining.values().all(|member| matches!(
            member.kind(),
            DurableMemberViewKind::Field(field)
                if !field.required()
                    && matches!(graph.value_shapes().view(field.value()), Some(ValueShapeView::Scalar(_)))
        )),
        MemberAdded,
    )
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
