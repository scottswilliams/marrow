//! The T01 store profile (design §G, finding 12).
//!
//! A meta cell written at first provision records the profile version, the value
//! codec version, and a canonical schema descriptor (root name, key type, and each
//! field's name, type, and required flag). Reopening recomputes the descriptor from
//! the verified image and refuses on any mismatch, so name-keyed cells can never be
//! reinterpreted under a different schema. D00 bumps the profile version, so a T01
//! store is refused by a later toolchain rather than silently read.

use crate::codec::value::{ScalarKind, VALUE_CODEC_VERSION, ValueShape};

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
        shape,
        required,
    } in fields
    {
        push_name(out, name);
        push_shape(out, shape);
        out.push(u8::from(*required));
    }
}

/// The composite-shape discriminants. Chosen outside the `kind_tag` range (`0x01..=0x07`)
/// so a scalar field's descriptor stays byte-identical to the pre-widening form (no
/// `PROFILE_VERSION` bump for adding composites): a leading byte `0x01..=0x07` is a scalar
/// kind, `0xF1`/`0xF2` a product/sum. This interim recursive encoding fingerprints a
/// composite field shape so the profile is well-formed and reopen agrees; the canonical
/// prefix-free value-shape descriptor of §A7 (with its deliberate `PROFILE_VERSION` bump
/// and drift KATs) supersedes it in the descriptor lane.
const SHAPE_PRODUCT: u8 = 0xF1;
const SHAPE_SUM: u8 = 0xF2;

/// Append a field's value shape to the descriptor, recursively. A scalar emits its single
/// kind tag (byte-identical to the pre-widening descriptor). A product emits its type
/// index, leaf count, and each leaf shape in order; a sum emits its type index, variant
/// count, and each variant's payload count and payload shapes. The structure and every
/// leaf kind are covered, so a shape change at any depth changes the descriptor.
fn push_shape(out: &mut Vec<u8>, shape: &ValueShape) {
    match shape {
        ValueShape::Scalar(kind) => out.push(kind_tag(*kind)),
        ValueShape::Product { ty, fields } => {
            out.push(SHAPE_PRODUCT);
            out.extend_from_slice(&ty.to_be_bytes());
            out.extend_from_slice(&(fields.len() as u16).to_be_bytes());
            for field in fields {
                push_shape(out, field);
            }
        }
        ValueShape::Sum { ty, variants } => {
            out.push(SHAPE_SUM);
            out.extend_from_slice(&ty.to_be_bytes());
            out.extend_from_slice(&(variants.len() as u16).to_be_bytes());
            for payload in variants {
                out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
                for leaf in payload {
                    push_shape(out, leaf);
                }
            }
        }
    }
}

/// Append a node's ordered key columns to the descriptor: a column count then each
/// column's kind tag, in order. A composite key changes the descriptor from a
/// single-column key of the same leading kind, so a store recorded under one key arity
/// refuses a reopen under another. Shared by the root and every branch key.
fn push_key(out: &mut Vec<u8>, columns: &[ScalarKind]) {
    out.extend_from_slice(&(columns.len() as u16).to_be_bytes());
    for kind in columns {
        out.push(kind_tag(*kind));
    }
}

/// Append a node's keyed branches to the descriptor: a branch count then, for each
/// branch in declaration order, its name, ordered key columns, own fields, and —
/// recursively — its own nested branches. The recursion makes the descriptor cover a
/// whole nested branch shape, so a change at any branch depth changes the descriptor and
/// a store's recorded profile refuses a reopen under a different sub-branch shape.
fn push_branches(out: &mut Vec<u8>, branches: &[BranchSchema]) {
    out.extend_from_slice(&(branches.len() as u16).to_be_bytes());
    for BranchSchema {
        name,
        key,
        fields,
        branches,
    } in branches
    {
        push_name(out, name);
        push_key(out, key);
        push_fields(out, fields);
        push_branches(out, branches);
    }
}

