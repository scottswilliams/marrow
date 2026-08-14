//! The issuance gate's memory half: a hostile maximum-amplification corpus is driven
//! through the production `compile` path in a subprocess and that subprocess's peak
//! resident set size is measured, so the wide carriers' width derivation
//! (`marrow-image::issuance`) is joined by a measured memory-feasibility figure rather
//! than an unmeasured assumption.
//!
//! Four corpora are measured, each a divergent generic at the widest body its own bound
//! admits: a `MAX_RECORD_FIELDS` generic struct, a `MAX_VARIANTS` x `MAX_PAYLOAD_FIELDS`
//! generic enum, a generic function filling its local frame, and all three together. Each
//! of the 4,096 instantiations the shared bound admits carries that body, and every
//! monomorphic body stays simultaneously retained because a public image-policy crossing
//! does not stop provisional construction, so the measured figures charge draft rows,
//! lookup indexes, the journal and policy ledger, and the live compiler diagnostic and
//! analysis-fact transients together.
//!
//! Each width is read from the bound that governs the construct it widens. The gate
//! previously read `MAX_STRUCT_LEAVES` for all of them — a durable value-shape bound whose
//! own declaration says it does not scale with the record width — and so measured bodies
//! sixty-four times narrower than the compiler admits. At the correct widths two of the
//! four corpora peak above the declared owned-heap ceiling; the verdicts are recorded
//! below.
//!
//! Measurement uses only what the platform already publishes about a process:
//! `/proc/self/status`'s `VmHWM` where it exists, and the base-system `/usr/bin/time`
//! reporter otherwise. Neither adds a dependency and neither needs `unsafe`. A
//! platform whose peak is not obtainable fails the gate loudly rather than passing
//! without a figure.

#[path = "common/owned_heap.rs"]
mod owned_heap;

use std::process::Command;

