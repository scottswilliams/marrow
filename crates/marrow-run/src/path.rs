//! Saved-path lowering near the managed-write layer.

use crate::*;

/// A saved path lowered from its source expression: the saved root, the record
/// identity keys, the chain of group/keyed-layer levels from outermost to
/// innermost, and how the path terminates. One [`lower`] pass walks the call/field
/// spine once and produces this; every saved record write, delete, merge, layer
/// read, and traversal then consumes these fields directly.
///
/// Callers always peel the trailing scalar field off the spine before lowering its
/// base, so `lower` is given a record, layer, or index path — never a trailing
/// `.field`. Each `.name` off a saved path it walks is therefore a group/layer hop,
/// and the only non-record terminal it produces is an index branch.
pub(crate) struct SavedPath {
    /// The `^root` name.
    pub(crate) root: String,
    /// The record identity keys (empty for a keyless singleton).
    pub(crate) identity: Vec<SavedKey>,
    /// The `(layer, key…)` levels, outermost first; keys are empty for an unkeyed
    /// group hop (`^root(id).name`) and present for a keyed layer (`.layer(key…)`).
    pub(crate) layers: Vec<(String, Vec<SavedKey>)>,
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
    Field(String),
    /// A declared index branch `^root.index(args…)`. It hangs directly off the root
    /// with no record identity or layer chain.
    Index,
}

impl SavedPath {
    /// The saved root and identity of a path that must be a plain record address —
    /// no layer chain and no index branch. Callers that only accept `^root` or
    /// `^root(id…)` (a record write, delete, merge, or layer base) use this to keep
    /// their "this saved path isn't supported here" rejection.
    pub(crate) fn into_record(
        self,
        span: SourceSpan,
    ) -> Result<(String, Vec<SavedKey>), RuntimeError> {
        if self.layers.is_empty() && matches!(self.terminal, Terminal::Record) {
            Ok((self.root, self.identity))
        } else {
            Err(unsupported("this saved path", span))
        }
    }

    /// The saved root, identity, and layer chain of a path that is not an index
    /// branch. Callers that peel the terminal field/layer off the spine before
    /// lowering the base (group-entry field reads/writes, layer-entry deletes) use
    /// this; an unexpected index terminal is rejected as an unsupported path.
    pub(crate) fn into_layers(self, span: SourceSpan) -> Result<LayerPath, RuntimeError> {
        if matches!(self.terminal, Terminal::Record) {
            Ok((self.root, self.identity, self.layers))
        } else {
            Err(unsupported("this saved path", span))
        }
    }

    /// Re-terminate a record-base path at the named scalar `field`, so a lowered
    /// `^root(id…)` or `^root(id…).layer(key…)` base gains a trailing
    /// [`Terminal::Field`]. Used when a place resolution peels the trailing `.field`
    /// off the spine; an index branch has no field below it and is rejected.
    pub(crate) fn into_field(
        mut self,
        field: String,
        span: SourceSpan,
    ) -> Result<Self, RuntimeError> {
        if matches!(self.terminal, Terminal::Record) {
            self.terminal = Terminal::Field(field);
            Ok(self)
        } else {
            Err(unsupported("this saved path", span))
        }
    }

    /// The encoded path segments this lowered path addresses: the root, identity
    /// record keys, each `(layer, key…)` level, and the trailing scalar field for a
    /// [`Terminal::Field`]. The single [`PathSegment`]-vec builder for the
    /// record/layer/field shape. An index branch has no such segment form, so this
    /// panics on [`Terminal::Index`] — callers route index paths separately.
    pub(crate) fn to_segments(&self) -> Vec<PathSegment> {
        let field = match &self.terminal {
            Terminal::Record => None,
            Terminal::Field(name) => Some(name.as_str()),
            Terminal::Index => panic!("an index branch has no record/layer segment form"),
        };
        saved_segments(&self.root, &self.identity, &self.layers, field)
    }

    /// The current value at this lowered path. A `Terminal::Field` reads that
    /// scalar field (top-level or inside the group chain), decoding it with the
    /// field's declared type; a `Terminal::Record` reads the whole record. An
    /// unpopulated element raises an absent-element fault, catchable or fatal per
    /// `position`. Shared by value-position field reads and `out`/`inout` seeds.
    pub(crate) fn read(
        &self,
        position: ReadPosition,
        span: SourceSpan,
        env: &mut Env<'_>,
    ) -> Result<Value, RuntimeError> {
        let Terminal::Field(field) = &self.terminal else {
            return match self.terminal {
                Terminal::Record => read_resource(&self.root, &self.identity, span, env),
                Terminal::Index => Err(unsupported("reading this saved path", span)),
                Terminal::Field(_) => unreachable!("guarded by the let-else"),
            };
        };
        let leaf = self.leaf_kind(env.program, field).ok_or_else(|| {
            // A top-level field rejects as "reading this field"; a group-entry field
            // as "reading this group field", keeping each read's message.
            let what = if self.layers.is_empty() {
                "reading this field"
            } else {
                "reading this group field"
            };
            unsupported(what, span)
        })?;
        let bytes = env
            .store
            .borrow()
            .read(&encode_path(&self.to_segments()))
            .map_err(|error| error.located(span))?;
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
        decode_leaf(&bytes, &leaf).ok_or_else(|| {
            RuntimeError::fault(
                RUN_TYPE,
                format!("stored value for `{field}` did not decode to a runtime value"),
                span,
            )
        })
    }

