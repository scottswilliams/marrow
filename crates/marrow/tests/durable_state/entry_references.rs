//! Checked entry references capture key operands once and carry presence facts.
//! The production compile/verify path checks operand order, field access, and
//! invalidation by entry erasure, including calls and loop back edges.

use crate::common::Project;
use marrow_verify::{SealedInstr, VerifiedImage};

#[path = "entry_references/presence_lifetime.rs"]
mod presence_lifetime;

#[path = "entry_references/binding_presence.rs"]
mod binding_presence;

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

const HEADER: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

fn keyOf(n: int): int {
    return n + 100
}
"#;

fn compile_verify(source: &str) -> VerifiedImage {
    compile_verify_with_ids(source, IDS)
}

fn compile_verify_with_ids(source: &str, ids: &str) -> VerifiedImage {
    Project::single(source).ids(ids).image()
}

/// The typed diagnostic codes a source that fails to compile carries.
fn compile_error_codes(source: &str) -> Vec<String> {
    compile_diagnostics(source)
        .into_iter()
        .map(|(code, _, _)| code)
        .collect()
}

/// The instruction stream of the function named `name`.
fn export_instrs<'a>(image: &'a VerifiedImage, name: &str) -> &'a [SealedInstr] {
    image
        .functions()
        .iter()
        .find(|function| function.name() == name)
        .expect("function present")
        .instrs()
}

/// Whether the verified image holds a function named `name`.
fn has_function(image: &VerifiedImage, name: &str) -> bool {
    image
        .functions()
        .iter()
        .any(|function| function.name() == name)
}

// --- Operand timing. ---

/// The key operand of a `ref` is lowered exactly once, at the binding, no matter
/// how many operations flow through the reference. Here the key is a call `keyOf(n)`,
/// and the reference is used three times (`exists`, and two field reads); the compiled
/// `use3` export therefore holds exactly one `Call` (the one key evaluation) while
/// carrying three durable effect sites.
#[test]
fn a_reference_key_operand_is_lowered_exactly_once() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn use3(n: int): int {
    ref p = ^counters[keyOf(n)] else { return 0 }
    const a = p.value
    const b = p.value
    return a + b
}
"#
    );
    let image = compile_verify(&source);
    let instrs = export_instrs(&image, "use3");

    let calls = instrs
        .iter()
        .filter(|instr| matches!(instr, SealedInstr::Call(_)))
        .count();
    assert_eq!(
        calls, 1,
        "the reference key call `keyOf(n)` is evaluated once at the binding, not per use"
    );

    // Three operations flow through the reference: one presence test and two field
    // reads. Each is its own effect site (compact sites, no cloned summaries).
    let exists = instrs
        .iter()
        .filter(|instr| matches!(instr, SealedInstr::DurExists(_)))
        .count();
    let reads = instrs
        .iter()
        .filter(|instr| matches!(instr, SealedInstr::DurReadFieldPresent { .. }))
        .count();
    assert_eq!(exists, 1, "one presence effect site");
    assert_eq!(reads, 2, "two field-read effect sites");
}

/// The binding evaluates and stores its key before testing presence. An
/// operand that faults at the binding therefore faults before any durable
/// operation is recorded — the effect sites are unreachable past the fault.
#[test]
fn an_entry_binding_evaluates_its_key_before_the_presence_test() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn readIt(n: int): int {
    ref p = ^counters[keyOf(n)] else { return 0 }
    return p.value
}
"#
    );
    let image = compile_verify(&source);
    let instrs = export_instrs(&image, "readIt");

    let call_at = instrs
        .iter()
        .position(|instr| matches!(instr, SealedInstr::Call(_)))
        .expect("the key call is emitted");
    let first_site = instrs.iter().position(|instr| {
        matches!(
            instr,
            SealedInstr::DurExists(_)
                | SealedInstr::DurReadField(_)
                | SealedInstr::DurReadEntry(_)
                | SealedInstr::DurSetField { .. }
                | SealedInstr::DurCreateEntry(_)
                | SealedInstr::DurReplaceEntry(_)
                | SealedInstr::DurEraseField(_)
                | SealedInstr::DurEraseEntry(_)
        )
    });
    // A key eval exists, and every effect site follows it.
    match first_site {
        Some(site_at) => assert!(
            call_at < site_at,
            "the key operand is evaluated before any durable effect site"
        ),
        None => panic!("the read export must carry a durable effect site"),
    }
}

