//! A public image-policy excess is a verdict of the production projection alone.
//!
//! An image count or byte ceiling belongs to the artifact `compile` produces, not to the
//! meaning of the program. So a project that crosses one still checks: the analysis path
//! never encodes, and its snapshot carries no diagnostic, no unavailable fact family, and
//! no refusal of its own. The production projection alone reports the bound.
//!
//! The suite proves that three ways. Behaviourally, the three reachable whole-program
//! ceilings (`Exports`, `Consts`, `Functions`) each refuse `compile` and
//! `compile_with_tests` while their snapshot answers every query. Structurally, every
//! [`ResourceLimitKind`] is classified without a wildcard into the drive-input, the
//! projection-only, and the image-policy set, and the enum's declared variant count is
//! read from the owner's source so a new variant cannot slip past the classification.
//! And over a frozen corpus, the six things `compile` reports about an accepted or
//! refused image — kind, accepted bytes, `ImageId`, exports, tests, and naming — are
//! pinned byte-for-byte against the values the base produced, so deferring the encode
//! past the semantic fence moved nothing a caller can observe.

use std::sync::Arc;

use marrow_compile::{
    ActiveCallOutcome, AnalysisResourceLimit, CompileFailure, CompletionOutcome, Fact,
    InputRevision, MAX_COMPLETION_CANDIDATES, ResourceLimitKind, analyze, compile,
    compile_with_tests,
};
use marrow_project::{FileIdentity, ProjectInput};

#[path = "common/project.rs"]
mod common_project;
#[path = "common/source_projection.rs"]
mod source_projection;

/// The suite's fixtures generate their sources, so they are borrowed into the shared
/// capture helper's `&str` pairs at each call.
fn project(files: &[(&str, String)]) -> ProjectInput {
    project_with_ids(files, None)
}

fn project_with_ids(files: &[(&str, String)], ids: Option<&[u8]>) -> ProjectInput {
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, source)| (*path, source.as_str()))
        .collect();
    common_project::project_with_ids(&borrowed, ids)
}

fn main_file() -> FileIdentity {
    FileIdentity::validate("src/main.mw")
        .expect("canonical test path")
        .0
}

// ---- Reds 1 and 2: an image-policy bound refuses the projection and nothing else.

/// The probe declarations every over-bound fixture ends with, and the offsets the query
/// assertions read. They are private so they do not perturb the export count the fixture
/// is built to cross, and each offset is captured while the source is assembled rather
/// than searched for afterwards.
struct Probes {
    source: String,
    /// Inside the `value` use in `return value` — a local's type is a hover fact.
    local_use: usize,
    /// Inside the `f0` callee name — the only definition family the snapshot covers.
    callee_name: usize,
    /// Between the parentheses of `f0()` — the position an active-call query resolves.
    inside_call: usize,
    /// At the start of the `seen` use — an expression-name position whose in-scope
    /// namespace a completion query enumerates.
    completion_point: usize,
}

fn probes(mut source: String) -> Probes {
    source.push_str("fn probeLocal(): int {\n    const value = 1\n    return ");
    let local_use = source.len() + 1;
    source.push_str("value\n}\n\nfn probeCall(): int {\n    return ");
    let callee_name = source.len() + 1;
    source.push_str("f0(");
    let inside_call = source.len();
    source.push_str(")\n}\n\nfn probeScope(): int {\n    const seen = 1\n    return ");
    let completion_point = source.len();
    source.push_str("seen\n}\n");
    Probes {
        source,
        local_use,
        callee_name,
        inside_call,
        completion_point,
    }
}

/// 257 public functions: one past `MAX_EXPORTS`. Every body returns the same literal, so
/// the constant table holds one entry and the export ceiling is the first bound crossed.
fn over_exports() -> Probes {
    let mut source = String::from("module main\n\n");
    for index in 0..257 {
        source.push_str(&format!("pub fn f{index}(): int {{\n    return 0\n}}\n\n"));
    }
    probes(source)
}

/// 1,025 distinct integer literals: one past `MAX_CONSTS`. The functions stay private and
/// well under the function ceiling, so the constant table is the first bound crossed.
fn over_consts() -> Probes {
    let mut source = String::from("module main\n\n");
    for index in 0..1025u32 {
        let literal = index + 1_000;
        source.push_str(&format!(
            "fn f{index}(): int {{\n    return {literal}\n}}\n\n"
        ));
    }
    probes(source)
}

