//! The trusted bulk importer: a closed private lifecycle-maintenance mode that populates a
//! provisioned store from external flat-scalar JSONL rows.
//!
//! External untyped rows have no valid cell form until the kernel places them, so every row
//! is created through [`create_entry`](marrow_kernel::durable::Durable::create_entry) — the
//! full write algebra, not a byte copy. The importer never opens the byte engine, mints a raw
//! cell key, or holds a transaction handle, and no bytecode opcode, host import, or
//! client-wire request reaches this mode.
//!
//! [`import_jsonl`] admits the [`PreparedImage`] under the store's exact active binding —
//! head binding, accepted ceiling, and head-map pin — before the engine opens, so a stale,
//! foreign, or over-demanding image performs no engine call and writes nothing. Import never
//! rebinds; `marrow run --store` owns the explicit compatible rebind. The import site is a
//! private reprojection of the admitted roots and never becomes an execution attachment.
//!
//! # Bounds
//!
//! Each JSONL line, row field count, and string value is capped by [`ImportLimits`] before
//! allocation, and rows commit in batches of [`ImportLimits::batch_rows`], so memory is
//! bounded by one line plus one batch however large the corpus. Batches are individually
//! atomic; the import is *not* one transaction, so a mid-import failure leaves the committed
//! prefix and reports its size, letting the caller discard and re-provision.

use std::io::BufRead;
use std::path::Path;

use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::{RuntimeScalar, ScalarKind};
use marrow_kernel::durable::{
    CommitRecovery, CommitResult, CreateOutcome, DemandCoverage, Durable, DurableCommitState,
    EntryValue, InvocationGrant, KernelFault, SessionError, SessionHost, SiteTarget, StoreSchema,
};
use marrow_kernel::equality::ValueDomain;
use marrow_local_wire::{Json, Lexer, WireError, encode};

use crate::actor::{AdmissionRefusal, BindingStrictness, ImageAdmission};
use crate::attachment::PreparedImage;
use crate::provision::{AdmitError, OpenError, open_admitted};
use crate::seam::Seam;
use marrow_codes::Code;

/// The bounds every import obeys before it allocates. The defaults suit a
/// personal-tool export; a caller may tighten them but the importer never runs unbounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportLimits {
    /// The maximum bytes of one JSONL line (excluding the newline). A longer line is refused
    /// before it is buffered.
    pub max_line_bytes: usize,
    /// The maximum number of JSON members one row object may carry.
    pub max_fields_per_row: usize,
    /// The maximum bytes of one decoded string value.
    pub max_string_bytes: usize,
    /// The number of rows committed per engine transaction — the batch memory bound.
    pub batch_rows: usize,
}

impl ImportLimits {
    /// The default import bounds: 1 MiB per line and per string, 4096 members per row (the
    /// record-width ceiling), and 1024 rows per batch.
    pub const DEFAULT: Self = Self {
        max_line_bytes: 1 << 20,
        max_fields_per_row: 4096,
        max_string_bytes: 1 << 20,
        batch_rows: 1024,
    };
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The target of an import: which declared root to populate and the source names of its key
/// columns, in key order. The store schema records key *kinds* but not their source names, so
/// the caller — which derived the schema from the verified image — supplies the names the JSONL
/// members are read by; the kinds come from the schema, its single owner. The root's fields
/// (names, shapes, required flags) are read from its [`StoreSchema`]; the importer refuses a
/// root whose shape it cannot map (see [`ImportError::UnsupportedShape`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportTarget {
    /// The target root's declaration index into the store's schema table.
    pub root: u16,
    /// The root's key column source names, in key-declaration order (one per column of the
    /// schema's key tuple).
    pub key_columns: Vec<String>,
}

/// The confirmed outcome of an import: how many rows were created and committed, and in how
/// many batches. Reported after the final batch commits (or, on failure, alongside the error's
/// committed prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportReport {
    pub rows_imported: u64,
    pub batches_committed: u64,
}

/// Why a target root cannot be mapped by the flat importer. Raised before any store write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShapeFault {
    /// The image's durable shape is not executable by the store kernel (a storeless image or
    /// a parked shape), so there is no store shape to import into.
    NotExecutable,
    /// The root index is beyond the declared root table.
    RootOutOfRange { root: u16, declared: usize },
    /// The root declares groups or keyed branches; the flat importer populates scalar-field
    /// roots only.
    HasGroupsOrBranches { root: String },
    /// The named key columns do not match the root's key arity.
    KeyArity {
        root: String,
        declared: usize,
        named: usize,
    },
    /// A key column's scalar kind is not importable (`int`, `bool`, `string` only).
    KeyScalarUnsupported { column: String, kind: ScalarKind },
    /// A field is not a scalar (a product or sum shape).
    FieldNotScalar { field: String },
    /// A field's scalar kind is not importable.
    FieldScalarUnsupported { field: String, kind: ScalarKind },
}

