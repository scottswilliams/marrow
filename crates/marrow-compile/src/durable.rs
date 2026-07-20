//! The durable graph registry (design §B/§C).
//!
//! The durable graph admits one or more `store` roots, each over its own resource
//! record and in declaration order (a root's DURABLE-table index is its RootId). A root
//! is a *singleton* (`store ^root: Record`, no key) or a
//! *keyed tuple* (`store ^root(k1: K1, k2: K2): Record`, one or more ordered
//! orderable durable-key columns). A resource's durable shape is a **member tree**:
//! its top-level stored fields, plus any static `group` field-path namespaces and
//! keyed `branch` placements, each of which recursively holds its own members. A
//! group is an unkeyed pathing construct (a `Group` ledger identity); a branch is a
//! keyed subtree — a distinct graph node with its own placement id and key tuple,
//! just like a root. Every admitted node has a complete ledger identity and a
//! contribution to the durable-contract identity the verifier independently
//! re-encodes.
//!
//! The executable durable subset the single-root kernel can serve at this stage is a flat
//! keyed root: one or more key columns, whose top-level fields are each a scalar or a
//! widened value (`struct`/`enum`/`Option`, framed inline), whose root-level `group`
//! members hold only such fields, and whose keyed `branch` placements are field-only
//! (nested to any depth). A singleton (keyless) root, a root whose resource declares a
//! nominal-typed field, a group nested in a branch or in another group, completes its
//! identity and verifies but has no executable operation sites — an operation over one is a
//! precise typed `check.unsupported` rejection at lowering ("not yet executable"). Those
//! shapes run when their lanes land. This module validates the declaration, adds the root,
//! its member tree, and — for the executable subset — its operation sites to the draft, and
//! exposes the resolved sites the function lowerer emits against.

use std::collections::BTreeSet;

use marrow_codes::Code;
use marrow_image::{
    DurableEnumMemberShape, DurableIndexComponent, DurableIndexShape, DurableMemberDef,
    DurableValueShape, FieldDef, ImageDraft, ImageType, KeyColumn, LedgerIdBytes, RecordTypeDef,
    RootDef, RootIdentity, Scalar, SemanticPath, SemanticStep, SemanticStepKind, SiteDef, bounds,
};
use marrow_project::{IdentityKind, IdentityLedger};
use marrow_syntax::{
    FieldDecl, GroupDecl, IndexDecl, KeyParam, ResourceDecl, ResourceMember, SourceSpan, StoreDecl,
    TypeExpr,
};

use crate::diag::{IdentityGap, SourceDiagnostic};
use crate::scalar::ScalarType;
use crate::types::{GArg, GenericInvariant, TypeMetadataSession, TypeRegistry};

/// The application's fixed ledger anchor path: one local application per
/// project, so the anchor is the project itself.
const APPLICATION_ANCHOR_PATH: &str = ".";

/// The most managed indexes one `store` root may declare. The checker owns this product
/// limit; it sits well below the image's structural `MAX_INDEXES` decode bound (32), which
/// stays as headroom. `8` keeps a root's per-write index maintenance bounded and small while
/// comfortably covering the identity-plus-a-few-secondary-orderings shape narrow indexes are
/// for.
const MAX_STORE_INDEXES: usize = 8;

/// One top-level stored field as an index-projection candidate: its source name, its
/// ledger id, and the base scalar of its stored value when that value is an orderable
/// durable-key scalar.
struct IndexFieldLeaf {
    name: String,
    id: LedgerIdBytes,
    scalar: Option<ScalarType>,
}

/// One admitted component of a managed-index projection. Its durable identity
/// reference and lowerer-facing scalar travel together so admission cannot produce a
/// component whose projection type is missing.
#[derive(Clone, Copy)]
struct ResolvedIndexComponent {
    component: DurableIndexComponent,
    scalar: ScalarType,
}

/// A resolved managed index: its image shape (for the durable identity), its source
/// name, and its projected components' scalar types in order. The projection lets the
/// lowerer type-check a source index-read operand list; the site is attached later.
struct BuiltIndex {
    shape: DurableIndexShape,
    name: String,
    projection: Vec<ScalarType>,
}

/// A managed index as the lowerer reads it: its source name, unique flag, its read
/// site (a scan site for a nonunique index, a lookup site for a unique one), and its
/// projected components' scalar types in projection order. A nonunique projection ends
/// with the root's identity keys; the scan holds the leading field components as a
/// prefix and yields the identity suffix as the source-root `Id(^root)`.
pub(crate) struct DurableIndex {
    pub(crate) name: String,
    pub(crate) unique: bool,
    pub(crate) site: u16,
    pub(crate) projection: Vec<ScalarType>,
}

/// The compiler scalar carried by an orderable durable-key stored shape. This stored
/// shape is the sole index-eligibility classifier: a nominal has already erased to
/// `int`, while a dense struct, closed enum, duration, or other non-key value returns
/// `None`.
fn orderable_key_scalar(value: &DurableValueShape) -> Option<ScalarType> {
    match value {
        DurableValueShape::Scalar(Scalar::Int) => Some(ScalarType::Int),
        DurableValueShape::Scalar(Scalar::Text) => Some(ScalarType::Text),
        DurableValueShape::Scalar(Scalar::Bool) => Some(ScalarType::Bool),
        DurableValueShape::Scalar(Scalar::Bytes) => Some(ScalarType::Bytes),
        DurableValueShape::Scalar(Scalar::Date) => Some(ScalarType::Date),
        DurableValueShape::Scalar(Scalar::Instant) => Some(ScalarType::Instant),
        DurableValueShape::Scalar(Scalar::Duration)
        | DurableValueShape::Struct(_)
        | DurableValueShape::Enum { .. } => None,
    }
}

/// One resolved durable field site.
pub(crate) struct DurableField {
    pub(crate) name: String,
    pub(crate) site: u16,
    /// The field's resolved value type: a scalar, or a widened composite (a dense
    /// `struct`, or a closed `enum`/`Option`/`Result`). The lowerer builds the read
    /// result and written-value type from it.
    pub(crate) ty: GArg,
    pub(crate) required: bool,
}

/// One resolved scalar field of an executable branch entry: its source name, value
/// scalar, required flag, and field-leaf operation site. The whole-payload
/// create/replace flows through the branch's materialized record; `site` is the
/// field-exact leaf a `^root(k).branch(bk).field` read or write addresses directly, one
/// level below the root.
pub(crate) struct DurableBranchField {
    pub(crate) name: String,
    pub(crate) scalar: ScalarType,
    pub(crate) required: bool,
    pub(crate) site: u16,
}

/// One scalar/widened leaf of an executable root-level `group`: its source name, value
/// type (a scalar or a widened composite), and required flag. A leaf is not addressed by
/// a durable site of its own — a group-leaf access reads or rewrites the whole group — so
/// it carries no site, only the shape a group-leaf read projects and a group-leaf write
/// stores into the group record's slot.
pub(crate) struct DurableGroupLeaf {
    pub(crate) name: String,
    pub(crate) ty: GArg,
    pub(crate) required: bool,
}

/// One executable root-level unkeyed `group` of a flat-executable root: a value unit of
/// the root entry addressed by the root's own key-path (a group is markerless — its
/// presence is the entry's presence). Its whole read/replace/erase address the
/// `GroupEntry` site `entry_site` over the group node; a group-leaf access
/// `^root(k).group.leaf` is a whole-group read-modify-write over the materialized group
/// `record`, so a leaf never has a durable site of its own.
pub(crate) struct DurableGroup {
    pub(crate) name: String,
    pub(crate) record: marrow_image::TypeId,
    pub(crate) entry_site: u16,
    pub(crate) fields: Vec<DurableGroupLeaf>,
}

impl DurableGroup {
    /// The declaration-order slot index and descriptor of the group leaf `name` — the
    /// slot into the group's materialized record a leaf read projects and a leaf write
    /// rewrites, so a group-leaf operation addresses the same slot the record types.
    pub(crate) fn field_index(&self, name: &str) -> Option<(u16, &DurableGroupLeaf)> {
        self.fields
            .iter()
            .enumerate()
            .find(|(_, field)| field.name == name)
            .map(|(index, field)| (index as u16, field))
    }
}

/// One executable keyed `branch` of a flat-executable root: a scalar-field keyed
/// scalar-field subtree one or more levels below the root, carrying its own nested
/// branches recursively. Its whole-entry operations address the key-path
/// `[root_key, branch_key, …]` through `entry_site`, and its constructor
/// `Resource.branch.…(field: value, …)` builds `record` from `fields` in declaration
/// order.
pub(crate) struct DurableBranch {
    pub(crate) name: String,
    /// The branch's ordered key columns (one or more), the whole composite branch key.
    pub(crate) key: Vec<ScalarType>,
    pub(crate) record: marrow_image::TypeId,
    pub(crate) entry_site: u16,
    pub(crate) fields: Vec<DurableBranchField>,
    pub(crate) branches: Vec<DurableBranch>,
}

impl DurableBranch {
    pub(crate) fn field(&self, name: &str) -> Option<&DurableBranchField> {
        self.fields.iter().find(|field| field.name == name)
    }

    /// The nested branch declared with the simple name `name`, if any.
    pub(crate) fn branch(&self, name: &str) -> Option<&DurableBranch> {
        self.branches.iter().find(|branch| branch.name == name)
    }

