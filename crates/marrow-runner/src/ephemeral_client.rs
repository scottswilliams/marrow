//! The terminal side of the ephemeral-memory companion path.
//!
//! [`EphemeralSession`] is the long-lived client half of the local wire: it spawns a verified
//! stock runner as an ephemeral-memory attached session, proves the handshake, and then submits
//! a *sequence* of calls against the one in-memory store that runner holds for the session's
//! life. A committed mutation is observable by a later call in the same session; when the session
//! ends the store is gone. Unlike the one-shot native [`attach_and_call`](crate::attach_and_call),
//! several calls share one runner process and one store.
//!
//! **A lost reply is classified, never replayed.** Each call advances through the handoff stages
//! [`HandoffStage`] tracks: before the request is written it is `BeforeSend`; once written it is
//! `Dispatched`. If the runner dies, the call's verdict is [`classify`]'d from the last stage it
//! reached — `NotStarted` when the request was never delivered, `OutcomeUnknown` when it was and
//! may have run. A mutating call whose reply is lost is reported as `OutcomeUnknown` and is *not*
//! resubmitted: the session marks itself dead and every later call is `NotStarted` without
//! touching the wire, so no request is ever silently sent twice. There is no delivery ledger and
//! no replay path — a lost outcome is unknowable from this side, and for an ephemeral store the
//! store died with the runner regardless.

use std::os::unix::net::UnixStream;
use std::path::Path;

use marrow_local_wire::{ClientMessage, HandoffStage, Id32, Json, LossClass, classify};
use marrow_verify::VerifiedImage;

use crate::terminal::{
    self, CALL_DEADLINE, CallOutcome, ClientError, Companion, connect_and_handshake, read_message,
    reply_to_outcome, require_interface, spawn_companion, write_message,
};

/// The result of one call over an ephemeral session.
pub enum EphemeralCall {
    /// The runner replied: the ordinary call outcome (a value, a fault, or a typed reject). This
    /// is the post-reply boundary — the outcome is known exactly.
    Replied(CallOutcome),
    /// The runner died before its reply arrived. The class is derived from how far the request
    /// got: [`LossClass::NotStarted`] (the request was never delivered, so it provably did not
    /// run) or [`LossClass::OutcomeUnknown`] (it was dispatched and may have run). Never
    /// replayed.
    Lost(LossClass),
}

/// A live ephemeral-memory session: the spawned runner, the handshaken socket, and the served
/// image. Dropping it hangs up the socket and tears the runner down; the in-memory store the
/// runner held is discarded with the process.
pub struct EphemeralSession<'a> {
    image: &'a VerifiedImage,
    // `stream` is declared before `_companion` so it drops first: the hangup the runner observes
    // precedes the drop guard's kill, giving the ordinary end a clean exit.
    stream: UnixStream,
    // An RAII guard, never read: dropping it kills the spawned runner and removes its staging
    // directory. `None` only for the in-crate unit tests, which drive `call` over a socket pair
    // without a spawned process; the production `open` path always holds a companion.
    _companion: Option<Companion>,
    // Set once a call detects the runner's death (or a broken reply). A dead session never writes
    // to the wire again, so a call on it provably never starts.
    dead: bool,
}

impl<'a> EphemeralSession<'a> {
    /// Spawn the verified companion at `runner_exe` as an ephemeral-memory attached session for
    /// `image` and complete the handshake, returning a session ready to call. `runner_exe` must
    /// already be the release-verified stock runner. The companion opens the in-memory store only
    /// after this handshake proves the launch nonce.
    pub fn open(
        runner_exe: &Path,
        image: &'a VerifiedImage,
        image_bytes: &[u8],
    ) -> Result<Self, ClientError> {
        let nonce = terminal::mint_nonce()?;
        let (companion, descriptor) =
            spawn_companion(runner_exe, "attach-ephemeral", image_bytes, None, nonce)?;

        // The companion must serve exactly the image we spawned it with, or we refuse before any
        // call.
        require_interface(&descriptor, image)?;

        let stream = connect_and_handshake(&descriptor, nonce, CALL_DEADLINE)?;
        Ok(Self {
            image,
            stream,
            _companion: Some(companion),
            dead: false,
        })
    }

