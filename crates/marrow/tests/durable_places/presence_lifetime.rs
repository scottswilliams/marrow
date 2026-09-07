//! Presence follows lexical scope and the effects evaluated before a protected use.

use super::{
    HEADER, IDS, REQUIRES_PRESENCE, compile_diagnostics_with_ids, compile_verify,
    compile_verify_with_ids, count_strict, export_instrs, position_of,
};

fn two_family_ids() -> String {
    IDS.replace(
        "high-water 0\n",
        concat!(
            "id root other 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n",
            "id key other.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n",
            "high-water 0\n",
        ),
    )
}

fn assert_presence_diagnostic(source: &str, ids: &str, write: &str) {
    let (line, column) = position_of(source, write);
    assert_eq!(
        compile_diagnostics_with_ids(source, ids),
        vec![(REQUIRES_PRESENCE.to_string(), line, column)],
    );
}

pub(super) fn assert_read_instead_of_write_requires_presence(source: &str, write: &str) {
    assert!(source.contains(write), "the protected write is present");
    let source = source.replace(write, "const value: int? = p.value");
    assert_presence_diagnostic(&source, IDS, "p.value");
}

#[test]
fn an_invalidated_required_read_refuses_an_optional_context() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn readAfterErase(n: int): int? {
    transaction {
        place p = ^counters[n]
        if exists(p) {
            delete p
            return p.value
        }
        return absent
    }
}
"#
    );
    assert_presence_diagnostic(&source, IDS, "p.value");
}

#[test]
fn an_inner_fact_cannot_escape_when_an_outer_family_is_erased() {
    let source = format!(
        "{HEADER}{}",
        r#"
store ^other[id: int]: Counter

pub fn put(n: int) {
    transaction {
        place outer = ^counters[n]
        place inner = ^other[n]
        if exists(outer) {
            if n > 0 {
                delete outer
                inner = Counter(value: 1)
            }
            inner.label = "outside the inner scope"
        }
    }
}
"#
    );
    assert_presence_diagnostic(&source, &two_family_ids(), "inner.label =");
}

#[test]
fn a_let_else_lifts_a_fresh_fact_after_an_outer_family_erase() {
    let source = format!(
        "{HEADER}{}",
        r#"
store ^other[id: int]: Counter

pub fn put(n: int) {
    transaction {
        place outer = ^counters[n]
        place lifted = ^other[n]
        if exists(outer) {
            delete outer
            const value = lifted else {
                return
            }
            lifted.label = "fresh proof"
            const proved: int = lifted.value
        }
    }
}
"#
    );
    let image = compile_verify_with_ids(&source, &two_family_ids());
    assert_eq!(count_strict(export_instrs(&image, "put")), 1);
}

#[test]
fn an_outer_erase_in_the_diverging_arm_keeps_the_restored_fact() {
    let source = format!(
        "{HEADER}{}",
        r#"
store ^other[id: int]: Counter

pub fn put(n: int) {
    transaction {
        place outer = ^counters[n]
        place lifted = ^other[n]
        if exists(outer) {
            const value = lifted else {
                delete outer
                return
            }
            lifted.label = "restored proof"
            const proved: int = lifted.value
        }
    }
}
"#
    );
    let image = compile_verify_with_ids(&source, &two_family_ids());
    assert_eq!(count_strict(export_instrs(&image, "put")), 1);
}

#[test]
fn an_erase_call_in_the_skipped_let_else_arm_keeps_the_lifted_fact() {
    let source = format!(
        "{HEADER}{}",
        r#"
fn wipe(n: int) {
    delete ^counters[n]
}

pub fn put(n: int) {
    transaction {
        place p = ^counters[n]
        const value = p else {
            wipe(n)
            return
        }
        p.label = "the erase arm was skipped"
        const proved: int = p.value
    }
}
"#
    );
    let image = compile_verify(&source);
    assert_eq!(count_strict(export_instrs(&image, "put")), 1);
}

const ERASE_HELPERS: &str = r#"
fn eraseAndValue(n: int): string {
    delete ^counters[n]
    return "erased"
}

fn relay(n: int): string {
    return eraseAndValue(n)
}

fn identity(value: string): string {
    return value
}

fn eraseGeneric<T>(n: int, value: T): T {
    delete ^counters[n]
    return value
}

fn second(before: string, value: int): int {
    return value
}
"#;

fn assignment_with_rhs(rhs: &str) -> String {
    format!(
        "{HEADER}{ERASE_HELPERS}
pub fn put(n: int) {{
    transaction {{
        place p = ^counters[n]
        if exists(p) {{
            p.label = {rhs}
        }}
    }}
}}
"
    )
}

fn assert_operand_erase_invalidates_read(rhs: &str) {
    let source = assignment_with_rhs(rhs).replace(
        &format!("p.label = {rhs}"),
        &format!("const evaluated = {rhs}\n            const read: int? = p.value"),
    );
    assert_presence_diagnostic(&source, IDS, "p.value");
}

#[test]
fn a_direct_rhs_erase_requires_a_fresh_presence_fact() {
    let source = assignment_with_rhs("eraseAndValue(n)");
    assert_presence_diagnostic(&source, IDS, "p.label =");
    assert_operand_erase_invalidates_read("eraseAndValue(n)");
}