    /// The declaration-order index and descriptor of the branch field `name` — the
    /// index into the branch's materialized record slots, so a field read of a
    /// materialized branch entry addresses the same slot the record types.
    pub(crate) fn field_index(&self, name: &str) -> Option<(u16, &DurableBranchField)> {
        self.fields
            .iter()
            .enumerate()
            .find(|(_, field)| field.name == name)
            .map(|(index, field)| (index as u16, field))
    }
}

/// One executable durable root, its operation sites, its executable root-level groups,
/// and its executable branches. A keyed root (any key arity) whose top-level fields are
/// scalars or widened values, whose root-level groups hold only such fields, and whose
/// only nested placements are field-only keyed branches reaches this form; its key columns
/// back the kernel-serviceable read/write path, each group is a value unit of the root
/// entry, and each branch adds its own key tuple below it.
pub(crate) struct DurableRoot {
    pub(crate) name: String,
    /// This root's DURABLE-table index (its declaration-ordered RootId) — the discriminant
    /// an entry identity `Id(^root)` carries, so two identities over different roots are
    /// distinct values and an identity addressed to the wrong root is a type error.
    pub(crate) root_id: u16,
    /// The resource (product) name backing this store — the head of a branch's
    /// qualified constructor path `Resource.branch(…)`.
    pub(crate) resource: String,
    /// The root's ordered key columns (one or more), the whole composite root key.
    pub(crate) key: Vec<ScalarType>,
    pub(crate) record: marrow_image::TypeId,
    pub(crate) entry_site: u16,
    pub(crate) fields: Vec<DurableField>,
    pub(crate) groups: Vec<DurableGroup>,
    pub(crate) branches: Vec<DurableBranch>,
    pub(crate) indexes: Vec<DurableIndex>,
}

impl DurableRoot {
    pub(crate) fn field(&self, name: &str) -> Option<&DurableField> {
        self.fields.iter().find(|field| field.name == name)
    }

    /// The executable root-level group declared with the simple name `name`, if any —
    /// the owner a group whole access `^root(k).group` or a group-leaf access
    /// `^root(k).group.leaf` resolves against.
    pub(crate) fn group(&self, name: &str) -> Option<&DurableGroup> {
        self.groups.iter().find(|group| group.name == name)
    }

    /// The executable branch declared with the simple name `name`, if any.
    pub(crate) fn branch(&self, name: &str) -> Option<&DurableBranch> {
        self.branches.iter().find(|branch| branch.name == name)
    }

    /// The managed index declared with the simple name `name`, if any — the owner a
    /// source index read (`^root.name[…]`) resolves against.
    pub(crate) fn index(&self, name: &str) -> Option<&DurableIndex> {
        self.indexes.iter().find(|index| index.name == name)
    }
}

/// The durable registry: every admitted `store` root, in declaration order. `roots`
/// holds the flat keyed roots the kernel can serve, addressed by name; `declared_roots`
/// names every admitted root (executable or parked — a singleton, a composite, or one
/// bearing groups or branches), so a durable operation over a not-yet-executable shape
/// reports precisely rather than as "no store". A root's index in the draft's DURABLE
/// table is its declaration order (RootId), so the two lists stay declaration-ordered.
#[derive(Default)]
pub(crate) struct DurableRegistry {
    roots: Vec<DurableRoot>,
    declared_roots: Vec<String>,
}

impl DurableRegistry {
    /// The executable flat keyed root declared with the placement name `name`, if any —
    /// the owner an entry address `^name[…]` resolves against.
    pub(crate) fn root_by_name(&self, name: &str) -> Option<&DurableRoot> {
        self.roots.iter().find(|root| root.name == name)
    }

    /// The executable flat keyed root whose backing resource is `resource`, if any — the
    /// owner of a branch constructor `Resource.branch(…)`. Each store binds one resource,
    /// so at most one executable root matches a resource name.
    pub(crate) fn root_by_resource(&self, resource: &str) -> Option<&DurableRoot> {
        self.roots.iter().find(|root| root.resource == resource)
    }

    /// The executable flat keyed root whose whole-payload entry site is `entry_site`, if
    /// any — the owner a source-local root `place` resolves its fields against. Each root
    /// has a distinct entry site, so at most one matches.
    pub(crate) fn root_by_entry_site(&self, entry_site: u16) -> Option<&DurableRoot> {
        self.roots.iter().find(|root| root.entry_site == entry_site)
    }

    /// The executable branch whose materialized entry record is the image type `ty`, if
    /// any — the owner that resolves a field of a materialized branch entry value read
    /// through `if const n = ^root(k)….branch(bk)`. Searches every executable root's whole
    /// recursive branch tree; each branch has its own materialized record type, so at most
    /// one branch matches.
    pub(crate) fn branch_by_record(&self, ty: marrow_image::TypeId) -> Option<&DurableBranch> {
        fn find(branches: &[DurableBranch], ty: marrow_image::TypeId) -> Option<&DurableBranch> {
            for branch in branches {
                if branch.record == ty {
                    return Some(branch);
                }
                if let Some(found) = find(&branch.branches, ty) {
                    return Some(found);
                }
            }
            None
        }
        self.roots.iter().find_map(|root| find(&root.branches, ty))
    }

    /// The name of a declared root of placement `name` that the kernel cannot yet serve (a
    /// singleton root, a resource declaring a nominal-typed field, or a group nested in a
    /// branch or another group). `Some` exactly when a root of that name is declared but
    /// not executable, so the lowerer can distinguish a not-yet-executable operation over a
    /// named root from an unknown name.
    pub(crate) fn not_yet_executable_root_named(&self, name: &str) -> Option<&str> {
        if self.root_by_name(name).is_some() {
            return None;
        }
        self.declared_roots
            .iter()
            .find(|declared| declared.as_str() == name)
            .map(String::as_str)
    }

    /// Build the registry from the project's store declarations, adding each admitted
    /// root and its complete ledger identity block to the draft in declaration order (so
    /// a root's DURABLE-table index is its RootId). A store whose placement name repeats
    /// an earlier one is a precise `check.type` rejection and does not enter the draft;
    /// an index, a missing or mismatched resource, a key column outside the closed
    /// orderable durable-key set, or a key tuple past the column bound reject that one
    /// store — and so does a durable graph whose identity is incomplete: every durable
    /// declaration (the application, the root placement, its product, each key column,
    /// each stored field, each group namespace, and each nested branch placement and key
    /// column) must have a live row in the committed `marrow.ids` ledger, or the
    /// declaration fails precisely with `check.durable_identity`. A store that fails
    /// validation contributes only its diagnostic; the other stores' roots stand, so one
    /// store's gap never erases the whole registry. The compiler only *reads* the ledger;
    /// minting lives in the `marrow run` convenience action (and in the accepted apply
    /// action when it lands).
    pub(crate) fn build(
        draft: &mut ImageDraft,
        records: &TypeRegistry,
        resources: &[(String, &ResourceDecl)],
        stores: &[(String, &StoreDecl)],
        ledger: Option<&IdentityLedger>,
        diagnostics: &mut Vec<SourceDiagnostic>,
    ) -> Result<Self, GenericInvariant> {
        if stores.is_empty() {
            return Ok(Self::default());
        }
        records.with_metadata_session(|metadata| {
            let mut registry = Self::default();
            let mut type_metadata = DurableTypeMetadata { records, metadata };
            for (file, store) in stores {
                // A repeated placement name has no unambiguous address and cannot key a second
                // DURABLE-table row; reject it and keep the first declaration.
                if registry.declared_roots.contains(&store.root.root) {
                    diagnostics.push(SourceDiagnostic::at(
                        Code::CheckType.as_str(),
                        file,
                        store.root.span,
                        format!(
                            "store root `^{}` is declared more than once; each store root has a \
                             distinct name",
                            store.root.root
                        ),
                    ));
                    continue;
                }
                if let Some(built) = build_one(
                    draft,
                    &mut type_metadata,
                    resources,
                    file,
                    store,
                    ledger,
                    diagnostics,
                )? {
                    registry.declared_roots.push(built.name);
                    if let Some(root) = built.executable {
                        registry.roots.push(root);
                    }
                }
            }
            Ok(registry)
        })
    }
}

/// One store declaration's build outcome: the placement name that entered the draft and,
/// when the root is a flat kernel-serviceable shape, its executable descriptor.
struct BuiltRoot {
    name: String,
    executable: Option<DurableRoot>,
}

/// The immutable type owner and its one operation-local validation session.
/// Durable construction passes them together so no store can silently open a
/// second metadata directory while the registry remains unchanged.
struct DurableTypeMetadata<'registry, 'session> {
    records: &'registry TypeRegistry,
    metadata: &'session mut TypeMetadataSession<'registry>,
}

