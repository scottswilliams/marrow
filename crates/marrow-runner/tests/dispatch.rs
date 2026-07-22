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

const ADD: &str = r#"pub fn add(a: int, b: int): int {
    return a + b
}
"#;

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
    let source = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn readValue(n: int): int {
    return ^counters[n].value ?? 0
}
"#;
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
    let source = r#"struct Point {
    x: int
    y: int
}

pub fn shift(p: Point, dx: int): Point {
    return Point(x: p.x + dx, y: p.y)
}
"#;
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

/// A storeless program exercising the earned collection carriers as both a return
/// and a parameter: a `List<int>`, and an ordered `Map<string, int>`.
const COLLECTIONS: &str = r#"pub fn nums(): List<int> {
    var xs: List<int> = List()
    xs = append(xs, 1)
    xs = append(xs, 2)
    return xs
}

pub fn total(xs: List<int>): int {
    var s = 0
    for x in xs {
        s = s + x
    }
    return s
}

pub fn tally(): Map<string, int> {
    var m: Map<string, int> = Map()
    m["a"] = 1
    m["b"] = 2
    return m
}

pub fn lookup(m: Map<string, int>, k: string): int {
    return m[k] ?? 0
}
"#;

fn array(items: Vec<Json>) -> Json {
    Json::Array(items)
}

#[test]
fn a_list_round_trips_through_the_codec() {
    let (service, ids) = build(COLLECTIONS, None);
    // Return: a built list crosses as a JSON array.
    assert_eq!(
        call(&service, id_of(&ids, "nums"), vec![]),
        ServerMessage::Value {
            data: array(vec![Json::Int(1), Json::Int(2)])
        }
    );
    // Parameter: a JSON array decodes onto the `List<int>` parameter.
    assert_eq!(
        call(
            &service,
            id_of(&ids, "total"),
            vec![array(vec![Json::Int(2), Json::Int(3), Json::Int(4)])],
        ),
        ServerMessage::Value { data: Json::Int(9) }
    );
}

#[test]
fn a_hostile_list_argument_is_rejected() {
    let (service, ids) = build(COLLECTIONS, None);
    // A non-array where a list is expected.
    assert_eq!(
        call(&service, id_of(&ids, "total"), vec![Json::Int(3)]),
        ServerMessage::Reject {
            code: "runner.arg_mismatch".to_string()
        }
    );
    // A list element of the wrong scalar type.
    assert_eq!(
        call(
            &service,
            id_of(&ids, "total"),
            vec![array(vec![Json::Int(1), Json::Str("x".to_string())])],
        ),
        ServerMessage::Reject {
            code: "runner.arg_mismatch".to_string()
        }
    );
}

#[test]
fn a_map_round_trips_through_the_codec() {
    let (service, ids) = build(COLLECTIONS, None);
    // Return: an ordered map crosses as an array of [key, value] pairs, in insertion
    // order, never a JS object.
    assert_eq!(
        call(&service, id_of(&ids, "tally"), vec![]),
        ServerMessage::Value {
            data: array(vec![
                array(vec![Json::Str("a".to_string()), Json::Int(1)]),
                array(vec![Json::Str("b".to_string()), Json::Int(2)]),
            ])
        }
    );
    // Parameter: a pair-array decodes onto the `Map<string, int>` parameter.
    let map = array(vec![
        array(vec![Json::Str("a".to_string()), Json::Int(10)]),
        array(vec![Json::Str("b".to_string()), Json::Int(20)]),
    ]);
    assert_eq!(
        call(
            &service,
            id_of(&ids, "lookup"),
            vec![map, Json::Str("b".to_string())],
        ),
        ServerMessage::Value {
            data: Json::Int(20)
        }
    );
}

#[test]
fn a_hostile_map_argument_is_rejected() {
    let (service, ids) = build(COLLECTIONS, None);
    let reject = ServerMessage::Reject {
        code: "runner.arg_mismatch".to_string(),
    };
    let key = Json::Str("k".to_string());
    // A duplicate key.
    let dup = array(vec![
        array(vec![Json::Str("a".to_string()), Json::Int(1)]),
        array(vec![Json::Str("a".to_string()), Json::Int(2)]),
    ]);
    assert_eq!(
        call(&service, id_of(&ids, "lookup"), vec![dup, key.clone()]),
        reject
    );
    // A mis-shaped entry (not a two-element pair).
    let bad_pair = array(vec![array(vec![Json::Str("a".to_string())])]);
    assert_eq!(
        call(&service, id_of(&ids, "lookup"), vec![bad_pair, key.clone()]),
        reject
    );
    // A key of the wrong scalar type (int where the key is a string).
    let bad_key = array(vec![array(vec![Json::Int(1), Json::Int(1)])]);
    assert_eq!(
        call(&service, id_of(&ids, "lookup"), vec![bad_key, key]),
        reject
    );
}

#[test]
fn a_record_with_an_extra_field_is_rejected() {
    let source = r#"struct Point {
    x: int
    y: int
}

pub fn shift(p: Point, dx: int): Point {
    return Point(x: p.x + dx, y: p.y)
}
"#;
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
    let source = r#"enum Shape {
    dot
    circle(radius: int)
}

pub fn grow(s: Shape): Shape {
    match s {
        dot => return Shape::dot
        circle(r) => return Shape::circle(radius: r + 1)
    }
}
"#;
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
