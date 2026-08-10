//! The query-local syntax policy: `completions` and `active_call` re-parse exactly one
//! already-admitted file's already-retained bytes per query.
//!
//! Three properties are pinned here.
//!
//! **Outcome agreement.** Parsing is a pure function of the source bytes, so an outcome
//! derived from a per-query parse equals the one the superseded retained trees produced.
//! This file freezes a corpus of rendered outcomes covering the classification cases
//! those trees served — clean files, a recovered-broken file, a file that never decoded —
//! so a divergence is a failing assertion rather than an assumption.
//!
//! **The parse-transient term.** `MAX_QUERY_PARSE_TRANSIENT_BYTES` is derived from the
//! per-file admission ceiling and the representation the parser builds, not measured on
//! a fixture. Three earlier attempts to publish a measured term were each beaten by a
//! denser admissible file; the accounting below closes over the grammar instead, and the
//! measurements are kept only as corroborating samples that must fall under it.
//!
//! **Latency.** The trade this policy accepts is a lex-and-parse per query in exchange
//! for retaining no tree. The budget is pinned at the maximum file the project owner
//! admits, in the densest shape a query can be handed, and it is asserted only in the
//! optimized profile the server ships in — every profile records the measurement.
//!
//! Parseability itself is never inferred from a query-local parse: `broken_files` stays
//! the snapshot's independent record, which is why a recovered-broken file still
//! classifies positions while its hover facts stay syntax-unavailable.

use std::mem::size_of;
use std::sync::Arc;
use std::time::Instant;

use marrow_compile::{
    ActiveCallOutcome, AnalysisSnapshot, CompletionOutcome, Fact, InputRevision, PositionClass,
    QueryError, Unavailability, analyze,
};
use marrow_project::{CaptureLimits, CapturedFile, FileIdentity, Manifest, ProjectInput};
use marrow_syntax::{
    Argument, ArmBinding, BinaryOp, Comment, Declaration, ElseIf, EnumMember, EnumPayloadField,
    Expression, ForName, IfConstBinding, IndexArg, IndexDecl, InterpolationPart, KeyParam,
    LiteralKind, MatchArm, NameSegment, ParamDecl, ResourceMember, SYNTAX_DIAGNOSTIC_COUNT_LIMIT,
    SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT, SourceSpan, Statement, Token, TypeExpr, TypeParamDecl,
    UnaryOp, UseDecl,
};

/// The largest file the project owner admits — the worst case a query-local parse can
/// be handed, since drive admission refuses anything larger before a snapshot exists.
const MAX_ADMITTED_FILE_BYTES: usize = 1 << 20;

/// The language server's owned-heap ceiling, which every retention and transient term
/// in the analysis layer is sized against. This file consumes it; it does not move it.
const H_OWNED_BYTES: usize = 640 * 1024 * 1024;

/// The editor-latency budget for one query of a maximum admitted file, in the optimized
/// profile the language server ships in.
///
/// This is the cost the retention bound buys: the snapshot retains no tree, so a query
/// lexes and parses the one file it names, then classifies the position over the result.
/// The budget covers the whole query, not the parse alone.
///
/// The number is derived from the densest shape a query can be handed, which is the same
/// shape the parse-transient term is derived over: single-byte statement lines in a file
/// whose module does not parse. On the recorded host that shape measures 112 ms worst of
/// nine after a warm query (123 ms in a process that had already built other
/// maximum-size fixtures), so the budget is twice that, rounded up, leaving room for a
/// machine roughly half as fast and for ordinary run-to-run variation.
///
/// It is a regression fence, not an immediacy claim. A maximum admitted file in its
/// densest shape does **not** answer inside the ~100 ms at which a response reads as
/// immediate; immediacy is carried by [`ORDINARY_QUERY_BUDGET_MS`], which is what an
/// editor session spends nearly all of its queries against. Earlier passes recorded
/// 15.3 ms for the parse alone and then 49 ms for a whole query over a comment-padded
/// file; both were fixture-bound, and neither is the worst case.
///
/// A parse cache is not the remedy if this fails: that would be a second retention owner
/// and would reopen the bound this policy closes.
const QUERY_BUDGET_MS: u128 = 250;

/// The same budget for a file of ordinary size, which is what an editor session actually
/// spends nearly all of its queries on.
const ORDINARY_FILE_BYTES: usize = 64 * 1024;
const ORDINARY_QUERY_BUDGET_MS: u128 = 10;

/// The working set of one query-local parse of one maximum admitted file: the tree, the
/// lexer's token vector, the expression parser's bounded token copy, and the syntax
/// collector's bounded rows, all live together at the peak.
///
/// **Derived, not sampled.** `the_query_parse_transient_closes_under_the_exported_term`
/// re-derives the accounted figure from the pinned representation and asserts it, the way
/// the retention term is accounted. Every measurement in this file and in the lane's
/// evidence is a corroborating sample under this bound, never the source of it — three
/// earlier passes each published a measured term and each was beaten by a denser
/// admissible file within days.
///
/// The accounting charges allocated capacity, not resident pages: a container's amortized
/// growth slack is allocated and paid for by an allocator, but a sampled
/// `maximum resident set size` never sees it, so a sample is a floor and this term is the
/// ceiling. Two things sit outside it, exactly as they do for the retention term: the
/// caller-shared source bytes, and per-allocation allocator overhead.
///
/// The term is a consequence of [`MAX_SOURCE_BYTE_CHARGE`], not an independent number:
/// the cap is the invariant and this is what the cap costs at the admission ceiling.
const MAX_QUERY_PARSE_TRANSIENT_BYTES: usize =
    MAX_ADMITTED_FILE_BYTES * (MAX_SOURCE_BYTE_CHARGE + TOKEN_CHARGE) + TOKEN_CHARGE + DIAGNOSTICS;

/// **The invariant of this file.** No node family the parser builds may charge more than
/// this many heap bytes per source byte it exclusively requires.
///
/// The derived figure is a maximum over roughly twenty families, so shrinking the widest
/// one promotes the next; a bound stated only as a total would let a widened field hide
/// behind whichever family happened to be largest. `no_node_family_exceeds_the_declared_source_byte_cap`
/// therefore asserts this per family, and the cap sits deliberately above the derived
/// maximum so one future field costs a review rather than a re-derivation of the ceiling.
const MAX_SOURCE_BYTE_CHARGE: usize = 320;

