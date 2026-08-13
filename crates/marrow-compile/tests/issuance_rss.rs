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

use std::process::Command;

use marrow_compile::{CompileFailure, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

/// The largest admissible struct body (`marrow-image::bounds::MAX_STRUCT_LEAVES`).
const ADMITTED_STRUCT_LEAVES: usize = 64;

/// The row-local target-authority ceiling for a hostile maximum-amplification compile.
///
/// Set from the measured peak of this corpus — 394 MiB on the aarch64-apple-darwin
/// authority at issuance — with room for allocator and toolchain variation across the
/// supported authorities, not from a target the implementation was tuned to reach. It
/// is a ratchet: a representation change that materially widens the retained
/// provisional population fails here instead of at some later capacity join. Raising
/// it is a scheduler decision with its own evidence, never a silent edit.
const MAX_HOSTILE_COMPILE_RSS_BYTES: u64 = 1 << 30;

/// The hostile maximum-amplification project: a divergent generic type over the
/// largest admissible body, plus a divergent generic function whose per-instance body
/// and span shape amplify alongside it. Both diverge on an ever-growing argument, so
/// the shared bound — not the source's size — fixes how many bodies are retained at
/// once.
fn hostile_corpus() -> String {
    let mut source = String::from("module main\n\nstruct Grow<T> {\n");
    for leaf in 0..ADMITTED_STRUCT_LEAVES - 1 {
        source.push_str(&format!("    leaf{leaf}: T\n"));
    }
    source.push_str("    next: Grow<List<T>>\n}\n\n");

    source.push_str("fn grow<T>(x: T): int {\n    var xs: List<T> = List()\n");
    for step in 0..ADMITTED_STRUCT_LEAVES {
        source.push_str(&format!("    var step{step}: List<T> = xs\n"));
        source.push_str(&format!("    xs = append(step{step}, x)\n"));
    }
    source.push_str("    return grow(xs)\n}\n\n");

    source.push_str("pub fn driver(): int {\n    return grow(1)\n}\n");
    source
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

/// The subprocess half: drive the hostile corpus through the production compile path
/// and, where the platform allows it, report this process's own peak. Run only by the
/// outer test.
#[test]
#[ignore = "the subprocess half of the hostile-amplification RSS gate"]
fn inner_hostile_amplification_compile() {
    let source = hostile_corpus();
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

/// The gate: run the hostile compile in a subprocess, measure its peak resident set
/// size, and hold it under the row-local target-authority ceiling. A platform that
/// publishes no peak fails here rather than passing unmeasured.
#[test]
fn a_hostile_amplification_compile_stays_within_its_measured_rss_ceiling() {
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
            .output()
            .expect("spawn the hostile-compile subprocess");
        (output, None)
    } else {
        let output = Command::new("/usr/bin/time")
            .arg("-l")
            .arg(&binary)
            .args(args)
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

    let peak = match &external_report {
        Some(report) => reported_peak_rss_bytes(report),
        None => stdout
            .lines()
            .find_map(|line| line.strip_prefix("PEAK_RSS_BYTES="))
            .and_then(|value| value.trim().parse::<u64>().ok()),
    }
    .expect("this platform publishes a peak resident set size for the measured process");

    assert!(
        peak > 0,
        "a measured peak of zero is a broken measurement, not a frugal compile",
    );
    assert!(
        peak <= MAX_HOSTILE_COMPILE_RSS_BYTES,
        "the hostile maximum-amplification compile peaked at {peak} bytes, over the \
         row-local ceiling of {MAX_HOSTILE_COMPILE_RSS_BYTES} bytes",
    );
    // The measured figure is the gate's exported evidence: printed so a capacity join
    // consumes a stated number instead of rediscovering it.
    println!("hostile-amplification compile peak RSS: {peak} bytes");
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
