//! Sequence and keyed builtins: keys/values/entries/reversed/neighbor/append.

use crate::*;

/// Where a saved read sits, which decides how an absent element fails. A
/// value-position read (`^book(id).title` used as a value) raises a catchable
/// `run.absent_element` fault a `try`/`catch` can bind; materialization after an
/// address/key has already been chosen stays a plain fatal fault.
#[derive(Clone, Copy)]
pub(crate) enum ReadPosition {
    Value,
    Materialization,
}

/// The order a saved-layer walk yields its children. `for`/`keys`/`values`/
/// `entries` enumerate `Ascending`; `reversed(...)` enumerates `Descending`; and
/// `next`/`prev` seek the next/previous neighbor. The whole walk reverses as one,
/// so a composite identity is true-reversed at every level, not only its
/// outermost component.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Ascending,
    Descending,
}

#[derive(Clone, Copy)]
pub(crate) enum StreamChildKind {
    Record,
    Index,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamBinding {
    Key,
    KeyValue,
}

pub(crate) struct SimpleSavedLayerLoop {
    pub(crate) prefix: Vec<PathSegment>,
    pub(crate) dir: Direction,
    pub(crate) child_kind: StreamChildKind,
    pub(crate) depth: usize,
    pub(crate) key_prefix: Vec<SavedKey>,
    pub(crate) value_source: Option<StreamValueSource>,
}

#[derive(Clone)]
pub(crate) enum StreamValueSource {
    Root {
        root: String,
    },
    ChildLayer {
        root: String,
        identity: Vec<SavedKey>,
        parents: Vec<(String, Vec<SavedKey>)>,
        layer: String,
    },
}

/// Classify saved-layer `for` loops that can walk keys through backend seeks
/// instead of first materializing the whole key list. A direct saved iterable
/// binds keys/identities; a two-name direct loop also reads the value for that
/// key inside the iteration. `keys(...)` remains address-only and cannot feed a
/// two-name binding.
pub(crate) fn simple_saved_layer_loop(
    iterable: &Expression,
    binding: StreamBinding,
    env: &mut Env<'_>,
) -> Result<Option<SimpleSavedLayerLoop>, RuntimeError> {
    let (iterable, dir) = match reversed_argument(iterable) {
        Some(inner) => (inner, Direction::Descending),
        None => (iterable, Direction::Ascending),
    };
    let (path, value_source) = if let Some(path) = keys_argument(iterable) {
        if binding == StreamBinding::KeyValue {
            return Ok(None);
        }
        (path, None)
    } else if is_saved_path(iterable) {
        (iterable, stream_value_source(iterable, env)?)
    } else {
        return Ok(None);
    };
    match path {
        Expression::SavedRoot { name, span } => {
            let depth = match root_identity_arity(env.program, name) {
                Some(arity) if arity > 0 => arity,
                Some(0) => {
                    return Err(type_error(
                        &format!("`^{name}` is a singleton with no identities to iterate"),
                        *span,
                    ));
                }
                Some(_) => unreachable!("positive identity arity handled above"),
                None => return Ok(None),
            };
            Ok(Some(SimpleSavedLayerLoop {
                prefix: vec![PathSegment::Root(name.clone())],
                dir,
                child_kind: StreamChildKind::Record,
                depth,
                key_prefix: Vec::new(),
                value_source,
            }))
        }
        Expression::Call {
            callee, args, span, ..
        } if matches!(callee.as_ref(), Expression::Field { base, .. } if matches!(base.as_ref(), Expression::SavedRoot { .. })) => {
            stream_index_branch(callee, args, dir, value_source, binding, *span, env)
        }
        Expression::Field { base, .. }
            if matches!(base.as_ref(), Expression::SavedRoot { .. })
                && is_iterable_index_branch(path, env) =>
        {
            stream_index_branch(path, &[], dir, value_source, binding, path.span(), env)
        }
        Expression::Field {
            base, name: layer, ..
        } => {
            let (root, identity, parents) = lower(base, env)?.into_layers(base.span())?;
            let mut with_layer = parents.clone();
            with_layer.push((layer.clone(), Vec::new()));
            Ok(Some(SimpleSavedLayerLoop {
                prefix: saved_segments(&root, &identity, &with_layer, None),
                dir,
                child_kind: StreamChildKind::Index,
                depth: 1,
                key_prefix: Vec::new(),
                value_source,
            }))
        }
        _ => Ok(None),
    }
}

fn stream_index_branch(
    callee: &Expression,
    args: &[Argument],
    dir: Direction,
    value_source: Option<StreamValueSource>,
    binding: StreamBinding,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<SimpleSavedLayerLoop>, RuntimeError> {
    if value_source.is_some() {
        return Ok(None);
    }
    let Expression::Field {
        base, name: index, ..
    } = callee
    else {
        return Ok(None);
    };
    let Expression::SavedRoot { name: root, .. } = base.as_ref() else {
        return Ok(None);
    };
    let resource = find_resource(env.program, root)
        .ok_or_else(|| unsupported("iterating this saved path", span))?;
    let Some(schema) = resource.indexes.iter().find(|i| &i.name == index) else {
        return Err(unsupported("iterating this saved path", span));
    };
    if schema.unique {
        return Ok(None);
    }
    if args
        .iter()
        .any(|arg| arg.mode.is_some() || arg.name.is_some())
        || args.len() > schema.args.len()
    {
        return Err(unsupported("iterating this saved path", span));
    }
    let mut prefix = vec![
        PathSegment::Root(root.clone()),
        PathSegment::Index(index.clone()),
    ];
    let mut arg_keys = Vec::new();
    for arg in args {
        let key = value_to_key(eval_expr(&arg.value, env)?)
            .ok_or_else(|| unsupported("an index key of this type", span))?;
        prefix.push(PathSegment::IndexKey(key.clone()));
        arg_keys.push(key);
    }
    let identity_arity = resource
        .saved_root
        .as_ref()
        .map_or(0, |root| root.identity_keys.len());
    let identity_start = schema.args.len().saturating_sub(identity_arity);
    let depth = if args.len() < identity_start {
        1
    } else {
        schema.args.len().saturating_sub(args.len())
    };
    let value_source = if binding == StreamBinding::KeyValue {
        if args.len() < identity_start {
            return Err(unsupported(
                "a two-name binding over this index branch",
                span,
            ));
        }
        Some(StreamValueSource::Root { root: root.clone() })
    } else {
        value_source
    };
    let key_prefix = arg_keys
        .get(identity_start..)
        .map_or_else(Vec::new, |keys| keys.to_vec());
    Ok(Some(SimpleSavedLayerLoop {
        prefix,
        dir,
        child_kind: StreamChildKind::Index,
        depth,
        key_prefix,
        value_source,
    }))
}

fn stream_value_source(
    path: &Expression,
    env: &mut Env<'_>,
) -> Result<Option<StreamValueSource>, RuntimeError> {
    match path {
        Expression::SavedRoot { name, .. } => {
            Ok(Some(StreamValueSource::Root { root: name.clone() }))
        }
        Expression::Field { base, name, .. } if !is_index_branch(path, env) => {
            let (root, identity, parents) = lower(base, env)?.into_layers(base.span())?;
            Ok(Some(StreamValueSource::ChildLayer {
                root,
                identity,
                parents,
                layer: name.clone(),
            }))
        }
        _ => Ok(None),
    }
}

pub(crate) fn stream_child_segment(kind: StreamChildKind, key: SavedKey) -> PathSegment {
    match kind {
        StreamChildKind::Record => PathSegment::RecordKey(key),
        StreamChildKind::Index => PathSegment::IndexKey(key),
    }
}

/// The absent-element error for a read at `position`: catchable in value
/// position, plain fatal during materialization.
pub(crate) fn absent_read(
    position: ReadPosition,
    message: String,
    span: SourceSpan,
) -> RuntimeError {
    match position {
        ReadPosition::Value => raise_fault(RUN_ABSENT, message, span),
        ReadPosition::Materialization => RuntimeError::fault(RUN_ABSENT, message, span),
    }
}

/// Evaluate `nextId(^root)`: the next integer identity for a single-`int` keyed
/// saved root (one past the highest existing key, or 1 when empty).
pub(crate) fn eval_next_id(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: "`nextId` takes one argument".into(),
            span,
        });
    };
    let Expression::SavedRoot { name, .. } = &arg.value else {
        return Err(unsupported("`nextId` of this path", span));
    };
    // Resolve the root to its schema (mirroring `eval_append`): an undeclared root
    // is a `run.unsupported`, while a declared root with no default integer policy
    // (composite, non-int, or singleton) surfaces as a catchable `write.*` fault
    // from `next_id`. The schema-driven gate replaces the old name-only scan, which
    // would invent a bogus `Int(1)` for any root.
    let resource = find_resource(env.program, name)
        .ok_or_else(|| unsupported("`nextId` of an undeclared saved root", span))?;
    let next = {
        let store = env.store.borrow();
        next_id(resource, &*store)
    };
    let next = next.map_err(|error| write_fault(error, span))?;
    Ok(Value::Int(next))
}

