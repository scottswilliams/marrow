//! Check-time transaction-ownership diagnostics.
//!
//! Pins the source-facing `check.*` diagnostic reported at the offending construct's span,
//! before an image is minted. `image.flow` remains the trust boundary and refuses a
//! tampered image (see `marrow-verify` hostiles); these are earlier, friendlier reports.
//!
//! The ownership contract:
//! - a mutating export begins its region at most once on any path, with paths that meet
//!   agreeing on whether it has run, and commits it on every normal exit after begin,
//!   with no empty region and no durable operation after commit;
//! - a transaction owner is not called;
//! - a `transaction` marker sits only in the owning export;
//! - explicit and propagated returns commit only their own active region.

use marrow_codes::Code;
use marrow_compile::{CompileFailure, SourceDiagnostic, compile, compile_with_tests};
use marrow_syntax::SourceSpan;

use super::project;

/// Committed identity ledger for the `Counter` schema, so every fixture is
/// identity-complete and only the transaction law under test can fail the compile.
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

/// Check-time diagnostics for `SCHEMA` + `ops`; empty when it compiles clean.
fn diagnostics(ops: &str) -> Vec<SourceDiagnostic> {
    let source = format!("{SCHEMA}{ops}");
    match compile(&project(&source, Some(IDS.as_bytes()))) {
        Ok(_) => Vec::new(),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_iter().collect(),
        Err(other) => panic!("source-triggered failure must remain diagnostics, got {other:?}"),
    }
}

/// The one diagnostic a fixture produces; each fixture isolates exactly one ownership law.
fn only(ops: &str) -> SourceDiagnostic {
    let mut diagnostics = diagnostics(ops);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected exactly one diagnostic, got {diagnostics:#?}",
    );
    diagnostics.pop().expect("one diagnostic")
}

/// The 1-based source line of `needle`, so a span assertion names the construct rather
/// than a magic number.
fn line_of(ops: &str, needle: &str) -> u32 {
    let source = format!("{SCHEMA}{ops}");
    let index = source
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` present"));
    (source[..index].bytes().filter(|&b| b == b'\n').count() as u32) + 1
}

/// The complete source span of `needle` in `SCHEMA` + `ops`; `needle` names exactly one
/// construct.
fn span_of(ops: &str, needle: &str) -> SourceSpan {
    let source = format!("{SCHEMA}{ops}");
    let start = source
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` present"));
    assert_eq!(
        source.matches(needle).count(),
        1,
        "`{needle}` names one construct"
    );
    let line_start = source[..start].rfind('\n').map_or(0, |at| at + 1);
    SourceSpan {
        start_byte: start,
        end_byte: start + needle.len(),
        line: (source[..start].bytes().filter(|&b| b == b'\n').count() as u32) + 1,
        column: (start - line_start + 1) as u32,
    }
}

#[test]
fn borrowed_instruction_bodies_keep_complete_transaction_coordinates() {
    let prelude = "fn padding(v: int): int {\n    var n = v\n    n = n + 1\n    n = n + 2\n    return n\n}\nfn identity<T>(v: T): T { return v }\n";
    let cases = [
        (
            Code::CheckTransactionEmpty,
            "pub fn empty() {\n    const n = identity(7)\n    transaction {}\n}\n",
            "{}",
        ),
        (
            Code::CheckTransactionOwnerCalled,
            "pub fn owner(id: int, v: int) {\n    transaction { ^counters[id] = Counter(value: v) }\n}\nfn callOwner<T>(id: int, v: int, tag: T) {\n    owner(id, v)\n}\nfn driver(id: int, v: int) { callOwner(id, v, true) }\n",
            "owner(id, v)",
        ),
        (
            Code::CheckDurableAfterCommit,
            "fn readTagged<T>(id: int, tag: T): int? { return ^counters[id].value }\npub fn owner(id: int): int? {\n    transaction { ^counters[id] = Counter(value: identity(7)) }\n    return readTagged(id, true)\n}\n",
            "readTagged(id, true)",
        ),
    ];
    for (code, body, needle) in cases {
        let ops = format!("{prelude}{body}");
        let diagnostic = only(&ops);
        assert_eq!(diagnostic.code(), code);
        assert_eq!(diagnostic.file().as_str(), "src/main.mw");
        assert_eq!(
            diagnostic.span(),
            span_of(&ops, needle),
            "{code:?} must keep its complete source coordinate"
        );
    }
    let early_return = "pub fn owner(id: int): int {\n    if identity(true) { return 0 }\n    transaction { ^counters[id] = Counter(value: 7) }\n    return 1\n}\n";
    assert!(diagnostics(&format!("{prelude}{early_return}")).is_empty());
}

