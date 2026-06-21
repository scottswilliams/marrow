use marrow_store::StoreError;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment as StoreDataPathSegment, TreeStore};

use crate::tooling::ToolingError;
use crate::{CheckedProgram, CheckedRuntimeProgram, CheckedSavedMember};

use super::path::{
    resolve_data_path, resolve_runtime_data_path, resolve_runtime_saved_data_path,
    resolve_saved_data_path,
};
use super::path_error::DataPathError;
use super::program::{DataProgram, inspection_root_place};
use super::record_nav;
use super::render::render_data_path_segments;
use super::shape::stored_key_mismatch;
use super::shape::{data_path_segment_for_member, declared_members_below_path, tooling_catalog_id};
use super::traversal::{
    data_roots_in_store, runtime_data_roots_in_store, runtime_saved_data_root_views_in_store,
    saved_data_root_views_in_store,
};
use super::{
    DataChild, DataChildView, DataChildViewsPage, DataChildrenPage, DataPathSegment,
    ResolvedDataPath, SavedDataPathSegment,
};

/// How a resolved path's children are scanned, decided once so that both the
/// child listing and its paging predicate read the classification the same way.
enum ChildScanKind {
    Roots,
    RecordChildren(ResolvedDataPath),
    Members {
        path: ResolvedDataPath,
        members: Vec<CheckedSavedMember>,
    },
    KeyChildren(ResolvedDataPath),
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
    segments: &[DataPathSegment],
) -> Result<ChildScanKind, ToolingError> {
    classify_child_scan_in(program, segments, resolve_data_path)
}

fn classify_runtime_child_scan(
    program: &CheckedRuntimeProgram,
    segments: &[DataPathSegment],
) -> Result<ChildScanKind, ToolingError> {
    classify_child_scan_in(program, segments, resolve_runtime_data_path)
}

fn classify_saved_child_scan(
    program: &CheckedProgram,
    segments: &[SavedDataPathSegment],
) -> Result<ChildScanKind, ToolingError> {
    classify_child_scan_in(program, segments, resolve_saved_data_path)
}

fn classify_runtime_saved_child_scan(
    program: &CheckedRuntimeProgram,
    segments: &[SavedDataPathSegment],
) -> Result<ChildScanKind, ToolingError> {
    classify_child_scan_in(program, segments, resolve_runtime_saved_data_path)
}

fn classify_child_scan_in<P, S>(
    program: &P,
    segments: &[S],
    resolve: fn(&P, &[S]) -> Result<Option<ResolvedDataPath>, ToolingError>,
) -> Result<ChildScanKind, ToolingError>
where
    P: DataProgram + ?Sized,
{
    if segments.is_empty() {
        return Ok(ChildScanKind::Roots);
    }
    let Some(path) = resolve(program, segments)? else {
        return Ok(ChildScanKind::Empty);
    };
    if path.storage.identity.len() < path.storage.identity_arity {
        return Ok(ChildScanKind::RecordChildren(path));
    }
    let place =
        inspection_root_place(program, path.root()).ok_or_else(|| DataPathError::UnknownRoot {
            root: path.root().to_string(),
        })?;
    if let Some(members) = declared_members_below_path(&place.root_members, &path.storage.data_path)
    {
        let members = members.to_vec();
        return Ok(ChildScanKind::Members { path, members });
    }
    if path.storage.data_path.is_empty() {
        return Ok(ChildScanKind::Leaf);
    }
    Ok(ChildScanKind::KeyChildren(path))
}

pub fn data_children(
    program: &CheckedProgram,
    store: &TreeStore,
    segments: &[DataPathSegment],
    limit: usize,
    resume: Option<&SavedKey>,
) -> Result<DataChildrenPage, ToolingError> {
    data_children_in(
        program,
        store,
        segments,
        limit,
        resume,
        classify_child_scan,
        data_roots_in_store,
    )
}

pub fn runtime_data_children(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    segments: &[DataPathSegment],
    limit: usize,
    resume: Option<&SavedKey>,
) -> Result<DataChildrenPage, ToolingError> {
    data_children_in(
        program,
        store,
        segments,
        limit,
        resume,
        classify_runtime_child_scan,
        runtime_data_roots_in_store,
    )
}

pub fn saved_data_child_views(
    program: &CheckedProgram,
    store: &TreeStore,
    segments: &[SavedDataPathSegment],
    limit: usize,
    resume: Option<&SavedKey>,
) -> Result<DataChildViewsPage, ToolingError> {
    data_child_views_in(
        program,
        store,
        segments,
        limit,
        resume,
        classify_saved_child_scan,
        saved_data_root_views_in_store,
    )
}

