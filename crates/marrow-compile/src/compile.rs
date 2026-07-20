//! The storeless subset checker and lowering to an [`ImageDraft`].
//!
//! The compiler opens no store and mints no verified image: it parses source,
//! checks the current subset, and lowers to canonical image bytes the independent
//! verifier rechecks. Coverage grows one slice at a time; a well-formed construct
//! outside the current subset is a typed `check.unsupported` diagnostic, never a
//! silent drop.

use std::collections::{BTreeMap, BTreeSet};

use marrow_codes::Code;
use marrow_image::{EncodedImage, ExportId, ImageBuildError, ImageDraft};
use marrow_project::{FileIdentity, ProjectInput};
use marrow_syntax::{
    AliasDecl, ConstDecl, Declaration, EnumDecl, FunctionDecl, NominalDecl, ParsedSource,
    ResourceDecl, ResourceMember, SourceSpan, StoreDecl, StructDecl, parse_source,
};

use crate::diag::SourceDiagnostic;
use crate::durable::DurableRegistry;
use crate::konst::ConstRegistry;
use crate::lower::{
    FnLowerer, FunctionRegistry, GenericRegistry, is_reserved_builtin_name, reserved_builtin_name,
};
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
    DurableMembers,
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
    /// One function frame's local count over the per-frame bound. Known only after
    /// lowering, so it has no pre-mutation source precheck.
    Locals,
    /// One function's encoded bytecode over the per-function byte bound. Known only
    /// after lowering.
    CodeBytes,
    /// A managed index's fully expanded projection over the component bound through a
    /// path the source precheck does not cover (a nonunique index whose appended
    /// identity keys carry the total past the bound).
    IndexComponents,
    /// A durable member tree nested past the depth bound. The exact image-side depth
    /// accounting is owned by the encoder, so the outcome is reported as a locationless
    /// resource limit rather than a divergent source count.
    DurableDepth,
    /// The ordered diagnostic set grew past the count bound, so the incomplete
    /// collection was discarded rather than surfaced as a truncated result.
    DiagnosticCount,
    /// The ordered diagnostic set grew past the total-byte bound, so the incomplete
    /// collection was discarded.
    DiagnosticBytes,
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

/// The largest ordered diagnostic set eligible for the [`CompileFailure::Diagnostics`]
/// arm, and the largest total message-byte footprint. A count or byte overflow
/// transactionally discards the incomplete collection and classifies as a
/// [`CompileResourceLimit`], so only a complete bounded diagnostic result reaches a
/// caller (§ law 9). The bounds sit far above any real edit cycle's diagnostic set
/// while still failing a pathological error avalanche closed.
const MAX_DIAGNOSTIC_COUNT: usize = 4096;
const MAX_DIAGNOSTIC_BYTES: usize = 1024 * 1024;

/// The bounded diagnostic collection owner: it seals an assembled ordered diagnostic
/// set into the arm it is eligible for. A set within both the count and byte bounds
/// commits as `Complete`; an empty set is `Empty` (its stage becomes a private
/// invariant unless a resource candidate exists); an overflow discards the whole
/// set (prefix included) and yields the resource limit that displaced it.
enum DiagnosticSeal {
    Complete(NonEmptySourceDiagnostics),
    Empty,
    Overflow(CompileResourceLimit),
}

