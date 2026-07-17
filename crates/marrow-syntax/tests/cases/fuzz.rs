//! The source-bytes fuzz driver: a thin input adapter over the reusable bounded
//! oracle in `common::oracle`. It carries no invariants of its own — it only feeds
//! the oracle a bounded, reproducible stream of inputs:
//!
//! 1. a deterministic corpus (the tracer-subset constructs, the documented library
//!    blocks and tracked `.mw` fixtures, pathological inputs, and every
//!    char-boundary truncation of the reference program), and
//! 2. a seeded, fixed-iteration random-mutation pass over that corpus.
//!
//! Arbitrary bytes reach the `&str`-typed front end through `String::from_utf8_lossy`,
//! the total decode the file boundary uses, so invalid UTF-8 and NUL bytes are
//! exercised as the replacement-bearing text the parser actually sees. The pass is
//! bounded and reproducible: one fixed seed, a fixed iteration budget, and a small
//! interesting-byte alphabet, so a failure reproduces exactly and no unbounded
//! campaign runs in CI. A minimized counterexample becomes a case in
//! [`regression_corpus`] and then a fix.

use crate::common;
use crate::common::oracle::{
    OVER_DEEP, assert_formatter_faithful, assert_total_invariants, has_error_diagnostic,
};

/// The tracer-subset constructs, each a small complete or near-complete program.
/// These are valid programs, so they also feed the faithful-formatter lens.
fn tracer_subset_programs() -> Vec<String> {
    [
        "module app\n",
        "module shelf::books\n\nuse std::clock\nuse shelf::books\n",
        "module app\n\nconst Max: int = 5\n",
        "module app\n\nalias Count = int\n\nalias MaybeCount = Count?\n\nfn f(n: Count): MaybeCount {\n    return n\n}\n",
        "module app\n\ntype Age: int in 0..=150 supports add, subtract, step, scale\n\ntype Percent: int in 0..101\n\nfn f(a: Age): Age? {\n    return Age.checked(a - Age(0))\n}\n",
        "module app\n\nconst Greeting = $\"hello {name}: {{literal}}\"\n",
        "module app\n\nresource Book {\n    required title: string\n    tags[pos: int]: string\n    notes[noteId: string] {\n        text: string\n    }\n}\n\nstore ^books[id: int]: Book {\n    index byShelf[shelf, id]\n    index uniq[id] unique\n}\n",
        "module app\n\nstruct Point {\n    x: int\n    y: int\n}\n\nfn origin(): int {\n    const p = Point(x: 0, y: 0)\n    return p.x\n}\n",
        "module app\n\nenum Status {\n    active\n    archived\n}\n",
        "module app\n\nenum Cat {\n    category feline {\n        tiger\n        lion\n    }\n}\n",
        "module app\n\nenum Shape {\n    dot\n    circle(radius: int)\n    rect(width: int, height: int)\n}\n",
        "module app\n\npub fn add(a: int, b: int): int {\n    return a + b\n}\n",
        "module app\n\nfn classify(n: int) {\n    if n < 0 {\n        return\n    } else if n > 0 {\n        return\n    } else return\n}\n",
        "module app\n\nfn each() {\n    for id in keys(^books) {\n        delete ^books[id]\n    }\n}\n",
        "module app\n\nfn clear() {\n    var b = Box(id: 1, note: \"x\")\n    b.note = \"y\"\n    unset b.note\n}\n",
        "module app\n\nfn ranged() {\n    for i in 10..=1 by -2 {\n        print($\"{i}\")\n    }\n}\n",
        "module app\n\nfn scan() {\n    for k in ^books at most 5 {\n        print($\"{k}\")\n    } on more {\n        print(\"more\")\n    }\n}\n",
        "module app\n\nfn scanBranch(lo: int) {\n    for p in ^books[lo].notes at most 3 from lo {\n        print($\"{p}\")\n    } on more {\n        print(\"more\")\n    }\n}\n",
        "module app\n\nfn loops() {\n    while ready {\n        break\n    }\n}\n",
        "module app\n\nfn label(s: Status) {\n    match s {\n        active => print(\"a\")\n        archived => print(\"b\")\n    }\n}\n",
        "module app\n\nfn area(s: Shape): int {\n    match s {\n        dot => return 0\n        circle(r) => return r\n        rect(w, h) => return w\n    }\n}\n",
        "module app\n\nfn commit(id: Id(^books)) {\n    transaction {\n        ^books[id].title = title\n    }\n}\n",
        "module app\n\nfn edit(id: int) {\n    transaction {\n        place b = ^books[id]\n        b.title = \"x\"\n        b = Book(title: \"y\")\n        delete b\n    }\n}\n",
        "module app\n\nfn risky(): Result<int, string> {\n    const x = try run()\n    return ok(x)\n}\n",
        "module app\n\nfn nested(o: Option<Option<int>>): int {\n    match o {\n        none => return 0\n        some(inner) => return depth(inner)\n    }\n}\n",
        "module app\n\nfn find(): Result<Option<int>, string> {\n    const x = try lookup()\n    return ok(some(x))\n}\n",
        "module app\n\nfn build(): List<int> {\n    var xs: List<int> = List()\n    xs = append(xs, 1)\n    for x in xs {\n        print($\"{x}\")\n    }\n    return xs\n}\n",
        "module app\n\nfn score(): Map<string, int> {\n    var m: Map<string, List<int>> = Map()\n    m = insert(m, \"a\", List())\n    for k, v in m {\n        print(k)\n    }\n    return get(m, \"a\")\n}\n",
        "module app\n\nfn amounts(): int? {\n    return absent\n}\n",
        "module app\n\nfn identity<T>(x: T): T {\n    return x\n}\n",
        "module app\n\npub fn firstOf<T supports equality, U supports order>(xs: List<T>, k: U): T? {\n    return first(xs)\n}\n",
        "module app\n\nstruct Pair<A, B> {\n    first: A\n    second: B\n}\n\nfn firstOf<A, B>(p: Pair<A, B>): A {\n    return p.first\n}\n",
        "module app\n\nenum Box<T> {\n    empty\n    full(value: T)\n}\n\npub enum Sorted<T supports order> {\n    blank\n    span(lo: T, hi: T)\n}\n",
        "module app\n\nstruct Wrapper<T> {\n    value: T\n}\n\nfn wrap<T>(x: T): Wrapper<T> {\n    return Wrapper(value: x)\n}\n",
        "module app\n\nfn compound() {\n    var total: int = 0\n    total += 1\n    total *= 2\n}\n",
        "module app\n\nfn strings() {\n    const b = b\"bytes\"\n    const d = 1.day\n    const n = start ?? 0\n}\n",
        "module app\n\nfn guard(a: int, b: int): int {\n    const q: int = checked a / b\n        on out_of_range {\n            return 0\n        } on zero_divisor return 0\n    return q\n}\n",
        "module app\n\nfn guardReturn(a: int, b: int): int {\n    return checked a + b\n        on out_of_range return 0\n}\n",
        "module app\n\ntest \"adds two numbers\" {\n    const sum = 1 + 1\n    assert sum == 2\n}\n",
        "module app\n\ntest \"a plain assertion\" {\n    assert true\n}\n",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Pathological inputs the front end must survive without panicking or unbounded
/// work: nesting past the layout and expression limits, a very long line, NUL and
/// invalid-UTF-8 bytes, unterminated and unbalanced constructs, and mixed line
/// endings. None is a valid program, so these feed only the total-invariant lens.
fn pathological_inputs() -> Vec<String> {
    let mut inputs = vec![
        String::new(),
        "\n\n\n".to_string(),
        "\t\t\t\n".to_string(),
        "\0\0\0\0\0".to_string(),
        // Invalid UTF-8 bytes, lossy-decoded exactly as the file boundary would.
        String::from_utf8_lossy(&[0xff, 0xfe, 0x80, b'a', 0x00, 0xc0]).into_owned(),
        "const X = \"unterminated".to_string(),
        "const X = $\"a{unterminated".to_string(),
        "const X = $\"a{$\"b{$\"c{".to_string(),
        "fn f() {\n    return (((((((((1".to_string(),
        "fn f() {\n    return )))))))))\n}\n".to_string(),
        "\\\\\\\\\\\n".to_string(),
        "resource R {\r\n    x: int\r\n}\r\n".to_string(),
        "const X = 999999999999999999999999999999.day\n".to_string(),
        "@#$%^&*~`|\n".to_string(),
        "module\nuse\nconst\nresource\nstore\nenum\nfn\n".to_string(),
        // A checked form with a malformed arm header and no body must recover.
        "fn f(a: int) {\n    const q = checked a + a\n        on nope\n}\n".to_string(),
    ];
    // Very long single line: a wide operand chain the expression parser bounds.
    inputs.push(format!("const X = {}\n", "1 + ".repeat(5_000)));
    // Deep nesting past every limit — block braces, expression, field access, members.
    inputs.push(deep_ifs(OVER_DEEP));
    inputs.push(deep_parens(OVER_DEEP));
    inputs.push(deep_enum_members(OVER_DEEP));
    inputs.push(deep_field_access(OVER_DEEP));
    inputs
}

fn deep_ifs(depth: usize) -> String {
    let mut source = String::from("module app\n\npub fn main() {\n");
    for level in 0..depth {
        source.push_str(&"    ".repeat(level + 1));
        source.push_str(&format!("if {level} < {} {{\n", level + 1));
    }
    source.push_str(&"    ".repeat(depth + 1));
    source.push_str("return\n");
    for level in (0..depth).rev() {
        source.push_str(&"    ".repeat(level + 1));
        source.push_str("}\n");
    }
    source.push_str("}\n");
    source
}

fn deep_parens(depth: usize) -> String {
    format!(
        "module app\n\npub fn main() {{\n    return {}1{}\n}}\n",
        "(".repeat(depth),
        ")".repeat(depth)
    )
}

fn deep_enum_members(depth: usize) -> String {
    let mut source = String::from("module app\n\nenum E {\n");
    for level in 0..depth {
        source.push_str("    ");
        source.push_str(&format!("m{level}\n"));
    }
    source.push_str("}\n");
    source
}

fn deep_field_access(depth: usize) -> String {
    format!(
        "module app\n\npub fn main() {{\n    return a{}\n}}\n",
        ".f".repeat(depth)
    )
}

/// Minimized counterexamples whose contract is the lossless token tiling and the
/// total invariants (not formatter faithfulness). A comment leader inside a string-
/// interpolation hole must stop at the hole boundary rather than run to the physical
/// line end, past the hole, overlapping the interpolation-close tokens and breaking
/// the tiling. `//` is the brace-surface comment leader (`;` is now ordinary
/// punctuation), so both the `//`-in-hole forms and the bare-punctuation forms feed
/// the total-invariant lens; the hole comment is not carried through formatting.
fn tiling_regressions() -> Vec<String> {
    [
        "const X = $\"a{g(1)//}b\"\n",
        "const X = $\"{a//}\"\n",
        "const X = $\"a{$\"b{//}\"}c\"\n",
        "const X = $\"a{g(1;)}b\"\n",
        "const X = $\"{a;}\"\n",
        "const X = $\"a{$\"b{;}\"}c\"\n",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Minimized counterexamples that are valid programs whose formatter faithfulness
/// (idempotent, comment-preserving, structure-preserving) regressed and is now
/// fixed. Each is asserted through the faithful lens so the fix stays pinned.
fn formatter_faithful_regressions() -> Vec<String> {
    // An empty-bodied compound statement or match arm left a dangling newline that
    // the block-level blank accounting doubled, so a single source blank before the
    // next statement rendered as two and, on the mixed shapes below, grew 1 -> 2
    // across formats. Every body-bearing header now joins its body through one
    // empty-body guard. (The former layout-column and `;`-comment regressions —
    // an own-line comment misread as outdented, an empty body claiming a following
    // sibling comment — were defect classes of indentation-as-structure and the `;`
    // comment leader; braces make the block boundary explicit and comment ownership
    // over braces is pinned by the `comment_ownership` suite, so those cases are
    // structurally impossible rather than rewritten.)
    [
        "module app\n\nfn f() {\n    match s {\n        d => {}\n    }\n\n    b\n}\n",
        "module app\n\nfn f() {\n    if x {}\n\n    b\n}\n",
        "module app\n\nfn f() {\n    while x {}\n\n    b\n}\n",
        "module app\n\nfn f() {\n    for i in xs {}\n\n    b\n}\n",
        "module app\n\nfn f() {\n    transaction {}\n\n    b\n}\n",
        "module app\n\nfn f(o: Option<int>): int {\n    match o {\n        some(v) => return v\n        none => return 0\n    }\n}\n",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// The valid-program corpus: the tracer-subset programs, the documented module
/// blocks, and the tracked `.mw` fixtures that parse cleanly. Each feeds both the
/// total and the faithful-formatter lens.
fn valid_programs() -> Vec<String> {
    let mut programs = tracer_subset_programs();
    for block in common::documented_module_blocks() {
        programs.push(block.source);
    }
    programs
}

/// The parser's nesting limit is calibrated for the 256 MB stack the CLI runs it
/// on (`WORKER_STACK_BYTES` in the `marrow` binary), where a 256-deep parse fits
/// but the small default test-thread stack does not. Run each driver body on a
/// matching worker stack so the oracle exercises the production environment, then
/// re-raise any panic on the test thread with its message.
fn on_worker_stack(body: impl FnOnce() + Send + 'static) {
    const WORKER_STACK_BYTES: usize = 256 * 1024 * 1024;
    let worker = std::thread::Builder::new()
        .stack_size(WORKER_STACK_BYTES)
        .spawn(body)
        .expect("spawn fuzz worker thread");
    if let Err(panic) = worker.join() {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn deterministic_corpus_holds_the_oracle_invariants() {
    on_worker_stack(deterministic_corpus_body);
}

fn deterministic_corpus_body() {
    let mut saw_error = false;
    let mut saw_over_deep = false;

    // Valid programs and the fixed formatter-faithfulness regressions: total
    // invariants plus the faithful-formatter contract.
    for source in valid_programs()
        .into_iter()
        .chain(formatter_faithful_regressions())
    {
        assert_total_invariants(&source);
        assert_formatter_faithful(&source);
    }

    // Tracked shared-syntax fixtures: total invariants always; the stronger
    // faithful lens only over those that parse cleanly (a legacy construct outside
    // the tracer subset still tiles and bounds, but its rendering is not a contract).
    for (path, source) in common::tracked_mw_fixtures() {
        assert_total_invariants(&source);
        if !marrow_syntax::parse_source(&source).has_errors() {
            assert_formatter_faithful(&source);
        } else {
            saw_error = true;
            let _ = path;
        }
    }

    // Pathological and tiling-regression inputs: total invariants only.
    for source in pathological_inputs()
        .into_iter()
        .chain(tiling_regressions())
    {
        assert_total_invariants(&source);
        saw_error |= has_error_diagnostic(&source);
        saw_over_deep |= marrow_syntax::parse_source(&source)
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_syntax::NESTING_LIMIT);
    }

    // Every char-boundary truncation of the reference program is a distinct
    // partially-written source; each must hold the total invariants.
    let sample = common::reference_sample();
    for end in char_boundaries(&sample) {
        assert_total_invariants(&sample[..end]);
    }

    assert!(saw_error, "the corpus must exercise the recovery path");
    assert!(saw_over_deep, "the corpus must exercise the nesting limit");
}

#[test]
fn seeded_random_mutation_pass_holds_the_total_invariants() {
    on_worker_stack(seeded_random_mutation_body);
}

fn seeded_random_mutation_body() {
    // A fixed default seed and iteration budget keep this reproducible and
    // CI-bounded: a failure reproduces exactly from the seed, and no unbounded
    // campaign runs. MARROW_FUZZ_SEED widens the search without editing code.
    const DEFAULT_SEED: u64 = 0x5241_4d5f_4655_5a5a; // "RAM_FUZZ"
    let seed: u64 = std::env::var("MARROW_FUZZ_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SEED);
    const ITERATIONS: usize = 4_000;
    const MAX_MUTATIONS: usize = 24;

    let seeds: Vec<Vec<u8>> = tracer_subset_programs()
        .into_iter()
        .chain(pathological_inputs())
        .map(String::into_bytes)
        .collect();

    let mut rng = SplitMix64::new(seed);
    let mut mutated_error = false;
    for _ in 0..ITERATIONS {
        let mut bytes = seeds[rng.below(seeds.len() as u64) as usize].clone();
        let rounds = 1 + rng.below(MAX_MUTATIONS as u64) as usize;
        for _ in 0..rounds {
            mutate(&mut bytes, &mut rng);
        }
        // The file boundary decodes bytes to text losslessly-or-lossy; feed the
        // parser exactly that, so invalid UTF-8 and NUL bytes are covered.
        let source = String::from_utf8_lossy(&bytes);
        assert_total_invariants(&source);
        mutated_error |= has_error_diagnostic(&source);
    }
    assert!(
        mutated_error,
        "the random-mutation pass must reach the recovery path"
    );
}

fn char_boundaries(source: &str) -> Vec<usize> {
    (0..=source.len())
        .filter(|&index| source.is_char_boundary(index))
        .collect()
}

/// The brace-grammar fuzz corpus (BS01): declarations with `{ … }` bodies —
/// resource, store, enum, and function — plus `=>` match arms, `//` and `///`
/// comments, `\u{}` escapes, bracket key groups, and angle generics, including the
/// unclosed and stray-brace forms that a member loop must survive. It is written
/// against the live brace grammar.
fn brace_grammar_corpus() -> Vec<String> {
    [
        "module app\nfn run() {\n    return\n}\n",
        "module app\nresource B {\n    required title: string\n    notes[id: string] {\n        text: string\n    }\n}\n",
        "module app\nstore ^books[id: int]: B {\n    index byTitle[title]\n}\n",
        "module app\nenum Cat {\n    lion\n    tiger {\n        bengal\n        siberian\n    }\n}\n",
        "module app\nfn area(s: Shape): int {\n    match s {\n        dot => return 0\n        circle(r) => {\n            return r\n        }\n    }\n    return -1\n}\n",
        "module app\n// a line comment\n/// a doc comment\nfn run() {\n    return // trailing\n}\n",
        "module app\nfn run(): Map<string, int> {\n    var m: Map<string, List<int>> = Map()\n    return get(m, \"a\")\n}\n",
        "module app\nconst S = \"a\\u{1F600}b\"\n",
        "module app\nfn run() {\n    ^books[1].title = \"x\"\n}\n",
        "module app\nfn run() {\n    if const a = ^c[1].v and const b = ^c[2].v and a < b {\n        return\n    }\n}\n",
        // Comment-bearing seeds (MSY01): a header-trailing comment before the `{`, a
        // cuddled one, an own-line body comment, inter-arm and arm-trailing match
        // comments, doc comments on members, and a declaration header comment — every
        // spelling must format to one fixed point that preserves the comment.
        "module app\nfn run(n: int) {\n    if n < 0 // note\n    {\n        return\n    }\n}\n",
        "module app\nfn run(n: int) {\n    while n < 0 { // w\n        n = n\n    }\n}\n",
        "module app\nfn run(n: int) {\n    transaction // t\n    {\n        n = n\n    }\n}\n",
        "module app\nfn run(n: int) {\n    match n {\n        // leading\n        dot => return // arm\n        // between\n        circle => {\n            return\n        }\n    }\n}\n",
        "module app\nresource B // r\n{\n    /// the title\n    t: string\n}\n",
        "module app\nfn run() // fn\n{\n    return\n}\n",
        // Unclosed and stray-brace forms: a member loop must terminate on these.
        "module app\nresource B {\n    t: string\n",
        "module app\nenum E {\n    a\n    b\n",
        "module app\nresource B {\n    a{b\n}\n",
        "module app\nresource B {\n",
        "module app\nstore ^books[id: int]: B {\n    index byT[t]\n",
        "module app\nfn run() {\n    match s {\n        dot =>\n    }\n",
        "module app\nfn a() {\n    }\n}\nfn b() {\n    return\n}\n",
        "module app\nfn run() {\n    if const a = x and{\n        return\n    }\n}\n",
        "module app\nenum E {\n    a {\n        b {\n            c\n",
        "module app\nresource B {\n    x[k: int]: string\n    g[j: int] {\n        y: int\n",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// The full total-invariant oracle under a wall-clock bound: parsing returns
/// promptly (so a member loop that fails to terminate on a missing `}` is a test
/// failure, not a hang), is deterministic, tiles the source losslessly, recovers with
/// a bounded number of well-spanned diagnostics, and — for a clean parse — is a
/// formatter fixed point that re-parses cleanly. The formatter leg now asserts
/// idempotence unconditionally over comments (MSY01), so a comment-bearing clean
/// parse is held to the same fixed-point contract as a comment-free one. The parse
/// runs on a large stack so a deep mutated input fails closed at the nesting limit
/// rather than overflowing.
fn assert_bounded_recovery(source: &str) {
    const WORKER_STACK_BYTES: usize = 256 * 1024 * 1024;
    let owned = source.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::Builder::new()
        .stack_size(WORKER_STACK_BYTES)
        .spawn(move || {
            assert_total_invariants(&owned);
            let _ = tx.send(());
        })
        .expect("spawn brace-grammar fuzz worker");
    match rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(()) => {
            if let Err(panic) = worker.join() {
                std::panic::resume_unwind(panic);
            }
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("parsing did not terminate within 10s for {source:?}");
        }
        // The worker dropped its sender without a result: it panicked inside an
        // invariant assertion. Re-raise that panic with its message.
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => match worker.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(()) => panic!("brace-grammar fuzz worker exited without a result for {source:?}"),
        },
    }
}

/// The brace-grammar corpus and a small seeded mutation pass hold the oracle
/// invariants under a per-iteration wall-clock bound, so a member loop that fails to
/// terminate on a missing `}` is a test failure rather than a hung suite. Each
/// cleanly-parsing corpus entry additionally holds the faithful lens (comment- and
/// structure-preserving), pinning the MSY01 comment-ownership fix.
#[test]
fn brace_grammar_corpus_holds_the_oracle_invariants_without_hanging() {
    for source in brace_grammar_corpus() {
        assert_bounded_recovery(&source);
        if !marrow_syntax::parse_source(&source).has_errors() {
            assert_formatter_faithful(&source);
        }
    }

    // A seeded, fixed-iteration mutation pass over the brace corpus, bounded to a few
    // hundred iterations so a failure reproduces exactly and CI stays bounded.
    const SEED: u64 = 0x4252_4143_455f_465a; // "BRACE_FZ"
    const ITERATIONS: usize = 300;
    const MAX_MUTATIONS: usize = 24;
    let seeds: Vec<Vec<u8>> = brace_grammar_corpus()
        .into_iter()
        .map(String::into_bytes)
        .collect();
    let mut rng = SplitMix64::new(SEED);
    for _ in 0..ITERATIONS {
        let mut bytes = seeds[rng.below(seeds.len() as u64) as usize].clone();
        let rounds = 1 + rng.below(MAX_MUTATIONS as u64) as usize;
        for _ in 0..rounds {
            mutate(&mut bytes, &mut rng);
        }
        let source = String::from_utf8_lossy(&bytes);
        assert_bounded_recovery(&source);
    }
}

/// Bytes chosen to stress the lexer and parser: string and interpolation
/// delimiters, layout, comment and path punctuation, an invalid-UTF-8 lead byte,
/// and NUL.
const INTERESTING: &[u8] = &[
    0x00, 0xff, b'"', b'\\', b'{', b'}', b'(', b')', b'\n', b'\t', b' ', b';', b':', b'^', b'.',
    b'=', b'$', b'+', b'-', b'<', b'>', b'~', b'?', b'a', b'1',
];

fn mutate(bytes: &mut Vec<u8>, rng: &mut SplitMix64) {
    if bytes.is_empty() {
        bytes.push(INTERESTING[rng.below(INTERESTING.len() as u64) as usize]);
        return;
    }
    match rng.below(6) {
        // Flip: replace a byte with an interesting one.
        0 => {
            let at = rng.below(bytes.len() as u64) as usize;
            bytes[at] = INTERESTING[rng.below(INTERESTING.len() as u64) as usize];
        }
        // Insert an interesting byte.
        1 => {
            let at = rng.below(bytes.len() as u64 + 1) as usize;
            bytes.insert(
                at,
                INTERESTING[rng.below(INTERESTING.len() as u64) as usize],
            );
        }
        // Delete a byte.
        2 => {
            let at = rng.below(bytes.len() as u64) as usize;
            bytes.remove(at);
        }
        // Truncate to a random length.
        3 => {
            let len = rng.below(bytes.len() as u64) as usize;
            bytes.truncate(len);
        }
        // Duplicate a bounded slice, so a construct can nest or repeat.
        4 => {
            let start = rng.below(bytes.len() as u64) as usize;
            let span = 1 + rng.below((bytes.len() - start).min(32) as u64) as usize;
            let slice = bytes[start..start + span].to_vec();
            let at = rng.below(bytes.len() as u64 + 1) as usize;
            for (offset, byte) in slice.into_iter().enumerate() {
                bytes.insert(at + offset, byte);
            }
        }
        // Xor a bit, reaching non-interesting bytes and invalid UTF-8.
        _ => {
            let at = rng.below(bytes.len() as u64) as usize;
            bytes[at] ^= 1 << (rng.below(8) as u32);
        }
    }
}

/// A tiny seeded PRNG (SplitMix64): deterministic and dependency-free, so the fuzz
/// pass reproduces exactly from its seed.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..bound`, or `0` when `bound` is zero.
    fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        self.next_u64() % bound
    }
}