impl std::fmt::Display for ShapeFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShapeFault::NotExecutable => write!(
                f,
                "the program's durable shape is not yet executable by the store"
            ),
            ShapeFault::RootOutOfRange { root, declared } => {
                write!(
                    f,
                    "root index {root} is out of range ({declared} declared root(s))"
                )
            }
            ShapeFault::HasGroupsOrBranches { root } => write!(
                f,
                "root `{root}` declares groups or keyed branches; the importer populates flat \
                 scalar roots only"
            ),
            ShapeFault::KeyArity {
                root,
                declared,
                named,
            } => write!(
                f,
                "root `{root}` has {declared} key column(s) but {named} were named"
            ),
            ShapeFault::KeyScalarUnsupported { column, kind } => write!(
                f,
                "key column `{column}` is {} (import maps int, bool, and string)",
                kind.name()
            ),
            ShapeFault::FieldNotScalar { field } => write!(
                f,
                "field `{field}` is not a scalar; the importer maps scalar fields only"
            ),
            ShapeFault::FieldScalarUnsupported { field, kind } => write!(
                f,
                "field `{field}` is {} (import maps int, bool, and string)",
                kind.name()
            ),
        }
    }
}

/// Why a source row could not be decoded or mapped to the target. A typed fact so a caller (or
/// test) asserts the category rather than parsing prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowFault {
    /// The line is not well-formed JSON, or a string in it exceeds
    /// [`ImportLimits::max_string_bytes`]: the lexer's typed refusal.
    Malformed(WireError),
    /// A member's value is an object or an array; the flat importer maps scalars only.
    Nested { name: String },
    /// A member name appears twice in one row.
    DuplicateMember { name: String },
    /// The row carries more members than [`ImportLimits::max_fields_per_row`].
    TooManyMembers { limit: usize },
    /// A key column is absent from the row.
    MissingKey { column: String },
    /// A key column is present but `null`.
    NullKey { column: String },
    /// A key column's value type does not match its declared scalar kind.
    KeyType {
        column: String,
        expected: ScalarKind,
        found: &'static str,
    },
    /// A required field is absent or `null`.
    MissingRequiredField { field: String },
    /// A field's value type does not match its declared scalar kind.
    FieldType {
        field: String,
        expected: ScalarKind,
        found: &'static str,
    },
    /// A member matched neither a key column nor a declared field.
    UnrecognizedMember { name: String },
    /// A durable entry with this row's key already exists (create yielded already-present).
    DuplicateKey,
    /// This row's value for a `unique` managed index collides with another row's.
    UniqueIndexCollision,
}

impl std::fmt::Display for RowFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RowFault::Malformed(WireError::StringLimit) => {
                write!(f, "a string value exceeds the import string limit")
            }
            RowFault::Malformed(_) => write!(f, "the line is not a well-formed JSON object"),
            RowFault::Nested { name } => write!(
                f,
                "member `{name}` is an object or array; the importer maps scalar members only"
            ),
            RowFault::DuplicateMember { name } => write!(f, "duplicate member `{name}`"),
            RowFault::TooManyMembers { limit } => {
                write!(f, "more than {limit} members in one row")
            }
            RowFault::MissingKey { column } => write!(f, "missing key column `{column}`"),
            RowFault::NullKey { column } => write!(f, "key column `{column}` is null"),
            RowFault::KeyType {
                column,
                expected,
                found,
            } => write!(
                f,
                "key column `{column}`: expected {}, found {found}",
                expected.name()
            ),
            RowFault::MissingRequiredField { field } => {
                write!(f, "required field `{field}` is absent or null")
            }
            RowFault::FieldType {
                field,
                expected,
                found,
            } => write!(
                f,
                "field `{field}`: expected {}, found {found}",
                expected.name()
            ),
            RowFault::UnrecognizedMember { name } => write!(f, "unrecognized member `{name}`"),
            RowFault::DuplicateKey => {
                write!(f, "a durable entry with this key already exists")
            }
            RowFault::UniqueIndexCollision => {
                write!(f, "this row collides with another on a unique index")
            }
        }
    }
}

/// Why a batch commit did not confirm — an operational fault, distinct from a row-data fault.
#[derive(Debug)]
pub enum CommitFault {
    /// The store handle was poisoned by an earlier interrupted commit.
    Poisoned,
    /// The engine could not open or complete the transaction.
    Engine(marrow_kernel::durable::StoreError),
    /// A non-engine kernel fault surfaced during a write (corruption, value range, or poison),
    /// carried as its stable dotted code.
    Kernel { code: Code },
    /// The engine confirmed that the batch did not commit.
    Aborted,
    /// The batch invocation did not complete; recovery classified whether its
    /// staged durable state is known old, known new, or unknown.
    Incomplete { durable: DurableCommitState },
}

