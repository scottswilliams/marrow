//! E07-M — the M-shaped corpus: ERP/clinical scale slices that feed the E07 veto.
//!
//! Three frozen projects under `fixtures/v01/e07_m_corpus/`, each driven from source
//! through the shared harness's production path (capture -> compile -> verify ->
//! attach -> VM):
//!
//! - `clinical` — a wide sparse `Chart`: 2000 declared fields (100 top-level + 20
//!   resource-level groups of 95) plus scalar-field branches, read and written
//!   presence-first, with the int-floor fixed-scale arithmetic slice (`bmiTimesTen`).
//!   The counted watch records its exact operation-site count and image byte size.
//! - `erp` — multiple store roots (catalog, order book, id counter), managed indexes
//!   read inside nested loops, a whole-entry partial-copy round trip, accumulation,
//!   an early-rejection/late-commit order placement, and the fixed-scale money idiom
//!   carried by a nominal `Cents` type.
//! - `m_traversal` — an org tree three keyed-branch layers deep: three-level nested
//!   bounded traversal with an `on more` arm at each level, a three-deep Result
//!   error-bubbling chain, and a bounded innermost-first purge.
//!
//! Assertions are typed VM outcomes (values, faults, Result variants), never rendered
//! prose. The freeze findings this lane records — the counted-watch numbers, the
//! keyed-scalar-leaf ceremony tally, and the enum-reuse verification bug — live in the
//! lane report, and the enforcement artifacts here (the exact site count, the
//! demand-shape checks) pin them against drift.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{CallOutcome, Project, Session};
use marrow_compile::compile;
use marrow_image::bounds::{MAX_IMAGE_BYTES, MAX_SITES};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, capture};
use marrow_verify::verify;
use marrow_vm::Value;

// ---------------------------------------------------------------------------
// Outcome helpers — assert typed shapes, never prose.
// ---------------------------------------------------------------------------

fn some_int(v: i64) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Int(v)))))
}

fn some_text(s: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(s.into())))))
}

fn text_of(value: &Value) -> String {
    match value {
        Value::Text(s) => s.to_string(),
        other => panic!("not text: {other:?}"),
    }
}

fn int_of(value: &Value) -> i64 {
    match value {
        Value::Int(n) => *n,
        other => panic!("not int: {other:?}"),
    }
}

/// The payload of a `Result` `ok(v)` (variant 0, declaration order).
fn ok_payload(value: Option<Value>) -> Value {
    match value {
        Some(Value::Enum(_, 0, payload)) => payload[0].clone(),
        other => panic!("expected ok(..), got {other:?}"),
    }
}

/// The `err(e)` message text (variant 1).
fn err_text(value: Option<Value>) -> String {
    match value {
        Some(Value::Enum(_, 1, payload)) => text_of(&payload[0]),
        other => panic!("expected err(..), got {other:?}"),
    }
}

fn i(n: i64) -> Value {
    Value::Int(n)
}

fn t(s: &str) -> Value {
    Value::Text(s.into())
}

fn call(session: &mut Session, export: &str, args: Vec<Value>) -> Option<Value> {
    session.call(export, args)
}

// ---------------------------------------------------------------------------
// The counted watch — clinical wide resource at 2000 declared fields.
// ---------------------------------------------------------------------------

fn fixture_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("v01")
        .join(name)
}

fn collect(base: &Path, dir: &Path, out: &mut Vec<CapturedFile>) {
    for entry in fs::read_dir(dir).expect("read src") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            collect(base, &path, out);
        } else {
            let key = path
                .strip_prefix(base.parent().unwrap())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push(CapturedFile::new(key, fs::read(&path).unwrap()));
        }
    }
}

