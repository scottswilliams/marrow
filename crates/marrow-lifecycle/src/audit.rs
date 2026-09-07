//! One read-only audit of a persistent store against the program it is bound to.
//!
//! The audit opens the store exactly as an attach does — the single-owner lock, then the
//! envelope and head admitted under it — but through the exact-binding gate the importer
//! uses: the presented image must be the store's active binding, never rebound. It then
//! opens the engine read-only and runs the kernel's bounded logical walk
//! ([`marrow_kernel::durable::DurableStore::logical_audit`]), and returns the
//! typed findings in source vocabulary together with a digest over the store's logical
//! content. No session opens, no authority resolves, and no data write happens; the lock is
//! released when the audit returns. Physical checksums are not verified, and an
//! inherited physical-recovery obligation is not discharged.
//!
//! The digest is a hash chain over the kernel's canonical cell stream: it starts at the
//! [`StoreDataDigest`] of the empty payload and, for each cell in key order, becomes the
//! digest of the previous state followed by the cell's key length, key, and value. Two
//! stores of one program with identical logical content have the same digest; one changed
//! cell changes it. It is reported, not persisted: the head's data-digest slot stays
//! reserved until the operation that maintains it lands.

use std::fmt::Write;
use std::path::Path;

use marrow_codes::Code;
use marrow_image::{ImageId, StoreDataDigest};
use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::{RuntimeScalar, encode_value};
use marrow_kernel::durable::{
    AuditFault, AuditFinding, AuditSite, AuditSummary, ContentDigest, NativeOpenAccess,
    SessionError, StoreProjection, StoreSchema,
};

use crate::actor::{AdmissionRefusal, ContractChanged, ExactRefusal, ImageAdmission};
use crate::attachment::PreparedImage;
use crate::authority::DemandExceedsCeiling;
use crate::image::HeadMapPinMismatch;
use crate::instance::StoreInstanceId;
use crate::provision::{AdmitError, OpenError, open_admitted};

/// One finding in source vocabulary: its stable code and the place it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub code: Code,
    pub place: String,
}

/// A completed logical inspection under one store's exact active binding.
/// Physical checksums are not verified; this report is not a recovery permit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreAudit {
    pub instance: StoreInstanceId,
    pub image_id: ImageId,
    pub summary: AuditSummary,
    pub findings: Vec<Finding>,
    pub digest: StoreDataDigest,
}

impl StoreAudit {
    /// Whether the logical walk found no inconsistency. Physical integrity is untested.
    pub fn is_clean(&self) -> bool {
        self.summary.findings == 0
    }
}

/// Why a store could not be audited. Every variant is an operational refusal decided before
/// any cell is read, except an engine failure during [`AuditError::Read`].
#[derive(Debug)]
pub enum AuditError {
    /// The image's durable shape is not executable by the store kernel, so no store can be
    /// bound to it.
    NotExecutable,
    /// The store could not be opened (not provisioned, incomplete, held, or corrupt).
    Open(OpenError),
    /// The store's active program differs from the presented image only in code: a
    /// code-only edit that `marrow run --store` has not rebound.
    ImageNotActive,
    /// The store's head names this image but records binding facts the image does not have.
    InconsistentBinding,
    /// The store's active binding differs from the presented image in a binding fact.
    ContractChanged(ContractChanged),
    /// The presented image's demand exceeds the store's accepted ceiling.
    DemandExceedsCeiling(DemandExceedsCeiling),
    /// The persisted head-map pin disagrees with the derived numbering.
    HeadMapPin(HeadMapPinMismatch),
    /// The semantic handle refused inspection, or the engine failed during the walk.
    Read(SessionError),
}

