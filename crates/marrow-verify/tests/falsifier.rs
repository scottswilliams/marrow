//! The bounded-representation falsifier: the whole durable contract graph journey, run on
//! a fixed 64 KiB worker stack in an isolated subprocess.
//!
//! Once no unvalidated recursive owner can be constructed, ordinary recursive `Clone`,
//! `Debug`, equality, traversal, and drop are bounded — every walk descends an explicit
//! stack of member runs rather than the machine stack, and every published graph is
//! depth-checked before it exists. That is a claim about frames, and a test that asserts it
//! on the ordinary test stack asserts nothing: a 2 MiB stack absorbs any recursion these
//! corpora could reach. So the journey runs on a stack too small to absorb one.
//!
//! **The protocol.** The parent re-invokes this binary's ignored child helper as a
//! subprocess. The child spawns a worker with a 64 KiB stack, runs the journey on it, joins
//! it, and only then prints the completion sentinel. The parent requires *both* a clean exit
//! status *and* that sentinel: a stack overflow aborts the process (no sentinel, nonzero
//! status), a panic fails the child test (no sentinel), and a journey that silently returned
//! early would print no sentinel while exiting cleanly. Neither signal alone is sufficient,
//! which is why both are checked.
//!
//! **This is the first half of the journey.** It runs from maximum construction through
//! publication, clone/debug/equality, the contract and body codecs, verification, early
//! return, unwind, and drop — terminating at the verified bounded graph. The kernel and
//! store legs (schema projection, current-order numbering, store intake) extend this same
//! harness rather than forking a second one; the sentinel and the parent-side check are
//! written to be extended, not replaced.
//!
//! A failure here is a representation finding, never a licence for a custom `Drop`, a second
//! arena, `ManuallyDrop`, `mem::forget`, or `unsafe`.

use marrow_image::{
    AdmittedGraphInputPlan, DeclarationMemberDef, DeclarationMemberShape, ExportId, FieldDef,
    FunctionDef, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef,
    RootOccurrenceDef, Scalar, SpanEntry,
};
use marrow_verify::verify;

/// The worker stack the journey runs on. Small enough that one recursive descent over a
/// maximum member run, a maximum value shape, or a maximum root set would overflow it, and
/// large enough for the bounded frames the journey actually uses.
const WORKER_STACK_BYTES: usize = 64 * 1024;

/// The completion sentinel. Printed by the child only after the whole journey has returned
/// and every owner it built has been dropped; required by the parent beside a clean status.
const SENTINEL: &str = "marrow.falsifier.imgbound-a.complete";

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PLACEMENT_ID: [u8; 16] = [0x0b; 16];
const ROOT_KEY_ID: [u8; 16] = [0x0c; 16];
const PRODUCT_ID: [u8; 16] = [0x0d; 16];

/// The widest member run this corpus declares.
///
/// The member bound is 8,192, but a run that wide does not fit the whole-image byte ceiling
/// once each member carries its ledger identity and value shape — measured: at 8,192 the
/// encode leg fails — and an image that cannot encode cannot be verified, which would cut
/// the journey short of its last two legs. A quarter of the bound encodes with room, so the
/// journey runs end to end; the encode leg asserts the fit rather than assuming it.
///
/// The depth this stack is really being asked about is not this width, though. A member run
/// is walked by an explicit stack of runs whatever its length, so what a wider run buys is
/// wire bytes, not frames.
const MEMBERS: usize = 2048;

/// Root occurrences over the one Product declaration. Several, so the publication leg
/// exercises the shared-declaration path rather than a single-root special case.
const OCCURRENCES: usize = 4;

fn field_id(ordinal: usize) -> LedgerIdBytes {
    let mut bytes = [0x20u8; 16];
    bytes[0] = ordinal as u8;
    bytes[1] = (ordinal >> 8) as u8;
    LedgerIdBytes::from_bytes(bytes)
}

fn placement_id(ordinal: usize) -> LedgerIdBytes {
    let mut bytes = [0x30u8; 16];
    bytes[0] = ordinal as u8;
    LedgerIdBytes::from_bytes(bytes)
}

