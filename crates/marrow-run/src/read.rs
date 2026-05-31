//! Saved-data reads and layer/index enumeration.

use crate::*;

/// The encoded path prefix of the saved layer a `for` iterable traverses, or
/// `None` for a range or a local value (which traverse no saved layer). A saved
/// layer is traversed only when the iterable is a saved path directly or wrapped
/// in `keys`/`values`/`entries`; iterating a local — the "collect keys first"
/// pattern — has no saved layer to guard. The prefix is the path whose child keys
/// the loop walks: `[Root]` for a primary root, `[Root, Index, IndexKey…]` for an
/// index branch, `[Root, RecordKey…, ChildLayer]` for a keyed/sequence layer. It
/// matches the prefix [`enumerate_layer`] reads children under, so a mutation that
/// changes that layer is caught by [`Env::guard_traversed_layer`].
pub(crate) fn traversed_layer_prefix(
    iterable: &Expression,
    env: &mut Env<'_>,
) -> Result<Option<Vec<PathSegment>>, RuntimeError> {
    // `reversed(...)` traverses the same layer (just backward), so unwrap it the
    // same way `keys`/`values`/`entries` are — the guarded prefix is unchanged by
    // direction. A `reversed(keys(L))` peels both wrappers.
    let unwrapped = reversed_argument(iterable).unwrap_or(iterable);
    let path = traversal_argument(unwrapped).unwrap_or(unwrapped);
    if !is_saved_path(path) {
        return Ok(None);
    }
    match path {
        Expression::SavedRoot { name, .. } => Ok(Some(vec![PathSegment::Root(name.clone())])),
        // An index branch `^root.index(args…)`: the prefix is the root, index name,
        // and the supplied index-key args (the levels below are reconstructed
        // identities, so the traversed layer is the branch the args reach).
        Expression::Call {
            callee, args, span, ..
        } if matches!(callee.as_ref(), Expression::Field { base, .. } if matches!(base.as_ref(), Expression::SavedRoot { .. })) =>
        {
            let Expression::Field {
                base, name: index, ..
            } = callee.as_ref()
            else {
                return Ok(None);
            };
            let Expression::SavedRoot { name: root, .. } = base.as_ref() else {
                return Ok(None);
            };
            let mut prefix = vec![
                PathSegment::Root(root.clone()),
                PathSegment::Index(index.clone()),
            ];
            for arg in args {
                prefix.push(PathSegment::IndexKey(
                    value_to_key(eval_expr(&arg.value, env)?)
                        .ok_or_else(|| unsupported("an index key of this type", *span))?,
                ));
            }
            Ok(Some(prefix))
        }
        // A keyed/sequence child layer `^root(id…).layer`.
        Expression::Field {
            base, name: layer, ..
        } => {
            let (root, identity) = lower(base, env)?.into_record(base.span())?;
            Ok(Some(saved_segments(
                &root,
                &identity,
                &[(layer.clone(), Vec::new())],
                None,
            )))
        }
        _ => Ok(None),
    }
}

/// The sole argument of a `keys`/`values`/`entries` call, or `None` for any other
/// expression. These wrap a saved layer without changing which layer is traversed.
pub(crate) fn traversal_argument(expr: &Expression) -> Option<&Expression> {
    let Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    let Expression::Name { segments, .. } = callee.as_ref() else {
        return None;
    };
    if segments.len() != 1 || !matches!(segments[0].as_str(), "keys" | "values" | "entries") {
        return None;
    }
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}

/// The single argument of a `reversed(<iterable>)` call, or `None` for any other
/// expression. Lets the loop materializer and write-guard see through the
/// `reversed` wrapper to the layer it traverses (its inner `keys`/`values`/
/// `entries` or bare saved path), exactly as [`traversal_argument`] does.
pub(crate) fn reversed_argument(expr: &Expression) -> Option<&Expression> {
    let Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    let Expression::Name { segments, .. } = callee.as_ref() else {
        return None;
    };
    if segments.len() != 1 || segments[0] != "reversed" {
        return None;
    }
    match args.as_slice() {
        [arg] if arg.mode.is_none() && arg.name.is_none() => Some(&arg.value),
        _ => None,
    }
}

/// The single argument of a `keys(<path>)` call, or `None` for any other
/// expression. Shared by the loop materializer and the standalone `keys` builtin.
pub(crate) fn keys_argument(expr: &Expression) -> Option<&Expression> {
    let Expression::Call { callee, args, .. } = expr else {
        return None;
    };
    let Expression::Name { segments, .. } = callee.as_ref() else {
        return None;
    };
    if segments.len() != 1 || segments[0] != "keys" {
        return None;
    }
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
    path: &Expression,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    enumerate_layer_dir(path, Direction::Ascending, env)
}

