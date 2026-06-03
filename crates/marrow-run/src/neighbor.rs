use marrow_check::{CheckedArg as ExecArg, CheckedExpr as ExecExpr, CheckedSavedPlace};
use marrow_store::key::SavedKey;
use marrow_syntax::SourceSpan;

use crate::collection::Direction;
use crate::env::Env;
use crate::error::{Located, RUN_ABSENT, RUN_TYPE, RuntimeError, raise_fault, unsupported};
use crate::path::{Terminal, direct_root_place, lower};
use crate::store::{DataAddress, catalog_id};
use crate::value::{Value, saved_key_to_value};

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
        return Err(RuntimeError {
            throw: None,
            origin: None,
            code: RUN_TYPE,
            message: format!("`{which}` takes one argument"),
            span,
        });
    };
    let target = neighbor_target(&arg.value, env)?;
    let neighbor = seek_neighbor(target, dir, span, env)?;
    match neighbor {
        Some(key) => {
            saved_key_to_value(key).ok_or_else(|| unsupported("a neighbor key of this type", span))
        }
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
    let last_keys = path
        .layers
        .last()
        .expect("non-empty checked above")
        .1
        .clone();
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
            match (anchor, dir) {
                (None, Direction::Ascending) => env
                    .store
                    .record_first_child(&store, &[])
                    .map_err(|error| error.located(span)),
                (None, Direction::Descending) => env
                    .store
                    .record_last_child(&store, &[])
                    .map_err(|error| error.located(span)),
                (Some(key), Direction::Ascending) => env
                    .store
                    .record_next_child(&store, &[], &key)
                    .map_err(|error| error.located(span)),
                (Some(key), Direction::Descending) => env
                    .store
                    .record_prev_child(&store, &[], &key)
                    .map_err(|error| error.located(span)),
            }
        }
        NeighborTarget::Data { parent, anchor } => match (anchor, dir) {
            (None, Direction::Ascending) => env
                .store
                .data_first_child(&parent.store, &parent.identity, &parent.path)
                .map_err(|error| error.located(span)),
            (None, Direction::Descending) => env
                .store
                .data_last_child(&parent.store, &parent.identity, &parent.path)
                .map_err(|error| error.located(span)),
            (Some(key), Direction::Ascending) => env
                .store
                .data_next_child(&parent.store, &parent.identity, &parent.path, &key)
                .map_err(|error| error.located(span)),
            (Some(key), Direction::Descending) => env
                .store
                .data_prev_child(&parent.store, &parent.identity, &parent.path, &key)
                .map_err(|error| error.located(span)),
        },
    }
}
