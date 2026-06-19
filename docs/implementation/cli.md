# CLI and project discovery

The CLI is wiring and envelope rendering, not semantics. Commands resolve a project directory to the config, checked program, and durable state they actually need, then print diagnostics, program output, or a tooling report in the formats that command owns. `check`, `evolve`, `test`, and `data` keep their structured `text`/`json`/`jsonl` reports; `run` keeps `text`/`json`; trace, backup, and restore are text-only. All meaning — parse, type-check, run/test admission, evolution verdicts, store keys, value decoding — is decided downstream in `marrow-project`, `marrow-check`, `marrow-run`, `marrow-json`, and `marrow-store` and only consumed here. `marrow-json` owns shared outbound rendering for entry returns, saved keys, data snapshot stamps, check surface ABI descriptors, checked surface read request DTO decode, sparse update request DTO decode, and result rendering; the CLI still owns command envelopes.

Four crates meet here: `marrow-project` owns the `marrow.json` schema,
source/test discovery, and the digest helper; `marrow-check::project_io` owns
project/catalog IO shared by CLI callers; `marrow-json` owns shared outbound
entry-return, saved-key, data-snapshot, surface ABI descriptor, and surface
read/update JSON DTO leaves plus checked surface request decode;
`marrow` is the binary that dispatches argv to one `cmd_*` module and renders
command envelopes.

## The shared spine

`main::main` installs a broken-pipe panic hook: a `Broken pipe` payload exits 0,
and every other panic defers to the default hook. It then dispatches `argv[1]`
to one command on a worker thread with a large stack (`run_on_worker_stack`).
The parser and runtime recurse over untrusted source on that stack, sized so
their fixed depth limits (`check.nesting_limit`, `run.depth`) always trip before
it overflows. Commands that read an existing project call CLI wrappers in
`main.rs` over shared project IO from `marrow-check::project_io`:

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

`marrow run` and `marrow test` use `marrow-run::ProjectSession` for project checking, catalog binding, store admission, and entry invocation. The command modules parse flags, choose hooks, and render the resulting reports. For `run --arg`, `cmd_run.rs` is only the argv adapter: it preserves repeated `name=value` pairs in argv order and passes them to `marrow-run::entry::CheckedEntryCall`, where the checked signature owns textual decoding. For `run --format json`, `cmd_run.rs` owns the result envelope while `marrow-json` renders the return value when it belongs to the existing entry-return JSON surface.

Stream separation is load-bearing: in text mode a program's own `print` output owns stdout; run tooling reports such as trace and dry-run plans go to stderr, so a stdout consumer never sees interleaving. `run --format json` switches to the envelope egress regime: stdout is a single result envelope containing captured program output and any renderable return value, while runtime faults still render on stderr. Trace is text-only and streams directly. `marrow test --format json|jsonl` owns stdout for its structured test-result report, with text trace output kept on stderr. Exit codes are 0 success, 1 failure, 2 usage.
Structured reports that include a `project` field render the canonical absolute project directory through `project_json_path`.

## Command families

| Command | Module | Behavior |
|---|---|---|
| `init` | `cmd_init.rs` | Creates the quickstart scaffold in a new target directory after validating the final path component as one module-name segment. |
| `check` | `cmd_check.rs` | Type-checks a project directory containing `marrow.json`; JSON output includes checker entry footprints and serialized surface ABI descriptors for successful checks. |
| `doctor` | `cmd_doctor.rs` | Aggregates read-only operator triage findings with exact next commands, including store-open failures, catalog validation, fence/stamp classification, engine profile, and a capped integrity sample. |
| `run` | `cmd_run.rs` | Parses run flags and repeated `--arg name=value` pairs, opens a `ProjectSession`, emits session notices, invokes the selected entry under a plain/trace/dry-run hook, and renders text output, JSON envelopes, or dry-run reports. |
| `test` | `cmd_test.rs` | Opens a test `ProjectSession`, filters its public zero-param test cases by qualified name substring, invokes each selected test, and renders pass/fail/error reports. |
| `fmt` | `cmd_fmt.rs` | Formats one file to stdout, or `--check`/`--write` over source roots; refuses stdin, a bare dir with no mode, and `--write` rewrites that would reduce retained comments. |
| `data <roots\|stats\|dump\|integrity\|recover\|get>` | `cmd_data.rs`, `cmd_data/` | Store inspection plus explicit recovery; read-only views either pin one live-store `ReadSnapshot` or mount `--backup` into memory, while `recover` performs only a write-capable store open. |
| `evolve <preview\|apply>` | `cmd_evolve/` | Read-only preview vs managed-write apply; preview can render parseable evolve scaffolds or derive the witness from `--from-backup`, and apply gates Retire-bearing witnesses on a recovery point before committing data plus catalog rows atomically. |
| `backup` / `restore` | `cmd_backup.rs`, `cmd_restore.rs`, `backup/` | Read-only archive write through a shared atomic artifact helper over a pinned snapshot, carrying the accepted-catalog rows in a typed section; transactional all-or-nothing replay of catalog rows and data into an empty native store, or an explicitly counted replace target, which then runs without re-running evolution. |

