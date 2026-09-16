//! The generic-mint recursion's machine-stack requirement at its own admitted bound.
//!
//! Monomorphization recurses natively. One nesting level is the cycle
//! `resolve_template_garg` → `mint_type_instance` → `mint_type_instance_with_requirement`
//! → `fill_type_body` → `fill_enum_type_body` → `enum_payload_leaf` →
//! `resolve_garg_annotation` → `resolve_template_garg`, and `MINT_DEPTH_LIMIT` (256)
//! bounds how many of those cycles a divergent generic can drive. The limit bounds the
//! *count* of frames; it says nothing about their size, so it does not by itself bound
//! the stack, and the only stack budget the compiler has ever stated is the 256 MiB the
//! CLI and the LSP happen to give their worker threads.
//!
//! Measured on this corpus (arm64, dev profile, unoptimized): the widest admitted generic
//! enum needs between 1,848 KiB and 1,860 KiB of stack to reach its depth refusal — about
//! 7.2 KiB per nesting level, or 90% of the 2 MiB a default Rust thread has. Every
//! `cargo test` thread is exactly that 2 MiB thread, which is why a frame that grows by a
//! few hundred bytes anywhere in the cycle turns the refusal into `SIGABRT` on the test
//! harness rather than a `check.instantiation_limit` diagnostic.
//!
//! [`MINT_STACK_BUDGET_BYTES`] is the budget this test asks the mint path to meet: 4 KiB
//! per admitted nesting level. It is not met today. Making the cycle iterative — an
//! explicit `Vec` of pending fills driven by the existing `fill_stack`, so a nesting level
//! costs a heap entry instead of eight machine frames — is what would meet it.

use marrow_codes::Code;
use marrow_compile::{CompileFailure, compile};

#[path = "common/project.rs"]
mod project_capture;

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

/// The mint path must reach its own depth refusal inside [`MINT_STACK_BUDGET_BYTES`].
///
/// Ignored because it does not: the recursion overflows the budget and aborts the process
/// rather than failing, which would take the rest of the test binary with it. Run it alone
/// to observe the abort.
#[test]
#[ignore = "the mint recursion needs ~1.8 MiB at the admitted bound and overflows this budget"]
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