/// 4,097 functions: one past `MAX_FUNCTIONS`, split across two modules so no single file
/// crosses the per-file outline bound. The count is the whole program's, so the split
/// changes nothing about which ceiling fires.
fn over_functions() -> (Probes, String) {
    let mut main = String::from("module main\n\n");
    for index in 0..2049 {
        main.push_str(&format!("fn f{index}(): int {{\n    return 0\n}}\n\n"));
    }
    let mut extra = String::from("module extra\n\n");
    for index in 2049..4097 {
        extra.push_str(&format!("fn g{index}(): int {{\n    return 0\n}}\n\n"));
    }
    (probes(main), extra)
}

/// Both production entries refuse with exactly `kind`, and the analysis path over the
/// same project yields an ordinary snapshot: no diagnostic, and every query answers
/// without an unavailability. The queries are the whole point — an image-policy bound
/// must be invisible to an editor, because the editor never asked for an image.
fn assert_projection_only(
    kind: ResourceLimitKind,
    probes: &Probes,
    files: &[(&str, String)],
    in_scope_names: u64,
) {
    match compile(&project(files)) {
        Err(CompileFailure::ResourceLimit(limit)) => assert_eq!(limit.kind(), kind),
        other => panic!("expected {kind:?} from compile, got {other:?}"),
    }
    match compile_with_tests(&project(files)) {
        Err(CompileFailure::ResourceLimit(limit)) => assert_eq!(limit.kind(), kind),
        other => panic!("expected {kind:?} from compile_with_tests, got {other:?}"),
    }

    let revision = InputRevision::new(11);
    let snapshot = analyze(Arc::new(project(files)), revision)
        .unwrap_or_else(|_| panic!("an image-policy bound still yields a snapshot for {kind:?}"));
    assert_eq!(
        snapshot.diagnostics(),
        &[],
        "an image-policy bound produces no diagnostic for {kind:?}",
    );
    assert_eq!(snapshot.revision(), revision);

    let file = main_file();
    let Ok(Fact::Present(_)) = snapshot.hover(&file, probes.local_use) else {
        panic!("hover over a local must answer past {kind:?}");
    };
    let Ok(Fact::Present(_)) = snapshot.definition(&file, probes.callee_name) else {
        panic!("definition of a callee must answer past {kind:?}");
    };
    let Ok(Fact::Present(_)) = snapshot.document_symbols(&file) else {
        panic!("the outline must answer past {kind:?}");
    };
    // The candidate cap is a query-local bound on one query's enumerated namespace, and a
    // wide project reaches it whether or not it also crosses an image ceiling. What the
    // image ceiling may never do is make the query unavailable.
    match snapshot.completions(&file, probes.completion_point) {
        Ok(CompletionOutcome::Ready(Fact::Present(_))) => {}
        Ok(CompletionOutcome::Refused(AnalysisResourceLimit::CompletionCandidateCount {
            ..
        })) => assert!(
            in_scope_names > MAX_COMPLETION_CANDIDATES,
            "a completion refusal past {kind:?} must be the candidate cap, not the image bound",
        ),
        _ => panic!("completions must answer past {kind:?}"),
    }
    let Ok(ActiveCallOutcome::Ready(Fact::Present(_))) =
        snapshot.active_call(&file, probes.inside_call)
    else {
        panic!("the active call must answer past {kind:?}");
    };
}

/// Red 1. The export ceiling refuses the image and nothing else.
#[test]
fn an_export_ceiling_refuses_the_projection_only() {
    let probes = over_exports();
    assert_projection_only(
        ResourceLimitKind::Exports,
        &probes,
        &[("src/main.mw", probes.source.clone())],
        // 257 declared functions plus the three probes.
        260,
    );
}

/// Red 2. The constant ceiling refuses the image and nothing else.
#[test]
fn a_constant_ceiling_refuses_the_projection_only() {
    let probes = over_consts();
    assert_projection_only(
        ResourceLimitKind::Consts,
        &probes,
        &[("src/main.mw", probes.source.clone())],
        // 1,025 declared functions plus the three probes.
        1_028,
    );
}

