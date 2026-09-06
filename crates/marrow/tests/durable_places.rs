//! D02 slice 2: source-local named `place` bindings and the operand-timing law.
//!
//! A `place p = ^root(key)` binding names one concrete durable entry address. Its
//! key tuple is evaluated exactly once at the binding; every operation through the
//! place (`p.field`, `p.field = v`, `p = Record(...)`, `exists(p)`, `delete p`,
//! `if const x = p`) reuses that pre-evaluated address rather than re-running the
//! key operand. The binding lowers to no new image structure — a `LocalSet` of the
//! key plus the ordinary per-operation effect sites — so these properties are
//! observed at the image level, through the full production path: capture ->
//! compile -> verify.

use marrow_verify::{SealedInstr, VerifiedImage};

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
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    marrow_verify::verify(&compiled.image.bytes).expect("verify")
}

/// The typed diagnostic codes a source that fails to compile carries.
fn compile_error_codes(source: &str) -> Vec<String> {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    match marrow_compile::compile(&project) {
        Ok(_) => Vec::new(),
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => {
            diagnostics.iter().map(|d| d.code().to_string()).collect()
        }
        Err(
            marrow_compile::CompileFailure::Invariant(_)
            | marrow_compile::CompileFailure::ResourceLimit(_),
        ) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
    }
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

// --- The operand-timing law. ---

/// The key operand of a `place` is lowered exactly once, at the binding, no matter
/// how many operations flow through the place. Here the key is a call `keyOf(n)`,
/// and the place is used three times (`exists`, and two field reads); the compiled
/// `use3` export therefore holds exactly one `Call` (the one key evaluation) while
/// carrying three durable effect sites.
#[test]
fn a_place_key_operand_is_lowered_exactly_once() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn use3(n: int): int {
    place p = ^counters[keyOf(n)]
    const present = exists(p)
    const a = p.value ?? 0
    const b = p.value ?? 0
    if present {
        return a + b
    }
    return 0
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
        "the place key call `keyOf(n)` is evaluated once at the binding, not per use"
    );

    // Three operations flow through the place: one presence test and two field
    // reads. Each is its own effect site (compact sites, no cloned summaries).
    let exists = instrs
        .iter()
        .filter(|instr| matches!(instr, SealedInstr::DurExists(_)))
        .count();
    let reads = instrs
        .iter()
        .filter(|instr| matches!(instr, SealedInstr::DurReadField(_)))
        .count();
    assert_eq!(exists, 1, "one presence effect site");
    assert_eq!(reads, 2, "two field-read effect sites");
}

