//! Shared canonical-JSON known-answer vectors, pinning the wire's byte spelling
//! for the divergence-prone shapes: string escaping, UTF-8 byte-order key
//! sorting, i64 boundaries, the `bytes` transfer string, and the temporal
//! scalar text forms.
//!
//! This crate owns the authoritative encoder (`marrow-local-wire`). The same
//! vectors are re-asserted against the non-authoritative JavaScript mirror (the
//! pinned Node supervision module) by an ignored driver in
//! `crates/marrow/tests/wire_kat.rs`, which reads the fixtures this test
//! generates. The vector table below is the single source: the committed
//! `tests/fixtures/canonical_kat.tsv` and `tests/fixtures/noncanonical_kat.tsv`
//! are its serialization, drift-gated here (rewrite with
//! `MARROW_UPDATE_WIRE_KAT=1`), so the two encoders cannot be pinned to
//! independently drifting answer keys.

use std::path::{Path, PathBuf};

use marrow_local_wire::{Json, WireError, encode, parse_strict};

/// One accepted vector: a human-specified canonical byte spelling for a value
/// the authoritative encoder must produce and the parser must accept.
struct Vector {
    /// A stable ASCII key, shared with the Node driver to pair the two sides.
    key: &'static str,
    value: Json,
    /// The exact canonical bytes (as UTF-8 text; a canonical value never
    /// contains a raw newline or tab, so it is one physical fixture field).
    canonical: &'static str,
}

/// The coarse rejection family both encoders must agree on. The finer
/// `WireError` variants (`StringLimit`, `DepthLimit`) are exercised by the
/// crate's own limit tests; the cross-encoder KAT pins only the two families a
/// hostile-but-bounded body falls into.
#[derive(Clone, Copy)]
enum RejectClass {
    Noncanonical,
    Malformed,
}

impl RejectClass {
    /// The wire-owner error this class maps to.
    fn wire_error(self) -> WireError {
        match self {
            RejectClass::Noncanonical => WireError::Noncanonical,
            RejectClass::Malformed => WireError::Malformed,
        }
    }

    /// The stable fixture token, matched by the Node driver against the mirror's
    /// `WireFormatError.code`.
    fn token(self) -> &'static str {
        match self {
            RejectClass::Noncanonical => "noncanonical",
            RejectClass::Malformed => "malformed",
        }
    }
}

/// One reject vector: an input both the authoritative parser and the mirror must
/// reject with the same rejection family.
struct Reject {
    key: &'static str,
    input: &'static str,
    class: RejectClass,
}

fn str_value(text: &str) -> Json {
    Json::Str(text.to_string())
}

fn obj(pairs: &[(&str, Json)]) -> Json {
    Json::Object(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect(),
    )
}

