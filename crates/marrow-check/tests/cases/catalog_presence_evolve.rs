use crate::support;
use marrow_catalog::{CatalogEntry, CatalogEntryKind, CatalogLifecycle, CatalogMetadata};
use marrow_check::{
    CHECK_CATALOG_INTENT, CatalogIntentDiagnostic, CatalogIntentKind, CatalogPathCandidate,
    CheckReport, DiagnosticPayload,
};

use support::catalog::{
    catalog, derived_id, entry_for_label as entry,
    store_index_entry_for_label as store_index_entry, write_catalog,
};
use support::{check_with_accepted, temp_project, write};

fn reserved_entry(
    kind: CatalogEntryKind,
    canonical_path: &str,
    label: &str,
    aliases: &[&str],
) -> CatalogEntry {
    CatalogEntry {
        lifecycle: CatalogLifecycle::Reserved,
        ..entry(kind, canonical_path, label, aliases)
    }
}

/// A rejected retire must never reserve its target. The target keeps its
/// accepted lifecycle: when the binding produced a proposal the entry is Active
/// there, and when nothing else changed no proposal is emitted at all, so the
/// accepted catalog (which had the entry Active) stands unchanged.
fn assert_entry_stays_active(program: &marrow_check::CheckedProgram, stable_id: &str) {
    let Some(proposal) = &program.catalog.proposal else {
        return;
    };
    let entry = proposal
        .entries
        .iter()
        .find(|entry| entry.stable_id == derived_id(stable_id))
        .expect("proposal must keep the retire target entry");
    assert_eq!(
        entry.lifecycle,
        CatalogLifecycle::Active,
        "a retire the source still declares must not reserve the entry: {entry:#?}"
    );
}

/// A source-dropped member that a rejected intent neither relocated nor reserved is a bare
/// removal: the projection drops it so an in-place run matches a reseed, which never records it.
/// The ambiguity diagnostic still fires; the entry simply does not linger as a phantom.
fn assert_member_dropped(program: &marrow_check::CheckedProgram, stable_id: &str) {
    let Some(proposal) = &program.catalog.proposal else {
        return;
    };
    assert!(
        !proposal
            .entries
            .iter()
            .any(|entry| entry.stable_id == derived_id(stable_id)),
        "a source-dropped member must not be carried forward in the proposal: {:#?}",
        proposal.entries
    );
}

fn assert_no_catalog_entry_at(program: &marrow_check::CheckedProgram, stable_id: &str, path: &str) {
    if let Some(proposal) = &program.catalog.proposal {
        assert!(
            !proposal
                .entries
                .iter()
                .any(|entry| entry.stable_id == derived_id(stable_id) && entry.path == path),
            "a rejected intent must not move `{stable_id}` to `{path}`: {:#?}",
            proposal.entries
        );
    }
}

fn accepted_candidate(
    kind: CatalogEntryKind,
    lifecycle: CatalogLifecycle,
    label: &str,
) -> CatalogPathCandidate {
    CatalogPathCandidate {
        kind,
        lifecycle,
        stable_id: derived_id(label),
    }
}

fn assert_catalog_path_ambiguity(
    report: &CheckReport,
    intent: CatalogIntentKind,
    path: &str,
    accepted: Vec<CatalogPathCandidate>,
    source: Vec<CatalogEntryKind>,
) {
    let payload = report
        .diagnostics
        .iter()
        .find_map(|diagnostic| match &diagnostic.payload {
            DiagnosticPayload::CatalogIntent(CatalogIntentDiagnostic::AmbiguousPath {
                intent: actual_intent,
                path: actual_path,
                accepted: actual_accepted,
                source: actual_source,
            }) if *actual_intent == intent && actual_path == path => {
                Some((actual_accepted, actual_source))
            }
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "expected catalog ambiguity payload for {intent:?} `{path}`: {:#?}",
                report.diagnostics
            )
        });
    assert_eq!(payload.0, &accepted);
    assert_eq!(payload.1, &source);
}

