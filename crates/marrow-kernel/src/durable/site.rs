//! The store's checked projection: its root schemas and the site table validated against
//! them.
//!
//! A site names one operation's durable target — a root's whole payload, a field leaf, a
//! keyed branch entry or one of its fields, an unkeyed group, or a managed-index read. Every
//! part of that name is a *position*: a root index, a branch path, a field index, a group
//! index, an index index. Positions carried as bare struct fields would reach session
//! setup as raw slice indices, making a position that names nothing a panic in the
//! resolver rather than a refusal at the boundary.
//!
//! [`StoreProjection`] closes that: it is minted only by [`StoreProjectionBuilder`], which
//! resolves every site against the **completed** root table before publication and stores
//! the resolved positions in typed form. A published projection therefore carries schemas
//! and sites that are known to describe each other — they cannot be separated and re-paired,
//! which is what made an entry-time check unable to close this class.

use super::schema::{BranchPos, FieldPos, GroupPos, IndexPos, RootPos, StoreSchema};

/// The store's roots and the site table checked against them: the one intake shape every
/// store, attachment, and lifecycle entry point accepts.
///
/// Opaque and minted only by [`StoreProjectionBuilder`]. The roots are in declaration
/// (image) order and the sites in image site-index order, so `Durable::site` resolves by
/// image site index exactly as before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreProjection {
    roots: Vec<StoreSchema>,
    sites: Vec<SiteSlot>,
}

impl StoreProjection {
    /// Begin a projection: declare every root, then every site.
    pub fn builder() -> StoreProjectionBuilder {
        StoreProjectionBuilder::default()
    }

    /// The store's roots by declaration position, each with its own id-keyed cell family.
    pub fn roots(&self) -> &[StoreSchema] {
        &self.roots
    }

    /// The checked site table, in image site-index order. A parked image site keeps its
    /// slot as a typed absence, so a resolved site index always means the image's.
    pub(crate) fn sites(&self) -> &[SiteSlot] {
        &self.sites
    }

    /// Reopen this projection's roots for a different site table, consuming the sites it
    /// carries. The importer's caller derives one projection from the image and then opens
    /// the store under the importer's own single create site, rather than reaching for the
    /// raw root table to pair with a second, unchecked site vector.
    pub fn reproject(self) -> StoreProjectionBuilder {
        StoreProjectionBuilder {
            roots: self.roots,
            sites: Vec::new(),
        }
    }
}

/// The sole minter of a [`StoreProjection`]: roots pushed one at a time, then sites.
/// Every site is resolved — and every refusal read — at [`finish`](Self::finish), once the
/// root table is complete. Nothing partially built escapes.
#[derive(Debug, Default)]
pub struct StoreProjectionBuilder {
    roots: Vec<StoreSchema>,
    sites: Vec<PendingSite>,
}

/// One site as the builder holds it before resolution: named by caller positions, or a
/// typed parked absence keeping the table index-aligned with the image's site table.
#[derive(Debug)]
enum PendingSite {
    Named { root: u16, target: SiteTarget },
    Parked,
}

impl StoreProjectionBuilder {
    /// Append one root in declaration order. A site's root position indexes this order.
    pub fn root(&mut self, schema: StoreSchema) -> &mut Self {
        self.roots.push(schema);
        self
    }

    /// Append one site in image site-index order, naming its root by declaration position.
    /// The target is resolved at [`finish`](Self::finish), once every root is complete: a
    /// site may name a branch, field, group, or index of a root declared after it.
    pub fn site(&mut self, root: u16, target: SiteTarget) -> &mut Self {
        self.sites.push(PendingSite::Named { root, target });
        self
    }

    /// Append one parked site: an image site the flat kernel does not execute. The slot
    /// stays in the table as a typed absence — never as a semantically valid site standing
    /// in for "not executable" — so the table remains index-aligned with the image's sites
    /// while an operation addressing the slot has nothing to resolve.
    pub fn parked_site(&mut self) -> &mut Self {
        self.sites.push(PendingSite::Parked);
        self
    }