fn key_id(ordinal: usize) -> LedgerIdBytes {
    let mut bytes = [0x50u8; 16];
    bytes[0] = ordinal as u8;
    LedgerIdBytes::from_bytes(bytes)
}

/// The construction budget the journey is admitted under: the image's own ceilings, which
/// is what an admission owner with no narrower census of its own states.
fn admitted_plan() -> AdmittedGraphInputPlan {
    AdmittedGraphInputPlan::admit(
        marrow_image::bounds::MAX_ADMITTED_PRODUCT_DECLARATIONS,
        marrow_image::bounds::MAX_ADMITTED_ROOT_OCCURRENCES,
        marrow_image::bounds::MAX_ADMITTED_DECLARATION_COMMANDS,
    )
    .expect("the image's own ceilings are admitted counts")
}

fn add_main(draft: &mut ImageDraft) {
    let src = draft.intern_string("src/main.mw");
    let name = draft.intern_string("main");
    let zero = draft.intern_int(0);
    let code = vec![Instr::ConstLoad(zero.index()), Instr::Return];
    let spans = (0..code.len())
        .map(|index| SpanEntry {
            instr_index: index as u32,
            line: 1,
            column: 1,
        })
        .collect();
    let main = draft
        .add_function(FunctionDef {
            name,
            source: src,
            params: Vec::new(),
            ret: ImageType::scalar(Scalar::Int),
            local_count: 0,
            spans,
            code,
        })
        .expect("every site operand is live");
    draft.add_export(ExportId::of_local("", "main"), main);
}

/// Build the maximum corpus: one Product of [`MEMBERS`] stored fields, its materialized
/// entry record, and [`OCCURRENCES`] roots projecting it.
fn maximum_draft() -> ImageDraft {
    let mut draft = ImageDraft::new();
    let int = draft.value_shapes_mut().scalar(Scalar::Int);

    let entry_name = draft.intern_string("R");
    let entry_fields: Vec<FieldDef> = (0..MEMBERS)
        .map(|ordinal| FieldDef {
            name: draft.intern_string(&format!("f{ordinal}")),
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        })
        .collect();
    let entry = draft.add_record_type(RecordTypeDef {
        name: entry_name,
        fields: entry_fields,
    });

    let members: Vec<DeclarationMemberDef> = (0..MEMBERS)
        .map(|ordinal| DeclarationMemberDef {
            parent: None,
            shape: DeclarationMemberShape::Field {
                id: field_id(ordinal),
                required: true,
                value: int,
            },
        })
        .collect();

    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            entry,
            members,
        )
        .expect("a well-formed declaration");

    for ordinal in 0..OCCURRENCES {
        let name = draft.intern_string(&format!("r{ordinal}"));
        let (placement, key) = if ordinal == 0 {
            (
                LedgerIdBytes::from_bytes(PLACEMENT_ID),
                LedgerIdBytes::from_bytes(ROOT_KEY_ID),
            )
        } else {
            (placement_id(ordinal), key_id(ordinal))
        };
        draft
            .add_root_occurrence(
                &admitted_plan(),
                LedgerIdBytes::from_bytes(PRODUCT_ID),
                RootOccurrenceDef {
                    name,
                    keys: vec![KeyColumn {
                        scalar: Scalar::Int,
                        id: key,
                    }],
                    placement,
                    indexes: Vec::new(),
                },
            )
            .expect("the Product is declared");
    }

    add_main(&mut draft);
    draft
}