#[test]
fn evolve_rename_authorizes_a_saved_data_backed_member_rename() {
    let root = temp_project("evolve-rename-member", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.title -> Book.subtitle\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "member-title",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "evolve rename intent must satisfy the catalog binding: {:#?}",
        report.diagnostics
    );
    let proposal = program.catalog.proposal.expect("proposal");
    CatalogMetadata::from_json(&proposal.to_json_pretty().expect("catalog renders"))
        .expect("proposal validates");
    let renamed = proposal
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "books::Book::subtitle"
        })
        .expect("renamed member entry");
    assert_eq!(renamed.stable_id, derived_id("member-title"));
    assert_eq!(renamed.lifecycle, CatalogLifecycle::Active);
    assert!(
        renamed
            .aliases
            .iter()
            .any(|alias| alias == "books::Book::title"),
        "old path must be recorded as an alias: {renamed:#?}"
    );
    // No stale entry remains at the old member path.
    assert!(
        !proposal
            .entries
            .iter()
            .any(|entry| entry.path == "books::Book::title"),
        "the old path must not linger as a separate entry: {:#?}",
        proposal.entries
    );
}

/// A round-trip rename (`title` -> `name` -> `title`) carries the same stable
/// identity back onto a path it already records as an alias from the first leg.
/// That new canonical path is canonical again for the same id, so the now-redundant
/// alias is dropped instead of self-colliding with the entry's own canonical path.
#[test]
fn evolve_rename_round_trip_onto_a_former_canonical_path_succeeds() {
    let root = temp_project("evolve-rename-round-trip-member", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.name -> Book.title\n",
        );
        // The accepted catalog reflects the first leg (`title` -> `name`): the member is
        // canonical at `name` and records `title` as an alias under the preserved id.
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::name",
                "member-title",
                &["books::Book::title"],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "a round-trip rename onto a former canonical path must satisfy the binding: {:#?}",
        report.diagnostics
    );
    let proposal = program.catalog.proposal.expect("proposal");
    CatalogMetadata::from_json(&proposal.to_json_pretty().expect("catalog renders"))
        .expect("proposal validates");
    let renamed = proposal
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "books::Book::title"
        })
        .expect("renamed member entry");
    assert_eq!(renamed.stable_id, derived_id("member-title"));
    assert_eq!(renamed.lifecycle, CatalogLifecycle::Active);
    assert!(
        !renamed
            .aliases
            .iter()
            .any(|alias| alias == "books::Book::title"),
        "the redundant alias equal to the new canonical path must be dropped: {renamed:#?}"
    );
    assert!(
        renamed
            .aliases
            .iter()
            .any(|alias| alias == "books::Book::name"),
        "the prior canonical path must be recorded as the new alias: {renamed:#?}"
    );
}

/// The same redundant-alias drop covers an enum member round-trip
/// (`a` -> `grp::a` -> `a`): the new canonical enum-member path equals a recorded
/// alias of the same identity, so the alias is dropped rather than self-colliding.
#[test]
fn evolve_rename_round_trip_onto_a_former_enum_member_path_succeeds() {
    let root = temp_project("evolve-rename-round-trip-enum-member", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum State\n\
             \x20   a\n\
             \x20   b\n\
             resource Book\n\
             \x20   required value: State\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename State::grp::a -> State::a\n",
        );
        // The accepted catalog reflects the first leg (`a` -> `grp::a`): the member is
        // canonical at `grp::a` and records `a` as an alias under the preserved id.
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::value",
                "member-value",
                &[],
            ),
            entry(CatalogEntryKind::Enum, "books::State", "enum-state", &[]),
            entry(
                CatalogEntryKind::EnumMember,
                "books::State::grp::a",
                "enum-member-a",
                &["books::State::a"],
            ),
            entry(
                CatalogEntryKind::EnumMember,
                "books::State::b",
                "enum-member-b",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "an enum-member round-trip rename onto a former path must satisfy the binding: {:#?}",
        report.diagnostics
    );
    let proposal = program.catalog.proposal.expect("proposal");
    CatalogMetadata::from_json(&proposal.to_json_pretty().expect("catalog renders"))
        .expect("proposal validates");
    let renamed = proposal
        .entries
        .iter()
        .find(|entry| entry.kind == CatalogEntryKind::EnumMember && entry.path == "books::State::a")
        .expect("renamed enum member entry");
    assert_eq!(renamed.stable_id, derived_id("enum-member-a"));
    assert!(
        !renamed
            .aliases
            .iter()
            .any(|alias| alias == "books::State::a"),
        "the redundant alias equal to the new canonical path must be dropped: {renamed:#?}"
    );
}