/// Resolve, validate, and commit one `store` declaration into the draft, returning its
/// build outcome or `None` when the store fails validation (its diagnostic is pushed and
/// no root, site, or application identity is committed for it, so a failing store cannot
/// corrupt an already-appended root; `build_extras` may append record types before the
/// gate, which is harmless — the pushed diagnostic fails compilation). The heavy resolution
/// runs against a local [`IdentityResolver`] and the completeness gate below precedes
/// every root/site/identity commit, so the draft is touched only once the store is known
/// admissible.
fn build_one(
    draft: &mut ImageDraft,
    type_metadata: &mut DurableTypeMetadata<'_, '_>,
    resources: &[(String, &ResourceDecl)],
    file: &str,
    store: &StoreDecl,
    ledger: Option<&IdentityLedger>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Result<Option<BuiltRoot>, GenericInvariant> {
    let records = type_metadata.records;
    let metadata = &mut *type_metadata.metadata;
    if store.root.keys.len() > bounds::MAX_KEY_COLUMNS {
        diagnostics.push(resource_limit(
            file,
            store.root.span,
            format!(
                "a store root key tuple has {} columns; the fixed limit is {}",
                store.root.keys.len(),
                bounds::MAX_KEY_COLUMNS
            ),
        ));
        return Ok(None);
    }
    // Resolve each root key column's scalar in declared tuple order. A singleton
    // root has no columns.
    let Some(key_scalars) = resolve_key_scalars(
        file,
        store.root.span,
        &store.root.keys,
        records,
        diagnostics,
    ) else {
        return Ok(None);
    };
    let Some(record) = records.by_name(&store.resource) else {
        diagnostics.push(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            file,
            store.span,
            format!("`{}` is not a resource in this project", store.resource),
        ));
        return Ok(None);
    };
    let Some((_, resource)) = resources
        .iter()
        .find(|(_, decl)| decl.name == store.resource)
    else {
        return Ok(None);
    };

    // Compiler-owned enum readiness is validated before the first ledger lookup.
    // This keeps a malformed Ready body out of both contextual Unsupported
    // diagnostics and durable identity resolution.
    metadata.validate_durable_value_metadata(
        record.fields.iter().map(|field| field.ty).chain(
            record
                .groups
                .iter()
                .flat_map(|group| group.fields.iter().map(|field| field.ty)),
        ),
    )?;

    // Resolve the durable graph's ledger identities. The application, the root
    // placement, its product, and each root key column anchor first; then the
    // resource's member tree (top-level fields, groups, and branches) anchors as
    // it is walked. A missing or retired anchor is a precise typed diagnostic
    // carrying the `(kind, path)` gap the mint action consumes.
    let mut resolver = IdentityResolver::new(file, store.span, ledger, diagnostics);
    let application = resolver.resolve(IdentityKind::Application, APPLICATION_ANCHOR_PATH);
    let placement = resolver.resolve(IdentityKind::Root, &store.root.root);
    let product = resolver.resolve(IdentityKind::Product, &store.resource);
    let key_ids: Vec<LedgerIdBytes> = store
        .root
        .keys
        .iter()
        .map(|key_param| {
            resolver.resolve(
                IdentityKind::Key,
                &format!("{}.{}", store.root.root, key_param.name),
            )
        })
        .collect();

    // The resource's member tree, in canonical order: its top-level fields
    // (aligned with the materialized record), then its static `group`
    // namespaces, then its keyed `branch` placements — each group and branch
    // recursively holding its own members. A top-level field's value shape is
    // drawn from the closed acyclic durable value set (a nominal scalar, a dense
    // struct, a closed enum, or an `Option` of one), the field anchoring the
    // ledger id while nested product leaves are shape bytes and each durable-
    // reachable enum contributes its own sum/member identities. `has_extras`
    // records whether the resource declares any group or branch.
    let mut members: Vec<DurableMemberDef> = record
        .fields
        .iter()
        .map(|field| DurableMemberDef::Field {
            id: resolver.resolve(
                IdentityKind::Field,
                &format!("{}.{}", store.resource, field.name),
            ),
            required: field.required,
            value: resolver.build_value_shape(records, metadata, field.ty, 1),
        })
        .collect();
    let groups_and_branches =
        resolver.build_extras(draft, records, &resource.members, &store.resource);

    if let Some(invariant) = resolver.invariant {
        return Err(invariant);
    }

    // Resolve the root's managed indexes before appending the group/branch members
    // (an index projects only the root's identity keys and top-level fields, so it
    // resolves against exactly those leaves). `members[0..record.fields.len()]` is
    // the top-level field member set, in record order, so each field's ledger id
    // and value shape is read from it. An index admission violation is a precise
    // `check.type` diagnostic that also marks the graph incomplete, so a rejected
    // index discards the whole durable graph rather than emitting a partial one.
    let key_entries: Vec<(String, LedgerIdBytes, ScalarType)> = store
        .root
        .keys
        .iter()
        .zip(&key_ids)
        .zip(&key_scalars)
        .map(|((key_param, id), scalar)| (key_param.name.clone(), *id, *scalar))
        .collect();
    let field_entries: Vec<IndexFieldLeaf> = record
        .fields
        .iter()
        .zip(&members)
        .map(|(field, member)| {
            let (id, value) = match member {
                DurableMemberDef::Field { id, value, .. } => (*id, value),
                _ => unreachable!("the first members are the record's top-level fields"),
            };
            IndexFieldLeaf {
                name: field.name.clone(),
                id,
                scalar: orderable_key_scalar(value),
            }
        })
        .collect();
    let built_indexes = resolver.build_indexes(
        &store.root.root,
        &key_entries,
        &field_entries,
        &store.indexes,
    );

    members.extend(groups_and_branches);

    // Every identity must resolve before the graph enters the image; a single
    // gap already reported precisely leaves the durable graph absent, so an
    // operation over it is not additionally mislabelled "not yet executable"
    // (the identity gap is the diagnosis, whatever the shape).
    if !resolver.complete {
        return Ok(None);
    }
    draft.set_application_identity(application);
    let key_columns: Vec<KeyColumn> = key_scalars
        .iter()
        .zip(&key_ids)
        .map(|(scalar, id)| KeyColumn {
            scalar: scalar.image(),
            id: *id,
        })
        .collect();

    // Emit the complete operation-site set for the whole durable graph now: one
    // whole-payload site per keyed placement (this root and every nested
    // `branch`) and one field-leaf site per stored field (top-level, group-scoped,
    // and branch-scoped, including a widened-field leaf). A site names its target
    // node by the node's semantic path — the chain of kind-tagged ledger ids from
    // the application down — so it follows the graph's ledger ids. The verifier
    // re-derives every site from its own reconstructed node set, so this path is a
    // producer claim, not a trusted address: a nested site completes its identity
    // and seals even though the flat-root kernel cannot execute over it yet. Sites
    // are emitted from the graph, not per operation, so the site table scales with
    // the graph rather than with operation count. The flat executable root's entry
    // and top-level-field sites are captured here for the lowerer.
    let root_steps = vec![
        SemanticStep::new(SemanticStepKind::Application, application),
        SemanticStep::new(SemanticStepKind::Placement, placement),
    ];
    let entry_site = draft
        .add_site(SiteDef::whole_payload(SemanticPath::from_steps(
            root_steps.clone(),
        )))
        .index();
    let (top_field_sites, top_groups, top_branches) =
        emit_root_member_sites(draft, &root_steps, &members);
    // One read site per managed index: a nonunique index is a progressive-prefix
    // scan, a unique index a complete-key exact lookup. There is deliberately no
    // index-write site — maintenance is compiler-owned. Every index site seals as
    // parked (an index node is never a flat-executable node); runtime traversal and
    // lookup land at E05.
    let mut lowered_indexes: Vec<DurableIndex> = Vec::with_capacity(built_indexes.len());
    for built in &built_indexes {
        let mut steps = root_steps.clone();
        steps.push(SemanticStep::new(SemanticStepKind::Index, built.shape.id));
        let path = SemanticPath::from_steps(steps);
        let site = if built.shape.unique {
            SiteDef::index_lookup(path)
        } else {
            SiteDef::index_scan(path)
        };
        let site_index = draft.add_site(site).index();
        lowered_indexes.push(DurableIndex {
            name: built.name.clone(),
            unique: built.shape.unique,
            site: site_index,
            projection: built.projection.clone(),
        });
    }
    let indexes: Vec<DurableIndexShape> =
        built_indexes.into_iter().map(|built| built.shape).collect();

    // Decide executability and capture the executable branch descriptors while the
    // member tree (which carries each branch's materialized record type) is still in
    // hand — it moves into the `RootDef` below.
    //
    // Executable durable operations exist for the flat keyed root whose top-level fields
    // are each a scalar or a widened composite (a dense struct, or a closed
    // `enum`/`Option`/`Result` — framed inline in the field cell by the durable value
    // codec), together with its root-level groups of such fields and its field-only keyed
    // branches nested to any depth — the shape the kernel serves. A singleton (keyless)
    // root, a nominal field, or a group nested in a branch or another group parks
    // (severed until its lane lands): it carries its identity and full site set, but the
    // lowerer reports any operation over it as not yet executable. Composite root keys and
    // keyed branches (including composite-keyed) are executable for whole/field sites; a
    // root-level group no longer parks, mirroring the verifier's independent
    // `member_flat_at_root`.
    // `record.fields` (the registry record) carries only the top-level value fields;
    // its unkeyed groups live in `record.groups`, so a group value never appears here.
    let all_fields_executable = record
        .fields
        .iter()
        .all(|f| matches!(f.ty, GArg::Scalar(_) | GArg::Struct(_) | GArg::Enum(_)));
    // A keyed root of executable fields with root-level scalar/widened-field groups and
    // only field-only branches is executable, at any key arity (one or more columns); a
    // singleton root (no key columns) parks. `member_flat_at_root` admits a root-level
    // group of storable-value fields while `member_keeps_root_flat` (the branch-member
    // predicate) keeps a group parked below the root, so a group in a branch or another
    // group never makes the root flat — mirroring the verifier's `member_flat_at_root`.
    let keyed = !key_scalars.is_empty();
    let members_flat = members.iter().all(member_flat_at_root);
    let executable = keyed && all_fields_executable && members_flat;
    let (branches, groups) = if executable {
        (
            build_executable_branches(records, resource, &top_branches),
            build_executable_groups(&record.groups, &top_groups),
        )
    } else {
        (Vec::new(), Vec::new())
    };

    let root_name = draft.intern_string(&store.root.root);
    let root_id = draft.add_root(RootDef {
        name: root_name,
        keys: key_columns,
        record: record.type_id,
        identity: RootIdentity {
            placement,
            product,
            members,
            indexes,
        },
    });

    if !executable {
        return Ok(Some(BuiltRoot {
            name: store.root.root.clone(),
            executable: None,
        }));
    }
    // A flat root's top-level fields map positionally to `top_field_sites`, so
    // `top_field_sites[i]` is the field-leaf site of `record.fields[i]` (both in
    // member/record order). Each field carries its resolved value type (a scalar or a
    // widened composite), from which the lowerer builds the read/written value type.
    let fields = record
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| DurableField {
            name: field.name.clone(),
            site: top_field_sites[index],
            ty: field.ty,
            required: field.required,
        })
        .collect();

    Ok(Some(BuiltRoot {
        name: store.root.root.clone(),
        executable: Some(DurableRoot {
            name: store.root.root.clone(),
            root_id,
            resource: store.resource.clone(),
            key: key_scalars.clone(),
            record: record.type_id,
            entry_site,
            fields,
            groups,
            branches,
            indexes: lowered_indexes,
        }),
    }))
}