/// The whole first half of the journey, in order. Every leg runs on the worker stack; the
/// function returns only when all of them have.
fn journey() {
    // 1. Maximum construction, and 2. publication of several roots over one declaration.
    let draft = maximum_draft();

    // 3. Traversal, equality, and Debug over the published graph. The node walk is the
    //    deepest traversal the representation has, and it is the one that used to recurse.
    let view = draft.contract_view();
    let nodes = view.semantic_nodes();
    assert_eq!(
        nodes.len(),
        OCCURRENCES * (1 + MEMBERS),
        "every root and every member of every root is a node"
    );
    let roots: Vec<_> = view.roots().collect();
    assert_eq!(roots.len(), OCCURRENCES);
    let shared = roots[0].members();
    assert_eq!(shared.len(), MEMBERS);
    // One member graph, shared: every occurrence names the same declaration, and walking
    // two of them yields the same member sequence rather than two copies of it.
    assert_eq!(
        roots[0].product(),
        roots[1].product(),
        "the occurrences share one Product declaration"
    );
    let walked: Vec<_> = roots[1].members().map(|member| member.kind()).collect();
    assert_eq!(walked.len(), MEMBERS, "the shared run walks in full");
    // Clone and equality over the arena the members' shapes live in: a representation that
    // owned an expanded shape per member would copy it here.
    let cloned = view.value_shapes().clone();
    assert_eq!(&cloned, view.value_shapes(), "cloning preserves the arena");
    let rendered = format!("{cloned:?}");
    assert!(!rendered.is_empty(), "Debug renders the arena");

    // 4. The contract codec: the identity streams its preimage without materializing it.
    let contract = view.contract_id().expect("the corpus is inside the bound");

    // 5. The body codec, and the whole-image encode it frames.
    let encoded = draft.encode().expect("the corpus fits every bound");
    assert!(
        encoded.bytes.len() <= marrow_image::bounds::MAX_IMAGE_BYTES,
        "the corpus must encode to be verifiable"
    );

    // 6. Verification: the independent reconstruction of the same graph from the bytes,
    //    which recomputes the same identity or refuses.
    let sealed = verify(&encoded.bytes).expect("the encoded corpus verifies");
    assert_eq!(
        sealed.semantic_nodes().len(),
        nodes.len(),
        "the verifier enumerates the same node set the compiler did"
    );
    drop(sealed);
    drop(encoded);

    // 7. Early return: an intake wider than its plan is refused before a row is appended,
    //    and the rejected input is dropped on the way out.
    let narrow = AdmittedGraphInputPlan::admit(1, 1, 1).expect("a one-command budget");
    let mut refused = ImageDraft::new();
    let name = refused.intern_string("R");
    let record = refused.add_record_type(RecordTypeDef {
        name,
        fields: Vec::new(),
    });
    let value = refused.value_shapes_mut().scalar(Scalar::Int);
    let two: Vec<DeclarationMemberDef> = (0..2)
        .map(|ordinal| DeclarationMemberDef {
            parent: None,
            shape: DeclarationMemberShape::Field {
                id: field_id(ordinal),
                required: true,
                value,
            },
        })
        .collect();
    refused
        .declare_product(&narrow, LedgerIdBytes::from_bytes(PRODUCT_ID), record, two)
        .expect_err("a command vector wider than its budget is refused");
    drop(refused);

    // 8. Unwind: a panic while the maximum graph is live drops it during unwinding, which
    //    is the leg a recursive drop glue would overflow on rather than return from.
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let held = maximum_draft();
        let _ = held.contract_view().semantic_nodes();
        panic!("{contract:?} unwinds with a maximum graph live");
    }));
    assert!(unwound.is_err(), "the unwind leg must actually unwind");

    // 9. Drop: the graph the whole journey was built on goes out last, ordinarily.
    drop(draft);
}

/// The child half of the protocol. Runs the journey on a 64 KiB worker stack, joins it, and
/// prints the sentinel only if it returned.
///
/// Ignored so an ordinary test run does not execute it directly: it is meaningful only as
/// the subprocess the parent below invokes and measures.
#[test]
#[ignore = "re-invoked as a subprocess by the_bounded_graph_journey_completes_on_a_64_kib_stack"]
fn falsifier_child() {
    // The default panic hook prints the unwind leg's deliberate panic, which would read as
    // a failure in the child's output. The leg asserts the unwind happened; the message is
    // noise, so it is suppressed for the duration and restored after.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    let worker = std::thread::Builder::new()
        .stack_size(WORKER_STACK_BYTES)
        .spawn(journey)
        .expect("spawn the falsifier worker");
    let outcome = worker.join();

    std::panic::set_hook(previous);
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
    println!("{SENTINEL}");
}

