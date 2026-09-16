//! The issuance gate's width half: four hostile amplification corpora, each a divergent
//! generic at the widest body its own bound admits, driven through the production
//! `compile` path in this process.
//!
//! The corpora are a `MAX_RECORD_FIELDS` generic struct, a `MAX_VARIANTS` x
//! `MAX_PAYLOAD_FIELDS` generic enum, a generic function filling its local frame, and all
//! three together. The function arm reaches all 4,096 instantiations the shared count
//! bound admits; the self-nesting type and enum arms stop at the separate 256-deep mint
//! bound, and the combined arm stops there with them.
//!
//! Each width is read from the bound that governs the construct it widens, so a bound
//! change moves the corpus with it.

use marrow_codes::Code;

use marrow_compile::{CompileFailure, compile};
use marrow_syntax::SourceSpan;

use super::project;

/// The widest admissible record declaration. A generic `struct` template is a record
/// declaration, so its width is `MAX_RECORD_FIELDS`, not the unrelated `MAX_STRUCT_LEAVES`
/// value-shape bound — reading that one would make the corpus sixty-four times narrower
/// than the widest body the compiler admits.
const ADMITTED_RECORD_FIELDS: usize = marrow_image::bounds::MAX_RECORD_FIELDS;

/// The widest admissible function body by the bound over a function's local slots.
///
/// Two slots of the frame are spent before the steps are: the parameter and the `xs`
/// accumulator the steps read. The corpus takes the rest, so the frame is full and one
/// more step would be refused by the frame bound instead of the instantiation ceiling.
const ADMITTED_LOCALS: usize = marrow_image::bounds::MAX_LOCALS - 2;

/// The widest admissible function body by the *other* bound that governs one: the bytes of
/// compiled code a single function admits.
///
/// Frame width and code width are independent dimensions — a body can fill all 256 local
/// slots and still carry a small fraction of the 64 KiB of code a function admits — and
/// each generic instance retains its own frame and code.
///
/// The number is **observed, not computed**: it is the largest padding whose body still
/// encodes, and one more statement is refused at its source span by the code-byte limit.
/// `the_function_arm_sits_exactly_at_the_code_byte_envelope` re-observes both halves of
/// that boundary on the arm's own generator, so this constant cannot drift.
const ADMITTED_CODE_PADDING: usize = 6145;

/// The type-amplification arm: a divergent generic type over the largest admissible
/// body, reached through a generic function's return annotation.
///
/// The annotation is what makes the arm live. A generic type is monomorphized only on
/// *use*, so declaring `Grow<T>` without naming it in a position that resolves it leaves
/// the arm dead and the corpus compiling. Naming it as `deepen`'s return type resolves it
/// once per instance, and `next: Grow<List<T>>` makes each resolution demand the next.
fn type_amplification_arm() -> String {
    let mut source = String::from("struct Grow<T> {\n");
    for leaf in 0..ADMITTED_RECORD_FIELDS - 1 {
        source.push_str(&format!("    leaf{leaf}: T\n"));
    }
    source.push_str("    next: Grow<List<T>>\n}\n\n");
    source.push_str("fn deepen<T>(x: T): Grow<T> {\n    return deepen(x)\n}\n\n");
    source
}

/// The widest admissible enum body: every variant a closed enum admits, each carrying
/// every payload leaf a variant admits.
const ADMITTED_VARIANTS: usize = marrow_image::bounds::MAX_VARIANTS;
const ADMITTED_PAYLOAD_FIELDS: usize = marrow_image::bounds::MAX_PAYLOAD_FIELDS;

/// The enum-amplification arm: a divergent generic enum at the widest admissible variant
/// and payload width.
///
/// Struct and enum fills take separate paths over separately bounded populations: a struct
/// fill materializes `MAX_RECORD_FIELDS` declared fields, while an enum fill materializes
/// `MAX_VARIANTS` variants each of `MAX_PAYLOAD_FIELDS` leaves. The enum shape is the
/// larger of the two per instantiation, so its cost is measured here rather than argued.
fn enum_amplification_arm() -> String {
    let mut source = String::from("struct Wrap<T> {\n    inner: T\n}\n\n");
    source.push_str("enum Grown<T> {\n");
    for variant in 0..ADMITTED_VARIANTS - 1 {
        let payload: Vec<String> = (0..ADMITTED_PAYLOAD_FIELDS)
            .map(|leaf| format!("p{leaf}: T"))
            .collect();
        source.push_str(&format!("    v{variant}({})\n", payload.join(", ")));
    }
    // The recursive variant carries the full payload width like every other: one leaf is
    // the recursion that mints the next instance, the rest are ordinary payload. A final
    // variant one leaf wide would leave the widest admitted enum body unmeasured at
    // exactly the variant whose resolution drives the amplification.
    let mut tail: Vec<String> = (0..ADMITTED_PAYLOAD_FIELDS - 1)
        .map(|leaf| format!("p{leaf}: T"))
        .collect();
    tail.push("n: Grown<Wrap<T>>".to_string());
    source.push_str(&format!("    next({})\n}}\n\n", tail.join(", ")));
    source.push_str("fn sprout<T>(x: T): Grown<T> {\n    return sprout(x)\n}\n\n");
    source
}

