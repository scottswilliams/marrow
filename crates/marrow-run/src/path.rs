//! Saved-path lowering near the managed-write layer.

use marrow_check::{
    CheckedArg as ExecArg, CheckedExpr as ExecExpr, CheckedSavedKeyParam, CheckedSavedPlace,
    CheckedSavedTerminal, StoreLeafKind,
};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::absent_read;
use crate::durable_read::{LayerEntryAddress, read_layer_entry_at, read_resource};
use crate::env::Env;
use crate::error::{RUN_ABSENT, RUN_TYPE, RuntimeError, key_type_fault, type_error, unsupported};
use crate::expr::eval_expr;
use crate::read::{
    iterable_index_branch_present, root_identity_present, validated_data_layer_present,
};
use crate::stdlib::exact_unique_index_lookup_value;
use crate::store::{DataAddress, LayerAddress, data_exists, read_data};
use crate::value::{Value, decode_leaf, value_to_key};
use crate::write_dispatch::{write_nested_field, write_resource, write_saved_field};

/// A saved path lowered from its source expression by one [`lower`] pass over the
/// call/field spine.
///
/// Callers always peel the trailing scalar field off the spine before lowering its
/// base, so `lower` is given a record, layer, or index path — never a trailing
/// `.field`. Each `.name` it walks is therefore a group/layer hop, and the only
/// non-record terminal it produces is an index branch.
pub(crate) struct SavedPath {
    pub(crate) place: CheckedSavedPlace,
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
    /// The record or group entry itself (`^root(id)`, `^root(id).layer(k)`).
    Record,
    /// A named scalar field of the record or innermost group entry, produced when a
    /// place resolution peels the trailing `.field` onto an otherwise-`Record` path.
    Field {
        name: String,
        catalog_id: Option<String>,
        leaf: Option<StoreLeafKind>,
    },
    /// A declared index branch `^root.index(args…)`, hanging directly off the root
    /// with no record identity or layer chain.
    Index,
}

impl SavedPath {
    /// The current value at this lowered path. A `Terminal::Field` reads that
    /// scalar field (top-level or inside the group chain), decoding it with the
    /// field's declared type; a `Terminal::Record` reads the whole record. An
    /// unpopulated element raises a fatal absent-element fault. Read-site
    /// resolution probes presence before calling this fixed-address read.
    pub(crate) fn read(&self, span: SourceSpan, env: &mut Env<'_>) -> Result<Value, RuntimeError> {
        match &self.terminal {
            Terminal::Record if self.layer_addresses.is_empty() => {
                read_resource(&self.place, &self.identity, span, env)
            }
            Terminal::Record => {
                let Some(layer_facts) = self.place.layers.last() else {
                    return Err(unsupported("reading this saved path", span));
                };
                read_layer_entry_at(
                    LayerEntryAddress {
                        place: &self.place,
                        identity: &self.identity,
                        layers: &self.layer_addresses,
                        layer_facts,
                    },
                    span,
                    env,
                )
            }
            Terminal::Index => Err(unsupported("reading this saved path", span)),
            Terminal::Field {
                name: field,
                catalog_id,
                leaf,
            } => {
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
                    // A group-entry field's absence is reported against its entry, a
                    // top-level field's against the field itself.
                    let what = if self.layers.is_empty() {
                        format!("`{field}` is absent")
                    } else {
                        format!("`{field}` entry is absent")
                    };
                    return Err(absent_read(what, span));
                };
                decode_leaf(env.program, &bytes, leaf).ok_or_else(|| {
                    RuntimeError::fault(
                        RUN_TYPE,
                        format!("stored value for `{field}` did not decode to a runtime value"),
                        span,
                    )
                })
            }
        }
    }

    pub(crate) fn is_present(&self, span: SourceSpan, env: &Env<'_>) -> Result<bool, RuntimeError> {
        let address = match &self.terminal {
            Terminal::Record if self.layer_addresses.is_empty() => {
                DataAddress::record(&self.place, &self.identity, span)?
            }
            Terminal::Record => {
                let address = DataAddress::layer_prefix(
                    &self.place,
                    &self.identity,
                    &self.layer_addresses,
                    span,
                )?;
                if let (Some(layer), Some(layer_address)) =
                    (self.place.layers.last(), self.layer_addresses.last())
                    && layer_address.keys.len() < layer.key_params.len()
                {
                    return validated_data_layer_present(
                        env.store,
                        &address,
                        &layer.key_params,
                        layer_address.keys.len(),
                        span,
                    );
                }
                address
            }
            Terminal::Field { catalog_id, .. } => DataAddress::member(
                &self.place,
                &self.identity,
                &self.layer_addresses,
                catalog_id,
                span,
            )?,
            Terminal::Index => return Ok(false),
        };
        data_exists(env.store, &address, span)
    }

    /// Routes the write through the terminal variant, delegating to the
    /// field or record write handler that a direct assignment would use.
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
/// The checker already resolved the path shape; runtime only evaluates the key
/// expressions and preserves the checked terminal.
pub(crate) fn lower(expr: &ExecExpr, env: &mut Env<'_>) -> Result<SavedPath, RuntimeError> {
    let place = expr
        .saved_place()
        .ok_or_else(|| unsupported("this saved path", expr.span()))?;
    lower_checked(place, env)
}

