//! The trusted bulk importer end to end: a realistic JSONL corpus populates a provisioned
//! store through the path kernel, kernel mediation is observed on read-back, authority is
//! enforced, bounded-batch behavior is measured on a large corpus, and the closed lifecycle
//! boundary is proven — the importer is unreachable from the bytecode and client-wire crates
//! and writes only through `create_entry`, never a raw cell/engine/transaction handle.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::RuntimeScalar;
use marrow_kernel::durable::{
    DemandCoverage, Durable, EntryValue, InvocationGrant, Presence, SiteSpec, SiteTarget,
    StoreSchema,
};
use marrow_kernel::equality::ValueDomain;
use marrow_lifecycle::{
    EngineKind, ImportError, ImportLimits, ImportTarget, LogicalHead, ProvisionRequest, RowFault,
    ShapeFault, StoreEnvelope, StoreInstanceId, active_binding, head_map, import_jsonl, open,
    provision,
};
use marrow_verify::{VerifiedImage, verify};

/// A `counters` root of `Counter` resources — a required `value: int` and a sparse
/// `label: string` — keyed by `id: int`. The flat scalar shape the importer targets.
const SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn readValue(n: int): int {
    return ^counters[n].value ?? 0
}
"#;

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

/// A durable program whose `books` root carries groups and a nested branch — a shape the flat
/// importer refuses.
const NESTED_SOURCE: &str = r#"resource Book {
    required title: string

    details {
        pages: int
    }

    notes[noteId: string] {
        required body: string
    }
}

store ^books[id: int]: Book

pub fn readTitle(id: int): string {
    return ^books[id].title ?? "?"
}
"#;

const NESTED_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a\n\
     id product Book 1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d\n\
     id field Book.title 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
     id group Book.details 20202020202020202020202020202020\n\
     id field Book.details.pages 21212121212121212121212121212121\n\
     id root Book.notes 30303030303030303030303030303030\n\
     id key Book.notes.noteId 31313131313131313131313131313131\n\
     id field Book.notes.body 32323232323232323232323232323232\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

fn compile(source: &str, ids: &str) -> VerifiedImage {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    verify(&compiled.image.bytes).expect("verify")
}

fn schemas_of(image: &VerifiedImage) -> Vec<StoreSchema> {
    marrow_vm::derive_store_schemas(image)
        .expect("flat-executable")
        .0
}

/// A unique scratch store directory, removed on drop.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "marrow-imp01-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&base).expect("scratch base");
        Self {
            dir: base.join("store"),
        }
    }
    fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(parent) = self.dir.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

/// Provision a fresh store at `dir` bound to `image`.
fn provision_from(dir: &Path, image: &VerifiedImage) {
    let schemas = schemas_of(image);
    let sites = marrow_vm::derive_store_schemas(image).expect("flat").1;
    let envelope = StoreEnvelope {
        instance: StoreInstanceId::draw().expect("entropy"),
        writer_toolchain: "0.1.0".to_string(),
        engine_kind: EngineKind::Redb,
        engine_format_version: 1,
    };
    let head = LogicalHead::provision(active_binding(image), head_map(image).expect("head map"));
    provision(
        dir,
        ProvisionRequest {
            envelope,
            head,
            schemas,
            sites,
        },
    )
    .expect("provision");
}

fn counter_target() -> ImportTarget {
    ImportTarget {
        root: 0,
        key_columns: vec!["id".to_string()],
    }
}

/// Read one `counters` entry back through a kernel read session — proving the imported row
/// landed as a proper kernel entry (a marker plus field leaves), not a raw byte blob.
fn read_entry(dir: &Path, image: &VerifiedImage, id: i64) -> Option<EntryValue> {
    let read_sites = vec![SiteSpec {
        root: 0,
        target: SiteTarget::WholePayload,
    }];
    let mut opened = open(dir, schemas_of(image), read_sites).expect("open for read-back");
    let mut read = opened
        .store
        .read_session(
            InvocationGrant::full_store(),
            DemandCoverage {
                read: true,
                write: false,
            },
        )
        .expect("read session");
    let site = read.site(0);
    read.read_entry(&site, &[KeyScalar::Int(id)]).expect("read")
}