/// Resolve each key column's scalar in declared tuple order, rejecting a key type
/// outside the closed orderable durable-key set. `None` (with a diagnostic) if any
/// column is not a supported key scalar; a singleton placement has no columns and
/// yields an empty vector. Shared by root and branch key tuples.
fn resolve_key_scalars(
    file: &str,
    span: SourceSpan,
    keys: &[KeyParam],
    records: &TypeRegistry,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Option<Vec<ScalarType>> {
    let mut scalars = Vec::with_capacity(keys.len());
    for key_param in keys {
        let Some(key) = scalar_of(&records.expand(&key_param.ty)) else {
            diagnostics.push(unsupported(file, span, "this key type"));
            return None;
        };
        // The closed orderable durable-key scalar set (frozen at C04): int, string,
        // bool, bytes, date, and instant. `duration` is a span, not an identity, so
        // it is not a durable key.
        if !matches!(
            key,
            ScalarType::Int
                | ScalarType::Text
                | ScalarType::Bool
                | ScalarType::Bytes
                | ScalarType::Date
                | ScalarType::Instant
        ) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                span,
                "a durable key column must be an orderable durable-key scalar (int, string, bool, bytes, date, or instant)"
                    .to_string(),
            ));
            return None;
        }
        scalars.push(key);
    }
    Some(scalars)
}

/// Resolves durable `(kind, path)` anchors against the committed ledger, pushing a
/// precise `check.durable_identity` diagnostic for each missing or retired anchor,
/// and building the group/branch member tree. `complete` stays true only while
/// every anchor resolved; the caller discards the graph when it is false, so an id
/// resolved to a placeholder on a gap never reaches the image.
struct IdentityResolver<'a> {
    file: &'a str,
    span: SourceSpan,
    ledger: Option<&'a IdentityLedger>,
    complete: bool,
    /// The durable anchor spellings of enums whose sum/member anchors have already
    /// been resolved, so an enum reachable from several durable fields resolves —
    /// and reports any identity gap — exactly once.
    seen_enums: BTreeSet<String>,
    /// The first compiler-owned enum-shape coherence failure. It bypasses source
    /// diagnostics and aborts the durable build at the compile invariant boundary.
    invariant: Option<GenericInvariant>,
    /// The struct/enum value types on the current value-shape recursion path. It
    /// bounds the recursion by the finite distinct-type set: a type already on the
    /// path closes a cycle and short-circuits before the depth check. A cycle whose
    /// repeat falls within the depth bound is therefore pre-empted here and left to
    /// the later value-cycle `check.recursion` pass alone; a finite acyclic value, or a
    /// cycle whose distinct prefix first crosses the depth bound, reports its own
    /// `check.resource_limit` (the latter case then also draws `check.recursion` from
    /// the cycle pass — both are truthful and land at real spans).
    value_path: Vec<ValueNode>,
    diagnostics: &'a mut Vec<SourceDiagnostic>,
}

/// One struct or enum value type on the durable value-shape recursion path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueNode {
    Struct(marrow_image::TypeId),
    Enum(marrow_image::EnumId),
}

impl<'a> IdentityResolver<'a> {
    fn new(
        file: &'a str,
        span: SourceSpan,
        ledger: Option<&'a IdentityLedger>,
        diagnostics: &'a mut Vec<SourceDiagnostic>,
    ) -> Self {
        Self {
            file,
            span,
            ledger,
            complete: true,
            seen_enums: BTreeSet::new(),
            invariant: None,
            value_path: Vec::new(),
            diagnostics,
        }
    }

    /// Build a durable field's stored value shape from its resolved value type, over
    /// the closed acyclic durable value set. A nominal scalar erases to its base
    /// `int`; a dense struct records its leaves positionally with no per-leaf ledger
    /// id (the containing field is the renamable durable declaration); a closed enum
    /// resolves its sum (kind 5) and per-member (kind 6) identities. A collection or
    /// abstract type parameter is not a durable value leaf — it is a precise
    /// `check.unsupported` that marks the graph incomplete, so the placeholder shape
    /// is discarded with the graph.
    fn build_value_shape(
        &mut self,
        records: &TypeRegistry,
        metadata: &mut TypeMetadataSession<'_>,
        ty: GArg,
        depth: usize,
    ) -> DurableValueShape {
        match ty {
            GArg::Scalar(scalar) => DurableValueShape::Scalar(scalar.image()),
            GArg::Nominal(_) => DurableValueShape::Scalar(ScalarType::Int.image()),
            GArg::Struct(type_id) => {
                // A struct already on the path closes a value cycle: leave it to the
                // later value-cycle pass (`check.recursion`) and drop the graph. The
                // cycle check precedes the depth check, so a cycle whose repeat falls
                // within the depth bound is pre-empted here and reported only by the
                // cycle pass. A finite acyclic value that reaches the depth bound is
                // genuinely over-deep and reports its own `check.resource_limit`; a
                // cycle whose distinct prefix crosses the depth bound first hits this
                // limit and additionally draws `check.recursion` — both truthful.
                if self.value_path.contains(&ValueNode::Struct(type_id)) {
                    self.complete = false;
                    return DurableValueShape::Scalar(ScalarType::Int.image());
                }
                if depth > bounds::MAX_DURABLE_VALUE_DEPTH {
                    self.reject_resource_limit(self.span, over_deep_value_message());
                    return DurableValueShape::Scalar(ScalarType::Int.image());
                }
                match records.struct_by_type(type_id) {
                    Some(info) => {
                        if info.fields.len() > bounds::MAX_STRUCT_LEAVES {
                            self.reject_resource_limit(
                                self.span,
                                format!(
                                    "a durable struct value carries more than the fixed limit \
                                     of {} leaves",
                                    bounds::MAX_STRUCT_LEAVES
                                ),
                            );
                            return DurableValueShape::Scalar(ScalarType::Int.image());
                        }
                        self.value_path.push(ValueNode::Struct(type_id));
                        let leaves = info
                            .fields
                            .iter()
                            .map(|field| {
                                self.build_value_shape(records, metadata, field.ty, depth + 1)
                            })
                            .collect();
                        self.value_path.pop();
                        DurableValueShape::Struct(leaves)
                    }
                    None => {
                        self.reject_value("this struct value");
                        DurableValueShape::Struct(Vec::new())
                    }
                }
            }
            GArg::Enum(enum_id) => {
                if self.value_path.contains(&ValueNode::Enum(enum_id)) {
                    self.complete = false;
                    return DurableValueShape::Scalar(ScalarType::Int.image());
                }
                if depth > bounds::MAX_DURABLE_VALUE_DEPTH {
                    self.reject_resource_limit(self.span, over_deep_value_message());
                    return DurableValueShape::Scalar(ScalarType::Int.image());
                }
                self.value_path.push(ValueNode::Enum(enum_id));
                let shape = self.build_enum_value_shape(records, metadata, enum_id, depth);
                self.value_path.pop();
                shape
            }
            GArg::Collection(_) => {
                self.reject_value(
                    "a collection stored directly in a durable field (a large collection \
                     belongs under a keyed branch)",
                );
                DurableValueShape::Scalar(ScalarType::Int.image())
            }
            GArg::Group(_) => {
                // A group is a materialized-value namespace, never a durable top-level
                // field value (a durable group is its own member-tree node, resolved by
                // `build_extras`). It cannot reach here through `record.fields`.
                self.reject_value("a group stored directly as a durable field value");
                DurableValueShape::Scalar(ScalarType::Int.image())
            }
            GArg::Param(_) => {
                self.reject_value("this value type");
                DurableValueShape::Scalar(ScalarType::Int.image())
            }
        }
    }

