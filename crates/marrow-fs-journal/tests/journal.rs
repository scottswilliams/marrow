//! The pending journal: the claim/append/replay protocol, the cooperative cache lock it
//! rests on, and the frame format both encode and decode.

mod common;

#[path = "journal/cache_lock.rs"]
mod cache_lock;
#[path = "journal/frame_codec.rs"]
mod frame_codec;
#[path = "journal/lifecycle.rs"]
mod lifecycle;
