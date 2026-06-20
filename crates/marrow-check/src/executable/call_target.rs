use std::collections::HashMap;

use crate::diagnostics::ConversionTarget;
use crate::expand_alias;
use crate::program::{CheckedProgram, MarrowType};
use crate::resolve::{Def, DefItem, Resolution, ResolvableKind, resolve, resolve_store_by_root};

use super::{
    CheckedArg, CheckedBuiltinCall, CheckedBuiltinCallDescriptor, CheckedBuiltinCallParameter,
    CheckedBuiltinReturnShape, CheckedBuiltinValueShape, CheckedCallTarget, CheckedExpr,
    CheckedIdentityConstructor, CheckedSavedKeyParam, CheckedStdCall, checked_resource_constructor,
    function_ref, resource_ref,
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
        if let Some(target) = Self::saved_path_call_target(callee) {
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

    fn saved_path_call_target(callee: &CheckedExpr) -> Option<Self> {
        let place = callee.saved_place()?;
        match &place.terminal {
            super::CheckedSavedTerminal::Index { .. } => Some(Self::SavedIndexLookup),
            super::CheckedSavedTerminal::Record
                if matches!(callee, CheckedExpr::SavedRoot { .. }) =>
            {
                Some(Self::SavedResourceRead)
            }
            super::CheckedSavedTerminal::Record => Some(Self::SavedLayerRead),
            super::CheckedSavedTerminal::Field { .. } => Some(Self::SavedLayerRead),
        }
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
                    presence: entry.presence,
                    requires_capability: entry.requires_capability,
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
    pub(crate) fn descriptor_for_name(name: &str) -> Option<&'static CheckedBuiltinCallDescriptor> {
        BUILTIN_CALLS
            .iter()
            .find(|descriptor| descriptor.spelling == name)
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        if let Some(descriptor) = Self::descriptor_for_name(name) {
            return Some(descriptor.call);
        }
        Some(match ConversionTarget::from_name(name)? {
            ConversionTarget::ErrorCode => Self::ErrorCode,
            ConversionTarget::Bytes => Self::Bytes,
            target => Self::Conversion(target.scalar()),
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

const VALUE: &[CheckedBuiltinCallParameter] = &[param("value", CheckedBuiltinValueShape::Value)];
const PATH: &[CheckedBuiltinCallParameter] = &[param("path", CheckedBuiltinValueShape::SavedPath)];
const ROOT: &[CheckedBuiltinCallParameter] = &[param("root", CheckedBuiltinValueShape::SavedRoot)];
const COLLECTION: &[CheckedBuiltinCallParameter] =
    &[param("collection", CheckedBuiltinValueShape::Collection)];
const LAYER_VALUE: &[CheckedBuiltinCallParameter] = &[
    param("layer", CheckedBuiltinValueShape::SavedLayer),
    param("value", CheckedBuiltinValueShape::Value),
];

#[rustfmt::skip]
const BUILTIN_CALLS: &[CheckedBuiltinCallDescriptor] = &[
    descriptor("print", CheckedBuiltinCall::Print, VALUE, ret_void(), "Writes rendered text to output with a newline."),
    descriptor("exists", CheckedBuiltinCall::Exists, PATH, ret_scalar(marrow_schema::ScalarType::Bool), "Returns true when the saved path exists."),
    descriptor("nextId", CheckedBuiltinCall::NextId, ROOT, ret_value(CheckedBuiltinValueShape::Identity), "Returns the next id for a saved root."),
    descriptor("append", CheckedBuiltinCall::Append, LAYER_VALUE, ret_scalar(marrow_schema::ScalarType::Int), "Appends a value to a layer and returns its key."),
    descriptor("keys", CheckedBuiltinCall::Keys, COLLECTION, ret_value(CheckedBuiltinValueShape::Sequence), "Returns the keys in a collection."),
    descriptor("count", CheckedBuiltinCall::Count, COLLECTION, ret_scalar(marrow_schema::ScalarType::Int), "Returns child count for a saved path, 1 for a scalar, or 0 when absent."),
    descriptor("values", CheckedBuiltinCall::Values, COLLECTION, ret_value(CheckedBuiltinValueShape::Sequence), "Returns the values in a collection."),
    descriptor("entries", CheckedBuiltinCall::Entries, COLLECTION, ret_value(CheckedBuiltinValueShape::Sequence), "Returns the entries in a collection."),
    descriptor("reversed", CheckedBuiltinCall::Reversed, COLLECTION, ret_value(CheckedBuiltinValueShape::Sequence), "Returns the collection in reverse order."),
    descriptor("next", CheckedBuiltinCall::Next, PATH, ret_value(CheckedBuiltinValueShape::Value), "Returns the next key after a saved path."),
    descriptor("prev", CheckedBuiltinCall::Prev, PATH, ret_value(CheckedBuiltinValueShape::Value), "Returns the previous key before a saved path."),
];

const fn descriptor(
    spelling: &'static str,
    call: CheckedBuiltinCall,
    params: &'static [CheckedBuiltinCallParameter],
    return_shape: CheckedBuiltinReturnShape,
    docs: &'static str,
) -> CheckedBuiltinCallDescriptor {
    CheckedBuiltinCallDescriptor {
        spelling,
        call,
        params,
        return_shape,
        docs,
    }
}

const fn param(
    label: &'static str,
    shape: CheckedBuiltinValueShape,
) -> CheckedBuiltinCallParameter {
    CheckedBuiltinCallParameter { label, shape }
}

const fn ret_void() -> CheckedBuiltinReturnShape {
    CheckedBuiltinReturnShape::Void
}

const fn ret_scalar(scalar: marrow_schema::ScalarType) -> CheckedBuiltinReturnShape {
    ret_value(CheckedBuiltinValueShape::Scalar(scalar))
}

const fn ret_value(shape: CheckedBuiltinValueShape) -> CheckedBuiltinReturnShape {
    CheckedBuiltinReturnShape::Value(shape)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_descriptors_round_trip_through_call_identity() {
        let mut spellings = std::collections::HashSet::new();
        for descriptor in BUILTIN_CALLS {
            assert!(
                spellings.insert(descriptor.spelling),
                "duplicate builtin descriptor for {}",
                descriptor.spelling
            );
            assert_eq!(
                CheckedBuiltinCall::from_name(descriptor.spelling),
                Some(descriptor.call)
            );
        }

        assert_eq!(CheckedBuiltinCall::from_name("write"), None);
    }
}