/// Amortized container growth. Every container in a parse tree is built by pushing —
/// none is pre-sized from an exact count and none is shrunk — and the standard library
/// grows a full container to `max(minimum capacity, 2 * len)`. Pinned by
/// `container_growth_stays_within_the_accounted_factor`.
const GROWTH: usize = 2;

/// The standard library's minimum non-zero capacity: eight elements for a one-byte
/// element (a `String`'s buffer), four for any wider element up to 1 KiB.
const MIN_BYTE_CAPACITY: usize = 8;
const MIN_ELEMENT_CAPACITY: usize = 4;

/// The worst-case bytes a `Vec<T>` grown by pushing to `len` elements holds.
fn vec_bytes<T>(len: usize) -> usize {
    MIN_ELEMENT_CAPACITY.max(GROWTH * len) * size_of::<T>()
}

/// The worst-case bytes a retained growable spelling of `len` source bytes holds.
fn string_bytes(len: usize) -> usize {
    MIN_BYTE_CAPACITY.max(GROWTH * len)
}

/// The bytes an exactly-sized `Box<[T]>` of `len` elements holds. A boxed slice has no
/// capacity field, so there is no growth slack to charge and no minimum capacity floor.
fn boxed_slice_bytes<T>(len: usize) -> usize {
    len * size_of::<T>()
}

/// The bytes an exactly-sized `Box<str>` of `len` source bytes holds, on the same terms.
fn boxed_str_bytes(len: usize) -> usize {
    len
}

/// What one expression node allocates directly: not the slot it occupies in its parent,
/// and not its children's slots, but its own retained spellings and parallel span
/// vectors, at the fewest source bytes the grammar admits for that variant.
///
/// The match destructures every field of every variant, so a new field or a new variant
/// fails to build here rather than silently widening the term.
fn expression_own_bytes(expression: &Expression) -> usize {
    match expression {
        // A literal retains a copy of its own token text.
        Expression::Literal {
            kind: _,
            text: _,
            span: _,
        } => boxed_str_bytes(1),
        // One segment costs its slot in the exactly-sized path and its own spelling. No
        // `Expression` retains a growable string, so every spelling here is exact.
        Expression::Name {
            segments: _,
            span: _,
        } => boxed_slice_bytes::<NameSegment>(1) + boxed_str_bytes(1),
        Expression::SavedRoot { name: _, span: _ } => boxed_str_bytes(1),
        Expression::Absent { span: _ } => 0,
        // An argument or key vector's buffer is charged to its elements' slots, and a
        // boxed operand to that operand's own slot, so an operator node owns nothing.
        Expression::Call {
            callee: _,
            args: _,
            multiline: _,
            span: _,
        } => 0,
        Expression::Keyed {
            base: _,
            keys: _,
            multiline: _,
            span: _,
        } => 0,
        Expression::Field {
            base: _,
            name: _,
            name_span: _,
            quoted: _,
            span: _,
        } => boxed_str_bytes(1),
        Expression::OptionalField {
            base: _,
            name: _,
            name_span: _,
            quoted: _,
            span: _,
        } => boxed_str_bytes(1),
        Expression::Unary {
            op: _,
            operand: _,
            span: _,
        } => 0,
        Expression::Binary {
            op: _,
            left: _,
            right: _,
            span: _,
        } => 0,
        Expression::Range {
            start: _,
            end: _,
            inclusive_end: _,
            step: _,
            span: _,
        } => 0,
        Expression::Membership {
            value: _,
            range: _,
            negated: _,
            span: _,
        } => 0,
        Expression::Interpolation { parts: _, span: _ } => 0,
        Expression::Try { inner: _, span: _ } => 0,
        // A recovery node boxes the receiver it preserves. That receiver is itself a node
        // whose slot is charged again below, so this is double-charged, never short.
        Expression::Error {
            span: _,
            recovery: _,
        } => size_of::<Expression>(),
    }
}

/// One value of every `Expression` variant. `expression_own_bytes` is exhaustive, so a
/// new variant fails to build there; `EXPRESSION_VARIANTS` keeps this list in step, so a
/// new variant cannot be given an arm and then skipped when the maximum is taken.
const EXPRESSION_VARIANTS: usize = 15;

fn expression_variants() -> Vec<Expression> {
    let span = SourceSpan::default();
    let leaf = || Box::new(Expression::Absent { span }) as Box<Expression>;
    vec![
        Expression::Literal {
            kind: LiteralKind::Integer,
            text: "1".into(),
            span,
        },
        Expression::Name {
            segments: Box::new([NameSegment::new("a", span)]),
            span,
        },
        Expression::SavedRoot {
            name: "a".into(),
            span,
        },
        Expression::Absent { span },
        Expression::Call {
            callee: leaf(),
            args: Vec::new(),
            multiline: false,
            span,
        },
        Expression::Keyed {
            base: leaf(),
            keys: Vec::new(),
            multiline: false,
            span,
        },
        Expression::Field {
            base: leaf(),
            name: "a".into(),
            name_span: span,
            quoted: false,
            span,
        },
        Expression::OptionalField {
            base: leaf(),
            name: "a".into(),
            name_span: span,
            quoted: false,
            span,
        },
        Expression::Unary {
            op: UnaryOp::Neg,
            operand: leaf(),
            span,
        },
        Expression::Binary {
            op: BinaryOp::Add,
            left: leaf(),
            right: leaf(),
            span,
        },
        Expression::Range {
            start: None,
            end: None,
            inclusive_end: false,
            step: None,
            span,
        },
        Expression::Membership {
            value: leaf(),
            range: leaf(),
            negated: false,
            span,
        },
        Expression::Interpolation {
            parts: Vec::new(),
            span,
        },
        Expression::Try {
            inner: leaf(),
            span,
        },
        Expression::Error {
            span,
            recovery: None,
        },
    ]
}