use marrow_compile::{CompileFailure, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

/// The widest admissible record declaration, read from the owner that fixes it rather
/// than hand-copied: a bound change must move the corpus with it, not leave it describing
/// a body the compiler no longer calls maximal.
///
/// This is `MAX_RECORD_FIELDS`, not `MAX_STRUCT_LEAVES`. The two are different bounds over
/// different subjects, and the owner says so at its own declaration: a dense inline
/// composite's leaf count is a value shape, not a record's field set, and does NOT scale
/// with `MAX_RECORD_FIELDS`. A generic `struct` template is a record declaration, so its
/// width is governed by the record bound; the corpus previously read the value-shape bound
/// and was sixty-four times narrower than the widest body the compiler admits, which is
/// not the hostile maximum it claimed to measure.
const ADMITTED_RECORD_FIELDS: usize = marrow_image::bounds::MAX_RECORD_FIELDS;

/// The widest admissible function body, by the bound that actually governs one: a
/// function's local slots. The function arm declares one local per step, so this is the
/// owner its width has to be read from — the record-field bound does not reach here, and
/// the value-shape bound never did.
///
/// Two slots of the frame are spent before the steps are: the parameter and the `xs`
/// accumulator the steps read. The corpus takes the rest, so the frame is full and one
/// more step would be refused by the frame bound instead of the instantiation ceiling —
/// which is the widest body that still measures what this gate exists to measure.
const ADMITTED_LOCALS: usize = marrow_image::bounds::MAX_LOCALS - 2;

/// The widest admissible function body by the *other* bound that governs one: the bytes of
/// compiled code a single function admits.
///
/// The frame width and the code width are independent — a body can fill all 256 local slots
/// and still carry a small fraction of the 64 KiB of code a function admits — and each
/// generic instance retains a copy of both. Measuring only the frame therefore measured one
/// of the two dimensions.
///
/// The number is **observed, not computed**: it is the largest padding whose body still
/// encodes, and one more statement is refused with the typed `CodeBytes` limit.
/// `the_function_arm_sits_exactly_at_the_code_byte_envelope` re-observes both halves of
/// that boundary on the arm's own generator, so this constant cannot drift away from the
/// bound it claims to sit at.
const ADMITTED_CODE_PADDING: usize = 6145;

/// The ceiling a hostile maximum-amplification compile is held under: the repository's
/// declared owned-heap authority, read from its owner.
///
/// It is deliberately **not** derived from this corpus's own measured peak. A gate that
/// sets its ceiling from its own sample proves only that the implementation equals
/// itself, and it can never fail: every regression simply becomes the new authority. If
/// an honest measurement exceeds this number, that is a finding about the compiler to be
/// recorded and adjudicated — not a reason to raise the number.
const MAX_HOSTILE_COMPILE_RSS_BYTES: u64 = owned_heap::H_OWNED_BYTES;

/// The type-amplification arm: a divergent generic type over the largest admissible
/// body, reached through a generic function's return annotation.
///
/// The annotation is what makes the arm live. A generic type is monomorphized only on
/// *use*, so declaring `Grow<T>` and never naming it in a position that resolves it
/// leaves the arm dead — the corpus compiles, the gate passes, and no generic type row is
/// ever built. Naming it as `deepen`'s return type resolves it once per instance, and
/// `next: Grow<List<T>>` is what makes each resolution demand the next one.
fn type_amplification_arm() -> String {
    let mut source = String::from("struct Grow<T> {\n");
    for leaf in 0..ADMITTED_RECORD_FIELDS - 1 {
        source.push_str(&format!("    leaf{leaf}: T\n"));
    }
    source.push_str("    next: Grow<List<T>>\n}\n\n");
    source.push_str("fn deepen<T>(x: T): Grow<T> {\n    return deepen(x)\n}\n\n");
    source
}

/// The widest admissible enum body: every variant a closed enum admits, each carrying
/// every payload leaf a variant admits.
const ADMITTED_VARIANTS: usize = marrow_image::bounds::MAX_VARIANTS;
const ADMITTED_PAYLOAD_FIELDS: usize = marrow_image::bounds::MAX_PAYLOAD_FIELDS;

/// The enum-amplification arm: a divergent generic enum at the widest admissible variant
/// and payload width.
///
/// This arm exists because the struct arm does not reach the enum template copy. A fill
/// copies its template body out before resolving it, and the two bodies are separate code
/// paths over separately bounded populations: a struct fill copies `MAX_RECORD_FIELDS`
/// declared fields, an enum fill copies `MAX_VARIANTS` variants each of
/// `MAX_PAYLOAD_FIELDS` leaves. The enum copy is the larger of the two per instantiation
/// and had no corpus at all, so its cost was asserted by the comment beside it rather than
/// measured.
fn enum_amplification_arm() -> String {
    let mut source = String::from("struct Wrap<T> {\n    inner: T\n}\n\n");
    source.push_str("enum Grown<T> {\n");
    for variant in 0..ADMITTED_VARIANTS - 1 {
        let payload: Vec<String> = (0..ADMITTED_PAYLOAD_FIELDS)
            .map(|leaf| format!("p{leaf}: T"))
            .collect();
        source.push_str(&format!("    v{variant}({})\n", payload.join(", ")));
    }
    // The recursive variant carries the full payload width like every other: one leaf is
    // the recursion that mints the next instance, and the rest are ordinary payload. A
    // final variant one leaf wide would leave the widest admitted enum body unmeasured at
    // exactly the variant whose resolution drives the amplification.
    let mut tail: Vec<String> = (0..ADMITTED_PAYLOAD_FIELDS - 1)
        .map(|leaf| format!("p{leaf}: T"))
        .collect();
    tail.push("n: Grown<Wrap<T>>".to_string());
    source.push_str(&format!("    next({})\n}}\n\n", tail.join(", ")));
    source.push_str("fn sprout<T>(x: T): Grown<T> {\n    return sprout(x)\n}\n\n");
    source
}

/// A project holding only the enum-amplification arm.
fn enum_only_corpus() -> String {
    format!(
        "module main\n\n{}pub fn driver(): int {{\n    const ignored = sprout(1)\n    return 0\n}}\n",
        enum_amplification_arm(),
    )
}

/// The function-amplification arm: a divergent generic function whose per-instance body
/// and span shape amplify, diverging on an ever-growing argument.
fn function_amplification_arm() -> String {
    let mut source = String::from("fn grow<T>(x: T): int {\n    var xs: List<T> = List()\n");
    for step in 0..ADMITTED_LOCALS {
        source.push_str(&format!("    var step{step}: List<T> = xs\n"));
        source.push_str(&format!("    xs = append(step{step}, x)\n"));
    }
    // The frame width and the code width are independent dimensions: a body may fill the
    // local frame and still carry a fraction of the code a function admits, and each
    // instance retains a copy of the code as well as of the frame. Padding to the
    // code-byte envelope is what drives the second dimension.
    for _ in 0..ADMITTED_CODE_PADDING {
        source.push_str("    xs = append(xs, x)\n");
    }
    source.push_str("    return grow(xs)\n}\n\n");
    source
}

/// The generic function arm's body with its divergence removed, so it reaches the encoder
/// that owns the code-byte bound. Derived from the arm's own generator by substitution, so
/// the two cannot drift into different bodies.
fn code_envelope_mirror(pad: usize) -> String {
    let mut arm = String::from("fn grow<T>(x: T): int {\n    var xs: List<T> = List()\n");
    for step in 0..ADMITTED_LOCALS {
        arm.push_str(&format!("    var step{step}: List<T> = xs\n"));
        arm.push_str(&format!("    xs = append(step{step}, x)\n"));
    }
    for _ in 0..pad {
        arm.push_str("    xs = append(xs, x)\n");
    }
    arm.push_str("    return grow(xs)\n}\n\n");
    // The divergence is replaced by a call of the same shape rather than removed: a
    // monomorphic self-call is a refused recursion cycle, and dropping the call instead
    // would measure a body one call instruction narrower than the arm's.
    let body = arm
        .replace("fn grow<T>(x: T): int", "fn grow(x: int): int")
        .replace("List<T>", "List<int>")
        .replace("    return grow(xs)\n", "    return settle(xs)\n");
    format!(
        "module main\n\nfn settle(v: List<int>): int {{\n    return 0\n}}\n\n{body}\
         pub fn driver(): int {{\n    return grow(1)\n}}\n"
    )
}

/// A project holding only the type-amplification arm.
fn type_only_corpus() -> String {
    format!(
        "module main\n\n{}pub fn driver(): int {{\n    const ignored = deepen(1)\n    return 0\n}}\n",
        type_amplification_arm(),
    )
}

/// A project holding only the function-amplification arm.
fn function_only_corpus() -> String {
    format!(
        "module main\n\n{}pub fn driver(): int {{\n    return grow(1)\n}}\n",
        function_amplification_arm(),
    )
}

/// The hostile maximum-amplification project: every arm in one project, so the measured
/// figure charges generic type rows, generic enum rows, and generic function rows
/// together, along with the
/// draft rows, lookup indexes, journal, policy ledger, and the live compiler diagnostic
/// and analysis-fact transients.
///
/// Type and function instances share **one** ceiling
/// (`type_insts.len() + fn_insts.len() >= MAX_INSTANTIATIONS`), so the two arms do not
/// each reach it — together they saturate it. That is the maximum this compiler admits,
/// and claiming two independent 4,096-row populations would overstate it.
fn hostile_corpus() -> String {
    format!(
        "module main\n\n{}{}{}pub fn driver(): int {{\n    const ignored = deepen(1)\n             const grown = sprout(1)\n    return grow(1)\n}}\n",
        type_amplification_arm(),
        enum_amplification_arm(),
        function_amplification_arm(),
    )
}

fn project(source: &str) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
        .expect("capture the hostile corpus")
}

