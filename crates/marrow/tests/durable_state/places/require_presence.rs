//! Require guards share the named-place proof's scope and invalidation rules.

use super::{HEADER, IDS, compile_diagnostics, compile_verify, position_of};
use crate::common::Project;
use marrow_codes::Code;
use marrow_vm::Value;

const REQUIRE: &str = "require exists(p) else failure(n)";
const EXPLICIT: &str = "if not exists(p) { return err(failure(n)) }";

fn program(body: &str) -> String {
    format!(
        "{HEADER}\nfn failure(n: int): string {{\n    delete ^counters[n]\n    ^counters[98] = Counter(value: 9)\n    return \"missing\"\n}}\n\npub fn put(n: int, again: bool): Result<int, string> {{\n    transaction {{\n        place p = ^counters[n]\n        place other = ^counters[99]\n        var repeat = again\n        {body}\n        return ok(7)\n    }}\n}}\n"
    )
}

#[test]
fn require_presence_preserves_lazy_failure_and_committed_results() {
    for guard in [EXPLICIT, REQUIRE] {
        let source = format!(
            "{}\npub fn seed() {{ transaction {{ ^counters[1] = Counter(value: 1) }} }}\npub fn read(n: int): int? {{ return ^counters[n].value }}\n",
            program(&format!(
                "^counters[99] = Counter(value: n)\n        {guard}\n        p.value = 7\n        const actual: int = p.value"
            ))
        );
        let mut session = Project::single(&source).ids(IDS).session();
        session.call("seed", vec![]);
        let Some(Value::Enum(_, variant, payload)) =
            session.call("put", vec![Value::Int(1), Value::Bool(false)])
        else {
            panic!("the guarded update returns a Result");
        };
        assert_eq!(variant, 0, "the present path returns ok");
        assert_eq!(&*payload, &[Value::Int(7)]);
        assert_eq!(
            session.call("read", vec![Value::Int(1)]),
            Some(Value::Optional(Some(Box::new(Value::Int(7)))))
        );
        assert_eq!(
            session.call("read", vec![Value::Int(98)]),
            Some(Value::Optional(None)),
            "the failure expression was not evaluated"
        );
        let Some(Value::Enum(_, variant, payload)) =
            session.call("put", vec![Value::Int(2), Value::Bool(false)])
        else {
            panic!("the absent path returns a Result");
        };
        assert_eq!(variant, 1, "the absent path returns err");
        assert_eq!(&*payload, &[Value::Text("missing".into())]);
        for (key, expected) in [(98, 9), (99, 2)] {
            assert_eq!(
                session.call("read", vec![Value::Int(key)]),
                Some(Value::Optional(Some(Box::new(Value::Int(expected))))),
                "normal error commits the failure expression and earlier writes"
            );
        }
        assert_eq!(
            session.call("read", vec![Value::Int(2)]),
            Some(Value::Optional(None))
        );
    }
}

#[test]
fn require_presence_ends_at_erasure_loop_and_scope_boundaries() {
    let bodies = [
        "GUARD\n delete p\n USE",
        "GUARD\n delete other\n USE",
        "GUARD\n const ignored = failure(n)\n USE",
        "GUARD\n while repeat { USE\n delete p\n repeat = false }",
        "GUARD\n while repeat { USE\n const ignored = failure(n)\n repeat = false }",
        "GUARD\n while repeat { delete p\n repeat = false }\n USE",
    ];
    for guard in [EXPLICIT, REQUIRE] {
        for body in bodies {
            for protected in ["p.value = 7", "const value: int? = p.value"] {
                let source = program(&body.replace("GUARD", guard).replace("USE", protected));
                let (line, column) = position_of(&source, "p.value");
                assert_eq!(
                    compile_diagnostics(&source),
                    vec![(
                        Code::CheckRequiresPresence.as_str().to_owned(),
                        line,
                        column
                    )],
                    "{source}"
                );
            }
        }
        let source = program(&format!("if again {{ {guard} }}\n p.value = 7"));
        let (line, column) = position_of(&source, "p.value");
        assert_eq!(
            compile_diagnostics(&source),
            vec![(
                Code::CheckRequiresPresence.as_str().to_owned(),
                line,
                column
            )]
        );
    }
}

#[test]
fn require_presence_can_be_renewed_inside_a_repeating_region() {
    for guard in [EXPLICIT, REQUIRE] {
        let source = program(&format!(
            "while repeat {{ {guard}\n p.value = 7\n const value: int = p.value\n delete p\n repeat = false }}"
        ));
        compile_verify(&source);
    }
}

#[test]
fn require_presence_does_not_refine_a_different_place_or_inline_path() {
    for condition in [
        "exists(other)",
        "exists(^counters[n])",
        "exists(p) and again",
    ] {
        let source = program(&format!(
            "require {condition} else \"missing\"\n p.value = 7"
        ));
        let (line, column) = position_of(&source, "p.value");
        assert_eq!(
            compile_diagnostics(&source),
            vec![(
                Code::CheckRequiresPresence.as_str().to_owned(),
                line,
                column
            )]
        );
    }
}
