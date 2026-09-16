//! Diagnostic identity across a representation or algorithm change.
//!
//! Three checks read the direct-call graph — recursion-cycle membership, the
//! requires-ambient-transaction closure, and the mutate/durable closure the ownership
//! lattice consumes — and a fourth, the value-containment cycle report, decides *where*
//! to report from declaration coordinates the declare pass owns. Their answers are a
//! property of the graph, but the diagnostics a reader sees are more: which functions
//! are named, at which spans, with which prose, and above all **in what order**. A
//! compiler's first reported error is the one a person acts on, so an algorithm that
//! computes the same closure while emitting the rows in a different sequence has
//! changed the product.
//!
//! Each corpus is therefore pinned as one ordered artifact: the `(file, code, line,
//! column)` tuple extended with the rendered message, because cycle *membership* is
//! carried in the prose and nowhere else. A set comparison or a code-only comparison
//! would both pass a reordering.
//!
//! A corpus is worth pinning only when a plausible wrong answer is observable in it:
//! several disjoint cycles rather than one, cycles of length one, two, and three,
//! non-participating functions interleaved between them, a transaction requirement
//! three calls deep through a generic instantiation, and — for the coordinate corpus —
//! cycles in two different modules, which a single-file corpus cannot see.

use marrow_codes::Code;
use marrow_compile::{CompileFailure, SourceDiagnostic, compile};
use marrow_image::bounds;
use marrow_project::ProjectInput;

use super::{ids, project_capture};

/// Every row a compilation reports: `(file, code, line, column, message)`.
///
/// The message is part of the shape rather than context: recursion-cycle membership is
/// spelled only in the prose, so a tuple without it would pass a rewrite that reported
/// the right rows at the right spans naming the wrong functions.
fn rows(diagnostics: &[SourceDiagnostic]) -> Vec<(String, Code, u32, u32, String)> {
    diagnostics
        .iter()
        .map(|row| {
            let span = row.span();
            (
                row.file().as_str().to_string(),
                row.code(),
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

/// The identity of each row — file, code, and exact span — one line per row, so a
/// mismatch prints as a readable diff rather than a wall of tuple syntax.
///
/// The rendered message is deliberately absent: these goldens pin which rows are
/// reported, under which code, at which construct, and in which order — not prose,
/// which is the renderer's output.
fn artifact(diagnostics: &[SourceDiagnostic]) -> String {
    rows(diagnostics)
        .into_iter()
        .map(|(file, code, line, column, _)| format!("{file}:{line}:{column} {}", code.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Disjoint recursion cycles of length one, two, and three, with acyclic functions
/// interleaved between them and a generic instantiated from inside a cycle.
///
/// `reject_recursion` reports one row per function that can reach itself, walking the
/// lowered set in image-index order. The interleaved acyclic functions make that order
/// observable: a traversal that emitted whole components together would group
/// `mutualA`/`mutualB` differently from a walk that visits functions in index order.
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
        "src/main.mw:3:1 check.recursion\n\
         src/main.mw:11:1 check.recursion\n\
         src/main.mw:15:1 check.recursion\n\
         src/main.mw:27:1 check.recursion\n\
         src/main.mw:31:1 check.recursion\n\
         src/main.mw:35:1 check.recursion",
        "the cycle-heavy artifact moved",
    );
}

/// The corpus earns its name: three disjoint cycles of three distinct lengths, and at
/// least as many functions on no cycle at all. A corpus that had quietly become trivial
/// — every function on one cycle, or none — would keep passing while proving nothing.
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

/// A transitive mutating chain three calls deep, reached through a generic, beside a
/// correctly wrapped export and a read-only export.
///
/// `outerCaller` mutates only through `middle` -> `inner`, so a
/// requires-ambient-transaction propagation that stopped at depth one would report
/// nothing for it.
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
        "src/main.mw:26:5 check.requires_transaction",
        "the transaction-heavy artifact moved",
    );
}

/// The value-cycle report names a declaration, so its artifact is a *coordinate*
/// artifact: the module the type was written in and the exact name span.
///
/// The corpus spans two modules on purpose: the declare pass owns one identity per
/// module and each declaration's span, and a single-module corpus would keep passing if
/// that owner returned the wrong module for every row. Acyclic declarations are
/// interleaved so "report everything" proves nothing, and the cycle set mixes a
/// self-cycle with a two-step cycle so a walk that conflated the two is observable.
///
/// Only struct cycles appear because a record cycle is not expressible in the admitted
/// subset: a resource field typed as a resource, and a struct field typed as a
/// resource, are both `check.unsupported` on the beta line, so the record arm of the
/// report has no source that reaches it.
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
        "src/main.mw:9:8 check.recursion\n\
         src/main.mw:13:8 check.recursion\n\
         src/main.mw:17:8 check.recursion\n\
         src/shapes.mw:7:8 check.recursion",
        "the value-cycle artifact moved",
    );
}

/// Store declarations whose written resource spelling binds four different ways:
/// an admitted resource, a name declared as another kind, a name declared nowhere,
/// and a second admitted resource in another module.
///
/// The binding a `store` resolves is decided once, before any store is built, and the
/// rows a reader sees depend on *which* stores refuse and in what order. A corpus with
/// one bad store would keep passing an owner that refused every store, and a
/// single-module corpus would keep passing one that resolved every spelling against the
/// first module's declarations. The admitted stores also carry the identity rows that
/// follow an accepted binding, so the artifact pins the refusals *and* their precedence
/// against those rows.
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
        "src/main.mw:13:7 check.durable_identity\n\
         src/main.mw:13:7 check.durable_identity\n\
         src/main.mw:13:7 check.durable_identity\n\
         src/main.mw:13:7 check.durable_identity\n\
         src/main.mw:13:7 check.durable_identity\n\
         src/main.mw:15:1 check.type\n\
         src/main.mw:17:1 check.type\n\
         src/other.mw:7:7 check.durable_identity\n\
         src/other.mw:7:7 check.durable_identity\n\
         src/other.mw:7:7 check.durable_identity\n\
         src/other.mw:7:7 check.durable_identity\n\
         src/other.mw:9:1 check.type"
    );
}