    /// Resolve every site against the completed roots and publish the projection, or return
    /// the first refusal. Consuming, so no partially resolved table is observable.
    ///
    /// The root table is also held to the store's number space here: a projection past
    /// [`MAX_STORE_NODES`](super::MAX_STORE_NODES) durable nodes is refused at mint, which is
    /// what lets `number_store`'s counter be total rather than checked at every step of every
    /// store open.
    pub fn finish(self) -> Result<StoreProjection, ProjectionBuildError> {
        let nodes: u64 = self.roots.iter().map(super::root_node_count).sum();
        if nodes > u64::from(super::MAX_STORE_NODES) {
            return Err(ProjectionBuildError::TooManyNodes);
        }
        if self.sites.len() > usize::from(u16::MAX) + 1 {
            return Err(ProjectionBuildError::TooManySites);
        }
        let mut sites = Vec::with_capacity(self.sites.len());
        for pending in &self.sites {
            sites.push(match pending {
                PendingSite::Named { root, target } => {
                    SiteSlot::Resolved(resolve(&self.roots, *root, target)?)
                }
                PendingSite::Parked => SiteSlot::Parked,
            });
        }
        Ok(StoreProjection {
            roots: self.roots,
            sites,
        })
    }
}

/// One slot of the published site table: a site resolved against the roots, or the typed
/// absence a parked image site keeps so later slots stay at their image indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SiteSlot {
    Resolved(Site),
    Parked,
}

/// One checked site: the root it addresses and its resolved target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Site {
    root: RootPos,
    target: CheckedTarget,
}

impl Site {
    /// The site's root by declaration position, resolved against the projection's root
    /// table.
    pub(crate) fn root(&self) -> RootPos {
        self.root
    }

    /// The site's resolved target.
    pub(crate) fn target(&self) -> &CheckedTarget {
        &self.target
    }
}

/// A site target whose every position is resolved against the completed root: the form the
/// resolver consumes, so it performs typed lookups rather than raw slice indexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckedTarget {
    WholePayload,
    FieldLeaf(FieldPos),
    BranchEntry(Box<[BranchPos]>),
    BranchField {
        branch: Box<[BranchPos]>,
        field: FieldPos,
    },
    GroupEntry(GroupPos),
    IndexScan(IndexPos),
    IndexLookup(IndexPos),
}

/// The closed operation-target set the kernel serves, as the caller names it: the root's
/// whole keyed payload, one of the root's field leaves, a keyed branch entry's whole payload
/// or one of its field leaves, one unkeyed group's record, or a managed-index read. It is
/// the physical projection of the verifier's closed `SemanticTarget`.
///
/// Opaque over its kind: a target is caller-named by position and carries no claim that the
/// position exists. It becomes addressable only by passing through
/// [`StoreProjectionBuilder`], which resolves it against the completed root.
///
/// A branch target names its node by a *branch path*: the per-level branch indices from the
/// root down to the addressed branch node, each an index into that level's
/// declaration-ordered branch list (the root's [`StoreSchema::branches`], then each branch's
/// [`BranchSchema::branches`](super::BranchSchema::branches)). A single-element path names a
/// direct child branch; a longer path names a nested branch one level deeper per element.
/// The node's key-path is the root key followed by one key per path element, so a path of
/// length `d` addresses a `(1 + d)`-element key-path `d` levels below the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteTarget(SiteTargetKind);

#[derive(Debug, Clone, PartialEq, Eq)]
enum SiteTargetKind {
    WholePayload,
    FieldLeaf(u16),
    BranchEntry(Box<[u16]>),
    BranchField { branch: Box<[u16]>, field: u16 },
    GroupEntry(u16),
    IndexScan(u16),
    IndexLookup(u16),
}

impl SiteTarget {
    /// The root entry's whole keyed payload.
    pub fn whole_payload() -> Self {
        Self(SiteTargetKind::WholePayload)
    }

    /// One of the root's top-level field leaves, by its position in the root's record.
    pub fn field_leaf(field: u16) -> Self {
        Self(SiteTargetKind::FieldLeaf(field))
    }

    /// The whole payload of the keyed branch entry the branch path descends to.
    pub fn branch_entry(path: impl Into<Box<[u16]>>) -> Self {
        Self(SiteTargetKind::BranchEntry(path.into()))
    }

    /// One field leaf of the keyed branch entry the branch path descends to.
    pub fn branch_field(path: impl Into<Box<[u16]>>, field: u16) -> Self {
        Self(SiteTargetKind::BranchField {
            branch: path.into(),
            field,
        })
    }

