//! The terminal side of the persistent (native) companion path.
//!
//! [`attach_and_call`] is the one-shot client half of the local wire: it spawns a verified stock
//! runner as a native attached session over a persistent store, admits nothing else, submits
//! exactly one call, and renders the result back as a runtime [`Value`](marrow_vm::Value). The
//! terminal (`marrow run … --store`) drives it. The runner is the sole opener of the store; this
//! side never touches the store directory, the engine, or a lifecycle state — it only speaks the
//! wire to the process that does.
//!
//! The one launched session is gated by two secrets exactly as the supervised g02p channel is:
//! the terminal mints a launch nonce, hands it to the spawned runner through the
//! `MARROW_RUNNER_NONCE` environment variable (so it is never echoed on the descriptor line),
//! proves it in the handshake, and checks the runner proves its session token and served
//! interface identity back before it sends the call. The spawn, descriptor, handshake, framing,
//! and reply decoding are the shared [`crate::terminal`] plumbing.

use std::path::Path;
use std::time::Duration;

use marrow_local_wire::{ClientMessage, HandoffStage, Id32, Json, LossClass, classify};
use marrow_verify::VerifiedImage;

use crate::terminal::{
    self, CALL_DEADLINE, CallOutcome, ClientError, connect_and_handshake, read_message,
    reply_to_outcome, require_interface, spawn_companion, write_message,
};

/// Spawn the verified companion at `runner_exe`, attach it to the persistent store at `store`,
/// and submit exactly one call to `export_id` with `args`. The companion is the sole opener of
/// the store. `runner_exe` must already be the release-verified stock runner (the terminal
/// verifies it against the release manifest before calling this).
pub fn attach_and_call(
    runner_exe: &Path,
    image: &VerifiedImage,
    image_bytes: &[u8],
    store: &Path,
    export_id: [u8; 32],
    args: Vec<Json>,
) -> Result<CallOutcome, ClientError> {
    let deadline = CALL_DEADLINE;
    let nonce = terminal::mint_nonce()?;

    let (mut companion, descriptor) =
        spawn_companion(runner_exe, "attach", image_bytes, Some(store), nonce)?;

    // The companion must serve exactly the image we spawned it with, or we refuse before sending
    // the call.
    require_interface(&descriptor, image)?;

    let outcome = call_over_socket(image, &descriptor, nonce, export_id, args, deadline);
    // The call is done and its socket dropped, so the companion has already seen the client hang
    // up and is exiting; wait for it here so the ordinary path is a clean exit rather than the
    // drop guard's kill. The guard still removes the staging directory.
    let _ = companion.child.wait();
    outcome
}

/// Connect, prove the nonce, verify the runner proves the session and interface back, submit one
/// request, and decode the reply.
fn call_over_socket(
    image: &VerifiedImage,
    descriptor: &crate::terminal::Descriptor,
    nonce: Id32,
    export_id: [u8; 32],
    args: Vec<Json>,
    deadline: Duration,
) -> Result<CallOutcome, ClientError> {
    // Before the request is on the wire, a connect/handshake or write failure means the call
    // provably did not start (`LossClass::NotStarted`): it surfaces as a `ClientError` the
    // caller may treat as undone, and is never replayed.
    let mut stream = connect_and_handshake(descriptor, nonce, deadline)?;
    write_message(
        &mut stream,
        &ClientMessage::Request {
            export: Id32::from_bytes(export_id),
            args,
        },
        deadline,
    )?;
    // The request is dispatched. If the runner now dies (or falls silent past the deadline)
    // before its reply arrives, the call may have run — wholly or partly — and its durable
    // outcome is unknowable from this side (`LossClass::OutcomeUnknown`). It is reported as a
    // distinct typed outcome, never replayed: a read-only refresh observes the current state.
    match read_message(&mut stream, deadline) {
        Ok(reply) => reply_to_outcome(image, export_id, reply),
        Err(error) if post_dispatch_read_failed(&error) => {
            debug_assert_eq!(
                classify(HandoffStage::Dispatched),
                LossClass::OutcomeUnknown
            );
            Ok(CallOutcome::OutcomeUnknown)
        }
        Err(error) => Err(error),
    }
}

/// Whether the socket read failed after the request was dispatched. Every socket I/O failure at
/// this boundary loses the reply: the request may have run, regardless of the operating-system
/// error kind. A lost reply classifies the dispatched call as
/// [`CallOutcome::OutcomeUnknown`]; a reply that arrives but does not decode stays its own
/// distinct wire/decode error, since the call demonstrably produced a reply.
fn post_dispatch_read_failed(error: &ClientError) -> bool {
    matches!(error, ClientError::Io(_))
}

#[cfg(test)]
mod tests {
    use super::post_dispatch_read_failed;
    use crate::terminal::ClientError;
    use marrow_local_wire::{HandoffStage, LossClass, WireError, classify};

    /// A reply lost to any socket-read I/O failure after dispatch is classified
    /// `OutcomeUnknown`, matching the wire loss model for a `Dispatched` handoff stage — the
    /// native one-shot call is reported outcome-unknown, never replayed.
    #[test]
    fn every_socket_read_io_after_dispatch_is_outcome_unknown() {
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::ConnectionRefused,
            std::io::ErrorKind::UnexpectedEof,
            std::io::ErrorKind::BrokenPipe,
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::ConnectionAborted,
            std::io::ErrorKind::NotConnected,
            std::io::ErrorKind::AddrInUse,
            std::io::ErrorKind::AddrNotAvailable,
            std::io::ErrorKind::TimedOut,
            std::io::ErrorKind::WriteZero,
            std::io::ErrorKind::Interrupted,
            std::io::ErrorKind::Unsupported,
            std::io::ErrorKind::OutOfMemory,
            std::io::ErrorKind::Other,
        ] {
            assert!(
                post_dispatch_read_failed(&ClientError::Io(std::io::Error::from(kind))),
                "every {kind:?} socket-read failure after dispatch loses the reply",
            );
        }
        assert!(post_dispatch_read_failed(&ClientError::Io(
            std::io::Error::from_raw_os_error(i32::MAX),
        )));
        assert_eq!(
            classify(HandoffStage::Dispatched),
            LossClass::OutcomeUnknown
        );
    }

    /// A reply that arrives but does not decode is not a lost reply — the call produced a
    /// reply, so it stays its own distinct error rather than being reported outcome-unknown.
    #[test]
    fn a_decoded_reply_error_is_not_a_lost_reply() {
        assert!(!post_dispatch_read_failed(&ClientError::ReplyDecode));
        assert!(!post_dispatch_read_failed(&ClientError::Handshake));
        assert!(!post_dispatch_read_failed(&ClientError::Wire(
            WireError::Malformed,
        )));
        // A before-send failure (a pre-dispatch handshake/connect error) classifies NotStarted,
        // the safe-to-consider-undone class, and never reaches the lost-reply path.
        assert_eq!(classify(HandoffStage::BeforeSend), LossClass::NotStarted);
    }
}
