//! A single declaration with many members must check in time proportional to
//! its member count, not its square. Both the per-declaration fact-collection
//! pass and the runtime program's saved-place construction once resolved each
//! member with a linear scan over the whole member set, making one large
//! resource or enum quadratic in its member count. These tests pin the linear
//! contract by comparing a base size against a 4x size: a quadratic pass would
//! take ~16x longer, so a generous sub-quadratic ratio fails loudly if the scan
//! returns.

use std::fmt::Write as _;
use std::time::Instant;

use marrow_check::check_project;

use crate::support::{self, config, temp_project, write};

/// Members at 4x the count must not take more than this multiple of the base
/// time. Linear work is ~4x; the historical quadratic was ~16x. The bound sits
/// well below quadratic yet far enough above linear to absorb timer noise on a
/// loaded machine.
const SUBQUADRATIC_RATIO: f64 = 8.0;

const BASE_MEMBERS: usize = 8_000;
const SCALED_MEMBERS: usize = BASE_MEMBERS * 4;

fn check_elapsed_secs(name: &str, source: String) -> f64 {
    let root = temp_project(name, |root| write(root, "src/m.mw", &source));
    let start = Instant::now();
    let (report, program) = check_project(&root, &config()).expect("check");
    support::assert_clean(&report);
    // The runtime program is where per-member saved-place construction lived, the
    // path that resolved each member's id and catalog id with a linear scan; build
    // it inside the timed window so a quadratic resolution reappears in the ratio.
    let _runtime = program.runtime();
    start.elapsed().as_secs_f64()
}

fn resource_source(member_count: usize) -> String {
    // A saved store drives the checked-saved-member construction the quadratic
    // path lived in: building one `CheckedSavedMember` per field resolved its id
    // and catalog id with a per-member linear scan over all members, so this
    // store-backed resource is the shape the linear contract must hold for.
    let mut source = String::from("module m\nresource Wide\n");
    for index in 0..member_count {
        let _ = writeln!(source, "    f{index}: int");
    }
    source.push_str("store ^wides(id: int): Wide\n");
    source
}

fn enum_source(member_count: usize) -> String {
    let mut source = String::from("module m\nenum Wide\n");
    for index in 0..member_count {
        let _ = writeln!(source, "    m{index}");
    }
    source
}

fn assert_subquadratic(base_secs: f64, scaled_secs: f64) {
    // A near-zero base reading would make the ratio meaningless; the scaled run
    // is the real guard, so floor the divisor.
    let base = base_secs.max(1e-3);
    let ratio = scaled_secs / base;
    assert!(
        ratio < SUBQUADRATIC_RATIO,
        "4x members took {ratio:.1}x the time (base {base_secs:.3}s, scaled {scaled_secs:.3}s); \
         expected sub-quadratic (< {SUBQUADRATIC_RATIO}x)",
    );
}

#[test]
fn one_resource_with_many_fields_checks_linearly() {
    let base = check_elapsed_secs("scaling-resource-base", resource_source(BASE_MEMBERS));
    let scaled = check_elapsed_secs("scaling-resource-scaled", resource_source(SCALED_MEMBERS));
    assert_subquadratic(base, scaled);
}

#[test]
fn one_enum_with_many_members_checks_linearly() {
    let base = check_elapsed_secs("scaling-enum-base", enum_source(BASE_MEMBERS));
    let scaled = check_elapsed_secs("scaling-enum-scaled", enum_source(SCALED_MEMBERS));
    assert_subquadratic(base, scaled);
}
