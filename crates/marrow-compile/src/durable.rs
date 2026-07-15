//! The durable graph registry (design §B/§C).
//!
//! The durable graph admits at most one `store` root over the project's single
//! resource record. A root is a *singleton* (`store ^root: Record`, no key) or a
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
//! The executable durable subset the single-root kernel can serve at this stage is
//! the flat single-column keyed root: one key column and no groups or branches. A
//! singleton or composite-key root, or any root whose resource declares a group or
//! a branch, completes its identity and verifies but has no executable operation
//! sites — an operation over one is a precise typed `check.unsupported` rejection at
//! lowering ("not yet executable"). The wider shapes run at E01. This module
//! validates the declaration, adds the root, its member tree, and — for the
//! executable subset — its operation sites to the draft, and exposes the resolved
//! sites the function lowerer emits against.

use marrow_codes::Code;
use marrow_image::{
    DurableMemberDef, ImageDraft, KeyColumn, LedgerIdBytes, RootDef, RootIdentity, SiteDef,
    SiteTarget, bounds,
};
use marrow_project::{IdentityKind, IdentityLedger};
use marrow_syntax::{
    FieldDecl, GroupDecl, KeyParam, ResourceDecl, ResourceMember, SourceSpan, StoreDecl, TypeExpr,
};

use crate::diag::{IdentityGap, SourceDiagnostic};
use crate::scalar::ScalarType;
use crate::types::TypeRegistry;

/// The application's fixed ledger anchor path: one local application per
/// project, so the anchor is the project itself.
const APPLICATION_ANCHOR_PATH: &str = ".";

/// One resolved durable field site.
pub(crate) struct DurableField {
    pub(crate) name: String,
    pub(crate) site: u16,
    pub(crate) scalar: ScalarType,
    pub(crate) required: bool,
}

/// The project's single executable durable root and its operation sites. Only a
/// flat single-column keyed root (one key column, no groups or branches) reaches
/// this form; its single key scalar backs the kernel-serviceable read/write path.
pub(crate) struct DurableRoot {
    pub(crate) name: String,
    pub(crate) key: ScalarType,
    pub(crate) record: marrow_image::TypeId,
    pub(crate) entry_site: u16,
    pub(crate) fields: Vec<DurableField>,
}

impl DurableRoot {
    pub(crate) fn field(&self, name: &str) -> Option<&DurableField> {
        self.fields.iter().find(|field| field.name == name)
    }
}

/// The durable registry: zero or one root. `executable` is populated only for the
/// flat single-column keyed root the kernel can serve; `declared_root` names any
/// admitted root (singleton, single-column, composite, or one bearing groups or
/// branches) so a durable operation over a not-yet-executable shape reports
/// precisely rather than as "no store".
#[derive(Default)]
pub(crate) struct DurableRegistry {
    executable: Option<DurableRoot>,
    declared_root: Option<String>,
}

impl DurableRegistry {
    /// The executable flat single-column keyed root, if the project declares one.
    pub(crate) fn root(&self) -> Option<&DurableRoot> {
        self.executable.as_ref()
    }

    /// The name of a declared root the kernel cannot yet serve (a singleton or
    /// composite key, or a resource with a group or branch). `Some` exactly when a
    /// root is declared but not executable, so the lowerer can distinguish a
    /// not-yet-executable operation from an operation with no store at all.
    pub(crate) fn not_yet_executable_root(&self) -> Option<&str> {
        match (&self.executable, &self.declared_root) {
            (None, Some(name)) => Some(name),
            _ => None,
        }
    }