/// Evaluate `append(^root(key…).layer, value)`: write `value` at the next 1-based
/// position of a keyed-leaf layer and return that position. The layer may sit
/// directly under the record or below keyed group entries.
pub(crate) fn eval_append(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [target, value] = args else {
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: "`append` takes a layer path and a value".into(),
            span,
        });
    };
    if let Expression::Name {
        segments,
        span: target_span,
    } = &target.value
        && let [name] = segments.as_slice()
    {
        if env.lookup(name).is_none() {
            return Err(assign_error(name, AssignError::Unbound, *target_span));
        }
        let appended = eval_expr(&value.value, env)?;
        let Some(Value::Sequence(mut items)) = env.lookup(name).cloned() else {
            return Err(unsupported("appending to this path", span));
        };
        items.push(appended);
        let pos = i64::try_from(items.len()).map_err(|_| overflow(span))?;
        env.assign(name, Value::Sequence(items))
            .map_err(|error| assign_error(name, error, *target_span))?;
        return Ok(Value::Int(pos));
    }
    let Expression::Field {
        base, name: layer, ..
    } = &target.value
    else {
        return Err(unsupported("appending to this path", span));
    };
    let (root, identity, parents) = lower(base, env)?.into_layers(base.span())?;
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("appending under this saved root", span))?;
    // Append adds a key to this layer's key set.
    env.guard_traversed_layer(
        &nested_layer_prefix(&root, &identity, &parents, layer),
        span,
    )?;
    let saved = value_to_saved(eval_expr(&value.value, env)?)
        .ok_or_else(|| unsupported("appending a resource value", span))?;
    let pos = {
        let store = env.store.borrow();
        if parents.is_empty() {
            next_layer_pos(resource, &identity, layer, &*store)
        } else {
            let parent_refs: Vec<(&str, &[SavedKey])> = parents
                .iter()
                .map(|(name, keys)| (name.as_str(), keys.as_slice()))
                .collect();
            next_nested_layer_pos(resource, &identity, &parent_refs, layer, &*store)
        }
    };
    let pos = pos.map_err(|error| write_fault(error, span))?;
    let plan = if parents.is_empty() {
        plan_layer_leaf_write(resource, &identity, layer, &[SavedKey::Int(pos)], &saved)
    } else {
        let parent_refs: Vec<(&str, &[SavedKey])> = parents
            .iter()
            .map(|(name, keys)| (name.as_str(), keys.as_slice()))
            .collect();
        plan_nested_layer_leaf_write(
            resource,
            &identity,
            &parent_refs,
            layer,
            &[SavedKey::Int(pos)],
            &saved,
        )
    };
    env.apply_plan(plan, span)?;
    Ok(Value::Int(pos))
}

