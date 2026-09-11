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

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "marrow-runner-commit-outcome-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch directory");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn compile() -> (marrow_verify::VerifiedImage, Vec<(String, Id32)>) {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        SOURCE.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    let ids = compiled
        .exports
        .iter()
        .map(|export| (export.item.clone(), Id32::from_bytes(*export.id.bytes())))
        .collect();
    (
        marrow_verify::verify(&compiled.image.bytes).expect("verify"),
        ids,
    )
}

fn id_of(ids: &[(String, Id32)], name: &str) -> Id32 {
    ids.iter()
        .find(|(item, _)| item == name)
        .map(|(_, id)| *id)
        .unwrap_or_else(|| panic!("missing export {name}"))
}

/// A unique-index collision faults at the colliding write, before the commit: the wire
/// carries it as an ordinary typed `Fault` with no durable state, the transaction rolls
/// back, and the owner stays usable.
#[test]
fn a_unique_index_fault_is_typed_and_does_not_retire_a_healthy_ephemeral_owner() {
    let (image, ids) = compile();
    let mut service = AttachedEphemeralService::mint(marrow_lifecycle::prepare(image));

    // A committed value, then a second entry whose `value` collides in the unique index.
    match decoded(service.handle(
        ClientMessage::Request {
            export: id_of(&ids, "set"),
            args: vec![Json::Int(1), Json::Int(5)],
        },
        Some(0),
    )) {
        ServerMessage::Value { .. } => {}
        other => panic!("the first entry commits, got {other:?}"),
    }
    let response = decoded(service.handle(
        ClientMessage::Request {
            export: id_of(&ids, "set"),
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
                export: id_of(&ids, "two"),
                args: Vec::new(),
            },
            Some(0)
        )),
        ServerMessage::Value { data: Json::Int(2) },
    );
    assert_eq!(
        decoded(service.handle(
            ClientMessage::Request {
                export: id_of(&ids, "readValue"),
                args: vec![Json::Int(1)],
            },
            Some(0)
        )),
        ServerMessage::Value { data: Json::Int(5) },
        "the first entry stands after the rolled-back collision",
    );
}

fn assert_known_new_then_read(service: &mut impl Handler, ids: &[(String, Id32)]) {
    let response = decoded(service.handle(
        ClientMessage::Request {
            export: id_of(ids, "writeThenFault"),
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
                export: id_of(ids, "readValue"),
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
    let (image, ids) = compile();
    let mut service = AttachedEphemeralService::mint(marrow_lifecycle::prepare(image));
    assert_known_new_then_read(&mut service, &ids);
}

#[test]
fn confirmed_commit_then_fault_is_known_new_through_the_native_attached_service() {
    let (image, ids) = compile();
    let scratch = Scratch::new();
    let store = scratch.0.join("store");
    let prepared = marrow_lifecycle::prepare(image);
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
    assert_known_new_then_read(&mut service, &ids);
}