impl std::fmt::Display for CommitFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommitFault::Poisoned => {
                write!(
                    f,
                    "the store handle is poisoned by an earlier interrupted commit"
                )
            }
            CommitFault::Engine(error) => write!(f, "the engine transaction failed: {error}"),
            CommitFault::Kernel { code } => {
                write!(f, "a durable write faulted ({})", code.as_str())
            }
            CommitFault::Aborted => write!(f, "the batch commit aborted"),
            CommitFault::Incomplete { durable } => {
                write!(
                    f,
                    "the batch invocation was incomplete ({})",
                    durable.as_str()
                )
            }
        }
    }
}

/// Why an import failed. A row fault names the 1-based line so the source can be corrected; the
/// [`committed`](ImportError::committed) prefix is intact and the caller may discard the store.
#[derive(Debug)]
pub enum ImportError {
    /// The store could not be opened (not provisioned, incomplete, held, or corrupt). No store
    /// write occurred.
    Open(OpenError),
    /// The exact-binding gate refused the presented image under the lock and before any
    /// engine call. Import never rebinds; `marrow run --store` performs the explicit rebind.
    Refused(AdmissionRefusal),
    /// The target root's shape is not importable from flat scalar rows. No store write occurred.
    UnsupportedShape(ShapeFault),
    /// Effective authority denied the write: the store's ceiling intersected with the import
    /// grant does not cover a durable write. No store write occurred.
    Denied,
    /// A source row did not decode or map to the target. Names the 1-based line; the batches
    /// committed before it stay in the store.
    Row {
        line: u64,
        fault: RowFault,
        committed: ImportReport,
    },
    /// A batch invocation did not complete. The store holds the earlier committed batches;
    /// [`CommitFault`] says whether this batch is known not to have landed or whether recovery
    /// classified its durable state separately.
    Commit {
        fault: CommitFault,
        committed: ImportReport,
    },
    /// Reading the source failed. The batches committed before the read error stay in the store.
    Io {
        error: std::io::Error,
        committed: ImportReport,
    },
}

impl ImportError {
    /// The stable dotted code a tool reports.
    pub fn code(&self) -> Code {
        match self {
            ImportError::Open(error) => error.code(),
            ImportError::Refused(refusal) => refusal.code(),
            ImportError::UnsupportedShape(_) => Code::CliDurableUnsupported,
            ImportError::Denied => Code::RunAuthority,
            ImportError::Row { .. } => Code::ConfigInvalid,
            ImportError::Commit { .. } => Code::RunCommit,
            ImportError::Io { .. } => Code::IoRead,
        }
    }

    /// The rows committed before this error, or a zero report for failures that wrote nothing.
    pub fn committed(&self) -> ImportReport {
        match self {
            ImportError::Row { committed, .. }
            | ImportError::Commit { committed, .. }
            | ImportError::Io { committed, .. } => *committed,
            ImportError::Open(_)
            | ImportError::Refused(_)
            | ImportError::UnsupportedShape(_)
            | ImportError::Denied => ImportReport {
                rows_imported: 0,
                batches_committed: 0,
            },
        }
    }
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Open(error) => write!(f, "{error}"),
            ImportError::Refused(refusal) => write!(f, "{refusal}"),
            ImportError::UnsupportedShape(fault) => {
                write!(f, "the target root is not importable: {fault}")
            }
            ImportError::Denied => write!(
                f,
                "the store does not permit a durable write, so the import is denied"
            ),
            ImportError::Row {
                line,
                fault,
                committed,
            } => write!(
                f,
                "line {line}: {fault} ({} row(s) already committed)",
                committed.rows_imported
            ),
            ImportError::Commit { fault, committed } => write!(
                f,
                "a batch commit failed: {fault} ({} row(s) already committed)",
                committed.rows_imported
            ),
            ImportError::Io { error, committed } => write!(
                f,
                "reading the import source failed: {error} ({} row(s) already committed)",
                committed.rows_imported
            ),
        }
    }
}

impl std::error::Error for ImportError {}

