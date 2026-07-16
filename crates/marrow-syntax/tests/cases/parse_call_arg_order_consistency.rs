//! Positional-after-named is rejected the same way in every call position the
//! grammar allows. Resource constructors, conversion calls, std-qualified calls,
//! and saved-path key lookups each parse their argument list through one shared
//! rule, so the typed reason and the diagnostic count must not vary by call
//! shape. The bare-identifier-call form is already covered elsewhere; this pins
//! the *other* parsed callee shapes to that same rule so they cannot diverge.

use crate::common;
use common::{parse_reason, reason_count};
use marrow_syntax::{ParseDiagnosticReason, parse_source};

/// Each case is a distinct callee shape in `primary_expr`, all reaching the same
/// `arguments()` rule through the shared postfix-call path. `reject` puts a
/// positional argument after a named one; `accept` is the mirror image with the
/// allowed ordering. Both tests drive from this one set so the rejected and
/// accepted halves provably cover the same callee shapes. The label names the
/// grammar position so a failure says which call shape diverged.
const CALL_SHAPES: &[CallShape] = &[
    // Resource constructor: `Error(...)` is parsed as a resource literal.
    CallShape {
        label: "resource constructor (Error)",
        reject: "const Made = Error(code: \"parse.error\", \"oops\")\n",
        accept: "const Made = Error(\"oops\", code: \"e\")\n",
    },
    // Conversion call: a scalar type keyword in call position.
    CallShape {
        label: "conversion call (int)",
        reject: "const Made = int(value: 1, 2)\n",
        accept: "const Made = int(1, scale: 2)\n",
    },
    // Std-qualified call: a `::` name path callee.
    CallShape {
        label: "std-qualified call",
        reject: "const Made = std::math::clamp(low: 0, 9)\n",
        accept: "const Made = std::math::clamp(0, high: 9)\n",
    },
    // Saved-path key lookup shaped as a call on a saved root.
    CallShape {
        label: "saved-root key lookup",
        reject: "const Made = ^books(id: 1, 2)\n",
        accept: "const Made = ^books(1, hint: 2)\n",
    },
];

struct CallShape {
    label: &'static str,
    reject: &'static str,
    accept: &'static str,
}

/// Every distinct parsed call shape rejects a positional argument after a named
/// one with the same typed reason, raised exactly once. A rule that fired for the
/// bare-identifier call but not for a constructor or conversion call would let a
/// silent positional back-fill through one syntactic door.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn positional_after_named_is_rejected_in_every_call_shape() {
    for shape in CALL_SHAPES {
        let parsed = parse_source(shape.reject);
        assert_eq!(
            reason_count(
                &parsed.diagnostics,
                parse_reason(ParseDiagnosticReason::PositionalArgumentAfterNamed),
            ),
            1,
            "{}: expected exactly one positional-after-named diagnostic: {:#?}",
            shape.label,
            parsed.diagnostics,
        );
    }
}

/// The mirror image: a positional argument *before* a named one is accepted in
/// every one of those same call shapes, so the rule rejects only the disallowed
/// ordering rather than any mix of positional and named arguments.
#[test]
#[ignore = "BS01: layout corpus, rewritten in the converter flip"]
fn positional_before_named_is_accepted_in_every_call_shape() {
    for shape in CALL_SHAPES {
        let parsed = parse_source(shape.accept);
        assert_eq!(
            reason_count(
                &parsed.diagnostics,
                parse_reason(ParseDiagnosticReason::PositionalArgumentAfterNamed),
            ),
            0,
            "{}: positional-before-named must be accepted: {:#?}",
            shape.label,
            parsed.diagnostics,
        );
    }
}