    /// Build the value shape of a durable-reachable closed enum, resolving its sum
    /// and per-member ledger identities once (anchored at the enum's canonical
    /// spelling and `<spelling>.<member>`). Member order is declaration order, so
    /// append-only member evolution preserves every existing member's id and code.
    fn build_enum_value_shape(
        &mut self,
        records: &TypeRegistry,
        metadata: &mut TypeMetadataSession<'_>,
        enum_id: marrow_image::EnumId,
        depth: usize,
    ) -> DurableValueShape {
        let Some((variants, spelling)) = self.accept_ready_shape(
            metadata.durable_enum_shape_and_anchor(enum_id),
            "this enum value",
        ) else {
            return DurableValueShape::Scalar(ScalarType::Int.image());
        };
        // Resolve (and gap-report) an enum's anchors only the first time it is
        // reached; a later occurrence looks its ids up quietly.
        let first_time = self.seen_enums.insert(spelling.clone());
        let sum = self.resolve_enum_anchor(IdentityKind::Sum, &spelling, first_time);
        let members = variants
            .iter()
            .map(|(name, payload)| {
                let id = self.resolve_enum_anchor(
                    IdentityKind::Member,
                    &format!("{spelling}.{name}"),
                    first_time,
                );
                let payload = payload
                    .iter()
                    .map(|arg| self.build_value_shape(records, metadata, *arg, depth + 1))
                    .collect();
                DurableEnumMemberShape { id, payload }
            })
            .collect();
        DurableValueShape::Enum { sum, members }
    }

    fn accept_ready_shape<T>(
        &mut self,
        result: Result<Option<T>, GenericInvariant>,
        subject: &str,
    ) -> Option<T> {
        match result {
            Ok(Some(value)) => Some(value),
            Ok(None) => {
                self.reject_value(subject);
                None
            }
            Err(invariant) => {
                self.remember_invariant(invariant);
                None
            }
        }
    }

    fn remember_invariant(&mut self, invariant: GenericInvariant) {
        self.complete = false;
        if self.invariant.is_none() {
            self.invariant = Some(invariant);
        }
    }

    /// Resolve one enum sum/member anchor. On the enum's first occurrence this is the
    /// ordinary gap-reporting `resolve`; on a later occurrence it looks the id up
    /// quietly, since the first occurrence already reported any gap and discarded the
    /// graph.
    fn resolve_enum_anchor(
        &mut self,
        kind: IdentityKind,
        path: &str,
        report: bool,
    ) -> LedgerIdBytes {
        if report {
            return self.resolve(kind, path);
        }
        match self.ledger.and_then(|ledger| ledger.lookup(kind, path)) {
            Some(id) => LedgerIdBytes::from_bytes(*id.bytes()),
            None => LedgerIdBytes::from_bytes([0u8; 16]),
        }
    }

    /// Report a durable field value type outside the closed acyclic durable value set
    /// and mark the graph incomplete, so its placeholder value shape never reaches
    /// the image.
    fn reject_value(&mut self, subject: &str) {
        self.complete = false;
        self.diagnostics
            .push(unsupported(self.file, self.span, subject));
    }

    /// Report a durable construct that crosses a fixed compiler-owned resource bound
    /// at `span`, and mark the graph incomplete so its placeholder never reaches the
    /// image.
    fn reject_resource_limit(&mut self, span: SourceSpan, message: String) {
        self.complete = false;
        self.diagnostics
            .push(resource_limit(self.file, span, message));
    }

    /// Resolve one anchor to its live ledger id. On a gap this reports the precise
    /// `(kind, path)` diagnostic, flips `complete` to false, and returns a
    /// placeholder id — the caller discards the whole graph when `complete` is
    /// false, so the placeholder is never encoded.
    fn resolve(&mut self, kind: IdentityKind, path: &str) -> LedgerIdBytes {
        if self.invariant.is_some() {
            return LedgerIdBytes::from_bytes([0u8; 16]);
        }
        let (live, retired) = match self.ledger {
            Some(ledger) => (ledger.lookup(kind, path), ledger.is_retired(kind, path)),
            None => (None, false),
        };
        match live {
            Some(id) => LedgerIdBytes::from_bytes(*id.bytes()),
            None => {
                self.complete = false;
                self.diagnostics
                    .push(identity_gap(self.file, self.span, kind, path, retired));
                LedgerIdBytes::from_bytes([0u8; 16])
            }
        }
    }

    /// Walk a resource's declared members, returning the durable member records for
    /// its static `group` namespaces (first, in source order) then its keyed `branch`
    /// placements (in source order) — its top-level stored fields are anchored by the
    /// caller against the materialized record. `container` is the anchor path prefix —
    /// the resource name at the top level, extended by each group or branch name as the
    /// walk descends. A keyed scalar leaf or a non-scalar field inside a group or branch
    /// is a precise `check.unsupported` rejection.
    fn build_extras(
        &mut self,
        draft: &mut ImageDraft,
        records: &TypeRegistry,
        members: &[ResourceMember],
        container: &str,
    ) -> Vec<DurableMemberDef> {
        let mut groups = Vec::new();
        let mut branches = Vec::new();
        for member in members {
            let ResourceMember::Group(group) = member else {
                continue;
            };
            let path = format!("{container}.{}", group.name);
            if group.keys.is_empty() {
                // A `group`: an unkeyed static field-path namespace. Its direct fields
                // flatten into the containing resource's namespace, so it mints no
                // record type of its own.
                let id = self.resolve(IdentityKind::Group, &path);
                let (inner, _record_fields) = self.build_member_tree(draft, records, group, &path);
                groups.push(DurableMemberDef::Group { id, members: inner });
            } else {
                // A keyed `branch`: a distinct keyed placement, like a root. Its entry
                // is a record of its own direct scalar fields; materialize that record
                // type (ordered like the member tree) so a whole branch-entry read
                // yields it and a create/replace supplies it. The record type name is
                // the qualified `Resource.branch` path — the branch's constructor
                // spelling; the branch's own `name` is the simple member name the
                // physical layer keys its family by.
                let placement = self.resolve(IdentityKind::Root, &path);
                let keys = self.build_branch_keys(records, group, &path);
                let (inner, record_fields) = self.build_member_tree(draft, records, group, &path);
                let record_name = draft.intern_string(&path);
                let record = draft.add_record_type(RecordTypeDef {
                    name: record_name,
                    fields: record_fields,
                });
                let name = draft.intern_string(&group.name);
                branches.push(DurableMemberDef::Branch {
                    placement,
                    name,
                    record,
                    keys,
                    members: inner,
                });
            }
        }
        groups.extend(branches);
        groups
    }

    /// The key tuple of a branch placement: each column's scalar and its ledger id
    /// anchored at `<branch path>.<column>`. A key type outside the closed orderable
    /// durable-key set is a precise diagnostic and marks the graph incomplete.
    fn build_branch_keys(
        &mut self,
        records: &TypeRegistry,
        group: &GroupDecl,
        path: &str,
    ) -> Vec<KeyColumn> {
        if group.keys.len() > bounds::MAX_KEY_COLUMNS {
            self.reject_resource_limit(
                group.span,
                format!(
                    "a branch key tuple has {} columns; the fixed limit is {}",
                    group.keys.len(),
                    bounds::MAX_KEY_COLUMNS
                ),
            );
            return Vec::new();
        }
        let scalars = match resolve_key_scalars(
            self.file,
            group.span,
            &group.keys,
            records,
            self.diagnostics,
        ) {
            Some(scalars) => scalars,
            None => {
                self.complete = false;
                return Vec::new();
            }
        };
        group
            .keys
            .iter()
            .zip(scalars)
            .map(|(key_param, scalar)| KeyColumn {
                scalar: scalar.image(),
                id: self.resolve(IdentityKind::Key, &format!("{path}.{}", key_param.name)),
            })
            .collect()
    }

    /// The member records of one group or branch body: its stored scalar fields,
    /// then its nested groups and branches. Field anchors are `<path>.<field>`.
    fn build_member_tree(
        &mut self,
        draft: &mut ImageDraft,
        records: &TypeRegistry,
        group: &GroupDecl,
        path: &str,
    ) -> (Vec<DurableMemberDef>, Vec<FieldDef>) {
        let mut members = Vec::new();
        let mut record_fields = Vec::new();
        for member in &group.members {
            let ResourceMember::Field(field) = member else {
                continue;
            };
            if let Some((def, record_field)) = self.build_field(draft, records, field, path) {
                members.push(def);
                record_fields.push(record_field);
            }
        }
        let extras = self.build_extras(draft, records, &group.members, path);
        members.extend(extras);
        (members, record_fields)
    }

