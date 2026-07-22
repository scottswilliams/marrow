//! The Club Locker application: the E07 flagship gate, every journey driven end to
//! end through the production path.
//!
//! Club Locker is an offline equipment-lending application (the A02a frozen journey
//! set): members and assets are keyed durable records, a checkout is one atomic
//! five-effect transaction, and human-facing member/asset/loan numbers are allocated
//! gaplessly from application-owned counters. The whole application is one module,
//! `fixtures/v01/club_locker`, authored as ordinary `.mw` source with a complete,
//! committed identity ledger; this file is the assertions.
//!
//! Each test drives the application through the shared harness's persistent ephemeral
//! attachment (`Project::session`): a committed `transaction` is observable by a later
//! reading export and a rolled-back one is not, so the domain journeys — provision,
//! checkout, return, unique collision, bounded traversal with overflow, exact erase
//! versus bounded subtree removal — are exercised against the real capture → compile →
//! verify → attach → VM pipeline with no raw fixture seeding. The failure-schedule and
//! backup/restore journeys (J7–J10, F1–F7) are native-store and lost-reply concerns
//! outside the memory path this gate measures.
//!
//! The frozen `marrow check` demand report and the counted bound watches
//! (exports/sites/image bytes) are pinned as their own tests: the demand sentence is
//! the A5B01 surface, and the watches are the ratified E07 bounds-audit obligation.

mod common;

use common::Project;
use marrow_project::{CaptureLimits, CapturedFile, Manifest, capture};
use marrow_verify::{VerifiedImage, verify};
use marrow_vm::Value;

// ---------------------------------------------------------------------------
// Value helpers: the harness passes typed VM values directly, so an export's
// `date` argument is an opaque day count and a `Result` return is a sealed enum.
// ---------------------------------------------------------------------------

/// A `date` argument. The stored day count is opaque to these journeys — no
/// assertion reads a date back — so any in-range day serves.
fn day(n: i32) -> Value {
    Value::Date(n)
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

/// The present arm of an optional return (`Optional(Some(..))`).
fn some_text(s: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(text(s)))))
}

fn some_int(n: i64) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Int(n)))))
}

/// The absent arm of an optional return.
fn absent() -> Option<Value> {
    Some(Value::Optional(None))
}

/// The `ok(v)` payload of a `Result` return. `Result` declares `ok` then `err`, so
/// `ok` is variant 0.
fn expect_ok_int(value: Option<Value>) -> i64 {
    match value {
        Some(Value::Enum(_, 0, payload)) => match payload.as_ref() {
            [Value::Int(n)] => *n,
            other => panic!("ok payload is not one int: {other:?}"),
        },
        other => panic!("not an ok result: {other:?}"),
    }
}

/// The `ok(v)` payload of a `Result<bool, _>` return.
fn expect_ok_bool(value: Option<Value>) -> bool {
    match value {
        Some(Value::Enum(_, 0, payload)) => match payload.as_ref() {
            [Value::Bool(b)] => *b,
            other => panic!("ok payload is not one bool: {other:?}"),
        },
        other => panic!("not an ok result: {other:?}"),
    }
}

