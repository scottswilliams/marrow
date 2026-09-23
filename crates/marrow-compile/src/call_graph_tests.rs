//! Call-graph, propagation and presence analysis over widening programs, driven
//! through the production compile path.

use marrow_codes::Code;
use std::fmt::Write as _;

use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};
use marrow_syntax::SourceSpan;

use crate::compile::{CompileFailure, ResourceLimitKind, check, compile, compile_with_tests};
use marrow_test_support::{ledger, project as project_capture};

fn project(source: String) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.into_bytes(),
    )];
    marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

/// A chain of `depth` functions, each calling the next, ending in a leaf. The
/// public driver adds the final edge, so the graph has exactly `depth` edges.
fn chain_source(depth: usize) -> String {
    let mut source = String::from("module main\n\n");
    for step in 0..depth {
        writeln!(source, "fn step{step:04}(n: int): int {{").expect("write");
        if step + 1 == depth {
            writeln!(source, "    return n").expect("write");
        } else {
            writeln!(source, "    return step{:04}(n)", step + 1).expect("write");
        }
        writeln!(source, "}}\n").expect("write");
    }
    writeln!(source, "pub fn driver(n: int): int {{").expect("write");
    writeln!(source, "    return step0000(n)").expect("write");
    writeln!(source, "}}").expect("write");
    source
}

/// A chain compiles to the same image bytes however deep it is rebuilt.
#[test]
fn a_call_chain_compiles_reproducibly() {
    for depth in [64usize, 256] {
        let input = project(chain_source(depth));
        let first = compile(&input).expect("the acyclic chain compiles");
        let second = compile(&input).expect("the acyclic chain compiles");
        assert_eq!(first.image.bytes, second.image.bytes, "depth {depth}");
    }
}

fn durable_project(source: &str, roots: &[String]) -> ProjectInput {
    let mut anchors = vec![
        "application .".to_string(),
        "product R".to_string(),
        "field R.value".to_string(),
    ];
    for root in roots {
        anchors.push(format!("root {root}"));
        anchors.push(format!("key {root}.id"));
    }
    let borrowed: Vec<&str> = anchors.iter().map(String::as_str).collect();
    let ids = ledger::ledger(&borrowed);
    project_capture::project_with_ids(&[("src/main.mw", source)], Some(&ids))
}

fn diagnostic_rows(
    result: Result<impl std::fmt::Debug, CompileFailure>,
) -> Vec<(Code, String, SourceSpan)> {
    let Err(CompileFailure::Diagnostics(rows)) = result else {
        panic!("expected source diagnostics, got {result:?}");
    };
    rows.iter()
        .map(|row| (row.code(), row.file().as_str().to_string(), row.span()))
        .collect()
}

#[test]
fn a_duplicate_test_hole_does_not_prevent_generic_presence_analysis() {
    let source = r#"module main
resource R {
    required value: int
}
store ^r[id: int]: R
pub fn write(id: int) {
    transaction {
        ref p = ^r[id] else { return }
        const queued = identity(1)
        p.value = queued
    }
}
fn erase(id: int) { delete ^r[id] }
fn identity<T>(x: T): T { return x }
test "same" {}
test "same" {}
"#;
    let input = durable_project(source, &["r".to_string()]);
    let observed = diagnostic_rows(compile_with_tests(&input));
    assert_eq!(observed.len(), 1);
    let (code, file, span) = &observed[0];
    assert_eq!(*code, Code::CheckNameConflict);
    assert_eq!(file, "src/main.mw");
    assert_eq!((span.line, span.column), (16, 6));
    assert_eq!(diagnostic_rows(check(&input)), observed);
    compile(&input).expect("excluding the duplicate tests permits the generic drain");
}

fn resource_source(roots: &[String]) -> String {
    let mut source = String::from("module main\nresource R { required value: int }\n");
    for root in roots {
        writeln!(source, "store ^{root}[id: int]: R").expect("write");
    }
    source
}

fn push_guarded_writes(source: &mut String, roots: &[String]) {
    for (index, root) in roots.iter().enumerate() {
        writeln!(
            source,
            "        ref p{index} = ^{root}[id] else {{ return }}"
        )
        .expect("write");
        writeln!(source, "        p{index}.value = noop(id)").expect("write");
    }
}

