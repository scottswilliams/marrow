//! Type-metadata projection: the scratch buffers, the per-pass view, and the
//! borrowed session that answer member, display, and durable-leaf questions about
//! an already-built [`TypeRegistry`](super::TypeRegistry).

use super::*;

/// One durable-validation walk over all resource leaves. These marks are separate
/// from metadata preflight: preflight may visit a generic argument or collection
/// before the durable walk expands that value's body.
struct DurableMetadataScratch {
    pending: VecDeque<(GArg, usize)>,
    expanded_records: Vec<bool>,
    expanded_enums: Vec<bool>,
    expanded_collections: Vec<bool>,
}

impl DurableMetadataScratch {
    fn new(metadata: &MetadataScratch, roots: Vec<GArg>) -> Self {
        Self {
            pending: roots.into_iter().map(|arg| (arg, 0)).collect(),
            expanded_records: vec![false; metadata.records.len()],
            expanded_enums: vec![false; metadata.enums.len()],
            expanded_collections: vec![false; metadata.seen_collections.len()],
        }
    }

    fn first_record(&mut self, id: TypeId) -> Option<bool> {
        let seen = self.expanded_records.get_mut(id.index() as usize)?;
        Some(!std::mem::replace(seen, true))
    }

    fn first_enum(&mut self, id: EnumId) -> Option<bool> {
        let seen = self.expanded_enums.get_mut(id.index() as usize)?;
        Some(!std::mem::replace(seen, true))
    }

    fn first_collection(&mut self, index: CollTypeId) -> Option<bool> {
        let seen = self.expanded_collections.get_mut(index.index() as usize)?;
        Some(!std::mem::replace(seen, true))
    }

    fn push(&mut self, arg: GArg, depth: usize) {
        self.pending.push_back((arg, depth));
    }
}

/// Place one generic instantiation row into the record/enum directory, rejecting a
/// second owner of the same image type identity. Shared by the full directory build
/// and the batch-scoped incremental extension so both classify identity once.
pub(super) fn place_generic_row(
    records: &mut Vec<Option<RecordMetadataOwner>>,
    enums: &mut Vec<Option<EnumMetadataOwner>>,
    row: usize,
    id: TypeInstId,
) -> Result<(), GenericInvariant> {
    #[cfg(test)]
    bump_scaling(|counts| counts.directory_row_visits += 1);
    match id {
        TypeInstId::Record(record_id) => {
            let index = record_id.index() as usize;
            if records.len() <= index {
                records.resize(index + 1, None);
            }
            let slot = &mut records[index];
            if slot.is_some() {
                return Err(GenericInvariant::TypeIdentityCollision(id));
            }
            *slot = Some(RecordMetadataOwner::GenericRow(row));
        }
        TypeInstId::Enum(enum_id) => {
            let index = enum_id.index() as usize;
            if enums.len() <= index {
                enums.resize(index + 1, None);
            }
            let slot = &mut enums[index];
            if slot.is_some() {
                return Err(GenericInvariant::TypeIdentityCollision(id));
            }
            *slot = Some(EnumMetadataOwner::GenericRow(row));
        }
    }
    Ok(())
}

/// The generic row a collection instantiation resolves to for ordering: the latest
/// (highest-row) generic target among the collection's element/key/value arguments and
/// any nested collection already resolved. `index` is the collection's own position, so
/// only strictly earlier child collections are consulted. Shared by the full directory
/// build and the batch-scoped incremental extension.
pub(super) fn collection_generic_target(
    records: &[Option<RecordMetadataOwner>],
    enums: &[Option<EnumMetadataOwner>],
    resolved_targets: &[Option<GenericRowRef>],
    index: usize,
    spec: CollSpec,
) -> Option<GenericRowRef> {
    let direct = |arg: GArg| -> Option<GenericRowRef> {
        match arg {
            GArg::Struct(id) => records
                .get(id.index() as usize)
                .and_then(|owner| match owner {
                    Some(RecordMetadataOwner::GenericRow(row)) => Some(GenericRowRef {
                        row: *row,
                        id: TypeInstId::Record(id),
                    }),
                    Some(
                        RecordMetadataOwner::ResourceRecord(_)
                        | RecordMetadataOwner::DeclaredStruct(_)
                        | RecordMetadataOwner::Group(_, _),
                    )
                    | None => None,
                }),
            GArg::Enum(id) => enums
                .get(id.index() as usize)
                .and_then(|owner| match owner {
                    Some(EnumMetadataOwner::GenericRow(row)) => Some(GenericRowRef {
                        row: *row,
                        id: TypeInstId::Enum(id),
                    }),
                    Some(EnumMetadataOwner::DeclaredEnum(_)) | None => None,
                }),
            GArg::Scalar(_)
            | GArg::Nominal(_)
            | GArg::Group(_)
            | GArg::Collection(_)
            | GArg::Param(_) => None,
        }
    };
    let mut latest: Option<GenericRowRef> = None;
    let mut consider = |candidate: Option<GenericRowRef>| {
        if candidate.is_some_and(|candidate| {
            latest.is_none_or(|current: GenericRowRef| candidate.row > current.row)
        }) {
            latest = candidate;
        }
    };
    let mut consider_arg = |arg: GArg| {
        consider(direct(arg));
        if let GArg::Collection(child) = arg
            && (child.index() as usize) < index
        {
            consider(
                resolved_targets
                    .get(child.index() as usize)
                    .copied()
                    .flatten(),
            );
        }
    };
    match spec {
        CollSpec::List { elem } => consider_arg(elem),
        CollSpec::Map { key, value } => {
            consider_arg(key);
            consider_arg(value);
        }
    }
    latest
}