/// A rename onto a path a DIFFERENT same-kind entry still records as a stale alias
/// (Book had `a` and `b`; `a` was renamed to `c`, recording `a` as `c`'s alias;
/// now `b` is renamed to `a`). The new canonical path `a` is live again under `b`'s
/// identity, so the stale alias on the sibling `c` is dropped rather than colliding
/// the path against a live entry. This is the sibling case the redundant-self-alias
/// drop alone did not cover.
#[test]
fn evolve_rename_onto_a_siblings_stale_alias_path_succeeds() {
    let root = temp_project("evolve-rename-onto-sibling-stale-alias", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   c: string\n\
             \x20   a: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.b -> Book.a\n",
        );
        // The accepted catalog reflects the first leg (`a` -> `c`): the member is
        // canonical at `c` and records `a` as an alias, while the sibling stays at `b`.
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::c",
                "member-c",
                &["books::Book::a"],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::b",
                "member-b",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "a rename onto a sibling's stale-alias path must satisfy the binding: {:#?}",
        report.diagnostics
    );
    let proposal = program.catalog.proposal.expect("proposal");
    CatalogMetadata::from_json(&proposal.to_json_pretty().expect("catalog renders"))
        .expect("proposal validates");
    let renamed = proposal
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember && entry.path == "books::Book::a"
        })
        .expect("renamed member entry now canonical at `a`");
    assert_eq!(renamed.stable_id, derived_id("member-b"));
    assert_eq!(renamed.lifecycle, CatalogLifecycle::Active);
    let sibling = proposal
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember
                && entry.stable_id == derived_id("member-c")
        })
        .expect("the renamed sibling entry");
    assert!(
        !sibling
            .aliases
            .iter()
            .any(|alias| alias == "books::Book::a"),
        "the sibling's stale alias equal to the new canonical path must be dropped: {sibling:#?}"
    );
}

/// The redundant-alias drop must not paper over a genuine duplicate-path collision:
/// two live entries that both claim the same canonical path still fail validation.
#[test]
fn evolve_rename_still_fails_closed_on_a_genuine_duplicate_path() {
    let collided = catalog(vec![
        entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
        entry(
            CatalogEntryKind::ResourceMember,
            "books::Book::a",
            "member-a",
            &[],
        ),
        entry(
            CatalogEntryKind::ResourceMember,
            "books::Book::a",
            "member-b",
            &[],
        ),
    ]);
    let rendered = collided.to_json_pretty().expect("catalog renders");
    assert!(
        CatalogMetadata::from_json(&rendered).is_err(),
        "two live entries sharing a canonical path must fail validation"
    );
}

#[test]
fn evolve_retire_fails_closed_when_accepted_path_matches_active_and_reserved_entries() {
    let root = temp_project("evolve-retire-accepted-path-ambiguous", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   retire Book.subtitle\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "member-title",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::subtitle",
                "member-subtitle",
                &[],
            ),
            reserved_entry(
                CatalogEntryKind::EnumMember,
                "books::Book::subtitle",
                "enum-member-subtitle",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "path-only retire must fail closed when accepted entries share the path: {:#?}",
        report.diagnostics
    );
    assert_catalog_path_ambiguity(
        &report,
        CatalogIntentKind::RetireTarget,
        "books::Book::subtitle",
        vec![
            accepted_candidate(
                CatalogEntryKind::ResourceMember,
                CatalogLifecycle::Active,
                "member-subtitle",
            ),
            accepted_candidate(
                CatalogEntryKind::EnumMember,
                CatalogLifecycle::Reserved,
                "enum-member-subtitle",
            ),
        ],
        vec![],
    );
    assert_member_dropped(&program, "member-subtitle");
}