    /// Submit one call to `export_id` with `args` against this session's store and resolve its
    /// outcome. A returned reply is [`EphemeralCall::Replied`]; a runner death is
    /// [`EphemeralCall::Lost`] with the class derived from how far the request got. The call is
    /// never retried, and a lost outcome is never resubmitted.
    pub fn call(
        &mut self,
        export_id: [u8; 32],
        args: Vec<Json>,
    ) -> Result<EphemeralCall, ClientError> {
        // A known-dead session delivered nothing: this request never started.
        if self.dead {
            return Ok(EphemeralCall::Lost(classify(HandoffStage::BeforeSend)));
        }

        let request = ClientMessage::Request {
            export: Id32::from_bytes(export_id),
            args,
        };
        // Before the request is written it is `BeforeSend`. A local encode failure never reached
        // the runner and is not a death — the session stays usable and the caller sees the real
        // error. A transport failure means the frame was not fully delivered (the runner never
        // decodes a whole request frame), so the call provably did not run and the session dies.
        if let Err(error) = write_message(&mut self.stream, &request, CALL_DEADLINE) {
            return match error {
                ClientError::Wire(_) => Err(error),
                _ => {
                    self.dead = true;
                    Ok(EphemeralCall::Lost(classify(HandoffStage::BeforeSend)))
                }
            };
        }

        // The request is now dispatched to the serial worker; from here a lost reply is
        // `OutcomeUnknown` — the call may have run, wholly or partly, and its outcome is
        // unknowable from this side. It is reported, never replayed.
        match read_message(&mut self.stream, CALL_DEADLINE) {
            Ok(message) => Ok(EphemeralCall::Replied(reply_to_outcome(
                self.image, export_id, message,
            )?)),
            Err(ClientError::Io(_)) | Err(ClientError::Wire(_)) => {
                self.dead = true;
                Ok(EphemeralCall::Lost(classify(HandoffStage::Dispatched)))
            }
            // A decoded-but-mismatched reply or an out-of-protocol message is a protocol fault the
            // reply *did* carry, not a lost outcome; but a session that observed it is untrustworthy
            // and is retired so no further request is written to it.
            Err(other) => {
                self.dead = true;
                Err(other)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::thread;
    use std::time::Duration;

    use marrow_local_wire::{ServerMessage, frame_body_len};

    /// Build a session over one end of a socket pair, with no spawned companion. The peer end is
    /// returned to the test to play the runner's role deterministically.
    fn paired(image: &VerifiedImage) -> (EphemeralSession<'_>, UnixStream) {
        let (client, peer) = UnixStream::pair().expect("socket pair");
        client.set_nonblocking(true).expect("nonblocking");
        let session = EphemeralSession {
            image,
            stream: client,
            _companion: None,
            dead: false,
        };
        (session, peer)
    }

    /// The compiled-and-verified bytes of a trivial storeless image. Obtaining the image is the
    /// sanctioned `bytes → verify` path, spelled inline at each call site (no alternate factory
    /// that returns a `VerifiedImage`).
    fn echo_bytes() -> Vec<u8> {
        let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
        let files = vec![marrow_project::CapturedFile::new(
            "src/main.mw".to_string(),
            b"pub fn echo(): int {\n    return 7\n}\n".to_vec(),
        )];
        let project = marrow_project::capture(
            &manifest,
            files,
            None,
            &marrow_project::CaptureLimits::DEFAULT,
        )
        .expect("capture");
        marrow_compile::compile(&project)
            .expect("compile")
            .image
            .bytes
    }

    fn echo_export(image: &VerifiedImage) -> [u8; 32] {
        *image
            .exports()
            .iter()
            .find(|export| image.function(export.function()).name() == "echo")
            .expect("echo export")
            .id()
            .bytes()
    }

    fn read_frame(stream: &mut UnixStream) -> Option<Vec<u8>> {
        stream.set_nonblocking(false).ok()?;
        let mut header = [0u8; 4];
        stream.read_exact(&mut header).ok()?;
        let len = frame_body_len(header).ok()?;
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).ok()?;
        Some(body)
    }

    /// A reply that arrives resolves as `Replied`, decoded against the export's return type.
    #[test]
    fn a_reply_resolves_as_replied() {
        let image = marrow_verify::verify(&echo_bytes()).expect("verify");
        let export = echo_export(&image);
        let (mut session, mut peer) = paired(&image);

        let responder = thread::spawn(move || {
            let _request = read_frame(&mut peer).expect("request frame");
            let reply = ServerMessage::Value { data: Json::Int(7) }
                .encode()
                .expect("encode reply");
            peer.write_all(&reply).expect("write reply");
        });

        match session.call(export, vec![]).expect("call") {
            EphemeralCall::Replied(CallOutcome::Value(Some(marrow_vm::Value::Int(7)))) => {}
            _ => panic!("expected a replied int value"),
        }
        responder.join().expect("responder");
    }

    /// A transport write failure — the send half is shut down before the request is written —
    /// is `NotStarted`: the frame provably never reaches the runner, so the call did not run, and
    /// the session is retired. This exercises the real failing-write arm (an `Io` error from the
    /// socket), not the pre-set `dead` flag.
    #[test]
    fn a_failed_write_is_not_started() {
        use std::net::Shutdown;

        let image = marrow_verify::verify(&echo_bytes()).expect("verify");
        let export = echo_export(&image);
        let (mut session, _peer) = paired(&image);
        session
            .stream
            .shutdown(Shutdown::Write)
            .expect("shut down the send half");

        match session.call(export, vec![]).expect("call") {
            EphemeralCall::Lost(LossClass::NotStarted) => {}
            _ => panic!("a failed write must classify NotStarted"),
        }
        // The session is retired: a later call still never starts and is never replayed.
        match session.call(export, vec![]).expect("second call") {
            EphemeralCall::Lost(LossClass::NotStarted) => {}
            _ => panic!("a call after a failed write must remain NotStarted"),
        }
    }

    /// The runner dying after the request is dispatched — no reply frame — resolves as
    /// `OutcomeUnknown`. The peer reads the whole request (so the write completed and the call is
    /// genuinely dispatched) and then drops without replying.
    #[test]
    fn a_death_after_dispatch_is_outcome_unknown() {
        let image = marrow_verify::verify(&echo_bytes()).expect("verify");
        let export = echo_export(&image);
        let (mut session, mut peer) = paired(&image);

        let killer = thread::spawn(move || {
            let _request = read_frame(&mut peer).expect("request frame");
            drop(peer); // die with no reply
        });

        match session.call(export, vec![]).expect("call") {
            EphemeralCall::Lost(LossClass::OutcomeUnknown) => {}
            _ => panic!("expected OutcomeUnknown after a lost reply"),
        }
        killer.join().expect("killer");

        // The session is now dead: a later call provably never starts and is never replayed.
        match session.call(export, vec![]).expect("second call") {
            EphemeralCall::Lost(LossClass::NotStarted) => {}
            _ => panic!("a call on a dead session must be NotStarted, never replayed"),
        }
    }

    /// Enforcement (no replay machinery, no delivery ledger). The call vocabulary is closed —
    /// there is a replied outcome and a lost one, and no third "replayed"/"resubmitted" outcome —
    /// and the session carries no per-request delivery ledger or replay buffer. Adding a replay
    /// outcome, a new loss class, or a ledger field breaks this exhaustive match/destructure at
    /// compile time, so a replay path cannot reappear unnoticed.
    #[test]
    fn the_session_admits_no_replay_or_delivery_ledger() {
        match EphemeralCall::Lost(LossClass::OutcomeUnknown) {
            EphemeralCall::Replied(_) | EphemeralCall::Lost(_) => {}
        }
        match LossClass::NotStarted {
            LossClass::NotStarted | LossClass::Interrupted | LossClass::OutcomeUnknown => {}
        }
        let image = marrow_verify::verify(&echo_bytes()).expect("verify");
        let (session, _peer) = paired(&image);
        let EphemeralSession {
            image: _,
            stream: _,
            _companion: _,
            dead: _,
        } = session;
    }

    /// A lost reply is never a replay: once dead, the session sends nothing further on the wire.
    #[test]
    fn a_dead_session_never_touches_the_wire() {
        let image = marrow_verify::verify(&echo_bytes()).expect("verify");
        let export = echo_export(&image);
        let (mut session, mut peer) = paired(&image);
        session.dead = true;

        match session.call(export, vec![]).expect("call on dead session") {
            EphemeralCall::Lost(LossClass::NotStarted) => {}
            _ => panic!("expected NotStarted on a dead session"),
        }
        // Nothing was written: a read on the peer end sees no request frame within a short window.
        peer.set_read_timeout(Some(Duration::from_millis(50)))
            .expect("timeout");
        let mut byte = [0u8; 1];
        assert!(
            peer.read(&mut byte).is_err() || peer.read(&mut byte).map(|n| n == 0).unwrap_or(false),
            "a dead session must not send a request frame",
        );
    }
}