/// Enumerate a saved layer's child keys in `dir` order — the direction-threaded
/// core of [`enumerate_layer`]. `Ascending` is `for`/`keys`/`values`/`entries`;
/// `Descending` is `reversed(...)`. The whole descent carries one direction, so a
/// composite identity reverses at every level.
pub(crate) fn enumerate_layer_dir(
    path: &Expression,
    dir: Direction,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    match path {
        // A primary keyed root: its immediate children are the record-key segments
        // of the (possibly composite) identity. A keyless singleton has none.
        Expression::SavedRoot { name, span } => {
            let arity = match root_identity_arity(env.program, name) {
                Some(0) => {
                    return Err(type_error(
                        &format!("`^{name}` is a singleton with no identities to iterate"),
                        *span,
                    ));
                }
                Some(arity) => arity,
                None => return Err(unsupported("iterating this saved path", *span)),
            };
            let prefix = vec![PathSegment::Root(name.clone())];
            collect_child_identities(&prefix, arity, &[], PathSegment::RecordKey, dir, *span, env)
        }
        // An index branch `^root.index(args…)` (a `Call` whose callee is a `.index`
        // off a saved root) or a keyed/sequence child layer `^root(id…).layer`.
        Expression::Call {
            callee, args, span, ..
        } if matches!(callee.as_ref(), Expression::Field { base, .. } if matches!(base.as_ref(), Expression::SavedRoot { .. })) => {
            enumerate_index_branch(callee, args, dir, *span, env)
        }
        Expression::Field { .. } => enumerate_child_layer(path, dir, env),
        other => Err(unsupported("iterating this saved path", other.span())),
    }
}

/// Enumerate the identities in a declared index branch `^root.index(args…)`. A
/// non-unique index ends with all identity keys, so the levels below the supplied
/// query args are the entry's remaining identity-key segments; descend them per
/// entry to reconstruct the full identity rather than only its first key component.
pub(crate) fn enumerate_index_branch(
    callee: &Expression,
    args: &[Argument],
    dir: Direction,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    let Expression::Field {
        base, name: index, ..
    } = callee
    else {
        return Err(unsupported("iterating this saved path", span));
    };
    let Expression::SavedRoot { name: root, .. } = base.as_ref() else {
        return Err(unsupported("iterating this saved path", span));
    };
    if args
        .iter()
        .any(|arg| arg.mode.is_some() || arg.name.is_some())
    {
        return Err(unsupported(
            "an index lookup with named or out arguments",
            span,
        ));
    }
    let mut prefix = vec![
        PathSegment::Root(root.clone()),
        PathSegment::Index(index.clone()),
    ];
    for arg in args {
        prefix.push(PathSegment::IndexKey(
            value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| unsupported("an index key of this type", span))?,
        ));
    }
    let schema = find_resource(env.program, root)
        .and_then(|resource| resource.indexes.iter().find(|i| &i.name == index))
        .ok_or_else(|| unsupported("iterating this saved path", span))?;
    let depth = schema.args.len().saturating_sub(args.len());
    collect_child_identities(&prefix, depth, &[], PathSegment::IndexKey, dir, span, env)
}

/// Enumerate the child keys of a keyed/sequence child layer `^root(id…).layer`.
/// The layer's keys are single-key (`pos: int` for a sequence, `playerId: string`
/// for a keyed tree), so each child key is a bare value.
pub(crate) fn enumerate_child_layer(
    path: &Expression,
    dir: Direction,
    env: &mut Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    let Expression::Field {
        base, name: layer, ..
    } = path
    else {
        return Err(unsupported("iterating this saved path", path.span()));
    };
    let span = path.span();
    let (root, identity) = lower(base, env)?.into_record(base.span())?;
    let prefix = saved_segments(&root, &identity, &[(layer.clone(), Vec::new())], None);
    collect_child_identities(&prefix, 1, &[], PathSegment::IndexKey, dir, span, env)
}

