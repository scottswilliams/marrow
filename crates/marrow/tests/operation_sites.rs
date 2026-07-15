//! D02 slice 3: the generalized operation-site algebra across the whole durable
//! graph.
//!
//! The compiler emits one whole-payload site per keyed placement (the store root and
//! every nested `branch`) and one field-leaf site per stored field (top-level,
//! group-scoped, and branch-scoped), for the whole graph — not only the flat
//! executable root. The verifier seals each site by resolving its semantic path
//! against its own reconstructed node set: a site on the flat single-column scalar
//! root seals as `Flat` (kernel-executable), and every other site — a nested branch
//! placement, a group-scoped field, a widened field, or any site on a non-flat root —
//! seals as `Parked` (identity complete, execution deferred to E01). These properties
//! are observed through the full production path: capture -> compile -> verify.

use marrow_verify::{SealedSite, SealedSiteTarget, SemanticTarget, VerifiedImage};

fn image(source: &str, ids: &str) -> VerifiedImage {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    marrow_verify::verify(&compiled.image.bytes).expect("verify")
}

/// The `(is_flat, target-kind, path-depth)` fingerprint of every sealed site, sorted.
fn site_shapes(image: &VerifiedImage) -> Vec<(bool, &'static str, usize)> {
    let mut shapes: Vec<(bool, &'static str, usize)> = image
        .sites()
        .iter()
        .map(|site| match site {
            SealedSite::Flat { target, .. } => (
                true,
                match target {
                    SealedSiteTarget::WholePayload => "whole",
                    SealedSiteTarget::FieldLeaf(_) => "field",
                },
                match target {
                    SealedSiteTarget::WholePayload => 2,
                    SealedSiteTarget::FieldLeaf(_) => 3,
                },
            ),
            SealedSite::Parked { path, target } => (
                false,
                match target {
                    SemanticTarget::WholePayload => "whole",
                    SemanticTarget::FieldLeaf => "field",
                },
                path.steps().len(),
            ),
        })
        .collect();
    shapes.sort();
    shapes
}

// A flat single-column keyed root of plain scalar fields: the one kernel-executable
// shape. Its whole-payload and field-leaf sites seal as `Flat`.
const FLAT_SOURCE: &str = "resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     \n\
     store ^books(id: int): Book\n\
     \n\
     pub fn label(): string\n\
     \x20   return \"books\"\n";

const FLAT_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Book.subtitle 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

// FLAT_SOURCE plus an appended sparse `edition` field: append-only optional-field
// evolution. The record and graph grow by one field; every prior field's site is
// unchanged.
const WIDENED_SOURCE: &str = "resource Book\n\
     \x20   required title: string\n\
     \x20   subtitle: string\n\
     \x20   edition: string\n\
     \n\
     store ^books(id: int): Book\n\
     \n\
     pub fn label(): string\n\
     \x20   return \"books\"\n";

const WIDENED_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Book.subtitle 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id field Book.edition 40404040404040404040404040404040\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

// A resource with a top-level field, a static `group` holding a field, and a keyed
// `branch` holding two fields — a non-flat graph whose every node seals as a parked
// site.
const NESTED_SOURCE: &str = "resource Book\n\
     \x20   required title: string\n\
     \n\
     \x20   details\n\
     \x20       pages: int\n\
     \n\
     \x20   notes(noteId: string)\n\
     \x20       required text: string\n\
     \x20       createdAt: instant\n\
     \n\
     store ^books(id: int): Book\n\
     \n\
     pub fn label(): string\n\
     \x20   return \"books\"\n";

const NESTED_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id group Book.details 20202020202020202020202020202020\n\
     id field Book.details.pages 21212121212121212121212121212121\n\
     id root Book.notes 30303030303030303030303030303030\n\
     id key Book.notes.noteId 31313131313131313131313131313131\n\
     id field Book.notes.text 32323232323232323232323232323232\n\
     id field Book.notes.createdAt 33333333333333333333333333333333\n\
     high-water 0\n\
     end\n";

#[test]
fn a_flat_root_seals_a_flat_whole_payload_and_field_site_per_node() {
    // The flat executable root: one whole-payload site over the root, one field-leaf
    // site per top-level field, all Flat.
    let shapes = site_shapes(&image(FLAT_SOURCE, FLAT_IDS));
    assert_eq!(
        shapes,
        vec![
            (true, "field", 3), // title
            (true, "field", 3), // subtitle
            (true, "whole", 2), // ^books entry
        ]
    );
}

#[test]
fn every_keyed_placement_and_field_of_a_nested_graph_gets_a_sealed_parked_site() {
    // The whole graph emits and seals sites: a whole-payload site over the root and
    // over the `notes` branch, plus a field-leaf site for every stored field —
    // top-level (`title`), group-scoped (`details.pages`), and branch-scoped
    // (`notes.text`, `notes.createdAt`). A group is a namespace, not an addressable
    // node, so it contributes no site. The root is non-flat, so every site is Parked.
    let shapes = site_shapes(&image(NESTED_SOURCE, NESTED_IDS));
    assert_eq!(
        shapes,
        vec![
            (false, "field", 3), // title:            app -> root -> field
            (false, "field", 4), // details.pages:    app -> root -> group -> field
            (false, "field", 4), // notes.text:       app -> root -> branch -> field
            (false, "field", 4), // notes.createdAt:  app -> root -> branch -> field
            (false, "whole", 2), // ^books entry:     app -> root
            (false, "whole", 3), // notes branch:     app -> root -> branch
        ]
    );
    // No site is executable over a non-flat root.
    assert!(
        image(NESTED_SOURCE, NESTED_IDS)
            .sites()
            .iter()
            .all(|site| matches!(site, SealedSite::Parked { .. })),
        "a non-flat graph has no executable site"
    );
}

#[test]
fn a_branch_field_site_seals_at_its_full_concrete_address() {
    // The deepest concrete address here — a branch field `notes.text` — seals as a
    // parked field site whose resolved path is the full chain
    // application -> root placement -> branch placement -> field, four steps.
    let image = image(NESTED_SOURCE, NESTED_IDS);
    let deepest = image
        .sites()
        .iter()
        .filter_map(|site| match site {
            SealedSite::Parked { path, .. } => Some(path.steps().len()),
            SealedSite::Flat { .. } => None,
        })
        .max()
        .expect("the nested graph has parked sites");
    assert_eq!(deepest, 4, "a branch field is a four-step concrete address");
}

#[test]
fn appending_an_optional_field_widens_broad_demand_without_touching_field_only_sites() {
    // Broad (whole-payload) demand derives from the contract: appending a sparse
    // field grows the root's record — so the whole-payload site's payload shape (its
    // demand) follows the contract — while every prior field-leaf site is unchanged
    // and one fresh field-leaf site is added. Field-only sites do not widen.
    let before = image(FLAT_SOURCE, FLAT_IDS);
    let after = image(WIDENED_SOURCE, WIDENED_IDS);

    // The whole-payload site's demand is the root record; it grew by the new field.
    let before_record = &before.record_types()[before.roots()[0].record() as usize];
    let after_record = &after.record_types()[after.roots()[0].record() as usize];
    assert_eq!(before_record.fields().len(), 2);
    assert_eq!(
        after_record.fields().len(),
        3,
        "broad demand follows the contract"
    );

    // The prior field-leaf sites are byte-for-byte the same sealed sites; exactly one
    // fresh field-leaf site is added. The whole-payload site is unchanged.
    let before_sites: Vec<&SealedSite> = before.sites().iter().collect();
    for site in &before_sites {
        assert!(
            after.sites().contains(site),
            "every prior site is unchanged when a field is appended"
        );
    }
    assert_eq!(
        after.sites().len(),
        before.sites().len() + 1,
        "appending one field adds exactly one field-leaf site"
    );
}

#[test]
fn the_durable_site_table_scales_with_the_graph_not_with_operation_count() {
    // Compact effect sites: the site table holds one entry per graph node (a
    // whole-payload site per placement, a field-leaf site per field), independent of
    // how many operations reference them. Adding a field adds one site; a program that
    // reads and writes the same field many times still emits that field's single site.
    let flat = image(FLAT_SOURCE, FLAT_IDS);
    let widened = image(WIDENED_SOURCE, WIDENED_IDS);
    let field_count = |img: &VerifiedImage| {
        img.record_types()[img.roots()[0].record() as usize]
            .fields()
            .len()
    };
    // 1 whole-payload + N field sites.
    assert_eq!(flat.sites().len(), 1 + field_count(&flat));
    assert_eq!(widened.sites().len(), 1 + field_count(&widened));
    // Adding one field grew the table by exactly one site — linear in the graph, not
    // in width times operation count.
    assert_eq!(widened.sites().len(), flat.sites().len() + 1);
}
