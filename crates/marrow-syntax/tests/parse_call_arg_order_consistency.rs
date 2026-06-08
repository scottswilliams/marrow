//! Positional-after-named is rejected the same way in every call position the
//! grammar allows. Resource constructors, conversion calls, std-qualified calls,
//! and saved-path key lookups each parse their argument list through one shared
//! rule, so the typed reason and the diagnostic count must not vary by call
//! shape. The bare-identifier-call form is already covered elsewhere; this pins
//! the *other* parsed callee shapes to that same rule so they cannot diverge.

use marrow_syntax::{ParseDiagnosticReason, parse_source};

mod common;

use common::{parse_reason, reason_count};

/// Each case is a distinct callee shape in `primary_expr`, all reaching the same
/// `arguments()` rule through the shared postfix-call path. The label names the
/// grammar position so a failure says which call shape diverged.
const POSITIONAL_AFTER_NAMED: &[(&str, &str)] = &[
    // Resource constructor: `Error(...)` is parsed as a resource literal.
    (
        "resource constructor (Error)",
        "const Made = Error(code: \"e\", \"oops\")\n",
    ),
    // Resource constructor with a user resource name.
    (
        "resource constructor (named resource)",
        "const Made = Book(title: \"t\", \"extra\")\n",
    ),
    // Conversion call: a scalar type keyword in call position.
    ("conversion call (int)", "const Made = int(value: 1, 2)\n"),
    // Std-qualified call: a `::` name path callee.
    (
        "std-qualified call",
        "const Made = std::math::clamp(low: 0, 9)\n",
    ),
    // Saved-path key lookup shaped as a call on a saved root.
    ("saved-root key lookup", "const Made = ^books(id: 1, 2)\n"),
];

/// Every distinct parsed call shape rejects a positional argument after a named
/// one with the same typed reason, raised exactly once. A rule that fired for the
/// bare-identifier call but not for a constructor or conversion call would let a
/// silent positional back-fill through one syntactic door.
#[test]
fn positional_after_named_is_rejected_in_every_call_shape() {
    for (label, source) in POSITIONAL_AFTER_NAMED {
        let parsed = parse_source(source);
        assert_eq!(
            reason_count(
                &parsed.diagnostics,
                parse_reason(ParseDiagnosticReason::PositionalArgumentAfterNamed),
            ),
            1,
            "{label}: expected exactly one positional-after-named diagnostic: {:#?}",
            parsed.diagnostics,
        );
    }
}

/// The mirror image: a positional argument *before* a named one is accepted in
/// every one of those same call shapes, so the rule rejects only the disallowed
/// ordering rather than any mix of positional and named arguments.
#[test]
fn positional_before_named_is_accepted_in_every_call_shape() {
    let cases = [
        (
            "resource constructor (Error)",
            "const Made = Error(\"oops\", code: \"e\")\n",
        ),
        ("conversion call (int)", "const Made = int(1, scale: 2)\n"),
        (
            "std-qualified call",
            "const Made = std::math::clamp(0, high: 9)\n",
        ),
        ("saved-root key lookup", "const Made = ^books(1, hint: 2)\n"),
    ];
    for (label, source) in cases {
        let parsed = parse_source(source);
        assert_eq!(
            reason_count(
                &parsed.diagnostics,
                parse_reason(ParseDiagnosticReason::PositionalArgumentAfterNamed),
            ),
            0,
            "{label}: positional-before-named must be accepted: {:#?}",
            parsed.diagnostics,
        );
    }
}