/// Assert a `Result` return is `err(..)` (variant 1), the typed failure outcome.
fn expect_err(value: Option<Value>) {
    match value {
        Some(Value::Enum(_, 1, _)) => {}
        other => panic!("not an err result: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Shared provisioning
// ---------------------------------------------------------------------------

/// A session provisioned through the domain exports: two members and three assets,
/// the fixture the checkout/return/retire journeys start from.
fn provisioned() -> common::Session {
    let mut club = Project::from_fixture("club_locker").session();
    club.call("registerMember", vec![text("Ada Lovelace"), day(20000)]);
    club.call("registerMember", vec![text("Grace Hopper"), day(20001)]);
    expect_ok_int(club.call(
        "registerAsset",
        vec![text("R-100"), text("racquets"), text("Racquet")],
    ));
    expect_ok_int(club.call(
        "registerAsset",
        vec![text("B-200"), text("balls"), text("Ball bucket")],
    ));
    expect_ok_int(club.call(
        "registerAsset",
        vec![text("R-300"), text("racquets"), text("Racquet 2")],
    ));
    club
}

// ---------------------------------------------------------------------------
// J1 — provision and first data
// ---------------------------------------------------------------------------

/// Provision registers members and assets through ordinary domain exports. Member
/// and asset numbers are allocated gaplessly from the application's own counters, and
/// the reads observe the committed entries.
#[test]
fn j1_provision_registers_members_and_assets() {
    let mut club = Project::from_fixture("club_locker").session();

    assert_eq!(
        club.call("registerMember", vec![text("Ada Lovelace"), day(20000)]),
        Some(Value::Int(1)),
        "the first member number is 1",
    );
    assert_eq!(
        club.call("registerMember", vec![text("Grace Hopper"), day(20001)]),
        Some(Value::Int(2)),
        "the counter advances gaplessly to 2",
    );

    assert_eq!(
        expect_ok_int(club.call(
            "registerAsset",
            vec![text("R-100"), text("racquets"), text("Racquet")]
        )),
        1,
    );
    assert_eq!(
        expect_ok_int(club.call(
            "registerAsset",
            vec![text("B-200"), text("balls"), text("Ball bucket")]
        )),
        2,
    );

    assert_eq!(
        club.call("memberName", vec![Value::Int(1)]),
        some_text("Ada Lovelace")
    );
    assert_eq!(
        club.call("memberExists", vec![Value::Int(1)]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        club.call("memberExists", vec![Value::Int(99)]),
        Some(Value::Bool(false))
    );
    assert_eq!(
        club.call("assetTag", vec![Value::Int(1)]),
        some_text("R-100")
    );
    assert_eq!(
        club.call("assetExists", vec![Value::Int(2)]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        club.call("tagTaken", vec![text("R-100")]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        club.call("tagTaken", vec![text("NOPE")]),
        Some(Value::Bool(false))
    );
    assert_eq!(
        club.call("assetNameByTag", vec![text("B-200")]),
        some_text("Ball bucket")
    );
    assert_eq!(club.call("assetNameByTag", vec![text("NOPE")]), absent());
}

// ---------------------------------------------------------------------------
// J4 — unique collision
// ---------------------------------------------------------------------------

/// Registering an asset whose tag duplicates an existing one is a typed collision
/// outcome that writes nothing: the tag guard returns before the counter is advanced,
/// so a following register still gets the next sequential number.
#[test]
fn j4_duplicate_tag_is_a_typed_collision_with_no_partial_write() {
    let mut club = Project::from_fixture("club_locker").session();

    assert_eq!(
        expect_ok_int(club.call(
            "registerAsset",
            vec![text("DUP"), text("racquets"), text("First")]
        )),
        1,
    );
    expect_err(club.call(
        "registerAsset",
        vec![text("DUP"), text("racquets"), text("Second")],
    ));

    // The collided attempt advanced no counter and created no entry: the next
    // successful register is number 2, and only the original name is registered.
    assert_eq!(
        expect_ok_int(club.call(
            "registerAsset",
            vec![text("OK"), text("racquets"), text("Third")]
        )),
        2,
    );
    assert_eq!(
        club.call("assetNameByTag", vec![text("DUP")]),
        some_text("First")
    );
    assert_eq!(
        club.call("assetExists", vec![Value::Int(3)]),
        Some(Value::Bool(false))
    );
}

// ---------------------------------------------------------------------------
// J2/J3 — checkout and return
// ---------------------------------------------------------------------------

/// One checkout is a single atomic transaction across five effects: allocate a loan
/// number, mark the asset on loan, add the member's active-loan entry, bump the
/// member's history sequence, and append the history event. Every effect is observable
/// together after commit.
#[test]
fn j2_checkout_is_one_atomic_five_effect_transaction() {
    let mut club = provisioned();

    let loan = expect_ok_int(club.call("checkout", vec![Value::Int(1), Value::Int(1), day(20010)]));
    assert_eq!(loan, 1, "the first loan number is 1");

    assert_eq!(
        club.call("assetOnLoanTo", vec![Value::Int(1)]),
        some_int(1),
        "the asset is marked on loan to member 1",
    );
    assert_eq!(
        club.call("loanNoFor", vec![Value::Int(1), Value::Int(1)]),
        some_int(1),
        "the member's active-loan tree carries the loan number",
    );
    assert_eq!(
        club.call("memberHistory", vec![Value::Int(1)]),
        Some(text("checkout;")),
        "the history event is appended",
    );
}

/// Return is the inverse transaction: the asset's on-loan state clears, the active-loan
/// entry is removed, and a second history event is appended — all together.
#[test]
fn j3_return_reverses_the_checkout_in_one_transaction() {
    let mut club = provisioned();

    expect_ok_int(club.call("checkout", vec![Value::Int(1), Value::Int(1), day(20010)]));
    let returned = expect_ok_int(club.call(
        "returnAsset",
        vec![Value::Int(1), Value::Int(1), day(20011)],
    ));
    assert_eq!(returned, 1, "return reports the loan number it closed");

    assert_eq!(club.call("assetOnLoanTo", vec![Value::Int(1)]), absent());
    assert_eq!(
        club.call("loanNoFor", vec![Value::Int(1), Value::Int(1)]),
        absent()
    );
    assert_eq!(
        club.call("memberHistory", vec![Value::Int(1)]),
        Some(text("checkout;return;")),
        "both events are in the member's log",
    );
}

/// Checkout refuses a suspended member and a double loan, and a refused checkout writes
/// nothing: the asset stays on loan to its first borrower.
#[test]
fn checkout_refuses_suspended_member_and_double_loan_with_no_partial_write() {
    let mut club = provisioned();

    // A suspended member cannot check out.
    club.call("suspendMember", vec![Value::Int(1)]);
    assert_eq!(
        club.call("memberIsActive", vec![Value::Int(1)]),
        Some(Value::Bool(false))
    );
    expect_err(club.call("checkout", vec![Value::Int(1), Value::Int(1), day(20010)]));
    assert_eq!(
        club.call("assetOnLoanTo", vec![Value::Int(1)]),
        absent(),
        "the refused checkout staged nothing",
    );

    // Reinstated, the member may check out; a second member's checkout of the same
    // asset is refused and leaves the first loan intact.
    club.call("reinstateMember", vec![Value::Int(1)]);
    expect_ok_int(club.call("checkout", vec![Value::Int(1), Value::Int(1), day(20010)]));
    expect_err(club.call("checkout", vec![Value::Int(2), Value::Int(1), day(20012)]));
    assert_eq!(
        club.call("assetOnLoanTo", vec![Value::Int(1)]),
        some_int(1),
        "the failed second checkout did not steal the asset",
    );
}

// ---------------------------------------------------------------------------
// J5 — bounded traversal with overflow, and the non-unique index scan
// ---------------------------------------------------------------------------

/// A member's history is read through a bounded traversal capped below the number of
/// events, so the overflow (`on more`) arm runs and the report ends with the overflow
/// marker. No cursor or page token crosses the export boundary.
#[test]
fn j5a_history_traversal_runs_the_overflow_arm() {
    let mut club = provisioned();

    // Two checkout/return cycles append four history events, above the report's bound
    // of three.
    expect_ok_int(club.call("checkout", vec![Value::Int(1), Value::Int(1), day(20010)]));
    expect_ok_int(club.call(
        "returnAsset",
        vec![Value::Int(1), Value::Int(1), day(20011)],
    ));
    expect_ok_int(club.call("checkout", vec![Value::Int(1), Value::Int(1), day(20012)]));
    expect_ok_int(club.call(
        "returnAsset",
        vec![Value::Int(1), Value::Int(1), day(20013)],
    ));

    assert_eq!(
        club.call("memberHistory", vec![Value::Int(1)]),
        Some(text("checkout;return;checkout;+more")),
        "the bounded report shows three events then the overflow marker",
    );
}

/// The non-unique `byCategory` index is read as a bounded scan with the category as a
/// bracket prefix, yielding each matching asset's identity in key order.
#[test]
fn j5b_category_scan_reads_the_nonunique_index_in_order() {
    let mut club = Project::from_fixture("club_locker").session();

    expect_ok_int(club.call(
        "registerAsset",
        vec![text("R-1"), text("racquets"), text("A")],
    ));
    expect_ok_int(club.call("registerAsset", vec![text("B-1"), text("balls"), text("B")]));
    expect_ok_int(club.call(
        "registerAsset",
        vec![text("R-2"), text("racquets"), text("C")],
    ));

    assert_eq!(
        club.call("assetsByCategory", vec![text("racquets")]),
        Some(text("R-1;R-2;")),
        "only the two racquets, in ascending identity order",
    );
    assert_eq!(
        club.call("assetsByCategory", vec![text("balls")]),
        Some(text("B-1;")),
    );
    assert_eq!(
        club.call("assetsByCategory", vec![text("empty")]),
        Some(text("")),
        "an empty category scans to nothing",
    );
}

// ---------------------------------------------------------------------------
// J6 — exact erase versus bounded subtree removal
// ---------------------------------------------------------------------------

/// Erasing an asset's payload is exact: the entry's own fields go, but its keyed
/// `serviceLog` descendants are preserved and remain reachable at their addresses.
#[test]
fn j6a_payload_erase_preserves_keyed_descendants() {
    let mut club = Project::from_fixture("club_locker").session();

    expect_ok_int(club.call(
        "registerAsset",
        vec![text("SVC"), text("racquets"), text("Serviced")],
    ));
    club.call(
        "addServiceLog",
        vec![Value::Int(1), Value::Int(1), text("restrung"), day(20005)],
    );
    club.call(
        "addServiceLog",
        vec![Value::Int(1), Value::Int(2), text("regripped"), day(20006)],
    );

    club.call("eraseAssetPayload", vec![Value::Int(1)]);
    assert_eq!(
        club.call("assetExists", vec![Value::Int(1)]),
        Some(Value::Bool(false)),
        "the asset payload is gone",
    );
    assert_eq!(
        club.call("serviceLogCount", vec![Value::Int(1)]),
        Some(Value::Int(2)),
        "the service-log descendants survive the payload erase",
    );
}

/// Retiring a member is a bounded, resumable job: each call removes at most a bounded
/// batch of active loans and history events over its own transaction and reports
/// whether more remain. The job resumes from committed store state, so a later call
/// continues where the last committed batch left off and never double-removes.
#[test]
fn j6b_member_retirement_is_a_bounded_resumable_batch() {
    let mut club = provisioned();

    // Three checkouts leave the member with three active loans and three history events,
    // above the retirement bound of two, so the job needs more than one batch.
    expect_ok_int(club.call("checkout", vec![Value::Int(1), Value::Int(1), day(20010)]));
    expect_ok_int(club.call("checkout", vec![Value::Int(1), Value::Int(2), day(20011)]));
    expect_ok_int(club.call("checkout", vec![Value::Int(1), Value::Int(3), day(20012)]));

    // The first batch drains a bounded slice and reports "more remain".
    assert!(
        !expect_ok_bool(club.call("retireMemberBatch", vec![Value::Int(1)])),
        "the first bounded batch does not finish the job",
    );
    assert_eq!(
        club.call("memberExists", vec![Value::Int(1)]),
        Some(Value::Bool(true)),
        "the member payload survives while its subtree is still draining",
    );

    // A resumed batch finishes the job: the member payload and every descendant are gone.
    assert!(
        expect_ok_bool(club.call("retireMemberBatch", vec![Value::Int(1)])),
        "the resumed batch completes the job",
    );
    assert_eq!(
        club.call("memberExists", vec![Value::Int(1)]),
        Some(Value::Bool(false))
    );
    assert_eq!(
        club.call("loanNoFor", vec![Value::Int(1), Value::Int(1)]),
        absent()
    );
    assert_eq!(
        club.call("memberHistory", vec![Value::Int(1)]),
        Some(text(""))
    );
}

// ---------------------------------------------------------------------------
// U2 shape — sparse optional contact fields
// ---------------------------------------------------------------------------

/// A member's contact fields are sparse present-or-clear: a fresh member reads one
/// absent (not a default), a guarded set makes it present, and a clear returns it to
/// absent. (This is the two-state sparse field, distinct from an `Option<T>` field's
/// absent/none/some three-state.)
#[test]
fn sparse_contact_fields_are_present_or_clear() {
    let mut club = Project::from_fixture("club_locker").session();

    assert_eq!(
        club.call("registerMember", vec![text("No Contact"), day(20000)]),
        Some(Value::Int(1))
    );
    assert_eq!(
        club.call("memberEmail", vec![Value::Int(1)]),
        absent(),
        "an unset sparse field reads absent, not a default",
    );

    club.call("setEmail", vec![Value::Int(1), text("ada@club.test")]);
    assert_eq!(
        club.call("memberEmail", vec![Value::Int(1)]),
        some_text("ada@club.test")
    );

    club.call("clearEmail", vec![Value::Int(1)]);
    assert_eq!(
        club.call("memberEmail", vec![Value::Int(1)]),
        absent(),
        "a clear returns the sparse field to absent",
    );
}

// ---------------------------------------------------------------------------
// The A5B01 demand surface — frozen bytes
// ---------------------------------------------------------------------------

/// `marrow check --demand` describes every export's durable access demand in source
/// spelling and exits 0 on the clean flagship project. The bytes are frozen: the demand describes
/// which durable places each export reads and writes, and never grants that access.
/// A checkout's five-effect transaction shows as its full read/write demand union; a
/// read-only report shows reads only; an index read renders under `reads`.
#[test]
fn check_reports_the_flagship_demand_sentences() {
    let output = Project::from_fixture("club_locker").run_cli("club-check", &["check", "--demand"]);
    assert!(
        output.status.success(),
        "check must succeed on the clean flagship: {}",
        output.stderr_text(),
    );
    assert_eq!(output.stdout_text(), CLUB_LOCKER_DEMAND_REPORT);
}

/// The frozen per-export demand report, one line per export in `module.item` order.
const CLUB_LOCKER_DEMAND_REPORT: &str = "\
clublocker.addServiceLog reads ^assets.serviceLog; writes ^assets.serviceLog
clublocker.assetExists reads ^assets
clublocker.assetNameByTag reads ^assets.byTag and ^assets.name
clublocker.assetOnLoanTo reads ^assets.onLoanTo
clublocker.assetTag reads ^assets.tag
clublocker.assetsByCategory reads ^assets and ^assets.byCategory
clublocker.checkout reads ^assets, ^idseq.value, ^members, ^members.activeLoans, and ^members.history; writes ^assets.onLoanTo, ^idseq.value, ^members.activeLoans, ^members.history, and ^members.historyCount
clublocker.clearEmail writes ^members.email
clublocker.eraseAssetPayload writes ^assets
clublocker.loanNoFor reads ^members.activeLoans.loanNo
clublocker.memberEmail reads ^members.email
clublocker.memberExists reads ^members
clublocker.memberHistory reads ^members.history
clublocker.memberIsActive reads ^members
clublocker.memberName reads ^members.name
clublocker.registerAsset reads ^assets, ^assets.byTag, and ^idseq.value; writes ^assets and ^idseq.value
clublocker.registerMember reads ^idseq.value and ^members; writes ^idseq.value and ^members
clublocker.reinstateMember reads ^members; writes ^members.standing
clublocker.retireMemberBatch reads ^members.activeLoans and ^members.history; writes ^members, ^members.activeLoans, and ^members.history
clublocker.returnAsset reads ^members, ^members.activeLoans, and ^members.history; writes ^assets.onLoanTo, ^members.activeLoans, ^members.history, and ^members.historyCount
clublocker.serviceLogCount reads ^assets.serviceLog
clublocker.setEmail reads ^members; writes ^members.email
clublocker.suspendMember reads ^members; writes ^members.standing
clublocker.tagTaken reads ^assets.byTag
";

// ---------------------------------------------------------------------------
// The ratified E07 bound watches — counted at authoring
// ---------------------------------------------------------------------------

/// The counted watches from the ratified E07 bounds audit: the flagship's export count
/// (MAX_EXPORTS = 256), its operation-site count (MAX_SITES = 8192), and its encoded
/// image size (MAX_IMAGE_BYTES = 512 KiB). All three clear their ceiling with wide
/// headroom; this test freezes the counts so a regression that pushes any toward its
/// bound is conspicuous. The frozen image size is the byte-identity witness for the M2
/// export widen: MAX_EXPORTS is a decode-time guard, never a stored byte, so raising it
/// leaves this in-bounds image byte-for-byte unchanged.
#[test]
fn flagship_bound_watches_are_counted_and_within_ceiling() {
    let (image_bytes, image) = compile_flagship();

    assert_eq!(
        image.exports().len(),
        24,
        "export count is the MAX_EXPORTS watch",
    );
    assert_eq!(
        image.sites().len(),
        FLAGSHIP_SITES,
        "operation-site count is the MAX_SITES=8192 watch",
    );
    assert_eq!(
        image_bytes, FLAGSHIP_IMAGE_BYTES,
        "encoded image size is the MAX_IMAGE_BYTES=512KiB watch",
    );

    assert!(
        image.exports().len() <= marrow_image::bounds::MAX_EXPORTS,
        "MAX_EXPORTS headroom",
    );
    assert!(image.sites().len() <= 8192, "MAX_SITES headroom");
    assert!(image_bytes <= 512 * 1024, "MAX_IMAGE_BYTES headroom");
}

/// The frozen operation-site count. The flagship has 6 keyed placements (the 3 store
/// roots plus the 3 keyed branches) and 2 managed indexes, all sealed eagerly, plus one
/// field-leaf site per field the code actually addresses (field-leaf emission is lazy, so
/// declared-but-untouched fields mint no site). With BND02 C1 the count dropped from the
/// former 26 (one site per declared node) to 17: the eager placement/index sites plus the
/// referenced field leaves only.
const FLAGSHIP_SITES: usize = 17;

/// The frozen encoded image size in bytes (shrunk with lazy field-leaf sites, BND02 C1).
const FLAGSHIP_IMAGE_BYTES: usize = 12583;

/// Capture and compile the on-disk flagship fixture through the production path,
/// returning the encoded image byte length and the verified image.
fn compile_flagship() -> (usize, VerifiedImage) {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v01/club_locker");
    let manifest_text = std::fs::read_to_string(root.join("marrow.toml")).expect("read manifest");
    let manifest = Manifest::parse(&manifest_text).expect("parse manifest");
    let ids = std::fs::read(root.join("marrow.ids")).expect("read ledger");
    let source = std::fs::read(root.join("src/clublocker.mw")).expect("read source");
    let files = vec![CapturedFile::new("src/clublocker.mw".to_string(), source)];
    let project = capture(&manifest, files, Some(&ids), &CaptureLimits::DEFAULT).expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    let bytes = compiled.image.bytes.len();
    let image = verify(&compiled.image.bytes).expect("verify");
    (bytes, image)
}