impl MetadataScratch {
    pub(super) fn try_new(view: &TypeMetadataView<'_>) -> Result<Self, GenericInvariant> {
        #[cfg(test)]
        METADATA_DIRECTORY_BUILDS.with(|count| count.set(count.get() + 1));
        #[cfg(test)]
        bump_scaling(|counts| counts.directory_builds += 1);
        let mut records = Vec::new();
        let mut enums = Vec::new();
        for (record_row, record) in view.registry.records.iter().enumerate() {
            let index = record.type_id.index() as usize;
            if records.len() <= index {
                records.resize(index + 1, None);
            }
            let slot = &mut records[index];
            if slot.is_some() {
                return Err(GenericInvariant::TypeIdentityCollision(TypeInstId::Record(
                    record.type_id,
                )));
            }
            *slot = Some(RecordMetadataOwner::ResourceRecord(record_row));
            for (group_row, group) in record.groups.iter().enumerate() {
                let index = group.type_id.index() as usize;
                if records.len() <= index {
                    records.resize(index + 1, None);
                }
                let slot = &mut records[index];
                if slot.is_some() {
                    return Err(GenericInvariant::TypeIdentityCollision(TypeInstId::Record(
                        group.type_id,
                    )));
                }
                *slot = Some(RecordMetadataOwner::Group(record_row, group_row));
            }
        }
        for (row, info) in view.registry.structs.iter().enumerate() {
            let index = info.type_id.index() as usize;
            if records.len() <= index {
                records.resize(index + 1, None);
            }
            let slot = &mut records[index];
            if slot.is_some() {
                return Err(GenericInvariant::TypeIdentityCollision(TypeInstId::Record(
                    info.type_id,
                )));
            }
            *slot = Some(RecordMetadataOwner::DeclaredStruct(row));
        }
        for (row, info) in view.registry.enums.iter().enumerate() {
            let index = info.enum_id.index() as usize;
            if enums.len() <= index {
                enums.resize(index + 1, None);
            }
            let slot = &mut enums[index];
            if slot.is_some() {
                return Err(GenericInvariant::TypeIdentityCollision(TypeInstId::Enum(
                    info.enum_id,
                )));
            }
            *slot = Some(EnumMetadataOwner::DeclaredEnum(row));
        }
        let mut semantic_keys = HashMap::with_capacity(view.generics.type_insts.len());
        for (row, inst) in view.generics.type_insts.iter().enumerate() {
            place_generic_row(&mut records, &mut enums, row, inst.id)?;
            let key = TypeInstSemanticKey {
                template: inst.template,
                args: &inst.args,
            };
            if let Some(first) = semantic_keys.insert(key, inst.id) {
                return Err(GenericInvariant::TypeInstantiationKeyCollision {
                    first,
                    duplicate: inst.id,
                });
            }
        }
        let mut collection_generic_targets = Vec::with_capacity(view.collections.len());
        for (index, spec) in view.collections.iter().copied().enumerate() {
            let latest = collection_generic_target(
                &records,
                &enums,
                &collection_generic_targets,
                index,
                spec,
            );
            collection_generic_targets.push(latest);
        }
        Ok(Self {
            records,
            enums,
            collection_generic_targets,
            seen_rows: vec![false; view.generics.type_insts.len()],
            seen_collections: vec![false; view.collections.len()],
            tasks: Vec::new(),
        })
    }

    pub(super) fn row(&self, id: TypeInstId) -> Option<usize> {
        match id {
            TypeInstId::Record(id) => {
                self.records
                    .get(id.index() as usize)
                    .and_then(|owner| match owner {
                        Some(RecordMetadataOwner::GenericRow(row)) => Some(*row),
                        Some(
                            RecordMetadataOwner::ResourceRecord(_)
                            | RecordMetadataOwner::DeclaredStruct(_)
                            | RecordMetadataOwner::Group(_, _),
                        )
                        | None => None,
                    })
            }
            TypeInstId::Enum(id) => {
                self.enums
                    .get(id.index() as usize)
                    .and_then(|owner| match owner {
                        Some(EnumMetadataOwner::GenericRow(row)) => Some(*row),
                        Some(EnumMetadataOwner::DeclaredEnum(_)) | None => None,
                    })
            }
        }
    }

    pub(super) fn declared_struct(&self, id: TypeId) -> Option<usize> {
        self.records
            .get(id.index() as usize)
            .and_then(|owner| match owner {
                Some(RecordMetadataOwner::DeclaredStruct(row)) => Some(*row),
                Some(
                    RecordMetadataOwner::ResourceRecord(_)
                    | RecordMetadataOwner::Group(_, _)
                    | RecordMetadataOwner::GenericRow(_),
                )
                | None => None,
            })
    }

    pub(super) fn resource_record(&self, id: TypeId) -> Option<usize> {
        self.records
            .get(id.index() as usize)
            .and_then(|owner| match owner {
                Some(RecordMetadataOwner::ResourceRecord(row)) => Some(*row),
                Some(
                    RecordMetadataOwner::DeclaredStruct(_)
                    | RecordMetadataOwner::Group(_, _)
                    | RecordMetadataOwner::GenericRow(_),
                )
                | None => None,
            })
    }

    pub(super) fn group(&self, id: TypeId) -> Option<(usize, usize)> {
        self.records
            .get(id.index() as usize)
            .and_then(|owner| match owner {
                Some(RecordMetadataOwner::Group(record, group)) => Some((*record, *group)),
                Some(
                    RecordMetadataOwner::ResourceRecord(_)
                    | RecordMetadataOwner::DeclaredStruct(_)
                    | RecordMetadataOwner::GenericRow(_),
                )
                | None => None,
            })
    }

    pub(super) fn declared_enum(&self, id: EnumId) -> Option<usize> {
        self.enums
            .get(id.index() as usize)
            .and_then(|owner| match owner {
                Some(EnumMetadataOwner::DeclaredEnum(row)) => Some(*row),
                Some(EnumMetadataOwner::GenericRow(_)) | None => None,
            })
    }

    fn first_row_visit(&mut self, row: usize) -> bool {
        let seen = &mut self.seen_rows[row];
        if *seen {
            false
        } else {
            *seen = true;
            true
        }
    }

    fn first_collection_visit(&mut self, index: CollTypeId) -> bool {
        let seen = &mut self.seen_collections[index.index() as usize];
        if *seen {
            false
        } else {
            *seen = true;
            true
        }
    }
}

