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
    Value,
    Store,
    Io,
    Config,
    Project,
}

impl Family {
    /// The first dotted segment codes in this family carry.
    pub const fn segment(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Fmt => "fmt",
            Self::Cli => "cli",
            Self::Check => "check",
            Self::Value => "value",
            Self::Store => "store",
            Self::Io => "io",
            Self::Config => "config",
            Self::Project => "project",
        }
    }

    /// The broad `kind` a tooling envelope reports for codes in this family. The
    /// first segment is not always the kind name (`value.*` is `runtime`), so the
    /// mapping is explicit.
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Check => "check",
            Self::Value => "runtime",
            Self::Store => "storage",
            Self::Io => "io",
            Self::Fmt | Self::Cli | Self::Config | Self::Project => "tooling",
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

/// Whether a code can be caught as an `Error` value inside a running `.mw`
/// program. Recoverable value-range and I/O faults are `Catchable`; static,
/// storage, and tooling codes never reach a running program as an `Error` and
/// are `NotApplicable`. The `Fatal` and `Conditional` classes have no members in
/// the current registry; the runtime faults they described return through a
/// later refounding lane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Catchability {
    Catchable,
    Fatal,
    Conditional,
    NotApplicable,
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
    ($($variant:ident => $string:expr, $family:ident, $severity:ident, $catch:ident, $life:ident, $meaning:expr);* $(;)?) => {
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

            /// Whether this code can be caught inside a running program.
            pub const fn catchability(self) -> Catchability {
                match self { $(Code::$variant => Catchability::$catch),* }
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
    ParseSyntax => r#"parse.syntax"#, Parse, Error, NotApplicable, Active, r#"The source is not well-formed Marrow: a bad token, a missing piece of a declaration, or an unexpected construct. The only `parse.*` code; the `message` says what was expected."#;
    FmtCommentLoss => r#"fmt.comment_loss"#, Fmt, Error, NotApplicable, Active, r#"`marrow fmt` would drop a retained comment while rewriting the source, so the command refuses instead of publishing lossy formatted output."#;
    CliCommandUnsupported => r#"cli.command_unsupported"#, Cli, Error, NotApplicable, Active, r#"A command name is recognized but not yet available on this beta line: its owning capability is being refounded and returns through a later lane. `marrow fmt`, `marrow --version`, and `marrow --help` are the currently available commands."#;
    CheckNestingLimit => r#"check.nesting_limit"#, Check, Error, NotApplicable, Active, r#"Source nests expressions or statement blocks deeper than the fixed parser limit (256). Raised by the parser at the offending span so pathologically nested source fails closed rather than overflowing the stack; see [execution limits](language/execution-limits.md)."#;
    ValueRange => r#"value.range"#, Value, Error, Catchable, Active, r#"A `date` or `instant` reaching the store codec lies outside Marrow's supported calendar range, years 0001-9999. This is a store-boundary integrity guard, not a source-arithmetic fault: every `.mw` temporal path (the `date`/`instant` constructors, `std::clock` parse and `addDays` helpers, and `+`/`-` arithmetic) shares the same 0001-9999 envelope and already raises `run.temporal_overflow` before an out-of-range value can be produced, so no ordinary checked program reaches this code. It fires only if a value that bypasses those bounds reaches the canonical encoder or key projection."#;
    StoreIo => r#"store.io"#, Store, Error, NotApplicable, Active, r#"An I/O operation on a persistent backend failed."#;
    StorePermissionDenied => r#"store.permission_denied"#, Store, Error, NotApplicable, Active, r#"The process lacks read/write access to the store directory or file. The message names the store path; grant access to that directory, then retry."#;
    StoreLocked => r#"store.locked"#, Store, Error, NotApplicable, Active, r#"The store file is held open by another process (a writer or a read-only inspection). Close the other process, then retry."#;
    StoreFormatVersion => r#"store.format_version"#, Store, Error, NotApplicable, Active, r#"The store's recorded format version is not the one this build supports."#;
    StoreCorruption => r#"store.corruption"#, Store, Error, NotApplicable, Active, r#"The store file or a tree-cell record is corrupt and could not be opened or decoded, including a truncated or torn store body."#;
    StoreRecoveryRequired => r#"store.recovery_required"#, Store, Error, NotApplicable, Active, r#"The store was not shut down cleanly, so a read-only open is refused until a write-capable open replays the interrupted commit. The recovery command returns with the refounded durable lifecycle; recovery is attempted, not guaranteed, and a store damaged beyond replay surfaces `store.corruption`."#;
    StoreLimit => r#"store.limit"#, Store, Error, NotApplicable, Active, r#"Marrow exhausted a fixed representational bound: a store framing length/count did not fit its `u32` field, a record/problem/index count overflowed, or the `u64` commit-ID sequence was exhausted."#;
    StoreCursor => r#"store.cursor"#, Store, Error, NotApplicable, Active, r#"A bounded scan cursor does not belong to the scan being resumed."#;
    StoreTransaction => r#"store.transaction"#, Store, Error, NotApplicable, Active, r#"A transaction or snapshot operation was requested in an invalid store state."#;
    StoreReadOnly => r#"store.read_only"#, Store, Error, NotApplicable, Active, r#"A write-capability operation was requested through a read-only store handle."#;
    IoRead => r#"io.read"#, Io, Error, Catchable, Active, r#"A read failed: a project source file or `marrow.toml` could not be read, or `std::io::readText`/`readBytes` failed."#;
    IoThread => r#"io.thread"#, Io, Error, NotApplicable, Active, r#"The CLI could not spawn the worker thread it uses for parsing, checking, and running."#;
    IoWrite => r#"io.write"#, Io, Error, Catchable, Active, r#"`std::io::writeText`/`writeBytes` failed."#;
    ConfigInvalid => r#"config.invalid"#, Config, Error, NotApplicable, Active, r#"A configuration input is invalid: the project manifest `marrow.toml` is malformed TOML, declares an unknown key, or declares no supported `edition`; or a command argument is not valid UTF-8. A malformed-manifest fault carries its `marrow.toml` line and column in `source_span`; a validation fault with no single source point carries none."#;
    ProjectSourcePath => r#"project.source_path"#, Project, Error, NotApplicable, Active, r#"A captured source file path is not a valid contained module identity: it is absolute, escapes the source root with `..`, is not a canonical forward-slash path, lives outside the fixed `src` source root, or is not a `.mw` file with a non-empty name."#;
    ProjectModuleCollision => r#"project.module_collision"#, Project, Error, NotApplicable, Active, r#"Two captured source files collide on module identity: they derive the same module name, or their paths differ only in case and would name the same file on a case-insensitive filesystem. The message names both files."#;
    ProjectCaptureLimit => r#"project.capture_limit"#, Project, Error, NotApplicable, Active, r#"A project capture exceeded a fixed bound: too many source files, one source file too large, or the source files together too large. The bound guards the compiler against an unbounded project tree."#;
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
        "value" => "runtime",
        "store" => "storage",
        "io" => "io",
        _ => "tooling",
    }
}

#[cfg(test)]
mod tests {
    use super::{Catchability, Code, Lifecycle, SeverityClass, kind_for_code};

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
    fn catchability_is_runtime_only() {
        let catchable: Vec<Code> = Code::ALL
            .iter()
            .copied()
            .filter(|c| c.catchability() != Catchability::NotApplicable)
            .collect();
        assert_eq!(
            catchable,
            [Code::ValueRange, Code::IoRead, Code::IoWrite],
            "the only codes that reach a running program as catchable Error values \
             are value.range, io.read, and io.write"
        );
        let conditional: Vec<Code> = Code::ALL
            .iter()
            .copied()
            .filter(|c| c.catchability() == Catchability::Conditional)
            .collect();
        assert!(
            conditional.is_empty(),
            "no code is Conditional after the shrink; the dual-constructed codes were deleted"
        );
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
