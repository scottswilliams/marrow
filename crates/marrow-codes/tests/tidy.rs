//! Standing repository gates: the workspace shape, the retired legacy families,
//! and the small set of absences no type or lint can express.
//!
//! 1. The workspace members are exactly the retained set, and `crates/` holds
//!    exactly those crates.
//! 2. The Cargo DAG respects the trust boundaries: the VM never decodes the
//!    image container, the analysis owners cannot reach a runtime or store
//!    crate, and the raw byte engine is consumed only through the path kernel.
//! 3. No tracked file names a forbidden legacy family as a Rust identifier or a
//!    crate reference.
//! 4. The absence scans in [`ABSENCE_SCANS`] hold.
//!
//! Every scan here is one substring search over tracked `.rs` files. A gate that
//! needed more than that — a Rust lexer, a call graph, an occurrence count —
//! would be policing something a visibility boundary, a Cargo edge, or a
//! behavioral test should carry instead.

use std::fs;
use std::path::Path;

#[path = "common/workspace.rs"]
mod workspace;

use workspace::{tracked_paths, workspace_root};

/// The exact set of workspace packages the workspace retains, which is also the
/// exact set of directories under `crates/`.
const RETAINED_MEMBERS: &[&str] = &[
    "marrow",
    "marrow-codes",
    "marrow-compile",
    "marrow-fs-journal",
    "marrow-image",
    "marrow-kernel",
    "marrow-lifecycle",
    "marrow-local-wire",
    "marrow-lsp",
    "marrow-project",
    "marrow-project-fs",
    "marrow-runner",
    "marrow-store",
    "marrow-syntax",
    "marrow-temporal",
    "marrow-verify",
    "marrow-vm",
];

#[test]
fn workspace_members_are_exactly_the_retained_set() {
    let manifest = fs::read_to_string(workspace_root().join("Cargo.toml"))
        .expect("read the workspace manifest");
    let listing = manifest
        .split_once("members = [")
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(body, _)| body)
        .expect("the workspace manifest lists its members");
    let mut declared: Vec<&str> = listing
        .split(',')
        .map(|entry| entry.trim().trim_matches('"'))
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.trim_start_matches("crates/"))
        .collect();
    declared.sort_unstable();

    let mut present: Vec<String> = fs::read_dir(workspace_root().join("crates"))
        .expect("read the crates directory")
        .flatten()
        .filter(|entry| entry.path().join("Cargo.toml").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    present.sort();

    let mut expected: Vec<&str> = RETAINED_MEMBERS.to_vec();
    expected.sort_unstable();

    assert_eq!(declared, expected, "the declared members drifted");
    assert_eq!(present, expected, "the crates directory drifted");
}

/// One workspace member's internal dependency edges.
struct PackageEdges {
    name: &'static str,
    /// `(dependency name, is_dev)` for every workspace-internal `marrow*` edge.
    edges: Vec<(String, bool)>,
}

/// Every member's `marrow*` edges, read from the dependency tables of its own
/// manifest. A path edge is spelled there or it does not exist, so resolution
/// adds nothing this gate needs.
fn workspace_edges() -> Vec<PackageEdges> {
    RETAINED_MEMBERS
        .iter()
        .map(|name| {
            let manifest = workspace_root()
                .join("crates")
                .join(name)
                .join("Cargo.toml");
            let text = fs::read_to_string(&manifest)
                .unwrap_or_else(|_| panic!("read {}", manifest.display()));
            let mut edges = Vec::new();
            let mut is_dev = false;
            for line in text.lines().map(str::trim) {
                if let Some(section) = line.strip_prefix('[') {
                    is_dev = section.starts_with("dev-dependencies");
                    continue;
                }
                let Some((key, _)) = line.split_once('=') else {
                    continue;
                };
                let key = key.trim();
                if key.starts_with("marrow") {
                    edges.push((key.to_owned(), is_dev));
                }
            }
            PackageEdges { name, edges }
        })
        .collect()
}

