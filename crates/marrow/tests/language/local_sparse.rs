//! End-to-end local-sparse-product tests: `resource` locals with field
//! assignment, `unset`, and an `Option`-typed sparse field travel the real
//! production path (capture -> compile -> encode -> verify -> VM) through the
//! built binary, via the `local_sparse` conformance fixture and inline
//! invalid-source projects asserting typed diagnostics.

use crate::common::{Project, conformance_dir, marrow_in};

/// The local-sparse conformance fixture passes end to end: a fresh sparse field
/// reads absent, assignment sets it present, `unset` clears it, value/copy
/// semantics are independent, and an `Option[string]` sparse field keeps absent
/// distinct from a present `Option` none.
#[test]
fn local_sparse_conformance_fixture_passes_on_the_production_path() {
    let output = marrow_in(
        &conformance_dir("local_sparse"),
        &["test", "--format", "jsonl"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "local_sparse fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":9"#), "{summary}");
}

/// A sparse field assignment flows through the VM: an export that builds a record,
/// assigns a sparse field, and reads it back yields the assigned value.
#[test]
fn a_sparse_field_assignment_flows_through_the_vm() {
    let workspace = Project::single(
        r#"resource Box {
    required id: int
    note: string
}

pub fn f(): string {
    var b = Box(id: 1)
    b.note = "hi"
    return b.note ?? "absent"
}
"#,
    )
    .materialize("assign");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":"hi""#), "{stdout}");
}

/// `unset` clears a present sparse field back to absent, observed through the VM.
#[test]
fn unset_clears_a_sparse_field_through_the_vm() {
    let workspace = Project::single(
        r#"resource Box {
    required id: int
    note: string
}

pub fn f(): string {
    var b = Box(id: 1, note: "hi")
    unset b.note
    return b.note ?? "absent"
}
"#,
    )
    .materialize("unset");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":"absent""#), "{stdout}");
}

/// A required field cannot be unset: it is a typed `check.type` at the field.
#[test]
fn unsetting_a_required_field_is_a_check_type_diagnostic() {
    let workspace = Project::single(
        r#"resource Box {
    required id: int
}

pub fn f(): int {
    var b = Box(id: 1)
    unset b.id
    return 0
}
"#,
    )
    .materialize("required-unset");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.type""#), "{stdout}");
}

/// `unset` on a durable place is rejected: durable erasure uses `delete`.
#[test]
fn unsetting_a_durable_place_is_a_check_type_diagnostic() {
    let workspace = Project::single(
        r#"resource Box {
    required id: int
    note: string
}

store ^boxes[id: int]: Box

pub fn f(k: int) {
    transaction {
        unset ^boxes[k].note
    }
}
"#,
    )
    .materialize("durable-unset");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl", "--", "1"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.type""#), "{stdout}");
}

/// A resource carrying an `Option` field is identity-complete: the store is
/// accepted and its durable identities are minted, including the sum and member ids
/// of the `Option[string]` reachable through it (`Option` is a closed enum). The
/// `Option`-valued field is not part of the kernel-executable flat scalar record, so
/// a durable operation over the store is a precise `check.unsupported`; it is no
/// longer a `check.type` on the declaration.
#[test]
fn a_store_over_an_option_field_resource_is_identity_complete() {
    let workspace = Project::single(
        r#"resource Box {
    required id: int
    tag: Option<string>
}

store ^boxes[id: int]: Box

pub fn f(): int {
    return 0
}
"#,
    )
    .materialize("store-option");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":0"#), "{stdout}");
    let ids = workspace.read(".marrow/ids");
    assert!(ids.contains("sum Option[string] "), "{ids}");
    assert!(ids.contains("member Option[string].none "), "{ids}");
    assert!(ids.contains("member Option[string].some "), "{ids}");
}

/// An `Option[string]` sparse field keeps three states distinct through the VM:
/// absent, a present `Option` none, and a present `Option` some. No dedicated
/// absent runtime value is needed — vacancy is one representation, a present none
/// another.
#[test]
fn an_option_typed_sparse_field_keeps_absent_and_present_none_distinct() {
    let workspace = Project::single(
        r#"resource Box {
    required id: int
    tag: Option<string>
}

pub fn classify(mode: int): string {
    var b = Box(id: 1)
    if mode == 1 {
        b.tag = none
    }
    if mode == 2 {
        b.tag = some("hi")
    }
    if const t = b.tag {
        match t {
            none => return "present-none"
            some(v) => return v
        }
    }
    return "absent"
}
"#,
    )
    .materialize("option-field");
    for (mode, expected) in [(0, "absent"), (1, "present-none"), (2, "hi")] {
        let output = workspace.marrow(&[
            "run",
            "classify",
            "--format",
            "jsonl",
            "--",
            &mode.to_string(),
        ]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "mode {mode}: {stdout}");
        assert!(
            stdout.contains(&format!(r#""data":"{expected}""#)),
            "mode {mode}: {stdout}"
        );
    }
}

/// Assigning to a field of a `const`-bound record is rejected: the binding is
/// immutable, so the field cannot be reassigned.
#[test]
fn assigning_a_field_of_a_const_record_is_a_check_type_diagnostic() {
    let workspace = Project::single(
        r#"resource Box {
    required id: int
    note: string
}

pub fn f(): int {
    const b = Box(id: 1)
    b.note = "x"
    return 0
}
"#,
    )
    .materialize("const-field");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.type""#), "{stdout}");
}
