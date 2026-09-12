//! Check-time transaction-ownership diagnostics (TX02).
//!
//! The ownership contract has four laws the independent verifier reconstructs from the
//! program image (`image.flow`); this suite pins the source-facing `check.*` diagnostic
//! the checker now reports for each, at the offending construct's span, before an image
//! is minted. The verifier stays the boundary — a tampered image is still refused at
//! `image.flow` (see `marrow-verify` hostiles); these are earlier, friendlier reports.
//!
//! The laws:
//! - the owner lattice — a mutating export owns one region, begun at most once and
//!   committed on every normal exit after begin, with no empty region and no durable
//!   operation after commit (`check.transaction_empty`, `check.transaction_reopened`,
//!   `check.transaction_uncommitted`, `check.durable_after_commit`);
//! - a transaction owner is not called (`check.transaction_owner_called`);
//! - a `transaction` marker sits only in the owning export (`check.transaction_misplaced`);
//! - explicit and propagated returns commit only their own active region.

use marrow_compile::{CompileFailure, SourceDiagnostic, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

/// The committed identity ledger for the `Counter` schema every fixture is written
/// against, so a store declaration is identity-complete and only the transaction law
/// under test can fail the compile.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

const SCHEMA: &str = "resource Counter {\n    required value: int\n    label: string\n}\n\nstore ^counters[id: int]: Counter\n\n";

/// Capture and compile `SCHEMA` + `ops`, returning the check-time diagnostics (an empty
/// vector when it compiles clean).
fn diagnostics(ops: &str) -> Vec<SourceDiagnostic> {
    let source = format!("{SCHEMA}{ops}");
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.into_bytes(),
    )];
    let project: ProjectInput = marrow_project::capture(
        &manifest,
        files,
        Some(IDS.as_bytes()),
        &CaptureLimits::DEFAULT,
    )
    .expect("capture");
    match compile(&project) {
        Ok(_) => Vec::new(),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_iter().collect(),
        Err(other) => panic!("source-triggered failure must remain diagnostics, got {other:?}"),
    }
}

/// The single diagnostic a fixture produces, failing if it compiled clean or produced
/// more than one (each fixture isolates exactly one ownership law).
fn only(ops: &str) -> SourceDiagnostic {
    let mut diagnostics = diagnostics(ops);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got {diagnostics:#?}",
    );
    diagnostics.pop().expect("one diagnostic")
}

