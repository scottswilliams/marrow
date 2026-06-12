use std::collections::HashMap;

use marrow_schema::ScalarType;

use crate::expand_alias;
use crate::program::{CheckedProgram, MarrowType};
use crate::resolve::{Def, DefItem, Resolution, ResolvableKind, resolve, resolve_store_by_root};

use super::{
    CheckedArg, CheckedBuiltinCall, CheckedCallTarget, CheckedExpr, CheckedIdentityConstructor,
    CheckedSavedKeyParam, CheckedStdCall, checked_resource_constructor, function_ref, resource_ref,
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
        if let Some(target) = Self::identity_constructor_call_target(&expanded, args, program) {
            return Some(target);
        }
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
                && resolve_store_by_root(program, root).is_some_and(|store| {
                    store.store.indexes.iter().any(|index| index.name == *name)
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

    fn identity_constructor_call_target(
        expanded: &[String],
        args: &[CheckedArg],
        program: &CheckedProgram,
    ) -> Option<Self> {
        if !matches!(expanded, [name] if name == "Id") {
            return None;
        }
        let first = args.first()?;
        let CheckedExpr::SavedRoot { name: root, .. } = &first.value else {
            return None;
        };
        let store = resolve_store_by_root(program, root)?;
        if store.store.identity_keys.is_empty() {
            return None;
        }
        let keys = store
            .store
            .identity_keys
            .iter()
            .map(|key| CheckedSavedKeyParam {
                name: key.name.clone(),
                scalar: key.ty.scalar(),
            })
            .collect();
        Some(Self::IdentityConstructor(CheckedIdentityConstructor {
            root: root.clone(),
            keys,
        }))
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
        if !has_named {
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

    /// Whether this builtin reads its path argument as attached saved data — the
    /// tree-traversal and ordered-navigation builtins whose argument is a saved
    /// collection rather than a plain value.
    pub(crate) fn reads_attached_data(self) -> bool {
        matches!(
            self,
            Self::Keys
                | Self::Values
                | Self::Entries
                | Self::Count
                | Self::Next
                | Self::Prev
                | Self::NextId
                | Self::Reversed
        )
    }

    /// Whether this builtin navigates to a neighbor key (`next`/`prev`), which
    /// records a positional presence read.
    pub(crate) fn is_neighbor_read(self) -> bool {
        matches!(self, Self::Next | Self::Prev)
    }
}