impl AuditError {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> &'static str {
        match self {
            AuditError::NotExecutable => Code::CliDurableUnsupported.as_str(),
            AuditError::Open(error) => error.code(),
            AuditError::ImageNotActive => Code::StoreImageNotActive.as_str(),
            AuditError::InconsistentBinding => Code::StoreCorruption.as_str(),
            AuditError::ContractChanged(refusal) => refusal.code(),
            AuditError::DemandExceedsCeiling(refusal) => refusal.code(),
            AuditError::HeadMapPin(refusal) => refusal.code(),
            AuditError::Read(SessionError::Poisoned) => Code::RunCommit.as_str(),
            AuditError::Read(SessionError::Denied) => Code::RunAuthority.as_str(),
            AuditError::Read(SessionError::Engine(error)) => error.code(),
        }
    }
}

impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditError::NotExecutable => {
                write!(
                    f,
                    "the program declares no durable place the store executes"
                )
            }
            AuditError::Open(error) => write!(f, "{error}"),
            AuditError::ImageNotActive => write!(
                f,
                "the program is not the store's active program: its code differs from the \
                 bound program. Present the store's active program and retry the audit"
            ),
            AuditError::InconsistentBinding => write!(
                f,
                "the store's head names this program but records binding facts the program \
                 does not have"
            ),
            AuditError::ContractChanged(refusal) => write!(f, "{refusal}"),
            AuditError::DemandExceedsCeiling(refusal) => write!(f, "{refusal}"),
            AuditError::HeadMapPin(refusal) => write!(f, "{refusal}"),
            AuditError::Read(SessionError::Poisoned) => write!(
                f,
                "the store cannot be inspected through a handle with an unresolved commit"
            ),
            AuditError::Read(SessionError::Denied) => write!(f, "store inspection was denied"),
            AuditError::Read(SessionError::Engine(error)) => {
                write!(f, "the store could not be read: {error}")
            }
        }
    }
}

impl std::error::Error for AuditError {}

/// Audit the store at `dir` against `prepared`, which must be its exact active binding.
pub fn audit(dir: &Path, prepared: PreparedImage) -> Result<StoreAudit, AuditError> {
    let (image, projection) = prepared.into_parts();
    let Some(projection) = projection else {
        return Err(AuditError::NotExecutable);
    };
    let admission = ImageAdmission::derive(&image, &projection);
    let names = Names::new(&projection);
    let opened = open_admitted(dir, projection, NativeOpenAccess::ReadOnly, |head| {
        admission.admit_exact(head)
    })
    .map_err(|error| match error {
        AdmitError::Open(error) => AuditError::Open(error),
        AdmitError::Refused(ExactRefusal::NotActive) => AuditError::ImageNotActive,
        AdmitError::Refused(ExactRefusal::InconsistentBinding) => AuditError::InconsistentBinding,
        AdmitError::Refused(ExactRefusal::ContractChanged(refusal)) => {
            AuditError::ContractChanged(refusal)
        }
        AdmitError::Refused(ExactRefusal::Admission(AdmissionRefusal::Exceeds(refusal))) => {
            AuditError::DemandExceedsCeiling(refusal)
        }
        AdmitError::Refused(ExactRefusal::Admission(AdmissionRefusal::CeilingCorrupt)) => {
            AuditError::Open(AdmissionRefusal::ceiling_corrupt())
        }
        AdmitError::Refused(ExactRefusal::Admission(AdmissionRefusal::Pin(refusal))) => {
            AuditError::HeadMapPin(refusal)
        }
    })?;
    let instance = opened.envelope.instance;
    let image_id = image.image_id();
    let mut digest = ChainDigest::new();
    let report = opened
        .logical_audit(&mut digest)
        .map_err(AuditError::Read)?;
    Ok(StoreAudit {
        instance,
        image_id,
        summary: report.summary,
        findings: report
            .findings
            .iter()
            .map(|finding| names.finding(finding))
            .collect(),
        digest: digest.finish(),
    })
}

/// The hash chain over the kernel's canonical cell stream (see the module documentation).
struct ChainDigest {
    state: StoreDataDigest,
}

