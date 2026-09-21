//! The source-bytes fuzz driver: a thin input adapter over the bounded oracle in
//! `common::oracle`. It carries no invariants of its own — it feeds the oracle a
//! deterministic corpus and a seeded, fixed-iteration mutation pass over it.
//!
//! Arbitrary bytes reach the `&str`-typed front end through `String::from_utf8_lossy`,
//! the total decode the file boundary uses, so invalid UTF-8 and NUL bytes are
//! exercised as the replacement-bearing text the parser actually sees. Fixed seeds, a
//! fixed iteration budget, and a small interesting-byte alphabet keep the pass bounded
//! and exactly reproducible, so nothing unbounded runs in CI.

use crate::common;
use crate::common::CompletePayload;
use crate::common::oracle::{
    OVER_DEEP, assert_formatter_faithful, assert_total_invariants, has_error_diagnostic,
};
use marrow_syntax::{
    Block, Declaration, Expression, InterpolationPart, ParsedSource, Severity, SourceFile,
    Statement, parse_source,
};

/// The tracer-subset constructs, each a small complete or near-complete program.
/// These are valid programs, so they also feed the faithful-formatter lens.
fn tracer_subset_programs() -> Vec<String> {
    [
        "module app\n",
        "module shelf::books\n\nuse shelf::books\nuse std::clock\n",
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
        "module app\n\nfn timing(): duration {\n    return 3 days\n}\n",
        "module app\n\nfn counting(n: int): int {\n    var s = 0\n    for i in 1..=n {\n        s += i\n    }\n    return s\n}\n",
        "module app\n\nfn within(x: int): bool {\n    return x in 0..10\n}\n",
        "module app\n\nfn without(x: int): bool {\n    return x not in 0..=100\n}\n",
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

/// Minimized counterexamples whose contract is the lossless token tiling and the total
/// invariants, not formatter faithfulness. A comment leader inside a string-
/// interpolation hole must stop at the hole boundary rather than run to the physical
/// line end, past the hole, overlapping the interpolation-close tokens and breaking the
/// tiling.
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

/// Minimized counterexamples that are valid programs, pinned through the faithful
/// lens: idempotent, comment-preserving, structure-preserving formatting.
///
/// The empty-body shapes are the hard cases — a body-bearing header must join its
/// empty body through the one empty-body guard, or the dangling newline it leaves
/// makes the block-level blank accounting grow a source blank 1 -> 2 per format.
fn formatter_faithful_regressions() -> Vec<String> {
    [
        "module app\n\nfn f() {\n    match s {\n        d => {}\n    }\n\n    b\n}\n",
        "module app\n\nfn f() {\n    if x {}\n\n    b\n}\n",
        "module app\n\nfn f() {\n    while x {}\n\n    b\n}\n",
        "module app\n\nfn f() {\n    for i in xs {}\n\n    b\n}\n",
        "module app\n\nfn f() {\n    transaction {}\n\n    b\n}\n",
        "module app\n\nfn f(o: Option<int>): int {\n    match o {\n        some(v) => return v\n        none => return 0\n    }\n}\n",
        // A terminal `else if` (no trailing `else`) must keep the braces around its
        // diverging then-branch: `} else if n > 0 return` does not re-parse.
        "module app\n\nfn f(n: int) {\n    if n < 0 {\n        return\n    } else if n > 0 {\n        return\n    }\n}\n",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// The valid-program corpus: the tracer-subset programs, the documented source
/// blocks, and the tracked `.mw` fixtures that parse cleanly. Each feeds both the
/// total and the faithful-formatter lens.
fn valid_programs() -> Vec<String> {
    let mut programs = tracer_subset_programs();
    for block in common::documented_source_blocks() {
        programs.push(block.source);
    }
    programs
}

/// The parser's nesting limit is calibrated for the 256 MB stack the CLI runs it on
/// (`WORKER_STACK_BYTES` in the `marrow` binary), where a 256-deep parse fits but the
/// small default test-thread stack does not. Each driver body runs on a matching
/// worker stack so the oracle exercises the production environment.
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

/// Every layout the parser admits beside the one the formatter writes: a `{` on the line
/// after its header, an `else` beginning its own line, and an inline `on more` arm. Each
/// formats to the canonical layout and is a fixed point from there.
fn accepted_layouts() -> Vec<String> {
    [
        "module app\nfn run(a: bool): int {\n    if a\n    {\n        return 1\n    }\n    return 0\n}\n",
        "module app\nfn run(a: bool): int {\n    if a {\n        return 1\n    }\n    else {\n        return 0\n    }\n}\n",
        "module app\nfn run(a: bool): int {\n    if a {\n        return 1\n    } else\n        return 0\n}\n",
        "module app\nfn run(): int {\n    for a in b at most 8 {\n        return 1\n    } on more return 2\n    return 0\n}\n",
        "module app\nfn run()\n{\n    return 0\n}\n",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn deterministic_corpus_body() {
    let mut saw_error = false;
    let mut saw_over_deep = false;

    // Valid programs: total invariants plus the faithful-formatter contract.
    for source in valid_programs()
        .into_iter()
        .chain(formatter_faithful_regressions())
        .chain(accepted_layouts())
    {
        assert_total_invariants(&source);
        assert_formatter_faithful(&source);
    }

    // Tracked shared-syntax fixtures: total invariants always; the faithful lens only
    // over those that parse cleanly, since a legacy construct's rendering is not a
    // contract.
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
            .complete()
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_syntax::NESTING_LIMIT);
    }

    // Every char-boundary truncation is a distinct partially-written source.
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
    // A fixed panel of diverse seeds runs by default, so CI is not green by the luck of
    // a single seed. The panel is bounded and reproducible — fixed seeds, a fixed
    // per-seed budget, a total under a couple of seconds. MARROW_FUZZ_SEED replaces the
    // panel with one seed at a wider budget to extend a search without editing code.
    const PANEL_SEEDS: [u64; 4] = [
        0x5241_4d5f_4655_5a5a, // "RAM_FUZZ"
        13,                    // minimized the terminal-`else if` braceless then-branch
        0x9E37_79B9_7F4A_7C15, // the SplitMix64 golden-ratio increment
        0xD1B5_4A32_D192_ED03,
    ];
    const PANEL_ITERATIONS: usize = 1_500;
    const OVERRIDE_ITERATIONS: usize = 4_000;
    const MAX_MUTATIONS: usize = 24;

    let (panel, iterations): (Vec<u64>, usize) = match std::env::var("MARROW_FUZZ_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
    {
        Some(seed) => (vec![seed], OVERRIDE_ITERATIONS),
        None => (PANEL_SEEDS.to_vec(), PANEL_ITERATIONS),
    };

    let seeds: Vec<Vec<u8>> = tracer_subset_programs()
        .into_iter()
        .chain(pathological_inputs())
        .map(String::into_bytes)
        .collect();

    let mut mutated_error = false;
    for panel_seed in panel {
        let mut rng = SplitMix64::new(panel_seed);
        for _ in 0..iterations {
            let mut bytes = seeds[rng.below(seeds.len() as u64) as usize].clone();
            let rounds = 1 + rng.below(MAX_MUTATIONS as u64) as usize;
            for _ in 0..rounds {
                mutate(&mut bytes, &mut rng);
            }
            // Exactly the decode the file boundary performs, so invalid UTF-8 and NUL
            // bytes are covered.
            let source = String::from_utf8_lossy(&bytes);
            assert_total_invariants(&source);
            mutated_error |= has_error_diagnostic(&source);
        }
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

/// The brace-grammar fuzz corpus: declarations with `{ … }` bodies, `=>` match arms,
/// `//` and `///` comments, `\u{}` escapes, bracket key groups, and angle generics,
/// including the unclosed and stray-brace forms a member loop must survive.
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
        // Comment-bearing seeds: every admitted comment spelling must format to one
        // fixed point that preserves the comment.
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

/// The full total-invariant oracle under a 10s wall-clock bound, so a member loop that
/// fails to terminate on a missing `}` is a test failure rather than a hang. The parse
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
        // invariant assertion, so re-raise that panic with its message.
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => match worker.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(()) => panic!("brace-grammar fuzz worker exited without a result for {source:?}"),
        },
    }
}

/// Each cleanly-parsing brace-corpus entry additionally holds the faithful lens,
/// pinning the comment-ownership contract over braces.
#[test]
fn brace_grammar_corpus_holds_the_oracle_invariants_without_hanging() {
    for source in brace_grammar_corpus() {
        assert_bounded_recovery(&source);
        if !marrow_syntax::parse_source(&source).has_errors() {
            assert_formatter_faithful(&source);
        }
    }

    // Bounded to a few hundred iterations so CI stays bounded and a failure reproduces
    // exactly from its seed.
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

/// Bytes chosen to stress the lexer and parser: string and interpolation delimiters,
/// block and key brackets, the `//`/`///` comment and `=>` arm leaders, path and
/// generic punctuation, an invalid-UTF-8 lead byte, and NUL. `/`, `[`, and `]` let the
/// insert path synthesize comment leaders and key groups from any seed.
const INTERESTING: &[u8] = &[
    0x00, 0xff, b'"', b'\\', b'{', b'}', b'(', b')', b'[', b']', b'\n', b'\t', b' ', b';', b':',
    b'^', b'.', b'=', b'$', b'+', b'-', b'/', b'<', b'>', b'~', b'?', b'a', b'1',
];

fn mutate(bytes: &mut Vec<u8>, rng: &mut SplitMix64) {
    if bytes.is_empty() {
        bytes.push(INTERESTING[rng.below(INTERESTING.len() as u64) as usize]);
        return;
    }
    // Flip, insert, delete, truncate, duplicate a bounded slice (so a construct can
    // nest or repeat), and xor a bit (reaching non-interesting bytes and invalid UTF-8).
    match rng.below(6) {
        0 => {
            let at = rng.below(bytes.len() as u64) as usize;
            bytes[at] = INTERESTING[rng.below(INTERESTING.len() as u64) as usize];
        }
        1 => {
            let at = rng.below(bytes.len() as u64 + 1) as usize;
            bytes.insert(
                at,
                INTERESTING[rng.below(INTERESTING.len() as u64) as usize],
            );
        }
        2 => {
            let at = rng.below(bytes.len() as u64) as usize;
            bytes.remove(at);
        }
        3 => {
            let len = rng.below(bytes.len() as u64) as usize;
            bytes.truncate(len);
        }
        4 => {
            let start = rng.below(bytes.len() as u64) as usize;
            let span = 1 + rng.below((bytes.len() - start).min(32) as u64) as usize;
            let slice = bytes[start..start + span].to_vec();
            let at = rng.below(bytes.len() as u64 + 1) as usize;
            for (offset, byte) in slice.into_iter().enumerate() {
                bytes.insert(at + offset, byte);
            }
        }
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

/// Whether any node in the tree is the parser's error placeholder. Every parse yields
/// a node — a failure is an `Expression::Error`/`Statement::Error` carrying its span.
fn expr_has_error(expr: &Expression) -> bool {
    match expr {
        Expression::Error { .. } => true,
        Expression::Call { callee, args, .. } => {
            expr_has_error(callee) || args.iter().any(|arg| expr_has_error(&arg.value))
        }
        Expression::Keyed { base, keys, .. } => {
            expr_has_error(base) || keys.iter().any(expr_has_error)
        }
        Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
            expr_has_error(base)
        }
        Expression::Unary { operand, .. } => expr_has_error(operand),
        Expression::Try { inner, .. } => expr_has_error(inner),
        Expression::Binary { operands, .. } => {
            expr_has_error(&operands.left) || expr_has_error(&operands.right)
        }
        Expression::Range {
            start, end, step, ..
        } => [start, end, step]
            .into_iter()
            .flatten()
            .any(|part| expr_has_error(part)),
        Expression::Membership { value, range, .. } => {
            expr_has_error(value) || expr_has_error(range)
        }
        Expression::Interpolation { parts, .. } => parts.iter().any(|part| match part {
            InterpolationPart::Expr(inner) => expr_has_error(inner),
            InterpolationPart::Text { .. } => false,
        }),
        Expression::Literal { .. }
        | Expression::Name { .. }
        | Expression::SavedRoot { .. }
        | Expression::Absent { .. } => false,
    }
}

fn block_has_error(block: &Block) -> bool {
    block.statements.iter().any(stmt_has_error)
}

fn stmt_has_error(stmt: &Statement) -> bool {
    match stmt {
        Statement::Error { .. } => true,
        Statement::Const { value, .. }
        | Statement::Assert { value, .. }
        | Statement::Expr { value, .. } => expr_has_error(value),
        Statement::Var { value, .. } | Statement::Return { value, .. } => {
            value.as_ref().is_some_and(expr_has_error)
        }
        Statement::Assign { target, value, .. }
        | Statement::CompoundAssign { target, value, .. } => {
            expr_has_error(target) || expr_has_error(value)
        }
        Statement::Delete { path, .. } => expr_has_error(path),
        Statement::PlaceBinding { place, .. } => expr_has_error(place),
        Statement::Unset { place, .. } => expr_has_error(place),
        Statement::If {
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            expr_has_error(condition)
                || block_has_error(then_block)
                || else_ifs.iter().any(|else_if| {
                    expr_has_error(&else_if.condition) || block_has_error(&else_if.block)
                })
                || else_block.as_ref().is_some_and(block_has_error)
        }
        Statement::IfConst {
            value,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            expr_has_error(value)
                || block_has_error(then_block)
                || else_ifs.iter().any(|else_if| {
                    expr_has_error(&else_if.condition) || block_has_error(&else_if.block)
                })
                || else_block.as_ref().is_some_and(block_has_error)
        }
        Statement::While {
            condition, body, ..
        } => expr_has_error(condition) || block_has_error(body),
        Statement::For {
            iterable,
            step,
            bound,
            body,
            ..
        } => {
            expr_has_error(iterable)
                || step.as_ref().is_some_and(expr_has_error)
                || bound.as_ref().is_some_and(|bound| {
                    expr_has_error(&bound.limit)
                        || bound.from.as_ref().is_some_and(expr_has_error)
                        || bound.on_more.as_ref().is_some_and(block_has_error)
                })
                || block_has_error(body)
        }
        Statement::Transaction { body, .. } => block_has_error(body),
        Statement::Match {
            scrutinee, arms, ..
        } => expr_has_error(scrutinee) || arms.iter().any(|arm| block_has_error(&arm.block)),
        Statement::Checked {
            op,
            out_of_range,
            zero_divisor,
            ..
        } => {
            expr_has_error(op)
                || [out_of_range, zero_divisor]
                    .into_iter()
                    .flatten()
                    .any(block_has_error)
        }
        Statement::IfConstChain {
            bindings,
            condition,
            then_block,
            else_ifs,
            else_block,
            ..
        } => {
            bindings
                .iter()
                .any(|binding| expr_has_error(&binding.value))
                || condition.as_ref().is_some_and(expr_has_error)
                || block_has_error(then_block)
                || else_ifs.iter().any(|else_if| {
                    expr_has_error(&else_if.condition) || block_has_error(&else_if.block)
                })
                || else_block.as_ref().is_some_and(block_has_error)
        }
        Statement::LetElse {
            value, else_block, ..
        } => expr_has_error(value) || block_has_error(else_block),
        Statement::Require {
            condition, value, ..
        } => expr_has_error(condition) || expr_has_error(value),
        Statement::Break { .. } | Statement::Continue { .. } => false,
    }
}

fn file_has_error(file: &SourceFile) -> bool {
    file.declarations
        .iter()
        .any(|declaration| match declaration {
            Declaration::Function(function) => block_has_error(&function.body),
            Declaration::Const(decl) => decl.value.as_ref().is_some_and(expr_has_error),
            _ => false,
        })
}

/// A well-formed program never yields the error placeholder: the documented
/// library parses to a tree with no error nodes and no diagnostics.
#[test]
fn valid_programs_yield_no_error_nodes() {
    for block in common::documented_source_blocks() {
        let parsed = parse_source(&block.source);
        assert!(
            !parsed.has_errors(),
            "documented block {} should parse cleanly: {:#?}",
            block.path,
            parsed.diagnostics
        );
        assert!(
            !file_has_error(&parsed.file),
            "documented block {} should hold no error nodes",
            block.path
        );
    }
}

/// Every prefix of every documented library parses without panicking, and any
/// error node it produces travels with a diagnostic. This is the soundness
/// foundation of the `has_errors` gate: an error node can never reach a downstream
/// crate that trusts a clean `has_errors` to mean a fully structured tree.
#[test]
fn every_error_node_travels_with_a_diagnostic() {
    let mut malformed_seen = false;
    let mut check = |source: &str, label: &str| {
        let ParsedSource { file, diagnostics } = parse_source(source);
        if file_has_error(&file) {
            malformed_seen = true;
            assert!(
                diagnostics
                    .complete()
                    .iter()
                    .any(|diagnostic| diagnostic.severity == Severity::Error),
                "an error node appeared with no diagnostic for {label}: {source:?}",
            );
        }
    };
    for block in common::documented_source_blocks() {
        // A truncation ending inside a block leaves it unclosed, which the parser
        // reports at the open delimiter with an empty body rather than an error node.
        for end in char_boundaries(&block.source) {
            check(&block.source[..end], &block.path);
        }
    }
    // Balanced bodies with a malformed interior statement: the error nodes this
    // property guards.
    for program in MALFORMED_BALANCED_PROGRAMS {
        check(program, "malformed-balanced program");
    }
    // A property that never exercised a single error node would be vacuous.
    assert!(
        malformed_seen,
        "expected a malformed program to produce an error node"
    );
}

/// Syntactically balanced programs whose interior does not structure: each parses to a
/// tree carrying an error node beside its diagnostic.
const MALFORMED_BALANCED_PROGRAMS: &[&str] = &[
    "pub fn f(): int {\n    const x = \n}\n",
    "pub fn f() {\n    @ \n}\n",
    "pub fn f() {\n    return 1 +\n}\n",
];