/// Populate the persistent store at `dir` from flat-scalar JSONL `source`, creating one durable
/// entry of the `target` root per line through the path kernel. Opens the store under the
/// prepared image's roots with a whole-payload import site on the target root, replacing the
/// sites the program declares (taking the single-owner lock and admitting the image under the
/// exact active binding before the engine opens), resolves a full write grant, and commits the
/// rows in bounded batches (see [`ImportLimits`]). The store is closed when the import
/// returns.
///
/// Each line is a JSON object whose members are the root's key columns and top-level scalar
/// fields, by source name. A value is a JSON string, integer, or boolean; `null` (or an absent
/// member) leaves a sparse field absent. A required field, or any key column, must be present
/// and non-null. An unrecognized member, a type mismatch, a duplicate key, or a malformed line
/// is a typed [`ImportError::Row`] naming the line.
///
/// `grant` is the invocation grant the privileged host minted; a grant without write coverage
/// is denied at the first batch's session open ([`ImportError::Denied`]) before any write,
/// because effective authority is `demand ∩ ceiling ∩ grant`.
pub fn import_jsonl(
    dir: &Path,
    prepared: PreparedImage,
    target: ImportTarget,
    source: impl BufRead,
    grant: InvocationGrant,
    limits: ImportLimits,
) -> Result<ImportReport, ImportError> {
    let (image, projection) = prepared.into_parts();
    let projection = projection.ok_or(ImportError::UnsupportedShape(ShapeFault::NotExecutable))?;
    let plan =
        RowPlan::resolve(projection.roots(), &target).map_err(ImportError::UnsupportedShape)?;

    // Reproject onto the importer's own site table: one whole-payload create site on the
    // target root. Sites are supplied per open and never persisted, so the import site is
    // independent of whatever operation sites the running program declares. The row plan
    // already proved the target root is declared, so the reprojection resolves.
    let mut builder = projection.reproject();
    builder.site(target.root, SiteTarget::whole_payload());
    let projection = builder
        .finish()
        .expect("the row plan refused a target root the projection does not declare");

    // The exact-binding gate runs under the single-owner lock and before any engine call, so
    // a stale, foreign, over-demanding, or incompletely mapped image opens no engine or session
    // and writes nothing. The pin is derived over the reprojection the engine actually opens
    // under; its numbering is the roots', which the reprojection keeps.
    let admission = ImageAdmission::derive(&image, projection);
    let mut opened = open_admitted(
        dir,
        marrow_kernel::durable::NativeOpenAccess::ReadWrite,
        Seam::NONE,
        |head| admission.admit(head, BindingStrictness::Exact),
    )
    .map_err(|error| match error {
        AdmitError::Open(error) => ImportError::Open(error),
        AdmitError::Refused(refusal) => ImportError::Refused(refusal),
    })?;

    match import_rows_into(&mut opened, &plan, source, grant, limits) {
        Ok(report) => Ok(report),
        Err(ImportRunError::Reported(error)) => Err(error),
        Err(ImportRunError::Indeterminate {
            recovery,
            committed,
        }) => {
            let (durable, _reopened) = opened.resolve_recovery(recovery);
            Err(ImportError::Commit {
                fault: CommitFault::Incomplete { durable },
                committed,
            })
        }
    }
}

enum ImportRunError {
    Reported(ImportError),
    Indeterminate {
        recovery: CommitRecovery,
        committed: ImportReport,
    },
}

impl From<ImportError> for ImportRunError {
    fn from(error: ImportError) -> Self {
        Self::Reported(error)
    }
}

/// The resolved plan for mapping a row to the target root's whole-payload write: the key
/// columns (name paired with the kind the schema owns) and one slot descriptor per top-level
/// field, checked once so the per-row loop maps without re-inspecting the schema. Refuses a
/// shape the flat importer cannot represent.
struct RowPlan {
    key_columns: Vec<KeyColumnPlan>,
    fields: Vec<FieldSlot>,
}

/// One resolved key column: its source name (for row lookup) and the scalar kind the schema
/// declares for it.
struct KeyColumnPlan {
    name: String,
    kind: ScalarKind,
}

/// One importable top-level field, in schema-declaration order: its source name, scalar kind,
/// and required flag.
struct FieldSlot {
    name: String,
    kind: ScalarKind,
    required: bool,
}

impl RowPlan {
    fn resolve(schemas: &[StoreSchema], target: &ImportTarget) -> Result<Self, ShapeFault> {
        let schema = schemas
            .get(target.root as usize)
            .ok_or(ShapeFault::RootOutOfRange {
                root: target.root,
                declared: schemas.len(),
            })?;

        if !schema.groups().is_empty() || !schema.branches().is_empty() {
            return Err(ShapeFault::HasGroupsOrBranches {
                root: schema.root_name().to_string(),
            });
        }

        if schema.key().len() != target.key_columns.len() {
            return Err(ShapeFault::KeyArity {
                root: schema.root_name().to_string(),
                declared: schema.key().len(),
                named: target.key_columns.len(),
            });
        }
        let mut key_columns = Vec::with_capacity(schema.key().len());
        for (name, &kind) in target.key_columns.iter().zip(schema.key()) {
            if !importable_scalar(kind) {
                return Err(ShapeFault::KeyScalarUnsupported {
                    column: name.clone(),
                    kind,
                });
            }
            key_columns.push(KeyColumnPlan {
                name: name.clone(),
                kind,
            });
        }

        let mut fields = Vec::with_capacity(schema.fields().len());
        for field in schema.fields() {
            let Some(kind) = field.shape().scalar_kind() else {
                return Err(ShapeFault::FieldNotScalar {
                    field: field.name().to_string(),
                });
            };
            if !importable_scalar(kind) {
                return Err(ShapeFault::FieldScalarUnsupported {
                    field: field.name().to_string(),
                    kind,
                });
            }
            fields.push(FieldSlot {
                name: field.name().to_string(),
                kind,
                required: field.required(),
            });
        }

        Ok(Self {
            key_columns,
            fields,
        })
    }