/// The whole exit-gate journey: a realistic JSONL corpus populates a provisioned store through
/// the importer, and reading entries back through the kernel proves each row is a typed durable
/// entry with its required field present and its sparse field present-or-absent per the source.
#[test]
fn a_realistic_corpus_populates_the_store_through_the_kernel() {
    let scratch = Scratch::new("bulk");
    let image = compile(SOURCE, IDS);
    provision_from(scratch.dir(), &image);

    // A realistic export: even ids carry a text label (with commas, quotes, and an escaped
    // newline — the text a positional CSV could not carry); odd ids leave the sparse label
    // null. One blank separator line is tolerated.
    let rows = 2_500u64;
    let mut jsonl = String::new();
    for id in 1..=rows {
        if id % 500 == 0 {
            jsonl.push('\n'); // a tolerated blank line
        }
        if id % 2 == 0 {
            jsonl.push_str(&format!(
                "{{\"id\": {id}, \"value\": {}, \"label\": \"row \\\"{id}\\\", ok\\nline2\"}}\n",
                id * 10
            ));
        } else {
            jsonl.push_str(&format!(
                "{{\"id\": {id}, \"value\": {}, \"label\": null}}\n",
                id * 10
            ));
        }
    }

    let limits = ImportLimits {
        batch_rows: 256,
        ..ImportLimits::DEFAULT
    };
    let report = import_jsonl(
        scratch.dir(),
        schemas_of(&image),
        counter_target(),
        Cursor::new(jsonl.into_bytes()),
        InvocationGrant::full_store(),
        limits,
    )
    .expect("import");

    assert_eq!(report.rows_imported, rows, "every row imported");
    let expected_batches = rows.div_ceil(limits.batch_rows as u64);
    assert_eq!(
        report.batches_committed, expected_batches,
        "the corpus committed in bounded batches",
    );

    // Kernel-mediated read-back: an even id has a present label, an odd id an absent one.
    let even = read_entry(scratch.dir(), &image, 4).expect("entry 4 present");
    assert_eq!(
        even.fields[0],
        Some(ValueDomain::Scalar(RuntimeScalar::Int(40))),
        "required value landed",
    );
    assert_eq!(
        even.fields[1],
        Some(ValueDomain::Scalar(RuntimeScalar::Str(
            "row \"4\", ok\nline2".to_string()
        ))),
        "the escaped label decoded through the kernel",
    );
    let odd = read_entry(scratch.dir(), &image, 5).expect("entry 5 present");
    assert_eq!(
        odd.fields[0],
        Some(ValueDomain::Scalar(RuntimeScalar::Int(50)))
    );
    assert_eq!(odd.fields[1], None, "a null label is a sparse-absent field");

    // A never-imported id is absent — the importer created exactly the corpus.
    let read_sites = vec![SiteSpec {
        root: 0,
        target: SiteTarget::WholePayload,
    }];
    let mut opened = open(scratch.dir(), schemas_of(&image), read_sites).expect("open");
    let mut read = opened
        .store
        .read_session(
            InvocationGrant::full_store(),
            DemandCoverage {
                read: true,
                write: false,
            },
        )
        .expect("read session");
    let site = read.site(0);
    assert_eq!(
        read.presence(&site, &[KeyScalar::Int(rows as i64 + 1)]),
        Ok(Presence::Absent),
        "an id beyond the corpus was never created",
    );
}

/// Effective authority is real: an import under a read-only grant is denied at the first
/// session open — `demand ∩ ceiling ∩ grant` refuses the write — and the store stays empty.
#[test]
fn a_read_only_grant_denies_the_import() {
    let scratch = Scratch::new("denied");
    let image = compile(SOURCE, IDS);
    provision_from(scratch.dir(), &image);

    let jsonl = "{\"id\": 1, \"value\": 10}\n";
    let denied = import_jsonl(
        scratch.dir(),
        schemas_of(&image),
        counter_target(),
        Cursor::new(jsonl.as_bytes().to_vec()),
        InvocationGrant {
            read: true,
            write: false,
        },
        ImportLimits::DEFAULT,
    );
    match denied {
        Err(ImportError::Denied) => {}
        other => panic!("a read-only grant must deny the import, got {other:?}"),
    }

    // No write occurred: the store has no entry 1.
    assert!(
        read_entry(scratch.dir(), &image, 1).is_none(),
        "a denied import writes nothing",
    );
}

/// A malformed line, a missing required field, and a duplicate key are typed row faults naming
/// the 1-based line; the batches committed before the fault stay in the store.
#[test]
fn a_row_fault_names_its_line_and_keeps_the_committed_prefix() {
    let scratch = Scratch::new("rowfault");
    let image = compile(SOURCE, IDS);
    provision_from(scratch.dir(), &image);

    // Lines 1-2 are good, line 3 is missing the required `value`. batch_rows=1 so lines 1-2 are
    // committed before line 3 faults.
    let jsonl = "{\"id\": 1, \"value\": 10}\n\
                 {\"id\": 2, \"value\": 20}\n\
                 {\"id\": 3, \"label\": \"no value\"}\n";
    let limits = ImportLimits {
        batch_rows: 1,
        ..ImportLimits::DEFAULT
    };
    match import_jsonl(
        scratch.dir(),
        schemas_of(&image),
        counter_target(),
        Cursor::new(jsonl.as_bytes().to_vec()),
        InvocationGrant::full_store(),
        limits,
    ) {
        Err(ImportError::Row {
            line, committed, ..
        }) => {
            assert_eq!(line, 3, "the fault names the 1-based line");
            assert_eq!(
                committed.rows_imported, 2,
                "the prior batches are committed"
            );
        }
        other => panic!("expected a row fault, got {other:?}"),
    }
    // The committed prefix is durable.
    assert!(read_entry(scratch.dir(), &image, 1).is_some());
    assert!(read_entry(scratch.dir(), &image, 2).is_some());
    assert!(read_entry(scratch.dir(), &image, 3).is_none());
}