impl TypeMetadataView<'_> {
    fn active_filling_row(&self, index: usize, id: TypeInstId) -> bool {
        let Some(start) = self.generics.fill_batch_start else {
            return false;
        };
        index >= start
            && index < self.generics.type_insts.len()
            && !self.generics.fill_stack.is_empty()
            && self.generics.fill_rows.get(&TypeInstKey::from(id)) == Some(&index)
    }

    pub(super) fn validate_args(
        &self,
        args: &[GArg],
        owner: Option<TypeInstId>,
    ) -> Result<(), GenericInvariant> {
        let mut scratch = MetadataScratch::try_new(self)?;
        self.validate_args_with(args, owner, &mut scratch)
    }

    pub(super) fn validate_args_with(
        &self,
        args: &[GArg],
        owner: Option<TypeInstId>,
        scratch: &mut MetadataScratch,
    ) -> Result<(), GenericInvariant> {
        self.validate_arg_iter_with(args.iter().copied(), owner, scratch)
    }

    fn validate_arg_iter_with<I>(
        &self,
        args: I,
        owner: Option<TypeInstId>,
        scratch: &mut MetadataScratch,
    ) -> Result<(), GenericInvariant>
    where
        I: DoubleEndedIterator<Item = GArg>,
    {
        // Profiles cannot disagree: the drain loop below empties `tasks` on every path
        // that returns `Ok`, and a `?` leaves entries behind only after an invariant that
        // has already ended the compile.
        debug_assert!(scratch.tasks.is_empty());
        let generic_parent = match owner {
            Some(id) => {
                let row = scratch
                    .row(id)
                    .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
                scratch.first_row_visit(row);
                Some(row)
            }
            None => None,
        };
        for arg in args.rev() {
            scratch.tasks.push(MetadataTask::Argument {
                arg,
                collection_parent: None,
                generic_parent,
            });
        }

        while let Some(task) = scratch.tasks.pop() {
            match task {
                MetadataTask::Argument {
                    arg,
                    collection_parent,
                    generic_parent,
                } => match arg {
                    GArg::Scalar(_) => {}
                    GArg::Nominal(id) => {
                        if self.registry.nominals.get(id.0 as usize).is_none() {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        }
                    }
                    GArg::Struct(id) => {
                        if scratch.declared_struct(id).is_some() {
                            continue;
                        }
                        self.queue_generic_target(
                            TypeInstId::Record(id),
                            arg,
                            generic_parent,
                            scratch,
                        )?;
                    }
                    GArg::Group(id) => {
                        if scratch.group(id).is_none() {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        }
                    }
                    GArg::Enum(id) => {
                        if scratch.declared_enum(id).is_some() {
                            continue;
                        }
                        self.queue_generic_target(
                            TypeInstId::Enum(id),
                            arg,
                            generic_parent,
                            scratch,
                        )?;
                    }
                    GArg::Collection(index) => {
                        if collection_parent.is_some_and(|parent| index >= parent) {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        }
                        let Some(spec) = self.collections.get(index.index() as usize).copied()
                        else {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        };
                        if !scratch.first_collection_visit(index) {
                            self.validate_revisited_collection_order(
                                index,
                                generic_parent,
                                scratch,
                            )?;
                            continue;
                        }
                        match spec {
                            CollSpec::List { elem } => scratch.tasks.push(MetadataTask::Argument {
                                arg: elem,
                                collection_parent: Some(index),
                                generic_parent,
                            }),
                            CollSpec::Map { key, value } => {
                                scratch.tasks.push(MetadataTask::Argument {
                                    arg: value,
                                    collection_parent: Some(index),
                                    generic_parent,
                                });
                                scratch.tasks.push(MetadataTask::Argument {
                                    arg: key,
                                    collection_parent: Some(index),
                                    generic_parent,
                                });
                            }
                        }
                    }
                    GArg::Param(index) => {
                        if self.generics.argument_domain != ArgumentDomain::TemplateProof {
                            return Err(GenericInvariant::TypeArgumentParameter(index));
                        }
                    }
                },
                MetadataTask::ReadyBody { row } => {
                    let inst = &self.generics.type_insts[row];
                    let TypeInstState::Ready(body) = &inst.state else {
                        return Err(GenericInvariant::ReadyBodyMissing(inst.id));
                    };
                    self.registry.validate_inst_body_metadata(
                        inst.template,
                        &inst.args,
                        inst.id,
                        body,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn validate_revisited_collection_order(
        &self,
        index: CollTypeId,
        generic_parent: Option<usize>,
        scratch: &MetadataScratch,
    ) -> Result<(), GenericInvariant> {
        let Some(parent) = generic_parent else {
            return Ok(());
        };
        let Some(summary) = scratch
            .collection_generic_targets
            .get(index.index() as usize)
            .copied()
            .flatten()
        else {
            return Ok(());
        };
        if summary.row < parent {
            return Ok(());
        }

        let mut pending = Vec::new();
        let mut seen = vec![false; self.collections.len()];
        pending.push(GArg::Collection(index));
        while let Some(arg) = pending.pop() {
            match arg {
                GArg::Struct(id) => {
                    if let Some(row) = scratch.row(TypeInstId::Record(id))
                        && row >= parent
                    {
                        return Err(GenericInvariant::TypeArgumentOrderViolation {
                            owner: self.generics.type_insts[parent].id,
                            target: TypeInstId::Record(id),
                        });
                    }
                }
                GArg::Enum(id) => {
                    if let Some(row) = scratch.row(TypeInstId::Enum(id))
                        && row >= parent
                    {
                        return Err(GenericInvariant::TypeArgumentOrderViolation {
                            owner: self.generics.type_insts[parent].id,
                            target: TypeInstId::Enum(id),
                        });
                    }
                }
                GArg::Collection(child) => {
                    let Some(child_summary) = scratch
                        .collection_generic_targets
                        .get(child.index() as usize)
                        .copied()
                        .flatten()
                    else {
                        continue;
                    };
                    if child_summary.row < parent {
                        continue;
                    }
                    let Some(mark) = seen.get_mut(child.index() as usize) else {
                        continue;
                    };
                    if std::mem::replace(mark, true) {
                        continue;
                    }
                    match self.collections[child.index() as usize] {
                        CollSpec::List { elem } => pending.push(elem),
                        CollSpec::Map { key, value } => {
                            pending.push(value);
                            pending.push(key);
                        }
                    }
                }
                GArg::Scalar(_) | GArg::Nominal(_) | GArg::Group(_) | GArg::Param(_) => {}
            }
        }
        Err(GenericInvariant::TypeArgumentOrderViolation {
            owner: self.generics.type_insts[parent].id,
            target: summary.id,
        })
    }

    fn queue_generic_target(
        &self,
        id: TypeInstId,
        arg: GArg,
        generic_parent: Option<usize>,
        scratch: &mut MetadataScratch,
    ) -> Result<(), GenericInvariant> {
        let Some(index) = scratch.row(id) else {
            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
        };
        if let Some(parent) = generic_parent
            && index >= parent
        {
            return Err(GenericInvariant::TypeArgumentOrderViolation {
                owner: self.generics.type_insts[parent].id,
                target: id,
            });
        }
        let inst = &self.generics.type_insts[index];
        match &inst.state {
            TypeInstState::Ready(_) => {
                self.registry.template_for_args(inst.template, &inst.args)?;
                if !scratch.first_row_visit(index) {
                    return Ok(());
                }
                scratch.tasks.push(MetadataTask::ReadyBody { row: index });
                for &nested in inst.args.iter().rev() {
                    scratch.tasks.push(MetadataTask::Argument {
                        arg: nested,
                        collection_parent: None,
                        generic_parent: Some(index),
                    });
                }
                Ok(())
            }
            TypeInstState::Filling { .. } if self.active_filling_row(index, id) => Ok(()),
            TypeInstState::Filling { .. } | TypeInstState::Rejected(_) => {
                Err(GenericInvariant::ReadyBodyMissing(id))
            }
        }
    }

    pub(super) fn ready_inst_header_with<'a>(
        &'a self,
        inst: &'a TypeInst,
        scratch: &mut MetadataScratch,
    ) -> Result<Option<&'a InstBody>, GenericInvariant> {
        let TypeInstState::Ready(body) = &inst.state else {
            return Ok(None);
        };
        let index = scratch
            .row(inst.id)
            .ok_or(GenericInvariant::ReadyBodyMissing(inst.id))?;
        self.registry.template_for_args(inst.template, &inst.args)?;
        self.validate_args_with(&inst.args, Some(inst.id), scratch)?;
        self.registry
            .validate_inst_body_metadata(inst.template, &inst.args, inst.id, body)?;
        // Profiles cannot disagree: nothing here branches on the flag. The
        // `validate_args_with` call above visits this row, and this restates that
        // postcondition beside the `Ok` it returns either way.
        debug_assert!(scratch.seen_rows[index]);
        Ok(Some(body))
    }

    pub(super) fn ready_inst_body_with<'a>(
        &'a self,
        inst: &'a TypeInst,
        scratch: &mut MetadataScratch,
    ) -> Result<Option<&'a InstBody>, GenericInvariant> {
        let Some(body) = self.ready_inst_header_with(inst, scratch)? else {
            return Ok(None);
        };
        self.validate_ready_body_with(inst, body, scratch)?;
        Ok(Some(body))
    }

    pub(super) fn validate_ready_body_with(
        &self,
        inst: &TypeInst,
        body: &InstBody,
        scratch: &mut MetadataScratch,
    ) -> Result<(), GenericInvariant> {
        self.validate_ready_body_shape(inst, body, scratch)?;
        match body {
            InstBody::Struct(fields) => {
                self.validate_arg_iter_with(fields.iter().map(|(_, arg)| *arg), None, scratch)?
            }
            InstBody::Enum(variants) => self.validate_arg_iter_with(
                variants
                    .iter()
                    .flat_map(|variant| variant.payload.iter().map(|(_, arg)| *arg)),
                None,
                scratch,
            )?,
        }
        Ok(())
    }

    fn ready_struct_field_with(
        &self,
        inst: &TypeInst,
        name: &str,
        scratch: &mut MetadataScratch,
    ) -> Result<StructFieldProjection, GenericInvariant> {
        let Some(body) = self.ready_inst_header_with(inst, scratch)? else {
            return Ok(StructFieldProjection::Absent);
        };
        self.validate_ready_body_shape(inst, body, scratch)?;
        let InstBody::Struct(fields) = body else {
            return Err(GenericInvariant::TypeBodyKindMismatch {
                id: inst.id,
                body: body.kind(),
            });
        };
        let Some((index, (_, ty))) = fields
            .iter()
            .enumerate()
            .find(|(_, (field_name, _))| field_name == name)
        else {
            return Ok(StructFieldProjection::Missing);
        };
        self.validate_args_with(std::slice::from_ref(ty), None, scratch)?;
        Ok(StructFieldProjection::Field {
            index: index as u16,
            ty: *ty,
        })
    }

    fn validate_ready_body_shape(
        &self,
        inst: &TypeInst,
        body: &InstBody,
        scratch: &MetadataScratch,
    ) -> Result<(), GenericInvariant> {
        let template = self.registry.template_for_args(inst.template, &inst.args)?;
        let mismatch = || GenericInvariant::ReadyBodyShapeMismatch(inst.id);
        let mut param_indices = HashMap::with_capacity(template.type_params.len());
        for (index, (name, _)) in template.type_params.iter().enumerate() {
            param_indices.entry(name.as_str()).or_insert(index);
        }
        match (&template.body, body) {
            (TemplateBody::Struct(expected), InstBody::Struct(actual)) => {
                if expected.len() != actual.len() {
                    return Err(mismatch());
                }
                for ((expected_name, expected_ty), (actual_name, actual_arg)) in
                    expected.iter().zip(actual)
                {
                    if expected_name != actual_name
                        || !self.ready_body_arg_matches(
                            expected_ty,
                            *actual_arg,
                            &inst.args,
                            &param_indices,
                            scratch,
                        )?
                    {
                        return Err(mismatch());
                    }
                }
            }
            (TemplateBody::Enum(expected), InstBody::Enum(actual)) => {
                if expected.len() != actual.len() {
                    return Err(mismatch());
                }
                for (expected_variant, actual_variant) in expected.iter().zip(actual) {
                    if expected_variant.name != actual_variant.name
                        || expected_variant.payload.len() != actual_variant.payload.len()
                    {
                        return Err(mismatch());
                    }
                    for (expected_field, (actual_name, actual_arg)) in
                        expected_variant.payload.iter().zip(&actual_variant.payload)
                    {
                        if expected_field.name != *actual_name
                            || !self.ready_body_arg_matches(
                                &expected_field.ty,
                                *actual_arg,
                                &inst.args,
                                &param_indices,
                                scratch,
                            )?
                        {
                            return Err(mismatch());
                        }
                    }
                }
            }
            (TemplateBody::Struct(_), InstBody::Enum(_))
            | (TemplateBody::Enum(_), InstBody::Struct(_)) => return Err(mismatch()),
        }
        Ok(())
    }

    fn ready_body_arg_matches<'a>(
        &'a self,
        expected: &'a TypeExpr,
        actual: GArg,
        args: &[GArg],
        param_indices: &HashMap<&str, usize>,
        scratch: &MetadataScratch,
    ) -> Result<bool, GenericInvariant> {
        let mut pending: Vec<(&TypeExpr, GArg)> = vec![(expected, actual)];
        while let Some((expected, actual)) = pending.pop() {
            #[cfg(test)]
            READY_BODY_MATCH_VISITS.with(|count| count.set(count.get() + 1));
            match expected {
                TypeExpr::Name { text, .. } => {
                    if let Some(expanded) = self.registry.aliases.get(text) {
                        pending.push((expanded, actual));
                        continue;
                    }
                    if let Some(&index) = param_indices.get(text.as_str()) {
                        if args.get(index).copied() != Some(actual) {
                            return Ok(false);
                        }
                        continue;
                    }
                    if let Some(scalar) = ScalarType::from_spelling(text) {
                        if actual != GArg::Scalar(scalar) {
                            return Ok(false);
                        }
                        continue;
                    }
                    let matches = match actual {
                        GArg::Nominal(id) => self
                            .registry
                            .nominals
                            .get(id.0 as usize)
                            .is_some_and(|info| info.name.as_str() == text.as_str()),
                        GArg::Struct(id) => scratch.declared_struct(id).is_some_and(|row| {
                            self.registry.structs[row].name.as_str() == text.as_str()
                        }),
                        GArg::Enum(id) => scratch.declared_enum(id).is_some_and(|row| {
                            self.registry.enums[row].name.as_str() == text.as_str()
                        }),
                        GArg::Scalar(_) | GArg::Group(_) | GArg::Collection(_) | GArg::Param(_) => {
                            false
                        }
                    };
                    if !matches {
                        return Ok(false);
                    }
                }
                TypeExpr::Apply {
                    head, args: nested, ..
                } if head == "List" => {
                    let [expected_elem] = nested.as_slice() else {
                        return Ok(false);
                    };
                    let GArg::Collection(index) = actual else {
                        return Ok(false);
                    };
                    let Some(CollSpec::List { elem }) =
                        self.collections.get(index.index() as usize).copied()
                    else {
                        return Ok(false);
                    };
                    pending.push((expected_elem, elem));
                }
                TypeExpr::Apply {
                    head, args: nested, ..
                } if head == "Map" => {
                    let [expected_key, expected_value] = nested.as_slice() else {
                        return Ok(false);
                    };
                    let GArg::Collection(index) = actual else {
                        return Ok(false);
                    };
                    let Some(CollSpec::Map { key, value }) =
                        self.collections.get(index.index() as usize).copied()
                    else {
                        return Ok(false);
                    };
                    pending.push((expected_value, value));
                    pending.push((expected_key, key));
                }
                TypeExpr::Apply {
                    head, args: nested, ..
                } => {
                    let id = match actual {
                        GArg::Struct(id) => TypeInstId::Record(id),
                        GArg::Enum(id) => TypeInstId::Enum(id),
                        GArg::Scalar(_)
                        | GArg::Nominal(_)
                        | GArg::Group(_)
                        | GArg::Collection(_)
                        | GArg::Param(_) => return Ok(false),
                    };
                    let Some(row) = scratch.row(id) else {
                        return Ok(false);
                    };
                    let nested_inst = &self.generics.type_insts[row];
                    let nested_template = self
                        .registry
                        .template_for_args(nested_inst.template, &nested_inst.args)?;
                    let expected_kind = id.kind();
                    let actual_kind = nested_template.body.kind();
                    if expected_kind != actual_kind {
                        return Err(GenericInvariant::TemplateKindMismatch {
                            template: nested_inst.template,
                            expected: actual_kind,
                            actual: expected_kind,
                        });
                    }
                    if nested_template.name.as_str() != head.as_str()
                        || nested.len() != nested_inst.args.len()
                    {
                        return Ok(false);
                    }
                    for (expected, actual) in nested.iter().zip(&nested_inst.args).rev() {
                        pending.push((expected, *actual));
                    }
                }
                TypeExpr::Optional { .. } | TypeExpr::Identity(_) | TypeExpr::Incomplete { .. } => {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    pub(super) fn ready_inst_header_by_id<'a>(
        &'a self,
        id: TypeInstId,
        scratch: &mut MetadataScratch,
    ) -> Result<Option<(&'a TypeInst, &'a InstBody)>, GenericInvariant> {
        let Some(row) = scratch.row(id) else {
            return Ok(None);
        };
        let inst = self
            .generics
            .type_insts
            .get(row)
            .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
        self.ready_inst_header_with(inst, scratch)
            .map(|body| body.map(|body| (inst, body)))
    }

    pub(super) fn ready_inst_by_id<'a>(
        &'a self,
        id: TypeInstId,
        scratch: &mut MetadataScratch,
    ) -> Result<Option<(&'a TypeInst, &'a InstBody)>, GenericInvariant> {
        let Some(row) = scratch.row(id) else {
            return Ok(None);
        };
        let inst = self
            .generics
            .type_insts
            .get(row)
            .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
        self.ready_inst_body_with(inst, scratch)
            .map(|body| body.map(|body| (inst, body)))
    }
}

impl TypeMetadataSession<'_> {
    fn ensure_healthy(&self) -> Result<(), GenericInvariant> {
        match self.failure {
            Some(invariant) => Err(invariant),
            None => Ok(()),
        }
    }

    fn remember<T>(&mut self, result: Result<T, GenericInvariant>) -> Result<T, GenericInvariant> {
        if let Err(invariant) = result
            && self.failure.is_none()
        {
            self.failure = Some(invariant);
        }
        result
    }

    pub(crate) fn validate_type_arguments(
        &mut self,
        args: &[GArg],
    ) -> Result<(), GenericInvariant> {
        self.ensure_healthy()?;
        let result = self.view.validate_args_with(args, None, &mut self.metadata);
        self.remember(result)
    }

    pub(crate) fn static_record_by_name(
        &mut self,
        name: &str,
    ) -> Result<Option<RecordInfo>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let Some(info) = self
                .view
                .registry
                .records
                .iter()
                .find(|info| info.name == name)
            else {
                return Ok(None);
            };
            let args = info
                .fields
                .iter()
                .chain(info.groups.iter().flat_map(|group| group.fields.iter()))
                .map(|field| field.ty)
                .collect::<Vec<_>>();
            self.view
                .validate_args_with(&args, None, &mut self.metadata)?;
            Ok(Some(info.clone()))
        })();
        self.remember(result)
    }

    pub(crate) fn static_group_by_name(
        &mut self,
        record: &str,
        group: &str,
    ) -> Result<Option<GroupInfo>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let Some(info) = self
                .view
                .registry
                .records
                .iter()
                .find(|info| info.name == record)
                .and_then(|info| info.groups.iter().find(|info| info.name == group))
            else {
                return Ok(None);
            };
            let args = info.fields.iter().map(|field| field.ty).collect::<Vec<_>>();
            self.view
                .validate_args_with(&args, None, &mut self.metadata)?;
            Ok(Some(info.clone()))
        })();
        self.remember(result)
    }

    pub(crate) fn static_struct_by_name(
        &mut self,
        name: &str,
    ) -> Result<Option<StructInfo>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let Some(info) = self.view.registry.struct_by_name(name) else {
                return Ok(None);
            };
            let args = info.fields.iter().map(|field| field.ty).collect::<Vec<_>>();
            self.view
                .validate_args_with(&args, None, &mut self.metadata)?;
            Ok(Some(info.clone()))
        })();
        self.remember(result)
    }

    pub(crate) fn static_enum_by_name(
        &mut self,
        name: &str,
    ) -> Result<Option<EnumInfo>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = Ok(self.view.registry.enum_by_name(name).cloned());
        self.remember(result)
    }

    pub(crate) fn static_named_type(
        &mut self,
        name: &str,
    ) -> Result<Option<StaticNamedType>, GenericInvariant> {
        self.ensure_healthy()?;
        let registry = self.view.registry;
        let result = Ok(if let Some(info) = registry.struct_by_name(name) {
            Some(StaticNamedType::Struct(info.type_id))
        } else if let Some(info) = registry.enum_by_name(name) {
            Some(StaticNamedType::Enum(info.enum_id))
        } else {
            registry
                .by_name(name)
                .map(|info| StaticNamedType::Record(info.type_id))
        });
        self.remember(result)
    }

    pub(crate) fn product_field(
        &mut self,
        ty: TypeId,
        name: &str,
    ) -> Result<ProductFieldProjection, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let Some(owner) = self
                .metadata
                .records
                .get(ty.index() as usize)
                .copied()
                .flatten()
            else {
                return Ok(ProductFieldProjection::Absent);
            };
            match owner {
                RecordMetadataOwner::ResourceRecord(record) => {
                    let info = &self.view.registry.records[record];
                    if let Some((index, field)) = info.field(name) {
                        self.view.validate_args_with(
                            std::slice::from_ref(&field.ty),
                            None,
                            &mut self.metadata,
                        )?;
                        return Ok(ProductFieldProjection::Field {
                            index,
                            ty: field.ty,
                            required: field.required,
                        });
                    }
                    if let Some((index, group)) = info.group(name) {
                        return Ok(ProductFieldProjection::Group {
                            index,
                            ty: group.type_id,
                        });
                    }
                    Ok(match self.view.registry.member(&info.name, name)? {
                        Binding::Refused(id, _) => ProductFieldProjection::RefusedMember(id),
                        Binding::Accepted(_) | Binding::Absent => {
                            ProductFieldProjection::MissingRecordField
                        }
                    })
                }
                RecordMetadataOwner::Group(record, group) => {
                    let owner = &self.view.registry.records[record];
                    let info = &owner.groups[group];
                    let Some((index, field)) = info.field(name) else {
                        let anchor = format!("{}.{}", owner.name, info.name);
                        return Ok(match self.view.registry.member(&anchor, name)? {
                            Binding::Refused(id, _) => ProductFieldProjection::RefusedMember(id),
                            Binding::Accepted(_) | Binding::Absent => {
                                ProductFieldProjection::MissingGroupField
                            }
                        });
                    };
                    self.view.validate_args_with(
                        std::slice::from_ref(&field.ty),
                        None,
                        &mut self.metadata,
                    )?;
                    Ok(ProductFieldProjection::Field {
                        index,
                        ty: field.ty,
                        required: field.required,
                    })
                }
                RecordMetadataOwner::DeclaredStruct(_) | RecordMetadataOwner::GenericRow(_) => {
                    Ok(ProductFieldProjection::Absent)
                }
            }
        })();
        self.remember(result)
    }

    pub(crate) fn struct_field(
        &mut self,
        ty: TypeId,
        name: &str,
    ) -> Result<StructFieldProjection, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let Some(owner) = self
                .metadata
                .records
                .get(ty.index() as usize)
                .copied()
                .flatten()
            else {
                return Ok(StructFieldProjection::Absent);
            };
            match owner {
                RecordMetadataOwner::DeclaredStruct(row) => {
                    let info = &self.view.registry.structs[row];
                    let Some((index, field)) = info.field(name) else {
                        return Ok(StructFieldProjection::Missing);
                    };
                    self.view.validate_args_with(
                        std::slice::from_ref(&field.ty),
                        None,
                        &mut self.metadata,
                    )?;
                    Ok(StructFieldProjection::Field {
                        index,
                        ty: field.ty,
                    })
                }
                RecordMetadataOwner::GenericRow(row) => self.view.ready_struct_field_with(
                    &self.view.generics.type_insts[row],
                    name,
                    &mut self.metadata,
                ),
                RecordMetadataOwner::ResourceRecord(_) | RecordMetadataOwner::Group(_, _) => {
                    Ok(StructFieldProjection::Absent)
                }
            }
        })();
        self.remember(result)
    }

    pub(crate) fn instantiation_of(
        &mut self,
        id: TypeInstId,
    ) -> Result<Option<(usize, Vec<GArg>)>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let Some((inst, _)) = self.view.ready_inst_header_by_id(id, &mut self.metadata)? else {
                return Ok(None);
            };
            Ok(Some((inst.template, inst.args.clone())))
        })();
        self.remember(result)
    }

    pub(crate) fn collection_spec(
        &mut self,
        index: CollTypeId,
    ) -> Result<CollSpec, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let arg = GArg::Collection(index);
            self.view
                .validate_args_with(std::slice::from_ref(&arg), None, &mut self.metadata)?;
            self.view
                .collections
                .get(index.index() as usize)
                .copied()
                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))
        })();
        self.remember(result)
    }

    pub(crate) fn reserved_instantiation(
        &mut self,
        id: EnumId,
    ) -> Result<Option<ReservedEnumArgs>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            let Some((inst, body)) = self
                .view
                .ready_inst_by_id(TypeInstId::Enum(id), &mut self.metadata)?
            else {
                return Ok(None);
            };
            match self.view.registry.type_templates[inst.template].reserved {
                Some(Reserved::Option) => {
                    let [inner] = inst.args.as_slice() else {
                        return Err(GenericInvariant::TypeArgumentCountMismatch {
                            template: inst.template,
                            expected: 1,
                            actual: inst.args.len(),
                        });
                    };
                    let InstBody::Enum(variants) = body else {
                        return Err(GenericInvariant::TypeBodyKindMismatch {
                            id: inst.id,
                            body: body.kind(),
                        });
                    };
                    let exact = variants.len() == 2
                        && variants[OPTION_NONE as usize].name == "none"
                        && variants[OPTION_NONE as usize].payload.is_empty()
                        && variants[OPTION_SOME as usize].name == "some"
                        && variants[OPTION_SOME as usize].payload.len() == 1
                        && variants[OPTION_SOME as usize].payload[0].0 == "value"
                        && variants[OPTION_SOME as usize].payload[0].1 == *inner;
                    if !exact {
                        return Err(GenericInvariant::ReadyBodyShapeMismatch(inst.id));
                    }
                    Ok(Some(ReservedEnumArgs::Option(*inner)))
                }
                Some(Reserved::Result) => {
                    let [ok, err] = inst.args.as_slice() else {
                        return Err(GenericInvariant::TypeArgumentCountMismatch {
                            template: inst.template,
                            expected: 2,
                            actual: inst.args.len(),
                        });
                    };
                    let InstBody::Enum(variants) = body else {
                        return Err(GenericInvariant::TypeBodyKindMismatch {
                            id: inst.id,
                            body: body.kind(),
                        });
                    };
                    let exact = variants.len() == 2
                        && variants[RESULT_OK as usize].name == "ok"
                        && variants[RESULT_OK as usize].payload.len() == 1
                        && variants[RESULT_OK as usize].payload[0].0 == "value"
                        && variants[RESULT_OK as usize].payload[0].1 == *ok
                        && variants[RESULT_ERR as usize].name == "err"
                        && variants[RESULT_ERR as usize].payload.len() == 1
                        && variants[RESULT_ERR as usize].payload[0].0 == "value"
                        && variants[RESULT_ERR as usize].payload[0].1 == *err;
                    if !exact {
                        return Err(GenericInvariant::ReadyBodyShapeMismatch(inst.id));
                    }
                    Ok(Some(ReservedEnumArgs::Result(*ok, *err)))
                }
                None => Ok(Some(ReservedEnumArgs::Other)),
            }
        })();
        self.remember(result)
    }

    pub(crate) fn garg_spelling(&mut self, arg: GArg) -> Result<String, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            self.view
                .validate_args_with(std::slice::from_ref(&arg), None, &mut self.metadata)?;
            garg_spelling_validated(
                self.view.registry,
                &self.view,
                &self.metadata,
                arg,
                &mut self.display,
            )
        })();
        self.remember(result)
    }

    pub(crate) fn durable_enum_shape_and_anchor(
        &mut self,
        id: EnumId,
    ) -> Result<Option<(ResolvedEnumVariants, String)>, GenericInvariant> {
        self.ensure_healthy()?;
        let result = (|| {
            if let Some(info) = self.view.registry.enum_by_id(id) {
                let variants = info
                    .variants
                    .iter()
                    .map(|variant| {
                        (
                            variant.name.clone(),
                            variant
                                .payload
                                .iter()
                                .map(|field| GArg::Scalar(field.scalar))
                                .collect(),
                        )
                    })
                    .collect();
                return Ok(Some((variants, info.name.clone())));
            }
            let inst_id = TypeInstId::Enum(id);
            let Some((_, body)) = self.view.ready_inst_by_id(inst_id, &mut self.metadata)? else {
                return Ok(None);
            };
            let InstBody::Enum(variants) = body else {
                return Err(GenericInvariant::TypeBodyKindMismatch {
                    id: inst_id,
                    body: TypeInstKind::Struct,
                });
            };
            let variants = variants
                .iter()
                .map(|variant| {
                    (
                        variant.name.clone(),
                        variant.payload.iter().map(|(_, arg)| *arg).collect(),
                    )
                })
                .collect();
            let spelling = self
                .view
                .registry
                .inst_anchor_spelling_validated(
                    &self.view,
                    &self.metadata,
                    inst_id,
                    &mut self.display,
                )?
                .ok_or(GenericInvariant::ReadyBodyMissing(inst_id))?;
            Ok(Some((variants, spelling)))
        })();
        self.remember(result)
    }

    pub(crate) fn validate_durable_value_metadata(
        &mut self,
        roots: impl IntoIterator<Item = GArg>,
    ) -> Result<(), GenericInvariant> {
        self.ensure_healthy()?;
        let roots: Vec<GArg> = roots.into_iter().collect();
        let result = (|| {
            self.view
                .validate_args_with(&roots, None, &mut self.metadata)?;
            let mut durable = DurableMetadataScratch::new(&self.metadata, roots);

            while let Some((arg, depth)) = durable.pending.pop_front() {
                if depth > marrow_image::bounds::MAX_DURABLE_VALUE_DEPTH {
                    continue;
                }
                self.view.validate_args_with(
                    std::slice::from_ref(&arg),
                    None,
                    &mut self.metadata,
                )?;
                let next_depth = depth + 1;
                match arg {
                    GArg::Struct(id) => {
                        let Some(first) = durable.first_record(id) else {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        };
                        if !first {
                            continue;
                        }
                        if let Some(row) = self.metadata.declared_struct(id) {
                            let Some(info) = self.view.registry.structs.get(row) else {
                                return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                            };
                            for field in &info.fields {
                                durable.push(field.ty, next_depth);
                            }
                            continue;
                        }
                        let Some(row) = self.metadata.row(TypeInstId::Record(id)) else {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        };
                        let inst = &self.view.generics.type_insts[row];
                        let Some(body) =
                            self.view.ready_inst_body_with(inst, &mut self.metadata)?
                        else {
                            return Err(GenericInvariant::ReadyBodyMissing(inst.id));
                        };
                        for &nested in &inst.args {
                            durable.push(nested, next_depth);
                        }
                        let InstBody::Struct(fields) = body else {
                            return Err(GenericInvariant::TypeBodyKindMismatch {
                                id: inst.id,
                                body: body.kind(),
                            });
                        };
                        for (_, field) in fields {
                            durable.push(*field, next_depth);
                        }
                    }
                    GArg::Enum(id) => {
                        let Some(first) = durable.first_enum(id) else {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        };
                        if !first || self.metadata.declared_enum(id).is_some() {
                            continue;
                        }
                        let Some(row) = self.metadata.row(TypeInstId::Enum(id)) else {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        };
                        let inst = &self.view.generics.type_insts[row];
                        let Some(body) =
                            self.view.ready_inst_body_with(inst, &mut self.metadata)?
                        else {
                            return Err(GenericInvariant::ReadyBodyMissing(inst.id));
                        };
                        for &nested in &inst.args {
                            durable.push(nested, next_depth);
                        }
                        let InstBody::Enum(variants) = body else {
                            return Err(GenericInvariant::TypeBodyKindMismatch {
                                id: inst.id,
                                body: body.kind(),
                            });
                        };
                        for variant in variants {
                            for (_, field) in &variant.payload {
                                durable.push(*field, next_depth);
                            }
                        }
                    }
                    GArg::Collection(index) => {
                        let Some(first) = durable.first_collection(index) else {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        };
                        if !first {
                            continue;
                        }
                        let Some(spec) = self.view.collections.get(index.index() as usize).copied()
                        else {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        };
                        match spec {
                            CollSpec::List { elem } => durable.push(elem, next_depth),
                            CollSpec::Map { key, value } => {
                                durable.push(key, next_depth);
                                durable.push(value, next_depth);
                            }
                        }
                    }
                    GArg::Scalar(_) | GArg::Nominal(_) | GArg::Group(_) => {}
                    GArg::Param(index) => {
                        return Err(GenericInvariant::TypeArgumentParameter(index));
                    }
                }
            }
            Ok(())
        })();
        self.remember(result)
    }
}