    /// Map one decoded row object to its key-path and whole-entry payload, in schema order.
    /// A member is consumed by exactly one column or field; a leftover member is an
    /// unrecognized column and rejects the row.
    fn map_row(&self, mut object: RowObject) -> Result<(Vec<KeyScalar>, EntryValue), RowFault> {
        let mut keys = Vec::with_capacity(self.key_columns.len());
        for column in &self.key_columns {
            let value = object
                .take(&column.name)
                .ok_or_else(|| RowFault::MissingKey {
                    column: column.name.clone(),
                })?;
            if value == Json::Null {
                return Err(RowFault::NullKey {
                    column: column.name.clone(),
                });
            }
            let found = describe(&value);
            let scalar = key_scalar(column.kind, value).ok_or_else(|| RowFault::KeyType {
                column: column.name.clone(),
                expected: column.kind,
                found,
            })?;
            keys.push(scalar);
        }

        let mut fields = Vec::with_capacity(self.fields.len());
        for slot in &self.fields {
            let slot_value = match object.take(&slot.name) {
                None | Some(Json::Null) => {
                    if slot.required {
                        return Err(RowFault::MissingRequiredField {
                            field: slot.name.clone(),
                        });
                    }
                    None
                }
                Some(value) => {
                    let found = describe(&value);
                    Some(
                        value_domain(slot.kind, value).ok_or_else(|| RowFault::FieldType {
                            field: slot.name.clone(),
                            expected: slot.kind,
                            found,
                        })?,
                    )
                }
            };
            fields.push(slot_value);
        }

        if let Some(extra) = object.remaining_name() {
            return Err(RowFault::UnrecognizedMember {
                name: extra.to_string(),
            });
        }

        Ok((
            keys,
            EntryValue {
                fields,
                groups: Vec::new(),
            },
        ))
    }
}

/// The import core: stream `source`, mapping each line through `plan` and creating it through
/// the path kernel in bounded batches. Every write is a kernel
/// [`create_entry`](Durable::create_entry) — authority resolved, site resolved,
/// planner-mediated, indexes maintained.
fn import_rows_into<H: SessionHost>(
    store: &mut H,
    plan: &RowPlan,
    mut source: impl BufRead,
    grant: InvocationGrant,
    limits: ImportLimits,
) -> Result<ImportReport, ImportRunError> {
    let write_demand = DemandCoverage {
        read: false,
        write: true,
    };

    let mut report = ImportReport {
        rows_imported: 0,
        batches_committed: 0,
    };
    let mut line_no: u64 = 0;
    let mut buf: Vec<u8> = Vec::new();

    // The batch accumulates fully-mapped rows — each tagged with its source line so a staging
    // fault names the offending row, not the batch boundary — before a single transaction
    // stages and commits them, so a decode/map fault never leaves a half-open transaction and
    // the memory footprint is one batch.
    let mut batch: Batch = Vec::new();

    loop {
        buf.clear();
        let read =
            read_line_bounded(&mut source, &mut buf, limits.max_line_bytes).map_err(|error| {
                ImportError::Io {
                    error,
                    committed: report,
                }
            })?;
        if read == LineRead::Eof {
            break;
        }
        line_no += 1;

        if is_blank(&buf) {
            continue; // JSONL tolerates blank separator lines.
        }

        let object = parse_row_object(&buf, &limits).map_err(|fault| ImportError::Row {
            line: line_no,
            fault,
            committed: report,
        })?;
        let (keys, entry) = plan.map_row(object).map_err(|fault| ImportError::Row {
            line: line_no,
            fault,
            committed: report,
        })?;
        batch.push((line_no, keys, entry));

        if batch.len() >= limits.batch_rows {
            commit_batch(store, grant, write_demand, &mut batch, &mut report)?;
        }
    }

    if !batch.is_empty() {
        commit_batch(store, grant, write_demand, &mut batch, &mut report)?;
    }

    Ok(report)
}

/// One batch of fully-mapped rows awaiting commit: each is its source line, key-path, and
/// whole-entry payload.
type Batch = Vec<(u64, Vec<KeyScalar>, EntryValue)>;

