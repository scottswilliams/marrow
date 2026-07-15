//! Request dispatch and the transfer codec, exercised in process against images
//! compiled through the production pipeline. No socket is bound here, so these run
//! under the ordinary sandbox; the channel discipline is covered by `channel.rs`.

use marrow_local_wire::{ClientMessage, Json, ServerMessage};
use marrow_runner::{Id32, Service};

/// The durable identity ledger used by the durable fixture below.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

/// Compile and verify one `src/main.mw`, build a runner service, and return it with
/// the export name → wire identity map.
fn build(source: &str, ids: Option<&[u8]>) -> (Service, Vec<(String, Id32)>) {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        ids,
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    let idmap = compiled
        .exports
        .iter()
        .map(|entry| (entry.item.clone(), Id32::from_bytes(*entry.id.bytes())))
        .collect();
    let image = marrow_verify::verify(&compiled.image.bytes).expect("verify");
    let service = Service::build(image).expect("service builds");
    (service, idmap)
}

fn id_of(idmap: &[(String, Id32)], name: &str) -> Id32 {
    idmap
        .iter()
        .find(|(item, _)| item == name)
        .map(|(_, id)| *id)
        .unwrap_or_else(|| panic!("no export {name}"))
}

fn call(service: &Service, export: Id32, args: Vec<Json>) -> ServerMessage {
    service.handle(ClientMessage::Request { export, args })
}

const ADD: &str = "pub fn add(a: int, b: int): int\n\x20   return a + b\n";

#[test]
fn a_storeless_call_returns_its_value() {
    let (service, ids) = build(ADD, None);
    let response = call(
        &service,
        id_of(&ids, "add"),
        vec![Json::Int(2), Json::Int(3)],
    );
    assert_eq!(response, ServerMessage::Value { data: Json::Int(5) });
}

#[test]
fn a_runtime_fault_maps_to_a_fault_response() {
    let (service, ids) = build(ADD, None);
    let response = call(
        &service,
        id_of(&ids, "add"),
        vec![Json::Int(i64::MAX), Json::Int(1)],
    );
    match response {
        ServerMessage::Fault { code, .. } => assert_eq!(code, "run.overflow"),
        other => panic!("expected a fault, got {other:?}"),
    }
}

#[test]
fn an_unknown_export_is_rejected() {
    let (service, _ids) = build(ADD, None);
    let response = call(&service, Id32::from_bytes([0; 32]), vec![]);
    assert_eq!(
        response,
        ServerMessage::Reject {
            code: "runner.unknown_export".to_string()
        }
    );
}

#[test]
fn an_argument_count_mismatch_is_rejected() {
    let (service, ids) = build(ADD, None);
    let response = call(&service, id_of(&ids, "add"), vec![Json::Int(1)]);
    assert_eq!(
        response,
        ServerMessage::Reject {
            code: "runner.arg_mismatch".to_string()
        }
    );
}

#[test]
fn an_argument_type_mismatch_is_rejected() {
    let (service, ids) = build(ADD, None);
    let response = call(
        &service,
        id_of(&ids, "add"),
        vec![Json::Str("x".to_string()), Json::Int(1)],
    );
    assert_eq!(
        response,
        ServerMessage::Reject {
            code: "runner.arg_mismatch".to_string()
        }
    );
}

#[test]
fn a_durable_export_is_rejected_in_the_trough() {
    let source = "resource Counter\n\
         \x20   required value: int\n\
         \x20   label: string\n\
         \n\
         store ^counters(id: int): Counter\n\
         \n\
         pub fn readValue(n: int): int\n\
         \x20   return ^counters(n).value ?? 0\n";
    let (service, ids) = build(source, Some(IDS.as_bytes()));
    let response = call(&service, id_of(&ids, "readValue"), vec![Json::Int(1)]);
    assert_eq!(
        response,
        ServerMessage::Reject {
            code: "runner.durable_unsupported".to_string()
        }
    );
}

#[test]
fn a_record_round_trips_through_the_codec() {
    let source = "struct Point\n\
         \x20   x: int\n\
         \x20   y: int\n\
         \n\
         pub fn shift(p: Point, dx: int): Point\n\
         \x20   return Point(x: p.x + dx, y: p.y)\n";
    let (service, ids) = build(source, None);
    let point = Json::Object(vec![
        ("x".to_string(), Json::Int(1)),
        ("y".to_string(), Json::Int(2)),
    ]);
    let response = call(&service, id_of(&ids, "shift"), vec![point, Json::Int(10)]);
    assert_eq!(
        response,
        ServerMessage::Value {
            data: Json::Object(vec![
                ("x".to_string(), Json::Int(11)),
                ("y".to_string(), Json::Int(2)),
            ])
        }
    );
}

#[test]
fn a_record_with_an_extra_field_is_rejected() {
    let source = "struct Point\n\
         \x20   x: int\n\
         \x20   y: int\n\
         \n\
         pub fn shift(p: Point, dx: int): Point\n\
         \x20   return Point(x: p.x + dx, y: p.y)\n";
    let (service, ids) = build(source, None);
    let point = Json::Object(vec![
        ("x".to_string(), Json::Int(1)),
        ("y".to_string(), Json::Int(2)),
        ("z".to_string(), Json::Int(3)),
    ]);
    let response = call(&service, id_of(&ids, "shift"), vec![point, Json::Int(0)]);
    assert_eq!(
        response,
        ServerMessage::Reject {
            code: "runner.arg_mismatch".to_string()
        }
    );
}

#[test]
fn an_enum_round_trips_through_the_codec() {
    let source = "enum Shape\n\
         \x20   dot\n\
         \x20   circle(radius: int)\n\
         \n\
         pub fn grow(s: Shape): Shape\n\
         \x20   match s\n\
         \x20       dot\n\
         \x20           return Shape::dot\n\
         \x20       circle(r)\n\
         \x20           return Shape::circle(radius: r + 1)\n";
    let (service, ids) = build(source, None);
    let export = id_of(&ids, "grow");

    let circle = Json::Object(vec![
        ("member".to_string(), Json::Str("circle".to_string())),
        ("payload".to_string(), Json::Array(vec![Json::Int(4)])),
    ]);
    let grown = call(&service, export, vec![circle]);
    assert_eq!(
        grown,
        ServerMessage::Value {
            data: Json::Object(vec![
                ("member".to_string(), Json::Str("circle".to_string())),
                ("payload".to_string(), Json::Array(vec![Json::Int(5)])),
            ])
        }
    );

    let dot = Json::Object(vec![
        ("member".to_string(), Json::Str("dot".to_string())),
        ("payload".to_string(), Json::Array(vec![])),
    ]);
    let same = call(&service, export, vec![dot]);
    assert_eq!(
        same,
        ServerMessage::Value {
            data: Json::Object(vec![
                ("member".to_string(), Json::Str("dot".to_string())),
                ("payload".to_string(), Json::Array(vec![])),
            ])
        }
    );
}

#[test]
fn the_service_interface_id_is_deterministic() {
    let (a, _) = build(ADD, None);
    let (b, _) = build(ADD, None);
    assert_eq!(a.interface_id(), b.interface_id());
}