/// The binding itself emits no durable effect site: it evaluates the key operand
/// and stores it, so the key evaluation strictly precedes every effect site. An
/// operand that faults at the binding therefore faults before any durable
/// operation is recorded — the effect sites are unreachable past the fault.
#[test]
fn a_place_binding_emits_no_effect_site_before_its_uses() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn readIt(n: int): int {
    place p = ^counters[keyOf(n)]
    return p.value ?? 0
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
                | SealedInstr::DurSetRequired(_)
                | SealedInstr::DurSetSparse(_)
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
/// through the place's one pre-evaluated key: the mutating export reads the key
/// slot for each operation and never re-calls `keyOf`.
#[test]
fn a_place_reused_across_writes_evaluates_its_key_once() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn writeIt(n: int, v: int) {
    transaction {
        place p = ^counters[keyOf(n)]
        p = Counter(value: v)
        p.label = "tag"
        delete p.label
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

// --- Structured presence analysis: the strict present-entry sparse set. ---

fn count_strict(instrs: &[SealedInstr]) -> usize {
    instrs
        .iter()
        .filter(|i| matches!(i, SealedInstr::DurSetSparsePresent { .. }))
        .count()
}

fn count_bare(instrs: &[SealedInstr]) -> usize {
    instrs
        .iter()
        .filter(|i| matches!(i, SealedInstr::DurSetSparse(_)))
        .count()
}

/// A sparse-field set through a `place` dominated by an `exists(p)` guard lowers to
/// the strict present-entry form (`DurSetSparsePresent`), which reads the key from
/// the place's slot and assumes the entry present; the same set with no dominating
/// guard stays the bare `DurSetSparse` (create-or-reconcile at commit).
#[test]
fn an_exists_guarded_sparse_set_is_strict() {
    let guarded = format!(
        "{HEADER}{}",
        r#"
pub fn tag(n: int) {
    transaction {
        place p = ^counters[n]
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
    assert_eq!(count_bare(instrs), 0, "no bare set remains");

    let unguarded = format!(
        "{HEADER}{}",
        r#"
pub fn tag(n: int) {
    transaction {
        place p = ^counters[n]
        p.label = "x"
    }
}
"#
    );
    let image = compile_verify(&unguarded);
    let instrs = export_instrs(&image, "tag");
    assert_eq!(count_strict(instrs), 0, "an unguarded set is not strict");
    assert_eq!(count_bare(instrs), 1, "the unguarded set stays bare");
}

/// An `if const c = p` entry read proves the entry present in its then-block, so a
/// sparse set through the same place there is strict.
#[test]
fn an_if_const_guarded_sparse_set_is_strict() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn tag(n: int) {
    transaction {
        place p = ^counters[n]
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

/// A whole-entry upsert (`p = Record(...)`) leaves the entry present, so a following
/// sparse set through the place is strict.
#[test]
fn a_sparse_set_after_an_upsert_is_strict() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn tag(n: int, v: int) {
    transaction {
        place p = ^counters[n]
        p = Counter(value: v)
        p.label = "x"
    }
}
"#
    );
    let image = compile_verify(&source);
    let instrs = export_instrs(&image, "tag");
    assert_eq!(count_strict(instrs), 1, "the post-upsert set is strict");
}

/// Presence facts attach to a lexical `place` binding only: an inline `^root(k)`
/// address never carries one, so an inline sparse set stays bare even under a guard.
#[test]
fn an_inline_sparse_set_is_never_strict() {
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
    let image = compile_verify(&source);
    let instrs = export_instrs(&image, "tag");
    assert_eq!(count_strict(instrs), 0);
    assert_eq!(count_bare(instrs), 1);
}

/// A presence fact does not survive a `delete p`: a sparse set after the entry is
/// erased is bare again (the compiler drops the fact; the verifier would reject a
/// strict set there).
#[test]
fn a_sparse_set_after_delete_is_not_strict() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn tag(n: int) {
    transaction {
        place p = ^counters[n]
        if exists(p) {
            delete p
            p.label = "x"
        }
    }
}
"#
    );
    let image = compile_verify(&source);
    let instrs = export_instrs(&image, "tag");
    assert_eq!(count_strict(instrs), 0, "presence is killed by the erase");
    assert_eq!(count_bare(instrs), 1);
}

/// The fact does not leak past the guarded block: a sparse set after the `if
/// exists(p)` block closes is bare, since the entry is not known present there.
#[test]
fn the_presence_fact_does_not_outlive_its_block() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn tag(n: int) {
    transaction {
        place p = ^counters[n]
        if exists(p) {
            p.label = "in"
        }
        p.label = "out"
    }
}
"#
    );
    let image = compile_verify(&source);
    let instrs = export_instrs(&image, "tag");
    assert_eq!(count_strict(instrs), 1, "only the in-block set is strict");
    assert_eq!(count_bare(instrs), 1, "the post-block set is bare");
}

/// Two places over distinct entries, each guarded and set in its own block,
/// interleaved: the presence fact is keyed to the place it was proven for, so inside
/// `if exists(p)` only the set through `p` is strict — a set through the co-resident,
/// unguarded `q` stays bare — and the mirror holds inside `if exists(q)`. The two
/// facts never merge across places, and neither survives past its own block.
#[test]
fn interleaved_guarded_places_keep_independent_presence_facts() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn tag(a: int, b: int) {
    transaction {
        place p = ^counters[a]
        place q = ^counters[b]
        if exists(p) {
            p.label = "p-strict"
            q.label = "q-bare"
        }
        if exists(q) {
            q.label = "q-strict"
            p.label = "p-bare"
        }
    }
}
"#
    );
    let image = compile_verify(&source);
    let instrs = export_instrs(&image, "tag");
    // Two strict sets (p in its guard, q in its guard); two bare sets (the
    // co-resident place in each block, whose fact is not proven there).
    assert_eq!(
        count_strict(instrs),
        2,
        "each place is strict only inside its own guard"
    );
    assert_eq!(
        count_bare(instrs),
        2,
        "a co-resident place's set is bare; one place's fact never covers another"
    );
}

// --- Scope and type rules. ---