#[test]
fn a_transitive_rhs_erase_requires_a_fresh_presence_fact() {
    let source = assignment_with_rhs("relay(n)");
    assert_presence_diagnostic(&source, IDS, "p.label =");
    assert_operand_erase_invalidates_read("relay(n)");
}

#[test]
fn an_argument_erase_requires_a_fresh_presence_fact() {
    let source = assignment_with_rhs("identity(eraseAndValue(n))");
    assert_presence_diagnostic(&source, IDS, "p.label =");
    assert_operand_erase_invalidates_read("identity(eraseAndValue(n))");
}

#[test]
fn a_transitive_argument_erase_requires_a_fresh_presence_fact() {
    let source = assignment_with_rhs("identity(relay(n))");
    assert_presence_diagnostic(&source, IDS, "p.label =");
    assert_operand_erase_invalidates_read("identity(relay(n))");
}

#[test]
fn a_generic_erase_and_an_earlier_argument_invalidate_a_required_read() {
    assert_operand_erase_invalidates_read("eraseGeneric(n, \"erased\")");
    let source = assignment_with_rhs("identity(\"unused\")").replace(
        "p.label = identity(\"unused\")",
        "const read = second(eraseAndValue(n), p.value)",
    );
    assert_presence_diagnostic(&source, IDS, "p.value");
}

#[test]
fn erasure_invalidates_hidden_outer_facts_after_a_fresh_inner_guard_ends() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn inspect(n: int): int? {
    transaction {
        place p = ^counters[n]
        if exists(p) {
            if exists(p) { delete p }
            if exists(p) { const fresh: int = p.value }
            return p.value
        }
        return absent
    }
}
"#
    );
    let (line, column) = super::position_of(&source, "return p.value");
    assert_eq!(
        compile_diagnostics_with_ids(&source, IDS),
        vec![(
            REQUIRES_PRESENCE.to_string(),
            line,
            column + "return ".len() as u32
        )],
    );
}

#[test]
fn leaving_an_inner_proof_scope_restores_an_untested_optional_read() {
    for inner in [
        "if exists(p) { const proved: int = p.value }",
        "if exists(p) { delete p }",
    ] {
        let source = format!(
            "{HEADER}\npub fn inspect(n: int): int? {{
    transaction {{
        place p = ^counters[n]
        {inner}
        const untested: int? = p.value
        return untested
    }}
}}\n"
        );
        let image = compile_verify(&source);
        assert_eq!(
            export_instrs(&image, "inspect")
                .iter()
                .filter(|op| matches!(op, marrow_verify::SealedInstr::DurReadField(_)))
                .count(),
            1,
        );
    }
}

#[test]
fn copied_values_and_sparse_reads_survive_entry_erasure() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn inspect(n: int): int? {
    transaction {
        place p = ^counters[n]
        if exists(p) {
            const before: int = p.value
            delete p
            const sparse: string? = p.label
            if (sparse ?? "") == "bonus" { return before + 1 }
            return before
        }
        return absent
    }
}
"#
    );
    let image = compile_verify(&source);
    let code = export_instrs(&image, "inspect");
    assert_eq!(
        code.iter()
            .filter(|op| matches!(op, marrow_verify::SealedInstr::DurReadFieldPresent { .. }))
            .count(),
        1
    );
    assert_eq!(
        code.iter()
            .filter(|op| matches!(op, marrow_verify::SealedInstr::DurReadField(_)))
            .count(),
        1
    );
}

#[test]
fn a_replacement_only_helper_preserves_entry_presence() {
    let source = format!(
        "{HEADER}{}",
        r#"
fn replace(n: int) {
    ^counters[n] = Counter(value: 2)
}

pub fn put(n: int) {
    transaction {
        place p = ^counters[n]
        if exists(p) {
            replace(n)
            p.label = "after replacement"
            delete p.label
            const proved: int = p.value
        }
    }
}
"#
    );
    let image = compile_verify(&source);
    assert_eq!(count_strict(export_instrs(&image, "put")), 1);
}

#[test]
fn an_ordinary_error_value_does_not_restore_an_erased_fact() {
    let source = format!(
        "{HEADER}{}",
        r#"
fn eraseAndError(n: int): Result<int, string> {
    delete ^counters[n]
    return err("erased")
}

pub fn put(n: int) {
    transaction {
        place p = ^counters[n]
        if exists(p) {
            const outcome = eraseAndError(n)
            match outcome {
                ok(value) => {
                }
                err(message) => {
                    p.label = "after ordinary error"
                }
            }
        }
    }
}
"#
    );
    assert_presence_diagnostic(&source, IDS, "p.label =");
    assert_read_instead_of_write_requires_presence(&source, "p.label = \"after ordinary error\"");
}

#[test]
fn a_fact_of_another_family_survives_an_entry_erase() {
    let source = format!(
        "{HEADER}{}",
        r#"
store ^other[id: int]: Counter

pub fn put(n: int) {
    transaction {
        place erased = ^counters[n]
        place kept = ^other[n]
        if exists(erased) {
            kept = Counter(value: 3)
            delete erased
            kept.label = "other family"
            const proved: int = kept.value
        }
    }
}
"#
    );
    let image = compile_verify_with_ids(&source, &two_family_ids());
    assert_eq!(count_strict(export_instrs(&image, "put")), 1);
}
