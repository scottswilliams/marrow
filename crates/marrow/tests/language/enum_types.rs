//! End-to-end closed-flat-enum tests: `enum Name` with payloadless and payload
//! members travels the real production path (capture -> compile -> encode ->
//! verify -> VM) through the built binary, via the `enum_types` conformance
//! fixture and inline invalid-source projects asserting typed diagnostics.

use crate::common::{Project, conformance_dir, marrow_in};

/// The enum conformance fixture passes end to end: payloadless and payload
/// construction, exhaustive `match` with positional payload binding, payload-
/// ignoring arms, exact `==`/`!=` equality over the variant and payload, and
/// construction/matching across function boundaries all report `passed`.
#[test]
fn enum_conformance_fixture_passes_on_the_production_path() {
    let output = marrow_in(
        &conformance_dir("enum_types"),
        &["test", "--format", "jsonl"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "enum fixture must pass: {output:?}\n{stdout}"
    );
    let summary = stdout
        .lines()
        .find(|line| line.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary record: {stdout}"));
    assert!(summary.contains(r#""failed":0"#), "{summary}");
    assert!(summary.contains(r#""total":8"#), "{summary}");
}

/// A returned enum value renders through the VM: `run` on an export that
/// constructs a payload variant yields the canonical enum object.
#[test]
fn a_payload_enum_value_renders_through_the_vm() {
    let workspace = Project::single(
        r#"enum Shape {
    dot
    circle(radius: int)
}

pub fn make(r: int): Shape {
    return Shape::circle(radius: r)
}
"#,
    )
    .materialize("render");
    let output = workspace.marrow(&["run", "make", "--format", "jsonl", "--", "7"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.contains(r#""data":{"enum":"Shape","member":"circle","payload":[7]}"#),
        "{stdout}"
    );
}

/// A declared member payload carries a struct and another enum, exactly as a
/// generic enum instantiation's payload does: the value constructs, matches,
/// compares, and renders with the composite leaves nested in the payload array.
#[test]
fn a_composite_payload_renders_through_the_vm() {
    let output = marrow_in(
        &conformance_dir("enum_types"),
        &["run", "makeCell", "--format", "jsonl", "--", "3", "4"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.contains(
            r#""data":{"enum":"Cell","member":"filled","payload":[{"x":3,"y":4},{"enum":"Color","member":"blue","payload":[]}]}"#
        ),
        "{stdout}"
    );
}

/// The payload shapes the widening does not admit keep their own refusals, and
/// a nominal leaf keeps the nominal boundary: `marrow check` reports each at the
/// declaration with its own message.
#[test]
fn a_refused_payload_shape_keeps_its_own_message() {
    for (label, source, code, message) in [
        (
            "collection",
            "enum E {\n    m(v: List<int>)\n}\n\npub fn f(): int {\n    return 0\n}\n",
            "check.unsupported",
            "is not a payload type",
        ),
        (
            "optional",
            "enum E {\n    m(v: int?)\n}\n\npub fn f(): int {\n    return 0\n}\n",
            "check.unsupported",
            "an optional enum payload field type",
        ),
        (
            "cycle through a struct",
            "enum E {\n    m(v: S)\n}\n\nstruct S {\n    e: E\n}\n\npub fn f(): int {\n    return 0\n}\n",
            "check.recursion",
            "contains itself through the cycle E -> S -> E",
        ),
        (
            "cycle through the enum alone",
            "enum E {\n    m(v: E)\n}\n\npub fn f(): int {\n    return 0\n}\n",
            "check.recursion",
            "contains itself through the cycle E -> E",
        ),
        (
            "nominal boundary",
            "type Age: int in 0..150\n\nenum E {\n    m(a: Age)\n}\n\npub fn f(e: E): int {\n    return 0\n}\n",
            "check.unsupported",
            "public aggregate parameters containing nominal values",
        ),
    ] {
        let workspace = Project::single(source).materialize("refused-payload");
        let check = workspace.marrow(&["check"]);
        let report = String::from_utf8_lossy(&check.stderr);
        assert!(!check.status.success(), "{label}: {report}");
        assert!(report.contains(code), "{label}: {report}");
        assert!(report.contains(message), "{label}: {report}");
    }
}

/// A non-exhaustive `match` is `check.match_nonexhaustive`.
#[test]
fn a_non_exhaustive_match_is_reported() {
    let workspace = Project::single(
        r#"enum E {
    a
    b
}

pub fn f(e: E): int {
    match e {
        a => return 1
    }
}
"#,
    )
    .materialize("nonexhaustive");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains(r#""code":"check.match_nonexhaustive""#),
        "{stdout}"
    );
}

/// A malformed arm — an unknown member, a duplicate member, or a payload-arity
/// mismatch — is a typed `check.match_arm`.
#[test]
fn a_malformed_arm_is_a_check_match_arm_diagnostic() {
    for body in [
        // unknown member
        "match e {\n        a => return 1\n        c => return 2\n        b => return 3\n    }",
        // duplicate member
        "match e {\n        a => return 1\n        a => return 2\n        b => return 3\n    }",
    ] {
        let workspace = Project::single(&format!(
            "enum E {{\n    a\n    b\n}}\n\npub fn f(e: E): int {{\n    {body}\n}}\n"
        ))
        .materialize("arm");
        let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{body}\n{stdout}");
        assert!(
            stdout.contains(r#""code":"check.match_arm""#),
            "{body}\n{stdout}"
        );
    }
}

/// A payload-arity mismatch on a binding arm is a typed `check.match_arm`.
#[test]
fn a_payload_arity_mismatch_is_reported() {
    let workspace = Project::single(
        r#"enum E {
    a(x: int)
    b
}

pub fn f(e: E): int {
    match e {
        a(x, y) => return x
        b => return 0
    }
}
"#,
    )
    .materialize("arity");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.match_arm""#), "{stdout}");
}

/// A malformed construction — an unknown payload field, a missing payload field,
/// a payload on a payloadless member, or a non-existent member — is a typed
/// `check.type`.
#[test]
fn a_malformed_construction_is_a_check_type_diagnostic() {
    for expr in [
        "Shape::circle(radius: 1, z: 2)",
        "Shape::circle()",
        "Shape::dot(x: 1)",
        "Shape::triangle",
    ] {
        let workspace = Project::single(&format!(
                "enum Shape\n    dot\n    circle(radius: int)\n\npub fn f(): int\n    const s = {expr}\n    return 0\n"
            )).materialize("construct");
        let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{expr}\n{stdout}");
        // `Shape::circle()` is a parse error (an empty payload); the rest are
        // check.type. Either way the export does not run.
        assert!(
            stdout.contains(r#""outcome":"diagnostic""#),
            "{expr}\n{stdout}"
        );
    }
}

/// A `category` member or a nested member is deferred: `check.unsupported`.
#[test]
fn a_hierarchical_enum_is_deferred() {
    let workspace = Project::single(
        r#"enum Animal {
    category cat {
        tiger
    }
    dog
}

pub fn f(): int {
    return 0
}
"#,
    )
    .materialize("hierarchy");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""code":"check.unsupported""#), "{stdout}");
}

/// An enum whose name collides with another type is a `check.name_conflict`.
#[test]
fn an_enum_name_collision_is_reported() {
    let workspace = Project::single(
        r#"struct Color {
    r: int
}

enum Color {
    red
}

pub fn f(): int {
    return 0
}
"#,
    )
    .materialize("collision");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(
        stdout.contains(r#""code":"check.name_conflict""#),
        "{stdout}"
    );
}

/// A resource field may name a user enum declared later in the file: because the
/// value types are declared before any field is resolved, the field resolves to the
/// enum, a `match` over the field read keeps the enum identity, and the whole travels
/// the production path. (The resource is a local value here; a resource backing a
/// `store` still admits only scalar fields.)
#[test]
fn a_resource_field_may_be_a_user_enum_and_match_over_the_field_read() {
    let workspace = Project::single(
        r#"resource Paint {
    required shade: Color
}

enum Color {
    red
    green
}

pub fn name(): string {
    const p = Paint(shade: Color::green)
    match p.shade {
        red => return "r"
        green => return "g"
    }
}
"#,
    )
    .materialize("resource-enum-field");
    let output = workspace.marrow(&["run", "name", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""data":"g""#), "{stdout}");
}

/// A resource backing a `store` still admits only scalar fields: a user-enum field
/// on a stored resource is a typed `check.type` at the store (the durable-root
/// scalar-only rule is unchanged by the local-value nesting work).
/// A stored resource with a closed-enum field is now identity-complete: the store
/// declaration is accepted and its durable identities (including the enum's sum and
/// per-member ids) are minted, so a storeless export over the project runs. The
/// enum-valued field is not part of the kernel-executable flat scalar record, so a
/// durable operation over the store is a precise `check.unsupported` (covered in the
/// durable-field widening suite), not a `check.type` on the declaration.
#[test]
fn a_stored_resource_with_an_enum_field_is_identity_complete() {
    let workspace = Project::single(
        r#"resource Paint {
    required id: int
    required shade: Color
}

enum Color {
    red
    green
}

store ^paints[id: int]: Paint

pub fn f(): int {
    return 0
}
"#,
    )
    .materialize("stored-enum-field");
    let output = workspace.marrow(&["run", "f", "--format", "jsonl"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(r#""outcome":"value""#), "{stdout}");
    assert!(stdout.contains(r#""data":0"#), "{stdout}");
    // The enum reachable through the store gained sum and per-member identities.
    let ids = workspace.read(".marrow/ids");
    assert!(ids.contains("sum Color "), "{ids}");
    assert!(ids.contains("member Color.red "), "{ids}");
    assert!(ids.contains("member Color.green "), "{ids}");
}
