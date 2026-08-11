//! The storeless subset checker and lowering to an [`ImageDraft`].
//!
//! The compiler opens no store and mints no verified image: it parses source,
//! checks the current subset, and lowers to canonical image bytes the independent
//! verifier rechecks. Coverage grows one slice at a time; a well-formed construct
//! outside the current subset is a typed `check.unsupported` diagnostic, never a
//! silent drop.

use std::collections::{BTreeMap, BTreeSet};

use marrow_codes::Code;
use marrow_image::{EncodedImage, ExportId, ImageBuildError, ImageDraft, Instr};
use marrow_project::{CaptureLimits, FileIdentity, ProjectInput};
use marrow_syntax::{
    AliasDecl, ConstDecl, Declaration, EnumDecl, NominalDecl, ResourceDecl, ResourceMember,
    SourceFile, SourceSpan, StoreDecl, StructDecl, parse_source,
};

use crate::analysis::{AnalysisFactCollector, BoundedAnalysisFacts, FactSink, FileRef};
use crate::decl::{
    Binding, DeclarationBudget, DeclarationLedgerFull, DeclarationNamespace, DeclarationOccurrence,
    DeclarationSite, DeclareError, MAX_DECLARATION_LEDGER_BYTES, SourceStage, refuse,
    refuse_at_earlier_stage,
};
use crate::demand::DurableNaming;
use crate::diag::{
    BoundedDiagnostics, CompileDiagnosticLimit, DiagnosticCollector, SourceDiagnostic,
};
use crate::durable::DurableRegistry;
use crate::konst::ConstRegistry;
use crate::lower::{
    BodyOutcome, DeclaredFn, FnLowerer, FunctionRegistry, GenericRegistry, ModuleBinding,
    ModuleLedger, SignatureOutcome, is_durable_place_op, is_mutation_instr,
    is_reserved_builtin_name, reserved_builtin_name,
};
use crate::types::BuildError;
use crate::types::{GenericInvariant, TypeRegistry};

/// One resolved public export: its dotted module, its item name, and the stable
/// [`ExportId`] the image carries. This directory is the only place a human export
/// name is paired with its id; the CLI resolves a caller-supplied path to an id
/// here, then dispatches into the image by that verified id.
#[derive(Debug, Clone)]
pub struct ExportEntry {
    pub module: String,
    pub item: String,
    pub id: ExportId,
}

/// The result of compiling a project: the canonical image bytes and the export
/// directory that maps declaration paths to their ids.
#[derive(Debug, Clone)]
pub struct Compiled {
    pub image: EncodedImage,
    pub exports: Vec<ExportEntry>,
    /// The durable-path naming join for the program's admitted durable graph, so a
    /// caller can describe each export's verifier-reconstructed demand in source
    /// spelling. Empty for a storeless project.
    pub naming: DurableNaming,
}

/// One discovered `test "name"` declaration: its report title, the module and
/// source file it lives in, and the source position of its header. The image
/// carries the title in its closed non-wire TEST-ENTRY table; this directory pairs
/// it with its location for reporting.
#[derive(Debug, Clone)]
pub struct TestEntry {
    pub name: String,
    pub module: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// The result of compiling a project *with* its tests: the image (carrying the
/// test functions and the TEST-ENTRY table), the export directory, and the test
/// directory `marrow test` reports against.
#[derive(Debug, Clone)]
pub struct CompiledTests {
    pub image: EncodedImage,
    pub exports: Vec<ExportEntry>,
    pub tests: Vec<TestEntry>,
    /// The durable-path naming join for the program's admitted durable graph (see
    /// [`Compiled::naming`]).
    pub naming: DurableNaming,
}

/// The ordered source diagnostics from a failed compilation.
///
/// This owner is statically nonempty. It preserves the compiler's original
/// diagnostic allocation and exposes only immutable or consuming access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonEmptySourceDiagnostics(Vec<SourceDiagnostic>);

impl NonEmptySourceDiagnostics {
    fn new(diagnostics: Vec<SourceDiagnostic>) -> Option<Self> {
        (!diagnostics.is_empty()).then_some(Self(diagnostics))
    }

    /// Borrow the diagnostics in compiler order.
    pub fn as_slice(&self) -> &[SourceDiagnostic] {
        &self.0
    }

    /// Iterate over the diagnostics in compiler order.
    pub fn iter(&self) -> std::slice::Iter<'_, SourceDiagnostic> {
        self.0.iter()
    }

    /// Recover the original diagnostic allocation in compiler order.
    pub fn into_vec(self) -> Vec<SourceDiagnostic> {
        self.0
    }
}

impl AsRef<[SourceDiagnostic]> for NonEmptySourceDiagnostics {
    fn as_ref(&self) -> &[SourceDiagnostic] {
        self.as_slice()
    }
}

impl<'a> IntoIterator for &'a NonEmptySourceDiagnostics {
    type Item = &'a SourceDiagnostic;
    type IntoIter = std::slice::Iter<'a, SourceDiagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for NonEmptySourceDiagnostics {
    type Item = SourceDiagnostic;
    type IntoIter = std::vec::IntoIter<SourceDiagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// An opaque compiler-coherence failure.
///
/// Its cause is intentionally private. Callers may distinguish this outcome
/// from source diagnostics but cannot classify compiler internals.
pub struct CompileInvariant(InvariantCause);

impl CompileInvariant {
    fn retain_private_cause(&self) {
        match &self.0 {
            InvariantCause::Generic(cause) => {
                let _ = cause;
            }
            InvariantCause::EmptyDiagnostics(stage) => {
                let _ = stage;
            }
            InvariantCause::UnavailableWithoutReport | InvariantCause::ReservedIndexMismatch => {}
            InvariantCause::ImageBuild(error) => {
                let _ = error;
            }
        }
    }
}

impl std::fmt::Debug for CompileInvariant {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.retain_private_cause();
        formatter.write_str("CompileInvariant")
    }
}

impl std::fmt::Display for CompileInvariant {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("compiler invariant failure")
    }
}

impl std::error::Error for CompileInvariant {}

/// Which fixed compiler-owned aggregate bound compilation exhausted. Each variant
/// names a whole-program count or byte ceiling that no single source construct is
/// at fault for; a bound one construct crosses is a `check.resource_limit` source
/// diagnostic instead. The enum is closed and exhaustively matchable so a
/// downstream consumer (bound-raise audits, the analysis-fact floor) can classify
/// every kind without a wildcard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceLimitKind {
    Strings,
    Consts,
    Types,
    Enums,
    Collections,
    Roots,
    Sites,
    Functions,
    Exports,
    TestEntries,
    ImageBytes,
    /// A single interned string over the per-entry byte bound reached through a path a
    /// source precheck does not yet cover (a folded constant or an interpolation
    /// segment), so it surfaces as a locationless resource limit rather than the
    /// synthetic diagnostic it once produced.
    StringBytes,
    /// One function's encoded bytecode over the per-function byte bound. Known only
    /// after lowering.
    CodeBytes,
    /// The ordered diagnostic set grew past the count bound, so the incomplete
    /// collection was discarded rather than surfaced as a truncated result.
    DiagnosticCount,
    /// The ordered diagnostic set grew past the total-byte bound, so the incomplete
    /// collection was discarded.
    DiagnosticBytes,
    /// The captured project contains more modules than the production compiler drive
    /// admits. A wider pure capture remains inspectable, but cannot enter compilation.
    ProjectFiles,
    /// One captured module contains more source bytes than the production compiler
    /// drive admits.
    ProjectFileBytes,
    /// The captured project's aggregate source bytes exceed the production compiler
    /// drive envelope.
    ProjectSourceBytes,
    /// The declaration ledgers' retained refusals crossed their shared byte ceiling.
    /// Neither the image bounds nor the diagnostic ceiling bounds this retention — a
    /// refused declaration never reaches the encoder, and a diagnostic collector at
    /// its ceiling keeps admitting and discarding while the pass runs on — so the
    /// term carries its own declared bound.
    DeclarationLedgerBytes,
}

impl ResourceLimitKind {
    /// The stable identifier for which fixed bound was exhausted. It names the kind — not
    /// its numeric limit or any source location — so an operator (or a bound-raise audit)
    /// can bisect which aggregate bound fired without re-running under instrumentation. The
    /// strings are a frozen surface: the CLI resource-limit record carries this verbatim.
    pub fn detail(self) -> &'static str {
        match self {
            ResourceLimitKind::Strings => "Strings",
            ResourceLimitKind::Consts => "Consts",
            ResourceLimitKind::Types => "Types",
            ResourceLimitKind::Enums => "Enums",
            ResourceLimitKind::Collections => "Collections",
            ResourceLimitKind::Roots => "Roots",
            ResourceLimitKind::Sites => "Sites",
            ResourceLimitKind::Functions => "Functions",
            ResourceLimitKind::Exports => "Exports",
            ResourceLimitKind::TestEntries => "TestEntries",
            ResourceLimitKind::ImageBytes => "ImageBytes",
            ResourceLimitKind::StringBytes => "StringBytes",
            ResourceLimitKind::CodeBytes => "CodeBytes",
            ResourceLimitKind::DiagnosticCount => "DiagnosticCount",
            ResourceLimitKind::DiagnosticBytes => "DiagnosticBytes",
            ResourceLimitKind::ProjectFiles => "ProjectFiles",
            ResourceLimitKind::ProjectFileBytes => "ProjectFileBytes",
            ResourceLimitKind::ProjectSourceBytes => "ProjectSourceBytes",
            ResourceLimitKind::DeclarationLedgerBytes => "DeclarationLedgerBytes",
        }
    }

    /// The human sentence fragment naming which bound was exhausted, for terminal
    /// output. It is the same fact [`detail`](Self::detail) carries as a machine
    /// identifier: a tool reads the identifier, a person reads this. Rendering the
    /// identifier itself into prose would put a Rust variant name in front of a
    /// person, so the two projections stay separate and exhaustive over one enum.
    pub fn description(self) -> &'static str {
        match self {
            ResourceLimitKind::Strings => "the interned string table is full",
            ResourceLimitKind::Consts => "the constant table is full",
            ResourceLimitKind::Types => "the type table is full",
            ResourceLimitKind::Enums => "the enum table is full",
            ResourceLimitKind::Collections => "the collection type table is full",
            ResourceLimitKind::Roots => "the durable root table is full",
            ResourceLimitKind::Sites => "the effect site table is full",
            ResourceLimitKind::Functions => "the function table is full",
            ResourceLimitKind::Exports => "the export table is full",
            ResourceLimitKind::TestEntries => "the test entry table is full",
            ResourceLimitKind::ImageBytes => "the program image is too large",
            ResourceLimitKind::StringBytes => "one text value is too large",
            ResourceLimitKind::CodeBytes => "one function's compiled code is too large",
            ResourceLimitKind::DiagnosticCount => "too many diagnostics to retain",
            ResourceLimitKind::DiagnosticBytes => "the diagnostics hold too much text to retain",
            ResourceLimitKind::ProjectFiles => "the project has too many source files",
            ResourceLimitKind::ProjectFileBytes => "one source file is too large",
            ResourceLimitKind::ProjectSourceBytes => "the project's source is too large",
            ResourceLimitKind::DeclarationLedgerBytes => "the declaration ledger is full",
        }
    }
}

/// A fixed compiler-owned resource bound compilation exhausted with no single
/// source construct at fault. It carries only its typed [`ResourceLimitKind`] and
/// the fixed bound as integers — never a source location, span, spelling, or a
/// fabricated count. The exact overrun count is not carried: the aggregate encode
/// bounds report which bound they are, not by how much, and inventing an actual
/// would reintroduce the fabricated data this boundary exists to remove. The caller
/// distinguishes this outcome from source diagnostics and from an opaque compiler
/// invariant, and reports a fixed operational record; a downstream bound-raise audit
/// consumes the kind and its limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompileResourceLimit {
    kind: ResourceLimitKind,
    limit: u64,
}

impl CompileResourceLimit {
    fn new(kind: ResourceLimitKind, limit: u64) -> Self {
        Self { kind, limit }
    }

    /// Which fixed bound was exhausted.
    pub fn kind(self) -> ResourceLimitKind {
        self.kind
    }

    /// The fixed bound the program exceeded.
    pub fn limit(self) -> u64 {
        self.limit
    }
}

impl std::fmt::Display for CompileResourceLimit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("compiler resource limit reached")
    }
}

impl std::error::Error for CompileResourceLimit {}

/// Map the collector's typed ceiling to its public resource-limit record: the
/// one failure-boundary translation, exhaustive over both kinds.
fn diagnostic_limit_failure(limit: CompileDiagnosticLimit) -> CompileResourceLimit {
    match limit {
        CompileDiagnosticLimit::Count { limit } => {
            CompileResourceLimit::new(ResourceLimitKind::DiagnosticCount, limit as u64)
        }
        CompileDiagnosticLimit::OwnedBytes { limit } => {
            CompileResourceLimit::new(ResourceLimitKind::DiagnosticBytes, limit as u64)
        }
    }
}

/// Why compilation produced no image. One central boundary owns the precedence
/// `Invariant > Diagnostics > ResourceLimit`: an opaque compiler-coherence failure
/// dominates every source diagnostic already accumulated, a complete bounded
/// diagnostic set dominates an independent later resource candidate, and a resource
/// limit surfaces only when no invariant and no complete diagnostic set exist.
#[derive(Debug)]
pub enum CompileFailure {
    /// One or more source diagnostics blocked compilation.
    Diagnostics(NonEmptySourceDiagnostics),
    /// A fixed compiler-owned aggregate resource bound was exhausted with no single
    /// source construct at fault, so no diagnostic and no image were produced.
    ResourceLimit(CompileResourceLimit),
    /// Private compiler state was incoherent.
    Invariant(CompileInvariant),
}

impl std::fmt::Display for CompileFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Diagnostics(_) => {
                formatter.write_str("compilation failed with source diagnostics")
            }
            Self::ResourceLimit(_) => {
                formatter.write_str("compilation reached a fixed resource limit")
            }
            Self::Invariant(_) => formatter.write_str("compiler invariant failure"),
        }
    }
}

impl std::error::Error for CompileFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Diagnostics(_) => None,
            Self::ResourceLimit(limit) => Some(limit),
            Self::Invariant(invariant) => Some(invariant),
        }
    }
}

/// The semantic stage a diagnostics terminal came from, retained for the
/// empty-boundary invariant. The parse and structural stages carry no tag: a
/// logically empty parse or structural terminal passes over instead of
/// crossing the boundary, so only semantic stages can reach it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompileStage {
    TypeInstantiation,
    TemplateProof,
    BodyLowering,
    PostLoweringValidation,
}

#[derive(Debug)]
enum InvariantCause {
    Generic(GenericInvariant),
    EmptyDiagnostics(CompileStage),
    /// A semantic artifact was unavailable, yet the semantic pass finished with a
    /// complete and empty diagnostic terminal. An unavailable artifact is always the
    /// consequence of a refusal that reported, so reaching the fence with nothing to
    /// report is a compiler-coherence failure — the generalization of
    /// [`InvariantCause::EmptyDiagnostics`] to the continued phase set.
    UnavailableWithoutReport,
    /// A generic instance took an image index other than the one the registry
    /// reserved for it, so a call site would name a different function than the one
    /// the image carries.
    ReservedIndexMismatch,
    /// An image-build variant unreachable from a coherent compiler: a producer-state
    /// contradiction (an invalid cross-reference, a local count below the parameter
    /// count) or a per-construct bound a source precheck already refuses before the
    /// draft is built. Kept opaque: it is a compiler-internal defect, not a source
    /// diagnostic or an aggregate resource limit.
    ImageBuild(ImageBuildError),
}

/// Project one stage's finished diagnostic terminal to the failure it forces,
/// or `None` for a logically empty stage the projection passes over. The
/// public precedence `Invariant > Diagnostics > ResourceLimit` is structural:
/// an invariant returns before any terminal is consulted, and a stage's own
/// Limited terminal is the only resource candidate that can displace its rows.
fn stage_failure(diagnostics: BoundedDiagnostics) -> Option<CompileFailure> {
    match diagnostics {
        BoundedDiagnostics::Complete { rows, .. } => {
            NonEmptySourceDiagnostics::new(rows).map(CompileFailure::Diagnostics)
        }
        BoundedDiagnostics::Limited { limit, .. } => Some(CompileFailure::ResourceLimit(
            diagnostic_limit_failure(limit),
        )),
    }
}