/// The accepted vectors. Each `canonical` is written by hand (not produced by
/// the encoder) so the assertions are genuine known answers.
fn accepted_vectors() -> Vec<Vector> {
    vec![
        // i64 boundaries: minimal spellings at the exact edges of the range.
        Vector {
            key: "int-min",
            value: Json::Int(i64::MIN),
            canonical: "-9223372036854775808",
        },
        Vector {
            key: "int-max",
            value: Json::Int(i64::MAX),
            canonical: "9223372036854775807",
        },
        Vector {
            key: "int-zero",
            value: Json::Int(0),
            canonical: "0",
        },
        Vector {
            key: "int-neg-one",
            value: Json::Int(-1),
            canonical: "-1",
        },
        // String escaping: quote and backslash take their two-character escapes.
        Vector {
            key: "str-quote-backslash",
            value: str_value("a\"b\\c"),
            canonical: r#""a\"b\\c""#,
        },
        // Every C0 control: the five named short escapes, all others `\u00xx`
        // with lowercase hex. Covers 0x0b (no short escape) between named ones.
        Vector {
            key: "str-c0-controls",
            value: str_value("\u{00}\u{07}\u{08}\u{09}\u{0a}\u{0b}\u{0c}\u{0d}\u{1f}"),
            canonical: "\"\\u0000\\u0007\\b\\t\\n\\u000b\\f\\r\\u001f\"",
        },
        // Non-ASCII BMP text passes through literally (no `\u` escaping).
        Vector {
            key: "str-bmp",
            value: str_value("café ☕"),
            canonical: "\"café ☕\"",
        },
        // An astral char passes through as its literal 4-byte UTF-8, never as a
        // `\u` surrogate pair.
        Vector {
            key: "str-astral",
            value: str_value("😀"),
            canonical: "\"😀\"",
        },
        // Key order is ascending UTF-8 byte order, not insertion order.
        Vector {
            key: "keys-ascii",
            value: obj(&[
                ("b", Json::Int(1)),
                ("a", Json::Int(2)),
                ("A", Json::Int(3)),
                ("10", Json::Int(4)),
            ]),
            canonical: r#"{"10":4,"A":3,"a":2,"b":1}"#,
        },
        // Numeric-looking keys sort by byte, not numerically: `"1" < "10" < "2"`.
        // This distinguishes byte-order sorting from a naive numeric key order.
        Vector {
            key: "keys-numeric",
            value: obj(&[
                ("10", Json::Int(1)),
                ("2", Json::Int(2)),
                ("1", Json::Int(3)),
            ]),
            canonical: r#"{"1":3,"10":1,"2":2}"#,
        },
        // UTF-8 byte order disagrees with UTF-16 code-unit order here: U+FFFD
        // (0xEF..) sorts before an astral char (0xF0..), but in UTF-16 the astral
        // char's lead surrogate (0xD83D) sorts before 0xFFFD. Canonical form must
        // follow UTF-8 bytes.
        Vector {
            key: "keys-astral-vs-fffd",
            value: obj(&[("😀", Json::Int(1)), ("\u{fffd}", Json::Int(2))]),
            canonical: "{\"\u{fffd}\":2,\"😀\":1}",
        },
        // The `bytes` transfer string: `0x` + lowercase hex, including empty.
        Vector {
            key: "bytes-empty",
            value: str_value("0x"),
            canonical: "\"0x\"",
        },
        Vector {
            key: "bytes-high",
            value: str_value("0x00ff10"),
            canonical: "\"0x00ff10\"",
        },
        // The temporal scalars' canonical text spellings cross as plain strings.
        Vector {
            key: "date",
            value: str_value("2026-07-15"),
            canonical: "\"2026-07-15\"",
        },
        Vector {
            key: "instant",
            value: str_value("2026-07-15T17:00:00Z"),
            canonical: "\"2026-07-15T17:00:00Z\"",
        },
        Vector {
            key: "instant-frac",
            value: str_value("2026-07-15T17:00:00.123456789Z"),
            canonical: "\"2026-07-15T17:00:00.123456789Z\"",
        },
        Vector {
            key: "duration",
            value: str_value("PT3600S"),
            canonical: "\"PT3600S\"",
        },
        Vector {
            key: "duration-neg-frac",
            value: str_value("-PT1.5S"),
            canonical: "\"-PT1.5S\"",
        },
        // A record combining a hard-escaped text field, a bytes string, and a
        // temporal string, exercising escaping and key sorting together.
        Vector {
            key: "record-mixed",
            value: obj(&[
                ("note", str_value("a\"b")),
                ("blob", str_value("0xff")),
                ("when", str_value("2026-07-15")),
            ]),
            canonical: r#"{"blob":"0xff","note":"a\"b","when":"2026-07-15"}"#,
        },
    ]
}

