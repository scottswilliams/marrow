//! `marrow serve`: a small data-IPC server.
//!
//! It owns one project store (opened read-only) and answers newline-delimited
//! JSON requests over a loopback TCP connection. The request/response shape lives
//! in [`protocol`]; this module is the transport — argument parsing, the accept
//! loop, and per-connection framing. It is distinct from `marrow lsp` (the editor
//! language server, which speaks `Content-Length`-framed JSON-RPC over stdio).
//!
//! Loopback TCP is the transport because it is the only dependency-free,
//! cross-platform socket in `std`; Unix sockets and Windows named pipes would each
//! add a dependency. The listener binds `127.0.0.1` only — exposing it beyond
//! loopback would require authentication and transport security.

mod protocol;

use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::net::TcpListener;
use std::process::ExitCode;
use std::time::Duration;

use marrow_check::CheckedProgram;
use marrow_store::StoreError;
use marrow_store::tree::TreeStore;

use crate::{load_checked_project, open_store_for_inspection};

#[cfg(test)]
pub(crate) mod test_support {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use marrow_check::{
        CheckedProgram, CheckedSavedMember, CheckedSavedPlace, checked_saved_root_place,
    };
    use marrow_store::cell::CatalogId;
    use marrow_store::key::SavedKey;
    use marrow_store::tree::{DataPathSegment, TreeStore};

    pub(crate) struct ServeState {
        pub(crate) program: CheckedProgram,
        pub(crate) store: TreeStore,
    }

    const CONFIG: &str = r#"{ "sourceRoots": ["src"] }"#;
    const SOURCE: &str = "module app\n\n\
                         resource Book at ^books(id: int)\n\
                         \x20\x20\x20\x20title: string\n\
                         \x20\x20\x20\x20tags(pos: int): string\n";

    pub(crate) fn empty_state() -> ServeState {
        ServeState {
            program: checked_program(),
            store: TreeStore::memory(),
        }
    }

    pub(crate) fn state_with_books(books: &[(i64, &str)]) -> ServeState {
        let state = empty_state();
        for (id, title) in books {
            write_book(&state, *id, title);
        }
        state
    }

    /// Write one `^books(id).title` record into an existing state's store, for a
    /// test that commits data into a store a connection is already serving.
    pub(crate) fn write_book(state: &ServeState, id: i64, title: &str) {
        let place = books_place(&state.program);
        let store_id = catalog_id(&place.store_catalog_id);
        let title_path = field_path(&place, "title");
        state
            .store
            .write_data_value(
                &store_id,
                &[SavedKey::Int(id)],
                &title_path,
                title.as_bytes().to_vec(),
            )
            .expect("write checked tree-cell fixture data");
    }

    fn checked_program() -> CheckedProgram {
        let root = temp_dir("serve-checked-fixture");
        write(&root, "marrow.json", CONFIG);
        write(&root, "src/app.mw", SOURCE);
        let config = marrow_project::parse_config(CONFIG).expect("parse fixture config");
        let (report, program) = marrow_check::check_project(&root, &config).expect("check fixture");
        assert!(
            !report.has_errors(),
            "serve fixture project must check cleanly: {report:#?}"
        );
        let accepted = marrow_check::commit_pending_identity(&root, &config, &program)
            .expect("commit fixture catalog");
        fs::remove_dir_all(&root).ok();
        match accepted {
            Some((report, program)) => {
                assert!(
                    !report.has_errors(),
                    "accepted serve fixture catalog must check cleanly: {:#?}",
                    report.diagnostics
                );
                program
            }
            None => program,
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

        let suffix = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "marrow-{name}-{}-{nanos}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create fixture dir");
        root
    }

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture dirs");
        fs::write(path, contents).expect("write fixture file");
    }

    fn books_place(program: &CheckedProgram) -> CheckedSavedPlace {
        checked_saved_root_place(program, "books", marrow_syntax::SourceSpan::default())
            .expect("checked books root")
    }

    pub(crate) fn books_store_id(program: &CheckedProgram) -> CatalogId {
        catalog_id(&books_place(program).store_catalog_id)
    }

    fn catalog_id(raw: &str) -> CatalogId {
        CatalogId::new(raw.to_string()).expect("catalog id")
    }

    fn member_catalog_id(members: &[CheckedSavedMember], name: &str) -> CatalogId {
        let member = members
            .iter()
            .find(|member| member.name == name)
            .expect("checked member");
        catalog_id(&member.catalog_id)
    }

    fn field_path(place: &CheckedSavedPlace, name: &str) -> Vec<DataPathSegment> {
        vec![DataPathSegment::Member(member_catalog_id(
            &place.root_members,
            name,
        ))]
    }
}

