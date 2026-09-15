#[path = "common/program.rs"]
mod program;
#[path = "common/scratch.rs"]
mod scratch;

use marrow_local_wire::{
    ClientMessage, DurableState, EncodedFrame, Id32, Json, ServerMessage, WireError,
};
use marrow_runner::{AttachedEphemeralService, AttachedService, Handler};

fn decoded(response: Result<EncodedFrame, WireError>) -> ServerMessage {
    let frame = response.expect("bounded response");
    let (message, turn) =
        ServerMessage::decode_with_turn(&frame.as_bytes()[4..]).expect("response decodes");
    assert_eq!(turn, Some(0));
    message
}

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id index counters.byValue 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     high-water 0\n\
     end\n";

const SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter {
    index byValue[value] unique
}

pub fn set(id: int, v: int) {
    transaction {
        ^counters[id] = Counter(value: v)
    }
}

pub fn two(): int {
    return 2
}

pub fn writeThenFault(id: int): int {
    transaction {
        ^counters[id] = Counter(value: 7)
    }
    return 1 / 0
}

pub fn readValue(id: int): int? {
    return ^counters[id].value
}
"#;

fn id_of(fixture: &program::Program, name: &str) -> Id32 {
    Id32::from_bytes(fixture.export_id(name))
}

/// A unique-index collision faults at the colliding write, before the commit: the wire
/// carries it as an ordinary typed `Fault` with no durable state, the transaction rolls
/// back, and the owner stays usable.
#[test]
fn a_unique_index_fault_is_typed_and_does_not_retire_a_healthy_ephemeral_owner() {
    let fixture = program::build(SOURCE.as_bytes().to_vec(), IDS.as_bytes());
    let mut service =
        AttachedEphemeralService::mint(marrow_lifecycle::prepare(fixture.image.clone()));

    // A committed value, then a second entry whose `value` collides in the unique index.
    match decoded(service.handle(
        ClientMessage::Request {
            export: id_of(&fixture, "set"),
            args: vec![Json::Int(1), Json::Int(5)],
        },
        Some(0),
    )) {
        ServerMessage::Value { .. } => {}
        other => panic!("the first entry commits, got {other:?}"),
    }
    let response = decoded(service.handle(
        ClientMessage::Request {
            export: id_of(&fixture, "set"),
            args: vec![Json::Int(2), Json::Int(5)],
        },
        Some(0),
    ));
    match response {
        ServerMessage::Fault { code, span } => {
            assert_eq!(code, "run.unique_index");
            assert!(span.line > 0);
        }
        other => panic!("expected typed fault response, got {other:?}"),
    }
    assert!(
        !service.close_after_response(),
        "a pre-commit fault has no live recovery fact and leaves the owner usable",
    );

    assert_eq!(
        decoded(service.handle(
            ClientMessage::Request {
                export: id_of(&fixture, "two"),
                args: Vec::new(),
            },
            Some(0)
        )),
        ServerMessage::Value { data: Json::Int(2) },
    );
    assert_eq!(
        decoded(service.handle(
            ClientMessage::Request {
                export: id_of(&fixture, "readValue"),
                args: vec![Json::Int(1)],
            },
            Some(0)
        )),
        ServerMessage::Value { data: Json::Int(5) },
        "the first entry stands after the rolled-back collision",
    );
}

fn assert_known_new_then_read(service: &mut impl Handler, fixture: &program::Program) {
    let response = decoded(service.handle(
        ClientMessage::Request {
            export: id_of(fixture, "writeThenFault"),
            args: vec![Json::Int(4)],
        },
        Some(0),
    ));
    match response {
        ServerMessage::Incomplete {
            code,
            durable,
            span,
        } => {
            assert_eq!(code, "run.divide_by_zero");
            assert_eq!(durable, DurableState::KnownNew);
            assert!(span.line > 0);
        }
        other => panic!("expected typed known-new response, got {other:?}"),
    }
    assert!(
        !service.close_after_response(),
        "known-new has no live recovery fact and leaves the owner usable",
    );
    assert_eq!(
        decoded(service.handle(
            ClientMessage::Request {
                export: id_of(fixture, "readValue"),
                args: vec![Json::Int(4)],
            },
            Some(0)
        )),
        ServerMessage::Value { data: Json::Int(7) },
        "the confirmed write remains even though later bytecode faulted",
    );
}

#[test]
fn confirmed_commit_then_fault_is_known_new_and_keeps_the_ephemeral_owner() {
    let fixture = program::build(SOURCE.as_bytes().to_vec(), IDS.as_bytes());
    let mut service =
        AttachedEphemeralService::mint(marrow_lifecycle::prepare(fixture.image.clone()));
    assert_known_new_then_read(&mut service, &fixture);
}

#[test]
fn confirmed_commit_then_fault_is_known_new_through_the_native_attached_service() {
    let fixture = program::build(SOURCE.as_bytes().to_vec(), IDS.as_bytes());
    let scratch = scratch::Scratch::new("commit-outcome");
    let store = scratch.store();
    let prepared = marrow_lifecycle::prepare(fixture.image.clone());
    let report = marrow_lifecycle::ProvisionReport::new(&store, &prepared)
        .expect("fixture is native executable");
    let approval = marrow_lifecycle::ProvisionApproval::accept(&report);
    marrow_lifecycle::provision_image(&store, &prepared, &approval)
        .expect("provision native fixture");
    let attachment =
        match marrow_lifecycle::attach(&store, prepared).expect("attach native fixture") {
            marrow_lifecycle::AttachOutcome::AlreadyActive(attachment) => attachment,
            marrow_lifecycle::AttachOutcome::Rebound { .. } => {
                panic!("the just-provisioned image is already active")
            }
        };
    let mut service = AttachedService::new(attachment);
    assert_known_new_then_read(&mut service, &fixture);
}