## Module map

### Project discovery (`marrow-project`)

| File | Responsibility |
|---|---|
| `crates/marrow-project/src/lib.rs` | `marrow.json` parse+validate, path-containment and plain-test-path checks, module-name derivation, `.mw` source/test discovery. |
| `crates/marrow-project/src/digest.rs` | `sha256_digest`: `sha256:<hex>` over bytes, used for catalog and analyzed-source integrity digests. |

### Project and catalog IO (`marrow-check`)

| File | Responsibility |
|---|---|
| `crates/marrow-check/src/project_io.rs` | Shared project loaders, accepted-catalog artifact reads, store-backed accepted-catalog reads, and merge-conflict/error mapping for CLI callers. |

### CLI core (`marrow`)

| File | Responsibility |
|---|---|
| `crates/marrow/src/main.rs` | argv dispatch, broken-pipe hook, CLI wrappers around shared project IO, store-path resolution, format parsing, and JSON envelope helpers. |
| `crates/marrow/src/cmd_init.rs` | `init`: validates the target directory's final path component and writes the quickstart project scaffold without overwriting an existing target. |
| `crates/marrow/src/cmd_check.rs` | `check`; also the located runtime-fault renderer reused by `run`. |
| `crates/marrow/src/cmd_doctor.rs` | `doctor`: read-only operator triage that aggregates config, check, catalog, store-open, stamp/fence, engine-profile, and bounded integrity-sample facts into stable `doctor.*` findings. |
| `crates/marrow/src/cmd_run.rs` | `run`: parse flags, preserve argv argument order, render session open errors/notices, execute through `ProjectSession::invoke` under a hook, emit text output, JSON envelopes, or dry-run reports. |
| `crates/marrow/src/cmd_test.rs` | `test`: filter session-provided test cases, invoke them through `ProjectSession::invoke`, print pass/fail/error summary. |
| `crates/marrow/src/cmd_fmt.rs` | `fmt`: format to stdout or `--check`/`--write`. |
| `crates/marrow/src/trace.rs` | `TraceHook` (a `StepHook`) and `WriteTargetNames` mapping catalog ids to store/member/index names. |
| `crates/marrow/src/dry_run.rs` | `DryRunHook` recording managed writes during isolated dry-run execution and aggregating per-root/per-index create/write/delete counts from the managed write stream. |

### Shared JSON (`marrow-json`)

| File | Responsibility |
|---|---|
| `crates/marrow-json/src/lib.rs`, `crates/marrow-json/src/surface.rs` | Outbound rendering for `marrow run --format json` entry returns, the saved-key JSON shape reused by trace and integrity tooling, the `store_snapshot` data-stamp shape reused by data inspection, serialized surface ABI DTOs for check output, checked surface read request DTO decode, sparse update request DTO decode, and context-aware surface read-result DTOs. It does not define routes, opaque cursor tokens, generated clients, or create/delete body decode. |

### Data and durability (`marrow`)

