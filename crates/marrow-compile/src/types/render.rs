//! Rendering type spellings for display: the record/enum display-owner claims and
//! the bounded best-effort and validated renderers that turn an instantiation row
//! back into source-shaped text.

use super::*;

fn claim_record_display_owner(
    owner: &mut Option<RecordMetadataOwner>,
    candidate: RecordMetadataOwner,
    id: TypeId,
) -> Result<(), GenericInvariant> {
    if owner.replace(candidate).is_some() {
        Err(GenericInvariant::TypeIdentityCollision(TypeInstId::Record(
            id,
        )))
    } else {
        Ok(())
    }
}

fn record_display_owner(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    id: TypeId,
) -> Result<Option<RecordMetadataOwner>, GenericInvariant> {
    let mut owner = None;
    for (record_row, record) in registry.records.iter().enumerate() {
        if record.type_id == id {
            claim_record_display_owner(
                &mut owner,
                RecordMetadataOwner::ResourceRecord(record_row),
                id,
            )?;
        }
        for (group_row, group) in record.groups.iter().enumerate() {
            if group.type_id == id {
                claim_record_display_owner(
                    &mut owner,
                    RecordMetadataOwner::Group(record_row, group_row),
                    id,
                )?;
            }
        }
    }
    for (row, info) in registry.structs.iter().enumerate() {
        if info.type_id == id {
            claim_record_display_owner(&mut owner, RecordMetadataOwner::DeclaredStruct(row), id)?;
        }
    }
    for (row, inst) in view.generics.type_insts.iter().enumerate() {
        if inst.id == TypeInstId::Record(id) {
            claim_record_display_owner(&mut owner, RecordMetadataOwner::GenericRow(row), id)?;
        }
    }
    Ok(owner)
}

fn claim_enum_display_owner(
    owner: &mut Option<EnumMetadataOwner>,
    candidate: EnumMetadataOwner,
    id: EnumId,
) -> Result<(), GenericInvariant> {
    if owner.replace(candidate).is_some() {
        Err(GenericInvariant::TypeIdentityCollision(TypeInstId::Enum(
            id,
        )))
    } else {
        Ok(())
    }
}

fn enum_display_owner(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    id: EnumId,
) -> Result<Option<EnumMetadataOwner>, GenericInvariant> {
    let mut owner = None;
    for (row, info) in registry.enums.iter().enumerate() {
        if info.enum_id == id {
            claim_enum_display_owner(&mut owner, EnumMetadataOwner::DeclaredEnum(row), id)?;
        }
    }
    for (row, inst) in view.generics.type_insts.iter().enumerate() {
        if inst.id == TypeInstId::Enum(id) {
            claim_enum_display_owner(&mut owner, EnumMetadataOwner::GenericRow(row), id)?;
        }
    }
    Ok(owner)
}