/// This process's peak resident set size, where the platform publishes it to the
/// process itself. `None` means the outer half must measure from outside.
fn self_peak_rss_bytes() -> Option<u64> {
    if cfg!(target_os = "linux") {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
        let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        return Some(kilobytes * 1024);
    }
    None
}

/// The peak reported by the base-system `/usr/bin/time` reporter, in bytes.
///
/// The two reporters disagree on unit: the BSD reporter prints bytes and the GNU one
/// prints kilobytes, and each says so in its own line, so the unit is read from the
/// line rather than assumed from the platform.
fn reported_peak_rss_bytes(report: &str) -> Option<u64> {
    let line = report.lines().find(|line| {
        line.to_ascii_lowercase()
            .contains("maximum resident set size")
    })?;
    let kilobytes = line.to_ascii_lowercase().contains("kbytes");
    let digits: String = line.chars().filter(|c| c.is_ascii_digit()).collect();
    let value: u64 = digits.parse().ok()?;
    Some(if kilobytes { value * 1024 } else { value })
}

/// The corpus the subprocess compiles, named by the outer half through the environment.
const CORPUS_SELECTOR: &str = "MARROW_ISSUANCE_RSS_CORPUS";

/// The three corpora the gate measures, each named so the subprocess can be told which
/// one to build.
fn corpus_by_name(name: &str) -> String {
    match name {
        "type" => type_only_corpus(),
        "enum" => enum_only_corpus(),
        "function" => function_only_corpus(),
        "both" => hostile_corpus(),
        other => panic!("unknown corpus `{other}`"),
    }
}

