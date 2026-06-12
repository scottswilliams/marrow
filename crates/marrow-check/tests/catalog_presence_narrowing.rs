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
fn if_exists_narrows_reads_inside_the_then_block() {
    let root = temp_project("presence-if-exists", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
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
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
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
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
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
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn guarded(id: int): string\n\
             \x20   var k: int = id\n\
             \x20   if exists(^books(k).subtitle)\n\
             \x20       k = 2\n\
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
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn guarded(id: int): string\n\
             \x20   var holder = Holder(id: id)\n\
             \x20   if exists(^books(holder.id).subtitle)\n\
             \x20       holder.id = 2\n\
             \x20       return ^books(holder.id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_saved_field_is_deleted() {
    assert_bare_present_read(
        "presence-if-exists-delete-field",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
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
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
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
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
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
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
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
             resource Item\n\
             \x20   note: string\n\
             store ^items(id: int): Item\n\
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
             resource Book\n\
             \x20   required isbn: string\n\
             store ^books(id: int): Book\n\
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
             resource Book\n\
             \x20   required title: string\n\
             \x20   tags(pos: int): string\n\
             store ^books(id: int): Book\n\
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
             resource Book\n\
             \x20   tags(pos: int): string\n\
             store ^books(id: int): Book\n\
             fn f()\n\
             \x20   for pos in ^books(1).tags\n\
             \x20   \x20   print(^books(1).tags(pos))\n",
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
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
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
             resource Book\n\
             \x20   scores(pos: int): int\n\
             store ^books(id: int): Book\n\
             fn f()\n\
             \x20   for score in values(^books(1).scores)\n\
             \x20   \x20   print(^books(1).scores(score))\n",
    );
}

#[test]
fn single_binding_entries_loop_does_not_narrow_entry_as_a_key() {
    assert_bare_present_read(
        "presence-single-entry-loop-not-key",
        "module books\n\
             resource Book\n\
             \x20   scores(pos: int): int\n\
             store ^books(id: int): Book\n\
             fn f()\n\
             \x20   for entry in entries(^books(1).scores)\n\
             \x20   \x20   print(^books(1).scores(entry))\n",
    );
}

#[test]
fn two_binding_keys_loop_does_not_narrow_ordinal_as_a_key() {
    assert_bare_present_read(
        "presence-two-binding-keys-loop-not-key",
        "module books\n\
             resource Book\n\
             \x20   scores(pos: int): int\n\
             store ^books(id: int): Book\n\
             fn f()\n\
             \x20   for ordinal, pos in keys(^books(1).scores)\n\
             \x20   \x20   print(^books(1).scores(ordinal))\n",
    );
}

#[test]
fn two_binding_reversed_keys_loop_does_not_narrow_ordinal_as_a_key() {
    assert_bare_present_read(
        "presence-two-binding-reversed-keys-loop-not-key",
        "module books\n\
             resource Book\n\
             \x20   scores(pos: int): int\n\
             store ^books(id: int): Book\n\
             fn f()\n\
             \x20   for ordinal, pos in reversed(keys(^books(1).scores))\n\
             \x20   \x20   print(^books(1).scores(ordinal))\n",
    );
}

#[test]
fn two_binding_saved_path_loop_narrows_the_key_binding() {
    let root = temp_project("presence-two-binding-saved-path-loop-key", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   scores(pos: int): int\n\
             store ^books(id: int): Book\n\
             fn f()\n\
             \x20   for pos, score in ^books(1).scores\n\
             \x20   \x20   print(^books(1).scores(pos))\n",
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
             resource Book\n\
             \x20   scores(pos: int): int\n\
             store ^books(id: int): Book\n\
             fn f()\n\
             \x20   for x, x in entries(^books(1).scores)\n\
             \x20   \x20   print(^books(1).scores(x))\n",
    );
}

#[test]
fn if_exists_narrowing_expires_when_same_condition_calls_saved_writer() {
    assert_bare_present_read(
        "presence-if-exists-condition-call-writes-saved",
        "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn dropSubtitle(id: int): bool\n\
             \x20   delete ^books(id).subtitle\n\
             \x20   return true\n\
             fn stale(id: int): string\n\
             \x20   if exists(^books(id).subtitle) and dropSubtitle(id)\n\
             \x20   \x20   return ^books(id).subtitle\n\
             \x20   return \"untitled\"\n",
    );
}

/// Check `src` as the single module `books`, returning the presence proofs it records.
fn presence_proofs(name: &str, src: &str) -> Vec<marrow_check::PresenceProofFact> {
    let root = temp_project(name, |root| write(root, "src/books.mw", src));
    let (report, program) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    program.facts.presence_proofs().to_vec()
}

#[test]
fn a_bare_maybe_present_read_pends_on_attached_data() {
    let root = temp_project("presence-bare-pending", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             fn bare(id: int): string\n\
             \x20   return ^books(id).subtitle\n",
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
    let proof = program
        .facts
        .presence_proofs()
        .iter()
        .find(|proof| proof.source == PresenceProofSource::AttachedData)
        .expect("attached-data proof");
    assert_eq!(proof.status, PresenceProofStatus::PendingAttachedData);
}

#[test]
fn a_bare_required_field_read_through_parameter_identity_requires_resolution() {
    assert_bare_present_read(
        "presence-required-param-id",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         fn requiredTitle(id: Id(^books)): string\n\
         \x20   return ^books(id).title\n",
    );
}

#[test]
fn early_return_if_not_exists_narrows_the_remainder() {
    let proofs = presence_proofs(
        "presence-early-return-narrowing",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn subtitleOrMissing(id: Id(^books)): string\n\
         \x20   if not exists(^books(id).subtitle)\n\
         \x20       return \"missing\"\n\
         \x20   return ^books(id).subtitle\n",
    );

    let proof = proofs
        .iter()
        .find(|proof| proof.source == PresenceProofSource::Narrowing)
        .expect("early-return narrowing proof");
    assert_eq!(proof.status, PresenceProofStatus::Discharged);
}

#[test]
fn if_not_exists_with_a_calling_body_does_not_narrow_the_remainder() {
    assert_bare_present_read(
        "presence-early-return-call-falls-through",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn note()\n\
         \x20   const value: int = 1\n\
         fn subtitleOrMissing(id: Id(^books)): string\n\
         \x20   if not exists(^books(id).subtitle)\n\
         \x20       note()\n\
         \x20   return ^books(id).subtitle\n",
    );
}

#[test]
fn if_not_exists_with_a_looping_body_does_not_narrow_the_remainder() {
    assert_bare_present_read(
        "presence-early-return-loop-falls-through",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn subtitleOrMissing(id: Id(^books)): string\n\
         \x20   if not exists(^books(id).subtitle)\n\
         \x20       while false\n\
         \x20           return \"missing\"\n\
         \x20   return ^books(id).subtitle\n",
    );
}

#[test]
fn if_const_binding_guard_discharges_and_binds_with_one_point_read() {
    let proofs = presence_proofs(
        "presence-if-const-binding-guard",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn guarded(id: Id(^books)): string\n\
         \x20   if const subtitle = ^books(id).subtitle\n\
         \x20       return subtitle\n\
         \x20   return \"missing\"\n",
    );

    assert_eq!(proofs.len(), 1, "{proofs:#?}");
    assert_eq!(proofs[0].source, PresenceProofSource::Narrowing);
    assert_eq!(proofs[0].status, PresenceProofStatus::Discharged);
    assert_eq!(proofs[0].read, PresenceProofRead::Direct);
}

#[test]
fn a_coalesce_fallback_discharges_via_narrowing() {
    let proofs = presence_proofs(
        "presence-coalesce-narrowing",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn fallback(id: int): string\n\
         \x20   return ^books(id).subtitle ?? \"untitled\"\n",
    );

    let proof = proofs
        .iter()
        .find(|proof| proof.source == PresenceProofSource::Narrowing)
        .expect("narrowing proof");
    assert_eq!(proof.status, PresenceProofStatus::Discharged);
}

#[test]
fn an_exists_guard_discharges_via_narrowing() {
    let proofs = presence_proofs(
        "presence-exists-narrowing",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn found(id: int): bool\n\
         \x20   return exists(^books(id).subtitle)\n",
    );

    let proof = proofs
        .iter()
        .find(|proof| proof.source == PresenceProofSource::Narrowing)
        .expect("narrowing proof");
    assert_eq!(proof.status, PresenceProofStatus::Discharged);
}

#[test]
fn an_optional_chain_fallback_discharges_via_narrowing() {
    let proofs = presence_proofs(
        "presence-optional-narrowing",
        "module books\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         fn optional(id: int): string\n\
         \x20   return ^books(id)?.subtitle ?? \"untitled\"\n",
    );

    let proof = proofs
        .iter()
        .find(|proof| proof.source == PresenceProofSource::Narrowing)
        .expect("narrowing proof");
    assert_eq!(proof.status, PresenceProofStatus::Discharged);
}

/// A new `PresenceProofSource` variant must be wired through the per-strategy tests
/// above; this match fails to compile until it is, so the suite cannot silently stop
/// covering a way presence is proven.
#[test]
fn presence_proof_sources_are_exhaustively_covered() {
    match PresenceProofSource::AttachedData {
        PresenceProofSource::AttachedData | PresenceProofSource::Narrowing => {}
    }
}