/// The heap one expression node adds per source byte it exclusively requires.
///
/// Its slot is charged in the widest placement the grammar admits — an element of a key
/// vector, which is wider than a `Box` and wider than being held inline in its parent —
/// and its own allocations at the one source byte a leaf needs. This is the rate every
/// content byte of a statement line is charged at.
fn expression_charge() -> usize {
    let variants = expression_variants();
    assert_eq!(
        variants.len(),
        EXPRESSION_VARIANTS,
        "every `Expression` variant needs a value here, or the maximum below is taken \
         over an incomplete list"
    );
    let own = variants
        .iter()
        .map(expression_own_bytes)
        .max()
        .expect("the variant list is not empty");
    GROWTH * size_of::<Expression>() + own
}

/// The largest charge in a `(kind, bytes, source bytes)` table, per source byte, taken
/// with `div_ceil` so integer division never rounds a charge down.
fn worst_charge(charges: Vec<(&'static str, usize, usize)>, floor: usize) -> usize {
    charges
        .into_iter()
        .map(|(_, bytes, source_bytes)| bytes.div_ceil(source_bytes))
        .fold(floor, usize::max)
}

/// The heap one content byte of a statement line can buy: the densest expression node,
/// or any other kind a statement line holds, whichever is larger. Taking the maximum
/// makes the accounting sound by construction rather than by the assertion below, which
/// records that the expression node is in fact the one that wins.
fn content_byte_charge() -> usize {
    worst_charge(statement_line_content_charges(), expression_charge())
}

/// The heap one source byte of a block of statements can buy.
///
/// A `Statement` is the widest node the parser stores in a vector, and it is stored in
/// exactly one: a block's statement list. Two source bytes are the least the grammar
/// spends on one statement — no two statements share a line, `statements` never builds
/// one from an empty line, and every statement is closed either by its own newline or by
/// its block's `}` — so a statement line of `L` bytes charges one statement slot plus
/// `L - 1` content bytes at the content rate. That is largest at `L = 2`.
fn statement_line_charge() -> usize {
    (GROWTH * size_of::<Statement>() + content_byte_charge()) / 2
}

/// The heap one source byte of a file can buy: a statement line, or anything the parser
/// builds outside one, whichever is larger.
fn source_byte_charge() -> usize {
    worst_charge(declaration_level_charges(), statement_line_charge())
}

/// Every node family the parser builds, with the heap one source byte of that family
/// charges — the union of the statement-line contents, the constructs outside a
/// statement line, the expression node, and the statement slot, **not** their maximum.
///
/// [`source_byte_charge`] is a maximum over these families, so shrinking the widest one
/// promotes the next and a cap asserted only in total lets a widened field hide behind
/// whichever family happens to be largest at the time. Asserting the cap over this list
/// is what makes a new or widened field fail a test rather than move the ceiling.
fn source_byte_charges_by_family() -> Vec<(&'static str, usize)> {
    statement_line_content_charges()
        .into_iter()
        .chain(declaration_level_charges())
        .map(|(kind, bytes, source_bytes)| (kind, bytes.div_ceil(source_bytes)))
        .chain([
            ("Expression", expression_charge()),
            ("a statement line", statement_line_charge()),
        ])
        .collect()
}

/// Everything else a statement line can hold, with what its slot and own allocations
/// cost and the fewest source bytes the grammar admits for one. Each minimum is the
/// construct's own spelling, exclusive of any child node's bytes.
fn statement_line_content_charges() -> Vec<(&'static str, usize, usize)> {
    vec![
        // A sole positional argument carries no separator of its own, so it is charged
        // against the first byte of its own value — deliberately double-charged.
        (
            "Argument",
            GROWTH * size_of::<Argument>() + boxed_str_bytes(1),
            1,
        ),
        // `a:b`: the type annotation is mandatory, not an `Option`.
        (
            "KeyParam",
            GROWTH * size_of::<KeyParam>() + string_bytes(1),
            3,
        ),
        // A member-path name and its own block.
        (
            "MatchArm",
            GROWTH * size_of::<MatchArm>()
                + boxed_slice_bytes::<NameSegment>(1)
                + boxed_str_bytes(1),
            3,
        ),
        // `else if`.
        ("ElseIf", GROWTH * size_of::<ElseIf>(), 7),
        // The `const` keyword each chained binding carries.
        (
            "IfConstBinding",
            GROWTH * size_of::<IfConstBinding>() + string_bytes(1),
            5,
        ),
        (
            "ArmBinding",
            GROWTH * size_of::<ArmBinding>() + string_bytes(1),
            1,
        ),
        (
            "ForName",
            GROWTH * size_of::<ForName>() + string_bytes(1),
            1,
        ),
        // The `//` marker.
        (
            "Comment",
            GROWTH * size_of::<Comment>() + string_bytes(1),
            2,
        ),
        (
            "TypeExpr",
            GROWTH * size_of::<TypeExpr>() + string_bytes(1),
            1,
        ),
        (
            "InterpolationPart",
            GROWTH * size_of::<InterpolationPart>() + string_bytes(1),
            1,
        ),
        ("a statement's own spelling", string_bytes(1), 1),
    ]
}

/// Everything the parser builds outside a statement line, on the same terms.
fn declaration_level_charges() -> Vec<(&'static str, usize, usize)> {
    vec![
        // The shortest declaration keyword is `fn`.
        (
            "Declaration",
            GROWTH * size_of::<Declaration>() + string_bytes(1),
            2,
        ),
        // `use`.
        (
            "UseDecl",
            GROWTH * size_of::<UseDecl>() + string_bytes(1) + vec_bytes::<SourceSpan>(1),
            3,
        ),
        // `a:b` — a field's type annotation is mandatory (a `TypeExpr`, not an
        // `Option`), and a group's own spelling is `a{}`. Three source bytes either way,
        // on the same rule as `KeyParam`.
        (
            "ResourceMember",
            GROWTH * size_of::<ResourceMember>() + string_bytes(1),
            3,
        ),
        (
            "EnumMember",
            GROWTH * size_of::<EnumMember>() + string_bytes(1),
            1,
        ),
        // `a:b`; a payload field's annotation is mandatory.
        (
            "EnumPayloadField",
            GROWTH * size_of::<EnumPayloadField>() + string_bytes(1),
            3,
        ),
        // `a:b`; a parameter's annotation is mandatory.
        (
            "ParamDecl",
            GROWTH * size_of::<ParamDecl>() + string_bytes(1),
            3,
        ),
        (
            "TypeParamDecl",
            GROWTH * size_of::<TypeParamDecl>() + string_bytes(1),
            1,
        ),
        // `index` is the keyword an index declaration pays for exclusively.
        (
            "IndexDecl",
            GROWTH * size_of::<IndexDecl>()
                + string_bytes(1)
                + boxed_slice_bytes::<IndexArg>(1)
                + string_bytes(1)
                + boxed_slice_bytes::<SourceSpan>(1),
            5,
        ),
    ]
}

/// How many token allocations are live beside the tree at the peak: the lexer's vector
/// for the whole file, plus the one filtered copy an expression parse holds.
/// `ExprParser::parse_complete` builds no further parser and the declaration parser holds
/// at most one at a time.
const LIVE_TOKEN_VECTORS: usize = 2;

/// The capacity slack one live token allocation carries. A `Vec` grown by pushing holds
/// the amortized growth factor; a slice handed to `Box<[Token]>` at close holds none,
/// because a `Box<[T]>` has no capacity field to hold any.
const TOKEN_VECTOR_SLACK: usize = GROWTH;

/// What the lexed tokens charge per source byte.
///
/// The lexer emits at most one token per source byte — every token but the zero-width
/// `Eof` sentinel covers at least one byte of source, pinned by
/// [`the_token_vector_holds_at_most_one_token_per_source_byte`] — so this is the whole
/// token cost of a source byte, and the trailing sentinel is one further token charged
/// outside the per-byte term.
const TOKEN_CHARGE: usize = LIVE_TOKEN_VECTORS * TOKEN_VECTOR_SLACK * size_of::<Token>();

/// The syntax collector's own retention, which is live beside the tree until
/// `parse_source` returns. Both of its ceilings are pinned by `marrow-syntax`; a row is
/// charged at 256 bytes, comfortably above the `Diagnostic` it wraps.
const DIAGNOSTIC_ROW_BYTES: usize = 256;

/// The collector's two pinned ceilings, each charged with the growth factor because both
/// containers are still grown by pushing.
const DIAGNOSTICS: usize = GROWTH * SYNTAX_DIAGNOSTIC_COUNT_LIMIT * DIAGNOSTIC_ROW_BYTES
    + GROWTH * SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT;

/// The accounted worst case of one query-local parse.
///
/// Written in exactly the shape of [`MAX_QUERY_PARSE_TRANSIENT_BYTES`], with the derived
/// `source_byte_charge()` where the term carries the declared cap. The two therefore
/// differ in one factor only, which is what makes the cap — rather than this figure —
/// the thing a reviewer has to agree with.
fn accounted_query_parse_transient() -> usize {
    MAX_ADMITTED_FILE_BYTES * (source_byte_charge() + TOKEN_CHARGE) + TOKEN_CHARGE + DIAGNOSTICS
}

fn captured(files: Vec<(&str, Vec<u8>)>) -> Arc<ProjectInput> {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let captured = files
        .into_iter()
        .map(|(path, bytes)| CapturedFile::new(path.to_string(), bytes))
        .collect();
    Arc::new(
        marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
            .expect("the fixture is inside the production admission envelope"),
    )
}

fn snapshot(files: Vec<(&str, Vec<u8>)>) -> Arc<AnalysisSnapshot> {
    match analyze(captured(files), InputRevision::new(1)) {
        Ok(snapshot) => snapshot,
        Err(failure) => panic!(
            "the fixture analyzes; failed at revision {}",
            failure.revision().get()
        ),
    }
}

fn identity(path: &str) -> FileIdentity {
    FileIdentity::validate(path)
        .expect("the fixture path is a canonical identity")
        .0
}

/// A stable rendering of one completion outcome, so an agreement assertion compares
/// exact classifications and candidate sets rather than a summary.
fn render_completions(outcome: Result<CompletionOutcome, QueryError>) -> String {
    match outcome {
        Err(QueryError::UnknownFile) => "error:unknown-file".to_string(),
        Err(QueryError::OffsetOutOfRange) => "error:offset".to_string(),
        Ok(CompletionOutcome::Refused(limit)) => format!("refused:{}", limit.description()),
        Ok(CompletionOutcome::Ready(Fact::Absent)) => "absent".to_string(),
        Ok(CompletionOutcome::Ready(Fact::Unavailable(Unavailability::Syntax))) => {
            "unavailable:syntax".to_string()
        }
        Ok(CompletionOutcome::Ready(Fact::Unavailable(Unavailability::Dependency))) => {
            "unavailable:dependency".to_string()
        }
        Ok(CompletionOutcome::Ready(Fact::Present(completions))) => {
            let class = match completions.class() {
                PositionClass::ExpressionName => "expr",
                PositionClass::Member => "member",
                PositionClass::EnumPath => "enum",
                PositionClass::TypeAnnotation => "type",
            };
            let labels: Vec<String> = completions
                .candidates()
                .iter()
                .map(|candidate| format!("{}:{}", candidate.label(), candidate.detail()))
                .collect();
            format!("{class}[{}]", labels.join(","))
        }
    }
}

fn render_active_call(outcome: Result<ActiveCallOutcome, QueryError>) -> String {
    match outcome {
        Err(QueryError::UnknownFile) => "error:unknown-file".to_string(),
        Err(QueryError::OffsetOutOfRange) => "error:offset".to_string(),
        Ok(ActiveCallOutcome::Refused(limit)) => format!("refused:{}", limit.description()),
        Ok(ActiveCallOutcome::Ready(Fact::Absent)) => "absent".to_string(),
        Ok(ActiveCallOutcome::Ready(Fact::Unavailable(Unavailability::Syntax))) => {
            "unavailable:syntax".to_string()
        }
        Ok(ActiveCallOutcome::Ready(Fact::Unavailable(Unavailability::Dependency))) => {
            "unavailable:dependency".to_string()
        }
        Ok(ActiveCallOutcome::Ready(Fact::Present(call))) => {
            let params: Vec<&str> = call.params().iter().map(|piece| piece.label()).collect();
            format!(
                "{}|{}|{:?}",
                call.signature(),
                params.join(","),
                call.active()
            )
        }
    }
}

const CLEAN: &str = "module main\n\n\
     struct Point {\n    x: int\n    y: int\n}\n\n\
     enum Colour {\n    red\n    green\n}\n\n\
     fn add(left: int, right: int): int {\n    return left + right\n}\n\n\
     pub fn run(): int {\n    var p: Point = Point(x: 1, y: 2)\n    return add(p.x, p.y)\n}\n";

/// A file with a real parse error whose recovered incomplete forms still classify.
const BROKEN: &str = "module broken\n\n\
     struct Item {\n    name: string\n}\n\n\
     pub fn go(): int {\n    var i: Item = Item(name: \"a\")\n    i.\n    return 0\n}\n";

fn corpus_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("src/main.mw", CLEAN.as_bytes().to_vec()),
        ("src/broken.mw", BROKEN.as_bytes().to_vec()),
        ("src/undecodable.mw", b"module bad\n\xFF\xFE".to_vec()),
    ]
}

