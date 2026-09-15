//! The generalized operation-site algebra across the durable graph, with lazy
//! field-leaf emission (BND02 C1).
//!
//! The compiler emits the bounded, per-node sites eagerly — one whole-payload site per
//! keyed placement (the store root and every nested `branch`) and one whole-group site per
//! static `group` — so a placement's identity handle exists whether or not code addresses
//! it. Field-leaf sites are emitted **lazily**: a field mints a leaf site only when an
//! instruction addresses it, deduplicated so repeated references share one site. An
//! untouched field therefore mints no site, so the site table — and the durable image —
//! scales with *referenced* fields, not with declared width. The verifier seals each
//! emitted site by resolving its semantic path against its own reconstructed node set: a
//! site on the flat single-column scalar root seals as `Flat` (kernel-executable). These
//! properties are observed through the full production path: capture -> compile -> verify.

use marrow_verify::{SealedSite, SealedSiteTarget, VerifiedImage};

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
                    marrow_verify::SemanticTarget::WholePayload => "whole",
                    marrow_verify::SemanticTarget::FieldLeaf => "field",
                    marrow_verify::SemanticTarget::GroupEntry => "group",
                    marrow_verify::SemanticTarget::IndexScan => "index_scan",
                    marrow_verify::SemanticTarget::IndexLookup => "index_lookup",
                },
                path.steps().len(),
            ),
        })
        .collect();
    shapes.sort();
    shapes
}

/// The number of field-leaf sites (top-level or branch), the lazily-emitted family.
fn field_site_count(image: &VerifiedImage) -> usize {
    image
        .sites()
        .iter()
        .filter(|site| {
            matches!(
                site,
                SealedSite::Flat {
                    target: SealedSiteTarget::FieldLeaf(_) | SealedSiteTarget::BranchField { .. },
                    ..
                } | SealedSite::Parked {
                    target: marrow_verify::SemanticTarget::FieldLeaf,
                    ..
                }
            )
        })
        .count()
}

// A flat single-column keyed root of scalar fields. `getTitle` reads `title`; `subtitle`
// is declared but never addressed.
const FLAT_SOURCE: &str = r#"resource Book {
    required title: string
    subtitle: string
}

store ^books[id: int]: Book

pub fn getTitle(id: int): string? {
    return ^books[id].title
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

// FLAT_SOURCE plus an appended sparse `edition` field that no code addresses: the record
// (broad demand) grows, but the field-leaf site table does not.
const WIDENED_SOURCE: &str = r#"resource Book {
    required title: string
    subtitle: string
    edition: string
}

store ^books[id: int]: Book

pub fn getTitle(id: int): string? {
    return ^books[id].title
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

// A flat root with one single-level single-column-keyed scalar-field branch `notes`.
// `getText` reads the branch field `notes.text`; `title` and `notes.pinned` are untouched.
const FLAT_BRANCH_SOURCE: &str = r#"resource Book {
    required title: string

    notes[noteId: string] {
        required text: string
        pinned: bool
    }
}

store ^books[id: int]: Book