    /// Write `value` to this lowered path, routing a scalar field or whole-record
    /// write the same way a direct assignment to the path would. Shared by direct
    /// saved writes and `out`/`inout` write-back.
    pub(crate) fn write(
        self,
        value: Value,
        span: SourceSpan,
        env: &mut Env<'_>,
    ) -> Result<(), RuntimeError> {
        match self.terminal {
            Terminal::Field(field) if self.layers.is_empty() => {
                write_saved_field(&self.root, &self.identity, &field, value, span, env)
            }
            Terminal::Field(field) => write_nested_field(
                &self.root,
                &self.identity,
                &self.layers,
                &field,
                value,
                span,
                env,
            ),
            Terminal::Record => write_resource(&self.root, &self.identity, value, span, env),
            Terminal::Index => Err(unsupported("writing this saved path", span)),
        }
    }

    /// The stored leaf kind of this path's [`Terminal::Field`]: a top-level field
    /// when the layer chain is empty, otherwise a nested group member. A scalar or
    /// enum field is a scalar leaf; an identity-typed field is a typed reference.
    fn leaf_kind(&self, program: &CheckedProgram, field: &str) -> Option<LeafKind> {
        if self.layers.is_empty() {
            resource_field_leaf(program, &self.root, field)
        } else {
            let layer_names: Vec<&str> = self.layers.iter().map(|(n, _)| n.as_str()).collect();
            resource_nested_member_leaf(program, &self.root, &layer_names, field)
        }
    }
}

/// Build the encoded path segments for a saved member: the root, the identity
/// record keys, each `(layer, key…)` level (outermost first), and an optional
/// trailing scalar field. The one builder every record/layer/field read, write,
/// and delete shares, so a path encodes byte-identically wherever it is built.
pub(crate) fn saved_segments(
    root: &str,
    identity: &[SavedKey],
    layers: &[(String, Vec<SavedKey>)],
    field: Option<&str>,
) -> Vec<PathSegment> {
    let mut segments = vec![PathSegment::Root(root.to_string())];
    segments.extend(identity.iter().cloned().map(PathSegment::RecordKey));
    for (name, keys) in layers {
        segments.push(PathSegment::ChildLayer(name.clone()));
        segments.extend(keys.iter().cloned().map(PathSegment::IndexKey));
    }
    if let Some(field) = field {
        segments.push(PathSegment::Field(field.to_string()));
    }
    segments
}

/// A lowered keyed group-entry path: the saved root name, the record identity
/// keys, and the chain of `(layer, key…)` levels from outermost to innermost.
pub(crate) type LayerPath = (String, Vec<SavedKey>, Vec<(String, Vec<SavedKey>)>);

