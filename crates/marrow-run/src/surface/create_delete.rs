use std::collections::{HashMap, HashSet};

use marrow_check::{
    CheckedFacts, CheckedProgram, ResourceMemberId, StoreFact, SurfaceCreateOperationDescriptor,
    SurfaceDeleteOperationDescriptor, SurfaceFact, SurfaceId,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_syntax::SourceSpan;

use crate::write::{ResourceValue, SuppliedIdentity, plan_resource_delete, plan_resource_write};

use super::{
    MissingRecord, PlannedSurfaceWriteValue, SurfaceError, SurfaceIdentityInputShape,
    SurfaceNodeReadShape, SurfaceReadIdentity, SurfaceReadRecord, SurfaceRecordReadPlan,
    SurfaceValue, SurfaceWriteCommit, SurfaceWriteMember, abi_mismatch, admit_surface_store,
    backing_node_operation, checked_surface, lower_surface_write_value,
    map_surface_write_plan_error, request_at, surface_store_error,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceCreateField {
    pub catalog_id: CatalogId,
    pub value: SurfaceValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceCreateInput<'a> {
    Singleton {
        fields: &'a [SurfaceCreateField],
    },
    Point {
        identity: &'a [SavedKey],
        fields: &'a [SurfaceCreateField],
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceDeleteInput<'a> {
    Singleton,
    Point { identity: &'a [SavedKey] },
}

pub struct SurfaceCreate<'a> {
    program: &'a CheckedProgram,
    store: &'a TreeStore,
    plan: SurfaceCreatePlan<'a>,
}

pub struct SurfaceDelete<'a> {
    program: &'a CheckedProgram,
    store: &'a TreeStore,
    plan: SurfaceDeletePlan<'a>,
}

struct SurfaceCreatePlan<'a> {
    surface: SurfaceId,
    surface_label: String,
    shape: SurfaceNodeReadShape,
    accepted_epoch: Option<u64>,
    source_digest: String,
    place: marrow_check::CheckedSavedPlace,
    record: SurfaceRecordReadPlan<'a>,
    fields: HashMap<CatalogId, SurfaceWriteMember>,
    span: SourceSpan,
}

struct SurfaceDeletePlan<'a> {
    surface: SurfaceId,
    surface_label: String,
    shape: SurfaceNodeReadShape,
    accepted_epoch: Option<u64>,
    source_digest: String,
    place: marrow_check::CheckedSavedPlace,
    record: SurfaceRecordReadPlan<'a>,
    span: SourceSpan,
}

impl<'a> SurfaceCreate<'a> {
    pub fn admit(
        program: &'a CheckedProgram,
        store: &'a TreeStore,
        surface: SurfaceId,
    ) -> Result<Self, SurfaceError> {
        admit_surface_store(program, store)?;
        Ok(Self {
            program,
            store,
            plan: SurfaceCreatePlan::prepare(program, surface)?,
        })
    }

    pub fn admit_by_operation_tag(
        program: &'a CheckedProgram,
        store: &'a TreeStore,
        operation_tag: &str,
    ) -> Result<Self, SurfaceError> {
        let surface = checked_create_surface_by_tag(program, operation_tag)?;
        admit_surface_store(program, store)?;
        Ok(Self {
            program,
            store,
            plan: SurfaceCreatePlan::prepare(program, surface.id)?,
        })
    }

    pub fn surface(&self) -> SurfaceId {
        self.plan.surface
    }

    pub fn shape(&self) -> SurfaceNodeReadShape {
        self.plan.shape
    }

    pub fn point_identity_shape(&self) -> Result<SurfaceIdentityInputShape, SurfaceError> {
        if self.plan.shape != SurfaceNodeReadShape::Point {
            return Err(request_at(
                format!(
                    "surface `{}` is not backed by a keyed store",
                    self.plan.surface_label
                ),
                self.plan.span,
            ));
        }
        self.plan.record.identity_input_shape()
    }

