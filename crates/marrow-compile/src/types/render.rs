//! Rendering type spellings for display: the bounded best-effort and validated
//! renderers that turn an instantiation row back into source-shaped text.

use super::*;

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

/// One best-effort display walk: the metadata it reads, the frames still to expand, the
/// text produced so far, and the cycle-guard nodes entered but not yet left.
struct BestEffortDisplayWalk<'a, 'v> {
    registry: &'a TypeRegistry,
    view: &'a TypeMetadataView<'v>,
    metadata: &'a MetadataScratch,
    display: &'a mut DisplayScratch,
    frames: Vec<BestEffortDisplayFrame>,
    output: String,
    entered: Vec<DisplayNode>,
}

impl BestEffortDisplayWalk<'_, '_> {
    /// Seed the stack with the root: an instantiation, or the collection argument that
    /// names one.
    fn seed(&mut self, root: BestEffortDisplayRoot) {
        match root {
            BestEffortDisplayRoot::Inst { id, generic_parent } => {
                self.frames.push(BestEffortDisplayFrame::Inst {
                    id,
                    generic_parent,
                    root: true,
                });
            }
            BestEffortDisplayRoot::Collection {
                index,
                generic_parent,
                collection_parent,
            } => self.frames.push(BestEffortDisplayFrame::Arg {
                arg: GArg::Collection(index),
                generic_parent,
                collection_parent,
            }),
        }
    }

    /// Expand frames until the stack empties. `Ok(None)` is a root that cannot be
    /// rendered; the caller unwinds whatever was entered on every path.
    fn run(&mut self) -> Result<Option<String>, GenericInvariant> {
        while let Some(frame) = self.frames.pop() {
            match frame {
                BestEffortDisplayFrame::Text(text) => self.output.push_str(text),
                BestEffortDisplayFrame::LeaveRow(row) => {
                    // The pop keeps `entered` in step for the caller's unwind.
                    self.entered.pop();
                    self.display.leave_row(row);
                }
                BestEffortDisplayFrame::LeaveCollection(index) => {
                    self.entered.pop();
                    self.display.leave_collection(index);
                }
                BestEffortDisplayFrame::Inst {
                    id,
                    generic_parent,
                    root,
                } => {
                    if !self.step_inst(id, generic_parent, root)? {
                        return Ok(None);
                    }
                }
                BestEffortDisplayFrame::Arg {
                    arg,
                    generic_parent,
                    collection_parent,
                } => self.step_arg(arg, generic_parent, collection_parent)?,
            }
        }
        Ok(Some(std::mem::take(&mut self.output)))
    }

