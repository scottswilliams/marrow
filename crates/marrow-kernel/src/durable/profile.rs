//! The T01 store profile (design §G, finding 12).
//!
//! A meta cell written at first provision records the profile version, the value
//! codec version, and a canonical schema descriptor (root name, key type, and each
//! field's name, type, and required flag). Reopening recomputes the descriptor from
//! the verified image and refuses on any mismatch, so name-keyed cells can never be
//! reinterpreted under a different schema. D00 bumps the profile version, so a T01
//! store is refused by a later toolchain rather than silently read.

use crate::codec::value::{ScalarKind, VALUE_CODEC_VERSION};

use super::{BranchSchema, FieldSchema, StoreSchema};

/// The T01 profile version. D00 introduces a distinct version.
const PROFILE_VERSION: u8 = 0x01;

/// The stable descriptor tag for a scalar kind. Independent of the language type
/// tags and of the key order tags; a frozen wire byte for the profile only.
fn kind_tag(kind: ScalarKind) -> u8 {
    match kind {
        ScalarKind::Bool => 0x01,
        ScalarKind::Int => 0x02,
        ScalarKind::Str => 0x03,
        ScalarKind::Bytes => 0x04,
        ScalarKind::Date => 0x05,
        ScalarKind::Duration => 0x06,
        ScalarKind::Instant => 0x07,
    }
}

fn push_name(out: &mut Vec<u8>, name: &str) {
    out.extend_from_slice(&(name.len() as u16).to_be_bytes());
    out.extend_from_slice(name.as_bytes());
}

/// Append a record's fields to the descriptor: a field count then each field's name,
/// kind tag, and required flag, in order. Shared by the root record and every branch
/// record so a node's field descriptor has one owner.
fn push_fields(out: &mut Vec<u8>, fields: &[FieldSchema]) {
    out.extend_from_slice(&(fields.len() as u16).to_be_bytes());
    for FieldSchema {
        name,
        kind,
        required,
    } in fields
    {
        push_name(out, name);
        out.push(kind_tag(*kind));
        out.push(u8::from(*required));
    }
}

/// The canonical profile descriptor bytes for `schema`: the root's name, key, and
/// fields, then its keyed branches (each a name, single key kind, and own fields) in
/// declaration order. A branch schema change therefore changes the descriptor, so a
/// store's recorded profile refuses a reopen under a different branch shape.
pub(super) fn descriptor(schema: &StoreSchema) -> Vec<u8> {
    let mut out = vec![PROFILE_VERSION];
    out.extend_from_slice(&VALUE_CODEC_VERSION.to_be_bytes());
    push_name(&mut out, &schema.root_name);
    out.push(kind_tag(schema.key));
    push_fields(&mut out, &schema.fields);
    out.extend_from_slice(&(schema.branches.len() as u16).to_be_bytes());
    for BranchSchema { name, key, fields } in &schema.branches {
        push_name(&mut out, name);
        out.push(kind_tag(*key));
        push_fields(&mut out, fields);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_distinguishes_schemas() {
        let base = StoreSchema {
            root_name: "counters".into(),
            key: ScalarKind::Str,
            fields: vec![
                FieldSchema {
                    name: "value".into(),
                    kind: ScalarKind::Int,
                    required: true,
                },
                FieldSchema {
                    name: "label".into(),
                    kind: ScalarKind::Str,
                    required: false,
                },
            ],
            branches: Vec::new(),
        };
        let same = descriptor(&base);
        assert_eq!(same, descriptor(&base), "descriptor is deterministic");

        // A changed key type, field name, field type, or required flag all differ.
        let mut key_changed = base.clone();
        key_changed.key = ScalarKind::Int;
        assert_ne!(descriptor(&base), descriptor(&key_changed));

        let mut required_changed = base.clone();
        required_changed.fields[0].required = false;
        assert_ne!(descriptor(&base), descriptor(&required_changed));

        // Adding a keyed branch changes the descriptor, so a store recorded flat
        // refuses a reopen whose schema grew a branch, and vice versa.
        let mut branch_added = base.clone();
        branch_added.branches.push(BranchSchema {
            name: "notes".into(),
            key: ScalarKind::Int,
            fields: vec![FieldSchema {
                name: "text".into(),
                kind: ScalarKind::Str,
                required: true,
            }],
        });
        assert_ne!(descriptor(&base), descriptor(&branch_added));

        // A change inside a branch's own record also differs.
        let mut branch_field_changed = branch_added.clone();
        branch_field_changed.branches[0].fields[0].required = false;
        assert_ne!(descriptor(&branch_added), descriptor(&branch_field_changed));
    }
}