pub fn getText(id: int, n: string): string? {
    return ^books[id].notes[n].text
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

// A resource with a top-level field, a static `group` holding a field, and a keyed
// `branch`. `getTitle` addresses only `title`, so no group- or branch-scoped field leaf is
// minted; the eager whole-payload, group-entry, and branch-entry sites are present anyway.
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

pub fn getTitle(id: int): string? {
    return ^books[id].title
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
fn a_flat_root_emits_a_field_site_only_for_a_referenced_field() {
    // `getTitle` reads `title` and never `subtitle`, so the site table is the eager
    // whole-payload site plus exactly one field-leaf site — `title`'s. The declared but
    // untouched `subtitle` mints no site: field-leaf emission is lazy.
    let shapes = site_shapes(&image(FLAT_SOURCE, FLAT_IDS));
    assert_eq!(
        shapes,
        vec![
            (true, "field", 3), // title (referenced)
            (true, "whole", 2), // ^books entry (eager)
        ],
        "an untouched field mints no site",
    );
}

#[test]
fn a_flat_branch_emits_eager_entry_sites_and_a_field_site_only_for_a_referenced_branch_field() {
    // `getText` reads `notes.text`. The whole-payload and branch-entry sites are eager
    // (present regardless of what code touches); the only field-leaf site is the referenced
    // `notes.text`. `title` and `notes.pinned`, untouched, mint no site.
    let shapes = site_shapes(&image(FLAT_BRANCH_SOURCE, FLAT_BRANCH_IDS));
    assert_eq!(
        shapes,
        vec![
            (true, "branch", 3), // notes branch entry (eager): app -> root -> branch
            (true, "field", 4),  // notes.text (referenced):    app -> root -> branch -> field
            (true, "whole", 2),  // ^books entry (eager):       app -> root
        ]
    );
}

#[test]
fn a_referenced_branch_field_seals_flat_at_its_four_step_address() {
    // The referenced branch field `notes.text` seals Flat at its full concrete address:
    // application -> root placement -> branch placement -> field, four steps.
    let image = image(FLAT_BRANCH_SOURCE, FLAT_BRANCH_IDS);
    let branch_levels = image
        .sites()
        .iter()
        .find_map(|site| match site {
            SealedSite::Flat {
                target: SealedSiteTarget::BranchField { branch, .. },
                ..
            } => Some(branch.len()),
            _ => None,
        })
        .expect("the referenced branch field seals as a Flat branch-field site");
    // One branch level -> the concrete address application -> root -> branch -> field,
    // four steps.
    assert_eq!(
        branch_levels, 1,
        "a single-level branch field is a four-step (2 + branch levels + field) address",
    );
}

#[test]
fn a_root_level_group_emits_its_eager_group_entry_site_and_no_untouched_field_leaf() {
    // `getTitle` touches only `title`. The whole-payload, root-level group-entry, and
    // branch-entry sites are all eager and present; the only field-leaf site is `title`.
    // No group-scoped (`details.pages`) or branch-scoped (`notes.*`) field leaf is minted,
    // because nothing addresses them — group leaves are reached through the whole-group
    // site, and the branch fields are untouched.
    let shapes = site_shapes(&image(NESTED_SOURCE, NESTED_IDS));
    assert_eq!(
        shapes,
        vec![
            (true, "branch", 3), // notes branch entry (eager)
            (true, "field", 3),  // title (referenced)
            (true, "group", 3),  // details group entry (eager whole-group site)
            (true, "whole", 2),  // ^books entry (eager)
        ]
    );
    assert_eq!(
        field_site_count(&image(NESTED_SOURCE, NESTED_IDS)),
        1,
        "only the referenced top-level field mints a leaf site",
    );
}

#[test]
fn appending_an_untouched_optional_field_grows_broad_demand_but_adds_no_site() {
    // Broad (whole-payload) demand derives from the contract, so appending a sparse field
    // grows the root's record. But that field is not addressed by any code, so — with lazy
    // field-leaf emission — no new site is minted: the site table is byte-for-byte the same.
    // This is the width decoupling: image cost tracks referenced fields, not declared width.
    let before = image(FLAT_SOURCE, FLAT_IDS);
    let after = image(WIDENED_SOURCE, WIDENED_IDS);

    let before_record = &before.record_types()[before.roots()[0].record() as usize];
    let after_record = &after.record_types()[after.roots()[0].record() as usize];
    assert_eq!(before_record.fields().len(), 2);
    assert_eq!(
        after_record.fields().len(),
        3,
        "broad demand follows the contract",
    );

    // Every prior site is unchanged, and no new site is added for the untouched field.
    for site in before.sites() {
        assert!(
            after.sites().contains(site),
            "every prior site is unchanged when an untouched field is appended",
        );
    }
    assert_eq!(
        after.sites().len(),
        before.sites().len(),
        "appending an untouched field adds no site — declared width does not drive the table",
    );
}

#[test]
fn the_durable_site_table_scales_with_referenced_fields_not_declared_width() {
    // The field-leaf site family is lazy and deduplicated. Reading a field any number of
    // times mints one site for it; declaring more fields the code never touches mints none.
    let one_read = image(FLAT_SOURCE, FLAT_IDS);
    // `title` read twice: still one `title` site (dedup).
    let two_reads = image(
        &FLAT_SOURCE.replace(
            "return ^books[id].title\n",
            "const a = ^books[id].title\n    return ^books[id].title\n",
        ),
        FLAT_IDS,
    );
    assert_eq!(
        field_site_count(&one_read),
        1,
        "one referenced field mints one leaf site",
    );
    assert_eq!(
        field_site_count(&two_reads),
        1,
        "reading the same field twice still mints one leaf site (dedup)",
    );

    // Declaring a wider record whose extra field is untouched adds no field-leaf site.
    let widened = image(WIDENED_SOURCE, WIDENED_IDS);
    assert_eq!(
        field_site_count(&widened),
        1,
        "an extra declared-but-untouched field adds no leaf site",
    );
}