    pub fn execute(
        &self,
        input: SurfaceCreateInput<'_>,
    ) -> Result<SurfaceReadRecord, SurfaceError> {
        match (self.plan.shape, input) {
            (SurfaceNodeReadShape::Singleton, SurfaceCreateInput::Singleton { fields }) => {
                self.create_identity(&[], fields)
            }
            (SurfaceNodeReadShape::Point, SurfaceCreateInput::Point { identity, fields }) => {
                self.create_point(identity, fields)
            }
            (SurfaceNodeReadShape::Singleton, SurfaceCreateInput::Point { .. }) => Err(request_at(
                format!(
                    "surface `{}` is a singleton create",
                    self.plan.surface_label
                ),
                self.plan.span,
            )),
            (SurfaceNodeReadShape::Point, SurfaceCreateInput::Singleton { .. }) => Err(request_at(
                format!("surface `{}` requires an identity", self.plan.surface_label),
                self.plan.span,
            )),
        }
    }

    pub fn create_point(
        &self,
        identity: &[SavedKey],
        fields: &[SurfaceCreateField],
    ) -> Result<SurfaceReadRecord, SurfaceError> {
        if self.plan.shape != SurfaceNodeReadShape::Point {
            return Err(request_at(
                format!(
                    "surface `{}` is not backed by a keyed store",
                    self.plan.surface_label
                ),
                self.plan.span,
            ));
        }
        self.plan.record.validate_identity(identity)?;
        self.create_identity(identity, fields)
    }

    pub fn create_singleton(
        &self,
        fields: &[SurfaceCreateField],
    ) -> Result<SurfaceReadRecord, SurfaceError> {
        if self.plan.shape != SurfaceNodeReadShape::Singleton {
            return Err(request_at(
                format!(
                    "surface `{}` is not backed by a keyless singleton store",
                    self.plan.surface_label
                ),
                self.plan.span,
            ));
        }
        self.create_identity(&[], fields)
    }

    fn create_identity(
        &self,
        identity: &[SavedKey],
        fields: &[SurfaceCreateField],
    ) -> Result<SurfaceReadRecord, SurfaceError> {
        self.plan
            .commit_create(self.program, self.store, identity, fields)
    }
}

impl<'a> SurfaceDelete<'a> {
    pub fn admit(
        program: &'a CheckedProgram,
        store: &'a TreeStore,
        surface: SurfaceId,
    ) -> Result<Self, SurfaceError> {
        admit_surface_store(program, store)?;
        Ok(Self {
            program,
            store,
            plan: SurfaceDeletePlan::prepare(program, surface)?,
        })
    }

    pub fn admit_by_operation_tag(
        program: &'a CheckedProgram,
        store: &'a TreeStore,
        operation_tag: &str,
    ) -> Result<Self, SurfaceError> {
        let surface = checked_delete_surface_by_tag(program, operation_tag)?;
        admit_surface_store(program, store)?;
        Ok(Self {
            program,
            store,
            plan: SurfaceDeletePlan::prepare(program, surface.id)?,
        })
    }

    pub fn surface(&self) -> SurfaceId {
        self.plan.surface
    }

    pub fn shape(&self) -> SurfaceNodeReadShape {
        self.plan.shape
    }

    pub fn point_identity_shape(&self) -> Result<SurfaceIdentityInputShape, SurfaceError> {
        if self.plan.shape != SurfaceNodeReadShape::Point {
            return Err(request_at(
                format!(
                    "surface `{}` is not backed by a keyed store",
                    self.plan.surface_label
                ),
                self.plan.span,
            ));
        }
        self.plan.record.identity_input_shape()
    }