/// Classify a producer-side [`ImageBuildError`] from `ImageDraft::encode` into the
/// compile-failure arm it belongs to. Encode runs only on the clean-diagnostics path,
/// so there is no coexisting diagnostic and the classification is total on its own.
///
/// A whole-program aggregate count, or the whole-image byte ceiling, has no single
/// source construct at fault and becomes a [`CompileResourceLimit`]. A per-construct
/// bound is refused earlier by a source precheck at its offending span, so reaching
/// it here means the draft was built past a bound the precheck should have caught — a
/// compiler-internal defect — and a producer-state contradiction (an invalid reference,
/// a local count below the parameters) is likewise unreachable from a coherent compiler;
/// both are opaque invariants. The match has no wildcard, so a new image-build variant
/// forces an explicit classification here.
fn image_build_outcome(error: ImageBuildError) -> ImagePolicyOutcome {
    use marrow_image::bounds;
    let aggregate = |kind: ResourceLimitKind, limit: usize| {
        ImagePolicyOutcome::ResourceLimit(CompileResourceLimit::new(kind, limit as u64))
    };
    match error {
        // Aggregate whole-program counts and the byte ceiling: no single offender.
        ImageBuildError::TooManyStrings => {
            aggregate(ResourceLimitKind::Strings, bounds::MAX_STRINGS)
        }
        ImageBuildError::TooManyConsts => aggregate(ResourceLimitKind::Consts, bounds::MAX_CONSTS),
        ImageBuildError::TooManyTypes => aggregate(ResourceLimitKind::Types, bounds::MAX_TYPES),
        ImageBuildError::TooManyEnums => aggregate(ResourceLimitKind::Enums, bounds::MAX_ENUMS),
        ImageBuildError::TooManyCollections => {
            aggregate(ResourceLimitKind::Collections, bounds::MAX_COLLECTIONS)
        }
        ImageBuildError::TooManyRoots => aggregate(ResourceLimitKind::Roots, bounds::MAX_ROOTS),
        ImageBuildError::TooManySites => aggregate(ResourceLimitKind::Sites, bounds::MAX_SITES),
        ImageBuildError::TooManyFunctions => {
            aggregate(ResourceLimitKind::Functions, bounds::MAX_FUNCTIONS)
        }
        ImageBuildError::TooManyExports => {
            aggregate(ResourceLimitKind::Exports, bounds::MAX_EXPORTS)
        }
        ImageBuildError::TooManyTestEntries => {
            aggregate(ResourceLimitKind::TestEntries, bounds::MAX_TEST_ENTRIES)
        }
        ImageBuildError::ImageTooLarge => {
            aggregate(ResourceLimitKind::ImageBytes, bounds::MAX_IMAGE_BYTES)
        }
        // Per-construct bounds reachable through a path no pre-mutation source
        // precheck yet covers: an honest locationless resource limit, never the
        // synthetic diagnostic.
        ImageBuildError::StringTooLong => {
            aggregate(ResourceLimitKind::StringBytes, bounds::MAX_STRING_BYTES)
        }
        ImageBuildError::CodeTooLong => {
            aggregate(ResourceLimitKind::CodeBytes, bounds::MAX_CODE_BYTES)
        }
        // Per-construct bounds a source precheck refuses before the draft is built, so
        // an encode-time occurrence is a defense-in-depth producer defect; and
        // producer-state contradictions unreachable from a coherent compiler. Both are
        // opaque invariants.
        //
        // The three durable-structural bounds are here for the same reason as the rest,
        // by three different arguments. Member depth and index components are refused at
        // their offending construct while the durable graph is walked. Per-Product member
        // count is bounded by the identity ledger a level above: every admitted member
        // holds a live ledger anchor, and the ledger's own row bound sits below the
        // member bound, so a coherent compiler cannot present an over-count graph.
        ImageBuildError::TooManyFields
        | ImageBuildError::TooManyDurableMembers
        | ImageBuildError::DurableTreeTooDeep
        | ImageBuildError::TooManyIndexComponents
        | ImageBuildError::TooManyStructLeaves
        | ImageBuildError::TooManyVariants
        | ImageBuildError::TooManyPayloadFields
        | ImageBuildError::TooManyIndexes
        | ImageBuildError::TooManyKeyColumns
        | ImageBuildError::DurableValueTooDeep
        | ImageBuildError::TooManyParams
        | ImageBuildError::TooManyLocals
        | ImageBuildError::LocalCountBelowParams
        // Two occurrences of one Product identity claiming a different graph or entry
        // record is unreachable from source: a Product's graph is built once, at its
        // first root, and every later root references it.
        | ImageBuildError::ProductGraphConflict
        | ImageBuildError::ProductEntryRecordConflict
        | ImageBuildError::InvalidReference(_) => {
            ImagePolicyOutcome::Invariant(InvariantCause::ImageBuild(error))
        }
    }
}

/// Why the production projection produced no image from a semantically checked
/// program. This is the verdict of the projection alone: it is taken strictly after
/// the semantic fence, and it is deliberately not an arm of [`SemanticOutcome`], so a
/// public image-policy excess can never be mistaken for — or represented as — semantic
/// unavailability. The analysis path never constructs one.
enum ImagePolicyOutcome {
    /// A fixed whole-program aggregate bound, with no single source construct at fault.
    ResourceLimit(CompileResourceLimit),
    /// A producer-state contradiction or a bound a source precheck already owns.
    Invariant(InvariantCause),
}

/// Whether a compilation includes the project's `test` declarations. A production
/// `run` image excludes them (tests are not shipped); `marrow test` includes them,
/// adding the test functions and the TEST-ENTRY table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestMode {
    Exclude,
    Include,
}

/// A parsed module: its file identity (for spans and diagnostics), its dotted
/// module name (for export identity), the parse tree, and the logical broken
/// status of its parse. Only the AST and that one bit survive parsing — the
/// module's syntax diagnostics are absorbed into the drive's parse collector
/// the moment the file is parsed (A5), so no per-module diagnostic state ever
/// accumulates with the file count.
/// A project module that never reached the semantic pass, with the stage that
/// refused it. It is still a module of the project: the module ledger declares it
/// refused so a `use` or a qualified call into it names that stage's report.
struct UnparsedModule {
    name: String,
    file: FileIdentity,
    at: FileRef,
    stage: SourceStage,
}

struct Module {
    file: FileIdentity,
    /// This module's position in the project's own module order — the coordinate every
    /// editor fact this module produces is retained under.
    at: FileRef,
    name: String,
    ast: SourceFile,
    broken: bool,
}

/// A lowered function's identity for recursion detection and the
/// requires-ambient-transaction check: its image index, the functions it calls
/// directly, where to report a cycle, and the durable-mutation and call sites it
/// performs outside any `transaction` block.
struct LoweredFn {
    index: u16,
    file: FileIdentity,
    name: String,
    span: SourceSpan,
    callees: Vec<u16>,
    /// Whether this function is a public export entry. An export that mutates owns its
    /// transaction; a non-export helper or test entry receives an ambient transaction
    /// from its caller or the test harness, so the requirement is reported only at
    /// export entries.
    is_export: bool,
    /// Whether this is a `test` body. A test body is one of two disjoint kinds: it
    /// performs durable operations directly, or it drives exports. Mixing the two is
    /// refused by the strict-separation check.
    is_test: bool,
    /// Spans of durable mutations this body performs outside any `transaction` block.
    unwrapped_mutations: Vec<SourceSpan>,
    /// Calls this body performs outside any `transaction` block, with their spans.
    unwrapped_calls: Vec<(u16, SourceSpan)>,
    /// Whether this body performs a durable-place operation directly.
    has_direct_durable_op: bool,
    /// Whether this body owns a `transaction` block.
    owns_transaction: bool,
    /// This body's lowered instruction tape and the parallel full source span of each
    /// instruction, consumed by the check-time transaction-ownership pass.
    code: Vec<Instr>,
    code_spans: Vec<SourceSpan>,
}

/// Compile a captured project into canonical program-image bytes and its export
/// directory, or return source diagnostics or an opaque compiler-coherence failure.
/// The production path excludes `test` declarations and emits an empty TEST-ENTRY
/// table.
pub fn compile(project: &ProjectInput) -> Result<Compiled, CompileFailure> {
    let built = drive(project, TestMode::Exclude)
        .map_err(CompileFailure::ResourceLimit)?
        .into_built()?;
    Ok(Compiled {
        image: built.image,
        exports: built.exports,
        naming: built.naming,
    })
}

/// Compile a captured project *with* its tests: the image additionally carries the
/// test functions and the closed TEST-ENTRY table, and the returned directory pairs
/// each test's title with its location for `marrow test`. Failure uses the same
/// source-diagnostic or opaque compiler-invariant boundary as [`compile`].
pub fn compile_with_tests(project: &ProjectInput) -> Result<CompiledTests, CompileFailure> {
    let built = drive(project, TestMode::Include)
        .map_err(CompileFailure::ResourceLimit)?
        .into_built()?;
    Ok(CompiledTests {
        image: built.image,
        exports: built.exports,
        tests: built.tests,
        naming: built.naming,
    })
}

/// The image, export directory, and (when included) test directory a compilation
/// produced.
struct Built {
    image: EncodedImage,
    exports: Vec<ExportEntry>,
    tests: Vec<TestEntry>,
    naming: DurableNaming,
}

/// The staged outcome of one analysis/lowering pass over a project. Diagnostics are
/// bucketed by the stage that produced them, so a single traversal serves both the
/// production compile — which projects the first non-empty stage and thereby
/// reproduces the historical staged early-return byte for byte — and the editor
/// analysis snapshot, which consumes every stage. There is no analyze/compile mode
/// flag forking control flow: the traversal is one and the same; only the projection
/// differs.
struct Driven {
    /// Invalid-UTF-8 and syntax diagnostics from every module, parseable or
    /// not: the parse stage's finished bounded terminal.
    parse: BoundedDiagnostics,
    /// Structural-bound diagnostics over the cleanly-parsed modules: that
    /// stage's finished bounded terminal.
    structural: BoundedDiagnostics,
    /// The semantic pass over the cleanly-parsed modules.
    semantic: SemanticOutcome,
    /// The editor facts this pass retained, already sealed against the snapshot
    /// count and byte ceilings: the complete set, or the typed limit that discarded
    /// it. Carried orthogonally to the semantic outcome; the production compile's
    /// projection ignores it and the analysis snapshot consumes it. Broken-module
    /// status inside it comes directly from decode and parse status — never
    /// reconstructed from retained diagnostics, so a Limited parse terminal cannot
    /// erase it.
    facts: BoundedAnalysisFacts,
    /// The modules whose declaration outline crossed a per-file bound. Nothing is
    /// retained for such a module, so the snapshot reports its outline as bounded-
    /// unavailable; every other module's outline is unaffected. The production compile
    /// ignores this entirely (symbol bounds are analysis-only and never fail
    /// compilation).
    symbol_bounded_files: Vec<FileRef>,
}

/// Allocation-free admission for one compiler drive.
///
/// [`marrow_project::capture`] intentionally accepts caller-selected limits. This
/// boundary keeps that pure construction API unchanged while ensuring every compiler
/// and analysis entry point uses the one production envelope before it mints or
/// retains parser, diagnostic, or fact state.
struct DriveInputAdmission;

/// Proof that a project passed drive admission: its module count is at most
/// `max_files`, which is inside the fact coordinate domain. Carrying the count out of
/// admission is what makes minting a coordinate per module total, so the drive states
/// the invariant instead of branching on it.
struct AdmittedModules(u16);

impl AdmittedModules {
    /// One coordinate per admitted module, in the project's own module order.
    fn coordinates(&self) -> impl Iterator<Item = FileRef> {
        (0..self.0).map(FileRef::admitted)
    }
}

/// The language server's owned-heap ceiling, which every retention and transient term in
/// this layer is sized against. Set elsewhere; consumed here.
const OWNED_HEAP_BYTES: usize = 640 * 1024 * 1024;

/// The heap one parse of one admitted file may allocate.
///
/// **DeclarationSite, not derived from a length.** It is two thirds of the owned-heap ceiling,
/// which keeps a third in reserve for everything a query holds beside the parse. Stating
/// it independently of any length is what makes the admission below a real gate: were it
/// defined as what some chosen length costs, comparing a file's charge against it would
/// reduce to comparing that file's length against the chosen one, for any rate, and a
/// widened representation would raise both sides equally and admit exactly as much as
/// before while silently costing more heap.
pub const MAX_QUERY_PARSE_TRANSIENT_BYTES: usize = OWNED_HEAP_BYTES * 2 / 3;

/// The longest source file this crate parses.
///
/// **Derived from the heap ceiling, not chosen.** It is the longest file whose parse fits
/// [`MAX_QUERY_PARSE_TRANSIENT_BYTES`] at the per-source-byte rate `marrow-syntax`
/// publishes for the representation its parser builds. Widening that representation
/// therefore narrows what is admitted, with no length edited here; `marrow-compile`'s
/// `the_admitted_length_and_the_exported_term_agree_with_the_derivation` re-derives the
/// rate from the representation and fails if the published constant drifts from it.
///
/// It is a byte count rather than a round number because it is a consequence: rounding it
/// would be choosing a length again.
pub const MAX_PARSED_FILE_BYTES: usize =
    marrow_syntax::max_parse_length(MAX_QUERY_PARSE_TRANSIENT_BYTES);

/// This crate never offers to parse a file the project owner would refuse to capture.
/// The two ceilings are set by different owners for different reasons — one bounds a
/// captured project, the other bounds this crate's heap — so the relation between them
/// is checked here rather than either copying the other's number.
const _: () = assert!(MAX_PARSED_FILE_BYTES <= CaptureLimits::DEFAULT.max_file_bytes());

impl DriveInputAdmission {
    fn check(project: &ProjectInput) -> Result<AdmittedModules, CompileResourceLimit> {
        let limits = CaptureLimits::DEFAULT;
        let modules = project.modules();
        if modules.len() > limits.max_files() {
            return Err(CompileResourceLimit::new(
                ResourceLimitKind::ProjectFiles,
                limits.max_files() as u64,
            ));
        }

        // Refuse before materializing: what a file's parse costs is arithmetic over its
        // byte length and a pinned rate, so an over-ceiling file is turned away here
        // rather than parsed and found to be too large afterwards. The reader is given
        // the length they can act on, not the heap figure it was derived from.
        if modules.iter().any(|module| {
            marrow_syntax::max_parse_bytes(module.source().len()) > MAX_QUERY_PARSE_TRANSIENT_BYTES
        }) {
            return Err(CompileResourceLimit::new(
                ResourceLimitKind::ProjectFileBytes,
                MAX_PARSED_FILE_BYTES as u64,
            ));
        }

        let mut source_bytes = 0usize;
        for module in modules {
            source_bytes = source_bytes
                .checked_add(module.source().len())
                .ok_or_else(|| {
                    CompileResourceLimit::new(
                        ResourceLimitKind::ProjectSourceBytes,
                        limits.max_total_bytes() as u64,
                    )
                })?;
        }
        if source_bytes > limits.max_total_bytes() {
            return Err(CompileResourceLimit::new(
                ResourceLimitKind::ProjectSourceBytes,
                limits.max_total_bytes() as u64,
            ));
        }
        Ok(AdmittedModules(modules.len() as u16))
    }
}

/// The outcome of the semantic pass over the cleanly-parsed modules: a checked program,
/// or the accumulated failure tagged with the stage that produced it.
enum SemanticOutcome {
    /// Every semantic artifact was available and no diagnostic stands: the program is
    /// checked. It carries the draft, not an image — encoding is the production
    /// projection's step, taken after this fence. The draft dominates this enum's size,
    /// and every refusal arm would otherwise pay for it, so the checked program is
    /// carried behind one allocation taken exactly once per clean compile.
    Checked(Box<CheckedProgram>),
    Diagnostics(BoundedDiagnostics, CompileStage),
    Invariant(InvariantCause),
    /// A fixed compiler-owned bound the semantic pass exhausted before it could
    /// finish, with no single source construct at fault. The pass stops here rather
    /// than dropping what it can no longer retain and fabricating an absence at
    /// every later use of it.
    ResourceLimit(CompileResourceLimit),
}

/// The declaration ledgers' shared retention ceiling, as its public record.
impl From<DeclarationLedgerFull> for CompileResourceLimit {
    fn from(_: DeclarationLedgerFull) -> Self {
        Self::new(
            ResourceLimitKind::DeclarationLedgerBytes,
            MAX_DECLARATION_LEDGER_BYTES as u64,
        )
    }
}

impl From<DeclarationLedgerFull> for SemanticOutcome {
    fn from(full: DeclarationLedgerFull) -> Self {
        Self::ResourceLimit(full.into())
    }
}

/// A ledger's two ways of refusing to record an occurrence reach the pass outcome
/// through the one place a build's failure arms are routed, so a ledger error and a
/// registry error cannot become different outcomes.
impl From<DeclareError> for SemanticOutcome {
    fn from(error: DeclareError) -> Self {
        BuildError::from(error).into()
    }
}

/// The one place a registry build's two failure arms become pass outcomes, so the
/// five builders that return [`BuildError`] cannot disagree about either.
impl From<BuildError> for SemanticOutcome {
    fn from(error: BuildError) -> Self {
        match error {
            BuildError::Invariant(invariant) => Self::Invariant(InvariantCause::Generic(invariant)),
            BuildError::LedgerFull(full) => full.into(),
        }
    }
}

/// The type registry resolved every declared type. Root of the artifact dependency
/// order: every later phase resolves annotations through it.
struct CompleteTypeRegistry;

/// The signature table and the proof that it is complete, kept in their own module
/// so the proof's private field is out of reach of every other line in this file.
mod signatures {
    use super::FunctionRegistry;

    /// Every declared function signature resolved, as a zero-size proof token.
    ///
    /// `Artifacts.functions` holds this, not the table. The private field is what
    /// makes the property the artifact set protects — a resolved signature table
    /// nothing vouches for is unrepresentable at `encode` — a compile-time one: the
    /// token has no literal form outside this module, so
    /// [`CompleteFunctionRegistry::complete`] below is the only expression in the
    /// crate that produces one. A fieldless unit struct would be constructible by
    /// name anywhere the type is visible, which is the whole of the encode gate.
    pub(super) struct SignaturesComplete(());

    /// The sole owner of the resolved signature table.
    ///
    /// The table is always built: a signature refused for a parameter or return type
    /// is a refused ledger entry, not a withheld table, so a call to it reuses that
    /// cause while every unrelated body still lowers and reports its own errors.
    /// Availability and the value are minted by this one owner.
    pub(super) struct CompleteFunctionRegistry(pub(super) FunctionRegistry);

    impl CompleteFunctionRegistry {
        /// The resolved signature table every dependent phase resolves call sites
        /// through. Always available: a refused signature answers with its cause.
        pub(super) fn signatures(&self) -> &FunctionRegistry {
            &self.0
        }