/// The subprocess half: drive the named corpus through the production compile path and,
/// where the platform allows it, report this process's own peak. Run only by the outer
/// test.
#[test]
#[ignore = "the subprocess half of the hostile-amplification RSS gate"]
fn inner_hostile_amplification_compile() {
    let name = std::env::var(CORPUS_SELECTOR).unwrap_or_else(|_| "both".to_string());
    let source = corpus_by_name(&name);
    let outcome = compile(&project(&source));
    // The corpus is hostile, not malformed: it exhausts the shared instantiation bound
    // and that exhaustion is a source diagnostic, so the whole provisional population
    // really was constructed before the refusal — which is the population being
    // measured.
    match outcome {
        Err(CompileFailure::Diagnostics(diagnostics)) => assert!(
            diagnostics
                .iter()
                .any(|row| row.code() == "check.instantiation_limit"),
            "the hostile corpus exhausts the shared instantiation bound: {diagnostics:#?}",
        ),
        other => panic!("the hostile corpus must refuse as a source diagnostic: {other:?}"),
    }
    if let Some(peak) = self_peak_rss_bytes() {
        println!("PEAK_RSS_BYTES={peak}");
    }
}

/// **The recorded verdict per corpus: whether its measured peak is under the declared
/// owned-heap ceiling.** Two of the four are not, and that is a finding about the
/// compiler rather than a ceiling to raise.
///
/// Measured on this host — aarch64 macOS 25.5, the workspace's pinned toolchain, `cargo
/// test` at the default `dev` profile, one subprocess per corpus, peak read from the base
/// system reporter. A `release` run reproduces every figure inside one percent, so the
/// peak is the retained population and not the profile:
///
/// ```text
/// corpus     peak RSS         vs. 640 MiB ceiling   what it drives
/// type         272,302,080 B   0.41x  under         MAX_RECORD_FIELDS fields per fill,
///                                                   256 deep (the mint-depth bound)
/// enum       1,075,691,520 B   1.60x  OVER          MAX_VARIANTS x MAX_PAYLOAD_FIELDS per
///                                                   fill, 256 deep
/// function   9,799,499,776 B  14.60x  OVER          MAX_LOCALS-2 locals AND the 64-KiB
///                                                   code envelope, 4096 instances
/// both         292,864,000 B   0.44x  under         the type arm reaches its depth bound
///                                                   first and stops the compile
/// ```
///
/// The function arm is the hostile maximum and it is **14.60x** the declared ceiling. Two
/// successive corpus defects hid it. The first read `MAX_STRUCT_LEAVES` — a durable
/// value-shape bound whose own declaration says it does not scale with the record width —
/// as the width of a generic `struct` template, leaving every arm sixty-four times narrow
/// with no enum arm at all. The second measured only one of a function body's two width
/// dimensions: the arm filled the local frame and carried a fraction of the 64 KiB of code
/// a function admits, and each instance retains a copy of the code as well as of the frame.
/// Padding to the code envelope moved the figure from 1.42 GB to 9.80 GB.
///
/// The combined corpus is **not** the maximum and cannot be: the type arm's divergence is
/// carried by a self-nesting field, so it exhausts the 256-deep mint bound long before the
/// 4096-wide count ceiling, and stops the compile before the function arm amplifies. See
/// `reaches_the_instantiation_bound` for the two bounds and why one diagnostic covers both.
///
/// Cost, recorded honestly: the function arm takes about 500 seconds on this host at the
/// `dev` profile, because it lowers roughly 27 million statements. That is a real charge
/// against the workspace battery and it is a consequence of measuring the true width.
///
/// The verdicts are pinned in the direction they hold, so this gate fails if an arm comes
/// under the ceiling (the finding is fixed and its record must be retired) as well as if
/// one goes over. It is not a ceiling raised to fit a measurement.
/// **The tier split, and why it is where it is.**
///
/// The workspace pillar puts compilation and test speed first and says a test costing
/// minutes does not belong in the default battery: genuinely necessary hostile measurement
/// goes behind an explicit opt-in tier. This gate is that pillar's first case, and the
/// split is drawn from measurement rather than from taste.
///
/// Three of the four corpora compile in under a second, because their divergence is carried
/// by a self-nesting field and the 256-deep mint bound stops them early. The function arm is
/// the one that reaches the 4096-wide count ceiling, and at the code-byte envelope it lowers
/// roughly 27 million statements: **about 500 s at the `dev` profile on this host.** It is
/// the whole cost.
///
/// So the arm's *measurement* is opt-in and everything else about it stays in the default
/// battery. `each_corpus_is_driven_at_the_width_of_the_bound_that_governs_it` (0.03 s),
/// `the_recorded_operation_envelope_is_exact` (0.03 s), and
/// `the_function_arm_sits_exactly_at_the_code_byte_envelope` (0.29 s) all read or compile the
/// arm's own generator, so a regression in how the arm is *built* still fails loudly by
/// default. Only the resident-set figure moves behind the tier.
///
/// Run the opt-in tier with:
///
/// ```text
/// cargo test -p marrow-compile --test issuance_rss -- --ignored
/// ```
///
/// Measured wall time: default tier **about 5 s**, opt-in tier **about 500 s**.
const FAST_CORPORA: &[&str] = &["type", "enum", "both"];

