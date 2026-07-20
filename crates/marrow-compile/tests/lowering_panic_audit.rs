//! Lowering-panic audit (E06 M2): lowering reports every source-level problem as a
//! typed diagnostic and never panics.
//!
//! `lower.rs` carries `expect`/`unreachable!`/`panic!` sites that assert invariants the
//! parser, checker, match-arm narrowing, or lowering's own bookkeeping establish before
//! the panicking line. The allowlist below records the reviewed class for each site.
//! This test drives one adversarial source shape per invariant class through the
//! production `compile` path. Each must come back as a typed diagnostic — a
//! `compile` that returned `Err` proves lowering did not abort, and the asserted
//! code proves the checker intercepted the shape before a lowering invariant could
//! be violated. A source-owned
//! exact allowlist independently counts the complete explicit panic-API family in
//! `lower.rs`, so an added, removed, duplicated, or renamed invocation is conspicuous.
//! This is deliberately limited to explicit APIs; it is not a total panic-freedom claim.
//! A regression that turned any sampled source into a panic would abort this test
//! process instead of returning `Err`, making the failure conspicuous.

use marrow_compile::{SourceDiagnostic, compile};
use marrow_project::{CaptureLimits, CapturedFile, Manifest, ProjectInput};

const LOWERING_SOURCE: &str = include_str!("../src/lower.rs");
const TEST_ONLY_BOUNDARY: &str = "\n#[cfg(test)]\nmod generic_cache_boundary_tests {";

fn production_lowering_source() -> &'static str {
    let code = mask_comments_and_literals(LOWERING_SOURCE);
    let boundaries: Vec<usize> = code
        .windows(TEST_ONLY_BOUNDARY.len())
        .enumerate()
        .filter_map(|(index, window)| (window == TEST_ONLY_BOUNDARY.as_bytes()).then_some(index))
        .collect();
    let [boundary] = boundaries.as_slice() else {
        panic!("lower.rs keeps one explicit generic-cache test-module boundary")
    };
    let open = code[*boundary..]
        .iter()
        .position(|byte| *byte == b'{')
        .map(|offset| *boundary + offset)
        .expect("the generic-cache test module has an opening delimiter");
    let close = matching_delimiter(&code, open)
        .expect("the generic-cache test module has a matching closing delimiter");
    assert!(
        code[close + 1..]
            .iter()
            .all(|byte| byte.is_ascii_whitespace()),
        "only whitespace or comments may follow the generic-cache test module"
    );
    let test_module = &LOWERING_SOURCE[*boundary..=close];
    assert!(
        test_module.contains("fn bare_enum_without_ready_variants_fails_without_unwinding()"),
        "the excluded suffix must remain the generic-cache test module"
    );
    &LOWERING_SOURCE[..*boundary]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InvariantClass {
    CheckerClassifiedType,
    MatchArmNarrowing,
    ParserGuaranteedShape,
    LoweringBookkeeping,
}