/// The parent half of the protocol: run the child and require a clean status **and** the
/// sentinel.
#[test]
fn the_bounded_graph_journey_completes_on_a_64_kib_stack() {
    let exe = std::env::current_exe().expect("the test binary's own path");
    let output = std::process::Command::new(exe)
        .args(["--exact", "falsifier_child", "--ignored", "--nocapture"])
        .output()
        .expect("run the falsifier child");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the falsifier child did not exit cleanly ({}): a stack overflow aborts rather than \
         returning, so this is a representation finding\n{stdout}\n{stderr}",
        output.status,
    );
    assert!(
        stdout.contains(SENTINEL),
        "the falsifier child exited cleanly without reaching its completion sentinel, so \
         some leg of the journey returned early\n{stdout}\n{stderr}",
    );
}

/// A stack too small for even the bounded journey, for the plant probe below. Chosen far
/// under [`WORKER_STACK_BYTES`] so the probe is about the parent's detection, not about
/// where the true frame requirement sits.
const STARVED_STACK_BYTES: usize = 1024;

/// The plant probe's child: the same journey on a stack that cannot hold it.
#[test]
#[ignore = "re-invoked as a subprocess by the_parent_check_detects_a_child_that_cannot_finish"]
fn falsifier_starved_child() {
    std::panic::set_hook(Box::new(|_| {}));
    let worker = std::thread::Builder::new()
        .stack_size(STARVED_STACK_BYTES)
        .spawn(journey)
        .expect("spawn the starved worker");
    let _ = worker.join();
    println!("{SENTINEL}");
}

/// **The plant probe.** A child whose journey cannot complete must not satisfy the parent's
/// check, so the check is exercised against a planted failure rather than only against the
/// passing case.
///
/// The starved child is deliberately written the *worse* way — it prints the sentinel
/// unconditionally after joining, ignoring the worker's outcome — so the probe proves the
/// exit-status half of the check is load-bearing on its own. An overflow aborts the process
/// before that line is reached, or the join returns an error the status still reflects.
#[test]
fn the_parent_check_detects_a_child_that_cannot_finish() {
    let exe = std::env::current_exe().expect("the test binary's own path");
    let output = std::process::Command::new(exe)
        .args([
            "--exact",
            "falsifier_starved_child",
            "--ignored",
            "--nocapture",
        ])
        .output()
        .expect("run the starved falsifier child");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !(output.status.success() && stdout.contains(SENTINEL)),
        "a child that cannot complete the journey satisfied the parent-side check, so the \
         check would pass on a representation that overflowed\n{stdout}",
    );
}

/// The protocol's own control: the sentinel is not printed by a child that did not finish.
///
/// A parent that only checked the exit status would pass on a child whose journey stopped
/// after its first leg, and a sentinel the harness printed unconditionally would pass on any
/// child at all. This asserts the two halves are actually coupled — the sentinel is written
/// once, after the join, and nowhere else in this file.
#[test]
fn the_sentinel_is_printed_once_and_only_after_the_journey() {
    let source = include_str!("falsifier.rs");
    let child = source
        .split_once("fn falsifier_child()")
        .expect("the child helper")
        .1
        .split_once("\n}\n")
        .expect("the child helper's body ends")
        .0;

    let prints = child.matches("println!").count();
    assert_eq!(
        prints, 1,
        "the measured child must write the sentinel exactly once, found {prints}"
    );
    let join = child.find(".join()").expect("the child joins its worker");
    let print = child
        .find("println!")
        .expect("the child prints the sentinel");
    let resume = child
        .find("resume_unwind")
        .expect("the child re-raises a worker panic");
    assert!(
        join < resume && resume < print,
        "the sentinel must be written after the worker is joined and after a worker panic \
         has been re-raised, not before either"
    );
}