/// Seal an assembled ordered diagnostic set: overflow of either bound discards the
/// incomplete collection and reports the displacing resource limit, so a truncated
/// diagnostic set never reaches the `Diagnostics` arm.
fn seal_diagnostics(diagnostics: Vec<SourceDiagnostic>) -> DiagnosticSeal {
    if diagnostics.len() > MAX_DIAGNOSTIC_COUNT {
        return DiagnosticSeal::Overflow(CompileResourceLimit::new(
            ResourceLimitKind::DiagnosticCount,
            MAX_DIAGNOSTIC_COUNT as u64,
        ));
    }
    let bytes: usize = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.len() + diagnostic.file().as_str().len())
        .sum();
    if bytes > MAX_DIAGNOSTIC_BYTES {
        return DiagnosticSeal::Overflow(CompileResourceLimit::new(
            ResourceLimitKind::DiagnosticBytes,
            MAX_DIAGNOSTIC_BYTES as u64,
        ));
    }
    match NonEmptySourceDiagnostics::new(diagnostics) {
        Some(diagnostics) => DiagnosticSeal::Complete(diagnostics),
        None => DiagnosticSeal::Empty,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompileStage {
    Parse,
    TypeInstantiation,
    FunctionSignatures,
    TemplateProof,
    BodyLowering,
    PostLoweringValidation,
}

#[derive(Debug)]
enum InvariantCause {
    Generic(GenericInvariant),
    EmptyDiagnostics(CompileStage),
    /// An image-build variant unreachable from a coherent compiler: a producer-state
    /// contradiction (an invalid cross-reference, a site path shorter than its
    /// minimum, a local count below the parameter count) or a per-construct bound a
    /// source precheck already refuses before the draft is built. Kept opaque: it is a
    /// compiler-internal defect, not a source diagnostic or an aggregate resource
    /// limit.
    ImageBuild(ImageBuildError),
}

/// The one boundary that assembles a total [`CompileFailure`] under the precedence
/// `Invariant > Diagnostics > ResourceLimit`. An invariant dominates unconditionally.
/// Otherwise the accumulated diagnostics are sealed: a complete bounded set is the
/// `Diagnostics` arm (dominating any independent `resource` candidate); an empty set
/// yields the `resource` candidate if one exists, else the empty-boundary invariant;
/// and a diagnostic-collector overflow discards the incomplete set and reports the
/// resource limit that displaced it.
fn compile_failure(
    diagnostics: Vec<SourceDiagnostic>,
    invariant: Option<InvariantCause>,
    resource: Option<CompileResourceLimit>,
    stage: CompileStage,
) -> CompileFailure {
    if let Some(cause) = invariant {
        return CompileFailure::Invariant(CompileInvariant(cause));
    }
    match seal_diagnostics(diagnostics) {
        DiagnosticSeal::Complete(diagnostics) => CompileFailure::Diagnostics(diagnostics),
        DiagnosticSeal::Overflow(limit) => CompileFailure::ResourceLimit(limit),
        DiagnosticSeal::Empty => match resource {
            Some(limit) => CompileFailure::ResourceLimit(limit),
            None => {
                CompileFailure::Invariant(CompileInvariant(InvariantCause::EmptyDiagnostics(stage)))
            }
        },
    }
}

fn diagnostic_failure(diagnostics: Vec<SourceDiagnostic>, stage: CompileStage) -> CompileFailure {
    compile_failure(diagnostics, None, None, stage)
}

/// Classify a producer-side [`ImageBuildError`] from `ImageDraft::encode` into the
/// compile-failure arm it belongs to. Encode runs only on the clean-diagnostics path,
/// so there is no coexisting diagnostic and the classification is total on its own.
///
/// A whole-program aggregate count, or the whole-image byte ceiling, has no single
/// source construct at fault and becomes a [`CompileResourceLimit`]. A per-construct
/// bound is refused earlier by a source precheck at its offending span, so reaching
/// it here means the draft was built past a bound the precheck should have caught — a
/// compiler-internal defect — and a producer-state contradiction (an invalid
/// reference, a too-short site path, a local count below the parameters) is likewise
/// unreachable from a coherent compiler; both are opaque invariants. The match has no
/// wildcard, so a new image-build variant forces an explicit classification here.
fn image_build_outcome(error: ImageBuildError) -> SemanticOutcome {
    use marrow_image::bounds;
    let stage = CompileStage::PostLoweringValidation;
    let aggregate = |kind: ResourceLimitKind, limit: usize| {
        SemanticOutcome::ResourceLimit(CompileResourceLimit::new(kind, limit as u64), stage)
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
        ImageBuildError::TooManyDurableMembers => aggregate(
            ResourceLimitKind::DurableMembers,
            bounds::MAX_DURABLE_MEMBERS,
        ),
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
        // Per-construct bounds knowable only after lowering, or reachable through a
        // path no pre-mutation source precheck yet covers: an honest locationless
        // resource limit, never the synthetic diagnostic.
        ImageBuildError::StringTooLong => {
            aggregate(ResourceLimitKind::StringBytes, bounds::MAX_STRING_BYTES)
        }
        ImageBuildError::TooManyLocals => aggregate(ResourceLimitKind::Locals, bounds::MAX_LOCALS),
        ImageBuildError::CodeTooLong => {
            aggregate(ResourceLimitKind::CodeBytes, bounds::MAX_CODE_BYTES)
        }
        ImageBuildError::TooManyIndexComponents => aggregate(
            ResourceLimitKind::IndexComponents,
            bounds::MAX_INDEX_COMPONENTS,
        ),
        ImageBuildError::DurableTreeTooDeep => {
            aggregate(ResourceLimitKind::DurableDepth, bounds::MAX_DURABLE_DEPTH)
        }
        // Per-construct bounds a source precheck refuses before the draft is built, so
        // an encode-time occurrence is a defense-in-depth producer defect; and
        // producer-state contradictions unreachable from a coherent compiler. Both are
        // opaque invariants.
        ImageBuildError::TooManyFields
        | ImageBuildError::TooManyStructLeaves
        | ImageBuildError::TooManyVariants
        | ImageBuildError::TooManyPayloadFields
        | ImageBuildError::TooManyIndexes
        | ImageBuildError::TooManyKeyColumns
        | ImageBuildError::DurableValueTooDeep
        | ImageBuildError::TooManyParams
        | ImageBuildError::SitePathTooShort
        | ImageBuildError::SitePathTooDeep
        | ImageBuildError::LocalCountBelowParams
        | ImageBuildError::InvalidReference(_) => {
            SemanticOutcome::Invariant(InvariantCause::ImageBuild(error), stage)
        }
    }
}

/// A resource-limit failure with no source diagnostic: an aggregate encode bound the
/// program exhausted. It carries no location. Diagnostics would dominate by
/// precedence, but encode runs only when diagnostics are empty, so this is the sole
/// candidate at its site.
fn resource_failure(limit: CompileResourceLimit, stage: CompileStage) -> CompileFailure {
    compile_failure(Vec::new(), None, Some(limit), stage)
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
/// module name (for export identity), and the parse tree.
struct Module {
    file: FileIdentity,
    name: String,
    parsed: ParsedSource,
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
}

/// Compile a captured project into canonical program-image bytes and its export
/// directory, or return source diagnostics or an opaque compiler-coherence failure.
/// The production path excludes `test` declarations and emits an empty TEST-ENTRY
/// table.
pub fn compile(project: &ProjectInput) -> Result<Compiled, CompileFailure> {
    let built = drive(project, TestMode::Exclude).into_built()?;
    Ok(Compiled {
        image: built.image,
        exports: built.exports,
    })
}

/// Compile a captured project *with* its tests: the image additionally carries the
/// test functions and the closed TEST-ENTRY table, and the returned directory pairs
/// each test's title with its location for `marrow test`. Failure uses the same
/// source-diagnostic or opaque compiler-invariant boundary as [`compile`].
pub fn compile_with_tests(project: &ProjectInput) -> Result<CompiledTests, CompileFailure> {
    let built = drive(project, TestMode::Include).into_built()?;
    Ok(CompiledTests {
        image: built.image,
        exports: built.exports,
        tests: built.tests,
    })
}

/// The image, export directory, and (when included) test directory a compilation
/// produced.
struct Built {
    image: EncodedImage,
    exports: Vec<ExportEntry>,
    tests: Vec<TestEntry>,
}

/// The staged outcome of one analysis/lowering pass over a project. Diagnostics are
/// bucketed by the stage that produced them, so a single traversal serves both the
/// production compile — which projects the first non-empty stage and thereby
/// reproduces the historical staged early-return byte for byte — and the editor
/// analysis snapshot, which consumes every stage. There is no analyze/compile mode
/// flag forking control flow: the traversal is one and the same; only the projection
/// differs.
struct Driven {
    /// Invalid-UTF-8 and syntax diagnostics from every module, parseable or not.
    parse: Vec<SourceDiagnostic>,
    /// Structural-bound diagnostics over the cleanly-parsed modules.
    structural: Vec<SourceDiagnostic>,
    /// The semantic pass over the cleanly-parsed modules.
    semantic: SemanticOutcome,
    /// Editor hover facts collected while lowering the cleanly-parsed bodies. Carried
    /// out of the traversal orthogonally to the semantic outcome; the production
    /// compile's projection ignores them and the analysis snapshot consumes them.
    hover_facts: Vec<crate::analysis::HoverFact>,
}

/// The outcome of the semantic pass over the cleanly-parsed modules: a complete image,
/// or the accumulated failure tagged with the stage that produced it.
enum SemanticOutcome {
    Built(Built),
    Diagnostics(Vec<SourceDiagnostic>, CompileStage),
    ResourceLimit(CompileResourceLimit, CompileStage),
    Invariant(InvariantCause, CompileStage),
}

impl Driven {
    /// Project the production compile result. The first non-empty stage in order —
    /// parse, then structural, then semantic — is the failure, byte-identical to the
    /// historical staged early-return (diagnostics are never sorted or deduped, so a
    /// stage's set is exactly what that stage would have returned). A fully clean pass
    /// yields the image.
    fn into_built(self) -> Result<Built, CompileFailure> {
        if !self.parse.is_empty() {
            return Err(diagnostic_failure(self.parse, CompileStage::Parse));
        }
        if !self.structural.is_empty() {
            return Err(diagnostic_failure(self.structural, CompileStage::Parse));
        }
        match self.semantic {
            SemanticOutcome::Built(built) => Ok(built),
            SemanticOutcome::Diagnostics(diagnostics, stage) => {
                Err(diagnostic_failure(diagnostics, stage))
            }
            SemanticOutcome::ResourceLimit(limit, stage) => Err(resource_failure(limit, stage)),
            SemanticOutcome::Invariant(cause, stage) => {
                Err(compile_failure(Vec::new(), Some(cause), None, stage))
            }
        }
    }
}

/// Parse every module, then analyze the cleanly-parsed ones. A module with a parse
/// error contributes its parse diagnostics and its declarations are left unanalyzed —
/// dependency resilience: a syntax error in one component does not suppress the
/// diagnostics or facts of an independent valid component. The semantic pass always
/// runs over whatever parsed cleanly; the projection decides what the production
/// compile reports.
fn drive(project: &ProjectInput, mode: TestMode) -> Driven {
    let mut parse = Vec::new();
    let mut parsed: Vec<Module> = Vec::new();
    for module in project.modules() {
        let file = module.identity().clone();
        let name = module.module().as_str().to_string();
        match std::str::from_utf8(module.source()) {
            Ok(source) => parsed.push(Module {
                file,
                name,
                parsed: parse_source(source),
            }),
            Err(_) => parse.push(SourceDiagnostic::at(
                Code::CheckUnsupported.as_str(),
                &file,
                // A non-UTF-8 file has no parsed construct to point at: a zero-length
                // span at the file start, whose 1-based point is 1:1.
                SourceSpan {
                    start_byte: 0,
                    end_byte: 0,
                    line: 1,
                    column: 1,
                },
                "source file is not valid UTF-8".to_string(),
            )),
        }
    }
    for module in &parsed {
        for diagnostic in &module.parsed.diagnostics {
            if diagnostic.severity == marrow_syntax::Severity::Error {
                parse.push(SourceDiagnostic::at(
                    diagnostic.code,
                    &module.file,
                    diagnostic.span,
                    diagnostic.message.clone(),
                ));
            }
        }
    }

    // Only cleanly-parsed modules enter analysis; a module with a parse error is
    // skipped as a dependent unit (its parse diagnostics are already recorded).
    let clean: Vec<Module> = parsed
        .into_iter()
        .filter(|module| {
            !module
                .parsed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == marrow_syntax::Severity::Error)
        })
        .collect();

    // Refuse a structural declaration bound at its offending construct before any
    // image structure is built. These counts are exact source properties — a record's
    // top-level field width and a function's parameter arity — so the check runs on the
    // parse tree ahead of the first draft mutation. Durable member-tree nesting depth is
    // not checked here: its exact accounting is the encoder's, and it surfaces as a
    // locationless `DurableDepth` resource limit rather than a divergent source count.
    let mut structural = Vec::new();
    check_structural_resource_bounds(&clean, &mut structural);

    let mut hover_facts = Vec::new();
    let semantic = run_semantic(&clean, project, mode, &mut hover_facts);
    Driven {
        parse,
        structural,
        semantic,
        hover_facts,
    }
}

/// Analyze the cleanly-parsed modules: build the named types and function signatures,
/// lower every body, and validate the whole, or return the accumulated failure tagged
/// with the stage that produced it. Editor hover facts from each monomorphic function
/// and test body are collected into `hover_facts` as they are lowered.
fn run_semantic(
    parsed: &[Module],
    project: &ProjectInput,
    mode: TestMode,
    hover_facts: &mut Vec<crate::analysis::HoverFact>,
) -> SemanticOutcome {
    let mut diagnostics = Vec::new();

    // The source-root-relative path is the authority for module identity. A file
    // that declares a `module` header is an importable module and must spell the
    // path-derived name (with `::` as the dotted separator). A file with no header
    // is a single-file script: it keeps a path-derived identity for its own scope
    // and its exports, but is not importable by module path.
    let mut module_names: BTreeSet<String> = BTreeSet::new();
    for module in parsed {
        if let Some(header) = &module.parsed.file.module {
            let declared = header.name.replace("::", ".");
            if declared == module.name {
                module_names.insert(module.name.clone());
            } else {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckModulePath.as_str(),
                    &module.file,
                    header.span,
                    format!(
                        "module header `{}` does not match its path; expected `module {}`",
                        header.name,
                        module.name.replace('.', "::")
                    ),
                ));
            }
        }
    }

    // Each module's `use` bindings (final segment -> dotted target). A `use` must
    // name an importable project module; two imports binding the same final segment
    // in one module are ambiguous.
    let mut imports: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for module in parsed {
        let bindings = imports.entry(module.name.clone()).or_default();
        for use_decl in &module.parsed.file.uses {
            let target = use_decl.name.replace("::", ".");
            let segment = target
                .rsplit('.')
                .next()
                .unwrap_or(target.as_str())
                .to_string();
            if !module_names.contains(&target) {
                diagnostics.push(SourceDiagnostic::at(
                    Code::CheckImport.as_str(),
                    &module.file,
                    use_decl.span,
                    format!("no module `{}` in this project", use_decl.name),
                ));
                continue;
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
    let functions: Vec<(FileIdentity, String, &FunctionDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.parsed.file.declarations.iter().filter_map(|decl| {
                if let Declaration::Function(function) = decl {
                    Some((module.file.clone(), module.name.clone(), function))
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
    let aliases: Vec<(FileIdentity, &AliasDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.parsed.file.declarations.iter().filter_map(|decl| {
                if let Declaration::Alias(alias) = decl {
                    Some((module.file.clone(), alias))
                } else {
                    None
                }
            })
        })
        .collect();
    let nominals: Vec<(FileIdentity, &NominalDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.parsed.file.declarations.iter().filter_map(|decl| {
                if let Declaration::Nominal(nominal) = decl {
                    Some((module.file.clone(), nominal))
                } else {
                    None
                }
            })
        })
        .collect();
    let resources: Vec<(FileIdentity, &ResourceDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.parsed.file.declarations.iter().filter_map(|decl| {
                if let Declaration::Resource(resource) = decl {
                    Some((module.file.clone(), resource))
                } else {
                    None
                }
            })
        })
        .collect();
    let structs: Vec<(FileIdentity, &StructDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.parsed.file.declarations.iter().filter_map(|decl| {
                if let Declaration::Struct(item) = decl {
                    Some((module.file.clone(), item))
                } else {
                    None
                }
            })
        })
        .collect();
    let enums: Vec<(FileIdentity, &EnumDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.parsed.file.declarations.iter().filter_map(|decl| {
                if let Declaration::Enum(item) = decl {
                    Some((module.file.clone(), item))
                } else {
                    None
                }
            })
        })
        .collect();
    let records = TypeRegistry::build(
        &mut draft,
        &aliases,
        &nominals,
        &structs,
        &enums,
        &resources,
        &mut diagnostics,
    );
    if let Some(invariant) = records.build_invariant() {
        return SemanticOutcome::Invariant(
            InvariantCause::Generic(invariant),
            CompileStage::TypeInstantiation,
        );
    }
    if records.has_instantiation_limit() {
        diagnostics.extend(records.take_generic_diagnostics().into_ordered());
        return SemanticOutcome::Diagnostics(diagnostics, CompileStage::TypeInstantiation);
    }
    let stores: Vec<(FileIdentity, &StoreDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.parsed.file.declarations.iter().filter_map(|decl| {
                if let Declaration::Store(store) = decl {
                    Some((module.file.clone(), store))
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
    ) {
        Ok(durable) => durable,
        Err(invariant) => {
            return SemanticOutcome::Invariant(
                InvariantCause::Generic(invariant),
                CompileStage::TypeInstantiation,
            );
        }
    };
    let signatures = match FunctionRegistry::build(
        &records,
        &mut draft,
        &durable,
        &functions,
        module_names,
        imports,
        &mut diagnostics,
    ) {
        Ok(Some(signatures)) => signatures,
        Ok(None) => {
            diagnostics.extend(records.take_generic_diagnostics().into_ordered());
            return SemanticOutcome::Diagnostics(diagnostics, CompileStage::FunctionSignatures);
        }
        Err(invariant) => {
            return SemanticOutcome::Invariant(
                InvariantCause::Generic(invariant),
                CompileStage::FunctionSignatures,
            );
        }
    };
    // Generic functions are templates with no image index; they are monomorphized at
    // each call site and once-checked below against their constraints.
    let generic_functions: Vec<(FileIdentity, String, &FunctionDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.parsed.file.declarations.iter().filter_map(|decl| {
                if let Declaration::Function(function) = decl
                    && !function.type_params.is_empty()
                {
                    Some((module.file.clone(), module.name.clone(), function))
                } else {
                    None
                }
            })
        })
        .collect();
    let generics = GenericRegistry::build(&generic_functions);

    // Module-private constants, evaluated before body lowering so a reference folds
    // to its value.
    let const_decls: Vec<(String, FileIdentity, &ConstDecl)> = parsed
        .iter()
        .flat_map(|module| {
            module.parsed.file.declarations.iter().filter_map(|decl| {
                if let Declaration::Const(konst) = decl {
                    Some((module.name.clone(), module.file.clone(), konst))
                } else {
                    None
                }
            })
        })
        .collect();
    let constants = ConstRegistry::build(&const_decls, &records, &mut diagnostics);

    // Once-checked template pass: every generic function's body is type-checked once
    // against its type parameters' constraints — independently of whether or how it
    // is instantiated — so an unconstrained parameter used with `==`/`<`, or any
    // other constraint violation, is caught here rather than per instantiation.
    for template in generics.templates() {
        let outcome = match FnLowerer::check_template(
            &draft,
            &records,
            &durable,
            &signatures,
            &generics,
            &constants,
            template,
        ) {
            Ok(outcome) => outcome,
            Err(invariant) => {
                return SemanticOutcome::Invariant(
                    InvariantCause::Generic(invariant),
                    CompileStage::TemplateProof,
                );
            }
        };
        diagnostics.extend(outcome.diagnostics);
        records.adopt_generic_diagnostics(outcome.generic);
        if records.has_instantiation_limit() {
            break;
        }
    }
    if records.has_instantiation_limit() {
        diagnostics.extend(records.take_generic_diagnostics().into_ordered());
        return SemanticOutcome::Diagnostics(diagnostics, CompileStage::TemplateProof);
    }

    // Generic instances are image functions with no stable identity, indexed after
    // every monomorphic function and test. `base` is that boundary; the shared
    // `Monomorph` assigns each distinct instance the next index from `base` in
    // discovery order, so draining its queue in order appends them to the image in
    // index order.
    let test_count: u16 = if mode == TestMode::Include {
        parsed
            .iter()
            .flat_map(|module| &module.parsed.file.declarations)
            .filter(|decl| matches!(decl, Declaration::Test(_)))
            .count() as u16
    } else {
        0
    };
    let base = signatures.concrete_count() + test_count;
    records.set_fn_base(base);

    // Lower each function, in the same order the registry assigned indices, minting
    // an export for each public function from its declaration path and recording its
    // direct-call edges for recursion detection. Other declarations are handled
    // above or not yet admitted. Generic templates are skipped here — they are
    // monomorphized on demand and drained below.
    let mut exports: Vec<ExportEntry> = Vec::new();
    let mut lowered: Vec<LoweredFn> = Vec::new();
    'function_bodies: for module in parsed {
        for declaration in &module.parsed.file.declarations {
            match declaration {
                Declaration::Function(function) if !function.type_params.is_empty() => {
                    // A generic template is not lowered in place.
                    let _ = function;
                }
                Declaration::Function(function) => {
                    let result = match FnLowerer::lower(
                        &mut draft,
                        &records,
                        &durable,
                        &signatures,
                        &generics,
                        &constants,
                        &mut diagnostics,
                        &module.file,
                        &module.name,
                        function,
                    ) {
                        Ok(Some(result)) => result,
                        Ok(None) => {
                            if records.has_instantiation_limit() {
                                break 'function_bodies;
                            }
                            continue;
                        }
                        Err(invariant) => {
                            return SemanticOutcome::Invariant(
                                InvariantCause::Generic(invariant),
                                CompileStage::BodyLowering,
                            );
                        }
                    };
                    for (span, display) in result.hover_facts {
                        hover_facts.push(crate::analysis::HoverFact {
                            file: module.file.clone(),
                            span,
                            display,
                        });
                    }
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
                    });
                    if function.public {
                        // The injectivity owner's own guard: every dotted module
                        // segment and the item must be ASCII identifiers before an
                        // ExportId is minted over them (see marrow-image::export_id).
                        // Unreachable through the current capture path, which already
                        // constrains both; kept so the id payload's injectivity never
                        // silently rests on an upstream layer alone.
                        if !valid_export_path(&module.name, &function.name) {
                            diagnostics.push(SourceDiagnostic::at(
                                Code::CheckModulePath.as_str(),
                                &module.file,
                                function.span,
                                format!(
                                    "export `{}` in module `{}` is not an ASCII \
                                     identifier path, so it cannot be exported",
                                    function.name, module.name
                                ),
                            ));
                            continue;
                        }
                        let id = ExportId::of_local(&module.name, &function.name);
                        draft.add_export(id, result.func);
                        exports.push(ExportEntry {
                            module: module.name.clone(),
                            item: function.name.clone(),
                            id,
                        });
                    }
                    if records.has_instantiation_limit() {
                        break 'function_bodies;
                    }
                }
                // Constants are evaluated into the const registry above; aliases,
                // resources, and stores are handled by their own registries; test
                // declarations are lowered separately below, after every function
                // has an index.
                Declaration::Alias(_)
                | Declaration::Nominal(_)
                | Declaration::Const(_)
                | Declaration::Resource(_)
                | Declaration::Struct(_)
                | Declaration::Enum(_)
                | Declaration::Store(_)
                | Declaration::Test(_) => {}
            }
        }
    }

    // Lower each `test "name"` body into a storeless, zero-argument function and
    // bind its title into the TEST-ENTRY table (only when tests are included). Tests
    // are lowered after every function so their bodies' calls resolve and their own
    // indices follow the functions'. Titles are unique across the project.
    let mut tests: Vec<TestEntry> = Vec::new();
    if mode == TestMode::Include && !records.has_instantiation_limit() {
        'test_bodies: for module in parsed {
            for declaration in &module.parsed.file.declarations {
                let Declaration::Test(test) = declaration else {
                    continue;
                };
                if tests.iter().any(|existing| existing.name == test.name) {
                    diagnostics.push(SourceDiagnostic::at(
                        Code::CheckNameConflict.as_str(),
                        &module.file,
                        test.name_span,
                        format!("a test named `{}` is already declared", test.name),
                    ));
                    continue;
                }
                let result = match FnLowerer::lower_test(
                    &mut draft,
                    &records,
                    &durable,
                    &signatures,
                    &generics,
                    &constants,
                    &mut diagnostics,
                    &module.file,
                    &module.name,
                    &test.name,
                    &test.body,
                ) {
                    Ok(Some(result)) => result,
                    Ok(None) => {
                        if records.has_instantiation_limit() {
                            break 'test_bodies;
                        }
                        continue;
                    }
                    Err(invariant) => {
                        return SemanticOutcome::Invariant(
                            InvariantCause::Generic(invariant),
                            CompileStage::BodyLowering,
                        );
                    }
                };
                for (span, display) in result.hover_facts {
                    hover_facts.push(crate::analysis::HoverFact {
                        file: module.file.clone(),
                        span,
                        display,
                    });
                }
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
                });
                let name_id = draft.intern_string(&test.name);
                draft.add_test_entry(name_id, result.func);
                tests.push(TestEntry {
                    name: test.name.clone(),
                    module: module.name.clone(),
                    file: module.file.as_str().to_string(),
                    line: test.name_span.line,
                    column: test.name_span.column,
                });
                if records.has_instantiation_limit() {
                    break 'test_bodies;
                }
            }
        }
    }

    // Drain the generic instantiation worklist: lower each monomorphized instance's
    // body into the image, in the order the instances were minted (so each instance's
    // image index equals the one the registry reserved). Lowering an instance body
    // may mint further instances, which the loop continues to drain. Only run when the
    // monomorphic pass is clean, so every function and test has already consumed its
    // image index and instances append after them.
    if diagnostics.is_empty() && !records.has_instantiation_limit() {
        while let Some((template_index, args, reserved)) = records.next_fn_pending() {
            let template = &generics.templates()[template_index];
            let result = match FnLowerer::lower_instance(
                &mut draft,
                &records,
                &durable,
                &signatures,
                &generics,
                &constants,
                &mut diagnostics,
                template,
                &args,
            ) {
                Ok(Some(result)) => result,
                Ok(None) => break,
                Err(invariant) => {
                    return SemanticOutcome::Invariant(
                        InvariantCause::Generic(invariant),
                        CompileStage::BodyLowering,
                    );
                }
            };
            debug_assert_eq!(
                result.func.index(),
                reserved,
                "instance image index must match its reserved index"
            );
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
            });
        }
    }

    // Report any diagnostics recorded while minting generic type instantiations
    // (the shared instantiation limit) and reject a value-containment cycle over the
    // full set of concrete types and generic instantiations minted anywhere (a
    // monomorphized `Tree[int]` containing `Tree[int]` is an ordinary record cycle),
    // now that every field and body annotation has been resolved.
    let stopped_on_limit = records.has_instantiation_limit();
    diagnostics.extend(records.take_generic_diagnostics().into_ordered());
    if stopped_on_limit {
        return SemanticOutcome::Diagnostics(diagnostics, CompileStage::BodyLowering);
    }
    if let Err(invariant) =
        crate::types::reject_value_cycles(&records, &structs, &resources, &mut diagnostics)
    {
        return SemanticOutcome::Invariant(
            InvariantCause::Generic(invariant),
            CompileStage::PostLoweringValidation,
        );
    }

    // The compiled subset does not admit recursion: the direct-call graph must be
    // acyclic. Reported at check time so the source carries the diagnostic. The
    // verifier independently rejects any cycle that still reaches it (image.closure),
    // so this is a source-facing check, not the trust boundary. Only run it once
    // every function, test, and generic instance lowered, so the indices are aligned.
    if diagnostics.is_empty() {
        reject_recursion(&lowered, &mut diagnostics);
    }

    // A function that mutates durable state carries a checked requires-ambient-
    // transaction effect: it is callable only inside a `transaction` block or from
    // another function carrying the effect. Reported at check time so the source, not
    // the image, carries the diagnostic; the verifier reconstructs the same closure and
    // rejects a tampered image (image.flow) as defense in depth. Run once the call
    // graph is acyclic so the effect fixpoint terminates and indices are aligned.
    if diagnostics.is_empty() {
        reject_missing_transaction(&lowered, &mut diagnostics);
    }

    // A test body reaches durable data in one of two disjoint ways — directly, or by
    // driving exports — and may not do both. Reported at check time so the source
    // carries the diagnostic; the verifier's test-entry phase rejects a mixed image
    // (image.test_entry) as defense in depth. Run once the call graph is acyclic.
    if diagnostics.is_empty() {
        reject_mixed_test_bodies(&lowered, &mut diagnostics);
    }

    if !diagnostics.is_empty() {
        return SemanticOutcome::Diagnostics(diagnostics, CompileStage::PostLoweringValidation);
    }

    match draft.encode() {
        Ok(image) => SemanticOutcome::Built(Built {
            image,
            exports,
            tests,
        }),
        Err(error) => image_build_outcome(error),
    }
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
    pub(crate) hover_facts: Vec<crate::analysis::HoverFact>,
    pub(crate) broken_files: Vec<FileIdentity>,
}

