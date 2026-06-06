use marrow_check::{CheckedExpr as ExecExpr, CheckedSavedTerminal};
use marrow_store::key::{SavedKey, decode_identity_payload_arity};
use marrow_syntax::SourceSpan;

use crate::env::Env;
use crate::error::{Located, RUN_TYPE, RuntimeError, unsupported};
use crate::expr::eval_expr;
use crate::store::IndexAddress;
use crate::value::{Value, identity_value, value_to_key};

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
    pub(crate) root: String,
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
                "an index lookup with named or inout arguments",
                place.span,
            ));
        }
        keys.push(
            value_to_key(eval_expr(&arg.value, env)?)
                .ok_or_else(|| unsupported("an index key of this type", place.span))?,
        );
    }
    Ok(Some(UniqueIndexLookup {
        address: IndexAddress::from_checked(catalog_id, keys, place.span)?,
        identity_arity: place.identity_keys.len(),
        index_name: index_name.clone(),
        root: place.root.clone(),
        remaining_key_depth: index_arg_count.saturating_sub(args.len()),
    }))
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
    read_unique_index_value(&lookup.address.keys, &lookup, span, env).map(|value| {
        Some(value.map_or(ExactUniqueIndexLookupValue::Absent, |_| {
            ExactUniqueIndexLookupValue::Present
        }))
    })
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
    let identity = decode_unique_index_identity(
        &entry.value,
        lookup.identity_arity,
        &lookup.index_name,
        span,
    )?;
    Ok(Some(identity_value(&lookup.root, identity)))
}

/// Decode a unique-index entry's stored value into the identity it points at, or the
/// single canonical store-corruption fault both unique-index read paths raise when the
/// bytes do not decode to an identity of the expected arity.
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
