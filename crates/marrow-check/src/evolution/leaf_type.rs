//! The identity-aware leaf token a resource member's durable bytes are accepted as.
//!
//! A leaf type change is the soundness hazard evolution must catch: bytes written under
//! one type must not be silently reread under another whose decoder happens to accept
//! them. Detecting it by comparing source spellings is wrong, because a spelling moves
//! under a pure rename: an enum renamed `Status -> State`, or a store root renamed
//! `^books -> ^library`, keeps every durable byte but changes how the type is written.
//!
//! The token names the *identity* the bytes were accepted under, not its spelling:
//!
//! - a scalar by its canonical name (`int`, `string`, ...);
//! - an enum by the stable catalog id of the enum it refers to;
//! - a store identity by the stable catalog id of the store it refers to, plus the
//!   identity arity.
//!
//! A pure rename leaves the referent's stable catalog id unchanged, so the token is
//! unchanged and the member is not a retype. A change across scalar/enum/identity, or
//! from one enum or store to a different one, changes the token and is a retype.

use std::collections::HashMap;

use marrow_project::CatalogEntryKind;
use marrow_schema::{KeyDef, Type};

use crate::CheckedProgram;
use crate::catalog::{CatalogKey, enum_path, store_path};
use crate::resolve::resolve_store_by_root;

/// The value-type token recorded for a leaf-position member whose declared value type
/// produces no identity token: an `unknown`, or a value cell that resolves to a `sequence`
/// (`map[K, sequence[V]]`), or any future leaf type the saved model cannot tokenize. A leaf
/// position always carries a comparable value token so a retype across the
/// tokenizable/non-tokenizable boundary is detected like any other.
const UNTOKENIZABLE_VALUE: &str = "untokenizable";

/// The identity-aware leaf token of a leaf-position member: its key-param shape and its
/// value-type token together, so a member that holds a single value cell — a plain field or
/// a keyed-leaf layer (a desugared `sequence`/`map`) — always yields a comparable token. A
/// plain `string` (`string`) and a `string(pos: int)` keyed leaf (`[int]string`) carry
/// different tokens, so a plain field becoming a keyed leaf, or a keyed leaf's key arity or
/// key type changing, is a retype the same way a value-type change is. The value token names
/// the referent's stable identity, not its spelling, so a pure enum or store rename is not a
/// retype; a tokenizable value type whose referent has no bound catalog id yet (a pending
/// first-run identity) returns `None`, since an unresolved referent cannot be compared stably.
fn member_leaf_token(
    program: &CheckedProgram,
    module: &str,
    ty: &Type,
    key_params: &[KeyDef],
    ids: &HashMap<CatalogKey, String>,
) -> Option<String> {
    let value = match ty {
        Type::Sequence(_) | Type::Unknown => UNTOKENIZABLE_VALUE.to_string(),
        _ => leaf_type_token(program, module, ty, ids)?,
    };
    if key_params.is_empty() {
        Some(value)
    } else {
        Some(format!("[{}]{value}", store_key_shape_token(key_params)))
    }
}

/// The identity-aware leaf token for a member declared with `ty` in `module`. The enum or
/// store referent's stable catalog id is read from `ids` (the binding map for current
/// source, which preserves a referent's id across a rename); its module and arity come
/// from `program`. `None` for a member with no single leaf cell (a group, keyed layer, or
/// a `sequence`/`unknown` type the saved model rejects) and for an enum or store whose
/// referent has no bound catalog id yet (a pending first-run identity), since an
/// unresolved referent cannot be compared stably.
fn leaf_type_token(
    program: &CheckedProgram,
    module: &str,
    ty: &Type,
    ids: &HashMap<CatalogKey, String>,
) -> Option<String> {
    match ty {
        Type::Scalar(scalar) => Some(scalar.name().to_string()),
        Type::Identity(root) => {
            let store = resolve_store_by_root(program, root)?;
            let store_id = ids.get(&CatalogKey::new(
                CatalogEntryKind::Store,
                store_path(&store.module.name, root),
            ))?;
            let arity = store.store.identity_keys.len();
            Some(format!("id:{store_id}:{arity}"))
        }
        Type::Named(name) => {
            let (enum_module, enum_name) =
                name.rsplit_once("::").unwrap_or((module, name.as_str()));
            let enum_id = ids.get(&CatalogKey::new(
                CatalogEntryKind::Enum,
                enum_path(enum_module, enum_name),
            ))?;
            Some(format!("enum:{enum_id}"))
        }
        Type::Sequence(_) | Type::Unknown => None,
    }
}

/// The identity-key shape token of a store: the comma-joined canonical spellings of its
/// identity keys in order, so both the arity and each key type are recorded (`int`,
/// `int,string`). A keyless singleton renders the empty string. The token names the
/// physical key shape durable records are addressed under, not the key parameter names, so
/// a pure key-parameter rename leaves it unchanged; a key-type or arity change does not.
pub(crate) fn store_key_shape_token(identity_keys: &[KeyDef]) -> String {
    identity_keys
        .iter()
        .map(|key| key.ty.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// The identity-aware structural signature of a resource member: the shape its durable data
/// occupies, by kind and identity, not source spelling. It is the comparison the default-deny
/// backstop fails closed on, so it must distinguish every structural shape a member can take:
///
/// - a leaf (a plain field or a keyed-leaf `map[K, V]`) records `leaf:<member-leaf-token>`,
///   where the leaf token already carries the value type by referent identity and the key
///   shape of a keyed-leaf map, so a value retype or a keyed-leaf re-key reads as a different
///   signature;
/// - an unkeyed group records `group`;
/// - a keyed group records `keyed-group:[<key-shape>]`, so a keyed-layer re-key (key type or
///   arity) and a plain-group<->keyed-group reshape both read as a different signature.
///
/// `None` only when a leaf member's value type cannot be tokenized stably yet (a pending
/// first-run referent), mirroring [`member_leaf_token`]; a group always has a signature, since
/// its shape needs no referent resolution. The token names identity, so a pure enum, store, or
/// key-parameter rename leaves it unchanged.
pub(crate) fn member_struct_token(
    program: &CheckedProgram,
    module: &str,
    leaf: Option<&Type>,
    key_params: &[KeyDef],
    ids: &HashMap<CatalogKey, String>,
) -> Option<String> {
    match leaf {
        Some(ty) => {
            let token = member_leaf_token(program, module, ty, key_params, ids)?;
            Some(format!("leaf:{token}"))
        }
        None if key_params.is_empty() => Some("group".to_string()),
        None => Some(format!(
            "keyed-group:[{}]",
            store_key_shape_token(key_params)
        )),
    }
}
