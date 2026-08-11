//! Integration suite for the pure project-input owner. Each case module owns one
//! concern: the manifest schema, file-identity derivation, and contained capture.

#[path = "cases/capture.rs"]
mod capture;
#[path = "cases/identity.rs"]
mod identity;
#[path = "cases/identity_kind_tags.rs"]
mod identity_kind_tags;
#[path = "cases/manifest.rs"]
mod manifest;