/// Frozen `(file, offset)` probes with the exact outcome each must produce. The offsets
/// are derived from the source text so an edit to a fixture cannot silently move a probe
/// off its intended position.
fn probes() -> Vec<(&'static str, usize, &'static str)> {
    let after_dot = CLEAN.find("p.x").expect("the member probe exists") + 2;
    let in_call = CLEAN.find("add(p.x").expect("the call probe exists") + 4;
    let in_type = CLEAN.find("var p: Point").expect("the type probe exists") + 7;
    let broken_dot = BROKEN.find("    i.\n").expect("the recovered probe exists") + 6;
    vec![
        ("src/main.mw", after_dot, "member"),
        ("src/main.mw", in_call, "call"),
        ("src/main.mw", in_type, "type"),
        ("src/broken.mw", broken_dot, "recovered"),
        ("src/undecodable.mw", 0, "undecodable"),
    ]
}

/// Every probe's completion and active-call outcome, in probe order.
fn corpus_outcomes(snapshot: &AnalysisSnapshot) -> Vec<(String, String, String)> {
    probes()
        .into_iter()
        .map(|(path, offset, label)| {
            let file = identity(path);
            (
                label.to_string(),
                render_completions(snapshot.completions(&file, offset)),
                render_active_call(snapshot.active_call(&file, offset)),
            )
        })
        .collect()
}

