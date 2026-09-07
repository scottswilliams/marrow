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
    }
}
"#
    );
    let image = compile_verify(&source);
    assert_eq!(count_strict(export_instrs(&image, "put")), 1);
}

fn assignment_with_rhs(rhs: &str) -> String {
    const HELPERS: &str = r#"
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
"#;
    format!(
        "{HEADER}{HELPERS}
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

#[test]
fn a_direct_rhs_erase_requires_a_fresh_presence_fact() {
    let source = assignment_with_rhs("eraseAndValue(n)");
    assert_presence_diagnostic(&source, IDS, "p.label =");
}

#[test]
fn a_transitive_rhs_erase_requires_a_fresh_presence_fact() {
    let source = assignment_with_rhs("relay(n)");
    assert_presence_diagnostic(&source, IDS, "p.label =");
}

#[test]
fn an_argument_erase_requires_a_fresh_presence_fact() {
    let source = assignment_with_rhs("identity(eraseAndValue(n))");
    assert_presence_diagnostic(&source, IDS, "p.label =");
}

#[test]
fn a_transitive_argument_erase_requires_a_fresh_presence_fact() {
    let source = assignment_with_rhs("identity(relay(n))");
    assert_presence_diagnostic(&source, IDS, "p.label =");
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
        }
    }
}
"#
    );
    let image = compile_verify_with_ids(&source, &two_family_ids());
    assert_eq!(count_strict(export_instrs(&image, "put")), 1);
}
