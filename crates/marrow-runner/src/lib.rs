//! The Marrow runner: invocation, admission, handoff, and classification for
//! storeless exports over the local wire.
//!
//! The runner owns the server side of the supervised local channel ([`channel`]):
//! it binds a private Unix listener, admits one authenticated client (proving a
//! launch nonce, proving a session token back), and serves that client's requests
//! serially against a launched [`VerifiedImage`](marrow_verify::VerifiedImage). It
//! consumes the verifier and the VM through their public APIs to execute exports; it
//! never compiles source and never opens a store. Durable execution is parked in the
//! trough (campaign law 5): a request naming a durable export is rejected until the
//! ephemeral-memory attachment (G02b) and the native companion path (F02) land.
//!
//! The wire grammar, framing, limits, and canonical JSON are the pure
//! [`marrow_local_wire`] crate's; this crate adds the process/socket discipline, the
//! transfer codec between wire JSON and runtime values, and export dispatch. The
//! long-lived attached-session mode is never named `serve` in the product surface.

mod channel;
mod descriptor;
mod execute;
mod transfer;

pub use channel::{AcceptError, Channel, Connection, Deadlines, LaunchSecrets, mint_id};
pub use descriptor::{Service, interface_of};
pub use marrow_local_wire::Id32;
