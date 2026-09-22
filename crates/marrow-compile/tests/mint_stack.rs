//! The generic-mint path's machine-stack requirement at its own admitted bound.
//!
//! `MINT_DEPTH_LIMIT` (256) bounds how many nesting levels a divergent generic can
//! drive. A bound on the *count* of levels bounds the stack only if a level costs a
//! bounded number of machine frames, and the compiler states no stack budget of its own:
//! the 256 MiB the CLI and the LSP give their worker threads is what they happen to
//! allocate, not a figure the mint path is held to.
//!
//! [`MINT_STACK_BUDGET_BYTES`] is that figure — 4 KiB per admitted nesting level, which
//! a default 2 MiB Rust thread (every `cargo test` thread is one) comfortably holds.
//! Minting reaches it by filling iteratively: a member needing a nested instantiation
//! reserves its row and queues the fill, so a nesting level costs one queue entry and
//! the machine stack carries one fill, whatever the depth.

use marrow_codes::Code;
use marrow_compile::{CompileFailure, compile};

use marrow_test_programs::project as project_capture;

/// The stack a 256-level bounded recursion is asked to fit in: 4 KiB per admitted level.
const MINT_STACK_BUDGET_BYTES: usize = 4 * 1024 * 256;

/// The widest admissible enum body, read from the bounds that govern one.
const ADMITTED_VARIANTS: usize = marrow_image::bounds::MAX_VARIANTS;
const ADMITTED_PAYLOAD_FIELDS: usize = marrow_image::bounds::MAX_PAYLOAD_FIELDS;

/// A divergent generic enum at the widest admissible variant and payload width: each
/// resolution of the recursive variant's `Grown<Wrap<T>>` leaf demands the next
/// instantiation, so the chain runs until the mint depth limit refuses it. This is the
/// `limits::issuance_amplification` enum arm; it is repeated here rather than shared
/// because that gate measures the refusal and this one measures the stack reaching it.
fn enum_amplification_corpus() -> String {
    let mut body = String::from("struct Wrap<T> {\n    inner: T\n}\n\nenum Grown<T> {\n");
    for variant in 0..ADMITTED_VARIANTS - 1 {
        let payload: Vec<String> = (0..ADMITTED_PAYLOAD_FIELDS)
            .map(|leaf| format!("p{leaf}: T"))
            .collect();
        body.push_str(&format!("    v{variant}({})\n", payload.join(", ")));
    }
    let mut tail: Vec<String> = (0..ADMITTED_PAYLOAD_FIELDS - 1)
        .map(|leaf| format!("p{leaf}: T"))
        .collect();
    tail.push("n: Grown<Wrap<T>>".to_string());
    body.push_str(&format!("    next({})\n}}\n\n", tail.join(", ")));
    body.push_str("fn sprout<T>(x: T): Grown<T> {\n    return sprout(x)\n}\n\n");
    format!(
        "module main\n\n{body}pub fn driver(): int {{\n    const ignored = sprout(1)\n    \
         return 0\n}}\n",
    )
}

/// Whether the corpus reaches its governing mint bound and is refused, rather than
/// compiling or failing some earlier way.
fn reaches_the_mint_bound(source: &str) -> bool {
    let project = project_capture::project_with_ids(&[("src/main.mw", source)], None);
    match compile(&project) {
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics
            .iter()
            .any(|row| row.code() == Code::CheckInstantiationLimit),
        _ => false,
    }
}

/// The mint path reaches its own depth refusal inside [`MINT_STACK_BUDGET_BYTES`].
///
/// A regression here overflows the worker's stack and aborts the process, taking the rest
/// of the binary with it, because that is what exceeding a stack budget does.
#[test]
fn the_admitted_mint_depth_fits_the_stack_budget() {
    let worker = std::thread::Builder::new()
        .stack_size(MINT_STACK_BUDGET_BYTES)
        .spawn(|| {
            assert!(
                reaches_the_mint_bound(&enum_amplification_corpus()),
                "the enum amplification arm must reach its mint bound",
            );
        })
        .expect("spawn the mint-depth worker");
    worker.join().expect("the mint-depth worker completes");
}
