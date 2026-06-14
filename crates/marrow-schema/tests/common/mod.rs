//! Shared helpers for the resource-compilation integration tests.
use marrow_schema::{SchemaError, SchemaErrorKind};

pub fn codes(errors: &[SchemaError]) -> Vec<&'static str> {
    errors.iter().map(|error| error.code).collect()
}

pub fn assert_kind(error: &SchemaError, kind: SchemaErrorKind) {
    assert_eq!(error.kind, kind);
}
