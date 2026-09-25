//! The provision report and the approval that gates a first provision.
//!
//! A [`ProvisionReport`] renders the store an image would provision in source vocabulary: the
//! destination, the durable roots by name, and the effects and initial authority ceiling in
//! demand terms, never an identity hash, witness id or ceiling id. A [`ProvisionApproval`] is
//! built only from a report and binds that report's image at that exact destination spelling,
//! within one process. [`provision_image`] refuses an approval for a different image or
//! destination spelling before any filesystem access. An approval records consent; it does not
//! authenticate the caller.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use marrow_image::ImageId;
use marrow_kernel::durable::NATIVE_ENGINE_FORMAT_VERSION;
use marrow_verify::CeilingDescriptor;

use crate::attachment::PreparedImage;
use crate::envelope::{EngineKind, StoreEnvelope};
use crate::head::LogicalHead;
use crate::image::{active_binding, head_map};
use crate::instance::{EntropyUnavailable, StoreInstanceId};
use crate::provision::{
    ProvisionCleanupFailure, ProvisionError, ProvisionFault, ProvisionRequest, Provisioned,
    provision,
};
use marrow_codes::Code;

/// The store an image would provision at one destination. It names the destination, the
/// durable roots by name, and whether the program reads and/or writes durable data (its
/// effects) plus the initial authority ceiling in the same demand terms. It also retains the
/// image identity, unrendered, so that an approval of this report binds that image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionReport {
    image: ImageId,
    destination: PathBuf,
    roots: Vec<String>,
    reads: bool,
    writes: bool,
}

impl ProvisionReport {
    /// Build the report for provisioning the prepared image at `destination`. The roots are
    /// named from the image's store projection (source spelling); the effects and ceiling
    /// are the image's demand union in reads/writes terms. An image with no store shape the
    /// kernel executes (a storeless program or a parked durable shape) has no store to
    /// provision, so it has no report.
    pub fn new(destination: &Path, prepared: &PreparedImage) -> Result<Self, ProvisionImageError> {
        let projection = prepared
            .projection()
            .ok_or(ProvisionImageError::NotExecutable)?;
        let ceiling = CeilingDescriptor::from_demand_union(prepared.image().demand_union());
        Ok(Self {
            image: prepared.image().image_id(),
            destination: destination.to_path_buf(),
            roots: projection
                .roots()
                .iter()
                .map(|schema| schema.root_name().to_string())
                .collect(),
            reads: ceiling.reads(),
            writes: ceiling.writes(),
        })
    }

    /// The directory the store would be provisioned into.
    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// The durable roots the store would hold, in source spelling and projection order.
    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    /// Whether the program reads durable data, and so whether the initial ceiling admits
    /// observing access.
    pub fn reads(&self) -> bool {
        self.reads
    }

    /// Whether the program writes durable data, and so whether the initial ceiling admits
    /// mutating access.
    pub fn writes(&self) -> bool {
        self.writes
    }

    /// The human-readable report, in source vocabulary. Contains no identity hash, witness or
    /// ceiling id: only the destination, the roots by name, and the effects and ceiling in
    /// demand terms.
    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "Provision a new durable store at {}",
            self.destination.display()
        );
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
}

/// Acceptance of one [`ProvisionReport`]: its image and its exact destination spelling. Built
/// only by [`ProvisionApproval::accept`], and meaningful only in the process that built the
/// report. It records consent and does not authenticate: any caller holding the image and the
/// destination can build one, and it grants no authority.
#[derive(Debug)]
pub struct ProvisionApproval {
    image: ImageId,
    destination: OsString,
}

impl ProvisionApproval {
    /// Accept `report`: the approval binds the report's image and destination spelling.
    pub fn accept(report: &ProvisionReport) -> Self {
        Self {
            image: report.image,
            destination: report.destination.as_os_str().to_os_string(),
        }
    }
}

/// Why a provision from an image failed before or during the write.
#[derive(Debug)]
pub enum ProvisionImageError {
    /// The image has no store shape the kernel executes (a storeless program or a parked
    /// durable shape), so there is no store to provision.
    NotExecutable,
    /// The approval was accepted for a different image or destination than this provision.
    /// No store is written.
    Unapproved,
    /// An OS entropy source was unavailable, so no store identity could be minted.
    Entropy(EntropyUnavailable),
    /// The head identity map could not be built: too many nodes or repeated declaration IDs.
    Head(crate::codec::FormatError),
    /// The underlying provision write failed.
    Provision(ProvisionError),
}

impl ProvisionImageError {
    /// The unconfirmed lifecycle stage and its actual store instance.
    pub fn uncertainty(&self) -> Option<(marrow_codes::StoreUncertainty, StoreInstanceId)> {
        use marrow_codes::StoreUncertainty;
        match self {
            Self::Provision(ProvisionError {
                fault: ProvisionFault::PublicationUncertain { instance, .. },
                ..
            }) => Some((StoreUncertainty::Publication, *instance)),
            Self::Provision(ProvisionError {
                fault: ProvisionFault::ActivationUncertain { instance, .. },
                ..
            }) => Some((StoreUncertainty::Activation, *instance)),
            _ => None,
        }
    }

    /// The stable dotted code a tool reports.
    pub fn code(&self) -> Code {
        match self {
            ProvisionImageError::NotExecutable => Code::CliDurableUnsupported,
            ProvisionImageError::Unapproved => Code::StoreProvisionUnapproved,
            ProvisionImageError::Entropy(_) => Code::IoRead,
            ProvisionImageError::Head(error) => error.code(),
            ProvisionImageError::Provision(error) => error.code(),
        }
    }

    /// Failed cleanup of an unpublished stage, independent of the primary failure.
    pub fn cleanup(&self) -> Option<&ProvisionCleanupFailure> {
        match self {
            Self::Provision(error) => error.cleanup.as_ref(),
            _ => None,
        }
    }
}

impl std::fmt::Display for ProvisionImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvisionImageError::NotExecutable => write!(
                f,
                "the program has no durable shape the store can execute, so it cannot be \
                 provisioned"
            ),
            ProvisionImageError::Unapproved => write!(
                f,
                "provisioning was refused: the approval was accepted for a different image or \
                 destination"
            ),
            ProvisionImageError::Entropy(error) => write!(f, "{error}"),
            ProvisionImageError::Head(error) => {
                write!(f, "the store head could not be built: it {error}")
            }
            ProvisionImageError::Provision(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ProvisionImageError {}

/// Provision a fresh store for the prepared image at `dest`, gated by `approval`. Refuses an
/// approval accepted for a different image or destination spelling before any filesystem
/// access, then mints a fresh store identity, derives the envelope (writer and engine
/// provenance) and the logical head (active binding + head identity map), and publishes the
/// complete store through [`provision`], retaining its identity if the final directory sync
/// fails. The preparation is borrowed: the caller keeps it to attach or import into the store
/// it just provisioned.
pub fn provision_image(
    dest: &Path,
    prepared: &PreparedImage,
    approval: &ProvisionApproval,
) -> Result<Provisioned, ProvisionImageError> {
    let image = prepared.image();
    // `OsStr` equality is byte-exact; `Path` equality would compare normalized components.
    if approval.image != image.image_id() || approval.destination.as_os_str() != dest.as_os_str() {
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

    provision(dest, ProvisionRequest { envelope, head }).map_err(ProvisionImageError::Provision)
}