impl ChainDigest {
    fn new() -> Self {
        Self {
            state: StoreDataDigest::compute(&[]),
        }
    }

    fn finish(self) -> StoreDataDigest {
        self.state
    }
}

impl ContentDigest for ChainDigest {
    fn absorb(&mut self, key: &[u8], value: &[u8]) {
        let mut payload = Vec::with_capacity(32 + 8 + key.len() + value.len());
        payload.extend_from_slice(self.state.bytes());
        payload.extend_from_slice(&(key.len() as u64).to_be_bytes());
        payload.extend_from_slice(key);
        payload.extend_from_slice(value);
        self.state = StoreDataDigest::compute(&payload);
    }
}

/// The source spellings a finding's site renders with: the projection's root, branch,
/// group, and field names, and each index's ledger identity (the image carries no index
/// name, so an index cell is named by the identity `.marrow/ids` records for it). Raw
/// bytes — an identity or an unplaceable key — render as lowercase hex.
struct Names {
    roots: Vec<StoreSchema>,
}

impl Names {
    fn new(projection: &StoreProjection) -> Self {
        Self {
            roots: projection.roots().to_vec(),
        }
    }

    fn finding(&self, finding: &AuditFinding) -> Finding {
        Finding {
            code: fault_code(finding.fault),
            place: self.place(&finding.site),
        }
    }

    fn place(&self, site: &AuditSite) -> String {
        match site {
            AuditSite::Node { root, branch, keys } => self.node(*root, branch, keys),
            AuditSite::Field {
                root,
                branch,
                keys,
                group,
                field,
            } => {
                let mut out = self.node(*root, branch, keys);
                let schema = &self.roots[usize::from(*root)];
                let fields = match group {
                    Some(group) => {
                        let group = &schema.groups()[usize::from(*group)];
                        out.push('.');
                        out.push_str(group.name());
                        group.fields()
                    }
                    None => branch_of(schema, branch).map_or(schema.fields(), |b| b.fields()),
                };
                out.push('.');
                out.push_str(fields[usize::from(*field)].name());
                out
            }
            AuditSite::IndexCell {
                root,
                index,
                values,
            } => {
                let schema = &self.roots[usize::from(*root)];
                let mut out = format!("^{}.index(", schema.root_name());
                out.push_str(&hex(schema.indexes()[usize::from(*index)].id()));
                out.push(')');
                push_keys(&mut out, values);
                out
            }
            AuditSite::UndeclaredIndex { root, id } => {
                format!(
                    "^{}.index({})",
                    self.roots[usize::from(*root)].root_name(),
                    hex(id)
                )
            }
            AuditSite::Cell { key } => format!("cell {}", hex(key)),
        }
    }

    /// `^root[k].branch[k]...` for the node the branch path and key path address.
    fn node(&self, root: u16, branch: &[u16], keys: &[KeyScalar]) -> String {
        let schema = &self.roots[usize::from(root)];
        let mut out = format!("^{}", schema.root_name());
        let mut keys = keys;
        let (head, tail) = keys.split_at(schema.key().len().min(keys.len()));
        push_keys(&mut out, head);
        keys = tail;
        let mut level = schema.branches();
        for &position in branch {
            let node = &level[usize::from(position)];
            out.push('.');
            out.push_str(node.name());
            let (head, tail) = keys.split_at(node.key().len().min(keys.len()));
            push_keys(&mut out, head);
            keys = tail;
            level = node.branches();
        }
        out
    }
}

/// The branch schema a branch path descends to, or `None` at the root.
fn branch_of<'a>(
    schema: &'a StoreSchema,
    branch: &[u16],
) -> Option<&'a marrow_kernel::durable::BranchSchema> {
    let mut level = schema.branches();
    let mut found = None;
    for &position in branch {
        let node = &level[usize::from(position)];
        level = node.branches();
        found = Some(node);
    }
    found
}