impl InvariantClass {
    fn label(self) -> &'static str {
        match self {
            Self::CheckerClassifiedType => "checker-classified type",
            Self::MatchArmNarrowing => "match-arm narrowing",
            Self::ParserGuaranteedShape => "parser-guaranteed shape",
            Self::LoweringBookkeeping => "lowering bookkeeping",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AllowedPanicSite {
    invocation: &'static str,
    class: InvariantClass,
    multiplicity: usize,
}

const ALLOWED_PANIC_SITES: &[AllowedPanicSite] = &[
    AllowedPanicSite {
        invocation: ".expect(\"classified as a nominal\")",
        class: InvariantClass::CheckerClassifiedType,
        multiplicity: 1,
    },
    AllowedPanicSite {
        invocation: ".expect(\"caller classified a nominal\")",
        class: InvariantClass::CheckerClassifiedType,
        multiplicity: 1,
    },
    AllowedPanicSite {
        invocation: "unreachable!(\"classified as an admitted binary op\")",
        class: InvariantClass::MatchArmNarrowing,
        multiplicity: 1,
    },
    AllowedPanicSite {
        invocation: ".expect(\"guard matched\")",
        class: InvariantClass::MatchArmNarrowing,
        multiplicity: 4,
    },
    AllowedPanicSite {
        invocation: "unreachable!(\"only and/or reach short-circuit lowering\")",
        class: InvariantClass::MatchArmNarrowing,
        multiplicity: 1,
    },
    AllowedPanicSite {
        invocation: "unreachable!(\"caller matched the text-floor names\")",
        class: InvariantClass::MatchArmNarrowing,
        multiplicity: 1,
    },
    AllowedPanicSite {
        invocation: "unreachable!(\"caller passes only a temporal scalar\")",
        class: InvariantClass::MatchArmNarrowing,
        multiplicity: 2,
    },
    AllowedPanicSite {
        invocation: "unreachable!(\"caller passes only a date-arithmetic builtin\")",
        class: InvariantClass::MatchArmNarrowing,
        multiplicity: 1,
    },
    AllowedPanicSite {
        invocation: ".expect(\"a set evaluates its value\")",
        class: InvariantClass::MatchArmNarrowing,
        multiplicity: 1,
    },
    AllowedPanicSite {
        invocation: "unreachable!(\"caller passes a non-empty argument list\")",
        class: InvariantClass::MatchArmNarrowing,
        multiplicity: 1,
    },
    AllowedPanicSite {
        invocation: ".expect(\"division has a right operand\")",
        class: InvariantClass::ParserGuaranteedShape,
        multiplicity: 1,
    },
    AllowedPanicSite {
        invocation: "unreachable!(\"patch target is not a jump: {other:?}\")",
        class: InvariantClass::LoweringBookkeeping,
        multiplicity: 1,
    },
    AllowedPanicSite {
        invocation: ".expect(\"loop present\")",
        class: InvariantClass::LoweringBookkeeping,
        multiplicity: 1,
    },
    AllowedPanicSite {
        invocation: ".expect(\"loop was pushed\")",
        class: InvariantClass::LoweringBookkeeping,
        multiplicity: 3,
    },
    AllowedPanicSite {
        invocation: "unreachable!(\"a group-leaf delete is handled before the shared key-path emit\")",
        class: InvariantClass::LoweringBookkeeping,
        multiplicity: 1,
    },
];

/// The generic-enum call guard and constructor share an
/// immutable registry borrow, so the successful template lookup is bound once rather
/// than repeated behind an expectation.
#[test]
fn generic_enum_dispatch_binds_one_template_lookup() {
    let body = LOWERING_SOURCE
        .split_once("fn lower_call_core(")
        .expect("lower_call_core remains present")
        .1
        .split_once("/// An unqualified call")
        .expect("next owner boundary remains present")
        .0;
    assert_eq!(
        body.matches("type_template_by_name(enum_name)").count(),
        1,
        "the immutable successful lookup must be consumed directly"
    );
}

#[derive(Debug, PartialEq, Eq)]
struct ExplicitPanicSite<'a> {
    invocation: &'a str,
    line: usize,
}

const PANIC_METHODS: &[&str] = &["expect", "expect_err", "unwrap", "unwrap_err"];
const PANIC_MACROS: &[&str] = &[
    "panic",
    "unreachable",
    "todo",
    "unimplemented",
    "assert",
    "assert_eq",
    "assert_matches",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_matches",
    "debug_assert_ne",
];

/// Enumerate the closed explicit-panic family without treating comments or literal
/// contents as Rust code. The extracted invocation text remains byte-exact so the
/// allowlist detects message and spelling changes without snapshotting line numbers.
fn explicit_panic_sites(source: &str) -> Vec<ExplicitPanicSite<'_>> {
    let code = mask_comments_and_literals(source);
    let mut sites = Vec::new();
    let mut cursor = 0;

    while cursor < code.len() {
        if !is_ident_start(code[cursor]) {
            cursor += 1;
            continue;
        }

        let ident_start = cursor;
        cursor += 1;
        while cursor < code.len() && is_ident_continue(code[cursor]) {
            cursor += 1;
        }
        let ident = &source[ident_start..cursor];
        let token_start = identifier_token_start(&code, ident_start);

        let invocation = if PANIC_METHODS.contains(&ident) {
            let open = next_non_whitespace(&code, cursor);
            match open {
                Some(open) if code[open] == b'(' => {
                    method_invocation_start(&code, token_start).map(|start| (start, open))
                }
                _ => None,
            }
        } else if PANIC_MACROS.contains(&ident) {
            let Some(bang) = next_non_whitespace(&code, cursor) else {
                continue;
            };
            if code[bang] != b'!' {
                continue;
            }
            let Some(open) = next_non_whitespace(&code, bang + 1) else {
                continue;
            };
            if matches!(code[open], b'(' | b'[' | b'{') {
                Some((token_start, open))
            } else {
                None
            }
        } else {
            None
        };

        let Some((start, open)) = invocation else {
            continue;
        };
        let close = matching_delimiter(&code, open)
            .expect("an invocation in compiling Rust source has a matching delimiter");
        sites.push(ExplicitPanicSite {
            invocation: &source[start..=close],
            line: source[..start]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1,
        });
    }

    sites
}

