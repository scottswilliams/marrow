//! The issuance gate's memory half: a hostile maximum-amplification corpus is driven
//! through the production `compile` path in a subprocess and that subprocess's peak
//! resident set size is measured, so the wide carriers' width derivation
//! (`marrow-image::issuance`) is joined by a measured memory-feasibility figure rather
//! than an unmeasured assumption.
//!
//! The corpus is a divergent generic over the largest admissible body: each of the
//! 4,096 instantiations the shared bound admits carries `MAX_STRUCT_LEAVES` member
//! rows, and every monomorphic body stays simultaneously retained because a public
//! image-policy crossing does not stop provisional construction. A generic function
//! whose body and span shape amplify per instance runs in the same project, so the
//! measured figure charges draft rows, lookup indexes, the journal and policy ledger,
//! and the live compiler diagnostic and analysis-fact transients together.
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

/// The largest admissible struct body, read from the owner that fixes it rather than
/// hand-copied: a bound change must move the corpus with it, not leave it describing a
/// body the compiler no longer calls maximal.
const ADMITTED_STRUCT_LEAVES: usize = marrow_image::bounds::MAX_STRUCT_LEAVES;

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
    for leaf in 0..ADMITTED_STRUCT_LEAVES - 1 {
        source.push_str(&format!("    leaf{leaf}: T\n"));
    }
    source.push_str("    next: Grow<List<T>>\n}\n\n");
    source.push_str("fn deepen<T>(x: T): Grow<T> {\n    return deepen(x)\n}\n\n");
    source
}

/// The function-amplification arm: a divergent generic function whose per-instance body
/// and span shape amplify, diverging on an ever-growing argument.
fn function_amplification_arm() -> String {
    let mut source = String::from("fn grow<T>(x: T): int {\n    var xs: List<T> = List()\n");
    for step in 0..ADMITTED_STRUCT_LEAVES {
        source.push_str(&format!("    var step{step}: List<T> = xs\n"));
        source.push_str(&format!("    xs = append(step{step}, x)\n"));
    }
    source.push_str("    return grow(xs)\n}\n\n");
    source
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

/// The hostile maximum-amplification project: both arms in one project, so the measured
/// figure charges generic type rows and generic function rows together, along with the
/// draft rows, lookup indexes, journal, policy ledger, and the live compiler diagnostic
/// and analysis-fact transients.
///
/// Type and function instances share **one** ceiling
/// (`type_insts.len() + fn_insts.len() >= MAX_INSTANTIATIONS`), so the two arms do not
/// each reach it — together they saturate it. That is the maximum this compiler admits,
/// and claiming two independent 4,096-row populations would overstate it.
fn hostile_corpus() -> String {
    format!(
        "module main\n\n{}{}pub fn driver(): int {{\n    const ignored = deepen(1)\n    return grow(1)\n}}\n",
        type_amplification_arm(),
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
    let mut peaks = Vec::new();
    for corpus in ["type", "function", "both"] {
        let peak = measured_peak_for(corpus);
        assert!(
            peak > 0,
            "a measured peak of zero is a broken measurement, not a frugal compile",
        );
        println!("hostile-amplification peak RSS [{corpus}]: {peak} bytes");
        peaks.push((corpus, peak));
    }
    let (worst_corpus, worst) = *peaks
        .iter()
        .max_by_key(|(_, peak)| *peak)
        .expect("three corpora were measured");
    assert!(
        worst <= MAX_HOSTILE_COMPILE_RSS_BYTES,
        "the hostile maximum-amplification compile peaked at {worst} bytes on the \
         `{worst_corpus}` corpus, over the declared owned-heap ceiling of \
         {MAX_HOSTILE_COMPILE_RSS_BYTES} bytes. Record this as a finding about the \
         compiler; do not raise the ceiling.",
    );
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

/// Whether compiling `source` refuses with the shared instantiation-limit diagnostic —
/// the observable proof that the corpus really drove generic rows to the ceiling.
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
        reaches_the_instantiation_bound(&function_only_corpus()),
        "the generic-function arm alone drives instantiation to the shared ceiling",
    );
    assert!(
        reaches_the_instantiation_bound(&hostile_corpus()),
        "both arms together drive instantiation to the shared ceiling",
    );
}