    pub fn execute(&self, input: SurfaceDeleteInput<'_>) -> Result<(), SurfaceError> {
        match (self.plan.shape, input) {
            (SurfaceNodeReadShape::Singleton, SurfaceDeleteInput::Singleton) => {
                self.delete_identity(&[])
            }
            (SurfaceNodeReadShape::Point, SurfaceDeleteInput::Point { identity }) => {
                self.delete_point(identity)
            }
            (SurfaceNodeReadShape::Singleton, SurfaceDeleteInput::Point { .. }) => Err(request_at(
                format!(
                    "surface `{}` is a singleton delete",
                    self.plan.surface_label
                ),
                self.plan.span,
            )),
            (SurfaceNodeReadShape::Point, SurfaceDeleteInput::Singleton) => Err(request_at(
                format!("surface `{}` requires an identity", self.plan.surface_label),
                self.plan.span,
            )),
        }
    }

    pub fn delete_point(&self, identity: &[SavedKey]) -> Result<(), SurfaceError> {
        if self.plan.shape != SurfaceNodeReadShape::Point {
            return Err(request_at(
                format!(
                    "surface `{}` is not backed by a keyed store",
                    self.plan.surface_label
                ),
                self.plan.span,
            ));
        }
        self.plan.record.validate_identity(identity)?;
        self.delete_identity(identity)
    }

    pub fn delete_singleton(&self) -> Result<(), SurfaceError> {
        if self.plan.shape != SurfaceNodeReadShape::Singleton {
            return Err(request_at(
                format!(
                    "surface `{}` is not backed by a keyless singleton store",
                    self.plan.surface_label
                ),
                self.plan.span,
            ));
        }
        self.delete_identity(&[])
    }

    fn delete_identity(&self, identity: &[SavedKey]) -> Result<(), SurfaceError> {
        self.plan.commit_delete(self.program, self.store, identity)
    }
}

impl<'a> SurfaceCreatePlan<'a> {
    fn prepare(program: &'a CheckedProgram, surface: SurfaceId) -> Result<Self, SurfaceError> {
        let surface = checked_surface(program, surface)?;
        super::require_stable_surface(surface)?;
        if surface.create.is_empty() {
            return Err(request_at(
                format!("surface `{}` declares no create fields", surface.name),
                surface.span,
            ));
        }
        let store = program.facts.store(surface.store);
        let (operation, shape) = backing_node_operation(surface)?;
        let record =
            SurfaceRecordReadPlan::prepare(&program.facts, store, operation, surface.span)?;
        let place = super::checked_saved_root_place(program, &store.root, surface.span)
            .ok_or_else(|| {
                abi_mismatch(
                    format!(
                        "surface `{}` backing store cannot be used for managed writes",
                        surface.name
                    ),
                    surface.span,
                )
            })?;
        let fields = surface_create_members(&program.facts, surface, store)?;
        Ok(Self {
            surface: surface.id,
            surface_label: surface.name.clone(),
            shape,
            accepted_epoch: program.catalog.accepted_epoch,
            source_digest: program.source_digest(),
            place,
            record,
            fields,
            span: surface.span,
        })
    }

    fn commit_create(
        &self,
        program: &CheckedProgram,
        store: &TreeStore,
        identity: &[SavedKey],
        fields: &[SurfaceCreateField],
    ) -> Result<SurfaceReadRecord, SurfaceError> {
        SurfaceWriteCommit::new(
            program,
            store,
            self.span,
            self.accepted_epoch,
            &self.source_digest,
        )
        .run(
            |store| self.require_absent(store, identity),
            |store| self.create_plan(identity, fields, store),
            |store| self.materialize_created(store, identity),
        )
    }

    fn create_plan(
        &self,
        identity: &[SavedKey],
        fields: &[SurfaceCreateField],
        store: &TreeStore,
    ) -> Result<crate::write_plan::WritePlan, SurfaceError> {
        let value = self.resource_value(fields)?;
        plan_resource_write(
            &self.place,
            identity,
            &value,
            store,
            self.record.facts,
            self.span,
        )
        .map_err(map_surface_write_plan_error)
    }