    /// The materialized record of one unkeyed group of the root entry. Its whole
    /// read/replace/erase confine to the group's own leaves under the group prefix, disjoint
    /// from the entry's top-level fields, its sibling groups, and its branches (the
    /// group-scoped payload-only law).
    pub fn group_entry(group: u16) -> Self {
        Self(SiteTargetKind::GroupEntry(group))
    }

    /// A managed index's nonunique progressive-prefix scan read, by the index's position in
    /// its root's index table. The read is bounded and yields the next distinct projected
    /// component; it observes only the index cell family and never a source entry.
    pub fn index_scan(index: u16) -> Self {
        Self(SiteTargetKind::IndexScan(index))
    }

    /// A managed index's unique complete-key lookup read, by the index's position in its
    /// root's index table. The read is a single exact probe yielding the one matching source
    /// key tuple or absent.
    pub fn index_lookup(index: u16) -> Self {
        Self(SiteTargetKind::IndexLookup(index))
    }
}

/// Why a site table did not resolve against the completed roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionBuildError {
    /// A site named a root position the projection does not declare.
    UnknownRoot,
    /// A site named a field position the addressed record does not declare.
    UnknownField,
    /// A branch path named a branch position its level does not declare, so the path
    /// descends into nothing.
    UnknownBranch,
    /// A site named a group position the root does not declare.
    UnknownGroup,
    /// A site named a managed-index position the root does not declare.
    UnknownIndex,
    /// A branch site carried an empty branch path. A path names its node one hop per
    /// element, so an empty path names no branch at all — it would alias the root entry
    /// under branch semantics, silently skipping the root's own groups.
    EmptyBranchPath,
    /// The site table holds more slots than a `u16` image site index can name.
    TooManySites,
    /// The root table declares more durable nodes than the store's cell-key number space
    /// ([`MAX_STORE_NODES`](super::MAX_STORE_NODES)) can name.
    TooManyNodes,
}

impl std::fmt::Display for ProjectionBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownRoot => write!(f, "a site names an undeclared durable root"),
            Self::UnknownField => write!(f, "a site names an undeclared field"),
            Self::UnknownBranch => write!(f, "a site names an undeclared branch"),
            Self::UnknownGroup => write!(f, "a site names an undeclared group"),
            Self::UnknownIndex => write!(f, "a site names an undeclared managed index"),
            Self::EmptyBranchPath => write!(f, "a branch site carries an empty branch path"),
            Self::TooManySites => {
                write!(
                    f,
                    "the site table holds more slots than a u16 site index can name"
                )
            }
            Self::TooManyNodes => {
                write!(
                    f,
                    "the store declares more durable nodes than its number space"
                )
            }
        }
    }
}

impl std::error::Error for ProjectionBuildError {}

/// Resolve one caller-named site against the completed root table. Every position is
/// checked here and nowhere else: the resolver downstream holds only typed positions.
fn resolve(
    roots: &[StoreSchema],
    root: u16,
    target: &SiteTarget,
) -> Result<Site, ProjectionBuildError> {
    let root = RootPos::resolve(roots, root).ok_or(ProjectionBuildError::UnknownRoot)?;
    let schema = root.of(roots);
    let target = match &target.0 {
        SiteTargetKind::WholePayload => CheckedTarget::WholePayload,
        SiteTargetKind::FieldLeaf(field) => CheckedTarget::FieldLeaf(
            schema
                .field_pos(*field)
                .ok_or(ProjectionBuildError::UnknownField)?,
        ),
        SiteTargetKind::BranchEntry(path) => {
            CheckedTarget::BranchEntry(resolve_path(schema, path)?)
        }
        SiteTargetKind::BranchField { branch, field } => {
            let path = resolve_path(schema, branch)?;
            CheckedTarget::BranchField {
                field: field_pos_at(schema, &path, *field)
                    .ok_or(ProjectionBuildError::UnknownField)?,
                branch: path,
            }
        }
        SiteTargetKind::GroupEntry(group) => CheckedTarget::GroupEntry(
            schema
                .group_pos(*group)
                .ok_or(ProjectionBuildError::UnknownGroup)?,
        ),
        SiteTargetKind::IndexScan(index) => CheckedTarget::IndexScan(
            schema
                .index_pos(*index)
                .ok_or(ProjectionBuildError::UnknownIndex)?,
        ),
        SiteTargetKind::IndexLookup(index) => CheckedTarget::IndexLookup(
            schema
                .index_pos(*index)
                .ok_or(ProjectionBuildError::UnknownIndex)?,
        ),
    };
    Ok(Site { root, target })
}