/// Red 2. The function ceiling refuses the image and nothing else.
#[test]
fn a_function_ceiling_refuses_the_projection_only() {
    let (probes, extra) = over_functions();
    assert_projection_only(
        ResourceLimitKind::Functions,
        &probes,
        &[
            ("src/main.mw", probes.source.clone()),
            ("src/extra.mw", extra),
        ],
        // 4,097 declared functions plus the three probes.
        4_100,
    );
}

// ---- Red 3: the wildcard-free partition.

/// Where a resource limit is decided. The three sets are disjoint by construction: a kind
/// belongs to exactly one, and the classification below is a wildcard-free match, so a new
/// variant is a build error here until someone states which owner decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// Decided by drive-input admission, before any stage runs. Reachable from `analyze`.
    DriveInput,
    /// Decided by the diagnostic collector over the retained set. Reachable from
    /// `analyze`.
    Projection,
    /// Decided by `ImageDraft::encode` over the production image. The analysis path never
    /// encodes, so no kind in this set is reachable from `analyze`.
    ImagePolicy,
    /// Decided by the semantic pass over what it must retain to answer later lookups,
    /// before any image is drafted. Reachable from `analyze`.
    Semantic,
}

fn owner_of(kind: ResourceLimitKind) -> Owner {
    match kind {
        ResourceLimitKind::ProjectFiles
        | ResourceLimitKind::ProjectFileBytes
        | ResourceLimitKind::ProjectSourceBytes => Owner::DriveInput,
        ResourceLimitKind::DiagnosticCount | ResourceLimitKind::DiagnosticBytes => {
            Owner::Projection
        }
        ResourceLimitKind::DeclarationLedgerBytes => Owner::Semantic,
        ResourceLimitKind::Strings
        | ResourceLimitKind::StringBytes
        | ResourceLimitKind::Consts
        | ResourceLimitKind::Types
        | ResourceLimitKind::Enums
        | ResourceLimitKind::Collections
        | ResourceLimitKind::Roots
        | ResourceLimitKind::DurableMembers
        | ResourceLimitKind::DurableDepth
        | ResourceLimitKind::IndexComponents
        | ResourceLimitKind::Sites
        | ResourceLimitKind::Functions
        | ResourceLimitKind::CodeBytes
        | ResourceLimitKind::Exports
        | ResourceLimitKind::TestEntries
        | ResourceLimitKind::ImageBytes => Owner::ImagePolicy,
    }
}

/// Every declared kind, classified. The list is checked against the owner's own source
/// below, so it cannot fall behind the enum.
const ALL_KINDS: &[ResourceLimitKind] = &[
    ResourceLimitKind::Strings,
    ResourceLimitKind::Consts,
    ResourceLimitKind::Types,
    ResourceLimitKind::Enums,
    ResourceLimitKind::Collections,
    ResourceLimitKind::Roots,
    ResourceLimitKind::DurableMembers,
    ResourceLimitKind::Sites,
    ResourceLimitKind::Functions,
    ResourceLimitKind::Exports,
    ResourceLimitKind::TestEntries,
    ResourceLimitKind::ImageBytes,
    ResourceLimitKind::StringBytes,
    ResourceLimitKind::CodeBytes,
    ResourceLimitKind::IndexComponents,
    ResourceLimitKind::DurableDepth,
    ResourceLimitKind::DiagnosticCount,
    ResourceLimitKind::DiagnosticBytes,
    ResourceLimitKind::ProjectFiles,
    ResourceLimitKind::ProjectFileBytes,
    ResourceLimitKind::ProjectSourceBytes,
    ResourceLimitKind::DeclarationLedgerBytes,
];

