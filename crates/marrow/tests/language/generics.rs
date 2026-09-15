//! End-to-end rank-1 generic function tests: generic definitions travel the real
//! production path (capture -> compile -> monomorphize -> encode -> verify -> VM)
//! through the built binary, via the `generics` conformance fixture and inline
//! projects that run a monomorphized generic and assert its rendered result.

use crate::common::{Project, conformance_dir, marrow_in};

/// The generic value-types conformance fixture passes end to end: user generic
/// `struct`/`enum` construction, field access, `match`, constrained instantiation,
/// nesting, generic-typed function parameters, and the refounded `Option`/`Result`
/// (ordinary generic enums) all report `passed` through the production path.
#[test]
fn generic_types_conformance_fixture_passes_on_the_production_path() {
    let output = marrow_in(
        &conformance_dir("generic_types"),
        &["test", "--format", "jsonl"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "generic_types fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":10"#), "{summary}");
}

/// The generics conformance fixture passes end to end: identity, first, swap-style
/// construction, and the `supports equality`/`supports order` constrained helpers
/// all report `passed` through the production path, each call monomorphized.
#[test]
fn generics_conformance_fixture_passes_on_the_production_path() {
    let output = marrow_in(&conformance_dir("generics"), &["test", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "generics fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":7"#), "{summary}");
}

/// A monomorphized generic runs through the VM: a `pub` export calls a generic
/// helper at a concrete type and returns its result, rendered by `marrow run`.
#[test]
fn a_monomorphized_generic_runs_through_the_vm() {
    let workspace = Project::single(
        r#"module main

fn firstOr<T>(xs: List<T>, fallback: T): T {
    for x in xs {
        return x
    }
    return fallback
}

pub fn head(): int {
    var xs: List<int> = List()
    xs = append(xs, 11)
    xs = append(xs, 22)
    return firstOr(xs, 0)
}
"#,
    )
    .materialize("run");
    let output = workspace.marrow(&["run", "head", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":11"#), "{stdout}");
}

/// The throwaway template check mints abstract `List<T>` and `Box<T>` rows, while
/// the real concrete instance mints and uses the corresponding `int` rows. Clone
/// state must not shift production type or collection indices: the CLI compiles and
/// verifies the image, then the VM returns the boxed value through the real export.
#[test]
fn template_check_mints_do_not_shift_production_type_or_collection_indices() {
    let workspace = Project::single(
        r#"module main

struct Box<T> {
    value: T
}

fn unwrap<T>(value: T): T {
    var values: List<T> = List()
    values = append(values, value)
    const boxed = Box(value: value)
    return boxed.value
}

pub fn run(): int {
    return unwrap(41)
}
"#,
    )
    .materialize("template-check-indices");
    let output = workspace.marrow(&["run", "run", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{output:?}\n{stdout}\n{stderr}");
    assert_eq!(stderr, "", "a verified run has no stderr: {stderr}");
    assert!(stdout.contains(r#""data":41"#), "{stdout}");
}

/// A `pub` generic function is not itself an invocable export: it has no single
/// image entry, so `marrow run` on its name fails to resolve, evidencing that
/// monomorphized instances carry no stable export identity.
#[test]
fn a_generic_function_is_not_an_export() {
    let workspace = Project::single(
        r#"module main

pub fn identity<T>(x: T): T {
    return x
}

pub fn concrete(): int {
    return identity(1)
}
"#,
    )
    .materialize("no-export");
    let output = workspace.marrow(&["run", "identity", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "a generic function is not a runnable export: {stdout}"
    );
    // The monomorphic entry that calls it does run.
    let concrete = workspace.marrow(&["run", "concrete", "--format", "jsonl"]);
    assert!(
        concrete.status.success(),
        "{}",
        String::from_utf8_lossy(&concrete.stdout)
    );
}

/// The outer production command observes the same total shared-limit result as the
/// compiler owner: one located diagnostic, no process unwind, and no value/artifact
/// record after the rejected generic body.
#[test]
fn instantiation_limit_is_one_diagnostic_and_no_partial_cli_output() {
    let workspace = Project::single(
        r#"module main

fn identity<T>(x: T): T {
    return x
}

fn grow<T>(x: T): int {
    const y = some(x)
    const next = grow(y)
    const held = identity(x)
    const z = some(y)
    return next
}

pub fn driver(): int {
    return grow(1)
}
"#,
    )
    .materialize("instantiation-limit");
    let output = workspace.marrow(&["run", "driver", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stdout}");
    assert_eq!(stderr, "", "the compiler process must not unwind: {stderr}");
    assert_eq!(
        stdout.as_ref(),
        "{\"code\":\"check.instantiation_limit\",\"kind\":\"run\",\"outcome\":\"diagnostic\",\"span\":{\"column\":20,\"line\":11}}\n",
        "one canonical diagnostic record and no partial output"
    );
}