/// A `place` must name a whole durable entry address. A non-durable value, a
/// field-projected address, another place, and a re-binding of an existing name are
/// each a typed `check.type` diagnostic.
#[test]
fn a_place_must_name_a_whole_durable_entry() {
    let non_durable = format!(
        "{HEADER}{}",
        "pub fn f(): int {\n    place p = 5\n    return 0\n}\n"
    );
    assert!(compile_error_codes(&non_durable).contains(&"check.type".to_string()));

    let field = format!(
        "{HEADER}{}",
        "pub fn f(n: int): int {\n    place p = ^counters[n].value\n    return 0\n}\n"
    );
    assert!(compile_error_codes(&field).contains(&"check.type".to_string()));

    let another_place = format!(
        "{HEADER}{}",
        "pub fn f(n: int): int {\n    place p = ^counters[n]\n    place q = p\n    return 0\n}\n"
    );
    assert!(compile_error_codes(&another_place).contains(&"check.type".to_string()));

    let rebind = format!(
        "{HEADER}{}",
        "pub fn f(n: int): int {\n    place p = ^counters[n]\n    place p = ^counters[n]\n    return 0\n}\n"
    );
    assert!(compile_error_codes(&rebind).contains(&"check.type".to_string()));
}

/// A place is a durable designation, not a first-class value: using its bare name in
/// value position (passing it, returning it) is a typed `check.type` diagnostic,
/// while `p.field`, `if const`, and `exists` are the read forms.
#[test]
fn a_bare_place_name_is_not_a_value() {
    let returned = format!(
        "{HEADER}{}",
        "pub fn f(n: int): int {\n    place p = ^counters[n]\n    return p\n}\n"
    );
    assert!(compile_error_codes(&returned).contains(&"check.type".to_string()));

    let passed = format!(
        "{HEADER}{}",
        "pub fn f(n: int): int {\n    place p = ^counters[n]\n    return keyOf(p)\n}\n"
    );
    assert!(compile_error_codes(&passed).contains(&"check.type".to_string()));
}

/// A place name and a value binding stay distinct: declaring a `const`/`var` that
/// reuses an in-scope place name is a typed `check.type` diagnostic, so a name
/// resolves to exactly one of a place or a value.
#[test]
fn a_value_binding_cannot_reuse_a_place_name() {
    let shadowed = format!(
        "{HEADER}{}",
        "pub fn f(n: int): int {\n    place p = ^counters[n]\n    const p = 1\n    return p\n}\n"
    );
    assert!(compile_error_codes(&shadowed).contains(&"check.type".to_string()));
}

/// Every place operation form compiles and verifies over the executable flat scalar
/// root, so the image is well-formed and identity-complete (execution is parked in
/// the trough until E01). One export exercises the whole algebra through a place.
#[test]
fn every_place_operation_form_compiles_and_verifies() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn present(n: int): bool {
    place p = ^counters[n]
    return exists(p)
}

pub fn titleOrZero(n: int): int {
    place p = ^counters[n]
    if const c = p {
        return c.value
    }
    return 0
}

