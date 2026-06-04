use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};

use crate::tooling::ToolingError;
use crate::{CheckedProgram, CheckedSavedMember, CheckedSavedMemberKind, checked_saved_root_place};

use super::query::resolve_data_query;
use super::shape::{declared_members_below_path, tooling_catalog_id};
use super::traversal::data_roots_in_store;
use super::{DataChild, DataChildrenPage, DataQuery, DataQuerySegment};

pub fn data_children(
    program: &CheckedProgram,
    store: &TreeStore,
    segments: &[DataQuerySegment],
    limit: usize,
    resume: Option<&SavedKey>,
) -> Result<DataChildrenPage, ToolingError> {
    if limit == 0 {
        return Err(ToolingError::Query(
            "`limit` must be greater than zero".to_string(),
        ));
    }
    if segments.is_empty() {
        let children = data_roots_in_store(program, store)?
            .into_iter()
            .map(DataChild::Member)
            .collect();
        return Ok(DataChildrenPage {
            children,
            truncated: false,
            cursor: None,
        });
    }

    let query = resolve_data_query(program, segments).map_err(ToolingError::Query)?;
    if query.storage.identity.len() < query.storage.identity_arity {
        return record_children(store, &query, limit, resume);
    }
    let Some((DataQuerySegment::Root(root), _)) = segments.split_first() else {
        return Err(ToolingError::Query(
            "path must start with a saved root".to_string(),
        ));
    };
    let place = checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .ok_or_else(|| ToolingError::Query(format!("unknown saved root `^{root}`")))?;
    if let Some(members) =
        declared_members_below_path(&place.root_members, &query.storage.data_path)
    {
        if resume.is_some() {
            return Err(ToolingError::Query(
                "declared members are not a paged child scan, so they take no `cursor`".to_string(),
            ));
        }
        return member_children(store, &query, members);
    }
    if query.storage.data_path.is_empty() {
        return Err(ToolingError::Query(
            "declared members are not a paged child scan, so they take no `cursor`".to_string(),
        ));
    }
    data_key_children(store, &query, limit, resume)
}

pub fn data_children_supports_paging(
    program: &CheckedProgram,
    segments: &[DataQuerySegment],
) -> Result<bool, ToolingError> {
    if segments.is_empty() {
        return Ok(false);
    }
    let query = resolve_data_query(program, segments).map_err(ToolingError::Query)?;
    if query.storage.identity.len() < query.storage.identity_arity {
        return Ok(true);
    }
    let Some((DataQuerySegment::Root(root), _)) = segments.split_first() else {
        return Err(ToolingError::Query(
            "path must start with a saved root".to_string(),
        ));
    };
    let place = checked_saved_root_place(program, root, marrow_syntax::SourceSpan::default())
        .ok_or_else(|| ToolingError::Query(format!("unknown saved root `^{root}`")))?;
    Ok(
        declared_members_below_path(&place.root_members, &query.storage.data_path).is_none()
            && !query.storage.data_path.is_empty(),
    )
}

fn record_children(
    store: &TreeStore,
    query: &DataQuery,
    limit: usize,
    resume: Option<&SavedKey>,
) -> Result<DataChildrenPage, ToolingError> {
    let first = match resume {
        Some(anchor) => {
            store.record_next_child(&query.storage.store, &query.storage.identity, anchor)?
        }
        None => store.record_first_child(&query.storage.store, &query.storage.identity)?,
    };
    page_key_children(first, limit, |anchor| {
        store
            .record_next_child(&query.storage.store, &query.storage.identity, anchor)
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
        let catalog = tooling_catalog_id(&member.catalog_id, "resource member")?;
        let mut path = query.storage.data_path.clone();
        path.push(DataPathSegment::Member(catalog));
        let present = if !member.key_params.is_empty() {
            store.data_subtree_exists(&query.storage.store, &query.storage.identity, &path)?
        } else {
            match &member.kind {
                CheckedSavedMemberKind::Field { .. } => store
                    .read_data_value(&query.storage.store, &query.storage.identity, &path)?
                    .is_some(),
                CheckedSavedMemberKind::Group => store.data_subtree_exists(
                    &query.storage.store,
                    &query.storage.identity,
                    &path,
                )?,
            }
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
    page_key_children(first, limit, |anchor| {
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