/// Lower a saved-path expression to a [`SavedPath`] by walking the call/field
/// spine once. A bare `^root` or keyed lookup `^root(key…)` is the record base; a
/// `^root.index(args…)` is an index-branch terminal; `….layer(key…)` and the
/// unkeyed group hop `….name` each append one layer level.
pub(crate) fn lower(expr: &Expression, env: &mut Env<'_>) -> Result<SavedPath, RuntimeError> {
    // An index branch `^root.index(args…)` hangs directly off the root: a call
    // whose callee names a declared index off a saved root. It carries no record
    // identity or layer chain, so classify it before the layer arm.
    if let Expression::Call { callee, .. } = expr
        && let Expression::Field { base, name, .. } = callee.as_ref()
        && let Expression::SavedRoot { name: root, .. } = base.as_ref()
        && find_resource(env.program, root)
            .is_some_and(|resource| resource.indexes.iter().any(|index| &index.name == name))
    {
        return Ok(SavedPath {
            root: root.clone(),
            identity: Vec::new(),
            layers: Vec::new(),
            terminal: Terminal::Index,
        });
    }
    // A keyed layer hop `….layer(key…)`: a call whose callee is a `.layer` access.
    if let Expression::Call { callee, args, span } = expr
        && let Expression::Field { base, name, .. } = callee.as_ref()
    {
        let mut path = lower(base, env)?;
        // The chain to this layer is the layers lowered so far plus its own name,
        // which resolves the key parameters the new keys are guarded against.
        let chain: Vec<&str> = path
            .layers
            .iter()
            .map(|(layer, _)| layer.as_str())
            .chain(std::iter::once(name.as_str()))
            .collect();
        let expected = layer_key_params(env.program, &path.root, &chain);
        let keys = lower_keys(args, *span, false, expected, env)?;
        path.layers.push((name.clone(), keys));
        return Ok(path);
    }
    // An unkeyed group hop `….name` (a `.field`/`?.field` off a saved path, not a
    // call) appends a zero-key layer level, so `^patients(id).name` descends into
    // the group `name`. An optional hop lowers the same; its short-circuit is a
    // read-time concern, not a path-shape one. The record base is handled by the
    // terminal arms below.
    if let Expression::Field { base, name, .. } | Expression::OptionalField { base, name, .. } =
        expr
        && is_saved_path(base)
    {
        let mut path = lower(base, env)?;
        path.layers.push((name.clone(), Vec::new()));
        return Ok(path);
    }
    // The record base: a bare singleton `^root` or a keyed lookup `^root(key…)`.
    // A bare saved root is a whole-resource address only for a keyless singleton
    // (`Settings at ^settings`). For a keyed root such as `^books`, addressing it
    // without an identity is a type error, not a silent read of the keyless path.
    if let Expression::SavedRoot { name, span } = expr {
        return match root_identity_arity(env.program, name) {
            Some(0) => Ok(record_path(name.clone(), Vec::new())),
            Some(arity) => Err(type_error(
                &format!(
                    "`^{name}` expects {arity} identity key(s), got 0; address a record with `^{name}(id)`"
                ),
                *span,
            )),
            None => Err(unsupported("this saved path", *span)),
        };
    }
    let Expression::Call { callee, args, span } = expr else {
        return Err(unsupported("this saved path", expr.span()));
    };
    let Expression::SavedRoot { name, .. } = callee.as_ref() else {
        return Err(unsupported("this saved path", *span));
    };
    let expected = root_identity_keys(env.program, name);
    Ok(record_path(
        name.clone(),
        lower_keys(args, *span, true, expected, env)?,
    ))
}

/// A record-base [`SavedPath`]: a root and identity with no layers and a record
/// terminal.
pub(crate) fn record_path(root: String, identity: Vec<SavedKey>) -> SavedPath {
    SavedPath {
        root,
        identity,
        layers: Vec::new(),
        terminal: Terminal::Record,
    }
}

/// The encoded segments of a presence/count target — the path that `exists`,
/// `count`, and `std::assert::absent` probe. Every shape goes through the one
/// canonical [`lower`], so the segments are byte-identical to those a read or
/// write to the same path builds (a keyed-leaf or group entry uses
/// `ChildLayer`/`IndexKey`, not record keys). The bare primary root `^books` is
/// the exception: `lower` rejects it as a value address (it has no identity), but
/// as a presence/count target it is the root node itself, whose unambiguous,
/// argument-free segment form is a single [`PathSegment::Root`]. A declared index
/// branch has no record/layer segment form, so it is routed through
/// [`enumerate_layer`] by the caller, never here.
pub(crate) fn node_segments(
    expr: &Expression,
    env: &mut Env<'_>,
) -> Result<Vec<PathSegment>, RuntimeError> {
    // A bare keyed primary root addresses the root node, whose children are its
    // records — `count(^books)`/`exists(^books)` operate on that node directly.
    if let Expression::SavedRoot { name, .. } = expr
        && root_identity_arity(env.program, name).is_some_and(|arity| arity > 0)
    {
        return Ok(vec![PathSegment::Root(name.clone())]);
    }
    Ok(lower(expr, env)?.to_segments())
}