/// Stage and commit one batch of mapped rows in a single kernel transaction. Drains `batch`;
/// advances `report` only on a confirmed commit. A denied session, a duplicate key (named at
/// its own source line), or a non-confirming commit is a typed error carrying the committed
/// prefix.
fn commit_batch<H: SessionHost>(
    store: &mut H,
    grant: InvocationGrant,
    demand: DemandCoverage,
    batch: &mut Batch,
    report: &mut ImportReport,
) -> Result<(), ImportRunError> {
    let mut txn = store
        .txn_session(grant, demand)
        .map_err(|error| match error {
            SessionError::Denied => ImportError::Denied,
            SessionError::Poisoned => ImportError::Commit {
                fault: CommitFault::Poisoned,
                committed: *report,
            },
            SessionError::Engine(engine) => ImportError::Commit {
                fault: CommitFault::Engine(engine),
                committed: *report,
            },
        })?;

    let site = txn.site(0);
    let staged = batch.len() as u64;
    for (line, keys, entry) in batch.drain(..) {
        match txn.create_entry(&site, &keys, entry) {
            Ok(CreateOutcome::Created) => {}
            Ok(CreateOutcome::AlreadyPresent) => {
                // The transaction drops un-committed (rolls back this batch). The fault names
                // the offending row's own source line, not the batch boundary.
                return Err(ImportError::Row {
                    line,
                    fault: RowFault::DuplicateKey,
                    committed: *report,
                }
                .into());
            }
            // A unique-index collision is a row-data fault: two source rows carry the same
            // value for a `unique` index. Name the offending row; the batch rolls back on drop.
            Err(KernelFault::UniqueIndexViolation) => {
                return Err(ImportError::Row {
                    line,
                    fault: RowFault::UniqueIndexCollision,
                    committed: *report,
                }
                .into());
            }
            // Any other kernel fault is operational (engine, corruption, poison, value range),
            // not a correctable row.
            Err(KernelFault::Engine(engine)) => {
                return Err(ImportError::Commit {
                    fault: CommitFault::Engine(engine),
                    committed: *report,
                }
                .into());
            }
            Err(other) => {
                return Err(ImportError::Commit {
                    fault: CommitFault::Kernel { code: other.code() },
                    committed: *report,
                }
                .into());
            }
        }
    }

    match txn.commit() {
        CommitResult::Committed => {
            report.rows_imported += staged;
            report.batches_committed += 1;
            Ok(())
        }
        CommitResult::Aborted => Err(ImportError::Commit {
            fault: CommitFault::Aborted,
            committed: *report,
        }
        .into()),
        CommitResult::Indeterminate(recovery) => Err(ImportRunError::Indeterminate {
            recovery,
            committed: *report,
        }),
        CommitResult::SessionFinished => Err(ImportError::Commit {
            fault: CommitFault::Incomplete {
                durable: DurableCommitState::Unknown,
            },
            committed: *report,
        }
        .into()),
    }
}

/// Whether the flat importer maps a scalar kind. `int`, `bool`, and `string` are the runtime
/// domain the kernel exercises; a temporal, byte, or other scalar has no unambiguous JSON form
/// here and is refused rather than guessed.
fn importable_scalar(kind: ScalarKind) -> bool {
    matches!(kind, ScalarKind::Int | ScalarKind::Bool | ScalarKind::Str)
}

/// Mint the key scalar of `kind` from a JSON value, or `None` on a type mismatch (no coercion).
fn key_scalar(kind: ScalarKind, value: Json) -> Option<KeyScalar> {
    Some(match (kind, value) {
        (ScalarKind::Int, Json::Int(n)) => KeyScalar::Int(n),
        (ScalarKind::Bool, Json::Bool(b)) => KeyScalar::Bool(b),
        (ScalarKind::Str, Json::Str(s)) => KeyScalar::Str(s),
        _ => return None,
    })
}

/// Build the value domain of `kind` from a JSON scalar, or `None` on a type mismatch.
fn value_domain(kind: ScalarKind, value: Json) -> Option<ValueDomain> {
    let scalar = match (kind, value) {
        (ScalarKind::Int, Json::Int(n)) => RuntimeScalar::Int(n),
        (ScalarKind::Bool, Json::Bool(b)) => RuntimeScalar::Bool(b),
        (ScalarKind::Str, Json::Str(s)) => RuntimeScalar::Str(s),
        _ => return None,
    };
    Some(ValueDomain::Scalar(scalar))
}

/// The kind of a JSON value in a type-mismatch report.
fn describe(value: &Json) -> &'static str {
    match value {
        Json::Str(_) => "a string",
        Json::Int(_) => "an integer",
        Json::Bool(_) => "a boolean",
        Json::Null => "null",
        Json::Array(_) => "an array",
        Json::Object(_) => "an object",
    }
}

/// One decoded row object: its members in source order. Lookup removes a member so each is
/// consumed once and a leftover is a detectable unrecognized column.
#[derive(Debug)]
struct RowObject {
    members: Vec<(String, Json)>,
}

impl RowObject {
    /// Remove and return the value of member `name`, or `None` if absent.
    fn take(&mut self, name: &str) -> Option<Json> {
        let position = self.members.iter().position(|(key, _)| key == name)?;
        Some(self.members.remove(position).1)
    }

