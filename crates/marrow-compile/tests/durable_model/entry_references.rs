//! Checked entry bindings have lexical scope and preserve only live presence facts.

use std::sync::OnceLock;

use marrow_codes::Code;
use marrow_compile::{CompileFailure, SourceDiagnostic, compile};

use super::{ids, project};

const SCHEMA: &str = r"module main
resource Entry {
    required value: int
    note: string
    children[id: int] {
        required value: int
    }
}
store ^entries[id: int]: Entry
store ^probes[id: int]: Entry
";

fn diagnostics(body: &str) -> Vec<SourceDiagnostic> {
    static LEDGER: OnceLock<Vec<u8>> = OnceLock::new();
    let ledger = LEDGER.get_or_init(|| {
        let source = format!("{SCHEMA}\npub fn empty() {{}}\n");
        ids::converged(|ledger| project(&source, ledger)).1
    });
    let source = format!("{SCHEMA}\n{body}");
    match compile(&project(&source, Some(ledger))) {
        Ok(_) => Vec::new(),
        Err(CompileFailure::Diagnostics(rows)) => rows.into_vec(),
        Err(other) => panic!("expected source diagnostics, got {other:?}"),
    }
}

fn accepted(body: &str) {
    let rows = diagnostics(body);
    assert!(rows.is_empty(), "{rows:#?}\n{body}");
}

fn rejected_at(body: &str, code: Code, needle: &str) {
    let rows = diagnostics(body);
    let source = format!("{SCHEMA}\n{body}");
    let start = source.find(needle).expect("refused construct is present");
    let line = u32::try_from(source[..start].bytes().filter(|b| *b == b'\n').count() + 1)
        .expect("small fixture line");
    assert!(
        rows.iter()
            .any(|row| row.code() == code && row.line() == line),
        "expected {code:?} at {needle:?}, line {line}; got {rows:#?}"
    );
}

#[test]
fn an_entry_binding_proves_required_reads_and_field_writes() {
    accepted(
        r"pub fn increment(id: int): int {
    transaction {
        ref entry = ^entries[id] else { return -1 }
        entry.value = entry.value + 1
        return entry.value
    }
}",
    );
}

#[test]
fn a_binding_is_absent_in_its_else_and_after_its_lexical_scope() {
    rejected_at(
        r"pub fn read(id: int): int {
    ref entry = ^entries[id] else {
        return entry.value ?? 0
    }
    return entry.value
}",
        Code::CheckType,
        "return entry.value ?? 0",
    );
    rejected_at(
        r"pub fn read(id: int, select: bool): int {
    if select {
        ref entry = ^entries[id] else { return -1 }
        return entry.value
    }
    return entry.value ?? 0
}",
        Code::CheckType,
        "return entry.value ?? 0",
    );
}

#[test]
fn an_entry_binding_requires_an_else_that_diverges() {
    rejected_at(
        r"pub fn read(id: int): int {
    ref entry = ^entries[id]
    return 0
}",
        Code::ParseSyntax,
        "ref entry",
    );
    rejected_at(
        r"pub fn read(id: int): int {
    ref entry = ^entries[id] else {}
    return entry.value
}",
        Code::CheckType,
        "ref entry",
    );
}

#[test]
fn only_a_direct_whole_entry_address_can_be_bound() {
    for target in ["entry", "entry.children[1]", "^entries[id].value", "42"] {
        let body = format!(
            "pub fn read(id: int): int {{\n    ref entry = ^entries[id] else {{ return -1 }}\n    ref other = {target} else {{ return -2 }}\n    return entry.value\n}}"
        );
        rejected_at(&body, Code::CheckType, "ref other");
    }
}

#[test]
fn obsolete_place_bindings_and_durable_value_pins_are_refused() {
    rejected_at(
        r"pub fn read(id: int): int {
    place entry = ^entries[id]
    return 0
}",
        Code::ParseSyntax,
        "place entry",
    );
    rejected_at(
        r"pub fn read(): int {
    for id, entry in ^entries at most 2 {} on more {}
    return 0
}",
        Code::CheckUnsupported,
        "for id, entry",
    );
}

