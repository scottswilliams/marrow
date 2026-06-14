//! Saved-data reads and layer/index enumeration.

use marrow_check::{
    CheckedBuiltinCall, CheckedCallTarget, CheckedExpr as ExecExpr, CheckedSavedKeyParam,
    CheckedSavedPlace, CheckedSavedTerminal, StoredValueMeaning,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, IndexEntry, IndexRangeBounds, TreeStore};
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::env::Env;
use crate::error::{
    Located, RUN_ABSENT, RuntimeError, overflow, raise_fault, type_error, unsupported,
};
use crate::expr::eval_expr;
use crate::path::{direct_root_place, guard_key_type, lower, lower_keys};
use crate::range_expr::checked_range;
use crate::saved_iter::{IndexCursor, RecordCursor, count_keyed_children};
use crate::store::{DataAddress, IndexAddress, LayerAddress};
use crate::value::{
    Value, index_key_to_value, saved_key_to_value, validate_place_identity_keys,
    value_to_index_key, value_to_key,
};

pub(crate) const INDEX_SCAN_PAGE_LIMIT: usize = 128;

/// The single argument of a `reversed(<iterable>)` call. Lets collection helpers
/// see through the wrapper without changing the saved layer they traverse.
pub(crate) fn reversed_argument(expr: &ExecExpr) -> Option<&ExecExpr> {
    let ExecExpr::Call { target, args, .. } = expr else {
        return None;
    };
    if !matches!(
        target,
        CheckedCallTarget::Builtin(CheckedBuiltinCall::Reversed)
    ) {
        return None;
    }
    match args.as_slice() {
        [arg] if arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}

/// The single argument of a `keys(<path>)` call, or `None` for any other
/// expression. Shared by the loop materializer and the standalone `keys` builtin.
pub(crate) fn keys_argument(expr: &ExecExpr) -> Option<&ExecExpr> {
    let ExecExpr::Call { target, args, .. } = expr else {
        return None;
    };
    if !matches!(target, CheckedCallTarget::Builtin(CheckedBuiltinCall::Keys)) {
        return None;
    }
    match args.as_slice() {
        [arg] if arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}

pub(crate) struct IndexBranchAddress {
    pub(crate) index: IndexAddress,
    pub(crate) arg_keys: Vec<SavedKey>,
    pub(crate) range: Option<IndexRangeBounds>,
    pub(crate) key_meanings: Vec<StoredValueMeaning>,
    pub(crate) identity_start: usize,
    pub(crate) depth: usize,
    pub(crate) yields_identity: bool,
}

#[derive(Clone)]
pub(crate) struct KeyRangeAddress {
    pub(crate) exact_prefix: Vec<SavedKey>,
    pub(crate) range: Option<IndexRangeBounds>,
}

pub(crate) enum IterableLayer<'a> {
    Root(&'a CheckedSavedPlace, KeyRangeAddress),
    Index(&'a CheckedSavedPlace, IndexBranchAddress),
    ChildLayer,
}

pub(crate) fn iterable_layer<'a>(
    path: &'a ExecExpr,
    env: &mut Env<'_>,
) -> Result<IterableLayer<'a>, RuntimeError> {
    if let Some(place) = direct_root_place(path) {
        return Ok(IterableLayer::Root(
            place,
            KeyRangeAddress {
                exact_prefix: Vec::new(),
                range: None,
            },
        ));
    }
    if let Some(place) = path.saved_place()
        && let Some(address) = iterable_root_key_range(place, path.span(), env)?
    {
        return Ok(IterableLayer::Root(place, address));
    }
    if let Some(place) = path.saved_place()
        && let Some(branch) = iterable_index_branch(place, path.span(), env)?
    {
        return Ok(IterableLayer::Index(place, branch));
    }
    match path {
        ExecExpr::Field { .. } => Ok(IterableLayer::ChildLayer),
        ExecExpr::Call { .. } if path.saved_place().is_some_and(layer_call_has_range) => {
            Ok(IterableLayer::ChildLayer)
        }
        other => Err(unsupported("iterating this saved path", other.span())),
    }
}

fn iterable_root_key_range(
    place: &CheckedSavedPlace,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<KeyRangeAddress>, RuntimeError> {
    if !matches!(place.terminal, CheckedSavedTerminal::Record) || !place.layers.is_empty() {
        return Ok(None);
    }
    let Some(range_position) = place
        .identity_args
        .iter()
        .position(|arg| is_key_range_expr(&arg.value))
    else {
        return Ok(None);
    };
    if range_position + 1 != place.identity_args.len()
        || place.identity_args.len() != place.identity_keys.len()
    {
        return Err(unsupported("iterating this saved path", span));
    }
    let exact_prefix = lower_keys(
        &place.identity_args[..range_position],
        span,
        false,
        None,
        &place.identity_keys,
        env,
    )?;
    let range = key_range_bounds(
        &place.identity_args[range_position].value,
        &place.identity_keys[range_position],
        span,
        env,
    )?;
    Ok(range.map(|range| KeyRangeAddress {
        exact_prefix,
        range: Some(range),
    }))
}

fn layer_call_has_range(place: &CheckedSavedPlace) -> bool {
    matches!(place.terminal, CheckedSavedTerminal::Record)
        && place
            .layers
            .last()
            .is_some_and(|layer| layer.args.iter().any(|arg| is_key_range_expr(&arg.value)))
}

pub(crate) fn is_key_range_expr(expr: &ExecExpr) -> bool {
    checked_range(expr).is_some()
}

fn iterable_index_branch(
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
    let index = place
        .indexes
        .iter()
        .find(|index| index.name == *name)
        .ok_or_else(|| unsupported("this index lookup", span))?;
    let mut arg_keys = Vec::new();
    let mut range = None;
    let mut range_position = None;
    for (position, arg) in args.iter().enumerate() {
        if arg.name.is_some() {
            return Err(unsupported("an index lookup with named arguments", span));
        }
        if let Some(bounds) =
            index_range_bounds(&arg.value, &index.keys[position].value_meaning, span, env)?
        {
            range = Some(bounds);
            range_position = Some(position);
            break;
        }
        let key = value_to_index_key(
            eval_expr(&arg.value, env)?,
            &index.keys[position].value_meaning,
            span,
        )?;
        arg_keys.push(key);
    }
    let identity_start = arg_count.saturating_sub(place.identity_keys.len());
    let depth = if args.len() < identity_start {
        1
    } else {
        arg_count.saturating_sub(args.len())
    };
    let yields_identity = range_position
        .map(|position| position + 1 >= identity_start)
        .unwrap_or(args.len() >= identity_start);
    Ok(Some(IndexBranchAddress {
        index: IndexAddress::from_place(place, name, arg_keys.clone(), span)?,
        arg_keys,
        range,
        key_meanings: index
            .keys
            .iter()
            .map(|key| key.value_meaning.clone())
            .collect(),
        identity_start,
        depth: if range_position.is_some() { 0 } else { depth },
        yields_identity,
    }))
}

fn index_range_bounds(
    expr: &ExecExpr,
    meaning: &marrow_check::StoredValueMeaning,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<IndexRangeBounds>, RuntimeError> {
    let Some(range) = checked_range(expr) else {
        return Ok(None);
    };
    let lower = match range.start {
        Some(start) => Some(value_to_index_key(eval_expr(start, env)?, meaning, span)?),
        None => None,
    };
    let upper = match range.end {
        Some(end) => Some(value_to_index_key(eval_expr(end, env)?, meaning, span)?),
        None => None,
    };
    Ok(Some(IndexRangeBounds {
        lower,
        upper,
        upper_inclusive: range.inclusive_end,
    }))
}

pub(crate) fn key_range_bounds(
    expr: &ExecExpr,
    expected: &CheckedSavedKeyParam,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<IndexRangeBounds>, RuntimeError> {
    let Some(range) = checked_range(expr) else {
        return Ok(None);
    };
    let lower = match range.start {
        Some(start) => Some(lower_range_bound(start, expected, span, env)?),
        None => None,
    };
    let upper = match range.end {
        Some(end) => Some(lower_range_bound(end, expected, span, env)?),
        None => None,
    };
    Ok(Some(IndexRangeBounds {
        lower,
        upper,
        upper_inclusive: range.inclusive_end,
    }))
}

fn lower_range_bound(
    expr: &ExecExpr,
    expected: &CheckedSavedKeyParam,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<SavedKey, RuntimeError> {
    let key = value_to_key(eval_expr(expr, env)?)
        .ok_or_else(|| unsupported("a key of this type", span))?;
    guard_key_type(expected, &key, span)?;
    Ok(key)
}

pub(crate) fn count_iterable_layer(
    path: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<usize, RuntimeError> {
    match iterable_layer(path, env)? {
        IterableLayer::Root(place, address) => {
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
            if address.range.is_some() {
                let cursor = RecordCursor::new_bounded(
                    &store,
                    arity,
                    Direction::Ascending,
                    path.span(),
                    address.range.clone(),
                );
                return count_keyed_children(
                    &cursor,
                    arity.saturating_sub(address.exact_prefix.len()),
                    &address.exact_prefix,
                    env,
                    path.span(),
                    |_, _| Ok(1),
                );
            }
            count_record_identities(&store, arity, path.span(), env)
        }
        IterableLayer::Index(place, branch) => count_index_branch(place, &branch, path.span(), env),
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
    count_index_branch(place, &branch, path.span(), env).map(Some)
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
    count_index_branch(place, &branch, path.span(), env).map(|count| Some(count > 0))
}

/// Count every record identity under a root of `arity` identity keys. A composite
/// root walks the first `arity - 1` levels and folds the bulk child count of the
/// final level into each walked prefix, avoiding a per-leaf descent.
fn count_record_identities(
    store: &CatalogId,
    arity: usize,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<usize, RuntimeError> {
    if arity <= 1 {
        return env
            .store
            .record_child_count(store, &[])
            .map_err(|error| error.located(span));
    }
    let cursor = RecordCursor::new(store, arity, Direction::Ascending, span);
    count_keyed_children(&cursor, arity - 1, &[], env, span, |prefix, env| {
        env.store
            .record_child_count(store, prefix)
            .map_err(|error| error.located(span))
    })
}

fn count_index_branch(
    place: &CheckedSavedPlace,
    branch: &IndexBranchAddress,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<usize, RuntimeError> {
    if branch.depth == 0 {
        return count_exact_index_tuple(place, branch, span, env);
    }
    let cursor = IndexCursor::new(&branch.index.index, Direction::Ascending, span);
    count_keyed_children(
        &cursor,
        branch.depth,
        &branch.arg_keys,
        env,
        span,
        |keys, env| {
            validate_walked_index_branch_yield(place, branch, keys, span, env)?;
            Ok(1)
        },
    )
}

fn count_exact_index_tuple(
    place: &CheckedSavedPlace,
    branch: &IndexBranchAddress,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<usize, RuntimeError> {
    let mut count = 0usize;
    let mut page = match &branch.range {
        Some(range) => env.store.scan_index_range(
            &branch.index.index,
            &branch.arg_keys,
            range,
            INDEX_SCAN_PAGE_LIMIT,
        ),
        None => {
            env.store
                .scan_index_tuple(&branch.index.index, &branch.arg_keys, INDEX_SCAN_PAGE_LIMIT)
        }
    }
    .map_err(|error| error.located(span))?;
    loop {
        for entry in &page.entries {
            validate_scanned_index_entry(place, branch, entry, span, env)?;
        }
        count = count
            .checked_add(page.entries.len())
            .ok_or_else(|| overflow(span))?;
        let Some(cursor) = page.cursor else {
            break;
        };
        page = match &branch.range {
            Some(range) => env.store.scan_index_range_after(
                &branch.index.index,
                &branch.arg_keys,
                range,
                &cursor,
                INDEX_SCAN_PAGE_LIMIT,
            ),
            None => env.store.scan_index_tuple_after(
                &branch.index.index,
                &branch.arg_keys,
                &cursor,
                INDEX_SCAN_PAGE_LIMIT,
            ),
        }
        .map_err(|error| error.located(span))?;
    }
    Ok(count)
}

pub(crate) fn validate_index_branch_yield(
    place: &CheckedSavedPlace,
    branch: &IndexBranchAddress,
    keys: &[SavedKey],
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    if branch.yields_identity {
        validate_place_identity_keys(place, keys, span)?;
        return Ok(None);
    }
    let Some(meaning) = branch.key_meanings.get(branch.arg_keys.len()) else {
        return Ok(None);
    };
    let [key] = keys else {
        return Err(unsupported("iterating a composite non-identity key", span));
    };
    index_key_to_value(env.program, key, meaning, span).map(Some)
}

fn validate_walked_index_branch_yield(
    place: &CheckedSavedPlace,
    branch: &IndexBranchAddress,
    keys: &[SavedKey],
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<(), RuntimeError> {
    if !branch.yields_identity {
        validate_index_branch_yield(place, branch, keys, span, env)?;
        return Ok(());
    }
    let mut index_keys = branch.arg_keys.clone();
    index_keys.extend_from_slice(keys);
    validate_scanned_index_tuple_entries(place, branch, &index_keys, span, env)?;
    let mut identity = branch
        .arg_keys
        .get(branch.identity_start..)
        .map_or_else(Vec::new, |keys| keys.to_vec());
    identity.extend_from_slice(keys);
    validate_place_identity_keys(place, &identity, span)
}

pub(crate) fn validate_walked_index_identity_entries(
    place: &CheckedSavedPlace,
    branch: &IndexBranchAddress,
    identity: &[SavedKey],
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<(), RuntimeError> {
    let mut index_keys = branch
        .arg_keys
        .get(..branch.identity_start)
        .map_or_else(Vec::new, |keys| keys.to_vec());
    index_keys.extend_from_slice(identity);
    validate_scanned_index_tuple_entries(place, branch, &index_keys, span, env)
}

pub(crate) fn validate_scanned_index_entry(
    place: &CheckedSavedPlace,
    branch: &IndexBranchAddress,
    entry: &IndexEntry,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<(), RuntimeError> {
    if entry.index_keys.len() != branch.key_meanings.len() {
        return Err(type_error(
            "stored index entry does not match the index key shape",
            span,
        ));
    }
    for (key, meaning) in entry.index_keys.iter().zip(&branch.key_meanings) {
        index_key_to_value(env.program, key, meaning, span)?;
    }
    if !branch.yields_identity {
        return Err(type_error(
            "stored index entry is not an identity-yielding tuple",
            span,
        ));
    }
    validate_place_identity_keys(place, &entry.identity, span)?;
    let Some(index_identity) = entry.index_keys.get(branch.identity_start..) else {
        return Err(type_error(
            "stored index entry does not match the index key shape",
            span,
        ));
    };
    if index_identity != entry.identity {
        return Err(type_error(
            "stored index entry identity does not match the index tuple",
            span,
        ));
    }
    Ok(())
}

fn validate_scanned_index_tuple_entries(
    place: &CheckedSavedPlace,
    branch: &IndexBranchAddress,
    index_keys: &[SavedKey],
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<(), RuntimeError> {
    let mut page = env
        .store
        .scan_index_tuple(&branch.index.index, index_keys, INDEX_SCAN_PAGE_LIMIT)
        .map_err(|error| error.located(span))?;
    loop {
        for entry in &page.entries {
            validate_scanned_index_entry(place, branch, entry, span, env)?;
        }
        let Some(cursor) = page.cursor else {
            break;
        };
        page = env
            .store
            .scan_index_tuple_after(
                &branch.index.index,
                index_keys,
                &cursor,
                INDEX_SCAN_PAGE_LIMIT,
            )
            .map_err(|error| error.located(span))?;
    }
    Ok(())
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

pub(crate) fn first_record_child(
    store: &TreeStore,
    store_id: &CatalogId,
    prefix: &[SavedKey],
    dir: Direction,
    arity: usize,
    span: SourceSpan,
) -> Result<Option<SavedKey>, RuntimeError> {
    match dir {
        Direction::Ascending => store.record_first_child_at_arity(store_id, prefix, arity),
        Direction::Descending => store.record_last_child_at_arity(store_id, prefix, arity),
    }
    .map_err(|error| error.located(span))
}

pub(crate) fn next_record_child(
    store: &TreeStore,
    store_id: &CatalogId,
    prefix: &[SavedKey],
    anchor: &SavedKey,
    dir: Direction,
    arity: usize,
    span: SourceSpan,
) -> Result<Option<SavedKey>, RuntimeError> {
    match dir {
        Direction::Ascending => store.record_next_child_at_arity(store_id, prefix, arity, anchor),
        Direction::Descending => store.record_prev_child_at_arity(store_id, prefix, arity, anchor),
    }
    .map_err(|error| error.located(span))
}

pub(crate) struct RecordChildRange<'a> {
    pub(crate) store_id: &'a CatalogId,
    pub(crate) prefix: &'a [SavedKey],
    pub(crate) range: &'a IndexRangeBounds,
    pub(crate) dir: Direction,
    pub(crate) arity: usize,
    pub(crate) span: SourceSpan,
}

pub(crate) fn first_record_child_in_range(
    store: &TreeStore,
    scan: RecordChildRange<'_>,
) -> Result<Option<SavedKey>, RuntimeError> {
    let child = match scan.dir {
        Direction::Ascending => match &scan.range.lower {
            Some(lower)
                if record_child_exists(
                    store,
                    scan.store_id,
                    scan.prefix,
                    lower,
                    scan.arity,
                    scan.span,
                )? =>
            {
                Some(lower.clone())
            }
            Some(lower) => store
                .record_next_child_at_arity(scan.store_id, scan.prefix, scan.arity, lower)
                .map_err(|error| error.located(scan.span))?,
            None => store
                .record_first_child_at_arity(scan.store_id, scan.prefix, scan.arity)
                .map_err(|error| error.located(scan.span))?,
        },
        Direction::Descending => match &scan.range.upper {
            Some(upper)
                if scan.range.upper_inclusive
                    && record_child_exists(
                        store,
                        scan.store_id,
                        scan.prefix,
                        upper,
                        scan.arity,
                        scan.span,
                    )? =>
            {
                Some(upper.clone())
            }
            Some(upper) => store
                .record_prev_child_at_arity(scan.store_id, scan.prefix, scan.arity, upper)
                .map_err(|error| error.located(scan.span))?,
            None => store
                .record_last_child_at_arity(scan.store_id, scan.prefix, scan.arity)
                .map_err(|error| error.located(scan.span))?,
        },
    };
    Ok(child.filter(|key| key_in_range(key, scan.range)))
}

pub(crate) fn next_record_child_in_range(
    store: &TreeStore,
    scan: RecordChildRange<'_>,
    anchor: &SavedKey,
) -> Result<Option<SavedKey>, RuntimeError> {
    let child = match scan.dir {
        Direction::Ascending => {
            store.record_next_child_at_arity(scan.store_id, scan.prefix, scan.arity, anchor)
        }
        Direction::Descending => {
            store.record_prev_child_at_arity(scan.store_id, scan.prefix, scan.arity, anchor)
        }
    }
    .map_err(|error| error.located(scan.span))?;
    Ok(child.filter(|key| key_in_range(key, scan.range)))
}

fn record_child_exists(
    store: &TreeStore,
    store_id: &CatalogId,
    prefix: &[SavedKey],
    child: &SavedKey,
    arity: usize,
    span: SourceSpan,
) -> Result<bool, RuntimeError> {
    let mut identity = prefix.to_vec();
    identity.push(child.clone());
    store
        .record_identity_exists_under(store_id, &identity, arity)
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

pub(crate) fn first_data_child_in_range(
    store: &TreeStore,
    address: &DataAddress,
    range: &IndexRangeBounds,
    dir: Direction,
    span: SourceSpan,
) -> Result<Option<SavedKey>, RuntimeError> {
    let child = match dir {
        Direction::Ascending => match &range.lower {
            Some(lower) if data_child_exists_at(store, address, lower, span)? => {
                Some(lower.clone())
            }
            Some(lower) => store
                .data_next_child(&address.store, &address.identity, &address.path, lower)
                .map_err(|error| error.located(span))?,
            None => store
                .data_first_child(&address.store, &address.identity, &address.path)
                .map_err(|error| error.located(span))?,
        },
        Direction::Descending => match &range.upper {
            Some(upper)
                if range.upper_inclusive && data_child_exists_at(store, address, upper, span)? =>
            {
                Some(upper.clone())
            }
            Some(upper) => store
                .data_prev_child(&address.store, &address.identity, &address.path, upper)
                .map_err(|error| error.located(span))?,
            None => store
                .data_last_child(&address.store, &address.identity, &address.path)
                .map_err(|error| error.located(span))?,
        },
    };
    Ok(child.filter(|key| key_in_range(key, range)))
}

pub(crate) fn next_data_child_in_range(
    store: &TreeStore,
    address: &DataAddress,
    anchor: &SavedKey,
    range: &IndexRangeBounds,
    dir: Direction,
    span: SourceSpan,
) -> Result<Option<SavedKey>, RuntimeError> {
    let child = match dir {
        Direction::Ascending => {
            store.data_next_child(&address.store, &address.identity, &address.path, anchor)
        }
        Direction::Descending => {
            store.data_prev_child(&address.store, &address.identity, &address.path, anchor)
        }
    }
    .map_err(|error| error.located(span))?;
    Ok(child.filter(|key| key_in_range(key, range)))
}

fn data_child_exists_at(
    store: &TreeStore,
    address: &DataAddress,
    child: &SavedKey,
    span: SourceSpan,
) -> Result<bool, RuntimeError> {
    let mut path = address.path.clone();
    path.push(DataPathSegment::Key(child.clone()));
    store
        .data_subtree_exists(&address.store, &address.identity, &path)
        .map_err(|error| error.located(span))
}

fn key_in_range(key: &SavedKey, range: &IndexRangeBounds) -> bool {
    if let Some(lower) = &range.lower
        && key < lower
    {
        return false;
    }
    if let Some(upper) = &range.upper {
        if range.upper_inclusive {
            if key > upper {
                return false;
            }
        } else if key >= upper {
            return false;
        }
    }
    true
}

pub(crate) fn collected_identity_value(
    keys: &[SavedKey],
    root: Option<&str>,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    if let Some(root) = root {
        return Ok(crate::value::identity_value(root, keys.to_vec()));
    }
    let [key] = keys else {
        return Err(unsupported("iterating a composite non-identity key", span));
    };
    Ok(saved_key_to_value(key.clone()))
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
