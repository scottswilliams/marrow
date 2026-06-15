//! Module-aware, visibility-aware runtime call dispatch, and the file id an
//! uncaught fault or throw carries: the raising frame's module, never overwritten
//! by an outer frame.

use crate::support;
use support::*;

use marrow_check::{CheckedRuntimeProgram, FileId};
use marrow_run::{RUN_DIVIDE_BY_ZERO, RUN_UNCAUGHT_THROW, Value};
use marrow_syntax::parse_source;
use std::path::PathBuf;

/// Build a checked program from `(module_name, source)` pairs, one source file
/// per pair, so module-aware dispatch and file ids use the production checker
/// artifact.
fn multi_module_program(modules: &[(&str, &str)]) -> CheckedRuntimeProgram {
    let files: Vec<(PathBuf, String)> = modules
        .iter()
        .map(|(name, source)| {
            let path = module_source_path(name);
            let text = format!("module {name}\n\n{source}");
            let parsed = parse_source(&text);
            assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
            (path, text)
        })
        .collect();
    checked_program_files(&files)
}

#[test]
fn bare_call_resolves_in_own_module_not_a_foreign_one() {
    // Two modules each declare `fn greet` returning a distinct value. `zzz::run`
    // calls a bare `greet()`. A bare name resolves in its own module first, so it
    // must run `zzz::greet` (2), never the foreign `aaa::greet` (1).
    let program = multi_module_program(&[
        ("aaa", "pub fn greet(): int\n    return 1\n"),
        (
            "zzz",
            "fn greet(): int\n    return 2\npub fn run(): int\n    return greet()\n",
        ),
    ]);
    assert_eq!(
        run(checked_entry!(&program, "zzz::run")),
        Ok(Some(Value::Int(2))),
        "a bare call must run the calling module's own function"
    );
}

#[test]
fn cross_module_call_to_a_private_fn_is_a_visibility_error() {
    checker_rejects_sources(
        &[
            "module aaa\nfn secret(): int\n    return 1\n",
            "module zzz\nfn run(): int\n    return aaa::secret()\n",
        ],
        "check.private_function",
    );
}

#[test]
fn uncaught_fault_in_entry_module_carries_its_file_id() {
    // A divide-by-zero raised in the entry module's own body stamps that
    // module's file id (index 0), so a renderer can name the file it lives in.
    let program = multi_module_program(&[(
        "a",
        "pub fn boom(): int\n    const boom = 1 / 0\n    return 0\n",
    )]);
    let error = run_expecting_error(checked_entry!(&program, "a::boom"));
    assert_eq!(error.code(), RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.origin, Some(FileId(0)));
    assert!(
        program
            .file_path(FileId(0))
            .is_some_and(|path| path.ends_with("src/a.mw"))
    );
}

#[test]
fn uncaught_fault_in_cross_module_callee_carries_the_callee_file_id() {
    // The entry `a::run` calls `b::boom`, which divides by zero. The fault is
    // uncaught, so its origin must be `b`'s file (index 1), the frame that
    // raised it, not the entry's `a`.
    let program = multi_module_program(&[
        ("a", "pub fn run(): int\n    return b::boom()\n"),
        (
            "b",
            "pub fn boom(): int\n    const boom = 1 / 0\n    return 0\n",
        ),
    ]);
    let error = run_expecting_error(checked_entry!(&program, "a::run"));
    assert_eq!(error.code(), RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.origin, Some(FileId(1)));
    assert!(
        program
            .file_path(FileId(1))
            .is_some_and(|path| path.ends_with("src/b.mw"))
    );
}

#[test]
fn uncaught_throw_from_cross_module_callee_carries_the_raising_frame_file_id() {
    // A language `throw` is catchable, so it re-spans at each call boundary on its
    // way out — the same path catchable faults take. Thrown in callee `b` and
    // never caught, its origin must stay `b`'s file (index 1), the frame that
    // first raised it, not the entry `a` it surfaces through.
    let program = multi_module_program(&[
        ("a", "pub fn run(): int\n    return b::boom()\n"),
        (
            "b",
            "pub fn boom(): int\n    throw Error(code: \"x.y\", message: \"bad\")\n",
        ),
    ]);
    let error = run_expecting_error(checked_entry!(&program, "a::run"));
    assert_eq!(error.code(), RUN_UNCAUGHT_THROW);
    assert_eq!(error.origin, Some(FileId(1)));
}

#[test]
fn checked_program_entry_fault_carries_origin() {
    let error = eval_source(
        "pub fn f(): int\n    const boom = 1 / 0\n    return 0\n",
        "f",
        Vec::new(),
    )
    .unwrap_err();
    assert_eq!(error.code(), RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.origin, Some(FileId(0)));
}

#[test]
fn outer_frame_does_not_overwrite_inner_origin() {
    // `a::outer` calls `a::mid` calls `b::boom`. The uncaught fault crosses
    // three frames in two modules; the deepest (`b`, index 1) wins and the outer
    // `a` frames must not overwrite it.
    let program = multi_module_program(&[
        (
            "a",
            "pub fn outer(): int\n    return mid()\n\n\
             fn mid(): int\n    return b::boom()\n",
        ),
        (
            "b",
            "pub fn boom(): int\n    const boom = 1 / 0\n    return 0\n",
        ),
    ]);
    let error = run_expecting_error(checked_entry!(&program, "a::outer"));
    assert_eq!(error.code(), RUN_DIVIDE_BY_ZERO);
    assert_eq!(error.origin, Some(FileId(1)));
}