    /// One stored scalar field of a group or branch: its ledger id, required flag,
    /// and scalar value shape. Group and branch leaves stay scalar-only on this line
    /// (top-level resource fields carry the widened value set); a keyed scalar leaf
    /// or a non-scalar group/branch field is a precise `check.unsupported` rejection
    /// and marks the graph incomplete.
    fn build_field(
        &mut self,
        draft: &mut ImageDraft,
        records: &TypeRegistry,
        field: &FieldDecl,
        container: &str,
    ) -> Option<(DurableMemberDef, FieldDef)> {
        if !field.keys.is_empty() {
            self.complete = false;
            self.diagnostics
                .push(unsupported(self.file, field.span, "a keyed field"));
            return None;
        }
        let Some(scalar) = scalar_of(&records.expand(&field.ty)) else {
            self.complete = false;
            self.diagnostics.push(unsupported(
                self.file,
                field.span,
                "a non-scalar field of a group or branch",
            ));
            return None;
        };
        let id = self.resolve(IdentityKind::Field, &format!("{container}.{}", field.name));
        let member = DurableMemberDef::Field {
            id,
            required: field.required,
            value: DurableValueShape::Scalar(scalar.image()),
        };
        // The record field mirrors the durable member: same order, same scalar, same
        // required flag. The branch entry's whole-payload read/create/replace flows
        // through this record type.
        let record_field = FieldDef {
            name: draft.intern_string(&field.name),
            ty: ImageType::scalar(scalar.image()),
            required: field.required,
        };
        Some((member, record_field))
    }

    /// Resolve a root's managed indexes into their durable identity shapes, enforcing
    /// the closed narrow-index admission rules against the root's identity keys and
    /// top-level fields. A `store` index is either a nonunique ordered projection that
    /// must end with every identity key in declaration order (so each row is distinct)
    /// or a `unique` projection that may omit the identity keys. Every projected leaf
    /// must be one identity key or one top-level field whose stored value is an
    /// orderable durable-key scalar; a nested path, a name resolving to nothing, a
    /// group/branch or non-key-scalar leaf, a singleton root, or an index name
    /// colliding with a key/field/earlier index is a precise `check.type` rejection.
    /// Any violation marks the graph incomplete, so a rejected index discards the whole
    /// durable graph. The index's own `Index` ledger identity resolves through the
    /// ledger like every other durable anchor (a gap is `check.durable_identity`).
    fn build_indexes(
        &mut self,
        root: &str,
        keys: &[(String, LedgerIdBytes, ScalarType)],
        fields: &[IndexFieldLeaf],
        indexes: &[IndexDecl],
    ) -> Vec<BuiltIndex> {
        // The checker caps the per-root index count well below the image's structural
        // decode bound (`marrow_image::bounds::MAX_INDEXES`): each declared index is
        // compiler-maintained on every write to the root, so the cap bounds a root's write
        // amplification. The tighter checker limit is a product choice; the image bound
        // remains as headroom for a later increase without an image-format change.
        if indexes.len() > MAX_STORE_INDEXES {
            // The count itself is malformed, so report it and discard the graph rather than
            // validating and minting identities for indexes that cannot all be admitted.
            self.reject_index(
                indexes[MAX_STORE_INDEXES].span,
                format!(
                    "store root `{root}` declares {} managed indexes; at most \
                     {MAX_STORE_INDEXES} are allowed",
                    indexes.len()
                ),
            );
            return Vec::new();
        }
        let mut shapes = Vec::with_capacity(indexes.len());
        let mut seen_names: Vec<&str> = Vec::new();
        for index in indexes {
            // The projected component count crosses the fixed image projection width
            // before the index's leaves are resolved or its identity minted.
            if index.args.len() > bounds::MAX_INDEX_COMPONENTS {
                self.reject_resource_limit(
                    index.span,
                    format!(
                        "a managed index projects {} components; the fixed limit is {}",
                        index.args.len(),
                        bounds::MAX_INDEX_COMPONENTS
                    ),
                );
                continue;
            }
            // The index name shares the root's source namespace with the identity keys,
            // the stored fields, and the other indexes; a collision has no unambiguous
            // address.
            let name_collision = keys.iter().any(|(name, _, _)| name == &index.name)
                || fields.iter().any(|leaf| leaf.name == index.name)
                || seen_names.contains(&index.name.as_str());
            if name_collision {
                self.reject_index(
                    index.span,
                    format!(
                        "index `{}` collides with an identity key, a stored field, or another \
                         index of `{root}`",
                        index.name
                    ),
                );
                continue;
            }
            seen_names.push(&index.name);

            // An index entry points at one stored identity, so a singleton root (no
            // identity to point to) admits none.
            if keys.is_empty() {
                self.reject_index(
                    index.span,
                    format!("index `{}` requires a keyed store root", index.name),
                );
                continue;
            }

            let Some(resolved) = self.resolve_index_components(index, keys, fields) else {
                continue;
            };
            // The image identity references and lowerer-facing scalar projection are
            // two views of the same admitted components, in the same order.
            let components = resolved.iter().map(|item| item.component).collect();
            let projection = resolved.iter().map(|item| item.scalar).collect();
            let id = self.resolve(IdentityKind::Index, &format!("{root}.{}", index.name));
            shapes.push(BuiltIndex {
                shape: DurableIndexShape {
                    id,
                    unique: index.unique,
                    components,
                },
                name: index.name.clone(),
                projection,
            });
        }
        shapes
    }

    /// Resolve and validate one index's ordered projection into leaf references, or
    /// `None` (with a diagnostic and the graph marked incomplete) on any violation. A
    /// component resolves to an identity key or a top-level orderable-key field and
    /// appears at most once; a nonunique index must additionally end with every
    /// identity key in declaration order and carry no identity key in a leading
    /// position.
    fn resolve_index_components(
        &mut self,
        index: &IndexDecl,
        keys: &[(String, LedgerIdBytes, ScalarType)],
        fields: &[IndexFieldLeaf],
    ) -> Option<Vec<ResolvedIndexComponent>> {
        let mut components = Vec::with_capacity(index.args.len());
        let mut leading_key = false;
        let trailing_start = index.args.len().saturating_sub(keys.len());
        let mut ok = true;
        let mut seen_args: Vec<&str> = Vec::with_capacity(index.args.len());
        for (position, arg) in index.args.iter().enumerate() {
            let span = index.arg_spans.get(position).copied().unwrap_or(index.span);
            if seen_args.contains(&arg.as_str()) {
                self.reject_index(
                    span,
                    format!(
                        "index `{}` repeats component `{arg}`; each projection component appears \
                         at most once",
                        index.name
                    ),
                );
                ok = false;
                continue;
            }
            seen_args.push(arg);
            if arg.contains('.') {
                self.reject_index(
                    span,
                    format!(
                        "index `{}` component `{arg}` reaches through a nested member; an index \
                         projects only top-level fields and identity keys",
                        index.name
                    ),
                );
                ok = false;
                continue;
            }
            if let Some((_, key_id, scalar)) = keys.iter().find(|(name, _, _)| name == arg) {
                if !index.unique && position < trailing_start {
                    leading_key = true;
                }
                components.push(ResolvedIndexComponent {
                    component: DurableIndexComponent::Key(*key_id),
                    scalar: *scalar,
                });
            } else if let Some(leaf) = fields.iter().find(|leaf| &leaf.name == arg) {
                let Some(scalar) = leaf.scalar else {
                    self.reject_index(
                        span,
                        format!(
                            "index `{}` component `{arg}` is not an orderable durable-key scalar",
                            index.name
                        ),
                    );
                    ok = false;
                    continue;
                };
                components.push(ResolvedIndexComponent {
                    component: DurableIndexComponent::Field(leaf.id),
                    scalar,
                });
            } else {
                self.reject_index(
                    span,
                    format!(
                        "index `{}` component `{arg}` names no identity key or stored field of \
                         this root",
                        index.name
                    ),
                );
                ok = false;
            }
        }
        if !ok {
            return None;
        }
        // A nonunique index distinguishes rows by ending with the complete identity
        // suffix, in declaration order, with no identity key appearing earlier.
        if !index.unique {
            let ends_with_identity = index.args.len() >= keys.len()
                && keys.iter().enumerate().all(|(offset, (_, key_id, _))| {
                    matches!(
                        components.get(trailing_start + offset),
                        Some(ResolvedIndexComponent {
                            component: DurableIndexComponent::Key(id),
                            ..
                        }) if *id == *key_id
                    )
                });
            if leading_key || !ends_with_identity {
                self.reject_index(
                    index.span,
                    format!(
                        "non-unique index `{}` must end with the store's identity keys in \
                         declaration order",
                        index.name
                    ),
                );
                return None;
            }
        }
        Some(components)
    }

    /// Report a managed-index admission violation and mark the durable graph
    /// incomplete, so a rejected index discards the whole graph rather than emitting a
    /// partial one.
    fn reject_index(&mut self, span: SourceSpan, message: String) {
        self.complete = false;
        self.diagnostics.push(SourceDiagnostic::at(
            Code::CheckType.as_str(),
            self.file,
            span,
            message,
        ));
    }
}

/// The operation sites and materialized record of one top-level branch: its
/// whole-payload entry site, its direct field-leaf sites in declaration order, and its
/// materialized record type. For an executable branch these back the branch's
/// whole-entry operations and its field-exact `^root(k).branch(bk).field` operations
/// respectively; a non-flat root parks them and consumes neither.
struct BranchSites {
    entry: u16,
    fields: Vec<u16>,
    record: marrow_image::TypeId,
    /// The captured sites of this branch's own nested branches, in declaration order, so a
    /// nested-branch lowerer resolves a deeper `^root(k).b(bk).s(sk)` path level by level.
    branches: Vec<BranchSites>,
}

/// A child semantic path: `parent` extended by one kind-tagged ledger-id step.
fn child_steps(
    parent: &[SemanticStep],
    kind: SemanticStepKind,
    id: LedgerIdBytes,
) -> Vec<SemanticStep> {
    let mut steps = parent.to_vec();
    steps.push(SemanticStep::new(kind, id));
    steps
}

