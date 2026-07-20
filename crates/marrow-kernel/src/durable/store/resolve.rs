//! Resolution of a sealed site target against the store schema into the executable
//! [`AuthorizedSite`] the kernel ops address: branch-path descent, index-projection
//! component kinds, and the record fields a site carries.

use super::super::{
    AuthTarget, AuthorizedSite, BranchHop, BranchSchema, FieldSchema, IndexComponent, SiteTarget,
    StoreSchema,
};
use crate::codec::value::{ScalarKind, ValueShape};

/// Resolve a sealed [`SiteTarget`] against the store schema into the executable
/// [`AuthorizedSite`] the kernel ops address, once at session setup: the addressed
/// node's root, its branch path, its own record fields (for whole-entry ops), and —
/// for a field target — the field's name, kind, and required flag. A branch target
/// walks its branch path through the recursive schema so the addressed node carries the
/// key kind and record of the branch the path descends to, at any depth.
pub(super) fn resolve_site(
    schema: &StoreSchema,
    root_index: u16,
    target: &SiteTarget,
) -> AuthorizedSite {
    // A managed-index read addresses no source node: it resolves to the index's cell
    // family identity, its read kind, and the scalar kind of each projected component
    // (root key columns and top-level fields, by position), and carries an empty branch
    // path since every index is root-level. The index position is local to this root's
    // schema (the image-wide position was rebased per root when the site table was built).
    if let SiteTarget::IndexScan(position) | SiteTarget::IndexLookup(position) = target {
        let index = &schema.indexes[*position as usize];
        let projection = index
            .projection
            .iter()
            .map(|component| index_component_kind(schema, *component))
            .collect();
        return AuthorizedSite::index(
            schema.root_name.clone(),
            root_index,
            schema.key.clone(),
            AuthTarget::index(index.id, index.unique, projection),
        );
    }
    // A root-level group addresses the root entry (empty branch path); it carries the
    // group's own record. Group-in-branch is durable-only future work, so a group site is
    // root-level at T01.
    if let SiteTarget::GroupEntry(position) = target {
        let group = &schema.groups[*position as usize];
        return AuthorizedSite::new(
            schema.root_name.clone(),
            root_index,
            schema.key.clone(),
            Vec::new(),
            AuthTarget::Group {
                name: group.name.clone(),
                fields: group.fields.clone(),
            },
        );
    }
    // The container node the site addresses: its branch path (one hop per branch-path
    // element) and own record fields. A root target's container is the root; a branch
    // target's (whole entry or field) is the branch node the path descends to.
    let (branch, container_fields): (Vec<BranchHop>, &[FieldSchema]) = match target {
        SiteTarget::WholePayload | SiteTarget::FieldLeaf(_) => (Vec::new(), &schema.fields),
        SiteTarget::BranchEntry(path) | SiteTarget::BranchField { branch: path, .. } => {
            resolve_branch_path(schema, path)
        }
        SiteTarget::IndexScan(_) | SiteTarget::IndexLookup(_) | SiteTarget::GroupEntry(_) => {
            unreachable!("index and group targets resolved above")
        }
    };
    // A whole-entry site enumerates the container's footprint, so it carries the
    // container's record and its groups; a field-target site carries its field plus the
    // container record so a staged set can reconcile the node at commit. A root entry's
    // footprint includes its groups (its own payload); a branch entry carries none, since
    // group-in-branch is not yet executable.
    let target = match target {
        SiteTarget::WholePayload => AuthTarget::Entry {
            fields: container_fields.to_vec(),
            groups: schema.groups.clone(),
        },
        SiteTarget::BranchEntry(_) => AuthTarget::Entry {
            fields: container_fields.to_vec(),
            groups: Vec::new(),
        },
        SiteTarget::FieldLeaf(index) | SiteTarget::BranchField { field: index, .. } => {
            AuthTarget::field(&container_fields[*index as usize], container_fields)
        }
        SiteTarget::IndexScan(_) | SiteTarget::IndexLookup(_) | SiteTarget::GroupEntry(_) => {
            unreachable!("index and group targets resolved above")
        }
    };
    AuthorizedSite::new(
        schema.root_name.clone(),
        root_index,
        schema.key.clone(),
        branch,
        target,
    )
}

/// The scalar kind of one managed-index projection component, resolved by position
/// against the root schema: an identity key column reads its column kind; a top-level
/// field reads its scalar shape. The verifier proves every projected leaf is a stored
/// orderable-key scalar, so a non-scalar field shape here is a forged image reaching an
/// index-ineligible projection — faulted as a kernel invariant breach rather than
/// silently mis-encoded.
fn index_component_kind(schema: &StoreSchema, component: IndexComponent) -> ScalarKind {
    match component {
        IndexComponent::Key(column) => schema.key[column as usize],
        IndexComponent::Field(field) => match &schema.fields[field as usize].shape {
            ValueShape::Scalar(kind) => *kind,
            _ => unreachable!("the verifier proves an index component is a stored scalar"),
        },
    }
}

/// Walk a branch path through the recursive branch schema, one hop per element: the hops
/// down to the addressed branch node — each carrying that branch's name and key kind —
/// and the node's own record fields. The single owner of branch-path-to-node resolution,
/// so a direct branch and a nested branch resolve the same way at increasing depth: hop
/// `i` indexes level `i`'s declaration-ordered branch list, and the next level's list is
/// that branch's own sub-branches.
fn resolve_branch_path<'a>(
    schema: &'a StoreSchema,
    path: &[u16],
) -> (Vec<BranchHop>, &'a [FieldSchema]) {
    let mut hops = Vec::with_capacity(path.len());
    let mut branches: &[BranchSchema] = &schema.branches;
    let mut fields: &[FieldSchema] = &schema.fields;
    for &index in path {
        let branch = &branches[index as usize];
        hops.push(BranchHop::new(branch.name.clone(), branch.key.clone()));
        fields = &branch.fields;
        branches = &branch.branches;
    }
    (hops, fields)
}