/// The image-identity directory of the monomorphization pass, plus the image-order
/// watermarks it has already classified. Extended in place as rows and collections are
/// appended; it holds identity mapping and per-walk marks, not argument keys.
pub(super) struct RowDirectory {
    scratch: MetadataScratch,
    pub(super) declared: DeclaredCounts,
    pub(super) built_type_insts: usize,
    pub(super) built_collections: usize,
}

/// The declared-type population a directory classified. DeclarationSite records (with their
/// groups), structs, and enums, and the groups of each record, are all fixed once
/// monomorphization begins — the declare phase completes before the first mint — so in
/// the production pipeline incremental extension only appends generic rows and
/// collections, and this length triple is a complete change-detector: a differing count
/// forces a rebuild. It is kept O(1) rather than summing group counts per probe so the
/// reuse check adds no per-mint factor in the declared-type count. A test that mutates a
/// committed declared type out of that append order reclassifies via
/// `invalidate_row_directory`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct DeclaredCounts {
    records: usize,
    structs: usize,
    enums: usize,
}

impl DeclaredCounts {
    pub(super) fn of(registry: &TypeRegistry) -> Self {
        Self {
            records: registry.records.len(),
            structs: registry.structs.len(),
            enums: registry.enums.len(),
        }
    }
}