| File | Responsibility |
|---|---|
| `crates/marrow/src/cmd_data.rs` | `data` dispatch, `roots`/`stats`/`dump`, live-or-backup read-target parsing, snapshot pinning, the streaming JSON-array envelope. |
| `crates/marrow/src/cmd_data/get.rs` | `data get`: one saved-data path, present/absent/children-only rendering. |
| `crates/marrow/src/cmd_data/integrity.rs` | `data integrity`: render typed saved-data findings, including incomplete-record and dangling-reference catalog/key identity fields, and exit FAILURE when any exist. |
| `crates/marrow-check/src/tooling/integrity.rs` | Shared integrity facts, including `sample_integrity_problems`, the bounded sample used by `doctor` so triage checks record values, completeness, and stored cells under one shared cap instead of running the full integrity scan silently. |
| `crates/marrow/src/cmd_evolve/mod.rs` | `evolve` dispatch, `preview_cmd`, and `apply_cmd` (the apply rejects managed-artifact backup paths, creates any required recovery backup before the store mutation, publishes the catalog atomically, then renders the project-root catalog file from the committed snapshot). |
| `crates/marrow/src/cmd_evolve/args.rs` | preview/apply grammar: preview owns `--from-backup` and `--scaffold`; apply owns `--maintenance`, repeated `--approve-retire <id>:<count>` folded into one `Approval`, and the mutually exclusive `--backup <path>` / `--no-backup` recovery decision. |
| `crates/marrow/src/cmd_evolve/render.rs` | all evolve output, including formatter-backed scaffold source, recovery-point receipt fields, and the `ApplyError` to code/message map. |
| `crates/marrow/src/cmd_evolve/store.rs` | `preview_store` (read-only) / `apply_store` (writable native). |
| `crates/marrow/src/cmd_backup.rs`, `cmd_restore.rs` | command wiring for `backup` / `restore`, plus the shared backup mount helpers used by backup-backed data inspection and evolve preview. |
| `crates/marrow/src/backup/artifact.rs` | shared backup artifact writer: adjacent temp file, owner-only create, debug write-failure injection, sync, read-back archive validation, then rename. |
| `crates/marrow/src/backup/mod.rs` | backup `FORMAT_VERSION` (the store-owned `TREE_BACKUP_ARCHIVE_FORMAT_VERSION`), `BackupManifest` (`source_digest`, `catalog_epoch`, `catalog_digest`, `state_digest`, `store_uid`, reserved `parent_snapshot_digest`, `engine`, `commit`, `record_count`, `archive_checksum`), `EngineDescriptor`, slim `CommitDescriptor`, `BackupError` taxonomy. |
| `crates/marrow/src/backup/archive.rs` | on-disk framing: `MARROWBK` magic, length-prefixed JSON manifest, length-prefixed catalog section, length-prefixed cell stream, and the integrity-checksum fold over manifest+catalog+cells. |
| `crates/marrow/src/backup/create.rs` | `create_backup`: capture the catalog snapshot into a typed section, stream the data cells in bounded memory, record the state digest and store UID, fold the whole archive into the manifest checksum. |
| `crates/marrow/src/backup/restore.rs` | `read_backup_prologue` + `restore_backup_with_prologue`: validate engine/source/catalog/state, reject non-empty `parent_snapshot_digest`, enforce empty-only or counted replace target mode, replay catalog rows and cells in one transaction, mint a fresh store UID, rebuild indexes, restamp identity, verify before commit; the in-memory mount path reuses the same prologue and replay validation without activating a native target. |

## Load-bearing invariants

- **Path containment.** Every project-relative path (source roots, `dataDir`, tests) is rejected if empty, absolute, or containing `..`, because each is later `Path::join`ed onto the project root. Test paths additionally reject glob metacharacters, so discovery is plain file-or-directory selection rather than pattern matching. `parse_config` double-parses (raw `Value` then typed `RawConfig`) to catch non-object roots and unknown-key spans.
- **Run admission.** `ProjectSession::open` owns run admission. A clean project
  with a pending durable identity has its baseline frozen into the store the
  first time a real `run` opens it write-capable; a memory-backed project with a
  durable surface refuses with `run.durable_store_required`, and a plain script
  runs over memory.
- **Run drift handling.** On native schema drift, a zero-record-mutation change
  is auto-applied through the production apply path, which publishes the
  advanced catalog snapshot in the apply transaction; the session then
  re-checks and re-fences. Any backfill/transform/destructive change fences
  naming `evolve apply` instead. The redb file lock forces real apply paths to
  drop the first handle before reopening. A store holding records under no
  accepted catalog is refused as populated-but-unstamped.
