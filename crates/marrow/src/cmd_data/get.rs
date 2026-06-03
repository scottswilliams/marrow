use std::process::ExitCode;

use marrow_check::{CheckedProgram, CheckedSavedMemberKind, checked_saved_root_place};
use marrow_store::StoreError;
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::path::{PathSegment, parse_path};
use marrow_store::tree::{DataPathSegment, TreeStore};
use serde_json::json;

use crate::{CheckFormat, load_checked_project, write_json};

use super::inspect::{checked_catalog_id, key_mismatch, push_key, render_value_bytes};

pub(super) fn data_get(args: &[String]) -> ExitCode {
    let (dir, path_text, format) = match super::data_get_args(args) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let parsed_segments = match parse_path(&path_text) {
        Ok(segments) => segments,
        Err(error) => {
            eprintln!("marrow data get: {}", error.message);
            return ExitCode::from(2);
        }
    };
    let segments = query_segments_from_path(&parsed_segments);
    let (config, program) = match load_checked_project(&dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    let query = match resolve_data_query(&program, &segments) {
        Ok(query) => query,
        Err(message) => {
            eprintln!("marrow data get: {message}");
            return ExitCode::from(2);
        }
    };
    let store = match super::open_tree_store(&dir, &config) {
        Ok(store) => store,
        Err(code) => return code,
    };
    let (value, presence) = match &store {
        Some(store) => match read_query(store, &query) {
            Ok(result) => result,
            Err(error) => return super::report_store_error(error, format),
        },
        None => (None, DataPresence::Absent),
    };
    match format {
        CheckFormat::Text => match &value {
            Some(bytes) => println!("{}", render_value_bytes(bytes)),
            None => match presence {
                DataPresence::ChildrenOnly => println!("(no value; has children)"),
                _ => println!("(absent)"),
            },
        },
        CheckFormat::Json | CheckFormat::Jsonl => {
            write_json(json!({
                "path": query.path,
                "presence": presence_name(presence),
                "value_b64": value.as_ref().map(|bytes| marrow_run::base64::encode(bytes)),
            }));
        }
    }
    ExitCode::SUCCESS
}

#[derive(Clone)]
pub(crate) struct DataQuery {
    pub(crate) path: String,
    pub(crate) root: String,
    pub(crate) store: CatalogId,
    pub(crate) identity: Vec<SavedKey>,
    pub(crate) identity_arity: usize,
    pub(crate) data_path: Vec<DataPathSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DataQuerySegment {
    Root(String),
    Member(String),
    Key(SavedKey),
}

pub(crate) fn query_segments_from_path(segments: &[PathSegment]) -> Vec<DataQuerySegment> {
    segments
        .iter()
        .map(|segment| match segment {
            PathSegment::Root(name) => DataQuerySegment::Root(name.clone()),
            PathSegment::Field(name) | PathSegment::ChildLayer(name) | PathSegment::Index(name) => {
                DataQuerySegment::Member(name.clone())
            }
            PathSegment::RecordKey(key) | PathSegment::IndexKey(key) => {
                DataQuerySegment::Key(key.clone())
            }
        })
        .collect()
}

#[derive(Clone, Copy)]
pub(crate) enum DataPresence {
    Absent,
    ValueOnly,
    ChildrenOnly,
}

pub(crate) fn resolve_data_query(
    program: &CheckedProgram,
    segments: &[DataQuerySegment],
) -> Result<DataQuery, String> {
    let Some((DataQuerySegment::Root(root), rest)) = segments.split_first() else {
        return Err("path must start with a saved root, such as `^books`".into());
    };
    let place = checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .ok_or_else(|| format!("unknown saved root `^{root}`"))?;
    let store =
        checked_catalog_id(&place.store_catalog_id, "store").map_err(|error| error.to_string())?;
    let mut identity = Vec::new();
    let mut index = 0usize;
    while let Some(segment) = rest.get(index) {
        let Some(key) = query_key(segment) else {
            break;
        };
        if identity.len() == place.identity_keys.len() {
            return Err(format!("`^{root}` has too many identity keys"));
        }
        if let Some(mismatch) = key_mismatch(place.identity_keys[identity.len()].scalar, key) {
            return Err(format!(
                "identity key is a {} where `^{root}` declares {}",
                mismatch.found.name(),
                mismatch.expected.name()
            ));
        }
        identity.push(key.clone());
        index += 1;
    }
    if index < rest.len() && identity.len() != place.identity_keys.len() {
        return Err(format!(
            "`^{root}` expects {} identity key(s) before member access",
            place.identity_keys.len()
        ));
    }

    let mut data_path = Vec::new();
    let mut members = place.root_members.as_slice();
    while let Some(segment) = rest.get(index) {
        let DataQuerySegment::Member(name) = segment else {
            return Err("a key must follow a saved root or a keyed member".into());
        };
        let member = members
            .iter()
            .find(|member| member.name == *name)
            .ok_or_else(|| format!("unknown saved member `{name}`"))?;
        data_path.push(DataPathSegment::Member(
            checked_catalog_id(&member.catalog_id, "resource member")
                .map_err(|error| error.to_string())?,
        ));
        index += 1;

        let mut key_count = 0usize;
        while let Some(key) = rest.get(index).and_then(query_key) {
            if key_count == member.key_params.len() {
                return Err(format!("member `{name}` has too many keys"));
            }
            if let Some(mismatch) = key_mismatch(member.key_params[key_count].scalar, key) {
                return Err(format!(
                    "`{name}` key is a {} where the schema declares {}",
                    mismatch.found.name(),
                    mismatch.expected.name()
                ));
            }
            data_path.push(DataPathSegment::Key(key.clone()));
            key_count += 1;
            index += 1;
        }

        if key_count < member.key_params.len() {
            if index < rest.len() {
                return Err(format!(
                    "member `{name}` needs all keys before nested access"
                ));
            }
            break;
        }
        members = match &member.kind {
            CheckedSavedMemberKind::Group => member.group_members.as_slice(),
            CheckedSavedMemberKind::Field { .. } => &[],
        };
    }

    Ok(DataQuery {
        path: render_query_segments(segments),
        root: root.clone(),
        store,
        identity,
        identity_arity: place.identity_keys.len(),
        data_path,
    })
}

fn query_key(segment: &DataQuerySegment) -> Option<&SavedKey> {
    match segment {
        DataQuerySegment::Key(key) => Some(key),
        DataQuerySegment::Root(_) | DataQuerySegment::Member(_) => None,
    }
}

pub(crate) fn render_query_segments(segments: &[DataQuerySegment]) -> String {
    let mut text = String::new();
    for segment in segments {
        match segment {
            DataQuerySegment::Root(name) => {
                text.push('^');
                text.push_str(name);
            }
            DataQuerySegment::Member(name) => {
                text.push('.');
                text.push_str(name);
            }
            DataQuerySegment::Key(key) => {
                push_key(&mut text, key);
            }
        }
    }
    text
}

pub(crate) fn read_query(
    store: &TreeStore,
    query: &DataQuery,
) -> Result<(Option<Vec<u8>>, DataPresence), StoreError> {
    if query.identity.len() < query.identity_arity {
        let has_children = store
            .record_first_child(&query.store, &query.identity)?
            .is_some();
        return Ok((
            None,
            if has_children {
                DataPresence::ChildrenOnly
            } else {
                DataPresence::Absent
            },
        ));
    }
    let value = store.read_data_value(&query.store, &query.identity, &query.data_path)?;
    let presence = if value.is_some() {
        DataPresence::ValueOnly
    } else if store.data_subtree_exists(&query.store, &query.identity, &query.data_path)? {
        DataPresence::ChildrenOnly
    } else {
        DataPresence::Absent
    };
    Ok((value, presence))
}

pub(crate) fn presence_name(presence: DataPresence) -> &'static str {
    match presence {
        DataPresence::Absent => "absent",
        DataPresence::ValueOnly => "value_only",
        DataPresence::ChildrenOnly => "children_only",
    }
}