#[test]
fn evolve_retire_fails_closed_when_accepted_path_matches_an_active_alias() {
    let root = temp_project("evolve-retire-active-alias-ambiguous", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   retire Book.subtitle\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "member-title",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::subtitle",
                "member-subtitle",
                &[],
            ),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Book::archived",
                "enum-member-archived",
                &["books::Book::subtitle"],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert_catalog_path_ambiguity(
        &report,
        CatalogIntentKind::RetireTarget,
        "books::Book::subtitle",
        vec![
            accepted_candidate(
                CatalogEntryKind::ResourceMember,
                CatalogLifecycle::Active,
                "member-subtitle",
            ),
            accepted_candidate(
                CatalogEntryKind::EnumMember,
                CatalogLifecycle::Active,
                "enum-member-archived",
            ),
        ],
        vec![],
    );
    assert_member_dropped(&program, "member-subtitle");
}

#[test]
fn evolve_rename_fails_closed_when_source_path_matches_active_and_reserved_entries() {
    let root = temp_project("evolve-rename-source-accepted-path-ambiguous", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   blurb: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.subtitle -> Book.blurb\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::subtitle",
                "member-subtitle",
                &[],
            ),
            reserved_entry(
                CatalogEntryKind::EnumMember,
                "books::Book::subtitle",
                "enum-member-subtitle",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "rename source must fail closed when accepted entries share the source path: {:#?}",
        report.diagnostics
    );
    assert_catalog_path_ambiguity(
        &report,
        CatalogIntentKind::RenameSource,
        "books::Book::subtitle",
        vec![
            accepted_candidate(
                CatalogEntryKind::ResourceMember,
                CatalogLifecycle::Active,
                "member-subtitle",
            ),
            accepted_candidate(
                CatalogEntryKind::EnumMember,
                CatalogLifecycle::Reserved,
                "enum-member-subtitle",
            ),
        ],
        vec![],
    );
    assert_no_catalog_entry_at(&program, "member-subtitle", "books::Book::blurb");
}

#[test]
fn evolve_rename_fails_closed_when_target_path_matches_an_active_alias() {
    let root = temp_project("evolve-rename-target-active-alias-ambiguous", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   blurb: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.subtitle -> Book.blurb\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::subtitle",
                "member-subtitle",
                &[],
            ),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Book::archived",
                "enum-member-archived",
                &["books::Book::blurb"],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert_catalog_path_ambiguity(
        &report,
        CatalogIntentKind::RenameTarget,
        "books::Book::blurb",
        vec![accepted_candidate(
            CatalogEntryKind::EnumMember,
            CatalogLifecycle::Active,
            "enum-member-archived",
        )],
        vec![CatalogEntryKind::ResourceMember],
    );
    assert_member_dropped(&program, "member-subtitle");
    assert_no_catalog_entry_at(&program, "member-subtitle", "books::Book::blurb");
}

#[test]
fn evolve_rename_fails_closed_when_target_path_matches_an_accepted_entry_of_another_kind() {
    let root = temp_project("evolve-rename-target-accepted-path-ambiguous", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   blurb: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.subtitle -> Book.blurb\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::subtitle",
                "member-subtitle",
                &[],
            ),
            entry(
                CatalogEntryKind::EnumMember,
                "books::Book::blurb",
                "enum-member-blurb",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "rename target must fail closed when accepted entries share the target path: {:#?}",
        report.diagnostics
    );
    assert_catalog_path_ambiguity(
        &report,
        CatalogIntentKind::RenameTarget,
        "books::Book::blurb",
        vec![accepted_candidate(
            CatalogEntryKind::EnumMember,
            CatalogLifecycle::Active,
            "enum-member-blurb",
        )],
        vec![CatalogEntryKind::ResourceMember],
    );
    assert_no_catalog_entry_at(&program, "member-subtitle", "books::Book::blurb");
}

