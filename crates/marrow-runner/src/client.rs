//! The terminal side of the persistent (native) companion path.
//!
//! [`attach_and_call`] is the one-shot client half of the local wire: it spawns a verified stock
//! runner as a native attached session over a persistent store, admits nothing else, submits
//! exactly one call, and renders the result back as a runtime [`Value`](marrow_vm::Value). The
//! terminal (`marrow run … --store`) drives it. The runner is the sole opener of the store; this
//! side never touches the store directory, the engine, or a lifecycle state — it only speaks the
//! wire to the process that does.
//!
//! The one launched session is gated by two secrets exactly as the supervised channel is:
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
    self, CALL_DEADLINE, CallOutcome, ClientError, CompanionCleanupError, connect_and_handshake,
    post_dispatch_cause, read_message_with_turn, reply_to_outcome, require_interface,
    require_reply_turn, spawn_companion, write_message_with_turn,
};

/// A one-shot call outcome and the independent result of settling its direct child.
#[must_use]
pub struct AttachCompletion {
    pub outcome: Result<CallOutcome, ClientError>,
    pub cleanup: Result<(), CompanionCleanupError>,
}

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
) -> AttachCompletion {
    let deadline = CALL_DEADLINE;
    let launch = terminal::mint_nonce()
        .map_err(terminal::CompanionStartupError::from)
        .and_then(|nonce| {
            spawn_companion(runner_exe, image_bytes, Some(store), nonce)
                .map(|(companion, descriptor)| (nonce, companion, descriptor))
        });
    let (nonce, companion, descriptor) = match launch {
        Ok(launch) => launch,
        Err(error) => {
            return AttachCompletion {
                outcome: Err(error.error),
                cleanup: error.cleanup,
            };
        }
    };
    let outcome = descriptor.and_then(|descriptor| {
        require_interface(&descriptor, image).map_err(ClientError::activation_unknown)?;
        call_over_socket(image, &descriptor, nonce, export_id, args, deadline)
    });
    // All descriptor and invocation transports are closed before observing child exit.
    AttachCompletion {
        outcome,
        cleanup: companion.settle(),
    }
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
    // Attach may already have rebound the store before the handshake. A missing startup
    // result cannot establish its outcome, even though no invocation has been sent.
    let mut stream =
        connect_and_handshake(descriptor, nonce, deadline, terminal::CompanionKind::Native)
            .map_err(ClientError::activation_unknown)?;
    const TURN: u32 = 0;
    write_message_with_turn(
        &mut stream,
        &ClientMessage::Request {
            export: Id32::from_bytes(export_id),
            args,
        },
        TURN,
        deadline,
    )?;
    // The request is dispatched. If the runner now dies (or falls silent past the deadline)
    // before one exact valid correlated reply is accepted, the call may have run — wholly or
    // partly — and its durable outcome is unknowable from this side. Transport, wire,
    // correlation, unsolicited-message, and value-decode causes remain typed evidence under
    // that outcome. It is never replayed.
    let result = match read_message_with_turn(&mut stream, deadline) {
        Ok((reply, received)) => require_reply_turn(TURN, received)
            .and_then(|()| reply_to_outcome(image, export_id, reply).map_err(post_dispatch_cause)),
        Err(error) => Err(post_dispatch_cause(error)),
    };
    match result {
        Ok(outcome) => Ok(outcome),
        Err(cause) => {
            debug_assert_eq!(
                classify(HandoffStage::Dispatched),
                LossClass::OutcomeUnknown
            );
            Ok(CallOutcome::OutcomeUnknown { cause })
        }
    }
}

/// Every socket-read cause after dispatch remains orthogonal evidence beneath
/// [`CallOutcome::OutcomeUnknown`]; accepting a reply requires exact framing, turn correlation,
/// message schema, and value decoding.
#[cfg(test)]
mod tests {
    use marrow_codes::Code;

    use crate::OutcomeUnknownCause;
    use crate::terminal::{ClientError, post_dispatch_cause, require_reply_turn};
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
            assert!(matches!(
                post_dispatch_cause(ClientError::Io(std::io::Error::from(kind))),
                OutcomeUnknownCause::Io(_),
            ));
        }
        assert!(matches!(
            post_dispatch_cause(ClientError::Io(
                std::io::Error::from_raw_os_error(i32::MAX,)
            )),
            OutcomeUnknownCause::Io(_),
        ));
        assert_eq!(
            classify(HandoffStage::Dispatched),
            LossClass::OutcomeUnknown
        );
    }

    /// Every invalid post-write reply retains its distinct cause without replacing
    /// the dispatched call's outcome-unknown disposition.
    #[test]
    fn invalid_replies_keep_typed_causes_under_outcome_unknown() {
        assert!(matches!(
            post_dispatch_cause(ClientError::ReplyDecode),
            OutcomeUnknownCause::ReplyDecode,
        ));
        assert!(require_reply_turn(0, Some(0)).is_ok());
        assert!(matches!(
            require_reply_turn(1, Some(0)),
            Err(OutcomeUnknownCause::TurnMismatch {
                expected: 1,
                received: 0,
            }),
        ));
        assert!(matches!(
            require_reply_turn(0, None),
            Err(OutcomeUnknownCause::UnsolicitedMessage),
        ));
        assert!(matches!(
            post_dispatch_cause(ClientError::Handshake),
            OutcomeUnknownCause::UnsolicitedMessage,
        ));
        assert!(matches!(
            post_dispatch_cause(ClientError::Wire(WireError::Malformed)),
            OutcomeUnknownCause::Wire(WireError::Malformed),
        ));
        assert_eq!(
            OutcomeUnknownCause::TurnMismatch {
                expected: 1,
                received: 0,
            }
            .code(),
            Code::WireMalformed,
        );
        assert_eq!(
            OutcomeUnknownCause::UnsolicitedMessage.code(),
            Code::WireMalformed,
        );
        assert_eq!(
            OutcomeUnknownCause::ReplyDecode.code(),
            Code::RunnerReplyEncode,
        );
        // A before-send failure (a pre-dispatch handshake/connect error) classifies NotStarted,
        // the safe-to-consider-undone class, and never reaches the lost-reply path.
        assert_eq!(classify(HandoffStage::BeforeSend), LossClass::NotStarted);
    }
}