fn fault_code(fault: AuditFault) -> Code {
    match fault {
        AuditFault::Undecodable => Code::StoreAuditUndecodable,
        AuditFault::OutsideSchema => Code::StoreAuditOutsideSchema,
        AuditFault::RequiredMissing => Code::StoreAuditRequiredMissing,
        AuditFault::OrphanLeaf => Code::StoreAuditOrphanLeaf,
        AuditFault::MarkerInvalid => Code::StoreAuditMarkerInvalid,
        AuditFault::IndexOrphan => Code::StoreAuditIndexOrphan,
        AuditFault::IndexStale => Code::StoreAuditIndexStale,
        AuditFault::IndexMissing => Code::StoreAuditIndexMissing,
        AuditFault::WitnessInvalid => Code::StoreAuditWitnessInvalid,
    }
}

/// `[k1, k2]` in the source spelling of each key value.
fn push_keys(out: &mut String, keys: &[KeyScalar]) {
    if keys.is_empty() {
        return;
    }
    out.push('[');
    for (index, key) in keys.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&spell_key(key));
    }
    out.push(']');
}

/// A key value as source spells it: a decimal `int`, `true`/`false`, a quoted string,
/// `0x`-prefixed bytes, and the canonical text of a temporal value.
fn spell_key(key: &KeyScalar) -> String {
    match key {
        KeyScalar::Int(value) => value.to_string(),
        KeyScalar::Bool(value) => value.to_string(),
        KeyScalar::Str(text) => format!("{text:?}"),
        KeyScalar::Bytes(bytes) => format!("0x{}", hex(bytes)),
        KeyScalar::Date(days) => temporal_text(&RuntimeScalar::Date(*days)),
        KeyScalar::Duration(nanos) => temporal_text(&RuntimeScalar::Duration(*nanos)),
        KeyScalar::Instant(nanos) => temporal_text(&RuntimeScalar::Instant(*nanos)),
    }
}

/// The canonical text of a temporal scalar through the kernel codec; a value outside the
/// codec's range (which no stored key holds) renders as its raw count.
fn temporal_text(scalar: &RuntimeScalar) -> String {
    match encode_value(scalar) {
        Ok(bytes) => String::from_utf8(bytes).unwrap_or_default(),
        Err(_) => format!("{scalar:?}"),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A changed cell, a reordered stream, and an empty stream all differ; the chain is a
    /// deterministic function of the ordered cell stream.
    #[test]
    fn the_chain_digest_is_deterministic_and_order_sensitive() {
        let chain = |cells: &[(&[u8], &[u8])]| {
            let mut digest = ChainDigest::new();
            for (key, value) in cells {
                digest.absorb(key, value);
            }
            digest.finish()
        };
        let a = chain(&[(b"k1", b"v1"), (b"k2", b"v2")]);
        assert_eq!(a, chain(&[(b"k1", b"v1"), (b"k2", b"v2")]));
        assert_ne!(a, chain(&[(b"k2", b"v2"), (b"k1", b"v1")]));
        assert_ne!(a, chain(&[(b"k1", b"v1"), (b"k2", b"v3")]));
        assert_ne!(a, chain(&[]));
        // The key/value split is framed: moving a byte across it changes the chain.
        assert_ne!(chain(&[(b"ab", b"c")]), chain(&[(b"a", b"bc")]));
    }

    #[test]
    fn keys_spell_as_source_does() {
        assert_eq!(spell_key(&KeyScalar::Int(-7)), "-7");
        assert_eq!(spell_key(&KeyScalar::Bool(true)), "true");
        assert_eq!(spell_key(&KeyScalar::Str("a\"b".into())), "\"a\\\"b\"");
        assert_eq!(spell_key(&KeyScalar::Bytes(vec![0x00, 0xff])), "0x00ff");
        assert_eq!(spell_key(&KeyScalar::Date(0)), "1970-01-01");
    }
}