#[test]
fn consumed_rename_fails_closed_when_source_alias_matches_another_active_accepted_entry() {
    let root = temp_project(
        "evolve-consumed-rename-source-active-alias-ambiguous",
        |root| {
            write(
                root,
                "src/books.mw",
                "module books\n\
             resource Book\n\
             \x20   blurb: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.subtitle -> Book.blurb\n",
            );
            let metadata = catalog(vec![
                entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
                entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
                entry(
                    CatalogEntryKind::ResourceMember,
                    "books::Book::blurb",
                    "member-subtitle",
                    &["books::Book::subtitle"],
                ),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Book::archived",
                    "enum-member-archived",
                    &["books::Book::subtitle"],
                ),
            ]);
            write_catalog(root, &metadata);
        },
    );

    let (report, program) = check_with_accepted(&root);

    assert_catalog_path_ambiguity(
        &report,
        CatalogIntentKind::RenameSource,
        "books::Book::subtitle",
        vec![
            accepted_candidate(
                CatalogEntryKind::ResourceMember,
                CatalogLifecycle::Active,
                "member-subtitle",
            ),
            accepted_candidate(
                CatalogEntryKind::EnumMember,
                CatalogLifecycle::Active,
                "enum-member-archived",
            ),
        ],
        vec![],
    );
    assert_no_catalog_entry_at(&program, "member-subtitle", "books::Book::subtitle");
}

#[test]
fn source_member_rename_without_evolve_intent_still_fails_closed() {
    let root = temp_project("evolve-rename-member-no-intent", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "member-title",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, _program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn evolve_retire_marks_the_proposal_entry_reserved() {
    let root = temp_project("evolve-retire", |root| {
        // The source has dropped `subtitle`; the accepted catalog still records it.
        // `retire` declares the destructive intent while reserving the old spelling.
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   retire Book.subtitle\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "member-title",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::subtitle",
                "member-subtitle",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let proposal = program.catalog.proposal.expect("proposal");
    let retired = proposal
        .entries
        .iter()
        .find(|entry| {
            entry.kind == CatalogEntryKind::ResourceMember
                && entry.stable_id == derived_id("member-subtitle")
        })
        .expect("retired member entry");
    assert_eq!(retired.lifecycle, CatalogLifecycle::Reserved);
}

#[test]
fn evolve_retire_of_a_still_declared_resource_member_fails_closed() {
    // The source still declares `Book.title` while `retire` names it. A retire is a
    // destructive drop of data the running program still reads and writes, so it
    // must be rejected until the source declaration is actually gone; the proposal
    // entry must stay Active rather than be silently reserved.
    let root = temp_project("evolve-retire-member-still-declared", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   retire Book.title\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "member-title",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "retiring a still-declared resource member must fail closed: {:#?}",
        report.diagnostics
    );
    assert_entry_stays_active(&program, "member-title");
}

#[test]
fn evolve_retire_of_a_still_declared_saved_root_fails_closed() {
    // The source still declares the saved root `^books` while `retire` names it.
    // Retiring it would drop a store the running program still reads and writes, so
    // it must be rejected and the store entry must stay Active.
    let root = temp_project("evolve-retire-root-still-declared", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   retire ^books\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "member-title",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "retiring a still-declared saved root must fail closed: {:#?}",
        report.diagnostics
    );
    assert_entry_stays_active(&program, "store-books");
}

#[test]
fn evolve_retire_of_a_still_declared_store_index_fails_closed() {
    // The source still declares the store index `^books.byTitle` while `retire`
    // names it. Retiring it would drop a derived index the running program still
    // maintains, so it must be rejected and the index entry must stay Active.
    let root = temp_project("evolve-retire-index-still-declared", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             \x20   index byTitle(title) unique\n\
             evolve\n\
             \x20   retire ^books.byTitle\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            // The retire fails closed on still-declared presence before any shape
            // comparison, so the accepted index shape is irrelevant here.
            store_index_entry("books::^books::byTitle", "index-by-title", ""),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "member-title",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "retiring a still-declared store index must fail closed: {:#?}",
        report.diagnostics
    );
    assert_entry_stays_active(&program, "index-by-title");
}

#[test]
fn evolve_target_that_resolves_to_nothing_is_diagnosed() {
    let root = temp_project("evolve-unknown-target", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   retire Book.missing\n",
        );
    });

    let (report, _program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_EVOLVE_TARGET),
        "{:#?}",
        report.diagnostics
    );
}

/// An unresolved rename, retire, or transform target must anchor its diagnostic at
/// the target token, not at the indented step keyword: a good error says where it
/// happened. Each step keyword is indented four spaces; the target token follows the
/// keyword and its space, so retire/rename land at column 12 and transform (a longer
/// keyword) at column 15 — token-precise in every case, never the column-1 indent.
#[test]
fn unresolved_evolve_target_anchors_at_the_target_token() {
    let root = temp_project("evolve-target-span", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   retire Book.ghost\n\
             \x20   rename ^nonexistent -> ^alsoGone\n\
             \x20   transform Book.ghostfield\n\
             \x20       return 0\n",
        );
    });

    let (report, _program) = check_with_accepted(&root);

    let mut columns: Vec<u32> = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == marrow_check::CHECK_EVOLVE_TARGET)
        .map(|diagnostic| diagnostic.span.column)
        .collect();
    columns.sort_unstable();

    // `retire Book.ghost` and `rename ^nonexistent` start at column 12;
    // `transform Book.ghostfield` (the longer keyword) starts at column 15.
    assert_eq!(
        columns,
        vec![12, 12, 15],
        "every unresolved evolve target must anchor at its target token: {:#?}",
        report.diagnostics
    );
}

