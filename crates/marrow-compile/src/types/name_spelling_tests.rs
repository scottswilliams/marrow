//! The spellings `Error`, `ErrorCode` and `unknown` are ordinary identifiers: each names
//! a type, a constant, a field, a function and a parameter through the production
//! compile path, and each such program is its own formatting fixed point.

use marrow_syntax::check_format;

use super::generic_instantiation_tests::project;
use crate::compile::compile;

#[test]
fn the_retired_type_spellings_are_ordinary_names_in_every_position() {
    for name in ["Error", "ErrorCode", "unknown"] {
        let programs = [
            (
                "type",
                format!(
                    "module main\n\nstruct {name} {{\n    value: int\n}}\n\n\
                     pub fn driver(): int {{\n    return {name}(value: 1).value\n}}\n"
                ),
            ),
            (
                "const",
                format!(
                    "module main\n\nconst {name}: int = 1\n\n\
                     pub fn driver(): int {{\n    return {name}\n}}\n"
                ),
            ),
            (
                "field",
                format!(
                    "module main\n\nstruct Holder {{\n    {name}: int\n}}\n\n\
                     pub fn driver(): int {{\n    return Holder({name}: 1).{name}\n}}\n"
                ),
            ),
            (
                "function",
                format!(
                    "module main\n\nfn {name}(): int {{\n    return 1\n}}\n\n\
                     pub fn driver(): int {{\n    return {name}()\n}}\n"
                ),
            ),
            (
                "parameter",
                format!(
                    "module main\n\npub fn driver({name}: int): int {{\n    return {name}\n}}\n"
                ),
            ),
        ];
        for (role, source) in programs {
            compile(&project(source.clone())).unwrap_or_else(|failure| {
                panic!("`{name}` as a {role} name compiles: {failure:?}")
            });
            let formatted = check_format(&source)
                .unwrap_or_else(|refusal| panic!("`{name}` as a {role} name formats: {refusal:?}"));
            assert_eq!(
                formatted, source,
                "`{name}` as a {role} name is a formatting fixed point"
            );
        }
    }
}