pub fn edit(n: int, v: int) {
    transaction {
        place p = ^counters[n]
        p = Counter(value: v)
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
// Every test in this section is red until the B2 complete-entries vertical lands. Each
// asserts the rule the design predicts through the production capture -> compile path:
// a field write through a place needs a presence fact that no erase of the entry's
// family — direct, through another binding, or inside a called helper — has ended, and
// a required field read through such a place has its declared type.

/// `(code, line, column)` of every diagnostic a source that fails to compile carries.
fn compile_diagnostics(source: &str) -> Vec<(String, u32, u32)> {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    match marrow_compile::compile(&project) {
        Ok(_) => Vec::new(),
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => diagnostics
            .iter()
            .map(|d| (d.code().to_string(), d.line(), d.column()))
            .collect(),
        Err(
            marrow_compile::CompileFailure::Invariant(_)
            | marrow_compile::CompileFailure::ResourceLimit(_),
        ) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
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

/// A presence fact ends at a call whose composed demand writes the entry's family:
/// `wipe(n)` erases `^counters[n]` inside the guarded block, so the sparse set after it
/// is refused at check time with a typed code at the write. Today the program checks
/// clean and faults at runtime with `run.corruption`.
#[test]
#[ignore = "B2 complete entries"]
fn a_sparse_set_after_a_helper_erase_of_the_family_is_refused_at_check() {
    let source = format!(
        "{HEADER}{}",
        r#"
fn wipe(n: int) {
    delete ^counters[n]
}

pub fn provedThenHelperErase(n: int): bool {
    transaction {
        place p = ^counters[n]
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
/// through `a` is refused at check time. Today the program checks clean and faults at
/// runtime with `run.corruption`.
#[test]
#[ignore = "B2 complete entries"]
fn a_sparse_set_after_an_erase_through_another_place_is_refused_at_check() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn provedThenAliasErase(n: int): bool {
    transaction {
        place a = ^counters[n]
        place b = ^counters[n]
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
/// field write on an entry no fact proves present is refused at check time rather than
/// creating the entry at commit. Today the write compiles to the bare set and the entry
/// is minted by the commit reconcile.
#[test]
#[ignore = "B2 complete entries"]
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

/// A present entry has every required field, so inside `if exists(p)` the required
/// `p.value` reads as a plain `int`, while the sparse `p.label` stays `string?`. Today
/// every durable field read is optional and the annotated binding is `check.type`.
#[test]
#[ignore = "B2 complete entries"]
fn a_required_field_reads_bare_through_a_place_proven_present() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn valueOf(n: int): int {
    place p = ^counters[n]
    if exists(p) {
        const v: int = p.value
        const l: string? = p.label
        return v + len(l ?? "")
    }
    return 0
}
"#
    );
    assert_eq!(
        compile_diagnostics(&source),
        Vec::<(String, u32, u32)>::new(),
        "a required read through a proven place has its declared type"
    );
    let image = compile_verify(&source);
    assert!(has_function(&image, "valueOf"));
}

// --- Complete entries, design v2: proof lifetime and the one clearing spelling. ---

/// A loop body is one region: a write through `p` inside the loop precedes, on the
/// back edge, the erase of `p`'s family later in the same body, so the write is
/// refused at check time when the loop closes. Both the direct erase and a helper
/// whose demand writes the family are refused. Today the direct form compiles to a
/// strict set the verifier then rejects (`image.flow`), and the helper form verifies
/// and faults at runtime.
#[test]
#[ignore = "B2 complete entries"]
fn a_field_write_inside_a_loop_that_erases_the_family_is_refused_at_check() {
    let direct = format!(
        "{HEADER}{}",
        r#"
pub fn writeThenEraseInLoop(n: int): bool {
    transaction {
        place p = ^counters[n]
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

    let through_helper = format!(
        "{HEADER}{}",
        r#"
fn wipe(n: int) {
    delete ^counters[n]
}

pub fn writeThenHelperInLoop(n: int): bool {
    transaction {
        place p = ^counters[n]
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
        "a call whose demand writes the family inside the loop ends the fact on the back edge"
    );
}

/// An erase of the family ends the fact whatever key it names: `delete ^counters[n + 1]`
/// may or may not be `p`'s entry, and the rule does not reason about keys. Today the
/// inline erase leaves the compiler's fact in place and the set is emitted strict.
#[test]
#[ignore = "B2 complete entries"]
fn an_inline_erase_of_another_key_in_the_family_is_refused_at_check() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn eraseNeighbourThenSet(n: int): bool {
    transaction {
        place p = ^counters[n]
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
/// strict. Today the continuation carries no fact and the set is bare.
#[test]
#[ignore = "B2 complete entries"]
fn a_negative_diverging_guard_carries_the_fact_into_the_continuation() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn setLabelIfPresent(n: int): bool {
    transaction {
        place p = ^counters[n]
        if not exists(p) {
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

/// A field is cleared by `delete p.f` and by nothing else: assigning `absent` to a
/// durable field is `check.type` at the write. Today the assignment compiles as a
/// sparse set of `absent`.
#[test]
#[ignore = "B2 complete entries"]
fn a_durable_field_assigned_absent_is_refused_naming_delete() {
    let source = format!(
        "{HEADER}{}",
        r#"
pub fn clearLabel(n: int) {
    transaction {
        place p = ^counters[n]
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
/// program with an erasing helper is refused. Today the erasing form compiles clean.
#[test]
#[ignore = "B2 complete entries"]
fn a_read_only_helper_keeps_the_fact_while_an_erasing_helper_ends_it() {
    let reading = format!(
        "{HEADER}{}",
        r#"
fn peek(n: int): int? {
    return ^counters[n].value
}

pub fn peekThenSet(n: int): int {
    transaction {
        place p = ^counters[n]
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
        place p = ^counters[n]
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
        "the discriminator: a call whose demand writes the family ends the fact"
    );
}
