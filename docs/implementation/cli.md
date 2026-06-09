# CLI and project discovery

The CLI is wiring and rendering, not semantics. Every command resolves a project directory to a `(config, checked program, store)` triple, then prints diagnostics, program output, or a tooling report in `text`/`json`/`jsonl`. All meaning — parse, type-check, evolution verdicts, store keys, value decoding — is decided downstream in `marrow-check`, `marrow-run`, and `marrow-store` and only consumed here.

Two crates: `marrow-project` owns the `marrow.json` schema, source/test discovery, the digest helper, and the accepted-catalog snapshot type. `marrow` is the binary that dispatches argv to one `cmd_*` module.

## The shared spine

`main::main` installs a broken-pipe panic hook (a `Broken pipe` payload exits 0, every other panic defers to the default hook) and dispatches `argv[1]` to one command. Each command's first lines call the shared loaders in `main.rs`:

- `load_config` / `load_checked_project` — dir to `ProjectConfig`, then to a `CheckedProgram`.
- `resolve_store_path` / `open_store_for_inspection` — locate and open the configured store; inspection always uses `open_read_only`.
- `commit_pending_identity` — freeze pending durable identity before a state-establishing flow.

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
| `backup` / `restore` | `cmd_backup.rs`, `cmd_restore.rs`, `backup/` | Read-only archive write over a pinned snapshot; transactional all-or-nothing replay into an empty native store. |

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
| `crates/marrow/src/cmd_evolve/mod.rs` | `evolve` dispatch and `check_data`; the `resume_completion` crash-window recovery. |
| `crates/marrow/src/cmd_evolve/args.rs` | apply grammar: `--maintenance`, repeated `--approve-retire <id>:<count>` folded into one `Approval`. |
| `crates/marrow/src/cmd_evolve/render.rs` | all evolve output, including the `ApplyError` to code/message map. |
| `crates/marrow/src/cmd_evolve/store.rs` | `preview_store` (read-only) / `apply_store` (writable native). |
| `crates/marrow/src/cmd_backup.rs`, `cmd_restore.rs` | command wiring for `backup` / `restore`. |
| `crates/marrow/src/backup/mod.rs` | `BackupManifest`, `EngineDescriptor`, `CommitDescriptor`, `BackupError` taxonomy. |
| `crates/marrow/src/backup/archive.rs` | on-disk framing: `MARROWBK` magic, length-prefixed JSON manifest, length-prefixed cell stream, checksum fold. |
| `crates/marrow/src/backup/create.rs` | `create_backup`: two-pass cell traversal in bounded memory behind the manifest header. |
| `crates/marrow/src/backup/restore.rs` | `restore_backup`: validate engine/source/catalog, replay cells in one transaction, rebuild indexes, restamp identity, verify before commit. |

## Load-bearing invariants

- **Path containment.** Every project-relative path (source roots, `dataDir`, tests, `acceptedCatalog`) is rejected if empty, absolute, or containing `..`, because each is later `Path::join`ed onto the project root. `parse_config` double-parses (raw `Value` then typed `RawConfig`) to catch non-object roots and unknown-key spans.
- **Run auto-apply.** `run` freezes pending identity before touching the store; on native schema-drift a zero-record-mutation change is auto-applied through the production apply path, advancing the accepted-catalog file in lockstep and re-checking+re-fencing. Any backfill/transform/destructive change fences naming `evolve apply` instead. The redb file lock forces `auto_apply_then_reopen` to drop its first handle before reopening.
- **Restore is all-or-nothing.** The whole replay runs in one transaction; any checksum/framing/verify failure rolls the target back to empty. Restore refuses a non-empty target, carries data cells only (indexes are rebuilt), replays bytes verbatim only when `EngineDescriptor` matches exactly, and proves the data compiles against the schema via the `verify` closure before commit. Raw byte validity is never enough.
- **Evolve resume.** `evolve apply` advances the store transaction then the accepted-catalog file as two steps with the file last; a crash between them is recovered by `resume_completion`, which re-reads commit metadata, rebinds the resume program, and re-verifies the proposal digest before publishing — so a divergent edit proposing the same next epoch is rejected as drift.
- **Render-only prose.** Integrity, evolve, and restore code assert stable dotted codes and typed verdicts/problems; diagnostic prose is never matched semantically.

## Read next

- `crates/marrow-project/src/lib.rs` — `parse_config`, `check_under_root`, `expected_module_name`, `CatalogMetadata::validate`: the config schema, path-containment guarantee, and catalog identity invariants.
- `crates/marrow/src/main.rs` — `load_checked_project`, `resolve_store_path`, `commit_pending_identity`: the shared spine behind every command's first lines.
- `crates/marrow/src/cmd_run.rs` — `open_run_store`, `auto_apply_then_reopen`, `execute`: fence, drift auto-apply, and the execution/report split.
- `crates/marrow/src/cmd_evolve/mod.rs` — `resume_completion`: crash-window recovery for a half-applied evolution.
- `crates/marrow/src/backup/restore.rs` — `restore_backup`: transactional replay, validation gate, and verify-before-commit rollback.
- `crates/marrow/src/trace.rs` — `WriteTargetNames::from_program`, `render_write_target`: catalog ids to human names, shared by trace and dry-run.
