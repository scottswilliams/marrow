//! The pending journal: the claim/append/replay protocol and the cooperative cache lock it
//! rests on.

#[path = "journal/cache_lock.rs"]
mod cache_lock;
#[path = "journal/lifecycle.rs"]
mod lifecycle;
