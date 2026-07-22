//! The provision approval: the rendered report a first provision presents, and the typed
//! acceptance that gates it.
//!
//! Provision is never silent. The report renders the store's shape in source vocabulary — the
//! destination, the durable roots by name, and the effects and initial authority ceiling in
//! demand terms — and never a raw hash, witness id, or ceiling id a human would have to
//! retype. A [`ProvisionApproval`] is the owner's explicit acceptance of one exact report; it
//! is constructed only from a report (interactively, or from the report's stable token for a
//! scripted flow), and [`provision_image`] refuses to write a store without one that matches
//! the report it would provision.

use std::path::Path;

use marrow_kernel::durable::{NATIVE_ENGINE_FORMAT_VERSION, SiteSpec, StoreSchema};
use marrow_verify::{CeilingDescriptor, VerifiedImage};

use crate::envelope::{EngineKind, StoreEnvelope};
use crate::head::LogicalHead;
use crate::image::{active_binding, head_map};
use crate::instance::{EntropyUnavailable, StoreInstanceId};
use crate::provision::{ProvisionError, ProvisionRequest, Provisioned, provision};

/// The report a first provision presents for acceptance, in source vocabulary only. It names
/// the destination, the durable roots by name, and whether the program reads and/or writes
/// durable data (its effects) plus the initial authority ceiling in the same demand terms —
/// never an identity hash, witness, or ceiling id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionReport {
    destination: String,
    roots: Vec<String>,
    reads: bool,
    writes: bool,
}

impl ProvisionReport {
    /// Render the report for provisioning `image` at `destination` under `schemas`. The roots
    /// are named from the schema (source spelling); the effects and ceiling are the image's
    /// demand union in reads/writes terms.
    pub fn new(destination: &Path, image: &VerifiedImage, schemas: &[StoreSchema]) -> Self {
        let ceiling = CeilingDescriptor::from_demand_union(image.demand_union());
        Self {
            destination: destination.display().to_string(),
            roots: schemas
                .iter()
                .map(|schema| schema.root_name.clone())
                .collect(),
            reads: ceiling.reads(),
            writes: ceiling.writes(),
        }
    }

    /// The human-readable report, in source vocabulary. Presented to the owner before a first
    /// provision; contains no identity hash, witness, or ceiling id — only the destination,
    /// the roots by name, and the effects and ceiling in demand terms.
    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let _ = writeln!(out, "Provision a new durable store at {}", self.destination);
        out.push_str("Durable roots:\n");
        if self.roots.is_empty() {
            out.push_str("  (none)\n");
        } else {
            for root in &self.roots {
                let _ = writeln!(out, "  - {root}");
            }
        }
        let effects = match (self.reads, self.writes) {
            (true, true) => "reads and writes durable data",
            (true, false) => "reads durable data",
            (false, true) => "writes durable data",
            (false, false) => "no durable effect",
        };
        let _ = writeln!(out, "Effects: {effects}");
        let _ = writeln!(
            out,
            "Initial authority ceiling: reads={}, writes={}",
            self.reads, self.writes,
        );
        out
    }

    /// A stable, compact token for the exact rendered report, so a scripted flow can carry an
    /// auditable acceptance without a human re-reading it. A non-cryptographic content hash of
    /// the render (this is a consent token, not a trust boundary): the same report always
    /// yields the same token, and any change to the destination, a root name, or the effects
    /// changes it. Sixteen lowercase hex characters.
    pub fn token(&self) -> String {
        // FNV-1a over the canonical render — deterministic, dependency-free, stable across
        // builds. Not a security digest: the approval attests consent, it does not authenticate.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in self.render().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{hash:016x}")
    }
}

/// An owner's explicit acceptance of one exact [`ProvisionReport`]. Constructed only from a
/// report — never defaulted — so a store is never provisioned without a report the owner (or
/// an auditable scripted acceptance) has seen. Carries the accepted report's token, which
/// [`provision_image`] checks against the report it would actually provision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionApproval {
    token: String,
}

