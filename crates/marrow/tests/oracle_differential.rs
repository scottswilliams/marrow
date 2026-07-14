//! Differential-oracle harness skeleton (lane B00).
//!
//! The A01 differential oracle is the frozen prototype binary, archived out of
//! tree with its SHA-256 and launcher contract in
//! `~/agents-work/marrow-oracle/CUSTODY.md`. This harness verifies the binary by
//! hash, launches it under the custody launcher contract (isolated HOME/TMPDIR/
//! cwd, minimal environment, wall-clock and output caps), runs `marrow test` on
//! one preserved-semantics fixture project, and parses the typed JSONL it emits
//! to prove the invocation pipeline works end to end.
//!
//! This is the skeleton only: the beta-side stack and the field-by-field
//! comparison (outcome/code/span/data) arrive at T01. The oracle-invoking test
//! is `#[ignore]` so the default gate never requires the archived binary; the
//! JSONL parser is exercised by a non-ignored unit test over a captured sample.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Custody constants (mirror ~/agents-work/marrow-oracle/CUSTODY.md).
// ---------------------------------------------------------------------------

const ORACLE_DIR_ENV: &str = "MARROW_ORACLE_DIR";
const DEFAULT_ORACLE_DIR: &str = "agents-work/marrow-oracle";
const ORACLE_BINARY: &str = "marrow-prototype-20453117";
const SHA256SUMS: &str = "SHA256SUMS";
const INVOCATION_TIMEOUT: Duration = Duration::from_secs(60);
const OUTPUT_CAP_BYTES: usize = 8 * 1024 * 1024;

/// The single preserved-semantics IN fixture the skeleton smoke-tests.
const SMOKE_FIXTURE: &str = "fixtures/v01/conformance/enum_semantics";

