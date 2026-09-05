//! The Marrow diagnostic code registry: the single owner of every dotted error
//! code string, its family, documented meaning, and static classification.
//!
//! A [`Code`] variant is the one place a diagnostic code exists. Every crate that
//! emits a code names the variant and renders the wire string through
//! [`Code::as_str`], so a code string is spelled exactly once in the whole
//! toolchain. The reference page `docs/error-codes.md` is generated from this
//! registry by [`generate`]; a drift test keeps the two byte-identical, so the
//! meaning prose lives here as the single source and the page cannot diverge.

mod docs;
pub use docs::generate;

/// The family a code belongs to, named by the first dotted segment of its string.
/// The family fixes the tooling [`Family::kind`] a code reports.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Family {
    Parse,
    Fmt,
    Cli,
    Check,
    Image,
    Run,
    Value,
    Store,
    Io,
    Config,
    Project,
    Wire,
    Runner,
}

impl Family {
    /// The first dotted segment codes in this family carry.
    pub const fn segment(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Fmt => "fmt",
            Self::Cli => "cli",
            Self::Check => "check",
            Self::Image => "image",
            Self::Run => "run",
            Self::Value => "value",
            Self::Store => "store",
            Self::Io => "io",
            Self::Config => "config",
            Self::Project => "project",
            Self::Wire => "wire",
            Self::Runner => "runner",
        }
    }

    /// The broad `kind` a tooling envelope reports for codes in this family. The
    /// first segment is not always the kind name (`value.*` is `runtime`), so the
    /// mapping is explicit.
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Check => "check",
            Self::Image => "artifact",
            Self::Run => "runtime",
            Self::Value => "runtime",
            Self::Store => "storage",
            Self::Io => "io",
            Self::Fmt | Self::Cli | Self::Config | Self::Project | Self::Wire | Self::Runner => {
                "tooling"
            }
        }
    }
}

/// Whether a code is emitted by the current build, and how it reaches a user. An
/// `Active` code is emitted and has a public product surface: a CLI or tooling
/// path an ordinary Marrow user can reach. An `Internal` code is emitted only by
/// an implementation-maintainer surface or as a defense-in-depth fail-closed
/// guard over an invariant the surrounding layers already close. The reference
/// renders internal codes separately from ordinary user-facing diagnostics.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lifecycle {
    Active,
    Internal,
}

macro_rules! codes {
    ($($variant:ident => $string:expr, $family:ident, $life:ident, $meaning:expr);* $(;)?) => {
        /// A diagnostic code: the single typed identity for one dotted error-code
        /// string. Construct the wire string with [`Code::as_str`].
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
        pub enum Code {
            $($variant),*
        }

        impl Code {
            /// Every registered code, in `docs/error-codes.md` order.
            pub const ALL: &'static [Code] = &[$(Code::$variant),*];

            /// The canonical dotted string, spelled once here for the whole toolchain.
            pub const fn as_str(self) -> &'static str {
                match self { $(Code::$variant => $string),* }
            }

            /// The family this code belongs to.
            pub const fn family(self) -> Family {
                match self { $(Code::$variant => Family::$family),* }
            }

            /// Whether the current build emits this code or reserves it.
            pub const fn lifecycle(self) -> Lifecycle {
                match self { $(Code::$variant => Lifecycle::$life),* }
            }

            /// The documented meaning, the single source of the code's reference prose.
            pub const fn meaning(self) -> &'static str {
                match self { $(Code::$variant => $meaning),* }
            }

            /// The registered code for a wire string, if any.
            pub fn from_code(string: &str) -> Option<Code> {
                match string { $($string => Some(Code::$variant),)* _ => None }
            }
        }
    };
}