impl RowDirectory {
    /// A directory classifying every currently declared and instantiated row, with its
    /// watermarks set to the current image lengths. Used to seed the cache.
    pub(super) fn build_full(view: &TypeMetadataView<'_>) -> Result<Self, GenericInvariant> {
        Ok(Self {
            scratch: MetadataScratch::try_new(view)?,
            declared: DeclaredCounts::of(view.registry),
            built_type_insts: view.generics.type_insts.len(),
            built_collections: view.collections.len(),
        })
    }

    /// Classify the type instantiations and collections appended since the last build,
    /// extending the directory in image order. Rows below the watermark were classified
    /// on a prior probe and are not revisited. A `(template, args)` semantic-key collision
    /// cannot arise on an appended row (mint dedup admits only a fresh key), so extension
    /// checks only image-identity placement; the full `try_new` semantic-key scan still
    /// runs on every cold or invalidated build and on every unrouted projection path.
    pub(super) fn extend(&mut self, view: &TypeMetadataView<'_>) -> Result<(), GenericInvariant> {
        // Atomic. A placement can fail on an identity collision, and it resizes the owner
        // before it does, so a failed extension would otherwise leave scratch rows the
        // watermark does not account for — a directory claiming a classification it does
        // not hold. The captured lengths are the inverse of exactly the appends below, and
        // running it here is what lets an admitted cache survive a failed probe instead of
        // being dropped with the classification it already paid for.
        let placed_records = self.scratch.records.len();
        let placed_enums = self.scratch.enums.len();
        let prior_type_insts = self.built_type_insts;
        let prior_collections = self.built_collections;
        match self.extend_appended(view) {
            Ok(()) => Ok(()),
            Err(invariant) => {
                self.rewind_to(
                    placed_records,
                    placed_enums,
                    prior_type_insts,
                    prior_collections,
                );
                Err(invariant)
            }
        }
    }