pub(crate) fn eval_local_collection_read(
    name: &str,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    match env.lookup(name).cloned() {
        Some(Value::Sequence(items)) => read_local_sequence(items, args, span, env).map(Some),
        Some(Value::LocalTree(entries)) => {
            let keys = eval_local_keys(args, span, env)?;
            entries
                .into_iter()
                .find(|entry| entry.keys == keys)
                .map(|entry| entry.value)
                .ok_or_else(|| {
                    absent_read(ReadPosition::Value, "`local tree` is absent".into(), span)
                })
                .map(Some)
        }
        _ => Ok(None),
    }
}

pub(crate) fn eval_local_collection_write(
    name: &str,
    args: &[Argument],
    value: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<bool, RuntimeError> {
    match env.lookup(name).cloned() {
        Some(Value::Sequence(mut items)) => {
            let index = eval_local_sequence_index(args, span, env)?;
            let value = eval_expr(value, env)?;
            if index == items.len() {
                items.push(value);
            } else if let Some(slot) = items.get_mut(index) {
                *slot = value;
            } else {
                return Err(unsupported(
                    "writing a sparse local sequence position",
                    span,
                ));
            }
            env.assign(name, Value::Sequence(items))
                .map_err(|error| assign_error(name, error, span))?;
            Ok(true)
        }
        Some(Value::LocalTree(mut entries)) => {
            let keys = eval_local_keys(args, span, env)?;
            let value = eval_expr(value, env)?;
            if let Some(entry) = entries.iter_mut().find(|entry| entry.keys == keys) {
                entry.value = value;
            } else {
                entries.push(LocalTreeEntry { keys, value });
                entries.sort_by_key(|entry| local_key_sort_key(&entry.keys));
            }
            env.assign(name, Value::LocalTree(entries))
                .map_err(|error| assign_error(name, error, span))?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub(crate) fn local_collection_count(
    value: Value,
    span: SourceSpan,
) -> Result<Value, RuntimeError> {
    match value {
        Value::Sequence(items) => i64::try_from(items.len())
            .map(Value::Int)
            .map_err(|_| overflow(span)),
        Value::LocalTree(entries) => i64::try_from(entries.len())
            .map(Value::Int)
            .map_err(|_| overflow(span)),
        _ => Err(unsupported("counting this value", span)),
    }
}

pub(crate) fn enumerate_local_collection_dir(
    value: Value,
    dir: Direction,
    span: SourceSpan,
) -> Result<Vec<Value>, RuntimeError> {
    let mut keys = match value {
        Value::Sequence(items) => (1..=items.len())
            .map(|pos| {
                i64::try_from(pos)
                    .map(Value::Int)
                    .map_err(|_| overflow(span))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Value::LocalTree(entries) => {
            let mut seen = Vec::<SavedKey>::new();
            for entry in entries {
                let Some(key) = entry.keys.first().cloned() else {
                    continue;
                };
                if !seen.contains(&key) {
                    seen.push(key);
                }
            }
            seen.into_iter()
                .map(saved_key_to_value)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| unsupported("iterating keys of this type", span))?
        }
        _ => return Err(unsupported("keys over this value", span)),
    };
    if dir == Direction::Descending {
        keys.reverse();
    }
    Ok(keys)
}

pub(crate) fn materialize_local_collection_dir(
    value: Value,
    dir: Direction,
    span: SourceSpan,
) -> Result<Vec<(Value, Value)>, RuntimeError> {
    let mut rows = match value {
        Value::Sequence(items) => items
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                let pos = i64::try_from(index + 1).map_err(|_| overflow(span))?;
                Ok((Value::Int(pos), value))
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?,
        Value::LocalTree(entries) => entries
            .into_iter()
            .map(|entry| {
                let key = entry
                    .keys
                    .first()
                    .cloned()
                    .and_then(saved_key_to_value)
                    .ok_or_else(|| unsupported("iterating keys of this type", span))?;
                Ok((key, entry.value))
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?,
        _ => return Err(unsupported("values/entries over this value", span)),
    };
    if dir == Direction::Descending {
        rows.reverse();
    }
    Ok(rows)
}

fn read_local_sequence(
    items: Vec<Value>,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let index = eval_local_sequence_index(args, span, env)?;
    items.get(index).cloned().ok_or_else(|| {
        absent_read(
            ReadPosition::Value,
            "`local sequence` is absent".into(),
            span,
        )
    })
}

fn eval_local_sequence_index(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<usize, RuntimeError> {
    let [arg] = args else {
        return Err(type_error("a local sequence lookup takes one key", span));
    };
    if arg.mode.is_some() || arg.name.is_some() {
        return Err(unsupported(
            "named or out arguments in a local collection lookup",
            span,
        ));
    }
    let Value::Int(pos) = eval_expr(&arg.value, env)? else {
        return Err(type_error("a local sequence key must be an int", span));
    };
    if pos < 1 {
        return Err(type_error("a local sequence key must be positive", span));
    }
    usize::try_from(pos - 1).map_err(|_| overflow(span))
}

fn eval_local_keys(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<SavedKey>, RuntimeError> {
    args.iter()
        .map(|arg| {
            if arg.mode.is_some() || arg.name.is_some() {
                return Err(unsupported(
                    "named or out arguments in a local collection lookup",
                    span,
                ));
            }
            value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| unsupported("a key of this type", span))
        })
        .collect()
}

fn local_key_sort_key(keys: &[SavedKey]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for key in keys {
        bytes.extend(encode_path(&[PathSegment::IndexKey(key.clone())]));
    }
    bytes
}

/// Evaluate `keys(<layer>)` as a value: enumerate the layer's child keys into a
/// [`Value::Sequence`]. Direct loops use this same enumeration only for
/// address-only collections such as index branches.
pub(crate) fn eval_keys(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [path] = args else {
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: "`keys` takes one argument".into(),
            span,
        });
    };
    if !is_saved_path(&path.value) {
        return Ok(Value::Sequence(enumerate_local_collection_dir(
            eval_expr(&path.value, env)?,
            Direction::Ascending,
            span,
        )?));
    }
    check_key_collection(&path.value, span, env)?;
    Ok(Value::Sequence(enumerate_layer(&path.value, env)?))
}

/// Evaluate `values(<layer>)`: each child materialized to its value, in key
/// order. The same materialization drives `for x in values(<layer>)`.
pub(crate) fn eval_values(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [path] = args else {
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: "`values` takes one argument".into(),
            span,
        });
    };
    if !is_saved_path(&path.value) {
        let values = materialize_local_collection_dir(
            eval_expr(&path.value, env)?,
            Direction::Ascending,
            span,
        )?
        .into_iter()
        .map(|(_, value)| value)
        .collect();
        return Ok(Value::Sequence(values));
    }
    let values = materialize_layer(&path.value, env)?
        .into_iter()
        .map(|(_, value)| value)
        .collect();
    Ok(Value::Sequence(values))
}

/// Evaluate `entries(<layer>)`: each child as a `[key, value]` pair sequence, in
/// key order. The two-name `for k, v in entries(<layer>)` binding unpacks each
/// pair; the same materialization drives it.
pub(crate) fn eval_entries(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [path] = args else {
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: "`entries` takes one argument".into(),
            span,
        });
    };
    if !is_saved_path(&path.value) {
        let entries = materialize_local_collection_dir(
            eval_expr(&path.value, env)?,
            Direction::Ascending,
            span,
        )?
        .into_iter()
        .map(|(key, value)| Value::Sequence(vec![key, value]))
        .collect();
        return Ok(Value::Sequence(entries));
    }
    let entries = materialize_layer(&path.value, env)?
        .into_iter()
        .map(|(key, value)| Value::Sequence(vec![key, value]))
        .collect();
    Ok(Value::Sequence(entries))
}

/// Evaluate `reversed(<iterable>)`: the same elements in reverse key order. Over a
/// saved layer — directly (`reversed(L)`/`reversed(keys(L))`), or wrapped in
/// `values`/`entries` — the reversal is pushed into the enumeration as a
/// `Descending` store walk, so composite identities reverse at every key level.
/// Over any other value the argument must already be an in-memory `sequence`,
/// whose `Vec` is reversed in place (e.g. `reversed(std::text::split(...))`).
pub(crate) fn eval_reversed(
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: "`reversed` takes one argument".into(),
            span,
        });
    };
    // `reversed(values(L))` / `reversed(entries(L))`: materialize the layer in
    // reverse key order, then shape the rows the way `values`/`entries` do.
    if let Some(inner) = values_or_entries(&arg.value) {
        let rows = if is_saved_path(inner.layer) {
            materialize_layer_dir(inner.layer, Direction::Descending, env)?
        } else {
            materialize_local_collection_dir(
                eval_expr(inner.layer, env)?,
                Direction::Descending,
                span,
            )?
        };
        return Ok(Value::Sequence(match inner.kind {
            MaterializeKind::Values => rows.into_iter().map(|(_, value)| value).collect(),
            MaterializeKind::Entries => rows
                .into_iter()
                .map(|(key, value)| Value::Sequence(vec![key, value]))
                .collect(),
        }));
    }
    // `reversed(keys(L))`: addresses, descending.
    if let Some(layer) = keys_argument(&arg.value) {
        if !is_saved_path(layer) {
            return Ok(Value::Sequence(enumerate_local_collection_dir(
                eval_expr(layer, env)?,
                Direction::Descending,
                span,
            )?));
        }
        check_key_collection(layer, span, env)?;
        return Ok(Value::Sequence(enumerate_layer_dir(
            layer,
            Direction::Descending,
            env,
        )?));
    }
    // `reversed(L)`: direct saved collections keep the same address-oriented
    // element as ordinary `for`: identities for roots and indexes, child keys for
    // keyed layers. Use `reversed(values(L))` when values are wanted.
    if is_saved_path(&arg.value) {
        if let Some(values) =
            unique_index_lookup_values(&arg.value, span, Direction::Descending, env)?
        {
            return Ok(Value::Sequence(values));
        }
        return Ok(Value::Sequence(enumerate_layer_dir(
            &arg.value,
            Direction::Descending,
            env,
        )?));
    }
    // Any other argument must evaluate to an in-memory sequence, reversed directly.
    match eval_expr(&arg.value, env)? {
        Value::Sequence(mut items) => {
            items.reverse();
            Ok(Value::Sequence(items))
        }
        Value::LocalTree(entries) => Ok(Value::Sequence(
            materialize_local_collection_dir(
                Value::LocalTree(entries),
                Direction::Descending,
                span,
            )?
            .into_iter()
            .map(|(_, value)| value)
            .collect(),
        )),
        _ => Err(unsupported(
            "reversing this value (expected an iterable)",
            span,
        )),
    }
}

