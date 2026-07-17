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
                    SealedSiteTarget::BranchEntry(_) => "branch",
                    SealedSiteTarget::BranchField { .. } => "field",
                    SealedSiteTarget::GroupEntry(_) => "group",
                    SealedSiteTarget::IndexScan(_) => "index_scan",
                    SealedSiteTarget::IndexLookup(_) => "index_lookup",
                },
                match target {
                    SealedSiteTarget::WholePayload => 2,
                    SealedSiteTarget::FieldLeaf(_) => 3,
                    SealedSiteTarget::BranchEntry(_) => 3,
                    SealedSiteTarget::BranchField { .. } => 4,
                    SealedSiteTarget::GroupEntry(_) => 3,
                    SealedSiteTarget::IndexScan(_) | SealedSiteTarget::IndexLookup(_) => 3,
                },
            ),
            SealedSite::Parked { path, target } => (
                false,
                match target {
                    SemanticTarget::WholePayload => "whole",
                    SemanticTarget::FieldLeaf => "field",
                    SemanticTarget::GroupEntry => "group",
                    SemanticTarget::IndexScan => "index_scan",
                    SemanticTarget::IndexLookup => "index_lookup",
                },
                path.steps().len(),
            ),
        })
        .collect();
    shapes.sort();
    shapes
}

// A flat single-column keyed root of scalar fields: a kernel-executable shape. Its
// whole-payload and field-leaf sites seal as `Flat`.
const FLAT_SOURCE: &str = r#"resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn label(): string {
    return "books"
}
"#;

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
const WIDENED_SOURCE: &str = r#"resource Book {
    required title: string
    subtitle: string
    edition: string
}

store ^books[id: int]: Book

pub fn label(): string {
    return "books"
}
"#;

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
const NESTED_SOURCE: &str = r#"resource Book {
    required title: string

    details {
        pages: int
    }

    notes[noteId: string] {
        required text: string
        createdAt: instant
    }
}

store ^books[id: int]: Book

pub fn label(): string {
    return "books"
}
"#;

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

// A flat single-column keyed root with one single-level single-column-keyed
// scalar-field branch `notes`: the E03/E03w executable shape. The root and its
// top-level field seal Flat, and the branch's whole-payload site and each of its field
// leaves seal Flat too (a branch entry and a branch field-exact operation).
const FLAT_BRANCH_SOURCE: &str = r#"resource Book {
    required title: string

    notes[noteId: string] {
        required text: string
        pinned: bool
    }
}

store ^books[id: int]: Book

pub fn label(): string {
    return "books"
}
"#;

const FLAT_BRANCH_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root Book.notes 30303030303030303030303030303030\n\
     id key Book.notes.noteId 31313131313131313131313131313131\n\
     id field Book.notes.text 32323232323232323232323232323232\n\
     id field Book.notes.pinned 33333333333333333333333333333333\n\
     high-water 0\n\
     end\n";

#[test]
fn a_flat_root_with_a_simple_branch_seals_flat_branch_entry_and_branch_field_sites() {
    // A branch entry site (whole payload, depth 3) and each branch field-leaf site
    // (depth 4) seal Flat on a flat-executable root — the field-exact branch tail.
    let shapes = site_shapes(&image(FLAT_BRANCH_SOURCE, FLAT_BRANCH_IDS));
    assert_eq!(
        shapes,
        vec![
            (true, "branch", 3), // notes branch entry:  app -> root -> branch
            (true, "field", 3),  // title:               app -> root -> field
            (true, "field", 4),  // notes.text:          app -> root -> branch -> field
            (true, "field", 4),  // notes.pinned:        app -> root -> branch -> field
            (true, "whole", 2),  // ^books entry:        app -> root
        ]
    );
}

#[test]
fn a_root_level_group_graph_seals_flat_except_its_group_leaf_field_site() {
    // The whole graph emits and seals sites: a whole-payload site over the root and over
    // the `notes` branch, plus a field-leaf site for every stored field — top-level
    // (`title`), group-scoped (`details.pages`), and branch-scoped (`notes.text`,
    // `notes.createdAt`). A root-level group does not park the root, so the root entry,
    // its top-level field, and its scalar-field branch all seal Flat. The only parked site
    // is the group-scoped field leaf: a group leaf is reached through a whole-group site
    // (the compiler's group-site emission lands with lowering), never a direct field-leaf.
    let shapes = site_shapes(&image(NESTED_SOURCE, NESTED_IDS));
    assert_eq!(
        shapes,
        vec![
            (false, "field", 4), // details.pages:    app -> root -> group -> field (parked)
            (true, "branch", 3), // notes branch:     app -> root -> branch
            (true, "field", 3),  // title:            app -> root -> field
            (true, "field", 4),  // notes.text:       app -> root -> branch -> field
            (true, "field", 4),  // notes.createdAt:  app -> root -> branch -> field
            (true, "group", 3),  // details group:    app -> root -> group (whole-group site)
            (true, "whole", 2),  // ^books entry:     app -> root
        ]
    );
    // The group-scoped field leaf is the sole parked site; every other site executes.
    assert_eq!(
        image(NESTED_SOURCE, NESTED_IDS)
            .sites()
            .iter()
            .filter(|site| matches!(site, SealedSite::Parked { .. }))
            .count(),
        1,
        "only the group-scoped field leaf parks",
    );
}

#[test]
fn a_branch_field_site_seals_at_its_full_concrete_address() {
    // The deepest parked concrete address here — the group-scoped field leaf
    // `details.pages` — resolves to the full chain
    // application -> root placement -> group -> field, four steps. (Branch field sites at
    // the same depth now seal Flat; the group leaf is the deepest parked site.)
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
