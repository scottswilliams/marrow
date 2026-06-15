use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::tooling::ToolingError;
use crate::{CheckedProgram, CheckedSavedMember, checked_saved_root_place};

use super::query::resolve_data_query;
use super::query_error::QueryError;
use super::record_nav;
use super::shape::stored_key_mismatch;
use super::shape::{declared_members_below_path, tooling_catalog_id};
use super::traversal::data_roots_in_store;
use super::{DataChild, DataChildrenPage, DataQuery, DataQuerySegment};

/// How a resolved query's children are scanned, decided once so that both the
/// child listing and its paging predicate read the classification the same way.
enum ChildScanKind {
    Roots,
    RecordChildren(DataQuery),
    Members {
        query: DataQuery,
        members: Vec<CheckedSavedMember>,
    },
    KeyChildren(DataQuery),
    Leaf,
    /// The path names durable identity that was never committed (a never-run
    /// project or a pending member), so it has no children yet.
    Empty,
}

impl ChildScanKind {
    fn is_paged(&self) -> bool {
        matches!(self, Self::RecordChildren(_) | Self::KeyChildren(_))
    }
}

fn classify_child_scan(
    program: &CheckedProgram,
    segments: &[DataQuerySegment],
) -> Result<ChildScanKind, ToolingError> {
    if segments.is_empty() {
        return Ok(ChildScanKind::Roots);
    }
    let Some(query) = resolve_data_query(program, segments)? else {
        return Ok(ChildScanKind::Empty);
    };
    if query.storage.identity.len() < query.storage.identity_arity {
        return Ok(ChildScanKind::RecordChildren(query));
    }
    let Some((DataQuerySegment::Root(root), _)) = segments.split_first() else {
        return Err(QueryError::MissingRoot.into());
    };
    let place = checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .ok_or_else(|| QueryError::UnknownRoot { root: root.clone() })?;
    if let Some(members) =
        declared_members_below_path(&place.root_members, &query.storage.data_path)
    {
        let members = members.to_vec();
        return Ok(ChildScanKind::Members { query, members });
    }
    if query.storage.data_path.is_empty() {
        return Ok(ChildScanKind::Leaf);
    }
    Ok(ChildScanKind::KeyChildren(query))
}

pub fn data_children(
    program: &CheckedProgram,
    store: &TreeStore,
    segments: &[DataQuerySegment],
    limit: usize,
    resume: Option<&SavedKey>,
) -> Result<DataChildrenPage, ToolingError> {
    if limit == 0 {
        return Err(QueryError::ZeroLimit.into());
    }
    match classify_child_scan(program, segments)? {
        ChildScanKind::Roots => {
            let children = data_roots_in_store(program, store)?
                .into_iter()
                .map(DataChild::Member)
                .collect();
            Ok(DataChildrenPage {
                children,
                truncated: false,
                cursor: None,
            })
        }
        ChildScanKind::RecordChildren(query) => record_children(store, &query, limit, resume),
        ChildScanKind::Members { query, members } => {
            if resume.is_some() {
                return Err(QueryError::MembersTakeNoCursor.into());
            }
            member_children(store, &query, &members)
        }
        ChildScanKind::Leaf => Err(QueryError::NoChildScan.into()),
        ChildScanKind::KeyChildren(query) => data_key_children(store, &query, limit, resume),
        ChildScanKind::Empty => Ok(DataChildrenPage {
            children: Vec::new(),
            truncated: false,
            cursor: None,
        }),
    }
}

pub fn data_children_supports_paging(
    program: &CheckedProgram,
    segments: &[DataQuerySegment],
) -> Result<bool, ToolingError> {
    Ok(classify_child_scan(program, segments)?.is_paged())
}

fn record_children(
    store: &TreeStore,
    query: &DataQuery,
    limit: usize,
    resume: Option<&SavedKey>,
) -> Result<DataChildrenPage, ToolingError> {
    let first = match resume {
        Some(anchor) => record_nav::next_record_child(
            store,
            &query.storage.store,
            &query.storage.identity,
            query.storage.identity_arity,
            anchor,
        )?,
        None => record_nav::first_record_child(
            store,
            &query.storage.store,
            &query.storage.identity,
            query.storage.identity_arity,
        )?,
    };
    let expected = query
        .storage
        .identity_key_scalars
        .get(query.storage.identity.len())
        .copied()
        .flatten();
    page_key_children(first, limit, expected, |anchor| {
        record_nav::next_record_child(
            store,
            &query.storage.store,
            &query.storage.identity,
            query.storage.identity_arity,
            anchor,
        )
        .map_err(ToolingError::Store)
    })
}

fn member_children(
    store: &TreeStore,
    query: &DataQuery,
    members: &[CheckedSavedMember],
) -> Result<DataChildrenPage, ToolingError> {
    let mut children = Vec::new();
    for member in members {
        let Some(catalog) = tooling_catalog_id(&member.catalog_id, "resource member")? else {
            continue;
        };
        let mut path = query.storage.data_path.clone();
        path.push(DataPathSegment::Member(catalog));
        let present = if member.is_plain_field() {
            store
                .read_data_value(&query.storage.store, &query.storage.identity, &path)?
                .is_some()
        } else {
            store.data_subtree_exists(&query.storage.store, &query.storage.identity, &path)?
        };
        if present {
            children.push(DataChild::Member(member.name.clone()));
        }
    }
    Ok(DataChildrenPage {
        children,
        truncated: false,
        cursor: None,
    })
}

fn data_key_children(
    store: &TreeStore,
    query: &DataQuery,
    limit: usize,
    resume: Option<&SavedKey>,
) -> Result<DataChildrenPage, ToolingError> {
    let first = match resume {
        Some(anchor) => store.data_next_child(
            &query.storage.store,
            &query.storage.identity,
            &query.storage.data_path,
            anchor,
        )?,
        None => store.data_first_child(
            &query.storage.store,
            &query.storage.identity,
            &query.storage.data_path,
        )?,
    };
    let expected = query
        .storage
        .data_key_scalars
        .get(query.storage.data_key_prefix_len)
        .copied()
        .flatten();
    page_key_children(first, limit, expected, |anchor| {
        store
            .data_next_child(
                &query.storage.store,
                &query.storage.identity,
                &query.storage.data_path,
                anchor,
            )
            .map_err(ToolingError::Store)
    })
}

fn page_key_children(
    first: Option<SavedKey>,
    limit: usize,
    expected: Option<crate::ScalarType>,
    mut next: impl FnMut(&SavedKey) -> Result<Option<SavedKey>, ToolingError>,
) -> Result<DataChildrenPage, ToolingError> {
    let mut children = Vec::new();
    let mut last = None;
    let mut child = first;
    while let Some(key) = child {
        if children.len() == limit {
            return Ok(DataChildrenPage {
                children,
                truncated: true,
                cursor: last,
            });
        }
        stored_key_mismatch(expected, &key)?;
        children.push(DataChild::Key(key.clone()));
        child = next(&key)?;
        last = Some(key);
    }
    Ok(DataChildrenPage {
        children,
        truncated: false,
        cursor: None,
    })
}