/// Trust-boundary Cargo-DAG gates: the VM never decodes the image
/// container, the compiler cannot reach the verifier/VM/kernel/store (it opens
/// no store and mints no VerifiedImage), and the raw byte engine is consumed
/// only through the path kernel. These edges are architecture, not convenience;
/// this test exists to make a regression conspicuous.
#[test]
fn cargo_dag_respects_the_trust_boundaries() {
    let packages = workspace_edges();
    let find = |name: &str| {
        packages
            .iter()
            .find(|package| package.name == name)
            .unwrap_or_else(|| panic!("workspace member {name} missing from metadata"))
    };

    // marrow-vm consumes only sealed images: no production edge to marrow-image
    // (a dev-dependency for building test artifacts is permitted).
    let vm = find("marrow-vm");
    assert!(
        !vm.edges
            .iter()
            .any(|(dep, is_dev)| dep == "marrow-image" && !is_dev),
        "marrow-vm must not have a production dependency on marrow-image"
    );

    // The editor analysis floor (revisioned `AnalysisSnapshot`, hover/definition
    // facts, checked formatting) is owned by the compiler, syntax, and pure
    // project-input crates. None may reach a runtime or store crate: analysis is a
    // pure function of captured source, and the downstream LSP consumes its facts
    // without acquiring an execution or storage edge through them. The reciprocal
    // clause holds too: no analysis owner reaches the LSP transport crate, so the
    // compiler/syntax/project owners stay upstream of tooling.
    let runtime_and_store = [
        "marrow-verify",
        "marrow-vm",
        "marrow-kernel",
        "marrow-store",
        "marrow-runner",
        "marrow-lsp",
    ];
    for owner in ["marrow-compile", "marrow-syntax", "marrow-project"] {
        let package = find(owner);
        for forbidden in runtime_and_store {
            assert!(
                !package.edges.iter().any(|(dep, _)| dep == forbidden),
                "{owner} is an analysis owner and must not depend on {forbidden}"
            );
        }
    }

    // The language server consumes published facts and the physical project adapter
    // only. It reconstructs no runtime, storage, image, verification, or wire
    // semantics, so it has no edge into any of those owners. Its allowed production
    // edges are the fact-surface consumers plus the code registry.
    let lsp = find("marrow-lsp");
    const LSP_FORBIDDEN: &[&str] = &[
        "marrow-kernel",
        "marrow-store",
        "marrow-vm",
        "marrow-image",
        "marrow-verify",
        "marrow-local-wire",
        "marrow-runner",
    ];
    for (dep, _) in &lsp.edges {
        assert!(
            !LSP_FORBIDDEN.contains(&dep.as_str()),
            "marrow-lsp reconstructs no semantics and must not depend on {dep}"
        );
    }
    // The LSP names the pure project boundary through the CAP facade's re-exports, not a
    // direct edge: `marrow-project` is deliberately absent.
    const LSP_ALLOWED: &[&str] = &[
        "marrow-codes",
        "marrow-compile",
        "marrow-project-fs",
        "marrow-syntax",
    ];
    assert!(
        !lsp.edges.iter().any(|(dep, _)| dep == "marrow-project"),
        "marrow-lsp must reach project facts through marrow-project-fs, not a direct marrow-project edge"
    );
    for (dep, is_dev) in &lsp.edges {
        if *is_dev {
            continue;
        }
        assert!(
            LSP_ALLOWED.contains(&dep.as_str()),
            "marrow-lsp has an unexpected production edge to {dep}"
        );
    }

    // marrow-local-wire is the pure protocol owner: framing, limits, the closed
    // grammar, and canonical JSON with no execution, storage, or process edge. Its
    // only internal dependency is the diagnostic-code registry, so a regression that
    // reached the VM, verifier, kernel, image, or store from the wire crate — the
    // exact coupling the pure-crate boundary forbids — is conspicuous here.
    let wire = find("marrow-local-wire");
    for (dep, _) in &wire.edges {
        assert_eq!(
            dep, "marrow-codes",
            "marrow-local-wire must depend on marrow-codes alone; found an edge to {dep}"
        );
    }

    // The physical project adapter is the sole filesystem owner below the tool
    // consumers. It depends on the pure project-input owner, the diagnostic-code
    // registry, and the sole descriptor-rooted publication owner — `.marrow/ids`
    // is a project-root artifact, so publishing it belongs here rather than to a
    // store-lifecycle owner or a second rename/sync/recovery model.
    let project_fs = find("marrow-project-fs");
    let mut project_fs_edges: Vec<(String, bool)> = project_fs.edges.clone();
    project_fs_edges.sort();
    assert_eq!(
        project_fs_edges,
        [
            ("marrow-codes".to_string(), false),
            ("marrow-fs-journal".to_string(), false),
            ("marrow-project".to_string(), false),
        ],
        "marrow-project-fs must depend only on marrow-project, marrow-codes, and \
         marrow-fs-journal"
    );
    let cli = find("marrow");
    assert!(
        cli.edges
            .iter()
            .any(|(dep, is_dev)| dep == "marrow-project-fs" && !is_dev),
        "marrow must consume marrow-project-fs in production"
    );
    // The CLI and the language server are the two tool consumers of the shared physical
    // adapter; no other crate may reach it.
    const PROJECT_FS_CONSUMERS: &[&str] = &["marrow", "marrow-lsp"];
    for package in &packages {
        if PROJECT_FS_CONSUMERS.contains(&package.name) {
            continue;
        }
        assert!(
            !package
                .edges
                .iter()
                .any(|(dep, _)| dep == "marrow-project-fs"),
            "{} must not consume marrow-project-fs before its separately owned integration",
            package.name
        );
    }

    // The runner executes storeless exports only: it consumes the verifier and VM
    // but never compiles source, so it has no production edge to the compiler (a
    // test-only dev edge, to build fixture images, is permitted). The store gate
    // below independently keeps it off the raw engine. Its production edges stay
    // within the wire/image/verify/vm/temporal/codes set.
    let runner = find("marrow-runner");
    const RUNNER_ALLOWED: &[&str] = &[
        "marrow-local-wire",
        "marrow-image",
        "marrow-verify",
        "marrow-vm",
        "marrow-lifecycle",
        "marrow-temporal",
        "marrow-codes",
    ];
    for (dep, is_dev) in &runner.edges {
        if *is_dev {
            continue;
        }
        assert!(
            RUNNER_ALLOWED.contains(&dep.as_str()),
            "marrow-runner has an unexpected production edge to {dep}"
        );
    }

    // The raw byte engine has exactly one production consumer: the path kernel. The VM's
    // private commit-fault tests implement a fault-injecting engine double against the
    // engine traits, a dev-only edge that never reaches a production build.
    for package in &packages {
        let production_store = package
            .edges
            .iter()
            .any(|(dep, is_dev)| dep == "marrow-store" && !is_dev);
        let dev_store = package
            .edges
            .iter()
            .any(|(dep, is_dev)| dep == "marrow-store" && *is_dev);
        if package.name == "marrow-kernel" {
            assert!(
                production_store,
                "marrow-kernel is the byte engine's consumer and must depend on marrow-store"
            );
        } else {
            assert!(
                !production_store,
                "{} must not depend on marrow-store; the path kernel is the engine's only consumer",
                package.name
            );
            assert!(
                !dev_store || package.name == "marrow-vm",
                "{} must not reach marrow-store even in tests; only the VM's private \
                 commit-fault double implements the engine traits",
                package.name
            );
        }
    }
}