impl ProvisionApproval {
    /// Accept `report` — the interactive path, after the owner has read the render.
    pub fn accept(report: &ProvisionReport) -> Self {
        Self {
            token: report.token(),
        }
    }

    /// Accept by a previously-rendered report token — the scripted path. The token is checked
    /// against the actual report at provision, so a token that does not match the store being
    /// provisioned is refused.
    pub fn from_token(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }

    /// The accepted report token.
    pub fn token(&self) -> &str {
        &self.token
    }
}

/// Why a provision from an image failed before or during the write.
#[derive(Debug)]
pub enum ProvisionImageError {
    /// The image's durable shape is not executable by the store kernel (a parked shape), so no
    /// schema could be derived.
    NotExecutable,
    /// No approval was presented for the exact report this provision would write: the accepted
    /// token does not match. The store is not written.
    Unapproved,
    /// An OS entropy source was unavailable, so no store identity could be minted.
    Entropy(EntropyUnavailable),
    /// The head identity map could not be built (its node count exceeds the bound).
    Head(crate::codec::FormatError),
    /// The underlying provision write failed.
    Provision(ProvisionError),
}

impl ProvisionImageError {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> &'static str {
        use marrow_codes::Code;
        match self {
            ProvisionImageError::NotExecutable => Code::CliDurableUnsupported.as_str(),
            ProvisionImageError::Unapproved => Code::ConfigInvalid.as_str(),
            ProvisionImageError::Entropy(_) => Code::IoRead.as_str(),
            ProvisionImageError::Head(error) => error.code(),
            ProvisionImageError::Provision(error) => error.code(),
        }
    }
}

impl std::fmt::Display for ProvisionImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvisionImageError::NotExecutable => write!(
                f,
                "the program's durable shape is not yet executable by the store, so it cannot be \
                 provisioned"
            ),
            ProvisionImageError::Unapproved => write!(
                f,
                "provision was not approved for this store: the accepted report does not match. \
                 Review the rendered report and accept it, then retry"
            ),
            ProvisionImageError::Entropy(error) => write!(f, "{error}"),
            ProvisionImageError::Head(error) => {
                write!(f, "the store head could not be built: {error}")
            }
            ProvisionImageError::Provision(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ProvisionImageError {}

/// Provision a fresh store for the verified `image` at `dest`, gated by `approval`. Rebuilds
/// the report the approval must match (so an approval accepted for a different store, image,
/// or destination is refused), mints a fresh store identity, derives the envelope (writer and
/// engine provenance) and the logical head (active binding + head identity map), and publishes
/// the store complete-or-not-at-all through [`provision`]. `schemas`/`sites` are the store
/// shape the caller derived from the image (`marrow_vm::derive_store_schemas`).
pub fn provision_image(
    dest: &Path,
    image: &VerifiedImage,
    schemas: Vec<StoreSchema>,
    sites: Vec<SiteSpec>,
    approval: &ProvisionApproval,
) -> Result<Provisioned, ProvisionImageError> {
    let report = ProvisionReport::new(dest, image, &schemas);
    if approval.token() != report.token() {
        return Err(ProvisionImageError::Unapproved);
    }

    let instance = StoreInstanceId::draw().map_err(ProvisionImageError::Entropy)?;
    let envelope = StoreEnvelope {
        instance,
        writer_toolchain: env!("CARGO_PKG_VERSION").to_string(),
        engine_kind: EngineKind::Redb,
        engine_format_version: NATIVE_ENGINE_FORMAT_VERSION,
    };
    let head = LogicalHead::provision(
        active_binding(image),
        crate::image::accepted_ceiling(image),
        head_map(image).map_err(ProvisionImageError::Head)?,
    );

    provision(
        dest,
        ProvisionRequest {
            envelope,
            head,
            schemas,
            sites,
        },
    )
    .map_err(ProvisionImageError::Provision)
}
