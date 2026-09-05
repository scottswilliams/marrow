//! Durable execution over a lifecycle-owned attachment.
//!
//! The lifecycle prepares an image once and pairs it privately with the store it admitted
//! the image for; the VM never receives an image and a host separately. [`run_export`]
//! looks an export up in the attachment's own image, opens the one session that export's
//! verified demand requires on the attachment's own host, and runs it. [`run_test`] runs a
//! fresh source test the same way over the store the lifecycle minted for it, or with no
//! session for a storeless entry.
//!
//! ```compile_fail
//! fn foreign_pair(
//!     image: &marrow_verify::VerifiedImage,
//!     attachment: &mut marrow_lifecycle::NativeAttachment,
//!     export: marrow_image::ExportId,
//! ) {
//!     let _ = marrow_vm::run_export(image, attachment, export, Vec::new());
//! }
//! ```
//!
//! ```compile_fail
//! fn bare_host(
//!     image: &marrow_verify::VerifiedImage,
//!     host: &mut marrow_kernel::durable::EphemeralAttachment,
//!     export: marrow_image::ExportId,
//! ) {
//!     let _ = marrow_vm::run_export(host, export, Vec::new());
//! }
//! ```

use marrow_kernel::durable::{DemandCoverage, InvocationGrant, SessionHost};
use marrow_lifecycle::{Attachment, FreshTest, TestHost};
use marrow_verify::{ExportDemand, ExportId, FunctionIndex, TestKind, VerifiedImage};

use crate::fault::{DurableExecutionFault, RuntimeFault};
use crate::run::{DriverDispatch, run, run_driver, run_durable, run_in_session};
use crate::value::Value;

/// The outcome of one durable invocation through an attachment or a fresh test.
pub enum DurableRun {
    /// The invocation ran; the inner result is its VM value or source-mapped fault.
    Ran(Result<Option<Value>, DurableExecutionFault>),
    /// The image's durable shape is not yet executable by the flat kernel (a singleton
    /// root, a group nested below a branch, or a nominal-typed field). Composite keys and
    /// field-only keyed branches nested to any depth are executable.
    Parked,
    /// Minting the store failed operationally; the stable code names why.
    Failed(&'static str),
}

/// Run the attachment's own export `export` with `args`, opening the session its verified
/// demand requires on the attachment's host: a transaction session for a mutating export
/// (whose own `transaction` region commits the staged writes) and a read session for a
/// read-only one, both under a full grant; a storeless export runs with no session. Because
/// the host persists across calls, a mutating export's committed writes are visible to a
/// later invocation on the same attachment, and a mutating export that faults before its
/// commit leaves the store unchanged. `None` when the image carries no export with that
/// identity, decided before any session opens.
pub fn run_export<H: SessionHost>(
    attachment: &mut Attachment<H>,
    export: ExportId,
    args: Vec<Value>,
) -> Option<DurableRun> {
    let (image, host) = attachment.bridge();
    let export = image.export_by_id(export)?;
    Some(run_on_host(
        image,
        export.function(),
        export.demand(),
        args,
        host,
    ))
}

/// Run one fresh source test: a storeless entry with no session, a direct-durable entry
/// against one harness session over its own fresh store, and a driver entry against that
/// store with each export call it makes as its own invocation boundary (see
/// [`TestDriver`]). Kind and demand come from the entry in the test's own image.
pub fn run_test(mut test: FreshTest) -> DurableRun {
    let execution = test.execution();
    let image = execution.image;
    let entry = execution.entry;
    let host = match execution.host {
        TestHost::Storeless => {
            return DurableRun::Ran(
                run(image, entry.func(), Vec::new()).map_err(DurableExecutionFault::from),
            );
        }
        TestHost::Ready(host) => host,
        TestHost::Parked => return DurableRun::Parked,
        TestHost::Failed(cause) => return DurableRun::Failed(cause),
    };
    match entry.kind() {
        // A storeless entry is never minted a store; the arm is total over the kind.
        TestKind::Storeless => DurableRun::Ran(
            run(image, entry.func(), Vec::new()).map_err(DurableExecutionFault::from),
        ),
        TestKind::DirectDurable => {
            run_on_host(image, entry.func(), entry.demand(), Vec::new(), host)
        }
        TestKind::Driver => {
            let mut driver = TestDriver { image, host };
            DurableRun::Ran(run_driver(image, entry.func(), Vec::new(), &mut driver))
        }
    }
}

/// Open the session `demand` requires on `host` and run `func` in it. A mutating demand
/// drives a transaction session (which also reads); a read-only demand drives a read
/// session, so a read-only invocation never opens a writer; an empty demand needs no
/// session.
fn run_on_host<H: SessionHost + ?Sized>(
    image: &VerifiedImage,
    func: FunctionIndex,
    demand: &ExportDemand,
    args: Vec<Value>,
    host: &mut H,
) -> DurableRun {
    if demand.is_empty() {
        return DurableRun::Ran(run(image, func, args).map_err(DurableExecutionFault::from));
    }
    let grant = InvocationGrant::full_store();
    let coverage = coverage(demand);
    let result = if coverage.write {
        match host.txn_session(grant, coverage) {
            Ok(mut session) => run_durable(image, func, args, &mut session),
            Err(_) => {
                return DurableRun::Failed(marrow_codes::Code::CliDurableUnsupported.as_str());
            }
        }
    } else {
        match host.read_session(grant, coverage) {
            Ok(mut session) => run_durable(image, func, args, &mut session),
            Err(_) => {
                return DurableRun::Failed(marrow_codes::Code::CliDurableUnsupported.as_str());
            }
        }
    };
    DurableRun::Ran(result)
}

fn coverage(demand: &ExportDemand) -> DemandCoverage {
    DemandCoverage {
        read: demand.reads(),
        write: demand.writes(),
    }
}

/// The invocation dispatcher for a driver test body: it owns the test's one store and turns
/// each call the driver frame makes into its own session.
struct TestDriver<'a, H: SessionHost + ?Sized> {
    image: &'a VerifiedImage,
    host: &'a mut H,
}