/// Collect the identities reachable below `prefix`, descending `depth` remaining
/// key levels. `make_segment` builds the [`PathSegment`] for each descent step —
/// `RecordKey` below a primary root, `IndexKey` below an index branch or child
/// layer. `keys` accumulates the key segments gathered so far. At the final level
/// each entry yields one identity: a single key value (renderable, addresses
/// `^root(key)`) for a single-key identity, or a [`Value::Identity`] for a
/// composite one.
pub(crate) fn collect_child_identities(
    prefix: &[PathSegment],
    depth: usize,
    keys: &[SavedKey],
    make_segment: fn(SavedKey) -> PathSegment,
    dir: Direction,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Vec<Value>, RuntimeError> {
    let children = {
        let store = env.store.borrow();
        let encoded = encode_path(prefix);
        // `reversed(...)` walks the store's double-ended range backward, so a
        // composite identity descends in reverse at every level — a true reverse,
        // not the outermost component flipped over an ascending tail.
        match dir {
            Direction::Ascending => store.child_keys(&encoded),
            Direction::Descending => store.child_keys_rev(&encoded),
        }
        .map_err(|_| RuntimeError {
            throw: None,
            origin: None,
            code: RUN_STORE,
            message: "could not read the keys at this path".into(),
            span,
        })?
    };
    let mut values = Vec::new();
    for child in children {
        let ChildSegment::Key(key) = child else {
            continue;
        };
        let mut keys = keys.to_vec();
        keys.push(key.clone());
        if depth <= 1 {
            // The last key level: a single-key identity stays a raw key value; a
            // composite one reconstructs its full `Value::Identity`.
            values.push(if keys.len() == 1 {
                saved_key_to_value(key)
                    .ok_or_else(|| unsupported("iterating keys of this type", span))?
            } else {
                Value::Identity(keys)
            });
        } else {
            let mut prefix = prefix.to_vec();
            prefix.push(make_segment(key));
            values.extend(collect_child_identities(
                &prefix,
                depth - 1,
                &keys,
                make_segment,
                dir,
                span,
                env,
            )?);
        }
    }
    Ok(values)
}

/// Read a scalar field off a saved record, e.g. `^books(id).title`. Lowers the
/// path, re-terminates it at the field, and reads the store, decoding the bytes
/// with the field's declared type from the resource schema. Top-level and
/// group-entry fields (`^root(key…).layer(key…).field`) take the same lowered
/// path; the layer chain it carries decides which read it is. An unpopulated
/// element is an absent-element error.
pub(crate) fn eval_saved_field(
    expr: &Expression,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Expression::Field {
        base, name, quoted, ..
    } = expr
    else {
        return Err(unsupported("this read", expr.span()));
    };
    read_saved_field(base, name, *quoted, expr.span(), env)
}

/// Read a saved field given its parts. Shared by the plain `.field` read and the
/// optional `?.field` read, which lower and read identically; only their
/// short-circuiting on an absent intermediate differs, and that is handled by the
/// callers raising/propagating `run.absent_element`.
pub(crate) fn read_saved_field(
    base: &Expression,
    name: &str,
    quoted: bool,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    // A quoted/raw segment under a managed root (`^books(id)."old-title"`) is raw
    // access: gated to maintenance, reading the literal segment rather than a
    // declared field. An unquoted field falls through to the declared-field path.
    if quoted {
        return eval_raw_field_read(base, name, span, env);
    }
    // A plain `^root(id…).field` base is a top-level field; a base reached through
    // one or more group layers (`^root(id…).layer(key…)….field` or the unkeyed
    // group hop `^root(id…).name.field`) is a field inside that group. Lowering the
    // base and re-terminating at the field carries the layer chain either way, and
    // `SavedPath::read` reads top-level or nested by whether that chain is empty.
    lower(base, env)?
        .into_field(name.to_string(), base.span())?
        .read(ReadPosition::Value, span, env)
}

/// Read an optional field `base?.name`. The read is the same as a plain field
/// read — saved off a `^root` chain, or local off a resource value — so the leaf
/// type is unchanged. An absent base or field surfaces as `run.absent_element`,
/// which short-circuits an enclosing `?.` chain and is caught by a `??` default;
/// outermost and unguarded, it surfaces like any absent read.
pub(crate) fn eval_optional_field(
    expr: &Expression,
    base: &Expression,
    name: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Expression::OptionalField { quoted, .. } = expr else {
        return Err(unsupported("this read", span));
    };
    if is_saved_path(base) {
        read_saved_field(base, name, *quoted, span, env)
    } else {
        eval_local_field_get(base, name, span, env)
    }
}

/// Read a resource identity from a declared index lookup `^root.index(args…)`.
/// A unique index stores the owning identity at the lookup path, so reading it
/// decodes back to a [`Value::Identity`]. A non-unique index has no single
/// identity to yield in value position; iterate it with `keys(...)` instead.
pub(crate) fn eval_index_lookup(
    resource: &ResourceSchema,
    index: &IndexSchema,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    if !index.unique {
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_UNSUPPORTED,
            message: format!(
                "non-unique index `{}` has no single identity in value position; \
                 iterate it with `keys(...)`",
                index.name
            ),
            span,
        });
    }
    // A unique index points to one resource, so `decode_identity` needs the
    // resource's saved root to know the identity arity.
    let root = resource
        .saved_root
        .as_ref()
        .ok_or_else(|| unsupported("an index on a resource with no saved root", span))?;
    let mut segments = vec![
        PathSegment::Root(root.root.clone()),
        PathSegment::Index(index.name.clone()),
    ];
    for arg in args {
        if arg.mode.is_some() || arg.name.is_some() {
            return Err(unsupported(
                "an index lookup with named or out arguments",
                span,
            ));
        }
        segments.push(PathSegment::IndexKey(
            value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| unsupported("an index key of this type", span))?,
        ));
    }
    let bytes = env
        .store
        .borrow()
        .read(&encode_path(&segments))
        .map_err(|error| error.located(span))?;
    let Some(bytes) = bytes else {
        return Err(absent_read(
            ReadPosition::Value,
            format!("`{}` has no entry for that key", index.name),
            span,
        ));
    };
    decode_identity(&bytes, root)
        .map(Value::Identity)
        .ok_or_else(|| RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: format!(
                "the `{}` index entry did not decode to an identity",
                index.name
            ),
            span,
        })
}