    /// Expand one instantiation frame into its template name and argument frames.
    /// `Ok(false)` is a root whose row is absent, still filling, or already on the
    /// display path — unrenderable rather than an invariant.
    fn step_inst(
        &mut self,
        id: TypeInstId,
        generic_parent: Option<usize>,
        root: bool,
    ) -> Result<bool, GenericInvariant> {
        let Some(row) = self.metadata.row(id) else {
            if root {
                return Ok(false);
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
                owner: self.view.generics.type_insts[parent].id,
                target: id,
            });
        }
        let inst = &self.view.generics.type_insts[row];
        if matches!(inst.state, TypeInstState::Filling { .. }) || !self.display.enter_row(row) {
            if root {
                return Ok(false);
            }
            let arg = match id {
                TypeInstId::Record(id) => GArg::Struct(id),
                TypeInstId::Enum(id) => GArg::Enum(id),
            };
            return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
        }
        self.entered.push(DisplayNode::Row(row));
        let template = self.registry.template_for_args(inst.template, &inst.args)?;
        if let TypeInstState::Ready(body) = &inst.state {
            self.registry
                .validate_inst_body_metadata(inst.template, &inst.args, inst.id, body)?;
        }
        self.output.push_str(&template.name);
        self.output.push('<');
        self.frames.push(BestEffortDisplayFrame::LeaveRow(row));
        self.frames.push(BestEffortDisplayFrame::Text(">"));
        for (index, arg) in inst.args.iter().copied().enumerate().rev() {
            self.frames.push(BestEffortDisplayFrame::Arg {
                arg,
                generic_parent: Some(row),
                collection_parent: None,
            });
            if index > 0 {
                self.frames.push(BestEffortDisplayFrame::Text(", "));
            }
        }
        Ok(true)
    }

    /// Expand one type-argument frame: a scalar or declared name renders inline, a
    /// generic row or collection pushes the frames that render it.
    fn step_arg(
        &mut self,
        arg: GArg,
        generic_parent: Option<usize>,
        collection_parent: Option<CollTypeId>,
    ) -> Result<(), GenericInvariant> {
        match arg {
            GArg::Scalar(scalar) => self.output.push_str(scalar.spelling()),
            GArg::Nominal(id) => self.output.push_str(
                &self
                    .registry
                    .nominals
                    .get(id.0 as usize)
                    .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                    .name,
            ),
            GArg::Struct(id) => match self.metadata.record_owner(id) {
                Some(RecordMetadataOwner::GenericRow(_)) => {
                    self.frames.push(BestEffortDisplayFrame::Inst {
                        id: TypeInstId::Record(id),
                        generic_parent,
                        root: false,
                    });
                }
                Some(RecordMetadataOwner::DeclaredStruct(row)) => self.output.push_str(
                    &self
                        .registry
                        .structs
                        .get(row)
                        .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                        .name,
                ),
                Some(RecordMetadataOwner::ResourceRecord(_) | RecordMetadataOwner::Group(_, _))
                | None => return Err(GenericInvariant::TypeArgumentTargetMissing(arg)),
            },
            GArg::Group(id) => match self.metadata.record_owner(id) {
                Some(RecordMetadataOwner::Group(record, group)) => self.output.push_str(
                    &self
                        .registry
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
            GArg::Enum(id) => match self.metadata.enum_owner(id) {
                Some(EnumMetadataOwner::GenericRow(_)) => {
                    self.frames.push(BestEffortDisplayFrame::Inst {
                        id: TypeInstId::Enum(id),
                        generic_parent,
                        root: false,
                    });
                }
                Some(EnumMetadataOwner::DeclaredEnum(row)) => self.output.push_str(
                    &self
                        .registry
                        .enums
                        .get(row)
                        .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?
                        .name,
                ),
                None => return Err(GenericInvariant::TypeArgumentTargetMissing(arg)),
            },
            GArg::Collection(index) => {
                if collection_parent.is_some_and(|parent| index >= parent)
                    || !self.display.enter_collection(index)
                {
                    return Err(GenericInvariant::TypeArgumentTargetMissing(arg));
                }
                self.entered.push(DisplayNode::Collection(index));
                let spec = self
                    .view
                    .collections
                    .get(index.index() as usize)
                    .copied()
                    .ok_or(GenericInvariant::TypeArgumentTargetMissing(arg))?;
                self.frames
                    .push(BestEffortDisplayFrame::LeaveCollection(index));
                self.frames.push(BestEffortDisplayFrame::Text(">"));
                match spec {
                    CollSpec::List { elem } => {
                        self.output.push_str("List<");
                        self.frames.push(BestEffortDisplayFrame::Arg {
                            arg: elem,
                            generic_parent,
                            collection_parent: Some(index),
                        });
                    }
                    CollSpec::Map { key, value } => {
                        self.output.push_str("Map<");
                        self.frames.push(BestEffortDisplayFrame::Arg {
                            arg: value,
                            generic_parent,
                            collection_parent: Some(index),
                        });
                        self.frames.push(BestEffortDisplayFrame::Text(", "));
                        self.frames.push(BestEffortDisplayFrame::Arg {
                            arg: key,
                            generic_parent,
                            collection_parent: Some(index),
                        });
                    }
                }
            }
            GArg::Param(index) => {
                self.output.push_str(&format!("<type parameter {index}>"));
            }
        }
        Ok(())
    }

    /// Leave every node this walk entered, in reverse order, whatever its outcome.
    fn unwind(&mut self) {
        while let Some(node) = self.entered.pop() {
            self.display.leave(node);
        }
    }
}

fn render_best_effort_display(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    metadata: &MetadataScratch,
    root: BestEffortDisplayRoot,
    display: &mut DisplayScratch,
) -> Result<Option<String>, GenericInvariant> {
    let mut walk = BestEffortDisplayWalk {
        registry,
        view,
        metadata,
        display,
        frames: Vec::new(),
        output: String::new(),
        entered: Vec::new(),
    };
    walk.seed(root);
    let result = walk.run();
    walk.unwind();
    result
}

pub(super) fn inst_spelling_for_display(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    metadata: &MetadataScratch,
    id: TypeInstId,
    generic_parent: Option<usize>,
    display: &mut DisplayScratch,
) -> Result<Option<String>, GenericInvariant> {
    render_best_effort_display(
        registry,
        view,
        metadata,
        BestEffortDisplayRoot::Inst { id, generic_parent },
        display,
    )
}

pub(super) fn collection_spelling_for_display(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    metadata: &MetadataScratch,
    index: CollTypeId,
    generic_parent: Option<usize>,
    collection_parent: Option<CollTypeId>,
    display: &mut DisplayScratch,
) -> Result<String, GenericInvariant> {
    render_best_effort_display(
        registry,
        view,
        metadata,
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
    render_validated_arg(registry, view, metadata, arg, display, DISPLAY)
}
/// How a validated spelling walker renders a generic instantiation and what it
/// does with an unsubstituted type parameter.
///
/// Diagnostics spell `Name<a, b>`; the durable anchor ledger spells `Name[a,b]` and
/// admits no type parameter at all, because an identity byte may not depend on a
/// parameter that was never bound. One walk serves both forms, so a ledger byte stays
/// stable by [`ANCHOR`] being fixed.
#[derive(Clone, Copy)]
pub(super) struct Spelling {
    open: char,
    close: &'static str,
    separator: &'static str,
    parameters: bool,
}

/// The angle-form spelling diagnostics and cycle labels read.
pub(super) const DISPLAY: Spelling = Spelling {
    open: '<',
    close: ">",
    separator: ", ",
    parameters: true,
};

/// The bracket-form, space-free spelling the opaque durable-anchor ledger reads.
pub(super) const ANCHOR: Spelling = Spelling {
    open: '[',
    close: "]",
    separator: ",",
    parameters: false,
};

#[derive(Clone, Copy)]
enum ValidatedFrame {
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

/// Spell a metadata-validated value-type argument, recursing through nested generic
/// instantiations and collections on an explicit frame stack.
///
/// A `TypeId` has exactly one metadata owner — resource record, declared struct,
/// group, or generic row — so the lookups below are mutually exclusive and their
/// order carries no meaning.
pub(super) fn render_validated_arg(
    registry: &TypeRegistry,
    view: &TypeMetadataView<'_>,
    metadata: &MetadataScratch,
    arg: GArg,
    display: &mut DisplayScratch,
    spelling: Spelling,
) -> Result<String, GenericInvariant> {
    let mut output = String::new();
    let mut frames = vec![ValidatedFrame::Arg(arg)];
    let mut entered = Vec::new();
    let result = (|| {
        while let Some(frame) = frames.pop() {
            match frame {
                ValidatedFrame::Text(text) => output.push_str(text),
                ValidatedFrame::Leave(node) => {
                    entered.pop();
                    display.leave(node);
                }
                ValidatedFrame::Arg(arg) => match arg {
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
                            frames.push(ValidatedFrame::Inst {
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
                            frames.push(ValidatedFrame::Inst {
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
                        frames.push(ValidatedFrame::Collection(index));
                    }
                    GArg::Param(index) => {
                        if !spelling.parameters {
                            return Err(GenericInvariant::TypeArgumentParameter(index));
                        }
                        output.push_str(&format!("<type parameter {index}>"));
                    }
                },
                ValidatedFrame::Inst { row, id, arg } => {
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
                    output.push(spelling.open);
                    frames.push(ValidatedFrame::Leave(node));
                    frames.push(ValidatedFrame::Text(spelling.close));
                    for (index, arg) in inst.args.iter().copied().enumerate().rev() {
                        frames.push(ValidatedFrame::Arg(arg));
                        if index > 0 {
                            frames.push(ValidatedFrame::Text(spelling.separator));
                        }
                    }
                }
                ValidatedFrame::Collection(index) => {
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
                    frames.push(ValidatedFrame::Leave(node));
                    frames.push(ValidatedFrame::Text(spelling.close));
                    match spec {
                        CollSpec::List { elem } => {
                            output.push_str("List");
                            output.push(spelling.open);
                            frames.push(ValidatedFrame::Arg(elem));
                        }
                        CollSpec::Map { key, value } => {
                            output.push_str("Map");
                            output.push(spelling.open);
                            frames.push(ValidatedFrame::Arg(value));
                            frames.push(ValidatedFrame::Text(spelling.separator));
                            frames.push(ValidatedFrame::Arg(key));
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