/// Which value shape a `values`/`entries` wrapper materializes.
pub(crate) enum MaterializeKind {
    Values,
    Entries,
}

/// A `values(L)`/`entries(L)` wrapper recognized inside `reversed(...)`: the inner
/// layer expression and which shape it materializes.
pub(crate) struct ValuesOrEntries<'a> {
    pub(crate) layer: &'a Expression,
    pub(crate) kind: MaterializeKind,
}

/// Classify a `values(<layer>)` or `entries(<layer>)` call, or `None` otherwise.
pub(crate) fn values_or_entries(expr: &Expression) -> Option<ValuesOrEntries<'_>> {
    let Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    let Expression::Name { segments, .. } = callee.as_ref() else {
        return None;
    };
    let kind = match segments.as_slice() {
        [name] if name == "values" => MaterializeKind::Values,
        [name] if name == "entries" => MaterializeKind::Entries,
        _ => return None,
    };
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => Some(ValuesOrEntries {
            layer: &arg.value,
            kind,
        }),
        _ => None,
    }
}

/// Evaluate `next(<element>)` (`Ascending`) or `prev(<element>)` (`Descending`):
/// the nearest stored neighbor in key order, skipping gaps. The argument is a
/// saved path scoped to one key level — a specific element (`^root(id)`,
/// `^root(id).layer(k)`) whose innermost key is the seek anchor, or a bare layer
/// (`^root`, `^root(id).layer`) whose first/last stored child is returned. The
/// result is the neighbor identity, read fields through `^root(neighbor).field`.
/// Stepping off the edge (`next` of the last, `prev` of the first) raises the
/// catchable `run.absent_element`, so it composes with `??`.
pub(crate) fn eval_neighbor(
    dir: Direction,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        let which = if dir == Direction::Ascending {
            "next"
        } else {
            "prev"
        };
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: format!("`{which}` takes one argument"),
            span,
        });
    };
    let (parent, anchor) = neighbor_target(&arg.value, env)?;
    let parent_bytes = encode_path(&parent);
    let neighbor = {
        let store = env.store.borrow();
        let result = match (&anchor, dir) {
            // A bare layer: the first stored child for `next`, the last for `prev`.
            (None, Direction::Ascending) => store.first_child(&parent_bytes),
            (None, Direction::Descending) => store.last_child(&parent_bytes),
            // A specific element: the sibling just past its key, gaps skipped.
            (Some(segment), Direction::Ascending) => store.next_sibling(&parent_bytes, segment),
            (Some(segment), Direction::Descending) => store.prev_sibling(&parent_bytes, segment),
        };
        result.map_err(|_| RuntimeError {
            throw: None,
            origin: None,
            code: RUN_STORE,
            message: "could not seek the neighbor at this path".into(),
            span,
        })?
    };
    match neighbor {
        Some(ChildSegment::Key(key)) => {
            saved_key_to_value(key).ok_or_else(|| unsupported("a neighbor key of this type", span))
        }
        // The store's neighbor seek navigates key positions only — it skips named
        // members (a declared index, field, or child layer), so a name here can come
        // only from a corrupt store row, not from stepping onto an index branch.
        Some(ChildSegment::Name(_)) => Err(unsupported("a neighbor at this path", span)),
        None => {
            let edge = if dir == Direction::Ascending {
                "after"
            } else {
                "before"
            };
            Err(raise_fault(
                RUN_ABSENT,
                format!("no element {edge} this position in its layer"),
                span,
            ))
        }
    }
}

