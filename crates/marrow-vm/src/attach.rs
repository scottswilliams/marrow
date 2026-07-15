//! Ephemeral-memory durable test execution (E01).
//!
//! The executor derives a durable [`StoreSchema`] and site table from a
//! [`VerifiedImage`] — the only source of a valid schema — mints a fresh
//! [`EphemeralAttachment`] bounded by the test-image demand union, opens one
//! session for the invocation's own demand under a full test grant, and runs the
//! test on the VM. Each durable source test runs against its own fresh attachment,
//! so tests never observe one another's writes.
//!
//! Only the flat single-column keyed root of scalar fields is executable here (the
//! read kernel E01 lands); a wider durable shape — a composite key, a nested branch
//! or group, or a non-scalar field — is [`DurableRun::Parked`], reported honestly
//! rather than run against a partial store.

use marrow_kernel::codec::value::ScalarKind;
use marrow_kernel::durable::{
    DemandCoverage, DeploymentCeiling, EphemeralAttachment, FieldSchema, InvocationGrant, SiteSpec,
    SiteTarget, StoreSchema,
};
use marrow_verify::{
    ImageType, Scalar, SealedSite, SealedSiteTarget, SealedTestEntry, VerifiedImage,
};

use crate::fault::RuntimeFault;
use crate::run::run_durable;
use crate::value::Value;

/// The outcome of attempting to run one durable test against a fresh ephemeral
/// attachment.
pub enum DurableRun {
    /// The test ran; the inner result is its VM value or source-mapped fault.
    Ran(Result<Option<Value>, RuntimeFault>),
    /// The image's durable shape is not yet executable by the ephemeral read kernel
    /// (a composite key, a nested member tree, or a non-scalar field). The durable
    /// trough narrows to the flat single root; wider shapes stay parked.
    Parked,
    /// Minting the attachment failed operationally; the stable code names why.
    Failed(&'static str),
}

/// Run one durable test entry against a fresh ephemeral-memory attachment. The
/// ceiling is the test-image demand union; the invocation demand is the entry's own
/// reconstructed demand under a full test grant.
pub fn run_durable_test(image: &VerifiedImage, entry: &SealedTestEntry) -> DurableRun {
    let Some((schema, sites)) = derive_schema(image) else {
        return DurableRun::Parked;
    };

    let union = image.test_demand_union();
    let ceiling = DeploymentCeiling::from_coverage(coverage(union.reads(), union.writes()));
    let mut attachment = match EphemeralAttachment::mint(schema, sites, ceiling) {
        Ok(attachment) => attachment,
        Err(_) => return DurableRun::Failed(marrow_codes::Code::CliDurableUnsupported.as_str()),
    };

    let grant = InvocationGrant::full_store();
    let demand = coverage(entry.demand().reads(), entry.demand().writes());
    let func = entry.func();

    // A mutating test drives a transaction session (which also reads); a read-only
    // test drives a read session, so a read-only demand never opens a writer.
    let result = if demand.write {
        match attachment.txn_session(grant, demand) {
            Ok(mut session) => run_durable(image, func, Vec::new(), &mut session),
            Err(_) => {
                return DurableRun::Failed(marrow_codes::Code::CliDurableUnsupported.as_str());
            }
        }
    } else {
        match attachment.read_session(grant, demand) {
            Ok(mut session) => run_durable(image, func, Vec::new(), &mut session),
            Err(_) => {
                return DurableRun::Failed(marrow_codes::Code::CliDurableUnsupported.as_str());
            }
        }
    };
    DurableRun::Ran(result)
}

fn coverage(read: bool, write: bool) -> DemandCoverage {
    DemandCoverage { read, write }
}

/// Derive the flat store schema and index-aligned site table from a verified image,
/// or `None` when the image's durable shape is not executable by the ephemeral read
/// kernel. The image is the sole source of a valid schema — a forged image cannot be
/// verified, so it can never reach this derivation.
fn derive_schema(image: &VerifiedImage) -> Option<(StoreSchema, Vec<SiteSpec>)> {
    // v0 carries at most one durable root; a durable test with demand has exactly
    // one. A flat executable root is single-column keyed with no member tree.
    let root = image.roots().first()?;
    if root.has_extras() {
        return None;
    }
    let [key] = root.keys() else {
        return None;
    };
    let key = scalar_kind(*key);

    let record = image.record_type(root.record());
    let mut fields = Vec::with_capacity(record.fields().len());
    for field in record.fields() {
        let ImageType::Scalar { scalar, .. } = field.ty else {
            // A non-scalar (enum or collection) field is not durably storable by the
            // flat kernel; the shape stays parked.
            return None;
        };
        fields.push(FieldSchema {
            name: field.name.to_string(),
            kind: scalar_kind(scalar),
            required: field.required,
        });
    }

    let schema = StoreSchema {
        root_name: root.name().to_string(),
        key,
        fields,
    };

    // The site table is index-aligned with the image's sites so `Durable::site`
    // resolves by image site index. A parked site is never referenced by a verified
    // durable opcode (the verifier refuses that in phase 3), so it maps to an inert
    // whole-payload placeholder that no execution observes.
    let sites = image
        .sites()
        .iter()
        .map(|site| match site {
            SealedSite::Flat {
                target: SealedSiteTarget::WholePayload,
                ..
            } => SiteSpec {
                target: SiteTarget::WholePayload,
            },
            SealedSite::Flat {
                target: SealedSiteTarget::FieldLeaf(field),
                ..
            } => SiteSpec {
                target: SiteTarget::FieldLeaf(*field),
            },
            SealedSite::Parked { .. } => SiteSpec {
                target: SiteTarget::WholePayload,
            },
        })
        .collect();

    Some((schema, sites))
}

/// Map an image scalar type to the runtime codec's scalar kind. Total over the
/// closed scalar domain the value/key codecs already support.
fn scalar_kind(scalar: Scalar) -> ScalarKind {
    match scalar {
        Scalar::Int => ScalarKind::Int,
        Scalar::Bool => ScalarKind::Bool,
        Scalar::Text => ScalarKind::Str,
        Scalar::Bytes => ScalarKind::Bytes,
        Scalar::Date => ScalarKind::Date,
        Scalar::Instant => ScalarKind::Instant,
        Scalar::Duration => ScalarKind::Duration,
    }
}