/// A duplicate key in one import is a typed row fault (create yields already-present); the
/// batch holding it rolls back.
#[test]
fn a_duplicate_key_is_a_row_fault() {
    let scratch = Scratch::new("dupe");
    let image = compile(SOURCE, IDS);
    provision_from(scratch.dir(), &image);

    let jsonl = "{\"id\": 7, \"value\": 1}\n{\"id\": 7, \"value\": 2}\n";
    match import_jsonl(
        scratch.dir(),
        schemas_of(&image),
        counter_target(),
        Cursor::new(jsonl.as_bytes().to_vec()),
        InvocationGrant::full_store(),
        ImportLimits::DEFAULT,
    ) {
        Err(ImportError::Row {
            fault: RowFault::DuplicateKey,
            ..
        }) => {}
        other => panic!("expected a duplicate-key row fault, got {other:?}"),
    }
}

/// A root with groups or a keyed branch is refused before any write: the flat importer maps
/// scalar-field roots only.
#[test]
fn a_nested_root_shape_is_refused() {
    let scratch = Scratch::new("nested");
    let image = compile(NESTED_SOURCE, NESTED_IDS);
    provision_from(scratch.dir(), &image);

    let jsonl = "{\"id\": 1, \"title\": \"x\"}\n";
    match import_jsonl(
        scratch.dir(),
        schemas_of(&image),
        ImportTarget {
            root: 0,
            key_columns: vec!["id".to_string()],
        },
        Cursor::new(jsonl.as_bytes().to_vec()),
        InvocationGrant::full_store(),
        ImportLimits::DEFAULT,
    ) {
        Err(ImportError::UnsupportedShape(ShapeFault::HasGroupsOrBranches { .. })) => {}
        other => panic!("a nested-shape root must be refused, got {other:?}"),
    }
}

/// Bounded-batch behavior on a large corpus: 40,000 rows stream through the importer with a
/// small batch, committing in exactly `ceil(rows / batch_rows)` batches. The source is streamed
/// from a lazy reader, so a whole-corpus import never materializes the corpus — memory is
/// bounded by one line plus one batch (campaign law 9). The wall time is recorded.
#[test]
fn a_large_corpus_commits_in_bounded_batches() {
    let scratch = Scratch::new("large");
    let image = compile(SOURCE, IDS);
    provision_from(scratch.dir(), &image);

    let rows = 40_000u64;
    let batch_rows = 500usize;
    let source = LazyJsonl {
        next: 1,
        last: rows,
    };

    let start = std::time::Instant::now();
    let report = import_jsonl(
        scratch.dir(),
        schemas_of(&image),
        counter_target(),
        std::io::BufReader::new(source),
        InvocationGrant::full_store(),
        ImportLimits {
            batch_rows,
            ..ImportLimits::DEFAULT
        },
    )
    .expect("large import");
    let elapsed = start.elapsed();

    assert_eq!(report.rows_imported, rows);
    assert_eq!(
        report.batches_committed,
        rows.div_ceil(batch_rows as u64),
        "the large corpus committed in bounded batches",
    );
    println!(
        "IMP01 large-corpus: {rows} rows in {} batches, {:?} ({:.0} rows/s)",
        report.batches_committed,
        elapsed,
        rows as f64 / elapsed.as_secs_f64(),
    );

    // Spot-check the tail landed through the kernel.
    let tail = read_entry(scratch.dir(), &image, rows as i64).expect("tail present");
    assert_eq!(
        tail.fields[0],
        Some(ValueDomain::Scalar(RuntimeScalar::Int(rows as i64 * 10))),
    );
}

/// A `Read` source that generates JSONL rows lazily, one at a time, so the corpus is never held
/// in memory. Proves the importer streams: it drives the large-corpus test without a
/// materialized buffer.
struct LazyJsonl {
    next: u64,
    last: u64,
}