const HELP: &str = "\
Usage:
  marrow serve [--port <port>] <projectdir>

Run the Marrow data server: a long-lived owner of the project's saved data that
answers newline-delimited JSON requests over a loopback TCP connection. It is a
read-only tooling surface and never writes managed data. The bound address is
printed on startup; `--port 0` (the default) lets the OS choose a free port.
";

/// The largest request line accepted, so a client that never sends a newline
/// cannot force an unbounded allocation.
const MAX_REQUEST_BYTES: u64 = 64 * 1024 * 1024;

/// Per-connection read timeout. The server is single-threaded and accepts
/// connections one at a time, so a client that connects and then stalls would
/// otherwise wedge the server for every other client. A stalled read past this
/// bound closes that connection and the accept loop moves on.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

pub fn run(args: &[String]) -> ExitCode {
    let mut port: u16 = 0;
    let mut dir: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--help" | "-h" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "--port" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    eprintln!("--port requires a value");
                    return ExitCode::from(2);
                };
                match value.parse() {
                    Ok(value) => port = value,
                    Err(_) => {
                        eprintln!("invalid --port: {value}");
                        return ExitCode::from(2);
                    }
                }
            }
            value if value.starts_with('-') => {
                eprintln!("unknown serve option: {value}");
                return ExitCode::from(2);
            }
            value => {
                if dir.replace(value.to_string()).is_some() {
                    eprintln!("marrow serve accepts one project directory");
                    return ExitCode::from(2);
                }
            }
        }
        index += 1;
    }
    let Some(dir) = dir else {
        eprintln!("missing project directory");
        return ExitCode::from(2);
    };

    let (config, program) = match load_checked_project(&dir) {
        Ok(checked) => checked,
        Err(code) => return code,
    };
    // A project with no saved data yet serves an empty store; inspection never
    // creates the backing file.
    let store = match open_store_for_inspection(&dir, &config) {
        Ok(Some(store)) => store,
        Ok(None) => TreeStore::memory(),
        Err(code) => return code,
    };

    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("could not bind 127.0.0.1:{port}: {error}");
            return ExitCode::FAILURE;
        }
    };
    match listener.local_addr() {
        // Print and flush the address before blocking on accept, so a client (or a
        // test using `--port 0`) can discover the chosen port.
        Ok(address) => {
            println!("marrow serve listening on {address}");
            let _ = io::stdout().flush();
        }
        Err(error) => {
            eprintln!("could not read the listen address: {error}");
            return ExitCode::FAILURE;
        }
    }

    match serve(&listener, &program, &store) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("serve error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Accept connections one at a time and serve each to completion. A single
/// connection's I/O error ends that connection, not the server. Each connection
/// carries a [`READ_TIMEOUT`] so a stalled client cannot wedge the accept loop.
fn serve(listener: &TcpListener, program: &CheckedProgram, store: &TreeStore) -> io::Result<()> {
    for stream in listener.incoming() {
        let stream = stream?;
        if let Err(error) = stream.set_read_timeout(Some(READ_TIMEOUT)) {
            eprintln!("connection error: {error}");
            continue;
        }
        let mut reader = BufReader::new(&stream);
        let mut writer = BufWriter::new(&stream);
        if let Err(error) = serve_connection(&mut reader, &mut writer, program, store) {
            eprintln!("connection error: {error}");
        }
    }
    Ok(())
}

/// The outcome of reading one request line. A `Bad` line (non-UTF-8 or over the
/// size limit) is recoverable — it earns a `protocol.malformed` reply and the
/// connection continues — whereas a genuine socket failure stays an `io::Error`
/// and ends the connection.
enum Line {
    /// A request line, with its trailing newline (if any) included.
    Request(String),
    /// A malformed line the client should be told about, with a reason.
    Bad(String),
    /// A clean EOF: the client hung up.
    Eof,
}

/// Serve one connection: read newline-delimited request lines, reply to each with
/// a newline-delimited JSON object, until the client hangs up (clean EOF).
///
/// The whole connection reads one pinned store snapshot, so every `debug_data_*`
/// read it answers observes one coherent version of saved data even while another
/// process commits. The snapshot also fixes the catalog epoch the connection sees:
/// if the store has evolved past the schema this serve binary was checked against,
/// every data op replies `protocol.stale_epoch` rather than rendering evolved data
/// under the stale schema.
fn serve_connection(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    program: &CheckedProgram,
    store: &TreeStore,
) -> io::Result<()> {
    let snapshot = match store.read_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            write_reply(writer, &snapshot_error_reply(&error.to_string()))?;
            return Ok(());
        }
    };
    let stale = match store_is_stale(program, store) {
        Ok(stale) => stale,
        Err(error) => {
            write_reply(writer, &snapshot_error_reply(&error.to_string()))?;
            return Ok(());
        }
    };
    let session = protocol::ProtocolSession::new(stale);
    loop {
        let line = match read_line_bounded(reader)? {
            Line::Eof => break,
            Line::Request(line) => line,
            Line::Bad(reason) => {
                // A non-UTF-8 or oversized line is not JSON, so it earns the same
                // structured reply as malformed JSON; the connection stays open.
                write_reply(writer, &malformed_reply(&reason))?;
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str(&line) {
            Ok(request) => session.handle_request(program, store, &request),
            Err(error) => malformed_reply(&error.to_string()),
        };
        write_reply(writer, &reply)?;
    }
    drop(snapshot);
    Ok(())
}

/// Whether the store has evolved past the schema this serve binary was checked
/// against: its stamped catalog epoch is newer than the program's accepted epoch.
/// A matching, older, or absent stamp serves normally.
fn store_is_stale(program: &CheckedProgram, store: &TreeStore) -> Result<bool, StoreError> {
    let Some(stored) = store.read_catalog_epoch()? else {
        return Ok(false);
    };
    Ok(match program.catalog.accepted_epoch {
        Some(accepted) => stored > accepted,
        None => true,
    })
}

/// A `protocol.malformed` reply envelope with a null id (the request could not be
/// parsed, so its id is unknown).
fn malformed_reply(message: &str) -> serde_json::Value {
    serde_json::json!({
        "id": serde_json::Value::Null,
        "error": { "code": protocol::PROTOCOL_MALFORMED, "message": message },
    })
}

/// A `store.*` reply envelope with a null id, sent when the connection cannot pin
/// a coherent read snapshot. The connection then ends rather than answering reads
/// against a snapshot it never acquired.
fn snapshot_error_reply(message: &str) -> serde_json::Value {
    serde_json::json!({
        "id": serde_json::Value::Null,
        "error": { "code": "store.io", "message": message },
    })
}

/// Write one newline-delimited JSON reply and flush it.
fn write_reply(writer: &mut impl Write, reply: &serde_json::Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(reply).expect("a reply serializes");
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}

/// Read one newline-terminated request line, bounded by [`MAX_REQUEST_BYTES`].
/// Returns [`Line::Eof`] on a clean hang-up and [`Line::Bad`] for a non-UTF-8 or
/// oversized line; a genuine socket failure is an `io::Error`.
fn read_line_bounded(reader: &mut impl BufRead) -> io::Result<Line> {
    let mut buf = Vec::new();
    let reader: &mut dyn BufRead = reader;
    let mut limited = reader.take(MAX_REQUEST_BYTES);
    let read = match limited.read_until(b'\n', &mut buf) {
        Ok(read) => read,
        // A stalled client hits the per-connection read timeout; close the
        // connection cleanly (like a hang-up) rather than reporting an error.
        Err(error) if is_timeout(error.kind()) => return Ok(Line::Eof),
        Err(error) => return Err(error),
    };
    if read == 0 {
        return Ok(Line::Eof);
    }
    if read as u64 == MAX_REQUEST_BYTES && !buf.ends_with(b"\n") {
        return Ok(Line::Bad("request line exceeds the size limit".to_string()));
    }
    match String::from_utf8(buf) {
        Ok(line) => Ok(Line::Request(line)),
        Err(_) => Ok(Line::Bad("request line is not valid UTF-8".to_string())),
    }
}

/// Whether a read error is a read-timeout expiry, which the platform reports as
/// `WouldBlock` (Unix) or `TimedOut` (Windows).
fn is_timeout(kind: io::ErrorKind) -> bool {
    matches!(kind, io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    use marrow_store::key::SavedKey;

    use crate::serve::test_support::{self, empty_state, state_with_books, write_book};

    #[test]
    fn serves_newline_delimited_requests_over_a_stream() {
        let state = state_with_books(&[(1, "Mort")]);

        let input = "{\"id\":1,\"op\":\"debug_data_roots\"}\n\n{\"id\":2,\"op\":\"nope\"}\n";
        let mut reader = Cursor::new(input.as_bytes());
        let mut output: Vec<u8> = Vec::new();
        serve_connection(&mut reader, &mut output, &state.program, &state.store).expect("serve");

        let replies: Vec<serde_json::Value> = String::from_utf8(output)
            .expect("utf8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("reply json"))
            .collect();
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0]["id"], serde_json::json!(1));
        assert_eq!(replies[0]["ok"]["roots"], serde_json::json!(["books"]));
        assert_eq!(replies[1]["id"], serde_json::json!(2));
        assert_eq!(
            replies[1]["error"]["code"],
            serde_json::json!(protocol::PROTOCOL_UNKNOWN_OP)
        );
    }

    /// One scripted request line, with an optional side effect (a store write that
    /// commits mid-connection) run just before the line is handed over.
    type ScriptedLine<'a> = (String, Option<Box<dyn FnOnce() + 'a>>);

    /// A reader that yields scripted request lines, running each line's side effect
    /// just before handing the line over. It lets a test prove the connection's
    /// pinned snapshot hides a concurrent commit: the write lands between the two
    /// reads, yet the second read does not observe it.
    struct ScriptedReader<'a> {
        lines: std::collections::VecDeque<ScriptedLine<'a>>,
        buffer: Vec<u8>,
        position: usize,
    }

    impl<'a> ScriptedReader<'a> {
        fn new(lines: Vec<ScriptedLine<'a>>) -> Self {
            Self {
                lines: lines.into(),
                buffer: Vec::new(),
                position: 0,
            }
        }

        fn refill(&mut self) {
            if self.position < self.buffer.len() {
                return;
            }
            if let Some((line, effect)) = self.lines.pop_front() {
                if let Some(effect) = effect {
                    effect();
                }
                self.buffer = line.into_bytes();
                self.position = 0;
            }
        }
    }

    impl Read for ScriptedReader<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let available = self.fill_buf()?;
            let count = available.len().min(buf.len());
            buf[..count].copy_from_slice(&available[..count]);
            self.consume(count);
            Ok(count)
        }
    }

    impl BufRead for ScriptedReader<'_> {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            self.refill();
            Ok(&self.buffer[self.position..])
        }

        fn consume(&mut self, amount: usize) {
            self.position += amount;
        }
    }

    #[test]
    fn a_connection_pins_one_snapshot_so_a_mid_connection_write_is_invisible() {
        let state = state_with_books(&[(1, "Mort")]);
        let children_request =
            "{\"id\":2,\"op\":\"debug_data_children\",\"path\":[{\"root\":\"books\"}]}\n";
        let mut reader = ScriptedReader::new(vec![
            ("{\"id\":1,\"op\":\"debug_data_roots\"}\n".to_string(), None),
            (
                children_request.to_string(),
                Some(Box::new(|| write_book(&state, 2, "Sourcery"))),
            ),
        ]);
        let mut output: Vec<u8> = Vec::new();
        serve_connection(&mut reader, &mut output, &state.program, &state.store).expect("serve");

        let replies: Vec<serde_json::Value> = String::from_utf8(output)
            .expect("utf8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("reply json"))
            .collect();
        assert_eq!(replies.len(), 2, "{replies:?}");
        // The mid-connection write of `^books(2)` committed between the two reads,
        // but the connection's pinned snapshot hides it: only `^books(1)` is seen.
        assert_eq!(
            replies[1]["ok"]["children"],
            serde_json::json!([{ "key": { "int": 1 } }]),
            "the second read must not observe the concurrent write: {replies:?}"
        );
        // The write did land in the store; a fresh connection would see it.
        let mut keys = Vec::new();
        let store_id = test_support::books_store_id(&state.program);
        let mut child = state
            .store
            .record_first_child(&store_id, &[])
            .expect("record child");
        while let Some(key) = child {
            let anchor = key.clone();
            keys.push(key);
            child = state
                .store
                .record_next_child(&store_id, &[], &anchor)
                .expect("record child");
        }
        assert_eq!(
            keys,
            vec![SavedKey::Int(1), SavedKey::Int(2)],
            "the concurrent write is durable in the store, just not in the snapshot"
        );
    }

    #[test]
    fn a_store_evolved_past_the_checked_schema_refuses_data_ops_with_stale_epoch() {
        let state = state_with_books(&[(1, "Mort")]);
        // Stamp the store's catalog epoch far past the schema this serve binary was
        // checked against, standing in for a store another process has evolved.
        state
            .store
            .write_catalog_epoch(u64::MAX)
            .expect("stamp a newer catalog epoch");
        let input = "{\"id\":1,\"op\":\"debug_data_roots\"}\n";
        let mut reader = Cursor::new(input.as_bytes());
        let mut output: Vec<u8> = Vec::new();
        serve_connection(&mut reader, &mut output, &state.program, &state.store).expect("serve");

        let reply: serde_json::Value =
            serde_json::from_str(String::from_utf8(output).expect("utf8").trim()).expect("reply");
        assert_eq!(
            reply["error"]["code"],
            serde_json::json!(protocol::PROTOCOL_STALE_EPOCH),
            "an evolved store must refuse data ops: {reply}"
        );
    }

    /// A reader that always reports a read-timeout (`WouldBlock`), standing in for
    /// a stalled client whose per-connection read timeout has expired.
    struct TimingOutReader;

    impl Read for TimingOutReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::WouldBlock, "timed out"))
        }
    }

    impl BufRead for TimingOutReader {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            Err(io::Error::new(io::ErrorKind::WouldBlock, "timed out"))
        }
        fn consume(&mut self, _amount: usize) {}
    }

    #[test]
    fn a_read_timeout_closes_the_connection_cleanly() {
        // A stalled read (the per-connection READ_TIMEOUT firing) ends the
        // connection like a clean hang-up, so serve() moves on to the next client
        // instead of the read propagating as a connection error.
        assert!(matches!(
            read_line_bounded(&mut TimingOutReader).expect("a timeout is not an error"),
            Line::Eof
        ));
        let state = empty_state();
        let mut output: Vec<u8> = Vec::new();
        serve_connection(
            &mut TimingOutReader,
            &mut output,
            &state.program,
            &state.store,
        )
        .expect("serve returns cleanly");
        assert!(output.is_empty(), "a timed-out connection sends no reply");
    }

    #[test]
    fn a_non_utf8_line_gets_a_malformed_reply_and_the_connection_stays_open() {
        let state = empty_state();
        // A non-UTF-8 byte sequence on the first line (0xff is never valid UTF-8),
        // then a well-formed request on the second.
        let mut input: Vec<u8> = b"\xff\xfe\n".to_vec();
        input.extend_from_slice(b"{\"id\":2,\"op\":\"debug_data_roots\"}\n");
        let mut reader = Cursor::new(input);
        let mut output: Vec<u8> = Vec::new();
        serve_connection(&mut reader, &mut output, &state.program, &state.store).expect("serve");

        let replies: Vec<serde_json::Value> = String::from_utf8(output)
            .expect("utf8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("reply json"))
            .collect();
        // The bad line is answered with protocol.malformed, and the connection
        // continued to serve the following valid request.
        assert_eq!(replies.len(), 2, "{replies:?}");
        assert_eq!(
            replies[0]["error"]["code"],
            serde_json::json!(protocol::PROTOCOL_MALFORMED)
        );
        assert_eq!(replies[1]["id"], serde_json::json!(2));
        assert_eq!(replies[1]["ok"]["roots"], serde_json::json!([]));
    }
}
