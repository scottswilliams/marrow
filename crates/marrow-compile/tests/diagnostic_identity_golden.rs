//! Diagnostic identity across a representation or algorithm change — the pattern of
//! record.
//!
//! Three checks read the direct-call graph to decide what to report: recursion-cycle
//! membership, the requires-ambient-transaction closure, and the mutate/durable
//! closure the ownership lattice consumes. A fourth, the value-containment cycle
//! report, decides *where* to report from declaration coordinates the declare pass
//! owns. Their *answers* are a property of the graph, but the diagnostics a reader
//! sees are more than the answers: which
//! functions are named, at which spans, with which prose, and above all **in what
//! order**. An algorithm that computes the same closure while emitting the same rows
//! in a different sequence has changed the product, because a compiler's first
//! reported error is the one a person acts on.
//!
//! This suite pins that whole surface as one ordered artifact per corpus, so a
//! rewrite of the underlying traversal is a byte-comparison rather than a judgement
//! call. It is deliberately not a set comparison and deliberately not a code-only
//! comparison: both would pass a reordering.
//!
//! **Pattern of record.** No diagnostic-identity golden existed in this tree before
//! this suite; the shapes it composes are the ordered `(file, code, line, column)`
//! tuple of `declaration_causality.rs` and the exact `codes()` vector of
//! `semantic_availability.rs`, extended with the rendered message because cycle
//! *membership* is carried in the prose ("`name` is part of a recursive call
//! cycle") and nowhere else. A later diagnostic-preserving conversion of a
//! whole-program analysis should cite this file rather than invent a fourth shape.
//!
//! **What a corpus must contain to be worth pinning.** Each fixture below is built
//! so that a plausible wrong answer is observable: several disjoint cycles rather
//! than one (so a traversal that reports components in discovery order instead of
//! function order is caught), cycles of length one, two, and three (so a self-loop
//! is not conflated with a component), non-participating functions interleaved
//! between them (so "report everything" passes nothing), a transitive transaction
//! requirement three calls deep through a generic instantiation (so a propagation
//! that stops at depth one is caught), and — for the coordinate corpus — cycles in
//! two different modules (so an owner returning one module for every row is caught,
//! which a single-file corpus cannot see).

use marrow_compile::{CompileFailure, SourceDiagnostic, compile};
use marrow_project::ProjectInput;

#[path = "common/ids.rs"]
mod ids;
#[path = "common/project.rs"]
mod project_capture;

/// Every row a compilation reports, as the ordered artifact this suite compares:
/// `(file, code, line, column, message)`.
///
/// The message is part of the shape rather than context. Recursion-cycle membership
/// is spelled only in the prose, so a tuple without it would pass a rewrite that
/// reported the right number of rows at the right spans naming the wrong functions.
fn rows(diagnostics: &[SourceDiagnostic]) -> Vec<(String, String, u32, u32, String)> {
    diagnostics
        .iter()
        .map(|row| {
            let span = row.span();
            (
                row.file().as_str().to_string(),
                row.code().to_string(),
                span.line,
                span.column,
                row.message().to_string(),
            )
        })
        .collect()
}

fn refused(project: &ProjectInput) -> Vec<SourceDiagnostic> {
    match compile(project) {
        Ok(_) => panic!("the corpus is built to be refused; it compiled"),
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics.into_iter().collect(),
        Err(other) => panic!("source-triggered failures must remain diagnostics: {other:?}"),
    }
}