        /// The completeness proof, `Some` exactly when every declared signature was
        /// accepted. Read from the ledger, not from a flag the build loop
        /// maintained.
        pub(super) fn complete(&self) -> Option<SignaturesComplete> {
            self.0
                .every_signature_accepted()
                .then_some(SignaturesComplete(()))
        }
    }
}

use signatures::{CompleteFunctionRegistry, SignaturesComplete};

/// Every generic template's once-checked proof was accepted and no instantiation
/// limit stopped the pass, so the queued instance set is trustworthy.
struct AcceptedQueuedTemplateProofs;

/// Every declared non-generic function body lowered into the draft.
struct CompleteDeclaredFunctionBodies;

/// Every declared test body lowered into the draft, with no duplicate-title skip.
/// A skip leaves a reserved index unminted — `base` counts declared tests including
/// duplicates — so this artifact, not the diagnostic set, is what the instance drain
/// requires. Vacuously available when tests are excluded.
struct CompleteDeclaredTestBodies;

/// Every body that entered lowering produced an image function, and the instance
/// drain, if it ran, completed.
///
/// **The claim is over the MINTED lowered set.** Indices actually minted are dense, so
/// a call graph keyed by index over this set is exact. Reserved-but-undrained instance
/// indices are outside the claim: when the drain was gated off, a call into an
/// unminted instance is simply absent from the graph. A traversal may therefore miss a
/// cycle through such an index — it can never fabricate one — and an undrained drain
/// already leaves an artifact unavailable, which fences the program off from `encode`.
struct CompleteLoweredFunctionSet(Vec<LoweredFn>);

impl CompleteLoweredFunctionSet {
    /// The functions that took an image index, in index order.
    fn functions(&self) -> &[LoweredFn] {
        &self.0
    }
}

/// No function in the minted lowered set reaches itself by direct calls.
///
/// **The claim is over the MINTED lowered set**, exactly as
/// [`CompleteLoweredFunctionSet`] defines it: acyclicity is established for the
/// functions that took an image index, not for reserved-but-undrained instances.
struct AcyclicCallGraph;

/// Every export entry that mutates durable state owns its transaction region, so the
/// requires-ambient-transaction fixpoint converged with nothing to report.
struct AmbientTransactionClosure;

/// The eight semantic artifacts of one pass, each present exactly when its phase ran
/// to completion. `encode` consumes all eight.
struct Artifacts {
    types: Option<CompleteTypeRegistry>,
    functions: Option<SignaturesComplete>,
    template_proofs: Option<AcceptedQueuedTemplateProofs>,
    function_bodies: Option<CompleteDeclaredFunctionBodies>,
    test_bodies: Option<CompleteDeclaredTestBodies>,
    lowered: Option<CompleteLoweredFunctionSet>,
    call_graph: Option<AcyclicCallGraph>,
    transactions: Option<AmbientTransactionClosure>,
}

impl Artifacts {
    /// Whether every artifact is available. Written as an exhaustive destructure so a
    /// ninth artifact is a build error here rather than a silently ignored field.
    fn all_available(&self) -> bool {
        let Artifacts {
            types,
            functions,
            template_proofs,
            function_bodies,
            test_bodies,
            lowered,
            call_graph,
            transactions,
        } = self;
        types.is_some()
            && functions.is_some()
            && template_proofs.is_some()
            && function_bodies.is_some()
            && test_bodies.is_some()
            && lowered.is_some()
            && call_graph.is_some()
            && transactions.is_some()
    }

    /// The semantic fence, in exact order, over the pass's finished terminal. An
    /// invariant returned earlier; a non-empty terminal — rows, or a `Limited` terminal
    /// reporting its own diagnostic bound — is the diagnostic outcome; an empty terminal
    /// with any artifact unavailable is an invariant, because an unavailable artifact
    /// always follows a refusal that reported. `None` is the checked program, and the
    /// image-policy verdict is taken strictly after that point.
    fn refusal(&self, terminal: BoundedDiagnostics) -> Option<SemanticOutcome> {
        if !terminal.is_empty() {
            return Some(SemanticOutcome::Diagnostics(
                terminal,
                CompileStage::PostLoweringValidation,
            ));
        }
        (!self.all_available()).then_some(SemanticOutcome::Invariant(
            InvariantCause::UnavailableWithoutReport,
        ))
    }
}

/// A semantically checked program, immediately before the production projection
/// encodes it. Holding the draft here rather than an [`EncodedImage`] is what keeps
/// the image-policy verdict out of the semantic outcome: the analysis path consumes
/// this value without ever encoding, so no image is allocated on it and no public
/// image bound is reachable from it.
struct CheckedProgram {
    draft: ImageDraft,
    exports: Vec<ExportEntry>,
    tests: Vec<TestEntry>,
    naming: DurableNaming,
}

impl CheckedProgram {
    /// The production projection: encode the checked draft into canonical image bytes.
    /// This is the single point at which a public image-policy bound is consulted, and
    /// [`Driven::into_built`] is its only caller.
    fn encode(self) -> Result<Built, ImagePolicyOutcome> {
        match self.draft.encode() {
            Ok(image) => Ok(Built {
                image,
                exports: self.exports,
                tests: self.tests,
                naming: self.naming,
            }),
            Err(error) => Err(image_build_outcome(error)),
        }
    }
}

impl Driven {
    /// Project the production compile result. The first logically non-empty
    /// stage in order — parse, then structural, then semantic — is the
    /// failure, byte-identical to the historical staged early-return (a
    /// stage's rows are never sorted, deduped, or merged with a later
    /// stage's, so no cross-stage limit strengthening can occur: a limit
    /// arises only within the stage whose own collector crossed it). The parse
    /// and structural stages carry no stage tag: an empty terminal passes over
    /// and a non-empty one is already the failure, so only a semantic stage can
    /// reach the tagged empty boundary. A semantic diagnostics terminal that is
    /// complete and empty is that empty-boundary invariant. A fully clean pass
    /// yields the image.
    fn into_built(self) -> Result<Built, CompileFailure> {
        if let Some(failure) = stage_failure(self.parse) {
            return Err(failure);
        }
        if let Some(failure) = stage_failure(self.structural) {
            return Err(failure);
        }
        match self.semantic {
            SemanticOutcome::Checked(program) => match program.encode() {
                Ok(built) => Ok(built),
                Err(ImagePolicyOutcome::ResourceLimit(limit)) => {
                    Err(CompileFailure::ResourceLimit(limit))
                }
                Err(ImagePolicyOutcome::Invariant(cause)) => {
                    Err(CompileFailure::Invariant(CompileInvariant(cause)))
                }
            },
            SemanticOutcome::Diagnostics(diagnostics, stage) => match stage_failure(diagnostics) {
                Some(failure) => Err(failure),
                None => Err(CompileFailure::Invariant(CompileInvariant(
                    InvariantCause::EmptyDiagnostics(stage),
                ))),
            },
            SemanticOutcome::Invariant(cause) => {
                Err(CompileFailure::Invariant(CompileInvariant(cause)))
            }
            SemanticOutcome::ResourceLimit(limit) => Err(CompileFailure::ResourceLimit(limit)),
        }
    }
}

/// Parse every module, then analyze the cleanly-parsed ones. A module with a parse
/// error contributes its parse diagnostics and its declarations are left unanalyzed —
/// dependency resilience: a syntax error in one component does not suppress the
/// diagnostics or facts of an independent valid component. The semantic pass always
/// runs over whatever parsed cleanly; the projection decides what the production
/// compile reports.
fn drive(project: &ProjectInput, mode: TestMode) -> Result<Driven, CompileResourceLimit> {
    // The admission proof carries one coordinate per module, so every fact this pass
    // retains has a coordinate before the first one allocates.
    let admitted = DriveInputAdmission::check(project)?;
    let mut parse = DiagnosticCollector::new();
    let mut facts = AnalysisFactCollector::new(project);
    // Every project module an earlier stage refused whole, with the stage that
    // reported why. They are modules of this project that the semantic pass never
    // sees, so the module ledger declares them refused and a `use` or qualified call
    // into one names that stage's report instead of denying the file exists.
    let mut unparsed: Vec<UnparsedModule> = Vec::new();

    // Pass one decodes and classifies UTF-8 only, appending each invalid
    // file's one typed row immediately, so invalid rows form a canonical-order
    // prefix (A5). A non-UTF-8 file never enters parsing, but it is still a
    // project module that did not parse; record it as broken so a qualified
    // call into it is a dependency gap rather than an absence.
    let mut decoded: Vec<(FileIdentity, FileRef, String, &str)> = Vec::new();
    for (at, module) in admitted.coordinates().zip(project.modules()) {
        let file = module.identity().clone();
        let name = module.module().as_str().to_string();
        match std::str::from_utf8(module.source()) {
            Ok(source) => decoded.push((file, at, name, source)),
            Err(error) => {
                parse.push(SourceDiagnostic::invalid_utf8(
                    &file,
                    error.valid_up_to(),
                    error.error_len(),
                ));
                facts.admit_broken(at);
                unparsed.push(UnparsedModule {
                    name,
                    file,
                    at,
                    stage: SourceStage::Decode,
                });
            }
        }
    }

    // Pass two parses one valid module at a time and immediately consumes its
    // syntax terminal through the one bridge, retaining only the AST and the
    // logical broken status (A5): no collection of un-absorbed parse results
    // ever exists, so retained diagnostic state is bounded by the compiler
    // collector plus the one in-flight file's syntax collector. Absorbing the
    // complete payload is equivalent to the historical Error-severity filter:
    // every syntax producer constructs `Severity::Error` (the private syntax
    // constructor fixes it).
    let mut parsed: Vec<Module> = Vec::new();
    for (file, at, name, source) in decoded {
        let result = parse_source(source);
        let broken = result.diagnostics.summary().count() != 0;
        parse.absorb_syntax(&file, result.diagnostics);
        if broken {
            facts.admit_broken(at);
            unparsed.push(UnparsedModule {
                name: name.clone(),
                file: file.clone(),
                at,
                stage: SourceStage::Parse,
            });
        }
        parsed.push(Module {
            file,
            at,
            name,
            ast: result.file,
            broken,
        });
    }

    // Only cleanly-parsed modules enter analysis; a module with a parse error is skipped
    // as a dependent unit, its parse diagnostics and broken status already recorded. Its
    // tree is dropped here rather than carried: no query reads a retained tree.
    let clean: Vec<Module> = parsed.into_iter().filter(|module| !module.broken).collect();

    // Project each cleanly-parsed module's declaration hierarchy from its parse tree — a
    // pure analysis byproduct of the one traversal, orthogonal to the semantic outcome
    // and ignored by the production compile's projection. A broken module contributes no
    // outline (a `document_symbols` query for it is syntax-unavailable).
    //
    // A module whose outline crosses a per-file count or depth bound is recorded here and
    // skipped: that file's outline becomes an unavailable fact, so nothing partial is
    // retained for it, while every other module still contributes its complete outline and
    // every other query for that same file still answers. The bound is per file and its
    // consequence is per file; it refuses no other fact and no snapshot.
    //
    // Deliberately no early-out on a crossed global ceiling, unlike the hover family: the
    // projection must visit every module to record each per-file crossing, charging the
    // outline it already built is a walk over existing nodes with no allocation, and it is
    // what lets a Bytes crossing strengthen to Count once the composed count crosses. Each
    // outline is dropped as it is charged, so the live peak is one module's outline either
    // way.
    let mut symbol_bounded_files: Vec<FileRef> = Vec::new();
    for module in &clean {
        match crate::analysis::project_document_symbols(&module.ast.declarations) {
            Ok(symbols) => facts.admit_symbols(module.at, symbols),
            Err(_) => symbol_bounded_files.push(module.at),
        }
    }

    // Refuse a structural declaration bound at its offending construct before any
    // image structure is built. These counts are exact source properties — a record's
    // top-level field width and a function's parameter arity — so the check runs on the
    // parse tree ahead of the first draft mutation. Durable member-tree nesting depth is
    // not checked here: it is a property of a resource's declared member tree, so the
    // durable graph builder refuses it at the offending member while it walks that tree.
    let mut structural = DiagnosticCollector::new();
    check_structural_resource_bounds(&clean, &mut structural);

    let semantic = run_semantic(&clean, project, mode, &unparsed, &mut facts);
    // Release every remaining tree before the terminals seal, so the traversal's peak
    // does not overlap the retained set. A completion or active-call query re-parses the
    // one file it names from the snapshot's own retained bytes.
    drop(clean);
    Ok(Driven {
        parse: parse.finish(),
        structural: structural.finish(),
        semantic,
        facts: facts.finish(),
        symbol_bounded_files,
    })
}