/// The frozen corpus outcomes. Each is exactly what the retained-tree path produced
/// before the trees were deleted, so a query-local parse that classified differently —
/// a recovered-broken file that stopped classifying, an undecodable file that stopped
/// being syntax-unavailable, a candidate set that changed — fails here.
#[test]
fn query_local_outcomes_match_the_frozen_corpus() {
    let snapshot = snapshot(corpus_files());
    let outcomes = corpus_outcomes(&snapshot);
    let rendered: Vec<String> = outcomes
        .iter()
        .map(|(label, completions, active)| format!("{label} => {completions} :: {active}"))
        .collect();
    assert_eq!(
        rendered,
        vec![
            "member => member[x:int,y:int] :: \
             fn add(left: int, right: int): int|left: int,right: int|Some(0)"
                .to_string(),
            "call => expr[p:Point,Colour:,add:(left: int, right: int): int,run:(): int,\
             none:,some:,ok:,err:,exists:,unreachable:,todo:,isEmpty:,contains:,trim:,\
             split:,lines:,join:,addDays:,daysBetween:,List:,Map:,Id:,maxInt:,minInt:] :: \
             fn add(left: int, right: int): int|left: int,right: int|Some(0)"
                .to_string(),
            "type => type[Point:,Colour:,int:,bool:,string:,bytes:,date:,instant:,\
             duration:,Option:,Result:,List:,Map:,Id:] :: absent"
                .to_string(),
            "recovered => member[name:string] :: absent".to_string(),
            "undecodable => unavailable:syntax :: unavailable:syntax".to_string(),
        ]
    );
}

/// Repeating a query re-derives the same outcome: the parse is transient, so nothing
/// accumulates and no second query sees different state.
#[test]
fn repeating_a_query_is_stable() {
    let snapshot = snapshot(corpus_files());
    assert_eq!(corpus_outcomes(&snapshot), corpus_outcomes(&snapshot));
    assert_eq!(corpus_outcomes(&snapshot), corpus_outcomes(&snapshot));
}

/// A recovered-broken file classifies positions, but parseability is never inferred
/// from the query-local parse: the snapshot's independent `broken_files` record still
/// makes its hover and document-symbol facts syntax-unavailable.
#[test]
fn parseability_is_not_inferred_from_a_query_local_parse() {
    let snapshot = snapshot(corpus_files());
    let broken = identity("src/broken.mw");
    assert!(matches!(
        snapshot.completions(&broken, BROKEN.find("    i.\n").expect("probe") + 6),
        Ok(CompletionOutcome::Ready(Fact::Present(_)))
    ));
    assert!(matches!(
        snapshot.hover(&broken, 0),
        Ok(Fact::Unavailable(Unavailability::Syntax))
    ));
    assert!(matches!(
        snapshot.document_symbols(&broken),
        Ok(Fact::Unavailable(Unavailability::Syntax))
    ));
}

/// A file of exactly the largest admitted size, reaching it with comment filler after a
/// substantial declaration set.
///
/// The query lexes and parses every one of the file's bytes either way, and this shape
/// spends more of them on lexing than the dense shapes do. It is neither the memory nor
/// the latency worst case — a comment byte builds no tree node — so it is a corroborating
/// sample, not what any term rests on.
fn maximum_admitted_file() -> Vec<u8> {
    let mut source = String::from("module big\n\n");
    for index in 0..2_000usize {
        source.push_str(&format!(
            "fn f{index}(a: int, b: int): int {{\n    var t: int = a\n    t = t + b\n    return t\n}}\n\n"
        ));
    }
    let filler = "// filler line carrying ordinary comment text for the lexer to scan\n";
    while source.len() + filler.len() <= MAX_ADMITTED_FILE_BYTES {
        source.push_str(filler);
    }
    while source.len() < MAX_ADMITTED_FILE_BYTES {
        source.push('\n');
    }
    assert_eq!(source.len(), MAX_ADMITTED_FILE_BYTES);
    source.into_bytes()
}

