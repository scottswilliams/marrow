//! Sequence and keyed builtins: keys/values/entries/reversed/neighbor/append.

use crate::*;

/// Where a saved read sits, which decides how an absent element fails. A
/// value-position read (`^book(id).title` used as a value) raises a catchable
/// `run.absent_element` fault a `try`/`catch` can bind; an `inout`/`out` seed
/// read is argument binding, not value position, so it stays a plain fatal fault.
#[derive(Clone, Copy)]
pub(crate) enum ReadPosition {
    Value,
    ArgSeed,
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

/// The absent-element error for a read at `position`: catchable in value
/// position, plain fatal as an argument seed.
pub(crate) fn absent_read(
    position: ReadPosition,
    message: String,
    span: SourceSpan,
) -> RuntimeError {
    match position {
        ReadPosition::Value => raise_fault(RUN_ABSENT, message, span),
        ReadPosition::ArgSeed => RuntimeError::fault(RUN_ABSENT, message, span),
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
/// position of a keyed-leaf layer and return that position. Reuses the write
/// planner's `next_layer_pos` (over the live store) and `plan_layer_leaf_write`.
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
    let (root, identity) = lower(base, env)?.into_record(base.span())?;
    let resource = find_resource(env.program, &root)
        .ok_or_else(|| unsupported("appending under this saved root", span))?;
    // Append adds a key to this layer's key set.
    env.guard_traversed_layer(&layer_prefix(&root, &identity, layer), span)?;
    let saved = value_to_saved(eval_expr(&value.value, env)?)
        .ok_or_else(|| unsupported("appending a resource value", span))?;
    let pos = {
        let store = env.store.borrow();
        next_layer_pos(resource, &identity, layer, &*store)
    };
    let pos = pos.map_err(|error| write_fault(error, span))?;
    let plan = plan_layer_leaf_write(resource, &identity, layer, &[SavedKey::Int(pos)], &saved);
    env.apply_plan(plan, span)?;
    Ok(Value::Int(pos))
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
        let rows = materialize_layer_dir(inner.layer, Direction::Descending, env)?;
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
        check_key_collection(layer, span, env)?;
        return Ok(Value::Sequence(enumerate_layer_dir(
            layer,
            Direction::Descending,
            env,
        )?));
    }
    // `reversed(L)`: elements for value-bearing saved collections, identities for
    // key-only index branches.
    if is_saved_path(&arg.value) {
        if let Some(values) =
            unique_index_lookup_values(&arg.value, span, Direction::Descending, env)?
        {
            return Ok(Value::Sequence(values));
        }
        if is_iterable_index_branch(&arg.value, env) {
            return Ok(Value::Sequence(enumerate_layer_dir(
                &arg.value,
                Direction::Descending,
                env,
            )?));
        }
        let values = materialize_layer_dir(&arg.value, Direction::Descending, env)?
            .into_iter()
            .map(|(_, value)| value)
            .collect();
        return Ok(Value::Sequence(values));
    }
    // Any other argument must evaluate to an in-memory sequence, reversed directly.
    match eval_expr(&arg.value, env)? {
        Value::Sequence(mut items) => {
            items.reverse();
            Ok(Value::Sequence(items))
        }
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
    layer: &'a Expression,
    kind: MaterializeKind,
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
            let (root, identity) = lower(base, env)?.into_record(base.span())?;
            keys.into_iter()
                .map(|key| {
                    let layer_key = value_to_key(key.clone())
                        .ok_or_else(|| unsupported("a key of this type", span))?;
                    // Materializing a known-present child key: an absent entry is a
                    // plain fatal fault, not a catchable value-position read.
                    let value = read_layer_entry(
                        &root,
                        &identity,
                        layer,
                        &[layer_key],
                        ReadPosition::ArgSeed,
                        span,
                        env,
                    )?;
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