codes! {
    ParseSyntax => r#"parse.syntax"#, Parse, Active, r#"The source is not well-formed Marrow: a bad token, a missing piece of a declaration, or an unexpected construct. The only `parse.*` code; the message says what was expected."#;
    FmtCommentLoss => r#"fmt.comment_loss"#, Fmt, Active, r#"`marrow fmt` would drop a comment while rewriting the file, so it writes nothing."#;
    FmtDiagnosticLimit => r#"fmt.diagnostic_limit"#, Fmt, Active, r#"`marrow fmt` needs a complete parse, and the file produced more parse diagnostics than the collector keeps (4096 diagnostics or 1 MiB of text), so it writes nothing. Fix the parse errors, then format."#;
    CliCommandUnsupported => r#"cli.command_unsupported"#, Cli, Active, r#"The command name is reserved and not implemented: `data`, `doctor`, `evolve`, `serve`, `backup`, and `restore`. `marrow --help` lists the implemented commands."#;
    CliCompilerInvariant => r#"cli.compiler_invariant"#, Cli, Internal, r#"The compiler detected an internal state inconsistency and failed closed without producing a program image or source diagnostic."#;
    CliCompilerResourceLimit => r#"cli.compiler_resource_limit"#, Cli, Active, r#"Compilation crossed a fixed bound that no single construct is at fault for: an aggregate count across the whole program, or the image byte ceiling. No image is produced and the outcome carries no source location. When function bodies alone exceed the image byte ceiling, checking stops at that body; diagnostics found before the stop are not reported and reappear once the program fits. A bound one construct crosses is `check.resource_limit` at that construct."#;
    CliInterfaceUnbuildable => r#"cli.interface_unbuildable"#, Cli, Active, r#"An export's signature cannot be projected onto the wire: it expands past the fixed interface budget, or it names a type the image does not declare. `marrow client typescript` and the runner refuse the whole program; the message names the export."#;
    CliDurableUnsupported => r#"cli.durable_unsupported"#, Cli, Active, r#"`marrow run` resolved an export that reads or writes durable data, and no store was given. `marrow` itself opens no store; the companion runner does. Run the export against a provisioned store: `marrow run <export> --store <dir>`. A storeless export is unaffected."#;
    CliInstallationDamaged => r#"cli.installation_damaged"#, Cli, Active, r#"`marrow run --store` could not use the companion runner: the release manifest beside the toolchain is missing or malformed, names another release, or the runner binary is absent or does not match its recorded identity. The store is untouched. Reinstall the toolchain."#;
    CliCeilingUnaccepted => r#"cli.ceiling_unaccepted"#, Cli, Active, r#"`marrow image` writes an image only when `--accept-ceiling <id>` names the image's own deployment ceiling. The argument was absent or named a different id, so no image was written. The message prints the id to accept."#;
    CheckNestingLimit => r#"check.nesting_limit"#, Check, Active, r#"Source nests expressions or blocks deeper than the parser limit (256). Reported at the offending span. The limit is listed under [execution limits](language/execution-limits.md)."#;
    CheckUnsupported => r#"check.unsupported"#, Check, Active, r#"The construct is well-formed Marrow that this compiler does not implement today. Reported at the construct's span. [Status](status.md) lists what is available."#;
    CheckType => r#"check.type"#, Check, Active, r#"An expression or declaration is not well-typed: a return value of the wrong type, an operator applied to the wrong operand type, a name that is not in scope, or a value used where another type is required."#;
    CheckNameConflict => r#"check.name_conflict"#, Check, Active, r#"Two declarations share a name in one scope: two functions in one module, or two declarations with one identifier. The message names both."#;
    CheckModulePath => r#"check.module_path"#, Check, Active, r#"A file's `module` header does not match the name derived from its path under `src`. `src/shelf/books.mw` declares `module shelf::books`; the message names the expected path."#;
    CheckImport => r#"check.import"#, Check, Active, r#"A `use` import names a module the project does not contain, or two imports in one module bind the same final segment. The message names the import."#;
    CheckVisibility => r#"check.visibility"#, Check, Active, r#"A call from one module names a function in another module that is not `pub`. A function without `pub` is callable only within its own module; mark it `pub` to call it from elsewhere."#;
    CheckRecursion => r#"check.recursion"#, Check, Active, r#"A definition is part of a cycle: a function that calls itself directly or through other functions, a type alias that expands to itself, or a struct, resource, or enum that contains itself. Marrow admits none of these. The message names the cycle."#;
    CheckRequiresTransaction => r#"check.requires_transaction"#, Check, Active, r#"A durable write, replacement, or delete runs outside a `transaction` block. A mutating export owns one block around its writes. A mutating helper is called only from inside a caller's block, and a function that calls one needs a block in turn. Reported at the write or the call; wrap it in a `transaction` block."#;
    CheckTransactionOwnerCalled => r#"check.transaction_owner_called"#, Check, Active, r#"A function calls an export that owns a `transaction` block. An owner's block begins and commits in its own frame and does not nest inside a caller's. Only a `test` body drives an owner, one transaction per call. Move the durable work into a helper without a block and call it inside the export's own block."#;
    CheckTransactionEmpty => r#"check.transaction_empty"#, Check, Active, r#"A `transaction` block performs no durable operation, directly or through a call. Such a block commits nothing. Remove it, or move the durable work inside it."#;
    CheckTransactionReopened => r#"check.transaction_reopened"#, Check, Active, r#"A mutating export opens a second `transaction` block. An export owns exactly one block and commits it on every path. Combine the durable work into one block."#;
    CheckTransactionUncommitted => r#"check.transaction_uncommitted"#, Check, Active, r#"A path leaves a `transaction` block without committing it. The block commits at each `return` written inside it and at its closing brace. A `try` or `require` guard whose `err` exit would return from inside the block bypasses both. Spell a deliberate failure as a `return` inside the block, and place a guard that fails without committing before the block."#;
    CheckDurableAfterCommit => r#"check.durable_after_commit"#, Check, Active, r#"A durable read or write follows the commit of a `transaction` block on some path, directly or through a call. Move the operation inside the block, or capture the value into a local before the block closes and return the local."#;
    CheckTransactionMisplaced => r#"check.transaction_misplaced"#, Check, Active, r#"A `transaction` block appears in a helper that is not `pub` or in a `test` body. Only an export owns a block: a helper runs inside its caller's block, and a test drives exports or touches durable data directly. Move the block to the export that owns the durable work."#;
    CheckAssertOutsideTest => r#"check.assert_outside_test"#, Check, Active, r#"An `assert` statement appears outside a `test` body. Move it into a test, or use `unreachable("...")` for an invariant inside a function."#;
    CheckTestDriverMix => r#"check.test_driver_mix"#, Check, Active, r#"A `test` body both touches durable data directly and calls an export that owns a `transaction` block. A body does one or the other: it reads and writes `^` places itself, or it drives exports, where each call commits on its own. Split the two into separate tests, or reach the data through the exports."#;
    CheckMatchNonexhaustive => r#"check.match_nonexhaustive"#, Check, Active, r#"A `match` over an enum does not cover every member. A `match` has exactly one arm per member and no wildcard arm. The message names the missing members."#;
    CheckMatchArm => r#"check.match_arm"#, Check, Active, r#"A `match` arm names a member the enum does not declare, repeats a member another arm covers, or binds the wrong number of payload names; or the value matched is not an enum. The message names the arm."#;
    CheckInstantiationLimit => r#"check.instantiation_limit"#, Check, Active, r#"Instantiating the program's generic functions and types needs more distinct instances, or deeper type nesting, than the fixed limit. A generic function that calls itself, or a generic type that nests inside itself, over an ever-growing type reaches it."#;
    CheckResourceLimit => r#"check.resource_limit"#, Check, Active, r#"One construct crosses a fixed bound of the program image: a declaration too wide, a stored value or member tree too deep, a key tuple or index too long, or a function body or string too large. Reported at the construct; the bounds are listed under [execution limits](language/execution-limits.md). An aggregate exhaustion with no single construct at fault is `cli.compiler_resource_limit`."#;
    CheckDurableIdentity => r#"check.durable_identity"#, Check, Active, r#"A durable declaration has no identity in `.marrow/ids`: the store root, a key component, the stored resource, one of its fields, or the application itself has no entry there, or names a retired one. The message names the kind and path. `marrow run` mints missing identities into `.marrow/ids`; commit that file. A retired path stays refused. The file is machine-written."#;
    ImageEnvelope => r#"image.envelope"#, Image, Active, r#"A program image failed envelope verification (phase 1): a bad magic or version, a digest that does not match the bytes, a malformed or misordered section, a length past the input, or trailing bytes. Nothing else is read."#;
    ImageTable => r#"image.table"#, Image, Active, r#"A program image failed table verification (phase 2): the string, type, durable, constant, function, export, or span table breaks its grammar with a duplicate or unsorted entry, an out-of-range index, a bad type tag or flag, or a durable operation that does not resolve against the declared roots."#;
    ImageFunction => r#"image.function"#, Image, Active, r#"A program image failed function verification (phase 3): bytecode that does not decode to instruction boundaries, a jump that leaves the function or targets a non-boundary, an unreachable instruction, a path that falls off the end, an operand stack that disagrees at a merge or return, a local read before it is set, or a broken per-opcode rule."#;
    ImageClosure => r#"image.closure"#, Image, Active, r#"A program image failed call and effect closure (phase 4): the call graph contains a cycle, or a recorded call or effect does not close consistently across the functions."#;
    ImageFlow => r#"image.flow"#, Image, Active, r#"A program image failed transaction-flow verification (phase 5): a transaction begun outside an export, a write outside the export's one owned block, a block not opened once and closed on every path, or a read-only export that writes. These are the rules `check.transaction_*` reports at source."#;
    ImageTestEntry => r#"image.test_entry"#, Image, Active, r#"A program image failed test-entry verification: the test-entry table is malformed, an `assert` sits in a function that is not a test, a test entry is an export, takes parameters, returns a value, or is called by another function, or a test body both touches durable data directly and drives a transaction-owning export."#;
    RunOverflow => r#"run.overflow"#, Run, Active, r#"A checked integer operation overflowed 64 bits: an add, subtract, multiply, or negate, or the `i64::MIN / -1` division and `i64::MIN % -1` remainder."#;
    RunDivideByZero => r#"run.divide_by_zero"#, Run, Active, r#"A division or remainder had a zero divisor."#;
    RunTextLimit => r#"run.text_limit"#, Run, Active, r#"A text concatenation would exceed the 64 KiB result bound."#;
    RunUnreachable => r#"run.unreachable"#, Run, Active, r#"The program reached an `unreachable("...")` statement. The text records the invariant the author believed held."#;
    RunTodo => r#"run.todo"#, Run, Active, r#"The program reached a `todo("...")` statement. The text names the deferred work."#;
    RunAssert => r#"run.assert"#, Run, Active, r#"A `test`'s `assert` condition was false, so the test fails. Only a test body produces this fault."#;
    RunCallDepth => r#"run.call_depth"#, Run, Active, r#"The call chain grew deeper than the fixed limit (64). Recursion is refused at check time, so this guards a very deep chain of distinct calls."#;
    RunBudget => r#"run.budget"#, Run, Active, r#"The invocation exhausted its fixed instruction budget (2^26 instructions), which is shared across the whole call tree. A loop that never terminates faults here."#;
    RunAuthority => r#"run.authority"#, Run, Active, r#"An export's durable demand is not covered by the store's ceiling intersected with the invocation grant, so the call is denied before the first store access. Demand never grants access; it is only checked against it."#;
    RunRequiredMissing => r#"run.required_missing"#, Run, Active, r#"A `transaction` block reached its commit with an entry it created or staged that still has a required field unset. The block rolls back before any store write. The invocation reports `incomplete` with durable state `known_old`, at the block's span."#;
    RunUniqueIndex => r#"run.unique_index"#, Run, Active, r#"A write would place two entries whose indexed values are equal but whose identities differ into one `unique` index. The whole transaction rolls back and the store is unchanged."#;
    RunCommit => r#"run.commit"#, Run, Active, r#"A commit did not complete. A confirmed abort leaves durable state unchanged (`known_old`). An indeterminate result is classified after the store is reopened and audited as `known_old`, `known_new`, or `unknown`. The invocation returns no value and is never retried. Reported at the block's span."#;
    RunOutcomeUnknown => r#"run.outcome_unknown"#, Run, Active, r#"A call was dispatched to the runner, but the caller could not accept one exact valid reply: a socket-read failure, a malformed frame, a mismatched turn, an unsolicited message, or a reply that did not decode. The call may have run, wholly or partly, and is never retried. Run a read-only export to observe durable state before acting again."#;
    RunRange => r#"run.range"#, Run, Active, r#"A value outside a nominal type's declared interval reached a construction or arithmetic result: `Age(n)` or a `supports` operation produced an int the type's `in` range does not admit. `Age.checked(n)` tests the range without faulting."#;
    RunCorruption => r#"run.corruption"#, Run, Active, r#"A verified program found the store or the image inconsistent and stopped: a field leaf with no entry marker, a cell that does not decode as its type, a stored schema that does not match the image, or a positional collection read past the collection's length. The compiler keeps every positional read in bounds, so the last case guards a hand-built or corrupted image."#;
    RunEnumVariant => r#"run.enum_variant"#, Run, Internal, r#"A bytecode enum-payload read named a member the value did not select. The compiler dispatches on the tag before reading a payload, so a compiled program does not reach this; it guards a hand-built or corrupted image."#;
    RunCollectionLimit => r#"run.collection_limit"#, Run, Active, r#"A `List` append or `Map` insert would grow a collection past 65,536 elements or 1 MiB."#;
    RunTemporalOverflow => r#"run.temporal_overflow"#, Run, Active, r#"A temporal operation left its supported domain: `addDays` or an `instant` plus or minus a `duration` left the years 0001-9999, or a `duration` sum overflowed the signed nanosecond range. Every temporal value shares this envelope, so an out-of-range value never reaches a stored value or key."#;
    ValueRange => r#"value.range"#, Value, Active, r#"A durable value cannot be represented by the store codec: at a durable write, a composite field's individually bounded scalar leaves exceed the dynamic 1 MiB aggregate encoded-value limit. Encoding completes before any store write, so the rejected write has no store effect. The same code closes codec range arms, such as a date outside 0001-9999, that checked source cannot produce."#;
    StoreIo => r#"store.io"#, Store, Active, r#"An I/O operation on a store failed."#;
    StorePermissionDenied => r#"store.permission_denied"#, Store, Active, r#"The process lacks read/write access to the store directory or file. The message names the store path; grant access to that directory, then retry."#;
    StoreLocked => r#"store.locked"#, Store, Active, r#"The store file is held open by another process (a writer or a read-only inspection). Close the other process, then retry."#;
    StoreFormatVersion => r#"store.format_version"#, Store, Active, r#"The store records a format version this build does not support."#;
    StoreCorruption => r#"store.corruption"#, Store, Active, r#"The store file or one of its cells is corrupt and could not be opened or decoded, including a truncated or torn store body."#;
    StoreRecoveryRequired => r#"store.recovery_required"#, Store, Active, r#"The store was left unclean by an interrupted shutdown, and a read-only open cannot repair it. A writing open recovers it; recovery replays no Marrow code and retries no invocation. If recovery cannot produce an openable store, the open reports `store.corruption`."#;
    StoreLimit => r#"store.limit"#, Store, Active, r#"A fixed bound of the store's representation is exhausted: a framing length or count that does not fit its field, an entry, problem, or index count that overflowed, or an exhausted commit-witness generation."#;
    StoreCursor => r#"store.cursor"#, Store, Active, r#"A traversal cursor does not belong to the traversal being resumed."#;
    StoreTransaction => r#"store.transaction"#, Store, Active, r#"A transaction or snapshot operation was requested in an invalid store state."#;
    StoreReadOnly => r#"store.read_only"#, Store, Active, r#"A write was requested through a read-only store handle."#;
    StoreContractChanged => r#"store.contract_changed"#, Store, Active, r#"The program image changes the durable contract or the exported interface versus the store's active binding, so it is not a code-only update. The store is intact and the prior program remains usable. Accepting a changed contract is future work; today a new store is provisioned from the new program. [Changing the program](operations/README.md#changing-the-program) describes the outcomes."#;
    StoreDemandExceedsCeiling => r#"store.demand_exceeds_ceiling"#, Store, Active, r#"The program image's durable demand exceeds the ceiling the store was provisioned under. The message names, for each place beyond the ceiling, the export, the effect (read, write, presence, delete, or iterate), and the place. No store call is made and the store is intact. Expand the store's accepted ceiling to cover the named demand before running the new program."#;
    IoRead => r#"io.read"#, Io, Active, r#"An operational read failed, such as reading a project source file, `marrow.toml`, a runner launch artifact, or a runner protocol frame."#;
    IoThread => r#"io.thread"#, Io, Active, r#"The CLI could not spawn the worker thread it uses for parsing, checking, and running."#;
    IoWrite => r#"io.write"#, Io, Active, r#"An operational write failed, such as creating an initialized project file, publishing a generated client or identity artifact, writing command output, or writing a runner protocol frame."#;
    ConfigInvalid => r#"config.invalid"#, Config, Active, r#"The project manifest `marrow.toml` is malformed TOML, declares an unknown key, or declares no supported `edition`; a command argument is not valid UTF-8; or `marrow init` targets a directory that already exists. A malformed manifest reports its line and column."#;
    ProjectSourcePath => r#"project.source_path"#, Project, Active, r#"A source file path is not a valid module identity: it is absolute, escapes `src` with `..`, is not a canonical forward-slash path, contains a NUL or control character, lives outside `src`, is not a `.mw` file with a non-empty name, or exceeds 4096 bytes. A project whose `src` is a symlink reports this before discovery."#;
    ProjectModuleCollision => r#"project.module_collision"#, Project, Active, r#"Two source files collide on module identity: they derive the same module name, or their paths differ only in case and would name the same file on a case-insensitive filesystem. The message names both files."#;
    ProjectCaptureLimit => r#"project.capture_limit"#, Project, Active, r#"A project capture exceeded a fixed bound: too many source files, one source file too large, or the source files together too large."#;
    ProjectIdsCorrupt => r#"project.ids_corrupt"#, Project, Active, r#"`.marrow/ids` is corrupt and is rejected whole: unresolved Git conflict markers, a malformed or duplicate line, two lines claiming one `(kind, path)` or one id (a double mint on parallel branches), a retired id reissued, an inconsistent retirement high-water, a truncated file missing its end marker, or a size past the fixed bound. Restore the file from version control."#;
    ProjectIdsMint => r#"project.ids_mint"#, Project, Active, r#"`marrow run` could not mint missing identities: an anchor was invalid, duplicated, live, or retired; the ledger would exceed its fixed size; the entropy source failed; or a candidate id collided. `.marrow/ids` is unchanged. Fix the source or the ledger state, then run again; an entropy failure or a collision may pass on another attempt."#;
    ProjectIdsLocation => r#"project.ids_location"#, Project, Active, r#"The identity ledger was found at the retired path `marrow.ids`. Its home is `.marrow/ids`: move it with `git mv marrow.ids .marrow/ids` and commit the move. When both exist, keep `.marrow/ids` and delete the root file; a project has exactly one ledger."#;
    ProjectIdsPublicationPending => r#"project.ids_publication_pending"#, Project, Active, r#"A `.marrow/ids` publication marker is live, so no command reads the ledger. `.marrow/ids.pending` means a publication was interrupted; `marrow run` settles it before it reads the project. A stray `.marrow/ids.pending.create` is not settled automatically: delete it and `.marrow/ids.publish.stage`, then run again."#;
    WireFrameTooLarge => r#"wire.frame_too_large"#, Wire, Active, r#"A frame declared a payload longer than the maximum frame size. It is rejected before its body is read."#;
    WireDepthLimit => r#"wire.depth_limit"#, Wire, Active, r#"A message's JSON nests deeper than the maximum depth. Decoding stops before the structure is built."#;
    WireStringLimit => r#"wire.string_limit"#, Wire, Active, r#"A message's JSON contains a string longer than the maximum string size."#;
    WireUnsupportedVersion => r#"wire.unsupported_version"#, Wire, Active, r#"A frame carried a protocol version this build does not speak. The runner and the generated client are a matched release pair."#;
    WireMalformed => r#"wire.malformed"#, Wire, Active, r#"A frame body is not a well-formed message: not valid JSON, a fractional or exponent number, an unknown message kind, a missing or mistyped field, or trailing bytes."#;
    WireNoncanonical => r#"wire.noncanonical"#, Wire, Active, r#"A frame body is valid JSON but not canonical: insignificant whitespace, unsorted or duplicate keys, a non-minimal number, or a non-canonical escape. A message has exactly one byte spelling."#;
    RunnerHandshake => r#"runner.handshake"#, Runner, Active, r#"A connection failed the handshake: the peer did not present the launch nonce, spoke an unsupported version, or sent a malformed hello. No session is established."#;
    RunnerUnknownExport => r#"runner.unknown_export"#, Runner, Active, r#"A request named an export the served image does not carry. Nothing runs."#;
    RunnerArgMismatch => r#"runner.arg_mismatch"#, Runner, Active, r#"A request's arguments do not match the export's signature: the count differs, or a value does not decode as the parameter type. Nothing runs."#;
    RunnerDurableUnsupported => r#"runner.durable_unsupported"#, Runner, Active, r#"A request named a durable export the runner cannot serve: the storeless serve mode has no store, or the program's durable shape is one the runner does not execute today. A storeless export, and a durable export over a provisioned store, are unaffected."#;
    RunnerReplyEncode => r#"runner.reply_encode"#, Runner, Internal, r#"A served export's return value failed to encode for the wire. Interface build excludes an export whose return shape is not transferable, so a served program does not reach this; the request fails closed."#;
    RunnerSpawn => r#"runner.spawn"#, Runner, Active, r#"The `marrow` process could not start the companion runner for a persistent run. The store is untouched."#;
}