/// A file of exactly the largest admitted size built from operator chains, kept as a
/// corroborating sample rather than as the source of the parse-transient term.
///
/// It is not the worst case, and the reasoning that once made it look like one was
/// wrong. A file is queryable only if the project it belongs to yields a snapshot at
/// all, and a maximum-size file whose statements resolve reaches a ceiling that refuses
/// `analyze` outright — the fact table, the image, or the interned string table,
/// depending on the shape — while one whose names do not resolve refuses with
/// `too many diagnostics to retain`. Neither yields a snapshot, so neither can be
/// queried. The densest *queryable* tree therefore comes from a file whose nodes charge
/// neither the fact ceiling nor the diagnostic ceiling: a module that failed to parse,
/// or names that never resolve inside one that did.
/// [`statement_dense_file`] is that shape, and it is denser than this one.
///
/// One deliberate type error keeps the whole-image byte ceiling (which a 1 MiB dense
/// program exceeds) from deciding queryability; the analysis is resilient, so the project
/// still yields a snapshot and the file still projects its outline.
fn dense_admitted_file() -> Vec<u8> {
    let mut source = String::from("module dense\n\nfn typeError(): int {\n    return \"x\"\n}\n\n");
    let mut index = 0usize;
    loop {
        let mut body = format!("fn f{index}(): int {{\n    return 1");
        for _ in 0..DENSE_OPERANDS {
            body.push_str("+1");
        }
        body.push_str("\n}\n\n");
        if source.len() + body.len() > MAX_ADMITTED_FILE_BYTES {
            break;
        }
        source.push_str(&body);
        index += 1;
    }
    // The remainder is shorter than one body; pad it out exactly.
    while source.len() < MAX_ADMITTED_FILE_BYTES {
        source.push('\n');
    }
    assert_eq!(source.len(), MAX_ADMITTED_FILE_BYTES);
    source.into_bytes()
}

/// Operands per body in the dense fixture: below the nesting the parser refuses, so every
/// body contributes a whole chain rather than an error node. A parser that narrows its
/// nesting bound fails `a_dense_maximum_admitted_file_is_queryable`, which pins that the
/// fixture's only diagnostic is its one deliberate type error.
const DENSE_OPERANDS: usize = 200;

/// The densest queryable shape the derivation predicts: bodies of single-byte statement
/// lines, so every two source bytes buy one whole `Statement` slot, and a trailing
/// declaration that does not parse, so the module never enters analysis and its nodes
/// charge neither the fact ceiling nor the diagnostic ceiling.
///
/// `per_body` sizes each block's statement vector just past a power of two, which is
/// where amortized growth holds the most slack — the condition the accounting charges
/// for and a resident-set measurement never sees.
fn statement_dense_file(per_body: usize) -> Vec<u8> {
    let mut source = String::from("module m\n\n");
    let mut index = 0usize;
    loop {
        let mut body = format!("fn f{index}() {{\n");
        for _ in 0..per_body {
            body.push_str("a\n");
        }
        body.push_str("}\n\n");
        if source.len() + body.len() + 16 > MAX_ADMITTED_FILE_BYTES {
            break;
        }
        source.push_str(&body);
        index += 1;
    }
    source.push_str("fn broken( {\n");
    while source.len() < MAX_ADMITTED_FILE_BYTES {
        source.push('\n');
    }
    assert_eq!(source.len(), MAX_ADMITTED_FILE_BYTES);
    source.into_bytes()
}

/// Statement lines per body in [`statement_dense_file`], one past a power of two.
const STATEMENT_DENSE_PER_BODY: usize = 257;

/// A second statement-dense shape, one past a much larger power of two. Bodies this long
/// hold their growth slack at a coarser granularity, so while every body is pre-sized
/// exactly this shape is indistinguishable from the one above — and before that it was
/// the denser of the two, which is why it is kept: a sample search that stopped at the
/// first "densest" shape had already missed it.
const STATEMENT_DENSE_PER_LONG_BODY: usize = 4097;

/// Every maximum-size shape this file can build, each queryable and each a corroborating
/// sample under the derived bound rather than a source of it.
fn maximum_admitted_shapes() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "statement-dense",
            statement_dense_file(STATEMENT_DENSE_PER_BODY),
        ),
        (
            "statement-dense-long-bodies",
            statement_dense_file(STATEMENT_DENSE_PER_LONG_BODY),
        ),
        ("name-chain", name_chain_file()),
        ("operator-chain", dense_admitted_file()),
        ("comment-padded", maximum_admitted_file()),
    ]
}

/// A clean maximum admitted file of one long operator chain per body over an undeclared
/// name, the shape an independent review used to beat the previously exported term. Kept
/// as a corroborating sample: every name is a diagnostic rather than a hover fact, so the
/// file charges no fact ceiling, and the module still parses.
fn name_chain_file() -> Vec<u8> {
    let mut source = String::from("module chain\n\n");
    let mut index = 0usize;
    loop {
        let mut body = format!("fn f{index}(): int {{\n    return a");
        for _ in 0..250 {
            body.push_str("+a");
        }
        body.push_str("\n}\n\n");
        if source.len() + body.len() + 16 > MAX_ADMITTED_FILE_BYTES {
            break;
        }
        source.push_str(&body);
        index += 1;
    }
    source.push_str("fn broken( {\n");
    while source.len() < MAX_ADMITTED_FILE_BYTES {
        source.push('\n');
    }
    assert_eq!(source.len(), MAX_ADMITTED_FILE_BYTES);
    source.into_bytes()
}