    fn resource_value(&self, fields: &[SurfaceCreateField]) -> Result<ResourceValue, SurfaceError> {
        let mut seen = HashSet::new();
        let mut value = ResourceValue::default();
        for field in fields {
            if !seen.insert(field.catalog_id.clone()) {
                return Err(request_at("surface create field is repeated", self.span));
            }
            let member = self.fields.get(&field.catalog_id).ok_or_else(|| {
                request_at(
                    "surface create field is not declared in the create set",
                    self.span,
                )
            })?;
            let lowered = lower_surface_write_value(
                &field.value,
                &member.value_meaning,
                self.record.facts,
                self.span,
            )?;
            match lowered {
                PlannedSurfaceWriteValue::Leaf(leaf) => {
                    value
                        .fields
                        .push((member_name(&self.place, member.member)?, leaf));
                }
                PlannedSurfaceWriteValue::Identity {
                    keys,
                    referenced_arity,
                } => {
                    value.identities.push(SuppliedIdentity {
                        field: member_name(&self.place, member.member)?,
                        keys,
                        referenced_arity,
                    });
                }
            }
        }
        if seen.len() != self.fields.len() {
            return Err(request_at(
                "surface create body does not include every declared create field",
                self.span,
            ));
        }
        Ok(value)
    }

    fn require_absent(&self, store: &TreeStore, identity: &[SavedKey]) -> Result<(), SurfaceError> {
        if store
            .data_subtree_exists(&self.record.store_catalog_id, identity, &[])
            .map_err(|error| surface_store_error(error, self.span))?
        {
            Err(super::conflict_error(
                "surface record already exists",
                self.span,
            ))
        } else {
            Ok(())
        }
    }

    fn materialize_created(
        &self,
        store: &TreeStore,
        identity: &[SavedKey],
    ) -> Result<SurfaceReadRecord, SurfaceError> {
        let output_identity = match self.shape {
            SurfaceNodeReadShape::Singleton => None,
            SurfaceNodeReadShape::Point => Some(SurfaceReadIdentity {
                store_catalog_id: self.record.store_catalog_id.clone(),
                keys: identity.to_vec(),
            }),
        };
        let mut budget = super::SurfaceMaterializationBudget::new();
        self.record.materialize(
            store,
            identity,
            output_identity,
            MissingRecord::InvalidData,
            &mut budget,
        )
    }
}

impl<'a> SurfaceDeletePlan<'a> {
    fn prepare(program: &'a CheckedProgram, surface: SurfaceId) -> Result<Self, SurfaceError> {
        let surface = checked_surface(program, surface)?;
        super::require_stable_surface(surface)?;
        if surface.delete.is_none() {
            return Err(request_at(
                format!("surface `{}` declares no delete operation", surface.name),
                surface.span,
            ));
        }
        let store = program.facts.store(surface.store);
        let (operation, shape) = backing_node_operation(surface)?;
        let record =
            SurfaceRecordReadPlan::prepare(&program.facts, store, operation, surface.span)?;
        let place = super::checked_saved_root_place(program, &store.root, surface.span)
            .ok_or_else(|| {
                abi_mismatch(
                    format!(
                        "surface `{}` backing store cannot be used for managed writes",
                        surface.name
                    ),
                    surface.span,
                )
            })?;
        Ok(Self {
            surface: surface.id,
            surface_label: surface.name.clone(),
            shape,
            accepted_epoch: program.catalog.accepted_epoch,
            source_digest: program.source_digest(),
            place,
            record,
            span: surface.span,
        })
    }

    fn commit_delete(
        &self,
        program: &CheckedProgram,
        store: &TreeStore,
        identity: &[SavedKey],
    ) -> Result<(), SurfaceError> {
        SurfaceWriteCommit::new(
            program,
            store,
            self.span,
            self.accepted_epoch,
            &self.source_digest,
        )
        .run(
            |store| self.require_present(store, identity),
            |store| {
                plan_resource_delete(&self.place, identity, store, self.record.facts, self.span)
                    .map_err(map_surface_write_plan_error)
            },
            |store| self.require_absent_after_delete(store, identity),
        )
    }

