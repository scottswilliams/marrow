//! The lifecycle crate's shared corpus: one compiled single-field store and the fixture
//! helpers its in-crate suites present images against.
//!
//! One populated single-field store is enough for admission, recovery and apply: each suite
//! varies the *image* it presents, not the corpus. Keeping the fixture here means one
//! temporary directory owner and one identity ledger, so a suite cannot silently disagree
//! with another about what a populated store contains. The scratch directory and the
//! compile helpers are the same files the integration suites use.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use marrow_fs_journal::{CustodyError, CustodyOp};
use marrow_verify::VerifiedImage;

use crate::seam::{Event, Observer, Seam, Step};
use crate::store_dir::AdmittedStoreDir;
use crate::{
    EngineKind, LogicalHead, ProvisionRequest, StoreEnvelope, StoreInstanceId, accepted_ceiling,
    active_binding, head_map, prepare,
};

#[path = "../tests/support/compile.rs"]
pub(crate) mod compile;
#[path = "../tests/support/scratch.rs"]
mod scratch;

pub(crate) use scratch::Scratch;

pub(crate) const SOURCE: &str = "resource Counter { required value: int }\nstore ^counters[id: int]: Counter\npub fn readValue(n: int): int { return ^counters[n].value ?? 0 }\n";
pub(crate) const IDS: &str = "marrow ids v0\nmachine-written by marrow; do not edit\nid application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\nid product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\nid field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\nid root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\nid key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\nhigh-water 0\nend\n";

/// The image bytes of `source` under the corpus ledger.
pub(crate) fn compile_bytes(source: &str) -> Vec<u8> {
    compile::compile_bytes(source, IDS)
}

pub(crate) fn request(image: &VerifiedImage, instance: StoreInstanceId) -> ProvisionRequest {
    ProvisionRequest {
        envelope: StoreEnvelope {
            instance,
            writer_toolchain: "0.1.0".into(),
            engine_kind: EngineKind::Redb,
            engine_format_version: marrow_kernel::durable::NATIVE_ENGINE_FORMAT_VERSION,
        },
        head: LogicalHead::provision(
            active_binding(image),
            accepted_ceiling(image),
            head_map(image).expect("head map"),
        ),
    }
}

pub(crate) fn populate_counter(dir: &Path, image: &VerifiedImage) {
    use marrow_kernel::codec::{key::KeyScalar, value::RuntimeScalar};
    use marrow_kernel::durable::{DemandCoverage, Durable, EntryValue, InvocationGrant};
    use marrow_kernel::equality::ValueDomain;

    let write_site = image
        .sites()
        .iter()
        .position(|site| {
            matches!(
                site,
                marrow_verify::SealedSite::Flat {
                    root: 0,
                    target: marrow_verify::SealedSiteTarget::WholePayload,
                }
            )
        })
        .expect("compiled entry write") as u16;
    let crate::AttachOutcome::AlreadyActive(mut attachment) =
        crate::attach(dir, prepare(image.clone())).expect("attach")
    else {
        panic!("initial binding");
    };
    let (_, host) = attachment.bridge();
    let mut txn = host
        .txn_session(
            InvocationGrant::full_store(),
            DemandCoverage {
                read: true,
                write: true,
            },
        )
        .expect("transaction");
    let site = txn.site(write_site);
    txn.create_entry(
        &site,
        &[KeyScalar::Int(7)],
        EntryValue {
            fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(42)))],
            groups: Vec::new(),
        },
    )
    .expect("write populated entry");
    assert!(matches!(
        txn.commit(),
        marrow_kernel::durable::CommitResult::Committed
    ));
}

/// An observer that acts on the first event `select` accepts and passes every other one.
struct Once<S, H> {
    select: S,
    hit: Mutex<Option<H>>,
    fired: Arc<AtomicBool>,
}

impl<S, H> Observer for Once<S, H>
where
    S: Fn(&Event<'_>) -> bool + Send + Sync,
    H: FnOnce(Event<'_>) -> Result<(), CustodyError> + Send,
{
    fn at(&self, event: Event<'_>) -> Result<(), CustodyError> {
        if !(self.select)(&event) {
            return Ok(());
        }
        // The guard is released before `hit` runs: a mutation may re-enter this seam.
        let hit = self.hit.lock().expect("armed hit").take();
        let Some(hit) = hit else {
            return Ok(());
        };
        self.fired.store(true, Ordering::SeqCst);
        hit(event)
    }
}

/// Whether an armed seam reached its event.
pub(crate) struct Reached(Arc<AtomicBool>);

impl Reached {
    pub(crate) fn assert(&self) {
        assert!(
            self.0.load(Ordering::SeqCst),
            "the armed checkpoint was not reached"
        );
    }
}

/// A seam that runs `hit` at the first event `select` accepts; later events pass.
pub(crate) fn once(
    select: impl Fn(&Event<'_>) -> bool + Send + Sync + 'static,
    hit: impl FnOnce(Event<'_>) -> Result<(), CustodyError> + Send + 'static,
) -> (Seam, Reached) {
    let fired = Arc::new(AtomicBool::new(false));
    let observer = Once {
        select,
        hit: Mutex::new(Some(hit)),
        fired: Arc::clone(&fired),
    };
    (Seam::armed(Arc::new(observer)), Reached(fired))
}

/// The I/O refusal a cut step's operation reports.
pub(crate) fn io_fault(op: CustodyOp) -> CustodyError {
    CustodyError::Io {
        op,
        source: std::io::Error::from(std::io::ErrorKind::Other),
    }
}

fn is_step(event: &Event<'_>, step: Step) -> bool {
    matches!(event, Event::Step { step: at, .. } if *at == step)
}

/// A seam that fails the first `step` with the refusal its operation reports.
pub(crate) fn cut(step: Step) -> Seam {
    let op = match step {
        Step::Append(_) => CustodyOp::Append,
        _ => CustodyOp::Sync,
    };
    once(
        move |event| is_step(event, step),
        move |_| Err(io_fault(op)),
    )
    .0
}

/// A seam that fails the publication's parent sync.
pub(crate) fn cut_parent_sync() -> Seam {
    once(
        |event| matches!(event, Event::ParentSync),
        |_| Err(io_fault(CustodyOp::Sync)),
    )
    .0
}

/// A seam that runs `mutation` over the directory when the sequence first reaches `step`.
pub(crate) fn mutate_at(
    step: Step,
    mutation: impl FnOnce(&AdmittedStoreDir) + Send + 'static,
) -> (Seam, Reached) {
    once(
        move |event| is_step(event, step),
        move |event| {
            let Event::Step { dir, .. } = event else {
                unreachable!("selected a step event");
            };
            mutation(dir);
            Ok(())
        },
    )
}
