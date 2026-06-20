//! Saved-data reads and layer/index enumeration.

use std::ops::ControlFlow;

use marrow_check::{
    CheckedBuiltinCall, CheckedCallTarget, CheckedExpr as ExecExpr, CheckedSavedKeyParam,
    CheckedSavedPlace, CheckedSavedTerminal, StoredValueMeaning,
};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, IndexEntry, IndexRangeBounds, TreeStore};
use marrow_store::value::{ScalarType, scalar_key_matches_type, validate_scalar_key};
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::env::{Env, Flow};
use crate::error::{
    Located, RUN_ABSENT, RuntimeError, overflow, raise_fault, type_error, unsupported,
};
use crate::expr::eval_expr;
use crate::path::{direct_root_place, guard_key_type, lower, lower_keys};
use crate::range_expr::checked_range;
use crate::saved_iter::{
    ChildCursor, IndexCursor, KeyedChildrenWalk, RecordCursor, count_keyed_children,
    walk_keyed_children_after,
};
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

/// An ordered range supplied for the final scanned index component. A scalar
/// component sorts by its order-preserving key bytes, so the store scans a byte
/// range directly. Enum components are stored as content-independent member ids
/// whose bytes do not follow the declared order, so the range resolves to the
/// in-range members in declaration order and each is scanned as an exact key.
pub(crate) enum IndexRange {
    Scalar(IndexRangeBounds),
    EnumMembers(Vec<SavedKey>),
}

/// A non-unique index branch addressed for iteration, counting, or presence.
/// `arg_keys` are the exact leading components the caller pinned; `walk_depth` is
/// the number of levels still to walk below them and below an enum-range member;
/// `range` bounds the position after `arg_keys` when the caller supplied an
/// ordered range. Every visited entry is a full index tuple, and its store
/// identity is the suffix at `identity_start..`.
pub(crate) struct IndexBranchAddress {
    pub(crate) index: IndexAddress,
    pub(crate) arg_keys: Vec<SavedKey>,
    pub(crate) range: Option<IndexRange>,
    pub(crate) key_meanings: Vec<StoredValueMeaning>,
    pub(crate) identity_start: usize,
    pub(crate) walk_depth: usize,
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
        // A keyed-layer call iterates as a child layer when its final argument is a
        // range bound, or when it pins only a partial key prefix and descends into
        // the inner sub-layer of a composite layer.
        ExecExpr::Call { .. }
            if path.saved_place().is_some_and(|place| {
                layer_call_has_range(place) || layer_call_is_partial(place)
            }) =>
        {
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

/// Whether the innermost layer is addressed by a partial key prefix — fewer keys
/// than it declares — so iterating it descends into the inner sub-layer of a
/// composite layer.
fn layer_call_is_partial(place: &CheckedSavedPlace) -> bool {
    matches!(place.terminal, CheckedSavedTerminal::Record)
        && place
            .layers
            .last()
            .is_some_and(|layer| layer.args.len() < layer.key_params.len())
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
    for (position, arg) in args.iter().enumerate() {
        if arg.name.is_some() {
            return Err(unsupported("an index lookup with named arguments", span));
        }
        if let Some(bounds) =
            index_range_bounds(&arg.value, &index.keys[position].value_meaning, span, env)?
        {
            range = Some(bounds);
            break;
        }
        let key = value_to_index_key(
            eval_expr(&arg.value, env)?,
            &index.keys[position].value_meaning,
            span,
        )?;
        arg_keys.push(key);
    }
    // A bare or partial-prefix walk whose leading unpinned component is an enum
    // must stream that level in declared ordinal order, not the member-id byte
    // order a raw cursor would yield. Treating it as a full-member range routes it
    // through the same ordinal walk a ranged enum scan uses, so bare and ranged
    // agree.
    if range.is_none()
        && let Some(StoredValueMeaning::Enum { members, .. }) =
            index.keys.get(arg_keys.len()).map(|key| &key.value_meaning)
    {
        range = Some(IndexRange::EnumMembers(enum_member_keys(
            members, span, env,
        )?));
    }
    let identity_start = arg_count.saturating_sub(place.identity_keys.len());
    // Levels still to walk below the exact prefix. A scalar range pins its
    // position to a single byte span, so the store scans it without a walk; an
    // enum range consumes one extra level per member; otherwise the loop walks
    // from the exact prefix down to the leaf.
    let walk_depth = match &range {
        Some(IndexRange::Scalar(_)) => 0,
        Some(IndexRange::EnumMembers(_)) => arg_count.saturating_sub(arg_keys.len() + 1),
        None => arg_count.saturating_sub(arg_keys.len()),
    };
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
        walk_depth,
    }))
}

