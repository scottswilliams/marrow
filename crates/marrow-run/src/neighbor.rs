use marrow_check::{CheckedArg as ExecArg, CheckedExpr as ExecExpr, CheckedSavedPlace};
use marrow_store::key::SavedKey;
use marrow_store::value::ScalarType;
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::env::Env;
use crate::error::{RUN_ABSENT, RUN_TYPE, RuntimeError, raise_fault, unsupported};
use crate::path::{Terminal, direct_root_place, lower_for_probe};
use crate::read::{
    first_data_child, first_record_child, next_data_child, next_record_child,
    validate_scanned_child_key,
};
use crate::store::{DataAddress, catalog_id};
use crate::value::{Value, identity_value, saved_key_to_value, validate_place_identity_keys};

pub(crate) fn eval_neighbor(
    dir: Direction,
    args: &[ExecArg],
    span: SourceSpan,
    env: &mut Env<'_>,
) -> Result<Value, RuntimeError> {
    let [arg] = args else {
        let which = if dir == Direction::Ascending {
            "next"
        } else {
            "prev"
        };
        return Err(RuntimeError::fault(
            RUN_TYPE,
            format!("`{which}` takes one argument"),
            span,
        ));
    };
    // A start position that addresses no node — non-positive or otherwise
    // unlowerable — has no neighbor, the same maybe-present absence a positive
    // out-of-range start resolves to, recoverable through `??`.
    let neighbor = match neighbor_target(&arg.value, env)? {
        Some(target) => seek_neighbor(&target, dir, span, env)?.map(|key| (target, key)),
        None => None,
    };
    match neighbor {
        Some((target, key)) => match &target {
            NeighborTarget::Record { place, .. } => {
                validate_place_identity_keys(place, std::slice::from_ref(&key), span)?;
                Ok(identity_value(&place.root, vec![key]))
            }
            NeighborTarget::Data { expected_key, .. } => {
                validate_scanned_child_key(std::slice::from_ref(expected_key), 0, &key, span)?;
                saved_key_to_value(key, span)
            }
        },
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

fn neighbor_target(
    expr: &ExecExpr,
    env: &mut Env<'_>,
) -> Result<Option<NeighborTarget>, RuntimeError> {
    let span = expr.span();
    if let Some(place) = direct_root_place(expr).filter(|place| !place.identity_keys.is_empty()) {
        return Ok(Some(NeighborTarget::Record {
            place: Box::new(place.clone()),
            anchor: None,
        }));
    }
    let Some(path) = lower_for_probe(expr, env)? else {
        return Ok(None);
    };
    if !matches!(path.terminal, Terminal::Record) {
        return Err(unsupported("`next`/`prev` of this path", span));
    }
    if path.layers.is_empty() {
        return match path.identity.as_slice() {
            [key] => Ok(Some(NeighborTarget::Record {
                place: Box::new(path.place),
                anchor: Some(key.clone()),
            })),
            _ => Err(unsupported(
                "`next`/`prev` of a composite-identity record (scope a single key level)",
                span,
            )),
        };
    }
    let Some((_, last_keys)) = path.layers.last() else {
        return Err(unsupported("`next`/`prev` of this path", span));
    };
    let last_keys = last_keys.clone();
    let key_params = path
        .place
        .layers
        .last()
        .map(|layer| layer.key_params.as_slice())
        .unwrap_or_default();
    // A composite layer is a chain of single-key sub-layers, so the column a
    // neighbor seek navigates is the first one the supplied prefix leaves unfilled.
    // A partial prefix (`cells(row)`) descends to that inner column and seeks its
    // edge entry; a fully-keyed leaf (`cells(row, col)`) is a position within the
    // final column, so the last supplied key anchors a sibling seek under the prefix
    // of the columns before it. Either way the seek scans exactly the sub-layer
    // `count` and iteration descend into on the same path.
    let (prefix, anchor, column) = if last_keys.len() < key_params.len() {
        (last_keys.as_slice(), None, last_keys.len())
    } else {
        let Some((last, rest)) = last_keys.split_last() else {
            return Err(unsupported("`next`/`prev` of this path", span));
        };
        (rest, Some(last.clone()), last_keys.len().saturating_sub(1))
    };
    let expected_key = key_params.get(column).and_then(|param| param.scalar);
    let mut parent_layers = path.layer_addresses;
    if let Some(last) = parent_layers.last_mut() {
        last.keys = prefix.to_vec();
    }
    let parent = DataAddress::layer_prefix(&path.place, &path.identity, &parent_layers, span)?;
    Ok(Some(NeighborTarget::Data {
        parent,
        anchor,
        expected_key,
    }))
}

enum NeighborTarget {
    Record {
        place: Box<CheckedSavedPlace>,
        anchor: Option<SavedKey>,
    },
    Data {
        parent: DataAddress,
        anchor: Option<SavedKey>,
        expected_key: Option<ScalarType>,
    },
}

fn seek_neighbor(
    target: &NeighborTarget,
    dir: Direction,
    span: SourceSpan,
    env: &Env<'_>,
) -> Result<Option<SavedKey>, RuntimeError> {
    match target {
        NeighborTarget::Record { place, anchor } => {
            let store = catalog_id(&place.store_catalog_id, "store", span)?;
            match anchor {
                None => {
                    first_record_child(env.store, &store, &[], dir, place.identity_keys.len(), span)
                }
                Some(key) => next_record_child(
                    env.store,
                    &store,
                    &[],
                    key,
                    dir,
                    place.identity_keys.len(),
                    span,
                ),
            }
        }
        NeighborTarget::Data { parent, anchor, .. } => match anchor {
            None => first_data_child(env.store, parent, dir, span),
            Some(key) => next_data_child(env.store, parent, key, dir, span),
        },
    }
}
