//! The durable graph registry (design §B/§C).
//!
//! The durable graph admits at most one `store` root over the project's single
//! resource record. A root is a *singleton* (`store ^root: Record`, no key) or a
//! *keyed tuple* (`store ^root(k1: K1, k2: K2): Record`, one or more ordered
//! orderable durable-key columns). Every admitted root is a distinct graph node
//! with a complete ledger identity — its placement, its stored product, one
//! identity per key column, and one per stored field — and a slot in the image
//! DURABLE table the verifier independently re-encodes.
//!
//! The executable durable subset the single-root kernel can serve at this stage is
//! the single-column keyed root. A singleton or composite-key root completes its
//! identity and verifies, but has no executable operation sites: an operation over
//! one is a precise typed `check.unsupported` rejection at lowering (the wider key
//! arities execute at E01). This module validates the declaration, adds the root
//! and — for the executable subset — its operation sites to the draft, and exposes
//! the resolved sites the function lowerer emits against.

use marrow_codes::Code;
use marrow_image::{
    ImageDraft, KeyColumn, LedgerIdBytes, RootDef, RootIdentity, SiteDef, SiteTarget, bounds,
};
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

/// The project's single executable durable root and its operation sites. Only a
/// single-column keyed root reaches this form; its single key scalar backs the
/// kernel-serviceable read/write path.
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
/// single-column keyed root the kernel can serve; `declared_root` names any
/// admitted root (singleton, single-column, or composite) so a durable operation
/// over a not-yet-executable shape reports precisely rather than as "no store".
#[derive(Default)]
pub(crate) struct DurableRegistry {
    executable: Option<DurableRoot>,
    declared_root: Option<String>,
}

impl DurableRegistry {
    /// The executable single-column keyed root, if the project declares one.
    pub(crate) fn root(&self) -> Option<&DurableRoot> {
        self.executable.as_ref()
    }

    /// The name of a declared root whose key arity the kernel cannot yet serve
    /// (singleton or composite). `Some` exactly when a root is declared but not
    /// executable, so the lowerer can distinguish a not-yet-executable operation
    /// from an operation with no store at all.
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
    /// incomplete: every durable declaration must have a live row in the committed
    /// `marrow.ids` ledger, or the declaration fails precisely with
    /// `check.durable_identity`. The compiler only *reads* the ledger; minting
    /// lives in the `marrow run` convenience action (and in the accepted apply
    /// action when it lands).
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
        if store.root.keys.len() > bounds::MAX_KEY_COLUMNS {
            diagnostics.push(unsupported(
                file,
                store.root.span,
                "a store key with more than eight columns",
            ));
            return Self::default();
        }
        // Resolve each key column's scalar in declared tuple order. A singleton
        // root has no columns.
        let mut key_scalars: Vec<ScalarType> = Vec::with_capacity(store.root.keys.len());
        for key_param in &store.root.keys {
            let Some(key) = scalar_of(&records.expand(&key_param.ty)) else {
                diagnostics.push(unsupported(file, store.root.span, "this key type"));
                return Self::default();
            };
            // The closed orderable durable-key scalar set (frozen at C04): int,
            // string, bool, bytes, date, and instant. `duration` is a span, not an
            // identity, so it is not a durable key.
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
                    "a store key column must be an orderable durable-key scalar (int, string, bool, bytes, date, or instant)"
                        .to_string(),
                ));
                return Self::default();
            }
            key_scalars.push(key);
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
        // declaration — the application, the root placement, its stored product,
        // each key column, and each stored field — must hold a live row in the
        // committed `marrow.ids` ledger; a missing or retired anchor is a precise
        // typed diagnostic carrying the `(kind, path)` gap the mint action
        // consumes.
        let mut resolver = IdentityResolver::new(file, store.span, ledger, diagnostics);
        let application = resolver.resolve(IdentityKind::Application, APPLICATION_ANCHOR_PATH);
        let placement = resolver.resolve(IdentityKind::Root, &store.root.root);
        let product = resolver.resolve(IdentityKind::Product, &store.resource);
        let key_ids: Vec<Option<LedgerIdBytes>> = store
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
        let field_ids: Vec<Option<LedgerIdBytes>> = record
            .fields
            .iter()
            .map(|field| {
                resolver.resolve(
                    IdentityKind::Field,
                    &format!("{}.{}", store.resource, field.name),
                )
            })
            .collect();

        // Every identity must resolve before the graph enters the image; a single
        // gap already reported precisely leaves the durable graph absent.
        // A durable graph whose identity is incomplete has already reported a
        // precise `check.durable_identity` gap per missing anchor; it does not
        // enter the image and leaves no declared-root marker, so an operation over
        // it is not additionally mislabelled "not yet executable" (the identity gap
        // is the diagnosis, whatever the root's key arity).
        let (Some(application), Some(placement), Some(product)) = (application, placement, product)
        else {
            return Self::default();
        };
        let Some(key_ids) = key_ids.into_iter().collect::<Option<Vec<_>>>() else {
            return Self::default();
        };
        let Some(field_ids) = field_ids.into_iter().collect::<Option<Vec<_>>>() else {
            return Self::default();
        };

        draft.set_application_identity(application);
        let root_name = draft.intern_string(&store.root.root);
        let keys: Vec<KeyColumn> = key_scalars
            .iter()
            .zip(&key_ids)
            .map(|(scalar, id)| KeyColumn {
                scalar: scalar.image(),
                id: *id,
            })
            .collect();
        draft.add_root(RootDef {
            name: root_name,
            keys,
            record: record.type_id,
            identity: RootIdentity {
                placement,
                product,
                fields: field_ids,
            },
        });

        // Operation sites — and therefore executable durable operations — exist
        // only for the single-column keyed root the kernel can serve. A singleton
        // or composite-key root carries its identity but no sites; the lowerer
        // reports any operation over it as not yet executable.
        let [key] = key_scalars.as_slice() else {
            return Self::declared(&store.root.root);
        };
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
    /// serve its key arity (a singleton or composite-key root). Used only after the
    /// root has entered the draft.
    fn declared(root: &str) -> Self {
        Self {
            executable: None,
            declared_root: Some(root.to_string()),
        }
    }
}

/// Resolves durable `(kind, path)` anchors against the committed ledger, pushing a
/// precise `check.durable_identity` diagnostic for each missing or retired anchor.
/// Returning `Option` per anchor lets the caller report every gap in one pass
/// rather than stopping at the first.
struct IdentityResolver<'a> {
    file: &'a str,
    span: SourceSpan,
    ledger: Option<&'a IdentityLedger>,
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
            diagnostics,
        }
    }

    fn resolve(&mut self, kind: IdentityKind, path: &str) -> Option<LedgerIdBytes> {
        let (live, retired) = match self.ledger {
            Some(ledger) => (ledger.lookup(kind, path), ledger.is_retired(kind, path)),
            None => (None, false),
        };
        match live {
            Some(id) => Some(LedgerIdBytes::from_bytes(*id.bytes())),
            None => {
                self.diagnostics
                    .push(identity_gap(self.file, self.span, kind, path, retired));
                None
            }
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