/// The whole-entry write form `p = Record(...)` and the field/erase forms all flow
/// through the reference's one pre-evaluated key: the mutating export reads the key
/// slot for each operation and never re-calls `keyOf`.
#[test]
fn a_reference_reused_across_writes_evaluates_its_key_once() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn writeIt(n: int, v: int) {
    transaction {
        const key = keyOf(n)
        ^counters[key] = Counter(value: v)
        ref p = ^counters[key] else { unreachable("created entry missing") }
        p.label = "tag"
        delete p.label
        const proved: int = p.value
    }
}
"#
    );
    let image = compile_verify(&source);
    let instrs = export_instrs(&image, "writeIt");
    let calls = instrs
        .iter()
        .filter(|instr| matches!(instr, SealedInstr::Call(_)))
        .count();
    assert_eq!(
        calls, 1,
        "the key is evaluated once even across several writes"
    );
    assert_eq!(
        instrs
            .iter()
            .filter(|op| matches!(op, SealedInstr::DurReadFieldPresent { .. }))
            .count(),
        1
    );
    // The whole-entry upsert (create/replace) plus the sparse set and erase sites.
    assert!(
        instrs
            .iter()
            .any(|instr| matches!(instr, SealedInstr::DurCreateEntry(_))),
        "the upsert lowers a create site"
    );
    assert!(
        instrs
            .iter()
            .any(|instr| matches!(instr, SealedInstr::DurEraseField(_))),
        "the field delete lowers an erase site"
    );
}

// --- Structured presence analysis: every field set is the present-entry form. ---

fn count_strict(instrs: &[SealedInstr]) -> usize {
    instrs
        .iter()
        .filter(|i| matches!(i, SealedInstr::DurSetField { .. }))
        .count()
}

/// A field set through a `ref` dominated by an `exists(p)` guard lowers to the
/// present-entry form (`DurSetField`), which reads the key from the reference's slot and
/// asserts the entry present; the same set with no dominating guard is refused at
/// check time, since a write never creates an entry.
#[test]
fn an_exists_guarded_sparse_set_is_strict() {
    let guarded = format!(
        "{HEADER}{}",
        r#"
pub fn tag(n: int) {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if exists(p) {
            p.label = "x"
        }
    }
}
"#
    );
    let instrs = compile_verify(&guarded);
    let instrs = export_instrs(&instrs, "tag");
    assert_eq!(count_strict(instrs), 1, "the guarded set lowers strict");

    let unguarded = format!(
        "{HEADER}{}",
        r#"
pub fn tag(n: int) {
    transaction {
        ^counters[n].label = "x"
    }
}
"#
    );
    assert_eq!(
        compile_error_codes(&unguarded),
        vec!["check.requires_presence".to_string()],
        "an unguarded set is refused"
    );
}

/// An `if const c = p` entry read proves the entry present in its then-block, so a
/// sparse set through the same reference there is strict.
#[test]
fn an_if_const_guarded_sparse_set_is_strict() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn tag(n: int) {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if const c = p {
            p.label = "x"
        }
    }
}
"#
    );
    let image = compile_verify(&source);
    let instrs = export_instrs(&image, "tag");
    assert_eq!(count_strict(instrs), 1);
}

/// Creation followed by a checked binding permits a sparse field update.
#[test]
fn a_sparse_set_after_creation_and_binding_is_strict() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn tag(n: int, v: int) {
    transaction {
        ^counters[n] = Counter(value: v)
        ref p = ^counters[n] else { unreachable("created entry missing") }
        p.label = "x"
    }
}
"#
    );
    let image = compile_verify(&source);
    let instrs = export_instrs(&image, "tag");
    assert_eq!(count_strict(instrs), 1, "the post-upsert set is strict");
}

/// Presence facts attach to a lexical `ref` binding only: an inline `^root(k)`
/// address never carries one, so an inline field set is refused even under a guard.
#[test]
fn an_inline_sparse_set_is_never_proven() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn tag(n: int) {
    transaction {
        if exists(^counters[n]) {
            ^counters[n].label = "x"
        }
    }
}
"#
    );
    assert_eq!(
        compile_error_codes(&source),
        vec!["check.requires_presence".to_string()]
    );
}

