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
use crate::env::Env;
use crate::error::{
    Located, RUN_ABSENT, RuntimeError, overflow, raise_fault, type_error, unsupported,
};
use crate::expr::eval_expr;
use crate::path::{direct_root_place, lower};
use crate::store::{DataAddress, IndexAddress, LayerAddress};
use crate::value::{Value, saved_key_to_value, value_to_key};

pub(crate) const INDEX_SCAN_PAGE_LIMIT: usize = 128;

/// The single argument of a `reversed(<iterable>)` call, or `None` for any other
/// expression. Lets collection helpers see through the wrapper without changing
/// the saved layer they traverse.
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

pub(crate) struct IndexBranchAddress {
    pub(crate) index: IndexAddress,
    pub(crate) arg_keys: Vec<SavedKey>,
    pub(crate) identity_start: usize,
    pub(crate) depth: usize,
}

pub(crate) enum IterableLayer<'a> {
    Root(&'a CheckedSavedPlace),
    Index(&'a CheckedSavedPlace, IndexBranchAddress),
    ChildLayer,
}

pub(crate) fn iterable_layer<'a>(
    path: &'a ExecExpr,
    env: &mut Env<'_>,
) -> Result<IterableLayer<'a>, RuntimeError> {
    if let Some(place) = direct_root_place(path) {
        return Ok(IterableLayer::Root(place));
    }
    if let Some(place) = path.saved_place()
        && let Some(branch) = iterable_index_branch(place, path.span(), env)?
    {
        return Ok(IterableLayer::Index(place, branch));
    }
    match path {
        ExecExpr::Field { .. } => Ok(IterableLayer::ChildLayer),
        other => Err(unsupported("iterating this saved path", other.span())),
    }
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
                "an index lookup with named or inout arguments",
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

pub(crate) fn count_iterable_layer(
    path: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<usize, RuntimeError> {
    match iterable_layer(path, env)? {
        IterableLayer::Root(place) => {
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
            let store = crate::store::catalog_id(&place.store_catalog_id, "store", path.span())?;
            count_record_identity_children(&store, arity, &[], path.span(), env)
        }
        IterableLayer::Index(_, branch) => count_index_branch(&branch, path.span(), env),
        IterableLayer::ChildLayer => {
            let address = child_layer_prefix_address(path, env)?;
            crate::store::data_child_count(env.store, &address, path.span())
        }
    }
}

pub(crate) fn count_iterable_index_branch(
    path: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<Option<usize>, RuntimeError> {
    let Some(place) = path.saved_place() else {
        return Ok(None);
    };
    let Some(branch) = iterable_index_branch(place, path.span(), env)? else {
        return Ok(None);
    };
    count_index_branch(&branch, path.span(), env).map(Some)
}

pub(crate) fn iterable_index_branch_present(
    path: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<Option<bool>, RuntimeError> {
    let Some(place) = path.saved_place() else {
        return Ok(None);
    };
    let Some(branch) = iterable_index_branch(place, path.span(), env)? else {
        return Ok(None);
    };
    index_branch_present(&branch, path.span(), env).map(Some)
}

fn count_record_identity_children(
    store: &CatalogId,
    depth: usize,
    keys: &[SavedKey],
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<usize, RuntimeError> {
    if depth <= 1 {
        return env
            .store
            .record_child_count(store, keys)
            .map_err(|error| error.located(span));
    }
    let mut count = 0usize;
    let mut child = first_record_child(env.store, store, keys, Direction::Ascending, span)?;
    while let Some(key) = child {
        let anchor = key.clone();
        let mut keys = keys.to_vec();
        keys.push(key);
        count = checked_count_add(
            count,
            count_record_identity_children(store, depth - 1, &keys, span, env)?,
            span,
        )?;
        child = next_record_child(
            env.store,
            store,
            &keys[..keys.len() - 1],
            &anchor,
            Direction::Ascending,
            span,
        )?;
    }
    Ok(count)
}

fn count_index_branch(
    branch: &IndexBranchAddress,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<usize, RuntimeError> {
    if branch.depth == 0 {
        return count_exact_index_tuple(branch, span, env);
    }
    count_index_identity_children(branch, branch.depth, &branch.arg_keys, span, env)
}

fn count_index_identity_children(
    branch: &IndexBranchAddress,
    depth: usize,
    query_keys: &[SavedKey],
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<usize, RuntimeError> {
    let mut count = 0usize;
    let mut child = first_index_child(
        env.store,
        &branch.index.index,
        query_keys,
        Direction::Ascending,
        span,
    )?;
    while let Some(key) = child {
        let anchor = key.clone();
        let mut query_keys = query_keys.to_vec();
        query_keys.push(key);
        count = if depth <= 1 {
            checked_count_add(count, 1, span)?
        } else {
            checked_count_add(
                count,
                count_index_identity_children(branch, depth - 1, &query_keys, span, env)?,
                span,
            )?
        };
        child = next_index_child(
            env.store,
            &branch.index.index,
            &query_keys[..query_keys.len() - 1],
            &anchor,
            Direction::Ascending,
            span,
        )?;
    }
    Ok(count)
}

fn count_exact_index_tuple(
    branch: &IndexBranchAddress,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<usize, RuntimeError> {
    let mut count = 0usize;
    let mut page = env
        .store
        .scan_index_tuple(&branch.index.index, &branch.arg_keys, INDEX_SCAN_PAGE_LIMIT)
        .map_err(|error| error.located(span))?;
    loop {
        count = checked_count_add(count, page.entries.len(), span)?;
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
    Ok(count)
}

fn index_branch_present(
    branch: &IndexBranchAddress,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<bool, RuntimeError> {
    if branch.depth == 0 {
        return env
            .store
            .scan_index_tuple(&branch.index.index, &branch.arg_keys, 1)
            .map(|page| !page.entries.is_empty())
            .map_err(|error| error.located(span));
    }
    first_index_child(
        env.store,
        &branch.index.index,
        &branch.arg_keys,
        Direction::Ascending,
        span,
    )
    .map(|child| child.is_some())
}

fn child_layer_prefix_address(
    path: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<DataAddress, RuntimeError> {
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
    DataAddress::layer_prefix(place, &base_path.identity, &layers, span)
}

fn checked_count_add(left: usize, right: usize, span: SourceSpan) -> Result<usize, RuntimeError> {
    left.checked_add(right).ok_or_else(|| overflow(span))
}

pub(crate) fn first_record_child(
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

pub(crate) fn next_record_child(
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

pub(crate) fn first_index_child(
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

pub(crate) fn next_index_child(
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

pub(crate) fn first_data_child(
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

pub(crate) fn next_data_child(
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
/// name. Shared by `inout` place reads.
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
