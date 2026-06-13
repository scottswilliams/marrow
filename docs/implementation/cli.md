# CLI and project discovery

The CLI is wiring and rendering, not semantics. Commands resolve a project directory to the config, checked program, and durable state they actually need, then print diagnostics, program output, or a tooling report in the formats that command owns. `check`, `evolve`, `test`, and `data` keep their structured `text`/`json`/`jsonl` reports; `run --dry-run` keeps `text`/`json`; trace, backup, and restore are text-only. All meaning — parse, type-check, evolution verdicts, store keys, value decoding — is decided downstream in `marrow-check`, `marrow-run`, and `marrow-store` and only consumed here.

Two crates: `marrow-project` owns the `marrow.json` schema, source/test discovery, and the digest helper. `marrow` is the binary that dispatches argv to one `cmd_*` module.

## The shared spine

`main::main` installs a broken-pipe panic hook (a `Broken pipe` payload exits 0, every other panic defers to the default hook), then dispatches `argv[1]` to one command on a worker thread with a large stack (`run_on_worker_stack`). The parser and runtime recurse over untrusted source on that stack, sized so their fixed depth limits (`check.nesting_limit`, `run.recursion_limit`) always trip before it overflows. Commands that read an existing project call the shared loaders in `main.rs`:

- `load_config` / `load_checked_project` — dir to `ProjectConfig`, then to a `CheckedProgram` bound against an accepted catalog supplied by the caller.
- `native_store_path` / `resolve_store_path` / `open_store_for_inspection` — locate and open the configured store; inspection uses `open_read_only`, while write-capable commands opt into the write-open path.
- `read_accepted_catalog_artifact` — the `check` owner of reading the fixed
  catalog artifact without opening the store. An absent file binds no catalog
  (a first run), and conflict markers surface `catalog.merge_conflict`.
- `read_accepted_store_catalog` — the durable-state loader for commands that
  inspect or write the store. It reads the fixed catalog artifact and, when
  present, opens the store read-only as a crash bridge. Store decode errors
  surface typed `store.*` codes.
- `establish_store_baseline` — freeze a project's first proposed identity into a write-capable store in one transaction (catalog rows, epoch, engine profile, commit metadata via `marrow_run::evolution::commit_catalog_baseline`), then rebind the program against the now-accepted snapshot. Runs only over an empty store with a pending non-empty proposal; a project past its baseline never churns.

Stream separation is load-bearing: a program's own `print` output owns stdout; run tooling reports such as trace and dry-run plans go to stderr, so a stdout JSON consumer never sees interleaving. Trace is text-only and streams directly. `marrow test --format json|jsonl` owns stdout for its structured test-result report, with text trace output kept on stderr. Exit codes are 0 success, 1 failure, 2 usage.

## Command families

| Command | Module | Behavior |
|---|---|---|
| `init` | `cmd_init.rs` | Creates the quickstart scaffold in a new target directory after validating the final path component as one module-name segment. |
| `check` | `cmd_check.rs` | Type-checks a project directory containing `marrow.json`; JSON output includes checker entry footprints. |
| `run` | `cmd_run.rs` | Freezes identity, opens and fences the store (auto-applies zero-mutation schema drift), executes the entry under a plain/trace/dry-run hook. |
| `test` | `cmd_test.rs` | Collects public zero-param fns in test modules, optionally filters by qualified name substring, runs each selected test over a fresh in-memory store; assert fault is FAIL, any other is ERROR, rendered as text/json/jsonl test-result reports. |
| `fmt` | `cmd_fmt.rs` | Formats one file to stdout, or `--check`/`--write` over source roots; refuses stdin, a bare dir with no mode, and `--write` rewrites that would reduce retained comments. |
| `data <roots\|stats\|dump\|integrity\|recover\|get>` | `cmd_data.rs`, `cmd_data/` | Store inspection plus explicit recovery; read-only views pin one `ReadSnapshot` so multi-pass output describes one store version, while `recover` performs only a write-capable store open. |
| `evolve <preview\|apply>` | `cmd_evolve/` | Read-only preview vs managed-write apply; apply gates destructive obligations and commits data plus catalog rows atomically. |
| `backup` / `restore` | `cmd_backup.rs`, `cmd_restore.rs`, `backup/` | Read-only archive write over a pinned snapshot, carrying the accepted-catalog rows in a typed section; transactional all-or-nothing replay of catalog rows and data into an empty native store, or an explicitly counted replace target, which then runs without re-running evolution. |

