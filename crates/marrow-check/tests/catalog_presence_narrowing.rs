mod support;

use marrow_check::{
    CHECK_BARE_MAYBE_PRESENT_READ, PresenceProofPlace, PresenceProofRead, PresenceProofSource,
    PresenceProofStatus, check_project,
};

use support::{config, temp_project, write};

/// Check a single `src/books.mw` module `src` and assert it raises the bare
/// maybe-present-read diagnostic: the load-bearing input is the mutation in `src` that
/// expires `if exists` narrowing, so a later read is no longer proven present.
fn assert_bare_present_read(name: &str, src: &str) {
    let root = temp_project(name, |root| write(root, "src/books.mw", src));
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn coalesce_rejects_non_saved_function_calls_outside_the_presence_ledger() {
    let root = temp_project("presence-coalesce-call", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             fn value(): string\n\
             \x20   return \"title\"\n\
             fn fallback(): string\n\
             \x20   return value() ?? \"untitled\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_OPERATOR_TYPE),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn if_exists_narrows_reads_inside_the_then_block() {
    let root = temp_project("presence-if-exists", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn guarded(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        program
            .facts
            .presence_proofs()
            .iter()
            .any(|proof| proof.source == PresenceProofSource::Narrowing),
        "{:#?}",
        program.facts.presence_proofs()
    );
}

#[test]
fn if_exists_narrowing_is_key_sensitive() {
    assert_bare_present_read(
        "presence-if-exists-keyed",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn guarded(a: int, b: int): string\n\
             \x20   if exists(^books(a).subtitle)\n\
             \x20       return ^books(b).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_is_binding_sensitive() {
    assert_bare_present_read(
        "presence-if-exists-shadowed-key",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn guarded(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       const id: int = 2\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_a_key_binding_is_assigned() {
    assert_bare_present_read(
        "presence-if-exists-mutated-key",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn guarded(id: int): string\n\
             \x20   var k: int = id\n\
             \x20   if exists(^books(k).subtitle)\n\
             \x20       k = 2\n\
             \x20       return ^books(k).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_a_key_binding_is_passed_inout() {
    assert_bare_present_read(
        "presence-if-exists-inout-key",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn setTo(inout value: int)\n\
             \x20   value = 2\n\
             fn guarded(id: int): string\n\
             \x20   var k: int = id\n\
             \x20   if exists(^books(k).subtitle)\n\
             \x20       setTo(inout k)\n\
             \x20       return ^books(k).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_a_key_field_is_assigned() {
    assert_bare_present_read(
        "presence-if-exists-mutated-key-field",
        "module books\n\
             resource Holder\n\
             \x20   required id: int\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn guarded(id: int): string\n\
             \x20   var holder = Holder(id: id)\n\
             \x20   if exists(^books(holder.id).subtitle)\n\
             \x20       holder.id = 2\n\
             \x20       return ^books(holder.id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_a_key_field_is_passed_inout() {
    assert_bare_present_read(
        "presence-if-exists-inout-key-field",
        "module books\n\
             resource Holder\n\
             \x20   required id: int\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn setTo(inout value: int)\n\
             \x20   value = 2\n\
             fn guarded(id: int): string\n\
             \x20   var holder = Holder(id: id)\n\
             \x20   if exists(^books(holder.id).subtitle)\n\
             \x20       setTo(inout holder.id)\n\
             \x20       return ^books(holder.id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_nested_condition_mutates_key() {
    assert_bare_present_read(
        "presence-if-exists-nested-condition-mutates-key",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn setTo(inout value: int): bool\n\
             \x20   value = 2\n\
             \x20   return true\n\
             fn guarded(id: int): string\n\
             \x20   var k: int = id\n\
             \x20   if exists(^books(k).subtitle)\n\
             \x20       if setTo(inout k)\n\
             \x20           return ^books(k).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_ignores_condition_proofs_after_key_mutation() {
    assert_bare_present_read(
        "presence-if-exists-condition-mutates-key",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn setTo(inout value: int): bool\n\
             \x20   value = 2\n\
             \x20   return true\n\
             fn guarded(id: int): string\n\
             \x20   var k: int = id\n\
             \x20   if exists(^books(k).subtitle) and setTo(inout k)\n\
             \x20       return ^books(k).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_saved_field_is_deleted() {
    assert_bare_present_read(
        "presence-if-exists-delete-field",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       delete ^books(id).subtitle\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_saved_root_is_replaced() {
    assert_bare_present_read(
        "presence-if-exists-replace-root",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       ^books(id) = Book(title: \"new\")\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_called_function_writes_saved_data() {
    assert_bare_present_read(
        "presence-if-exists-call-writes-saved",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn dropSubtitle(id: int)\n\
             \x20   delete ^books(id).subtitle\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       dropSubtitle(id)\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_called_function_transitively_writes_saved_data() {
    assert_bare_present_read(
        "presence-if-exists-call-transitive-writes-saved",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn dropSubtitle(id: int)\n\
             \x20   delete ^books(id).subtitle\n\
             fn relay(id: int)\n\
             \x20   dropSubtitle(id)\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle)\n\
             \x20       relay(id)\n\
             \x20       return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_only_child_of_parent_is_deleted() {
    let root = temp_project("presence-if-exists-delete-only-child", |root| {
        write(
            root,
            "src/items.mw",
            "module items\n\
             resource Item at ^items(id: int)\n\
             \x20   note: string\n\
             fn stale(id: int): Item\n\
             \x20   if exists(^items(id))\n\
             \x20       delete ^items(id).note\n\
             \x20       return ^items(id)\n\
             \x20   return Item()\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn unique_index_coalesce_records_presence_proof() {
    let root = temp_project("presence-index-coalesce", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required isbn: string\n\
             \n\
             \x20   index byIsbn(isbn) unique\n\
             \n\
             fn lookup(isbn: string, fallback: Id(^books)): Id(^books)\n\
             \x20   return ^books.byIsbn(isbn) ?? fallback\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_OPERATOR_TYPE),
        "{:#?}",
        report.diagnostics
    );
    let proof = program
        .facts
        .presence_proofs()
        .iter()
        .find(|proof| proof.source == PresenceProofSource::Narrowing)
        .expect("narrowing proof");
    assert!(
        matches!(proof.place, PresenceProofPlace::StoreIndex(_)),
        "{:#?}",
        program.facts.presence_proofs()
    );
    assert_eq!(proof.read, PresenceProofRead::Direct);
    assert_eq!(proof.keys.len(), 1);
}

#[test]
fn next_coalesce_records_read_site_resolution() {
    let root = temp_project("presence-next-coalesce", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   tags(pos: int): string\n\
             fn nextPos(id: int, pos: int): int\n\
             \x20   return next(^books(id).tags(pos)) ?? -1\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    let proof_sources: Vec<_> = program
        .facts
        .presence_proofs()
        .iter()
        .map(|proof| proof.source)
        .collect();
    assert!(
        proof_sources.contains(&PresenceProofSource::Narrowing),
        "{proof_sources:#?}"
    );
    let next_proof = program
        .facts
        .presence_proofs()
        .iter()
        .find(|proof| proof.read == PresenceProofRead::Next)
        .expect("next proof");
    assert!(matches!(next_proof.place, PresenceProofPlace::Saved(_)));
    assert_eq!(next_proof.keys.len(), 3);
    assert!(
        !proof_sources.contains(&PresenceProofSource::AttachedData),
        "{proof_sources:#?}"
    );
}

#[test]
fn for_loop_over_saved_layer_narrows_iterated_entry_reads() {
    let root = temp_project("presence-loop-narrowing", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   tags(pos: int): string\n\
             fn f()\n\
             \x20   for pos in ^books(1).tags\n\
             \x20   \x20   write(^books(1).tags(pos))\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert!(
        program
            .facts
            .presence_proofs()
            .iter()
            .any(|proof| proof.source == PresenceProofSource::Narrowing),
        "{:#?}",
        program.facts.presence_proofs()
    );
}

#[test]
fn unknown_cannot_reenter_a_saved_identity_keyspace() {
    let root = temp_project("identity-unknown-keyspace", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             fn save(raw: unknown)\n\
             \x20   ^books(raw).title = \"bad\"\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_KEY_TYPE),
        "unknown must not act as any for saved identity keys: {:#?}",
        report.diagnostics
    );
}

#[test]
fn values_loop_does_not_narrow_value_as_an_entry_key() {
    assert_bare_present_read(
        "presence-values-loop-not-key",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   scores(pos: int): int\n\
             fn f()\n\
             \x20   for score in values(^books(1).scores)\n\
             \x20   \x20   write(^books(1).scores(score))\n",
    );
}

#[test]
fn single_binding_entries_loop_does_not_narrow_entry_as_a_key() {
    assert_bare_present_read(
        "presence-single-entry-loop-not-key",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   scores(pos: int): int\n\
             fn f()\n\
             \x20   for entry in entries(^books(1).scores)\n\
             \x20   \x20   write(^books(1).scores(entry))\n",
    );
}

#[test]
fn two_binding_keys_loop_does_not_narrow_ordinal_as_a_key() {
    assert_bare_present_read(
        "presence-two-binding-keys-loop-not-key",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   scores(pos: int): int\n\
             fn f()\n\
             \x20   for ordinal, pos in keys(^books(1).scores)\n\
             \x20   \x20   write(^books(1).scores(ordinal))\n",
    );
}

#[test]
fn two_binding_reversed_keys_loop_does_not_narrow_ordinal_as_a_key() {
    assert_bare_present_read(
        "presence-two-binding-reversed-keys-loop-not-key",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   scores(pos: int): int\n\
             fn f()\n\
             \x20   for ordinal, pos in reversed(keys(^books(1).scores))\n\
             \x20   \x20   write(^books(1).scores(ordinal))\n",
    );
}

#[test]
fn two_binding_saved_path_loop_narrows_the_key_binding() {
    let root = temp_project("presence-two-binding-saved-path-loop-key", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   scores(pos: int): int\n\
             fn f()\n\
             \x20   for pos, score in ^books(1).scores\n\
             \x20   \x20   write(^books(1).scores(pos))\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

#[test]
fn duplicate_entries_loop_bindings_do_not_narrow_the_visible_value_as_a_key() {
    assert_bare_present_read(
        "presence-duplicate-entries-loop-bindings-not-key",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   scores(pos: int): int\n\
             fn f()\n\
             \x20   for x, x in entries(^books(1).scores)\n\
             \x20   \x20   write(^books(1).scores(x))\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_same_condition_calls_saved_writer() {
    assert_bare_present_read(
        "presence-if-exists-condition-call-writes-saved",
        "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   subtitle: string\n\
             fn dropSubtitle(id: int): bool\n\
             \x20   delete ^books(id).subtitle\n\
             \x20   return true\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle) and dropSubtitle(id)\n\
             \x20   \x20   return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn bare_maybe_present_read_errors_and_resolved_reads_record_allowed_proof_sources() {
    let root = temp_project("presence-ledger", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book at ^books(id: int)\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             fn requiredTitle(id: int): string\n\
             \x20   return ^books(id).title\n\
             fn bare(id: int): string\n\
             \x20   return ^books(id).subtitle\n\
             fn fallback(id: int): string\n\
             \x20   return ^books(id).subtitle ?? \"untitled\"\n\
             fn found(id: int): bool\n\
             \x20   return exists(^books(id).subtitle)\n\
             fn optional(id: int): string\n\
             \x20   return ^books(id)?.subtitle ?? \"untitled\"\n",
        );
    });

    let (report, program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_BARE_MAYBE_PRESENT_READ),
        "{:#?}",
        report.diagnostics
    );
    let proof_sources: Vec<_> = program
        .facts
        .presence_proofs()
        .iter()
        .map(|proof| proof.source)
        .collect();
    let mut observed_sources: Vec<_> = proof_sources.clone();
    observed_sources.sort_by_key(|source| format!("{source:?}"));
    observed_sources.dedup();
    // Compile-time tripwire: a new PresenceProofSource variant must also be
    // added to all_sources below, or this match stops being exhaustive.
    match PresenceProofSource::AttachedData {
        PresenceProofSource::AttachedData
        | PresenceProofSource::Declaration
        | PresenceProofSource::Narrowing => {}
    }
    let mut all_sources = vec![
        PresenceProofSource::AttachedData,
        PresenceProofSource::Declaration,
        PresenceProofSource::Narrowing,
    ];
    all_sources.sort_by_key(|source| format!("{source:?}"));
    assert_eq!(
        observed_sources, all_sources,
        "every presence-proof source variant must occur once in this fixture"
    );
    assert!(
        program
            .facts
            .presence_proofs()
            .iter()
            .any(|proof| proof.status == PresenceProofStatus::PendingAttachedData),
        "{:#?}",
        program.facts.presence_proofs()
    );
    assert!(
        program
            .facts
            .presence_proofs()
            .iter()
            .any(|proof| proof.status == PresenceProofStatus::Discharged),
        "{:#?}",
        program.facts.presence_proofs()
    );
    let mut proof_ids: Vec<_> = program
        .facts
        .presence_proofs()
        .iter()
        .map(|proof| proof.id)
        .collect();
    proof_ids.sort_by_key(|id| id.0);
    proof_ids.dedup();
    assert_eq!(
        proof_ids.len(),
        program.facts.presence_proofs().len(),
        "presence proof ids must be unique"
    );
}