/// A presence fact does not survive a `delete p`: a field set after the entry is
/// erased is refused (the compiler drops the fact; the verifier would reject a
/// present-entry set there).
#[test]
fn a_sparse_set_after_delete_is_refused() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn tag(n: int) {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if exists(p) {
            delete p
            p.label = "x"
        }
    }
}
"#
    );
    assert_eq!(
        compile_error_codes(&source),
        vec!["check.requires_presence".to_string()],
        "presence is killed by the erase"
    );
}

/// The fact does not leak past the guarded block: a field set after the `if
/// exists(p)` block closes is refused, since the entry is not known present there.
#[test]
fn the_presence_fact_does_not_outlive_its_block() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn tag(n: int) {
    transaction {
        if exists(^counters[n]) {
            ref p = ^counters[n] else { return }
            p.label = "in"
        }
        ^counters[n].label = "out"
    }
}
"#
    );
    assert_eq!(
        compile_error_codes(&source),
        vec!["check.requires_presence".to_string()],
        "the post-block set is refused"
    );
}

/// Two references over distinct entries, each guarded and set in its own block,
/// interleaved: the presence fact is keyed to the reference it was proven for, so inside
/// `if exists(p)` only a set through `p` is admitted — a set through the co-resident,
/// unguarded `q` is refused — and the mirror holds inside `if exists(q)`. The two
/// facts never merge across references, and neither survives past its own block.
#[test]
fn interleaved_guarded_references_keep_independent_presence_facts() {
    let own = format!(
        "{HEADER}{}",
        r#"
pub fn tag(a: int, b: int) {
    transaction {
        ref p = ^counters[a] else { unreachable("missing") }
        ref q = ^counters[b] else { unreachable("missing") }
        if exists(p) {
            p.label = "p-strict"
        }
        if exists(q) {
            q.label = "q-strict"
        }
    }
}
"#
    );
    let image = compile_verify(&own);
    assert_eq!(
        count_strict(export_instrs(&image, "tag")),
        2,
        "each reference is written inside its own guard"
    );

    let crossed = format!(
        "{HEADER}{}",
        r#"
pub fn tag(a: int, b: int) {
    transaction {
        ref p = ^counters[a] else { return }
        ^counters[b].label = "q-unproven"
        ref q = ^counters[b] else { return }
        ^counters[a].label = "p-unproven"
    }
}
"#
    );
    assert_eq!(
        compile_error_codes(&crossed),
        vec![
            "check.requires_presence".to_string(),
            "check.requires_presence".to_string()
        ],
        "one reference's fact never covers another"
    );
}

// --- Scope and type rules. ---

/// A `ref` must name a whole durable entry address. A non-durable value, a
/// field-projected address, another reference, and a re-binding of an existing name are
/// each a typed `check.type` diagnostic.
#[test]
fn a_reference_must_name_a_whole_durable_entry() {
    let non_durable = format!(
        "{HEADER}{}",
        "pub fn f(): int {\n    ref p = 5 else { unreachable(\"missing\") }\n    return 0\n}\n"
    );
    assert!(compile_error_codes(&non_durable).contains(&"check.type".to_string()));

    let field = format!(
        "{HEADER}{}",
        "pub fn f(n: int): int {\n    ref p = ^counters[n].value else { unreachable(\"missing\") }\n    return 0\n}\n"
    );
    assert!(compile_error_codes(&field).contains(&"check.type".to_string()));

    let another_reference = format!(
        "{HEADER}{}",
        "pub fn f(n: int): int {\n    ref p = ^counters[n] else { unreachable(\"missing\") }\n    ref q = p else { unreachable(\"missing\") }\n    return 0\n}\n"
    );
    assert!(compile_error_codes(&another_reference).contains(&"check.type".to_string()));

    let rebind = format!(
        "{HEADER}{}",
        "pub fn f(n: int): int {\n    ref p = ^counters[n] else { unreachable(\"missing\") }\n    ref p = ^counters[n] else { unreachable(\"missing\") }\n    return 0\n}\n"
    );
    assert!(compile_error_codes(&rebind).contains(&"check.type".to_string()));
}