/// The bytes a parsed file's block statement vectors hold, which is the accounting's
/// dominant term and the one a corroborating sample can count exactly rather than
/// sample through a resident set.
fn statement_vector_bytes(source: &[u8]) -> usize {
    let text = std::str::from_utf8(source).expect("the fixture is UTF-8");
    marrow_syntax::parse_source(text)
        .file
        .declarations
        .iter()
        .map(|declaration| match declaration {
            Declaration::Function(function) => function.body.statements.capacity(),
            Declaration::Test(test) => test.body.statements.capacity(),
            _ => 0,
        })
        .sum::<usize>()
        * size_of::<Statement>()
}

/// The worst wall time of five completion queries over `path`, after one warm query.
fn worst_query_ms(snapshot: &AnalysisSnapshot, path: &str) -> u128 {
    let file = identity(path);
    let _ = snapshot.completions(&file, 64);
    let mut worst = 0u128;
    for _ in 0..5 {
        let started = Instant::now();
        let outcome = snapshot.completions(&file, 64);
        worst = worst.max(started.elapsed().as_millis());
        assert!(outcome.is_ok(), "the query resolves");
    }
    worst
}

/// A budget is a property of the optimized profile the language server ships in, so this
/// asserts **only** there: the unoptimized profile runs the same code about an order of
/// magnitude slower, and asserting against it would pin a number no user ever meets. Every
/// profile records the measurement, so an unoptimized run still reports what it saw; a
/// caller reading a green unoptimized run has observed the measurement, not the budget.
#[track_caller]
fn assert_within_budget(measured: u128, budget: u128, bytes: usize) {
    eprintln!("query-local completion over {bytes} bytes: worst {measured} ms");
    if cfg!(debug_assertions) {
        return;
    }
    assert!(
        measured <= budget,
        "a query over a {bytes}-byte file took {measured} ms, over the {budget} ms budget"
    );
}

/// One query over a maximum admitted file stays inside the editor-latency budget in every
/// maximum-size shape reachable here, including the statement-dense one the budget is
/// derived from. The comment-padded and operator-chain shapes are kept because each was
/// once published as the worst case and neither is.
#[test]
fn a_query_over_a_maximum_admitted_file_stays_in_budget_when_optimized() {
    let path = "src/shape.mw";
    for (label, bytes) in maximum_admitted_shapes() {
        let snapshot = snapshot(vec![(path, bytes)]);
        eprintln!("shape {label}");
        assert_within_budget(
            worst_query_ms(&snapshot, path),
            QUERY_BUDGET_MS,
            MAX_ADMITTED_FILE_BYTES,
        );
    }
}

/// One query over an ordinary-sized file — what an editor session spends nearly all of
/// its queries on — stays far inside the budget, so the worst case above is the tail and
/// not the common cost.
#[test]
fn a_query_over_an_ordinary_file_stays_far_inside_the_budget_when_optimized() {
    let mut source = String::from("module ordinary\n\n");
    let filler = "// filler line carrying ordinary comment text for the lexer to scan\n";
    for index in 0..120usize {
        source.push_str(&format!(
            "fn f{index}(a: int, b: int): int {{\n    var t: int = a\n    t = t + b\n    return t\n}}\n\n"
        ));
    }
    while source.len() + filler.len() <= ORDINARY_FILE_BYTES {
        source.push_str(filler);
    }
    let snapshot = snapshot(vec![("src/ordinary.mw", source.into_bytes())]);
    assert_within_budget(
        worst_query_ms(&snapshot, "src/ordinary.mw"),
        ORDINARY_QUERY_BUDGET_MS,
        ORDINARY_FILE_BYTES,
    );
}

/// A maximum admitted file yields a snapshot, so the budgets above measure a reachable
/// worst case rather than a project the compiler would refuse.
///
/// Building the snapshot is also the measurement point for the analysis build transient:
/// running this test alone, against a run of the querying test above, separates what the
/// drive spends materializing every module's tree from what one query-local parse costs.
#[test]
fn a_maximum_admitted_file_yields_a_snapshot() {
    let snapshot = snapshot(vec![("src/big.mw", maximum_admitted_file())]);
    let file = identity("src/big.mw");
    assert!(matches!(
        snapshot.document_symbols(&file),
        Ok(Fact::Present(_))
    ));
}

/// A *uniformly dense* maximum admitted file also yields a snapshot and answers queries.
/// Running this test alone measures the live parse tree of one such file.
#[test]
fn a_dense_maximum_admitted_file_is_queryable() {
    let snapshot = snapshot(vec![("src/dense.mw", dense_admitted_file())]);
    let file = identity("src/dense.mw");
    assert_eq!(
        snapshot.diagnostics().len(),
        1,
        "the fixture's only diagnostic is its one deliberate type error; a parser that \
         refused this nesting would report one per body and shrink the tree this sample \
         corroborates the term with"
    );
    assert!(matches!(
        snapshot.document_symbols(&file),
        Ok(Fact::Present(_))
    ));
    assert!(
        snapshot.completions(&file, 64).is_ok(),
        "the sample answers the query whose parse it corroborates"
    );
}

/// Containers grow by doubling from a small minimum, never past it. The accounting
/// charges that slack because an allocator holds it; a resident-set measurement does not
/// see it, which is why a sample can never establish this term.
#[test]
fn container_growth_stays_within_the_accounted_factor() {
    let mut spans: Vec<SourceSpan> = Vec::new();
    let mut spellings: String = String::new();
    for length in 1..=4096usize {
        spans.push(SourceSpan::default());
        spellings.push('a');
        assert!(
            spans.capacity() * size_of::<SourceSpan>() <= vec_bytes::<SourceSpan>(length),
            "a span vector of {length} grew to {} elements, past the accounted capacity",
            spans.capacity()
        );
        assert!(
            spellings.capacity() <= string_bytes(length),
            "a spelling of {length} bytes grew to {} bytes, past the accounted capacity",
            spellings.capacity()
        );
    }
}

