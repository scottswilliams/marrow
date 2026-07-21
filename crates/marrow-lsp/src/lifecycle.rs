//! The server lifecycle state machine.
//!
//! The exact phase order is
//! `AwaitInitialize → InitializeReplyPending → AwaitInitialized → Running →
//! ShutdownReplyPending → AwaitExit`, with a first-wins terminal. This module owns the
//! phase transitions and the phase-gating of every inbound method: which requests are
//! answered, which get `-32002` (server not initialized) or `-32600` (invalid in
//! state), and which notifications are discarded. It performs no I/O and reads no
//! document or semantic state; the coordinator drives it and applies its decisions.

/// The lifecycle phase.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// No `initialize` accepted yet.
    AwaitInitialize,
    /// `initialize` accepted; its response has not been delivered. `initialized` may be
    /// latched but cannot advance the lifecycle before the response is delivered.
    InitializeReplyPending {
        /// Whether a valid `initialized` arrived while the response was pending.
        initialized_latched: bool,
    },
    /// The initialize response was delivered without a latched `initialized`; awaiting
    /// the `initialized` notification.
    AwaitInitialized,
    /// Normal operation: documents and semantic requests are served.
    Running,
    /// `shutdown` accepted; its response has not been delivered. Every later request is
    /// `-32600`; only a valid `exit` advances.
    ShutdownReplyPending,
    /// `shutdown` response delivered; awaiting `exit`.
    AwaitExit,
}

/// What the coordinator should do with an inbound request in the current phase, before
/// method-specific routing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RequestGate {
    /// Route to the method handler (the request is admissible in this phase).
    Route,
    /// Reply `-32002`: the server is not initialized.
    NotInitialized,
    /// Reply `-32600`: the request is invalid in this phase (a retried `initialize`, or
    /// any request after `shutdown`).
    InvalidInPhase,
}

/// The JSON-RPC error codes the lifecycle emits.
pub const SERVER_NOT_INITIALIZED: i32 = -32002;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const CONTENT_MODIFIED: i32 = -32801;
pub const REQUEST_FAILED: i32 = -32803;
pub const INTERNAL_ERROR: i32 = -32603;
pub const PARSE_ERROR: i32 = -32700;

/// The lifecycle owner.
pub struct Lifecycle {
    phase: Phase,
    /// Set once a first-wins terminal (EOF, exit, or producer fault) occurs.
    terminal: bool,
}

impl Lifecycle {
    /// A fresh lifecycle awaiting `initialize`.
    pub fn new() -> Self {
        Self {
            phase: Phase::AwaitInitialize,
            terminal: false,
        }
    }

    /// The current phase.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Whether a terminal has been reached.
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// Gate an ordinary (non-lifecycle) request by phase. Lifecycle methods
    /// (`initialize`, `shutdown`) are handled by their own methods, not here.
    pub fn gate_request(&self) -> RequestGate {
        match self.phase {
            Phase::Running => RequestGate::Route,
            Phase::AwaitInitialize
            | Phase::InitializeReplyPending { .. }
            | Phase::AwaitInitialized => RequestGate::NotInitialized,
            Phase::ShutdownReplyPending | Phase::AwaitExit => RequestGate::InvalidInPhase,
        }
    }

    /// Handle a valid, well-formed `initialize` request. Returns whether it is accepted
    /// (moving to `InitializeReplyPending`) or rejected as invalid-in-phase (`-32600`
    /// for a retried initialize).
    pub fn on_initialize(&mut self) -> RequestGate {
        match self.phase {
            Phase::AwaitInitialize => {
                self.phase = Phase::InitializeReplyPending {
                    initialized_latched: false,
                };
                RequestGate::Route
            }
            // A later `initialize` request is always `-32600`.
            _ => RequestGate::InvalidInPhase,
        }
    }

    /// Record delivery of the `initialize` response. Advances to `Running` when
    /// `initialized` was already latched, otherwise to `AwaitInitialized`. Returns
    /// whether the server entered `Running` (so the coordinator enqueues the first
    /// analysis).
    pub fn on_initialize_delivered(&mut self) -> bool {
        if let Phase::InitializeReplyPending {
            initialized_latched,
        } = self.phase
        {
            if initialized_latched {
                self.phase = Phase::Running;
                true
            } else {
                self.phase = Phase::AwaitInitialized;
                false
            }
        } else {
            false
        }
    }

    /// Handle a valid `initialized` notification. Returns whether the server entered
    /// `Running` (so the coordinator enqueues the first analysis). An early, duplicate,
    /// or late `initialized` is ignored.
    pub fn on_initialized(&mut self) -> bool {
        match self.phase {
            Phase::InitializeReplyPending {
                initialized_latched: false,
            } => {
                // Latch it; the lifecycle advances only when the response is delivered.
                self.phase = Phase::InitializeReplyPending {
                    initialized_latched: true,
                };
                false
            }
            Phase::AwaitInitialized => {
                self.phase = Phase::Running;
                true
            }
            // Early (before initialize accepted), duplicate, or post-Running: discard.
            _ => false,
        }
    }

