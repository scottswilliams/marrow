//! Local source dependencies: the `[dependencies]` schema, the alias-rooted
//! module names a dependency contributes, and the one budget every captured tree
//! shares.

use marrow_codes::Code;
use marrow_project::{
    CaptureBound, CaptureError, CaptureErrorKind, CaptureLimits, CapturedDependency, CapturedFile,
    DependencyAlias, DependencyAliasReason, DependencyPathReason, DependencyShapeFault, Manifest,
    ManifestErrorKind, ProjectInput, SourceOrigin,
};

const EMPTY_LEDGER: &[u8] =
    b"marrow ids v0\nmachine-written by marrow; do not edit\nhigh-water 0\nend\n";
const LIBRARY_LEDGER: &[u8] = b"marrow ids v0\nmachine-written by marrow; do not edit\n\
                                id product Pair 03030303030303030303030303030303\n\
                                high-water 0\nend\n";

fn manifest(source: &str) -> Manifest {
    Manifest::parse(source).expect("valid manifest")
}

fn one_dependency() -> Manifest {
    manifest("edition = \"2026\"\n\n[dependencies]\ngraphtext = { path = \"../graphtext\" }\n")
}

fn alias(spelling: &str) -> DependencyAlias {
    DependencyAlias::parse(spelling).expect("valid alias")
}

fn root_file(path: &str, body: &str) -> CapturedFile {
    CapturedFile::new(path.to_string(), body.as_bytes().to_vec())
}

fn dependency_file(spelling: &str, path: &str, body: &str) -> CapturedFile {
    CapturedFile::in_dependency(alias(spelling), path.to_string(), body.as_bytes().to_vec())
}