/// The variant names the owner declares, read from its source through the shared
/// production projection, so a `{` or a variant name inside a comment, a string, or a
/// test module cannot widen or truncate the body — and the count is the enum's own truth
/// rather than a number this test remembers. The projection is the one answer to what
/// counts as code; a hand-rolled comment strip here was a weaker second one.
fn declared_kind_names() -> Vec<String> {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/compile.rs"))
        .expect("the owner's source is readable from its own crate");
    let code = source_projection::production_code(&source);
    let header = "pub enum ResourceLimitKind {";
    let start = code
        .find(header)
        .expect("the owner declares ResourceLimitKind")
        + header.len();
    let len = code[start..]
        .find('}')
        .expect("the enum body is brace-delimited");
    code[start..start + len]
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// Red 3. Each kind is classified exactly once, the analysis path can reach exactly the
/// drive-input and projection kinds, and no image-policy kind is among them.
#[test]
fn every_resource_limit_kind_has_exactly_one_owner() {
    let declared = declared_kind_names();
    assert_eq!(
        declared.len(),
        ALL_KINDS.len(),
        "the classified list fell behind the declared enum: {declared:?}",
    );
    for name in &declared {
        assert!(
            ALL_KINDS
                .iter()
                .any(|kind| format!("{kind:?}") == name.as_str()),
            "declared kind {name} is not classified",
        );
    }

    let analysis_reachable: Vec<ResourceLimitKind> = ALL_KINDS
        .iter()
        .copied()
        .filter(|kind| {
            matches!(
                owner_of(*kind),
                Owner::DriveInput | Owner::Projection | Owner::Semantic
            )
        })
        .collect();
    assert_eq!(
        analysis_reachable,
        vec![
            ResourceLimitKind::DiagnosticCount,
            ResourceLimitKind::DiagnosticBytes,
            ResourceLimitKind::ProjectFiles,
            ResourceLimitKind::ProjectFileBytes,
            ResourceLimitKind::ProjectSourceBytes,
            ResourceLimitKind::DeclarationLedgerBytes,
        ],
        "the analysis path reaches exactly the drive-input, diagnostic, and semantic bounds",
    );
    assert!(
        ALL_KINDS
            .iter()
            .filter(|kind| owner_of(**kind) == Owner::ImagePolicy)
            .count()
            > 0,
        "the image-policy set is non-empty",
    );
}

// ---- Red 4: the frozen corpus.

/// A distinct 16-byte identity rendered as 32 lowercase hex, seeded by `n`.
fn hexid(n: u64) -> String {
    format!("{n:032x}")
}

/// The durable identity ledger the `Wide` resource and its `^wide` root need: the
/// application, the product, one identity per declared field, the root, and its key
/// column.
fn item_ids() -> Vec<u8> {
    let mut out = String::from("marrow ids v0\nmachine-written by marrow; do not edit\n");
    for (seed, anchor) in [
        "application .",
        "product Wide",
        "field Wide.tag",
        "field Wide.note",
        "root wide",
        "key wide.id",
    ]
    .iter()
    .enumerate()
    {
        out.push_str(&format!("id {anchor} {}\n", hexid(seed as u64 + 1)));
    }
    out.push_str("high-water 0\nend\n");
    out.into_bytes()
}

/// One frozen corpus entry: a project, and the identity ledger it needs (if any).
struct Corpus {
    name: &'static str,
    files: Vec<(&'static str, String)>,
    ids: Option<Vec<u8>>,
}

fn corpus() -> Vec<Corpus> {
    let (over_functions_main, over_functions_extra) = over_functions();
    vec![
        Corpus {
            name: "unit",
            files: vec![(
                "src/main.mw",
                "module main\n\npub fn f(): int {\n    return 1\n}\n".to_string(),
            )],
            ids: None,
        },
        Corpus {
            name: "multi-module",
            files: vec![
                (
                    "src/main.mw",
                    "module main\n\npub fn f(): int {\n    return 1\n}\n".to_string(),
                ),
                (
                    "src/other.mw",
                    "module other\n\npub fn g(): int {\n    return 2\n}\n\nfn h(): int {\n    return 3\n}\n"
                        .to_string(),
                ),
            ],
            ids: None,
        },
        Corpus {
            name: "tests",
            files: vec![(
                "src/main.mw",
                "module main\n\npub fn f(): int {\n    return 1\n}\n\ntest \"one\" {\n    assert 1 + 1 == 2\n}\n\ntest \"two\" {\n    assert f() == 1\n}\n"
                    .to_string(),
            )],
            ids: None,
        },
        Corpus {
            name: "durable",
            files: vec![(
                "src/main.mw",
                "module main\n\nresource Wide {\n    required tag: int\n    note: int\n}\n\nstore ^wide[id: int]: Wide\n\npub fn noop(): int {\n    return 0\n}\n"
                    .to_string(),
            )],
            ids: Some(item_ids()),
        },
        Corpus {
            name: "over-exports",
            files: vec![("src/main.mw", over_exports().source)],
            ids: None,
        },
        Corpus {
            name: "over-consts",
            files: vec![("src/main.mw", over_consts().source)],
            ids: None,
        },
        Corpus {
            name: "over-functions",
            files: vec![
                ("src/main.mw", over_functions_main.source),
                ("src/extra.mw", over_functions_extra),
            ],
            ids: None,
        },
    ]
}

/// Everything a caller observes about the image `compile_with_tests` accepted or
/// refused, read through the typed public surface rather than a `{:?}` rendering: the
/// refused kind and its bound, or the accepted byte count, the `ImageId`, the export
/// directory, the test directory, and the durable naming join. Diagnostics are
/// deliberately absent — phase continuation changes diagnostic sets by design, while none
/// of these six may move.
///
/// A `Debug` derive is not part of this contract. Pinning rendered `{:?}` text made every
/// derive on `ExportEntry`, `TestEntry`, `ExportId`, or `LedgerIdBytes` a golden change,
/// so a formatting edit read as an image-projection regression and a real regression read
/// as formatting. Each fact is compared as itself instead.
#[derive(Debug, PartialEq, Eq)]
enum ImageDigest {
    /// `(kind name, bound)`. The kind is rendered by name only so the frozen table stays
    /// a const; the enum's own variant set is pinned separately by red 3.
    Refused(String, u64),
    Accepted {
        bytes: usize,
        image_id: String,
        /// `(module, item, export id in hex)`.
        exports: Vec<(String, String, String)>,
        /// `(title, module, file, line, column)`.
        tests: Vec<(String, String, String, u32, u32)>,
        /// The compiler-owned ledger-id-to-spelling join. It publishes no typed
        /// enumeration — `demand_sentence` is its whole public surface, and it needs a
        /// demand set no caller of `compile_with_tests` holds — so this one field is
        /// still its rendering. It is the only golden a derive can churn.
        naming: String,
    },
    /// No corpus entry may report a diagnostic or an invariant; both are named so a
    /// failure says which happened.
    Diagnostics,
    Invariant,
}

fn image_digest(input: &ProjectInput) -> ImageDigest {
    match compile_with_tests(input) {
        Ok(compiled) => ImageDigest::Accepted {
            bytes: compiled.image.bytes.len(),
            image_id: compiled.image.image_id.to_hex(),
            exports: compiled
                .exports
                .iter()
                .map(|export| {
                    (
                        export.module.clone(),
                        export.item.clone(),
                        export.id.to_hex(),
                    )
                })
                .collect(),
            tests: compiled
                .tests
                .iter()
                .map(|test| {
                    (
                        test.name.clone(),
                        test.module.clone(),
                        test.file.clone(),
                        test.line,
                        test.column,
                    )
                })
                .collect(),
            naming: format!("{:?}", compiled.naming),
        },
        Err(CompileFailure::ResourceLimit(limit)) => {
            ImageDigest::Refused(format!("{:?}", limit.kind()), limit.limit())
        }
        Err(CompileFailure::Diagnostics(_)) => ImageDigest::Diagnostics,
        Err(CompileFailure::Invariant(_)) => ImageDigest::Invariant,
    }
}

/// One frozen corpus entry, in the const-constructible shape [`ImageDigest`] is compared
/// against.
struct FrozenDigest {
    name: &'static str,
    bytes: usize,
    image_id: &'static str,
    /// `(module, item, export id in hex)`.
    exports: &'static [(&'static str, &'static str, &'static str)],
    /// `(title, module, file, line, column)`.
    tests: &'static [(&'static str, &'static str, &'static str, u32, u32)],
    naming: &'static str,
}

