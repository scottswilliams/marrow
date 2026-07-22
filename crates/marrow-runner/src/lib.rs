//! The Marrow runner: invocation, admission, handoff, and classification for exports over the
//! local wire.
//!
//! The runner owns the server side of the supervised local channel ([`channel`]):
//! it binds a private Unix listener, admits one authenticated client (proving a
//! launch nonce, proving a session token back), and serves that client's requests
//! serially against a launched [`VerifiedImage`](marrow_verify::VerifiedImage). It
//! consumes the verifier and the VM through their public APIs to execute exports; it
//! never compiles source. It serves an image three ways, a structurally distinct launch each:
//! the storeless [`Service`] opens no store and runs only an image's storeless exports; the
//! native [`AttachedService`] binds a persistent store through the privileged lifecycle actor;
//! and the [`AttachedEphemeralService`] binds a fresh process-local in-memory store that never
//! outlives the session. The client half of the ephemeral path is [`EphemeralSession`], which
//! classifies a lost reply through [`LossClass`] and never replays it.
//!
//! The wire grammar, framing, limits, and canonical JSON are the pure
//! [`marrow_local_wire`] crate's; this crate adds the process/socket discipline, the
//! transfer codec between wire JSON and runtime values, and export dispatch. The
//! long-lived attached-session mode is never named `serve` in the product surface.

mod attach;
mod attach_ephemeral;
mod channel;
mod client;
mod descriptor;
mod dispatch;
mod ephemeral_client;
mod execute;
mod refusal;
mod terminal;
mod transfer;

pub use attach::AttachedService;
pub use attach_ephemeral::AttachedEphemeralService;
pub use channel::{AcceptError, Channel, Connection, Deadlines, Handler, LaunchSecrets, mint_id};
pub use client::attach_and_call;
pub use descriptor::{Service, interface_of};
pub use ephemeral_client::{EphemeralCall, EphemeralSession};
pub use marrow_local_wire::{DurableState, Id32, Json, LossClass};
pub use refusal::RefusalService;
pub use terminal::{CallOutcome, ClientError};