fn capture(
    manifest: &Manifest,
    files: Vec<CapturedFile>,
    dependencies: &[CapturedDependency<'_>],
) -> Result<ProjectInput, CaptureError> {
    marrow_project::capture_origins(manifest, files, None, dependencies, &CaptureLimits::DEFAULT)
}

fn module_names(input: &ProjectInput) -> Vec<String> {
    input
        .modules()
        .iter()
        .map(|module| module.module().as_str().to_string())
        .collect()
}

// --- Manifest schema ----------------------------------------------------------

#[test]
fn a_dependency_table_parses_into_alias_ordered_entries() {
    let manifest = manifest(
        "edition = \"2026\"\n\n[dependencies]\nzeta = { path = \"../zeta\" }\n\
         alpha = { path = \"vendor/alpha\" }\n",
    );
    let declared: Vec<(&str, &str)> = manifest
        .dependencies()
        .iter()
        .map(|dependency| (dependency.alias().as_str(), dependency.path().as_str()))
        .collect();
    assert_eq!(
        declared,
        vec![("alpha", "vendor/alpha"), ("zeta", "../zeta")],
        "entries are held in alias order, whatever order the manifest spells them in"
    );
}

#[test]
fn a_misshapen_dependency_table_is_a_closed_schema_fault() {
    let cases: Vec<(&str, DependencyShapeFault)> = vec![
        ("dependencies = 1\n", DependencyShapeFault::NotATable),
        (
            "[dependencies]\nlib = \"../lib\"\n",
            DependencyShapeFault::EntryNotATable,
        ),
        (
            "[dependencies]\nlib = {}\n",
            DependencyShapeFault::MissingPath,
        ),
        (
            "[dependencies]\nlib = { path = 1 }\n",
            DependencyShapeFault::PathNotString,
        ),
        (
            "[dependencies]\nlib = { path = \"../lib\", version = \"1\" }\n",
            DependencyShapeFault::UnknownKey {
                key: "version".to_string(),
            },
        ),
    ];
    for (tail, fault) in cases {
        let source = format!("edition = \"2026\"\n{tail}");
        let error = Manifest::parse(&source).expect_err("a misshapen table rejects");
        assert_eq!(
            error.code(),
            Code::ConfigInvalid,
            "the table's shape belongs to the closed manifest schema: {tail}"
        );
        match error.kind() {
            ManifestErrorKind::DependencyShape { fault: actual, .. } => {
                assert_eq!(actual, &fault, "{tail}");
            }
            other => panic!("expected a shape fault for {tail}, got {other:?}"),
        }
    }
}

#[test]
fn an_alias_that_is_not_an_identifier_is_refused() {
    let long = "a".repeat(65);
    let cases = vec![
        ("1lib", DependencyAliasReason::NotIdentifier),
        ("my-lib", DependencyAliasReason::NotIdentifier),
        ("my.lib", DependencyAliasReason::NotIdentifier),
        ("", DependencyAliasReason::NotIdentifier),
        (
            long.as_str(),
            DependencyAliasReason::TooLong {
                limit: 64,
                actual: 65,
            },
        ),
    ];
    for (spelling, reason) in cases {
        let source = format!(
            "edition = \"2026\"\n\n[dependencies]\n\"{spelling}\" = {{ path = \"../l\" }}\n"
        );
        let error = Manifest::parse(&source).expect_err("an unusable alias rejects");
        assert_eq!(error.code(), Code::ProjectDependencyAlias, "{spelling}");
        match error.kind() {
            ManifestErrorKind::DependencyAlias {
                alias,
                reason: actual,
            } => {
                assert_eq!(alias, spelling);
                assert_eq!(actual, &reason, "{spelling}");
            }
            other => panic!("expected an alias fault for `{spelling}`, got {other:?}"),
        }
    }
}

#[test]
fn a_path_that_is_not_a_canonical_relative_spelling_is_refused() {
    let long = format!("../{}", "a".repeat(4096));
    let cases = vec![
        ("/abs/lib", DependencyPathReason::Absolute),
        (".", DependencyPathReason::NonCanonical),
        ("", DependencyPathReason::NonCanonical),
        ("./lib", DependencyPathReason::NonCanonical),
        ("lib//x", DependencyPathReason::NonCanonical),
        // A `..` after a named segment would re-enter the tree from above and give
        // one directory two spellings.
        ("lib/../other", DependencyPathReason::NonCanonical),
        (
            long.as_str(),
            DependencyPathReason::TooLong {
                limit: 4096,
                actual: 4099,
            },
        ),
    ];
    for (spelling, reason) in cases {
        let source =
            format!("edition = \"2026\"\n\n[dependencies]\nlib = {{ path = \"{spelling}\" }}\n");
        let error = Manifest::parse(&source).expect_err("an unusable path rejects");
        assert_eq!(error.code(), Code::ProjectDependencyPath, "{spelling}");
        match error.kind() {
            ManifestErrorKind::DependencyPath {
                path,
                reason: actual,
                ..
            } => {
                assert_eq!(path, spelling);
                assert_eq!(actual, &reason, "{spelling}");
            }
            other => panic!("expected a path fault for `{spelling}`, got {other:?}"),
        }
    }
    // A leading run of `..` is how a sibling library is named, and is admitted.
    let sibling =
        manifest("edition = \"2026\"\n\n[dependencies]\nlib = { path = \"../../lib\" }\n");
    assert_eq!(sibling.dependencies()[0].path().as_str(), "../../lib");
}

#[test]
fn one_alias_cannot_be_declared_twice() {
    // TOML itself owns duplicate-key rejection, so the manifest never holds two
    // entries for one alias.
    let error = Manifest::parse(
        "edition = \"2026\"\n\n[dependencies]\nlib = { path = \"../a\" }\nlib = { path = \"../b\" }\n",
    )
    .expect_err("a duplicate alias rejects");
    assert_eq!(error.code(), Code::ConfigInvalid);
    assert_eq!(error.kind(), &ManifestErrorKind::Malformed);
}

// --- Capture ------------------------------------------------------------------

#[test]
fn a_dependency_module_is_rooted_at_the_consumers_alias() {
    let manifest = one_dependency();
    let input = capture(
        &manifest,
        vec![
            root_file("src/main.mw", "pub fn main()\n"),
            dependency_file("graphtext", "src/text.mw", "module text\n"),
            dependency_file("graphtext", "src/parse/pair.mw", "module parse::pair\n"),
        ],
        &[CapturedDependency::new(&alias("graphtext"), None)],
    )
    .expect("a two-tree capture");

    assert_eq!(
        module_names(&input),
        ["main", "graphtext.parse.pair", "graphtext.text"],
        "the root's modules come first, then the dependency's under its alias"
    );
    assert_eq!(
        input.modules()[1].identity().as_str(),
        "src/parse/pair.mw",
        "an identity stays relative to the tree it came from"
    );
    assert_eq!(
        input.origins(),
        [
            SourceOrigin::Root,
            SourceOrigin::Dependency(alias("graphtext"))
        ]
    );
    assert_eq!(input.modules()[0].origin(), &SourceOrigin::Root);
    assert_eq!(
        input.modules()[1].origin(),
        &SourceOrigin::Dependency(alias("graphtext"))
    );
}

#[test]
fn the_same_two_trees_capture_identically_whatever_order_they_arrive_in() {
    let manifest = one_dependency();
    let ordered = capture(
        &manifest,
        vec![
            root_file("src/a.mw", "a"),
            root_file("src/b.mw", "b"),
            dependency_file("graphtext", "src/text.mw", "t"),
        ],
        &[CapturedDependency::new(&alias("graphtext"), None)],
    )
    .expect("ordered capture");
    let shuffled = capture(
        &manifest,
        vec![
            dependency_file("graphtext", "src/text.mw", "t"),
            root_file("src/b.mw", "b"),
            root_file("src/a.mw", "a"),
        ],
        &[CapturedDependency::new(&alias("graphtext"), None)],
    )
    .expect("shuffled capture");
    assert_eq!(ordered, shuffled);
}

#[test]
fn an_alias_cannot_occupy_a_root_modules_first_segment() {
    // The alias-rooted path `graphtext.text` would otherwise name both the root's
    // own `src/graphtext/text.mw` and the dependency's `src/text.mw`.
    let manifest = one_dependency();
    let error = capture(
        &manifest,
        vec![
            root_file("src/graphtext/text.mw", "module graphtext::text\n"),
            dependency_file("graphtext", "src/text.mw", "module text\n"),
        ],
        &[CapturedDependency::new(&alias("graphtext"), None)],
    )
    .expect_err("a colliding alias refuses");
    assert_eq!(error.code(), Code::ProjectDependencyAlias);
    match error.kind() {
        CaptureErrorKind::DependencyAlias {
            alias: offender,
            reason: DependencyAliasReason::RootModuleCollision { module },
        } => {
            assert_eq!(offender.as_str(), "graphtext");
            assert_eq!(module.as_str(), "graphtext.text");
        }
        other => panic!("expected an alias collision, got {other:?}"),
    }
}

#[test]
fn the_captured_trees_must_be_exactly_the_declared_ones() {
    let manifest = one_dependency();
    let other = alias("other");
    let graphtext = alias("graphtext");
    let cases: Vec<(&str, Vec<CapturedDependency<'_>>, DependencyAliasReason)> = vec![
        (
            "none captured",
            Vec::new(),
            DependencyAliasReason::Uncaptured,
        ),
        (
            "an undeclared tree",
            vec![
                CapturedDependency::new(&graphtext, None),
                CapturedDependency::new(&other, None),
            ],
            DependencyAliasReason::Undeclared,
        ),
        (
            "one tree twice",
            vec![
                CapturedDependency::new(&graphtext, None),
                CapturedDependency::new(&graphtext, None),
            ],
            DependencyAliasReason::Duplicate,
        ),
    ];
    for (label, dependencies, reason) in cases {
        let error = capture(&manifest, Vec::new(), &dependencies)
            .expect_err("an unmatched origin set refuses");
        assert_eq!(error.code(), Code::ProjectDependencyAlias, "{label}");
        match error.kind() {
            CaptureErrorKind::DependencyAlias { reason: actual, .. } => {
                assert_eq!(actual, &reason, "{label}");
            }
            other => panic!("expected an alias fault for {label}, got {other:?}"),
        }
    }
}

#[test]
fn two_trees_may_hold_the_same_identity() {
    let manifest = one_dependency();
    let input = capture(
        &manifest,
        vec![
            root_file("src/main.mw", "root"),
            dependency_file("graphtext", "src/main.mw", "library"),
        ],
        &[CapturedDependency::new(&alias("graphtext"), None)],
    )
    .expect("one identity per tree is not a collision");
    assert_eq!(module_names(&input), ["main", "graphtext.main"]);
}

#[test]
fn a_bound_crossed_only_in_the_sum_of_two_trees_refuses_once() {
    let manifest = one_dependency();
    let graphtext = alias("graphtext");
    let files = vec![
        root_file("src/a.mw", "aa"),
        root_file("src/b.mw", "bb"),
        dependency_file("graphtext", "src/text.mw", "tt"),
    ];
    let dependencies = [CapturedDependency::new(&graphtext, None)];

    let limits = CaptureLimits::new(2, 1 << 20, 64 << 20);
    let error =
        marrow_project::capture_origins(&manifest, files.clone(), None, &dependencies, &limits)
            .expect_err("three files over a two-file bound refuses");
    assert_eq!(error.code(), Code::ProjectCaptureLimit);
    assert_eq!(
        error.kind(),
        &CaptureErrorKind::CaptureLimit {
            bound: CaptureBound::FileCount,
            limit: 2,
            actual: 3,
        },
        "the file count spans every tree, so neither tree alone reaches the bound"
    );

    let limits = CaptureLimits::new(4096, 1 << 20, 5);
    let error = marrow_project::capture_origins(&manifest, files, None, &dependencies, &limits)
        .expect_err("six bytes over a five-byte bound refuses");
    assert_eq!(
        error.kind(),
        &CaptureErrorKind::CaptureLimit {
            bound: CaptureBound::TotalBytes,
            limit: 5,
            actual: 6,
        },
    );
}

#[test]
fn each_tree_keeps_its_own_identity_ledger() {
    let manifest = one_dependency();
    let graphtext = alias("graphtext");
    let input = marrow_project::capture_origins(
        &manifest,
        vec![dependency_file("graphtext", "src/text.mw", "t")],
        Some(EMPTY_LEDGER),
        &[CapturedDependency::new(&graphtext, Some(LIBRARY_LEDGER))],
        &CaptureLimits::DEFAULT,
    )
    .expect("a two-ledger capture");

    let root = input
        .identity_ledger()
        .expect("the root committed an artifact");
    assert_eq!(
        input.identity_ledger_for(&SourceOrigin::Root),
        Some(root),
        "the root accessor and the per-origin accessor name one ledger"
    );
    let library = input
        .identity_ledger_for(&SourceOrigin::Dependency(graphtext))
        .expect("the dependency committed an artifact");
    assert_ne!(
        root, library,
        "a dependency's declarations resolve against the ledger its own tree committed"
    );
}
