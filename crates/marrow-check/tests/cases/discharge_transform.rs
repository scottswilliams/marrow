use crate::support;
use crate::support_discharge;
use marrow_check::evolution::{RepairReason, Verdict, preview};
use marrow_check::{CHECK_UNRESOLVED_OPTIONAL, check_project};
use marrow_store::tree::TreeStore;
use marrow_store::value::Scalar;

use support::{check_with_accepted, config, temp_project, write};
use support_discharge::*;

/// A checked transform computing a new member from a sibling discharges to an
/// applyable transform verdict carrying the read-member catalog ids. The read member
/// `price` decodes under its current type, so the transform is activatable and the
/// verdict names the ids apply reads to build the `old` binding.
#[test]
fn transform_from_decodable_sibling_is_applyable() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-transform-applyable", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return old.price * 100\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "price", Scalar::Int(3));
    seed.member(1, "priceCents", Scalar::Int(300));

    let result = witness(&program, &store);

    let cents_id = member_catalog_id(&place, "priceCents")?;
    let price_id = member_catalog_id(&place, "price")?;

    assert!(result.is_activatable(), "{result:#?}");
    match verdict_for(&result, &cents_id) {
        Verdict::Transform { reads } => assert!(
            reads.iter().any(|id| id.as_str() == price_id),
            "transform reads must name `price`: {reads:#?}"
        ),
        other => panic!("expected transform, got {other:#?}"),
    }

    Ok(())
}

/// A transform body whose read member does not decode under its current type fails
/// closed: the read member's stored bytes were written under an incompatible type, so
/// reading `old.<member>` is unsound. The transform target discharges to a blocking
/// repair (it cannot be recomputed) and the witness is not activatable.
#[test]
fn transform_undecodable_read_member_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-transform-undecodable", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return old.price * 100\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // `price` was written as a string under its old type; it cannot decode as the
    // current `int`, so reading `old.price` is unsound and the transform must block.
    seed.record(1);
    seed.member(1, "price", Scalar::Str("not-an-int".into()));
    seed.member(1, "priceCents", Scalar::Int(0));

    let (result, diagnostics) = preview(&program, &store).expect("preview");
    let cents_id = member_catalog_id(&place, "priceCents")?;

    assert!(!result.is_activatable(), "{result:#?}");
    assert!(
        matches!(
            verdict_for(&result, &cents_id),
            Verdict::RepairRequired {
                reason: RepairReason::UndecodableTransformInput
            }
        ),
        "{:#?}",
        result.verdicts
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == cents_id),
        "{diagnostics:#?}"
    );

    Ok(())
}

/// A transform body that performs a saved write is impure and rejected at check time.
#[test]
fn transform_saved_write_is_check_error() {
    let root = temp_project("discharge-transform-impure-write", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       ^books(1).price = 9\n\
             \x20       return old.price * 100\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == marrow_check::CHECK_EVOLVE_TRANSFORM),
        "expected an impure-transform error: {:#?}",
        report.diagnostics
    );
}

