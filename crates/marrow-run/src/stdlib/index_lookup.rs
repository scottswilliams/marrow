use marrow_check::{CheckedExpr as ExecExpr, CheckedSavedTerminal};
use marrow_store::cell::CatalogId;
use marrow_store::key::{SavedKey, decode_identity_payload_arity};
use marrow_store::tree::TreeStore;
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::env::Env;
use crate::error::{Located, RUN_TYPE, RuntimeError, unsupported};
use crate::expr::eval_expr;
use crate::store::IndexAddress;
use crate::value::{Value, identity_value, value_to_key};

pub(crate) fn is_index_branch(expr: &ExecExpr, _env: &Env<'_>) -> bool {
    matches!(
        expr.saved_place().map(|place| &place.terminal),
        Some(CheckedSavedTerminal::Index { .. })
    )
}

pub(crate) fn is_iterable_index_branch(expr: &ExecExpr, _env: &Env<'_>) -> bool {
    matches!(
        expr.saved_place().map(|place| &place.terminal),
        Some(CheckedSavedTerminal::Index { unique: false, .. })
    )
}

pub(crate) fn check_key_collection(
    expr: &ExecExpr,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<(), RuntimeError> {
    if is_index_branch(expr, env) && !is_iterable_index_branch(expr, env) {
        return Err(unsupported("keys over a unique index lookup", span));
    }
    Ok(())
}

pub(crate) struct UniqueIndexLookup {
    pub(crate) address: IndexAddress,
    pub(crate) identity_arity: usize,
    pub(crate) index_name: String,
    pub(crate) remaining_key_depth: usize,
}

pub(crate) fn unique_index_lookup(
    expr: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<Option<UniqueIndexLookup>, RuntimeError> {
    let Some(place) = expr.saved_place() else {
        return Ok(None);
    };
    let CheckedSavedTerminal::Index {
        name: index_name,
        catalog_id,
        args,
        unique: true,
        arg_count: index_arg_count,
        ..
    } = &place.terminal
    else {
        return Ok(None);
    };
    let mut keys = Vec::new();
    for arg in args {
        if arg.mode.is_some() || arg.name.is_some() {
            return Err(unsupported(
                "an index lookup with named or out arguments",
                place.span,
            ));
        }
        keys.push(
            value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| unsupported("an index key of this type", place.span))?,
        );
    }
    Ok(Some(UniqueIndexLookup {
        address: IndexAddress::new(catalog_id, keys, place.span)?,
        identity_arity: place.identity_keys.len(),
        index_name: index_name.clone(),
        remaining_key_depth: index_arg_count.saturating_sub(args.len()),
    }))
}

pub(crate) fn unique_index_lookup_values(
    expr: &ExecExpr,
    span: SourceSpan,
    dir: Direction,
    env: &mut Env<'_>,
) -> Result<Option<Vec<Value>>, RuntimeError> {
    let Some(lookup) = unique_index_lookup(expr, env)? else {
        return Ok(None);
    };
    if lookup.remaining_key_depth > 0 {
        return collect_unique_index_values(
            &lookup.address.keys,
            lookup.remaining_key_depth,
            &lookup,
            dir,
            span,
            env,
        )
        .map(Some);
    }
    read_unique_index_value(&lookup.address.keys, &lookup, span, env)
        .map(|value| Some(value.map_or_else(Vec::new, |value| vec![value])))
}

fn collect_unique_index_values(
    prefix: &[SavedKey],
    depth: usize,
    lookup: &UniqueIndexLookup,
    dir: Direction,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    let mut values = Vec::new();
    let mut child = first_index_child(env.store, &lookup.address.index, prefix, dir, span)?;
    while let Some(key) = child {
        let anchor = key.clone();
        let mut path = prefix.to_vec();
        path.push(key);
        if depth <= 1 {
            if let Some(value) = read_unique_index_value(&path, lookup, span, env)? {
                values.push(value);
            }
        } else {
            values.extend(collect_unique_index_values(
                &path,
                depth - 1,
                lookup,
                dir,
                span,
                env,
            )?);
        }
        child = next_index_child(
            env.store,
            &lookup.address.index,
            &path[..path.len() - 1],
            &anchor,
            dir,
            span,
        )?;
    }
    Ok(values)
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

fn read_unique_index_value(
    keys: &[SavedKey],
    lookup: &UniqueIndexLookup,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    let page = env
        .store
        .scan_index_tuple(&lookup.address.index, keys, 1)
        .map_err(|error| error.located(span))?;
    let Some(entry) = page.entries.first() else {
        return Ok(None);
    };
    let identity =
        decode_identity_payload_arity(&entry.value, lookup.identity_arity).ok_or_else(|| {
            RuntimeError {
                throw: None,
                origin: None,
                code: RUN_TYPE,
                message: format!(
                    "the `{}` index entry did not decode to an identity",
                    lookup.index_name
                ),
                span,
            }
        })?;
    Ok(Some(identity_value(identity)))
}