impl Code {
    /// The tooling `kind` for this code, derived from its family.
    pub const fn kind(self) -> &'static str {
        self.family().kind()
    }
}

/// The tooling `kind` for any dotted code string, including ones the registry
/// does not name (reserved look-alikes or codes minted outside the toolchain).
/// A registered code resolves through its typed family; an unknown string falls
/// back to first-segment classification so the mapping stays total. Generic
/// string consumers, such as the language server, call this.
pub fn kind_for_code(code: &str) -> &'static str {
    if let Some(code) = Code::from_code(code) {
        return code.kind();
    }
    match code.split('.').next().unwrap_or("") {
        "parse" => "parse",
        "check" => "check",
        "image" => "artifact",
        "run" => "runtime",
        "value" => "runtime",
        "store" => "storage",
        "io" => "io",
        _ => "tooling",
    }
}

#[cfg(test)]
mod tests {
    use super::Family;
    use super::{Code, Lifecycle, kind_for_code};

    #[test]
    fn strings_are_unique_and_round_trip() {
        let mut seen = std::collections::BTreeSet::new();
        for &code in Code::ALL {
            assert!(
                seen.insert(code.as_str()),
                "duplicate code string {}",
                code.as_str()
            );
            assert_eq!(Code::from_code(code.as_str()), Some(code));
        }
    }