impl FrozenDigest {
    fn expected(&self) -> ImageDigest {
        ImageDigest::Accepted {
            bytes: self.bytes,
            image_id: self.image_id.to_string(),
            exports: self
                .exports
                .iter()
                .map(|(module, item, id)| (module.to_string(), item.to_string(), id.to_string()))
                .collect(),
            tests: self
                .tests
                .iter()
                .map(|(name, module, file, line, column)| {
                    (
                        name.to_string(),
                        module.to_string(),
                        file.to_string(),
                        *line,
                        *column,
                    )
                })
                .collect(),
            naming: self.naming.to_string(),
        }
    }
}

const F_ID: &str = "13e67fafae418c8c50bbc520d97f63360a8ad63c1ad066300cfd15b09a858ffc";
const G_ID: &str = "ef57adc8aece6d659330e3615306d07f90ea88b9b47f9c2b52c86e35b140bba8";
const NOOP_ID: &str = "acc0e4892dc9149eef25a215d6fd6304ea6382d717452ed67a7662bb77995f19";

/// The empty durable join, as the one field with no typed public projection renders it.
///
/// `DurableNaming` publishes the two demand renderings and nothing that enumerates the
/// join itself, so a derived `Debug` is what a golden can read here. It is not a stable
/// surface, and it is pinned knowing that: this string also carries `LedgerIdBytes`'s own
/// derived `Debug` and the crate-private `PathSigil`'s variant names, so a representation
/// change to either rewrites these goldens without any change to what `compile` reports
/// about an image — a diff to read as churn and regenerate, not as a contract change.
///
/// Adding a typed projection to spell the join would settle that, and the only caller it
/// would ever have is this file. The crate does not carry a public entry point for a test
/// to read, so the churn is the cheaper of the two.
const NO_NAMING: &str = "DurableNaming { by_id: {} }";