/// Analyze the cleanly-parsed modules: build the named types and function signatures,
/// lower every body, and validate the whole, or return the accumulated failure tagged
/// with the stage that produced it. Editor hover facts from each monomorphic function and
/// test body are admitted to the fact ledger as it is lowered, and each generic
/// template body's facts once at its template proof.
fn run_semantic(
    parsed: &[Module],
    project: &ProjectInput,
    mode: TestMode,
    unparsed: &[UnparsedModule],
    facts: &mut AnalysisFactCollector,
) -> SemanticOutcome {
    /// A path's segments in the dotted spelling this crate identifies modules by. The
    /// source spells the same path with `::`; the two are different representations of
    /// one path, so this builds the identity from the segments rather than rewriting the
    /// separators of a rendered spelling.
    fn dotted_module_path(segments: &[marrow_syntax::NameSegment]) -> String {
        segments
            .iter()
            .map(marrow_syntax::NameSegment::text)
            .collect::<Vec<_>>()
            .join(".")
    }

    let mut diagnostics = DiagnosticCollector::new();
    // One retention budget for the whole pass. Every ledger below charges its
    // retained refusals against it, so the declared ceiling bounds what the pass
    // holds rather than what any single namespace holds.
    let budget = DeclarationBudget::default();
    // Store roots whose durable identity failed admission, steered to their identity
    // reports once each across the whole compile rather than at every reference.
    // The source-root-relative path is the authority for module identity. A file
    // that declares a `module` header is an importable module and must spell the
    // path-derived name (with `::` as the dotted separator). A file with no header
    // is a single-file script: it keeps a path-derived identity for its own scope
    // and its exports, but is not importable by module path.
    //
    // A refused module keeps its dotted path: the file is in the project whether or
    // not it was admitted, so denying that the project contains it is a false
    // statement about a file the reader can see. Module order is not observed —
    // nothing takes a slot from this ledger — so the stage-refused modules are
    // declared first and the header-checked ones after.
    let mut modules = ModuleLedger::new(DeclarationNamespace::Module, budget.clone());
    for module in unparsed {
        let refusal = refuse_at_earlier_stage(
            DeclarationSite::whole_file(&module.name, &module.file, module.at),
            module.stage,
        );
        if let Err(error) =
            modules.declare(module.name.clone(), DeclarationOccurrence::Refused(refusal))
        {
            return error.into();
        }
    }
    for module in parsed {
        let Some(header) = &module.ast.module else {
            continue;
        };
        let declared = dotted_module_path(&header.segments);
        let occurrence = if declared == module.name {
            DeclarationOccurrence::Accepted(ModuleBinding)
        } else {
            DeclarationOccurrence::Refused(refuse(
                &mut diagnostics,
                DeclarationSite {
                    name: &module.name,
                    file: &module.file,
                    at: module.at,
                    span: header.span,
                },
                Code::CheckModulePath.as_str(),
                format!(
                    "module header `{}` does not match its path; expected `module {}`",
                    marrow_syntax::name_path_spelling(&header.segments),
                    module.name.replace('.', "::")
                ),
            ))
        };
        if let Err(error) = modules.declare(module.name.clone(), occurrence) {
            return error.into();
        }
    }

    // Each module's `use` bindings (final segment -> dotted target). A `use` must
    // name an importable project module; two imports binding the same final segment
    // in one module are ambiguous.
    let mut imports: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for module in parsed {
        let bindings = imports.entry(module.name.clone()).or_default();
        for use_decl in &module.ast.uses {
            let target = dotted_module_path(&use_decl.segments);
            let segment = target
                .rsplit('.')
                .next()
                .unwrap_or(target.as_str())
                .to_string();
            let spelling = marrow_syntax::name_path_spelling(&use_decl.segments);
            let binding = match modules.lookup(target.as_str()) {
                Ok(binding) => binding,
                Err(drift) => {
                    return SemanticOutcome::Invariant(InvariantCause::Generic(drift.into()));
                }
            };
            match binding {
                Binding::Accepted(ModuleBinding) => {}
                // The project contains the module and refused it. The import fails
                // for that cause, which it names, rather than denying the module.
                Binding::Refused(_, summary) => {
                    diagnostics.push(SourceDiagnostic::at(
                        Code::CheckImport.as_str(),
                        &module.file,
                        use_decl.span,
                        format!(
                            "`{spelling}` is a module of this project, but its declaration \
                             was refused, so this import binds nothing. {}",
                            summary.correction()
                        ),
                    ));
                    continue;
                }
                Binding::Absent => {
                    diagnostics.push(SourceDiagnostic::at(
                        Code::CheckImport.as_str(),
                        &module.file,
                        use_decl.span,
                        format!("no module `{spelling}` in this project"),
                    ));
                    continue;
                }
            }
            if bindings.iter().any(|(seg, _)| seg == &segment) {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckImport.as_str(),
                    &module.file,
                    use_decl.span,
                    format!("import `{segment}` is already bound by another `use` in this module"),
                ));
                continue;
            }
            bindings.push((segment, target));
        }
    }

    // A module has at most one function with a given name, so an unqualified or
    // qualified call resolves to one target.
    reject_duplicate_functions(parsed, &mut diagnostics);

    // The function signatures paired with their dotted module, in declaration order
    // (the order lowering assigns image indices).
    let functions: Vec<DeclaredFn<'_>> = parsed
        .iter()
        .flat_map(|module| {
            module.ast.declarations.iter().filter_map(|decl| {
                if let Declaration::Function(function) = decl {
                    Some(DeclaredFn {
                        file: module.file.clone(),
                        at: module.at,
                        module: module.name.clone(),
                        decl: function,
                    })
                } else {
                    None
                }
            })
        })
        .collect();

    // Build the named types — transparent aliases plus the single project record
    // type — and the function signatures before body lowering, so annotations,
    // constructors, field reads, and forward calls resolve.
    let mut draft = ImageDraft::new();
    let aliases: Vec<(FileRef, FileIdentity, &AliasDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.ast.declarations.iter().filter_map(|decl| {
                if let Declaration::Alias(alias) = decl {
                    Some((module.at, module.file.clone(), alias))
                } else {
                    None
                }
            })
        })
        .collect();
    let nominals: Vec<(FileRef, FileIdentity, &NominalDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.ast.declarations.iter().filter_map(|decl| {
                if let Declaration::Nominal(nominal) = decl {
                    Some((module.at, module.file.clone(), nominal))
                } else {
                    None
                }
            })
        })
        .collect();
    let resources: Vec<(FileRef, FileIdentity, &ResourceDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.ast.declarations.iter().filter_map(|decl| {
                if let Declaration::Resource(resource) = decl {
                    Some((module.at, module.file.clone(), resource))
                } else {
                    None
                }
            })
        })
        .collect();
    let structs: Vec<(FileRef, FileIdentity, &StructDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.ast.declarations.iter().filter_map(|decl| {
                if let Declaration::Struct(item) = decl {
                    Some((module.at, module.file.clone(), item))
                } else {
                    None
                }
            })
        })
        .collect();
    let enums: Vec<(FileRef, FileIdentity, &EnumDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.ast.declarations.iter().filter_map(|decl| {
                if let Declaration::Enum(item) = decl {
                    Some((module.at, module.file.clone(), item))
                } else {
                    None
                }
            })
        })
        .collect();
    let records = match TypeRegistry::build(
        &mut draft,
        &aliases,
        &nominals,
        &structs,
        &enums,
        &resources,
        &mut diagnostics,
        budget.clone(),
    ) {
        Ok(records) => records,
        Err(error) => return error.into(),
    };
    if let Some(invariant) = records.build_invariant() {
        return SemanticOutcome::Invariant(InvariantCause::Generic(invariant));
    }
    if records.has_instantiation_limit() {
        records
            .take_generic_diagnostics()
            .merge_into(&mut diagnostics);
        return SemanticOutcome::Diagnostics(diagnostics.finish(), CompileStage::TypeInstantiation);
    }
    // Each store carries its file in both representations: the owned identity a
    // diagnostic renders, and the `Copy` coordinate a retained refusal keeps.
    let stores: Vec<(FileRef, FileIdentity, &StoreDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.ast.declarations.iter().filter_map(|decl| {
                if let Declaration::Store(store) = decl {
                    Some((module.at, module.file.clone(), store))
                } else {
                    None
                }
            })
        })
        .collect();
    let durable = match DurableRegistry::build(
        &mut draft,
        &records,
        &resources,
        &stores,
        project.identity_ledger(),
        &mut diagnostics,
        budget.clone(),
    ) {
        Ok(durable) => durable,
        Err(error) => return error.into(),
    };
    // The type registry resolved every declared type: every later phase resolves its
    // annotations through it.
    let types = Some(CompleteTypeRegistry);

    // The signature table is always built. A refused signature is a refused ledger
    // entry, so every phase below still resolves call sites — a call to the refused
    // function reuses its declaration's cause, and every unrelated body lowers and
    // reports its own errors instead of being silenced by one bad annotation.
    let signatures = match FunctionRegistry::build(
        &records,
        &mut draft,
        &durable,
        &functions,
        modules,
        imports,
        &mut diagnostics,
        budget.clone(),
    ) {
        Ok(signatures) => CompleteFunctionRegistry(signatures),
        Err(error) => return error.into(),
    };
    let function_registry = signatures.complete();
    // Generic functions are templates with no image index; they are monomorphized at
    // each call site and once-checked below against their constraints.
    let generic_functions: Vec<DeclaredFn<'_>> = parsed
        .iter()
        .flat_map(|module| {
            module.ast.declarations.iter().filter_map(|decl| {
                if let Declaration::Function(function) = decl
                    && !function.type_params.is_empty()
                {
                    Some(DeclaredFn {
                        file: module.file.clone(),
                        at: module.at,
                        module: module.name.clone(),
                        decl: function,
                    })
                } else {
                    None
                }
            })
        })
        .collect();
    let generics = GenericRegistry::build(&generic_functions);

    // Module-private constants, evaluated before body lowering so a reference folds
    // to its value.
    let const_decls: Vec<(String, FileRef, FileIdentity, &ConstDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.ast.declarations.iter().filter_map(|decl| {
                if let Declaration::Const(konst) = decl {
                    Some((module.name.clone(), module.at, module.file.clone(), konst))
                } else {
                    None
                }
            })
        })
        .collect();
    let constants = match ConstRegistry::build(&const_decls, &records, &mut diagnostics, budget) {
        Ok(constants) => constants,
        Err(full) => return full.into(),
    };

    // Everything from the template proof to the instance drain resolves call sites
    // through the signature table, which is always available.
    let resolution = Resolution {
        records: &records,
        durable: &durable,
        signatures: signatures.signatures(),
        generics: &generics,
        constants: &constants,
    };
    let phases = match registry_phases(
        parsed,
        mode,
        resolution,
        &mut draft,
        &mut diagnostics,
        facts,
    ) {
        Ok(phases) => phases,
        Err(PhaseStop::StageDiagnostics(stage)) => {
            return SemanticOutcome::Diagnostics(diagnostics.finish(), stage);
        }
        Err(PhaseStop::Invariant(cause)) => return SemanticOutcome::Invariant(cause),
    };
    let RegistryPhases {
        template_proofs,
        function_bodies,
        test_bodies,
        lowered_set,
        exports,
        tests,
    } = phases;

    // Report any diagnostics recorded while minting generic type instantiations
    // (the shared instantiation limit) and reject a value-containment cycle over the
    // full set of concrete types and generic instantiations minted anywhere (a
    // monomorphized `Tree[int]` containing `Tree[int]` is an ordinary record cycle),
    // now that every field and body annotation has been resolved.
    let stopped_on_limit = records.has_instantiation_limit();
    records
        .take_generic_diagnostics()
        .merge_into(&mut diagnostics);
    if stopped_on_limit {
        return SemanticOutcome::Diagnostics(diagnostics.finish(), CompileStage::BodyLowering);
    }
    if let Err(invariant) =
        crate::types::reject_value_cycles(&records, &structs, &resources, &mut diagnostics)
    {
        return SemanticOutcome::Invariant(InvariantCause::Generic(invariant));
    }

    // The compiled subset does not admit recursion: the direct-call graph must be
    // acyclic. Reported at check time so the source carries the diagnostic. The
    // verifier independently rejects any cycle that still reaches it (image.closure),
    // so this is a source-facing check, not the trust boundary. Only run it once
    // every function, test, and generic instance lowered, so the indices are aligned.
    let call_graph = lowered_set
        .as_ref()
        .and_then(|set| reject_recursion(set, &mut diagnostics));

    // A function that mutates durable state carries a checked requires-ambient-
    // transaction effect: it is callable only inside a `transaction` block or from
    // another function carrying the effect. Reported at check time so the source, not
    // the image, carries the diagnostic; the verifier reconstructs the same closure and
    // rejects a tampered image (image.flow) as defense in depth. Run once the call
    // graph is acyclic so the effect fixpoint terminates and indices are aligned.
    let transactions = lowered_set
        .as_ref()
        .zip(call_graph.as_ref())
        .and_then(|(set, acyclic)| reject_missing_transaction(set, acyclic, &mut diagnostics));

    // The remaining transaction-ownership laws — exactly one region per mutating export,
    // committed on every path with no durable operation after the commit; a `transaction`
    // marker only in the export that owns it; and no call to a transaction owner — are
    // reconstructed from the lowered tape and reported at the offending source construct.
    // Reported at check time so the source, not the image, carries the diagnostic; the
    // verifier reconstructs the same lattice from the image alone and rejects a tampered
    // image (image.flow) as defense in depth. Run once the call graph is acyclic and no
    // requires-ambient-transaction report already stands, so the closures converge and a
    // single mutation cannot cascade into an ownership report.
    if let (Some(set), Some(closure)) = (&lowered_set, &transactions) {
        reject_transaction_ownership(set, closure, &mut diagnostics);
    }

    // A test body reaches durable data in one of two disjoint ways — directly, or by
    // driving exports — and may not do both. Reported at check time so the source
    // carries the diagnostic; the verifier's test-entry phase rejects a mixed image
    // (image.test_entry) as defense in depth. Run once the call graph is acyclic.
    if let (Some(set), Some(acyclic)) = (&lowered_set, &call_graph) {
        reject_mixed_test_bodies(set, acyclic, &mut diagnostics);
    }

    // The semantic fence, in exact order. An invariant has already returned above. A
    // non-empty terminal — rows, or a Limited terminal reporting its own diagnostic
    // bound — is the diagnostic outcome. An empty terminal with any artifact
    // unavailable is an invariant: an unavailable artifact always follows a refusal
    // that reported. Only an empty terminal with all eight artifacts available is a
    // checked program, and the image-policy verdict is taken strictly after this
    // point, in the production projection alone.
    let artifacts = Artifacts {
        types,
        functions: function_registry,
        template_proofs,
        function_bodies,
        test_bodies,
        lowered: lowered_set,
        call_graph,
        transactions,
    };
    if let Some(refusal) = artifacts.refusal(diagnostics.finish()) {
        return refusal;
    }
    SemanticOutcome::Checked(Box::new(CheckedProgram {
        draft,
        exports,
        tests,
        naming: durable.naming(),
    }))
}

/// Everything the registry-dependent phases resolve names through. Shared and read-only
/// for the whole region: the type registry records an instantiation behind its own
/// interior mutability, so no phase needs to own it.
#[derive(Clone, Copy)]
struct Resolution<'a, 'p> {
    records: &'a TypeRegistry,
    durable: &'a DurableRegistry,
    signatures: &'a FunctionRegistry,
    generics: &'a GenericRegistry<'p>,
    constants: &'a ConstRegistry,
}

/// A registry-dependent phase ended the whole semantic pass. The caller owns the
/// diagnostic collector, so a stop names the outcome instead of sealing the terminal.
enum PhaseStop {
    /// The shared instantiation limit stopped this stage. Its diagnostics are already
    /// merged into the caller's collector.
    StageDiagnostics(CompileStage),
    Invariant(InvariantCause),
}

/// What the phases that resolve through the function registry produced: their artifacts,
/// the image content they minted, and the export and test-entry tables they filled.
struct RegistryPhases {
    template_proofs: Option<AcceptedQueuedTemplateProofs>,
    function_bodies: Option<CompleteDeclaredFunctionBodies>,
    test_bodies: Option<CompleteDeclaredTestBodies>,
    lowered_set: Option<CompleteLoweredFunctionSet>,
    exports: Vec<ExportEntry>,
    tests: Vec<TestEntry>,
}

/// How a declaration-lowering loop ended.
///
/// Only [`DeclarationExit::Exhausted`] mints the set's completeness artifact. A refusal
/// leaves the refused declaration's reserved index unminted; a stop on the shared
/// instantiation limit leaves the whole unvisited suffix unminted, which is the same
/// hole once per declaration after the stop. The exit is named so neither can mint a
/// completeness artifact over a truncated set: a flag cleared at the refusal site alone
/// would leave the limit stop claiming a complete set.
#[derive(Clone, Copy)]
enum DeclarationExit {
    /// Every declaration in the set took the index reserved for it.
    Exhausted,
    /// A body was refused; its reserved index was never minted.
    Refused,
    /// The shared instantiation limit stopped the loop, leaving its unvisited suffix
    /// unlowered.
    StoppedOnInstantiationLimit,
}

impl DeclarationExit {
    /// Whether every declaration in the set took the index reserved for it.
    fn complete(self) -> bool {
        matches!(self, DeclarationExit::Exhausted)
    }
}

/// The declared monomorphic function bodies, lowered into the draft.
struct LoweredFunctions {
    lowered: Vec<LoweredFn>,
    exports: Vec<ExportEntry>,
    exit: DeclarationExit,
}

/// The declared test bodies, lowered into the draft and bound into the test-entry table.
struct LoweredTests {
    lowered: Vec<LoweredFn>,
    entries: Vec<TestEntry>,
    exit: DeclarationExit,
}

/// Run every phase that resolves call sites through the complete function registry: the
/// once-checked template proof, the declared function bodies, the declared test bodies,
/// and the generic instance drain. Each phase records its own artifact as it completes;
/// none is inferred from the diagnostic set.
fn registry_phases(
    parsed: &[Module],
    mode: TestMode,
    resolution: Resolution<'_, '_>,
    draft: &mut ImageDraft,
    diagnostics: &mut DiagnosticCollector,
    facts: &mut AnalysisFactCollector,
) -> Result<RegistryPhases, PhaseStop> {
    let records = resolution.records;

    // Once-checked template pass: every generic function's body is type-checked once
    // against its type parameters' constraints — independently of whether or how it is
    // instantiated — so an unconstrained parameter used with `==`/`<`, or any other
    // constraint violation, is caught here rather than per instantiation.
    for template in resolution.generics.templates() {
        let outcome = FnLowerer::check_template(
            draft,
            records,
            resolution.durable,
            resolution.signatures,
            resolution.generics,
            resolution.constants,
            facts.sink(template.at()),
            template,
        )
        .map_err(|invariant| PhaseStop::Invariant(InvariantCause::Generic(invariant)))?;
        diagnostics.absorb(outcome.diagnostics);
        records.adopt_generic_diagnostics(outcome.generic);
        if records.has_instantiation_limit() {
            break;
        }
    }
    if records.has_instantiation_limit() {
        records.take_generic_diagnostics().merge_into(diagnostics);
        return Err(PhaseStop::StageDiagnostics(CompileStage::TemplateProof));
    }
    // Reaching here is the artifact: the pass returned above on the one outcome that
    // withholds it, so every phase below runs with the queued instance set trustworthy.
    // Writing it as a value rather than an `Option` keeps the gates below falsifiable —
    // a conjunct that cannot be false proves nothing about the phase it claims to gate.
    let template_proofs = AcceptedQueuedTemplateProofs;

    // Generic instances are image functions with no stable identity, indexed after every
    // monomorphic function and test. `base` is that boundary; the shared `Monomorph`
    // assigns each distinct instance the next index from `base` in discovery order, so
    // draining its queue in order appends them to the image in index order.
    let test_count: u16 = if mode == TestMode::Include {
        parsed
            .iter()
            .flat_map(|module| &module.ast.declarations)
            .filter(|decl| matches!(decl, Declaration::Test(_)))
            .count() as u16
    } else {
        0
    };
    records.set_fn_base(resolution.signatures.concrete_count() + test_count);

    let functions = lower_declared_functions(parsed, resolution, draft, diagnostics, facts)?;
    let function_bodies = functions
        .exit
        .complete()
        .then_some(CompleteDeclaredFunctionBodies);

    let tests = lower_declared_tests(parsed, mode, resolution, draft, diagnostics, facts)?;
    let test_bodies = tests.exit.complete().then_some(CompleteDeclaredTestBodies);

    let mut lowered = functions.lowered;
    lowered.extend(tests.lowered);

    // Drain the generic instantiation worklist: lower each monomorphized instance's body
    // into the image, in the order the instances were minted (so each instance's image
    // index equals the one the registry reserved). Lowering an instance body may mint
    // further instances, which the loop continues to drain. Its precondition is that
    // every declared body took the index reserved for it, so instances append after
    // them — carried by the three artifacts above, not by an empty diagnostic set.
    let mut drain_lowered_every_instance = true;
    if function_bodies.is_some() && test_bodies.is_some() {
        while let Some((template_index, args, reserved)) = records.next_fn_pending() {
            let template = &resolution.generics.templates()[template_index];
            let result = match FnLowerer::lower_instance(
                draft,
                records,
                resolution.durable,
                resolution.signatures,
                resolution.generics,
                resolution.constants,
                diagnostics,
                FactSink::Discarding,
                template,
                &args,
            ) {
                Ok(BodyOutcome::Lowered(result)) => result,
                Ok(BodyOutcome::Refused) => {
                    drain_lowered_every_instance = false;
                    break;
                }
                Err(invariant) => {
                    return Err(PhaseStop::Invariant(InvariantCause::Generic(invariant)));
                }
            };
            // The registry reserved this index before the body was lowered; the draft
            // assigned the one the body actually took. A divergence means the image would
            // carry an instance under an index some call site does not name, so it is a
            // typed invariant in release exactly as in debug.
            if result.func.index() != reserved {
                return Err(PhaseStop::Invariant(InvariantCause::ReservedIndexMismatch));
            }
            lowered.push(LoweredFn {
                index: result.func.index(),
                file: template.source_file().clone(),
                name: template.name().to_string(),
                span: template.span(),
                callees: result.callees,
                is_export: false,
                is_test: false,
                unwrapped_mutations: result.unwrapped_mutations,
                unwrapped_calls: result.unwrapped_calls,
                has_direct_durable_op: result.has_direct_durable_op,
                owns_transaction: result.owns_transaction,
                code: result.code,
                code_spans: result.code_spans,
            });
        }
    }

    // The lowered set is complete when every declared function body lowered and the
    // drain — if it ran — lowered every instance it was offered, under the accepted
    // template proofs the whole region runs beneath. A duplicate test title is a
    // declaration refusal, not a lowering refusal: the minted indices stay dense, so it
    // withholds only the drain.
    let lowered_set = (function_bodies.is_some() && drain_lowered_every_instance)
        .then_some(CompleteLoweredFunctionSet(lowered));

    Ok(RegistryPhases {
        template_proofs: Some(template_proofs),
        function_bodies,
        test_bodies,
        lowered_set,
        exports: functions.exports,
        tests: tests.entries,
    })
}

