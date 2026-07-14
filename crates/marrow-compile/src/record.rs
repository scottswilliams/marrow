//! The project record type (design §B).
//!
//! The T01 subset admits exactly one record type per project: a `resource` with
//! required and sparse scalar fields, no groups and no keyed children. This module
//! lowers that declaration into the image [`RecordTypeDef`] and a registry the
//! function lowerer consults to resolve constructors and field reads.

use marrow_codes::Code;
use marrow_image::{FieldDef, ImageDraft, RecordTypeDef, TypeId};
use marrow_syntax::{ResourceDecl, ResourceMember, TypeExpr};

use crate::diag::SourceDiagnostic;
use crate::scalar::ScalarType;

/// One resolved record field, in declaration order.
pub(crate) struct FieldInfo {
    pub(crate) name: String,
    pub(crate) scalar: ScalarType,
    pub(crate) required: bool,
}

/// The project's single record type.
pub(crate) struct RecordInfo {
    pub(crate) type_id: TypeId,
    pub(crate) name: String,
    pub(crate) fields: Vec<FieldInfo>,
}

impl RecordInfo {
    pub(crate) fn field(&self, name: &str) -> Option<(u16, &FieldInfo)> {
        self.fields
            .iter()
            .enumerate()
            .find(|(_, field)| field.name == name)
            .map(|(index, field)| (index as u16, field))
    }
}

/// The project record registry: zero or one record type.
#[derive(Default)]
pub(crate) struct RecordRegistry {
    record: Option<RecordInfo>,
}

impl RecordRegistry {
    pub(crate) fn by_name(&self, name: &str) -> Option<&RecordInfo> {
        self.record.as_ref().filter(|info| info.name == name)
    }

    pub(crate) fn by_name_for_type(&self, ty: TypeId) -> Option<&RecordInfo> {
        self.record.as_ref().filter(|info| info.type_id == ty)
    }

    /// Build the registry from the project's resource declarations, adding the one
    /// admitted record type to the draft. More than one resource, groups, keyed
    /// fields, or non-scalar field types are rejected.
    pub(crate) fn build(
        draft: &mut ImageDraft,
        resources: &[(String, &ResourceDecl)],
        diagnostics: &mut Vec<SourceDiagnostic>,
    ) -> Self {
        if resources.len() > 1 {
            for (file, resource) in &resources[1..] {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckUnsupported.as_str(),
                    file,
                    resource.name_span,
                    "the beta line admits only one resource type per project".to_string(),
                ));
            }
        }
        let Some((file, resource)) = resources.first() else {
            return Self::default();
        };

        let mut fields = Vec::new();
        let mut field_defs = Vec::new();
        let name_id = draft.intern_string(&resource.name);
        for member in &resource.members {
            let ResourceMember::Field(field) = member else {
                diagnostics.push(unsupported(file, member.span(), "a resource group"));
                continue;
            };
            if !field.keys.is_empty() {
                diagnostics.push(unsupported(file, field.span, "a keyed field"));
                continue;
            }
            let Some(scalar) = scalar_of(&field.ty) else {
                diagnostics.push(unsupported(file, field.ty.span(), "this field type"));
                continue;
            };
            let field_name_id = draft.intern_string(&field.name);
            field_defs.push(FieldDef {
                name: field_name_id,
                ty: scalar.image(),
                required: field.required,
            });
            fields.push(FieldInfo {
                name: field.name.clone(),
                scalar,
                required: field.required,
            });
        }

        let type_id = draft.add_record_type(RecordTypeDef {
            name: name_id,
            fields: field_defs,
        });
        Self {
            record: Some(RecordInfo {
                type_id,
                name: resource.name.clone(),
                fields,
            }),
        }
    }
}

fn scalar_of(ty: &TypeExpr) -> Option<ScalarType> {
    match ty {
        TypeExpr::Name { text, .. } => ScalarType::from_spelling(text),
        _ => None,
    }
}

fn unsupported(file: &str, span: marrow_syntax::SourceSpan, subject: &str) -> SourceDiagnostic {
    SourceDiagnostic::at(
        Code::CheckUnsupported.as_str(),
        file,
        span,
        format!("{subject} is not yet supported on the beta line"),
    )
}