- **Dry-run classification.** `run --dry-run` takes the same checked path but
  opens native stores read-only for classification: it reports would-freeze,
  would-apply, or would-fence as tooling content, does not freeze or auto-apply,
  and does not execute the entry when the fence would not pass.
- **Arguments and envelopes.** `SessionEntry` carries the selected entry name
  plus raw text argument pairs into `CheckedEntryCall`, where the checked
  signature decodes scalars, enums, scalar/enum sequences, and single-key
  identities or rejects unsupported surfaces with `run.entry_argument`.
  `cmd_run.rs` captures program stdout only for `--format json`, renders the
  result envelope with the session-owned store stamp, and asks `marrow-json` to
  render the CLI-compatible return-value leaf. Resource and local-tree returns
  remain outside that return surface and fault as `run.entry_surface`. The
  envelope emits `committed` only when the invocation advanced the store commit.
- **Restore is all-or-nothing.** The whole replay runs in one transaction; any checksum/framing/verify failure rolls the target back to its prior state. Restore targets are empty-only by default; counted replace mode first confirms the live record count from `--replace --count N`, then clears and replays inside the restore transaction. Restore carries data cells only (indexes are rebuilt), replays bytes verbatim only when `EngineDescriptor` matches exactly, and proves the data compiles against the schema via the `verify` closure before commit. Raw byte validity is never enough.
- **Backup-backed reads are non-activating.** `data roots|stats|dump|integrity|get --backup` loads the current config and source, validates the backup prologue and carried catalog through the restore contract, replays the archive into `TreeStore::memory`, verifies the mounted data against the checked schema, and then inspects that memory store. It does not open the configured native store, take the store lock, read `marrow.catalog.json`, render a catalog file, or write durable state. `evolve preview --from-backup` uses the same in-memory data mount for its witness but still compares the current source-tree catalog artifact with the backup identity, so a project that has advanced past the backup reports the typed restore drift refusal instead of previewing across catalog histories.
- **Evolve apply.** `evolve apply` reads accepted identity from `marrow.catalog.json`, with the durable-state loader using the store snapshot as the crash bridge for commands that inspect or write the store. It freezes a pending baseline into the store, then previews the witness. A Retire-bearing witness requires `--backup <path>` or `--no-backup`; the backup path must not resolve under managed project paths (`marrow.json`, `marrow.catalog.json`, source roots, test paths, or the native data directory/store file), then uses the shared typed backup artifact writer and validates the archive before apply establishes any missing store UID or stages evolution work. The apply publishes the activated catalog snapshot, advances the epoch, and commits the data in one transaction; after commit, the CLI renders the project-root file from the committed store snapshot.
- **Render-only prose.** Integrity, evolve, and restore code assert stable dotted codes and typed verdicts/problems; diagnostic prose is never matched semantically.

## Read next

- `crates/marrow-project/src/lib.rs` — `parse_config`, `check_under_root`, `expected_module_name`: the config schema and path-containment guarantee.
- `crates/marrow-check/src/project_io.rs` — shared project/catalog loading used by CLI commands.
- `crates/marrow/src/main.rs` — `load_checked_project`, `resolve_store_path`,
  `read_accepted_catalog_artifact`, `read_accepted_store_catalog`,
  `establish_store_baseline`: CLI wrappers and rendering boundaries around the
  shared loaders.
- `crates/marrow-run/src/project_session.rs` — `ProjectSession::open`, run/test admission, fence, drift auto-apply, test-case discovery, and `ProjectSession::invoke`.
- `crates/marrow/src/cmd_run.rs` — run flag parsing, session notice/error rendering, hook selection, and report emission.
- `crates/marrow/src/cmd_evolve/mod.rs` — `apply_cmd`: read store identity, freeze baseline when needed, preview, enforce recovery-point choice for Retire witnesses, apply (which publishes the catalog atomically).
- `crates/marrow/src/backup/restore.rs` — `read_backup_prologue` / `restore_backup_with_prologue`: catalog-section fingerprint gate, empty-only or counted replace target gate, transactional catalog+data replay, and verify-before-commit rollback.
- `crates/marrow/src/trace.rs` — `WriteTargetNames::from_program`, `render_write_target`: catalog ids to human names, shared by trace and dry-run.
