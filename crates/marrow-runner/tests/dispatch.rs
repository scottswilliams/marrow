//! Request dispatch and the transfer codec, exercised in process against images
//! compiled through the production pipeline. No socket is bound here, so these run
//! under the ordinary sandbox; the channel discipline is covered by `channel.rs`.

use marrow_local_wire::{ClientMessage, Json, MAX_FRAME, ServerMessage, frame_body_len};
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

fn framed_call(
    service: &Service,
    export: Id32,
    args: Vec<Json>,
    expected_frame_bytes: usize,
) -> ServerMessage {
    let frame = ClientMessage::Request { export, args }
        .encode()
        .expect("request fits a complete frame");
    assert_eq!(frame.len(), expected_frame_bytes);
    let header = frame[..4].try_into().expect("four-byte frame header");
    let body_len = frame_body_len(header).expect("frame body is admitted");
    assert_eq!(body_len, frame.len() - 4);
    assert!(body_len <= MAX_FRAME);
    let request = ClientMessage::decode(&frame[4..]).expect("framed request decodes");
    service.handle(request)
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
fn framed_maps_enforce_count_bytes_and_exact_cap_replacement() {
    let source = r#"pub fn mapCount(m: Map<int, bool>): int {
    return length(m)
}
pub fn mapBytes(m: Map<string, List<int>>): int {
    return length(m)
}
pub fn replaceAtCap(m: Map<int, int>): int {
    var changed = m
    changed[0] = 9
    return changed[0] ?? -1
}
pub fn growAtCap(m: Map<int, int>): int {
    var changed = m
    changed[0] = 9
    changed[65536] = 10
    return length(changed)
}
"#;
    let (service, ids) = build(source, None);
    let reject = ServerMessage::Reject {
        code: "runner.arg_mismatch".to_string(),
    };
    // Nine structural bytes per pair isolate cardinality from the byte limit.
    for (count, frame_bytes, expected) in [
        (
            65_536,
            906_513,
            ServerMessage::Value {
                data: Json::Int(65_536),
            },
        ),
        (65_537, 906_527, reject.clone()),
    ] {
        let entries = (0..count)
            .map(|key| array(vec![Json::Int(key), Json::Bool(false)]))
            .collect();
        assert_eq!(
            framed_call(
                &service,
                id_of(&ids, "mapCount"),
                vec![array(entries)],
                frame_bytes
            ),
            expected,
            "Map count {count}"
        );
    }
    // Each key and child-list header costs one byte; all children are valid.
    for (case, first_key, second_len, frame_bytes, expected) in [
        (
            "exact",
            "a",
            65_534,
            262_329,
            ServerMessage::Value { data: Json::Int(8) },
        ),
        ("key-excess", "aa", 65_534, 262_330, reject.clone()),
        ("value-excess", "a", 65_535, 262_331, reject),
    ] {
        let entries = [
            (first_key, 65_536),
            ("b", second_len),
            ("c", 0),
            ("d", 0),
            ("e", 0),
            ("f", 0),
            ("g", 0),
            ("h", 0),
        ]
        .into_iter()
        .map(|(key, len)| {
            array(vec![
                Json::Str(key.to_string()),
                array(vec![Json::Int(0); len]),
            ])
        })
        .collect();
        assert_eq!(
            framed_call(
                &service,
                id_of(&ids, "mapBytes"),
                vec![array(entries)],
                frame_bytes
            ),
            expected,
            "Map aggregate {case}"
        );
    }
    // Both limits are exact: replacement keeps the existing key charge and count.
    for name in ["replaceAtCap", "growAtCap"] {
        let entries = (0..65_536)
            .map(|key| array(vec![Json::Int(key), Json::Int(0)]))
            .collect();
        let reply = framed_call(&service, id_of(&ids, name), vec![array(entries)], 644_369);
        match (name, reply) {
            ("replaceAtCap", ServerMessage::Value { data: Json::Int(9) }) => {}
            ("growAtCap", ServerMessage::Fault { code, .. }) => {
                assert_eq!(code, "run.collection_limit");
            }
            _ => panic!("unexpected exact-cap Map {name} outcome"),
        }
    }
}