impl<H: SessionHost + ?Sized> DriverDispatch for TestDriver<'_, H> {
    fn invoke(
        &mut self,
        func: FunctionIndex,
        args: Vec<Value>,
        depth: u32,
        budget: &mut u64,
    ) -> Result<Option<Value>, DurableExecutionFault> {
        let demand = self.image.function_demand(func);
        // A storeless callee needs no session.
        if demand.is_empty() {
            return run_in_session(self.image, func, args, depth, budget, None);
        }
        let grant = InvocationGrant::full_store();
        let cover = coverage(demand);
        // A mutating call drives a transaction session (which also reads and, on its
        // own `TxnCommit`, commits to the store); a read-only call drives a read
        // session, so a read-only demand never opens a writer. Either session closes
        // when this invocation returns — a committed writer persists, a dropped one
        // rolls back — before the next call opens its own.
        if cover.write {
            match self.host.txn_session(grant, cover) {
                Ok(mut session) => {
                    run_in_session(self.image, func, args, depth, budget, Some(&mut session))
                }
                Err(_) => Err(session_open_fault(self.image, func)),
            }
        } else {
            match self.host.read_session(grant, cover) {
                Ok(mut session) => {
                    run_in_session(self.image, func, args, depth, budget, Some(&mut session))
                }
                Err(_) => Err(session_open_fault(self.image, func)),
            }
        }
    }
}

/// A driver invocation whose session could not open — the authority resolved against
/// the store's ceiling and the invocation grant refused it. The callee's demand is a
/// subset of the test-image union the ceiling is minted from, so a well-formed image
/// never reaches this; it is mapped to a source-positioned `run.authority` fault rather
/// than a panic.
fn session_open_fault(image: &VerifiedImage, func: FunctionIndex) -> DurableExecutionFault {
    let (line, column) = image.function(func).span_at(0).unwrap_or((1, 1));
    RuntimeFault::new(marrow_codes::Code::RunAuthority.as_str(), line, column).into()
}
