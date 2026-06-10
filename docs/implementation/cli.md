# CLI and project discovery

The CLI is wiring and rendering, not semantics. Every command resolves a project directory to a `(config, checked program, store)` triple, then prints diagnostics, program output, or a tooling report in `text`/`json`/`jsonl`. All meaning — parse, type-check, evolution verdicts, store keys, value decoding — is decided downstream in `marrow-check`, `marrow-run`, and `marrow-store` and only consumed here.

Two crates: `marrow-project` owns the `marrow.json` schema, source/test discovery, the digest helper, and the accepted-catalog snapshot type. `marrow` is the binary that dispatches argv to one `cmd_*` module.

## The shared spine

`main::main` installs a broken-pipe panic hook (a `Broken pipe` payload exits 0, every other panic defers to the default hook), then dispatches `argv[1]` to one command on a worker thread with a large stack (`run_on_worker_stack`). The parser and runtime recurse over untrusted source on that stack, sized so their fixed depth limits (`check.nesting_limit`, `run.recursion_limit`) always trip before it overflows. Each command's first lines call the shared loaders in `main.rs`:

- `load_config` / `load_checked_project` — dir to `ProjectConfig`, then to a `CheckedProgram` bound against the accepted catalog the store publishes.
- `resolve_store_path` / `open_store_for_inspection` — locate and open the configured store; inspection always uses `open_read_only`.
- `read_accepted_store_catalog` — the one owner of "open the store read-only and read its accepted snapshot"; absent store or in-memory backend binds no catalog (a first run), a decode error surfaces a typed `store.*` code. `check`/`lsp`/`data`/`serve` read durable identity only through this, never a file.
- `establish_store_baseline` — freeze a project's first proposed identity into a write-capable store in one transaction (catalog rows, epoch, engine profile, commit metadata via `marrow_run::evolution::commit_catalog_baseline`), then rebind the program against the now-accepted snapshot. Runs only over an empty store with a pending non-empty proposal; a project past its baseline never churns.

Stream separation is load-bearing: a program's own `print`/`write` output owns stdout; every tooling report (trace, dry-run plan, test summary, run diagnostics) goes to stderr, so a stdout JSON consumer never sees interleaving. Exit codes are 0 success, 1 failure, 2 usage.

## Command families

| Command | Module | Behavior |
|---|---|---|
| `check` | `cmd_check.rs` | Type-checks a single file (via a synthesized one-file scratch project, so it reaches `check.*` rules) or a project dir; `--data` delegates to evolve data-check. |
| `run` | `cmd_run.rs` | Freezes identity, opens and fences the store (auto-applies zero-mutation schema drift), executes the entry under a plain/trace/dry-run hook. |
| `test` | `cmd_test.rs` | Collects public zero-param fns in test modules, runs each over a fresh in-memory store; assert fault is FAIL, any other is ERROR. |
| `fmt` | `cmd_fmt.rs` | Formats one file to stdout, or `--check`/`--write` over source roots; refuses stdin and a bare dir with no mode. |
| `data <roots\|stats\|dump\|integrity\|get>` | `cmd_data.rs`, `cmd_data/` | Read-only inspection; pins one `ReadSnapshot` so multi-pass views describe one store version. |
| `evolve <preview\|apply>` | `cmd_evolve/` | Read-only preview vs managed-write apply; apply gates destructive obligations and recovers half-applied evolutions. |
| `backup` / `restore` | `cmd_backup.rs`, `cmd_restore.rs`, `backup/` | Read-only archive write over a pinned snapshot, carrying the accepted-catalog rows in a typed section; transactional all-or-nothing replay of catalog rows and data into an empty native store, which then runs with no resume step. |

## Module map

### Project discovery (`marrow-project`)

| File | Responsibility |
|---|---|
| `crates/marrow-project/src/lib.rs` | `marrow.json` parse+validate, path-containment checks, module-name derivation, `.mw` source/test discovery, the `CatalogMetadata` accepted-catalog snapshot type and its validation. |
| `crates/marrow-project/src/digest.rs` | `sha256_digest`: `sha256:<hex>` over bytes, used for catalog and analyzed-source integrity digests. |

### CLI core (`marrow`)

| File | Responsibility |
|---|---|
| `crates/marrow/src/main.rs` | argv dispatch, broken-pipe hook, and the shared loaders, store-path resolution, format parsing, and JSON envelope helpers. |
| `crates/marrow/src/cmd_check.rs` | `check`; also the located runtime-fault renderer reused by `run`. |
| `crates/marrow/src/cmd_run.rs` | `run`: fence, auto-apply, re-check, execute under a hook, emit the report. |
| `crates/marrow/src/cmd_test.rs` | `test`: discover and run test fns, print pass/fail/error summary. |
| `crates/marrow/src/cmd_fmt.rs` | `fmt`: format to stdout or `--check`/`--write`. |
| `crates/marrow/src/trace.rs` | `TraceHook` (a `StepHook`) and `WriteTargetNames` mapping catalog ids to store/member/index names. |
| `crates/marrow/src/dry_run.rs` | `DryRunHook` recording managed writes inside a rolled-back savepoint. |

