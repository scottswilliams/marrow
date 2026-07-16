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

const HEADER: &str = "resource Counter\n\
     \x20   required value: int\n\
     \x20   label: string\n\
     \n\
     store ^counters(id: int): Counter\n\
     \n\
     fn keyOf(n: int): int\n\
     \x20   return n + 100\n\
     \n";

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
        Err(diagnostics) => diagnostics.iter().map(|d| d.code.to_string()).collect(),
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
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_place_key_operand_is_lowered_exactly_once() {
    let source = format!(
        "{HEADER}\
         pub fn use3(n: int): int\n\
         \x20   place p = ^counters(keyOf(n))\n\
         \x20   const present = exists(p)\n\
         \x20   const a = p.value ?? 0\n\
         \x20   const b = p.value ?? 0\n\
         \x20   if present\n\
         \x20       return a + b\n\
         \x20   return 0\n"
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
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_place_binding_emits_no_effect_site_before_its_uses() {
    let source = format!(
        "{HEADER}\
         pub fn readIt(n: int): int\n\
         \x20   place p = ^counters(keyOf(n))\n\
         \x20   return p.value ?? 0\n"
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
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_place_reused_across_writes_evaluates_its_key_once() {
    let source = format!(
        "{HEADER}\
         pub fn writeIt(n: int, v: int)\n\
         \x20   transaction\n\
         \x20       place p = ^counters(keyOf(n))\n\
         \x20       p = Counter(value: v)\n\
         \x20       p.label = \"tag\"\n\
         \x20       delete p.label\n"
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
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn an_exists_guarded_sparse_set_is_strict() {
    let guarded = format!(
        "{HEADER}\
         pub fn tag(n: int)\n\
         \x20   transaction\n\
         \x20       place p = ^counters(n)\n\
         \x20       if exists(p)\n\
         \x20           p.label = \"x\"\n"
    );
    let instrs = compile_verify(&guarded);
    let instrs = export_instrs(&instrs, "tag");
    assert_eq!(count_strict(instrs), 1, "the guarded set lowers strict");
    assert_eq!(count_bare(instrs), 0, "no bare set remains");

    let unguarded = format!(
        "{HEADER}\
         pub fn tag(n: int)\n\
         \x20   transaction\n\
         \x20       place p = ^counters(n)\n\
         \x20       p.label = \"x\"\n"
    );
    let image = compile_verify(&unguarded);
    let instrs = export_instrs(&image, "tag");
    assert_eq!(count_strict(instrs), 0, "an unguarded set is not strict");
    assert_eq!(count_bare(instrs), 1, "the unguarded set stays bare");
}

/// An `if const c = p` entry read proves the entry present in its then-block, so a
/// sparse set through the same place there is strict.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn an_if_const_guarded_sparse_set_is_strict() {
    let source = format!(
        "{HEADER}\
         pub fn tag(n: int)\n\
         \x20   transaction\n\
         \x20       place p = ^counters(n)\n\
         \x20       if const c = p\n\
         \x20           p.label = \"x\"\n"
    );
    let image = compile_verify(&source);
    let instrs = export_instrs(&image, "tag");
    assert_eq!(count_strict(instrs), 1);
}

/// A whole-entry upsert (`p = Record(...)`) leaves the entry present, so a following
/// sparse set through the place is strict.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_sparse_set_after_an_upsert_is_strict() {
    let source = format!(
        "{HEADER}\
         pub fn tag(n: int, v: int)\n\
         \x20   transaction\n\
         \x20       place p = ^counters(n)\n\
         \x20       p = Counter(value: v)\n\
         \x20       p.label = \"x\"\n"
    );
    let image = compile_verify(&source);
    let instrs = export_instrs(&image, "tag");
    assert_eq!(count_strict(instrs), 1, "the post-upsert set is strict");
}

/// Presence facts attach to a lexical `place` binding only: an inline `^root(k)`
/// address never carries one, so an inline sparse set stays bare even under a guard.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn an_inline_sparse_set_is_never_strict() {
    let source = format!(
        "{HEADER}\
         pub fn tag(n: int)\n\
         \x20   transaction\n\
         \x20       if exists(^counters(n))\n\
         \x20           ^counters(n).label = \"x\"\n"
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
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_sparse_set_after_delete_is_not_strict() {
    let source = format!(
        "{HEADER}\
         pub fn tag(n: int)\n\
         \x20   transaction\n\
         \x20       place p = ^counters(n)\n\
         \x20       if exists(p)\n\
         \x20           delete p\n\
         \x20           p.label = \"x\"\n"
    );
    let image = compile_verify(&source);
    let instrs = export_instrs(&image, "tag");
    assert_eq!(count_strict(instrs), 0, "presence is killed by the erase");
    assert_eq!(count_bare(instrs), 1);
}

/// The fact does not leak past the guarded block: a sparse set after the `if
/// exists(p)` block closes is bare, since the entry is not known present there.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn the_presence_fact_does_not_outlive_its_block() {
    let source = format!(
        "{HEADER}\
         pub fn tag(n: int)\n\
         \x20   transaction\n\
         \x20       place p = ^counters(n)\n\
         \x20       if exists(p)\n\
         \x20           p.label = \"in\"\n\
         \x20       p.label = \"out\"\n"
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
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn interleaved_guarded_places_keep_independent_presence_facts() {
    let source = format!(
        "{HEADER}\
         pub fn tag(a: int, b: int)\n\
         \x20   transaction\n\
         \x20       place p = ^counters(a)\n\
         \x20       place q = ^counters(b)\n\
         \x20       if exists(p)\n\
         \x20           p.label = \"p-strict\"\n\
         \x20           q.label = \"q-bare\"\n\
         \x20       if exists(q)\n\
         \x20           q.label = \"q-strict\"\n\
         \x20           p.label = \"p-bare\"\n"
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
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_place_must_name_a_whole_durable_entry() {
    let non_durable = format!("{HEADER}pub fn f(): int\n    place p = 5\n    return 0\n");
    assert!(compile_error_codes(&non_durable).contains(&"check.type".to_string()));

    let field =
        format!("{HEADER}pub fn f(n: int): int\n    place p = ^counters(n).value\n    return 0\n");
    assert!(compile_error_codes(&field).contains(&"check.type".to_string()));

    let another_place = format!(
        "{HEADER}pub fn f(n: int): int\n    place p = ^counters(n)\n    place q = p\n    return 0\n"
    );
    assert!(compile_error_codes(&another_place).contains(&"check.type".to_string()));

    let rebind = format!(
        "{HEADER}pub fn f(n: int): int\n    place p = ^counters(n)\n    place p = ^counters(n)\n    return 0\n"
    );
    assert!(compile_error_codes(&rebind).contains(&"check.type".to_string()));
}

/// A place is a durable designation, not a first-class value: using its bare name in
/// value position (passing it, returning it) is a typed `check.type` diagnostic,
/// while `p.field`, `if const`, and `exists` are the read forms.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_bare_place_name_is_not_a_value() {
    let returned =
        format!("{HEADER}pub fn f(n: int): int\n    place p = ^counters(n)\n    return p\n");
    assert!(compile_error_codes(&returned).contains(&"check.type".to_string()));

    let passed =
        format!("{HEADER}pub fn f(n: int): int\n    place p = ^counters(n)\n    return keyOf(p)\n");
    assert!(compile_error_codes(&passed).contains(&"check.type".to_string()));
}

/// A place name and a value binding stay distinct: declaring a `const`/`var` that
/// reuses an in-scope place name is a typed `check.type` diagnostic, so a name
/// resolves to exactly one of a place or a value.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn a_value_binding_cannot_reuse_a_place_name() {
    let shadowed = format!(
        "{HEADER}pub fn f(n: int): int\n    place p = ^counters(n)\n    const p = 1\n    return p\n"
    );
    assert!(compile_error_codes(&shadowed).contains(&"check.type".to_string()));
}

/// Every place operation form compiles and verifies over the executable flat scalar
/// root, so the image is well-formed and identity-complete (execution is parked in
/// the trough until E01). One export exercises the whole algebra through a place.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn every_place_operation_form_compiles_and_verifies() {
    let source = format!(
        "{HEADER}\
         pub fn present(n: int): bool\n\
         \x20   place p = ^counters(n)\n\
         \x20   return exists(p)\n\
         \n\
         pub fn titleOrZero(n: int): int\n\
         \x20   place p = ^counters(n)\n\
         \x20   if const c = p\n\
         \x20       return c.value\n\
         \x20   return 0\n\
         \n\
         pub fn edit(n: int, v: int)\n\
         \x20   transaction\n\
         \x20       place p = ^counters(n)\n\
         \x20       p = Counter(value: v)\n\
         \x20       p.label = \"x\"\n\
         \x20       delete p\n"
    );
    let image = compile_verify(&source);
    for name in ["present", "titleOrZero", "edit"] {
        assert!(
            has_function(&image, name),
            "export `{name}` is present in the verified image"
        );
    }
}