/// The role-by-region matrix: a `test` body owning a region and a generic instance
/// owning one are each a misplaced marker, and a `test` calling a generic helper that
/// requires an ambient transaction is refused at the call.
#[test]
fn test_and_instance_bodies_answer_to_the_ownership_laws() {
    let cases = [
        (
            Code::CheckTransactionMisplaced,
            "fn bump(id: int) {\n    ^counters[id] = Counter(value: 1)\n}\ntest \"owns\" {\n    transaction {\n        bump(1)\n    }\n}\n",
            "transaction {",
        ),
        (
            Code::CheckTransactionMisplaced,
            "fn tagged<T>(id: int, tag: T) {\n    transaction {\n        ^counters[id] = Counter(value: 1)\n    }\n}\ntest \"drives\" {\n    tagged(1, true)\n}\n",
            "transaction {",
        ),
        (
            Code::CheckRequiresTransaction,
            "fn bumpTagged<T>(id: int, tag: T) {\n    ^counters[id] = Counter(value: 1)\n}\ntest \"calls\" {\n    bumpTagged(1, true)\n}\n",
            "bumpTagged(1, true)",
        ),
    ];
    for (code, ops, needle) in cases {
        let source = format!("{SCHEMA}{ops}");
        let mut diagnostics = match compile_with_tests(&project(&source, Some(IDS.as_bytes()))) {
            Ok(_) => Vec::new(),
            Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_vec(),
            Err(other) => panic!("source-triggered failure must remain diagnostics, got {other:?}"),
        };
        assert_eq!(diagnostics.len(), 1, "{code:?}: {diagnostics:#?}");
        let diagnostic = diagnostics.pop().expect("one diagnostic");
        assert_eq!(diagnostic.code(), code, "{diagnostic:#?}");
        assert_eq!(diagnostic.line(), line_of(ops, needle), "{diagnostic:#?}");
    }
}

/// An empty region commits nothing and opens no store session.
#[test]
fn an_empty_transaction_is_rejected_at_the_block() {
    let ops = "pub fn emptyRegion() {\n    transaction {\n    }\n}\n";
    let diagnostic = only(ops);
    assert_eq!(diagnostic.code(), Code::CheckTransactionEmpty);
    assert_eq!(diagnostic.line(), line_of(ops, "transaction {"));
    assert!(
        diagnostic.message().contains("no durable operation"),
        "steers to the empty-region remedy: {}",
        diagnostic.message()
    );
}

/// An early return before begin has no staged writes to commit.
#[test]
fn an_early_return_before_the_region_compiles() {
    let ops = "pub fn maybeSet(id: int, v: int, skip: bool) {\n    if skip {\n        return\n    }\n    transaction {\n        ^counters[id] = Counter(value: v)\n    }\n}\n";
    assert!(diagnostics(ops).is_empty());
}