/// Drive the analysis pass over a project — test bodies included, per the editor
/// analysis contract — and resolve its complete diagnostic picture under the shared
/// precedence `Invariant > Diagnostics > ResourceLimit`. The complete union of every
/// stage's diagnostics is sealed against the same CRES01 count/byte bounds the
/// production compile uses, so a diagnostic avalanche transactionally becomes a resource
/// limit rather than a retained partial set — no partial or truncated snapshot is
/// admitted.
pub(crate) fn analyze_project(project: &ProjectInput) -> ProjectAnalysis {
    let driven = drive(project, TestMode::Include);
    let hover_facts = driven.hover_facts;
    // A file that contributed a parse-stage diagnostic did not parse cleanly; a fact
    // query in it is syntax-unavailable rather than absent.
    let mut broken_files: Vec<FileIdentity> = Vec::new();
    for diagnostic in &driven.parse {
        if !broken_files.iter().any(|file| file == diagnostic.file()) {
            broken_files.push(diagnostic.file().clone());
        }
    }
    let outcome = analyze_outcome(driven.parse, driven.structural, driven.semantic);
    ProjectAnalysis {
        outcome,
        hover_facts,
        broken_files,
    }
}

/// Resolve the complete diagnostic outcome under the shared precedence from the driven
/// stage buckets.
fn analyze_outcome(
    parse: Vec<SourceDiagnostic>,
    structural: Vec<SourceDiagnostic>,
    semantic: SemanticOutcome,
) -> Analyzed {
    let mut diagnostics = parse;
    diagnostics.extend(structural);
    // The parse and structural prechecks preempt the semantic pass in the production
    // compile: `into_built` returns those stages before the semantic outcome is
    // consulted. So a real precheck diagnostic dominates a semantic invariant or
    // resource limit here too — otherwise a defense-in-depth encode outcome (a bound the
    // precheck already owns) would diverge from the production result. The semantic
    // pass's own diagnostics still union in for dependency resilience.
    let precheck_present = !diagnostics.is_empty();
    let mut resource = None;
    let mut invariant = None;
    match semantic {
        SemanticOutcome::Invariant(cause, _) => invariant = Some(CompileInvariant(cause)),
        SemanticOutcome::Diagnostics(semantic, _) => diagnostics.extend(semantic),
        SemanticOutcome::ResourceLimit(limit, _) => resource = Some(limit),
        SemanticOutcome::Built(_) => {}
    }
    if let Some(invariant) = invariant
        && !precheck_present
    {
        return Analyzed::Invariant(invariant);
    }
    match seal_diagnostics(diagnostics) {
        DiagnosticSeal::Complete(diagnostics) => Analyzed::Diagnostics(diagnostics.into_vec()),
        DiagnosticSeal::Overflow(limit) => Analyzed::ResourceLimit(limit),
        DiagnosticSeal::Empty => match resource {
            Some(limit) => Analyzed::ResourceLimit(limit),
            None => Analyzed::Diagnostics(Vec::new()),
        },
    }
}