/// One, a full stripe's worth, and one past a stripe of durable families all admit
/// the guarded writes and encode reproducibly: the presence summary's striping is
/// invisible in the image.
#[test]
fn presence_striping_is_invisible_in_the_image() {
    for families in [1usize, 64, 65] {
        let roots: Vec<String> = (0..families).map(|index| format!("r{index}")).collect();
        let mut source = resource_source(&roots);
        source.push_str("fn noop(x: int): int { return x }\nfn erase_all(id: int) {\n");
        for root in &roots {
            writeln!(source, "    delete ^{root}[id]").expect("write");
        }
        source.push_str("}\npub fn write(id: int) {\n    transaction {\n");
        push_guarded_writes(&mut source, &roots);
        source.push_str("    }\n}\n");
        let input = durable_project(&source, &roots);
        let compiled = compile(&input).expect("the RHS calls do not erase any guarded entry");
        let again = compile(&input).expect("the RHS calls do not erase any guarded entry");
        assert_eq!(
            compiled.image.bytes, again.image.bytes,
            "presence work for {families} families is invisible in the image",
        );
    }
}

/// Narrow bodies reach presence checking beyond the final image function limit.
/// Two modules keep their declaration outlines below the separate per-file bound.
#[test]
fn presence_summaries_cover_functions_beyond_final_image_policy() {
    let functions = marrow_image::bounds::MAX_FUNCTIONS + 1;
    assert_eq!(functions, 4_097);
    let mut main = String::from(
        "module main\n\
         resource R { required value: int }\n\
         store ^r[id: int]: R\n\
         fn noop(x: int): int { return x }\n\
         fn erase(id: int) { delete ^r[id] }\n\
         pub fn write(id: int) {\n\
             transaction {\n\
                 ref p = ^r[id] else { return }\n\
                 p.value = noop(id)\n\
             }\n\
         }\n",
    );
    let split = functions.div_ceil(2);
    for index in 3..split {
        writeln!(main, "fn pad{index}() {{}}").expect("write");
    }
    let mut extra = String::from("module extra\n");
    for index in split..functions {
        writeln!(extra, "fn pad{index}() {{}}").expect("write");
    }
    let ids = ledger::ledger(&[
        "application .",
        "product R",
        "field R.value",
        "root r",
        "key r.id",
    ]);
    let input = project_capture::project_with_ids(
        &[("src/main.mw", &main), ("src/extra.mw", &extra)],
        Some(&ids),
    );
    let result = compile(&input);
    let Err(CompileFailure::ResourceLimit(limit)) = result else {
        panic!("expected the final function limit, got {result:?}");
    };
    assert_eq!(limit.kind(), ResourceLimitKind::Functions);
    assert_eq!(limit.limit(), marrow_image::bounds::MAX_FUNCTIONS as u64);
}

fn dense_presence_source(repetitions: usize, protected_root: &str) -> (ProjectInput, u32) {
    let mut roots: Vec<String> = (0..63).map(|index| format!("r{index}")).collect();
    roots.push("keep".to_string());
    let mut source = resource_source(&roots);
    source.push_str("fn noop(x: int): int { return x }\nfn erase_many(id: int) {\n");
    for root in &roots[..63] {
        writeln!(source, "    delete ^{root}[id]").expect("write");
    }
    source.push_str(
        "}\nfn erase_keep(id: int) { delete ^keep[id] }\n\
         pub fn probe(id: int) {\n    transaction {\n",
    );
    push_guarded_writes(&mut source, &roots);
    source.push_str("    }\n}\npub fn repeat(id: int) {\n    transaction {\n");
    writeln!(
        source,
        "        ref p = ^{protected_root}[id] else {{ return }}"
    )
    .expect("write");
    for _ in 0..repetitions {
        source.push_str("            erase_many(id)\n");
    }
    let write_line = source.lines().count() as u32 + 1;
    source.push_str("            p.value = 1\n    }\n}\n");
    (durable_project(&source, &roots), write_line)
}

/// An eraser over 63 other families, repeated, never erases the one family a
/// pending query guards.
#[test]
fn dense_erasers_leave_an_independent_pending_family_present() {
    for repetitions in [8usize, 16] {
        let (input, _) = dense_presence_source(repetitions, "keep");
        compile(&input).expect("the dense eraser leaves the pending family present");
    }
}

#[test]
fn the_first_matching_eraser_drains_the_query_once() {
    let (input, write_line) = dense_presence_source(16, "r0");
    let rows = diagnostic_rows(compile(&input));
    assert_eq!(rows.len(), 1);
    let (code, file, span) = &rows[0];
    assert_eq!(*code, Code::CheckRequiresPresence);
    assert_eq!(file, "src/main.mw");
    assert_eq!((span.line, span.column), (write_line, 13));
}