    /// The appending half of [`Self::extend`], which its caller makes atomic.
    fn extend_appended(&mut self, view: &TypeMetadataView<'_>) -> Result<(), GenericInvariant> {
        let type_insts = view.generics.type_insts.len();
        for row in self.built_type_insts..type_insts {
            // During an isolated template proof the reused directory already classifies the
            // whole settled population, so extension only reaches the rows the proof body
            // itself mints. Counting them here is the proof's per-template row cost — the
            // owner-decoupled successor to the discarded clone's whole-population replay.
            #[cfg(test)]
            if view.generics.argument_domain == ArgumentDomain::TemplateProof {
                bump_scaling(|counts| counts.template_proof_rows += 1);
            }
            let id = view.generics.type_insts[row].id;
            place_generic_row(&mut self.scratch.records, &mut self.scratch.enums, row, id)?;
        }
        self.built_type_insts = type_insts;
        let collections = view.collections.len();
        for index in self.built_collections..collections {
            let target = collection_generic_target(
                &self.scratch.records,
                &self.scratch.enums,
                &self.scratch.collection_generic_targets,
                index,
                view.collections[index],
            );
            self.scratch.collection_generic_targets.push(target);
        }
        self.built_collections = collections;
        Ok(())
    }

    /// Discard the classification of every row and collection appended during a
    /// generic-template proof pass, restoring the directory to the pre-proof image so the
    /// cache stays reusable without a full rebuild and holds no truncated-row identity a
    /// later real mint would collide with. The image record/enum id ceilings shrink to the
    /// pre-proof draft counts (`records`/`enums`) — every proof row reserved an id at or
    /// above them — and the watermarks return to the pre-proof instantiation and collection
    /// counts. The per-walk marks are re-sized on the next probe by `reset_marks`.
    pub(super) fn rewind_to(
        &mut self,
        records: usize,
        enums: usize,
        type_insts: usize,
        collections: usize,
    ) {
        self.scratch.records.truncate(records);
        self.scratch.enums.truncate(enums);
        self.scratch
            .collection_generic_targets
            .truncate(collections);
        self.built_type_insts = type_insts;
        self.built_collections = collections;
    }
    // drop-path audit sentinel: end of RowDirectory::rewind_to