## Module map

### Project discovery (`marrow-project`)

| File | Responsibility |
|---|---|
| `crates/marrow-project/src/lib.rs` | `marrow.json` parse+validate, path-containment and plain-test-path checks, module-name derivation, `.mw` source/test discovery. |
| `crates/marrow-project/src/digest.rs` | `sha256_digest`: `sha256:<hex>` over bytes, used for catalog and analyzed-source integrity digests. |

### CLI core (`marrow`)

| File | Responsibility |
|---|---|
| `crates/marrow/src/main.rs` | argv dispatch, broken-pipe hook, and the shared loaders, store-path resolution, format parsing, and JSON envelope helpers. |
| `crates/marrow/src/cmd_init.rs` | `init`: validates the target directory's final path component and writes the quickstart project scaffold without overwriting an existing target. |
| `crates/marrow/src/cmd_check.rs` | `check`; also the located runtime-fault renderer reused by `run`. |
| `crates/marrow/src/cmd_run.rs` | `run`: fence, auto-apply, re-check, execute under a hook, emit the report. |
| `crates/marrow/src/cmd_test.rs` | `test`: discover, filter, and run test fns, print pass/fail/error summary. |
| `crates/marrow/src/cmd_fmt.rs` | `fmt`: format to stdout or `--check`/`--write`. |
| `crates/marrow/src/trace.rs` | `TraceHook` (a `StepHook`) and `WriteTargetNames` mapping catalog ids to store/member/index names. |
| `crates/marrow/src/dry_run.rs` | `DryRunHook` recording managed writes during isolated dry-run execution. |

### Data and durability (`marrow`)

| File | Responsibility |
|---|---|
| `crates/marrow/src/cmd_data.rs` | `data` dispatch, `roots`/`stats`/`dump`, snapshot pinning, the streaming JSON-array envelope. |
| `crates/marrow/src/cmd_data/get.rs` | `data get`: one path query, present/absent/children-only rendering. |
| `crates/marrow/src/cmd_data/integrity.rs` | `data integrity`: render typed saved-data findings, including incomplete-record and dangling-reference catalog/key identity fields, and exit FAILURE when any exist. |
| `crates/marrow/src/cmd_evolve/mod.rs` | `evolve` dispatch, `preview_cmd`, and `apply_cmd` (the apply publishes the catalog atomically, then renders the project-root catalog file from the committed snapshot). |
| `crates/marrow/src/cmd_evolve/args.rs` | apply grammar: `--maintenance`, repeated `--approve-retire <id>:<count>` folded into one `Approval`. |
| `crates/marrow/src/cmd_evolve/render.rs` | all evolve output, including the `ApplyError` to code/message map. |
| `crates/marrow/src/cmd_evolve/store.rs` | `preview_store` (read-only) / `apply_store` (writable native). |
| `crates/marrow/src/cmd_backup.rs`, `cmd_restore.rs` | command wiring for `backup` / `restore`. |
| `crates/marrow/src/backup/mod.rs` | backup `FORMAT_VERSION` 5, `BackupManifest` (`source_digest`, `catalog_epoch`, `catalog_digest`, `state_digest`, `store_uid`, reserved `parent_snapshot_digest`, `engine`, `commit`, `record_count`, `archive_checksum`), `EngineDescriptor`, slim `CommitDescriptor`, `BackupError` taxonomy. |
| `crates/marrow/src/backup/archive.rs` | on-disk framing: `MARROWBK` magic, length-prefixed JSON manifest, length-prefixed catalog section, length-prefixed cell stream, and the integrity-checksum fold over manifest+catalog+cells. |
| `crates/marrow/src/backup/create.rs` | `create_backup`: capture the catalog snapshot into a typed section, stream the data cells in bounded memory, record the state digest and store UID, fold the whole archive into the manifest checksum. |
| `crates/marrow/src/backup/restore.rs` | `read_backup_prologue` + `restore_backup_with_prologue`: validate engine/source/catalog/state, reject non-empty `parent_snapshot_digest`, enforce empty-only or counted replace target mode, replay catalog rows and cells in one transaction, mint a fresh store UID, rebuild indexes, restamp identity, verify before commit. |