    #[test]
    fn string_starts_with_family_segment() {
        for &code in Code::ALL {
            let prefix = format!("{}.", code.family().segment());
            assert!(
                code.as_str().starts_with(&prefix),
                "code {} does not start with family segment {}",
                code.as_str(),
                code.family().segment()
            );
        }
    }

    #[test]
    fn kind_for_code_matches_family() {
        for &code in Code::ALL {
            assert_eq!(kind_for_code(code.as_str()), code.kind());
        }
        assert_eq!(kind_for_code("unknown.family"), "tooling");
        assert_eq!(kind_for_code("value.range"), "runtime");
    }

    #[test]
    fn retained_runtime_codes_describe_their_actual_source_channels() {
        let value_range = Code::ValueRange.meaning();
        assert!(
            value_range.contains("1 MiB") && value_range.contains("aggregate"),
            "value.range must document the source-reachable aggregate durable-value bound"
        );
        assert!(
            !value_range.contains("no ordinary checked program reaches"),
            "value.range is reachable through an ordinary checked durable write"
        );

        for code in [Code::IoRead, Code::IoWrite] {
            let meaning = code.meaning();
            assert!(
                !meaning.contains("std::io") && !meaning.contains("catchable"),
                "{} is an operational tooling code, not a source Error channel",
                code.as_str()
            );
        }
    }

