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

/// The severity a code renders under. Most codes are hard failures; a handful of
/// advisories are warnings that leave the command passing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeverityClass {
    Error,
    Warning,
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
    ($($variant:ident => $string:expr, $family:ident, $severity:ident, $life:ident, $meaning:expr);* $(;)?) => {
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

            /// The severity this code renders under.
            pub const fn severity_class(self) -> SeverityClass {
                match self { $(Code::$variant => SeverityClass::$severity),* }
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
    ParseSyntax => r#"parse.syntax"#, Parse, Error, Active, r#"The source is not well-formed Marrow: a bad token, a missing piece of a declaration, or an unexpected construct. The only `parse.*` code; the `message` says what was expected."#;
    FmtCommentLoss => r#"fmt.comment_loss"#, Fmt, Error, Active, r#"`marrow fmt` would drop a retained comment while rewriting the source, so the command refuses instead of publishing lossy formatted output."#;
    CliCommandUnsupported => r#"cli.command_unsupported"#, Cli, Error, Active, r#"A command name is recognized but not yet available on this beta line: its owning capability is being refounded and returns through a later lane. `marrow fmt`, `marrow --version`, and `marrow --help` are the currently available commands."#;
    CliTransferExcluded => r#"cli.transfer_excluded"#, Cli, Error, Active, r#"An export's signature reaches a value type outside the wire transfer graph — a finite collection, until the earned transfer extension lands — so the program's wire interface cannot be built. `marrow client typescript` and the stock runner refuse the whole program rather than serving a partial interface; the message names the export position that is excluded."#;
    CliDurableUnsupported => r#"cli.durable_unsupported"#, Cli, Error, Active, r#"`marrow run` resolved a durable export — one whose verified demand reads or writes durable data — that the beta line cannot yet execute. The export compiled, independently verified, and completed its durable identity, but the CLI no longer opens a store in process (T01's in-process open ended at D00, where the durable-run trough begins). Durable execution returns as the ephemeral-memory preview and later the persistent companion path. A storeless export is unaffected."#;
    CheckNestingLimit => r#"check.nesting_limit"#, Check, Error, Active, r#"Source nests expressions or statement blocks deeper than the fixed parser limit (256). Raised by the parser at the offending span so pathologically nested source fails closed rather than overflowing the stack; see [execution limits](language/execution-limits.md)."#;
    CheckUnsupported => r#"check.unsupported"#, Check, Error, Active, r#"A parsed construct is well-formed Marrow but outside the subset the beta line currently compiles. Its owning language capability is being refounded lane by lane and returns through a later one; until then the construct is absent by the capability trough, and the checker reports this at its span."#;
    CheckType => r#"check.type"#, Check, Error, Active, r#"An expression or declaration is not well-typed in the compiled subset: a return value whose type does not match the declared return type, an operator applied to the wrong operand type, a use of a name that is not in scope, or a value used where a different type is required."#;
    CheckNameConflict => r#"check.name_conflict"#, Check, Error, Active, r#"Two declarations collide on a name the compiler must resolve uniquely: two functions in one module share a name, or two declarations share an identifier in the same scope. The message names the colliding declarations."#;
    CheckModulePath => r#"check.module_path"#, Check, Error, Active, r#"A file's `module` header does not match the module name derived from its source-root-relative path. The path is the authority for module identity, so `src/shelf/books.mw` must declare `module shelf::books`; the message names the expected path."#;
    CheckImport => r#"check.import"#, Check, Error, Active, r#"A `use` import cannot be resolved: it names a module the project does not contain, or two imports in one module bind the same final segment and are ambiguous. The message names the offending import."#;
    CheckVisibility => r#"check.visibility"#, Check, Error, Active, r#"A call from one module names a function in another module that is not `pub`. A function without `pub` is callable only within its own module; mark it `pub` to expose it across the module boundary."#;
    CheckRecursion => r#"check.recursion"#, Check, Error, Active, r#"A definition is part of a cycle the language requires to be acyclic: a function on a direct or mutual recursion cycle (the compiled subset does not admit recursion), a type alias whose expansion reaches itself, or a value type (struct, record, or enum) that contains itself directly or transitively (an infinite value; recursive nominal values are deferred). The message names the cycle. This is reported at check time so the source, not the image, carries the diagnostic."#;
    CheckRequiresTransaction => r#"check.requires_transaction"#, Check, Error, Active, r#"A durable mutation runs where no ambient transaction is available. A durable write, replacement, or erase executes only inside a `transaction` block: an export that mutates owns one block around its mutations, and a helper that mutates is callable only from within a caller's transaction. The requirement propagates transitively — a function that calls a mutating function itself requires an ambient transaction — so calling a mutating helper, or performing a durable mutation, directly in an export body outside a `transaction` block is refused at check time at the mutation or call-site span. Wrap the mutation or the call in a `transaction` block."#;
    CheckAssertOutsideTest => r#"check.assert_outside_test"#, Check, Error, Active, r#"An `assert` statement appears outside a `test` declaration. `assert` is the test-owned assertion: it is legal only inside a `test "name"` body, never in an ordinary function. Move the assertion into a test, or use `unreachable("...")` for an in-program invariant fault."#;
    CheckTestDriverMix => r#"check.test_driver_mix"#, Check, Error, Active, r#"A `test` body both performs a durable operation directly and drives a transaction-owning export. A test body is one of two kinds: it performs durable reads and writes directly, running against the harness session, or it drives the application's exports, where each export call is its own invocation boundary — a mutating export commits and a later reading export observes the committed state. The two cannot be combined in one body, because the driven export's commit would consume the harness session the direct operation needs. Split the direct durable operations and the export driving into separate tests, or reach the durable data through the exports the test drives."#;
    CheckMatchNonexhaustive => r#"check.match_nonexhaustive"#, Check, Error, Active, r#"A `match` over an enum does not cover every selectable member of that enum. A flat enum's `match` must have exactly one arm per member and no wildcard arm; the message names the missing members. Add an arm for each uncovered member."#;
    CheckMatchArm => r#"check.match_arm"#, Check, Error, Active, r#"A `match` arm is not well-formed against its scrutinee enum: it names a member the enum does not declare, repeats a member another arm already covers, binds a number of payload names that does not match the member's payload, or the scrutinee is not an enum value. The message names the offending arm."#;
    CheckInstantiationLimit => r#"check.instantiation_limit"#, Check, Error, Active, r#"Monomorphizing a program requires more distinct generic instantiations, or deeper generic type nesting, than the fixed limit. A well-typed program with acyclic call and value-containment graphs mints finitely many instances; this bound (campaign law 9) fails a divergent monomorphization — a generic function that calls itself, or a generic type that nests inside itself, over an ever-growing type — with a typed error before the instantiation worklist or the minting recursion grows unboundedly."#;
    CheckDurableIdentity => r#"check.durable_identity"#, Check, Error, Active, r#"A durable declaration lacks its complete ledger identity: the store root, key column, stored resource, one of its fields, or the application itself has no matching entry in the machine-written `marrow.ids` identity artifact — or its `(kind, path)` names a retired identity that can never be reused. The message names the identity kind and path. `marrow run` mints missing identities into `marrow.ids` (commit that file); a retired path stays refused. `marrow.ids` is machine-written only and is never edited by hand."#;
    ImageEnvelope => r#"image.envelope"#, Image, Error, Active, r#"A program image failed envelope verification (phase 1): a bad magic or version, a digest that does not match the image bytes, a malformed or misordered section frame, a declared length past the input, or trailing bytes. The image is rejected before any table is read."#;
    ImageTable => r#"image.table"#, Image, Error, Active, r#"A program image failed table verification (phase 2): a string, type, durable, constant, function, export, or span table violates its grammar — a duplicate or unsorted entry, an out-of-range index, a bad type tag or flag, or an operation site that does not resolve against the declared roots and records."#;
    ImageFunction => r#"image.function"#, Image, Error, Active, r#"A program image failed per-function verification (phase 3): the bytecode does not decode to instruction boundaries, a jump leaves the function or lands off a boundary, an instruction is unreachable or a path falls off the end without returning, the typed operand stack does not agree at a merge or a return, a local is read before it is initialized, or a per-opcode rule is violated."#;
    ImageClosure => r#"image.closure"#, Image, Error, Active, r#"A program image failed call/effect-closure verification (phase 4): the call graph contains a cycle (recursion is not admitted), or a recorded call or effect does not close consistently across the function set."#;
    ImageFlow => r#"image.flow"#, Image, Error, Active, r#"A program image failed transaction-flow verification (phase 5): a transaction is begun outside an export entry, a mutation or mutating call sits outside the single owned transaction region, the region is not opened exactly once and closed on every path, or a read-only export contains a mutation."#;
    ImageTestEntry => r#"image.test_entry"#, Image, Error, Active, r#"A program image failed test-entry verification: the closed non-wire TEST-ENTRY table is malformed (an out-of-range or duplicate/unsorted name or function index), an `assert` instruction sits in a function that is not a test entry, or a test entry is also an export, takes parameters, does not return unit, reads or writes durable data, or is called by another function. A test entry is a storeless zero-argument entry point, never an export or durable identity."#;
    RunOverflow => r#"run.overflow"#, Run, Error, Active, r#"A checked integer operation overflowed the 64-bit range at runtime: an add, subtract, multiply, negate, or the `i64::MIN / -1` division and `i64::MIN % -1` remainder cases whose result is unrepresentable. The fault is mapped to the source span of the operation and is not catchable inside the program."#;
    RunDivideByZero => r#"run.divide_by_zero"#, Run, Error, Active, r#"A division or remainder operation had a zero divisor at runtime. The fault is mapped to the source span of the operation and is not catchable inside the program."#;
    RunTextLimit => r#"run.text_limit"#, Run, Error, Active, r#"A text concatenation would exceed the fixed 64 KiB result bound, so the operation faults rather than allocating unboundedly. Mapped to the source span of the concatenation and not catchable inside the program."#;
    RunUnreachable => r#"run.unreachable"#, Run, Error, Active, r#"A program reached an `unreachable("...")` statement, the sole application-declared invariant fault. The static text records the invariant the author believed held; reaching the statement means it did not. The fault is mapped to the statement's source span and is not catchable inside the program."#;
    RunTodo => r#"run.todo"#, Run, Error, Active, r#"A program reached a `todo("...")` statement, an unfinished path the author marked as not yet implemented. The static text names the deferred work. Like `unreachable`, `todo` diverges and satisfies return-path analysis; reaching it maps the fault to the statement's source span and is not catchable inside the program."#;
    RunAssert => r#"run.assert"#, Run, Error, Active, r#"A `test`'s `assert` condition was false at runtime, so the test fails. `marrow test` reports the test as failed and maps the fault to the assertion's source span. Only a `test` body can produce this fault; it is not catchable inside the program."#;
    RunCallDepth => r#"run.call_depth"#, Run, Error, Active, r#"Runtime call depth exceeded the fixed limit (64). Static recursion is already rejected at verification, so this guards a pathologically deep non-recursive call chain; mapped to the call site and not catchable inside the program."#;
    RunBudget => r#"run.budget"#, Run, Error, Active, r#"A running program exhausted the fixed per-invocation instruction budget, shared across the whole call tree so total work stays bounded regardless of loop or call structure. A non-terminating loop faults here rather than running forever. The fault stops execution and is not catchable inside the program."#;
    RunAuthority => r#"run.authority"#, Run, Error, Active, r#"An export's verified durable demand is not covered by the deployment ceiling intersected with the invocation grant, so the call is denied before the first engine access. The demand never grants access; it is only checked against it. Not catchable inside the program."#;
    RunRequiredMissing => r#"run.required_missing"#, Run, Error, Active, r#"A durable transaction reached its commit with an entry it created or staged that still leaves a required field unset. The transaction rolls back rather than committing a partial entry, and the fault is mapped to the transaction's source span. Not catchable inside the program."#;
    RunUniqueIndex => r#"run.unique_index"#, Run, Error, Active, r#"A durable write would place two distinct entries into one `unique` managed index — two rows whose unique projection is equal but which name different store identities. Managed-index maintenance detects the collision when it stages the row and faults, rolling the whole transaction back without poisoning the store. The fault is mapped to the operation's source span and is not catchable inside the program."#;
    RunCommit => r#"run.commit"#, Run, Error, Active, r#"A durable transaction commit did not confirm. The store handle is poisoned and every later operation fails; the process must exit and reopen, where the recorded witness classifies whether the commit completed. The fault is mapped to the transaction's source span and is not catchable inside the program."#;
    RunRange => r#"run.range"#, Run, Error, Active, r#"A value outside a nominal type's declared interval reached a construction or arithmetic result at runtime: `Age(n)` or a `supports`-unlocked operation produced an int the type's `in` range does not admit. The fault is mapped to the source span of the operation and is not catchable inside the program; use `Type.checked(n)` for a fault-free range test."#;
    RunCorruption => r#"run.corruption"#, Run, Error, Active, r#"A verified program hit an internally inconsistent artifact and failed closed rather than reading past it. The path kernel found the durable store inconsistent — a field leaf with no entry marker (an orphan leaf), a cell it could not decode as its typed value, or a stored schema descriptor that does not match the program image — or a bytecode positional collection read (a list element or a map key/value at an index) addressed a position past the collection's length. The compiler keeps every positional read in bounds, so an ordinary compiled program never reaches the collection case; it guards a hand-built or corrupted image whose index the verifier's type check does not bound. The fault is mapped to the operation's source span and is not catchable inside the program."#;
    RunEnumVariant => r#"run.enum_variant"#, Run, Error, Internal, r#"A defense-in-depth guard: a bytecode enum-payload read named a variant the running enum value did not select. The compiler dispatches on the enum tag before extracting a variant's payload, so ordinary compiled programs never reach this; it fails an image closed rather than reading a differently-typed payload leaf when a hand-built or corrupted image extracts the wrong variant. Mapped to the operation's source span and not catchable inside the program."#;
    RunCollectionLimit => r#"run.collection_limit"#, Run, Error, Active, r#"A `List` append or `Map` insert would grow a collection past a fixed representational bound: more than 65536 elements, or an aggregate value size over 1 MiB. The operation faults rather than allocating unboundedly, mapped to its source span, and is not catchable inside the program."#;
    RunTemporalOverflow => r#"run.temporal_overflow"#, Run, Error, Active, r#"A temporal operation produced a result outside its supported domain at runtime: `addDays` or `instant +/- duration` left the supported calendar range (years 0001-9999), or `duration +/- duration` overflowed the signed-nanosecond `i128` range. The fault is mapped to the source span of the operation and is not catchable inside the program. Every `.mw` temporal path shares this 0001-9999 / `i128` envelope, so an out-of-range value never escapes into a stored value or key."#;
    ValueRange => r#"value.range"#, Value, Error, Active, r#"A durable value could not be represented by the canonical store codec. Ordinary checked source can reach this at a durable write when a composite field's individually bounded scalar leaves exceed the dynamic 1 MiB aggregate encoded-value limit. The fault is mapped to the write span and is not catchable inside the program; encoding completes before any store write, so the rejected operation has no store effect. The same code also closes defense-in-depth codec range arms, including a date or instant outside years 0001-9999, although checked temporal source cannot produce those values."#;
    StoreIo => r#"store.io"#, Store, Error, Active, r#"An I/O operation on a persistent backend failed."#;
    StorePermissionDenied => r#"store.permission_denied"#, Store, Error, Active, r#"The process lacks read/write access to the store directory or file. The message names the store path; grant access to that directory, then retry."#;
    StoreLocked => r#"store.locked"#, Store, Error, Active, r#"The store file is held open by another process (a writer or a read-only inspection). Close the other process, then retry."#;
    StoreFormatVersion => r#"store.format_version"#, Store, Error, Active, r#"The store's recorded format version is not the one this build supports."#;
    StoreCorruption => r#"store.corruption"#, Store, Error, Active, r#"The store file or a tree-cell record is corrupt and could not be opened or decoded, including a truncated or torn store body."#;
    StoreRecoveryRequired => r#"store.recovery_required"#, Store, Error, Active, r#"The store was not shut down cleanly, so a read-only open is refused until a write-capable open replays the interrupted commit. The recovery command returns with the refounded durable lifecycle; recovery is attempted, not guaranteed, and a store damaged beyond replay surfaces `store.corruption`."#;
    StoreLimit => r#"store.limit"#, Store, Error, Active, r#"Marrow exhausted a fixed representational bound: a store framing length/count did not fit its `u32` field, a record/problem/index count overflowed, or the `u64` commit-ID sequence was exhausted."#;
    StoreCursor => r#"store.cursor"#, Store, Error, Active, r#"A bounded scan cursor does not belong to the scan being resumed."#;
    StoreTransaction => r#"store.transaction"#, Store, Error, Active, r#"A transaction or snapshot operation was requested in an invalid store state."#;
    StoreReadOnly => r#"store.read_only"#, Store, Error, Active, r#"A write-capability operation was requested through a read-only store handle."#;
    IoRead => r#"io.read"#, Io, Error, Active, r#"An operational read failed, such as reading a project source file, `marrow.toml`, a runner launch artifact, or a runner protocol frame."#;
    IoThread => r#"io.thread"#, Io, Error, Active, r#"The CLI could not spawn the worker thread it uses for parsing, checking, and running."#;
    IoWrite => r#"io.write"#, Io, Error, Active, r#"An operational write failed, such as creating an initialized project file, publishing a generated client or identity artifact, writing command output, or writing a runner protocol frame."#;
    ConfigInvalid => r#"config.invalid"#, Config, Error, Active, r#"A configuration input or project-setup precondition is invalid: the project manifest `marrow.toml` is malformed TOML, declares an unknown key, or declares no supported `edition`; a command argument is not valid UTF-8; or `marrow init` targets a directory that already exists. A malformed-manifest fault carries its `marrow.toml` line and column in `source_span`; a validation fault with no single source point carries none."#;
    ProjectSourcePath => r#"project.source_path"#, Project, Error, Active, r#"A captured source file path is not a valid contained module identity: it is absolute, escapes the source root with `..`, is not a canonical forward-slash path, contains a NUL or ASCII control character, lives outside the fixed `src` source root, or is not a `.mw` file with a non-empty name. A project whose `src` root is itself a symlink is refused with this code before discovery."#;
    ProjectModuleCollision => r#"project.module_collision"#, Project, Error, Active, r#"Two captured source files collide on module identity: they derive the same module name, or their paths differ only in case and would name the same file on a case-insensitive filesystem. The message names both files."#;
    ProjectCaptureLimit => r#"project.capture_limit"#, Project, Error, Active, r#"A project capture exceeded a fixed bound: too many source files, one source file too large, or the source files together too large. The bound guards the compiler against an unbounded project tree."#;
    ProjectIdsCorrupt => r#"project.ids_corrupt"#, Project, Error, Active, r#"The committed `marrow.ids` identity artifact is corrupt and is rejected whole, never half-read: unresolved Git conflict markers, a malformed or duplicate row, two rows claiming one `(kind, path)` anchor or one id (the signature of a conflicting double-mint on parallel branches), a retired id reissued by a live row, an inconsistent retirement high-water, a truncated (torn) file missing its end marker, or a size past the fixed artifact bound. `marrow.ids` is machine-written only: restore it from version control rather than editing it."#;
    ProjectIdsMint => r#"project.ids_mint"#, Project, Error, Active, r#"`marrow run` could not mint a missing durable identity: the OS entropy source was unavailable, or a freshly drawn id collided with an existing or retired one (minting never retries a draw). The `marrow.ids` artifact is left byte-for-byte unchanged; rerun to draw fresh entropy."#;
    WireFrameTooLarge => r#"wire.frame_too_large"#, Wire, Error, Active, r#"A local-wire frame declared a payload longer than the fixed maximum frame size, so the framed message is rejected before its body is read or allocated (campaign law 9). The single wire owner rejects an oversized frame rather than buffering unbounded bytes off the socket."#;
    WireDepthLimit => r#"wire.depth_limit"#, Wire, Error, Active, r#"A local-wire message's canonical JSON nests arrays or objects deeper than the fixed maximum depth, so decoding is refused before the structure is fully materialized (campaign law 9). The bound fails a pathologically nested payload closed rather than recursing unboundedly."#;
    WireStringLimit => r#"wire.string_limit"#, Wire, Error, Active, r#"A local-wire message's canonical JSON contains a string longer than the fixed maximum string size (campaign law 9). The bound fails an oversized string closed rather than allocating it."#;
    WireUnsupportedVersion => r#"wire.unsupported_version"#, Wire, Error, Active, r#"A local-wire frame carried a protocol version byte this build does not speak. The runner and the generated client are a matched release pair; a version this build does not recognize is rejected at the frame boundary before the body is interpreted."#;
    WireMalformed => r#"wire.malformed"#, Wire, Error, Active, r#"A local-wire frame body is not a well-formed protocol message: its bytes are not valid JSON, carry a fractional or exponent number Marrow has no value for, name an unknown message kind, omit a required field, use a field of the wrong JSON type, or leave trailing bytes after the value. The single wire owner rejects it rather than acting on a partially understood message."#;
    WireNoncanonical => r#"wire.noncanonical"#, Wire, Error, Active, r#"A local-wire frame body is valid JSON but not in canonical form: it carries insignificant whitespace, object keys that are unsorted or duplicated, a non-minimal number spelling, or a non-canonical string escape. The single wire owner accepts only the one canonical encoding so a message has exactly one byte spelling."#;
    RunnerHandshake => r#"runner.handshake"#, Runner, Error, Active, r#"A local-wire connection failed the runner handshake and was closed fail-closed: the connecting peer did not present the expected launch nonce, spoke an unsupported protocol version, or sent a malformed hello. No session is established and no request is served over the connection."#;
    RunnerUnknownExport => r#"runner.unknown_export"#, Runner, Error, Active, r#"A local-wire request named an export identity the served program image does not carry. The runner dispatches only on a verified export id present in the image it was launched with; an unknown id is rejected without running anything."#;
    RunnerArgMismatch => r#"runner.arg_mismatch"#, Runner, Error, Active, r#"A local-wire request's arguments do not match the target export's verified signature: the argument count differs, or an argument value does not decode into the declared parameter type. The runner rejects the request before running rather than coercing a mismatched value."#;
    RunnerDurableUnsupported => r#"runner.durable_unsupported"#, Runner, Error, Active, r#"A local-wire request named an export whose verified demand reads or writes durable data. The stock runner executes only storeless exports on this beta line; durable execution returns with the ephemeral-memory attachment and later the persistent companion path. A storeless export is unaffected."#;
    RunnerReplyEncode => r#"runner.reply_encode"#, Runner, Error, Internal, r#"A defense-in-depth guard: a served export's return value failed to encode for the wire. Interface build excludes an export whose return shape is not transferable, so ordinary served programs never reach this; the runner fails the request closed rather than emitting a partial reply."#;
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
    use super::{Code, Lifecycle, SeverityClass, kind_for_code};

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
    fn warnings_are_advisories() {
        let warnings: Vec<&str> = Code::ALL
            .iter()
            .filter(|c| c.severity_class() == SeverityClass::Warning)
            .map(|c| c.as_str())
            .collect();
        assert!(
            warnings.is_empty(),
            "no retained code carries Warning severity after the shrink, found {warnings:?}"
        );
    }
}