/// Store roots whose managed indexes violate one admission rule each: the per-root
/// count cap, the projection width cap, a name collision with a stored field, a name
/// collision with an earlier index, and a singleton root that has no identity to point
/// at.
///
/// Admitted indexes are declared first and interleaved between the refused roots, so an
/// owner that refused every index would not pass, and the count-cap root carries nine
/// otherwise-valid indexes so the cap is observed on a body that is wrong only in its
/// length.
const INDEX_ADMISSION_MAIN: &str = r#"module main

use other

resource Book {
    required title: string
    required isbn: string
    shelf: string

    details {
        pages: int
    }
}

resource Single {
    required label: string
}

store ^books[id: int]: Book {
    index byIsbn[isbn] unique
    index byShelf[shelf, id]
}

store ^only: Single {
    index byLabel[label]
}

store ^collide[id: int]: Book {
    index title[isbn] unique
    index sameName[isbn] unique
    index sameName[shelf] unique
}

store ^many[id: int]: Book {
    index a1[isbn] unique
    index a2[shelf, id]
    index a3[title, id]
    index a4[isbn, id]
    index a5[shelf, title, id]
    index a6[title, isbn, id]
    index a7[isbn, shelf, id]
    index a8[shelf, isbn, id]
    index a9[title, shelf, id]
}
"#;

/// The component-resolution half of the same surface, in a second module: a component
/// repeated within one index, a component reaching through a nested member, a component
/// naming nothing, a component whose stored value is not an orderable durable key, and
/// a non-unique index that does not end with the root's identity keys.
///
/// A table keyed by declaration must answer for the module its row came from, and a
/// single-file corpus would keep passing an owner that resolved every index against the
/// first module's declarations.
const INDEX_ADMISSION_OTHER: &str = r#"module other

resource Note {
    required text: string
    required tag: string
    weight: Option<duration>

    body {
        line: int
    }
}

store ^notes[id: int]: Note {
    index repeatArg[tag, tag, id]
    index nestedArg[body.line, id]
    index absentArg[missing, id]
    index unorderedArg[weight, id]
    index noSuffix[tag]
}
"#;

