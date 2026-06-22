use std::collections::HashMap;
use std::path::Path;

use crate::AnalysisSnapshot;

use super::{ByteSpan, SourceSemanticTokenRole, SourceSemanticTokenStyle, byte_span};
use crate::tooling::type_annotations::identity_type_annotations;

pub(super) fn identity_type_annotation_overrides(
    snapshot: &AnalysisSnapshot,
    path: &Path,
) -> HashMap<ByteSpan, SourceSemanticTokenStyle> {
    identity_type_annotations(snapshot, path)
        .into_iter()
        .map(|fact| {
            (
                byte_span(fact.constructor_span),
                SourceSemanticTokenStyle::plain(SourceSemanticTokenRole::IdentityTypeConstructor),
            )
        })
        .collect()
}
