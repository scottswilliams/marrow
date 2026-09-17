//! The Marrow runner: invocation, admission, handoff, and classification for exports over the
//! local wire.
//!
//! The runner owns the server side of the supervised local channel ([`channel`]):
//! it binds a private Unix listener, admits one authenticated client (proving a
//! launch nonce, proving a session token back), and serves that client's requests
//! serially against a launched [`VerifiedImage`](marrow_verify::VerifiedImage). It
//! consumes the verifier and the VM through their public APIs to execute exports; it
//! never compiles source. It serves an image two ways, a structurally distinct launch each:
//! the storeless [`Service`] opens no store and runs only an image's storeless exports, and
//! the native [`AttachedService`] binds a persistent store through the privileged lifecycle
//! actor. Every handler resolves a request through one export classifier ([`dispatch`]).
//!
//! The wire grammar, framing, limits, and canonical JSON are the pure
//! [`marrow_local_wire`] crate's; this crate adds the process/socket discipline, the
//! transfer codec between wire JSON and runtime values, and export dispatch. The
//! long-lived attached-session mode is never named `serve` in the product surface.

mod attach;
mod channel;
mod client;
mod descriptor;
mod dispatch;
mod execute;
mod refusal;
mod staging;
mod terminal;
mod transfer;

pub use attach::AttachedService;
pub use channel::{AcceptError, Channel, Connection, Deadlines, Handler, LaunchSecrets, mint_id};
pub use client::{AttachCompletion, attach_and_call};
pub use descriptor::Service;
pub use marrow_local_wire::{Id32, Json, write_json_string};
pub use refusal::RefusalService;
pub use staging::{StagedImage, stage_image};
pub use terminal::{
    CallOutcome, CauseKind, ClientError, CompanionCleanupError, CompanionStartupError, Direction,
    OutcomeUnknownCause,
};