/// Split a `next`/`prev` argument into the parent layer prefix and the optional
/// anchor child segment to seek past. A specific element — a record `^root(id…)`
/// (single-key identity) or a keyed group entry `^root(id…).layer(k)` — anchors at
/// its innermost key, with the parent being everything above it. A bare layer —
/// the primary keyed root `^root` or an unkeyed group hop `^root(id).layer` — has
/// no anchor (its `first`/`last` child is sought). Other shapes (index branches,
/// composite multi-level identities, fields) are unsupported.
pub(crate) fn neighbor_target(
    expr: &Expression,
    env: &mut Env<'_>,
) -> Result<(Vec<PathSegment>, Option<Vec<u8>>), RuntimeError> {
    let span = expr.span();
    // A bare primary keyed root `^root`: seek among its record identities.
    if let Expression::SavedRoot { name, .. } = expr
        && root_identity_arity(env.program, name).is_some_and(|arity| arity > 0)
    {
        return Ok((vec![PathSegment::Root(name.clone())], None));
    }
    let path = lower(expr, env)?;
    if !matches!(path.terminal, Terminal::Record) {
        return Err(unsupported("`next`/`prev` of this path", span));
    }
    // No layers: a record address `^root(id…)`. Its identity must be a single key
    // (one level), the anchor; the parent is the root. The anchor is the encoded
    // record-key child segment (kind tag + key) the store's seek compares against.
    if path.layers.is_empty() {
        return match path.identity.as_slice() {
            [key] => Ok((
                vec![PathSegment::Root(path.root.clone())],
                Some(encode_path(&[PathSegment::RecordKey(key.clone())])),
            )),
            // A composite identity addresses many levels; `next`/`prev` are scoped
            // to one key level, so a multi-key record is out of scope here.
            _ => Err(unsupported(
                "`next`/`prev` of a composite-identity record (scope a single key level)",
                span,
            )),
        };
    }
    // A layer chain: the parent is the root, identity, and every layer level above
    // the innermost, ending at the innermost layer's name. The innermost layer's
    // last key is the anchor — an encoded index-key child segment; an unkeyed group
    // hop (`^root(id).layer`) has none, so it is a bare layer whose first/last child
    // is sought.
    let (last_name, last_keys) = path.layers.last().expect("non-empty checked above");
    let prior = &path.layers[..path.layers.len() - 1];
    let mut parent = saved_segments(&path.root, &path.identity, prior, None);
    parent.push(PathSegment::ChildLayer(last_name.clone()));
    match last_keys.as_slice() {
        [] => Ok((parent, None)),
        [key] => Ok((
            parent,
            Some(encode_path(&[PathSegment::IndexKey(key.clone())])),
        )),
        _ => Err(unsupported(
            "`next`/`prev` of a multi-key layer position (scope a single key level)",
            span,
        )),
    }
}