/// Render an artifact as one line per row, so a mismatch prints as a readable diff
/// rather than a wall of tuple syntax.
fn artifact(diagnostics: &[SourceDiagnostic]) -> String {
    rows(diagnostics)
        .into_iter()
        .map(|(file, code, line, column, message)| {
            format!("{file}:{line}:{column} {code} {message}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Corpus A — cycle-heavy.
// ---------------------------------------------------------------------------

/// Disjoint recursion cycles of length one, two, and three, with acyclic functions
/// interleaved between them and a generic instantiated from inside a cycle.
///
/// `reject_recursion` reports one row per function that can reach itself, walking
/// the lowered set in image-index order. The interleaved acyclic functions are what
/// make the *order* of the reported rows observable: a traversal that emitted whole
/// components together would group `mutualA`/`mutualB` differently from a walk that
/// visits functions in index order.
const CYCLE_HEAVY: &str = r#"module main

fn selfLoop(n: int): int {
    return selfLoop(n)
}

fn quiet(n: int): int {
    return n
}

fn mutualA(n: int): int {
    return mutualB(n)
}

fn mutualB(n: int): int {
    return mutualA(n)
}

fn alsoQuiet(n: int): int {
    return quiet(n)
}

fn identity<T>(value: T): T {
    return value
}

fn triangleA(n: int): int {
    return triangleB(identity(n))
}

fn triangleB(n: int): int {
    return triangleC(n)
}

fn triangleC(n: int): int {
    return triangleA(n)
}

pub fn driver(n: int): int {
    return alsoQuiet(n)
}
"#;

#[test]
fn the_cycle_heavy_corpus_reports_its_exact_ordered_artifact() {
    let project = project_capture::project(&[("src/main.mw", CYCLE_HEAVY)]);
    let diagnostics = refused(&project);

    assert_eq!(
        artifact(&diagnostics),
        "src/main.mw:3:1 check.recursion `selfLoop` is part of a recursive call cycle\n\
         src/main.mw:11:1 check.recursion `mutualA` is part of a recursive call cycle\n\
         src/main.mw:15:1 check.recursion `mutualB` is part of a recursive call cycle\n\
         src/main.mw:27:1 check.recursion `triangleA` is part of a recursive call cycle\n\
         src/main.mw:31:1 check.recursion `triangleB` is part of a recursive call cycle\n\
         src/main.mw:35:1 check.recursion `triangleC` is part of a recursive call cycle",
        "the cycle-heavy artifact moved",
    );
}

/// The corpus earns its name: it holds three disjoint cycles, of three distinct
/// lengths, and at least as many functions that are on no cycle at all.
///
/// A golden over a corpus that had quietly become trivial — every function on one
/// cycle, or none — would keep passing while proving nothing, so the shape is
/// asserted rather than assumed.
#[test]
fn the_cycle_heavy_corpus_is_actually_cycle_heavy() {
    let project = project_capture::project(&[("src/main.mw", CYCLE_HEAVY)]);
    let reported = rows(&refused(&project)).len();
    assert_eq!(reported, 6, "one row per function on a cycle");
    assert!(
        CYCLE_HEAVY.matches("\nfn ").count() + CYCLE_HEAVY.matches("\npub fn ").count() > reported,
        "the corpus must contain functions on no cycle, or reporting everything passes",
    );
}

// ---------------------------------------------------------------------------
// Corpus B — transaction-closure-heavy, acyclic.
// ---------------------------------------------------------------------------

/// A transitive mutating chain three calls deep, reached through a generic, beside a
/// correctly wrapped export and a read-only export.
///
/// `reject_recursion` yields the acyclic witness the transaction closures require,
/// so this corpus exercises the requires-ambient-transaction propagation rather than
/// the cycle report. `outerCaller` mutates only through `middle` -> `inner`, so a
/// propagation that stopped at depth one would report nothing for it.
const TRANSACTION_HEAVY: &str = r#"module main

resource Counter {
    required value: int
}

store ^counters[id: int]: Counter

fn inner(id: int, v: int) {
    ^counters[id] = Counter(value: v)
}

fn middle(id: int, v: int) {
    inner(id, v)
}

fn outerCaller(id: int, v: int) {
    middle(id, v)
}

fn identity<T>(value: T): T {
    return value
}

pub fn unwrapped(id: int, v: int) {
    outerCaller(identity(id), v)
}

pub fn wrapped(id: int, v: int) {
    transaction {
        outerCaller(id, v)
    }
}

pub fn readOnly(id: int): int? {
    return ^counters[id].value
}
"#;

#[test]
fn the_transaction_heavy_corpus_reports_its_exact_ordered_artifact() {
    let project = ids::minted(|ledger| {
        project_capture::project_with_ids(&[("src/main.mw", TRANSACTION_HEAVY)], ledger)
    });
    let diagnostics = refused(&project);

    assert_eq!(
        artifact(&diagnostics),
        "src/main.mw:26:5 check.requires_transaction calling `outerCaller` here has no ambient \
         transaction. A durable write, replacement, or erase executes only inside a \
         `transaction` block. Wrap the call in a `transaction { … }` block.",
        "the transaction-heavy artifact moved",
    );
}

/// The transitive depth is real: the reported call is three edges from the mutation,
/// so a depth-one propagation cannot produce this artifact.
#[test]
fn the_transaction_heavy_corpus_requires_transitive_propagation() {
    assert!(
        TRANSACTION_HEAVY.contains("fn outerCaller")
            && TRANSACTION_HEAVY.contains("    middle(id, v)")
            && TRANSACTION_HEAVY.contains("    inner(id, v)"),
        "the mutating chain must stay three deep for this corpus to bind",
    );
}
// ---------------------------------------------------------------------------
// Corpus C — value-containment cycles across two modules.
// ---------------------------------------------------------------------------

/// The value-cycle report names a declaration, so its artifact is a *coordinate*
/// artifact: the module the type was written in and the exact name span.
///
/// The corpus spans two modules on purpose. The declare pass owns one identity per
/// module and each declaration's span, and a single-module corpus would keep
/// passing if that owner returned the wrong module for every row. Acyclic
/// declarations are interleaved so "report everything" proves nothing, and the
/// cycle set mixes a self-cycle and a two-step cycle so a walk that conflated a
/// self-loop with a component is observable, and the second module carries a cycle
/// of its own so a wrong module coordinate cannot hide behind a single-file corpus.
///
/// Only struct cycles appear because a record cycle is not expressible in the
/// admitted subset: a resource field typed as a resource, and a struct field typed
/// as a resource, are both `check.unsupported` on the beta line, so the record arm
/// of the report has no source that reaches it today. That predates this suite.
const VALUE_CYCLE_MAIN: &str = r#"module main

use shapes

struct Settled {
    value: int
}

struct Knot {
    me: Knot
}

struct StepA {
    next: StepB
}

struct StepB {
    back: StepA
}

pub fn driver(n: int): int {
    return n
}
"#;

const VALUE_CYCLE_SHAPES: &str = r#"module shapes

struct Calm {
    value: int
}

struct Coil {
    me: Coil
}
"#;

#[test]
fn the_value_cycle_corpus_reports_its_exact_ordered_artifact() {
    let project = ids::minted(|ledger| {
        project_capture::project_with_ids(
            &[
                ("src/main.mw", VALUE_CYCLE_MAIN),
                ("src/shapes.mw", VALUE_CYCLE_SHAPES),
            ],
            ledger,
        )
    });
    let diagnostics = refused(&project);

    assert_eq!(
        artifact(&diagnostics),
        "src/main.mw:9:8 check.recursion value type `Knot` contains itself through the cycle \
         Knot -> Knot\n\
         src/main.mw:13:8 check.recursion value type `StepA` contains itself through the cycle \
         StepA -> StepB -> StepA\n\
         src/main.mw:17:8 check.recursion value type `StepB` contains itself through the cycle \
         StepB -> StepA -> StepB\n\
         src/shapes.mw:7:8 check.recursion value type `Coil` contains itself through the cycle \
         Coil -> Coil",
        "the value-cycle artifact moved",
    );
}

/// The corpus earns its name: cycles of two distinct shapes in two distinct
/// modules, with acyclic declarations in both. A corpus that had drifted to a
/// single module, or to no acyclic declaration, would keep passing while proving
/// neither the module coordinate nor the interleaving.
#[test]
fn the_value_cycle_corpus_spans_two_modules_and_stays_interleaved() {
    assert!(
        VALUE_CYCLE_MAIN.contains("struct Settled")
            && VALUE_CYCLE_MAIN.contains("struct Knot")
            && VALUE_CYCLE_MAIN.contains("struct StepA")
            && VALUE_CYCLE_MAIN.contains("struct StepB"),
        "the main module must keep an acyclic struct beside its cyclic ones",
    );
    assert!(
        VALUE_CYCLE_SHAPES.contains("struct Calm") && VALUE_CYCLE_SHAPES.contains("struct Coil"),
        "the second module must keep an acyclic struct beside its cyclic one",
    );
}
// ---------------------------------------------------------------------------
// Corpus D — store resource bindings.
// ---------------------------------------------------------------------------

/// Store declarations whose written resource spelling binds four different ways:
/// an admitted resource, a name declared as another kind, a name declared nowhere,
/// and a second admitted resource in another module.
///
/// The binding a `store` resolves is decided once, before any store is built, and
/// the rows a reader sees depend on *which* stores refuse and in what order. A
/// corpus with one bad store would keep passing an owner that refused every store,
/// and a single-module corpus would keep passing one that resolved every spelling
/// against the first module's declarations. The admitted stores interleaved between
/// the refused ones are what make "refuse everything" observable, and they are also
/// what carries the identity gaps that follow an admitted binding — so the artifact
/// pins the refusals *and* their precedence against the rows an accepted binding
/// goes on to produce.
const STORE_BINDING_MAIN: &str = r#"module main

use other

struct NotAResource {
    value: int
}

resource Kept {
    required title: string
}

store ^kept[id: int]: Kept

store ^shaped[id: int]: NotAResource

store ^nowhere[id: int]: NeverDeclared

pub fn driver(n: int): int {
    return n
}
"#;

const STORE_BINDING_OTHER: &str = r#"module other

resource Elsewhere {
    required label: string
}

store ^elsewhere[id: int]: Elsewhere

store ^alsoNowhere[id: int]: StillNeverDeclared
"#;

/// The artifact this corpus reported before the store binding became a typed row,
/// captured from the pre-conversion tree and unchanged by it.
const STORE_BINDING_ARTIFACT: &str = "src/main.mw:13:7 check.durable_identity durable identity for application `.` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
src/main.mw:13:7 check.durable_identity durable identity for root `kept` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
src/main.mw:13:7 check.durable_identity durable identity for product `Kept` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
src/main.mw:13:7 check.durable_identity durable identity for key `kept.id` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
src/main.mw:13:7 check.durable_identity durable identity for field `Kept.title` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
src/main.mw:15:1 check.type `NotAResource` is not a resource in this project\n\
src/main.mw:17:1 check.type `NeverDeclared` is not a resource in this project\n\
src/other.mw:7:7 check.durable_identity durable identity for root `elsewhere` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
src/other.mw:7:7 check.durable_identity durable identity for product `Elsewhere` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
src/other.mw:7:7 check.durable_identity durable identity for key `elsewhere.id` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
src/other.mw:7:7 check.durable_identity durable identity for field `Elsewhere.label` is missing from .marrow/ids; `marrow run` mints missing identities (commit the updated .marrow/ids)\n\
src/other.mw:9:1 check.type `StillNeverDeclared` is not a resource in this project";

#[test]
fn the_store_binding_corpus_reports_its_exact_ordered_artifact() {
    let project = project_capture::project_with_ids(
        &[
            ("src/main.mw", STORE_BINDING_MAIN),
            ("src/other.mw", STORE_BINDING_OTHER),
        ],
        None,
    );
    let diagnostics = refused(&project);

    assert_eq!(
        artifact(&diagnostics),
        STORE_BINDING_ARTIFACT,
        "the store-binding artifact moved",
    );
}

/// The corpus earns its name: every binding class is present, in two modules, with
/// admitted stores interleaved between the refused ones.
#[test]
fn the_store_binding_corpus_covers_every_binding_class() {
    assert!(
        STORE_BINDING_MAIN.contains("resource Kept")
            && STORE_BINDING_MAIN.contains("struct NotAResource")
            && STORE_BINDING_MAIN.contains(": NeverDeclared"),
        "the main module must keep an admitted, a wrong-kind, and an undeclared binding",
    );
    assert!(
        STORE_BINDING_OTHER.contains("resource Elsewhere")
            && STORE_BINDING_OTHER.contains(": StillNeverDeclared"),
        "the second module must keep an admitted and an undeclared binding of its own",
    );
}
