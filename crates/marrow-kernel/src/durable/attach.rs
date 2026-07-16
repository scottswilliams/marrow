//! The production ephemeral-memory attachment (E01).
//!
//! An attachment is a live binding between a verified program image and a durable
//! store the path kernel drives. The ephemeral-memory attachment is the first
//! production form: a fresh in-memory store, minted only from a derived
//! [`StoreSchema`] and [`SiteSpec`] table (which the executor derives from a
//! [`VerifiedImage`](marrow_verify::VerifiedImage), the sole source of a valid
//! schema) plus an explicit [`DeploymentCeiling`]. Each mint draws a nonforgeable
//! [`AttachmentId`] from process entropy, and each invocation opens its own session
//! bounded by `demand ∩ ceiling ∩ grant`.
//!
//! This is the T01 store session machinery generalized, not a second kernel: an
//! attachment owns exactly one [`DurableStore`] and delegates every session open to
//! it. The persistent native attachment (F02) reuses the same session machinery
//! over a durable engine.

use std::sync::atomic::{AtomicU64, Ordering};

use marrow_store::MemoryEngine;

use super::store::{DurableStore, ReadSession, TxnSession};
use super::{DemandCoverage, InvocationGrant, SessionError, SiteSpec, StoreSchema};

/// A nonforgeable attachment identity: 128 bits drawn from OS entropy when an
/// attachment is minted. It is a capability token — unguessable and constructible
/// only through [`AttachmentId::draw`] inside [`EphemeralAttachment::mint`] — never
/// derived from the image, the ceiling, or a wall clock, so a forged image or
/// ceiling cannot reproduce a live attachment's id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AttachmentId([u8; 16]);

impl AttachmentId {
    /// Draw a fresh identity from the OS entropy source. No clock, hash, counter, or
    /// retry: an entropy failure surfaces as [`AttachError::Entropy`] and no
    /// attachment is minted.
    fn draw() -> Result<Self, AttachError> {
        draw_entropy().map(Self)
    }

    /// The 16 identity bytes.
    pub fn bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// The opaque ceiling-id binding token: the 32 bytes minted by the image-side
/// ceiling-descriptor owner (`marrow_image::CeilingDescriptor`) over a verified
/// demand union. The kernel never recomputes or interprets it — it only records the
/// token and reports it back for equality, so the kernel stays free of any image
/// dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CeilingIdToken([u8; 32]);

impl CeilingIdToken {
    /// Wrap the 32 token bytes the image-side ceiling-descriptor owner minted.
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The 32 token bytes.
    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// The deployment ceiling an ephemeral attachment is bounded by: the read/write
/// coverage the path kernel admits before intersecting the per-invocation grant,
/// carried alongside the [`CeilingIdToken`] binding token minted by the image-side
/// ceiling-descriptor owner (`marrow_image::CeilingDescriptor`). The kernel treats
/// the id as opaque — it never recomputes it and needs no image dependency — but
/// records it so a minted attachment is bound to the exact ceiling it was minted
/// under. The attach path derives both the coverage and the id from the verified
/// image's demand union and binds them together, so widening the coverage would
/// change the id. Comparing the token against an independently supplied ceiling
/// arrives with the persistent native attachment (F02). The read/write coverage is
/// the projection the T01 store ceiling checks; a path-granular atom-level ceiling
/// is a later lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeploymentCeiling {
    coverage: DemandCoverage,
    ceiling_id: CeilingIdToken,
}

impl DeploymentCeiling {
    /// A ceiling from an explicit read/write coverage and the ceiling-id binding
    /// token. The attach path derives both from the verified image's demand union, so
    /// the coverage the kernel checks and the id it binds move together — widening the
    /// coverage would change the id.
    pub fn new(coverage: DemandCoverage, ceiling_id: CeilingIdToken) -> Self {
        Self {
            coverage,
            ceiling_id,
        }
    }

    /// The read/write coverage this ceiling admits.
    pub fn coverage(&self) -> DemandCoverage {
        self.coverage
    }

    /// The ceiling-id binding token this ceiling carries.
    pub fn ceiling_id(&self) -> CeilingIdToken {
        self.ceiling_id
    }
}

/// A failure to mint an ephemeral attachment, before any session opens.
#[derive(Debug)]
pub enum AttachError {
    /// The OS entropy source was unavailable, so no nonforgeable id could be drawn.
    Entropy(std::io::Error),
}

/// A production ephemeral-memory attachment: a fresh in-memory durable store bound
/// to one program image's schema, sites, and deployment ceiling, tagged with a
/// nonforgeable id. Owns exactly one [`DurableStore`]; every session delegates to
/// it. Dropping the attachment discards all its data.
pub struct EphemeralAttachment {
    id: AttachmentId,
    /// The ceiling-id binding token: the attachment is bound to the exact deployment
    /// ceiling it was minted under, so its recorded authority bound cannot be swapped
    /// for a wider ceiling after minting.
    ceiling_id: CeilingIdToken,
    store: DurableStore<MemoryEngine>,
}

impl EphemeralAttachment {
    /// Mint a fresh ephemeral attachment over an empty in-memory store. The schema
    /// and site table are the executor's derivation from a verified image; the
    /// ceiling bounds every session's authority. A fresh id is drawn from entropy
    /// before the store is built, so a mint either yields a fully-formed attachment
    /// or fails without one.
    pub fn mint(
        schema: StoreSchema,
        sites: Vec<SiteSpec>,
        ceiling: DeploymentCeiling,
    ) -> Result<Self, AttachError> {
        let id = AttachmentId::draw()?;
        let store = DurableStore::from_engine_with_ceiling(
            MemoryEngine::new(),
            schema,
            sites,
            ceiling.coverage(),
        );
        Ok(Self {
            id,
            ceiling_id: ceiling.ceiling_id(),
            store,
        })
    }

