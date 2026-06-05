//! Saved-path lowering near the managed-write layer.

use marrow_check::{
    CheckedArg as ExecArg, CheckedExpr as ExecExpr, CheckedSavedKeyParam, CheckedSavedMember,
    CheckedSavedPlace, CheckedSavedTerminal, StoreLeafKind,
};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::{ReadPosition, absent_read};
use crate::durable_read::read_resource;
use crate::env::Env;
use crate::error::{Located, RUN_TYPE, RuntimeError, key_type_fault, type_error, unsupported};
use crate::expr::eval_expr;
use crate::read::iterable_index_branch_present;
use crate::stdlib::exact_unique_index_lookup_value;
use crate::store::{DataAddress, LayerAddress, catalog_id, data_exists, read_data};
use crate::value::{Value, decode_leaf, value_to_key};
use crate::write_dispatch::{write_nested_field, write_resource, write_saved_field};

/// A saved path lowered from its source expression: the saved root, the record
/// identity keys, the chain of group/keyed-layer levels from outermost to
/// innermost, and how the path terminates. One [`lower`] pass walks the call/field
/// spine once and produces this; every saved record write, delete, layer
/// read, and traversal then consumes these fields directly.
///
/// Callers always peel the trailing scalar field off the spine before lowering its
/// base, so `lower` is given a record, layer, or index path — never a trailing
/// `.field`. Each `.name` off a saved path it walks is therefore a group/layer hop,
/// and the only non-record terminal it produces is an index branch.
pub(crate) struct SavedPath {
    pub(crate) place: CheckedSavedPlace,
    pub(crate) members: Vec<CheckedSavedMember>,
    /// The record identity keys (empty for a keyless singleton).
    pub(crate) identity: Vec<SavedKey>,
    /// The `(layer, key…)` levels, outermost first; keys are empty for an unkeyed
    /// group hop (`^root(id).name`) and present for a keyed layer (`.layer(key…)`).
    pub(crate) layers: Vec<(String, Vec<SavedKey>)>,
    pub(crate) layer_addresses: Vec<LayerAddress>,
    pub(crate) terminal: Terminal,
}

/// How a [`SavedPath`] terminates.
pub(crate) enum Terminal {
    /// The path stops at the record or group entry itself (`^root(id)`,
    /// `^root(id).layer(k)`).
    Record,
    /// The path stops at a named scalar field of the record or innermost group
    /// entry (`^root(id).field`, `^root(id).layer(k).field`). Produced when a place
    /// resolution peels the trailing `.field` onto an otherwise-`Record` path.
    Field {
        name: String,
        catalog_id: Option<String>,
        leaf: Option<StoreLeafKind>,
    },
    /// A declared index branch `^root.index(args…)`. It hangs directly off the root
    /// with no record identity or layer chain.
    Index,
}

impl SavedPath {
    /// The current value at this lowered path. A `Terminal::Field` reads that
    /// scalar field (top-level or inside the group chain), decoding it with the
    /// field's declared type; a `Terminal::Record` reads the whole record. An
    /// unpopulated element raises an absent-element fault, catchable or fatal per
    /// `position`.
    pub(crate) fn read(
        &self,
        position: ReadPosition,
        span: SourceSpan,
        env: &mut Env<'_>,
    ) -> Result<Value, RuntimeError> {
        let Terminal::Field {
            name: field,
            catalog_id,
            leaf,
        } = &self.terminal
        else {
            return match self.terminal {
                Terminal::Record => read_resource(&self.place, &self.identity, span, env),
                Terminal::Index => Err(unsupported("reading this saved path", span)),
                Terminal::Field { .. } => unreachable!("guarded by the let-else"),
            };
        };
        let leaf = leaf.as_ref().ok_or_else(|| {
            let what = if self.layers.is_empty() {
                "reading this field"
            } else {
                "reading this group field"
            };
            unsupported(what, span)
        })?;
        let address = DataAddress::member(
            &self.place,
            &self.identity,
            &self.layer_addresses,
            catalog_id,
            span,
        )?;
        let bytes = read_data(env.store, &address, span)?;
        let Some(bytes) = bytes else {
            // A top-level field reads "is absent"; a group-entry field "entry is
            // absent", keeping each read's message as it was.
            let what = if self.layers.is_empty() {
                format!("`{field}` is absent")
            } else {
                format!("`{field}` entry is absent")
            };
            return Err(absent_read(position, what, span));
        };
        decode_leaf(env.program, &bytes, leaf).ok_or_else(|| {
            RuntimeError::fault(
                RUN_TYPE,
                format!("stored value for `{field}` did not decode to a runtime value"),
                span,
            )
        })
    }