/// Compile a fixture directly through the production compiler and return the raw
/// emitted image byte length alongside the verified image, so the counted watch can
/// size the image the verifier decodes.
fn compile_and_size(name: &str) -> (usize, marrow_verify::VerifiedImage) {
    let root = fixture_dir(name);
    let manifest = Manifest::parse(&fs::read_to_string(root.join("marrow.toml")).unwrap()).unwrap();
    let ids = fs::read(root.join("marrow.ids")).ok();
    let mut files = Vec::new();
    let src = root.join("src");
    collect(&src, &src, &mut files);
    let input = capture(&manifest, files, ids.as_deref(), &CaptureLimits::DEFAULT)
        .expect("capture fixture");
    let compiled = compile(&input).expect("fixture compiles");
    let bytes = compiled.image.bytes.len();
    let image = verify(&compiled.image.bytes).expect("fixture verifies");
    (bytes, image)
}

/// The M-shaped counted watch. With lazy field-leaf emission (BND02 C1) the site count
/// is the eager per-node sites (placements and whole-group sites) plus one field-leaf site
/// per field the code actually addresses — not one per declared field. This is the
/// sparse-at-scale win: a 2000-field resource whose code touches a handful of fields emits
/// a handful of field sites, so the site count and image size collapse. The count is frozen
/// so a regression back to eager per-declared-field emission is conspicuous.
#[test]
fn counted_watch_clinical_2000_fields() {
    let (bytes, image) = compile_and_size("e07_m_corpus/clinical");
    let sites = image.sites().len();

    // Recorded freeze count (BND02 C1 re-baseline): 27 operation sites — the eager
    // per-node sites (1 root placement + 2 branch placements + 20 whole-group sites) plus
    // one field-leaf site per field the clinical code addresses. Former eager emission was
    // 2028 (one leaf per declared field); lazy emission drops it ~98% because the fixture
    // touches only a handful of its 2000 declared fields, so declared-but-untouched fields
    // mint no site. The emitted image shrank correspondingly.
    assert_eq!(
        sites, 27,
        "clinical operation-site count is frozen at 27 (lazy field-leaf emission)"
    );
    assert_eq!(image.roots().len(), 1, "one top-level store root");

    assert!(
        sites < MAX_SITES,
        "site count {sites} clears the {MAX_SITES} site table with headroom",
    );
    assert!(
        bytes < MAX_IMAGE_BYTES,
        "image {bytes} bytes clears the {MAX_IMAGE_BYTES}-byte image bound",
    );
    // The counted watch does not red at 2000 fields: with lazy field-leaf emission the
    // site count (27) and image size are a small fraction of budget — declared width no
    // longer drives either.
    assert!(
        bytes < MAX_IMAGE_BYTES / 2,
        "image byte size stays well under budget"
    );
}

// ---------------------------------------------------------------------------
// Clinical — presence-first reads and writes over the wide sparse chart.
// ---------------------------------------------------------------------------

fn clinical() -> Session {
    Project::from_fixture("e07_m_corpus/clinical").session()
}

#[test]
fn clinical_records_and_reads_a_required_field() {
    let mut s = clinical();
    assert_eq!(
        call(&mut s, "nameOf", vec![i(1)]),
        Some(Value::Optional(None))
    );
    call(&mut s, "recordName", vec![i(1), t("Ada Lovelace")]);
    assert_eq!(
        call(&mut s, "nameOf", vec![i(1)]),
        some_text("Ada Lovelace")
    );
}

#[test]
fn clinical_group_leaf_writes_land_and_read_back() {
    let mut s = clinical();
    call(&mut s, "recordName", vec![i(2), t("Grace Hopper")]);
    call(&mut s, "setVital", vec![i(2), i(120), i(80), i(66)]);
    assert_eq!(call(&mut s, "systolicOf", vec![i(2)]), some_int(120));

    // The presence prelude surfaces each missing precondition as a typed err, and
    // the happy path only once every guard passes.
    call(&mut s, "recordName", vec![i(3), t("Katherine Johnson")]);
    assert_eq!(
        err_text(s.call("bpSummary", vec![i(3)])),
        "Katherine Johnson has no systolic",
    );
    call(&mut s, "setVital", vec![i(3), i(118), i(76), i(60)]);
    assert_eq!(
        text_of(&ok_payload(s.call("bpSummary", vec![i(3)]))),
        "Katherine Johnson 118/76"
    );

    // An unknown chart is rejected before any field read.
    assert_eq!(err_text(s.call("bpSummary", vec![i(999)])), "unknown chart");
}