    fn require_present(
        &self,
        store: &TreeStore,
        identity: &[SavedKey],
    ) -> Result<(), SurfaceError> {
        if store
            .data_subtree_exists(&self.record.store_catalog_id, identity, &[])
            .map_err(|error| surface_store_error(error, self.span))?
        {
            Ok(())
        } else {
            Err(super::surface_error_at(
                super::SURFACE_ABSENT,
                "surface record is absent",
                self.span,
            ))
        }
    }

    fn require_absent_after_delete(
        &self,
        store: &TreeStore,
        identity: &[SavedKey],
    ) -> Result<(), SurfaceError> {
        if store
            .data_subtree_exists(&self.record.store_catalog_id, identity, &[])
            .map_err(|error| surface_store_error(error, self.span))?
        {
            Err(super::write_error(
                "surface delete did not remove the record",
                self.span,
            ))
        } else {
            Ok(())
        }
    }
}

fn checked_create_surface_by_tag<'a>(
    program: &'a CheckedProgram,
    operation_tag: &str,
) -> Result<&'a SurfaceFact, SurfaceError> {
    super::require_unique_surface_operation_tag(program, operation_tag)?;
    let mut matched = None;
    for surface in program.facts.surfaces() {
        let Some(descriptor) = SurfaceCreateOperationDescriptor::from_surface(program, surface)
        else {
            continue;
        };
        if descriptor.operation_tag != operation_tag {
            continue;
        }
        if matched.is_some() {
            return Err(abi_mismatch(
                "surface operation tag matches multiple checked create operations",
                surface.span,
            ));
        }
        matched = Some(surface);
    }
    matched.ok_or_else(|| {
        abi_mismatch(
            "surface operation tag is not exported by this checked program",
            SourceSpan::default(),
        )
    })
}

fn checked_delete_surface_by_tag<'a>(
    program: &'a CheckedProgram,
    operation_tag: &str,
) -> Result<&'a SurfaceFact, SurfaceError> {
    super::require_unique_surface_operation_tag(program, operation_tag)?;
    let mut matched = None;
    for surface in program.facts.surfaces() {
        let Some(descriptor) = SurfaceDeleteOperationDescriptor::from_surface(program, surface)
        else {
            continue;
        };
        if descriptor.operation_tag != operation_tag {
            continue;
        }
        if matched.is_some() {
            return Err(abi_mismatch(
                "surface operation tag matches multiple checked delete operations",
                surface.span,
            ));
        }
        matched = Some(surface);
    }
    matched.ok_or_else(|| {
        abi_mismatch(
            "surface operation tag is not exported by this checked program",
            SourceSpan::default(),
        )
    })
}

fn surface_create_members(
    facts: &CheckedFacts,
    surface: &SurfaceFact,
    store: &StoreFact,
) -> Result<HashMap<CatalogId, SurfaceWriteMember>, SurfaceError> {
    let mut fields = HashMap::new();
    for field in &surface.create {
        let member = super::resource_member(facts, field.member, field.span)?;
        if member.resource != store.resource || member.parent.is_some() {
            return Err(abi_mismatch(
                "checked surface create member is outside the backing root fields",
                field.span,
            ));
        }
        let catalog_id = super::catalog_id(&member.catalog_id, "resource member", field.span)?;
        let value_meaning = super::value_meaning_from_member(member, field.span)?;
        if fields
            .insert(
                catalog_id,
                SurfaceWriteMember {
                    member: member.id,
                    value_meaning,
                },
            )
            .is_some()
        {
            return Err(abi_mismatch(
                "checked surface create set repeats a member catalog id",
                field.span,
            ));
        }
    }
    Ok(fields)
}

fn member_name(
    place: &marrow_check::CheckedSavedPlace,
    member: ResourceMemberId,
) -> Result<String, SurfaceError> {
    place
        .root_members
        .iter()
        .find(|candidate| candidate.id == Some(member))
        .map(|candidate| candidate.name.clone())
        .ok_or_else(|| {
            request_at(
                "surface create field is not a root saved field",
                SourceSpan::default(),
            )
        })
}