/// The maximal arm, measured only in the opt-in tier. It is the hostile maximum — the
/// figure the recorded verdict table calls 14.60x — so nothing here may quietly claim the
/// default battery measured it.
const MAXIMAL_CORPUS: &str = "function";

const RECORDED_ARM_VERDICTS: &[(&str, bool)] = &[
    ("both", true),
    ("enum", false),
    ("function", false),
    ("type", true),
];

/// The gate: compile each corpus in its own subprocess, measure every peak, and hold the
/// **largest** under the declared owned-heap ceiling. A platform that publishes no peak
/// fails here rather than passing unmeasured.
///
/// All three are measured because generic type and generic function instances share one
/// ceiling: whichever arm reaches it first stops the compile, so a project holding both
/// does not retain more than a project holding the heavier one. The maximum admitted
/// amplification is therefore the maximum over the corpora, not the combined corpus, and
/// measuring only one arm — as this gate previously did — reports whichever arm happened
/// to be written rather than the worst case.
#[test]
fn a_hostile_amplification_compile_stays_within_its_measured_rss_ceiling() {
    measure_corpora(FAST_CORPORA);
}

/// The maximal arm's peak — the opt-in half of the tier. This is the figure the recorded
/// table calls 14.60x the declared ceiling, and the default battery does **not** measure it.
#[test]
#[ignore = "the maximal amplification arm: about 500 s, run with --ignored"]
fn the_maximal_amplification_arm_stays_within_its_measured_rss_ceiling() {
    measure_corpora(&[MAXIMAL_CORPUS]);
}

/// Measure each named corpus in its own subprocess and hold every peak against its recorded
/// verdict. A platform that publishes no peak fails here rather than passing unmeasured.
fn measure_corpora(corpora: &[&str]) {
    let mut peaks = Vec::new();
    for corpus in corpora.iter().copied() {
        let peak = measured_peak_for(corpus);
        assert!(
            peak > 0,
            "a measured peak of zero is a broken measurement, not a frugal compile",
        );
        println!("hostile-amplification peak RSS [{corpus}]: {peak} bytes");
        peaks.push((corpus, peak));
    }
    for (corpus, peak) in &peaks {
        let under = *peak <= MAX_HOSTILE_COMPILE_RSS_BYTES;
        let recorded = RECORDED_ARM_VERDICTS
            .iter()
            .find(|(name, _)| name == corpus)
            .map(|(_, under)| *under)
            .unwrap_or_else(|| panic!("`{corpus}` has no recorded verdict"));
        assert_eq!(
            under, recorded,
            "the `{corpus}` corpus peaked at {peak} bytes against the declared owned-heap \
             ceiling of {MAX_HOSTILE_COMPILE_RSS_BYTES} bytes, which is not the recorded \
             verdict. If an arm came under the ceiling, the finding is fixed and its record \
             is retired; if one went over, that is a new finding. Either way it is \
             adjudicated here — the ceiling is not raised.",
        );
        // A runaway regression inside a recorded overshoot is still a regression: the
        // over/under classification alone would absorb a further doubling of an arm that is
        // already recorded as over. This tripwire sits above the largest recorded figure —
        // the function arm's 14.60x — and catches a doubling of it. It is a regression
        // tripwire read from the recorded measurement, not a ceiling: the declared
        // owned-heap authority above is the only ceiling, and it is not moved here.
        assert!(
            *peak < 22 * MAX_HOSTILE_COMPILE_RSS_BYTES,
            "the `{corpus}` corpus peaked at {peak} bytes, past even the recorded overshoot",
        );
    }
    let (worst_corpus, worst) = *peaks
        .iter()
        .max_by_key(|(_, peak)| *peak)
        .expect("every corpus was measured");
    // The measured figure is the gate's exported evidence: printed so a capacity join
    // consumes a stated number instead of rediscovering it.
    println!("hostile-amplification worst peak RSS: {worst} bytes ({worst_corpus})");
}

