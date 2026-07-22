//! The ephemeral-memory terminal path over a real companion process and a real socket.
//!
//! This is the G02b exit-gate journey: the E06 Workshop image is attached to a *fresh in-memory*
//! store held by one spawned `marrow-runner attach-ephemeral` process, and the whole
//! add / read / correct / cross-root rollback / re-read journey runs over that one session. Unlike
//! the native path — where each call is its own process against a persistent store — every call
//! here shares one runner process and one in-RAM store, so a committed write is observable by a
//! later call *in the same session* and is gone when the session ends. The terminal-side client
//! under test is [`EphemeralSession`]; the memory store is never provisioned (it is minted empty
//! in the runner) and never persists.

use std::path::PathBuf;

use marrow_runner::{CallOutcome, EphemeralCall, EphemeralSession, Json};
use marrow_verify::VerifiedImage;
use marrow_vm::Value;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("fixtures/v01/conformance/workshop")
}

fn runner_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_marrow-runner"))
}

fn compile_verify() -> (VerifiedImage, Vec<u8>) {
    let source = std::fs::read(fixture_dir().join("src/main.mw")).expect("read fixture source");
    let ids = std::fs::read(fixture_dir().join(".marrow/ids")).expect("read fixture ledger");
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source,
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(&ids),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let bytes = marrow_compile::compile(&project)
        .expect("compile")
        .image
        .bytes;
    let image = marrow_verify::verify(&bytes).expect("verify");
    (image, bytes)
}

fn export_id(image: &VerifiedImage, name: &str) -> [u8; 32] {
    *image
        .exports()
        .iter()
        .find(|export| image.function(export.function()).name() == name)
        .unwrap_or_else(|| panic!("export `{name}` present"))
        .id()
        .bytes()
}

fn present_name(name: &str) -> Option<Value> {
    Some(Value::Optional(Some(Box::new(Value::Text(name.into())))))
}

/// One live session driving a sequence of calls against one in-memory store.
struct Session<'a> {
    inner: EphemeralSession<'a>,
    image: &'a VerifiedImage,
}

impl<'a> Session<'a> {
    fn call(&mut self, name: &str, args: Vec<Json>) -> CallOutcome {
        match self
            .inner
            .call(export_id(self.image, name), args)
            .unwrap_or_else(|error| panic!("call `{name}` failed: {}", error.code()))
        {
            EphemeralCall::Replied(outcome) => outcome,
            EphemeralCall::Lost(class) => panic!("call `{name}` lost the session: {class:?}"),
        }
    }

    fn value(&mut self, name: &str, args: Vec<Json>) -> Option<Value> {
        match self.call(name, args) {
            CallOutcome::Value(value) => value,
            CallOutcome::Fault { code, .. } => panic!("`{name}` faulted: {code}"),
            CallOutcome::Reject { code } => panic!("`{name}` rejected: {code}"),
            CallOutcome::OutcomeUnknown => panic!("`{name}` outcome unknown"),
        }
    }

    fn fault(&mut self, name: &str, args: Vec<Json>) -> String {
        match self.call(name, args) {
            CallOutcome::Fault { code, .. } => code,
            CallOutcome::Value(_) => panic!("`{name}` did not fault"),
            CallOutcome::Reject { code } => panic!("`{name}` rejected: {code}"),
            CallOutcome::OutcomeUnknown => panic!("`{name}` outcome unknown"),
        }
    }
}

