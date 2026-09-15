//! Fixtures shared by the generic-registry test batteries: a leaked armed draft, a
//! bare type-name annotation, an empty registry carrying only templates, a mint
//! site, and the two invariant takers.
//!
//! One owner, so a registry field added to [`TypeRegistry`] is added here once and
//! every battery keeps building the same fixture.

use super::*;

use crate::compile::admitted;
use marrow_image::ImageDraft;

/// A fresh armed transaction over its own leaked owner, for fixtures that never
/// touch the owner again.
pub(super) fn fresh_draft() -> DraftTxn<'static> {
    let owner: &'static mut ImageDraft = Box::leak(Box::new(ImageDraft::new()));
    admitted(owner)
}

/// A bare `Name` type annotation with no spans.
pub(super) fn name(text: &str) -> TypeExpr {
    TypeExpr::Name {
        text: text.to_string(),
        segment_spans: Vec::new(),
        span: SourceSpan::default(),
    }
}

/// An empty registry carrying `templates` and nothing else.
pub(super) fn test_registry(templates: Vec<TypeTemplate>) -> TypeRegistry {
    TypeRegistry {
        named: DeclarationLedger::new(
            DeclarationNamespace::NamedType,
            DeclarationBudget::default(),
        ),
        members: DeclarationLedger::new(
            DeclarationNamespace::ResourceMember,
            DeclarationBudget::default(),
        ),
        aliases: AliasTable::default(),
        nominals: Vec::new(),
        structs: Vec::new(),
        enums: Vec::new(),
        records: AdmittedRecords::default(),
        type_templates: templates,
        generics: RefCell::default(),
        collections: RefCell::default(),
        collection_index: RefCell::default(),
        row_directory: RefCell::default(),
        coordinates: DeclarationCoordinates::default(),
    }
}

/// A mint site in the one test file, at `line`.
pub(super) fn site(line: u32) -> MintSite<'static> {
    MintSite {
        file: crate::test_main_file_identity(),
        span: SourceSpan {
            line,
            column: 9,
            ..SourceSpan::default()
        },
    }
}

/// The encoded bytes and id of a draft, the shape a "nothing was written" assertion
/// compares.
pub(super) fn draft_fingerprint(draft: &ImageDraft) -> (Vec<u8>, marrow_image::ImageId) {
    let encoded = draft.encode().expect("test draft encodes");
    (encoded.bytes, encoded.image_id)
}

pub(super) fn take_resolve_invariant<T>(result: Result<T, ResolveError>) -> GenericInvariant {
    match result {
        Err(ResolveError::Invariant(invariant)) => invariant,
        Err(ResolveError::Refusal(_)) => {
            panic!("malformed Ready metadata must not become a source refusal")
        }
        Ok(_) => panic!("malformed Ready metadata must not reach a semantic reader"),
    }
}

pub(super) fn take_reader_invariant<T>(result: Result<T, GenericInvariant>) -> GenericInvariant {
    match result {
        Err(invariant) => invariant,
        Ok(_) => panic!("malformed Ready metadata must not reach a semantic reader"),
    }
}
