use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

fn temp_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("marrow-{name}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("create dir");
    root
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).expect("create dirs");
    fs::write(path, contents).expect("write file");
}

const CONFIG: &str =
    r#"{ "sourceRoots": ["src"], "store": { "backend": "native", "dataDir": ".data" } }"#;
const SRC: &str = "module app\n\
                   \n\
                   resource Counter at ^counter(id: int)\n\
                   \x20\x20\x20\x20required value: int\n\
                   \n\
                   pub fn seed()\n\
                   \x20\x20\x20\x20var c: Counter\n\
                   \x20\x20\x20\x20c.value = 42\n\
                   \x20\x20\x20\x20transaction\n\
                   \x20\x20\x20\x20\x20\x20\x20\x20^counter(1) = c\n";

fn native_project(name: &str) -> PathBuf {
    let root = temp_dir(name);
    write(&root, "marrow.json", CONFIG);
    write(&root, "src/app.mw", SRC);
    root
}

fn marrow(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_marrow"))
        .args(args)
        .output()
        .expect("run marrow")
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
fn serve_answers_saved_roots_over_a_loopback_socket() {
    let project = native_project("serve-roots");
    let dir = project.to_str().unwrap().to_string();
    assert_eq!(
        marrow(&["run", "--entry", "app::seed", &dir]).status.code(),
        Some(0)
    );

    let (mut child, address) = spawn_serve(&dir);
    let reply = request(&address, &json!({ "id": 1, "op": "saved_roots" }));
    child.kill().ok();
    child.wait().ok();
    fs::remove_dir_all(&project).ok();

    assert_eq!(reply["id"], json!(1), "{reply}");
    assert_eq!(reply["ok"]["roots"], json!(["counter"]), "{reply}");
}

#[test]
fn serving_an_unseeded_project_serves_empty_roots_and_creates_no_store() {
    let project = native_project("serve-empty");
    let dir = project.to_str().unwrap().to_string();

    let (mut child, address) = spawn_serve(&dir);
    let reply = request(&address, &json!({ "id": 2, "op": "saved_roots" }));
    child.kill().ok();
    child.wait().ok();
    // Serving is read-only: it must not materialize the store file.
    let created = project.join(".data").join("marrow.redb").exists();
    fs::remove_dir_all(&project).ok();

    assert_eq!(reply["ok"]["roots"], json!([]), "{reply}");
    assert!(!created, "serve must not create the store");
}