#[test]
fn clinical_int_floor_bmi_uses_fixed_x10_scale() {
    let mut s = clinical();
    call(&mut s, "recordName", vec![i(4), t("patient")]);
    // 80 kg, 2.00 m -> BMI 20.0 -> carried at x10 scale as 200 (an exact ratio).
    call(&mut s, "setWeight", vec![i(4), i(80000)]);
    call(&mut s, "setHeight", vec![i(4), i(2000)]);
    assert_eq!(int_of(&ok_payload(s.call("bmiTimesTen", vec![i(4)]))), 200);

    // A fractional ratio pins the half-add rounding: 81 kg / 2.00 m is BMI 20.25, so
    // the x10 value is 202.5, which the (+ areaMm2/2) half-add rounds up to 203.
    // Plain truncation would yield 202, so this case fails if the half-add is dropped.
    call(&mut s, "recordName", vec![i(9), t("rounds up")]);
    call(&mut s, "setWeight", vec![i(9), i(81000)]);
    call(&mut s, "setHeight", vec![i(9), i(2000)]);
    assert_eq!(int_of(&ok_payload(s.call("bmiTimesTen", vec![i(9)]))), 203);

    // A missing measurement bubbles the source-constructed err.
    call(&mut s, "recordName", vec![i(5), t("no height")]);
    call(&mut s, "setWeight", vec![i(5), i(70000)]);
    assert_eq!(err_text(s.call("bmiTimesTen", vec![i(5)])), "no height");
}

#[test]
fn clinical_three_state_option_field_is_absent_none_or_some() {
    let mut s = clinical();
    call(&mut s, "recordName", vec![i(6), t("three state")]);
    // Absent: never recorded.
    assert_eq!(
        call(&mut s, "glucoseState", vec![i(6)]),
        Some(Value::Text("unrecorded".into()))
    );
    // Present `some`.
    call(&mut s, "recordGlucose", vec![i(6), i(95)]);
    assert_eq!(
        call(&mut s, "glucoseState", vec![i(6)]),
        Some(Value::Text("measured".into()))
    );
    // Present `none` — recorded as unmeasurable, distinct from absent.
    call(&mut s, "recordGlucoseUnmeasurable", vec![i(6)]);
    assert_eq!(
        call(&mut s, "glucoseState", vec![i(6)]),
        Some(Value::Text("unmeasurable".into()))
    );
}

#[test]
fn clinical_observation_branch_accumulates_present_values() {
    let mut s = clinical();
    call(&mut s, "recordName", vec![i(7), t("obs")]);
    call(
        &mut s,
        "addObservation",
        vec![i(7), i(1), t("HR"), Value::Instant(0), i(60)],
    );
    call(
        &mut s,
        "addObservation",
        vec![i(7), i(2), t("HR"), Value::Instant(1_000_000_000), i(72)],
    );
    assert_eq!(
        call(&mut s, "observationCount", vec![i(7)]),
        Some(Value::Int(2))
    );
    assert_eq!(
        call(&mut s, "observationTotal", vec![i(7)]),
        Some(Value::Int(132))
    );
}

// The keyed-scalar-leaf ceremony: an ordered list of diagnosis codes is modelled as
// a single-field wrapper branch (the qualified constructor at the write, a
// whole-entry traversal at the read) because a keyed scalar leaf `diagnoses(pos):
// string` is not yet executable. The round trip still works; the wrapper is the
// ceremony this corpus records.
#[test]
fn clinical_keyed_scalar_leaf_wrapper_round_trips() {
    let mut s = clinical();
    call(&mut s, "recordName", vec![i(8), t("dx")]);
    assert_eq!(
        call(&mut s, "diagnosisCount", vec![i(8)]),
        Some(Value::Int(0))
    );
    call(&mut s, "addDiagnosis", vec![i(8), i(1), t("E11.9")]);
    call(&mut s, "addDiagnosis", vec![i(8), i(2), t("I10")]);
    assert_eq!(
        call(&mut s, "diagnosisCount", vec![i(8)]),
        Some(Value::Int(2))
    );
}

