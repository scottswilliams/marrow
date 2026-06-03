//! Saved-data reads and layer/index enumeration.

use marrow_check::{
    CheckedBuiltinCall, CheckedCallTarget, CheckedExpr as ExecExpr, CheckedSavedPlace,
    CheckedSavedTerminal,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::env::{Env, TraversedLayer};
use crate::error::{Located, RUN_ABSENT, RuntimeError, raise_fault, type_error, unsupported};
use crate::expr::eval_expr;
use crate::path::{direct_root_place, lower};
use crate::store::{DataAddress, IndexAddress, LayerAddress};
use crate::value::{Value, saved_key_to_value, value_to_key};

const INDEX_SCAN_PAGE_LIMIT: usize = 128;

pub(crate) fn traversed_layer(
    iterable: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<Option<TraversedLayer>, RuntimeError> {
    // `reversed(...)` traverses the same layer (just backward), so unwrap it the
    // same way `keys`/`values`/`entries` are — the guarded prefix is unchanged by
    // direction. A `reversed(keys(L))` peels both wrappers.
    let unwrapped = reversed_argument(iterable).unwrap_or(iterable);
    let path = traversal_argument(unwrapped).unwrap_or(unwrapped);
    if path.saved_place().is_none() {
        return Ok(None);
    }
    if let Some(place) = direct_root_place(path) {
        return TraversedLayer::record(place, path.span()).map(Some);
    }
    if let Some(place) = path.saved_place()
        && let Some(layer) = index_traversal_layer(place, path.span(), env)?
    {
        return Ok(Some(layer));
    }
    match path {
        // A keyed/sequence child layer `^root(id…).layer`.
        ExecExpr::Field { base, .. } => {
            let base_path = lower(base, env)?;
            let Some(place) = path.saved_place() else {
                return Err(unsupported("iterating this saved path", path.span()));
            };
            let Some(layer_facts) = place.layers.last() else {
                return Err(unsupported("iterating this saved path", path.span()));
            };
            let mut layers = base_path.layer_addresses;
            layers.push(LayerAddress::from_checked(layer_facts, Vec::new()));
            let address =
                DataAddress::layer_prefix(place, &base_path.identity, &layers, path.span())?;
            Ok(Some(TraversedLayer::data(address)))
        }
        _ => Ok(None),
    }
}

/// The sole argument of a `keys`/`values`/`entries` call, or `None` for any other
/// expression. These wrap a saved layer without changing which layer is traversed.
pub(crate) fn traversal_argument(expr: &ExecExpr) -> Option<&ExecExpr> {
    let ExecExpr::Call { target, args, .. } = expr else {
        return None;
    };
    matches!(
        target,
        CheckedCallTarget::Builtin(
            CheckedBuiltinCall::Keys | CheckedBuiltinCall::Values | CheckedBuiltinCall::Entries
        )
    )
    .then_some(())?;
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}

/// The single argument of a `reversed(<iterable>)` call, or `None` for any other
/// expression. Lets the loop materializer and write-guard see through the
/// `reversed` wrapper to the layer it traverses (its inner `keys`/`values`/
/// `entries` or bare saved path), exactly as [`traversal_argument`] does.
pub(crate) fn reversed_argument(expr: &ExecExpr) -> Option<&ExecExpr> {
    let ExecExpr::Call { target, args, .. } = expr else {
        return None;
    };
    matches!(
        target,
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Reversed)
    )
    .then_some(())?;
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}

/// The single argument of a `keys(<path>)` call, or `None` for any other
/// expression. Shared by the loop materializer and the standalone `keys` builtin.
pub(crate) fn keys_argument(expr: &ExecExpr) -> Option<&ExecExpr> {
    let ExecExpr::Call { target, args, .. } = expr else {
        return None;
    };
    matches!(target, CheckedCallTarget::Builtin(CheckedBuiltinCall::Keys)).then_some(())?;
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}

/// Enumerate the child keys of a saved layer for address-oriented traversal:
/// `keys(...)`, direct index-branch loops, and the materialization helpers that
/// pair keys with values. Classifies the path once and descends one shared
/// key-collector ([`collect_child_identities`]):
///
/// - `^root` (a keyed primary root) yields its record identities — a bare key
///   value for a single-key identity, a [`Value::Identity`] for a composite one;
///   a keyless singleton has no identities to iterate (a type error).
/// - `^root.index(args…)` yields the identities in that index branch.
/// - `^root(id…).layer` yields the keyed/sequence layer's child keys.
pub(crate) fn enumerate_layer(
    path: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    enumerate_layer_dir(path, Direction::Ascending, env)
}

/// Enumerate a saved layer's child keys in `dir` order — the direction-threaded
/// core of [`enumerate_layer`]. `Ascending` is `for`/`keys`/`values`/`entries`;
/// `Descending` is `reversed(...)`. The whole descent carries one direction, so a
/// composite identity reverses at every level.
pub(crate) fn enumerate_layer_dir(
    path: &ExecExpr,
    dir: Direction,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    if let Some(place) = direct_root_place(path) {
        let arity = place.identity_keys.len();
        if arity == 0 {
            return Err(type_error(
                &format!(
                    "`^{}` is a singleton with no identities to iterate",
                    place.root
                ),
                path.span(),
            ));
        }
        return collect_record_identities(place, arity, &[], dir, path.span(), env);
    }
    if let Some(place) = path.saved_place()
        && let Some(branch) = iterable_index_branch(place, path.span(), env)?
    {
        return enumerate_index_branch(&branch, dir, path.span(), env);
    }
    match path {
        ExecExpr::Field { .. } => enumerate_child_layer(path, dir, env),
        other => Err(unsupported("iterating this saved path", other.span())),
    }
}

pub(crate) struct IndexBranchAddress {
    pub(crate) index: IndexAddress,
    pub(crate) arg_keys: Vec<SavedKey>,
    pub(crate) identity_start: usize,
    pub(crate) depth: usize,
}

pub(crate) fn index_traversal_layer(
    place: &CheckedSavedPlace,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<TraversedLayer>, RuntimeError> {
    let CheckedSavedTerminal::Index { name, args, .. } = &place.terminal else {
        return Ok(None);
    };
    let mut keys = Vec::new();
    for arg in args {
        if arg.mode.is_some() || arg.name.is_some() {
            return Err(unsupported(
                "an index lookup with named or out arguments",
                span,
            ));
        }
        keys.push(
            value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| unsupported("an index key of this type", span))?,
        );
    }
    let address = IndexAddress::from_place(place, name, keys, span)?;
    Ok(Some(TraversedLayer::index(address)))
}

pub(crate) fn iterable_index_branch(
    place: &CheckedSavedPlace,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<IndexBranchAddress>, RuntimeError> {
    let CheckedSavedTerminal::Index {
        name,
        args,
        unique: false,
        arg_count,
        ..
    } = &place.terminal
    else {
        return Ok(None);
    };
    if args.len() > *arg_count {
        return Err(unsupported("iterating this saved path", span));
    }
    let mut arg_keys = Vec::new();
    for arg in args {
        if arg.mode.is_some() || arg.name.is_some() {
            return Err(unsupported(
                "an index lookup with named or out arguments",
                span,
            ));
        }
        let key = value_to_key(eval_expr(&arg.value, env)?)
            .ok_or_else(|| unsupported("an index key of this type", span))?;
        arg_keys.push(key);
    }
    let identity_start = arg_count.saturating_sub(place.identity_keys.len());
    let depth = if args.len() < identity_start {
        1
    } else {
        arg_count.saturating_sub(args.len())
    };
    Ok(Some(IndexBranchAddress {
        index: IndexAddress::from_place(place, name, arg_keys.clone(), span)?,
        arg_keys,
        identity_start,
        depth,
    }))
}

/// Enumerate the identities in a declared index branch `^root.index(args…)`. A
/// non-unique index ends with all identity keys, so the levels below the supplied
/// query args are the entry's remaining identity-key segments; descend them per
/// entry to reconstruct the full identity rather than only its first key component.
pub(crate) fn enumerate_index_branch(
    branch: &IndexBranchAddress,
    dir: Direction,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    if branch.depth == 0 {
        return collect_exact_index_tuple(branch, dir, span, env);
    }
    let key_prefix = branch
        .arg_keys
        .get(branch.identity_start..)
        .map_or_else(Vec::new, |keys| keys.to_vec());
    collect_index_identities(
        branch,
        branch.depth,
        &branch.arg_keys,
        &key_prefix,
        dir,
        span,
        env,
    )
}

/// Enumerate the child keys of a keyed/sequence child layer `^root(id…).layer`.
/// The layer's keys are single-key (`pos: int` for a sequence, `playerId: string`
/// for a keyed tree), so each child key is a bare value.
pub(crate) fn enumerate_child_layer(
    path: &ExecExpr,
    dir: Direction,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    let ExecExpr::Field { base, .. } = path else {
        return Err(unsupported("iterating this saved path", path.span()));
    };
    let span = path.span();
    let base_path = lower(base, env)?;
    let Some(place) = path.saved_place() else {
        return Err(unsupported("iterating this saved path", path.span()));
    };
    let Some(layer_facts) = place.layers.last() else {
        return Err(unsupported("iterating this saved path", path.span()));
    };
    let mut layers = base_path.layer_addresses;
    layers.push(LayerAddress::from_checked(layer_facts, Vec::new()));
    let address = DataAddress::layer_prefix(place, &base_path.identity, &layers, span)?;
    collect_data_child_values(&address, dir, span, env)
}

pub(crate) fn collect_record_identities(
    place: &CheckedSavedPlace,
    depth: usize,
    keys: &[SavedKey],
    dir: Direction,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    let store = crate::store::catalog_id(&place.store_catalog_id, "store", span)?;
    let mut values = Vec::new();
    let mut child = first_record_child(env.store, &store, keys, dir, span)?;
    while let Some(key) = child {
        let anchor = key.clone();
        let mut keys = keys.to_vec();
        keys.push(key.clone());
        if depth <= 1 {
            values.push(collected_identity_value(&keys, span)?);
        } else {
            values.extend(collect_record_identities(
                place,
                depth - 1,
                &keys,
                dir,
                span,
                env,
            )?);
        }
        child = next_record_child(
            env.store,
            &store,
            &keys[..keys.len() - 1],
            &anchor,
            dir,
            span,
        )?;
    }
    Ok(values)
}

fn collect_index_identities(
    branch: &IndexBranchAddress,
    depth: usize,
    query_keys: &[SavedKey],
    keys: &[SavedKey],
    dir: Direction,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    let mut values = Vec::new();
    let mut child = first_index_child(env.store, &branch.index.index, query_keys, dir, span)?;
    while let Some(key) = child {
        let anchor = key.clone();
        let mut query_keys = query_keys.to_vec();
        query_keys.push(key.clone());
        let mut identity_keys = keys.to_vec();
        identity_keys.push(key);
        if depth <= 1 {
            values.push(collected_identity_value(&identity_keys, span)?);
        } else {
            values.extend(collect_index_identities(
                branch,
                depth - 1,
                &query_keys,
                &identity_keys,
                dir,
                span,
                env,
            )?);
        }
        child = next_index_child(
            env.store,
            &branch.index.index,
            &query_keys[..query_keys.len() - 1],
            &anchor,
            dir,
            span,
        )?;
    }
    Ok(values)
}

fn collect_exact_index_tuple(
    branch: &IndexBranchAddress,
    dir: Direction,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    let mut values = Vec::new();
    let mut page = env
        .store
        .scan_index_tuple(&branch.index.index, &branch.arg_keys, INDEX_SCAN_PAGE_LIMIT)
        .map_err(|error| error.located(span))?;
    loop {
        for entry in page.entries {
            values.push(collected_identity_value(&entry.identity, span)?);
        }
        let Some(cursor) = page.cursor else {
            break;
        };
        page = env
            .store
            .scan_index_tuple_after(
                &branch.index.index,
                &branch.arg_keys,
                &cursor,
                INDEX_SCAN_PAGE_LIMIT,
            )
            .map_err(|error| error.located(span))?;
    }
    if matches!(dir, Direction::Descending) {
        values.reverse();
    }
    Ok(values)
}

fn collect_data_child_values(
    address: &DataAddress,
    dir: Direction,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    let mut values = Vec::new();
    let mut child = first_data_child(env.store, address, dir, span)?;
    while let Some(key) = child {
        let anchor = key.clone();
        values.push(
            saved_key_to_value(key)
                .ok_or_else(|| unsupported("iterating keys of this type", span))?,
        );
        child = next_data_child(env.store, address, &anchor, dir, span)?;
    }
    Ok(values)
}

fn first_record_child(
    store: &TreeStore,
    store_id: &CatalogId,
    prefix: &[SavedKey],
    dir: Direction,
    span: SourceSpan,
) -> Result<Option<SavedKey>, RuntimeError> {
    match dir {
        Direction::Ascending => store.record_first_child(store_id, prefix),
        Direction::Descending => store.record_last_child(store_id, prefix),
    }
    .map_err(|error| error.located(span))
}

fn next_record_child(
    store: &TreeStore,
    store_id: &CatalogId,
    prefix: &[SavedKey],
    anchor: &SavedKey,
    dir: Direction,
    span: SourceSpan,
) -> Result<Option<SavedKey>, RuntimeError> {
    match dir {
        Direction::Ascending => store.record_next_child(store_id, prefix, anchor),
        Direction::Descending => store.record_prev_child(store_id, prefix, anchor),
    }
    .map_err(|error| error.located(span))
}

fn first_index_child(
    store: &TreeStore,
    index: &CatalogId,
    prefix: &[SavedKey],
    dir: Direction,
    span: SourceSpan,
) -> Result<Option<SavedKey>, RuntimeError> {
    match dir {
        Direction::Ascending => store.index_first_child(index, prefix),
        Direction::Descending => store.index_last_child(index, prefix),
    }
    .map_err(|error| error.located(span))
}

fn next_index_child(
    store: &TreeStore,
    index: &CatalogId,
    prefix: &[SavedKey],
    anchor: &SavedKey,
    dir: Direction,
    span: SourceSpan,
) -> Result<Option<SavedKey>, RuntimeError> {
    match dir {
        Direction::Ascending => store.index_next_child(index, prefix, anchor),
        Direction::Descending => store.index_prev_child(index, prefix, anchor),
    }
    .map_err(|error| error.located(span))
}

fn first_data_child(
    store: &TreeStore,
    address: &DataAddress,
    dir: Direction,
    span: SourceSpan,
) -> Result<Option<SavedKey>, RuntimeError> {
    match dir {
        Direction::Ascending => {
            store.data_first_child(&address.store, &address.identity, &address.path)
        }
        Direction::Descending => {
            store.data_last_child(&address.store, &address.identity, &address.path)
        }
    }
    .map_err(|error| error.located(span))
}

fn next_data_child(
    store: &TreeStore,
    address: &DataAddress,
    anchor: &SavedKey,
    dir: Direction,
    span: SourceSpan,
) -> Result<Option<SavedKey>, RuntimeError> {
    match dir {
        Direction::Ascending => {
            store.data_next_child(&address.store, &address.identity, &address.path, anchor)
        }
        Direction::Descending => {
            store.data_prev_child(&address.store, &address.identity, &address.path, anchor)
        }
    }
    .map_err(|error| error.located(span))
}

pub(crate) fn collected_identity_value(
    keys: &[SavedKey],
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    if let [key] = keys {
        return saved_key_to_value(key.clone())
            .ok_or_else(|| unsupported("iterating keys of this type", span));
    }
    Ok(Value::Identity(keys.to_vec()))
}

/// Read a field of a local resource value, e.g. `book.shelf`. An unpopulated
/// field is an absent-element error.
pub(crate) fn eval_local_field_get(
    base: &ExecExpr,
    field: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Value::Resource(fields) = eval_expr(base, env)? else {
        return Err(unsupported("a field of a non-resource value", span));
    };
    match fields.into_iter().find(|(name, _)| name == field) {
        Some((_, value)) => Ok(value),
        None => Err(raise_fault(
            RUN_ABSENT,
            format!("`{field}` is absent"),
            span,
        )),
    }
}

/// Read a field of the local resource bound to `base`, from a pre-resolved base
/// name. Shared by `out`/`inout` place reads.
pub(crate) fn read_local_field(
    base: &str,
    field: &str,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Value, RuntimeError> {
    let Some(Value::Resource(fields)) = env.lookup(base) else {
        return Err(unsupported("a field of a non-resource local", span));
    };
    fields
        .iter()
        .find(|(name, _)| name == field)
        .map(|(_, value)| value.clone())
        .ok_or_else(|| RuntimeError {
            throw: None,
            origin: None,
            code: RUN_ABSENT,
            message: format!("`{field}` is absent"),
            span,
        })
}