/// Transform bodies are pure functions of `old`; reading any saved index would
/// let unrelated stored data influence every transformed record.
#[test]
fn transform_index_reads_are_check_errors() {
    let root = temp_project("discharge-transform-impure-index-read", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required shelf: string\n\
             \x20   required isbn: string\n\
             \x20   required published: int\n\
             \x20   required shelfMetric: int\n\
             \x20   required isbnMetric: int\n\
             \x20   required rangeMetric: int\n\
             store ^books(id: int): Book\n\
             \x20   index byShelf(shelf, id)\n\
             \x20   index byIsbn(isbn) unique\n\
             \x20   index byPublished(published, id)\n\
             evolve\n\
             \x20   transform Book.shelfMetric\n\
             \x20       return count(^books.byShelf(\"fiction\"))\n\
             \x20   transform Book.isbnMetric\n\
             \x20       return count(^books.byIsbn(\"978\"))\n\
             \x20   transform Book.rangeMetric\n\
             \x20       return count(^books.byPublished(1..10))\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    let transform_errors = report
        .diagnostics
        .iter()
        .filter(|d| d.code == marrow_check::CHECK_EVOLVE_TRANSFORM)
        .count();
    assert_eq!(
        transform_errors, 3,
        "expected one impure-transform error for each index read: {:#?}",
        report.diagnostics
    );
}

/// A transform body whose result type does not match the target member type is a
/// check error.
#[test]
fn transform_return_type_mismatch_is_check_error() {
    let root = temp_project("discharge-transform-rettype", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return \"a string\"\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == marrow_check::CHECK_RETURN_TYPE),
        "expected a return-type error: {:#?}",
        report.diagnostics
    );
}

/// A transform body reads `old.<member>` from the record's existing value set, so
/// a sparse member must be resolved at the read site just like a saved-root read.
#[test]
fn transform_bare_sparse_read_requires_resolution() {
    let root = temp_project("discharge-transform-sparse-bare", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             \x20   required summary: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.summary\n\
             \x20       return old.subtitle\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_UNRESOLVED_OPTIONAL),
        "{:#?}",
        report.diagnostics
    );
}

/// `??` resolves a sparse transform read at the read site.
#[test]
fn transform_coalesce_resolves_sparse_read() {
    let root = temp_project("discharge-transform-sparse-coalesce", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             \x20   required summary: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.summary\n\
             \x20       return old.subtitle ?? old.title\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_UNRESOLVED_OPTIONAL),
        "{:#?}",
        report.diagnostics
    );
}

/// `if const` resolves a sparse transform read before the branch consumes it.
#[test]
fn transform_if_const_resolves_sparse_read() {
    let root = temp_project("discharge-transform-sparse-if-const", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             \x20   required summary: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.summary\n\
             \x20       if const subtitle = old.subtitle\n\
             \x20           return subtitle\n\
             \x20       return old.title\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_UNRESOLVED_OPTIONAL),
        "{:#?}",
        report.diagnostics
    );
}

/// `exists` resolves a sparse transform read for the guarded branch.
#[test]
fn transform_exists_resolves_sparse_read() {
    let root = temp_project("discharge-transform-sparse-exists", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             \x20   required summary: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.summary\n\
             \x20       if exists(old.subtitle)\n\
             \x20           return old.subtitle\n\
             \x20       return old.title\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");

    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == CHECK_UNRESOLVED_OPTIONAL),
        "{:#?}",
        report.diagnostics
    );
}

/// Reading the transform's own target via `old.<target>` is a check error: the target
/// is the value being replaced, so its old bytes are not a sound input.
#[test]
fn transform_reading_own_target_is_check_error() {
    let root = temp_project("discharge-transform-readself", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required priceCents: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return old.priceCents * 2\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == marrow_check::CHECK_EVOLVE_TRANSFORM),
        "expected a read-own-target error: {:#?}",
        report.diagnostics
    );
}

/// Reading another transform's target via `old.<member>` is a check error: that
/// member's old bytes are about to be rewritten by its own transform, so they are not
/// a sound input for this one.
#[test]
fn transform_reading_other_transform_target_is_check_error() {
    let root = temp_project("discharge-transform-readother", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required a: int\n\
             \x20   required b: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.a\n\
             \x20       return 1\n\
             \x20   transform Book.b\n\
             \x20       return old.a + 1\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == marrow_check::CHECK_EVOLVE_TRANSFORM),
        "expected a read-other-transform-target error: {:#?}",
        report.diagnostics
    );
}

/// A transform body that directly reads a saved root (`^books(1).price`) is impure
/// and rejected at check time. Such a read escapes the per-record `old` model and the
/// decodability proof: it would let one record's value be written to every record. A
/// transform body may only read `old`.
#[test]
fn transform_reading_saved_root_is_check_error() {
    let root = temp_project("discharge-transform-savedread", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return ^books(1).price * 100\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == marrow_check::CHECK_EVOLVE_TRANSFORM),
        "expected a saved-read impurity error: {:#?}",
        report.diagnostics
    );
}