#[test]
fn collection_limits_reach_nested_record_and_enum_arguments() {
    let source = r#"struct Items {
    xs: List<int>
}
pub fn nestedCount(xs: List<List<int>>): int {
    return length(xs)
}
pub fn recordCount(value: Items): int {
    return length(value.xs)
}
pub fn choiceCount(value: Option<Items>): int {
    match value {
        none => return 0
        some(items) => return length(items.xs)
    }
}
"#;
    let (service, ids) = build(source, None);
    for count in [1, 65_537] {
        for name in ["nestedCount", "recordCount", "choiceCount"] {
            let list = array(vec![Json::Int(0); count]);
            let (input, wrapper_bytes) = match name {
                "nestedCount" => (array(vec![list]), 2),
                "recordCount" => (Json::Object(vec![("xs".to_string(), list)]), 7),
                "choiceCount" => (
                    Json::Object(vec![
                        ("member".to_string(), Json::Str("some".to_string())),
                        (
                            "payload".to_string(),
                            array(vec![Json::Object(vec![("xs".to_string(), list)])]),
                        ),
                    ]),
                    37,
                ),
                _ => unreachable!("fixed recursion cases"),
            };
            let expected = if count == 1 {
                ServerMessage::Value { data: Json::Int(1) }
            } else {
                ServerMessage::Reject {
                    code: "runner.arg_mismatch".to_string(),
                }
            };
            assert_eq!(
                framed_call(
                    &service,
                    id_of(&ids, name),
                    vec![input],
                    118 + 2 * count + 1 + wrapper_bytes,
                ),
                expected,
                "{name}/child-count-{count}"
            );
        }
    }
    assert_eq!(
        framed_call(
            &service,
            id_of(&ids, "choiceCount"),
            vec![Json::Object(vec![
                ("member".to_string(), Json::Str("none".to_string())),
                ("payload".to_string(), array(vec![])),
            ])],
            148,
        ),
        ServerMessage::Value { data: Json::Int(0) }
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
fn framed_list_arguments_enforce_the_element_limit() {
    let source = r#"pub fn count(xs: List<int>): int {
    return length(xs)
}
"#;
    let (service, ids) = build(source, None);
    let export = id_of(&ids, "count");
    assert_eq!(
        framed_call(
            &service,
            export,
            vec![array(vec![Json::Int(0); 65_536])],
            131_191,
        ),
        ServerMessage::Value {
            data: Json::Int(65_536)
        }
    );
    assert_eq!(
        framed_call(
            &service,
            export,
            vec![array(vec![Json::Int(0); 65_537])],
            131_193,
        ),
        ServerMessage::Reject {
            code: "runner.arg_mismatch".to_string()
        }
    );
}