/// One corpus's peak, measured from outside the process that compiles it.
fn measured_peak_for(corpus: &str) -> u64 {
    let binary = std::env::current_exe().expect("the test binary's own path");
    let args = [
        "--exact",
        "inner_hostile_amplification_compile",
        "--ignored",
        "--nocapture",
    ];

    let (output, external_report) = if self_peak_rss_bytes().is_some() {
        let output = Command::new(&binary)
            .args(args)
            .env(CORPUS_SELECTOR, corpus)
            .output()
            .expect("spawn the hostile-compile subprocess");
        (output, None)
    } else {
        let output = Command::new("/usr/bin/time")
            .arg("-l")
            .arg(&binary)
            .args(args)
            .env(CORPUS_SELECTOR, corpus)
            .output()
            .expect("spawn the hostile-compile subprocess under the system reporter");
        let report = String::from_utf8_lossy(&output.stderr).into_owned();
        (output, Some(report))
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the hostile compile subprocess exited cleanly: {stdout}\n{stderr}",
    );
    assert!(
        stdout.contains("1 passed"),
        "the subprocess ran and passed the hostile compile: {stdout}",
    );

    match &external_report {
        Some(report) => reported_peak_rss_bytes(report),
        None => stdout
            .lines()
            .find_map(|line| line.strip_prefix("PEAK_RSS_BYTES="))
            .and_then(|value| value.trim().parse::<u64>().ok()),
    }
    .expect("this platform publishes a peak resident set size for the measured process")
}

/// The reporter parser reads each reporter's own unit rather than assuming one.
#[test]
fn the_peak_reporter_parser_reads_each_reporters_unit() {
    assert_eq!(
        reported_peak_rss_bytes("      1245184  maximum resident set size\n"),
        Some(1_245_184),
    );
    assert_eq!(
        reported_peak_rss_bytes("\tMaximum resident set size (kbytes): 2048\n"),
        Some(2_048 * 1024),
    );
    assert_eq!(reported_peak_rss_bytes("no such line\n"), None);
}

/// Whether compiling `source` refuses with the shared generic-mint diagnostic — the
/// observable proof that the corpus really drove generic rows until the compiler stopped
/// minting them.
///
/// **This does not identify which bound stopped it, and it cannot.** The compiler refuses a
/// generic type mint when either `type_insts.len() + fn_insts.len() >= MAX_INSTANTIATIONS`
/// (4096) or `fill_stack.len() >= MINT_DEPTH_LIMIT` (256), and reports both through one
/// `check.instantiation_limit` code with one message. A corpus whose divergence is carried
/// by a *self-nesting field* — `struct Grow<T> { next: Grow<List<T>> }` — recurses through
/// `fill_type_body`, so it reaches the depth bound at 256 nested mints long before the
/// count ceiling at 4096. The type and enum arms below are both of that shape.
///
/// So the type and enum figures are the peaks of a 256-deep amplification, not of a
/// 4096-wide one, and they are recorded as such rather than as the count ceiling's maximum.
/// The function arm is different: `reserve_fn_instance` gates on the count alone with no
/// depth check, so that arm does reach 4096.
///
/// Making the type and enum arms count-bounded needs a breadth-driven shape — divergence
/// carried by the generic *function* while a wide, non-self-nesting generic type is
/// resolved once per instance — which is a corpus this lane records as a finding rather
/// than one it invents a fourth unverified premise about.
fn reaches_the_instantiation_bound(source: &str) -> bool {
    match compile(&project(source)) {
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics
            .iter()
            .any(|row| row.code() == "check.instantiation_limit"),
        _ => false,
    }
}