/// The canonical profile descriptor bytes for `schema`: the root's name, key, and
/// fields, then its whole nested branch shape in declaration order. A branch schema
/// change at any depth therefore changes the descriptor, so a store's recorded profile
/// refuses a reopen under a different branch shape.
pub(super) fn descriptor(schema: &StoreSchema) -> Vec<u8> {
    let mut out = vec![PROFILE_VERSION];
    out.extend_from_slice(&VALUE_CODEC_VERSION.to_be_bytes());
    push_name(&mut out, &schema.root_name);
    push_key(&mut out, &schema.key);
    push_fields(&mut out, &schema.fields);
    push_branches(&mut out, &schema.branches);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_distinguishes_schemas() {
        let base = StoreSchema {
            root_name: "counters".into(),
            key: vec![ScalarKind::Str],
            fields: vec![
                FieldSchema::scalar("value", ScalarKind::Int, true),
                FieldSchema::scalar("label", ScalarKind::Str, false),
            ],
            branches: Vec::new(),
        };
        let same = descriptor(&base);
        assert_eq!(same, descriptor(&base), "descriptor is deterministic");

        // A changed key type, field name, field type, or required flag all differ.
        let mut key_changed = base.clone();
        key_changed.key = vec![ScalarKind::Int];
        assert_ne!(descriptor(&base), descriptor(&key_changed));

        let mut required_changed = base.clone();
        required_changed.fields[0].required = false;
        assert_ne!(descriptor(&base), descriptor(&required_changed));

        // Adding a keyed branch changes the descriptor, so a store recorded flat
        // refuses a reopen whose schema grew a branch, and vice versa.
        let mut branch_added = base.clone();
        branch_added.branches.push(BranchSchema {
            name: "notes".into(),
            key: vec![ScalarKind::Int],
            fields: vec![FieldSchema::scalar("text", ScalarKind::Str, true)],
            branches: Vec::new(),
        });
        assert_ne!(descriptor(&base), descriptor(&branch_added));

        // A change inside a branch's own record also differs.
        let mut branch_field_changed = branch_added.clone();
        branch_field_changed.branches[0].fields[0].required = false;
        assert_ne!(descriptor(&branch_added), descriptor(&branch_field_changed));

        // Adding a nested sub-branch, and a change inside it, both differ: the
        // descriptor covers the whole nested branch shape, not just the first level.
        let mut sub_branch_added = branch_added.clone();
        sub_branch_added.branches[0].branches.push(BranchSchema {
            name: "tags".into(),
            key: vec![ScalarKind::Str],
            fields: vec![FieldSchema::scalar("weight", ScalarKind::Int, false)],
            branches: Vec::new(),
        });
        assert_ne!(descriptor(&branch_added), descriptor(&sub_branch_added));

        let mut sub_branch_field_changed = sub_branch_added.clone();
        sub_branch_field_changed.branches[0].branches[0].key = vec![ScalarKind::Int];
        assert_ne!(
            descriptor(&sub_branch_added),
            descriptor(&sub_branch_field_changed),
        );
    }

    #[test]
    fn descriptor_distinguishes_widened_field_shapes() {
        let base = |shape: ValueShape| StoreSchema {
            root_name: "accounts".into(),
            key: vec![ScalarKind::Int],
            fields: vec![FieldSchema {
                name: "f".into(),
                shape,
                required: true,
            }],
            branches: Vec::new(),
        };
        let scalar = base(ValueShape::Scalar(ScalarKind::Int));
        let product = base(ValueShape::Product {
            ty: 3,
            fields: vec![ValueShape::Scalar(ScalarKind::Str)],
        });
        let sum = base(ValueShape::Sum {
            ty: 3,
            variants: vec![vec![], vec![ValueShape::Scalar(ScalarKind::Str)]],
        });

        // A composite descriptor is deterministic and distinct from a scalar one and from
        // a different composite shape.
        assert_eq!(descriptor(&product), descriptor(&product));
        assert_ne!(descriptor(&scalar), descriptor(&product));
        assert_ne!(descriptor(&scalar), descriptor(&sum));
        assert_ne!(descriptor(&product), descriptor(&sum));

        // A shape change at depth (the sum's payload leaf kind, or its type index) differs.
        let sum_other_leaf = base(ValueShape::Sum {
            ty: 3,
            variants: vec![vec![], vec![ValueShape::Scalar(ScalarKind::Int)]],
        });
        assert_ne!(descriptor(&sum), descriptor(&sum_other_leaf));
        let sum_other_ty = base(ValueShape::Sum {
            ty: 4,
            variants: vec![vec![], vec![ValueShape::Scalar(ScalarKind::Str)]],
        });
        assert_ne!(descriptor(&sum), descriptor(&sum_other_ty));

        // A scalar field's descriptor is byte-identical to the pre-widening form: a single
        // kind tag with no composite discriminant. Adding composites bumps no version.
        assert_eq!(descriptor(&scalar), {
            let mut out = vec![PROFILE_VERSION];
            out.extend_from_slice(&VALUE_CODEC_VERSION.to_be_bytes());
            push_name(&mut out, "accounts");
            push_key(&mut out, &[ScalarKind::Int]);
            out.extend_from_slice(&1u16.to_be_bytes());
            push_name(&mut out, "f");
            out.push(kind_tag(ScalarKind::Int));
            out.push(1);
            out.extend_from_slice(&0u16.to_be_bytes());
            out
        });
    }
}