/// A reference is a durable designation, not a first-class value: using its bare name in
/// value position (passing it, returning it) is a typed `check.type` diagnostic,
/// while `p.field`, `if const`, and `exists` are the read forms.
#[test]
fn a_bare_reference_name_is_not_a_value() {
    let returned = format!(
        "{HEADER}{}",
        "pub fn f(n: int): int {\n    ref p = ^counters[n] else { unreachable(\"missing\") }\n    return p\n}\n"
    );
    assert!(compile_error_codes(&returned).contains(&"check.type".to_string()));

    let passed = format!(
        "{HEADER}{}",
        "pub fn f(n: int): int {\n    ref p = ^counters[n] else { unreachable(\"missing\") }\n    return keyOf(p)\n}\n"
    );
    assert!(compile_error_codes(&passed).contains(&"check.type".to_string()));
}

/// A reference name and a value binding stay distinct: declaring a `const`/`var` that
/// reuses an in-scope reference name is a typed `check.type` diagnostic, so a name
/// resolves to exactly one of a reference or a value.
#[test]
fn a_value_binding_cannot_reuse_a_reference_name() {
    let shadowed = format!(
        "{HEADER}{}",
        "pub fn f(n: int): int {\n    ref p = ^counters[n] else { unreachable(\"missing\") }\n    const p = 1\n    return p\n}\n"
    );
    assert!(compile_error_codes(&shadowed).contains(&"check.type".to_string()));
}

/// Every reference operation form compiles and verifies over the executable flat scalar
/// root, so the image is well-formed and identity-complete (execution is parked in
/// the trough). One export exercises the whole algebra through a reference.
#[test]
fn every_reference_operation_form_compiles_and_verifies() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn present(n: int): bool {
    return exists(^counters[n])
}

pub fn titleOrZero(n: int): int {
    ref p = ^counters[n] else { unreachable("missing") }
    if const c = p {
        return c.value
    }
    return 0
}

pub fn edit(n: int, v: int) {
    transaction {
        ^counters[n] = Counter(value: v)
        ref p = ^counters[n] else { unreachable("created entry missing") }
        p.label = "x"
        delete p
    }
}
"#
    );
    let image = compile_verify(&source);
    for name in ["present", "titleOrZero", "edit"] {
        assert!(
            has_function(&image, name),
            "export `{name}` is present in the verified image"
        );
    }
}

// --- Complete entries: a field write updates an entry the compiler has proved present. ---
//
// A field write through a reference needs a presence fact that no entry erase in the
// family — direct, through another binding, or inside a called helper — has ended.

/// `(code, line, column)` of every diagnostic a source that fails to compile carries.
fn compile_diagnostics(source: &str) -> Vec<(String, u32, u32)> {
    compile_diagnostics_with_ids(source, IDS)
}

fn compile_diagnostics_with_ids(source: &str, ids: &str) -> Vec<(String, u32, u32)> {
    match Project::single(source).ids(ids).try_image() {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics
            .all()
            .into_iter()
            .map(|(code, line, column)| (code.to_string(), line, column))
            .collect(),
    }
}

/// The 1-based `(line, column)` of the first occurrence of `needle` in `source`.
fn position_of(source: &str, needle: &str) -> (u32, u32) {
    let offset = source.find(needle).expect("the needle is in the source");
    let line = source[..offset].matches('\n').count() as u32 + 1;
    let column = source[..offset].rsplit('\n').next().map_or(0, str::len) as u32 + 1;
    (line, column)
}

/// The refusal a field write without a live presence fact reports.
const REQUIRES_PRESENCE: &str = "check.requires_presence";

/// A presence fact ends at a call that can erase an entry in the guarded family:
/// `wipe(n)` erases `^counters[n]` inside the guarded block, so the sparse set after it
/// is refused at check time with a typed code at the write.
#[test]
fn a_sparse_set_after_a_helper_erase_of_the_family_is_refused_at_check() {
    let source = format!(
        "{HEADER}{}",
        r#"
fn wipe(n: int) {
    delete ^counters[n]
}

pub fn provedThenHelperErase(n: int): bool {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if exists(p) {
            wipe(n)
            p.label = "after helper erase"
            return true
        }
        return false
    }
}
"#
    );
    let (line, column) = position_of(&source, "p.label = \"after helper erase\"");
    assert_eq!(
        compile_diagnostics(&source),
        vec![(REQUIRES_PRESENCE.to_string(), line, column)],
        "the set after the helper erase is refused at the write, with no other report"
    );
}