    /// Handle a valid `shutdown` request. Returns whether it is accepted (moving to
    /// `ShutdownReplyPending`) or invalid-in-phase.
    pub fn on_shutdown(&mut self) -> RequestGate {
        match self.phase {
            Phase::Running => {
                self.phase = Phase::ShutdownReplyPending;
                RequestGate::Route
            }
            Phase::AwaitInitialize
            | Phase::InitializeReplyPending { .. }
            | Phase::AwaitInitialized => RequestGate::NotInitialized,
            // A second shutdown, or shutdown after shutdown, is `-32600`.
            Phase::ShutdownReplyPending | Phase::AwaitExit => RequestGate::InvalidInPhase,
        }
    }

    /// Record delivery of the `shutdown` response, moving to `AwaitExit`.
    pub fn on_shutdown_delivered(&mut self) {
        if self.phase == Phase::ShutdownReplyPending {
            self.phase = Phase::AwaitExit;
        }
    }

    /// The process exit code for a valid `exit` notification: `0` after an accepted
    /// shutdown, `1` before. Marks the terminal.
    pub fn on_exit(&mut self) -> u8 {
        self.terminal = true;
        match self.phase {
            Phase::ShutdownReplyPending | Phase::AwaitExit => 0,
            _ => 1,
        }
    }

    /// Mark a first-wins terminal from EOF or a producer fault (no exit): exit code `1`.
    pub fn on_terminal(&mut self) -> u8 {
        self.terminal = true;
        1
    }
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_initialize_then_initialized_then_running() {
        let mut life = Lifecycle::new();
        assert_eq!(life.gate_request(), RequestGate::NotInitialized);
        assert_eq!(life.on_initialize(), RequestGate::Route);
        // initialized arrives before the response is delivered: latched, not advanced.
        assert!(!life.on_initialized());
        assert_eq!(
            life.phase(),
            Phase::InitializeReplyPending {
                initialized_latched: true
            }
        );
        // Delivering the response with a latched initialized enters Running.
        assert!(life.on_initialize_delivered());
        assert_eq!(life.phase(), Phase::Running);
        assert_eq!(life.gate_request(), RequestGate::Route);
    }

    #[test]
    fn initialized_after_delivery_enters_running() {
        let mut life = Lifecycle::new();
        life.on_initialize();
        assert!(!life.on_initialize_delivered());
        assert_eq!(life.phase(), Phase::AwaitInitialized);
        assert_eq!(life.gate_request(), RequestGate::NotInitialized);
        assert!(life.on_initialized());
        assert_eq!(life.phase(), Phase::Running);
    }

    #[test]
    fn requests_before_running_are_not_initialized() {
        let mut life = Lifecycle::new();
        assert_eq!(life.gate_request(), RequestGate::NotInitialized);
        life.on_initialize();
        assert_eq!(life.gate_request(), RequestGate::NotInitialized);
    }

    #[test]
    fn retried_initialize_is_invalid() {
        let mut life = Lifecycle::new();
        assert_eq!(life.on_initialize(), RequestGate::Route);
        assert_eq!(life.on_initialize(), RequestGate::InvalidInPhase);
    }

    #[test]
    fn early_and_duplicate_initialized_are_ignored() {
        let mut life = Lifecycle::new();
        // Early: before initialize.
        assert!(!life.on_initialized());
        assert_eq!(life.phase(), Phase::AwaitInitialize);
        life.on_initialize();
        life.on_initialize_delivered();
        assert!(life.on_initialized());
        // Duplicate after Running: ignored, stays Running.
        assert!(!life.on_initialized());
        assert_eq!(life.phase(), Phase::Running);
    }

    #[test]
    fn shutdown_then_requests_are_invalid_and_exit_is_zero() {
        let mut life = Lifecycle::new();
        life.on_initialize();
        life.on_initialize_delivered();
        life.on_initialized();
        assert_eq!(life.on_shutdown(), RequestGate::Route);
        assert_eq!(life.gate_request(), RequestGate::InvalidInPhase);
        life.on_shutdown_delivered();
        assert_eq!(life.phase(), Phase::AwaitExit);
        assert_eq!(life.on_exit(), 0);
        assert!(life.is_terminal());
    }

    #[test]
    fn shutdown_before_running_is_not_initialized() {
        let mut life = Lifecycle::new();
        assert_eq!(life.on_shutdown(), RequestGate::NotInitialized);
    }

    #[test]
    fn exit_before_shutdown_is_nonzero() {
        let mut life = Lifecycle::new();
        life.on_initialize();
        life.on_initialize_delivered();
        life.on_initialized();
        assert_eq!(life.on_exit(), 1);
    }

    #[test]
    fn eof_terminal_is_nonzero() {
        let mut life = Lifecycle::new();
        assert_eq!(life.on_terminal(), 1);
        assert!(life.is_terminal());
    }
}
