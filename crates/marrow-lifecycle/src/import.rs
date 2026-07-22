//! The trusted bulk importer: a closed private lifecycle-maintenance mode that populates a
//! provisioned store from external flat-scalar JSONL rows, every write passing the typed path
//! kernel.
//!
//! # Why this is not raw seeding
//!
//! The importer maps each external row to a typed durable place and creates it through the
//! kernel's [`create_entry`](marrow_kernel::durable::Durable::create_entry) — so authority is
//! resolved (`demand ∩ ceiling ∩ grant`), the site is resolved from the store schema, the
//! consequence planner writes the entry marker and field leaves, and managed indexes are
//! maintained. It never opens the byte engine, mints a raw cell key, or holds a transaction
//! handle. External, untyped rows have no valid cell form until the kernel places them, so
//! import goes through the full write algebra, not a byte copy.
//!
//! # The closed lifecycle boundary
//!
//! [`import_jsonl`] opens the persistent store through [`open`](crate::open), which internally
//! retains the non-cloneable, non-serializable single-owner lock for the store's entire open
//! lifetime.
//! Nothing below this crate depends on `marrow-lifecycle` (the Cargo trust boundary: only the
//! privileged CLI host does), so no bytecode, client-wire, or host-adapter path can enter the
//! import mode. The engine-generic core is crate-private and adds no privilege a caller with
//! direct kernel access would not already have.
//!
//! # Bounds (campaign law 9)
//!
//! Every input is bounded before allocation: each JSONL line, the field count of a row, and
//! each string value are capped by [`ImportLimits`], and the store is populated in bounded
//! batches (one engine transaction per [`ImportLimits::batch_rows`] rows), so a whole-corpus
//! import never materializes the corpus — memory is bounded by one line plus one batch. Batches
//! are individually atomic; the import is *not* one transaction, so a mid-import failure leaves
//! the committed prefix and reports its size, letting the caller discard and re-provision.

use std::io::BufRead;
use std::path::Path;

use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::{RuntimeScalar, ScalarKind, ValueShape};
use marrow_kernel::durable::{
    CommitRecovery, CommitResult, CreateOutcome, DemandCoverage, Durable, DurableCommitState,
    EntryValue, InvocationGrant, KernelFault, SessionError, SessionHost, SiteSpec, SiteTarget,
    StoreSchema,
};
use marrow_kernel::equality::ValueDomain;

use crate::provision::{OpenError, open};

