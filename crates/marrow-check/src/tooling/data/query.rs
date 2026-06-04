use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::DataPathSegment;

use crate::{CheckedProgram, CheckedSavedMemberKind, checked_saved_root_place};

use super::render::render_query_segments;
use super::shape::{QueryMemberKind, key_mismatch, query_segment_for_member, tooling_catalog_id};
use super::{DataQuery, DataQuerySegment};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StorageDataQuery {
    pub(crate) store: CatalogId,
    pub(crate) identity: Vec<SavedKey>,
    pub(crate) identity_arity: usize,
    pub(crate) data_path: Vec<DataPathSegment>,
}

pub fn resolve_data_query(
    program: &CheckedProgram,
    segments: &[DataQuerySegment],
) -> Result<DataQuery, String> {
    let steps: Vec<QuerySegment<'_>> = segments.iter().map(QuerySegment::from_data).collect();
    resolve_query_steps(program, render_query_segments(segments), &steps)
}

pub fn resolve_source_text_data_query(
    program: &CheckedProgram,
    segments: &[crate::PathSegment],
) -> Result<DataQuery, String> {
    let steps: Vec<QuerySegment<'_>> = segments
        .iter()
        .map(QuerySegment::from_source_text)
        .collect();
    resolve_query_steps(program, crate::display_path(segments), &steps)
}

fn resolve_query_steps(
    program: &CheckedProgram,
    path: String,
    segments: &[QuerySegment<'_>],
) -> Result<DataQuery, String> {
    let Some((QuerySegment::Root(root), rest)) = segments.split_first() else {
        return Err("path must start with a saved root, such as `^books`".into());
    };
    let place = checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .ok_or_else(|| format!("unknown saved root `^{}`", root))?;
    let store =
        tooling_catalog_id(&place.store_catalog_id, "store").map_err(|error| error.to_string())?;
    let mut identity = Vec::new();
    let mut rendered_segments = vec![DataQuerySegment::Root((*root).to_string())];
    let mut index = 0usize;
    while let Some(segment) = rest.get(index) {
        let Some(key) = query_key(segment) else {
            break;
        };
        if identity.len() == place.identity_keys.len() {
            return Err(format!("`^{}` has too many identity keys", root));
        }
        if let Some(mismatch) = key_mismatch(place.identity_keys[identity.len()].scalar, key) {
            return Err(format!(
                "identity key is a {} where `^{}` declares {}",
                mismatch.found.name(),
                root,
                mismatch.expected.name()
            ));
        }
        identity.push(key.clone());
        rendered_segments.push(DataQuerySegment::Key(key.clone()));
        index += 1;
    }
    if index < rest.len() && identity.len() != place.identity_keys.len() {
        return Err(format!(
            "`^{}` expects {} identity key(s) before member access",
            root,
            place.identity_keys.len(),
        ));
    }

    let mut data_path = Vec::new();
    let mut members = place.root_members.as_slice();
    while let Some(segment) = rest.get(index) {
        let Some((name, kind)) = query_member(segment) else {
            return Err("a key must follow a saved root or a keyed member".into());
        };
        let member = members
            .iter()
            .find(|member| member.name == *name && kind.matches(member))
            .ok_or_else(|| kind.unknown_message(name))?;
        rendered_segments.push(query_segment_for_member(member));
        data_path.push(DataPathSegment::Member(
            tooling_catalog_id(&member.catalog_id, "resource member")
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
            rendered_segments.push(DataQuerySegment::Key(key.clone()));
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

    Ok(DataQuery::new(
        path,
        (*root).to_string(),
        rendered_segments,
        StorageDataQuery {
            store,
            identity,
            identity_arity: place.identity_keys.len(),
            data_path,
        },
    ))
}

#[derive(Clone, Copy)]
enum QuerySegment<'a> {
    Root(&'a str),
    Member(&'a str, QueryMemberKind),
    Key(&'a SavedKey),
}

impl<'a> QuerySegment<'a> {
    fn from_data(segment: &'a DataQuerySegment) -> Self {
        match segment {
            DataQuerySegment::Root(name) => Self::Root(name),
            DataQuerySegment::Field(name) => Self::Member(name, QueryMemberKind::Field),
            DataQuerySegment::Layer(name) => Self::Member(name, QueryMemberKind::Layer),
            DataQuerySegment::Key(key) => Self::Key(key),
        }
    }

    fn from_source_text(segment: &'a crate::PathSegment) -> Self {
        match segment {
            crate::PathSegment::Root(name) => Self::Root(name),
            crate::PathSegment::Field(name)
            | crate::PathSegment::ChildLayer(name)
            | crate::PathSegment::Index(name) => Self::Member(name, QueryMemberKind::SourceText),
            crate::PathSegment::RecordKey(key) | crate::PathSegment::IndexKey(key) => {
                Self::Key(key)
            }
        }
    }
}

fn query_key<'a>(segment: &QuerySegment<'a>) -> Option<&'a SavedKey> {
    match segment {
        QuerySegment::Key(key) => Some(key),
        QuerySegment::Root(_) | QuerySegment::Member(_, _) => None,
    }
}

fn query_member<'a>(segment: &QuerySegment<'a>) -> Option<(&'a str, QueryMemberKind)> {
    match segment {
        QuerySegment::Member(name, kind) => Some((name, *kind)),
        QuerySegment::Root(_) | QuerySegment::Key(_) => None,
    }
}

pub fn data_query_under_prefix(candidate: &DataQuery, prefix: &DataQuery) -> bool {
    candidate.root == prefix.root
        && candidate.storage.store == prefix.storage.store
        && candidate.storage.identity_arity == prefix.storage.identity_arity
        && candidate
            .storage
            .identity
            .starts_with(&prefix.storage.identity)
        && candidate
            .storage
            .data_path
            .starts_with(&prefix.storage.data_path)
}
