//! Drift gate: `docs/error-codes.md` is generated from the registry, so the
//! committed page must equal what [`marrow_codes::generate`] renders. Update the
//! page by rerunning generation (set `MARROW_UPDATE_ERROR_CODES=1`) and reviewing
//! the diff as a documented-meaning contract change.

fn error_codes_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("error-codes.md")
}

#[test]
fn error_codes_doc_is_generated_from_registry() {
    let generated = marrow_codes::generate();
    let path = error_codes_path();

    if std::env::var_os("MARROW_UPDATE_ERROR_CODES").is_some() {
        std::fs::write(&path, &generated).expect("write error-codes.md");
        return;
    }

    let committed = std::fs::read_to_string(&path).expect("read error-codes.md");
    assert!(
        generated == committed,
        "docs/error-codes.md drifted from the registry. Rerun generation with \
         MARROW_UPDATE_ERROR_CODES=1 and review the diff as a contract change."
    );
}

/// Marrow has no out-of-bounds fault class: a local bracket read yields the
/// presence-typed optional (absent when out of range), never a fault. No registered
/// `run.*` code is an out-of-bounds/collection-range fault. This absence is an
/// enforcement artifact — reintroducing such a code fails this test.
#[test]
fn no_out_of_bounds_fault_code_is_registered() {
    for code in marrow_codes::Code::ALL {
        let name = code.as_str();
        assert!(
            !name.contains("collection_range") && !name.contains("out_of_bounds"),
            "an out-of-bounds fault code `{name}` is registered; a bracket read \
             yields the optional, so no such fault class exists"
        );
    }
}

/// The generated reference describes the channels the production toolchain actually
/// exposes. Source Marrow has no throwable `Error` channel or `std::io` module, while
/// `value.range` is reachable through the bounded durable-value encoder.
#[test]
fn generated_reference_has_no_prototype_source_error_channel() {
    let generated = marrow_codes::generate();
    for false_claim in [
        "thrown errors are `Error` values",
        "catchable `Error` values",
        "`std::io`",
        "std::io::",
        "no ordinary checked program reaches this code",
        "store write/read boundary",
        "projecting it to an order-preserving key",
    ] {
        assert!(
            !generated.contains(false_claim),
            "generated reference retained false source-channel claim: {false_claim}"
        );
    }
    assert!(
        generated.contains("dynamic 1 MiB") && generated.contains("aggregate"),
        "generated reference must name the source-reachable aggregate value bound"
    );

    let registry_source = include_str!("../src/lib.rs");
    for removed_axis in ["pub enum Catchability", "fn catchability("] {
        assert!(
            !registry_source.contains(removed_axis),
            "the deleted catchability classification returned to the registry: {removed_axis}"
        );
    }
}