fn validate_display_semantic_key(
    view: &TypeMetadataView<'_>,
    row: usize,
    id: TypeInstId,
) -> Result<(), GenericInvariant> {
    let inst = view
        .generics
        .type_insts
        .get(row)
        .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
    let mut first = None;
    for candidate in &view.generics.type_insts {
        if candidate.template == inst.template && candidate.args == inst.args {
            if let Some(first) = first {
                return Err(GenericInvariant::TypeInstantiationKeyCollision {
                    first,
                    duplicate: candidate.id,
                });
            }
            first = Some(candidate.id);
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum BestEffortDisplayRoot {
    Inst {
        id: TypeInstId,
        generic_parent: Option<usize>,
    },
    Collection {
        index: CollTypeId,
        generic_parent: Option<usize>,
        collection_parent: Option<CollTypeId>,
    },
}

#[derive(Clone, Copy)]
enum BestEffortDisplayFrame {
    Arg {
        arg: GArg,
        generic_parent: Option<usize>,
        collection_parent: Option<CollTypeId>,
    },
    Inst {
        id: TypeInstId,
        generic_parent: Option<usize>,
        root: bool,
    },
    Text(&'static str),
    LeaveRow(usize),
    LeaveCollection(CollTypeId),
}

fn best_effort_display_inst_row(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    id: TypeInstId,
) -> Result<Option<usize>, GenericInvariant> {
    Ok(match id {
        TypeInstId::Record(id) => match record_display_owner(registry, view, id)? {
            Some(RecordMetadataOwner::GenericRow(row)) => Some(row),
            Some(
                RecordMetadataOwner::ResourceRecord(_)
                | RecordMetadataOwner::DeclaredStruct(_)
                | RecordMetadataOwner::Group(_, _),
            )
            | None => None,
        },
        TypeInstId::Enum(id) => match enum_display_owner(registry, view, id)? {
            Some(EnumMetadataOwner::GenericRow(row)) => Some(row),
            Some(EnumMetadataOwner::DeclaredEnum(_)) | None => None,
        },
    })
}

fn render_best_effort_display(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    root: BestEffortDisplayRoot,
    display: &mut DisplayScratch,
) -> Result<Option<String>, GenericInvariant> {
    let mut frames = Vec::new();
    match root {
        BestEffortDisplayRoot::Inst { id, generic_parent } => {
            frames.push(BestEffortDisplayFrame::Inst {
                id,
                generic_parent,
                root: true,
            });
        }
        BestEffortDisplayRoot::Collection {
            index,
            generic_parent,
            collection_parent,
        } => frames.push(BestEffortDisplayFrame::Arg {
            arg: GArg::Collection(index),
            generic_parent,
            collection_parent,
        }),
    }
    let mut output = String::new();
    let mut entered = Vec::new();
    let result = (|| {
        while let Some(frame) = frames.pop() {
            match frame {
                BestEffortDisplayFrame::Text(text) => output.push_str(text),
                BestEffortDisplayFrame::LeaveRow(row) => {
                    // Profiles cannot disagree: `leave_row` takes the frame's own row,
                    // not the popped one, so nothing here reads what this compares. The
                    // pop keeps `entered` in step for the unwind path below.
                    let removed = entered.pop();
                    debug_assert_eq!(removed, Some(DisplayNode::Row(row)));
                    display.leave_row(row);
                }
                BestEffortDisplayFrame::LeaveCollection(index) => {
                    // Unread on the same terms as the row arm above.
                    let removed = entered.pop();
                    debug_assert_eq!(removed, Some(DisplayNode::Collection(index)));
                    display.leave_collection(index);
                }
                BestEffortDisplayFrame::Inst {
                    id,
                    generic_parent,
                    root,
                } => {
                    let Some(row) = best_effort_display_inst_row(registry, view, id)? else {
                        if root {
                            return Ok(None);
                        }
                        let arg = match id {
                            TypeInstId::Record(id) => GArg::Struct(id),
                            TypeInstId::Enum(id) => GArg::Enum(id),
                        };
                        return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                    };
                    if let Some(parent) = generic_parent
                        && row >= parent
                    {
                        return Err(GenericInvariant::TypeArgumentOrderViolation {
                            owner: view.generics.type_insts[parent].id,
                            target: id,
                        });
                    }
                    validate_display_semantic_key(view, row, id)?;
                    let inst = &view.generics.type_insts[row];
                    if matches!(inst.state, TypeInstState::Filling { .. })
                        || !display.enter_row(row)
                    {
                        if root {
                            return Ok(None);
                        }
                        let arg = match id {
                            TypeInstId::Record(id) => GArg::Struct(id),
                            TypeInstId::Enum(id) => GArg::Enum(id),
                        };
                        return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                    }
                    entered.push(DisplayNode::Row(row));
                    let template = registry.template_for_args(inst.template, &inst.args)?;
                    if let TypeInstState::Ready(body) = &inst.state {
                        registry.validate_inst_body_metadata(
                            inst.template,
                            &inst.args,
                            inst.id,
                            body,
                        )?;
                    }
                    output.push_str(&template.name);
                    output.push('<');
                    frames.push(BestEffortDisplayFrame::LeaveRow(row));
                    frames.push(BestEffortDisplayFrame::Text(">"));
                    for (index, arg) in inst.args.iter().copied().enumerate().rev() {
                        frames.push(BestEffortDisplayFrame::Arg {
                            arg,
                            generic_parent: Some(row),
                            collection_parent: None,
                        });
                        if index > 0 {
                            frames.push(BestEffortDisplayFrame::Text(", "));
                        }
                    }
                }
                BestEffortDisplayFrame::Arg {
                    arg,
                    generic_parent,
                    collection_parent,
                } => match arg {
                    GArg::Scalar(scalar) => output.push_str(scalar.spelling()),
                    GArg::Nominal(id) => output.push_str(
                        &registry
                            .nominals
                            .get(id.0 as usize)
                            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                            .name,
                    ),
                    GArg::Struct(id) => match record_display_owner(registry, view, id)? {
                        Some(RecordMetadataOwner::GenericRow(_)) => {
                            frames.push(BestEffortDisplayFrame::Inst {
                                id: TypeInstId::Record(id),
                                generic_parent,
                                root: false,
                            });
                        }
                        Some(RecordMetadataOwner::DeclaredStruct(row)) => output.push_str(
                            &registry
                                .structs
                                .get(row)
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                .name,
                        ),
                        Some(
                            RecordMetadataOwner::ResourceRecord(_)
                            | RecordMetadataOwner::Group(_, _),
                        )
                        | None => return Err(GenericInvariant::TypeArgumentTargetMissing(arg)),
                    },
                    GArg::Group(id) => match record_display_owner(registry, view, id)? {
                        Some(RecordMetadataOwner::Group(record, group)) => output.push_str(
                            &registry
                                .records
                                .get(record)
                                .and_then(|record| record.groups.get(group))
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                .name,
                        ),
                        Some(
                            RecordMetadataOwner::ResourceRecord(_)
                            | RecordMetadataOwner::DeclaredStruct(_)
                            | RecordMetadataOwner::GenericRow(_),
                        )
                        | None => return Err(GenericInvariant::TypeArgumentTargetMissing(arg)),
                    },
                    GArg::Enum(id) => match enum_display_owner(registry, view, id)? {
                        Some(EnumMetadataOwner::GenericRow(_)) => {
                            frames.push(BestEffortDisplayFrame::Inst {
                                id: TypeInstId::Enum(id),
                                generic_parent,
                                root: false,
                            });
                        }
                        Some(EnumMetadataOwner::DeclaredEnum(row)) => output.push_str(
                            &registry
                                .enums
                                .get(row)
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                .name,
                        ),
                        None => return Err(GenericInvariant::TypeArgumentTargetMissing(arg)),
                    },
                    GArg::Collection(index) => {
                        if collection_parent.is_some_and(|parent| index >= parent)
                            || !display.enter_collection(index)
                        {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        }
                        entered.push(DisplayNode::Collection(index));
                        let spec = view
                            .collections
                            .get(index.index() as usize)
                            .copied()
                            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                        frames.push(BestEffortDisplayFrame::LeaveCollection(index));
                        frames.push(BestEffortDisplayFrame::Text(">"));
                        match spec {
                            CollSpec::List { elem } => {
                                output.push_str("List<");
                                frames.push(BestEffortDisplayFrame::Arg {
                                    arg: elem,
                                    generic_parent,
                                    collection_parent: Some(index),
                                });
                            }
                            CollSpec::Map { key, value } => {
                                output.push_str("Map<");
                                frames.push(BestEffortDisplayFrame::Arg {
                                    arg: value,
                                    generic_parent,
                                    collection_parent: Some(index),
                                });
                                frames.push(BestEffortDisplayFrame::Text(", "));
                                frames.push(BestEffortDisplayFrame::Arg {
                                    arg: key,
                                    generic_parent,
                                    collection_parent: Some(index),
                                });
                            }
                        }
                    }
                    GArg::Param(index) => {
                        output.push_str(&format!("<type parameter {index}>"));
                    }
                },
            }
        }
        Ok(Some(output))
    })();
    while let Some(node) = entered.pop() {
        display.leave(node);
    }
    result
}

pub(super) fn inst_spelling_for_display(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    id: TypeInstId,
    generic_parent: Option<usize>,
    display: &mut DisplayScratch,
) -> Result<Option<String>, GenericInvariant> {
    render_best_effort_display(
        registry,
        view,
        BestEffortDisplayRoot::Inst { id, generic_parent },
        display,
    )
}

pub(super) fn collection_spelling_for_display(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    index: CollTypeId,
    generic_parent: Option<usize>,
    collection_parent: Option<CollTypeId>,
    display: &mut DisplayScratch,
) -> Result<String, GenericInvariant> {
    render_best_effort_display(
        registry,
        view,
        BestEffortDisplayRoot::Collection {
            index,
            generic_parent,
            collection_parent,
        },
        display,
    )?
    .ok_or(GenericInvariant::TypeArgumentTargetMissing(
        GArg::Collection(index),
    ))
}

/// The canonical angle-form display spelling of a metadata-validated value-type
/// argument. The caller supplies the same immutable owner view and directory used
/// for semantic validation, so a graph walk never rebuilds or searches the cache.
pub(super) fn garg_spelling_validated(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    metadata: &MetadataScratch,
    arg: GArg,
    display: &mut DisplayScratch,
) -> Result<String, GenericInvariant> {
    render_validated_display_arg(registry, view, metadata, arg, display)
}

#[derive(Clone, Copy)]
enum ValidatedDisplayFrame {
    Arg(GArg),
    Inst {
        row: usize,
        id: TypeInstId,
        arg: GArg,
    },
    Collection(CollTypeId),
    Text(&'static str),
    Leave(DisplayNode),
}

pub(super) fn render_validated_display_arg(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    metadata: &MetadataScratch,
    arg: GArg,
    display: &mut DisplayScratch,
) -> Result<String, GenericInvariant> {
    let mut output = String::new();
    let mut frames = vec![ValidatedDisplayFrame::Arg(arg)];
    let mut entered = Vec::new();
    let result = (|| {
        while let Some(frame) = frames.pop() {
            match frame {
                ValidatedDisplayFrame::Text(text) => output.push_str(text),
                ValidatedDisplayFrame::Leave(node) => {
                    // Profiles cannot disagree: `leave` takes the frame's own node, so
                    // nothing here reads what this compares; the pop keeps `entered` in
                    // step for the unwind path below.
                    let removed = entered.pop();
                    debug_assert_eq!(removed, Some(node));
                    display.leave(node);
                }
                ValidatedDisplayFrame::Arg(arg) => match arg {
                    GArg::Scalar(scalar) => output.push_str(scalar.spelling()),
                    GArg::Nominal(id) => output.push_str(
                        &registry
                            .nominals
                            .get(id.0 as usize)
                            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                            .name,
                    ),
                    GArg::Struct(id) => {
                        if metadata.resource_record(id).is_some() {
                            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                        }
                        if let Some(row) = metadata.row(TypeInstId::Record(id)) {
                            frames.push(ValidatedDisplayFrame::Inst {
                                row,
                                id: TypeInstId::Record(id),
                                arg,
                            });
                        } else {
                            let row = metadata
                                .declared_struct(id)
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                            output.push_str(
                                &registry
                                    .structs
                                    .get(row)
                                    .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                    .name,
                            );
                        }
                    }
                    GArg::Group(id) => {
                        let (record, group) = metadata
                            .group(id)
                            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                        output.push_str(
                            &registry
                                .records
                                .get(record)
                                .and_then(|record| record.groups.get(group))
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                .name,
                        );
                    }
                    GArg::Enum(id) => {
                        if let Some(row) = metadata.row(TypeInstId::Enum(id)) {
                            frames.push(ValidatedDisplayFrame::Inst {
                                row,
                                id: TypeInstId::Enum(id),
                                arg,
                            });
                        } else {
                            let row = metadata
                                .declared_enum(id)
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                            output.push_str(
                                &registry
                                    .enums
                                    .get(row)
                                    .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                    .name,
                            );
                        }
                    }
                    GArg::Collection(index) => {
                        frames.push(ValidatedDisplayFrame::Collection(index));
                    }
                    GArg::Param(index) => {
                        output.push_str(&format!("<type parameter {index}>"));
                    }
                },
                ValidatedDisplayFrame::Inst { row, id, arg } => {
                    let inst = view
                        .generics
                        .type_insts
                        .get(row)
                        .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
                    if !matches!(inst.state, TypeInstState::Ready(_)) || !display.enter_row(row) {
                        return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                    }
                    let node = DisplayNode::Row(row);
                    entered.push(node);
                    let template = registry
                        .type_templates
                        .get(inst.template)
                        .ok_or(GenericInvariant::TypeTemplateMissing(inst.template))?;
                    output.push_str(&template.name);
                    output.push('<');
                    frames.push(ValidatedDisplayFrame::Leave(node));
                    frames.push(ValidatedDisplayFrame::Text(">"));
                    for (index, arg) in inst.args.iter().copied().enumerate().rev() {
                        frames.push(ValidatedDisplayFrame::Arg(arg));
                        if index > 0 {
                            frames.push(ValidatedDisplayFrame::Text(", "));
                        }
                    }
                }
                ValidatedDisplayFrame::Collection(index) => {
                    let arg = GArg::Collection(index);
                    if !display.enter_collection(index) {
                        return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                    }
                    let node = DisplayNode::Collection(index);
                    entered.push(node);
                    let spec = view
                        .collections
                        .get(index.index() as usize)
                        .copied()
                        .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                    frames.push(ValidatedDisplayFrame::Leave(node));
                    frames.push(ValidatedDisplayFrame::Text(">"));
                    match spec {
                        CollSpec::List { elem } => {
                            output.push_str("List<");
                            frames.push(ValidatedDisplayFrame::Arg(elem));
                        }
                        CollSpec::Map { key, value } => {
                            output.push_str("Map<");
                            frames.push(ValidatedDisplayFrame::Arg(value));
                            frames.push(ValidatedDisplayFrame::Text(", "));
                            frames.push(ValidatedDisplayFrame::Arg(key));
                        }
                    }
                }
            }
        }
        Ok(output)
    })();
    while let Some(node) = entered.pop() {
        display.leave(node);
    }
    result
}

/// The durable-anchor spelling of a bare value-type argument: the space-free,
/// bracket-form opaque-ledger twin of [`garg_spelling`], recursing through nested
/// generic instantiations. It never calls the angle-form display owner, so the
/// ledger bytes stay byte-stable and independent of diagnostic spelling. The
/// deliberate near-duplication is the isolation boundary the durable identity relies
/// on; do not merge the two behind a shared delimiter policy.
#[cfg(test)]
pub(super) fn garg_anchor_spelling(
    registry: &TypeRegistry,
    arg: GArg,
) -> Result<String, GenericInvariant> {
    let view = registry.metadata_view();
    let mut metadata = MetadataScratch::try_new(&view)?;
    view.validate_args_with(std::slice::from_ref(&arg), None, &mut metadata)?;
    let mut display = DisplayScratch::for_view(&view);
    garg_anchor_spelling_validated(registry, &view, &metadata, arg, &mut display)
}

#[derive(Clone, Copy)]
enum ValidatedAnchorFrame {
    Arg(GArg),
    Inst {
        row: usize,
        id: TypeInstId,
        arg: GArg,
    },
    Collection(CollTypeId),
    Text(&'static str),
    Leave(DisplayNode),
}

#[cfg(test)]
fn garg_anchor_spelling_validated(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    metadata: &MetadataScratch,
    arg: GArg,
    display: &mut DisplayScratch,
) -> Result<String, GenericInvariant> {
    render_validated_anchor_arg(registry, view, metadata, arg, display)
}

pub(super) fn render_validated_anchor_arg(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    metadata: &MetadataScratch,
    arg: GArg,
    display: &mut DisplayScratch,
) -> Result<String, GenericInvariant> {
    let mut output = String::new();
    let mut frames = vec![ValidatedAnchorFrame::Arg(arg)];
    let mut entered = Vec::new();
    let result = (|| {
        while let Some(frame) = frames.pop() {
            match frame {
                ValidatedAnchorFrame::Text(text) => output.push_str(text),
                ValidatedAnchorFrame::Leave(node) => {
                    // Unread on the same terms as the validated-display walker above.
                    let removed = entered.pop();
                    debug_assert_eq!(removed, Some(node));
                    display.leave(node);
                }
                ValidatedAnchorFrame::Arg(arg) => match arg {
                    GArg::Scalar(scalar) => output.push_str(scalar.spelling()),
                    GArg::Nominal(id) => output.push_str(
                        &registry
                            .nominals
                            .get(id.0 as usize)
                            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                            .name,
                    ),
                    GArg::Struct(id) => {
                        if let Some(row) = metadata.declared_struct(id) {
                            output.push_str(
                                &registry
                                    .structs
                                    .get(row)
                                    .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                    .name,
                            );
                        } else {
                            let inst_id = TypeInstId::Record(id);
                            let row = metadata
                                .row(inst_id)
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                            frames.push(ValidatedAnchorFrame::Inst {
                                row,
                                id: inst_id,
                                arg,
                            });
                        }
                    }
                    GArg::Group(id) => {
                        let (record, group) = metadata
                            .group(id)
                            .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                        output.push_str(
                            &registry
                                .records
                                .get(record)
                                .and_then(|record| record.groups.get(group))
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                .name,
                        );
                    }
                    GArg::Enum(id) => {
                        if let Some(row) = metadata.declared_enum(id) {
                            output.push_str(
                                &registry
                                    .enums
                                    .get(row)
                                    .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                                    .name,
                            );
                        } else {
                            let inst_id = TypeInstId::Enum(id);
                            let row = metadata
                                .row(inst_id)
                                .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                            frames.push(ValidatedAnchorFrame::Inst {
                                row,
                                id: inst_id,
                                arg,
                            });
                        }
                    }
                    GArg::Collection(index) => {
                        frames.push(ValidatedAnchorFrame::Collection(index));
                    }
                    GArg::Param(index) => {
                        return Err(GenericInvariant::TypeArgumentParameter(index));
                    }
                },
                ValidatedAnchorFrame::Inst { row, id, arg } => {
                    let inst = view
                        .generics
                        .type_insts
                        .get(row)
                        .ok_or(GenericInvariant::ReadyBodyMissing(id))?;
                    if !matches!(inst.state, TypeInstState::Ready(_)) || !display.enter_row(row) {
                        return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                    }
                    let node = DisplayNode::Row(row);
                    entered.push(node);
                    let template = registry
                        .type_templates
                        .get(inst.template)
                        .ok_or(GenericInvariant::TypeTemplateMissing(inst.template))?;
                    output.push_str(&template.name);
                    output.push('[');
                    frames.push(ValidatedAnchorFrame::Leave(node));
                    frames.push(ValidatedAnchorFrame::Text("]"));
                    for (index, arg) in inst.args.iter().copied().enumerate().rev() {
                        frames.push(ValidatedAnchorFrame::Arg(arg));
                        if index > 0 {
                            frames.push(ValidatedAnchorFrame::Text(","));
                        }
                    }
                }
                ValidatedAnchorFrame::Collection(index) => {
                    let arg = GArg::Collection(index);
                    if !display.enter_collection(index) {
                        return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                    }
                    let node = DisplayNode::Collection(index);
                    entered.push(node);
                    let spec = view
                        .collections
                        .get(index.index() as usize)
                        .copied()
                        .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                    frames.push(ValidatedAnchorFrame::Leave(node));
                    frames.push(ValidatedAnchorFrame::Text("]"));
                    match spec {
                        CollSpec::List { elem } => {
                            output.push_str("List[");
                            frames.push(ValidatedAnchorFrame::Arg(elem));
                        }
                        CollSpec::Map { key, value } => {
                            output.push_str("Map[");
                            frames.push(ValidatedAnchorFrame::Arg(value));
                            frames.push(ValidatedAnchorFrame::Text(","));
                            frames.push(ValidatedAnchorFrame::Arg(key));
                        }
                    }
                }
            }
        }
        Ok(output)
    })();
    while let Some(node) = entered.pop() {
        display.leave(node);
    }
    result
}