/// A third module whose single index crosses the fixed projection width, generated
/// rather than written out because the cap is 72 components. The components
/// deliberately name nothing: the width is checked before any leaf is resolved, so this
/// reports the width and only the width.
fn index_width_module() -> String {
    let components = (1..=bounds::MAX_INDEX_COMPONENTS + 1)
        .map(|at| format!("c{at}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "module wide\n\nresource Wide {{\n    required label: string\n}}\n\n\
         store ^wide[id: int]: Wide {{\n    index tooWide[{components}]\n}}\n"
    )
}

#[test]
fn the_index_admission_corpus_reports_its_exact_ordered_artifact() {
    let wide = index_width_module();
    let project = project_capture::project_with_ids(
        &[
            ("src/main.mw", INDEX_ADMISSION_MAIN),
            ("src/other.mw", INDEX_ADMISSION_OTHER),
            ("src/wide.mw", &wide),
        ],
        None,
    );
    let diagnostics = refused(&project);

    assert_eq!(
        artifact(&diagnostics),
        "src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:24:7 check.durable_identity\n\
         src/main.mw:24:7 check.durable_identity\n\
         src/main.mw:24:7 check.durable_identity\n\
         src/main.mw:25:1 check.type\n\
         src/main.mw:28:7 check.durable_identity\n\
         src/main.mw:28:7 check.durable_identity\n\
         src/main.mw:29:1 check.type\n\
         src/main.mw:28:7 check.durable_identity\n\
         src/main.mw:31:1 check.type\n\
         src/main.mw:34:7 check.durable_identity\n\
         src/main.mw:34:7 check.durable_identity\n\
         src/main.mw:43:1 check.type\n\
         src/other.mw:13:7 check.durable_identity\n\
         src/other.mw:13:7 check.durable_identity\n\
         src/other.mw:13:7 check.durable_identity\n\
         src/other.mw:13:7 check.durable_identity\n\
         src/other.mw:13:7 check.durable_identity\n\
         src/other.mw:13:7 check.durable_identity\n\
         src/other.mw:13:7 check.durable_identity\n\
         src/other.mw:13:7 check.durable_identity\n\
         src/other.mw:13:7 check.durable_identity\n\
         src/other.mw:13:7 check.durable_identity\n\
         src/other.mw:13:7 check.durable_identity\n\
         src/other.mw:14:26 check.type\n\
         src/other.mw:15:21 check.type\n\
         src/other.mw:16:21 check.type\n\
         src/other.mw:17:24 check.type\n\
         src/other.mw:18:1 check.type\n\
         src/wide.mw:7:7 check.durable_identity\n\
         src/wide.mw:7:7 check.durable_identity\n\
         src/wide.mw:7:7 check.durable_identity\n\
         src/wide.mw:7:7 check.durable_identity\n\
         src/wide.mw:8:1 check.resource_limit"
    );
}

/// The corpus earns its name: the pinned artifact carries one row for every managed-
/// index admission rule the durable builder enforces, and admitted indexes beside them.
/// A corpus that had quietly stopped reaching a rule would keep passing while proving
/// nothing about it, and one with no admitted index would keep passing an owner that
/// refused every index.
#[test]
fn the_index_admission_corpus_reaches_every_admission_rule() {
    let wide = index_width_module();
    let project = project_capture::project_with_ids(
        &[
            ("src/main.mw", INDEX_ADMISSION_MAIN),
            ("src/other.mw", INDEX_ADMISSION_OTHER),
            ("src/wide.mw", &wide),
        ],
        None,
    );
    let reported = rows(&refused(&project));
    let reaches = |needle: &str| {
        reported
            .iter()
            .any(|(_, _, _, _, message)| message.contains(needle))
    };
    for rule in [
        "declares 9 managed indexes; at most 8 are allowed",
        "a managed index projects 73 components; the fixed limit is 72",
        "index `title` collides with an identity key, a stored field, or another index",
        "index `sameName` collides with an identity key, a stored field, or another index",
        "index `byLabel` requires a keyed store root",
        "index `repeatArg` repeats component `tag`",
        "index `nestedArg` component `body.line` reaches through a nested member",
        "index `absentArg` component `missing` names no identity key or stored field",
        "index `unorderedArg` component `weight` is not an orderable durable-key scalar",
        "non-unique index `noSuffix` must end with the store's identity keys",
    ] {
        assert!(
            reaches(rule),
            "the corpus no longer reaches this rule: {rule}",
        );
    }
    for admitted in ["index `books.byIsbn`", "index `books.byShelf`"] {
        assert!(
            reaches(admitted),
            "the corpus must keep admitted indexes beside the refused ones: {admitted}",
        );
    }
}

/// The two durable key tuples a program can declare, each one column past the fixed
/// width: a keyed `branch` placement's tuple, and a `store` root's own.
///
/// The two are the same shape under the same limit, declared in two different places
/// and reported with two different subjects. A corpus carrying only one of them would
/// keep passing an owner that had collapsed the two subjects into whichever one it
/// still reached — exactly the failure a shared renderer can introduce. The over-wide
/// branch hangs off an admitted root so the branch refusal is reached at all.
const KEY_WIDTH_MAIN: &str = r#"module main

resource Slim {
    required label: string

    deep[a: int, b: int, c: int, d: int, e: int, f: int, g: int, h: int, i: int] {
        required note: string
    }
}

resource Plain {
    required label: string
}

store ^branchy[id: int]: Slim

store ^wide[k1: int, k2: int, k3: int, k4: int, k5: int, k6: int, k7: int, k8: int, k9: int]: Plain

store ^atCap[c1: int, c2: int, c3: int, c4: int, c5: int, c6: int, c7: int, c8: int]: Plain
"#;

#[test]
fn the_key_width_corpus_reports_its_exact_ordered_artifact() {
    let project = project_capture::project_with_ids(&[("src/main.mw", KEY_WIDTH_MAIN)], None);
    let diagnostics = refused(&project);

    assert_eq!(
        artifact(&diagnostics),
        "src/main.mw:15:7 check.durable_identity\n\
         src/main.mw:15:7 check.durable_identity\n\
         src/main.mw:15:7 check.durable_identity\n\
         src/main.mw:15:7 check.durable_identity\n\
         src/main.mw:15:7 check.durable_identity\n\
         src/main.mw:15:7 check.durable_identity\n\
         src/main.mw:6:1 check.resource_limit\n\
         src/main.mw:15:7 check.durable_identity\n\
         src/main.mw:17:7 check.resource_limit\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity\n\
         src/main.mw:19:7 check.durable_identity"
    );
}

/// Both key-tuple subjects are present, both over-wide tuples really are one column
/// past the fixed width, and the admitted tuple sits exactly at the cap, so the corpus
/// cannot go vacuous by the limit moving underneath it.
#[test]
fn the_key_width_corpus_carries_both_tuple_subjects() {
    assert_eq!(
        bounds::MAX_KEY_COLUMNS + 1,
        9,
        "the corpus declares nine-column tuples because the fixed limit is eight",
    );
    let project = project_capture::project_with_ids(&[("src/main.mw", KEY_WIDTH_MAIN)], None);
    let reported = rows(&refused(&project));
    let subjects: Vec<&str> = reported
        .iter()
        .filter(|(_, code, _, _, _)| *code == Code::CheckResourceLimit)
        .map(|(_, _, _, _, message)| message.as_str())
        .collect();
    assert_eq!(
        subjects,
        [
            "a branch key tuple has 9 columns; the fixed limit is 8",
            "a store root key tuple has 9 columns; the fixed limit is 8",
        ],
        "the corpus must carry both key-tuple subjects, and only those",
    );
    assert!(
        reported.iter().any(
            |(_, code, _, _, message)| *code == Code::CheckDurableIdentity
                && message.contains("key `atCap.c8`")
        ),
        "the at-cap tuple must stay admitted: its eighth column anchors instead of \
         earning a width refusal",
    );
}

/// A store root tuple that is both over-wide and carries a column outside the
/// durable-key scalar set.
///
/// The two refusals are ranked: the width cap is reported and the key type is not. A
/// tuple past the fixed width has no admissible column list to judge, so telling its
/// author about one column's type first would steer them at the smaller of two faults.
/// The key-width corpus alone keeps passing with the ranking inverted, because none of
/// its tuples is both.
const KEY_RANK_MAIN: &str = r#"module main

resource Plain {
    required label: string
}

store ^both[k1: int, k2: int, k3: int, k4: int, k5: int, k6: int, k7: int, k8: int, k9: duration]: Plain
"#;

#[test]
fn an_over_wide_tuple_reports_its_width_rather_than_a_column_type() {
    let project = project_capture::project_with_ids(&[("src/main.mw", KEY_RANK_MAIN)], None);
    let diagnostics = refused(&project);

    assert_eq!(
        rows(&diagnostics),
        vec![(
            "src/main.mw".to_string(),
            Code::CheckResourceLimit,
            7,
            7,
            "a store root key tuple has 9 columns; the fixed limit is 8".to_string(),
        )],
        "an over-wide tuple reports its width, and reports nothing about a column",
    );
}

/// Two branch key columns that are not durable-key scalars — one a scalar outside
/// the durable-key set, one no scalar at all — declared in one module and occurred
/// by a store in another.
///
/// Both refusals are about the branch declaration, so both are attributed to the module
/// that declares it rather than to whichever store first built the resource's graph.
/// The two modules are what make that attribution observable: a single-file corpus
/// would agree either way.
///
/// The corpus carries both refusing columns because the attribution feeds both arms of
/// key-scalar resolution: `duration` is a scalar the durable-key set excludes and earns
/// `check.type`, while a struct is not a scalar at all and earns `check.unsupported`.
/// Pinning one arm would leave the other free to move.
const BRANCH_KEY_MODEL: &str = r#"module model

struct Note {
    n: int
}

resource R {
    required title: string

    items[k: duration] {
        required v: string
    }

    notes[k: Note] {
        required v: string
    }
}
"#;

const BRANCH_KEY_MAIN: &str = r#"module main

use model

store ^r[id: int]: R

pub fn driver(n: int): int {
    return n
}
"#;

#[test]
fn a_branch_key_refusal_is_attributed_to_the_declaring_module() {
    let project = ids::minted(|ledger| {
        project_capture::project_with_ids(
            &[
                ("src/main.mw", BRANCH_KEY_MAIN),
                ("src/model.mw", BRANCH_KEY_MODEL),
            ],
            ledger,
        )
    });
    let diagnostics = refused(&project);

    assert_eq!(
        rows(&diagnostics),
        vec![
            (
                "src/model.mw".to_string(),
                Code::CheckType,
                10,
                1,
                "a durable key column must be an orderable durable-key scalar (int, string, \
                 bool, bytes, date, or instant)"
                    .to_string(),
            ),
            (
                "src/model.mw".to_string(),
                Code::CheckUnsupported,
                14,
                1,
                "this key type is not yet supported on the beta line".to_string(),
            ),
        ],
        "both branch key refusals are reported in the module that declares the branch, \
         each at its own branch's span, in declaration order",
    );
}

/// A generic template and a concrete struct sharing one name, the template first,
/// the concrete one containing itself.
///
/// The value-cycle report names the declaration whose coordinate the declare pass
/// reserved — the concrete struct, which is the one on the cycle. A report that instead
/// searched the raw declaration list by name would take the first match, the generic
/// template, and pin the cycle at the template's span. The template is declared first
/// on purpose; declared second, the name search and the coordinate agree.
const HOMONYM_CYCLE: &str = r#"module main

struct A<T> {
    value: T
}

struct A {
    me: A
}

pub fn driver(n: int): int {
    return n
}
"#;

#[test]
fn a_value_cycle_is_reported_at_the_concrete_declaration_not_a_homonym_template() {
    let project = project_capture::project(&[("src/main.mw", HOMONYM_CYCLE)]);
    let diagnostics = refused(&project);

    assert_eq!(
        rows(&diagnostics),
        vec![
            (
                "src/main.mw".to_string(),
                Code::CheckNameConflict,
                3,
                8,
                "`A` is already declared as a struct".to_string(),
            ),
            (
                "src/main.mw".to_string(),
                Code::CheckRecursion,
                7,
                8,
                "value type `A` contains itself through the cycle A -> A".to_string(),
            ),
        ],
        "the cycle is reported at the concrete declaration's own name span",
    );
}