#[test]
fn framed_nested_lists_enforce_the_parent_byte_limit() {
    let source = r#"pub fn count(xs: List<List<int>>): int {
    return length(xs)
}
"#;
    let (service, ids) = build(source, None);
    let export = id_of(&ids, "count");
    // Eight child headers plus 131,071 eight-byte ints exactly fill the parent.
    let exact = [65_536, 65_535, 0, 0, 0, 0, 0, 0]
        .into_iter()
        .map(|len| array(vec![Json::Int(0); len]))
        .collect();
    assert_eq!(
        framed_call(&service, export, vec![array(exact)], 262_283),
        ServerMessage::Value { data: Json::Int(8) }
    );
    // Each child is valid; their parent needs 2 * (1 + 524,288) bytes.
    let excess = (0..2).map(|_| array(vec![Json::Int(0); 65_536])).collect();
    assert_eq!(
        framed_call(&service, export, vec![array(excess)], 262_267),
        ServerMessage::Reject {
            code: "runner.arg_mismatch".to_string()
        }
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
    // Return: an ordered map crosses as an array of [key, value] pairs in ascending
    // key order, never a JS object.
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
fn a_map_argument_accepts_unique_pairs_in_reverse_order() {
    let (service, ids) = build(COLLECTIONS, None);
    let map = array(vec![
        array(vec![Json::Str("b".to_string()), Json::Int(20)]),
        array(vec![Json::Str("a".to_string()), Json::Int(10)]),
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
fn map_arguments_preserve_canonical_order_and_lookup_for_every_key_type() {
    let text = |value: &str| Json::Str(value.to_string());
    let text_keys = |values: &[&str]| values.iter().map(|value| text(value)).collect::<Vec<_>>();
    let families = [
        ("bool", vec![Json::Bool(false), Json::Bool(true)]),
        ("int", [-2, -1, 2, 10].into_iter().map(Json::Int).collect()),
        ("string", text_keys(&["a", "aa", "b"])),
        ("bytes", text_keys(&["0x", "0x00", "0x0000", "0x01"])),
        (
            "date",
            text_keys(&["1969-12-31", "1970-01-01", "2000-02-29"]),
        ),
        (
            "instant",
            text_keys(&[
                "1969-12-31T23:59:59.999999999Z",
                "1970-01-01T00:00:00Z",
                "1970-01-01T00:00:00.000000001Z",
            ]),
        ),
        ("duration", text_keys(&["-PT2S", "-PT1S", "PT2S", "PT10S"])),
    ];
    let mut source = String::new();
    for (kind, _) in &families {
        source.push_str(&format!(
            r#"struct Observed_{kind} {{
    entries: Map<{kind}, int>
    found: int
}}

pub fn observe_{kind}(m: Map<{kind}, int>, k: {kind}): Observed_{kind} {{
    return Observed_{kind}(entries: m, found: m[k] ?? -1)
}}

"#,
        ));
    }
    source.push_str(
        r#"pub fn nested(m: Map<string, Map<int, int>>): Map<string, Map<int, int>> {
    return m
}
"#,
    );
    let (service, ids) = build(&source, None);
    let pair = |key, value| array(vec![key, value]);
    for (kind, keys) in families {
        let export = id_of(&ids, &format!("observe_{kind}"));
        let entries = keys
            .iter()
            .enumerate()
            .map(|(index, key)| {
                let value = i64::try_from(index + 1).expect("small key fixture") * 10;
                pair(key.clone(), Json::Int(value))
            })
            .collect::<Vec<_>>();
        let reversed = entries.iter().cloned().rev().collect::<Vec<_>>();
        for (case, input, ordered) in [
            ("empty", vec![], vec![]),
            (
                "singleton",
                vec![entries[0].clone()],
                vec![entries[0].clone()],
            ),
            ("sorted", entries.clone(), entries.clone()),
            ("reversed", reversed, entries.clone()),
        ] {
            // Query every admitted key, including missing keys in the small controls.
            for (index, key) in keys.iter().enumerate() {
                let found = if index < ordered.len() {
                    i64::try_from(index + 1).expect("small key fixture") * 10
                } else {
                    -1
                };
                assert_eq!(
                    call(&service, export, vec![array(input.clone()), key.clone()]),
                    ServerMessage::Value {
                        data: Json::Object(vec![
                            ("entries".to_string(), array(ordered.clone())),
                            ("found".to_string(), Json::Int(found)),
                        ])
                    },
                    "{kind}/{case}/lookup-{index}"
                );
            }
        }
        let wrong_key = if kind == "int" {
            Json::Bool(false)
        } else {
            Json::Int(1)
        };
        for (case, input) in [
            (
                "nonadjacent-duplicate",
                array(vec![
                    entries[0].clone(),
                    entries[1].clone(),
                    pair(keys[0].clone(), Json::Int(999)),
                ]),
            ),
            ("malformed-pair", array(vec![array(vec![keys[0].clone()])])),
            ("non-pair", array(vec![Json::Null])),
            (
                "wrong-value",
                array(vec![pair(keys[0].clone(), Json::Bool(false))]),
            ),
            ("wrong-key", array(vec![pair(wrong_key, Json::Int(1))])),
        ] {
            assert_eq!(
                call(&service, export, vec![input, keys[0].clone()]),
                ServerMessage::Reject {
                    code: "runner.arg_mismatch".to_string()
                },
                "{kind}/{case}"
            );
        }
    }
    let nested_input = array(vec![
        pair(
            text("b"),
            array(vec![
                pair(Json::Int(2), Json::Int(20)),
                pair(Json::Int(-10), Json::Int(10)),
            ]),
        ),
        pair(
            text("a"),
            array(vec![
                pair(Json::Int(10), Json::Int(40)),
                pair(Json::Int(-2), Json::Int(30)),
            ]),
        ),
    ]);
    let nested_output = array(vec![
        pair(
            text("a"),
            array(vec![
                pair(Json::Int(-2), Json::Int(30)),
                pair(Json::Int(10), Json::Int(40)),
            ]),
        ),
        pair(
            text("b"),
            array(vec![
                pair(Json::Int(-10), Json::Int(10)),
                pair(Json::Int(2), Json::Int(20)),
            ]),
        ),
    ]);
    assert_eq!(
        call(&service, id_of(&ids, "nested"), vec![nested_input]),
        ServerMessage::Value {
            data: nested_output
        },
        "nested/outer-and-inner-reversed"
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

/// A minted identity ledger for the `Id(^assets)` parameter program below.
const ASSET_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . b3af9f5a22196ee6b8c9fce9e79ae9ca\n\
     id product Asset 92a979ffbe70cf2e7137331e49b30820\n\
     id field Asset.name b70ebba5779562a8337a1108d8a9b185\n\
     id field Asset.tag 68d9361920d704294e3ca3b0ccc944dc\n\
     id root assets 17e4e04d857b5cfb4bb1cab6e3ad9f8f\n\
     id key assets.id 13b92ecafc9d29ad2fe21d3d9a42505c\n\
     id index assets.byTag 46bada0700e76dde99799c210ce4441e\n\
     high-water 0\n\
     end\n";

/// A storeless export taking an `Id(^assets)` parameter, so the identity codec's
/// decode half is exercised through the served interface (the argument decodes even
/// though the body reads no durable data).
const ID_PARAM: &str = r#"resource Asset {
    required tag: string
    required name: string
}

store ^assets[id: int]: Asset {
    index byTag[tag] unique
}

pub fn keyId(who: Id(^assets)): bool {
    return true
}
"#;

#[test]
fn an_identity_argument_round_trips_and_hostiles_are_rejected() {
    let (service, ids) = build(ID_PARAM, Some(ASSET_IDS.as_bytes()));
    // A single-column identity key tuple decodes onto the `Id(^assets)` parameter.
    assert_eq!(
        call(
            &service,
            id_of(&ids, "keyId"),
            vec![array(vec![Json::Int(1)])],
        ),
        ServerMessage::Value {
            data: Json::Bool(true)
        }
    );
    let reject = ServerMessage::Reject {
        code: "runner.arg_mismatch".to_string(),
    };
    // Wrong arity: two keys for a single-column root.
    assert_eq!(
        call(
            &service,
            id_of(&ids, "keyId"),
            vec![array(vec![Json::Int(1), Json::Int(2)])],
        ),
        reject
    );
    // Wrong key scalar type: a string where the key column is int.
    assert_eq!(
        call(
            &service,
            id_of(&ids, "keyId"),
            vec![array(vec![Json::Str("x".to_string())])],
        ),
        reject
    );
    // A non-array identity.
    assert_eq!(
        call(&service, id_of(&ids, "keyId"), vec![Json::Int(1)]),
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
