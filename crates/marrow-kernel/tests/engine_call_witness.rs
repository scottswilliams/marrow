//! Engine-call witness: a rejected authority check touches the engine zero times.
//!
//! The path kernel resolves effective authority (`demand ∩ ceiling ∩ grant`) before
//! the first engine call. This harness proves that operationally rather than by
//! reading the source: a counting engine wraps the in-memory backend and tallies
//! every store-access entry point (`read_view`, `begin`, `audit_integrity`). Opening
//! a session whose demand exceeds the ceiling returns `Denied` with the tally still
//! at zero; a permitted session reads the profile cell, so its tally is nonzero —
//! confirming the counter actually observes engine access.

use std::cell::Cell;
use std::rc::Rc;

use marrow_kernel::codec::value::ScalarKind;
use marrow_kernel::durable::{
    DemandCoverage, DurableStore, FieldSchema, InvocationGrant, SessionError, SiteSpec, SiteTarget,
    StoreSchema,
};
use marrow_store::{ByteEngine, MemoryEngine, StoreError};

/// A byte engine that counts every store-access call and delegates to an in-memory
/// backend. The count is the witness: it must stay zero across an authority
/// rejection, since the kernel resolves authority before any engine access.
struct CountingEngine {
    inner: MemoryEngine,
    /// A shared tally the test reads independently of the store that owns the
    /// engine, so the witness survives the store consuming the engine.
    calls: Rc<Cell<usize>>,
}

impl CountingEngine {
    fn new(calls: Rc<Cell<usize>>) -> Self {
        Self {
            inner: MemoryEngine::new(),
            calls,
        }
    }

    fn count(&self) {
        self.calls.set(self.calls.get() + 1);
    }
}

impl ByteEngine for CountingEngine {
    type View<'a> = <MemoryEngine as ByteEngine>::View<'a>;
    type Txn<'a> = <MemoryEngine as ByteEngine>::Txn<'a>;

    fn read_view(&self) -> Result<Self::View<'_>, StoreError> {
        self.count();
        self.inner.read_view()
    }

    fn begin(&mut self) -> Result<Self::Txn<'_>, StoreError> {
        self.count();
        self.inner.begin()
    }

    fn require_write_access(&self, op: &'static str) -> Result<(), StoreError> {
        // A pure capability probe with no store I/O; not an engine access, so it is
        // deliberately uncounted.
        self.inner.require_write_access(op)
    }

    fn audit_integrity(&mut self) -> Result<(), StoreError> {
        self.count();
        self.inner.audit_integrity()
    }
}

fn schema() -> StoreSchema {
    StoreSchema {
        root_name: "counters".into(),
        key: ScalarKind::Int,
        fields: vec![FieldSchema {
            name: "value".into(),
            kind: ScalarKind::Int,
            required: true,
        }],
    }
}

fn sites() -> Vec<SiteSpec> {
    vec![SiteSpec {
        target: SiteTarget::WholePayload,
    }]
}

fn read_only_ceiling() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: false,
    }
}

fn writing_demand() -> DemandCoverage {
    DemandCoverage {
        read: true,
        write: true,
    }
}

/// A writing demand under a read-only ceiling is denied at the transaction-session
/// open, and the engine is never touched.
#[test]
fn a_denied_transaction_open_makes_zero_engine_calls() {
    let calls = Rc::new(Cell::new(0));
    let mut store = DurableStore::from_engine_with_ceiling(
        CountingEngine::new(calls.clone()),
        schema(),
        sites(),
        read_only_ceiling(),
    );
    let denied = store.txn_session(InvocationGrant::full_store(), writing_demand());
    assert!(matches!(denied, Err(SessionError::Denied)));
    assert_eq!(
        calls.get(),
        0,
        "a denied authority check must touch the engine zero times",
    );
}

/// A read demand denied by a no-read grant is refused before the read view opens.
#[test]
fn a_denied_read_open_makes_zero_engine_calls() {
    let calls = Rc::new(Cell::new(0));
    let mut store = DurableStore::from_engine_with_ceiling(
        CountingEngine::new(calls.clone()),
        schema(),
        sites(),
        DemandCoverage {
            read: true,
            write: false,
        },
    );
    let no_read_grant = InvocationGrant {
        read: false,
        write: false,
    };
    let denied = store.read_session(
        no_read_grant,
        DemandCoverage {
            read: true,
            write: false,
        },
    );
    assert!(matches!(denied, Err(SessionError::Denied)));
    assert_eq!(
        calls.get(),
        0,
        "a denied read must touch the engine zero times",
    );
}

/// The witness is real: a permitted session reads the profile cell, so it makes a
/// nonzero number of engine calls. Without this a broken counter would pass the
/// zero-call assertions vacuously.
#[test]
fn a_permitted_open_makes_a_nonzero_number_of_engine_calls() {
    let calls = Rc::new(Cell::new(0));
    let mut store = DurableStore::from_engine_with_ceiling(
        CountingEngine::new(calls.clone()),
        schema(),
        sites(),
        read_only_ceiling(),
    );
    let read = store.read_session(
        InvocationGrant::full_store(),
        DemandCoverage {
            read: true,
            write: false,
        },
    );
    assert!(read.is_ok());
    drop(read);
    assert!(
        calls.get() > 0,
        "a permitted read reads the profile cell, so the counter must observe it",
    );
}