    /// The name of any member not yet consumed, for an unrecognized-column report.
    fn remaining_name(&self) -> Option<&str> {
        self.members.first().map(|(name, _)| name.as_str())
    }
}

/// Whether a line is blank (only ASCII whitespace) — a tolerated JSONL separator.
fn is_blank(line: &[u8]) -> bool {
    line.iter().all(|b| b.is_ascii_whitespace())
}

/// Decode one line as a flat JSON object of scalar members through the workspace's one JSON
/// lexer. Insignificant whitespace is accepted; a nested value, a duplicate member, a member
/// count over the limit, and trailing content are each a typed refusal. The input slice is
/// already bounded by the line limit; each string is bounded by the lexer.
fn parse_row_object(line: &[u8], limits: &ImportLimits) -> Result<RowObject, RowFault> {
    let text = std::str::from_utf8(line).map_err(|_| RowFault::Malformed(WireError::Malformed))?;
    let mut lexer = Lexer::new(text, limits.max_string_bytes);
    lexer.skip_ws();
    lexer.expect(b'{').map_err(RowFault::Malformed)?;
    let mut members: Vec<(String, Json)> = Vec::new();
    lexer.skip_ws();
    if !lexer.take(b'}') {
        loop {
            lexer.skip_ws();
            let key = lexer.string().map_err(RowFault::Malformed)?;
            if members.iter().any(|(existing, _)| *existing == key) {
                return Err(RowFault::DuplicateMember { name: key });
            }
            if members.len() >= limits.max_fields_per_row {
                return Err(RowFault::TooManyMembers {
                    limit: limits.max_fields_per_row,
                });
            }
            lexer.skip_ws();
            lexer.expect(b':').map_err(RowFault::Malformed)?;
            lexer.skip_ws();
            if matches!(lexer.peek(), Some(b'{' | b'[')) {
                return Err(RowFault::Nested { name: key });
            }
            let start = lexer.offset();
            let value = lexer.scalar().map_err(RowFault::Malformed)?;
            // The lexer reads any digit run; an integer is held to the wire's one canonical
            // spelling here, so `01` is refused rather than read as `1`.
            if matches!(value, Json::Int(_)) && lexer.since(start) != encode(&value) {
                return Err(RowFault::Malformed(WireError::Noncanonical));
            }
            members.push((key, value));
            lexer.skip_ws();
            if lexer.take(b',') {
                continue;
            }
            if lexer.take(b'}') {
                break;
            }
            return Err(RowFault::Malformed(WireError::Malformed));
        }
    }
    lexer.skip_ws();
    if !lexer.at_end() {
        return Err(RowFault::Malformed(WireError::Malformed));
    }
    Ok(RowObject { members })
}

// ---------------------------------------------------------------------------
// Bounded line reader
// ---------------------------------------------------------------------------

/// Whether a bounded line read reached end of input or read a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineRead {
    Line,
    Eof,
}

/// Read one line (up to and excluding the newline) into `buf`, refusing a line longer than
/// `limit` before it is fully buffered. Returns [`LineRead::Eof`] when no more input remains.
/// A trailing line without a newline is returned as its own line.
fn read_line_bounded(
    source: &mut impl BufRead,
    buf: &mut Vec<u8>,
    limit: usize,
) -> std::io::Result<LineRead> {
    let mut any = false;
    loop {
        let available = source.fill_buf()?;
        if available.is_empty() {
            return Ok(if any { LineRead::Line } else { LineRead::Eof });
        }
        any = true;
        if let Some(newline) = available.iter().position(|&b| b == b'\n') {
            enforce_limit(buf.len() + newline, limit)?;
            buf.extend_from_slice(&available[..newline]);
            source.consume(newline + 1); // drop the newline itself.
            return Ok(LineRead::Line);
        }
        enforce_limit(buf.len() + available.len(), limit)?;
        let taken = available.len();
        buf.extend_from_slice(available);
        source.consume(taken);
    }
}

