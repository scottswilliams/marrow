use std::collections::HashMap;

use marrow_schema::ScalarType;

use crate::expand_alias;
use crate::program::{CheckedProgram, MarrowType};
use crate::resolve::{Def, DefItem, Resolution, ResolvableKind, resolve};

use super::{
    CheckedArg, CheckedBuiltinCall, CheckedCallTarget, CheckedExpr, CheckedStdCall,
    checked_resource_constructor, function_ref, resource_ref,
};

impl CheckedCallTarget {
    pub(super) fn for_call(
        callee: &CheckedExpr,
        args: &[CheckedArg],
        program: &CheckedProgram,
        from_module: &str,
        aliases: &HashMap<String, Vec<String>>,
        scope: &[HashMap<String, MarrowType>],
    ) -> Option<Self> {
        if let Some(target) = Self::saved_path_call_target(callee, program) {
            return Some(target);
        }
        if let Some(target) = Self::local_collection_call_target(callee, scope) {
            return Some(target);
        }
        let expanded = expanded_name_call(callee, aliases)?;
        if let Some(target) = Self::constructor_call_target(&expanded, program, from_module) {
            return Some(target);
        }
        if let Some(target) = Self::pure_call_target(&expanded, args) {
            return Some(target);
        }
        Self::function_call_target(&expanded, program, from_module)
    }

    fn local_collection_call_target(
        callee: &CheckedExpr,
        scope: &[HashMap<String, MarrowType>],
    ) -> Option<Self> {
        let CheckedExpr::Name { segments, .. } = callee else {
            return None;
        };
        let [name] = segments.as_slice() else {
            return None;
        };
        let ty = scope
            .iter()
            .rev()
            .find_map(|frame| frame.get(name.as_str()))?;
        matches!(ty, MarrowType::Sequence(_) | MarrowType::LocalTree { .. })
            .then(|| Self::LocalCollection { name: name.clone() })
    }

    fn saved_path_call_target(callee: &CheckedExpr, program: &CheckedProgram) -> Option<Self> {
        if let CheckedExpr::Field { base, name, .. } = callee {
            if let CheckedExpr::SavedRoot { name: root, .. } = base.as_ref()
                && program.modules.iter().any(|module| {
                    module.stores.iter().any(|store| {
                        store.root == *root && store.indexes.iter().any(|index| index.name == *name)
                    })
                })
            {
                return Some(Self::SavedIndexLookup);
            }
            return Some(Self::SavedLayerRead);
        }
        if matches!(callee, CheckedExpr::SavedRoot { .. }) {
            return Some(Self::SavedResourceRead);
        }
        None
    }

    fn constructor_call_target(
        expanded: &[String],
        program: &CheckedProgram,
        from_module: &str,
    ) -> Option<Self> {
        if matches!(expanded, [name] if name == "Error") {
            return Some(Self::ErrorConstructor);
        }
        if let Resolution::Found(Def {
            module,
            item: DefItem::Resource(resource),
            ..
        }) = resolve(program, from_module, expanded, ResolvableKind::Resource)
            && let Some(resource_ref) = resource_ref(program, module, resource)
        {
            return Some(Self::ResourceConstructor(checked_resource_constructor(
                program,
                module,
                resource,
                resource_ref,
            )));
        }
        None
    }

    fn pure_call_target(expanded: &[String], args: &[CheckedArg]) -> Option<Self> {
        let has_named = args.iter().any(|arg| arg.name.is_some());
        let has_moded = args.iter().any(|arg| arg.mode.is_some());
        if !has_named && !has_moded {
            if let [name] = expanded
                && let Some(builtin) = CheckedBuiltinCall::from_name(name)
            {
                return Some(Self::Builtin(builtin));
            }
            if let [first, module, op] = expanded
                && first == "std"
                && let Some(entry) = marrow_schema::stdlib::lookup(module, op)
            {
                return Some(Self::Std(CheckedStdCall {
                    module: entry.module,
                    op: entry.op,
                    capability: entry.capability,
                }));
            }
        }
        None
    }

    fn function_call_target(
        expanded: &[String],
        program: &CheckedProgram,
        from_module: &str,
    ) -> Option<Self> {
        match resolve(program, from_module, expanded, ResolvableKind::Function) {
            Resolution::Found(Def {
                module,
                item: DefItem::Function(function),
                ..
            }) => function_ref(program, module, function).map(Self::Function),
            Resolution::Found(Def {
                item: DefItem::Resource(_),
                ..
            })
            | Resolution::NotVisible(_)
            | Resolution::Ambiguous(_)
            | Resolution::Unresolved => None,
        }
    }
}

fn expanded_name_call(
    callee: &CheckedExpr,
    aliases: &HashMap<String, Vec<String>>,
) -> Option<Vec<String>> {
    let CheckedExpr::Name { segments, .. } = callee else {
        return None;
    };
    Some(expand_alias(segments, aliases))
}

impl CheckedBuiltinCall {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "print" => Self::Print,
            "write" => Self::Write,
            "exists" => Self::Exists,
            "nextId" => Self::NextId,
            "append" => Self::Append,
            "bytes" => Self::Bytes,
            "ErrorCode" => Self::ErrorCode,
            "keys" => Self::Keys,
            "count" => Self::Count,
            "values" => Self::Values,
            "entries" => Self::Entries,
            "reversed" => Self::Reversed,
            "next" => Self::Next,
            "prev" => Self::Prev,
            other => Self::Conversion(ScalarType::from_scalar_name(other)?),
        })
    }
}