/// Renaming a whole enum carries its members forward on their accepted identity: a stored enum
/// value is keyed on the member's stable id, so minting fresh member ids would orphan every saved
/// cell even though a rename is identity-preserving. The proposal must keep each member's accepted
/// id under the new enum path and record the old spelling as an alias.
#[test]
fn evolve_rename_of_a_whole_enum_carries_its_members_forward() {
    let root = temp_project("evolve-rename-enum-cascade", |root| {
        write(
            root,
            "src/pets.mw",
            "module pets\n\
             enum Animal\n\
             \x20   bengal\n\
             \x20   housecat\n\
             resource Pet\n\
             \x20   required kind: Animal\n\
             store ^pets(id: int): Pet\n\
             evolve\n\
             \x20   rename Cat -> Animal\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Enum, "pets::Cat", "enum-cat", &[]),
            entry(
                CatalogEntryKind::EnumMember,
                "pets::Cat::bengal",
                "member-bengal",
                &[],
            ),
            entry(
                CatalogEntryKind::EnumMember,
                "pets::Cat::housecat",
                "member-housecat",
                &[],
            ),
            entry(CatalogEntryKind::Resource, "pets::Pet", "res-pet", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "pets::Pet::kind",
                "member-kind",
                &[],
            ),
            entry(CatalogEntryKind::Store, "pets::^pets", "store-pets", &[]),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "a whole-enum rename must satisfy the catalog binding: {:#?}",
        report.diagnostics
    );
    let proposal = program.catalog.proposal.expect("proposal");
    for (member, id) in [("bengal", "member-bengal"), ("housecat", "member-housecat")] {
        let new_path = format!("pets::Animal::{member}");
        let old_path = format!("pets::Cat::{member}");
        let carried = proposal
            .entries
            .iter()
            .find(|entry| entry.kind == CatalogEntryKind::EnumMember && entry.path == new_path)
            .unwrap_or_else(|| panic!("member `{new_path}` in proposal: {:#?}", proposal.entries));
        assert_eq!(
            carried.stable_id,
            derived_id(id),
            "member `{member}` must keep its accepted id, not mint a fresh one",
        );
        assert!(
            carried.aliases.iter().any(|alias| alias == &old_path),
            "member `{member}` must record its old spelling as an alias: {carried:#?}",
        );
        assert!(
            !proposal.entries.iter().any(|entry| entry.path == old_path),
            "the old member path must not linger as a separate entry: {:#?}",
            proposal.entries
        );
    }
}

/// A rename whose source names an accepted entry but whose DESTINATION the current source does not
/// declare fails on the destination, not the source: `Book.title` is a real accepted member, so the
/// carry-forward message ("does not name an accepted catalog entry to carry forward") misattributes
/// the failure to the resolved source. The unresolved destination leg gets its own message naming
/// the undeclared to-path, anchored at the destination token, not the accepted from-path token.
#[test]
fn unresolved_rename_destination_is_diagnosed_at_the_destination_token() {
    let root = temp_project("evolve-rename-unresolved-destination", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   final: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.title -> Book.mid\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::title",
                "member-title",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, _program) = check_with_accepted(&root);

    let target: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == marrow_check::CHECK_EVOLVE_TARGET)
        .collect();
    assert_eq!(
        target.len(),
        1,
        "the unresolved destination is one diagnostic: {:#?}",
        report.diagnostics
    );
    let diagnostic = target[0];
    assert!(
        diagnostic.message.contains("books::Book::mid")
            && diagnostic.message.contains("current source"),
        "the destination leg names the undeclared to-path, not a carry-forward failure: {}",
        diagnostic.message
    );
    assert!(
        !diagnostic.message.contains("carry forward"),
        "the destination leg must not reuse the source carry-forward message: {}",
        diagnostic.message
    );
    // `\x20   rename Book.title -> ` is 4 + "rename "(7) + "Book.title"(10) + " -> "(4) columns, so
    // the `Book.mid` destination token starts at column 26 — never the from-token column 12.
    assert_eq!(
        diagnostic.span.column, 26,
        "the diagnostic anchors at the destination token: {diagnostic:#?}"
    );
}

/// The check.evolve_target enumeration names every catalog-addressable kind the grammar lists,
/// including a bare `resource`: renaming or retiring a resource carries its whole member subtree,
/// so a resource is an addressable target and the message that lists the kinds must say so.
#[test]
fn evolve_target_kind_enumeration_names_a_resource() {
    let root = temp_project("evolve-target-kind-enumeration", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   retire ^books(1)\n",
        );
    });

    let (report, _program) = check_with_accepted(&root);

    let diagnostic = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == marrow_check::CHECK_EVOLVE_TARGET)
        .expect("an unaddressable evolve target is diagnosed");
    assert!(
        diagnostic.message.contains("a resource, a resource member"),
        "the addressable-kind enumeration must list a bare resource: {}",
        diagnostic.message
    );
}