    /// Every registered code renders into the generated reference, in the section
    /// its lifecycle names. Without this, a variant added to the table but dropped
    /// from the generator's layout would vanish from the page while the byte-exact
    /// drift gate stayed green.
    #[test]
    fn generated_reference_covers_every_code_in_its_section() {
        let generated = crate::generate();
        let (active_part, internal_part) = generated
            .split_once(crate::docs::INTERNAL_HEADING)
            .expect("generated reference has the internal-codes section");
        for &code in Code::ALL {
            let row_prefix = format!("| `{}` |", code.as_str());
            let (section, name) = match code.lifecycle() {
                Lifecycle::Active => (active_part, "active"),
                Lifecycle::Internal => (internal_part, "internal"),
            };
            assert!(
                section.contains(&row_prefix),
                "{} is missing from the {name} section of the generated reference",
                code.as_str()
            );
        }
    }

    #[test]
    fn compiler_invariant_is_one_internal_cli_error() {
        let code = Code::CliCompilerInvariant;
        assert_eq!(code.as_str(), "cli.compiler_invariant");
        assert_eq!(code.family(), Family::Cli);
        assert_eq!(code.lifecycle(), Lifecycle::Internal);
        assert_eq!(
            code.meaning(),
            "The compiler detected an internal state inconsistency and failed closed without \
             producing a program image or source diagnostic."
        );
    }
}