/// Read a keyed-layer entry off a saved record. A leaf layer
/// (`^books(id).tags(pos)`) reads its single value; a group layer
/// (`^books(id).versions(v)`) materializes the whole entry. The `callee` is the
/// layer field `^books(id).<layer>` and `keys` are the layer key arguments.
pub(crate) fn eval_saved_layer_read(
    callee: &Expression,
    keys: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Expression::Field {
        base, name: layer, ..
    } = callee
    else {
        return Err(unsupported("this read", span));
    };
    let (root, identity) = lower(base, env)?.into_record(base.span())?;
    let expected = layer_key_params(env.program, &root, &[layer]);
    let layer_keys = lower_keys(keys, span, false, expected, env)?;
    read_layer_entry(
        &root,
        &identity,
        layer,
        &layer_keys,
        ReadPosition::Value,
        span,
        env,
    )
}

/// Read one keyed-layer entry from a lowered record identity and layer keys. A
/// leaf layer reads its single decoded value; a group layer materializes its
/// entry as a [`Value::Resource`]. Shared by [`eval_saved_layer_read`] and the
/// `values`/`entries` materializer.
pub(crate) fn read_layer_entry(
    root: &str,
    identity: &[SavedKey],
    layer: &str,
    layer_keys: &[SavedKey],
    position: ReadPosition,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let entry = saved_segments(
        root,
        identity,
        &[(layer.to_string(), layer_keys.to_vec())],
        None,
    );

    // A leaf layer reads one value; a group layer materializes its entry.
    let Some(leaf) = resource_layer_leaf(env.program, root, layer) else {
        return read_group_entry(root, layer, &entry, span, env);
    };
    let bytes = env
        .store
        .borrow()
        .read(&encode_path(&entry))
        .map_err(|error| error.located(span))?;
    let Some(bytes) = bytes else {
        return Err(absent_read(
            position,
            format!("`{layer}` entry is absent"),
            span,
        ));
    };
    decode_leaf(&bytes, &leaf).ok_or_else(|| RuntimeError {
        throw: None,
        origin: None,
        code: RUN_TYPE,
        message: format!("stored value in `{layer}` did not decode to a runtime value"),
        span,
    })
}

/// Materialize a keyed GROUP entry `^root(key…).layer(key…)` (its path already
/// lowered into `entry`) as a [`Value::Resource`]: each present member field, in
/// declaration order, decoded by its type; sparse members are omitted. Mirrors a
/// whole-resource read scoped to one group entry.
pub(crate) fn read_group_entry(
    root: &str,
    layer: &str,
    entry: &[PathSegment],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let resource = find_resource(env.program, root)
        .ok_or_else(|| unsupported("reading this saved root", span))?;
    let declared = resource
        .descend_layers(&[layer])
        .filter(|node| matches!(node.element, Element::Group))
        .ok_or_else(|| unsupported("reading this layer", span))?;
    let store = env.store.borrow();
    let fields =
        materialize_resource_members(env.program, &declared.members, entry, &*store, span)?;
    Ok(Value::Resource(fields))
}

