//! The durable-value encoder's one-MiB whole-value bound is reachable from ordinary
//! checked source. Each scalar leaf remains below its separate 64-KiB ceiling; only
//! the aggregate record crosses the durable-cell bound. The kernel plans the complete
//! encoded value before applying a write, so a rejected value leaves no entry behind.

use marrow_codes::Code;
use marrow_verify::VerifiedImage;
use marrow_vm::{
    DurableRun, MemoryAttachment, MintOutcome, RuntimeFault, Value, mint_ephemeral, prepare,
    run_export,
};

use crate::common::Project;

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Box 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Box.payload 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root boxes 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key boxes.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

const SOURCE: &str = r#"struct Payload {
    f00: string
    f01: string
    f02: string
    f03: string
    f04: string
    f05: string
    f06: string
    f07: string
    f08: string
    f09: string
    f10: string
    f11: string
    f12: string
    f13: string
    f14: string
    f15: string
    f16: string
}

resource Box {
    required payload: Payload
}

store ^boxes[id: int]: Box

pub fn write(id: int, leaf: string) {
    transaction {
        ^boxes[id] = Box(payload: Payload(
            f00: leaf, f01: leaf, f02: leaf, f03: leaf, f04: leaf,
            f05: leaf, f06: leaf, f07: leaf, f08: leaf, f09: leaf,
            f10: leaf, f11: leaf, f12: leaf, f13: leaf, f14: leaf,
            f15: leaf, f16: leaf
        ))
    }
}

pub fn present(id: int): bool {
    return exists(^boxes[id])
}
"#;

/// Resolve `name` to its sealed export id.
fn export_id(image: &VerifiedImage, name: &str) -> marrow_verify::ExportId {
    image
        .exports()
        .iter()
        .find(|export| {
            image
                .function(export.function())
                .expect("verified function")
                .body()
                .name()
                == name
        })
        .expect("export present")
        .id()
}

fn attach(image: &VerifiedImage) -> MemoryAttachment {
    match mint_ephemeral(prepare(image.clone())).into_mint() {
        MintOutcome::Ready(attachment) => attachment,
        MintOutcome::Storeless | MintOutcome::Parked => {
            panic!("the durable-value fixture must be executable")
        }
        MintOutcome::Failed(cause) => panic!("attachment mint failed: {}", cause.as_str()),
    }
}

fn run(
    image: &VerifiedImage,
    attachment: &mut MemoryAttachment,
    name: &str,
    args: Vec<Value>,
) -> Result<Option<Value>, RuntimeFault> {
    match run_export(attachment, export_id(image, name), args).expect("the export is in the image")
    {
        DurableRun::Ran(Ok(value)) => Ok(value),
        DurableRun::Ran(Err(marrow_vm::DurableExecutionFault::Runtime(fault))) => Err(fault),
        DurableRun::Ran(Err(marrow_vm::DurableExecutionFault::Incomplete(incomplete))) => {
            match incomplete.into_disposition() {
                marrow_vm::IncompleteDisposition::Classified { durable, .. } => {
                    panic!("{name} was incomplete ({durable:?})")
                }
                marrow_vm::IncompleteDisposition::Pending { recovery, .. } => {
                    drop(recovery);
                    panic!("{name} reached pending commit recovery")
                }
            }
        }
        DurableRun::Parked => panic!("{name} parked"),
        DurableRun::Failed(code) => panic!("{name} failed before execution: {}", code.as_str()),
    }
}

fn text_of_len(len: usize) -> Value {
    Value::Text("x".repeat(len).into())
}

fn write_line() -> u32 {
    let byte = SOURCE
        .find("^boxes[id] = Box")
        .expect("write expression is present");
    SOURCE[..byte].lines().count() as u32
}

#[test]
fn aggregate_durable_value_bound_is_source_reachable_and_write_atomic() {
    let image = Project::single(SOURCE).ids(IDS).image();
    let mut attachment = attach(&image);

    // Seventeen 61,680-byte scalar leaves are individually below 64 KiB, but their
    // length-framed aggregate exceeds 1 MiB. The ordinary source write reaches the
    // canonical value.range mapping at its assignment span.
    let fault = run(
        &image,
        &mut attachment,
        "write",
        vec![Value::Int(1), text_of_len(61_680)],
    )
    .expect_err("the over-cap aggregate must fault");
    assert_eq!(fault.code(), Code::ValueRange);
    assert_eq!(fault.line(), write_line());
    assert_eq!(fault.column(), 9);

    // Encoding completes before the kernel applies any cell write. The rejected
    // whole-entry write therefore leaves no marker or field cell behind.
    assert_eq!(
        run(&image, &mut attachment, "present", vec![Value::Int(1)]).expect("presence probe runs"),
        Some(Value::Bool(false))
    );

    // The adjacent below-cap value uses the same source path and all seventeen
    // leaves, proving the boundary rather than a generally unexecutable fixture.
    run(
        &image,
        &mut attachment,
        "write",
        vec![Value::Int(2), text_of_len(61_670)],
    )
    .expect("the below-cap aggregate commits");
    assert_eq!(
        run(&image, &mut attachment, "present", vec![Value::Int(2)]).expect("presence probe runs"),
        Some(Value::Bool(true))
    );
}
