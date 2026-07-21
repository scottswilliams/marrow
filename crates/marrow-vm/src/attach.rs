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
    FieldSchema, GroupSchema, IndexComponent, IndexSchema, InvocationGrant, SiteSpec, SiteTarget,
    StoreSchema,
};
use marrow_verify::{
    CeilingDescriptor, ExportDemand, FunctionIndex, ImageType, Scalar, SealedExport, SealedIndex,
    SealedIndexComponent, SealedSite, SealedSiteTarget, SealedTestEntry, VerifiedImage,
};

use crate::fault::RuntimeFault;
use crate::run::{DriverDispatch, run_driver, run_durable, run_in_session};
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
    let Some((schemas, sites)) = derive_schemas(image) else {
        return DurableRun::Parked;
    };

    // The deployment ceiling admits exactly the test-image demand union.
    let ceiling = deployment_ceiling(image.test_demand_union());
    let mut attachment = match EphemeralAttachment::mint(schemas, sites, ceiling) {
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

/// Run one durable *driver* test entry against a fresh persistent ephemeral
/// attachment. The test body drives exports: each durable call it makes is its own
/// invocation boundary (see [`TestDriver`]), so a mutating export commits to the
/// attachment and a later reading export observes the committed state, exactly as a
/// terminal drives an application. The attachment is minted per test and discarded
/// at the end, so no test observes another's writes. A driver test performs no
/// direct durable operation — the verifier's test-entry phase refuses a body that
/// mixes the two.
pub fn run_driver_test(image: &VerifiedImage, entry: &SealedTestEntry) -> DurableRun {
    let Some((schemas, sites)) = derive_schemas(image) else {
        return DurableRun::Parked;
    };
    let ceiling = deployment_ceiling(image.test_demand_union());
    let mut attachment = match EphemeralAttachment::mint(schemas, sites, ceiling) {
        Ok(attachment) => attachment,
        Err(_) => return DurableRun::Failed(marrow_codes::Code::CliDurableUnsupported.as_str()),
    };
    let mut driver = TestDriver {
        image,
        attachment: &mut attachment,
    };
    DurableRun::Ran(run_driver(image, entry.func(), Vec::new(), &mut driver))
}

/// The invocation dispatcher for a driver test body: it owns one persistent
/// attachment and turns each call the driver frame makes into its own session.
struct TestDriver<'a> {
    image: &'a VerifiedImage,
    attachment: &'a mut EphemeralAttachment,
}

impl DriverDispatch for TestDriver<'_> {
    fn invoke(
        &mut self,
        func: FunctionIndex,
        args: Vec<Value>,
        depth: u32,
        budget: &mut u64,
    ) -> Result<Option<Value>, RuntimeFault> {
        let demand = self.image.function_demand(func);
        // A storeless callee needs no session.
        if demand.is_empty() {
            return run_in_session(self.image, func, args, depth, budget, None);
        }
        let grant = InvocationGrant::full_store();
        let cover = coverage(demand.reads(), demand.writes());
        // A mutating call drives a transaction session (which also reads and, on its
        // own `TxnCommit`, commits to the attachment); a read-only call drives a read
        // session, so a read-only demand never opens a writer. Either session closes
        // when this invocation returns — a committed writer persists, a dropped one
        // rolls back — before the next call opens its own.
        if cover.write {
            match self.attachment.txn_session(grant, cover) {
                Ok(mut session) => {
                    run_in_session(self.image, func, args, depth, budget, Some(&mut session))
                }
                Err(_) => Err(session_open_fault(self.image, func)),
            }
        } else {
            match self.attachment.read_session(grant, cover) {
                Ok(mut session) => {
                    run_in_session(self.image, func, args, depth, budget, Some(&mut session))
                }
                Err(_) => Err(session_open_fault(self.image, func)),
            }
        }
    }
}