/// A project holding only the enum-amplification arm.
fn enum_only_corpus() -> String {
    format!(
        "module main\n\n{}pub fn driver(): int {{\n    const ignored = sprout(1)\n    return 0\n}}\n",
        enum_amplification_arm(),
    )
}

/// The function-amplification arm: a divergent generic function whose per-instance body
/// and span shape amplify, diverging on an ever-growing argument.
fn function_amplification_arm() -> String {
    let mut source = String::from("fn grow<T>(x: T): int {\n    var xs: List<T> = List()\n");
    for step in 0..ADMITTED_LOCALS {
        source.push_str(&format!("    var step{step}: List<T> = xs\n"));
        source.push_str(&format!("    xs = append(step{step}, x)\n"));
    }
    // Padding to the code-byte envelope drives the second, independent width dimension:
    // each instance retains its own code as well as its frame.
    for _ in 0..ADMITTED_CODE_PADDING {
        source.push_str("    xs = append(xs, x)\n");
    }
    source.push_str("    return grow(xs)\n}\n\n");
    source
}

/// The generic function arm's body with its divergence removed, so it reaches the encoder
/// that owns the code-byte bound. Derived from the arm's own generator by substitution, so
/// the two cannot drift into different bodies.
fn code_envelope_mirror(pad: usize) -> String {
    let mut arm = String::from("fn grow<T>(x: T): int {\n    var xs: List<T> = List()\n");
    for step in 0..ADMITTED_LOCALS {
        arm.push_str(&format!("    var step{step}: List<T> = xs\n"));
        arm.push_str(&format!("    xs = append(step{step}, x)\n"));
    }
    for _ in 0..pad {
        arm.push_str("    xs = append(xs, x)\n");
    }
    arm.push_str("    return grow(xs)\n}\n\n");
    // The divergence is replaced by a call of the same shape rather than removed: a
    // monomorphic self-call is a refused recursion cycle, and dropping the call would
    // measure a body one call instruction narrower than the arm's.
    let body = arm
        .replace("fn grow<T>(x: T): int", "fn grow(x: int): int")
        .replace("List<T>", "List<int>")
        .replace("    return grow(xs)\n", "    return settle(xs)\n");
    format!(
        "module main\n\nfn settle(v: List<int>): int {{\n    return 0\n}}\n\n{body}\
         pub fn driver(): int {{\n    return grow(1)\n}}\n"
    )
}

/// A project holding only the type-amplification arm.
fn type_only_corpus() -> String {
    format!(
        "module main\n\n{}pub fn driver(): int {{\n    const ignored = deepen(1)\n    return 0\n}}\n",
        type_amplification_arm(),
    )
}

/// The combined hostile project contains every arm. It charges their admitted source and
/// declarations together, but lowering stops at the type arm's first depth refusal before
/// the enum and function calls instantiate their bodies.
///
/// This is an early-stop interaction corpus, not the maximum. Type and function instances
/// share one count ceiling (`type_insts.len() + fn_insts.len() >= MAX_INSTANTIATIONS`),
/// but the self-nesting type arm reaches the separate 256-deep mint bound first, so the
/// function-only corpus is the admitted maximum this gate measures.
fn hostile_corpus() -> String {
    format!(
        "module main\n\n{}{}{}pub fn driver(): int {{\n    const ignored = deepen(1)\n             const grown = sprout(1)\n    return grow(1)\n}}\n",
        type_amplification_arm(),
        enum_amplification_arm(),
        function_amplification_arm(),
    )
}