/// Resolve a branch path level by level: hop `i` names a branch of level `i`'s
/// declaration-ordered list, and the next level is that branch's own sub-branches. The walk
/// is bounded by the schema's own branch depth, which construction already capped.
fn resolve_path(
    schema: &StoreSchema,
    path: &[u16],
) -> Result<Box<[BranchPos]>, ProjectionBuildError> {
    if path.is_empty() {
        return Err(ProjectionBuildError::EmptyBranchPath);
    }
    let mut resolved = Vec::with_capacity(path.len());
    let mut level = schema.branches();
    for step in path {
        let pos = BranchPos::resolve(level, *step).ok_or(ProjectionBuildError::UnknownBranch)?;
        level = pos.of(level).branches();
        resolved.push(pos);
    }
    Ok(resolved.into_boxed_slice())
}

/// Resolve a field position against the record the branch path addresses: the branch the
/// path descends to, or the root's own record when the path is empty. The walk is total —
/// every hop was resolved against the level it re-enters, in the same order.
fn field_pos_at(schema: &StoreSchema, path: &[BranchPos], field: u16) -> Option<FieldPos> {
    let mut level = schema.branches();
    let mut branch = None;
    for pos in path {
        let next = pos.of(level);
        level = next.branches();
        branch = Some(next);
    }
    match branch {
        Some(branch) => branch.field_pos(field),
        None => schema.field_pos(field),
    }
}

#[cfg(test)]
mod tests {
    use super::super::schema::StoreSchemaBuilder;
    use super::super::{MAX_STORE_NODES, number_store, root_node_count};
    use super::{ProjectionBuildError, StoreProjection};
    use crate::codec::value::ScalarKind;

    /// A two-root fixture exercising every numbered node kind: fields, a group with its own
    /// fields, and a nested branch tree.
    fn two_roots() -> StoreProjection {
        let mut first = StoreSchemaBuilder::root("patients", vec![ScalarKind::Str]);
        first.scalar_field("name", ScalarKind::Str, true);
        first.scalar_field("age", ScalarKind::Int, false);
        first.open_group("vitals");
        first.scalar_field("pulse", ScalarKind::Int, false);
        first.close_group();
        first.open_branch("visits", vec![ScalarKind::Int]);
        first.scalar_field("note", ScalarKind::Str, false);
        first.open_branch("charges", vec![ScalarKind::Int]);
        first.scalar_field("amount", ScalarKind::Int, true);
        first.close_branch();
        first.close_branch();
        let first = first.finish().expect("the patients schema builds");

        let mut second = StoreSchemaBuilder::root("wards", vec![ScalarKind::Str]);
        second.scalar_field("floor", ScalarKind::Int, true);
        let second = second.finish().expect("the wards schema builds");

        let mut projection = StoreProjection::builder();
        projection.root(first).root(second);
        projection.finish().expect("two plain roots project")
    }

    /// An empty branch path names no branch at all: resolving it would alias the root
    /// entry under branch semantics — replace and erase would skip the root's own groups —
    /// so the projection refuses it at mint for both branch target forms.
    #[test]
    fn an_empty_branch_path_is_refused_at_mint() {
        let mut builder = StoreSchemaBuilder::root("counters", vec![ScalarKind::Str]);
        builder.scalar_field("value", ScalarKind::Int, false);
        let schema = builder.finish().expect("a flat schema builds");

        for target in [
            super::SiteTarget::branch_entry(Vec::new()),
            super::SiteTarget::branch_field(Vec::new(), 0),
        ] {
            let mut projection = StoreProjection::builder();
            projection.root(schema.clone());
            projection.site(0, target);
            assert_eq!(
                projection.finish().unwrap_err(),
                ProjectionBuildError::EmptyBranchPath,
            );
        }
    }

