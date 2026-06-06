use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

mod support;

use support::{TempProject, marrow};

fn native_project(name: &str) -> TempProject {
    support::temp_project(name, |root| {
        support::write(root, "marrow.json", support::native_config());
        support::write(root, "src/app.mw", support::counter_source());
    })
}

/// Spawn `marrow serve --port 0 <dir>` and return the child plus the loopback
/// address it printed on startup.
fn spawn_serve(dir: &str) -> (Child, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(["serve", "--port", "0", dir])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn marrow serve");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read the address line");
    let address = line
        .trim()
        .rsplit(' ')
        .next()
        .filter(|address| address.contains(':'))
        .unwrap_or_else(|| {
            let mut stderr = String::new();
            child.kill().ok();
            child
                .stderr
                .take()
                .map(|mut handle| handle.read_to_string(&mut stderr));
            panic!("serve did not print an address; stderr: {stderr}");
        })
        .to_string();
    (child, address)
}

/// Send one request to a running server and return its parsed reply.
fn request(address: &str, body: &Value) -> Value {
    let stream = TcpStream::connect(address).expect("connect to serve");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    let mut line = serde_json::to_vec(body).expect("serialize request");
    line.push(b'\n');
    (&stream).write_all(&line).expect("write request");
    (&stream).flush().expect("flush request");
    let mut reader = BufReader::new(&stream);
    let mut reply = String::new();
    reader.read_line(&mut reply).expect("read reply");
    serde_json::from_str(reply.trim()).expect("reply json")
}

#[test]
fn serve_answers_debug_data_roots_over_a_loopback_socket() {
    let project = native_project("serve-roots");
    let dir = project.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0)
    );

    let (mut child, address) = spawn_serve(&dir);
    let reply = request(&address, &json!({ "id": 1, "op": "debug_data_roots" }));
    child.kill().ok();
    child.wait().ok();

    assert_eq!(reply["id"], json!(1), "{reply}");
    assert_eq!(reply["ok"]["roots"], json!(["counter"]), "{reply}");
}

#[test]
fn serve_answers_path_addressed_reads_over_a_loopback_socket() {
    let project = native_project("serve-reads");
    let dir = project.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0)
    );

    let (mut child, address) = spawn_serve(&dir);
    let get = request(
        &address,
        &json!({
            "id": 1, "op": "debug_data_get",
            "path": [{"root": "counter"}, {"key": {"int": 1}}, {"field": "value"}],
        }),
    );
    let children = request(
        &address,
        &json!({ "id": 2, "op": "debug_data_children", "path": [{"root": "counter"}] }),
    );
    let walk = request(
        &address,
        &json!({ "id": 3, "op": "debug_data_walk", "path": [{"root": "counter"}], "limit": 100 }),
    );
    child.kill().ok();
    child.wait().ok();

    // The stored int 42 encodes canonically as "42", which is base64 "NDI=".
    assert_eq!(get["ok"]["presence"], json!("value_only"), "{get}");
    assert_eq!(get["ok"]["value"], json!("NDI="), "{get}");
    assert_eq!(
        children["ok"]["children"],
        json!([{ "key": { "int": 1 } }]),
        "{children}"
    );
    // The walk reaches the one stored field and is not truncated.
    assert_eq!(walk["ok"]["truncated"], json!(false), "{walk}");
    assert_eq!(
        walk["ok"]["entries"].as_array().map(Vec::len),
        Some(1),
        "{walk}"
    );
}

#[test]
fn serving_an_unseeded_project_serves_empty_roots_and_creates_no_store() {
    let project = native_project("serve-empty");
    let dir = project.to_str().unwrap().to_string();

    let (mut child, address) = spawn_serve(&dir);
    let reply = request(&address, &json!({ "id": 2, "op": "debug_data_roots" }));
    child.kill().ok();
    child.wait().ok();
    // Serving is read-only: it must not materialize the store file.
    let created = project.join(".data").join("marrow.redb").exists();

    assert_eq!(reply["ok"]["roots"], json!([]), "{reply}");
    assert!(!created, "serve must not create the store");
}