/// A presence fact ends at any erase of the entry's family in the same region, whichever
/// binding spells it: `delete b` erases the entry `a` was proven for, so the sparse set
/// through `a` is refused at check time.
#[test]
fn a_sparse_set_after_an_erase_through_another_reference_is_refused_at_check() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn provedThenAliasErase(n: int): bool {
    transaction {
        ref a = ^counters[n] else { unreachable("missing") }
        ref b = ^counters[n] else { unreachable("missing") }
        if exists(a) {
            delete b
            a.label = "after alias erase"
            return true
        }
        return false
    }
}
"#
    );
    let (line, column) = position_of(&source, "a.label = \"after alias erase\"");
    assert_eq!(
        compile_diagnostics(&source),
        vec![(REQUIRES_PRESENCE.to_string(), line, column)],
        "the set after the erase through the other binding is refused at the write"
    );
}

/// Whole-entry assignment is the only way an entry comes into existence: a required
/// field write on an entry no fact proves present is refused at check time.
#[test]
fn a_field_write_on_an_unproven_entry_is_refused_at_check() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn create(n: int, v: int) {
    transaction {
        ^counters[n].value = v
    }
}
"#
    );
    let (line, column) = position_of(&source, "^counters[n].value = v");
    assert_eq!(
        compile_diagnostics(&source),
        vec![(REQUIRES_PRESENCE.to_string(), line, column)],
        "a field write never creates an entry; it is refused where no presence fact holds"
    );
}

/// `p.value` reads as `int` inside
/// `if exists(p)`, while `p.label` stays `string?`.
#[test]
fn a_required_field_reads_bare_through_a_reference_proven_present() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn valueOf(n: int): int {
    ref p = ^counters[n] else { unreachable("missing") }
    if exists(p) {
        const v: int = p.value
        const l: string? = p.label
        if (l ?? "") == "bonus" { return v + 1 }
        return v
    }
    return 0
}
"#
    );
    assert_eq!(
        compile_diagnostics(&source),
        Vec::<(String, u32, u32)>::new(),
        "a required read through a proven reference has its declared type"
    );
    let image = compile_verify(&source);
    assert!(has_function(&image, "valueOf"));
}

/// A loop body is one region: a write through `p` inside the loop precedes, on the
/// back edge, the erase of `p`'s family later in the same body, so the write is
/// refused at check time when the loop closes. Both the direct erase and a helper
/// that erases an entry in the family end the proof.
#[test]
fn a_field_write_inside_a_loop_that_erases_the_family_is_refused_at_check() {
    let direct = format!(
        "{HEADER}{}",
        r#"
pub fn writeThenEraseInLoop(n: int): bool {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if exists(p) {
            for k in ^counters at most 10 {
                p.label = "in loop"
                delete p
            } on more {
            }
            return true
        }
        return false
    }
}
"#
    );
    let (line, column) = position_of(&direct, "p.label = \"in loop\"");
    assert_eq!(
        compile_diagnostics(&direct),
        vec![(REQUIRES_PRESENCE.to_string(), line, column)],
        "the write precedes the erase on the loop's back edge"
    );
    presence_lifetime::assert_read_instead_of_write_requires_presence(
        &direct,
        "p.label = \"in loop\"",
    );

    let through_helper = format!(
        "{HEADER}{}",
        r#"
fn wipe(n: int) {
    delete ^counters[n]
}

pub fn writeThenHelperInLoop(n: int): bool {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if exists(p) {
            for k in ^counters at most 10 {
                p.label = "in loop"
                wipe(k)
            } on more {
            }
            return true
        }
        return false
    }
}
"#
    );
    let (line, column) = position_of(&through_helper, "p.label = \"in loop\"");
    assert_eq!(
        compile_diagnostics(&through_helper),
        vec![(REQUIRES_PRESENCE.to_string(), line, column)],
        "a call that erases an entry in the family ends the fact on the loop's back edge"
    );
    presence_lifetime::assert_read_instead_of_write_requires_presence(
        &through_helper,
        "p.label = \"in loop\"",
    );
}

/// An erase of the family ends the fact whatever key it names: `delete ^counters[n + 1]`
/// may or may not be `p`'s entry, and the rule does not reason about keys.
#[test]
fn an_inline_erase_of_another_key_in_the_family_is_refused_at_check() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn eraseNeighbourThenSet(n: int): bool {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if exists(p) {
            delete ^counters[n + 1]
            p.label = "after neighbour erase"
            return true
        }
        return false
    }
}
"#
    );
    let (line, column) = position_of(&source, "p.label = \"after neighbour erase\"");
    assert_eq!(
        compile_diagnostics(&source),
        vec![(REQUIRES_PRESENCE.to_string(), line, column)],
        "an erase of the family, on any key, ends every fact in the family"
    );
}