/// Forbidden legacy families, spelled as the concrete identifiers or crate
/// references that would appear in retained source if a deleted owner leaked
/// back in. Each is a real Rust token, never an English word, so the scan has no
/// false positives against ordinary prose.
const FORBIDDEN_FAMILIES: &[&str] = &[
    // Deleted crate references: source edges (`use marrow_x` / `marrow_x::`)
    // and manifest/doc spellings (`marrow-x`).
    "marrow_check",
    "marrow_run",
    "marrow_schema",
    "marrow_catalog",
    "marrow_json",
    "marrow-check",
    "marrow-run",
    "marrow-schema",
    "marrow-catalog",
    "marrow-json",
    // The surface construct: AST nodes, the keyword variant, the codes family,
    // and the wire ABI types all share the `Surface` identifier prefix.
    "Surface",
    // The composed prototype session owner.
    "ProjectSession",
    // The deleted structural-optional value variant.
    "Value::Absent",
    // The tree-walking interpreter's owning type.
    "Interpreter",
    // Store-owned language vocabulary that moved to the path kernel: the
    // key/value scalar types and the deleted tree-cell/catalog-id key substrate.
    // The kernel now owns `KeyScalar`/`RuntimeScalar`; these old spellings must
    // not reappear in the store or anywhere else.
    "SavedKey",
    "SavedValue",
    "CatalogId",
    "DataPathSegment",
];