fn enforce_limit(len: usize, limit: usize) -> std::io::Result<()> {
    if len > limit {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("a source line exceeds {limit} bytes"),
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_object_parses_to_its_members() {
        let object = parse_row_object(
            br#"{"id": 7, "name": "ok", "active": true, "note": null}"#,
            &ImportLimits::DEFAULT,
        )
        .expect("parse");
        assert_eq!(object.members.len(), 4);
    }

    #[test]
    fn string_escapes_and_unicode_decode() {
        // Exercises the JSON escapes and \u decoding (BMP and a surrogate pair)
        // while keeping the byte-string literal ASCII.
        let mut object = parse_row_object(
            br#"{"t": "a\t\"b\"\nA\uD83D\uDE00\u00e9"}"#,
            &ImportLimits::DEFAULT,
        )
        .expect("parse");
        let Json::Str(text) = object.take("t").expect("member") else {
            panic!("expected a string");
        };
        assert_eq!(text, "a\t\"b\"\nA\u{1F600}\u{00E9}");
    }

    #[test]
    fn nested_and_float_and_dupes_are_refused() {
        let limits = ImportLimits {
            max_fields_per_row: 2,
            max_string_bytes: 4,
            ..ImportLimits::DEFAULT
        };
        let refusal = |line: &[u8]| parse_row_object(line, &limits).expect_err("refused");
        assert_eq!(
            refusal(br#"{"a": {"b": 1}}"#),
            RowFault::Nested { name: "a".into() }
        );
        assert_eq!(
            refusal(br#"{"a": [1,2]}"#),
            RowFault::Nested { name: "a".into() }
        );
        assert_eq!(
            refusal(br#"{"a": 1.5}"#),
            RowFault::Malformed(WireError::Malformed)
        );
        assert_eq!(
            refusal(br#"{"a": 1, "a": 2}"#),
            RowFault::DuplicateMember { name: "a".into() }
        );
        assert_eq!(
            refusal(br#"{"a": 01}"#),
            RowFault::Malformed(WireError::Noncanonical)
        );
        assert_eq!(
            refusal(br#"{"a": 1} junk"#),
            RowFault::Malformed(WireError::Malformed)
        );
        assert_eq!(
            refusal(br#"{"a": 1, "b": 2, "c": 3}"#),
            RowFault::TooManyMembers { limit: 2 }
        );
        assert_eq!(
            refusal(br#"{"a": "12345"}"#),
            RowFault::Malformed(WireError::StringLimit)
        );
        assert_eq!(
            refusal(&[b'{', 0xff, b'}']),
            RowFault::Malformed(WireError::Malformed)
        );
    }

    #[test]
    fn a_row_maps_keys_and_sparse_fields() {
        let plan = RowPlan {
            key_columns: vec![KeyColumnPlan {
                name: "id".into(),
                kind: ScalarKind::Int,
            }],
            fields: vec![
                FieldSlot {
                    name: "value".into(),
                    kind: ScalarKind::Int,
                    required: true,
                },
                FieldSlot {
                    name: "label".into(),
                    kind: ScalarKind::Str,
                    required: false,
                },
            ],
        };
        let object =
            parse_row_object(br#"{"id": 3, "value": 42}"#, &ImportLimits::DEFAULT).expect("parse");
        let (keys, entry) = plan.map_row(object).expect("map");
        assert_eq!(keys, vec![KeyScalar::Int(3)]);
        assert_eq!(entry.fields.len(), 2);
        assert!(entry.fields[0].is_some(), "required value present");
        assert!(entry.fields[1].is_none(), "sparse label absent");
    }

    #[test]
    fn a_missing_required_field_or_key_is_a_row_error() {
        let plan = RowPlan {
            key_columns: vec![KeyColumnPlan {
                name: "id".into(),
                kind: ScalarKind::Int,
            }],
            fields: vec![FieldSlot {
                name: "value".into(),
                kind: ScalarKind::Int,
                required: true,
            }],
        };
        let map = |json: &[u8]| {
            plan.map_row(parse_row_object(json, &ImportLimits::DEFAULT).expect("parse"))
                .expect_err("map should fail")
        };
        assert!(matches!(
            map(br#"{"id": 1}"#),
            RowFault::MissingRequiredField { .. }
        ));
        assert!(matches!(
            map(br#"{"value": 1}"#),
            RowFault::MissingKey { .. }
        ));
        assert!(matches!(
            map(br#"{"id": 1, "value": 2, "x": 3}"#),
            RowFault::UnrecognizedMember { .. }
        ));
        assert!(matches!(
            map(br#"{"id": "not-int", "value": 2}"#),
            RowFault::KeyType { .. }
        ));
        assert!(matches!(
            map(br#"{"id": null, "value": 2}"#),
            RowFault::NullKey { .. }
        ));
    }

    #[test]
    fn an_over_long_line_is_refused() {
        let limits = ImportLimits {
            max_line_bytes: 8,
            ..ImportLimits::DEFAULT
        };
        let mut buf = Vec::new();
        let mut source = std::io::Cursor::new(b"0123456789\n".to_vec());
        assert!(read_line_bounded(&mut source, &mut buf, limits.max_line_bytes).is_err());
    }

    #[test]
    fn the_line_reader_splits_and_handles_a_missing_final_newline() {
        let mut source = std::io::Cursor::new(b"one\ntwo\nthree".to_vec());
        let mut lines = Vec::new();
        loop {
            let mut buf = Vec::new();
            match read_line_bounded(&mut source, &mut buf, 1 << 20).expect("read") {
                LineRead::Line => lines.push(String::from_utf8(buf).unwrap()),
                LineRead::Eof => break,
            }
        }
        assert_eq!(lines, vec!["one", "two", "three"]);
    }
}