/// The full Workshop journey over one ephemeral session: add commits across both roots and is
/// read back by a *later call on the same session* (the store lives in the runner's RAM for the
/// session's life); a committed move advances the tally; an unguarded move on an absent asset
/// faults and rolls its whole cross-root region back; the final reads show every root at its
/// prior committed value — all within one in-memory store that never touched disk.
#[test]
fn workshop_journey_over_one_ephemeral_session() {
    let (image, bytes) = compile_verify();
    let inner =
        EphemeralSession::open(&runner_exe(), &image, &bytes).expect("open the ephemeral session");
    let mut session = Session {
        inner,
        image: &image,
    };
    let epoch = marrow_temporal::format_instant(0).expect("epoch instant");

    // add commits an asset across ^assets and ^tallies; a later call on the same session reads
    // it back from the same in-memory store.
    assert_eq!(
        session.value(
            "add",
            vec![
                Json::Int(1),
                Json::Str("T-100".into()),
                Json::Str("Cordless Drill".into()),
                Json::Str("power".into()),
                Json::Str(epoch.clone()),
            ],
        ),
        Some(Value::Bool(true)),
    );
    assert_eq!(
        session.value("assetName", vec![Json::Int(1)]),
        present_name("Cordless Drill"),
    );
    assert_eq!(session.value("catalogued", vec![]), Some(Value::Int(1)));

    // A committed cross-root move, then read back on the same session.
    session.value("recordMove", vec![Json::Int(1), Json::Str("Bay 3".into())]);
    assert_eq!(
        session.value("location", vec![Json::Int(1)]),
        present_name("Bay 3"),
    );
    assert_eq!(session.value("moveCount", vec![]), Some(Value::Int(1)));

    // Cross-root rollback: a move on an absent asset faults required-missing and rolls the whole
    // staged region back across both roots.
    assert_eq!(
        session.fault("recordMove", vec![Json::Int(2), Json::Str("Bay 9".into())]),
        "run.required_missing",
    );

    // Every root stands at its prior committed value after the rolled-back fault.
    assert_eq!(
        session.value("assetName", vec![Json::Int(1)]),
        present_name("Cordless Drill"),
    );
    assert_eq!(
        session.value("location", vec![Json::Int(1)]),
        present_name("Bay 3"),
    );
    assert_eq!(
        session.value("present", vec![Json::Int(2)]),
        Some(Value::Bool(false)),
    );
    assert_eq!(session.value("catalogued", vec![]), Some(Value::Int(1)));
    assert_eq!(session.value("moveCount", vec![]), Some(Value::Int(1)));
}

/// A committed add is observable with its `log` descendant on a later call in the same session:
/// the asset name and its first note entry both read back from the one in-memory store.
#[test]
fn a_committed_add_is_observable_with_its_log_descendant() {
    let (image, bytes) = compile_verify();
    let inner =
        EphemeralSession::open(&runner_exe(), &image, &bytes).expect("open the ephemeral session");
    let mut session = Session {
        inner,
        image: &image,
    };
    let epoch = marrow_temporal::format_instant(0).expect("epoch instant");

    session.value(
        "add",
        vec![
            Json::Int(7),
            Json::Str("T-700".into()),
            Json::Str("Sander".into()),
            Json::Str("power".into()),
            Json::Str(epoch),
        ],
    );
    assert_eq!(
        session.value("assetName", vec![Json::Int(7)]),
        present_name("Sander"),
    );
    assert_eq!(
        session.value("noteText", vec![Json::Int(7), Json::Int(1)]),
        present_name("catalogued"),
    );
}

/// A fresh session opens an empty store: an asset committed in one session is *not* visible in a
/// second session, because the in-memory store was discarded with the first runner. This is the
/// ephemeral contract — no persistence across sessions.
#[test]
fn a_new_session_starts_from_an_empty_store() {
    let (image, bytes) = compile_verify();
    let epoch = marrow_temporal::format_instant(0).expect("epoch instant");

    {
        let inner = EphemeralSession::open(&runner_exe(), &image, &bytes).expect("open first");
        let mut first = Session {
            inner,
            image: &image,
        };
        first.value(
            "add",
            vec![
                Json::Int(5),
                Json::Str("T-500".into()),
                Json::Str("Jigsaw".into()),
                Json::Str("power".into()),
                Json::Str(epoch),
            ],
        );
        assert_eq!(
            first.value("assetName", vec![Json::Int(5)]),
            present_name("Jigsaw"),
        );
    }

    // A second session's store is empty: the prior asset is absent.
    let inner = EphemeralSession::open(&runner_exe(), &image, &bytes).expect("open second");
    let mut second = Session {
        inner,
        image: &image,
    };
    assert_eq!(
        second.value("present", vec![Json::Int(5)]),
        Some(Value::Bool(false)),
    );
}