// ---------------------------------------------------------------------------
// ERP — multi-root, indexes in nested loops, partial-copy round trip, money.
// ---------------------------------------------------------------------------

fn erp() -> Session {
    Project::from_fixture("e07_m_corpus/erp").session()
}

#[test]
fn erp_partial_copy_round_trip_preserves_unread_fields() {
    let mut s = erp();
    call(
        &mut s,
        "addItem",
        vec![t("A1"), t("Widget"), t("hardware"), i(500)],
    );
    call(&mut s, "setBarcode", vec![t("A1"), t("0001")]);
    // The markup reads the whole entry into a local, changes one field, and writes
    // it back — the sparse `barcode` set earlier survives the round trip.
    assert_eq!(
        int_of(&ok_payload(
            s.call("applyMarkupPercent", vec![t("A1"), i(20)])
        )),
        600
    );
    assert_eq!(call(&mut s, "priceOf", vec![t("A1")]), some_int(600));
    assert_eq!(
        call(&mut s, "barcodeTaken", vec![t("0001")]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(&mut s, "nameByBarcode", vec![t("0001")]),
        some_text("Widget")
    );
}

#[test]
fn erp_fixed_scale_money_validates_and_accumulates_in_int() {
    let mut s = erp();
    // The nominal Cents guards the unit price; the line total accumulates in int.
    assert_eq!(
        int_of(&ok_payload(
            s.call("extendedPriceCents", vec![i(250), i(4)])
        )),
        1000
    );
    assert_eq!(
        err_text(s.call("extendedPriceCents", vec![i(-1), i(4)])),
        "unit price out of fixed-scale range"
    );
    assert_eq!(
        err_text(s.call("extendedPriceCents", vec![i(250), i(-1)])),
        "negative quantity"
    );
}

#[test]
fn erp_order_placement_allocates_late_commits_and_totals() {
    let mut s = erp();
    call(
        &mut s,
        "addItem",
        vec![t("SKU1"), t("Bolt"), t("hardware"), i(150)],
    );
    call(
        &mut s,
        "addItem",
        vec![t("SKU2"), t("Nut"), t("hardware"), i(90)],
    );

    // Early rejection: a missing customer returns err before allocating.
    assert_eq!(
        err_text(s.call("placeOrder", vec![t(""), Value::Instant(0)])),
        "missing customer"
    );

    let oid = int_of(&ok_payload(
        s.call("placeOrder", vec![t("Acme"), Value::Instant(0)]),
    ));
    assert_eq!(oid, 1);
    assert_eq!(
        int_of(&ok_payload(
            s.call("addLine", vec![i(oid), i(1), t("SKU1"), i(10)])
        )),
        1
    );
    assert_eq!(
        int_of(&ok_payload(
            s.call("addLine", vec![i(oid), i(2), t("SKU2"), i(5)])
        )),
        2
    );
    // 10*150 + 5*90 = 1950.
    assert_eq!(
        call(&mut s, "orderTotalCents", vec![i(oid)]),
        Some(Value::Int(1950))
    );

    // A nonpositive quantity is rejected and stages no line.
    assert_eq!(
        err_text(s.call("addLine", vec![i(oid), i(3), t("SKU1"), i(0)])),
        "nonpositive quantity"
    );
}

#[test]
fn erp_index_scan_nested_in_a_loop_accumulates_catalog_value() {
    let mut s = erp();
    call(
        &mut s,
        "addItem",
        vec![t("H1"), t("Hammer"), t("tools"), i(1200)],
    );
    call(
        &mut s,
        "addItem",
        vec![t("H2"), t("Saw"), t("tools"), i(1500)],
    );
    call(
        &mut s,
        "addItem",
        vec![t("K1"), t("Fork"), t("kitchen"), i(300)],
    );

    // tools: 1200 + 1500, kitchen: 300 = 3000.
    assert_eq!(
        call(&mut s, "catalogValueOfTwo", vec![t("tools"), t("kitchen")]),
        Some(Value::Int(3000)),
    );
}

// ---------------------------------------------------------------------------
// M-shaped traversal — three-level nesting, error bubbling, bounded purge.
// ---------------------------------------------------------------------------

fn m_traversal() -> Session {
    Project::from_fixture("e07_m_corpus/m_traversal").session()
}

fn seed_org(s: &mut Session, org: i64) {
    call(s, "createOrg", vec![i(org), t("Org")]);
    call(s, "addDepartment", vec![i(org), i(1), t("Eng")]);
    call(s, "addTeam", vec![i(org), i(1), i(1), t("Platform")]);
    call(s, "addTeam", vec![i(org), i(1), i(2), t("Product")]);
    call(
        s,
        "addMember",
        vec![i(org), i(1), i(1), i(1), t("a"), Value::Bool(true)],
    );
    call(
        s,
        "addMember",
        vec![i(org), i(1), i(1), i(2), t("b"), Value::Bool(false)],
    );
    call(
        s,
        "addMember",
        vec![i(org), i(1), i(2), i(1), t("c"), Value::Bool(true)],
    );
}

#[test]
fn m_three_level_traversal_counts_active_members() {
    let mut s = m_traversal();
    seed_org(&mut s, 1);
    // Two of three seeded members are active, across two teams under one department.
    assert_eq!(
        call(&mut s, "activeMemberCount", vec![i(1)]),
        Some(Value::Int(2))
    );
}

#[test]
fn m_error_bubbles_three_deep_from_source_to_boundary() {
    let mut s = m_traversal();
    seed_org(&mut s, 1);
    // A present member surfaces its name through the three-deep `try` chain.
    assert_eq!(
        text_of(&ok_payload(
            s.call("lookupMemberName", vec![i(1), i(1), i(1), i(1)])
        )),
        "a"
    );
    // An absent member's err is constructed once at the deepest layer and surfaced
    // unchanged at the boundary.
    assert_eq!(
        err_text(s.call("lookupMemberName", vec![i(1), i(1), i(1), i(99)])),
        "no such member"
    );
}

#[test]
fn m_bounded_purge_removes_the_whole_subtree() {
    let mut s = m_traversal();
    seed_org(&mut s, 1);
    assert_eq!(
        call(&mut s, "orgExists", vec![i(1)]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        call(&mut s, "memberActive", vec![i(1), i(1), i(1), i(1)]),
        Some(Value::Bool(true))
    );

    call(&mut s, "purgeOrg", vec![i(1)]);

    // Every node is gone: the root reads absent and a deep member reads its
    // sparse-field default.
    assert_eq!(
        call(&mut s, "orgExists", vec![i(1)]),
        Some(Value::Bool(false))
    );
    assert_eq!(
        call(&mut s, "orgName", vec![i(1)]),
        Some(Value::Optional(None))
    );
    assert_eq!(
        call(&mut s, "memberActive", vec![i(1), i(1), i(1), i(1)]),
        Some(Value::Bool(false))
    );
    assert_eq!(
        call(&mut s, "activeMemberCount", vec![i(1)]),
        Some(Value::Int(0))
    );
}

#[test]
fn m_absent_member_read_is_a_clean_outcome_not_a_fault() {
    let mut s = m_traversal();
    // Reading through a fully absent tree yields the sparse default, never a fault.
    assert!(matches!(
        s.try_call("memberActive", vec![i(1), i(1), i(1), i(1)]),
        CallOutcome::Value(Some(Value::Bool(false))),
    ));
}