/// Whether compilation returns the shared `check.instantiation_limit` code.
///
/// The code alone does not distinguish the 4,096-instance count limit from the 256-level
/// type-instantiation nesting limit: the self-nesting type and enum corpora reach the depth
/// limit first, while the function corpus reaches the count limit because function
/// reservation has no depth guard.
fn reaches_a_generic_mint_bound(source: &str) -> bool {
    match compile(&project(source, None)) {
        Err(CompileFailure::Diagnostics(diagnostics)) => diagnostics
            .iter()
            .any(|row| row.code() == Code::CheckInstantiationLimit),
        _ => false,
    }
}

/// Each fast type-amplification corpus is live: alone or combined, it drives generic
/// instantiation to a governing mint bound.
///
/// A dead shape reaches neither mint bound, so the shared refusal shows the arm is
/// populated; it does not claim which of the two bounds fired.
#[test]
fn each_fast_type_amplification_corpus_reaches_a_generic_mint_bound() {
    assert!(
        reaches_a_generic_mint_bound(&type_only_corpus()),
        "the generic-type arm alone reaches a generic-mint bound",
    );
    assert!(
        reaches_a_generic_mint_bound(&enum_only_corpus()),
        "the generic-enum arm alone reaches a generic-mint bound",
    );
    assert!(
        reaches_a_generic_mint_bound(&hostile_corpus()),
        "the combined corpus reaches a generic-mint bound",
    );
}

// The maximal arm's *liveness* is not asserted separately here: compiling it costs ~500 s,
// and `inner_hostile_amplification_compile` — the subprocess the peak measurement already
// spawns for it — asserts the same `check.instantiation_limit` refusal on the same corpus.

/// Each corpus is driven at the width of the bound that governs the construct it widens.
///
/// Every other assertion here stays green on a corpus sixty-four times too narrow: a narrow
/// corpus still reaches a generic-mint bound and still refuses as a source diagnostic. Only
/// this test measures how wide the bodies are. The widths are counted out of the generated
/// source rather than recomputed from the constants that generate it, so a corpus that
/// stopped emitting what it claims to emit fails here.
#[test]
fn each_corpus_is_driven_at_the_width_of_the_bound_that_governs_it() {
    // Asserting the two bounds differ is what makes swapping one back for the other loud
    // instead of merely narrower.
    assert_ne!(
        marrow_image::bounds::MAX_RECORD_FIELDS,
        marrow_image::bounds::MAX_STRUCT_LEAVES,
        "the record width and the dense value-shape leaf count are separate bounds",
    );

    let structs = type_amplification_arm();
    assert_eq!(
        structs.matches("    leaf").count() + 1,
        marrow_image::bounds::MAX_RECORD_FIELDS,
        "the generic struct template is declared at the full record-field width",
    );

    // Scoped to the enum declaration: the arm also carries a wrapper struct and a driver
    // function, whose own annotations are not variant payloads.
    let arm = enum_amplification_arm();
    let opened = arm
        .find("enum Grown<T> {")
        .expect("the enum arm declares its enum");
    let enums = &arm[opened..arm[opened..].find("\n}\n").expect("the enum closes") + opened];
    assert_eq!(
        enums.matches("    v").count() + 1,
        marrow_image::bounds::MAX_VARIANTS,
        "the generic enum template is declared at the full variant width",
    );
    // The recursive variant's last leaf is the recursion, so it contributes one fewer `: T`.
    assert_eq!(
        enums.matches(": T,").count() + enums.matches(": T)").count(),
        marrow_image::bounds::MAX_VARIANTS * marrow_image::bounds::MAX_PAYLOAD_FIELDS - 1,
        "every variant carries the full payload width, the recursive variant included",
    );
    assert!(
        enums.contains(&format!(
            "p{}: T, n: Grown<Wrap<T>>)",
            marrow_image::bounds::MAX_PAYLOAD_FIELDS - 2
        )),
        "the recursive variant's last leaf is the recursion and the rest are payload",
    );

    let functions = function_amplification_arm();
    assert_eq!(
        functions.matches("    var step").count(),
        marrow_image::bounds::MAX_LOCALS - 2,
        "the generic function fills its local frame, less the parameter and accumulator",
    );
    assert_eq!(
        functions.matches("    xs = append(xs, x)").count(),
        ADMITTED_CODE_PADDING,
        "the generic function is padded to the code-byte envelope, the second and \
         independent dimension of a function body's width",
    );
}