pub fn runtime_saved_data_child_views(
    program: &CheckedRuntimeProgram,
    store: &TreeStore,
    segments: &[SavedDataPathSegment],
    limit: usize,
    resume: Option<&SavedKey>,
) -> Result<DataChildViewsPage, ToolingError> {
    data_child_views_in(
        program,
        store,
        segments,
        limit,
        resume,
        classify_runtime_saved_child_scan,
        runtime_saved_data_root_views_in_store,
    )
}

fn data_children_in<P>(
    program: &P,
    store: &TreeStore,
    segments: &[DataPathSegment],
    limit: usize,
    resume: Option<&SavedKey>,
    classify: fn(&P, &[DataPathSegment]) -> Result<ChildScanKind, ToolingError>,
    roots: fn(&P, &TreeStore) -> Result<Vec<String>, StoreError>,
) -> Result<DataChildrenPage, ToolingError>
where
    P: DataProgram + ?Sized,
{
    if limit == 0 {
        return Err(DataPathError::ZeroLimit.into());
    }
    match classify(program, segments)? {
        ChildScanKind::Roots => {
            let children = roots(program, store)?
                .into_iter()
                .map(DataChild::Root)
                .collect();
            Ok(DataChildrenPage {
                children,
                truncated: false,
                cursor: None,
            })
        }
        ChildScanKind::RecordChildren(path) => record_children(store, &path, limit, resume),
        ChildScanKind::Members { path, members } => {
            if resume.is_some() {
                return Err(DataPathError::MembersTakeNoCursor.into());
            }
            member_children(store, &path, &members)
        }
        ChildScanKind::Leaf => Err(DataPathError::NoChildScan.into()),
        ChildScanKind::KeyChildren(path) => data_key_children(store, &path, limit, resume),
        ChildScanKind::Empty => Ok(DataChildrenPage {
            children: Vec::new(),
            truncated: false,
            cursor: None,
        }),
    }
}

fn data_child_views_in<P>(
    program: &P,
    store: &TreeStore,
    segments: &[SavedDataPathSegment],
    limit: usize,
    resume: Option<&SavedKey>,
    classify: fn(&P, &[SavedDataPathSegment]) -> Result<ChildScanKind, ToolingError>,
    roots: fn(&P, &TreeStore) -> Result<Vec<DataChildView>, StoreError>,
) -> Result<DataChildViewsPage, ToolingError>
where
    P: DataProgram + ?Sized,
{
    if limit == 0 {
        return Err(DataPathError::ZeroLimit.into());
    }
    match classify(program, segments)? {
        ChildScanKind::Roots => Ok(DataChildViewsPage {
            children: roots(program, store)?,
            truncated: false,
            cursor: None,
        }),
        ChildScanKind::RecordChildren(path) => {
            key_child_views(record_children(store, &path, limit, resume)?)
        }
        ChildScanKind::Members { path, members } => {
            if resume.is_some() {
                return Err(DataPathError::MembersTakeNoCursor.into());
            }
            member_child_views(program, store, &path, &members)
        }
        ChildScanKind::Leaf => Err(DataPathError::NoChildScan.into()),
        ChildScanKind::KeyChildren(path) => {
            key_child_views(data_key_children(store, &path, limit, resume)?)
        }
        ChildScanKind::Empty => Ok(DataChildViewsPage {
            children: Vec::new(),
            truncated: false,
            cursor: None,
        }),
    }
}

pub fn data_children_supports_paging(
    program: &CheckedProgram,
    segments: &[DataPathSegment],
) -> Result<bool, ToolingError> {
    Ok(classify_child_scan(program, segments)?.is_paged())
}

pub fn runtime_data_children_supports_paging(
    program: &CheckedRuntimeProgram,
    segments: &[DataPathSegment],
) -> Result<bool, ToolingError> {
    Ok(classify_runtime_child_scan(program, segments)?.is_paged())
}