/// Materialize a layer's children as `(key, value)` pairs in key order: a whole
/// record per child key for a primary root `^books`, or each entry's value for a
/// keyed/sequence child layer `^books(id).tags`. Reuses [`enumerate_layer`] for
/// the keys and the existing whole-record / layer-entry reads for the values.
/// Index branches inspect identities only, so `values`/`entries`
/// over one is rejected; iterate it or use `keys(...)` instead.
pub(crate) fn materialize_layer(
    path: &Expression,
    env: &mut Env<'_>,
) -> Result<Vec<(Value, Value)>, RuntimeError> {
    materialize_layer_dir(path, Direction::Ascending, env)
}

/// [`materialize_layer`] in `dir` order: the keys enumerate in that direction, so
/// `reversed(values(L))` / `reversed(entries(L))` materialize each child value in
/// reverse key order.
pub(crate) fn materialize_layer_dir(
    path: &Expression,
    dir: Direction,
    env: &mut Env<'_>,
) -> Result<Vec<(Value, Value)>, RuntimeError> {
    if !is_saved_path(path) {
        return materialize_local_collection_dir(eval_expr(path, env)?, dir, path.span());
    }
    let keys = enumerate_layer_dir(path, dir, env)?;
    match path {
        // A primary keyed root: each child key is a record identity, materialized
        // by a whole-record read.
        Expression::SavedRoot { name, span } => keys
            .into_iter()
            .map(|key| {
                let identity = identity_keys(&key, *span)?;
                Ok((key, read_resource(name, &identity, *span, env)?))
            })
            .collect(),
        // A keyed/sequence child layer `^root(id…).layer`: each child key addresses
        // one entry, materialized by a layer-entry read.
        Expression::Field {
            base, name: layer, ..
        } => {
            let span = path.span();
            let (root, identity, parents) = lower(base, env)?.into_layers(base.span())?;
            keys.into_iter()
                .map(|key| {
                    let layer_key = value_to_key(key.clone())
                        .ok_or_else(|| unsupported("a key of this type", span))?;
                    // Materializing a known-present child key: an absent entry is a
                    // plain fatal fault, not a catchable value-position read.
                    let value = if parents.is_empty() {
                        read_layer_entry(
                            &root,
                            &identity,
                            layer,
                            &[layer_key],
                            ReadPosition::Materialization,
                            span,
                            env,
                        )?
                    } else {
                        read_layer_entry_at(
                            LayerEntryAddress {
                                root: &root,
                                identity: &identity,
                                parent_layers: &parents,
                                layer,
                                layer_keys: &[layer_key],
                            },
                            ReadPosition::Materialization,
                            span,
                            env,
                        )?
                    };
                    Ok((key, value))
                })
                .collect()
        }
        // An index branch `^root.index(args…)` yields identities for `keys(...)`;
        // its marker values are a raw inspection detail, not `values`/`entries`.
        other => Err(unsupported(
            "values/entries over this path (use keys(...) or direct iteration)",
            other.span(),
        )),
    }
}

/// The identity keys a primary-root child value addresses: a single-key identity
/// arrives as a bare key value, a composite one as a [`Value::Identity`].
pub(crate) fn identity_keys(key: &Value, span: SourceSpan) -> Result<Vec<SavedKey>, RuntimeError> {
    match key {
        Value::Identity(keys) => Ok(keys.clone()),
        other => Ok(vec![
            value_to_key(other.clone()).ok_or_else(|| unsupported("a key of this type", span))?,
        ]),
    }
}