/// Read a whole resource `^root(key…)` into a materialized [`Value::Resource`]:
/// each present plain field in schema order, with unkeyed groups represented as
/// nested resources. Absent sparse fields and empty groups are omitted.
pub(crate) fn eval_resource_read(
    callee: &Expression,
    args: &[Argument],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Expression::SavedRoot { name: root, .. } = callee else {
        return Err(unsupported("this read", span));
    };
    let expected = root_identity_keys(env.program, root);
    let identity = lower_keys(args, span, true, expected, env)?;
    read_resource(root, &identity, span, env)
}

/// Read a whole resource from a pre-lowered identity into a materialized
/// [`Value::Resource`]: direct plain fields and unkeyed-group descendants in
/// schema order, with sparse fields and empty groups omitted. Shared by
/// [`eval_resource_read`] and `out`/`inout` place reads.
pub(crate) fn read_resource(
    root: &str,
    identity: &[SavedKey],
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Value, RuntimeError> {
    let resource = find_resource(env.program, root)
        .ok_or_else(|| unsupported("reading this saved root", span))?;
    let arity = resource
        .saved_root
        .as_ref()
        .map_or(0, |saved| saved.identity_keys.len());
    if identity.len() != arity {
        // A whole-resource read needs the root's full identity: a keyed root such
        // as `^books` is a collection of records, not a readable value on its own.
        return Err(type_error(
            &format!(
                "`^{root}` expects {arity} identity key(s), got {}",
                identity.len()
            ),
            span,
        ));
    }
    let prefix = saved_segments(root, identity, &[], None);

    let store = env.store.borrow();
    let fields =
        materialize_resource_members(env.program, &resource.members, &prefix, &*store, span)?;
    Ok(Value::Resource(fields))
}

fn materialize_resource_members(
    program: &CheckedProgram,
    members: &[Node],
    prefix: &[PathSegment],
    store: &dyn Backend,
    span: SourceSpan,
) -> Result<Vec<(String, Value)>, RuntimeError> {
    let mut fields = Vec::new();
    for node in members {
        if let Some(ty) = node.plain_field_type() {
            let mut segments = prefix.to_vec();
            segments.push(PathSegment::Field(node.name.clone()));
            let Some(bytes) = store
                .read(&encode_path(&segments))
                .map_err(|error| error.located(span))?
            else {
                continue;
            };
            let leaf = leaf_kind(program, ty)
                .ok_or_else(|| unsupported("reading this field type", span))?;
            let value = decode_leaf(&bytes, &leaf).ok_or_else(|| RuntimeError {
                throw: None,
                origin: None,
                code: RUN_TYPE,
                message: format!("stored value for `{}` did not decode", node.name),
                span,
            })?;
            fields.push((node.name.clone(), value));
        } else if node.key_params.is_empty() && matches!(node.element, Element::Group) {
            let mut group_prefix = prefix.to_vec();
            group_prefix.push(PathSegment::ChildLayer(node.name.clone()));
            let nested =
                materialize_resource_members(program, &node.members, &group_prefix, store, span)?;
            if !nested.is_empty() {
                fields.push((node.name.clone(), Value::Resource(nested)));
            }
        }
    }
    Ok(fields)
}

/// Read a quoted/raw segment `^root(key…)."segment"` under maintenance: the bytes
/// at the literal path are returned as a string (raw segments hold untyped data
/// the schema does not model, so they decode as their stored text). Off
/// maintenance, [`raw_segment_path`] rejects it; an absent segment is a catchable
/// absent-element read.
pub(crate) fn eval_raw_field_read(
    base: &Expression,
    segment: &str,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let (path, ..) = raw_segment_path(base, segment, span, env)?;
    let bytes = {
        let store = env.store.borrow();
        store
            .read(&encode_path(&path))
            .map_err(|error| error.located(span))?
    };
    let Some(bytes) = bytes else {
        return Err(absent_read(
            ReadPosition::Value,
            format!("`\"{segment}\"` is absent"),
            span,
        ));
    };
    decode_value(&bytes, ScalarType::Str)
        .map(saved_value_to_value)
        .ok_or_else(|| RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: format!("raw segment `\"{segment}\"` did not decode as text"),
            span,
        })
}

/// Read a field of a local resource value, e.g. `book.shelf`. An unpopulated
/// field is an absent-element error.
pub(crate) fn eval_local_field_get(
    base: &Expression,
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