fn oracle_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(ORACLE_DIR_ENV) {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(DEFAULT_ORACLE_DIR)
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/marrow`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root two levels above the crate manifest")
        .to_path_buf()
}

// ---------------------------------------------------------------------------
// The oracle-invoking smoke test (ignored by default: needs the archived binary).
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires the archived A01 oracle binary; run with --ignored once it is present"]
fn oracle_runs_one_preserved_semantics_fixture() {
    let dir = oracle_dir();
    let binary = dir.join(ORACLE_BINARY);
    assert!(
        binary.exists(),
        "oracle binary not found at {}; set {ORACLE_DIR_ENV} to its directory",
        binary.display()
    );

    verify_oracle_hash(&dir, &binary);

    let scratch = ScratchDir::new().expect("create scratch dir");
    let project = scratch.path().join("project");
    copy_tree(&workspace_root().join(SMOKE_FIXTURE), &project).expect("copy fixture project");

    let output = launch_oracle(
        &binary,
        &scratch,
        &["test", project_str(&project), "--format", "jsonl"],
    );

    let records = parse_jsonl(&output);
    assert!(
        !records.is_empty(),
        "oracle produced no JSONL records; pipeline broken"
    );
    let summary = records
        .iter()
        .find(|record| record.get_str("kind") == Some("summary"))
        .expect("oracle test output ends with a summary record");
    // The pipeline works when the frozen oracle checks, runs, and reports the
    // fixture's tests as a typed selected/total count. The field-by-field
    // beta-vs-oracle comparison is deferred to T01.
    assert!(
        summary.get_num("selected").is_some_and(|n| n > 0.0),
        "summary selected count should be positive: {summary:?}"
    );
    assert_eq!(
        summary.get_num("failed"),
        Some(0.0),
        "preserved-semantics fixture must pass on the oracle: {summary:?}"
    );
}

/// Verify the archived binary against the `SHA256SUMS` custody record before any
/// invocation. A mismatch is a hard failure: the harness never launches an
/// unverified binary.
fn verify_oracle_hash(dir: &Path, binary: &Path) {
    let sums = std::fs::read_to_string(dir.join(SHA256SUMS)).expect("read SHA256SUMS");
    let expected = sums
        .lines()
        .find_map(|line| {
            let (hash, name) = line.split_once("  ")?;
            (name.trim() == ORACLE_BINARY).then(|| hash.trim().to_string())
        })
        .expect("SHA256SUMS names the oracle binary");
    let bytes = std::fs::read(binary).expect("read oracle binary");
    let actual = sha256_hex(&bytes);
    assert_eq!(
        actual, expected,
        "oracle binary hash mismatch: refusing to launch an unverified binary"
    );
}

/// Launch the oracle under the custody launcher contract and return its combined
/// stdout+stderr, truncated at the output cap. Fails on a non-empty exit that is
/// not the fixture's own reporting, on timeout, or on an over-cap flood.
fn launch_oracle(binary: &Path, scratch: &ScratchDir, args: &[&str]) -> String {
    let mut child = Command::new(binary)
        .args(args)
        // Minimal environment, isolated HOME/TMPDIR/cwd per the custody contract.
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", scratch.home())
        .env("TMPDIR", scratch.tmp())
        .current_dir(scratch.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn oracle");

    // Read the piped streams on threads so a flood cannot deadlock the pipe while
    // we wait on the deadline.
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let out_handle = std::thread::spawn(move || read_capped(&mut stdout));
    let err_handle = std::thread::spawn(move || read_capped(&mut stderr));

    let deadline = Instant::now() + INVOCATION_TIMEOUT;
    loop {
        match child.try_wait().expect("poll oracle") {
            Some(_status) => break,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!("oracle invocation exceeded {INVOCATION_TIMEOUT:?}");
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }

    let mut combined = out_handle.join().expect("join stdout reader");
    combined.push_str(&err_handle.join().expect("join stderr reader"));
    assert!(
        combined.len() <= OUTPUT_CAP_BYTES,
        "oracle output exceeded the {OUTPUT_CAP_BYTES}-byte cap"
    );
    combined
}

fn read_capped(reader: &mut impl std::io::Read) -> String {
    use std::io::Read;
    let mut buf = Vec::new();
    let _ = reader
        .take((OUTPUT_CAP_BYTES + 1) as u64)
        .read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

fn project_str(path: &Path) -> &str {
    path.to_str().expect("fixture path is valid UTF-8")
}

// ---------------------------------------------------------------------------
// A per-invocation scratch directory with isolated HOME and TMPDIR.
// ---------------------------------------------------------------------------

struct ScratchDir {
    root: PathBuf,
}

impl ScratchDir {
    fn new() -> std::io::Result<Self> {
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let root = base.join(format!(
            "marrow-oracle-harness-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("home"))?;
        std::fs::create_dir_all(root.join("tmp"))?;
        Ok(Self { root })
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Minimal JSONL parser: one flat JSON object per line, values are string,
// number, bool, or a one-level nested object (the `span`/`data` fields). Only
// the typed fields the differential compares are read back. Dependency-free so
// the CLI keeps no serde stack.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Json {
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
    Obj(BTreeMap<String, Json>),
}

#[derive(Debug, Clone, PartialEq)]
struct Record(BTreeMap<String, Json>);

impl Record {
    fn get_str(&self, key: &str) -> Option<&str> {
        match self.0.get(key) {
            Some(Json::Str(s)) => Some(s),
            _ => None,
        }
    }

    fn get_num(&self, key: &str) -> Option<f64> {
        match self.0.get(key) {
            Some(Json::Num(n)) => Some(*n),
            _ => None,
        }
    }
}

fn parse_jsonl(text: &str) -> Vec<Record> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| match parse_value(&mut Cursor::new(line)) {
            Some(Json::Obj(map)) => Record(map),
            other => panic!("JSONL line is not an object: {line}\n(parsed {other:?})"),
        })
        .collect()
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.pos += 1;
        Some(byte)
    }

    fn expect(&mut self, byte: u8) {
        assert_eq!(self.bump(), Some(byte), "expected {}", byte as char);
    }
}

fn parse_value(cursor: &mut Cursor) -> Option<Json> {
    cursor.skip_ws();
    match cursor.peek()? {
        b'{' => parse_object(cursor),
        b'"' => Some(Json::Str(parse_string(cursor))),
        b't' | b'f' => parse_bool(cursor),
        b'n' => parse_null(cursor),
        _ => parse_number(cursor),
    }
}

fn parse_object(cursor: &mut Cursor) -> Option<Json> {
    cursor.expect(b'{');
    let mut map = BTreeMap::new();
    cursor.skip_ws();
    if cursor.peek() == Some(b'}') {
        cursor.bump();
        return Some(Json::Obj(map));
    }
    loop {
        cursor.skip_ws();
        let key = parse_string(cursor);
        cursor.skip_ws();
        cursor.expect(b':');
        let value = parse_value(cursor)?;
        map.insert(key, value);
        cursor.skip_ws();
        match cursor.bump() {
            Some(b',') => continue,
            Some(b'}') => break,
            other => panic!("malformed object near {other:?}"),
        }
    }
    Some(Json::Obj(map))
}

fn parse_string(cursor: &mut Cursor) -> String {
    cursor.expect(b'"');
    let mut out = String::new();
    while let Some(byte) = cursor.bump() {
        match byte {
            b'"' => return out,
            b'\\' => match cursor.bump() {
                Some(b'"') => out.push('"'),
                Some(b'\\') => out.push('\\'),
                Some(b'/') => out.push('/'),
                Some(b'n') => out.push('\n'),
                Some(b't') => out.push('\t'),
                Some(b'r') => out.push('\r'),
                Some(other) => out.push(other as char),
                None => break,
            },
            other => out.push(other as char),
        }
    }
    panic!("unterminated string");
}

fn parse_number(cursor: &mut Cursor) -> Option<Json> {
    let start = cursor.pos;
    while let Some(byte) = cursor.peek() {
        if byte.is_ascii_digit() || matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E') {
            cursor.pos += 1;
        } else {
            break;
        }
    }
    let text = std::str::from_utf8(&cursor.bytes[start..cursor.pos]).ok()?;
    text.parse().ok().map(Json::Num)
}

fn parse_bool(cursor: &mut Cursor) -> Option<Json> {
    if cursor.bytes[cursor.pos..].starts_with(b"true") {
        cursor.pos += 4;
        Some(Json::Bool(true))
    } else if cursor.bytes[cursor.pos..].starts_with(b"false") {
        cursor.pos += 5;
        Some(Json::Bool(false))
    } else {
        None
    }
}

fn parse_null(cursor: &mut Cursor) -> Option<Json> {
    if cursor.bytes[cursor.pos..].starts_with(b"null") {
        cursor.pos += 4;
        Some(Json::Null)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Compact SHA-256 (FIPS 180-4), test-only so the CLI keeps no crypto dependency.
// ---------------------------------------------------------------------------

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finish();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        hex.push(char::from_digit((byte & 0xf) as u32, 16).unwrap());
    }
    hex
}

struct Sha256 {
    state: [u32; 8],
    buffer: Vec<u8>,
    length: u64,
}

impl Sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: Vec::with_capacity(64),
            length: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.length += data.len() as u64;
        self.buffer.extend_from_slice(data);
        while self.buffer.len() >= 64 {
            let block: [u8; 64] = self.buffer[..64].try_into().unwrap();
            self.compress(&block);
            self.buffer.drain(..64);
        }
    }

    fn finish(mut self) -> [u8; 32] {
        let bit_len = self.length.wrapping_mul(8);
        self.buffer.push(0x80);
        while self.buffer.len() % 64 != 56 {
            self.buffer.push(0);
        }
        self.buffer.extend_from_slice(&bit_len.to_be_bytes());
        let buffer = std::mem::take(&mut self.buffer);
        for block in buffer.chunks_exact(64) {
            self.compress(block.try_into().unwrap());
        }
        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut h = self.state;
        for (&k, &word) in Self::K.iter().zip(&w) {
            let s1 = h[4].rotate_right(6) ^ h[4].rotate_right(11) ^ h[4].rotate_right(25);
            let ch = (h[4] & h[5]) ^ ((!h[4]) & h[6]);
            let t1 = h[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(k)
                .wrapping_add(word);
            let s0 = h[0].rotate_right(2) ^ h[0].rotate_right(13) ^ h[0].rotate_right(22);
            let maj = (h[0] & h[1]) ^ (h[0] & h[2]) ^ (h[1] & h[2]);
            let t2 = s0.wrapping_add(maj);
            h[7] = h[6];
            h[6] = h[5];
            h[5] = h[4];
            h[4] = h[3].wrapping_add(t1);
            h[3] = h[2];
            h[2] = h[1];
            h[1] = h[0];
            h[0] = t1.wrapping_add(t2);
        }
        for (state, value) in self.state.iter_mut().zip(h) {
            *state = state.wrapping_add(value);
        }
    }
}

// ---------------------------------------------------------------------------
// Non-ignored unit tests over captured samples: the JSONL parser and the hasher
// are exercised by the default gate without the archived binary.
// ---------------------------------------------------------------------------

/// A captured sample of the oracle's `marrow test --format jsonl` output over the
/// `enum_semantics` fixture (two test records plus the summary), so the parser is
/// gated on real oracle output without invoking the binary.
const SAMPLE_JSONL: &str = concat!(
    r#"{"file":"/p/tests/t.mw","kind":"test","name":"tests::t::a","outcome":"passed","span":{"column":1,"line":10}}"#,
    "\n",
    r#"{"file":"/p/tests/t.mw","kind":"test","name":"tests::t::b","outcome":"passed","span":{"column":1,"line":13}}"#,
    "\n",
    r#"{"errored":0,"failed":0,"kind":"summary","passed":2,"selected":2,"total":2}"#,
);

#[test]
fn parses_captured_oracle_jsonl_into_typed_records() {
    let records = parse_jsonl(SAMPLE_JSONL);
    assert_eq!(records.len(), 3);

    let tests: Vec<&Record> = records
        .iter()
        .filter(|record| record.get_str("kind") == Some("test"))
        .collect();
    assert_eq!(tests.len(), 2);
    assert_eq!(tests[0].get_str("outcome"), Some("passed"));
    assert_eq!(tests[0].get_str("name"), Some("tests::t::a"));

    // The nested span object is parsed one level deep.
    match &tests[0].0["span"] {
        Json::Obj(span) => {
            assert_eq!(span.get("line"), Some(&Json::Num(10.0)));
            assert_eq!(span.get("column"), Some(&Json::Num(1.0)));
        }
        other => panic!("span is not an object: {other:?}"),
    }

    let summary = records
        .iter()
        .find(|record| record.get_str("kind") == Some("summary"))
        .expect("sample ends with a summary record");
    assert_eq!(summary.get_num("selected"), Some(2.0));
    assert_eq!(summary.get_num("failed"), Some(0.0));
}

#[test]
fn sha256_matches_known_answer_vectors() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