/// A durable read after the region's commit cannot reach a live session.
#[test]
fn a_durable_read_after_commit_is_rejected() {
    let ops = "pub fn setAndGet(id: int, v: int): int? {\n    transaction {\n        ^counters[id] = Counter(value: v)\n    }\n    return ^counters[id].value\n}\n";
    let diagnostic = only(ops);
    assert_eq!(diagnostic.code(), Code::CheckDurableAfterCommit);
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

/// An export that owns a region is an invocation boundary, so it cannot be called.
#[test]
fn calling_a_transaction_owner_is_rejected() {
    let ops = "pub fn owner(id: int, v: int) {\n    transaction {\n        ^counters[id] = Counter(value: v)\n    }\n}\n\npub fn driver(id: int, v: int) {\n    owner(id, v)\n}\n";
    let diagnostic = only(ops);
    assert_eq!(diagnostic.code(), Code::CheckTransactionOwnerCalled);
    assert_eq!(diagnostic.line(), line_of(ops, "owner(id, v)\n}"));
    assert!(
        diagnostic.message().contains("`owner`")
            && diagnostic.message().contains("invocation boundary"),
        "names the owner and the boundary rule: {}",
        diagnostic.message()
    );
}

/// A helper runs inside its caller's region, so owning one of its own misplaces the marker.
#[test]
fn a_helper_owning_a_region_is_rejected() {
    let ops = "fn helperOwns(id: int, v: int) {\n    transaction {\n        ^counters[id] = Counter(value: v)\n    }\n}\n";
    let diagnostic = only(ops);
    assert_eq!(diagnostic.code(), Code::CheckTransactionMisplaced);
    assert!(
        diagnostic
            .message()
            .contains("only in the export that owns it")
            || diagnostic.message().contains("owning export"),
        "steers to moving the block to the owner: {}",
        diagnostic.message()
    );
}

/// A propagated error commits its owner's active region before returning.
#[test]
fn a_try_exiting_an_owned_region_compiles() {
    let ops = "fn check(v: int): Result<int, string> {\n    if v > 0 {\n        return ok(v)\n    }\n    return err(\"value must be positive\")\n}\n\npub fn setChecked(id: int, v: int): Result<int, string> {\n    transaction {\n        const w = try check(v)\n        ^counters[id] = Counter(value: w)\n    }\n    return ok(v)\n}\n";
    assert!(diagnostics(ops).is_empty());
}

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

/// Either propagated exit commits; success continues to the explicit return.
#[test]
fn try_then_require_inside_a_region_compile() {
    let ops = "fn check(v: int): Result<int, string> {\n    if v > 0 {\n        return ok(v)\n    }\n    return err(\"value must be positive\")\n}\n\npub fn setChecked(id: int, v: int): Result<int, string> {\n    transaction {\n        const w = try check(v)\n        require w < 100 else \"value too large\"\n        ^counters[id] = Counter(value: w)\n        return ok(w)\n    }\n}\n";
    assert!(diagnostics(ops).is_empty());
}

/// A helper owns no region, so its `require` failure exit is ordinary control flow into the
/// export's committing in-region `return`, exactly like a helper's `try`.
#[test]
fn a_require_in_a_helper_joining_the_region_compiles() {
    let ops = "fn validate(v: int): Result<int, string> {\n    require v > 0 else \"value must be positive\"\n    return ok(v)\n}\n\nfn apply(id: int, v: int): Result<int, string> {\n    const w = try validate(v)\n    ^counters[id] = Counter(value: w)\n    return ok(w)\n}\n\npub fn setChecked(id: int, v: int): Result<int, string> {\n    transaction {\n        return apply(id, v)\n    }\n}\n";
    assert!(
        diagnostics(ops).is_empty(),
        "a helper's require joins the caller's region: {:#?}",
        diagnostics(ops)
    );
}

/// Region membership is transitive over calls, so a guard stays legal at any helper depth.
#[test]
fn a_require_two_helpers_deep_inside_the_region_compiles() {
    let ops = "fn guard(v: int): Result<int, string> {\n    require v > 0 else \"value must be positive\"\n    return ok(v)\n}\n\nfn validate(v: int): Result<int, string> {\n    const w = try guard(v)\n    require w < 100 else \"value too large\"\n    return ok(w)\n}\n\nfn apply(id: int, v: int): Result<int, string> {\n    const w = try validate(v)\n    ^counters[id] = Counter(value: w)\n    return ok(w)\n}\n\npub fn setChecked(id: int, v: int): Result<int, string> {\n    transaction {\n        return apply(id, v)\n    }\n}\n";
    assert!(
        diagnostics(ops).is_empty(),
        "requires at any helper depth join the caller's region: {:#?}",
        diagnostics(ops)
    );
}

/// The commit has already happened on that path, so the implicit failure exit cannot bypass it.
#[test]
fn a_require_after_the_regions_commit_compiles() {
    let ops = "pub fn setChecked(id: int, v: int): Result<int, string> {\n    transaction {\n        ^counters[id] = Counter(value: v)\n    }\n    require v > 0 else \"value must be positive\"\n    return ok(v)\n}\n";
    assert!(
        diagnostics(ops).is_empty(),
        "a require after the commit does not bypass it: {:#?}",
        diagnostics(ops)
    );
}

/// A region around only reads carries read demand, so it is not an empty region.
#[test]
fn a_read_only_region_compiles() {
    let ops = "pub fn peek(id: int): int? {\n    var out: int? = absent\n    transaction {\n        out = ^counters[id].value\n    }\n    return out\n}\n";
    assert!(
        diagnostics(ops).is_empty(),
        "a read-only region is admitted"
    );
}

/// An in-region `return` is a commit site, so both the guard-return and the fall-through
/// commit.
#[test]
fn an_in_region_guard_return_compiles() {
    let ops = "pub fn addOnce(id: int, v: int): bool {\n    transaction {\n        if exists(^counters[id]) {\n            return false\n        }\n        ^counters[id] = Counter(value: v)\n    }\n    return true\n}\n";
    assert!(
        diagnostics(ops).is_empty(),
        "an in-region guard-return commits on both exits"
    );
}

/// A mutating helper needs no region of its own; the owner wraps the call.
#[test]
fn a_mutating_helper_inside_the_owners_region_compiles() {
    let ops = "fn writeIt(id: int, v: int) {\n    ^counters[id] = Counter(value: v)\n}\n\npub fn wrap(id: int, v: int) {\n    transaction {\n        writeIt(id, v)\n    }\n}\n";
    assert!(
        diagnostics(ops).is_empty(),
        "a helper mutating inside the owner's region is admitted"
    );
}

/// One region-shape row: a label, the export source after `SCHEMA`, and the ordered
/// diagnostics it receives, each named by the exact construct it is reported at.
type RegionRow = (&'static str, String, Vec<(Code, &'static str)>);

