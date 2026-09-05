//! The image/store attachment: the one capability that pairs a verified image with the
//! store lifecycle admitted it for.
//!
//! [`prepare`] derives an image's store projection once and retains the image behind a
//! shared handle. Every route that opens an engine consumes that preparation — the native
//! attach/rebind actor (`crate::attach`), the persistent provision and import, and the
//! in-memory mint here — so no caller supplies an image and a store shape separately, and no
//! caller pairs an image with a host it did not admit for that image. The result of a native
//! or memory pairing is an [`Attachment`]: the retained image and its host behind private
//! fields, constructed only by this crate. The VM borrows both through
//! [`Attachment::bridge`] and executes the retained image's own exports against the host.
//!
//! A source test is minted the same way ([`fresh_test`]): the entry is checked against the
//! retained image before any store is minted, a storeless entry gets no store at all, and the
//! VM runs the owned image's own entry through [`FreshTest::execution`].
//!
//! The pairing is private: the fields are not public, no constructor takes a caller's host or
//! image, and the bridge hands out only an unsized host reference, so the concrete host
//! cannot be replaced.
//!
//! ```compile_fail
//! use std::rc::Rc;
//! fn forge(
//!     image: Rc<marrow_verify::VerifiedImage>,
//!     host: Box<marrow_kernel::durable::EphemeralAttachment>,
//! ) -> marrow_lifecycle::MemoryAttachment {
//!     marrow_lifecycle::Attachment { image, host }
//! }
//! ```
//!
//! ```compile_fail
//! fn replace_host(
//!     attachment: &mut marrow_lifecycle::MemoryAttachment,
//!     other: Box<marrow_kernel::durable::EphemeralAttachment>,
//! ) {
//!     let (_, host) = attachment.bridge();
//!     let _ = std::mem::replace(host, *other);
//! }
//! ```

use std::rc::Rc;

use marrow_kernel::durable::{
    CeilingIdToken, CommitRecovery, DemandCoverage, DeploymentCeiling, DurableCommitState,
    EphemeralAttachment, SessionHost, StoreProjection,
};
use marrow_verify::{CeilingDescriptor, ExportDemand, SealedTestEntry, TestKind, VerifiedImage};

use crate::envelope::StoreEnvelope;
use crate::head::LogicalHead;
use crate::image::derive_projection;
use crate::provision::OpenStore;

/// The engine the in-memory attachment's sessions run over.
pub type MemoryEngine = <EphemeralAttachment as SessionHost>::Engine;

/// A verified image prepared for attachment: the image behind one shared handle and its
/// store projection, derived exactly once. The projection is `None` for a storeless image and
/// for a durable shape the flat kernel does not execute yet; a route that needs an engine
/// refuses such an image with its own typed outcome.
pub struct PreparedImage {
    image: Rc<VerifiedImage>,
    projection: Option<StoreProjection>,
}

/// Prepare `image` for attachment, deriving its store projection once.
pub fn prepare(image: VerifiedImage) -> PreparedImage {
    let projection = derive_projection(&image);
    PreparedImage {
        image: Rc::new(image),
        projection,
    }
}

impl PreparedImage {
    /// The retained image handle, for inspection; it derefs to the verified image.
    pub fn image(&self) -> &Rc<VerifiedImage> {
        &self.image
    }

    /// The store projection every route opens the image's engine under, for inspection:
    /// `None` for a storeless image or a durable shape the flat kernel does not execute.
    pub fn projection(&self) -> Option<&StoreProjection> {
        self.projection.as_ref()
    }

    pub(crate) fn into_parts(self) -> (Rc<VerifiedImage>, Option<StoreProjection>) {
        (self.image, self.projection)
    }
}

/// A verified image paired with the host lifecycle admitted it for. The pairing is the
/// capability the VM executes durable exports through; both halves are private and only this
/// crate's native and memory factories construct one.
pub struct Attachment<H: SessionHost> {
    image: Rc<VerifiedImage>,
    host: H,
}

