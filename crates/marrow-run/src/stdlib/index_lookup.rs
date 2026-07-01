use marrow_check::{
    CheckedArg as ExecArg, CheckedExpr as ExecExpr, CheckedSavedPlace, CheckedSavedTerminal,
};
use marrow_store::key::{SavedKey, decode_identity_payload_arity};
use marrow_syntax::SourceSpan;

use crate::collection::absent_read;
use crate::env::Env;
use crate::error::{Located, RUN_ABSENT, RUN_TYPE, RuntimeError, type_error, unsupported};
use crate::expr::eval_expr;
use crate::store::{DataAddress, IndexAddress, data_exists};
use crate::value::{Value, identity_value, validate_place_identity_keys, value_to_index_key};

pub(crate) enum ExactUniqueIndexLookupValue {
    Absent,
    Present,
}

impl ExactUniqueIndexLookupValue {
    pub(crate) fn count(&self) -> i64 {
        match self {
            Self::Absent => 0,
            Self::Present => 1,
        }
    }

    pub(crate) fn is_present(&self) -> bool {
        matches!(self, Self::Present)
    }
}

pub(crate) fn check_key_collection(expr: &ExecExpr, span: SourceSpan) -> Result<(), RuntimeError> {
    if matches!(
        expr.saved_place().map(|place| &place.terminal),
        Some(CheckedSavedTerminal::Index { unique: true, .. })
    ) {
        return Err(unsupported("keys over a unique index lookup", span));
    }
    Ok(())
}

pub(crate) struct UniqueIndexLookup {
    pub(crate) address: IndexAddress,
    pub(crate) identity_arity: usize,
    pub(crate) index_name: String,
    pub(crate) place: CheckedSavedPlace,
    pub(crate) remaining_key_depth: usize,
}

pub(crate) fn unique_index_lookup(
    expr: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<Option<UniqueIndexLookup>, RuntimeError> {
    let Some(place) = expr.saved_place() else {
        return Ok(None);
    };
    unique_index_lookup_from_place(place, place.span, env)
}

fn unique_index_lookup_from_place(
    place: &CheckedSavedPlace,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<UniqueIndexLookup>, RuntimeError> {
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
    let keys = index_lookup_keys(args, place, span, env)?;
    Ok(Some(UniqueIndexLookup {
        address: IndexAddress::from_checked(catalog_id, keys, span)?,
        identity_arity: place.identity_keys.len(),
        index_name: index_name.clone(),
        place: place.clone(),
        remaining_key_depth: index_arg_count.saturating_sub(args.len()),
    }))
}

pub(crate) fn read_exact_unique_index_lookup_value(
    place: &CheckedSavedPlace,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let Some(lookup) = unique_index_lookup_from_place(place, span, env)? else {
        return Err(unsupported("a checked saved index lookup", span));
    };
    if lookup.remaining_key_depth > 0 {
        return Err(unsupported(
            "using an incomplete unique index lookup as a collection",
            span,
        ));
    }
    let index_name = lookup.index_name.clone();
    read_unique_index_identity(&lookup, span, env)?
        .map(|identity| identity_value(&lookup.place.root, identity))
        .ok_or_else(|| absent_read(format!("`{index_name}` has no entry for that key"), span))
}

pub(crate) fn exact_unique_index_lookup_value(
    expr: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<ExactUniqueIndexLookupValue>, RuntimeError> {
    let Some(lookup) = unique_index_lookup(expr, env)? else {
        return Ok(None);
    };
    if lookup.remaining_key_depth > 0 {
        return Err(unsupported(
            "using an incomplete unique index lookup as a collection",
            span,
        ));
    }
    read_unique_index_identity(&lookup, span, env).map(|identity| {
        Some(identity.map_or(ExactUniqueIndexLookupValue::Absent, |_| {
            ExactUniqueIndexLookupValue::Present
        }))
    })
}

pub(crate) fn read_exact_unique_index_lookup_if_present(
    expr: &ExecExpr,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Option<Value>, RuntimeError> {
    let Some(lookup) = unique_index_lookup(expr, env)? else {
        return Ok(None);
    };
    if lookup.remaining_key_depth > 0 {
        return Err(unsupported(
            "using an incomplete unique index lookup as a collection",
            span,
        ));
    }
    read_unique_index_identity(&lookup, span, env)
        .map(|identity| identity.map(|identity| identity_value(&lookup.place.root, identity)))
}

fn index_lookup_keys(
    args: &[ExecArg],
    place: &CheckedSavedPlace,
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Vec<SavedKey>, RuntimeError> {
    let CheckedSavedTerminal::Index { name, .. } = &place.terminal else {
        return Err(unsupported("this index lookup", span));
    };
    let index = place
        .indexes
        .iter()
        .find(|index| index.name == *name)
        .ok_or_else(|| unsupported("this index lookup", span))?;
    let mut keys = Vec::with_capacity(args.len());
    for (position, arg) in args.iter().enumerate() {
        if arg.name.is_some() {
            return Err(unsupported("an index lookup with named arguments", span));
        }
        keys.push(value_to_index_key(
            eval_expr(&arg.value, env)?,
            &index.keys[position].value_meaning,
            span,
        )?);
    }
    Ok(keys)
}

pub(crate) fn read_unique_index_identity(
    lookup: &UniqueIndexLookup,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Option<Vec<SavedKey>>, RuntimeError> {
    let page = env
        .store
        .scan_index_tuple(&lookup.address.index, &lookup.address.keys, 2)
        .map_err(|error| error.located(span))?;
    if page.truncated || page.entries.len() > 1 {
        return Err(type_error(
            "stored unique index has multiple entries for one tuple",
            span,
        ));
    }
    let Some(entry) = page.entries.first() else {
        return Ok(None);
    };
    let identity = decode_unique_index_identity(
        &entry.value,
        lookup.identity_arity,
        &lookup.index_name,
        span,
    )?;
    validate_place_identity_keys(&lookup.place, &identity, span)?;
    if entry.identity != identity {
        return Err(type_error(
            "stored unique index identity does not match the entry payload",
            span,
        ));
    }
    if entry.index_keys != lookup.address.keys {
        return Err(type_error(
            "stored unique index entry does not match the requested tuple",
            span,
        ));
    }
    // A present key with no backing record is a dangling index entry: an integrity
    // fault, not the empty optional a missing key resolves to. It stays fatal so a
    // `Id(^store)?` lookup never yields an identity whose record was deleted out
    // from under the index.
    let record = DataAddress::record(&lookup.place, &identity, span)?;
    if !data_exists(env.store, &record, span)? {
        return Err(RuntimeError::fault(
            RUN_ABSENT,
            format!(
                "the `{}` index has an entry for that key but its `^{}` record is gone",
                lookup.index_name, lookup.place.root
            ),
            span,
        ));
    }
    Ok(Some(identity))
}

/// Decode a unique-index entry value into the identity it points at, raising the canonical
/// store-corruption fault when the bytes do not decode to an identity of the expected arity.
pub(crate) fn decode_unique_index_identity(
    entry_value: &[u8],
    identity_arity: usize,
    index_name: &str,
    span: SourceSpan,
) -> Result<Vec<SavedKey>, RuntimeError> {
    decode_identity_payload_arity(entry_value, identity_arity).ok_or_else(|| {
        RuntimeError::fault(
            RUN_TYPE,
            format!("the `{index_name}` index entry did not decode to an identity"),
            span,
        )
    })
}