    /// N/N+1 on the site table's `u16` index space: the last nameable slot fills, and one
    /// more is refused rather than published beyond what an image site index can address.
    #[test]
    fn the_site_table_refuses_past_its_u16_index_space() {
        let build = |slots: u32| {
            let mut builder = StoreSchemaBuilder::root("counters", vec![ScalarKind::Str]);
            builder.scalar_field("value", ScalarKind::Int, false);
            let schema = builder.finish().expect("a flat schema builds");
            let mut projection = StoreProjection::builder();
            projection.root(schema);
            for _ in 0..slots {
                projection.parked_site();
            }
            projection.finish()
        };
        assert!(build(1 << 16).is_ok());
        assert_eq!(
            build((1 << 16) + 1).unwrap_err(),
            ProjectionBuildError::TooManySites,
        );
    }

    /// A parked image site stays a typed absence at its own index: the slots around it keep
    /// their image positions, and nothing about the parked slot resolves to a real target.
    #[test]
    fn a_parked_site_is_a_typed_absence_at_its_image_index() {
        let mut builder = StoreSchemaBuilder::root("counters", vec![ScalarKind::Str]);
        builder.scalar_field("value", ScalarKind::Int, false);
        let schema = builder.finish().expect("a flat schema builds");

        let mut projection = StoreProjection::builder();
        projection.root(schema);
        projection.site(0, super::SiteTarget::whole_payload());
        projection.parked_site();
        projection.site(0, super::SiteTarget::field_leaf(0));
        let projection = projection.finish().expect("the named sites resolve");

        let sites = projection.sites();
        assert_eq!(sites.len(), 3);
        assert!(matches!(sites[0], super::SiteSlot::Resolved(_)));
        assert!(matches!(sites[1], super::SiteSlot::Parked));
        assert!(matches!(sites[2], super::SiteSlot::Resolved(_)));
    }

    /// The split pre-order, pinned as a known answer: root, its fields, each group (the
    /// group node then its fields), each branch (the branch node, its fields, then its
    /// sub-branches), then the next root continuing the same store-wide counter. This is
    /// the order the store persisted before the walk was checked, so the pin is the
    /// traversal-rewrite regression gate.
    #[test]
    fn numbering_is_the_split_pre_order_known_answer() {
        let numbering = number_store(&two_roots());

        let patients = &numbering[0];
        assert_eq!(patients.root(), 0);
        assert_eq!(patients.fields(), &[1, 2]);
        assert_eq!(patients.groups()[0].number(), 3);
        assert_eq!(patients.groups()[0].fields(), &[4]);
        let visits = &patients.branches()[0];
        assert_eq!(visits.number(), 5);
        assert_eq!(visits.fields(), &[6]);
        let charges = &visits.branches()[0];
        assert_eq!(charges.number(), 7);
        assert_eq!(charges.fields(), &[8]);

        let wards = &numbering[1];
        assert_eq!(wards.root(), 9);
        assert_eq!(wards.fields(), &[10]);
    }

    /// The refusal's count and the walk's output are two readings of one definition of
    /// "node"; holding them equal keeps the mint-time bound honest about what the counter
    /// later numbers.
    #[test]
    fn the_node_count_equals_the_walk_it_bounds() {
        let projection = two_roots();
        let numbering = number_store(&projection);
        let numbered: u64 = numbering
            .iter()
            .map(|root| {
                let mut nodes = 1 + root.fields().len() as u64;
                for group in root.groups() {
                    nodes += 1 + group.fields().len() as u64;
                }
                let mut stack: Vec<_> = root.branches().iter().collect();
                while let Some(branch) = stack.pop() {
                    nodes += 1 + branch.fields().len() as u64;
                    stack.extend(branch.branches());
                }
                nodes
            })
            .sum();
        let counted: u64 = projection.roots().iter().map(root_node_count).sum();
        assert_eq!(counted, numbered);
    }

    /// One root with the widest field table the number space admits beside it: exactly at
    /// the bound the projection publishes, one field past it the projection refuses.
    #[test]
    fn the_number_space_bound_refuses_at_mint_and_admits_at_the_edge() {
        let build = |fields: u32| {
            let mut builder = StoreSchemaBuilder::root("wide", vec![ScalarKind::Str]);
            for i in 0..fields {
                builder.scalar_field(format!("f{i}"), ScalarKind::Int, false);
            }
            let schema = builder.finish().expect("a flat wide schema builds");
            let mut projection = StoreProjection::builder();
            projection.root(schema);
            projection.finish()
        };

        assert!(build(MAX_STORE_NODES - 1).is_ok());
        assert_eq!(
            build(MAX_STORE_NODES).unwrap_err(),
            ProjectionBuildError::TooManyNodes,
        );
    }
}