/// Lower each declared monomorphic function, in the same order the registry assigned
/// indices, minting an export for each public function from its declaration path and
/// recording its direct-call edges for recursion detection. Generic templates are
/// skipped — they are monomorphized on demand and drained separately.
fn lower_declared_functions(
    parsed: &[Module],
    resolution: Resolution<'_, '_>,
    draft: &mut ImageDraft,
    diagnostics: &mut DiagnosticCollector,
    facts: &mut AnalysisFactCollector,
) -> Result<LoweredFunctions, PhaseStop> {
    let records = resolution.records;
    let mut lowered: Vec<LoweredFn> = Vec::new();
    let mut exports: Vec<ExportEntry> = Vec::new();
    let mut exit = DeclarationExit::Exhausted;
    // The signature build walked these same declarations in this same order, so the
    // refusal a body asks about is the one its own declaration received. Asking by
    // name would answer for the first declaration of a repeated name at every later
    // one.
    let mut signatures = resolution.signatures.declarations();
    for module in parsed {
        for declaration in &module.ast.declarations {
            let Declaration::Function(function) = declaration else {
                // Constants are evaluated into the const registry before this pass;
                // aliases, nominals, records, resources, and stores are handled by their
                // own registries; test declarations are lowered after every function has
                // an index.
                continue;
            };
            if !function.type_params.is_empty() {
                // A generic template is not lowered in place.
                continue;
            }
            match signatures.next_at(module.at, function.name_span) {
                // The signature was refused and reported at the annotation it could
                // not resolve. Lowering the body would resolve the same annotation
                // again and report it a second time, and there is no parameter list
                // to bind, so the declaration is refused whole: it takes no image
                // index, exactly as a body refused for its own error does.
                Ok(SignatureOutcome::Refused) => {
                    exit = DeclarationExit::Refused;
                    continue;
                }
                Ok(SignatureOutcome::Resolved) => {}
                Err(drift) => {
                    return Err(PhaseStop::Invariant(InvariantCause::Generic(drift.into())));
                }
            }
            let result = match FnLowerer::lower(
                draft,
                records,
                resolution.durable,
                resolution.signatures,
                resolution.generics,
                resolution.constants,
                diagnostics,
                facts.sink(module.at),
                &module.file,
                &module.name,
                function,
            ) {
                Ok(BodyOutcome::Lowered(result)) => result,
                Ok(BodyOutcome::Refused) => {
                    if records.has_instantiation_limit() {
                        return Ok(LoweredFunctions {
                            lowered,
                            exports,
                            exit: DeclarationExit::StoppedOnInstantiationLimit,
                        });
                    }
                    exit = DeclarationExit::Refused;
                    continue;
                }
                Err(invariant) => {
                    return Err(PhaseStop::Invariant(InvariantCause::Generic(invariant)));
                }
            };
            lowered.push(LoweredFn {
                index: result.func.index(),
                file: module.file.clone(),
                name: function.name.clone(),
                span: function.span,
                callees: result.callees,
                is_export: function.public,
                is_test: false,
                unwrapped_mutations: result.unwrapped_mutations,
                unwrapped_calls: result.unwrapped_calls,
                has_direct_durable_op: result.has_direct_durable_op,
                owns_transaction: result.owns_transaction,
                code: result.code,
                code_spans: result.code_spans,
            });
            if function.public {
                // The injectivity owner's own guard: every dotted module segment and the
                // item must be ASCII identifiers before an ExportId is minted over them
                // (see marrow-image::export_id). Unreachable through the current capture
                // path, which already constrains both; kept so the id payload's
                // injectivity never silently rests on an upstream layer alone.
                if valid_export_path(&module.name, &function.name) {
                    let id = ExportId::of_local(&module.name, &function.name);
                    draft.add_export(id, result.func);
                    exports.push(ExportEntry {
                        module: module.name.clone(),
                        item: function.name.clone(),
                        id,
                    });
                } else {
                    diagnostics.push(SourceDiagnostic::at(
                        Code::CheckModulePath.as_str(),
                        &module.file,
                        function.span,
                        format!(
                            "export `{}` in module `{}` is not an ASCII identifier path, \
                             so it cannot be exported",
                            function.name, module.name
                        ),
                    ));
                    continue;
                }
            }
            if records.has_instantiation_limit() {
                return Ok(LoweredFunctions {
                    lowered,
                    exports,
                    exit: DeclarationExit::StoppedOnInstantiationLimit,
                });
            }
        }
    }
    Ok(LoweredFunctions {
        lowered,
        exports,
        exit,
    })
}

/// Lower each `test "name"` body into a storeless, zero-argument function and bind its
/// title into the TEST-ENTRY table. Tests are lowered after every function so their
/// bodies' calls resolve and their own indices follow the functions'. Titles are unique
/// across the project; a duplicate title skips its body, which leaves the index reserved
/// for it unminted. With tests excluded no test is declared to lower, so the set is
/// vacuously exhausted.
fn lower_declared_tests(
    parsed: &[Module],
    mode: TestMode,
    resolution: Resolution<'_, '_>,
    draft: &mut ImageDraft,
    diagnostics: &mut DiagnosticCollector,
    facts: &mut AnalysisFactCollector,
) -> Result<LoweredTests, PhaseStop> {
    let records = resolution.records;
    let mut lowered: Vec<LoweredFn> = Vec::new();
    let mut entries: Vec<TestEntry> = Vec::new();
    if mode != TestMode::Include {
        return Ok(LoweredTests {
            lowered,
            entries,
            exit: DeclarationExit::Exhausted,
        });
    }
    if records.has_instantiation_limit() {
        return Ok(LoweredTests {
            lowered,
            entries,
            exit: DeclarationExit::StoppedOnInstantiationLimit,
        });
    }
    let mut exit = DeclarationExit::Exhausted;
    for module in parsed {
        for declaration in &module.ast.declarations {
            let Declaration::Test(test) = declaration else {
                continue;
            };
            if entries.iter().any(|existing| existing.name == test.name) {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckNameConflict.as_str(),
                    &module.file,
                    test.name_span,
                    format!("a test named `{}` is already declared", test.name),
                ));
                exit = DeclarationExit::Refused;
                continue;
            }
            let result = match FnLowerer::lower_test(
                draft,
                records,
                resolution.durable,
                resolution.signatures,
                resolution.generics,
                resolution.constants,
                diagnostics,
                facts.sink(module.at),
                &module.file,
                &module.name,
                &test.name,
                &test.body,
            ) {
                Ok(BodyOutcome::Lowered(result)) => result,
                Ok(BodyOutcome::Refused) => {
                    if records.has_instantiation_limit() {
                        return Ok(LoweredTests {
                            lowered,
                            entries,
                            exit: DeclarationExit::StoppedOnInstantiationLimit,
                        });
                    }
                    exit = DeclarationExit::Refused;
                    continue;
                }
                Err(invariant) => {
                    return Err(PhaseStop::Invariant(InvariantCause::Generic(invariant)));
                }
            };
            lowered.push(LoweredFn {
                index: result.func.index(),
                file: module.file.clone(),
                name: test.name.clone(),
                span: test.name_span,
                callees: result.callees,
                is_export: false,
                is_test: true,
                unwrapped_mutations: result.unwrapped_mutations,
                unwrapped_calls: result.unwrapped_calls,
                has_direct_durable_op: result.has_direct_durable_op,
                owns_transaction: result.owns_transaction,
                code: result.code,
                code_spans: result.code_spans,
            });
            let name_id = draft.intern_string(&test.name);
            draft.add_test_entry(name_id, result.func);
            entries.push(TestEntry {
                name: test.name.clone(),
                module: module.name.clone(),
                file: module.file.as_str().to_string(),
                line: test.name_span.line,
                column: test.name_span.column,
            });
            if records.has_instantiation_limit() {
                return Ok(LoweredTests {
                    lowered,
                    entries,
                    exit: DeclarationExit::StoppedOnInstantiationLimit,
                });
            }
        }
    }
    Ok(LoweredTests {
        lowered,
        entries,
        exit,
    })
}

/// The complete diagnostic picture the editor analysis snapshot consumes: every stage's
/// diagnostics over every module — the resilient union, not the first-non-empty
/// projection the production compile takes — or the dominating non-diagnostic failure.
pub(crate) enum Analyzed {
    /// The complete bounded diagnostic set, in compiler order (empty for a clean
    /// project). A snapshot is producible.
    Diagnostics(Vec<SourceDiagnostic>),
    /// An aggregate resource bound with no diagnostic to dominate it.
    ResourceLimit(CompileResourceLimit),
    /// An opaque compiler-coherence failure that dominates everything.
    Invariant(CompileInvariant),
}

/// The complete analysis of a project for the editor snapshot: the diagnostic outcome,
/// the retained editor facts, and the identities of files that did not parse (so a
/// position in one of them is a syntax-unavailable fact, not an absent one).
pub(crate) struct ProjectAnalysis {
    pub(crate) outcome: Analyzed,
    pub(crate) facts: BoundedAnalysisFacts,
    pub(crate) symbol_bounded_files: Box<[FileRef]>,
}

/// Drive the analysis pass over a project — test bodies included, per the editor
/// analysis contract — and resolve its complete diagnostic picture under the shared
/// precedence `Invariant > Diagnostics > ResourceLimit`. The complete union of every
/// stage's diagnostics is sealed against the same CRES01 count/byte bounds the
/// production compile uses, so a diagnostic avalanche transactionally becomes a resource
/// limit rather than a retained partial set — no partial or truncated snapshot is
/// admitted.
pub(crate) fn analyze_project(
    project: &ProjectInput,
) -> Result<ProjectAnalysis, CompileResourceLimit> {
    let driven = drive(project, TestMode::Include)?;
    // The ledger records broken files directly from decode and parse status, so the
    // fact survives even when the parse stage's diagnostic payload was discarded by a
    // limit.
    let facts = driven.facts;
    let symbol_bounded_files = driven.symbol_bounded_files.into_boxed_slice();
    let outcome = analyze_outcome(driven.parse, driven.structural, driven.semantic);
    Ok(ProjectAnalysis {
        outcome,
        facts,
        symbol_bounded_files,
    })
}

/// Resolve the complete diagnostic outcome under the shared precedence from the driven
/// stage terminals. Analysis alone creates a temporary fourth collector that
/// consumes parse, then structural, then semantic diagnostics before one
/// finish — the ordered cross-stage union, in which an OwnedBytes limit may
/// strengthen to Count across stages (the production projection never merges
/// stages, so it never strengthens across them).
fn analyze_outcome(
    parse: BoundedDiagnostics,
    structural: BoundedDiagnostics,
    semantic: SemanticOutcome,
) -> Analyzed {
    // The parse and structural prechecks preempt the semantic pass in the production
    // compile: `into_built` returns those stages before the semantic outcome is
    // consulted. So a real precheck diagnostic dominates a semantic invariant or
    // resource limit here too — otherwise a defense-in-depth encode outcome (a bound the
    // precheck already owns) would diverge from the production result. The semantic
    // pass's own diagnostics still union in for dependency resilience.
    let precheck_present = !parse.is_empty() || !structural.is_empty();
    let mut union = DiagnosticCollector::new();
    union.absorb(parse);
    union.absorb(structural);
    match semantic {
        SemanticOutcome::Invariant(cause) if !precheck_present => {
            return Analyzed::Invariant(CompileInvariant(cause));
        }
        SemanticOutcome::Diagnostics(semantic, _) => union.absorb(semantic),
        // With prechecks present the semantic invariant is suppressed: the precheck
        // union is the analysis result. A checked program contributes no diagnostic —
        // and is never encoded here, so no image-policy bound is reachable from the
        // analysis path at all.
        SemanticOutcome::Invariant(..) | SemanticOutcome::Checked(_) => {}
        // A semantic bound the pass could not run past. It is a resource limit for
        // the same reason a precheck one is — no source construct is at fault — and
        // it yields to a real precheck diagnostic like the other semantic arms.
        SemanticOutcome::ResourceLimit(limit) if !precheck_present => {
            return Analyzed::ResourceLimit(limit);
        }
        SemanticOutcome::ResourceLimit(..) => {}
    }
    match union.finish() {
        BoundedDiagnostics::Complete { rows, .. } => Analyzed::Diagnostics(rows),
        BoundedDiagnostics::Limited { limit, .. } => {
            Analyzed::ResourceLimit(diagnostic_limit_failure(limit))
        }
    }
}

/// Report a `check.name_conflict` for every function name declared more than once
/// within a single module (a `Call` must resolve to a unique target) and for every
/// function whose name is a reserved built-in the compiler intercepts in call
/// position (`some`/`exists`/`trim`/...); such a function would be admitted and
/// then never reached. Functions of the same name in different modules are distinct
/// and do not conflict.
fn reject_duplicate_functions(parsed: &[Module], diagnostics: &mut DiagnosticCollector) {
    for module in parsed {
        let mut seen: Vec<&str> = Vec::new();
        for declaration in &module.ast.declarations {
            let Declaration::Function(function) = declaration else {
                continue;
            };
            if is_reserved_builtin_name(&function.name) {
                diagnostics.push(reserved_builtin_name(
                    &module.file,
                    function.span,
                    &function.name,
                ));
                continue;
            }
            if seen.contains(&function.name.as_str()) {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckNameConflict.as_str(),
                    &module.file,
                    function.span,
                    format!(
                        "a function named `{}` is already declared in this module",
                        function.name
                    ),
                ));
            } else {
                seen.push(&function.name);
            }
        }
    }
}

/// Report `check.recursion` for every function that participates in a direct or
/// mutual recursion cycle. A function is on a cycle exactly when it can reach
/// itself by following direct calls, so each function is checked for reachability
/// back to itself over the edge set.
fn reject_recursion(
    lowered: &CompleteLoweredFunctionSet,
    diagnostics: &mut DiagnosticCollector,
) -> Option<AcyclicCallGraph> {
    let lowered = lowered.functions();
    // Adjacency by image index. Indices are dense (0..lowered.len()) and each
    // function appears once, so a plain vec keyed by index is exact.
    let mut callees: Vec<&[u16]> = vec![&[]; lowered.len()];
    for function in lowered {
        if (function.index as usize) < callees.len() {
            callees[function.index as usize] = &function.callees;
        }
    }
    let mut reported = false;
    for function in lowered {
        if reaches_self(function.index, &callees) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckRecursion.as_str(),
                &function.file,
                function.span,
                format!("`{}` is part of a recursive call cycle", function.name),
            ));
            reported = true;
        }
    }
    (!reported).then_some(AcyclicCallGraph)
}

/// Whether `start` can reach itself by following direct calls.
fn reaches_self(start: u16, callees: &[&[u16]]) -> bool {
    let mut stack: Vec<u16> = callees
        .get(start as usize)
        .map(|targets| targets.to_vec())
        .unwrap_or_default();
    let mut visited = vec![false; callees.len()];
    while let Some(node) = stack.pop() {
        if node == start {
            return true;
        }
        if (node as usize) >= visited.len() || visited[node as usize] {
            continue;
        }
        visited[node as usize] = true;
        if let Some(targets) = callees.get(node as usize) {
            stack.extend_from_slice(targets);
        }
    }
    false
}

/// Report `check.requires_transaction` for every durable mutation or mutating call an
/// export entry performs outside a `transaction` block.
///
/// A function *requires an ambient transaction* when it performs a durable mutation
/// not enclosed in its own `transaction` block — directly, or by calling a function
/// that itself requires one at a site the block does not cover. That property is a
/// monotone fixpoint over the acyclic call graph. A non-export helper that requires a
/// transaction is legal: it runs inside its caller's region. The requirement is
/// therefore reported only where a caller cannot satisfy it — at an export entry, at
/// the specific unwrapped mutation or call-site span. A test entry receives its
/// ambient transaction from the test harness and is likewise exempt.
/// `_acyclic` is the prerequisite, not an unused argument: the monotone fixpoint
/// below terminates only over an acyclic call graph.
fn reject_missing_transaction(
    lowered: &CompleteLoweredFunctionSet,
    _acyclic: &AcyclicCallGraph,
    diagnostics: &mut DiagnosticCollector,
) -> Option<AmbientTransactionClosure> {
    let lowered = lowered.functions();
    let count = lowered.len();
    let mut by_index: Vec<Option<&LoweredFn>> = vec![None; count];
    for function in lowered {
        if (function.index as usize) < count {
            by_index[function.index as usize] = Some(function);
        }
    }

    // `requires[i]`: function `i` mutates outside its own transaction block. The base
    // case is a direct unwrapped mutation; the inductive case is an unwrapped call to a
    // function that itself requires one. Recursion is already rejected, so the boolean
    // fixpoint over the acyclic graph converges.
    let mut requires: Vec<bool> = by_index
        .iter()
        .map(|entry| entry.is_some_and(|f| !f.unwrapped_mutations.is_empty()))
        .collect();
    loop {
        let mut changed = false;
        for (i, entry) in by_index.iter().enumerate() {
            let Some(function) = entry else { continue };
            if requires[i] {
                continue;
            }
            if function
                .unwrapped_calls
                .iter()
                .any(|(callee, _)| (*callee as usize) < count && requires[*callee as usize])
            {
                requires[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Report at export entries only. Deduplicate by source position so a single write
    // that lowers to several instructions (an upsert's replace and create arms share
    // one span) yields one diagnostic.
    let mut reported = false;
    for function in lowered {
        if !function.is_export {
            continue;
        }
        let mut seen: BTreeSet<(u32, u32)> = BTreeSet::new();
        for span in &function.unwrapped_mutations {
            if seen.insert((span.line, span.column)) {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckRequiresTransaction.as_str(),
                    &function.file,
                    *span,
                    "the durable mutation here has no ambient transaction. A durable write, \
                     replacement, or erase executes only inside a `transaction` block. Wrap it \
                     in a `transaction { … }` block."
                        .to_string(),
                ));
                reported = true;
            }
        }
        for (callee, span) in &function.unwrapped_calls {
            if (*callee as usize) >= count || !requires[*callee as usize] {
                continue;
            }
            if seen.insert((span.line, span.column)) {
                let name = by_index[*callee as usize]
                    .map(|f| f.name.as_str())
                    .unwrap_or("a mutating function");
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckRequiresTransaction.as_str(),
                    &function.file,
                    *span,
                    format!(
                        "calling `{name}` here has no ambient transaction. A durable write, \
                         replacement, or erase executes only inside a `transaction` block. Wrap \
                         the call in a `transaction {{ … }}` block."
                    ),
                ));
                reported = true;
            }
        }
    }
    (!reported).then_some(AmbientTransactionClosure)
}