/// The rows of [`transaction_region_family_is_refused_at_its_construct`].
fn transaction_region_rows() -> Vec<RegionRow> {
    use Code::{
        CheckTransactionConditional as Conditional, CheckTransactionMisplaced as Misplaced,
        CheckTransactionOwnerCalled as OwnerCalled, CheckTransactionReopened as Reopened,
        CheckTransactionUncommitted as Uncommitted,
    };
    const W1: &str = "{ ^counters[id] = Counter(value: 1) }";
    const W2: &str = "{ ^counters[id] = Counter(value: 2) }";
    vec![
        (
            "one-armed if",
            format!("pub fn f(id: int, go: bool) {{\n    if go {{\n        transaction {W1}\n    }}\n}}\n"),
            vec![(Conditional, W1)],
        ),
        (
            "two sequential blocks",
            format!("pub fn f(id: int) {{\n    transaction {W1}\n    transaction {W2}\n}}\n"),
            vec![(Reopened, W2)],
        ),
        (
            "else only",
            format!("pub fn f(id: int, go: bool) {{\n    if go {{\n    }} else {{\n        transaction {W1}\n    }}\n}}\n"),
            vec![(Conditional, W1)],
        ),
        (
            "nested if",
            format!("pub fn f(id: int, a: bool, b: bool) {{\n    if a {{\n        if b {{\n            transaction {W1}\n        }}\n    }}\n}}\n"),
            vec![(Conditional, W1)],
        ),
        (
            "if const",
            "pub fn f(id: int) {\n    if const c = ^counters[id] {\n        transaction { ^counters[id] = Counter(value: c.value + 1) }\n    }\n}\n".to_string(),
            vec![(Conditional, "{ ^counters[id] = Counter(value: c.value + 1) }")],
        ),
        (
            "match arm",
            format!("enum Mode {{\n    write\n    skip\n}}\n\npub fn f(id: int, m: Mode) {{\n    match m {{\n        write => {{\n            transaction {W1}\n        }}\n        skip => {{}}\n    }}\n}}\n"),
            vec![(Conditional, W1)],
        ),
        (
            "read-only region in a one-armed if",
            "pub fn f(id: int, go: bool): int {\n    var out = 0\n    if go {\n        transaction { out = ^counters[id].value ?? 0 }\n    }\n    return out\n}\n".to_string(),
            vec![(Conditional, "{ out = ^counters[id].value ?? 0 }")],
        ),
        (
            "break after the commit",
            format!("pub fn f(id: int, go: bool) {{\n    var i = 0\n    while go and i < 3 {{\n        transaction {W1}\n        break\n    }}\n}}\n"),
            vec![(Conditional, W1)],
        ),
        (
            "conditional block then an unconditional block",
            format!("pub fn f(id: int, go: bool) {{\n    if go {{\n        transaction {W1}\n    }}\n    transaction {W2}\n}}\n"),
            vec![(Conditional, W1)],
        ),
        // The committed arm reaches the second block before the other arm does, so the
        // walk sees a reopen and a conflict at one begin; the conflict names the block.
        (
            "block in one arm of an if-else then an unconditional block",
            format!("pub fn f(id: int, go: bool) {{\n    var n = 0\n    if go {{\n        transaction {W1}\n    }} else {{\n        n += 1\n    }}\n    transaction {W2}\n}}\n"),
            vec![(Conditional, W1)],
        ),
        (
            "block in an on-more arm",
            format!("pub fn f(id: int) {{\n    for k in ^counters at most 2 {{\n    }} on more {{\n        transaction {W1}\n    }}\n}}\n"),
            vec![(Conditional, W1)],
        ),
        (
            "while body",
            format!("pub fn f(id: int) {{\n    var i = 0\n    while i < 3 {{\n        transaction {W1}\n        i += 1\n    }}\n}}\n"),
            vec![(Reopened, W1)],
        ),
        (
            "bounded for body",
            "pub fn f() {\n    for k in ^counters at most 10 {\n        transaction { ^counters[k] = Counter(value: 0) }\n    } on more {\n    }\n}\n".to_string(),
            vec![(Reopened, "{ ^counters[k] = Counter(value: 0) }")],
        ),
        (
            "one-armed if inside a loop",
            format!("pub fn f(id: int, go: bool) {{\n    var i = 0\n    while i < 3 {{\n        i += 1\n        if go {{\n            transaction {W1}\n        }}\n    }}\n}}\n"),
            vec![(Reopened, W1)],
        ),
        (
            "break out of the block",
            "pub fn f(id: int) {\n    var i = 0\n    while i < 3 {\n        transaction {\n            ^counters[i] = Counter(value: i)\n            break\n        }\n    }\n}\n".to_string(),
            vec![(Uncommitted, "break")],
        ),
        (
            "continue out of the block",
            "pub fn f(id: int) {\n    var i = 0\n    while i < 3 {\n        i += 1\n        transaction {\n            ^counters[i] = Counter(value: i)\n            continue\n        }\n    }\n}\n".to_string(),
            vec![(Uncommitted, "continue")],
        ),
        (
            "break out of the block in a range for",
            "pub fn f(id: int) {\n    for i in 0..3 {\n        transaction {\n            ^counters[i] = Counter(value: i)\n            break\n        }\n    }\n}\n".to_string(),
            vec![(Uncommitted, "break")],
        ),
        (
            "continue out of the block in a list for",
            "pub fn f(xs: List<int>) {\n    for x in xs {\n        transaction {\n            ^counters[x] = Counter(value: x)\n            continue\n        }\n    }\n}\n".to_string(),
            vec![(Uncommitted, "continue")],
        ),
        (
            "conditional block nested in the region",
            format!("pub fn f(id: int, go: bool) {{\n    transaction {{\n        if go {{\n            transaction {W1}\n        }}\n        ^counters[id + 1] = Counter(value: 8)\n    }}\n}}\n"),
            vec![(Reopened, W1)],
        ),
        (
            "block with a require nested in the region",
            "pub fn f(id: int, v: int): Result<int, string> {\n    transaction {\n        transaction {\n            require v > 0 else \"value must be positive\"\n            ^counters[id] = Counter(value: v)\n            return ok(v)\n        }\n    }\n}\n".to_string(),
            vec![(Reopened, "{\n            require v > 0 else \"value must be positive\"\n            ^counters[id] = Counter(value: v)\n            return ok(v)\n        }")],
        ),
        (
            "block nested in the region",
            format!("pub fn f(id: int) {{\n    transaction {{\n        transaction {W1}\n    }}\n}}\n"),
            vec![(Reopened, W1)],
        ),
        (
            "nested block then a write",
            format!("pub fn f(id: int) {{\n    transaction {{\n        transaction {W1}\n        ^counters[id + 1] = Counter(value: 8)\n    }}\n}}\n"),
            vec![(Reopened, W1)],
        ),
        (
            "block in a loop nested in the region",
            format!("pub fn f(id: int) {{\n    transaction {{\n        var i = 0\n        while i < 3 {{\n            transaction {W1}\n            i += 1\n        }}\n    }}\n}}\n"),
            vec![(Reopened, W1)],
        ),
        (
            "helper owning a block reached through another helper",
            format!("fn inner(id: int) {{\n    transaction {W1}\n}}\nfn middle(id: int) {{\n    inner(id)\n}}\npub fn outer(id: int) {{\n    middle(id)\n}}\n"),
            vec![(Misplaced, W1), (OwnerCalled, "inner(id)")],
        ),
        (
            "helper whose block breaks out of its loop",
            "fn h(id: int) {\n    var i = 0\n    while i < 3 {\n        transaction {\n            ^counters[i] = Counter(value: i)\n            break\n        }\n    }\n}\npub fn f(id: int) {\n    transaction {\n        h(id)\n    }\n}\n".to_string(),
            vec![(Misplaced, "{\n            ^counters[i] = Counter(value: i)\n            break\n        }"), (OwnerCalled, "h(id)")],
        ),
        (
            "helper whose block continues its loop",
            "fn h(id: int) {\n    var i = 0\n    while i < 3 {\n        i += 1\n        transaction {\n            ^counters[i] = Counter(value: i)\n            continue\n        }\n    }\n}\npub fn f(id: int) {\n    h(id)\n}\n".to_string(),
            vec![(Misplaced, "{\n            ^counters[i] = Counter(value: i)\n            continue\n        }"), (OwnerCalled, "h(id)")],
        ),
    ]
}

/// A matrix of control-flow shapes that place a `transaction` block where paths disagree
/// on whether it has run, begin it twice, or leave it open, each paired with the exact
/// ordered diagnostics it receives at their complete source spans. A block in a
/// one-armed `if` is reported at the block rather than emitted as an image the verifier
/// refuses at `image.flow`. `diagnostics` panics on a compiler invariant, so every row
/// also pins that no such source reaches one.
#[test]
fn transaction_region_family_is_refused_at_its_construct() {
    let mismatches: Vec<String> = transaction_region_rows()
        .into_iter()
        .filter_map(|(label, ops, expected)| {
            let got: Vec<(Code, String, SourceSpan)> = diagnostics(&ops)
                .iter()
                .map(|d| (d.code(), d.file().as_str().to_string(), d.span()))
                .collect();
            let want: Vec<(Code, String, SourceSpan)> = expected
                .iter()
                .map(|&(code, needle)| (code, "src/main.mw".to_string(), span_of(&ops, needle)))
                .collect();
            (got != want).then(|| format!("{label}: got {got:?}, want {want:?}"))
        })
        .collect();
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}