/// Lower a saved path for a read or presence probe, folding a position that
/// addresses no node into `None`. A non-positive sequence position anywhere on the
/// spine lowers to the catchable absent fault; lowering is pure address
/// resolution, so that fault means the whole address names no node. A write
/// surfaces it, but a probe resolves it: the `??`/`if const`/`exists` site treats
/// it as absence rather than propagating it.
pub(crate) fn lower_for_probe(
    expr: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<Option<SavedPath>, RuntimeError> {
    match lower(expr, env) {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.code() == RUN_ABSENT && error.is_catchable() => Ok(None),
        Err(error) => Err(error),
    }
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
            KeyRole::IdentityRead,
            Some(&place.root),
            &place.identity_keys,
            env,
        )?
    };
    let mut layers = Vec::with_capacity(place.layers.len());
    let mut layer_addresses = Vec::with_capacity(place.layers.len());
    for layer in &place.layers {
        let keys = lower_keys(
            &layer.args,
            layer.span,
            KeyRole::Layer,
            None,
            &layer.key_params,
            env,
        )?;
        layer_addresses.push(LayerAddress::from_checked(layer, keys.clone()));
        layers.push((layer.name.clone(), keys));
    }
    let terminal = match &place.terminal {
        CheckedSavedTerminal::Record => Terminal::Record,
        CheckedSavedTerminal::Field {
            name,
            catalog_id,
            leaf,
            ..
        } => Terminal::Field {
            name: name.clone(),
            catalog_id: catalog_id.clone(),
            leaf: leaf.clone(),
        },
        CheckedSavedTerminal::Index { .. } => Terminal::Index,
    };
    Ok(SavedPath {
        place: place.clone(),
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
        return root_identity_present(place, span, env);
    }
    if let Some(value) = exact_unique_index_lookup_value(expr, span, env)? {
        return Ok(value.is_present());
    }
    if let Some(present) = iterable_index_branch_present(expr, env)? {
        return Ok(present);
    }
    match lower_for_probe(expr, env)? {
        Some(path) => path.is_present(span, env),
        None => Ok(false),
    }
}

/// Which keyspace a [`lower_keys`] call addresses. The role decides two
/// independent things: whether a sole identity-valued argument may splice its
/// keys in (only a record-identity read carries an identity to splice), and
/// whether the 1-based sequence-position rule applies (only a saved layer is a
/// sequence; record identity is keyed independently and may hold any int).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyRole {
    /// A record-identity read, whose sole identity argument may splice.
    IdentityRead,
    /// Raw record-identity key components: the `Id(^store, …)` constructor and
    /// identity-range prefixes. No splice and no sequence guard.
    IdentityKeys,
    /// A saved layer position, subject to the 1-based sequence rule.
    Layer,
}

