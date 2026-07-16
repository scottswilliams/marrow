//! Ephemeral-memory durable execution (E01 tests; E02 export transactions).
//!
//! The executor derives a durable [`StoreSchema`] and site table from a
//! [`VerifiedImage`] — the only source of a valid schema — mints an
//! [`EphemeralAttachment`], opens the session an invocation's demand requires under
//! a full grant, and runs on the VM. [`run_durable_test`] mints a fresh attachment
//! per durable source test, so tests never observe one another's writes.
//! [`mint_ephemeral`] plus [`run_export`] instead keep one attachment across several
//! export invocations, so a mutating export's committed `transaction` region is
//! observable by a later read invocation and a rolled-back one is not (E02).
//!
//! The flat keyed root (one or more key columns) whose fields are scalars or widened
//! values, with field-only keyed branches nested to any depth, is executable here; a
//! parked durable shape — a singleton root, a group, or a nominal-typed field — is
//! [`DurableRun::Parked`], reported honestly rather than run against a partial store.

use marrow_kernel::codec::value::{ScalarKind, ValueShape};
use marrow_kernel::durable::{
    BranchSchema, CeilingIdToken, DemandCoverage, DeploymentCeiling, EphemeralAttachment,
    FieldSchema, InvocationGrant, SiteSpec, SiteTarget, StoreSchema,
};
use marrow_verify::{
    CeilingDescriptor, ExportDemand, ImageType, Scalar, SealedExport, SealedSite, SealedSiteTarget,
    SealedTestEntry, VerifiedImage,
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
    /// (a singleton root, a group, or a nominal-typed field). Composite keys and
    /// field-only keyed branches nested to any depth are executable.
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

    // The deployment ceiling admits exactly the test-image demand union.
    let ceiling = deployment_ceiling(image.test_demand_union());
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

/// Derive the deployment ceiling a fresh attachment is bounded by from a demand
/// union. Building the descriptor from the union derives both the read/write
/// coverage the kernel checks and the 32-byte ceiling-id binding token from the same
/// verified atoms, so a wider ceiling would carry a different id — the ceiling is
/// bound to the verified image, never supplied independently.
fn deployment_ceiling(union: ExportDemand) -> DeploymentCeiling {
    let descriptor = CeilingDescriptor::from_demand_union(union);
    DeploymentCeiling::new(
        coverage(descriptor.reads(), descriptor.writes()),
        CeilingIdToken::new(*descriptor.ceiling_id().bytes()),
    )
}

/// Whether a persistent ephemeral attachment could be minted for a whole image.
pub enum Ephemeral {
    /// A minted attachment over the image's flat executable durable shape. The caller
    /// keeps it and drives several export invocations against the same store.
    Ready(EphemeralAttachment),
    /// The image's durable shape is not yet executable by the flat kernel (a
    /// singleton root, a group, or a nominal-typed field).
    Parked,
    /// Minting the attachment failed operationally; the stable code names why.
    Failed(&'static str),
}

/// Mint one persistent ephemeral attachment for a whole verified image: a single
/// in-memory durable store bound to the image's schema, sites, and whole-program
/// demand-union ceiling, which serves every export invocation in sequence. The
/// persistent counterpart of [`run_durable_test`]'s per-test mint — here the caller
/// retains the attachment and drives several exports against the same store, so a
/// committed transaction is observable by a later read and a rolled-back one is not.
pub fn mint_ephemeral(image: &VerifiedImage) -> Ephemeral {
    let Some((schema, sites)) = derive_schema(image) else {
        return Ephemeral::Parked;
    };
    let ceiling = deployment_ceiling(image.demand_union());
    match EphemeralAttachment::mint(schema, sites, ceiling) {
        Ok(attachment) => Ephemeral::Ready(attachment),
        Err(_) => Ephemeral::Failed(marrow_codes::Code::CliDurableUnsupported.as_str()),
    }
}

/// Run one export against a persistent attachment, opening the session its verified
/// demand requires: a transaction session for a mutating export (whose own
/// `transaction` region commits the staged writes) and a read session for a
/// read-only one, both under a full grant. Because the attachment persists across
/// calls, a mutating export's committed writes are visible to a later read invocation
/// on the same attachment, and a mutating export that faults before its commit leaves
/// the store unchanged.
pub fn run_export(
    image: &VerifiedImage,
    attachment: &mut EphemeralAttachment,
    export: &SealedExport,
    args: Vec<Value>,
) -> DurableRun {
    let grant = InvocationGrant::full_store();
    let demand = coverage(export.demand().reads(), export.demand().writes());
    let func = export.function();
    let result = if demand.write {
        match attachment.txn_session(grant, demand) {
            Ok(mut session) => run_durable(image, func, args, &mut session),
            Err(_) => {
                return DurableRun::Failed(marrow_codes::Code::CliDurableUnsupported.as_str());
            }
        }
    } else {
        match attachment.read_session(grant, demand) {
            Ok(mut session) => run_durable(image, func, args, &mut session),
            Err(_) => {
                return DurableRun::Failed(marrow_codes::Code::CliDurableUnsupported.as_str());
            }
        }
    };
    DurableRun::Ran(result)
}

/// Derive the flat store schema and index-aligned site table from a verified image,
/// or `None` when the image's durable shape is not executable by the ephemeral read
/// kernel. The image is the sole source of a valid schema — a forged image cannot be
/// verified, so it can never reach this derivation.
fn derive_schema(image: &VerifiedImage) -> Option<(StoreSchema, Vec<SiteSpec>)> {
    // v0 carries at most one durable root; a durable test with demand has exactly
    // one. A flat executable root is keyed (one or more columns) with no member tree.
    //
    // The executable layout is the keyed root (any key arity, its fields scalar or widened
    // composite) plus field-only keyed branches nested to any depth, including composite-keyed
    // branches. A group at any level parks the root; a singleton (keyless) root parks; bounded
    // traversal over a composite-keyed layer parks separately. A parked shape stays parked
    // until its owner lands it.
    let root = image.roots().first()?;
    // A root with a group at any level (or a branch enclosing one) is not yet executable
    // (`has_extras`); field-only branches nested to any depth, with composite keys, are
    // executable and do not set that flag. A singleton root has no key columns and parks.
    if root.has_extras() || root.keys().is_empty() {
        return None;
    }
    let key: Vec<ScalarKind> = root
        .keys()
        .iter()
        .map(|scalar| scalar_kind(*scalar))
        .collect();

    let fields = field_schemas(image, root.record())?;

    // Each executable branch derives its own record and nested branches from the image; the
    // sealed branch tree is in declaration order, so a `BranchEntry` branch path indexes it
    // level by level. `branch_schema` recurses over the sealed sub-branch tree, so a whole
    // nested branch shape becomes a recursive `BranchSchema` the store profile describes.
    let mut branches = Vec::with_capacity(root.branches().len());
    for branch in root.branches() {
        branches.push(branch_schema(image, branch)?);
    }

    let schema = StoreSchema {
        root_name: root.name().to_string(),
        key,
        fields,
        branches,
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
            SealedSite::Flat {
                target: SealedSiteTarget::BranchEntry(branch),
                ..
            } => SiteSpec {
                target: SiteTarget::BranchEntry(branch.clone()),
            },
            SealedSite::Flat {
                target: SealedSiteTarget::BranchField { branch, field },
                ..
            } => SiteSpec {
                target: SiteTarget::BranchField {
                    branch: branch.clone(),
                    field: *field,
                },
            },
            SealedSite::Parked { .. } => SiteSpec {
                target: SiteTarget::WholePayload,
            },
        })
        .collect();

    Some((schema, sites))
}

/// Derive one branch's recursive [`BranchSchema`] from the image: its name, key columns,
/// materialized record fields, and — recursively — its own nested branches. `None` when a
/// record field is not an inline field value (a collection), mirroring [`field_schemas`].
/// The verifier proves an executable branch's fields are storable and its sub-branches are
/// simple, so this is defense in depth over that proof.
fn branch_schema(
    image: &VerifiedImage,
    branch: &marrow_verify::SealedBranch,
) -> Option<BranchSchema> {
    let mut branches = Vec::with_capacity(branch.branches().len());
    for sub in branch.branches() {
        branches.push(branch_schema(image, sub)?);
    }
    Some(BranchSchema {
        name: branch.name().to_string(),
        key: branch
            .keys()
            .iter()
            .map(|scalar| scalar_kind(*scalar))
            .collect(),
        fields: field_schemas(image, branch.record())?,
        branches,
    })
}

/// The kernel field schemas of a node's materialized record: one per field, in order,
/// each carrying the field's storable value shape (a scalar, a dense product, or a
/// closed sum). `None` when a field is a collection or unit — shapes the durable field
/// codec never stores inline — so the whole derivation parks. The verifier proves an
/// executable node's record fields are a scalar or a widened composite, so this is
/// defense in depth over that proof.
fn field_schemas(image: &VerifiedImage, record: u16) -> Option<Vec<FieldSchema>> {
    let record = image.record_type(record);
    let mut fields = Vec::with_capacity(record.fields().len());
    for field in record.fields() {
        fields.push(FieldSchema {
            name: field.name.to_string(),
            shape: value_shape(image, field.ty)?,
            required: field.required,
        });
    }
    Some(fields)
}

/// Derive a field's kernel [`ValueShape`] from its image type, recursively: a scalar
/// carries its kind; a record becomes a product of its fields' shapes in declaration
/// order; a closed enum (`Option`/`Result`/a user `enum`) becomes a sum of its variants'
/// dense payload shapes. A collection or unit is not an inline field value, so it parks
/// (`None`). The image is depth-bounded by the verifier, so the recursion terminates.
fn value_shape(image: &VerifiedImage, ty: ImageType) -> Option<ValueShape> {
    match ty {
        ImageType::Scalar { scalar, .. } => Some(ValueShape::Scalar(scalar_kind(scalar))),
        ImageType::Record { idx, .. } => {
            let record = image.record_type(idx);
            let mut fields = Vec::with_capacity(record.fields().len());
            for field in record.fields() {
                fields.push(value_shape(image, field.ty)?);
            }
            Some(ValueShape::Product { ty: idx, fields })
        }
        ImageType::Enum { idx, .. } => {
            let sealed = image.enums().get(idx as usize)?;
            let mut variants = Vec::with_capacity(sealed.variants().len());
            for variant in sealed.variants() {
                let mut payload = Vec::with_capacity(variant.payload.len());
                for leaf in &variant.payload {
                    payload.push(value_shape(image, *leaf)?);
                }
                variants.push(payload);
            }
            Some(ValueShape::Sum { ty: idx, variants })
        }
        ImageType::Unit | ImageType::Collection { .. } => None,
    }
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