#[test]
fn evolve_binding_that_would_collide_identity_is_reported_at_check() {
    // A rename carries the accepted `member-a` onto `Book.c` while the source also
    // freshly declares `Book.c`. The two would share the path `books::Book::c` in
    // the proposal, an identity collision that must surface as a check diagnostic
    // rather than be deferred to apply. The proposal a check produces must always
    // validate.
    let root = temp_project("evolve-binding-identity-collision", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   c: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.a -> Book.c\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::a",
                "member-a",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::c",
                "member-c",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "an identity collision must be reported at check: {:#?}",
        report.diagnostics
    );
    if let Some(proposal) = program.catalog.proposal {
        CatalogMetadata::from_json(&proposal.to_json_pretty().expect("catalog renders"))
            .expect("a proposal a check produces must validate");
    }
}

#[test]
fn evolve_rename_whose_source_is_still_declared_fails_closed() {
    // A rename means the old spelling is gone from source. Here `Book.a` is still
    // a live source member while a rename also carries it to `Book.c`, so the
    // accepted entry `member-a` must not be aliased onto two live source members.
    let root = temp_project("evolve-rename-source-still-declared", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   a: string\n\
             \x20   c: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.a -> Book.c\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::a",
                "member-a",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, _program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "a rename whose source is still declared must fail closed: {:#?}",
        report.diagnostics
    );
}

