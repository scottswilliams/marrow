//! `marrow serve`: a small data-IPC server.
//!
//! It owns one project store (opened read-only) and answers newline-delimited
//! JSON requests over a loopback TCP connection. The request/response shape lives
//! in [`protocol`]; this module is the transport — argument parsing, the accept
//! loop, and per-connection framing. It is distinct from `marrow lsp` (the editor
//! language server, which speaks `Content-Length`-framed JSON-RPC over stdio).
//!
//! Loopback TCP is the v1 transport: it is the only dependency-free, cross-platform
//! socket in `std` (local IPC over Unix sockets or Windows named pipes is a later,
//! dependency-bearing transport). The listener binds `127.0.0.1` only; exposing it
//! beyond loopback would require authentication and transport security.

mod protocol;

use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::net::TcpListener;
use std::process::ExitCode;

use marrow_store::backend::Backend;

use crate::{load_config, open_store_for_inspection};

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

    let config = match load_config(&dir) {
        Ok(config) => config,
        Err(code) => return code,
    };
    // A project with no saved data yet serves an empty store; inspection never
    // creates the backing file.
    let store: Box<dyn Backend> = match open_store_for_inspection(&dir, &config) {
        Ok(Some(store)) => store,
        Ok(None) => Box::new(marrow_store::mem::MemStore::new()),
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

    match serve(&listener, store.as_ref()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("serve error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Accept connections one at a time and serve each to completion. A single
/// connection's I/O error ends that connection, not the server.
fn serve(listener: &TcpListener, store: &dyn Backend) -> io::Result<()> {
    for stream in listener.incoming() {
        let stream = stream?;
        let mut reader = BufReader::new(&stream);
        let mut writer = BufWriter::new(&stream);
        if let Err(error) = serve_connection(&mut reader, &mut writer, store) {
            eprintln!("connection error: {error}");
        }
    }
    Ok(())
}

/// Serve one connection: read newline-delimited request lines, reply to each with
/// a newline-delimited JSON object, until the client hangs up (clean EOF).
fn serve_connection(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    store: &dyn Backend,
) -> io::Result<()> {
    while let Some(line) = read_line_bounded(reader)? {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str(&line) {
            Ok(request) => protocol::handle_request(store, &request),
            Err(error) => serde_json::json!({
                "id": serde_json::Value::Null,
                "error": { "code": protocol::PROTOCOL_MALFORMED, "message": error.to_string() },
            }),
        };
        let mut bytes = serde_json::to_vec(&reply).expect("a reply serializes");
        bytes.push(b'\n');
        writer.write_all(&bytes)?;
        writer.flush()?;
    }
    Ok(())
}

/// Read one newline-terminated request line, bounded by [`MAX_REQUEST_BYTES`].
/// `Ok(None)` on a clean EOF (the client hung up).
fn read_line_bounded(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let reader: &mut dyn BufRead = reader;
    let mut limited = reader.take(MAX_REQUEST_BYTES);
    let read = limited.read_until(b'\n', &mut buf)?;
    if read == 0 {
        return Ok(None);
    }
    if read as u64 == MAX_REQUEST_BYTES && !buf.ends_with(b"\n") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request line exceeds the size limit",
        ));
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    use marrow_store::mem::MemStore;
    use marrow_store::path::{PathSegment, SavedKey, encode_path};

    #[test]
    fn serves_newline_delimited_requests_over_a_stream() {
        let mut store = MemStore::new();
        store.write(
            &encode_path(&[
                PathSegment::Root("books".into()),
                PathSegment::RecordKey(SavedKey::Int(1)),
                PathSegment::Field("title".into()),
            ]),
            b"Mort".to_vec(),
        );

        // Two requests on two lines; the blank line is ignored.
        let input = "{\"id\":1,\"op\":\"saved_roots\"}\n\n{\"id\":2,\"op\":\"nope\"}\n";
        let mut reader = Cursor::new(input.as_bytes());
        let mut output: Vec<u8> = Vec::new();
        serve_connection(&mut reader, &mut output, &store).expect("serve");

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
}