/// The three-state ownership lattice a mutating export's region walks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TxnState {
    BeforeBegin,
    InTxn,
    AfterCommit,
}

/// The control-flow successors of the instruction at `index`, in tape order. Mirrors
/// the verifier's flow-successor relation over the same opcode set so the check-time
/// lattice walks the identical CFG the image verification does.
fn instr_successors(code: &[Instr], index: usize) -> Vec<usize> {
    match &code[index] {
        Instr::Return | Instr::Unreachable(_) | Instr::Todo(_) => Vec::new(),
        Instr::Jump(target) => vec![*target as usize],
        Instr::JumpIfFalse(target)
        | Instr::BranchPresent(target)
        | Instr::IntAddChecked(target)
        | Instr::IntSubChecked(target)
        | Instr::IntMulChecked(target)
        | Instr::IntNegChecked(target)
        | Instr::IntDivChecked(target)
        | Instr::IntRemChecked(target) => vec![*target as usize, index + 1],
        _ => vec![index + 1],
    }
}

/// Report the transaction-ownership lattice laws at check time, at their source spans.
///
/// The ownership contract has four remaining laws the verifier reconstructs from the
/// image (image.flow) and this pass promotes to source-facing `check.*` diagnostics:
///
/// - the owner lattice — a mutating export owns exactly one `transaction` region,
///   begun once and committed on every path, with no durable operation after the
///   commit and no empty (no-op) region;
/// - a transaction owner is not called by another function;
/// - a `transaction` marker sits only in the export that owns it;
/// - a prefix `try` whose implicit `err` exit would leave an owned region uncommitted
///   is an uncommitted exit (a `return` is a commit site; a `try` is not).
///
/// The pass walks each function's lowered tape — the same instruction sequence the
/// verifier reconstructs from the image — so a program the checker rejects here is
/// exactly one the verifier would reject at image.flow: the checker is never stricter
/// than the boundary. The requires-ambient-transaction pass runs first and already
/// covers a durable mutation outside any region, so this pass need not restate it.
/// `_closure` is the prerequisite, not an unused argument: an unsatisfied
/// requires-ambient-transaction report already stands otherwise, and a single
/// unwrapped mutation would cascade into a second ownership report.
fn reject_transaction_ownership(
    lowered: &CompleteLoweredFunctionSet,
    _closure: &AmbientTransactionClosure,
    diagnostics: &mut DiagnosticCollector,
) {
    let lowered = lowered.functions();
    let count = lowered.len();
    let mut by_index: Vec<Option<&LoweredFn>> = vec![None; count];
    for function in lowered {
        if (function.index as usize) < count {
            by_index[function.index as usize] = Some(function);
        }
    }

    let has_begin: Vec<bool> = by_index
        .iter()
        .map(|entry| entry.is_some_and(|f| f.code.iter().any(|i| matches!(i, Instr::TxnBegin))))
        .collect();
    let has_commit: Vec<bool> = by_index
        .iter()
        .map(|entry| entry.is_some_and(|f| f.code.iter().any(|i| matches!(i, Instr::TxnCommit))))
        .collect();

    // `mutates[f]` / `durable[f]`: `f` or a transitive callee stages a mutation / performs
    // any durable operation. The base case is a direct opcode; the inductive case unions
    // each callee's closure. Recursion is already rejected, so the monotone boolean
    // fixpoint over the acyclic graph converges. These mirror the verifier's mutate and
    // non-empty-atom closures the lattice consumes.
    let mut mutates: Vec<bool> = by_index
        .iter()
        .map(|entry| entry.is_some_and(|f| f.code.iter().any(is_mutation_instr)))
        .collect();
    let mut durable: Vec<bool> = by_index
        .iter()
        .map(|entry| entry.is_some_and(|f| f.code.iter().any(is_durable_place_op)))
        .collect();
    loop {
        let mut changed = false;
        for (i, entry) in by_index.iter().enumerate() {
            let Some(function) = entry else { continue };
            for &callee in &function.callees {
                let c = callee as usize;
                if c >= count {
                    continue;
                }
                if mutates[c] && !mutates[i] {
                    mutates[i] = true;
                    changed = true;
                }
                if durable[c] && !durable[i] {
                    durable[i] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    for function in lowered {
        let i = function.index as usize;
        if i >= count {
            continue;
        }

        // A transaction owner may not be called — except from a test body, which drives
        // an owner as a terminal would, each call its own invocation boundary. Reported at
        // every call site to an owner; the verifier returns on the first, so a function
        // that calls an owner is not examined for its own ownership laws besides this.
        if !function.is_test {
            let mut reported = false;
            for (idx, instr) in function.code.iter().enumerate() {
                let Instr::Call(target) = instr else { continue };
                let t = *target as usize;
                if t >= count || !has_begin[t] {
                    continue;
                }
                let name = by_index[t].map(|f| f.name.as_str()).unwrap_or("an export");
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckTransactionOwnerCalled.as_str(),
                    &function.file,
                    function.code_spans[idx],
                    format!(
                        "calling `{name}` here calls a transaction owner. An export that owns \
                         a `transaction` block is an invocation boundary and may not be called \
                         from another function; only a `test` body may drive it. Inline the \
                         durable work into this export's own `transaction` block, or move it \
                         into a helper — a function with no `transaction` block — that this \
                         export calls inside its region."
                    ),
                ));
                reported = true;
            }
            if reported {
                continue;
            }
        }

        // A `transaction` whose closure performs no durable operation commits nothing and
        // opens no session; refuse it at the block.
        if function.is_export && has_begin[i] && !durable[i] {
            if let Some(span) = first_marker_span(function) {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckTransactionEmpty.as_str(),
                    &function.file,
                    span,
                    "this `transaction` block performs no durable operation, so it commits \
                     nothing and opens no store session. Perform the durable read or write \
                     the transaction is meant to group, or remove the empty block."
                        .to_string(),
                ));
            }
            continue;
        }

        // A mutating (or region-owning) export runs the owner lattice.
        if function.is_export && (mutates[i] || has_begin[i]) {
            if let Some((code, span, message)) = owner_lattice_violation(function, &durable, count)
            {
                diagnostics.push(SourceDiagnostic::at(code, &function.file, span, message));
            }
            continue;
        }

        // Every other function is a helper or a `test` body; neither owns a region, so a
        // `transaction` marker in one is misplaced.
        if has_begin[i] || has_commit[i] {
            if let Some(span) = first_marker_span(function) {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckTransactionMisplaced.as_str(),
                    &function.file,
                    span,
                    "a `transaction` block belongs only in the export that owns it. A helper \
                     runs inside its caller's region and carries no `transaction` block of its \
                     own, and a `test` body drives owning exports rather than owning a region. \
                     Move the `transaction` block to the owning export."
                        .to_string(),
                ));
            }
            continue;
        }
    }
}

/// The source span of a function's first `transaction` marker (its begin, else its
/// commit), for reporting a region-level ownership violation.
fn first_marker_span(function: &LoweredFn) -> Option<SourceSpan> {
    function
        .code
        .iter()
        .position(|instr| matches!(instr, Instr::TxnBegin | Instr::TxnCommit))
        .map(|idx| function.code_spans[idx])
}

/// The first owner-lattice violation on `function`'s tape, or `None` when the region is
/// well formed. Entry states are computed by a CFG fixpoint mirroring the verifier's
/// lattice, then the tape is scanned in index order so the earliest offending construct
/// is reported deterministically. A merge whose incoming states disagree is unreachable
/// on lowered output; first-writer-wins keeps the walk deterministic and can only make
/// the checker miss a case the verifier still catches, never reject a legal one.
fn owner_lattice_violation(
    function: &LoweredFn,
    durable: &[bool],
    count: usize,
) -> Option<(&'static str, SourceSpan, String)> {
    let code = &function.code;
    if code.is_empty() {
        return None;
    }
    let mut entry: Vec<Option<TxnState>> = vec![None; code.len()];
    entry[0] = Some(TxnState::BeforeBegin);
    let mut worklist = vec![0usize];
    while let Some(index) = worklist.pop() {
        // The worklist only enqueues instructions whose entry state is set.
        let Some(state) = entry[index] else { continue };
        let next = match &code[index] {
            Instr::TxnBegin => TxnState::InTxn,
            Instr::TxnCommit => TxnState::AfterCommit,
            _ => state,
        };
        for successor in instr_successors(code, index) {
            if successor < code.len() && entry[successor].is_none() {
                entry[successor] = Some(next);
                worklist.push(successor);
            }
        }
    }

    for (idx, instr) in code.iter().enumerate() {
        let Some(state) = entry[idx] else { continue };
        match instr {
            Instr::TxnBegin if state != TxnState::BeforeBegin => {
                return Some((
                    Code::CheckTransactionReopened.as_str(),
                    function.code_spans[idx],
                    "this reopens a `transaction` region the export already owns. A mutating \
                     export begins its region exactly once and commits it on every path; \
                     combine the durable work into a single `transaction` block."
                        .to_string(),
                ));
            }
            Instr::Return if state != TxnState::AfterCommit => {
                return Some((
                    Code::CheckTransactionUncommitted.as_str(),
                    function.code_spans[idx],
                    "this path leaves the `transaction` region without committing it. A \
                     region's commit sites are its exits — each `return` inside the block and \
                     the closing brace — so an exit that bypasses them leaves staged writes \
                     uncommitted. Spell a deliberate failure as an in-region `return` (a \
                     commit site), and place a guard that must not commit before the block. A \
                     prefix `try` or a `require` guard is ordinary control flow, not a commit, \
                     so its implicit `err` exit may not cross a region its own function owns."
                        .to_string(),
                ));
            }
            _ => {
                let durable_here = is_durable_place_op(instr)
                    || matches!(instr, Instr::Call(t) if (*t as usize) < count && durable[*t as usize]);
                if durable_here && state == TxnState::AfterCommit {
                    return Some((
                        Code::CheckDurableAfterCommit.as_str(),
                        function.code_spans[idx],
                        "this durable operation runs after the `transaction` region commits. \
                         The commit consumes the region's store session, so no durable read or \
                         write may follow it. Move the operation inside the `transaction` \
                         block, or capture the value into a local before the block closes and \
                         return the local."
                            .to_string(),
                    ));
                }
            }
        }
    }
    None
}

/// Report `check.test_driver_mix` for every `test` body that both performs a durable
/// operation directly and drives a transaction-owning export. The two invocation
/// models — one harness session for direct operations, one session per driven export
/// call — cannot share a body: the driven export's commit would consume the harness
/// session the direct operation needs. Only a directly-owned transaction counts as a
/// driven owner; because a transaction owner is never reached through a helper, the
/// test body's direct call edges carry the whole relation.
/// `_acyclic` is the prerequisite, not an unused argument: the reachability walk
/// below terminates only over an acyclic call graph.
fn reject_mixed_test_bodies(
    lowered: &CompleteLoweredFunctionSet,
    _acyclic: &AcyclicCallGraph,
    diagnostics: &mut DiagnosticCollector,
) {
    let lowered = lowered.functions();
    let count = lowered.len();
    let mut owns_transaction = vec![false; count];
    for function in lowered {
        if (function.index as usize) < count {
            owns_transaction[function.index as usize] = function.owns_transaction;
        }
    }
    for test in lowered.iter().filter(|f| f.is_test) {
        if !test.has_direct_durable_op {
            continue;
        }
        let drives_owner = test
            .callees
            .iter()
            .any(|callee| (*callee as usize) < count && owns_transaction[*callee as usize]);
        if drives_owner {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckTestDriverMix.as_str(),
                &test.file,
                test.span,
                "this test body performs a durable operation directly and also drives an \
                 export that owns a transaction. A test either works durable data directly, \
                 in the harness session, or drives exports, where each call is its own \
                 invocation boundary; the two cannot share one body. Split them into \
                 separate tests, or reach the durable data through the exports it drives."
                    .to_string(),
            ));
        }
    }
}

/// Report `check.resource_limit` for every structural declaration bound a source
/// construct crosses whose count is an exact property of the parse tree: a record
/// type (a `resource` or `struct`) wider than [`MAX_RECORD_FIELDS`] top-level fields,
/// or a function with more than [`MAX_PARAMS`] parameters. The refusal lands at the
/// offending construct's span before the image structure is built. Bounds knowable
/// only after type resolution (durable value depth, struct-leaf width, key tuples,
/// index projections) or lowering (locals, code bytes) are owned elsewhere.
fn check_structural_resource_bounds(parsed: &[Module], diagnostics: &mut DiagnosticCollector) {
    for module in parsed {
        for declaration in &module.ast.declarations {
            match declaration {
                Declaration::Resource(resource) => {
                    check_record_field_width(
                        &module.file,
                        resource.name_span,
                        &resource.members,
                        diagnostics,
                    );
                }
                Declaration::Struct(item) => {
                    check_record_field_width(
                        &module.file,
                        item.name_span,
                        &item.members,
                        diagnostics,
                    );
                }
                Declaration::Function(function)
                    if function.params.len() > marrow_image::bounds::MAX_PARAMS =>
                {
                    diagnostics.push(SourceDiagnostic::at(
                        Code::CheckResourceLimit.as_str(),
                        &module.file,
                        function.span,
                        format!(
                            "a function declares {} parameters; the fixed limit is {}",
                            function.params.len(),
                            marrow_image::bounds::MAX_PARAMS
                        ),
                    ));
                }
                _ => {}
            }
        }
    }
}

/// Report a record type whose top-level `name: Type` field members exceed the image
/// record-field width. Group and branch members are not top-level record fields, so
/// they are not counted here.
fn check_record_field_width(
    file: &FileIdentity,
    span: SourceSpan,
    members: &[ResourceMember],
    diagnostics: &mut DiagnosticCollector,
) {
    let fields = members
        .iter()
        .filter(|member| matches!(member, ResourceMember::Field(_)))
        .count();
    if fields > marrow_image::bounds::MAX_RECORD_FIELDS {
        diagnostics.push(SourceDiagnostic::at(
            Code::CheckResourceLimit.as_str(),
            file,
            span,
            format!(
                "a record type declares {fields} top-level fields; the fixed limit is {}",
                marrow_image::bounds::MAX_RECORD_FIELDS
            ),
        ));
    }
}

/// Whether an export declaration path is valid to mint an [`ExportId`] over:
/// every dotted module segment and the item must be non-empty ASCII identifiers
/// (a letter or `_`, then letters, digits, or `_`; never a `.`). This is what
/// keeps the id payload's dotted `module` join injective over segments, so it is
/// checked here — immediately before minting — rather than assumed from capture.
fn valid_export_path(module: &str, item: &str) -> bool {
    module.split('.').all(is_ascii_identifier) && is_ascii_identifier(item)
}

