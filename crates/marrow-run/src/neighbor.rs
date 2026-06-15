use marrow_check::{CheckedArg as ExecArg, CheckedExpr as ExecExpr, CheckedSavedPlace};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::env::Env;
use crate::error::{RUN_ABSENT, RUN_TYPE, RuntimeError, raise_fault, unsupported};
use crate::path::{Terminal, direct_root_place, lower};
use crate::read::{first_data_child, first_record_child, next_data_child, next_record_child};
use crate::store::{DataAddress, catalog_id};
use crate::value::{Value, identity_value, saved_key_to_value};

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
    let target = neighbor_target(&arg.value, env)?;
    let identity_root = match &target {
        NeighborTarget::Record { place, .. } => Some(place.root.clone()),
        NeighborTarget::Data { .. } => None,
    };
    let neighbor = seek_neighbor(target, dir, span, env)?;
    match neighbor {
        Some(key) => match identity_root {
            Some(root) => Ok(identity_value(&root, vec![key])),
            None => Ok(saved_key_to_value(key)),
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

fn neighbor_target(expr: &ExecExpr, env: &mut Env<'_>) -> Result<NeighborTarget, RuntimeError> {
    let span = expr.span();
    if let Some(place) = direct_root_place(expr).filter(|place| !place.identity_keys.is_empty()) {
        return Ok(NeighborTarget::Record {
            place: Box::new(place.clone()),
            anchor: None,
        });
    }
    let path = lower(expr, env)?;
    if !matches!(path.terminal, Terminal::Record) {
        return Err(unsupported("`next`/`prev` of this path", span));
    }
    if path.layers.is_empty() {
        return match path.identity.as_slice() {
            [key] => Ok(NeighborTarget::Record {
                place: Box::new(path.place),
                anchor: Some(key.clone()),
            }),
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
    let mut parent_layers = path.layer_addresses;
    if let Some(last) = parent_layers.last_mut() {
        last.keys.clear();
    }
    let parent = DataAddress::layer_prefix(&path.place, &path.identity, &parent_layers, span)?;
    match last_keys.as_slice() {
        [] => Ok(NeighborTarget::Data {
            parent,
            anchor: None,
        }),
        [key] => Ok(NeighborTarget::Data {
            parent,
            anchor: Some(key.clone()),
        }),
        _ => Err(unsupported(
            "`next`/`prev` of a multi-key layer position (scope a single key level)",
            span,
        )),
    }
}

enum NeighborTarget {
    Record {
        place: Box<CheckedSavedPlace>,
        anchor: Option<SavedKey>,
    },
    Data {
        parent: DataAddress,
        anchor: Option<SavedKey>,
    },
}

fn seek_neighbor(
    target: NeighborTarget,
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
                    &key,
                    dir,
                    place.identity_keys.len(),
                    span,
                ),
            }
        }
        NeighborTarget::Data { parent, anchor } => match anchor {
            None => first_data_child(env.store, &parent, dir, span),
            Some(key) => next_data_child(env.store, &parent, &key, dir, span),
        },
    }
}