/// Both amplification arms are live: each one, alone, drives generic instantiation to the
/// shared ceiling.
///
/// This is the assertion the previous corpus could not make. It declared `Grow<T>` and
/// never used it, so the generic-type arm built no row at all and the measured figure
/// described a function-only workload while claiming combined amplification. An arm that
/// is dead cannot reach the bound, so reaching it is what shows the arm is populated.
#[test]
fn each_amplification_arm_independently_reaches_the_instantiation_bound() {
    assert!(
        reaches_the_instantiation_bound(&type_only_corpus()),
        "the generic-type arm alone drives instantiation to the shared ceiling",
    );
    assert!(
        reaches_the_instantiation_bound(&enum_only_corpus()),
        "the generic-enum arm alone drives instantiation to the shared ceiling",
    );
    assert!(
        reaches_the_instantiation_bound(&hostile_corpus()),
        "every arm together drives generic minting to a bound",
    );
}

// The maximal arm's *liveness* is not asserted separately here. Compiling it costs ~500 s,
// and `inner_hostile_amplification_compile` — the subprocess the peak measurement already
// spawns for it — asserts the same `check.instantiation_limit` refusal on the same corpus.
// A second compile would double the opt-in tier's wall time to prove a fact the first one
// already proves, which is the disproportion the speed pillar names.

/// Each corpus is driven at the width of the bound that governs the construct it widens.
///
/// **This is the assertion the gate did without, and it is the one that catches the defect
/// the corpus actually had.** Reading a value-shape bound as a record's declared width left
/// every arm sixty-four times narrow while every other assertion here stayed green: a
/// narrow corpus still reaches the instantiation ceiling, still refuses as a source
/// diagnostic, and still classifies under the owned-heap ceiling. Nothing measured how wide
/// the bodies were, so nothing noticed that they were not the widest the compiler admits.
///
/// The widths are counted out of the generated source rather than recomputed from the same
/// constants that generate it, so a corpus that stopped emitting what it claims to emit
/// fails here.
#[test]
fn each_corpus_is_driven_at_the_width_of_the_bound_that_governs_it() {
    // A record declaration's width is the record-field bound. These are different bounds
    // over different subjects, and the corpus read the wrong one; asserting they differ is
    // what makes swapping one back for the other loud instead of merely narrower.
    assert_ne!(
        marrow_image::bounds::MAX_RECORD_FIELDS,
        marrow_image::bounds::MAX_STRUCT_LEAVES,
        "the record width and the dense value-shape leaf count are separate bounds",
    );

    let structs = type_amplification_arm();
    assert_eq!(
        structs.matches("    leaf").count() + 1,
        marrow_image::bounds::MAX_RECORD_FIELDS,
        "the generic struct template is declared at the full record-field width",
    );

    // Scoped to the enum declaration: the arm also carries a wrapper struct and a driver
    // function, whose own annotations are not variant payloads.
    let arm = enum_amplification_arm();
    let opened = arm
        .find("enum Grown<T> {")
        .expect("the enum arm declares its enum");
    let enums = &arm[opened..arm[opened..].find("\n}\n").expect("the enum closes") + opened];
    assert_eq!(
        enums.matches("    v").count() + 1,
        marrow_image::bounds::MAX_VARIANTS,
        "the generic enum template is declared at the full variant width",
    );
    // Every variant carries the full payload width, the recursive one included: its last
    // leaf is the recursion, so it contributes one fewer `: T`.
    assert_eq!(
        enums.matches(": T,").count() + enums.matches(": T)").count(),
        marrow_image::bounds::MAX_VARIANTS * marrow_image::bounds::MAX_PAYLOAD_FIELDS - 1,
        "every variant carries the full payload width, the recursive variant included",
    );
    assert!(
        enums.contains(&format!(
            "p{}: T, n: Grown<Wrap<T>>)",
            marrow_image::bounds::MAX_PAYLOAD_FIELDS - 2
        )),
        "the recursive variant's last leaf is the recursion and the rest are payload",
    );

    let functions = function_amplification_arm();
    assert_eq!(
        functions.matches("    var step").count(),
        marrow_image::bounds::MAX_LOCALS - 2,
        "the generic function fills its local frame, less the parameter and accumulator",
    );
    assert_eq!(
        functions.matches("    xs = append(xs, x)").count(),
        ADMITTED_CODE_PADDING,
        "the generic function is padded to the code-byte envelope, the second and \
         independent dimension of a function body's width",
    );
}