/// Whether `text` is a non-empty ASCII identifier.
fn is_ascii_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::valid_export_path;
    use super::{
        AcceptedQueuedTemplateProofs, AcyclicCallGraph, AmbientTransactionClosure, Analyzed,
        Artifacts, BoundedDiagnostics, Built, CompileFailure, CompileStage,
        CompleteDeclaredFunctionBodies, CompleteDeclaredTestBodies, CompleteFunctionRegistry,
        CompleteLoweredFunctionSet, CompleteTypeRegistry, DeclarationExit, Driven, InvariantCause,
        SemanticOutcome, SignaturesComplete, analyze_outcome,
    };
    use crate::compile::Declaration;
    use crate::diag::{DiagnosticCollector, MAX_DIAGNOSTIC_COUNT, SourceDiagnostic};
    use crate::lower::FunctionRegistry;
    use crate::types::{
        CollectionKind, GenericCacheInvariant, GenericInvariant, ProofCloneError, Reserved,
        TypeInstKind,
    };
    use marrow_codes::Code;
    use marrow_syntax::SourceSpan;
    use std::collections::BTreeMap;

    /// The minting guard rejects every input class whose dotted join would break
    /// the ExportId payload's injectivity, even though the current capture path
    /// cannot produce them.
    #[test]
    fn export_path_validation_guards_the_id_payload() {
        // Ordinary declaration paths mint.
        assert!(valid_export_path("main", "run"));
        assert!(valid_export_path("shelf.books", "add"));
        assert!(valid_export_path("a_b", "_x1"));

        // Empty or dotted components would let two distinct declaration paths
        // collide on one payload.
        assert!(!valid_export_path("", "run"));
        assert!(!valid_export_path("a", ""));
        assert!(!valid_export_path("a..b", "run"));
        assert!(!valid_export_path("a.", "run"));
        assert!(!valid_export_path(".a", "run"));
        assert!(!valid_export_path("a", "b.c"));

        // Non-ASCII and non-identifier characters are outside the frozen payload
        // domain.
        assert!(!valid_export_path("caf\u{e9}", "run"));
        assert!(!valid_export_path("a", "r\u{e9}sum\u{e9}"));
        assert!(!valid_export_path("a-b", "run"));
        assert!(!valid_export_path("1a", "run"));
        assert!(!valid_export_path("a", "1run"));
        assert!(!valid_export_path("a b", "run"));
    }

    fn diagnostic(code: &'static str, line: u32) -> SourceDiagnostic {
        SourceDiagnostic::at(
            code,
            crate::test_main_file_identity(),
            SourceSpan {
                line,
                column: 7,
                ..SourceSpan::default()
            },
            "retained source diagnostic".to_string(),
        )
    }

    fn proof_clone_cause() -> InvariantCause {
        InvariantCause::Generic(GenericInvariant::ProofClone(
            ProofCloneError::UnstableFillState,
        ))
    }

    fn stage_label(stage: CompileStage) -> &'static str {
        match stage {
            CompileStage::TypeInstantiation => "type instantiation",
            CompileStage::TemplateProof => "template proof",
            CompileStage::BodyLowering => "body lowering",
            CompileStage::PostLoweringValidation => "post-lowering validation",
        }
    }

    fn private_generic_cause_label(cause: GenericInvariant) -> &'static str {
        match cause {
            GenericInvariant::ProofClone(cause) => match cause {
                ProofCloneError::UnstableFillState => "unstable proof clone",
                ProofCloneError::LimitOwnerNotOpen => "closed limit owner",
            },
            GenericInvariant::CacheState(cause) => match cause {
                GenericCacheInvariant::ActiveBatchMissing => "active batch missing",
                GenericCacheInvariant::ActiveBatchRange => "active batch range",
                GenericCacheInvariant::ActiveRowCardinality => "active row cardinality",
                GenericCacheInvariant::ActiveRowKeyMismatch => "active row key mismatch",
                GenericCacheInvariant::ActiveFillStackNotEmpty => "active stack not empty",
                GenericCacheInvariant::FailureIndexOutOfRange => "failure index range",
                GenericCacheInvariant::DependentIndexOutOfRange => "dependent index range",
                GenericCacheInvariant::StableRowInActiveBatch => "stable active row",
                GenericCacheInvariant::IncompleteRowWithoutRefusal => "incomplete row",
                GenericCacheInvariant::FillingReuseOutsideBatch => "orphan Filling reuse",
                GenericCacheInvariant::SettledRowMissing => "settled row missing",
                GenericCacheInvariant::SettledRowStillFilling => "settled row Filling",
                GenericCacheInvariant::FillStackMismatch => "fill stack mismatch",
                GenericCacheInvariant::MintIndexDrift => "mint index drift",
                GenericCacheInvariant::MintKeyAlreadyPresent => "mint key already present",
            },
            GenericInvariant::ReservedTemplateMissing(reserved) => match reserved {
                Reserved::Option => "Option template missing",
                Reserved::Result => "Result template missing",
            },
            GenericInvariant::TypeTemplateMissing(_) => "type template missing",
            GenericInvariant::TypeArgumentCountMismatch { .. } => "type argument count mismatch",
            GenericInvariant::TemplateKindMismatch {
                expected, actual, ..
            } => match (expected, actual) {
                (TypeInstKind::Struct, TypeInstKind::Struct) => "struct is struct",
                (TypeInstKind::Struct, TypeInstKind::Enum) => "enum where struct expected",
                (TypeInstKind::Enum, TypeInstKind::Struct) => "struct where enum expected",
                (TypeInstKind::Enum, TypeInstKind::Enum) => "enum is enum",
            },
            GenericInvariant::TypeBodyKindMismatch { body, .. } => match body {
                TypeInstKind::Struct => "Ready struct body mismatch",
                TypeInstKind::Enum => "Ready enum body mismatch",
            },
            GenericInvariant::ReadyBodyShapeMismatch(_) => "Ready body shape mismatch",
            GenericInvariant::ReadyBodyMissing(_) => "Ready body missing",
            GenericInvariant::ReadyEnumVariantMissing { .. } => "Ready enum variant missing",
            GenericInvariant::TypeIdentityCollision(_) => "type identity collision",
            GenericInvariant::TypeInstantiationKeyCollision { .. } => {
                "type instantiation key collision"
            }
            GenericInvariant::TypeArgumentOrderViolation { .. } => "type argument order violation",
            GenericInvariant::TypeArgumentTargetMissing(_) => "type argument target missing",
            GenericInvariant::TypeArgumentParameter(_) => "concrete type argument is a parameter",
            GenericInvariant::CollectionIndexMismatch { kind, .. } => match kind {
                CollectionKind::List => "List owner mismatch",
                CollectionKind::Map => "Map owner mismatch",
            },
            GenericInvariant::DurableResourceMissing(_) => "durable resource missing",
            GenericInvariant::DeclarationIndexDrift => "declaration index drift",
            GenericInvariant::DurableConstructionRefused => "durable construction refused",
        }
    }

    #[test]
    fn private_generic_cause_classification_has_no_wildcard() {
        assert_eq!(
            private_generic_cause_label(GenericInvariant::ProofClone(
                ProofCloneError::UnstableFillState
            )),
            "unstable proof clone"
        );
    }

    /// A driven pass whose stage terminals are exactly `parse`, `structural`,
    /// and `semantic`, with no orthogonal analysis facts. The projection under
    /// test reads only these.
    fn driven(
        parse: BoundedDiagnostics,
        structural: BoundedDiagnostics,
        semantic: SemanticOutcome,
    ) -> Driven {
        Driven {
            parse,
            structural,
            semantic,
            facts: crate::analysis::BoundedAnalysisFacts::Complete(
                crate::analysis::RetainedFacts::default(),
            ),
            symbol_bounded_files: Vec::new(),
        }
    }

    fn empty_terminal() -> BoundedDiagnostics {
        DiagnosticCollector::new().finish()
    }

    fn finished(rows: Vec<SourceDiagnostic>) -> BoundedDiagnostics {
        let mut collector = DiagnosticCollector::new();
        for row in rows {
            collector.push(row);
        }
        collector.finish()
    }

    /// A semantic invariant reaches the public boundary opaque: no partial
    /// image, a private cause, and the fixed rendering.
    #[test]
    fn a_semantic_invariant_is_opaque_at_the_public_boundary() {
        let outcome: Result<Built, CompileFailure> = driven(
            empty_terminal(),
            empty_terminal(),
            SemanticOutcome::Invariant(proof_clone_cause()),
        )
        .into_built();
        let Err(failure) = outcome else {
            panic!("an invariant must not produce a partial image")
        };

        assert_eq!(failure.to_string(), "compiler invariant failure");
        assert!(std::error::Error::source(&failure).is_some());
        let CompileFailure::Invariant(invariant) = failure else {
            panic!("the private invariant must stay an invariant")
        };
        assert!(matches!(
            invariant.0,
            InvariantCause::Generic(GenericInvariant::ProofClone(
                ProofCloneError::UnstableFillState
            ))
        ));
        assert_eq!(format!("{invariant:?}"), "CompileInvariant");
        assert_eq!(invariant.to_string(), "compiler invariant failure");
        assert!(std::error::Error::source(&invariant).is_none());
    }

    /// An earlier stage's complete diagnostics dominate a later semantic
    /// invariant or resource limit: the projection reports the first
    /// logically non-empty stage and never mixes stages.
    #[test]
    fn an_earlier_stage_dominates_a_later_semantic_failure() {
        let parse_row = diagnostic(Code::CheckType.as_str(), 3);
        let failure = driven(
            finished(vec![parse_row.clone()]),
            empty_terminal(),
            SemanticOutcome::Invariant(proof_clone_cause()),
        )
        .into_built()
        .map(|_| ())
        .expect_err("a parse-stage row fails compilation");
        let CompileFailure::Diagnostics(diagnostics) = failure else {
            panic!("the parse stage's own rows are the failure")
        };
        assert_eq!(diagnostics.as_slice(), std::slice::from_ref(&parse_row));
    }

    /// Diagnostics preserve their collector order and allocation behind a
    /// statically nonempty owner. Every borrowed and owned iteration surface
    /// observes that order; recovering the vector recovers the collector's
    /// allocation without a copy.
    #[test]
    fn diagnostic_failure_preserves_order_allocation_and_iteration_views() {
        let expected = vec![
            diagnostic(Code::CheckType.as_str(), 4),
            diagnostic(Code::CheckType.as_str(), 9),
        ];
        let terminal = finished(expected.clone());
        let original_ptr = match &terminal {
            BoundedDiagnostics::Complete { rows, .. } => rows.as_ptr(),
            BoundedDiagnostics::Limited { .. } => panic!("two rows stay complete"),
        };
        let failure = driven(
            terminal,
            empty_terminal(),
            SemanticOutcome::Invariant(proof_clone_cause()),
        )
        .into_built()
        .map(|_| ())
        .expect_err("a nonempty parse stage fails compilation");
        assert_eq!(
            failure.to_string(),
            "compilation failed with source diagnostics"
        );
        assert!(std::error::Error::source(&failure).is_none());
        let CompileFailure::Diagnostics(diagnostics) = failure else {
            panic!("a nonempty source failure must remain diagnostics")
        };
        assert_eq!(diagnostics.as_slice(), expected.as_slice());
        let as_ref: &[SourceDiagnostic] = diagnostics.as_ref();
        assert_eq!(as_ref, expected.as_slice());
        assert_eq!(diagnostics.iter().cloned().collect::<Vec<_>>(), expected);
        assert_eq!(
            (&diagnostics).into_iter().cloned().collect::<Vec<_>>(),
            expected
        );
        let recovered = diagnostics.into_vec();
        assert_eq!(
            recovered.as_ptr(),
            original_ptr,
            "the collector's allocation is recovered without a copy"
        );
        assert_eq!(recovered, expected);
    }

    /// A completeness artifact is minted for a declaration set only when every
    /// declaration in it took the index reserved for it. A stop on the shared
    /// instantiation limit leaves the loop's unvisited suffix unlowered, so it mints
    /// nothing — the artifact's documented claim would otherwise be false for every
    /// declaration after the stop, leaving the truncated set's honesty resting entirely
    /// on the caller's separate limit return.
    #[test]
    fn only_an_exhausted_declaration_set_is_complete() {
        assert!(DeclarationExit::Exhausted.complete());
        assert!(!DeclarationExit::Refused.complete());
        assert!(!DeclarationExit::StoppedOnInstantiationLimit.complete());
    }

    /// Availability and the value are minted by one owner, and the availability
    /// proof is zero-sized.
    ///
    /// `Artifacts.functions` holds the proof token, never the table, so what `encode`
    /// consumes cannot be forged outside [`CompleteFunctionRegistry`] — the property
    /// the artifact set protects, that a resolved signature table nothing vouches for
    /// is unrepresentable at encode, is preserved verbatim while the table itself
    /// stays available to every phase that resolves call sites through it.
    ///
    /// Unforgeability is enforced by the token's private field rather than asserted
    /// here: `SignaturesComplete(())` has no literal form outside the `signatures`
    /// module, so writing one in this file does not compile. Zero size is what this
    /// test still owns — the proof must cost the artifact set nothing.
    #[test]
    fn the_signature_completeness_proof_is_zero_sized() {
        assert_eq!(std::mem::size_of::<SignaturesComplete>(), 0);
    }

    /// A refused signature standing behind an accepted duplicate of its name still
    /// withholds the completeness proof.
    ///
    /// The proof reads the ledger's refused set. A set that answered only for the
    /// keys a lookup resolves to a refusal would skip this one, and `Artifacts`
    /// would hold a proof minted over a table the compiler refused a declaration
    /// in — the amendment's sentence would be false for exactly this source shape.
    /// Resolve `functions` through the production registry owners, over a project
    /// that declares nothing else.
    ///
    /// Built through the real builders rather than hand-assembled: a completeness
    /// proof is only meaningful over a table the production build produced.
    fn signature_registry(functions: &[crate::lower::DeclaredFn<'_>]) -> FunctionRegistry {
        let budget = crate::decl::DeclarationBudget::default();
        let mut draft = marrow_image::ImageDraft::new();
        let mut diagnostics = DiagnosticCollector::new();
        let records = crate::types::TypeRegistry::build(
            &mut draft,
            &[],
            &[],
            &[],
            &[],
            &[],
            &mut diagnostics,
            budget.clone(),
        )
        .expect("the test registry stays within the ledger budget");
        let durable = crate::durable::DurableRegistry::build(
            &mut draft,
            &records,
            &[],
            &[],
            None,
            &mut diagnostics,
            budget.clone(),
        )
        .expect("an empty project builds an empty durable registry");
        crate::lower::FunctionRegistry::build(
            &records,
            &mut draft,
            &durable,
            functions,
            crate::lower::ModuleLedger::new(
                crate::decl::DeclarationNamespace::Module,
                budget.clone(),
            ),
            BTreeMap::new(),
            &mut diagnostics,
            budget,
        )
        .expect("the signature ledger stays within its budget")
    }

    #[test]
    fn a_refusal_behind_an_accepted_duplicate_withholds_the_completeness_proof() {
        let (identity, _) =
            marrow_project::FileIdentity::validate("src/main.mw").expect("a valid source path");
        let parsed = marrow_syntax::parse_source(
            "module main\n\nfn dup(a: int): int {\n    return a\n}\n\n\
             fn dup(a: Nope): int {\n    return 1\n}\n",
        );
        let functions: Vec<crate::lower::DeclaredFn<'_>> = parsed
            .file
            .declarations
            .iter()
            .filter_map(|decl| match decl {
                Declaration::Function(function) => Some(crate::lower::DeclaredFn {
                    file: identity.clone(),
                    at: crate::analysis::FileRef::admitted(0),
                    module: "main".to_string(),
                    decl: function,
                }),
                _ => None,
            })
            .collect();
        assert_eq!(functions.len(), 2, "both declarations are parsed");
        let signatures = signature_registry(&functions);

        assert!(
            !signatures.every_signature_accepted(),
            "the second `dup` was refused for its parameter type",
        );
        assert!(
            CompleteFunctionRegistry(signatures).complete().is_none(),
            "a refused signature withholds the completeness proof",
        );
    }

    /// The fence's positive direction. Production cannot reach it — an unavailable
    /// artifact always follows a refusal that reported, so the terminal is non-empty and
    /// the diagnostic arm wins — which is exactly why the rule needs a direct test:
    /// without one, deleting the availability arm outright leaves the whole workspace
    /// green. Every artifact is withheld in turn, so each conjunct of the availability
    /// destructure is load-bearing, and the all-available base is checked to be checked.
    #[test]
    fn an_empty_terminal_with_a_withheld_artifact_is_an_invariant() {
        let available = || Artifacts {
            types: Some(CompleteTypeRegistry),
            functions: CompleteFunctionRegistry(signature_registry(&[])).complete(),
            template_proofs: Some(AcceptedQueuedTemplateProofs),
            function_bodies: Some(CompleteDeclaredFunctionBodies),
            test_bodies: Some(CompleteDeclaredTestBodies),
            lowered: Some(CompleteLoweredFunctionSet(Vec::new())),
            call_graph: Some(AcyclicCallGraph),
            transactions: Some(AmbientTransactionClosure),
        };
        assert!(
            available().refusal(empty_terminal()).is_none(),
            "an empty terminal with every artifact available is a checked program"
        );

        /// One artifact withheld from an otherwise complete set, by name.
        type Withhold = (&'static str, fn(&mut Artifacts));
        let withhold: [Withhold; 8] = [
            ("types", |a| a.types = None),
            ("functions", |a| a.functions = None),
            ("template_proofs", |a| a.template_proofs = None),
            ("function_bodies", |a| a.function_bodies = None),
            ("test_bodies", |a| a.test_bodies = None),
            ("lowered", |a| a.lowered = None),
            ("call_graph", |a| a.call_graph = None),
            ("transactions", |a| a.transactions = None),
        ];
        for (name, withhold) in withhold {
            let mut artifacts = available();
            withhold(&mut artifacts);
            let refusal = artifacts
                .refusal(empty_terminal())
                .unwrap_or_else(|| panic!("withholding {name} must refuse the program"));
            assert!(
                matches!(
                    refusal,
                    SemanticOutcome::Invariant(InvariantCause::UnavailableWithoutReport)
                ),
                "withholding {name} with an empty terminal is the unavailable-without-report \
                 invariant, not a checked program or a diagnostic outcome"
            );
        }
    }

    /// A complete-but-empty semantic diagnostics terminal is a private
    /// invariant carrying the exact stage that attempted to cross the
    /// boundary; a logically empty parse or structural terminal instead
    /// passes over. The matcher intentionally has no wildcard, so adding a
    /// stage requires updating this contract.
    #[test]
    fn an_empty_semantic_terminal_is_an_exact_invariant_at_every_stage() {
        for stage in [
            CompileStage::TypeInstantiation,
            CompileStage::TemplateProof,
            CompileStage::BodyLowering,
            CompileStage::PostLoweringValidation,
        ] {
            let empty = driven(
                empty_terminal(),
                empty_terminal(),
                SemanticOutcome::Diagnostics(empty_terminal(), stage),
            )
            .into_built()
            .map(|_| ())
            .expect_err("an empty semantic terminal must not build");
            let CompileFailure::Invariant(invariant) = empty else {
                panic!("an empty diagnostic terminal must become a compiler invariant")
            };
            let InvariantCause::EmptyDiagnostics(actual) = invariant.0 else {
                panic!("the empty boundary keeps its private stage")
            };
            assert_eq!(stage_label(actual), stage_label(stage));
            assert_eq!(actual, stage);
        }
    }

    /// The analysis union reads the same terminals the production projection
    /// does, row by row: with empty prechecks a semantic invariant or
    /// resource limit passes through; with prechecks present it is
    /// suppressed for the precheck union; a semantic empty terminal is a
    /// truthful empty snapshot (where production reports the empty-boundary
    /// invariant above); and the union of stages may cross a ceiling no
    /// single stage crossed.
    #[test]
    fn the_analysis_union_follows_the_stage_table() {
        let row = |line| diagnostic(Code::CheckType.as_str(), line);

        // Empty prechecks pass the semantic failure through.
        assert!(matches!(
            analyze_outcome(
                empty_terminal(),
                empty_terminal(),
                SemanticOutcome::Invariant(proof_clone_cause()),
            ),
            Analyzed::Invariant(_)
        ));

        // A precheck row suppresses the semantic invariant.
        let Analyzed::Diagnostics(rows) = analyze_outcome(
            finished(vec![row(3)]),
            empty_terminal(),
            SemanticOutcome::Invariant(proof_clone_cause()),
        ) else {
            panic!("prechecks suppress the semantic failure")
        };
        assert_eq!(rows, vec![row(3)]);

        // A semantic empty terminal with empty prechecks is an empty snapshot.
        let Analyzed::Diagnostics(rows) = analyze_outcome(
            empty_terminal(),
            empty_terminal(),
            SemanticOutcome::Diagnostics(empty_terminal(), CompileStage::BodyLowering),
        ) else {
            panic!("a clean union is a producible snapshot")
        };
        assert!(rows.is_empty());

        // The ordered union: parse, then structural, then semantic rows.
        let Analyzed::Diagnostics(rows) = analyze_outcome(
            finished(vec![row(1)]),
            finished(vec![row(2)]),
            SemanticOutcome::Diagnostics(finished(vec![row(3)]), CompileStage::BodyLowering),
        ) else {
            panic!("a bounded union is a producible snapshot")
        };
        assert_eq!(rows, vec![row(1), row(2), row(3)]);

        // Analysis alone strengthens an OwnedBytes limit to Count across
        // stages: a byte-limited parse terminal plus enough semantic rows to
        // cross the count ceiling resolves as the count limit.
        let byte_limited = BoundedDiagnostics::Limited {
            count: MAX_DIAGNOSTIC_COUNT - 5,
            owned_bytes: crate::diag::MAX_DIAGNOSTIC_BYTES + 1,
            limit: crate::diag::CompileDiagnosticLimit::OwnedBytes {
                limit: crate::diag::MAX_DIAGNOSTIC_BYTES,
            },
        };
        let semantic_rows: Vec<SourceDiagnostic> = (0..10).map(|line| row(line + 1)).collect();
        let Analyzed::ResourceLimit(limit) = analyze_outcome(
            byte_limited,
            empty_terminal(),
            SemanticOutcome::Diagnostics(finished(semantic_rows), CompileStage::BodyLowering),
        ) else {
            panic!("a limited union is the displacing resource limit")
        };
        assert_eq!(limit.kind(), super::ResourceLimitKind::DiagnosticCount);
    }

    #[test]
    fn public_invariant_is_worker_transferable_without_exposing_its_cause() {
        fn assert_worker_type<T: Send + Sync + 'static>() {}

        assert_worker_type::<super::CompileInvariant>();
    }

    /// The frozen kind-detail surface: each aggregate bound names itself with a stable
    /// identifier the CLI resource-limit record carries verbatim. A drift here is a
    /// deliberate change to that published surface.
    #[test]
    fn resource_limit_kind_detail_is_frozen() {
        use super::ResourceLimitKind::*;
        for (kind, detail) in [
            (Strings, "Strings"),
            (Consts, "Consts"),
            (Types, "Types"),
            (Enums, "Enums"),
            (Collections, "Collections"),
            (Roots, "Roots"),
            (Sites, "Sites"),
            (Functions, "Functions"),
            (Exports, "Exports"),
            (TestEntries, "TestEntries"),
            (ImageBytes, "ImageBytes"),
            (StringBytes, "StringBytes"),
            (CodeBytes, "CodeBytes"),
            (DiagnosticCount, "DiagnosticCount"),
            (DiagnosticBytes, "DiagnosticBytes"),
            (ProjectFiles, "ProjectFiles"),
            (ProjectFileBytes, "ProjectFileBytes"),
            (ProjectSourceBytes, "ProjectSourceBytes"),
            (DeclarationLedgerBytes, "DeclarationLedgerBytes"),
        ] {
            assert_eq!(kind.detail(), detail);
        }
    }

    #[test]
    fn a_limited_stage_terminal_is_the_displacing_resource_limit() {
        let mut collector = DiagnosticCollector::new();
        for line in 0..=MAX_DIAGNOSTIC_COUNT as u32 {
            collector.push(diagnostic(Code::CheckType.as_str(), line + 1));
        }
        let failure = driven(
            collector.finish(),
            empty_terminal(),
            SemanticOutcome::Invariant(proof_clone_cause()),
        )
        .into_built()
        .map(|_| ())
        .expect_err("an over-ceiling stage must not build");
        let CompileFailure::ResourceLimit(limit) = failure else {
            panic!("an overflowing diagnostic collection is discarded for a resource limit")
        };
        assert_eq!(limit.kind(), super::ResourceLimitKind::DiagnosticCount);
        assert_eq!(limit.limit(), MAX_DIAGNOSTIC_COUNT as u64);
    }

    #[test]
    fn public_resource_limit_is_worker_transferable() {
        fn assert_worker_type<T: Send + Sync + 'static>() {}

        assert_worker_type::<super::CompileResourceLimit>();
    }

    /// The image-build classifier routes an aggregate whole-program bound to the
    /// resource-limit arm and a producer-state contradiction to an opaque invariant,
    /// never to a source diagnostic with a fabricated location.
    #[test]
    fn image_build_errors_classify_without_a_fabricated_location() {
        let aggregate = super::image_build_outcome(marrow_image::ImageBuildError::TooManyFunctions);
        let super::ImagePolicyOutcome::ResourceLimit(limit) = aggregate else {
            panic!("an aggregate count is a resource limit")
        };
        assert_eq!(limit.kind(), super::ResourceLimitKind::Functions);

        let prechecked = super::image_build_outcome(marrow_image::ImageBuildError::TooManyLocals);
        assert!(
            matches!(prechecked, super::ImagePolicyOutcome::Invariant(_)),
            "a compiler draft past the source-prechecked local bound is an opaque invariant"
        );

        let contradiction =
            super::image_build_outcome(marrow_image::ImageBuildError::InvalidReference("x"));
        assert!(
            matches!(contradiction, super::ImagePolicyOutcome::Invariant(_)),
            "a producer-state contradiction is an opaque invariant, not a diagnostic"
        );
    }

    #[test]
    fn a_hostile_image_draft_retains_the_direct_too_many_locals_error() {
        let mut draft = marrow_image::ImageDraft::new();
        let name = draft.intern_string("hostile");
        let source = draft.intern_string("src/main.mw");
        let Ok(local_count) = u16::try_from(marrow_image::bounds::MAX_LOCALS + 1) else {
            panic!("the current image local bound has a representable hostile successor")
        };
        draft
            .add_function(marrow_image::FunctionDef {
                name,
                source,
                params: Vec::new(),
                ret: marrow_image::ImageType::Unit,
                local_count,
                code: vec![marrow_image::Instr::Return],
                spans: vec![marrow_image::SpanEntry {
                    instr_index: 0,
                    line: 1,
                    column: 1,
                }],
            })
            .expect("a storeless body names no operation site");

        assert!(matches!(
            draft.encode(),
            Err(marrow_image::ImageBuildError::TooManyLocals)
        ));
    }
}

/// The driver's stage-tagged accumulator is one traversal projected two ways: the
/// production compile takes the first non-empty stage (parse, then structural, then
/// semantic), byte-identical to the historical staged early-return; the editor
/// analysis snapshot consumes every stage. This gate proves the projection is faithful
/// over a corpus of clean projects and one intentionally-failing project per stage
/// stop, and that the traversal is dependency-resilient — a syntax error in one
/// component does not suppress the analysis of an independent valid component.
#[cfg(test)]
mod driver_agreement {
    use super::*;
    use marrow_project::{CaptureLimits, CapturedFile, Manifest};

    fn project(files: &[(&str, &str)]) -> ProjectInput {
        let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
        let captured = files
            .iter()
            .map(|(path, source)| CapturedFile::new(path.to_string(), source.as_bytes().to_vec()))
            .collect();
        marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
            .expect("capture project")
    }

    /// The rows a logically non-empty stage terminal projects (empty for a
    /// limited terminal, mirroring `compiled`'s resource arm), or `None` for
    /// a stage the projection passes over.
    fn stage_rows(stage: &BoundedDiagnostics) -> Option<Vec<SourceDiagnostic>> {
        match stage {
            BoundedDiagnostics::Complete { rows, .. } => (!rows.is_empty()).then(|| rows.clone()),
            BoundedDiagnostics::Limited { .. } => Some(Vec::new()),
        }
    }

    /// The diagnostics the first-non-empty-stage projection of a driven pass yields,
    /// or `None` for a clean pass; a resource limit or invariant carries no
    /// diagnostics.
    fn projected(driven: &Driven) -> Option<Vec<SourceDiagnostic>> {
        if let Some(rows) = stage_rows(&driven.parse) {
            return Some(rows);
        }
        if let Some(rows) = stage_rows(&driven.structural) {
            return Some(rows);
        }
        match &driven.semantic {
            SemanticOutcome::Checked(_) => None,
            SemanticOutcome::Diagnostics(diagnostics, _) => match diagnostics {
                BoundedDiagnostics::Complete { rows, .. } => Some(rows.clone()),
                BoundedDiagnostics::Limited { .. } => Some(Vec::new()),
            },
            SemanticOutcome::Invariant(..) | SemanticOutcome::ResourceLimit(..) => Some(Vec::new()),
        }
    }

    /// The pinned invalid-UTF-8 facts flow through the production drive: the
    /// typed row retains the exact `Utf8Error` numbers — a truncated multi-byte
    /// sequence at end of input reports `error_len: None`, an invalid byte
    /// reports its sequence length — while broken-file status is recorded
    /// independently of the retained diagnostics.
    #[test]
    fn drive_retains_the_exact_invalid_utf8_facts() {
        let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
        let captured = vec![
            CapturedFile::new("src/mid.mw".to_string(), b"ok\xFFrest".to_vec()),
            CapturedFile::new("src/tail.mw".to_string(), b"ok \xF0\x9F".to_vec()),
        ];
        let input = marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
            .expect("capture project");
        let driven = drive(&input, TestMode::Include).expect("test input is drive-admitted");
        let rows = stage_rows(&driven.parse).expect("both files fail to decode");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].invalid_utf8_facts(), Some((2, Some(1))));
        assert_eq!(rows[1].invalid_utf8_facts(), Some((3, None)));
        let broken: Vec<&str> = driven
            .facts
            .expect_complete()
            .broken_files
            .iter()
            .map(|at| at.of(&input).as_str())
            .collect();
        assert_eq!(broken, vec!["src/mid.mw", "src/tail.mw"]);
    }

    /// The diagnostics `compile_with_tests` reports, or `None` for a built image; a
    /// resource limit or invariant carries no diagnostics.
    fn compiled(result: &Result<CompiledTests, CompileFailure>) -> Option<Vec<SourceDiagnostic>> {
        match result {
            Ok(_) => None,
            Err(CompileFailure::Diagnostics(diagnostics)) => Some(diagnostics.as_slice().to_vec()),
            Err(CompileFailure::ResourceLimit(_) | CompileFailure::Invariant(_)) => {
                Some(Vec::new())
            }
        }
    }

    /// `compile_with_tests` is exactly the first-non-empty-stage projection of the one
    /// driven traversal — same diagnostics, same order, no stage mixing.
    fn assert_projection_faithful(files: &[(&str, &str)]) {
        let input = project(files);
        let driven = drive(&input, TestMode::Include).expect("test input is drive-admitted");
        assert_eq!(
            projected(&driven),
            compiled(&compile_with_tests(&input)),
            "projection diverged from compile_with_tests for {files:?}",
        );
    }

    #[test]
    fn projection_is_faithful_across_stage_stops() {
        // Clean (image builds).
        assert_projection_faithful(&[("src/main.mw", "pub fn f(): int {\n    return 1\n}\n")]);
        // Parse stop: a malformed header.
        assert_projection_faithful(&[("src/main.mw", "pub fn f(: int {\n    return 1\n}\n")]);
        // Semantic stop: a call to an undefined function.
        assert_projection_faithful(&[(
            "src/main.mw",
            "pub fn f(): int {\n    return missing()\n}\n",
        )]);
        // A multi-module project with an interdependence.
        assert_projection_faithful(&[
            (
                "src/library.mw",
                "module library\n\npub fn helper(): int {\n    return 2\n}\n",
            ),
            (
                "src/main.mw",
                "module main\nuse library\n\npub fn f(): int {\n    return library::helper()\n}\n",
            ),
        ]);
    }

    /// A syntax error in one module does not suppress the analysis of an independent
    /// valid module: the broken module contributes its parse diagnostic, and the valid
    /// module is still analyzed to the semantic stage (its own diagnostic is present in
    /// the driven accumulator), even though the production compile projects only the
    /// parse stage.
    #[test]
    fn an_independent_valid_module_is_analyzed_past_a_sibling_parse_error() {
        let files = &[
            (
                "src/broken.mw",
                "module broken\n\npub fn g(: int {\n    return 1\n}\n",
            ),
            (
                "src/valid.mw",
                "module valid\n\npub fn h(): int {\n    return missing()\n}\n",
            ),
        ];
        let input = project(files);
        let driven = drive(&input, TestMode::Include).expect("test input is drive-admitted");

        // The broken module's parse error is recorded.
        let parse_rows = stage_rows(&driven.parse).expect("the parse stage is non-empty");
        assert!(
            parse_rows
                .iter()
                .any(|d| d.file().as_str() == "src/broken.mw"),
            "the broken module's parse diagnostic must be recorded: {parse_rows:?}",
        );

        // The valid module was analyzed despite the sibling parse error: its own
        // semantic diagnostic reached the semantic stage.
        let semantic = match &driven.semantic {
            SemanticOutcome::Diagnostics(BoundedDiagnostics::Complete { rows, .. }, _) => {
                rows.clone()
            }
            _ => panic!("expected semantic diagnostics from the valid module"),
        };
        assert!(
            semantic.iter().any(|d| d.file().as_str() == "src/valid.mw"),
            "the valid module must be analyzed past the sibling parse error: {semantic:?}",
        );

        // The production compile still projects only the parse stage — byte-identical
        // to the historical parse hard-stop.
        assert_eq!(
            compiled(&compile_with_tests(&input)),
            stage_rows(&driven.parse)
        );
    }

    /// A valid module that genuinely depends on a parse-failed module is analyzed
    /// without a dangling reference: the broken module is absent from the analyzed
    /// set, so the dependent's references reduce to the ordinary missing-module
    /// diagnostic family — never a panic, an invariant, or a fabricated fact — and the
    /// production compile still projects only the broken module's parse stage. This
    /// pins the cross-reference case the midpoint review probed by hand.
    #[test]
    fn a_module_depending_on_a_parse_failed_module_reduces_to_the_missing_module_family() {
        let files = &[
            (
                "src/base.mw",
                "module base\n\npub fn provide(: int {\n    return 1\n}\n",
            ),
            (
                "src/dependent.mw",
                "module dependent\nuse base\n\npub fn f(): int {\n    return base::provide()\n}\n",
            ),
        ];
        let input = project(files);
        let driven = drive(&input, TestMode::Include).expect("test input is drive-admitted");

        // The broken base module's parse error is recorded.
        let parse_rows = stage_rows(&driven.parse).expect("the parse stage is non-empty");
        assert!(
            parse_rows
                .iter()
                .any(|d| d.file().as_str() == "src/base.mw")
        );

        // The dependent module is analyzed, and its cross-references to the absent base
        // module reduce to the ordinary missing-module family — no panic, no invariant,
        // no resource limit.
        let semantic = match &driven.semantic {
            SemanticOutcome::Diagnostics(BoundedDiagnostics::Complete { rows, .. }, _) => {
                rows.clone()
            }
            _ => panic!("a dependent module must still produce ordinary semantic diagnostics"),
        };
        let dependent: Vec<&SourceDiagnostic> = semantic
            .iter()
            .filter(|d| d.file().as_str() == "src/dependent.mw")
            .collect();
        assert!(
            !dependent.is_empty(),
            "the dependent module must be analyzed against the absent base: {semantic:?}",
        );
        // A reference into a module the parse stage refused is causal, not absent: the
        // import names that stage's report and the qualified call is steered to it,
        // carrying the declaring code. Neither denies the module or its callee exists.
        let referring_family = [
            marrow_codes::Code::CheckImport.as_str(),
            marrow_codes::Code::ParseSyntax.as_str(),
        ];
        assert!(
            dependent
                .iter()
                .all(|d| referring_family.contains(&d.code())),
            "a reference to a parse-failed module reduces to the import failure and the \
             steer to its parse report, never a fabricated or invariant code: {dependent:?}",
        );
        assert!(
            dependent
                .iter()
                .all(|d| !d.message().contains("is not in scope")
                    && !d.message().contains("no module")),
            "the base module is a file of this project; no row may deny it or its \
             callee: {dependent:?}",
        );

        // Byte-identical: the production compile projects only the base module's parse
        // stage.
        assert_eq!(
            compiled(&compile_with_tests(&input)),
            stage_rows(&driven.parse)
        );
    }
}