fn index_range_bounds(
    expr: &ExecExpr,
    meaning: &StoredValueMeaning,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<IndexRange>, RuntimeError> {
    let Some(range) = checked_range(expr) else {
        return Ok(None);
    };
    if let StoredValueMeaning::Enum { members, .. } = meaning {
        return enum_range_members(&range, members, span, env).map(Some);
    }
    let lower = match range.start {
        Some(start) => Some(value_to_index_key(eval_expr(start, env)?, meaning, span)?),
        None => None,
    };
    let upper = match range.end {
        Some(end) => Some(value_to_index_key(eval_expr(end, env)?, meaning, span)?),
        None => None,
    };
    Ok(Some(IndexRange::Scalar(IndexRangeBounds {
        lower,
        upper,
        upper_inclusive: range.inclusive_end,
    })))
}

/// Resolve an ordered enum range to the in-range members in declaration order.
/// The store keys enum components by content-independent member ids, which do
/// not sort by ordinal, so an open bound spans every declared member rather than
/// a key-byte tail. Each kept member becomes an exact lookup key.
fn enum_range_members(
    range: &crate::range_expr::CheckedRange<'_>,
    members: &[marrow_check::EnumMemberId],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<IndexRange, RuntimeError> {
    let ordinal = |expr: &ExecExpr, env: &mut Env<'_>| -> Result<usize, RuntimeError> {
        let Value::Enum(value) = eval_expr(expr, env)? else {
            return Err(type_error(
                "this index range bound takes an enum value",
                span,
            ));
        };
        members
            .iter()
            .position(|member| *member == value.member_id())
            .ok_or_else(|| type_error("this index range bound takes a different enum", span))
    };
    let start = match range.start {
        Some(start) => ordinal(start, env)?,
        None => 0,
    };
    let end = match range.end {
        Some(end) => {
            let position = ordinal(end, env)?;
            if range.inclusive_end {
                position + 1
            } else {
                position
            }
        }
        None => members.len(),
    };
    let in_range = members.get(start..end.min(members.len())).unwrap_or(&[]);
    Ok(IndexRange::EnumMembers(enum_member_keys(
        in_range, span, env,
    )?))
}

/// Resolve a declared enum-member list to the index keys an enum component holds,
/// in the given declaration order. The store keys an enum component by each
/// member's content-independent committed id, so iterating an enum level in
/// ordinal order means scanning these member keys in turn — the one owner shared
/// by a ranged enum scan and a bare enum-led walk.
fn enum_member_keys(
    members: &[marrow_check::EnumMemberId],
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Vec<SavedKey>, RuntimeError> {
    members
        .iter()
        .map(|member| {
            let catalog_id = env
                .program
                .facts()
                .enum_member(*member)
                .and_then(|member| member.catalog_id.clone())
                .ok_or_else(|| {
                    type_error("this enum index component has no committed member", span)
                })?;
            Ok(SavedKey::Str(catalog_id))
        })
        .collect()
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
    let key = value_to_key(eval_expr(expr, env)?, span)?
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
                    place.identity_keys.iter().map(|key| key.scalar).collect(),
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
            count_record_identities(place, &store, path.span(), env)
        }
        IterableLayer::Index(place, branch) => count_index_branch(place, &branch, path.span(), env),
        IterableLayer::ChildLayer => {
            let prefix = child_layer_prefix_address(path, env)?;
            validated_data_child_count(
                env.store,
                &prefix.address,
                &prefix.key_scalars,
                prefix.exact_key_count,
                path.span(),
            )
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
    place: &CheckedSavedPlace,
    store: &CatalogId,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<usize, RuntimeError> {
    let key_scalars: Vec<_> = place.identity_keys.iter().map(|key| key.scalar).collect();
    let depth = key_scalars.len();
    let cursor = RecordCursor::new(store, key_scalars, Direction::Ascending, span);
    count_keyed_children(&cursor, depth, &[], env, span, |_, _| Ok(1))
}

pub(crate) fn root_identity_present(
    place: &CheckedSavedPlace,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<bool, RuntimeError> {
    let store = crate::store::catalog_id(&place.store_catalog_id, "store", span)?;
    count_record_identities(place, &store, span, env).map(|count| count > 0)
}

fn count_index_branch(
    place: &CheckedSavedPlace,
    branch: &IndexBranchAddress,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<usize, RuntimeError> {
    let mut count = 0usize;
    stream_index_branch(
        place,
        branch,
        Direction::Ascending,
        span,
        env,
        &mut |_, _| {
            count = count.checked_add(1).ok_or_else(|| overflow(span))?;
            Ok(ControlFlow::Continue(()))
        },
    )?;
    Ok(count)
}

/// Stream the store identities a non-unique index branch yields, in `dir` order,
/// validating each entry. A scalar range scans an order-preserving key-byte span;
/// an enum range walks each in-range member in declaration order; a plain branch
/// walks every level below the exact prefix. The identity is the trailing
/// `identity_start..` suffix of the full index tuple, and a corrupt or stale
/// entry fails closed rather than yielding a wrong identity.
pub(crate) fn stream_index_branch(
    place: &CheckedSavedPlace,
    branch: &IndexBranchAddress,
    dir: Direction,
    span: SourceSpan,
    env: &mut Env<'_>,
    visit: &mut impl FnMut(Vec<SavedKey>, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
) -> Result<Flow, RuntimeError> {
    match &branch.range {
        Some(IndexRange::Scalar(range)) => {
            scan_scalar_range(place, branch, range, dir, span, env, visit)
        }
        Some(IndexRange::EnumMembers(members)) => {
            reject_unknown_enum_member_children(branch, span, env)?;
            for member in members_in_dir(members, dir) {
                let mut prefix = branch.arg_keys.clone();
                prefix.push(member);
                if let ControlFlow::Break(flow) =
                    walk_index_prefix(place, branch, &prefix, dir, span, env, visit)?
                {
                    return Ok(flow);
                }
            }
            Ok(Flow::Normal)
        }
        None => match walk_index_prefix(place, branch, &branch.arg_keys, dir, span, env, visit)? {
            ControlFlow::Continue(()) => Ok(Flow::Normal),
            ControlFlow::Break(flow) => Ok(flow),
        },
    }
}

fn members_in_dir(members: &[SavedKey], dir: Direction) -> Vec<SavedKey> {
    match dir {
        Direction::Ascending => members.to_vec(),
        Direction::Descending => members.iter().rev().cloned().collect(),
    }
}

/// Fail closed when the enum level holds a physical child that is not a declared
/// member of the type. Scanning the enum level by its declared members streams it
/// in ordinal order, but it would also silently skip a corrupt or stale member id
/// the type never declared; this guard rescans the physical children once so such
/// an entry raises a typed fault rather than vanishing from the result. It
/// validates against every declared member, not the in-range subset, so a valid
/// member outside a partial range is kept while a corrupt one still fails closed.
fn reject_unknown_enum_member_children(
    branch: &IndexBranchAddress,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<(), RuntimeError> {
    let level = branch.arg_keys.len();
    let Some(StoredValueMeaning::Enum { members, .. }) = branch.key_meanings.get(level) else {
        return Ok(());
    };
    let declared = enum_member_keys(members, span, env)?;
    let index = &branch.index.index;
    let prefix = &branch.arg_keys;
    let mut child = first_index_child(env.store, index, prefix, Direction::Ascending, span)?;
    while let Some(key) = child {
        if !declared.contains(&key) {
            return Err(type_error(
                "stored enum index component is not a declared member",
                span,
            ));
        }
        child = next_index_child(env.store, index, prefix, &key, Direction::Ascending, span)?;
    }
    Ok(())
}

/// Walk `branch.walk_depth` levels below `prefix`, yielding the store identity of
/// each leaf entry. `prefix` is the exact argument keys, with an enum-range
/// member already appended when present. A fully pinned tuple (`walk_depth == 0`)
/// is read as an exact index entry rather than walked.
fn walk_index_prefix(
    place: &CheckedSavedPlace,
    branch: &IndexBranchAddress,
    prefix: &[SavedKey],
    dir: Direction,
    span: SourceSpan,
    env: &mut Env<'_>,
    visit: &mut impl FnMut(Vec<SavedKey>, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
) -> Result<ControlFlow<Flow>, RuntimeError> {
    if branch.walk_depth == 0 {
        return scan_exact_index_tuple(place, branch, prefix, span, env, visit);
    }
    // Preserve the loop's `ControlFlow<Flow>` rather than collapsing it to a bare
    // `Flow`: when this walk is one in-range enum member among several, the member
    // loop in `stream_index_branch` must distinguish a body `break` (which stops
    // the whole walk) from the member's natural completion (which advances to the
    // next member).
    let cursor = IndexCursor::new(&branch.index.index, dir, span);
    walk_keyed_children_after(
        env,
        KeyedChildrenWalk {
            depth: branch.walk_depth,
            query_prefix: prefix,
            identity_prefix: prefix,
            after_identity: None,
        },
        &mut |env: &mut Env<'_>, prefix: &[SavedKey]| cursor.first(env, prefix),
        &mut |env: &mut Env<'_>, prefix: &[SavedKey], anchor: &SavedKey| {
            cursor.next(env, prefix, anchor)
        },
        &mut |tuple: Vec<SavedKey>, env: &mut Env<'_>| {
            let identity = walked_index_tuple_identity(place, branch, &tuple, span, env)?;
            visit(identity, env)
        },
    )
}

/// Read the exact index entry at a fully pinned tuple, yielding its store
/// identity. A non-unique index tuple is distinct per entry, so a corrupt sibling
/// under the same key fails closed in `validate_scanned_index_entry`.
fn scan_exact_index_tuple(
    place: &CheckedSavedPlace,
    branch: &IndexBranchAddress,
    tuple: &[SavedKey],
    span: SourceSpan,
    env: &mut Env<'_>,
    visit: &mut impl FnMut(Vec<SavedKey>, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
) -> Result<ControlFlow<Flow>, RuntimeError> {
    let mut page = env
        .store
        .scan_index_tuple(&branch.index.index, tuple, INDEX_SCAN_PAGE_LIMIT)
        .map_err(|error| error.located(span))?;
    loop {
        for entry in std::mem::take(&mut page.entries) {
            validate_scanned_index_entry(place, branch, &entry, span, env)?;
            if let ControlFlow::Break(flow) = visit(entry.identity, env)? {
                return Ok(ControlFlow::Break(flow));
            }
        }
        let Some(cursor) = page.cursor else {
            return Ok(ControlFlow::Continue(()));
        };
        page = env
            .store
            .scan_index_tuple_after(&branch.index.index, tuple, &cursor, INDEX_SCAN_PAGE_LIMIT)
            .map_err(|error| error.located(span))?;
    }
}

fn scan_scalar_range(
    place: &CheckedSavedPlace,
    branch: &IndexBranchAddress,
    range: &IndexRangeBounds,
    dir: Direction,
    span: SourceSpan,
    env: &mut Env<'_>,
    visit: &mut impl FnMut(Vec<SavedKey>, &mut Env<'_>) -> Result<ControlFlow<Flow>, RuntimeError>,
) -> Result<Flow, RuntimeError> {
    let index = &branch.index.index;
    let prefix = &branch.arg_keys;
    let mut page = match dir {
        Direction::Ascending => {
            env.store
                .scan_index_range(index, prefix, range, INDEX_SCAN_PAGE_LIMIT)
        }
        Direction::Descending => {
            env.store
                .scan_index_range_reverse(index, prefix, range, INDEX_SCAN_PAGE_LIMIT)
        }
    }
    .map_err(|error| error.located(span))?;
    loop {
        for entry in std::mem::take(&mut page.entries) {
            validate_scanned_index_entry(place, branch, &entry, span, env)?;
            if let ControlFlow::Break(flow) = visit(entry.identity, env)? {
                return Ok(flow);
            }
        }
        let Some(cursor) = page.cursor else {
            return Ok(Flow::Normal);
        };
        page = match dir {
            Direction::Ascending => env.store.scan_index_range_after(
                index,
                prefix,
                range,
                &cursor,
                INDEX_SCAN_PAGE_LIMIT,
            ),
            Direction::Descending => env.store.scan_index_range_before(
                index,
                prefix,
                range,
                &cursor,
                INDEX_SCAN_PAGE_LIMIT,
            ),
        }
        .map_err(|error| error.located(span))?;
    }
}

/// Validate a fully walked index tuple and return the store identity it points
/// to. Each component must decode under its declared meaning and the trailing
/// `identity_start..` suffix must be a valid store identity; the exact tuple is
/// rescanned so a corrupt sibling entry under the same key fails closed rather
/// than yielding a wrong identity.
pub(crate) fn walked_index_tuple_identity(
    place: &CheckedSavedPlace,
    branch: &IndexBranchAddress,
    tuple: &[SavedKey],
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Vec<SavedKey>, RuntimeError> {
    if tuple.len() != branch.key_meanings.len() {
        return Err(type_error(
            "stored index entry does not match the index key shape",
            span,
        ));
    }
    for (key, meaning) in tuple.iter().zip(&branch.key_meanings) {
        index_key_to_value(env.program, key, meaning, span)?;
    }
    let identity = tuple
        .get(branch.identity_start..)
        .map_or_else(Vec::new, <[SavedKey]>::to_vec);
    validate_place_identity_keys(place, &identity, span)?;
    validate_scanned_index_tuple_entries(place, branch, tuple, span, env)?;
    Ok(identity)
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

pub(crate) struct ChildLayerPrefixAddress {
    pub(crate) address: DataAddress,
    pub(crate) key_scalars: Vec<Option<ScalarType>>,
    pub(crate) exact_key_count: usize,
}

fn child_layer_prefix_address(
    path: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<ChildLayerPrefixAddress, RuntimeError> {
    let span = path.span();
    let (base, exact_args) = match path {
        ExecExpr::Field { base, .. } => (base.as_ref(), &[][..]),
        // A partial-key layer call pins a prefix and counts the inner sub-layer.
        ExecExpr::Call { callee, args, .. } => match callee.as_ref() {
            ExecExpr::Field { base, .. } => (base.as_ref(), args.as_slice()),
            _ => return Err(unsupported("iterating this saved path", span)),
        },
        _ => return Err(unsupported("iterating this saved path", span)),
    };
    let base_path = lower(base, env)?;
    let Some(place) = path.saved_place() else {
        return Err(unsupported("iterating this saved path", path.span()));
    };
    let Some(layer_facts) = place.layers.last() else {
        return Err(unsupported("iterating this saved path", path.span()));
    };
    let exact_prefix = lower_keys(exact_args, span, false, None, &layer_facts.key_params, env)?;
    let exact_key_count = exact_prefix.len();
    let mut layers = base_path.layer_addresses;
    layers.push(LayerAddress::from_checked(layer_facts, exact_prefix));
    let address = DataAddress::layer_prefix(place, &base_path.identity, &layers, span)?;
    Ok(ChildLayerPrefixAddress {
        address,
        key_scalars: layer_facts
            .key_params
            .iter()
            .map(|param| param.scalar)
            .collect(),
        exact_key_count,
    })
}

pub(crate) fn validated_data_layer_present(
    store: &TreeStore,
    address: &DataAddress,
    key_params: &[CheckedSavedKeyParam],
    exact_key_count: usize,
    span: SourceSpan,
) -> Result<bool, RuntimeError> {
    let key_scalars: Vec<_> = key_params.iter().map(|param| param.scalar).collect();
    let mut child = first_data_child(store, address, Direction::Ascending, span)?;
    let mut present = false;
    while let Some(key) = child {
        validate_scanned_child_key(&key_scalars, exact_key_count, &key, span)?;
        present = true;
        child = next_data_child(store, address, &key, Direction::Ascending, span)?;
    }
    if present {
        Ok(true)
    } else {
        crate::store::data_exists(store, address, span)
    }
}

pub(crate) fn validated_data_child_count(
    store: &TreeStore,
    address: &DataAddress,
    key_scalars: &[Option<ScalarType>],
    exact_key_count: usize,
    span: SourceSpan,
) -> Result<usize, RuntimeError> {
    let mut count = 0usize;
    let mut child = first_data_child(store, address, Direction::Ascending, span)?;
    while let Some(key) = child {
        validate_scanned_child_key(key_scalars, exact_key_count, &key, span)?;
        count = count.checked_add(1).ok_or_else(|| overflow(span))?;
        child = next_data_child(store, address, &key, Direction::Ascending, span)?;
    }
    Ok(count)
}

pub(crate) fn validate_scanned_child_key(
    key_scalars: &[Option<ScalarType>],
    position: usize,
    key: &SavedKey,
    span: SourceSpan,
) -> Result<(), RuntimeError> {
    validate_scalar_key(key).map_err(|error| error.located(span))?;
    if let Some(Some(expected)) = key_scalars.get(position)
        && !scalar_key_matches_type(key, *expected)
    {
        return Err(type_error(
            "stored layer keys do not match the layer key type",
            span,
        ));
    }
    Ok(())
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
    saved_key_to_value(key.clone(), span)
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
