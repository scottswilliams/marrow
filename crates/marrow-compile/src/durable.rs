//! The durable store registry (design §B/§C).
//!
//! The T01 subset admits exactly one durable root: a `store ^root(key): Record`
//! over the project's single record type, no indexes and no singleton roots. This
//! module validates that declaration, adds the root and its operation sites to the
//! image draft, and exposes the resolved sites the function lowerer emits against.

use marrow_codes::Code;
use marrow_image::{ImageDraft, LedgerIdBytes, RootDef, RootIdentity, SiteDef, SiteTarget};
use marrow_project::{IdentityKind, IdentityLedger};
use marrow_syntax::{SourceSpan, StoreDecl};

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

/// The project's single durable root and its operation sites.
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

/// The durable registry: zero or one root.
#[derive(Default)]
pub(crate) struct DurableRegistry {
    root: Option<DurableRoot>,
}

impl DurableRegistry {
    pub(crate) fn root(&self) -> Option<&DurableRoot> {
        self.root.as_ref()
    }

    /// Build the registry from the project's store declarations, adding the one
    /// admitted root, its ledger identity block, and its sites to the draft. More
    /// than one store, an index, a missing or mismatched resource, a multi-column
    /// key, or a key outside the closed orderable durable-key set are rejected —
    /// and so is a durable graph whose identity is incomplete: every durable
    /// declaration must have a live row in the committed `marrow.ids` ledger, or
    /// the declaration fails precisely with `check.durable_identity`. The
    /// compiler only *reads* the ledger; minting lives in the `marrow run`
    /// convenience action (and in the accepted apply action when it lands).
    pub(crate) fn build(
        draft: &mut ImageDraft,
        records: &TypeRegistry,
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
        let [key_param] = store.root.keys.as_slice() else {
            diagnostics.push(unsupported(
                file,
                store.root.span,
                "a store with other than one key column",
            ));
            return Self::default();
        };
        let Some(key) = scalar_of(&records.expand(&key_param.ty)) else {
            diagnostics.push(unsupported(file, store.root.span, "this key type"));
            return Self::default();
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
                store.root.span,
                "a store key must be an orderable durable-key scalar (int, string, bool, bytes, date, or instant)"
                    .to_string(),
            ));
            return Self::default();
        }
        let Some(record) = records.by_name(&store.resource) else {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                store.span,
                format!("`{}` is not a resource in this project", store.resource),
            ));
            return Self::default();
        };

        // Durable storage is scalar-leaf; a resource carrying an enum-valued field
        // has no store representation, so it cannot back a `store`.
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

        // Resolve the durable graph's ledger identities. Every durable
        // declaration — the application, the root placement, its key column, the
        // stored product, and each stored field — must hold a live row in the
        // committed `marrow.ids` ledger; a missing or retired anchor is a
        // precise typed diagnostic carrying the `(kind, path)` gap the mint
        // action consumes.
        let mut anchors: Vec<(IdentityKind, String)> = vec![
            (
                IdentityKind::Application,
                APPLICATION_ANCHOR_PATH.to_string(),
            ),
            (IdentityKind::Root, store.root.root.clone()),
            (
                IdentityKind::Key,
                format!("{}.{}", store.root.root, key_param.name),
            ),
            (IdentityKind::Product, store.resource.clone()),
        ];
        anchors.extend(record.fields.iter().map(|field| {
            (
                IdentityKind::Field,
                format!("{}.{}", store.resource, field.name),
            )
        }));
        let mut ids: Vec<LedgerIdBytes> = Vec::with_capacity(anchors.len());
        let mut complete = true;
        for (kind, path) in &anchors {
            let (live, retired) = match ledger {
                Some(ledger) => (ledger.lookup(*kind, path), ledger.is_retired(*kind, path)),
                None => (None, false),
            };
            match live {
                Some(id) => ids.push(LedgerIdBytes::from_bytes(*id.bytes())),
                None => {
                    complete = false;
                    diagnostics.push(identity_gap(file, store.span, *kind, path, retired));
                }
            }
        }
        if !complete {
            return Self::default();
        }
        let [
            application,
            placement,
            key_identity,
            product,
            field_ids @ ..,
        ] = ids.as_slice()
        else {
            unreachable!("the anchor list always has four leading identities");
        };

        draft.set_application_identity(*application);
        let root_name = draft.intern_string(&store.root.root);
        draft.add_root(RootDef {
            name: root_name,
            key: key.image(),
            record: record.type_id,
            identity: RootIdentity {
                placement: *placement,
                product: *product,
                key: *key_identity,
                fields: field_ids.to_vec(),
            },
        });
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
            root: Some(DurableRoot {
                name: store.root.root.clone(),
                key,
                record: record.type_id,
                entry_site,
                fields,
            }),
        }
    }
}

fn scalar_of(ty: &marrow_syntax::TypeExpr) -> Option<ScalarType> {
    match ty {
        marrow_syntax::TypeExpr::Name { text, .. } => ScalarType::from_spelling(text),
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