    /// This attachment's nonforgeable identity.
    pub fn id(&self) -> AttachmentId {
        self.id
    }

    /// The ceiling-id binding token this attachment was minted under.
    pub fn ceiling_id(&self) -> CeilingIdToken {
        self.ceiling_id
    }

    /// Open a read session for a read-only invocation, resolving authority against
    /// the attachment's ceiling and the invocation grant before the first engine
    /// call.
    pub fn read_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<ReadSession<'_, MemoryEngine>, SessionError> {
        self.store.read_session(grant, demand)
    }

    /// Open a transaction session for a mutating invocation, resolving authority
    /// against the attachment's ceiling and the invocation grant before the first
    /// engine call.
    pub fn txn_session(
        &mut self,
        grant: InvocationGrant,
        demand: DemandCoverage,
    ) -> Result<TxnSession<'_, MemoryEngine>, SessionError> {
        self.store.txn_session(grant, demand)
    }
}

/// The per-process monotonic counter mixed into an entropy draw only as a
/// last-resort distinctness aid; the OS entropy read is the nonforgeability source.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Draw 16 bytes from the OS entropy source. Reads `/dev/urandom` on Unix — the same
/// approved source the durable-identity mint uses — and refuses on a platform
/// without one rather than substituting a predictable value.
#[cfg(unix)]
fn draw_entropy() -> Result<[u8; 16], AttachError> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(AttachError::Entropy)?;
    // Mix a process-monotonic counter into the low bytes so two attachments minted
    // in the same nanosecond still differ even under a degenerate entropy source.
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed).to_be_bytes();
    for (slot, mixed) in bytes[8..].iter_mut().zip(counter) {
        *slot ^= mixed;
    }
    Ok(bytes)
}

#[cfg(not(unix))]
fn draw_entropy() -> Result<[u8; 16], AttachError> {
    let _ = COUNTER.fetch_add(1, Ordering::Relaxed);
    Err(AttachError::Entropy(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "minting an ephemeral attachment requires an OS entropy source on this platform",
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::key::KeyScalar;
    use crate::codec::value::ScalarKind;
    use crate::durable::{Durable, FieldSchema, Presence, SiteTarget};

    fn schema() -> StoreSchema {
        StoreSchema {
            root_name: "counters".into(),
            key: vec![ScalarKind::Int],
            fields: vec![FieldSchema {
                name: "value".into(),
                kind: ScalarKind::Int,
                required: true,
            }],
            branches: Vec::new(),
        }
    }

    fn sites() -> Vec<SiteSpec> {
        vec![SiteSpec {
            target: SiteTarget::WholePayload,
        }]
    }

    /// A read-only ceiling with a fixed id byte pattern; the kernel treats the id as
    /// opaque, so the exact bytes only matter for the binding-token assertions.
    fn read_only() -> DeploymentCeiling {
        DeploymentCeiling::new(
            DemandCoverage {
                read: true,
                write: false,
            },
            CeilingIdToken::new([0x11; 32]),
        )
    }

    #[test]
    fn each_mint_draws_a_distinct_id() {
        let a = EphemeralAttachment::mint(schema(), sites(), read_only()).expect("mint");
        let b = EphemeralAttachment::mint(schema(), sites(), read_only()).expect("mint");
        assert_ne!(a.id(), b.id(), "two attachments must not share an id");
    }

    #[test]
    fn a_read_only_ceiling_denies_a_writing_invocation() {
        let mut att = EphemeralAttachment::mint(schema(), sites(), read_only()).expect("mint");
        // A writing demand exceeds a read-only ceiling even under a full grant.
        let denied = att.txn_session(
            InvocationGrant::full_store(),
            DemandCoverage {
                read: true,
                write: true,
            },
        );
        assert!(matches!(denied, Err(SessionError::Denied)));
    }

    #[test]
    fn the_mint_binds_the_ceiling_id() {
        // A ceiling carries an id token; the attachment records it, so a store is
        // bound to the exact ceiling it was minted under. Two distinct ceilings mint
        // attachments that report distinct ids.
        let a = EphemeralAttachment::mint(
            schema(),
            sites(),
            DeploymentCeiling::new(
                DemandCoverage {
                    read: true,
                    write: false,
                },
                CeilingIdToken::new([0x22; 32]),
            ),
        )
        .expect("mint");
        assert_eq!(a.ceiling_id(), CeilingIdToken::new([0x22; 32]));

        let b = EphemeralAttachment::mint(
            schema(),
            sites(),
            DeploymentCeiling::new(
                DemandCoverage {
                    read: true,
                    write: true,
                },
                CeilingIdToken::new([0x33; 32]),
            ),
        )
        .expect("mint");
        assert_ne!(a.ceiling_id(), b.ceiling_id());
    }

    #[test]
    fn a_fresh_attachment_observes_no_entries() {
        let mut att = EphemeralAttachment::mint(schema(), sites(), read_only()).expect("mint");
        let mut read = att
            .read_session(
                InvocationGrant::full_store(),
                DemandCoverage {
                    read: true,
                    write: false,
                },
            )
            .expect("read session");
        let site = read.site(0);
        assert_eq!(
            read.presence(&site, &[KeyScalar::Int(1)]),
            Ok(Presence::Absent),
            "a freshly minted attachment starts empty",
        );
    }
}