### Data and durability (`marrow`)

| File | Responsibility |
|---|---|
| `crates/marrow/src/cmd_data.rs` | `data` dispatch, `roots`/`stats`/`dump`, snapshot pinning, the streaming JSON-array envelope. |
| `crates/marrow/src/cmd_data/get.rs` | `data get`: one path query, present/absent/children-only rendering. |
| `crates/marrow/src/cmd_data/integrity.rs` | `data integrity`: stream decode problems per record, FAILURE when any exist. |
| `crates/marrow/src/cmd_evolve/mod.rs` | `evolve` dispatch, `check_data`, and `apply_cmd` (the apply publishes the catalog atomically, so there is no post-apply publish or resume step here). |
| `crates/marrow/src/cmd_evolve/args.rs` | apply grammar: `--maintenance`, repeated `--approve-retire <id>:<count>` folded into one `Approval`. |
| `crates/marrow/src/cmd_evolve/render.rs` | all evolve output, including the `ApplyError` to code/message map. |
| `crates/marrow/src/cmd_evolve/store.rs` | `preview_store` (read-only) / `apply_store` (writable native). |
| `crates/marrow/src/cmd_backup.rs`, `cmd_restore.rs` | command wiring for `backup` / `restore`. |
| `crates/marrow/src/backup/mod.rs` | `BackupManifest` (catalog digest/row-count fingerprint and one `archive_checksum`), `EngineDescriptor`, `CommitDescriptor`, `BackupError` taxonomy. |
| `crates/marrow/src/backup/archive.rs` | on-disk framing: `MARROWBK` magic, length-prefixed JSON manifest, length-prefixed catalog section, length-prefixed cell stream, and the one integrity-checksum fold over manifest+catalog+cells. |
| `crates/marrow/src/backup/create.rs` | `create_backup`: capture the catalog snapshot into a typed section, stream the data cells in bounded memory, fold the whole archive into the manifest checksum. |
| `crates/marrow/src/backup/restore.rs` | `read_backup_prologue` + `restore_backup_with_prologue`: validate engine/source/catalog and the catalog-section fingerprint, replay catalog rows and cells in one transaction, rebuild indexes, restamp identity, verify before commit. |

## Load-bearing invariants

- **Path containment.** Every project-relative path (source roots, `dataDir`, tests) is rejected if empty, absolute, or containing `..`, because each is later `Path::join`ed onto the project root. `parse_config` double-parses (raw `Value` then typed `RawConfig`) to catch non-object roots and unknown-key spans.
- **Run baseline and auto-apply.** A clean project with a pending durable identity has its baseline frozen into the store the first time `run` opens it write-capable; a memory-backed project with a durable surface refuses with `run.durable_store_required` rather than running an identity nothing stamps, and a plain script runs over memory. On native schema-drift a zero-record-mutation change is auto-applied through the production apply path, which publishes the advanced catalog snapshot in the apply transaction; the run then re-checks and re-fences. Any backfill/transform/destructive change fences naming `evolve apply` instead. The redb file lock forces `auto_apply_then_reopen` to drop its first handle before reopening. A store holding records under no accepted catalog is refused as populated-but-unstamped.
- **Restore is all-or-nothing.** The whole replay runs in one transaction; any checksum/framing/verify failure rolls the target back to empty. Restore refuses a non-empty target, carries data cells only (indexes are rebuilt), replays bytes verbatim only when `EngineDescriptor` matches exactly, and proves the data compiles against the schema via the `verify` closure before commit. Raw byte validity is never enough.
- **Evolve apply.** `evolve apply` reads accepted identity from the store, freezes a pending baseline into it, then applies the witness's durable work. The apply publishes the activated catalog snapshot, advances the epoch, and commits the data in one transaction, so the accepted catalog the read paths bind is the store snapshot itself — there is no separate publish step and no crash window to resume.
- **Render-only prose.** Integrity, evolve, and restore code assert stable dotted codes and typed verdicts/problems; diagnostic prose is never matched semantically.

## Read next

- `crates/marrow-project/src/lib.rs` — `parse_config`, `check_under_root`, `expected_module_name`, `CatalogMetadata::validate`: the config schema, path-containment guarantee, and catalog identity invariants.
- `crates/marrow/src/main.rs` — `load_checked_project`, `resolve_store_path`, `read_accepted_store_catalog`, `establish_store_baseline`: the shared spine behind every command's first lines.
- `crates/marrow/src/cmd_run.rs` — `open_run_store`, `auto_apply_then_reopen`, `execute`: fence, drift auto-apply, and the execution/report split.
- `crates/marrow/src/cmd_evolve/mod.rs` — `apply_cmd`: read store identity, freeze baseline, preview, apply (which publishes the catalog atomically).
- `crates/marrow/src/backup/restore.rs` — `read_backup_prologue` / `restore_backup_with_prologue`: catalog-section fingerprint gate, transactional catalog+data replay, and verify-before-commit rollback.
- `crates/marrow/src/trace.rs` — `WriteTargetNames::from_program`, `render_write_target`: catalog ids to human names, shared by trace and dry-run.