    /// Reset the per-walk visitation marks to cover every current row and collection.
    /// The directory content persists; only the traversal state is cleared for the next
    /// probe.
    pub(super) fn reset_marks(&mut self, view: &TypeMetadataView<'_>) {
        let type_insts = view.generics.type_insts.len();
        let collections = view.collections.len();
        self.scratch.seen_rows.clear();
        self.scratch.seen_rows.resize(type_insts, false);
        self.scratch.seen_collections.clear();
        self.scratch.seen_collections.resize(collections, false);
        self.scratch.tasks.clear();
    }
}

/// A borrowed row directory. On drop it is returned to the registry cache so the next
/// mint probe extends it rather than rebuilding over every prior row.
pub(super) struct RowDirectoryGuard<'r> {
    registry: &'r TypeRegistry,
    directory: Option<RowDirectory>,
}

impl<'r> RowDirectoryGuard<'r> {
    /// Seat a classified directory in its guard. The fields stay private to this owner, so
    /// the only way to hold a directory outside the registry's cell is through the guard
    /// that puts it back.
    pub(super) fn seat(registry: &'r TypeRegistry, directory: RowDirectory) -> Self {
        Self {
            registry,
            directory: Some(directory),
        }
    }
}

impl RowDirectoryGuard<'_> {
    #[expect(
        clippy::expect_used,
        reason = "the directory is Some from construction until Drop takes it; no other \
                  path clears it, so this guard cannot observe None"
    )]
    pub(super) fn scratch(&mut self) -> &mut MetadataScratch {
        &mut self
            .directory
            .as_mut()
            .expect("directory is present until drop")
            .scratch
    }
}

impl std::ops::Deref for RowDirectoryGuard<'_> {
    type Target = MetadataScratch;

    #[expect(
        clippy::expect_used,
        reason = "the directory is Some from construction until Drop takes it; no other \
                  path clears it, so this guard cannot observe None"
    )]
    fn deref(&self) -> &MetadataScratch {
        &self
            .directory
            .as_ref()
            .expect("directory is present until drop")
            .scratch
    }
}

impl std::ops::DerefMut for RowDirectoryGuard<'_> {
    fn deref_mut(&mut self) -> &mut MetadataScratch {
        self.scratch()
    }
}

impl Drop for RowDirectoryGuard<'_> {
    fn drop(&mut self) {
        if let Some(directory) = self.directory.take() {
            *self.registry.row_directory.borrow_mut() = Some(directory);
        }
    }
}