/// A transform body that calls a user-defined function is impure and rejected at check
/// time: this narrow model evaluates a transform as a self-contained pure expression over
/// `old` and does not propagate the callee's own effects into the transform, so the call
/// fails closed.
#[test]
fn transform_calling_user_function_is_check_error() {
    let root = temp_project("discharge-transform-callsfn", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required price: int\n\
             \x20   required priceCents: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.priceCents\n\
             \x20       return cents(old.price)\n\
             fn cents(price: int): int\n\
             \x20   return price * 100\n\
             pub fn add(price: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == marrow_check::CHECK_EVOLVE_TRANSFORM),
        "expected a user-function-call impurity error: {:#?}",
        report.diagnostics
    );
}

/// Reading `old.<member>` of a member a `default` in the same evolve block rewrites is
/// a check error: `old` exposes the pre-evolution value, not the post-default value the
/// developer intends, so the transform would compute from a value the same evolution is
/// changing.
#[test]
fn transform_reading_same_block_default_target_is_check_error() {
    let root = temp_project("discharge-transform-readdefault", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required base: int\n\
             \x20   required total: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   default Book.base = 10\n\
             \x20   transform Book.total\n\
             \x20       return old.base + 1\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == marrow_check::CHECK_EVOLVE_TRANSFORM),
        "expected a read-of-same-block-default-target error: {:#?}",
        report.diagnostics
    );
}

/// A transform target must be a top-level saved resource member: read resolution and
/// the per-record write address only handle a plain top-level field, so a nested target
/// (`Book.name.first`) is rejected at check time rather than silently mis-resolving.
#[test]
fn transform_of_nested_member_is_check_error() {
    let root = temp_project("discharge-transform-nested", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   name\n\
             \x20       required first: string\n\
             \x20       required last: string\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   transform Book.name.first\n\
             \x20       return \"x\"\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == marrow_check::CHECK_EVOLVE_TRANSFORM),
        "expected a nested-target error: {:#?}",
        report.diagnostics
    );
}

/// The witness composes the existing fingerprints: the accepted and proposal
/// catalog epoch/digest, the store engine profile + commit id, and the affected
/// catalog ids.
#[test]
fn witness_composes_catalog_and_store_fingerprints() -> Result<(), Box<dyn std::error::Error>> {
    // Commit a first schema, then add an optional member so the next check proposes
    // a changed catalog: the witness must carry both fingerprints.
    let root = temp_project("discharge-witness", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root).expect("committed fixture");
    let accepted_epoch = accepted.catalog.accepted_epoch.expect("accepted epoch");
    let accepted_digest = accepted.catalog.accepted_digest.clone().expect("digest");

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    // Adding `subtitle` without committing the new identity is exactly the pending
    // signal: the check reports a catalog-intent diagnostic, yet the proposal still
    // forms, so the witness must carry both the accepted and proposal fingerprints.
    let (_report, program) = check_with_accepted(&root);
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let (witness, _diagnostics) = preview(&program, &store).expect("preview");

    assert_eq!(witness.accepted_catalog.epoch, accepted_epoch);
    assert_eq!(witness.accepted_catalog.digest, accepted_digest);
    let proposal = witness.proposal_catalog.clone().expect("proposal");
    assert_eq!(
        proposal.epoch,
        accepted_epoch + 1,
        "proposal advances the accepted epoch"
    );
    assert_eq!(
        Some(proposal.digest),
        program
            .catalog
            .proposal
            .as_ref()
            .map(|catalog| catalog.digest.clone())
    );
    // No commit metadata was stamped, so the witness records no commit id.
    assert_eq!(witness.store_commit_id, None);
    // The subtitle member the proposal newly adds is among the affected ids. Its
    // bound place id is empty until the proposal is accepted, so read the minted
    // stable id from the proposal entries.
    let subtitle_id = new_member_proposal_id(&program, "books::Book::subtitle")?;
    assert!(
        witness
            .changed_root_catalog_ids
            .iter()
            .any(|id| id.as_str() == subtitle_id),
        "{witness:#?}"
    );

    Ok(())
}