/// The digests the base produced, pinned. Regenerating them is not a repair: a change
/// here is a change to what `compile` reports about an image, and it is a contract change
/// reviewed as one.
const FROZEN_ACCEPTED: &[FrozenDigest] = &[
    FrozenDigest {
        name: "unit",
        bytes: 241,
        image_id: "75054e7c9d536984fe524c510504a1f840234e369a5e4fb66745a70708b67a7d",
        exports: &[("main", "f", F_ID)],
        tests: &[],
        naming: NO_NAMING,
    },
    FrozenDigest {
        name: "multi-module",
        bytes: 397,
        image_id: "9720941f296e9fa79d36ca8d8eb05049fd6d76e828ed2cff51f0fc4883a554bb",
        exports: &[("main", "f", F_ID), ("other", "g", G_ID)],
        tests: &[],
        naming: NO_NAMING,
    },
    FrozenDigest {
        name: "tests",
        bytes: 462,
        image_id: "c614971142d05f7e18acc3c3f32207a5575f5beda03eb47da4a77df9c040474b",
        exports: &[("main", "f", F_ID)],
        tests: &[
            ("one", "main", "src/main.mw", 7, 6),
            ("two", "main", "src/main.mw", 11, 6),
        ],
        naming: NO_NAMING,
    },
    FrozenDigest {
        name: "durable",
        bytes: 430,
        image_id: "ab026b325171aef8996b70df12d79574a09a6ae7e7ee9f3e6c9366503e6a428b",
        exports: &[("main", "noop", NOOP_ID)],
        tests: &[],
        naming: "DurableNaming { by_id: {LedgerIdBytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, \
                 0, 0, 0, 0, 3]): (Child, \"tag\"), LedgerIdBytes([0, 0, 0, 0, 0, 0, 0, 0, \
                 0, 0, 0, 0, 0, 0, 0, 4]): (Child, \"note\"), LedgerIdBytes([0, 0, 0, 0, 0, \
                 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5]): (Root, \"wide\")} }",
    },
];

/// The corpus entries the projection refuses, and the exact bound each reports.
const FROZEN_REFUSED: &[(&str, &str, u64)] = &[
    ("over-exports", "Exports", 256),
    ("over-consts", "Consts", 1024),
    ("over-functions", "Functions", 4096),
];

/// Red 4. Over the frozen corpus, the six things `compile` reports about an image are
/// identical to the values the base produced, fact by fact.
#[test]
fn the_image_projection_is_byte_identical_to_the_base() {
    let expected: Vec<(String, ImageDigest)> = FROZEN_ACCEPTED
        .iter()
        .map(|frozen| (frozen.name.to_string(), frozen.expected()))
        .chain(FROZEN_REFUSED.iter().map(|(name, kind, bound)| {
            (
                name.to_string(),
                ImageDigest::Refused(kind.to_string(), *bound),
            )
        }))
        .collect();
    let actual: Vec<(String, ImageDigest)> = corpus()
        .into_iter()
        .map(|entry| {
            let input = project_with_ids(&entry.files, entry.ids.as_deref());
            (entry.name.to_string(), image_digest(&input))
        })
        .collect();
    assert_eq!(
        actual.len(),
        expected.len(),
        "every corpus entry is pinned exactly once",
    );
    for (actual, expected) in actual.iter().zip(&expected) {
        assert_eq!(
            actual.0, expected.0,
            "the corpus order and the frozen order must agree",
        );
        assert_eq!(
            actual.1, expected.1,
            "the image projection moved for `{}`",
            actual.0,
        );
    }
}