fn record_children(
    store: &TreeStore,
    path: &ResolvedDataPath,
    limit: usize,
    resume: Option<&SavedKey>,
) -> Result<DataChildrenPage, ToolingError> {
    let first = match resume {
        Some(anchor) => record_nav::next_record_child(
            store,
            &path.storage.store,
            &path.storage.identity,
            path.storage.identity_arity,
            anchor,
        )?,
        None => record_nav::first_record_child(
            store,
            &path.storage.store,
            &path.storage.identity,
            path.storage.identity_arity,
        )?,
    };
    let expected = path
        .storage
        .identity_key_scalars
        .get(path.storage.identity.len())
        .copied()
        .flatten();
    page_key_children(first, limit, expected, |anchor| {
        record_nav::next_record_child(
            store,
            &path.storage.store,
            &path.storage.identity,
            path.storage.identity_arity,
            anchor,
        )
        .map_err(ToolingError::Store)
    })
}

fn member_children(
    store: &TreeStore,
    path: &ResolvedDataPath,
    members: &[CheckedSavedMember],
) -> Result<DataChildrenPage, ToolingError> {
    let mut children = Vec::new();
    for member in members {
        let Some(catalog) = tooling_catalog_id(&member.catalog_id, "resource member")? else {
            continue;
        };
        let mut data_path = path.storage.data_path.clone();
        data_path.push(StoreDataPathSegment::Member(catalog));
        let present = if member.is_plain_field() {
            store
                .read_data_value(&path.storage.store, &path.storage.identity, &data_path)?
                .is_some()
        } else {
            store.data_subtree_exists(&path.storage.store, &path.storage.identity, &data_path)?
        };
        if present {
            children.push(DataChild::from(data_path_segment_for_member(member)));
        }
    }
    Ok(DataChildrenPage {
        children,
        truncated: false,
        cursor: None,
    })
}

fn member_child_views(
    program: &(impl DataProgram + ?Sized),
    store: &TreeStore,
    path: &ResolvedDataPath,
    members: &[CheckedSavedMember],
) -> Result<DataChildViewsPage, ToolingError> {
    let mut children = Vec::new();
    for member in members {
        let Some(member_catalog_id) = member.catalog_id.clone() else {
            continue;
        };
        if !program.has_accepted_catalog_id(&member_catalog_id) {
            continue;
        }
        let Some(catalog) = tooling_catalog_id(&member.catalog_id, "resource member")? else {
            continue;
        };
        let mut data_path = path.storage.data_path.clone();
        data_path.push(StoreDataPathSegment::Member(catalog));
        let present = if member.is_plain_field() {
            store
                .read_data_value(&path.storage.store, &path.storage.identity, &data_path)?
                .is_some()
        } else {
            store.data_subtree_exists(&path.storage.store, &path.storage.identity, &data_path)?
        };
        if present {
            let segment = if member.is_plain_field() {
                SavedDataPathSegment::Field { member_catalog_id }
            } else {
                SavedDataPathSegment::Layer { member_catalog_id }
            };
            children.push(DataChildView {
                segment,
                label: member.name.clone(),
            });
        }
    }
    Ok(DataChildViewsPage {
        children,
        truncated: false,
        cursor: None,
    })
}

fn data_key_children(
    store: &TreeStore,
    path: &ResolvedDataPath,
    limit: usize,
    resume: Option<&SavedKey>,
) -> Result<DataChildrenPage, ToolingError> {
    let first = match resume {
        Some(anchor) => store.data_next_child(
            &path.storage.store,
            &path.storage.identity,
            &path.storage.data_path,
            anchor,
        )?,
        None => store.data_first_child(
            &path.storage.store,
            &path.storage.identity,
            &path.storage.data_path,
        )?,
    };
    let expected = path
        .storage
        .data_key_scalars
        .get(path.storage.data_key_prefix_len)
        .copied()
        .flatten();
    page_key_children(first, limit, expected, |anchor| {
        store
            .data_next_child(
                &path.storage.store,
                &path.storage.identity,
                &path.storage.data_path,
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

fn key_child_views(page: DataChildrenPage) -> Result<DataChildViewsPage, ToolingError> {
    let children = page
        .children
        .into_iter()
        .map(|child| match child {
            DataChild::Key(key) => {
                let label = render_data_path_segments(&[DataPathSegment::Key(key.clone())]);
                Ok(DataChildView {
                    segment: SavedDataPathSegment::Key(key),
                    label,
                })
            }
            DataChild::Root(_) | DataChild::Field(_) | DataChild::Layer(_) => {
                Err(DataPathError::NoChildScan.into())
            }
        })
        .collect::<Result<Vec<_>, ToolingError>>()?;
    Ok(DataChildViewsPage {
        children,
        truncated: page.truncated,
        cursor: page.cursor,
    })
}
