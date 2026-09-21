//! Frozen evidence for the conformance corpus: the image digest of every
//! `fixtures/v01/conformance/*` project is pinned here, so a lowering change that
//! alters image bytes is a deliberate act that updates this table in the same commit.
//! Each fixture is captured through the production filesystem path, dependencies
//! included, and compiled with its tests, as `marrow test` compiles it.

use std::fs;

use marrow_compile::compile_with_tests;
use marrow_project_fs::{OverlaySnapshot, capture_project};

use crate::common::conformance_dir;

/// One line per fixture, in directory order: the fixture and its image digest.
const DIGESTS: &[(&str, &str)] = &[
    (
        "alias_types",
        "5ee9464e718a4ee76a53e9a4faf2d5add0b2487a97e69f0bc4b87b2c0eaa4a8c",
    ),
    (
        "collections",
        "6228224a7a5bcaa157a14768519b29a9aa510a2674aad6b36e74773e168ef879",
    ),
    (
        "enum_types",
        "329cba6bedec59bb1e01af0b844c4cbed9b3b9230abb45b9e9e7cb3b33492a59",
    ),
    (
        "generic_types",
        "994a8ebc38b769cafac9c468c9cf2f616ff525b37ea5c5834df3c62c38bb2ea3",
    ),
    (
        "generics",
        "e220aeda94009c308007a906484450b72a7c58def212a195f8097e814e97c98e",
    ),
    (
        "graph_report",
        "62edec1fe80446b274920b6f4e0e330b3e1eb8ad7ca04d842cec28e667b3ccdd",
    ),
    (
        "graph_report_lib",
        "e20582f67f67474e06a81ecc22dad3657571fb2b64139e33b1fea5d4d92e567b",
    ),
    (
        "groups",
        "a46eb1b412ae8f1dcbd9deb31103df0665ded6507e8812abcd7d3373d13f3720",
    ),
    (
        "local_sparse",
        "a3bb6f98b79ef4a5984d8e8297368601c6bcce27ca6b8158404abca0db19642b",
    ),
    (
        "nominal_types",
        "cc9617869a334964e372a814d17e05a57df370dd80c1a184ab3e4daa3cb43915",
    ),
    (
        "option_result",
        "2912c8b8003238223af3e1f069e05805bc7cec5e11a9d92a577a28807f3867ff",
    ),
    (
        "place_counter",
        "37376560708489f5803eed2f6a3b2dc9a83972c61635b3c657a048274d51ffcd",
    ),
    (
        "resource_values",
        "0f074a2f880a0a81dc65170ef75c5da2e4e29dd393ed0cba49f9ae438a33ace9",
    ),
    (
        "struct_types",
        "d02bcc6e3f3cf519340fe200b1e820213cf3258af1cc7d9e439563281992efe8",
    ),
    (
        "temporal",
        "bd9489f819a4ffaca486541c2c68ffb0818bc2ae9f7cf3bcc59721670284c243",
    ),
    (
        "tracer_counter",
        "add1d49f20272ed3833c3ba9976d68ae7daa79c1ec55793aae0994cd9f0c5a78",
    ),
    (
        "value_equality",
        "1ed06e247451fa85b7ff7bdfb0855f0fd1c4c4a113de5246c2d2c396b0f3072e",
    ),
    (
        "workshop",
        "ae11fc4bbbf76cdd7d059edc4766783c4ff9f42541f0af3f6a18e21f9ed17580",
    ),
];

#[test]
fn every_conformance_fixture_image_digest_is_pinned() {
    let root = conformance_dir("");
    let mut names: Vec<String> = fs::read_dir(&root)
        .expect("the conformance corpus is present")
        .map(|entry| entry.expect("a corpus entry"))
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let actual: Vec<(String, String)> = names
        .iter()
        .map(|name| {
            let project = capture_project(&root.join(name), OverlaySnapshot::empty())
                .unwrap_or_else(|failure| panic!("capture `{name}`: {failure:?}"));
            let compiled = compile_with_tests(&project)
                .unwrap_or_else(|failure| panic!("compile `{name}`: {failure:?}"));
            (name.clone(), compiled.image.image_id.to_hex())
        })
        .collect();
    let expected: Vec<(String, String)> = DIGESTS
        .iter()
        .map(|(name, digest)| (name.to_string(), digest.to_string()))
        .collect();
    let table: String = actual
        .iter()
        .map(|(name, digest)| format!("    (\"{name}\", \"{digest}\"),\n"))
        .collect();
    assert_eq!(
        actual, expected,
        "the conformance image digests moved; if the change is deliberate, the table is:\n{table}"
    );
}