/// Whether `contents` names a forbidden family. A `marrow*` crate token matches
/// only as a whole crate reference, never as a prefix of a longer name — so the
/// deleted interpreter crate `marrow-run`/`marrow_run` does not false-match the
/// retained `marrow-runner`/`marrow_runner`. The non-crate identifiers (`Surface`,
/// `Interpreter`, …) keep matching as prefixes, which is intended. A `.md#`
/// fragment ending in `)` is treated as a document-link destination.
fn names_forbidden_family(contents: &str, family: &str) -> bool {
    if !family.starts_with("marrow") {
        return contents.contains(family);
    }
    let mut from = 0;
    while let Some(offset) = contents[from..].find(family) {
        let start = from + offset;
        let end = start + family.len();
        let extends = contents[end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        let document_fragment =
            contents[..start].ends_with(".md#") && contents[end..].starts_with(')');
        if !extends && !document_fragment {
            return true;
        }
        from = end;
    }
    false
}

#[test]
fn no_tracked_file_names_a_forbidden_family() {
    let this_file = Path::new(file!())
        .file_name()
        .expect("this test file has a name")
        .to_owned();

    let mut violations: Vec<String> = Vec::new();
    for relative in tracked_paths() {
        let path = workspace_root().join(relative);
        if path.file_name() == Some(this_file.as_os_str()) {
            continue;
        }
        let Ok(contents) = fs::read_to_string(&path) else {
            continue; // binary/non-utf8 tracked asset
        };
        for family in FORBIDDEN_FAMILIES {
            if names_forbidden_family(&contents, family) {
                violations.push(format!("{relative}: {family}"));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "forbidden legacy families still present:\n{}",
        violations.join("\n")
    );
}

/// One absence scan: none of `needles` may occur in a tracked `.rs` file whose
/// path starts with one of `roots`.
///
/// Each entry states an invariant that no type, visibility, or lint can carry.
/// `unsafe` is deliberately absent: `unsafe_code = "forbid"` at the workspace
/// root already refuses it at compile time.
struct AbsenceScan {
    subject: &'static str,
    roots: &'static [&'static str],
    needles: &'static [&'static str],
}

const ABSENCE_SCANS: &[AbsenceScan] = &[
    AbsenceScan {
        subject: "a bounded kernel owner stops being bounded through one of these",
        roots: &["crates/marrow-kernel/src/"],
        needles: &["ManuallyDrop", "mem::forget"],
    },
    AbsenceScan {
        subject: "the image site binder validates rows it borrows, so shared mutation or an \
                  aliasing split would let a row change under a validation that already answered",
        roots: &[
            "crates/marrow-image/src/product.rs",
            "crates/marrow-image/src/site_plan.rs",
            "crates/marrow-image/src/draft.rs",
        ],
        needles: &[
            "Cell<",
            "RefCell<",
            "UnsafeCell<",
            "Mutex<",
            "RwLock<",
            "split_at_mut",
            "as *mut",
            "as *const",
        ],
    },
    // A Marrow temporal value is pure: it never derives from a wall or monotonic
    // clock, a timezone database, or a date/time crate. The kernel's durable store
    // substrate is out of scope — storage ownership may record host time as forensic
    // process metadata, which never feeds a language temporal value.
    AbsenceScan {
        subject: "an ambient clock reached the temporal language path",
        roots: &[
            "crates/marrow-temporal/src/",
            "crates/marrow-compile/src/",
            "crates/marrow-image/src/",
            "crates/marrow-verify/src/",
            "crates/marrow-vm/src/",
            "crates/marrow-syntax/src/",
            "crates/marrow-kernel/src/codec/",
        ],
        needles: &[
            "SystemTime",
            "UNIX_EPOCH",
            "Instant::now",
            "chrono",
            "OffsetDateTime",
            "PrimitiveDateTime",
        ],
    },
    // `marrow-project` owns project input plus pure admitted identity mutation and
    // canonical serialization; `marrow-compile` is a read-only ledger consumer. The
    // CLI owns physical `.marrow/ids` publication, so neither pure owner can read or
    // write an identity artifact.
    AbsenceScan {
        subject: "a filesystem edge reached a pure owner",
        roots: &["crates/marrow-project/src/", "crates/marrow-compile/src/"],
        needles: &["std::fs", "std::io::Read", "File::open", "File::create"],
    },
    // Severity is owned by the diagnostic payload (`marrow_syntax::Severity`, fixed at
    // construction) and catchability is not a language axis at all. A per-code table
    // for either would be a second owner consumers could classify codes against.
    AbsenceScan {
        subject: "a deleted registry classification axis returned",
        roots: &["crates/marrow-codes/src/"],
        needles: &[
            "pub enum Catchability",
            "fn catchability(",
            "pub enum SeverityClass",
            "fn severity_class(",
        ],
    },
    // No current envelope claims power-loss durability, so the full-flush fcntl and
    // the std sync wrappers — whose Darwin implementation issues that fcntl — stay
    // out of the journal.
    AbsenceScan {
        subject: "a sync stronger or weaker than plain fsync entered the journal",
        roots: &["crates/marrow-fs-journal/src/"],
        needles: &[
            "fcntl_fullfsync",
            "F_FULLFSYNC",
            "sync_all",
            "sync_data",
            "fdatasync",
        ],
    },
];

#[test]
fn the_absence_scans_hold() {
    let mut violations: Vec<String> = Vec::new();
    for scan in ABSENCE_SCANS {
        let mut read = 0usize;
        for relative in tracked_paths() {
            if !relative.ends_with(".rs")
                || !scan.roots.iter().any(|root| relative.starts_with(root))
            {
                continue;
            }
            read += 1;
            let contents = fs::read_to_string(workspace_root().join(relative))
                .unwrap_or_else(|_| panic!("read {relative}"));
            for needle in scan.needles {
                if contents.contains(needle) {
                    violations.push(format!("{relative}: {needle} — {}", scan.subject));
                }
            }
        }
        assert!(
            read > 0,
            "the scan for `{}` read no file; its roots moved",
            scan.subject
        );
    }

    assert!(violations.is_empty(), "{}", violations.join("\n"));
}