/// Evaluate a keyed lookup's arguments to saved key segments, rejecting named
/// arguments. Each argument is one raw key, except that an [`KeyRole::IdentityRead`]
/// with a sole identity-valued argument splices that identity's lowered keys in as
/// the full key vector.
pub(crate) fn lower_keys(
    args: &[ExecArg],
    span: SourceSpan,
    role: KeyRole,
    expected_root: Option<&str>,
    expected: &[CheckedSavedKeyParam],
    env: &mut Env<'_>,
) -> Result<Vec<SavedKey>, RuntimeError> {
    if args.iter().any(|arg| arg.name.is_some()) {
        return Err(unsupported("a keyed lookup with named arguments", span));
    }
    let mut keys = Vec::with_capacity(args.len());
    for (position, arg) in args.iter().enumerate() {
        match eval_expr(&arg.value, env)? {
            // An identity is the whole lookup key only as the sole argument of a
            // record lookup; it cannot be one component among raw keys. Its root
            // provenance must match the checked root before its keys are spliced.
            Value::Identity(identity) if role == KeyRole::IdentityRead && args.len() == 1 => {
                let Some(expected_root) = expected_root else {
                    return Err(unsupported("an identity splice without a saved root", span));
                };
                let keys = identity.into_keys_for_root(expected_root, span)?;
                check_spliced_identity(&keys, expected, span)?;
                return Ok(keys);
            }
            Value::Identity(_) if role == KeyRole::IdentityRead => {
                return Err(unsupported("an identity mixed with other keys", span));
            }
            value => {
                let key = value_to_key(value, span)?
                    .ok_or_else(|| unsupported("a key of this type", span))?;
                // Guard the key's scalar kind against the declared key type, so a
                // wrong-typed key faults here rather than corrupting the keyspace.
                // An unresolved schema passes no expectations, so the guard skips
                // and arity faults still fire downstream.
                if let Some(def) = expected.get(position) {
                    guard_key_type(def, &key, span)?;
                }
                // The 1-based sequence rule is a property of a saved layer, never
                // of record identity: a non-positive single-int record key is a
                // valid identity, so the guard fires only for a layer position.
                if role == KeyRole::Layer {
                    guard_sequence_position(expected, &key, span)?;
                }
                keys.push(key);
            }
        }
    }
    Ok(keys)
}

/// Guard a spliced identity's arity and key bytes against the checked target
/// keyspace, catching byte-shape mismatches before address lowering.
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

/// A single int-keyed layer is the canonical sequence shape, whose positions are
/// 1-based: a position below 1 addresses no node. Reject it as absent so a read
/// resolves it through `??`/`if const`/`exists`/`catch` and a write raises before
/// any store mutation, never persisting an unreachable element. Only a
/// [`KeyRole::Layer`] reaches this guard; record identity is keyed independently,
/// so a non-positive record id stays a valid key.
fn guard_sequence_position(
    expected: &[CheckedSavedKeyParam],
    key: &SavedKey,
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    if let ([param], SavedKey::Int(pos)) = (expected, key)
        && param.scalar == Some(marrow_schema::ScalarType::Int)
        && *pos < 1
    {
        return Err(absent_read(
            "a sequence position below 1 is absent".into(),
            span,
        ));
    }
    Ok(())
}

/// Guard one lowered key's scalar kind against its declared key type, the single
/// typed-keyspace check record lookups, layer lookups, and spliced identities all
/// share. A non-scalar (defer) declaration carries no expectation, so the guard
/// skips and any arity fault still fires downstream.
pub(crate) fn guard_key_type(
    declared: &CheckedSavedKeyParam,
    key: &SavedKey,
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    if let Some(expected) = declared.scalar
        && !marrow_store::value::scalar_key_matches_type(key, expected)
    {
        return Err(key_type_fault(expected, key.scalar_type(), span));
    }
    Ok(())
}
