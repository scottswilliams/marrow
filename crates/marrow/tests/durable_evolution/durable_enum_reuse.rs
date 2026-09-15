//! Two durable fields of the SAME enum type must verify.
//!
//! An enum's durable identity (its sum id and per-member ids) is a per-declaration
//! claim, minted once at the enum's canonical spelling and referenced by every field
//! whose stored value is that enum. A wide sparse resource that declares two fields of
//! one enum type — including two `Option<int>` optional-int fields — reuses that one
//! identity twice. The compiler emits the shared identity faithfully; the verifier must
//! read the two references as one claim, not reject the reuse as a duplicate ledger id.

use crate::common::Project;

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0\n\
     id product Reading d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0\n\
     id root readings b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0\n\
     id key readings.id c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0\n\
     id field Reading.id e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0\n\
     id field Reading.glucose e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1\n\
     id field Reading.lactate e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2\n\
     id sum Option[int] 60606060606060606060606060606060\n\
     id member Option[int].none 61616161616161616161616161616161\n\
     id member Option[int].some 62626262626262626262626262626262\n\
     high-water 0\n\
     end\n";

// Two durable fields of the one enum type `Option<int>`: the natural shape of a wide
// sparse resource where many optional readings share the one optional-int type.
const SOURCE: &str = r#"resource Reading {
    required id: int
    glucose: Option<int>
    lactate: Option<int>
}

store ^readings[id: int]: Reading

pub fn recordGlucose(id: int, v: int) {
    transaction {
        ^readings[id] = Reading(id: id, glucose: some(v))
    }
}
"#;

#[test]
fn two_fields_of_one_enum_type_verify() {
    // `.image()` compiles and independently verifies: the verifier reads the two
    // references to one enum identity as a single per-declaration claim.
    Project::single(SOURCE).ids(IDS).image();
}