impl std::io::Read for LazyJsonl {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.next > self.last {
            return Ok(0);
        }
        let line = format!(
            "{{\"id\": {id}, \"value\": {value}}}\n",
            id = self.next,
            value = self.next * 10
        );
        self.next += 1;
        let bytes = line.as_bytes();
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        // A short read is legal for `Read`; `BufReader` re-invokes until the delimiter. To keep
        // the helper simple we only ever hand back whole lines that fit, which they do for this
        // fixture's line width against BufReader's 8 KiB buffer.
        assert!(
            n == bytes.len(),
            "the fixture line must fit the read buffer"
        );
        Ok(n)
    }
}

/// The closed lifecycle boundary (unconstructibility, crate level): the bytecode executor and
/// the client-wire crate do not depend on `marrow-lifecycle`, so no bytecode/client path can
/// name — let alone call — the import mode. Only the privileged CLI host reaches it. This is
/// the Cargo trust boundary the type-level guard rests on (the mode requires an `OpenStore`,
/// whose non-`Clone`, non-serializable owner lock has no constructor below this crate).
#[test]
fn the_import_mode_is_unreachable_from_bytecode_and_client() {
    let crates = crates_dir();
    let depends_on_lifecycle = |crate_name: &str| -> bool {
        let manifest = crates.join(crate_name).join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest)
            .unwrap_or_else(|_| panic!("read {}", manifest.display()));
        // A dependency edge names the crate outside the `[dev-dependencies]` section. Tests may
        // legitimately depend on lifecycle (marrow-vm is a dev-dependency of lifecycle, not the
        // reverse); production reachability is what the boundary forbids.
        production_deps(&text).contains("marrow-lifecycle")
    };

    assert!(
        !depends_on_lifecycle("marrow-vm"),
        "the bytecode executor must not depend on marrow-lifecycle",
    );
    assert!(
        !depends_on_lifecycle("marrow-local-wire"),
        "the client-wire crate must not depend on marrow-lifecycle",
    );
    // Positive control: the privileged host does reach it, so the boundary is drawn at the
    // right place (not vacuously true because nothing depends on lifecycle).
    assert!(
        depends_on_lifecycle("marrow-runner"),
        "the privileged CLI host is the legitimate caller of the lifecycle",
    );
}

/// The only workspace crates depending on `marrow-lifecycle` in production are the privileged
/// host and lifecycle itself — an absence gate over the whole member set, so a new production
/// edge from any bytecode/client/host-adapter crate into the import mode fails here.
#[test]
fn no_unexpected_crate_reaches_the_lifecycle() {
    let crates = crates_dir();
    let allowed = ["marrow-runner", "marrow-lifecycle"];
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&crates)
        .expect("read crates dir")
        .flatten()
    {
        let manifest = entry.path().join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if allowed.contains(&name.as_str()) {
            continue;
        }
        if production_deps(&text).contains("marrow-lifecycle") {
            offenders.push(name);
        }
    }
    assert!(
        offenders.is_empty(),
        "unexpected production dependency on marrow-lifecycle: {offenders:?}",
    );
}

/// The raw-seeding absence gate: the importer writes exclusively through the kernel's
/// `create_entry` — its source names no byte-engine transaction primitive. A regression that
/// added a raw `begin`/`put`/`remove` seeding path (which would leak production-ward, bypassing
/// authority, site resolution, and index maintenance) is made conspicuous here.
#[test]
fn the_importer_writes_only_through_the_kernel() {
    let import_src = crates_dir()
        .join("marrow-lifecycle")
        .join("src")
        .join("import.rs");
    let text = std::fs::read_to_string(&import_src).expect("read import.rs");
    // Strip doc comments and line comments so prose that names these primitives (to explain why
    // the importer avoids them) does not trip the scan; only code lines are checked.
    let code: String = text
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        code.contains("create_entry"),
        "the importer must write through the kernel's create_entry",
    );
    // The raw-engine markers a seeding path would necessarily name: the byte-engine crate, its
    // transaction trait/primitives, and the restore slice's cell-replay seams. The importer
    // touches none of them — it reaches the engine only transitively through the kernel.
    for forbidden in [
        "marrow_store",
        "ByteEngine",
        "WriteTxn",
        ".begin(",
        "insert_cells",
        "visit_cells",
    ] {
        assert!(
            !code.contains(forbidden),
            "the importer must not name the raw engine primitive `{forbidden}` — every write \
             passes the path kernel",
        );
    }
}

/// The workspace `crates/` directory, from this crate's manifest directory.
fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .to_path_buf()
}

/// The production dependency section of a `Cargo.toml`, excluding `[dev-dependencies]` and
/// `[build-dependencies]`, as a single string for substring checks.
fn production_deps(manifest: &str) -> String {
    let mut out = String::new();
    let mut in_dev = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_dev = trimmed.contains("dev-dependencies") || trimmed.contains("build-dependencies");
        } else if !in_dev {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}