/// The persistent pairing: the image and the open store the lifecycle actor admitted it
/// against, holding the store's single-owner lock.
pub type NativeAttachment = Attachment<OpenStore>;

/// The in-memory pairing: the image and a fresh process-local store minted from its own
/// projection. The host is boxed: an `EphemeralAttachment` measures 192 bytes on a 64-bit
/// target against the 8-byte image handle the other mint outcomes carry, so boxing keeps
/// every [`EphemeralOutcome`] arm and [`FreshTest`] state 16 bytes (pinned by
/// `tests::memory_attachment_size`).
pub type MemoryAttachment = Attachment<Box<EphemeralAttachment>>;

impl<H: SessionHost> Attachment<H> {
    pub(crate) fn new(image: Rc<VerifiedImage>, host: H) -> Self {
        Self { image, host }
    }

    /// The retained image.
    pub fn image(&self) -> &Rc<VerifiedImage> {
        &self.image
    }

    /// The execution seam: the retained image and its host, borrowed together. The host is an
    /// unsized reference, so a caller opens sessions on it and cannot replace it.
    pub fn bridge(&mut self) -> (&VerifiedImage, &mut dyn SessionHost<Engine = H::Engine>) {
        (&self.image, &mut self.host)
    }
}

impl NativeAttachment {
    /// The persisted envelope the open admitted.
    pub fn envelope(&self) -> &StoreEnvelope {
        &self.host.envelope
    }

    /// The persisted logical head the open admitted.
    pub fn head(&self) -> &LogicalHead {
        &self.host.head
    }

    /// Consume an indeterminate commit's recovery fact, carrying the same image through the
    /// store's reopen and audit. A known result returns the same pairing over the reopened
    /// store; unknown retires it. This substitutes no image and no store.
    pub fn resolve_recovery(self, recovery: CommitRecovery) -> (DurableCommitState, Option<Self>) {
        let Self { image, host } = self;
        let (state, host) = host.resolve_recovery(recovery);
        (state, host.map(|host| Self { image, host }))
    }
}

/// Whether an in-memory attachment could be minted for a prepared image. Every arm owns the
/// image, so a service over a parked or failed mint still runs the image's storeless exports.
pub enum EphemeralOutcome {
    /// The pairing over the image's executable durable shape.
    Ready(MemoryAttachment),
    /// The image's durable shape is not yet executable by the flat kernel.
    Parked(Rc<VerifiedImage>),
    /// Minting the store failed operationally; `cause` is the stable code.
    Failed {
        image: Rc<VerifiedImage>,
        cause: &'static str,
    },
}

impl EphemeralOutcome {
    /// The owned image, whatever the mint outcome.
    pub fn image(&self) -> &Rc<VerifiedImage> {
        match self {
            EphemeralOutcome::Ready(attachment) => attachment.image(),
            EphemeralOutcome::Parked(image) | EphemeralOutcome::Failed { image, .. } => image,
        }
    }
}

/// Mint one in-memory store for the prepared image's whole-program demand-union ceiling and
/// pair it with the image. The store serves every export invocation in sequence, so a
/// committed transaction is observable by a later read and a rolled-back one is not.
pub fn mint_ephemeral(prepared: PreparedImage) -> EphemeralOutcome {
    let (image, projection) = prepared.into_parts();
    let Some(projection) = projection else {
        return EphemeralOutcome::Parked(image);
    };
    let ceiling = deployment_ceiling(image.demand_union());
    match EphemeralAttachment::mint(projection, ceiling) {
        Ok(host) => EphemeralOutcome::Ready(Attachment::new(image, Box::new(host))),
        Err(_) => EphemeralOutcome::Failed {
            image,
            cause: marrow_codes::Code::CliDurableUnsupported.as_str(),
        },
    }
}