/// `if not exists(p) { return … }` proves `p` present for the rest of the block, the
/// same way a let-else does: the guarded block diverges, so control reaches the
/// continuation only with the entry present. The set after the guard compiles and is
/// strict.
#[test]
fn a_negative_diverging_guard_carries_the_fact_into_the_continuation() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn setLabelIfPresent(n: int): bool {
    transaction {
        ref p = ^counters[n] else {
            return false
        }
        p.label = "after the guard"
        return true
    }
}
"#
    );
    assert_eq!(
        compile_diagnostics(&source),
        Vec::<(String, u32, u32)>::new(),
        "the continuation of a diverging negative guard is proven"
    );
    let image = compile_verify(&source);
    let instrs = export_instrs(&image, "setLabelIfPresent");
    assert_eq!(
        count_strict(instrs),
        1,
        "the set after the guard is the present-entry form"
    );
}

/// A sparse field is cleared by `delete p.f`: assigning `absent` to a durable
/// field is `check.type` at the write.
#[test]
fn a_durable_field_assigned_absent_is_refused_naming_delete() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn clearLabel(n: int) {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if exists(p) {
            p.label = absent
        }
    }
}
"#
    );
    let (line, column) = position_of(&source, "p.label = absent");
    assert_eq!(
        compile_diagnostics(&source),
        vec![("check.type".to_string(), line, column)],
        "`= absent` on a durable field is refused; `delete p.label` is the clearing form"
    );
}

/// A call whose demand only reads the family keeps the fact: `peek(n)` reads
/// `^counters` between the guard and the set, and the set stays strict; the same
/// program with an erasing helper is refused.
#[test]
fn a_read_only_helper_keeps_the_fact_while_an_erasing_helper_ends_it() {
    let reading = format!(
        "{HEADER}{}",
        r#"
fn peek(n: int): int? {
    return ^counters[n].value
}

pub fn peekThenSet(n: int): int {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if exists(p) {
            const seen = peek(n) ?? 0
            p.label = "after peek"
            return seen
        }
        return 0
    }
}
"#
    );
    assert_eq!(
        compile_diagnostics(&reading),
        Vec::<(String, u32, u32)>::new()
    );
    let image = compile_verify(&reading);
    assert_eq!(
        count_strict(export_instrs(&image, "peekThenSet")),
        1,
        "a read-only call leaves the fact in place"
    );

    let erasing = format!(
        "{HEADER}{}",
        r#"
fn wipe(n: int) {
    delete ^counters[n]
}

pub fn wipeThenSet(n: int): int {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if exists(p) {
            wipe(n)
            p.label = "after wipe"
            return 1
        }
        return 0
    }
}
"#
    );
    let (line, column) = position_of(&erasing, "p.label = \"after wipe\"");
    assert_eq!(
        compile_diagnostics(&erasing),
        vec![(REQUIRES_PRESENCE.to_string(), line, column)],
        "a call that erases an entry in the family ends the fact"
    );
}

/// A `while` body is a loop region like a bounded traversal: the write through `p`
/// precedes `delete p` on the back edge, so it is refused at the loop's close.
#[test]
fn a_field_write_inside_a_while_loop_that_erases_the_family_is_refused_at_check() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn whileErase(n: int): bool {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if exists(p) {
            var i = 0
            while i < 2 {
                p.label = "in while"
                delete p
                i += 1
            }
            return true
        }
        return false
    }
}
"#
    );
    let (line, column) = position_of(&source, "p.label = \"in while\"");
    assert_eq!(
        compile_diagnostics(&source),
        vec![(REQUIRES_PRESENCE.to_string(), line, column)],
        "a while body is one proof region"
    );
    presence_lifetime::assert_read_instead_of_write_requires_presence(
        &source,
        "p.label = \"in while\"",
    );
}

/// A write inside an inner loop is refused when the outer loop's body erases the
/// family after it: the obligation covers the outermost loop entered after the
/// fact was established, including its nested work, and that loop's close resolves it.
#[test]
fn a_field_write_inside_nested_loops_that_erase_the_family_is_refused_at_check() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn nestedErase(n: int): bool {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if exists(p) {
            for k in ^counters at most 10 {
                for j in 0..2 {
                    p.label = "nested"
                }
                delete p
            } on more {
            }
            return true
        }
        return false
    }
}
"#
    );
    let (line, column) = position_of(&source, "p.label = \"nested\"");
    assert_eq!(
        compile_diagnostics(&source),
        vec![(REQUIRES_PRESENCE.to_string(), line, column)],
        "the outer loop's erase reaches the inner loop's write on the back edge"
    );
    presence_lifetime::assert_read_instead_of_write_requires_presence(
        &source,
        "p.label = \"nested\"",
    );
}