/// Emit one stored field's field-leaf site under `parent_steps`, returning its index.
fn emit_field_site(
    draft: &mut ImageDraft,
    parent_steps: &[SemanticStep],
    id: LedgerIdBytes,
) -> u16 {
    let steps = child_steps(parent_steps, SemanticStepKind::Field, id);
    draft
        .add_site(SiteDef::field_leaf(SemanticPath::from_steps(steps)))
        .index()
}

/// Emit one keyed placement's whole-payload site at `steps`, returning its index.
fn emit_placement_site(draft: &mut ImageDraft, steps: &[SemanticStep]) -> u16 {
    draft
        .add_site(SiteDef::whole_payload(SemanticPath::from_steps(
            steps.to_vec(),
        )))
        .index()
}

/// Emit the operation sites of the root's whole member tree under `root_steps`,
/// capturing the root's direct field-leaf sites, each root-level group's `GroupEntry`
/// site (in declaration order), and each top-level branch's captured sites (recursively)
/// for the flat executable lowerer. A root-level group emits one `GroupEntry` site over
/// the group node and its leaf field-leaf sites (parked, for identity completeness); a
/// group leaf is never executed through its own site, so only the group's entry site is
/// captured. The emission order is pre-order, a placement or group node before its
/// members, mirroring [`marrow_image::DurableContractDescriptor::semantic_nodes`] so every
/// site resolves against the verifier's independently reconstructed node set.
fn emit_root_member_sites(
    draft: &mut ImageDraft,
    root_steps: &[SemanticStep],
    members: &[DurableMemberDef],
) -> (Vec<u16>, Vec<u16>, Vec<BranchSites>) {
    let mut top_field_sites = Vec::new();
    let mut top_group_sites = Vec::new();
    let mut top_branches = Vec::new();
    for member in members {
        match member {
            DurableMemberDef::Field { id, .. } => {
                top_field_sites.push(emit_field_site(draft, root_steps, *id));
            }
            DurableMemberDef::Group { id, members } => {
                let steps = child_steps(root_steps, SemanticStepKind::Group, *id);
                let entry = draft
                    .add_site(SiteDef::group_entry(SemanticPath::from_steps(
                        steps.clone(),
                    )))
                    .index();
                top_group_sites.push(entry);
                emit_subtree_sites(draft, &steps, members);
            }
            DurableMemberDef::Branch {
                placement,
                members,
                record,
                ..
            } => {
                top_branches.push(emit_branch_sites(
                    draft, root_steps, *placement, *record, members,
                ));
            }
        }
    }
    (top_field_sites, top_group_sites, top_branches)
}

/// Emit one keyed branch's sites under `parent_steps` and capture them recursively: its
/// whole-payload entry site, its direct field-leaf sites in declaration order, and each
/// nested branch's captured sites. A static `group` inside a branch parks the whole root
/// (`member_keeps_root_flat` refuses it), so on the executable path only fields and nested
/// branches occur; a group is still emitted without capture for identity completeness. The
/// direct field order is the branch's materialized-record order — the leaf the verifier
/// seals as `BranchField(field)` — and the nested-branch order indexes the sealed branch
/// tree, so the compiler's and verifier's independent resolutions agree.
fn emit_branch_sites(
    draft: &mut ImageDraft,
    parent_steps: &[SemanticStep],
    placement: LedgerIdBytes,
    record: marrow_image::TypeId,
    members: &[DurableMemberDef],
) -> BranchSites {
    let steps = child_steps(parent_steps, SemanticStepKind::Placement, placement);
    let entry = emit_placement_site(draft, &steps);
    let mut fields = Vec::new();
    let mut branches = Vec::new();
    for inner in members {
        match inner {
            DurableMemberDef::Field { id, .. } => {
                fields.push(emit_field_site(draft, &steps, *id));
            }
            DurableMemberDef::Group { id, members } => {
                let steps = child_steps(&steps, SemanticStepKind::Group, *id);
                emit_subtree_sites(draft, &steps, members);
            }
            DurableMemberDef::Branch {
                placement,
                members,
                record,
                ..
            } => {
                branches.push(emit_branch_sites(
                    draft, &steps, *placement, *record, members,
                ));
            }
        }
    }
    BranchSites {
        entry,
        fields,
        record,
        branches,
    }
}

/// Emit the field-leaf and whole-payload sites of every node in a member subtree under
/// `parent_steps`, without capturing indices: a stored field yields a field-leaf site,
/// a static `group` recurses (a namespace, no site of its own), and a keyed `branch`
/// yields a whole-payload site and recurses. The single recursive site emitter for
/// parked nested content, reached from [`emit_root_member_sites`].
fn emit_subtree_sites(
    draft: &mut ImageDraft,
    parent_steps: &[SemanticStep],
    members: &[DurableMemberDef],
) {
    for member in members {
        match member {
            DurableMemberDef::Field { id, .. } => {
                emit_field_site(draft, parent_steps, *id);
            }
            DurableMemberDef::Group { id, members } => {
                let steps = child_steps(parent_steps, SemanticStepKind::Group, *id);
                emit_subtree_sites(draft, &steps, members);
            }
            DurableMemberDef::Branch {
                placement, members, ..
            } => {
                let steps = child_steps(parent_steps, SemanticStepKind::Placement, *placement);
                emit_placement_site(draft, &steps);
                emit_subtree_sites(draft, &steps, members);
            }
        }
    }
}

/// Whether a durable member keeps its containing root flat-executable, mirroring the
/// verifier's independent `keeps_root_flat`: a field (scalar or widened struct/enum — the
/// durable field codec frames a composite inline in its cell), or a field-only keyed
/// branch (one or more key columns) whose direct members recursively keep flat. A static
/// `group` does not. (A `Field`'s value shape is always a scalar, struct, or enum — a
/// collection field is rejected upstream — so any field keeps the root flat here.)
fn member_keeps_root_flat(member: &DurableMemberDef) -> bool {
    match member {
        DurableMemberDef::Field { value, .. } => matches!(
            value,
            DurableValueShape::Scalar(_)
                | DurableValueShape::Struct(_)
                | DurableValueShape::Enum { .. }
        ),
        DurableMemberDef::Group { .. } => false,
        DurableMemberDef::Branch { keys, members, .. } => {
            !keys.is_empty() && members.iter().all(member_keeps_root_flat)
        }
    }
}

/// Whether a root's *direct* member keeps the root flat-executable, mirroring the
/// verifier's independent `member_flat_at_root`. It admits one more shape than
/// [`member_keeps_root_flat`]: a root-level unkeyed `group` whose own members are all
/// storable-value fields (a scalar or a widened composite). A group is a value unit of the
/// root entry, executable at the root level; a group nested in a branch or in another
/// group still parks, because branch members are classified by [`member_keeps_root_flat`]
/// (which keeps `Group => false`), so a group below the root's direct members never makes
/// its enclosing branch flat.
fn member_flat_at_root(member: &DurableMemberDef) -> bool {
    match member {
        DurableMemberDef::Field { .. } => member_keeps_root_flat(member),
        DurableMemberDef::Group { members, .. } => members
            .iter()
            .all(|inner| matches!(inner, DurableMemberDef::Field { .. })),
        DurableMemberDef::Branch { .. } => member_keeps_root_flat(member),
    }
}

/// The executable root-level group descriptors of a flat-executable root, in declaration
/// order. Each group's materialized record and its scalar/widened leaves come from the
/// registry `GroupInfo` (`groups`), and its captured `GroupEntry` site from `sites` — both
/// in the same declaration order, so a group descriptor and its site align by position.
/// Called only when the caller has proven the root flat-executable, so every group is a
/// storable-value-field group.
fn build_executable_groups(groups: &[crate::types::GroupInfo], sites: &[u16]) -> Vec<DurableGroup> {
    groups
        .iter()
        .zip(sites)
        .map(|(group, &entry_site)| DurableGroup {
            name: group.name.clone(),
            record: group.type_id,
            entry_site,
            fields: group
                .fields
                .iter()
                .map(|leaf| DurableGroupLeaf {
                    name: leaf.name.clone(),
                    ty: leaf.ty,
                    required: leaf.required,
                })
                .collect(),
        })
        .collect()
}

/// The executable branch descriptors of a flat-executable root, in declaration order,
/// recursively. Each branch's materialized record type and its whole-payload, per-field,
/// and nested-branch sites come from `top_branches`, and its simple name, key, field plan,
/// and nested branches from the source resource declaration — all in the same declaration
/// order, so a branch path indexes both the sealed branch tree and this one identically.
/// Called only when the caller has proven the root flat-executable, so every branch is a
/// scalar-field keyed branch (its nested members are scalar fields and simple
/// branches).
fn build_executable_branches(
    records: &TypeRegistry,
    resource: &ResourceDecl,
    top_branches: &[BranchSites],
) -> Vec<DurableBranch> {
    build_branches(records, &resource.members, top_branches)
}

