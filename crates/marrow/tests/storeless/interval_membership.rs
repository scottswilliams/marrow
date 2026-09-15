//! End-to-end interval-membership tests: `value in lo..hi` / `value not in lo..hi`
//! travels the real production path (capture → compile → encode → verify → VM) through
//! the built binary. The half-open/inclusive boundaries and the `not in` negation are
//! exercised over a parameterized `f(x: int): bool`; the malformed forms assert their
//! typed diagnostic codes.

use crate::common::Project;

/// A single-export project whose `f(x: int): bool` body is `body`.
fn membership(body: &str) -> Project {
    Project::single(&format!(
        "module main\n\npub fn f(x: int): bool {{\n    return {body}\n}}\n"
    ))
}

/// Evaluate `f(x): bool` whose body is `body`, at argument `x`, returning the value.
fn eval(name: &str, body: &str, x: i64) -> bool {
    let output = membership(body).run_cli(
        name,
        &["run", "f", "--format", "jsonl", "--", &x.to_string()],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{body} at {x}: {stdout}");
    if stdout.contains(r#""data":true"#) {
        true
    } else if stdout.contains(r#""data":false"#) {
        false
    } else {
        panic!("no bool value for {body} at {x}: {stdout}");
    }
}

/// Compile a body expected to fail; return the typed diagnostic code.
fn reject(name: &str, body: &str) -> String {
    let output = membership(body).run_cli(name, &["run", "f", "--format", "jsonl", "--", "0"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{body} must fail: {stdout}");
    stdout
        .split(r#""code":""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no code for {body}: {stdout}"))
        .to_string()
}

#[test]
fn a_half_open_range_excludes_its_upper_bound() {
    let body = "x in 0..10";
    assert!(!eval("half-open", body, -1));
    assert!(eval("half-open", body, 0));
    assert!(eval("half-open", body, 9));
    assert!(!eval("half-open", body, 10));
    assert!(!eval("half-open", body, 11));
}

#[test]
fn an_inclusive_range_includes_its_upper_bound() {
    let body = "x in 0..=10";
    assert!(eval("inclusive", body, 10));
    assert!(!eval("inclusive", body, 11));
}

#[test]
fn not_in_is_the_negation() {
    let body = "x not in 0..10";
    assert!(!eval("not-in", body, 5));
    assert!(eval("not-in", body, 20));
    assert!(eval("not-in", body, -1));
}

#[test]
fn a_non_range_right_operand_is_a_type_error() {
    assert_eq!(reject("non-range", "x in 5"), "check.type");
}

#[test]
fn a_membership_range_takes_no_step() {
    assert_eq!(reject("step", "x in 0..10 by 2"), "check.type");
}

#[test]
fn an_open_ended_membership_range_is_a_type_error() {
    assert_eq!(reject("open", "x in 0.."), "check.type");
}

#[test]
fn a_chained_membership_is_a_parse_error() {
    assert_eq!(reject("chained", "x in 0..10 in 0..3"), "parse.syntax");
}