    /// Write `value` to this lowered path, routing a scalar field or whole-record
    /// write the same way a direct assignment to the path would. Shared by direct
    /// saved writes.
    pub(crate) fn write(
        self,
        value: Value,
        span: SourceSpan,
        env: &mut Env<'_>,
    ) -> Result<(), RuntimeError> {
        let field = match &self.terminal {
            Terminal::Field { name, .. } => Some(name.clone()),
            Terminal::Record => None,
            Terminal::Index => return Err(unsupported("writing this saved path", span)),
        };
        match field {
            Some(field) if self.layers.is_empty() => {
                write_saved_field(self, &field, value, span, env)
            }
            Some(field) => write_nested_field(self, &field, value, span, env),
            None => write_resource(self, value, span, env),
        }
    }
}

/// Lower a checked saved-place descriptor to the concrete runtime keys it names.
/// The checker already resolved root, layer, index, and field shape; runtime only
/// evaluates the key expressions and preserves the checked terminal.
pub(crate) fn lower(expr: &ExecExpr, env: &mut Env<'_>) -> Result<SavedPath, RuntimeError> {
    let place = expr
        .saved_place()
        .ok_or_else(|| unsupported("this saved path", expr.span()))?;
    lower_checked(place, env)
}

fn lower_checked(place: &CheckedSavedPlace, env: &mut Env<'_>) -> Result<SavedPath, RuntimeError> {
    let identity = if matches!(place.terminal, CheckedSavedTerminal::Index { .. }) {
        Vec::new()
    } else if place.identity_args.is_empty() && !place.identity_keys.is_empty() {
        Err(type_error(
            &format!(
                "`^{}` expects {} identity key(s), got 0; address a record with `^{}(id)`",
                place.root,
                place.identity_keys.len(),
                place.root
            ),
            place.span,
        ))?
    } else {
        lower_keys(
            &place.identity_args,
            place.span,
            true,
            Some(&place.root),
            &place.identity_keys,
            env,
        )?
    };
    let mut layers = Vec::with_capacity(place.layers.len());
    let mut layer_addresses = Vec::with_capacity(place.layers.len());
    for layer in &place.layers {
        let keys = lower_keys(&layer.args, layer.span, false, None, &layer.key_params, env)?;
        layer_addresses.push(LayerAddress::from_checked(layer, keys.clone()));
        layers.push((layer.name.clone(), keys));
    }
    let terminal = match &place.terminal {
        CheckedSavedTerminal::Record => Terminal::Record,
        CheckedSavedTerminal::Field {
            name,
            catalog_id,
            leaf,
        } => Terminal::Field {
            name: name.clone(),
            catalog_id: catalog_id.clone(),
            leaf: leaf.clone(),
        },
        CheckedSavedTerminal::Index { .. } => Terminal::Index,
    };
    Ok(SavedPath {
        place: place.clone(),
        members: place.members.clone(),
        identity,
        layers,
        layer_addresses,
        terminal,
    })
}

pub(crate) fn direct_root_place(expr: &ExecExpr) -> Option<&CheckedSavedPlace> {
    let place = expr.saved_place()?;
    (matches!(place.terminal, CheckedSavedTerminal::Record)
        && place.identity_args.is_empty()
        && place.layers.is_empty())
    .then_some(place)
}

