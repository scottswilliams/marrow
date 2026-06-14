//! The std::io file builtins, capability gating, and the rule that irreversible
//! host effects (file writes, output, logging) are rejected inside a transaction
//! before the effect lands.

use crate::support;
use support::*;

use marrow_run::{Host, RUN_CAPABILITY, Value};
use marrow_store::tree::TreeStore;
use std::cell::RefCell;
use std::rc::Rc;

/// A program exercising the four `std::io` file builtins.
const IO_SAMPLE: &str = "\
pub fn saveText(path: string, text: string)
    std::io::writeText(path, text)

pub fn loadText(path: string): string
    return std::io::readText(path)

pub fn saveBytes(path: string, data: bytes)
    std::io::writeBytes(path, data)

pub fn loadBytes(path: string): bytes
    return std::io::readBytes(path)

pub fn loadOrCode(path: string): string
    try
        return std::io::readText(path)
    catch err: Error
        return err.code
";

#[test]
fn io_round_trips_text_through_a_file() {
    let program = checked_program(IO_SAMPLE);
    let store = TreeStore::memory();
    let host = Host::new().with_filesystem();
    let dir = TempDir::new("marrow-run-test").expect("temp dir");
    let path = dir.path().join("note.txt").to_string_lossy().into_owned();
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(
            &program,
            "test::saveText",
            Value::Str(path.clone()),
            Value::Str("hello".into())
        ),
    )
    .expect("write");
    let loaded = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::loadText", Value::Str(path)),
    )
    .expect("read")
    .value;
    assert_eq!(loaded, Some(Value::Str("hello".into())));
}

#[test]
fn irreversible_host_effects_inside_a_transaction_are_rejected_before_the_effect() {
    let dir = TempDir::new("marrow-run-test").expect("temp dir");
    let path = dir.path().join("effect.txt");
    let program = checked_program(
        "pub fn write_in_txn(path: string)\n    transaction\n        std::io::writeText(path, \"leaked\")\n",
    );
    let store = TreeStore::memory();
    let host = Host::new().with_filesystem();
    assert_run_error(
        run_entry_with_host(
            &store,
            &host,
            checked_entry!(
                &program,
                "test::write_in_txn",
                Value::Str(path.to_string_lossy().into_owned())
            ),
        ),
        RUN_CAPABILITY,
    );
    assert!(
        !path.exists(),
        "host write must be rejected before creating the file"
    );
}

#[test]
fn output_inside_a_transaction_is_rejected_before_the_effect() {
    let program =
        checked_program("pub fn print_in_txn()\n    transaction\n        print(\"leaked\")\n");
    let store = TreeStore::memory();
    assert_run_error(
        run_entry(&store, checked_entry!(&program, "test::print_in_txn")),
        RUN_CAPABILITY,
    );
}

#[test]
fn log_inside_a_transaction_is_rejected_before_the_effect() {
    let program = checked_program(
        "pub fn log_in_txn()\n    transaction\n        std::log::info(\"leaked\")\n",
    );
    let store = TreeStore::memory();
    let log = Rc::new(RefCell::new(String::new()));
    let host = Host::new().with_log_sink(Rc::clone(&log));
    assert_run_error(
        run_entry_with_host(&store, &host, checked_entry!(&program, "test::log_in_txn")),
        RUN_CAPABILITY,
    );
    assert_eq!(log.borrow().as_str(), "");
}

#[test]
fn io_round_trips_bytes_through_a_file() {
    let program = checked_program(IO_SAMPLE);
    let store = TreeStore::memory();
    let host = Host::new().with_filesystem();
    let dir = TempDir::new("marrow-run-test").expect("temp dir");
    let path = dir.path().join("blob.bin").to_string_lossy().into_owned();
    let data = Value::Bytes(vec![0, 1, 2, 255, 128]);
    run_entry_with_host(
        &store,
        &host,
        checked_entry!(
            &program,
            "test::saveBytes",
            Value::Str(path.clone()),
            data.clone()
        ),
    )
    .expect("write");
    let loaded = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::loadBytes", Value::Str(path)),
    )
    .expect("read")
    .value;
    assert_eq!(loaded, Some(data));
}

#[test]
fn io_without_a_filesystem_capability_is_a_capability_error() {
    let program = checked_program(IO_SAMPLE);
    let store = TreeStore::memory();
    let result = run_entry(
        &store,
        checked_entry!(&program, "test::loadText", Value::Str("x".into())),
    );
    assert_run_error(result, RUN_CAPABILITY);
}

#[test]
fn an_io_error_raises_a_catchable_error() {
    // Reading a missing file (with the capability present) raises a typed Error
    // the program can `catch`, not a runtime fault.
    let program = checked_program(IO_SAMPLE);
    let store = TreeStore::memory();
    let host = Host::new().with_filesystem();
    let dir = TempDir::new("marrow-run-test").expect("temp dir");
    let missing = dir.path().join("absent.txt").to_string_lossy().into_owned();
    let code = run_entry_with_host(
        &store,
        &host,
        checked_entry!(&program, "test::loadOrCode", Value::Str(missing)),
    )
    .expect("caught")
    .value;
    assert_eq!(code, Some(Value::Str("io.read".into())));
}
