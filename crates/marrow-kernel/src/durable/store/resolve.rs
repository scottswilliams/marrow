//! Resolution of a sealed site target against the store schema and its numbering into the
//! executable [`AuthorizedSite`] the kernel ops address: branch-path descent, index-
//! projection component kinds, and the numbered record fields a site carries. No source
//! spelling enters a resolved site — every addressed node carries its cell-key number
//! (FR01 §3), so the physical layer keys cells by number, never by name.

use super::super::{
    AuthTarget, AuthorizedSite, BranchHop, BranchNumbering, BranchSchema, FieldSchema,
    GroupNumbering, IndexComponent, ResolvedField, ResolvedGroup, RootNumbering, SiteTarget,
    StoreSchema,
};
use crate::codec::value::{ScalarKind, ValueShape};

/// Resolve a sealed [`SiteTarget`] against the store schema and its [`RootNumbering`] into
/// the executable [`AuthorizedSite`] the kernel ops address, once at session setup: the
/// addressed node's root number, its branch path (numbered hops), its own numbered record
/// fields (for whole-entry ops), and — for a field target — the field's number, kind, and
/// required flag. A branch target walks its branch path through the recursive schema and
/// numbering so the addressed node carries the key kind and numbered record of the branch
/// the path descends to, at any depth.
pub(super) fn resolve_site(
    schema: &StoreSchema,
    numbering: &RootNumbering,
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
            numbering.root,
            root_index,
            schema.key.clone(),
            AuthTarget::index(index.id, index.unique, projection),
        );
    }
    // A root-level group addresses the root entry (empty branch path); it carries the
    // group's own numbered record. Group-in-branch is durable-only future work, so a group
    // site is root-level at T01.
    if let SiteTarget::GroupEntry(position) = target {
        let group = &schema.groups[*position as usize];
        let group_numbering = &numbering.groups[*position as usize];
        return AuthorizedSite::new(
            numbering.root,
            root_index,
            schema.key.clone(),
            Vec::new(),
            AuthTarget::Group {
                number: group_numbering.number,
                fields: resolve_fields(&group.fields, &group_numbering.fields),
            },
        );
    }
    // The container node the site addresses: its branch path (one numbered hop per branch-
    // path element) and own numbered record fields. A root target's container is the root; a
    // branch target's (whole entry or field) is the branch node the path descends to.
    let (branch, container_fields): (Vec<BranchHop>, Vec<ResolvedField>) = match target {
        SiteTarget::WholePayload | SiteTarget::FieldLeaf(_) => (
            Vec::new(),
            resolve_fields(&schema.fields, &numbering.fields),
        ),
        SiteTarget::BranchEntry(path) | SiteTarget::BranchField { branch: path, .. } => {
            resolve_branch_path(schema, numbering, path)
        }
        SiteTarget::IndexScan(_) | SiteTarget::IndexLookup(_) | SiteTarget::GroupEntry(_) => {
            unreachable!("index and group targets resolved above")
        }
    };
    // A whole-entry site enumerates the container's footprint, so it carries the
    // container's numbered record and its numbered groups; a field-target site carries its
    // field plus the container record so a staged set can reconcile the node at commit. A
    // root entry's footprint includes its groups (its own payload); a branch entry carries
    // none, since group-in-branch is not yet executable.
    let target = match target {
        SiteTarget::WholePayload => AuthTarget::Entry {
            fields: container_fields,
            groups: resolve_groups(&schema.groups, &numbering.groups),
        },
        SiteTarget::BranchEntry(_) => AuthTarget::Entry {
            fields: container_fields,
            groups: Vec::new(),
        },
        SiteTarget::FieldLeaf(index) | SiteTarget::BranchField { field: index, .. } => {
            AuthTarget::field(&container_fields[*index as usize], &container_fields)
        }
        SiteTarget::IndexScan(_) | SiteTarget::IndexLookup(_) | SiteTarget::GroupEntry(_) => {
            unreachable!("index and group targets resolved above")
        }
    };
    AuthorizedSite::new(
        numbering.root,
        root_index,
        schema.key.clone(),
        branch,
        target,
    )
}

/// The resolved fields of a record: one [`ResolvedField`] per field, pairing the schema's
/// value shape and required flag with the field's cell-key number, in declaration order.
/// The single point where a `FieldSchema` becomes number-keyed — the resolved layer carries
/// no source spelling past here.
fn resolve_fields(fields: &[FieldSchema], numbers: &[u32]) -> Vec<ResolvedField> {
    fields
        .iter()
        .zip(numbers)
        .map(|(field, number)| ResolvedField {
            number: *number,
            name: field.name.clone(),
            shape: field.shape.clone(),
            required: field.required,
        })
        .collect()
}

/// The resolved groups of a root: one [`ResolvedGroup`] per group, each carrying the group's
/// cell-key number and its resolved fields, in declaration order.
fn resolve_groups(
    groups: &[super::super::GroupSchema],
    numberings: &[GroupNumbering],
) -> Vec<ResolvedGroup> {
    groups
        .iter()
        .zip(numberings)
        .map(|(group, numbering)| ResolvedGroup {
            number: numbering.number,
            fields: resolve_fields(&group.fields, &numbering.fields),
        })
        .collect()
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

/// Walk a branch path through the recursive branch schema and numbering, one hop per
/// element: the numbered hops down to the addressed branch node — each carrying that
/// branch's cell-key number and key kind — and the node's own resolved record fields. The
/// single owner of branch-path-to-node resolution, so a direct branch and a nested branch
/// resolve the same way at increasing depth: hop `i` indexes level `i`'s declaration-ordered
/// branch list, and the next level's list is that branch's own sub-branches.
fn resolve_branch_path(
    schema: &StoreSchema,
    numbering: &RootNumbering,
    path: &[u16],
) -> (Vec<BranchHop>, Vec<ResolvedField>) {
    let mut hops = Vec::with_capacity(path.len());
    let mut branches: &[BranchSchema] = &schema.branches;
    let mut branch_numbers: &[BranchNumbering] = &numbering.branches;
    let mut fields: &[FieldSchema] = &schema.fields;
    let mut field_numbers: &[u32] = &numbering.fields;
    for &index in path {
        let branch = &branches[index as usize];
        let branch_numbering = &branch_numbers[index as usize];
        hops.push(BranchHop::new(branch_numbering.number, branch.key.clone()));
        fields = &branch.fields;
        field_numbers = &branch_numbering.fields;
        branches = &branch.branches;
        branch_numbers = &branch_numbering.branches;
    }
    (hops, resolve_fields(fields, field_numbers))
}