/// Whether a saved path holds a value or any children — the presence test behind
/// `exists` and `std::assert::absent`. A declared index branch has no
/// record/layer segment form, so — exactly as `count` does — its presence is
/// whether the branch enumerates any entries.
pub(crate) fn saved_path_present(
    expr: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<bool, RuntimeError> {
    if let Some(place) = direct_root_place(expr).filter(|place| !place.identity_keys.is_empty()) {
        let store = catalog_id(&place.store_catalog_id, "store", span)?;
        return env
            .store
            .record_first_child(&store, &[])
            .map(|key| key.is_some())
            .map_err(|error| error.located(span));
    }
    if let Some(value) = exact_unique_index_lookup_value(expr, span, env)? {
        return Ok(value.is_present());
    }
    if let Some(present) = iterable_index_branch_present(expr, env)? {
        return Ok(present);
    }
    let path = lower(expr, env)?;
    let address = match &path.terminal {
        Terminal::Record if path.layer_addresses.is_empty() => {
            DataAddress::record(&path.place, &path.identity, span)?
        }
        Terminal::Record => {
            DataAddress::layer_prefix(&path.place, &path.identity, &path.layer_addresses, span)?
        }
        Terminal::Field { catalog_id, .. } => DataAddress::member(
            &path.place,
            &path.identity,
            &path.layer_addresses,
            catalog_id,
            span,
        )?,
        Terminal::Index => return Ok(false),
    };
    data_exists(env.store, &address, span)
}

/// Evaluate a keyed lookup's arguments to saved key segments, rejecting named or
/// mode arguments. When `allow_identity_splice` (the record-identity position), a
/// sole identity-valued argument (`^root(id)` where `id: Id(^root)`) splices its
/// lowered keys in as the full key vector and an identity mixed with raw keys is
/// rejected; otherwise (a keyed layer or index lookup) each argument is one raw
/// key.
pub(crate) fn lower_keys(
    args: &[ExecArg],
    span: SourceSpan,
    allow_identity_splice: bool,
    expected_root: Option<&str>,
    expected: &[CheckedSavedKeyParam],
    env: &mut Env<'_>,
) -> Result<Vec<SavedKey>, RuntimeError> {
    if args
        .iter()
        .any(|arg| arg.mode.is_some() || arg.name.is_some())
    {
        return Err(unsupported(
            "a keyed lookup with named or inout arguments",
            span,
        ));
    }
    let mut keys = Vec::with_capacity(args.len());
    for (position, arg) in args.iter().enumerate() {
        match eval_expr(&arg.value, env)? {
            // An identity is the whole lookup key only as the sole argument of a
            // record lookup; it cannot be one component among raw keys. Its root
            // provenance must match the checked root before its keys are spliced.
            Value::Identity(identity) if allow_identity_splice && args.len() == 1 => {
                let Some(expected_root) = expected_root else {
                    return Err(unsupported("an identity splice without a saved root", span));
                };
                let keys = identity.into_keys_for_root(expected_root, span)?;
                check_spliced_identity(&keys, expected, span)?;
                return Ok(keys);
            }
            Value::Identity(_) if allow_identity_splice => {
                return Err(unsupported("an identity mixed with other keys", span));
            }
            value => {
                let key =
                    value_to_key(value).ok_or_else(|| unsupported("a key of this type", span))?;
                // Guard the key's scalar kind against the declared key type, so a
                // wrong-typed key faults here rather than corrupting the keyspace.
                // An unresolved schema passes no expectations, so the guard skips
                // and arity faults still fire downstream.
                if let Some(def) = expected.get(position) {
                    guard_key_type(def, &key, span)?;
                }
                keys.push(key);
            }
        }
    }
    Ok(keys)
}

/// Guard a spliced identity's key bytes against the checked target keyspace.
/// Checked places own nominal root identity; this catches byte-shape mismatches
/// before address lowering.
pub(crate) fn check_spliced_identity(
    identity: &[SavedKey],
    expected: &[CheckedSavedKeyParam],
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    if expected.is_empty() {
        return Ok(());
    }
    if identity.len() != expected.len() {
        return Err(type_error(
            &format!(
                "an identity with {} key(s) was spliced where {} is declared",
                identity.len(),
                expected.len()
            ),
            span,
        ));
    }
    for (key, def) in identity.iter().zip(expected) {
        guard_key_type(def, key, span)?;
    }
    Ok(())
}

/// Guard one lowered key's scalar kind against its declared key type, the single
/// typed-keyspace check every key path shares: record lookups, layer lookups, and
/// spliced identities all route through it. A non-scalar (defer) declaration
/// passes no expectation, so the guard skips and any arity fault still fires
/// downstream. A wrong scalar is a `key_type_fault`.
pub(crate) fn guard_key_type(
    declared: &CheckedSavedKeyParam,
    key: &SavedKey,
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    if let Some(expected) = declared.scalar
        && expected != key.scalar_type()
    {
        return Err(key_type_fault(expected, key.scalar_type(), span));
    }
    Ok(())
}