#[test]
fn evolve_rename_onto_a_live_accepted_target_fails_closed() {
    // Both `Book.a` and `Book.b` are live accepted entries the source still
    // declares; renaming `a` onto `b` would silently no-op (b already binds), so
    // a declared intent that cannot move identity must be diagnosed.
    let root = temp_project("evolve-rename-onto-live-target", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   a: string\n\
             \x20   b: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.a -> Book.b\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::a",
                "member-a",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::b",
                "member-b",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, _program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "a rename onto a live accepted target must fail closed: {:#?}",
        report.diagnostics
    );
}

#[test]
fn two_renames_onto_the_same_target_conflict() {
    // The rename graph must be injective: two renames targeting `Book.c` cannot
    // both carry their identity forward, so the collision is diagnosed instead of
    // collapsing last-writer-wins and orphaning one accepted entry.
    let root = temp_project("evolve-rename-target-conflict", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   c: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Book.a -> Book.c\n\
             \x20   rename Book.b -> Book.c\n",
        );
        let metadata = catalog(vec![
            entry(CatalogEntryKind::Resource, "books::Book", "res-book", &[]),
            entry(CatalogEntryKind::Store, "books::^books", "store-books", &[]),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::a",
                "member-a",
                &[],
            ),
            entry(
                CatalogEntryKind::ResourceMember,
                "books::Book::b",
                "member-b",
                &[],
            ),
        ]);
        write_catalog(root, &metadata);
    });

    let (report, _program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_CATALOG_INTENT),
        "two renames onto one target must conflict: {:#?}",
        report.diagnostics
    );
}

#[test]
fn evolve_transform_body_reports_undefined_identifiers() {
    // A transform body is held to the same name-resolution rules a function body
    // is: an undefined identifier is caught at check time, not left as unchecked
    // free text.
    let root = temp_project("evolve-transform-undefined-name", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.title\n\
             \x20   \x20   const x: string = totallyUndefinedVar\n",
        );
    });

    let (report, _program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_UNRESOLVED_NAME),
        "undefined identifier in a transform body must be reported: {:#?}",
        report.diagnostics
    );
}

#[test]
fn evolve_transform_body_reports_unknown_calls() {
    // A transform body resolves call targets the same way a function body does: an
    // unknown call is caught at check time.
    let root = temp_project("evolve-transform-undefined-call", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.title\n\
             \x20   \x20   const y: string = nonexistentFn()\n",
        );
    });

    let (report, _program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_UNRESOLVED_CALL),
        "unknown call in a transform body must be reported: {:#?}",
        report.diagnostics
    );
}

#[test]
fn evolve_transform_body_rejects_return_absent() {
    let root = temp_project("evolve-transform-return-absent", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.title\n\
             \x20   \x20   return absent\n",
        );
    });

    let (report, _program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_UNRESOLVED_OPTIONAL),
        "`return absent` into a non-optional transform target must raise the one rule: {:#?}",
        report.diagnostics
    );
}

#[test]
fn evolve_transform_body_rejects_plain_return() {
    let root = temp_project("evolve-transform-plain-return", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.title\n\
             \x20   \x20   return\n",
        );
    });

    let (report, _program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_RETURN_VALUE),
        "plain `return` in a transform body must be rejected: {:#?}",
        report.diagnostics
    );
}

#[test]
fn evolve_transform_match_arm_rejects_return_absent() {
    let root = temp_project("evolve-transform-match-return-absent", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   draft\n\
             \x20   final\n\
             resource Book\n\
             \x20   status: Status\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.title\n\
             \x20   \x20   match old.status\n\
             \x20   \x20       Status::draft\n\
             \x20   \x20           return absent\n\
             \x20   \x20       Status::final\n\
             \x20   \x20           return \"final\"\n",
        );
    });

    let (report, _program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_UNRESOLVED_OPTIONAL),
        "`return absent` into a non-optional transform target must raise the one rule: {:#?}",
        report.diagnostics
    );
}

#[test]
fn evolve_default_value_type_mismatch_is_diagnosed() {
    let root = temp_project("evolve-default-type", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required count: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   default Book.count = \"not a number\"\n",
        );
    });

    let (report, _program) = check_with_accepted(&root);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_EVOLVE_TYPE),
        "{:#?}",
        report.diagnostics
    );
}