/// The function arm's body sits exactly at the code-byte envelope: it encodes, and one
/// more statement is refused with the typed `CodeBytes` limit.
///
/// This is what makes `ADMITTED_CODE_PADDING` an observation rather than a number someone
/// chose. Both halves are asserted, because a padding that merely encodes proves only that
/// the body is *somewhere* under the bound — which is the state the arm was already in,
/// carrying a small fraction of the code a function admits while the gate reported it as
/// the widest admissible body.
#[test]
fn the_function_arm_sits_exactly_at_the_code_byte_envelope() {
    match compile(&project(&code_envelope_mirror(ADMITTED_CODE_PADDING))) {
        Ok(_) => {}
        other => panic!(
            "the arm's body at {ADMITTED_CODE_PADDING} padding statements must encode: \
             {other:?}"
        ),
    }
    match compile(&project(&code_envelope_mirror(ADMITTED_CODE_PADDING + 1))) {
        Err(CompileFailure::ResourceLimit(limit)) => assert_eq!(
            limit.limit(),
            marrow_image::bounds::MAX_CODE_BYTES as u64,
            "one statement past the envelope is refused by the code-byte bound itself",
        ),
        other => panic!(
            "one statement past the envelope must be refused with the code-byte limit: \
             {other:?}"
        ),
    }
}

/// **The exact operation envelope each corpus drives, recorded as an artifact.**
///
/// Every figure is counted out of the generated source rather than restated from the
/// constants that generate it, so a corpus that stopped emitting what it claims to emit
/// fails here rather than reporting a width it no longer drives. This is the table a
/// capacity join reads instead of rediscovering the widths from the generators.
#[test]
fn the_recorded_operation_envelope_is_exact() {
    let structs = type_amplification_arm();
    let arm = enum_amplification_arm();
    // Scoped to the enum declaration: the arm also carries a wrapper struct and a driver
    // function, whose own annotations are not variant payloads.
    let opened = arm
        .find("enum Grown<T> {")
        .expect("the enum arm declares its enum");
    let enums = &arm[opened..arm[opened..].find("\n}\n").expect("the enum closes") + opened];
    let functions = function_amplification_arm();

    let envelope: Vec<(&str, &str, usize)> = vec![
        (
            "type",
            "declared record fields per template",
            structs.matches(": T\n").count() + structs.matches(": Grow<List<T>>\n").count(),
        ),
        (
            "enum",
            "declared variants per template",
            enums.matches("\n    v").count() + enums.matches("\n    next(").count(),
        ),
        (
            "enum",
            "declared payload leaves per template",
            enums.matches(": T,").count()
                + enums.matches(": T)").count()
                + enums.matches(": Grown<Wrap<T>>)").count(),
        ),
        (
            "function",
            "declared local slots per instance",
            functions.matches("    var ").count() + 1,
        ),
        (
            "function",
            "declared statements per instance",
            functions.matches("\n    ").count(),
        ),
    ];

    let expected: Vec<(&str, &str, usize)> = vec![
        (
            "type",
            "declared record fields per template",
            marrow_image::bounds::MAX_RECORD_FIELDS,
        ),
        (
            "enum",
            "declared variants per template",
            marrow_image::bounds::MAX_VARIANTS,
        ),
        (
            "enum",
            "declared payload leaves per template",
            marrow_image::bounds::MAX_VARIANTS * marrow_image::bounds::MAX_PAYLOAD_FIELDS,
        ),
        (
            "function",
            "declared local slots per instance",
            marrow_image::bounds::MAX_LOCALS,
        ),
        (
            "function",
            "declared statements per instance",
            1 + 2 * ADMITTED_LOCALS + ADMITTED_CODE_PADDING + 1,
        ),
    ];

    assert_eq!(
        envelope, expected,
        "the operation envelope the corpora drive moved; each figure is the bound that \
         governs its construct, counted out of the generated source",
    );
    for (corpus, dimension, width) in &envelope {
        println!("operation envelope [{corpus}] {dimension}: {width}");
    }
}