/// The 1-based source line of `needle` in `SCHEMA` + `ops` (the whole compiled source),
/// so a span assertion names the construct rather than a magic number.
fn line_of(ops: &str, needle: &str) -> u32 {
    let source = format!("{SCHEMA}{ops}");
    let index = source
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` present"));
    (source[..index].bytes().filter(|&b| b == b'\n').count() as u32) + 1
}

#[test]
fn borrowed_instruction_bodies_keep_complete_transaction_coordinates() {
    let prelude = "fn padding(v: int): int {\n    var n = v\n    n = n + 1\n    n = n + 2\n    return n\n}\nfn identity<T>(v: T): T { return v }\n";
    let cases = [
        (
            "check.transaction_empty",
            "pub fn empty() {\n    const n = identity(7)\n    transaction {}\n}\n",
            "{}",
        ),
        (
            "check.transaction_owner_called",
            "pub fn owner(id: int, v: int) {\n    transaction { ^counters[id] = Counter(value: v) }\n}\nfn callOwner<T>(id: int, v: int, tag: T) {\n    owner(id, v)\n}\nfn driver(id: int, v: int) { callOwner(id, v, true) }\n",
            "owner(id, v)",
        ),
        (
            "check.durable_after_commit",
            "fn readTagged<T>(id: int, tag: T): int? { return ^counters[id].value }\npub fn owner(id: int): int? {\n    transaction { ^counters[id] = Counter(value: identity(7)) }\n    return readTagged(id, true)\n}\n",
            "readTagged(id, true)",
        ),
    ];
    for (code, body, needle) in cases {
        let ops = format!("{prelude}{body}");
        let diagnostic = only(&ops);
        let source = format!("{SCHEMA}{ops}");
        let start = source.find(needle).expect("offending construct exists");
        let line_start = source[..start].rfind('\n').map_or(0, |at| at + 1);
        assert_eq!(diagnostic.code(), code);
        assert_eq!(diagnostic.file().as_str(), "src/main.mw");
        assert_eq!(
            diagnostic.span(),
            marrow_syntax::SourceSpan {
                start_byte: start,
                end_byte: start + needle.len(),
                line: line_of(&ops, needle),
                column: (start - line_start + 1) as u32,
            },
            "{code} must keep its complete source coordinate"
        );
    }
    let early_return = "pub fn owner(id: int): int {\n    if identity(true) { return 0 }\n    transaction { ^counters[id] = Counter(value: 7) }\n    return 1\n}\n";
    assert!(diagnostics(&format!("{prelude}{early_return}")).is_empty());
}

// ---------------------------------------------------------------------------
// Law (a): the owner lattice.
// ---------------------------------------------------------------------------

/// An empty (no-op) `transaction` block commits nothing and opens no store session; it
/// is refused at the block, at the `transaction` keyword.
#[test]
fn an_empty_transaction_is_rejected_at_the_block() {
    let ops = "pub fn emptyRegion() {\n    transaction {\n    }\n}\n";
    let diagnostic = only(ops);
    assert_eq!(diagnostic.code(), "check.transaction_empty");
    assert_eq!(diagnostic.line(), line_of(ops, "transaction {"));
    assert!(
        diagnostic.message().contains("no durable operation"),
        "steers to the empty-region remedy: {}",
        diagnostic.message()
    );
}

/// A second `transaction` region in one mutating export reopens a region the export
/// already owns; refused at the reopening block.
#[test]
fn a_second_region_reopens_an_owned_transaction() {
    let ops = "pub fn twoRegions(id: int, v: int) {\n    transaction {\n        ^counters[id] = Counter(value: v)\n    }\n    transaction {\n        ^counters[id] = Counter(value: v + 1)\n    }\n}\n";
    let diagnostic = only(ops);
    assert_eq!(diagnostic.code(), "check.transaction_reopened");
    assert!(
        diagnostic.message().contains("exactly once") || diagnostic.message().contains("single"),
        "steers to the one-region remedy: {}",
        diagnostic.message()
    );
}

/// An early return before begin has no staged writes to commit.
#[test]
fn an_early_return_before_the_region_compiles() {
    let ops = "pub fn maybeSet(id: int, v: int, skip: bool) {\n    if skip {\n        return\n    }\n    transaction {\n        ^counters[id] = Counter(value: v)\n    }\n}\n";
    assert!(diagnostics(ops).is_empty());
}

/// A durable read after the region's commit cannot reach a live session; refused at the
/// read, which follows the closing brace.
#[test]
fn a_durable_read_after_commit_is_rejected() {
    let ops = "pub fn setAndGet(id: int, v: int): int? {\n    transaction {\n        ^counters[id] = Counter(value: v)\n    }\n    return ^counters[id].value\n}\n";
    let diagnostic = only(ops);
    assert_eq!(diagnostic.code(), "check.durable_after_commit");
    assert_eq!(
        diagnostic.line(),
        line_of(ops, "return ^counters[id].value")
    );
    assert!(
        diagnostic.message().contains("after the `transaction`")
            || diagnostic.message().contains("consumes"),
        "steers to moving the read inside the region: {}",
        diagnostic.message()
    );
}

// ---------------------------------------------------------------------------
// Law (b): a transaction owner is not called.
// ---------------------------------------------------------------------------

/// Calling an export that owns a `transaction` block is calling an invocation boundary;
/// refused at the call site.
#[test]
fn calling_a_transaction_owner_is_rejected() {
    let ops = "pub fn owner(id: int, v: int) {\n    transaction {\n        ^counters[id] = Counter(value: v)\n    }\n}\n\npub fn driver(id: int, v: int) {\n    owner(id, v)\n}\n";
    let diagnostic = only(ops);
    assert_eq!(diagnostic.code(), "check.transaction_owner_called");
    assert_eq!(diagnostic.line(), line_of(ops, "owner(id, v)\n}"));
    assert!(
        diagnostic.message().contains("`owner`")
            && diagnostic.message().contains("invocation boundary"),
        "names the owner and the boundary rule: {}",
        diagnostic.message()
    );
}

// ---------------------------------------------------------------------------
// Law (c): a `transaction` marker sits only in the owning export.
// ---------------------------------------------------------------------------

/// A non-`pub` helper that opens its own `transaction` block misplaces the marker: a
/// helper runs inside its caller's region. Refused at the block.
#[test]
fn a_helper_owning_a_region_is_rejected() {
    let ops = "fn helperOwns(id: int, v: int) {\n    transaction {\n        ^counters[id] = Counter(value: v)\n    }\n}\n";
    let diagnostic = only(ops);
    assert_eq!(diagnostic.code(), "check.transaction_misplaced");
    assert!(
        diagnostic
            .message()
            .contains("only in the export that owns it")
            || diagnostic.message().contains("owning export"),
        "steers to moving the block to the owner: {}",
        diagnostic.message()
    );
}

// ---------------------------------------------------------------------------
// Normal exits commit only the exiting function's active region.
// ---------------------------------------------------------------------------

/// A propagated error commits its owner's active region before returning.
#[test]
fn a_try_exiting_an_owned_region_compiles() {
    let ops = "fn check(v: int): Result<int, string> {\n    if v > 0 {\n        return ok(v)\n    }\n    return err(\"value must be positive\")\n}\n\npub fn setChecked(id: int, v: int): Result<int, string> {\n    transaction {\n        const w = try check(v)\n        ^counters[id] = Counter(value: w)\n    }\n    return ok(v)\n}\n";
    assert!(diagnostics(ops).is_empty());
}

// ---------------------------------------------------------------------------
// Require and try follow the same region-exit rule.
// ---------------------------------------------------------------------------

/// A require failure commits its owner's active region before returning.
#[test]
fn a_require_inside_an_owned_region_compiles() {
    let ops = "pub fn setChecked(id: int, v: int): Result<int, string> {\n    transaction {\n        require v > 0 else \"value must be positive\"\n        ^counters[id] = Counter(value: v)\n        return ok(v)\n    }\n}\n";
    assert!(diagnostics(ops).is_empty());
}

/// A require failure before begin returns without opening a region.
#[test]
fn a_require_before_an_owned_region_compiles() {
    let ops = "pub fn setChecked(id: int, v: int): Result<int, string> {\n    require v > 0 else \"value must be positive\"\n    transaction {\n        ^counters[id] = Counter(value: v)\n        return ok(v)\n    }\n}\n";
    assert!(diagnostics(ops).is_empty());
}

/// A `require` inside a region nested in another region: the reopened-region law
/// fires first (the earliest offending construct wins), and the composition is
/// still refused before any image is minted.
#[test]
fn a_require_inside_a_nested_region_is_still_refused() {
    let ops = "pub fn setChecked(id: int, v: int): Result<int, string> {\n    transaction {\n        transaction {\n            require v > 0 else \"value must be positive\"\n            ^counters[id] = Counter(value: v)\n            return ok(v)\n        }\n    }\n}\n";
    let diagnostic = only(ops);
    assert_eq!(diagnostic.code(), "check.transaction_reopened");
}

/// Either propagated exit commits; success continues to the explicit return.
#[test]
fn try_then_require_inside_a_region_compile() {
    let ops = "fn check(v: int): Result<int, string> {\n    if v > 0 {\n        return ok(v)\n    }\n    return err(\"value must be positive\")\n}\n\npub fn setChecked(id: int, v: int): Result<int, string> {\n    transaction {\n        const w = try check(v)\n        require w < 100 else \"value too large\"\n        ^counters[id] = Counter(value: w)\n        return ok(w)\n    }\n}\n";
    assert!(diagnostics(ops).is_empty());
}

// ---------------------------------------------------------------------------
// Accepted helper and owner forms retain their separate region ownership.
// ---------------------------------------------------------------------------

/// A `require` in a helper that joins its caller's region owns no region itself:
/// its failure exit is ordinary control flow into the export's committing
/// in-region `return`, exactly like a helper's `try`.
#[test]
fn a_require_in_a_helper_joining_the_region_compiles() {
    let ops = "fn validate(v: int): Result<int, string> {\n    require v > 0 else \"value must be positive\"\n    return ok(v)\n}\n\nfn apply(id: int, v: int): Result<int, string> {\n    const w = try validate(v)\n    ^counters[id] = Counter(value: w)\n    return ok(w)\n}\n\npub fn setChecked(id: int, v: int): Result<int, string> {\n    transaction {\n        return apply(id, v)\n    }\n}\n";
    assert!(
        diagnostics(ops).is_empty(),
        "a helper's require joins the caller's region: {:#?}",
        diagnostics(ops)
    );
}

/// A `require` in a helper reached through another helper inside the region:
/// region membership is transitive over calls, and neither helper owns the
/// region, so the guard stays legal at any depth.
#[test]
fn a_require_two_helpers_deep_inside_the_region_compiles() {
    let ops = "fn guard(v: int): Result<int, string> {\n    require v > 0 else \"value must be positive\"\n    return ok(v)\n}\n\nfn validate(v: int): Result<int, string> {\n    const w = try guard(v)\n    require w < 100 else \"value too large\"\n    return ok(w)\n}\n\nfn apply(id: int, v: int): Result<int, string> {\n    const w = try validate(v)\n    ^counters[id] = Counter(value: w)\n    return ok(w)\n}\n\npub fn setChecked(id: int, v: int): Result<int, string> {\n    transaction {\n        return apply(id, v)\n    }\n}\n";
    assert!(
        diagnostics(ops).is_empty(),
        "requires at any helper depth join the caller's region: {:#?}",
        diagnostics(ops)
    );
}

/// A `require` after the region's closing commit: the commit has already
/// happened on that path, so the implicit failure exit no longer bypasses it.
#[test]
fn a_require_after_the_regions_commit_compiles() {
    let ops = "pub fn setChecked(id: int, v: int): Result<int, string> {\n    transaction {\n        ^counters[id] = Counter(value: v)\n    }\n    require v > 0 else \"value must be positive\"\n    return ok(v)\n}\n";
    assert!(
        diagnostics(ops).is_empty(),
        "a require after the commit does not bypass it: {:#?}",
        diagnostics(ops)
    );
}

/// A read-only export that opens a `transaction` around only reads is admitted (the
/// region carries read demand); it is not an empty region.
#[test]
fn a_read_only_region_compiles() {
    let ops = "pub fn peek(id: int): int? {\n    var out: int? = absent\n    transaction {\n        out = ^counters[id].value\n    }\n    return out\n}\n";
    assert!(
        diagnostics(ops).is_empty(),
        "a read-only region is admitted"
    );
}

/// An in-region `return` is a commit site: it commits the staged writes, then returns
/// the value it captured inside the region. Both the guard-return and the fall-through
/// commit, so the region is well formed.
#[test]
fn an_in_region_guard_return_compiles() {
    let ops = "pub fn addOnce(id: int, v: int): bool {\n    transaction {\n        if exists(^counters[id]) {\n            return false\n        }\n        ^counters[id] = Counter(value: v)\n    }\n    return true\n}\n";
    assert!(
        diagnostics(ops).is_empty(),
        "an in-region guard-return commits on both exits"
    );
}

/// A mutating helper that runs inside its caller's region — no `transaction` block of
/// its own — is well formed; the owner wraps the call.
#[test]
fn a_mutating_helper_inside_the_owners_region_compiles() {
    let ops = "fn writeIt(id: int, v: int) {\n    ^counters[id] = Counter(value: v)\n}\n\npub fn wrap(id: int, v: int) {\n    transaction {\n        writeIt(id, v)\n    }\n}\n";
    assert!(
        diagnostics(ops).is_empty(),
        "a helper mutating inside the owner's region is admitted"
    );
}