## Load-bearing invariants

- **Path containment.** Every project-relative path (source roots, `dataDir`, tests) is rejected if empty, absolute, or containing `..`, because each is later `Path::join`ed onto the project root. Test paths additionally reject glob metacharacters, so discovery is plain file-or-directory selection rather than pattern matching. `parse_config` double-parses (raw `Value` then typed `RawConfig`) to catch non-object roots and unknown-key spans.
- **Run baseline and auto-apply.** A clean project with a pending durable identity has its baseline frozen into the store the first time `run` opens it write-capable; a memory-backed project with a durable surface refuses with `run.durable_store_required` rather than running an identity nothing stamps, and a plain script runs over memory. On native schema-drift a zero-record-mutation change is auto-applied through the production apply path, which publishes the advanced catalog snapshot in the apply transaction; the run then re-checks and re-fences. Any backfill/transform/destructive change fences naming `evolve apply` instead. The redb file lock forces `auto_apply_then_reopen` to drop its first handle before reopening. A store holding records under no accepted catalog is refused as populated-but-unstamped.
- **Restore is all-or-nothing.** The whole replay runs in one transaction; any checksum/framing/verify failure rolls the target back to its prior state. Restore targets are empty-only by default; counted replace mode first confirms the live record count from `--replace --count N`, then clears and replays inside the restore transaction. Restore carries data cells only (indexes are rebuilt), replays bytes verbatim only when `EngineDescriptor` matches exactly, and proves the data compiles against the schema via the `verify` closure before commit. Raw byte validity is never enough.
- **Evolve apply.** `evolve apply` reads accepted identity from `marrow.catalog.json`, with the durable-state loader using the store snapshot as the crash bridge for commands that inspect or write the store. It freezes a pending baseline into the store, then applies the witness's durable work. The apply publishes the activated catalog snapshot, advances the epoch, and commits the data in one transaction; after commit, the CLI renders the project-root file from the committed store snapshot.
- **Render-only prose.** Integrity, evolve, and restore code assert stable dotted codes and typed verdicts/problems; diagnostic prose is never matched semantically.

## Read next

- `crates/marrow-project/src/lib.rs` — `parse_config`, `check_under_root`, `expected_module_name`: the config schema and path-containment guarantee.
- `crates/marrow/src/main.rs` — `load_checked_project`, `resolve_store_path`, `read_accepted_catalog_artifact`, `read_accepted_store_catalog`, `establish_store_baseline`: the shared spine behind every command's first lines.
- `crates/marrow/src/cmd_run.rs` — `open_run_store`, `auto_apply_then_reopen`, `execute`: fence, drift auto-apply, and the execution/report split.
- `crates/marrow/src/cmd_evolve/mod.rs` — `apply_cmd`: read store identity, freeze baseline, preview, apply (which publishes the catalog atomically).
- `crates/marrow/src/backup/restore.rs` — `read_backup_prologue` / `restore_backup_with_prologue`: catalog-section fingerprint gate, empty-only or counted replace target gate, transactional catalog+data replay, and verify-before-commit rollback.
- `crates/marrow/src/trace.rs` — `WriteTargetNames::from_program`, `render_write_target`: catalog ids to human names, shared by trace and dry-run.