/// Inputs both encoders must reject, with the family they must agree on.
fn reject_vectors() -> Vec<Reject> {
    vec![
        Reject {
            key: "neg-zero",
            input: "-0",
            class: RejectClass::Noncanonical,
        },
        Reject {
            key: "int-overflow-max",
            input: "9223372036854775808",
            class: RejectClass::Malformed,
        },
        Reject {
            key: "int-overflow-min",
            input: "-9223372036854775809",
            class: RejectClass::Malformed,
        },
        Reject {
            key: "unsorted-keys",
            input: r#"{"b":1,"a":2}"#,
            class: RejectClass::Noncanonical,
        },
        Reject {
            key: "uppercase-hex-escape",
            input: "\"\\u00FF\"",
            class: RejectClass::Noncanonical,
        },
        Reject {
            key: "escaped-slash",
            input: r#""\/""#,
            class: RejectClass::Noncanonical,
        },
    ]
}

/// The authoritative encoder produces exactly the hand-specified canonical bytes,
/// and the parser round-trips them back to the same value.
#[test]
fn authoritative_encoder_pins_canonical_bytes() {
    for vector in accepted_vectors() {
        assert_eq!(
            encode(&vector.value),
            vector.canonical,
            "encoding drift for {}",
            vector.key
        );
        // Parse re-encodes to the same canonical bytes. Object pairs are compared
        // by re-encoding rather than by value, since the parser returns keys in
        // canonical (sorted) order while a vector may be built unsorted to
        // exercise the encoder's sort.
        let parsed = parse_strict(vector.canonical.as_bytes())
            .unwrap_or_else(|e| panic!("{} should parse: {e:?}", vector.key));
        assert_eq!(
            encode(&parsed),
            vector.canonical,
            "round trip for {}",
            vector.key
        );
    }
}

/// Each reject input is refused with the agreed rejection family.
#[test]
fn reject_inputs_are_refused_by_family() {
    for reject in reject_vectors() {
        assert_eq!(
            parse_strict(reject.input.as_bytes()),
            Err(reject.class.wire_error()),
            "rejection family for {}",
            reject.key
        );
    }
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Serialize the accepted vectors as `key\tcanonical` lines under a header.
fn accepted_fixture() -> String {
    let mut out = String::from(
        "# Canonical-JSON known-answer vectors: key<TAB>canonical-bytes.\n\
         # The single source is crates/marrow-local-wire/tests/canonical_kat.rs.\n",
    );
    for vector in accepted_vectors() {
        out.push_str(vector.key);
        out.push('\t');
        out.push_str(vector.canonical);
        out.push('\n');
    }
    out
}

/// Serialize the reject vectors as `key\tfamily\tinput` lines under a header.
fn reject_fixture() -> String {
    let mut out = String::from(
        "# Non-canonical / malformed known-answer vectors: key<TAB>family<TAB>input.\n\
         # The single source is crates/marrow-local-wire/tests/canonical_kat.rs.\n",
    );
    for reject in reject_vectors() {
        out.push_str(reject.key);
        out.push('\t');
        out.push_str(reject.class.token());
        out.push('\t');
        out.push_str(reject.input);
        out.push('\n');
    }
    out
}

/// Assert a committed fixture byte-matches its regenerated content, or rewrite it
/// under `MARROW_UPDATE_WIRE_KAT=1` (mirroring the tracer drift gate's updater).
fn assert_single_sourced(path: &Path, generated: &str) {
    if std::env::var_os("MARROW_UPDATE_WIRE_KAT").is_some_and(|v| v == "1") {
        std::fs::write(path, generated).expect("rewrite KAT fixture");
        return;
    }
    let on_disk = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert_eq!(
        generated,
        on_disk,
        "KAT fixture drift at {}: rerun with MARROW_UPDATE_WIRE_KAT=1 after a deliberate table change",
        path.display()
    );
}

/// The committed fixtures the Node driver reads are exactly what the vector table
/// serializes, so both encoders are pinned to one answer key.
#[test]
fn kat_fixtures_are_single_sourced() {
    let dir = fixtures_dir();
    assert_single_sourced(&dir.join("canonical_kat.tsv"), &accepted_fixture());
    assert_single_sourced(&dir.join("noncanonical_kat.tsv"), &reject_fixture());
}