#[test]
fn a_key_only_traversal_can_skip_absent_payloads_explicitly() {
    accepted(
        r"pub fn sum(): int {
    var total = 0
    for id in ^entries at most 2 {
        ref entry = ^entries[id] else { continue }
        total += entry.value
    } on more { return -1 }
    return total
}",
    );
}

#[test]
fn child_creation_and_binding_need_no_parent_payload_proof() {
    accepted(
        r"pub fn write(parent: int, child: int): int {
    transaction {
        ^entries[parent].children[child] = Entry.children(value: 7)
        ref entry = ^entries[parent].children[child] else { return -1 }
        entry.value = entry.value + 1
        return entry.value
    }
}",
    );
}

#[test]
fn returning_after_an_absent_arm_erase_preserves_the_success_edge_outer_fact() {
    accepted(
        r"pub fn write(id: int): int {
    transaction {
        ref outer = ^entries[id] else { return -1 }
        ref inner = ^probes[id] else {
            delete outer
            return -2
        }
        outer.value = inner.value
        return outer.value
    }
}",
    );
}

const ERASE_HELPER: &str = "fn eraseOuter(id: int) { delete ^entries[id] }\n";

#[test]
fn a_prior_erasing_call_is_not_forgotten_by_a_later_entry_binding() {
    let body = format!(
        "{ERASE_HELPER}{}",
        r"pub fn write(id: int): int {
    transaction {
        ref outer = ^entries[id] else { return -1 }
        eraseOuter(id)
        ref inner = ^probes[id] else { return -2 }
        outer.value = inner.value
        return 0
    }
}"
    );
    rejected_at(
        &body,
        Code::CheckRequiresPresence,
        "outer.value = inner.value",
    );
}

#[test]
fn a_skipped_erasing_call_in_the_absent_arm_preserves_the_success_edge() {
    let body = format!(
        "{ERASE_HELPER}{}",
        r"pub fn write(id: int): int {
    transaction {
        ref outer = ^entries[id] else { return -1 }
        ref inner = ^probes[id] else {
            eraseOuter(id)
            return -2
        }
        outer.value = inner.value
        return outer.value
    }
}"
    );
    accepted(&body);
}

#[test]
fn an_absent_arm_that_erases_and_continues_invalidates_an_outer_loop_fact() {
    rejected_at(
        r"pub fn write(id: int): int {
    transaction {
        ref outer = ^entries[id] else { return -1 }
        for key in ^probes at most 2 {
            ref inner = ^probes[key] else {
                delete outer
                continue
            }
            outer.value = inner.value
        } on more { return -2 }
        return 0
    }
}",
        Code::CheckRequiresPresence,
        "outer.value = inner.value",
    );
}

#[test]
fn a_loop_absence_exit_keeps_erasing_call_effects_for_post_loop_uses() {
    for exit in ["break", "continue"] {
        let body = format!(
            "{ERASE_HELPER}pub fn write(id: int): int {{\n    transaction {{\n        ref outer = ^entries[id] else {{ return -1 }}\n        for key in ^probes at most 2 {{\n            ref inner = ^probes[key] else {{\n                eraseOuter(id)\n                {exit}\n            }}\n        }} on more {{ return -2 }}\n        outer.value = 7\n        return 0\n    }}\n}}"
        );
        rejected_at(&body, Code::CheckRequiresPresence, "outer.value = 7");
    }
}

#[test]
fn a_loop_absence_exit_keeps_direct_erase_effects_for_post_loop_uses() {
    for exit in ["break", "continue"] {
        let body = format!(
            "pub fn write(id: int): int {{\n    transaction {{\n        ref outer = ^entries[id] else {{ return -1 }}\n        for key in ^probes at most 2 {{\n            ref inner = ^probes[key] else {{\n                delete outer\n                {exit}\n            }}\n        }} on more {{ return -2 }}\n        outer.value = 7\n        return 0\n    }}\n}}"
        );
        rejected_at(&body, Code::CheckRequiresPresence, "outer.value = 7");
    }
}
