use marrow_check::tooling::{
    CallableArgumentStyle, CallableParameter, CallableSignature, CallableSignatureKind,
    CallableValueShape, render_callable_signature, render_marrow_type,
};
use marrow_check::{MarrowType, ScalarType};

use crate::support;

#[test]
fn canonical_type_and_callable_rendering_is_bound_to_the_checked_program() {
    let source = "\
module m

resource Book
    title: string

store ^books(id: int): Book

pub fn keep(book: Book): Book
    return book
";
    let (report, program) =
        support::check_module_report_program("tooling-render-checked-program", source);
    support::assert_clean(&report);
    let module = program.facts.module_id("m").expect("m module");
    let book = program
        .facts
        .resource_id(module, "Book")
        .expect("Book resource");
    let ty = MarrowType::Sequence(Box::new(MarrowType::Resource(book)));

    assert_eq!(render_marrow_type(&program, &ty), "sequence[m::Book]");

    let callable = CallableSignature {
        path: vec!["m".to_string(), "keep".to_string()],
        kind: CallableSignatureKind::Builtin,
        argument_style: CallableArgumentStyle::NamedFields,
        docs: Vec::new(),
        params: vec![CallableParameter {
            label: "book".to_string(),
            required: true,
            repeat: false,
            shape: CallableValueShape::Type(MarrowType::Resource(book)),
            docs: Vec::new(),
        }],
        return_shape: Some(CallableValueShape::Type(MarrowType::Primitive(
            ScalarType::Int,
        ))),
    };
    assert_eq!(
        render_callable_signature(&program, &callable),
        "m::keep(book: m::Book): int"
    );
}
