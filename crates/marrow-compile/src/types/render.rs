//! Rendering type spellings for display and for the durable anchor ledger: one
//! validated walker turns an instantiation row back into source-shaped text.

use super::*;

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