/// The function arm's body sits exactly at the code-byte envelope: it encodes, and one
/// more statement is refused at the first instruction that would cross the bound. The
/// refused instruction is deliberately in a binary expression's left operand; the unknown
/// name on the right proves that refusal stops the construct immediately rather than
/// continuing to lower a sibling operand.
///
/// Both halves are asserted: a padding that merely encodes proves only that the body is
/// *somewhere* under the bound, which would leave `ADMITTED_CODE_PADDING` a chosen number
/// rather than an observed boundary.
#[test]
fn the_function_arm_sits_exactly_at_the_code_byte_envelope() {
    match compile(&project(&code_envelope_mirror(ADMITTED_CODE_PADDING), None)) {
        Ok(_) => {}
        other => panic!(
            "the arm's body at {ADMITTED_CODE_PADDING} padding statements must encode: \
             {other:?}"
        ),
    }
    let over_bound = code_envelope_mirror(ADMITTED_CODE_PADDING + 1).replace(
        "    return settle(xs)\n",
        "    return -settle(xs) + missing\n",
    );
    let crossing_text = "-settle(xs)";
    let crossing_start = over_bound
        .rfind(crossing_text)
        .expect("the over-bound mirror carries its crossing left operand");
    let offending_span = SourceSpan {
        start_byte: crossing_start,
        end_byte: crossing_start + crossing_text.len(),
        line: over_bound.as_bytes()[..crossing_start]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count() as u32
            + 1,
        column: 12,
    };
    match compile(&project(&over_bound, None)) {
        Err(CompileFailure::Diagnostics(diagnostics)) => {
            let rows: Vec<_> = diagnostics.iter().collect();
            assert_eq!(
                rows.len(),
                1,
                "the first crossing is the complete diagnostic set: {diagnostics:#?}",
            );
            assert_eq!(rows[0].code(), Code::CheckResourceLimit);
            assert_eq!(rows[0].file().as_str(), "src/main.mw");
            assert_eq!(rows[0].span(), offending_span);
        }
        other => panic!(
            "one statement past the envelope must be refused at its source span: \
             {other:?}"
        ),
    }
}

/// The exact operation envelope each corpus drives, recorded as an artifact so a capacity
/// join reads the widths here instead of rediscovering them from the generators.
///
/// Every figure is counted out of the generated source rather than restated from the
/// constants that generate it, so a corpus that stopped emitting what it claims to emit
/// fails here rather than reporting a width it does not drive.
#[test]
fn the_recorded_operation_envelope_is_exact() {
    let structs = type_amplification_arm();
    let arm = enum_amplification_arm();
    // Scoped to the enum declaration: the arm also carries a wrapper struct and a driver
    // function, whose own annotations are not variant payloads.
    let opened = arm
        .find("enum Grown<T> {")
        .expect("the enum arm declares its enum");
    let enums = &arm[opened..arm[opened..].find("\n}\n").expect("the enum closes") + opened];
    let functions = function_amplification_arm();

    let envelope: Vec<(&str, &str, usize)> = vec![
        (
            "type",
            "declared record fields per template",
            structs.matches(": T\n").count() + structs.matches(": Grow<List<T>>\n").count(),
        ),
        (
            "enum",
            "declared variants per template",
            enums.matches("\n    v").count() + enums.matches("\n    next(").count(),
        ),
        (
            "enum",
            "declared payload leaves per template",
            enums.matches(": T,").count()
                + enums.matches(": T)").count()
                + enums.matches(": Grown<Wrap<T>>)").count(),
        ),
        (
            "function",
            "declared local slots per instance",
            functions.matches("    var ").count() + 1,
        ),
        (
            "function",
            "declared statements per instance",
            functions.matches("\n    ").count(),
        ),
    ];

    let expected: Vec<(&str, &str, usize)> = vec![
        (
            "type",
            "declared record fields per template",
            marrow_image::bounds::MAX_RECORD_FIELDS,
        ),
        (
            "enum",
            "declared variants per template",
            marrow_image::bounds::MAX_VARIANTS,
        ),
        (
            "enum",
            "declared payload leaves per template",
            marrow_image::bounds::MAX_VARIANTS * marrow_image::bounds::MAX_PAYLOAD_FIELDS,
        ),
        (
            "function",
            "declared local slots per instance",
            marrow_image::bounds::MAX_LOCALS,
        ),
        (
            "function",
            "declared statements per instance",
            1 + 2 * ADMITTED_LOCALS + ADMITTED_CODE_PADDING + 1,
        ),
    ];

    assert_eq!(
        envelope, expected,
        "the operation envelope the corpora drive moved; each figure is the bound that \
         governs its construct, counted out of the generated source",
    );
    for (corpus, dimension, width) in &envelope {
        println!("operation envelope [{corpus}] {dimension}: {width}");
    }
}
