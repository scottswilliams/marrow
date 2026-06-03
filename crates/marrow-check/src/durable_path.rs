//! Checked classification for decoded durable store paths.

use marrow_schema::{KeyDef, Type};
use marrow_store::path::{PathSegment, SavedKey};
use marrow_store::value::ScalarType;

use crate::CheckedProgram;
use crate::facts::EnumId;
use crate::resolve::resolve_store_by_root;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorePathClass {
    Scalar(ScalarType),
    Identity {
        store_root: String,
        arity: usize,
    },
    IndexMarker,
    KeyTypeMismatch {
        expected: ScalarType,
        found: ScalarType,
    },
    Orphan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LeafKind {
    Scalar(ScalarType),
    Identity { store_root: String, arity: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreLeafKind {
    Scalar(ScalarType),
    Enum { enum_id: EnumId },
    Identity { store_root: String, arity: usize },
}

pub fn classify_store_path(program: &CheckedProgram, segments: &[PathSegment]) -> StorePathClass {
    let Some((PathSegment::Root(root), rest)) = segments.split_first() else {
        return StorePathClass::Orphan;
    };
    let Some(arity) = checked_root_identity_arity(program, root) else {
        return StorePathClass::Orphan;
    };

    let identity_keys = rest
        .iter()
        .take_while(|segment| matches!(segment, PathSegment::RecordKey(_)))
        .count();
    let after_identity = &rest[identity_keys..];

    if identity_keys == 0
        && let Some((PathSegment::Field(name), keys)) = after_identity.split_first()
        && keys
            .iter()
            .all(|segment| matches!(segment, PathSegment::IndexKey(_)))
    {
        let Some(store) = resolve_store_by_root(program, root) else {
            return StorePathClass::Orphan;
        };
        if store.store.indexes.iter().any(|index| index.name == *name) {
            return StorePathClass::IndexMarker;
        }
        if arity == 0 {
            return classify_member(program, root, after_identity);
        }
        return StorePathClass::Orphan;
    }

    if identity_keys != arity {
        return StorePathClass::Orphan;
    }
    if let Some(store) = resolve_store_by_root(program, root)
        && let Some(mismatch) = key_type_mismatch(
            &store.store.identity_keys,
            rest[..identity_keys].iter().filter_map(record_key),
        )
    {
        return mismatch;
    }
    classify_member(program, root, after_identity)
}

pub fn identity_leaf_key_mismatch(
    program: &CheckedProgram,
    store_root: &str,
    keys: &[SavedKey],
) -> Option<(ScalarType, ScalarType)> {
    let declared = checked_identity_key_defs(program, store_root)?;
    match key_type_mismatch(declared, keys.iter()) {
        Some(StorePathClass::KeyTypeMismatch { expected, found }) => Some((expected, found)),
        _ => None,
    }
}

fn classify_member(
    program: &CheckedProgram,
    root: &str,
    members: &[PathSegment],
) -> StorePathClass {
    let mut named: Vec<(&str, Vec<&SavedKey>)> = Vec::new();
    for segment in members {
        match segment {
            PathSegment::Field(name) | PathSegment::ChildLayer(name) | PathSegment::Index(name) => {
                named.push((name.as_str(), Vec::new()));
            }
            PathSegment::IndexKey(key) => match named.last_mut() {
                Some((_, keys)) => keys.push(key),
                None => return StorePathClass::Orphan,
            },
            PathSegment::RecordKey(_) | PathSegment::Root(_) => return StorePathClass::Orphan,
        }
    }
    let names: Vec<&str> = named.iter().map(|(name, _)| *name).collect();
    let Some((&last, layers)) = names.split_last() else {
        return StorePathClass::Orphan;
    };

    if let Some(store) = resolve_store_by_root(program, root) {
        for (depth, (name, keys)) in named.iter().enumerate() {
            if keys.is_empty() {
                continue;
            }
            let chain: Vec<&str> = names[..depth]
                .iter()
                .copied()
                .chain(std::iter::once(*name))
                .collect();
            let Some(node) = store.resource.descend_layers(&chain) else {
                continue;
            };
            if let Some(mismatch) = key_type_mismatch(&node.key_params, keys.iter().copied()) {
                return mismatch;
            }
        }
    }

    if layers.is_empty() {
        if let Some(leaf) = resource_field_leaf(program, root, last) {
            return leaf_class(leaf);
        }
        if let Some(leaf) = resource_layer_leaf(program, root, last) {
            return leaf_class(leaf);
        }
        return StorePathClass::Orphan;
    }

    if let Some(leaf) = resource_nested_member_leaf(program, root, layers, last) {
        return leaf_class(leaf);
    }
    StorePathClass::Orphan
}

fn leaf_class(leaf: LeafKind) -> StorePathClass {
    match leaf {
        LeafKind::Scalar(ty) => StorePathClass::Scalar(ty),
        LeafKind::Identity { store_root, arity } => StorePathClass::Identity { store_root, arity },
    }
}

fn key_type_mismatch<'a>(
    declared: &[KeyDef],
    found: impl Iterator<Item = &'a SavedKey>,
) -> Option<StorePathClass> {
    declared
        .iter()
        .zip(found)
        .find_map(|(def, key)| match def.ty.scalar() {
            Some(expected) if expected != key.scalar_type() => {
                Some(StorePathClass::KeyTypeMismatch {
                    expected,
                    found: key.scalar_type(),
                })
            }
            _ => None,
        })
}

fn record_key(segment: &PathSegment) -> Option<&SavedKey> {
    match segment {
        PathSegment::RecordKey(key) => Some(key),
        _ => None,
    }
}

fn checked_root_identity_arity(program: &CheckedProgram, root: &str) -> Option<usize> {
    resolve_store_by_root(program, root).map(|store| store.store.identity_keys.len())
}

fn checked_identity_key_defs<'p>(program: &'p CheckedProgram, root: &str) -> Option<&'p [KeyDef]> {
    resolve_store_by_root(program, root).map(|store| store.store.identity_keys.as_slice())
}

fn leaf_kind(program: &CheckedProgram, ty: &Type) -> Option<LeafKind> {
    match ty {
        Type::Identity(root) => {
            let identity_keys = checked_identity_key_defs(program, root)?;
            Some(LeafKind::Identity {
                store_root: root.clone(),
                arity: identity_keys.len(),
            })
        }
        other => other.stored_scalar().map(LeafKind::Scalar),
    }
}

fn resource_field_leaf(program: &CheckedProgram, root: &str, field: &str) -> Option<LeafKind> {
    let ty = resolve_store_by_root(program, root)?
        .resource
        .field_type(&[field])?
        .clone();
    leaf_kind(program, &ty)
}

fn resource_layer_leaf(program: &CheckedProgram, root: &str, layer: &str) -> Option<LeafKind> {
    let ty = resolve_store_by_root(program, root)?
        .resource
        .leaf_type(&[layer])?
        .clone();
    leaf_kind(program, &ty)
}

fn resource_nested_member_leaf(
    program: &CheckedProgram,
    root: &str,
    layers: &[&str],
    field: &str,
) -> Option<LeafKind> {
    let resource = resolve_store_by_root(program, root)?.resource;
    let mut chain = layers.to_vec();
    chain.push(field);
    let ty = resource.field_type(&chain)?.clone();
    leaf_kind(program, &ty)
}