/// A loop whose body erases the family ends the fact for everything after the loop,
/// whichever key the erase names.
#[test]
fn a_field_write_after_a_loop_that_erases_the_family_is_refused_at_check() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn eraseAllThenSet(n: int): bool {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if exists(p) {
            for k in ^counters at most 10 {
                delete ^counters[k]
            } on more {
            }
            p.label = "after the loop"
            return true
        }
        return false
    }
}
"#
    );
    let (line, column) = position_of(&source, "p.label = \"after the loop\"");
    assert_eq!(
        compile_diagnostics(&source),
        vec![(REQUIRES_PRESENCE.to_string(), line, column)],
        "an erase of the family inside the loop ends the fact past the loop"
    );
    presence_lifetime::assert_read_instead_of_write_requires_presence(
        &source,
        "p.label = \"after the loop\"",
    );
}

/// Only a diverging negative guard proves its continuation: when the block of
/// `if not exists(p)` falls through, the write after it has no proof.
#[test]
fn a_non_diverging_negative_guard_does_not_prove_the_continuation() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn noteThenSet(n: int): int {
    transaction {
        var missing = 0
        if not exists(^counters[n]) {
            missing = 1
        }
        ^counters[n].label = "unproven"
        return missing
    }
}
"#
    );
    let (line, column) = position_of(&source, "^counters[n].label = \"unproven\"");
    assert_eq!(
        compile_diagnostics(&source),
        vec![(REQUIRES_PRESENCE.to_string(), line, column)],
        "a negative guard that falls through proves nothing"
    );
}

/// A durable field set takes a bare value: an optional operand is `check.type` at the
/// write, naming `delete`, so clearing has one spelling.
#[test]
fn a_durable_field_assigned_an_optional_value_is_refused_naming_delete() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn setMaybe(n: int, flag: bool) {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if exists(p) {
            var maybe: string? = absent
            if flag {
                maybe = "set"
            }
            p.label = maybe
        }
    }
}
"#
    );
    let (line, column) = position_of(&source, "p.label = maybe");
    assert_eq!(
        compile_diagnostics(&source),
        vec![("check.type".to_string(), line, column)],
        "a `string?` operand is refused; `delete p.label` clears the field"
    );
}

/// The loop rule refuses a write that an entry erase in the same family, direct or
/// through a call, can precede on the back edge. A loop body that only updates
/// fields keeps its fact, and the set stays strict.
#[test]
fn a_field_write_inside_a_loop_with_no_erase_stays_accepted() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn relabelInLoop(n: int): bool {
    transaction {
        ref p = ^counters[n] else { unreachable("missing") }
        if exists(p) {
            for k in ^counters at most 10 {
                p.label = "each visit"
                const proved: int = p.value
            } on more {
            }
            return true
        }
        return false
    }
}
"#
    );
    assert_eq!(
        compile_diagnostics(&source),
        Vec::<(String, u32, u32)>::new()
    );
    let image = compile_verify(&source);
    assert_eq!(count_strict(export_instrs(&image, "relabelInLoop")), 1);
}

/// A reference bound inside the loop body on each iteration is
/// never refused by the loop rule, even when the same body erases the family after the
/// write: the fact was established after the loop was entered, so the back edge cannot
/// place the erase before the proof.
#[test]
fn a_per_iteration_reference_proof_stays_accepted() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn relabelThenErase(): int {
    transaction {
        var visited = 0
        for k in ^counters at most 10 {
            ref entry = ^counters[k] else { continue }
            entry.label = "visited"
            const proved: int = entry.value
            const untested: int? = ^counters[k].value
            delete entry
            visited += 1
        } on more {
        }
        return visited
    }
}
"#
    );
    assert_eq!(
        compile_diagnostics(&source),
        Vec::<(String, u32, u32)>::new()
    );
    let image = compile_verify(&source);
    assert_eq!(count_strict(export_instrs(&image, "relabelThenErase")), 1);
}
