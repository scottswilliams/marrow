//! The presence lattice is keyed by `(root, key-slot)`, not by key-slot alone. Two
//! int-keyed roots (`^aaa` + `^bbb`) share a resource shape — a required `tag` and a sparse
//! `note` — and a function reads a single key parameter `k` used against both roots, so the
//! same key-slot addresses both. A presence guard proving `^aaa[k]` present must not be
//! read as proving `^bbb[k]` present (no phantom marker), and a write named on `^bbb[k]`
//! must address `^bbb`, never `^aaa`.
//!
//! The negative case: guarded by `^aaa[k]` presence, a field write to `^bbb[k]` has no
//! proof of its own and is refused at check time — the guard over one root never proves
//! the sibling root's entry present.

use marrow_verify::VerifiedImage;
use marrow_vm::Value;

use crate::common::{Project, Session};

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Aaa 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Aaa.tag 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Aaa.note 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root aaa 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key aaa.k 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id product Bbb 1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d\n\
     id field Bbb.tag 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
     id field Bbb.note 1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f\n\
     id root bbb 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key bbb.k 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

const SOURCE: &str = r#"resource Aaa {
    required tag: string
    note: string
}

resource Bbb {
    required tag: string
    note: string
}

store ^aaa[k: int]: Aaa
store ^bbb[k: int]: Bbb

pub fn putAaa(k: int, tag: string) {
    transaction {
        ^aaa[k] = Aaa(tag: tag)
    }
}

pub fn putBbb(k: int, tag: string) {
    transaction {
        ^bbb[k] = Bbb(tag: tag)
    }
}

pub fn aaaNote(k: int): string? {
    return ^aaa[k].note
}

pub fn bbbTag(k: int): string? {
    return ^bbb[k].tag
}

pub fn bbbNote(k: int): string? {
    return ^bbb[k].note
}

pub fn setAaaNoteIfPresent(k: int, n: string) {
    transaction {
        place a = ^aaa[k]
        if const found = a {
            a.note = n
        }
    }
}
"#;

/// An ephemeral session over the two-root fixture.
fn open() -> Session {
    Project::single(SOURCE).ids(IDS).session()
}

fn text(v: &str) -> Value {
    Value::Text(v.into())
}

fn some_text(v: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(v.into())))))
}

fn absent() -> Option<Value> {
    Some(Value::Optional(None))
}

/// Positive control: a presence guard on `^aaa[k]` followed by a sparse write on that same
/// entry commits normally, and the sibling root `^bbb[k]` — sharing the key-slot — is never
/// touched.
#[test]
fn a_present_guarded_write_addresses_its_own_root_only() {
    let mut session = open();

    session.call("putAaa", vec![Value::Int(1), text("a")]);
    session.call("setAaaNoteIfPresent", vec![Value::Int(1), text("hello")]);

    assert_eq!(
        session.call("aaaNote", vec![Value::Int(1)]),
        some_text("hello"),
        "the present-guarded sparse write committed on ^aaa",
    );
    // ^bbb[1] shares the key-slot but was never written.
    assert_eq!(
        session.call("bbbTag", vec![Value::Int(1)]),
        absent(),
        "the sibling root ^bbb was not phantom-written",
    );
    assert_eq!(session.call("bbbNote", vec![Value::Int(1)]), absent(),);
}

/// A presence guard proving `^aaa[k]` present does not phantom-mark `^bbb[k]` present: a
/// write through a `^bbb` place inside the `^aaa` guard has no proof and is refused at
/// check time, so no write to the sibling root can rest on the wrong root's guard.
#[test]
fn a_cross_root_guarded_write_does_not_phantom_the_sibling_root() {
    let source = format!(
        "{TWO_ROOT_SCHEMA}
pub fn setBbbNoteUnderAaaGuard(k: int, n: string) {{
    transaction {{
        place a = ^aaa[k]
        place b = ^bbb[k]
        if exists(a) {{
            b.note = n
        }}
    }}
}}
"
    );
    assert_eq!(
        compile_source(&source).err(),
        Some(vec!["check.requires_presence".to_string()]),
        "a guard over ^aaa[k] proves nothing about ^bbb[k]",
    );
}

// --- Complete entries, design v2: a call ends facts by family, not by any write. ---

/// Compile and verify `source` against the two-root ledger: the verified image, or
/// the diagnostic codes when it does not compile.
fn compile_source(source: &str) -> Result<VerifiedImage, Vec<String>> {
    Project::single(source)
        .ids(IDS)
        .try_image()
        .map_err(|diagnostics| diagnostics.codes().iter().map(|c| c.to_string()).collect())
}

const TWO_ROOT_SCHEMA: &str = r#"resource Aaa {
    required tag: string
    note: string
}

resource Bbb {
    required tag: string
    note: string
}

store ^aaa[k: int]: Aaa
store ^bbb[k: int]: Bbb
"#;

/// A helper that erases `^bbb` leaves a fact on `^aaa[k]` in place, while the
/// same helper erasing `^aaa` ends it.
#[test]
fn an_other_root_helper_keeps_the_fact_while_a_same_root_helper_ends_it() {
    let other_root = format!(
        "{TWO_ROOT_SCHEMA}
fn touchBbb(k: int) {{
    delete ^bbb[k]
}}

pub fn setAaaNote(k: int, n: string) {{
    transaction {{
        place a = ^aaa[k]
        if exists(a) {{
            touchBbb(k)
            a.note = n
        }}
    }}
}}
"
    );
    let image = compile_source(&other_root).expect("an erase of another root keeps the fact");
    let strict = image
        .functions()
        .iter()
        .find(|function| function.name() == "setAaaNote")
        .expect("export present")
        .instrs()
        .iter()
        .filter(|instr| matches!(instr, marrow_verify::SealedInstr::DurSetField { .. }))
        .count();
    assert_eq!(strict, 1, "the set after the other-root helper is strict");

    let same_root = format!(
        "{TWO_ROOT_SCHEMA}
fn touchAaa(k: int) {{
    delete ^aaa[k]
}}

pub fn setAaaNote(k: int, n: string) {{
    transaction {{
        place a = ^aaa[k]
        if exists(a) {{
            touchAaa(k)
            a.note = n
        }}
    }}
}}
"
    );
    assert_eq!(
        compile_source(&same_root).err(),
        Some(vec!["check.requires_presence".to_string()]),
        "a helper that erases `^aaa` ends the fact"
    );
}