/// The parse transient is accounted, not sampled: an arithmetic property of the per-file
/// admission ceiling and the representation the parser builds.
///
/// The accounting runs in three steps.
///
/// 1. **A statement is the widest node the parser stores in a vector**, and two source
///    bytes are the least the grammar spends on one. So one source byte of a block buys
///    at most half a statement slot plus one content byte.
/// 2. **A content byte buys at most one expression node**, in the widest placement the
///    grammar admits for one, plus that node's own allocations. Every other kind a
///    statement line can hold, and every kind outside one, charges less per byte —
///    asserted below rather than asserted in prose.
/// 3. **The tokens and the diagnostics are live beside the tree**: the file's lexed
///    vector, one filtered copy inside the expression parser, and the collector's two
///    pinned ceilings.
///
/// A representation change moves the derived figure, which is why the term carries
/// headroom over it rather than equalling it.
#[test]
fn the_query_parse_transient_closes_under_the_exported_term() {
    eprintln!(
        "Statement {}, Expression {}, TypeExpr {}, Declaration {}, Token {}",
        size_of::<Statement>(),
        size_of::<Expression>(),
        size_of::<TypeExpr>(),
        size_of::<Declaration>(),
        size_of::<Token>()
    );
    assert_eq!(
        expression_charge(),
        240,
        "the expression node's charge moved"
    );
    assert_eq!(
        content_byte_charge(),
        241,
        "the densest content byte's charge moved"
    );
    assert_eq!(
        statement_line_charge(),
        448,
        "the densest statement line's charge moved"
    );
    assert_eq!(
        source_byte_charge(),
        448,
        "the densest source byte's charge moved"
    );
    let accounted = accounted_query_parse_transient();
    assert_eq!(
        accounted, 608_174_208,
        "the accounted query-local parse transient moved; re-derive the term before \
         changing this number"
    );
    assert!(
        accounted <= MAX_QUERY_PARSE_TRANSIENT_BYTES,
        "the accounted {accounted} bytes exceed the exported \
         MAX_QUERY_PARSE_TRANSIENT_BYTES {MAX_QUERY_PARSE_TRANSIENT_BYTES}"
    );
}

/// **The enforcement artifact.** Every node family charges under the declared cap.
///
/// The derived bound is a maximum over roughly twenty families. A field added to any one
/// of them widens that family's charge, and this fails at that family — before the
/// widening can be absorbed by the distance between the derived maximum and the exported
/// term, and whether or not the family that grew is the one currently deciding the
/// maximum. `expression_own_bytes` and the two charge tables destructure exhaustively, so
/// a genuinely new field or variant fails to build here first.
#[test]
fn no_node_family_exceeds_the_declared_source_byte_cap() {
    let families = source_byte_charges_by_family();
    for (kind, charge) in &families {
        assert!(
            *charge <= MAX_SOURCE_BYTE_CHARGE,
            "`{kind}` charges {charge} bytes per source byte, over the declared cap of \
             {MAX_SOURCE_BYTE_CHARGE}; shrink the family or re-derive the cap and the \
             exported term together"
        );
    }
    let widest = families
        .iter()
        .max_by_key(|(_, charge)| *charge)
        .expect("the family list is not empty");
    eprintln!(
        "widest node family: {} at {} bytes/source byte",
        widest.0, widest.1
    );
    assert_eq!(
        source_byte_charge(),
        widest.1,
        "the derived maximum disagrees with the widest family, so the layered charge \
         functions and the flat family list have drifted apart"
    );
}

/// The exported term keeps a third of the owned-heap ceiling in reserve, so a later
/// widening inside the cap cannot quietly re-approach it. A cap raised far enough to
/// break this is a decision about `H_owned`, not a representation detail.
#[test]
fn the_exported_term_stays_well_under_the_owned_heap_ceiling() {
    assert!(
        MAX_QUERY_PARSE_TRANSIENT_BYTES * 3 <= H_OWNED_BYTES * 2,
        "the exported {MAX_QUERY_PARSE_TRANSIENT_BYTES} bytes are over two thirds of the \
         {H_OWNED_BYTES}-byte owned-heap ceiling"
    );
}

/// The lexer emits at most one token per source byte, which is the half of
/// [`TOKEN_CHARGE`] that a type cannot carry: `Box<[Token]>` makes growth slack
/// unrepresentable, but nothing in the type stops the lexer from emitting two tokens for
/// one byte. Every token but the zero-width `Eof` sentinel covers at least one byte of
/// source, so the count is bounded by the file's length plus that sentinel.
#[test]
fn the_token_vector_holds_at_most_one_token_per_source_byte() {
    for (label, bytes) in maximum_admitted_shapes() {
        let text = std::str::from_utf8(&bytes).expect("the fixture is UTF-8");
        let tokens = marrow_syntax::lex_source(text).tokens.len();
        eprintln!("{label}: {tokens} tokens over {} source bytes", text.len());
        assert!(
            tokens <= text.len() + 1,
            "{label} lexed {tokens} tokens from {} source bytes, over the one-per-byte \
             bound TOKEN_CHARGE rests on",
            text.len()
        );
    }
}

/// The corroborating samples: every maximum-size shape reachable here is queryable, and
/// each one's statement vectors — the accounting's dominant term, counted exactly rather
/// than sampled through a resident set — stay under the derived bound.
///
/// The statement-dense shape is the densest of them and it is the one the derivation
/// predicts, which is the point: the term was derived first and the sample fell under it,
/// rather than the sample being searched for and published as a term.
#[test]
fn every_maximum_admitted_shape_stays_under_the_derived_bound() {
    let mut densest = 0;
    for (label, bytes) in maximum_admitted_shapes() {
        let statements = statement_vector_bytes(&bytes);
        eprintln!("{label}: statement vectors hold {statements} bytes");
        assert!(
            statements <= MAX_QUERY_PARSE_TRANSIENT_BYTES,
            "{label} holds {statements} bytes of statement vectors alone, over the term"
        );
        densest = densest.max(statements);
        let path = "src/shape.mw";
        let snapshot = snapshot(vec![(path, bytes)]);
        assert!(
            snapshot.completions(&identity(path), 64).is_ok(),
            "{label} answers the query whose parse the term bounds"
        );
    }
    assert!(
        densest > MAX_QUERY_PARSE_TRANSIENT_BYTES / 2,
        "no sample reaches half the derived bound, so the derivation has drifted far \
         from what the parser actually builds and should be re-derived"
    );
}
