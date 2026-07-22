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

use marrow_local_wire::{ClientMessage, Id32, Json};
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
    let mut stream = connect_and_handshake(descriptor, nonce, deadline)?;
    write_message(
        &mut stream,
        &ClientMessage::Request {
            export: Id32::from_bytes(export_id),
            args,
        },
        deadline,
    )?;
    reply_to_outcome(image, export_id, read_message(&mut stream, deadline)?)
}