/// Build the [`DurableBranch`] descriptors for the keyed branches among `members`, zipped
/// positionally against their captured `sites`, recursing into each branch's own members
/// and captured nested-branch sites. The source keyed groups and the captured `BranchSites`
/// are both in declaration order, so the zip pairs each branch with its own sites.
fn build_branches(
    records: &TypeRegistry,
    members: &[ResourceMember],
    sites: &[BranchSites],
) -> Vec<DurableBranch> {
    members
        .iter()
        .filter_map(|member| match member {
            ResourceMember::Group(group) if !group.keys.is_empty() => Some(group),
            _ => None,
        })
        .zip(sites)
        .map(|(group, sites)| {
            let key = group
                .keys
                .iter()
                .map(|column| {
                    scalar_of(&records.expand(&column.ty))
                        .expect("an executable branch key column is an orderable key scalar")
                })
                .collect();
            let fields = group
                .members
                .iter()
                .filter_map(|member| match member {
                    ResourceMember::Field(field) => Some(field),
                    _ => None,
                })
                .zip(&sites.fields)
                .map(|(field, &site)| {
                    let scalar = scalar_of(&records.expand(&field.ty))
                        .expect("an executable branch field is a scalar");
                    DurableBranchField {
                        name: field.name.clone(),
                        scalar,
                        required: field.required,
                        site,
                    }
                })
                .collect();
            let branches = build_branches(records, &group.members, &sites.branches);
            DurableBranch {
                name: group.name.clone(),
                key,
                record: sites.record,
                entry_site: sites.entry,
                fields,
                branches,
            }
        })
        .collect()
}

fn scalar_of(ty: &TypeExpr) -> Option<ScalarType> {
    match ty {
        TypeExpr::Name { text, .. } => ScalarType::from_spelling(text),
        _ => None,
    }
}

/// The precise missing/retired-identity diagnostic: the typed `(kind, path)`
/// gap plus a message naming the identity and the command that mints it.
fn identity_gap(
    file: &str,
    span: SourceSpan,
    kind: IdentityKind,
    path: &str,
    retired: bool,
) -> SourceDiagnostic {
    let message = if retired {
        format!(
            "durable identity for {} `{}` was retired in marrow.ids and can never be reused; \
             declare a fresh name",
            kind.keyword(),
            path
        )
    } else {
        format!(
            "durable identity for {} `{}` is missing from marrow.ids; \
             `marrow run` mints missing identities (commit the updated marrow.ids)",
            kind.keyword(),
            path
        )
    };
    SourceDiagnostic::identity_gap(
        Code::CheckDurableIdentity.as_str(),
        file,
        span,
        message,
        IdentityGap {
            kind,
            path: path.to_string(),
            retired,
        },
    )
}

fn unsupported(file: &str, span: SourceSpan, subject: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        span,
        format!("{subject} is not yet supported on the beta line"),
    )
}

/// A `check.resource_limit`: one durable construct crosses a fixed compiler-owned
/// bound the image cannot represent, reported at the offending construct's span so
/// the source, not a fabricated location, carries the diagnostic.
fn resource_limit(file: &str, span: SourceSpan, message: String) -> SourceDiagnostic {
    SourceDiagnostic::at(Code::CheckResourceLimit.as_str(), file, span, message)
}

fn over_deep_value_message() -> String {
    format!(
        "a durable field value nests structs or enums deeper than the fixed limit of {} levels",
        bounds::MAX_DURABLE_VALUE_DEPTH
    )
}

#[cfg(test)]
mod generic_enum_shape_tests {
    use super::*;
    use crate::types::{MintSite, TypeInstId, TypeInstKind};
    use marrow_syntax::{Declaration, parse_source};

    /// A committed reserved enum reaches the durable-shape owner
    /// with its exact member and payload layout. Missing ledger rows may make the
    /// enclosing graph incomplete, but do not turn a Ready enum into an unavailable
    /// generic row.
    #[test]
    fn ready_option_reaches_the_durable_enum_shape_owner() {
        let mut draft = ImageDraft::new();
        let mut build_diagnostics = Vec::new();
        let records =
            TypeRegistry::build(&mut draft, &[], &[], &[], &[], &[], &mut build_diagnostics);
        assert!(build_diagnostics.is_empty());
        let option = records
            .instantiate_reserved_option(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                MintSite {
                    file: "src/main.mw",
                    span: SourceSpan {
                        line: 1,
                        column: 1,
                        ..SourceSpan::default()
                    },
                },
            )
            .expect("Ready Option mints");

        let mut diagnostics = Vec::new();
        let mut resolver =
            IdentityResolver::new("src/main.mw", SourceSpan::default(), None, &mut diagnostics);
        let shape = records
            .with_metadata_session(|metadata| {
                Ok::<_, GenericInvariant>(
                    resolver.build_enum_value_shape(&records, metadata, option, 0),
                )
            })
            .expect("the Ready Option metadata session opens");
        let DurableValueShape::Enum { members, .. } = shape else {
            panic!("a Ready Option remains enum-shaped")
        };
        assert_eq!(members.len(), 2);
        assert!(members[0].payload.is_empty());
        assert_eq!(members[1].payload.len(), 1);
        assert_eq!(
            members[1].payload[0],
            DurableValueShape::Scalar(ScalarType::Int.image())
        );
        assert!(resolver.seen_enums.contains("Option[int]"));
        assert!(
            !resolver.complete,
            "the test intentionally supplies no ledger"
        );
        drop(resolver);
        assert_eq!(diagnostics.len(), 3, "sum plus two member identity gaps");
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code == Code::CheckDurableIdentity.as_str())
        );
    }

    /// An image enum with no Ready semantic row is refused before
    /// durable identity spelling or member resolution can observe it.
    #[test]
    fn unavailable_enum_stops_before_durable_identity_resolution() {
        let mut draft = ImageDraft::new();
        let mut build_diagnostics = Vec::new();
        let records =
            TypeRegistry::build(&mut draft, &[], &[], &[], &[], &[], &mut build_diagnostics);
        assert!(build_diagnostics.is_empty());
        let name = draft.intern_string("Unavailable");
        let unavailable = draft.add_enum_type(marrow_image::EnumTypeDef {
            name,
            variants: Vec::new(),
        });
        let mut diagnostics = Vec::new();
        let mut resolver =
            IdentityResolver::new("src/main.mw", SourceSpan::default(), None, &mut diagnostics);

        let shape = records
            .with_metadata_session(|metadata| {
                Ok::<_, GenericInvariant>(resolver.build_enum_value_shape(
                    &records,
                    metadata,
                    unavailable,
                    0,
                ))
            })
            .expect("the unavailable enum metadata session opens");
        assert_eq!(shape, DurableValueShape::Scalar(ScalarType::Int.image()));
        assert!(!resolver.complete);
        assert!(resolver.seen_enums.is_empty());
        drop(resolver);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, Code::CheckUnsupported.as_str());
        assert!(diagnostics[0].identity.is_none());
    }

    #[test]
    fn ready_enum_with_struct_body_is_not_contextualized_or_resolved() {
        let mut draft = ImageDraft::new();
        let mut build_diagnostics = Vec::new();
        let records =
            TypeRegistry::build(&mut draft, &[], &[], &[], &[], &[], &mut build_diagnostics);
        let option = records
            .instantiate_reserved_option(
                &mut draft,
                GArg::Scalar(ScalarType::Int),
                MintSite {
                    file: "src/main.mw",
                    span: SourceSpan::default(),
                },
            )
            .expect("Option row mints ready");
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(option),
            body: TypeInstKind::Struct,
        };
        let mut diagnostics = Vec::new();
        let mut resolver =
            IdentityResolver::new("src/main.mw", SourceSpan::default(), None, &mut diagnostics);

        assert!(
            resolver
                .accept_ready_shape::<()>(Err(expected), "this enum value")
                .is_none()
        );
        assert_eq!(resolver.invariant, Some(expected));
        assert!(resolver.seen_enums.is_empty());
        drop(resolver);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn durable_typed_error_stops_before_identity_or_draft_effects() {
        let parsed = parse_source(
            r#"resource Holder {
    required value: Option<int>
}

store ^holders[id: int]: Holder
"#,
        );
        assert!(parsed.diagnostics.is_empty());
        let resource = parsed
            .file
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Resource(resource) => Some(resource),
                _ => None,
            })
            .expect("resource parses");
        let resources = vec![("src/main.mw".to_string(), resource)];
        let mut draft = ImageDraft::new();
        let mut diagnostics = Vec::new();
        let records =
            TypeRegistry::build(&mut draft, &[], &[], &[], &[], &resources, &mut diagnostics);
        assert!(diagnostics.is_empty());
        let option = match records.by_name("Holder").expect("record exists").fields[0].ty {
            GArg::Enum(id) => id,
            _ => panic!("resource field resolves to Option"),
        };
        let expected = GenericInvariant::TypeBodyKindMismatch {
            id: TypeInstId::Enum(option),
            body: TypeInstKind::Struct,
        };
        let before = draft.encode().expect("seeded draft encodes");
        let mut resolver =
            IdentityResolver::new("src/main.mw", SourceSpan::default(), None, &mut diagnostics);
        assert!(
            resolver
                .accept_ready_shape::<()>(Err(expected), "this durable value")
                .is_none()
        );
        assert_eq!(resolver.invariant, Some(expected));
        assert!(resolver.seen_enums.is_empty());
        drop(resolver);
        assert!(diagnostics.is_empty());
        let after = draft.encode().expect("rejected draft still encodes");
        assert_eq!(after.bytes, before.bytes);
        assert_eq!(after.image_id, before.image_id);
    }
}