/// Whether a saved path holds a value or any children — the presence test behind
/// `exists` and `std::assert::absent`. A declared index branch has no
/// record/layer segment form, so — exactly as `count` does — its presence is
/// whether the branch enumerates any entries; every other shape probes the
/// canonical [`node_segments`].
pub(crate) fn saved_path_present(
    expr: &Expression,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<bool, RuntimeError> {
    if is_index_branch(expr, env) {
        return Ok(!enumerate_layer(expr, env)?.is_empty());
    }
    let segments = node_segments(expr, env)?;
    let store = env.store.borrow();
    let presence = store
        .presence(&encode_path(&segments))
        .map_err(|error| error.located(span))?;
    Ok(!matches!(presence, Presence::Absent))
}

/// Evaluate a keyed lookup's arguments to saved key segments, rejecting named/out
/// arguments. When `allow_identity_splice` (the record-identity position), a sole
/// identity-valued argument (`^root(id)` where `id: Resource::Id`) splices its
/// lowered keys in as the full key vector and an identity mixed with raw keys is
/// rejected; otherwise (a keyed layer or index lookup) each argument is one raw
/// key.
pub(crate) fn lower_keys(
    args: &[Argument],
    span: SourceSpan,
    allow_identity_splice: bool,
    expected: &[KeyDef],
    env: &mut Env<'_>,
) -> Result<Vec<SavedKey>, RuntimeError> {
    if args
        .iter()
        .any(|arg| arg.mode.is_some() || arg.name.is_some())
    {
        return Err(unsupported(
            "a keyed lookup with named or out arguments",
            span,
        ));
    }
    let mut keys = Vec::with_capacity(args.len());
    for (position, arg) in args.iter().enumerate() {
        match eval_expr(&arg.value, env)? {
            // An identity is the whole lookup key only as the sole argument of a
            // record lookup; it cannot be one component among raw keys. The
            // identity carries no resource name, so a foreign one laundered through
            // a dynamic value can reach here; its key scalars are guarded against
            // the declared key types just like raw keys, so a wrong-scalar splice
            // faults before any store write rather than corrupting the keyspace.
            Value::Identity(identity) if allow_identity_splice && args.len() == 1 => {
                check_spliced_identity(&identity, expected, span)?;
                return Ok(identity);
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

/// Guard a spliced identity's keys against the target keyspace, the same scalar
/// and arity check raw keys pass. A `Value::Identity` carries no resource name, so
/// a foreign identity laundered through a dynamic value can be spliced into the
/// wrong root; this catches the byte-incompatible cases (a different key count, or
/// a key whose scalar kind the declared key position does not allow) before any
/// store write. An unresolved schema passes no expectations and the guard skips,
/// matching the raw-key path.
pub(crate) fn check_spliced_identity(
    identity: &[SavedKey],
    expected: &[KeyDef],
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
/// typed-keyspace check every key path shares — a record/layer lookup, a spliced
/// identity, and an `Name::Id(...)` constructor all route their keys through it. A
/// non-scalar (defer) declaration passes no expectation, so the guard skips and
/// any arity fault still fires downstream. A wrong scalar is a `key_type_fault`.
pub(crate) fn guard_key_type(
    declared: &KeyDef,
    key: &SavedKey,
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    if let Some(expected) = declared.ty.scalar()
        && expected != key.scalar_type()
    {
        return Err(key_type_fault(expected, key.scalar_type(), span));
    }
    Ok(())
}

#[cfg(test)]
mod to_segments_tests {
    use super::*;

    fn book_path(terminal: Terminal) -> SavedPath {
        SavedPath {
            root: "books".into(),
            identity: vec![SavedKey::Int(7)],
            layers: vec![("versions".into(), vec![SavedKey::Int(2)])],
            terminal,
        }
    }

    // The single builder must reproduce the exact segment sequence the open-coded
    // builders emitted for each shape, so a path encodes byte-identically wherever
    // it is built. A nested field is `Root, RecordKey, ChildLayer, IndexKey, Field`.
    #[test]
    fn nested_field_terminal_segments_match_the_open_coded_form() {
        assert_eq!(
            book_path(Terminal::Field("text".into())).to_segments(),
            vec![
                PathSegment::Root("books".into()),
                PathSegment::RecordKey(SavedKey::Int(7)),
                PathSegment::ChildLayer("versions".into()),
                PathSegment::IndexKey(SavedKey::Int(2)),
                PathSegment::Field("text".into()),
            ]
        );
    }

    // A top-level field is `Root, RecordKey, Field` — no layer segments.
    #[test]
    fn top_level_field_terminal_drops_the_layer_chain() {
        let path = SavedPath {
            root: "books".into(),
            identity: vec![SavedKey::Int(7)],
            layers: Vec::new(),
            terminal: Terminal::Field("title".into()),
        };
        assert_eq!(
            path.to_segments(),
            vec![
                PathSegment::Root("books".into()),
                PathSegment::RecordKey(SavedKey::Int(7)),
                PathSegment::Field("title".into()),
            ]
        );
    }

    // A record terminal stops at the group entry with no trailing field segment.
    #[test]
    fn record_terminal_has_no_trailing_field() {
        assert_eq!(
            book_path(Terminal::Record).to_segments(),
            vec![
                PathSegment::Root("books".into()),
                PathSegment::RecordKey(SavedKey::Int(7)),
                PathSegment::ChildLayer("versions".into()),
                PathSegment::IndexKey(SavedKey::Int(2)),
            ]
        );
    }

    // An index branch has no record/layer segment form, so `to_segments` refuses it
    // rather than emitting a malformed path.
    #[test]
    #[should_panic(expected = "index branch has no record/layer segment form")]
    fn index_terminal_has_no_segment_form() {
        let _ = SavedPath {
            root: "books".into(),
            identity: Vec::new(),
            layers: Vec::new(),
            terminal: Terminal::Index,
        }
        .to_segments();
    }
}
