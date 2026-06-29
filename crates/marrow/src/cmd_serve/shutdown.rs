//! Unix signal handling for the foreground surface server.
//!
//! `serve --write` holds the native store handle for its process lifetime. The first
//! SIGINT or SIGTERM asks the server loop to return so that handle drops on the normal
//! stack; a second termination signal exits immediately if shutdown is stuck somewhere
//! outside the polling loop.

#[cfg(unix)]
pub(super) use unix::{Shutdown, install};

#[cfg(not(unix))]
pub(super) use fallback::{Shutdown, install};

#[cfg(unix)]
mod unix {
    use std::io;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::flag;

    pub(in crate::cmd_serve) struct Shutdown {
        signal: Arc<AtomicUsize>,
        _force: Arc<AtomicBool>,
    }

    impl Shutdown {
        pub(in crate::cmd_serve) fn requested(&self) -> Option<i32> {
            match self.signal.load(Ordering::SeqCst) {
                0 => None,
                signal => Some(signal as i32),
            }
        }
    }

    pub(in crate::cmd_serve) fn install() -> io::Result<Shutdown> {
        let signal = Arc::new(AtomicUsize::new(0));
        let force = Arc::new(AtomicBool::new(false));
        for signum in [SIGINT, SIGTERM] {
            flag::register_conditional_shutdown(signum, 128 + signum, Arc::clone(&force))?;
            flag::register_usize(signum, Arc::clone(&signal), signum as usize)?;
            flag::register(signum, Arc::clone(&force))?;
        }
        Ok(Shutdown {
            signal,
            _force: force,
        })
    }
}

#[cfg(not(unix))]
mod fallback {
    use std::io;

    pub(in crate::cmd_serve) struct Shutdown;

    impl Shutdown {
        pub(in crate::cmd_serve) fn requested(&self) -> Option<i32> {
            None
        }
    }

    pub(in crate::cmd_serve) fn install() -> io::Result<Shutdown> {
        Ok(Shutdown)
    }
}