/// One source test selected from its own image: the retained image, the checked entry index,
/// and the store the entry's kind needs — none for a storeless entry, a fresh in-memory store
/// for a durable one. Runnable exactly once, through the VM.
pub struct FreshTest {
    image: Rc<VerifiedImage>,
    entry: usize,
    state: FreshTestState,
}

enum FreshTestState {
    Storeless,
    Ready(Box<EphemeralAttachment>),
    Parked,
    Failed(&'static str),
}

/// Select test entry `index` of the prepared image. `None` when the image has no such entry,
/// decided before any store is minted. A storeless entry clones only the image handle; a
/// durable entry mints its own fresh store from the prepared projection under the test-image
/// demand-union ceiling, parks when the shape is not executable, and reports an operational
/// mint failure by its stable code.
pub fn fresh_test(prepared: &PreparedImage, index: usize) -> Option<FreshTest> {
    let entry = prepared.image.test_entries().get(index)?;
    let state = match (entry.kind(), prepared.projection()) {
        (TestKind::Storeless, _) => FreshTestState::Storeless,
        (TestKind::DirectDurable | TestKind::Driver, None) => FreshTestState::Parked,
        (TestKind::DirectDurable | TestKind::Driver, Some(projection)) => {
            let ceiling = deployment_ceiling(prepared.image.test_demand_union());
            match EphemeralAttachment::mint(projection.clone(), ceiling) {
                Ok(host) => FreshTestState::Ready(Box::new(host)),
                Err(_) => {
                    FreshTestState::Failed(marrow_codes::Code::CliDurableUnsupported.as_str())
                }
            }
        }
    };
    Some(FreshTest {
        image: Rc::clone(&prepared.image),
        entry: index,
        state,
    })
}

/// The execution seam of a fresh test: the owned image, its selected entry, and the host the
/// entry runs against.
pub struct TestExecution<'a> {
    pub image: &'a VerifiedImage,
    pub entry: &'a SealedTestEntry,
    pub host: TestHost<'a>,
}

/// Where a fresh test runs; the entry's kind in the owned image says how.
pub enum TestHost<'a> {
    /// A storeless entry: no session.
    Storeless,
    /// A durable entry over its own fresh in-memory store.
    Ready(&'a mut dyn SessionHost<Engine = MemoryEngine>),
    /// A durable entry whose image shape the flat kernel does not execute yet.
    Parked,
    /// A durable entry whose store could not be minted; the stable code names why.
    Failed(&'static str),
}

impl FreshTest {
    /// Borrow the image, the entry, and the host together for execution.
    pub fn execution(&mut self) -> TestExecution<'_> {
        let entry = &self.image.test_entries()[self.entry];
        let host = match &mut self.state {
            FreshTestState::Storeless => TestHost::Storeless,
            FreshTestState::Ready(host) => TestHost::Ready(&mut **host),
            FreshTestState::Parked => TestHost::Parked,
            FreshTestState::Failed(cause) => TestHost::Failed(cause),
        };
        TestExecution {
            image: &self.image,
            entry,
            host,
        }
    }
}

/// The deployment ceiling a fresh in-memory store is bounded by, from a demand union. The
/// descriptor derives both the read/write coverage the kernel checks and the ceiling-id
/// binding token from the same verified atoms, so the ceiling is bound to the verified image
/// and never supplied independently.
fn deployment_ceiling(union: ExportDemand) -> DeploymentCeiling {
    let descriptor = CeilingDescriptor::from_demand_union(union);
    DeploymentCeiling::new(
        DemandCoverage {
            read: descriptor.reads(),
            write: descriptor.writes(),
        },
        CeilingIdToken::new(*descriptor.ceiling_id().bytes()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The measured sizes the boxing decision above rests on.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn memory_attachment_size() {
        assert_eq!(std::mem::size_of::<EphemeralAttachment>(), 192);
        assert_eq!(std::mem::size_of::<Rc<VerifiedImage>>(), 8);
        assert_eq!(std::mem::size_of::<MemoryAttachment>(), 16);
    }
}