    /// Build the registry from the project's store declarations, adding the one
    /// admitted root and its complete ledger identity block to the draft. More
    /// than one store, an index, a missing or mismatched resource, a key column
    /// outside the closed orderable durable-key set, or a key tuple past the
    /// column bound are rejected — and so is a durable graph whose identity is
    /// incomplete: every durable declaration (the application, the root placement,
    /// its product, each key column, each stored field, each group namespace, and
    /// each nested branch placement and key column) must have a live row in the
    /// committed `marrow.ids` ledger, or the declaration fails precisely with
    /// `check.durable_identity`. The compiler only *reads* the ledger; minting
    /// lives in the `marrow run` convenience action (and in the accepted apply
    /// action when it lands).
    pub(crate) fn build(
        draft: &mut ImageDraft,
        records: &TypeRegistry,
        resources: &[(String, &ResourceDecl)],
        stores: &[(String, &StoreDecl)],
        ledger: Option<&IdentityLedger>,
        diagnostics: &mut Vec<SourceDiagnostic>,
    ) -> Self {
        if stores.len() > 1 {
            for (file, store) in &stores[1..] {
                diagnostics.push(unsupported(
                    file,
                    store.span,
                    "more than one store root per project",
                ));
            }
        }
        let Some((file, store)) = stores.first() else {
            return Self::default();
        };

        if !store.indexes.is_empty() {
            diagnostics.push(unsupported(file, store.span, "a store index"));
            return Self::default();
        }
        if store.root.keys.len() > bounds::MAX_KEY_COLUMNS {
            diagnostics.push(unsupported(
                file,
                store.root.span,
                "a store key with more than eight columns",
            ));
            return Self::default();
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
            return Self::default();
        };
        let Some(record) = records.by_name(&store.resource) else {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                store.span,
                format!("`{}` is not a resource in this project", store.resource),
            ));
            return Self::default();
        };
        let Some((_, resource)) = resources
            .iter()
            .find(|(_, decl)| decl.name == store.resource)
        else {
            return Self::default();
        };

        // A durable resource stores scalar leaves; a top-level enum-valued field has
        // no store representation, so it cannot back a `store`.
        if let Some(field) = record
            .fields
            .iter()
            .find(|field| field.ty.as_scalar().is_none())
        {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                store.span,
                format!(
                    "a stored resource has only scalar fields; `{}` is not a scalar",
                    field.name
                ),
            ));
            return Self::default();
        }

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
        // recursively holding its own members. `has_extras` records whether the
        // resource declares any group or branch, which makes even a single-column
        // keyed root not yet executable.
        let mut members: Vec<DurableMemberDef> = record
            .fields
            .iter()
            .map(|field| DurableMemberDef::Field {
                id: resolver.resolve(
                    IdentityKind::Field,
                    &format!("{}.{}", store.resource, field.name),
                ),
                scalar: field
                    .ty
                    .as_scalar()
                    .expect("a stored resource field is a scalar")
                    .image(),
                required: field.required,
            })
            .collect();
        let (groups_and_branches, has_extras) =
            resolver.build_extras(records, &resource.members, &store.resource);
        members.extend(groups_and_branches);

        // Every identity must resolve before the graph enters the image; a single
        // gap already reported precisely leaves the durable graph absent, so an
        // operation over it is not additionally mislabelled "not yet executable"
        // (the identity gap is the diagnosis, whatever the shape).
        if !resolver.complete {
            return Self::default();
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

        let root_name = draft.intern_string(&store.root.root);
        draft.add_root(RootDef {
            name: root_name,
            keys: key_columns,
            record: record.type_id,
            identity: RootIdentity {
                placement,
                product,
                members,
            },
        });

        // Operation sites — and therefore executable durable operations — exist only
        // for the flat single-column keyed root the kernel can serve. A singleton,
        // composite-key, or group/branch-bearing root carries its identity but no
        // sites; the lowerer reports any operation over it as not yet executable.
        let [key] = key_scalars.as_slice() else {
            return Self::declared(&store.root.root);
        };
        if has_extras {
            return Self::declared(&store.root.root);
        }
        let entry_site = draft
            .add_site(SiteDef {
                root: 0,
                target: SiteTarget::Entry,
            })
            .index();
        let fields = record
            .fields
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let site = draft
                    .add_site(SiteDef {
                        root: 0,
                        target: SiteTarget::Field(index as u16),
                    })
                    .index();
                DurableField {
                    name: field.name.clone(),
                    site,
                    scalar: field
                        .ty
                        .as_scalar()
                        .expect("a stored resource field is a scalar"),
                    required: field.required,
                }
            })
            .collect();

        Self {
            executable: Some(DurableRoot {
                name: store.root.root.clone(),
                key: *key,
                record: record.type_id,
                entry_site,
                fields,
            }),
            declared_root: Some(store.root.root.clone()),
        }
    }

    /// A registry recording that a root of the named placement is declared, in the
    /// image with a complete identity, but not executable — the kernel cannot yet
    /// serve its shape (a singleton or composite key, or a group- or branch-bearing
    /// resource). Used only after the root has entered the draft.
    fn declared(root: &str) -> Self {
        Self {
            executable: None,
            declared_root: Some(root.to_string()),
        }
    }
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
    diagnostics: &'a mut Vec<SourceDiagnostic>,
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
            diagnostics,
        }
    }

    /// Resolve one anchor to its live ledger id. On a gap this reports the precise
    /// `(kind, path)` diagnostic, flips `complete` to false, and returns a
    /// placeholder id — the caller discards the whole graph when `complete` is
    /// false, so the placeholder is never encoded.
    fn resolve(&mut self, kind: IdentityKind, path: &str) -> LedgerIdBytes {
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
    /// its static `group` namespaces and keyed `branch` placements (its top-level
    /// stored fields are anchored by the caller against the materialized record) and
    /// whether any such group or branch is present. `container` is the anchor path
    /// prefix — the resource name at the top level, extended by each group or branch
    /// name as the walk descends. A keyed scalar leaf or a non-scalar field inside a
    /// group or branch is a precise `check.unsupported` rejection.
    fn build_extras(
        &mut self,
        records: &TypeRegistry,
        members: &[ResourceMember],
        container: &str,
    ) -> (Vec<DurableMemberDef>, bool) {
        let mut groups = Vec::new();
        let mut branches = Vec::new();
        for member in members {
            let ResourceMember::Group(group) = member else {
                continue;
            };
            let path = format!("{container}.{}", group.name);
            if group.keys.is_empty() {
                // A `group`: an unkeyed static field-path namespace.
                let id = self.resolve(IdentityKind::Group, &path);
                let inner = self.build_member_tree(records, group, &path);
                groups.push(DurableMemberDef::Group { id, members: inner });
            } else {
                // A keyed `branch`: a distinct keyed placement, like a root.
                let placement = self.resolve(IdentityKind::Root, &path);
                let keys = self.build_branch_keys(records, group, &path);
                let inner = self.build_member_tree(records, group, &path);
                branches.push(DurableMemberDef::Branch {
                    placement,
                    keys,
                    members: inner,
                });
            }
        }
        let has_extras = !groups.is_empty() || !branches.is_empty();
        groups.extend(branches);
        (groups, has_extras)
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
        records: &TypeRegistry,
        group: &GroupDecl,
        path: &str,
    ) -> Vec<DurableMemberDef> {
        let mut fields = Vec::new();
        for member in &group.members {
            let ResourceMember::Field(field) = member else {
                continue;
            };
            if let Some(def) = self.build_field(records, field, path) {
                fields.push(def);
            }
        }
        let (extras, _) = self.build_extras(records, &group.members, path);
        fields.extend(extras);
        fields
    }

    /// One stored scalar field of a group or branch: its ledger id, scalar, and
    /// required flag. A keyed scalar leaf or a non-scalar field is a precise
    /// `check.unsupported` rejection and marks the graph incomplete.
    fn build_field(
        &mut self,
        records: &TypeRegistry,
        field: &FieldDecl,
        container: &str,
    ) -> Option<DurableMemberDef> {
        if !field.keys.is_empty() {
            self.complete = false;
            self.diagnostics
                .push(unsupported(self.file, field.span, "a keyed field"));
            return None;
        }
        let Some(scalar) = scalar_of(&records.expand(&field.ty)) else {
            self.complete = false;
            self.diagnostics.push(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                self.file,
                field.span,
                format!(
                    "a stored field has a scalar type; `{}` is not a scalar",
                    field.name
                ),
            ));
            return None;
        };
        let id = self.resolve(IdentityKind::Field, &format!("{container}.{}", field.name));
        Some(DurableMemberDef::Field {
            id,
            scalar: scalar.image(),
            required: field.required,
        })
    }
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