/// The bounds every import obeys before it allocates (campaign law 9). The defaults suit a
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
/// test) asserts the category rather than parsing prose; the malformed-JSON detail is retained
/// as text because it is inherently free-form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowFault {
    /// The line was not a valid flat-scalar JSON object; carries the decoder's detail.
    Malformed(String),
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
            RowFault::Malformed(detail) => write!(f, "{detail}"),
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
    Kernel { code: &'static str },
    /// A staged entry left a required field unset (a defense-in-depth reconcile fault; the
    /// mapper normally rejects such a row before staging).
    RequiredMissing { field: String },
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
            CommitFault::Kernel { code } => write!(f, "a durable write faulted ({code})"),
            CommitFault::RequiredMissing { field } => {
                write!(f, "a staged entry left required field `{field}` unset")
            }
            CommitFault::Aborted => write!(f, "the batch commit aborted"),
            CommitFault::Incomplete { durable } => {
                let state = match durable {
                    DurableCommitState::KnownOld => "known_old",
                    DurableCommitState::KnownNew => "known_new",
                    DurableCommitState::Unknown => "unknown",
                };
                write!(f, "the batch invocation was incomplete ({state})")
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
    pub fn code(&self) -> &'static str {
        use marrow_codes::Code;
        match self {
            ImportError::Open(error) => error.code(),
            ImportError::UnsupportedShape(_) => Code::CliDurableUnsupported.as_str(),
            ImportError::Denied => Code::RunAuthority.as_str(),
            ImportError::Row { .. } => Code::ConfigInvalid.as_str(),
            ImportError::Commit { .. } => Code::RunCommit.as_str(),
            ImportError::Io { .. } => Code::IoRead.as_str(),
        }
    }

    /// The rows committed before this error, or a zero report for failures that wrote nothing.
    pub fn committed(&self) -> ImportReport {
        match self {
            ImportError::Row { committed, .. }
            | ImportError::Commit { committed, .. }
            | ImportError::Io { committed, .. } => *committed,
            ImportError::Open(_) | ImportError::UnsupportedShape(_) | ImportError::Denied => {
                ImportReport {
                    rows_imported: 0,
                    batches_committed: 0,
                }
            }
        }
    }
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::Open(error) => write!(f, "{error}"),
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
/// entry of the `target` root per line through the path kernel. Opens the store under `schemas`
/// with a whole-payload import site on the target root (taking the single-owner lock), resolves
/// a full write grant, and commits the rows in bounded batches (see [`ImportLimits`]). The store
/// is closed when the import returns.
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
    schemas: Vec<StoreSchema>,
    target: ImportTarget,
    source: impl BufRead,
    grant: InvocationGrant,
    limits: ImportLimits,
) -> Result<ImportReport, ImportError> {
    let plan = RowPlan::resolve(&schemas, &target).map_err(ImportError::UnsupportedShape)?;

    // Open with the importer's own site table: one whole-payload create site on the target
    // root. Sites are supplied per open and never persisted, so the import site is independent
    // of whatever operation sites the running program declares.
    let sites = vec![SiteSpec {
        root: target.root,
        target: SiteTarget::WholePayload,
    }];
    let mut opened = open(dir, schemas, sites).map_err(ImportError::Open)?;

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

        if !schema.groups.is_empty() || !schema.branches.is_empty() {
            return Err(ShapeFault::HasGroupsOrBranches {
                root: schema.root_name.clone(),
            });
        }

        if schema.key.len() != target.key_columns.len() {
            return Err(ShapeFault::KeyArity {
                root: schema.root_name.clone(),
                declared: schema.key.len(),
                named: target.key_columns.len(),
            });
        }
        let mut key_columns = Vec::with_capacity(schema.key.len());
        for (name, kind) in target.key_columns.iter().zip(&schema.key) {
            if !importable_scalar(*kind) {
                return Err(ShapeFault::KeyScalarUnsupported {
                    column: name.clone(),
                    kind: *kind,
                });
            }
            key_columns.push(KeyColumnPlan {
                name: name.clone(),
                kind: *kind,
            });
        }

        let mut fields = Vec::with_capacity(schema.fields.len());
        for field in &schema.fields {
            let ValueShape::Scalar(kind) = &field.shape else {
                return Err(ShapeFault::FieldNotScalar {
                    field: field.name.clone(),
                });
            };
            if !importable_scalar(*kind) {
                return Err(ShapeFault::FieldScalarUnsupported {
                    field: field.name.clone(),
                    kind: *kind,
                });
            }
            fields.push(FieldSlot {
                name: field.name.clone(),
                kind: *kind,
                required: field.required,
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
            if value == JsonScalar::Null {
                return Err(RowFault::NullKey {
                    column: column.name.clone(),
                });
            }
            let found = value.describe();
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
                None | Some(JsonScalar::Null) => {
                    if slot.required {
                        return Err(RowFault::MissingRequiredField {
                            field: slot.name.clone(),
                        });
                    }
                    None
                }
                Some(value) => {
                    let found = value.describe();
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
/// the path kernel in bounded batches. Crate-private, with one caller ([`import_jsonl`]); the
/// split keeps `import_jsonl` a thin lock-guarded open plus site setup and isolates the bounded
/// streaming loop here. Every write is a kernel [`create_entry`](Durable::create_entry) —
/// authority resolved, site resolved, planner-mediated, indexes maintained.
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
        CommitResult::RequiredMissing { field, .. } => Err(ImportError::Commit {
            fault: CommitFault::RequiredMissing { field },
            committed: *report,
        }
        .into()),
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
fn key_scalar(kind: ScalarKind, value: JsonScalar) -> Option<KeyScalar> {
    Some(match (kind, value) {
        (ScalarKind::Int, JsonScalar::Int(n)) => KeyScalar::Int(n),
        (ScalarKind::Bool, JsonScalar::Bool(b)) => KeyScalar::Bool(b),
        (ScalarKind::Str, JsonScalar::Str(s)) => KeyScalar::Str(s),
        _ => return None,
    })
}

/// Build the value domain of `kind` from a JSON scalar, or `None` on a type mismatch.
fn value_domain(kind: ScalarKind, value: JsonScalar) -> Option<ValueDomain> {
    let scalar = match (kind, value) {
        (ScalarKind::Int, JsonScalar::Int(n)) => RuntimeScalar::Int(n),
        (ScalarKind::Bool, JsonScalar::Bool(b)) => RuntimeScalar::Bool(b),
        (ScalarKind::Str, JsonScalar::Str(s)) => RuntimeScalar::Str(s),
        _ => return None,
    };
    Some(ValueDomain::Scalar(scalar))
}

// ---------------------------------------------------------------------------
// Bounded flat-scalar JSONL decoder
// ---------------------------------------------------------------------------

/// A decoded JSON scalar — the whole value grammar the flat importer accepts. Nested objects,
/// arrays, and fractional/exponent numbers are rejected by the decoder before they reach here.
#[derive(Debug, Clone, PartialEq)]
enum JsonScalar {
    Str(String),
    Int(i64),
    Bool(bool),
    Null,
}

impl JsonScalar {
    fn describe(&self) -> &'static str {
        match self {
            JsonScalar::Str(_) => "a string",
            JsonScalar::Int(_) => "an integer",
            JsonScalar::Bool(_) => "a boolean",
            JsonScalar::Null => "null",
        }
    }
}

/// One decoded row object: its members in source order. Lookup removes a member so each is
/// consumed once and a leftover is a detectable unrecognized column.
struct RowObject {
    members: Vec<(String, JsonScalar)>,
}

impl RowObject {
    /// Remove and return the value of member `name`, or `None` if absent.
    fn take(&mut self, name: &str) -> Option<JsonScalar> {
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

/// Parse a bounded flat JSON object of scalar members, rejecting a non-UTF-8 line and mapping a
/// decode failure to a typed [`RowFault::Malformed`] carrying the decoder's detail. The UTF-8
/// check up front lets the decoder pass raw string bytes through without re-validating each
/// continuation byte.
fn parse_row_object(line: &[u8], limits: &ImportLimits) -> Result<RowObject, RowFault> {
    if std::str::from_utf8(line).is_err() {
        return Err(RowFault::Malformed(
            "the line is not valid UTF-8".to_string(),
        ));
    }
    decode_object(line, limits).map_err(RowFault::Malformed)
}

/// Decode a bounded flat JSON object of scalar members from a UTF-8-validated line. Rejects
/// nesting, arrays, non-integer numbers, duplicate keys, an over-limit member count, and
/// trailing content. Bounded by `limits` throughout; the input slice is already bounded by the
/// line limit.
fn decode_object(line: &[u8], limits: &ImportLimits) -> Result<RowObject, String> {
    let mut parser = JsonLine {
        bytes: line,
        pos: 0,
        max_string_bytes: limits.max_string_bytes,
    };
    parser.skip_ws();
    parser.expect(b'{')?;
    let mut members: Vec<(String, JsonScalar)> = Vec::new();

    parser.skip_ws();
    if parser.peek() == Some(b'}') {
        parser.pos += 1;
    } else {
        loop {
            parser.skip_ws();
            let key = parser.parse_string()?;
            if members.iter().any(|(existing, _)| *existing == key) {
                return Err(format!("duplicate member `{key}`"));
            }
            if members.len() >= limits.max_fields_per_row {
                return Err(format!(
                    "more than {} members in one row",
                    limits.max_fields_per_row
                ));
            }
            parser.skip_ws();
            parser.expect(b':')?;
            parser.skip_ws();
            let value = parser.parse_scalar()?;
            members.push((key, value));
            parser.skip_ws();
            match parser.next_byte() {
                Some(b',') => continue,
                Some(b'}') => break,
                Some(other) => {
                    return Err(format!("expected `,` or `}}`, found `{}`", other as char));
                }
                None => return Err("unterminated object".to_string()),
            }
        }
    }

    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err("trailing content after the object".to_string());
    }
    Ok(RowObject { members })
}

/// A cursor over one JSONL line's bytes.
struct JsonLine<'a> {
    bytes: &'a [u8],
    pos: usize,
    max_string_bytes: usize,
}

impl JsonLine<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn next_byte(&mut self) -> Option<u8> {
        let byte = self.bytes.get(self.pos).copied()?;
        self.pos += 1;
        Some(byte)
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b) if b == b' ' || b == b'\t' || b == b'\r' || b == b'\n')
        {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        match self.next_byte() {
            Some(found) if found == byte => Ok(()),
            Some(found) => Err(format!(
                "expected `{}`, found `{}`",
                byte as char, found as char
            )),
            None => Err(format!("expected `{}`, found end of line", byte as char)),
        }
    }

    /// Parse a JSON scalar value: a string, integer, `true`, `false`, or `null`. A nested
    /// object/array or a fractional/exponent number is a typed refusal.
    fn parse_scalar(&mut self) -> Result<JsonScalar, String> {
        match self.peek() {
            Some(b'"') => self.parse_string().map(JsonScalar::Str),
            Some(b't') | Some(b'f') => self.parse_bool(),
            Some(b'n') => self.parse_null(),
            Some(b) if b == b'-' || b.is_ascii_digit() => self.parse_integer(),
            Some(b'{') | Some(b'[') => {
                Err("nested objects and arrays are not importable".to_string())
            }
            Some(other) => Err(format!("unexpected value byte `{}`", other as char)),
            None => Err("expected a value, found end of line".to_string()),
        }
    }

    fn parse_bool(&mut self) -> Result<JsonScalar, String> {
        if self.consume_literal(b"true") {
            Ok(JsonScalar::Bool(true))
        } else if self.consume_literal(b"false") {
            Ok(JsonScalar::Bool(false))
        } else {
            Err("malformed boolean literal".to_string())
        }
    }

    fn parse_null(&mut self) -> Result<JsonScalar, String> {
        if self.consume_literal(b"null") {
            Ok(JsonScalar::Null)
        } else {
            Err("malformed null literal".to_string())
        }
    }

    fn consume_literal(&mut self, literal: &[u8]) -> bool {
        if self.bytes[self.pos..].starts_with(literal) {
            self.pos += literal.len();
            true
        } else {
            false
        }
    }

    /// Parse a JSON integer: optional `-`, then `0` alone or a non-zero leading digit run. A
    /// `.`, `e`, or `E` is rejected (floats are outside the scalar domain). Range-checked to
    /// `i64`.
    fn parse_integer(&mut self) -> Result<JsonScalar, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                // A leading zero must stand alone (no `0123`).
                if matches!(self.peek(), Some(d) if d.is_ascii_digit()) {
                    return Err("a leading zero is not a valid integer".to_string());
                }
            }
            Some(d) if d.is_ascii_digit() => {
                while matches!(self.peek(), Some(d) if d.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            _ => return Err("malformed number".to_string()),
        }
        if matches!(self.peek(), Some(b'.') | Some(b'e') | Some(b'E')) {
            return Err("fractional and exponent numbers are not importable".to_string());
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| "malformed number".to_string())?;
        text.parse::<i64>()
            .map(JsonScalar::Int)
            .map_err(|_| "integer out of the signed 64-bit range".to_string())
    }

    /// Parse a JSON string: an opening quote, UTF-8 content with the JSON escapes (`\" \\ \/ \b
    /// \f \n \r \t` and `\uXXXX` including surrogate pairs), and a closing quote. Bounded by
    /// `max_string_bytes`; a raw control byte or a lone/invalid escape is a typed refusal.
    fn parse_string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        // Accumulate bytes: pass-through bytes come from a UTF-8-validated line, and each escape
        // produces a valid scalar's bytes, so the whole is valid UTF-8 (checked once at the end).
        let mut out: Vec<u8> = Vec::new();
        loop {
            let byte = self.next_byte().ok_or("unterminated string")?;
            match byte {
                b'"' => break,
                b'\\' => {
                    let escape = self.next_byte().ok_or("unterminated escape")?;
                    match escape {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0C),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let ch = self.parse_unicode_escape()?;
                            let mut encoded = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
                        }
                        other => {
                            return Err(format!("invalid string escape `\\{}`", other as char));
                        }
                    }
                }
                // A raw control character is invalid JSON inside a string.
                0x00..=0x1F => return Err("a raw control character in a string".to_string()),
                // Any other byte passes through verbatim: an ASCII byte, or a byte of a multibyte
                // sequence the line-level UTF-8 validation already accepted.
                _ => out.push(byte),
            }
            if out.len() > self.max_string_bytes {
                return Err(format!(
                    "a string value exceeds {} bytes",
                    self.max_string_bytes
                ));
            }
        }
        String::from_utf8(out).map_err(|_| "an invalid UTF-8 sequence in a string".to_string())
    }

    /// Parse the four hex digits after a `\u`, decoding a UTF-16 unit and pairing a high
    /// surrogate with a following `\u` low surrogate.
    fn parse_unicode_escape(&mut self) -> Result<char, String> {
        let unit = self.parse_hex4()?;
        // A high surrogate must be followed by a `\u` low surrogate; combine to a scalar.
        if (0xD800..=0xDBFF).contains(&unit) {
            if self.next_byte() != Some(b'\\') || self.next_byte() != Some(b'u') {
                return Err("a high surrogate without a low surrogate".to_string());
            }
            let low = self.parse_hex4()?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return Err("an invalid low surrogate".to_string());
            }
            let combined = 0x1_0000 + (((unit - 0xD800) as u32) << 10) + (low - 0xDC00) as u32;
            char::from_u32(combined).ok_or_else(|| "an invalid surrogate pair".to_string())
        } else if (0xDC00..=0xDFFF).contains(&unit) {
            Err("a lone low surrogate".to_string())
        } else {
            char::from_u32(unit as u32).ok_or_else(|| "an invalid unicode escape".to_string())
        }
    }

    fn parse_hex4(&mut self) -> Result<u16, String> {
        let mut value: u16 = 0;
        for _ in 0..4 {
            let digit = self.next_byte().ok_or("a truncated \\u escape")?;
            let nibble = match digit {
                b'0'..=b'9' => digit - b'0',
                b'a'..=b'f' => digit - b'a' + 10,
                b'A'..=b'F' => digit - b'A' + 10,
                _ => return Err("a non-hex digit in a \\u escape".to_string()),
            };
            value = (value << 4) | u16::from(nibble);
        }
        Ok(value)
    }
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
        let JsonScalar::Str(text) = object.take("t").expect("member") else {
            panic!("expected a string");
        };
        assert_eq!(text, "a\t\"b\"\nA\u{1F600}\u{00E9}");
    }

    #[test]
    fn nested_and_float_and_dupes_are_refused() {
        let limits = ImportLimits::DEFAULT;
        assert!(parse_row_object(br#"{"a": {"b": 1}}"#, &limits).is_err());
        assert!(parse_row_object(br#"{"a": [1,2]}"#, &limits).is_err());
        assert!(parse_row_object(br#"{"a": 1.5}"#, &limits).is_err());
        assert!(parse_row_object(br#"{"a": 1, "a": 2}"#, &limits).is_err());
        assert!(parse_row_object(br#"{"a": 01}"#, &limits).is_err());
        assert!(parse_row_object(br#"{"a": 1} junk"#, &limits).is_err());
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
