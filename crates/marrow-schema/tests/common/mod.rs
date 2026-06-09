//! Shared helpers for the resource-compilation integration-test binaries.
//!
//! Each test binary that pulls this in via `mod common` uses only the subset of
//! helpers it needs, so unused-helper warnings here are expected.
#![allow(dead_code)]

use marrow_schema::{SchemaError, SchemaErrorKind};

pub fn codes(errors: &[SchemaError]) -> Vec<&'static str> {
    errors.iter().map(|error| error.code).collect()
}

pub fn assert_kind(error: &SchemaError, kind: SchemaErrorKind) {
    assert_eq!(error.kind, kind);
}