/// A driver invocation whose session could not open — the authority resolved against
/// the attachment's ceiling and the invocation grant refused it. The callee's demand
/// is a subset of the test-image union the ceiling is minted from, so a well-formed
/// image never reaches this; it is mapped to a source-positioned `run.authority`
/// fault rather than a panic.
fn session_open_fault(image: &VerifiedImage, func: FunctionIndex) -> RuntimeFault {
    let (line, column) = image.function(func).span_at(0).unwrap_or((1, 1));
    RuntimeFault::new(marrow_codes::Code::RunAuthority.as_str(), line, column)
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
    /// keeps it and drives several export invocations against the same store. Boxed
    /// because the attachment owns the whole store schema and is far larger than the
    /// other variants.
    Ready(Box<EphemeralAttachment>),
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
    let Some((schemas, sites)) = derive_schemas(image) else {
        return Ephemeral::Parked;
    };
    let ceiling = deployment_ceiling(image.demand_union());
    match EphemeralAttachment::mint(schemas, sites, ceiling) {
        Ok(attachment) => Ephemeral::Ready(Box::new(attachment)),
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

/// Derive the store's root-indexed schema table and the index-aligned site table from a
/// verified image, or `None` when the image's durable shape is not executable by the
/// ephemeral kernel. Every declared root must be flat-executable; if any one parks, the
/// whole image parks, since a partial store — some roots served, others silently absent —
/// is never minted. The image is the sole source of a valid schema — a forged image cannot
/// be verified, so it can never reach this derivation.
/// The public projection from a verified image to the kernel's root-indexed schema and site
/// tables — the store shape a persistent provision opens the engine under. The persistent
/// lifecycle and the terminal companion derive the same tables the ephemeral attachment
/// does, so a store is provisioned under exactly the schema the running program expects.
/// `None` when the image's durable shape is not yet executable by the flat kernel.
pub fn derive_store_schemas(image: &VerifiedImage) -> Option<(Vec<StoreSchema>, Vec<SiteSpec>)> {
    derive_schemas(image)
}

fn derive_schemas(image: &VerifiedImage) -> Option<(Vec<StoreSchema>, Vec<SiteSpec>)> {
    // A durable image declares at least one root; a storeless image never reaches attach.
    if image.roots().is_empty() {
        return None;
    }

    // One StoreSchema per declared root, in declaration order, plus each root's offset into
    // the image-wide managed-index table. A site names its index by that image-wide
    // position; the kernel resolves it against its own root's schema, so the offset rebases
    // the position to root-local when the site table is built.
    let mut schemas = Vec::with_capacity(image.roots().len());
    let mut index_offsets = Vec::with_capacity(image.roots().len());
    let mut running_indexes = 0u16;
    for (root_index, root) in image.roots().iter().enumerate() {
        let schema = derive_root_schema(image, root_index as u16, root)?;
        index_offsets.push(running_indexes);
        running_indexes += schema.indexes.len() as u16;
        schemas.push(schema);
    }

    // The site table is index-aligned with the image's sites so `Durable::site` resolves by
    // image site index. A parked site is never referenced by a verified durable opcode (the
    // verifier refuses that in phase 3), so it maps to an inert root-0 whole-payload
    // placeholder that no execution observes.
    let sites = image
        .sites()
        .iter()
        .map(|site| build_site(site, &index_offsets))
        .collect();

    Some((schemas, sites))
}

/// Derive one root's [`StoreSchema`] from the image, or `None` when the root is not
/// flat-executable (a singleton keyless root, or a group nested below its direct members).
fn derive_root_schema(
    image: &VerifiedImage,
    root_index: u16,
    root: &marrow_verify::SealedRoot,
) -> Option<StoreSchema> {
    // The executable layout is the keyed root (any key arity, its fields scalar or widened
    // composite) plus root-level unkeyed groups of storable-value fields plus field-only keyed
    // branches nested to any depth, including composite-keyed branches. A root with a group
    // nested below its direct members (or a nested/composite-keyed shape the flat kernel
    // cannot serve) is not yet executable (`has_extras`); a singleton (keyless) root has no
    // key columns and parks.
    if root.has_extras() || root.keys().is_empty() {
        return None;
    }
    let key: Vec<ScalarKind> = root
        .keys()
        .iter()
        .map(|scalar| scalar_kind(*scalar))
        .collect();

    // The unified root record is `[leading value fields][one Record slot per root-level
    // group]`, in declaration order. The kernel's flat field set is only the leading value
    // fields; the trailing group slots become `groups` below. `field_schemas` derives every
    // slot (a group slot is a product), so split off the trailing group slots by count.
    let group_count = root.groups().len();
    let all_fields = field_schemas(image, root.record())?;
    let field_count = all_fields.len().checked_sub(group_count)?;
    let fields = all_fields[..field_count].to_vec();

    // Each root-level group derives its own materialized record from the image; a group is a
    // value unit of the root entry, addressed by the root's key-path, so it carries a field
    // set but no key.
    let mut groups = Vec::with_capacity(group_count);
    for group in root.groups() {
        groups.push(GroupSchema {
            name: group.name().to_string(),
            fields: field_schemas(image, group.record())?,
        });
    }

    // Each executable branch derives its own record and nested branches from the image; the
    // sealed branch tree is in declaration order, so a `BranchEntry` branch path indexes it
    // level by level. `branch_schema` recurses over the sealed sub-branch tree, so a whole
    // nested branch shape becomes a recursive `BranchSchema` the store profile describes.
    let mut branches = Vec::with_capacity(root.branches().len());
    for branch in root.branches() {
        branches.push(branch_schema(image, branch)?);
    }

    // This root's own managed indexes, in declaration order, each with a position-resolved
    // projection the kernel maintains. An index over a parked root never reaches here (the
    // root parks above before its indexes are read).
    let indexes = image
        .indexes()
        .iter()
        .filter(|index| index.root() == root_index)
        .map(index_schema)
        .collect();

    Some(StoreSchema {
        root_name: root.name().to_string(),
        key,
        fields,
        groups,
        branches,
        indexes,
    })
}

/// Project one sealed site to a kernel [`SiteSpec`], tagging it with its root's declaration
/// position and rebasing an index-read position from image-wide to root-local. A parked
/// site — never referenced by a verified durable opcode — maps to an inert root-0
/// whole-payload placeholder.
fn build_site(site: &SealedSite, index_offsets: &[u16]) -> SiteSpec {
    let (root, target) = match site {
        SealedSite::Flat { root, target } => (*root, target),
        SealedSite::Parked { .. } => {
            return SiteSpec {
                root: 0,
                target: SiteTarget::WholePayload,
            };
        }
    };
    let target = match target {
        SealedSiteTarget::WholePayload => SiteTarget::WholePayload,
        SealedSiteTarget::FieldLeaf(field) => SiteTarget::FieldLeaf(*field),
        SealedSiteTarget::BranchEntry(branch) => SiteTarget::BranchEntry(branch.clone()),
        SealedSiteTarget::BranchField { branch, field } => SiteTarget::BranchField {
            branch: branch.clone(),
            field: *field,
        },
        SealedSiteTarget::GroupEntry(group) => SiteTarget::GroupEntry(*group),
        // An index-read site names its index by image-wide position; the kernel resolves it
        // against this root's own schema, so rebase it by the root's index offset.
        SealedSiteTarget::IndexScan(index) => {
            SiteTarget::IndexScan(*index - index_offsets[root as usize])
        }
        SealedSiteTarget::IndexLookup(index) => {
            SiteTarget::IndexLookup(*index - index_offsets[root as usize])
        }
    };
    SiteSpec { root, target }
}

/// Derive one managed index's kernel [`IndexSchema`] from its sealed form: its stable
/// identity (as raw bytes, keeping the kernel image-free), its `unique` flag, and its
/// position-resolved projection. The verifier already resolved every projection component
/// to a record/key position, so this is a direct structural projection.
fn index_schema(index: &SealedIndex) -> IndexSchema {
    IndexSchema {
        id: *index.id().bytes(),
        unique: index.unique(),
        projection: index
            .projection()
            .iter()
            .map(|component| match component {
                SealedIndexComponent::Key(column) => IndexComponent::Key(*column),
                SealedIndexComponent::Field(field) => IndexComponent::Field(*field),
            })
            .collect(),
    }
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
        // An entry identity is not an inline durable field value on this line, so it
        // parks like a collection or unit.
        ImageType::Unit | ImageType::Collection { .. } | ImageType::Identity { .. } => None,
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