fn is_ident_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_ident_continue(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit()
}

fn previous_non_whitespace(code: &[u8], before: usize) -> Option<usize> {
    code[..before]
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
}

fn next_non_whitespace(code: &[u8], mut cursor: usize) -> Option<usize> {
    while cursor < code.len() && code[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    (cursor < code.len()).then_some(cursor)
}

fn identifier_token_start(code: &[u8], ident_start: usize) -> usize {
    let Some(raw_start) = ident_start.checked_sub(2) else {
        return ident_start;
    };
    if &code[raw_start..ident_start] == b"r#"
        && raw_start
            .checked_sub(1)
            .is_none_or(|before| !is_ident_continue(code[before]))
    {
        raw_start
    } else {
        ident_start
    }
}

fn method_invocation_start(code: &[u8], token_start: usize) -> Option<usize> {
    let before = previous_non_whitespace(code, token_start)?;
    if code[before] == b'.' {
        return Some(before);
    }
    if code[before] != b':' {
        return None;
    }
    let first_colon = previous_non_whitespace(code, before)?;
    (code[first_colon] == b':').then_some(token_start)
}

fn matching_delimiter(code: &[u8], open: usize) -> Option<usize> {
    let mut stack = vec![match code[open] {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        _ => return None,
    }];

    for (offset, byte) in code[open + 1..].iter().copied().enumerate() {
        match byte {
            b'(' => stack.push(b')'),
            b'[' => stack.push(b']'),
            b'{' => stack.push(b'}'),
            b')' | b']' | b'}' if stack.last() == Some(&byte) => {
                stack.pop();
                if stack.is_empty() {
                    return Some(open + 1 + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// Preserve byte offsets and newlines while blanking nested comments and literals.
/// Character literals are recognized only when a closing apostrophe follows one
/// character or one escape, so lifetime syntax remains visible as ordinary code.
fn mask_comments_and_literals(source: &str) -> Vec<u8> {
    let bytes = source.as_bytes();
    let mut code = bytes.to_vec();
    let mut cursor = 0;

    while cursor < bytes.len() {
        if bytes[cursor..].starts_with(b"//") {
            let end = bytes[cursor..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |offset| cursor + offset);
            blank(&mut code, cursor, end);
            cursor = end;
        } else if bytes[cursor..].starts_with(b"/*") {
            let end = block_comment_end(bytes, cursor);
            blank(&mut code, cursor, end);
            cursor = end;
        } else if let Some(end) = raw_string_end(bytes, cursor) {
            blank(&mut code, cursor, end);
            cursor = end;
        } else if bytes[cursor] == b'"' {
            let end = quoted_string_end(bytes, cursor);
            blank(&mut code, cursor, end);
            cursor = end;
        } else if bytes[cursor] == b'\'' {
            if let Some(end) = character_literal_end(source, cursor) {
                blank(&mut code, cursor, end);
                cursor = end;
            } else {
                cursor += 1;
            }
        } else {
            cursor += 1;
        }
    }

    code
}

fn blank(code: &mut [u8], start: usize, end: usize) {
    for byte in &mut code[start..end] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

fn block_comment_end(bytes: &[u8], start: usize) -> usize {
    let mut depth = 1;
    let mut cursor = start + 2;
    while cursor < bytes.len() {
        if bytes[cursor..].starts_with(b"/*") {
            depth += 1;
            cursor += 2;
        } else if bytes[cursor..].starts_with(b"*/") {
            depth -= 1;
            cursor += 2;
            if depth == 0 {
                return cursor;
            }
        } else {
            cursor += 1;
        }
    }
    bytes.len()
}

fn raw_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut cursor = start;
    if matches!(bytes.get(cursor), Some(b'b' | b'c')) {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'r') {
        return None;
    }
    cursor += 1;

    let hashes_start = cursor;
    while bytes.get(cursor) == Some(&b'#') {
        cursor += 1;
    }
    let hashes = cursor - hashes_start;
    if bytes.get(cursor) != Some(&b'"') {
        return None;
    }
    cursor += 1;

    while cursor < bytes.len() {
        if bytes[cursor] == b'"'
            && bytes.get(cursor + 1..cursor + 1 + hashes)
                == Some(&bytes[hashes_start..hashes_start + hashes])
        {
            return Some(cursor + 1 + hashes);
        }
        cursor += 1;
    }
    Some(bytes.len())
}

fn quoted_string_end(bytes: &[u8], start: usize) -> usize {
    let mut cursor = start + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor = (cursor + 2).min(bytes.len()),
            b'"' => return cursor + 1,
            _ => cursor += 1,
        }
    }
    bytes.len()
}

fn character_literal_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let content = start + 1;
    let next = *bytes.get(content)?;

    let closing = if next == b'\\' {
        let escape = content + 1;
        match *bytes.get(escape)? {
            b'x' => escape.checked_add(3)?,
            b'u' if bytes.get(escape + 1) == Some(&b'{') => {
                let close_brace = bytes[escape + 2..].iter().position(|byte| *byte == b'}')?;
                escape + 3 + close_brace
            }
            _ => escape + 1,
        }
    } else {
        let character = source[content..].chars().next()?;
        content + character.len_utf8()
    };

    (bytes.get(closing) == Some(&b'\'')).then_some(closing + 1)
}

#[test]
fn the_explicit_panic_counter_covers_the_closed_family_and_ignores_non_code() {
    let source = r####"
value.expect("live");
value.expect_err("live");
value.unwrap();
value.unwrap_err();
panic!("live");
unreachable!["live"];
todo! { "live" };
unimplemented!("live");
assert!(live);
assert_eq!(live, live);
assert_ne!(live, other);
debug_assert!(live);
debug_assert_eq!(live, live);
assert_matches!(live, Some(_));
debug_assert_matches!(live, Some(_));
debug_assert_ne!(live, other);
// value.expect("comment") and panic!("comment")
/* unreachable!("outer comment"); /* todo!("nested comment"); */ */
const NORMAL: &str = "value.unwrap(); assert!(not_code);";
const RAW: &str = r#"value.unwrap_err(); debug_assert!(not_code);"#;
const QUOTE: char = '\"';
const BYTE_QUOTE: u8 = b'\"';
const APOSTROPHE: char = '\'';
const HEX_QUOTE: char = '\x22';
const UNICODE_QUOTE: char = '\u{22}';
const UNICODE: char = 'λ';
const LIFETIME: PhantomData<&'static str> = PhantomData;
fn expect(value: Value) {}
expect(value);
Option::expect(value, "qualified expect");
Result::expect_err(value, "qualified expect_err");
Option::unwrap(value);
Result::unwrap_err(value);
value.r#expect("raw dot expect");
value.r#expect_err("raw dot expect_err");
value.r#unwrap();
value.r#unwrap_err();
Option::r#expect(value, "raw qualified expect");
Result::r#expect_err(value, "raw qualified expect_err");
Option::r#unwrap(value);
Result::r#unwrap_err(value);
r#panic!("raw macro");
r#debug_assert_matches!(live, Some(_));
assert_eq!(')', ')');
debug_assert_eq!(b'(', b'(');
after_literals.expect("live after literals");
panic!("live after literals");
"####;

    let invocations: Vec<_> = explicit_panic_sites(source)
        .into_iter()
        .map(|site| site.invocation)
        .collect();
    assert_eq!(
        invocations,
        vec![
            ".expect(\"live\")",
            ".expect_err(\"live\")",
            ".unwrap()",
            ".unwrap_err()",
            "panic!(\"live\")",
            "unreachable![\"live\"]",
            "todo! { \"live\" }",
            "unimplemented!(\"live\")",
            "assert!(live)",
            "assert_eq!(live, live)",
            "assert_ne!(live, other)",
            "debug_assert!(live)",
            "debug_assert_eq!(live, live)",
            "assert_matches!(live, Some(_))",
            "debug_assert_matches!(live, Some(_))",
            "debug_assert_ne!(live, other)",
            "expect(value, \"qualified expect\")",
            "expect_err(value, \"qualified expect_err\")",
            "unwrap(value)",
            "unwrap_err(value)",
            ".r#expect(\"raw dot expect\")",
            ".r#expect_err(\"raw dot expect_err\")",
            ".r#unwrap()",
            ".r#unwrap_err()",
            "r#expect(value, \"raw qualified expect\")",
            "r#expect_err(value, \"raw qualified expect_err\")",
            "r#unwrap(value)",
            "r#unwrap_err(value)",
            "r#panic!(\"raw macro\")",
            "r#debug_assert_matches!(live, Some(_))",
            "assert_eq!(')', ')')",
            "debug_assert_eq!(b'(', b'(')",
            ".expect(\"live after literals\")",
            "panic!(\"live after literals\")",
        ]
    );
}

#[test]
fn every_explicit_lowering_panic_is_allowlisted() {
    let sites = explicit_panic_sites(production_lowering_source());
    let expected_total: usize = ALLOWED_PANIC_SITES
        .iter()
        .map(|allowed| allowed.multiplicity)
        .sum();
    let mut drift = Vec::new();

    if sites.len() != expected_total {
        drift.push(format!(
            "complete explicit-family count is {}, allowlist multiplicity is {}",
            sites.len(),
            expected_total
        ));
    }

    for (index, allowed) in ALLOWED_PANIC_SITES.iter().enumerate() {
        if ALLOWED_PANIC_SITES[..index]
            .iter()
            .any(|prior| prior.invocation == allowed.invocation)
        {
            drift.push(format!(
                "duplicate allowlist entry {:?}",
                allowed.invocation
            ));
            continue;
        }
        let actual = sites
            .iter()
            .filter(|site| site.invocation == allowed.invocation)
            .count();
        if actual != allowed.multiplicity {
            drift.push(format!(
                "{} {:?}: expected {}, found {}",
                allowed.class.label(),
                allowed.invocation,
                allowed.multiplicity,
                actual
            ));
        }
    }

    let unclassified: Vec<_> = sites
        .iter()
        .filter(|site| {
            !ALLOWED_PANIC_SITES
                .iter()
                .any(|allowed| allowed.invocation == site.invocation)
        })
        .collect();
    if !unclassified.is_empty() {
        drift.push(format!(
            "{} unclassified explicit site(s):\n{}",
            unclassified.len(),
            unclassified
                .iter()
                .map(|site| format!("  line {}: {}", site.line, site.invocation))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    assert!(
        drift.is_empty(),
        "lower.rs explicit-panic allowlist drift:\n{}",
        drift.join("\n")
    );
}

#[test]
fn explicit_panic_audit_excludes_only_the_pinned_test_module() {
    let production = production_lowering_source();
    assert!(!production.contains("mod generic_cache_boundary_tests"));
    assert!(LOWERING_SOURCE[production.len()..].starts_with(TEST_ONLY_BOUNDARY));
}

fn project(source: &str) -> ProjectInput {
    let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
    let files = vec![CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    marrow_project::capture(&manifest, files, None, &CaptureLimits::DEFAULT)
        .expect("capture project")
}

/// `compile` must return a diagnostic carrying `code`, not panic and not succeed.
fn rejects_with(source: &str, code: &str) {
    match compile(&project(source)) {
        Ok(_) => panic!("expected `{code}`, but the program compiled:\n{source}"),
        Err(marrow_compile::CompileFailure::Diagnostics(diagnostics)) => assert!(
            diagnostics
                .iter()
                .any(|d: &SourceDiagnostic| d.code == code),
            "expected `{code}` for:\n{source}\ngot {diagnostics:#?}",
        ),
        Err(marrow_compile::CompileFailure::Invariant(_)) => {
            panic!("source-triggered compiler failures must remain diagnostics")
        }
    }
}

/// Loop-bookkeeping class: `break`/`continue` reach lowering only inside a loop, where
/// the loop context is present. Outside a loop the checker rejects them first.
#[test]
fn break_and_continue_outside_a_loop_are_diagnostics_not_panics() {
    rejects_with(
        "pub fn f(): int {\n    break\n    return 0\n}\n",
        "check.type",
    );
    rejects_with(
        "pub fn f(): int {\n    continue\n    return 0\n}\n",
        "check.type",
    );
}

/// Checker-classified-type class: a `match` scrutinee lowers only after it resolves to
/// an enum. A scrutinee that is not an enum is rejected before lowering.
#[test]
fn a_match_on_a_non_enum_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(n: int): int {\n    match n {\n        x => return x\n    }\n}\n",
        "check.match_arm",
    );
}

/// Match-arm-narrowing class: a builtin dispatch reaches its op only after the caller
/// matched its name and arity. A mis-arity call is rejected before that point.
#[test]
fn a_mis_arity_builtin_call_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(s: string): int {\n    return length(s, s)\n}\n",
        "check.type",
    );
}

/// Op-classification class: an arithmetic/comparison op lowers only after its operands
/// type-check. An ill-typed operator is rejected before op classification.
#[test]
fn an_ill_typed_operator_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(a: string, b: string): int {\n    return a / b\n}\n",
        "check.type",
    );
}

/// Enum-classification class: a bare enum member lowers only after it resolves to its
/// enum's variants. An unresolved member is rejected before lowering reaches it.
#[test]
fn an_unresolved_enum_member_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(): int {\n    const x = Nope::member\n    return 0\n}\n",
        "check.unsupported",
    );
}

/// List-literal class: the inferred-element path runs only for a non-empty list. An
/// empty `List()` with no element or annotation type is rejected before that path.
#[test]
fn an_empty_inferred_list_is_a_diagnostic_not_a_panic() {
    rejects_with(
        "pub fn f(): int {\n    const xs = List()\n    return 0\n}\n",
        "check.type",
    );
}
