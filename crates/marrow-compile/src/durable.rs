//! The durable store registry (design §B/§C).
//!
//! The T01 subset admits exactly one durable root: a `store ^root(key): Record`
//! over the project's single record type, no indexes and no singleton roots. This
//! module validates that declaration, adds the root and its operation sites to the
//! image draft, and exposes the resolved sites the function lowerer emits against.

use marrow_codes::Code;
use marrow_image::{ImageDraft, RootDef, SiteDef, SiteTarget};
use marrow_syntax::{SourceSpan, StoreDecl};

use crate::diag::SourceDiagnostic;
use crate::record::RecordRegistry;
use crate::scalar::ScalarType;

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
    /// admitted root and its sites to the draft. More than one store, an index, a
    /// missing or mismatched resource, a multi-column key, or a non-`int`/`string`
    /// key are rejected.
    pub(crate) fn build(
        draft: &mut ImageDraft,
        records: &RecordRegistry,
        stores: &[(String, &StoreDecl)],
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
        let Some(key) = scalar_of(&key_param.ty) else {
            diagnostics.push(unsupported(file, store.root.span, "this key type"));
            return Self::default();
        };
        if !matches!(key, ScalarType::Int | ScalarType::Text) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckType.as_str(),
                file,
                store.root.span,
                "a store key must be `int` or `string`".to_string(),
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

        let root_name = draft.intern_string(&store.root.root);
        draft.add_root(RootDef {
            name: root_name,
            key: key.image(),
            record: record.type_id,
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
                    scalar: field.scalar,
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

fn unsupported(file: &str, span: SourceSpan, subject: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        span,
        format!("{subject} is not yet supported on the beta line"),
    )
}