/// Report a `check.name_conflict` for every function name declared more than once
/// within a single module (a `Call` must resolve to a unique target) and for every
/// function whose name is a reserved built-in the compiler intercepts in call
/// position (`some`/`exists`/`trim`/...); such a function would be admitted and
/// then never reached. Functions of the same name in different modules are distinct
/// and do not conflict.
fn reject_duplicate_functions(parsed: &[Module], diagnostics: &mut Vec<SourceDiagnostic>) {
    for module in parsed {
        let mut seen: Vec<&str> = Vec::new();
        for declaration in &module.parsed.file.declarations {
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
fn reject_recursion(lowered: &[LoweredFn], diagnostics: &mut Vec<SourceDiagnostic>) {
    // Adjacency by image index. Indices are dense (0..lowered.len()) and each
    // function appears once, so a plain vec keyed by index is exact.
    let mut callees: Vec<&[u16]> = vec![&[]; lowered.len()];
    for function in lowered {
        if (function.index as usize) < callees.len() {
            callees[function.index as usize] = &function.callees;
        }
    }
    for function in lowered {
        if reaches_self(function.index, &callees) {
            diagnostics.push(SourceDiagnostic::at(
                Code::CheckRecursion.as_str(),
                &function.file,
                function.span,
                format!("`{}` is part of a recursive call cycle", function.name),
            ));
        }
    }
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
fn reject_missing_transaction(lowered: &[LoweredFn], diagnostics: &mut Vec<SourceDiagnostic>) {
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
            }
        }
    }
}

/// Report `check.test_driver_mix` for every `test` body that both performs a durable
/// operation directly and drives a transaction-owning export. The two invocation
/// models — one harness session for direct operations, one session per driven export
/// call — cannot share a body: the driven export's commit would consume the harness
/// session the direct operation needs. Only a directly-owned transaction counts as a
/// driven owner; because a transaction owner is never reached through a helper, the
/// test body's direct call edges carry the whole relation.
fn reject_mixed_test_bodies(lowered: &[LoweredFn], diagnostics: &mut Vec<SourceDiagnostic>) {
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
fn check_structural_resource_bounds(parsed: &[Module], diagnostics: &mut Vec<SourceDiagnostic>) {
    for module in parsed {
        for declaration in &module.parsed.file.declarations {
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
    diagnostics: &mut Vec<SourceDiagnostic>,
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
    use super::{Built, CompileFailure, CompileStage, InvariantCause, compile_failure};
    use crate::diag::SourceDiagnostic;
    use crate::types::{
        CollectionKind, GenericCacheInvariant, GenericInvariant, ProofCloneError, Reserved,
        TypeInstKind,
    };
    use marrow_codes::Code;
    use marrow_syntax::SourceSpan;

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
            CompileStage::Parse => "parse",
            CompileStage::TypeInstantiation => "type instantiation",
            CompileStage::FunctionSignatures => "function signatures",
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

    /// A private compiler-coherence failure dominates source diagnostics already
    /// accumulated at that stage. The public result contains no partial image.
    #[test]
    fn invariant_dominates_existing_diagnostics_at_the_public_boundary() {
        let outcome: Result<Built, CompileFailure> = Err(compile_failure(
            vec![diagnostic(Code::CheckType.as_str(), 3)],
            Some(proof_clone_cause()),
            None,
            CompileStage::TemplateProof,
        ));
        let Err(failure) = outcome else {
            panic!("an invariant must not produce a partial image")
        };

        assert_eq!(failure.to_string(), "compiler invariant failure");
        assert!(std::error::Error::source(&failure).is_some());
        let CompileFailure::Invariant(invariant) = failure else {
            panic!("the private invariant must dominate retained diagnostics")
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

    /// Diagnostics preserve their original order and allocation behind a statically
    /// nonempty owner. Every borrowed and owned iteration surface observes that order.
    #[test]
    fn diagnostic_failure_preserves_order_allocation_and_iteration_views() {
        let expected = vec![
            diagnostic(Code::CheckType.as_str(), 4),
            diagnostic(Code::CheckType.as_str(), 9),
        ];
        let mut original = Vec::with_capacity(8);
        original.extend(expected.iter().cloned());
        let original_ptr = original.as_ptr();
        let original_capacity = original.capacity();
        let failure = compile_failure(original, None, None, CompileStage::BodyLowering);
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
        assert_eq!(recovered.as_ptr(), original_ptr);
        assert_eq!(recovered.capacity(), original_capacity);
        assert_eq!(recovered, expected);

        let mut original = Vec::with_capacity(8);
        original.extend(expected.iter().cloned());
        let original_ptr = original.as_ptr();
        let failure = compile_failure(original, None, None, CompileStage::BodyLowering);
        let CompileFailure::Diagnostics(diagnostics) = failure else {
            panic!("a nonempty source failure must remain diagnostics")
        };
        let iterator: std::vec::IntoIter<SourceDiagnostic> = diagnostics.into_iter();
        assert_eq!(iterator.as_slice().as_ptr(), original_ptr);
        assert_eq!(iterator.as_slice(), expected.as_slice());
        assert_eq!(iterator.collect::<Vec<_>>(), expected);
    }

    /// An empty source-diagnostic vector is a private invariant carrying the exact
    /// stage that attempted to cross the boundary. The matcher intentionally has no
    /// wildcard, so adding a stage requires updating this contract.
    #[test]
    fn empty_failure_is_an_exact_invariant_at_every_compile_stage() {
        for stage in [
            CompileStage::Parse,
            CompileStage::TypeInstantiation,
            CompileStage::FunctionSignatures,
            CompileStage::TemplateProof,
            CompileStage::BodyLowering,
            CompileStage::PostLoweringValidation,
        ] {
            let empty = compile_failure(Vec::new(), None, None, stage);
            let CompileFailure::Invariant(invariant) = empty else {
                panic!("an empty diagnostic failure must become a compiler invariant")
            };
            let InvariantCause::EmptyDiagnostics(actual) = invariant.0 else {
                panic!("the empty boundary keeps its private stage")
            };
            assert_eq!(stage_label(actual), stage_label(stage));
            assert_eq!(actual, stage);
        }
    }

    #[test]
    fn public_invariant_is_worker_transferable_without_exposing_its_cause() {
        fn assert_worker_type<T: Send + Sync + 'static>() {}

        assert_worker_type::<super::CompileInvariant>();
    }

    fn resource(kind: super::ResourceLimitKind) -> super::CompileResourceLimit {
        super::CompileResourceLimit::new(kind, 64)
    }

    /// A resource candidate surfaces only when no invariant and no diagnostic set
    /// coexist: it is the lowest arm of the `Invariant > Diagnostics > ResourceLimit`
    /// precedence.
    #[test]
    fn resource_limit_surfaces_with_no_invariant_and_no_diagnostics() {
        let failure = compile_failure(
            Vec::new(),
            None,
            Some(resource(super::ResourceLimitKind::Functions)),
            CompileStage::PostLoweringValidation,
        );
        let CompileFailure::ResourceLimit(limit) = failure else {
            panic!("an aggregate bound with no diagnostics is a resource limit")
        };
        assert_eq!(limit.kind(), super::ResourceLimitKind::Functions);
        assert_eq!(limit.limit(), 64);
        assert_eq!(
            format!("{limit}"),
            "compiler resource limit reached",
            "the limit's display carries no location or count"
        );
    }

    /// A complete bounded diagnostic set dominates an independent later resource
    /// candidate.
    #[test]
    fn complete_diagnostics_dominate_a_resource_candidate() {
        let failure = compile_failure(
            vec![diagnostic(Code::CheckType.as_str(), 3)],
            None,
            Some(resource(super::ResourceLimitKind::Sites)),
            CompileStage::PostLoweringValidation,
        );
        assert!(
            matches!(failure, CompileFailure::Diagnostics(_)),
            "a complete diagnostic set outranks a resource candidate"
        );
    }

    /// A private invariant dominates a coexisting resource candidate.
    #[test]
    fn invariant_dominates_a_resource_candidate() {
        let failure = compile_failure(
            Vec::new(),
            Some(proof_clone_cause()),
            Some(resource(super::ResourceLimitKind::ImageBytes)),
            CompileStage::PostLoweringValidation,
        );
        assert!(
            matches!(failure, CompileFailure::Invariant(_)),
            "an invariant outranks a resource candidate"
        );
    }

    /// A diagnostic set past the count bound transactionally discards the incomplete
    /// collection (its prefix included) and reports the displacing resource limit; no
    /// partial `Diagnostics` candidate survives.
    #[test]
    fn diagnostic_count_overflow_discards_the_incomplete_collection() {
        let overflowing: Vec<SourceDiagnostic> = (0..=super::MAX_DIAGNOSTIC_COUNT as u32)
            .map(|line| diagnostic(Code::CheckType.as_str(), line + 1))
            .collect();
        assert!(overflowing.len() > super::MAX_DIAGNOSTIC_COUNT);
        let failure = compile_failure(
            overflowing,
            None,
            None,
            CompileStage::PostLoweringValidation,
        );
        let CompileFailure::ResourceLimit(limit) = failure else {
            panic!("an overflowing diagnostic collection is discarded for a resource limit")
        };
        assert_eq!(limit.kind(), super::ResourceLimitKind::DiagnosticCount);
        assert_eq!(limit.limit(), super::MAX_DIAGNOSTIC_COUNT as u64);
    }

    /// An empty diagnostic set with no resource candidate is still the private
    /// empty-boundary invariant, so the resource arm never masks that invariant.
    #[test]
    fn empty_without_a_resource_candidate_stays_an_invariant() {
        let failure = compile_failure(Vec::new(), None, None, CompileStage::Parse);
        assert!(matches!(failure, CompileFailure::Invariant(_)));
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
        let super::SemanticOutcome::ResourceLimit(limit, _) = aggregate else {
            panic!("an aggregate count is a resource limit")
        };
        assert_eq!(limit.kind(), super::ResourceLimitKind::Functions);

        let contradiction =
            super::image_build_outcome(marrow_image::ImageBuildError::InvalidReference("x"));
        assert!(
            matches!(contradiction, super::SemanticOutcome::Invariant(_, _)),
            "a producer-state contradiction is an opaque invariant, not a diagnostic"
        );
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

    /// The diagnostics the first-non-empty-stage projection of a driven pass yields,
    /// or `None` for a clean pass; a resource limit or invariant carries no
    /// diagnostics.
    fn projected(driven: &Driven) -> Option<Vec<SourceDiagnostic>> {
        if !driven.parse.is_empty() {
            return Some(driven.parse.clone());
        }
        if !driven.structural.is_empty() {
            return Some(driven.structural.clone());
        }
        match &driven.semantic {
            SemanticOutcome::Built(_) => None,
            SemanticOutcome::Diagnostics(diagnostics, _) => Some(diagnostics.clone()),
            SemanticOutcome::ResourceLimit(..) | SemanticOutcome::Invariant(..) => Some(Vec::new()),
        }
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
        let driven = drive(&input, TestMode::Include);
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
        let driven = drive(&input, TestMode::Include);

        // The broken module's parse error is recorded.
        assert!(
            driven
                .parse
                .iter()
                .any(|d| d.file().as_str() == "src/broken.mw"),
            "the broken module's parse diagnostic must be recorded: {:?}",
            driven.parse,
        );

        // The valid module was analyzed despite the sibling parse error: its own
        // semantic diagnostic reached the semantic stage.
        let semantic = match &driven.semantic {
            SemanticOutcome::Diagnostics(diagnostics, _) => diagnostics.clone(),
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
            Some(driven.parse.clone())
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
        let driven = drive(&input, TestMode::Include);

        // The broken base module's parse error is recorded.
        assert!(
            driven
                .parse
                .iter()
                .any(|d| d.file().as_str() == "src/base.mw")
        );

        // The dependent module is analyzed, and its cross-references to the absent base
        // module reduce to the ordinary missing-module family — no panic, no invariant,
        // no resource limit.
        let semantic = match &driven.semantic {
            SemanticOutcome::Diagnostics(diagnostics, _) => diagnostics.clone(),
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
        let missing_family = [
            marrow_codes::Code::CheckImport.as_str(),
            marrow_codes::Code::CheckType.as_str(),
        ];
        assert!(
            dependent.iter().all(|d| missing_family.contains(&d.code)),
            "a reference to a parse-failed module reduces to the ordinary missing-reference \
             family (check.import / check.type), never a fabricated or invariant code: {dependent:?}",
        );

        // Byte-identical: the production compile projects only the base module's parse
        // stage.
        assert_eq!(
            compiled(&compile_with_tests(&input)),
            Some(driven.parse.clone())
        );
    }
}
