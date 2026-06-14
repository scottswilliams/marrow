use crate::support;
use crate::support_discharge;
use marrow_check::check_project;
use marrow_check::evolution::{RejectedDefault, RepairReason, Verdict, preview};
use marrow_store::tree::TreeStore;
use marrow_store::value::{Scalar, encode_value};

use support::{config, temp_project, with_code, write};
use support_discharge::*;

/// Adding an optional sparse field over existing records is a no-op. The store
/// needs no rewrite and the witness records zero backfill.
#[test]
fn optional_sparse_add_needs_no_rewrite() {
    let root = temp_project("discharge-optional-add", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));
    seed.record(2);
    seed.member(2, "title", Scalar::Str("Hyperion".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    let subtitle_id = member_catalog_id(&place, "subtitle");
    assert!(
        matches!(verdict_for(&result, &subtitle_id), Verdict::NoOp),
        "{:#?}",
        result.verdicts
    );
    assert_eq!(result.counts.records_to_backfill, 0);
    assert_eq!(result.counts.records_lacking_member, 0);
    assert_eq!(result.counts.scanned_records, 2);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

/// A newly-required member with an `evolve default` discharges to a backfill plan:
/// the witness counts the records lacking it.
#[test]
fn required_with_default_backfills_old_records() {
    let root = temp_project("discharge-required-default", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   default Book.pages = 0\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // Old records carry `title` but predate the new required `pages`.
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));
    seed.record(2);
    seed.member(2, "title", Scalar::Str("Hyperion".into()));

    let result = witness(&program, &store);

    let pages_id = member_catalog_id(&place, "pages");
    match verdict_for(&result, &pages_id) {
        // The constant evolve default `0` flows into the witness as the encoded
        // int the apply phase backfills with.
        Verdict::Default { value } => {
            assert_eq!(value.scalar_type, marrow_store::value::ScalarType::Int);
            assert_eq!(
                value.encoded,
                marrow_store::value::encode_value(&Scalar::Int(0)).unwrap()
            );
        }
        other => panic!("expected default, got {other:#?}"),
    }
    assert_eq!(result.counts.records_lacking_member, 2);
    assert_eq!(result.counts.records_to_backfill, 2);
}

/// A constant temporal or bytes default written through a validating constructor
/// (`date("...")`, `duration("...")`, `bytes("...")`) is a constant the checker
/// evaluates against the same canonical form a stored value must satisfy, so each
/// discharges to a `Default` backfill rather than forcing a transform.
#[test]
fn constant_temporal_and_bytes_defaults_discharge_as_default() {
    let root = temp_project("discharge-temporal-bytes-default", |root| {
        write(
            root,
            "src/events.mw",
            "module events\n\
             resource Event\n\
             \x20   required title: string\n\
             \x20   required day: date\n\
             \x20   required span: duration\n\
             \x20   required payload: bytes\n\
             store ^events(id: int): Event\n\
             evolve\n\
             \x20   default Event.day = date(\"2020-01-01\")\n\
             \x20   default Event.span = duration(\"PT3600S\")\n\
             \x20   default Event.payload = bytes(\"hi\")\n\
             pub fn add(title: string): Id(^events)\n\
             \x20   return nextId(^events)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "events");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // Old records carry `title` but predate the three new required members.
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Launch".into()));

    let result = witness(&program, &store);

    let expect_default = |member: &str, expected: Scalar| {
        let member_id = member_catalog_id(&place, member);
        match verdict_for(&result, &member_id) {
            Verdict::Default { value } => {
                assert_eq!(value.scalar_type, expected.ty(), "{member} type");
                assert_eq!(
                    value.encoded,
                    encode_value(&expected).unwrap(),
                    "{member} encoded value",
                );
            }
            other => panic!("expected default for `{member}`, got {other:#?}"),
        }
    };
    // `2020-01-01` is 18262 days after the Unix epoch; one hour is 3.6e12 ns.
    expect_default("day", Scalar::Date(18262));
    expect_default("span", Scalar::Duration(3_600_000_000_000));
    expect_default("payload", Scalar::Bytes(b"hi".to_vec()));
}

/// A `bytes` default takes the argument string's raw UTF-8 bytes, so every string is a
/// valid canonical `bytes` value. The argument is read as a string, not a bytes literal:
/// a backslash-x sequence contributes its literal characters, never a decoded byte. This
/// pins the any-string contract `bytes(string)` carries at runtime.
#[test]
fn bytes_default_accepts_any_string_as_raw_utf8() {
    let root = temp_project("discharge-bytes-default-any-string", |root| {
        write(
            root,
            "src/events.mw",
            "module events\n\
             resource Event\n\
             \x20   required title: string\n\
             \x20   required payload: bytes\n\
             store ^events(id: int): Event\n\
             evolve\n\
             \x20   default Event.payload = bytes(\"a\\\\x00b\")\n\
             pub fn add(title: string): Id(^events)\n\
             \x20   return nextId(^events)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "events");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Launch".into()));

    let (result, _diagnostics) = preview(&program, &store).expect("preview");

    let member_id = member_catalog_id(&place, "payload");
    match verdict_for(&result, &member_id) {
        Verdict::Default { value } => {
            assert_eq!(value.scalar_type, marrow_store::value::ScalarType::Bytes);
            assert_eq!(
                value.encoded,
                b"a\\x00b".to_vec(),
                "the bytes default is the argument string's literal UTF-8, not a decoded escape",
            );
        }
        other => panic!("expected a bytes default, got {other:#?}"),
    }
}

/// A temporal constructor default whose string is not the canonical saved form is not
/// a value the store could read back, so it fails closed as out of range rather than
/// silently normalizing or accepting it. The validation is exactly the canonical-form
/// boundary stored values pass.
#[test]
fn non_canonical_temporal_default_fails_closed() {
    let root = temp_project("discharge-noncanonical-temporal-default", |root| {
        write(
            root,
            "src/events.mw",
            "module events\n\
             resource Event\n\
             \x20   required title: string\n\
             \x20   required day: date\n\
             store ^events(id: int): Event\n\
             evolve\n\
             \x20   default Event.day = date(\"2020-2-30\")\n\
             pub fn add(title: string): Id(^events)\n\
             \x20   return nextId(^events)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "events");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Launch".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    let day_id = member_catalog_id(&place, "day");
    // A declared default the checker cannot encode is not a missing member: the
    // developer named a fill, so the verdict names the rejected default by its typed
    // cause rather than collapsing into the no-default-at-all case.
    assert!(
        matches!(
            verdict_for(&result, &day_id),
            Verdict::RepairRequired {
                reason: RepairReason::DefaultRejected {
                    reason: RejectedDefault::NotEncodable
                }
            }
        ),
        "{:#?}",
        result.verdicts
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == day_id),
        "{diagnostics:#?}"
    );
}

/// An `evolve default` whose value is not a constant the checker can evaluate is not
/// a default at all; it fails closed with a diagnostic steering the developer to a
/// transform. A per-record-varying fill is a transform, not a default.
#[test]
fn non_constant_default_fails_closed_with_transform_hint() {
    let root = temp_project("discharge-nonconst-default", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   default Book.pages = 1 + 1\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    let pages_id = member_catalog_id(&place, "pages");
    // A declared non-constant default is steered to a transform by a typed cause, not by
    // the missing-member verdict a member with no default carries.
    assert!(
        matches!(
            verdict_for(&result, &pages_id),
            Verdict::RepairRequired {
                reason: RepairReason::DefaultRejected {
                    reason: RejectedDefault::NotConstant
                }
            }
        ),
        "{:#?}",
        result.verdicts
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == pages_id),
        "{diagnostics:#?}"
    );
}

#[test]
fn entries_default_value_is_rejected_as_loop_head_only() {
    let root = temp_project("discharge-default-entries-value", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   default Book.pages = entries(^books)\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.collection_unsupported");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

/// A newly-required member with no default and records missing it cannot attach
/// data. Activation fails and the diagnostic names the exact records.
#[test]
fn required_without_default_fails_naming_records() {
    let root = temp_project("discharge-required-no-default", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required pages: int\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));
    seed.record(2);
    seed.member(2, "title", Scalar::Str("Hyperion".into()));

    let (witness, diagnostics) = preview(&program, &store).expect("preview");

    let pages_id = member_catalog_id(&place, "pages");
    assert!(!witness.is_activatable(), "{witness:#?}");
    // Both seeded records lack the new required member: the typed count proves the
    // record total and the diagnostic names the member by its catalog id; the
    // per-record key list ("1", "2") is carried only in the message.
    assert_eq!(witness.counts.records_lacking_member, 2, "{witness:#?}");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == pages_id),
        "{diagnostics:#?}"
    );
}

/// A present required member is not proven unless its bytes decode under the
/// current leaf type. Presence alone would let an old scalar type's bytes activate
/// as the new type and fault later at read time.
#[test]
fn required_present_member_with_incompatible_bytes_repairs() {
    let root = temp_project("discharge-required-invalid-bytes", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: int\n\
             store ^books(id: int): Book\n\
             pub fn add(title: int): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root);
    let place = root_place(&program, "books");
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member(1, "title", Scalar::Str("not an int".into()));

    let (witness, diagnostics) = preview(&program, &store).expect("preview");

    let title_id = member_catalog_id(&place, "title");
    assert!(!witness.is_activatable(), "{witness:#?}");
    assert!(
        matches!(
            verdict_for(&witness, &title_id),
            Verdict::RepairRequired {
                reason: RepairReason::InvalidStoredValue
            }
        ),
        "{:#?}",
        witness.verdicts
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.catalog_id.as_str() == title_id),
        "{diagnostics:#?}"
    );
}
