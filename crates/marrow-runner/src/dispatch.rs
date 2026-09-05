//! Serving one request against a launched image, shared by the two attached sessions.
//!
//! The native attached session ([`crate::AttachedService`], over a persistent store) and the
//! ephemeral-memory attached session ([`crate::AttachedEphemeralService`], over a process-local
//! in-memory store) decode, run, and encode a `Request` identically — an unknown export and an
//! argument-shape mismatch are the same typed rejects, a storeless export runs without a
//! session, and a durable run projects onto the wire the same way. Only the attachment the
//! durable export runs through differs, so that one classifier lives here rather than being
//! duplicated per attachment kind.
//!
//! Decoding borrows the attachment's image immutably and finishes to owned values plus the
//! copied export identity, so the borrow ends before the attachment is driven mutably.

use marrow_codes::Code;
use marrow_image::ExportId;
use marrow_local_wire::{DurableState, Json, ServerMessage, Span};
use marrow_verify::VerifiedImage;
use marrow_vm::{
    DurableCommitState, DurableExecutionFault, DurableRun, IncompleteDisposition, Value,
};

use crate::transfer;

/// A decoded `Request`: the export's identity, whether its verified demand is durable, and
/// its arguments as owned runtime values. Owns nothing borrowed from the image.
pub(crate) struct DecodedRequest {
    pub(crate) export: ExportId,
    pub(crate) durable: bool,
    pub(crate) values: Vec<Value>,
}

/// Resolve a `Request`'s export id and decode its args against the export's verified signature.
///
/// On success the decoded request is returned for the caller to run against its attachment;
/// on failure a typed reject is returned ready to send — an unknown export or an argument
/// count/shape mismatch, never a partial reply.
pub(crate) fn decode_request(
    image: &VerifiedImage,
    export_id: &[u8; 32],
    args: &[Json],
) -> Result<DecodedRequest, ServerMessage> {
    let export_id = ExportId::from_bytes(*export_id);
    let Some(export) = image.export_by_id(export_id) else {
        return Err(reject(Code::RunnerUnknownExport));
    };
    let function = image.function(export.function());
    if function.params().len() != args.len() {
        return Err(reject(Code::RunnerArgMismatch));
    }
    let mut values = Vec::with_capacity(args.len());
    for (ty, json) in function.params().iter().zip(args) {
        match transfer::decode_arg(image, ty, json) {
            Some(value) => values.push(value),
            None => return Err(reject(Code::RunnerArgMismatch)),
        }
    }
    Ok(DecodedRequest {
        export: export_id,
        durable: !export.demand().is_empty(),
        values,
    })
}

/// Run a storeless export (empty demand) with no session and project its outcome onto the wire.
/// A storeless export needs no attachment, so both session kinds run it the same way, and a
/// service whose attachment parked or failed still serves it. The export was resolved by
/// [`decode_request`] from this same image.
pub(crate) fn run_storeless(
    image: &VerifiedImage,
    export: ExportId,
    values: Vec<Value>,
) -> ServerMessage {
    let Some(export) = image.export_by_id(export) else {
        return reject(Code::RunnerUnknownExport);
    };
    match marrow_vm::run(image, export.function(), values) {
        Ok(value) => value_message(image, value.as_ref()),
        Err(fault) => fault_message(&fault),
    }
}

/// Project a durable run outcome onto a wire response. A verified durable export whose shape the
/// attachment cannot serve, or a session that could not open, is a typed reject — never a
/// partial reply.
#[must_use = "a retirement projection must close its attached service after replying"]
pub(crate) enum RunProjection {
    Reply(ServerMessage),
    RetireAfter(ServerMessage),
}

/// Project the attachment's own run of an export. `None` — the attachment's image carries no
/// such export — cannot follow a successful decode against that same image, and is the same
/// typed reject.
pub(crate) fn project_durable_run(image: &VerifiedImage, run: Option<DurableRun>) -> RunProjection {
    let Some(run) = run else {
        return RunProjection::Reply(reject(Code::RunnerUnknownExport));
    };
    let response = match run {
        DurableRun::Ran(Ok(value)) => value_message(image, value.as_ref()),
        DurableRun::Ran(Err(DurableExecutionFault::Runtime(fault))) => fault_message(&fault),
        DurableRun::Ran(Err(DurableExecutionFault::Incomplete(incomplete))) => {
            return match incomplete.into_disposition() {
                IncompleteDisposition::Classified { fault, durable } => {
                    let response = incomplete_message(&fault, durable);
                    if durable == DurableCommitState::Unknown {
                        RunProjection::RetireAfter(response)
                    } else {
                        RunProjection::Reply(response)
                    }
                }
                IncompleteDisposition::Pending { fault, recovery } => {
                    // Only the memory-backed attachment reaches this generic projector.
                    // Its engine never returns an indeterminate commit; if that invariant
                    // changes, consuming the fact is paired with an explicit retirement
                    // projection rather than dropping it into an ordinary fault.
                    drop(recovery);
                    RunProjection::RetireAfter(incomplete_message(
                        &fault,
                        DurableCommitState::Unknown,
                    ))
                }
            };
        }
        DurableRun::Parked => reject(Code::RunnerDurableUnsupported),
        DurableRun::Failed(code) => ServerMessage::Reject {
            code: code.to_string(),
        },
    };
    RunProjection::Reply(response)
}

/// Encode a returned value into a `Value` response, downgrading an unencodable value (never
/// reached for a served export, whose return shape is transferable) to a typed reject rather
/// than a partial reply.
fn value_message(image: &VerifiedImage, value: Option<&Value>) -> ServerMessage {
    match value {
        None => ServerMessage::Value { data: Json::Null },
        Some(value) => match transfer::encode_value(image, value) {
            Some(data) => ServerMessage::Value { data },
            None => reject(Code::RunnerReplyEncode),
        },
    }
}

/// Encode a source-mapped runtime fault into a `Fault` response.
fn fault_message(fault: &marrow_vm::RuntimeFault) -> ServerMessage {
    ServerMessage::Fault {
        code: fault.code().to_string(),
        span: Span {
            line: fault.line(),
            column: fault.column(),
        },
    }
}

pub(crate) fn incomplete_message(
    fault: &marrow_vm::RuntimeFault,
    durable: DurableCommitState,
) -> ServerMessage {
    let durable = match durable {
        DurableCommitState::KnownOld => DurableState::KnownOld,
        DurableCommitState::KnownNew => DurableState::KnownNew,
        DurableCommitState::Unknown => DurableState::Unknown,
    };
    ServerMessage::Incomplete {
        code: fault.code().to_string(),
        durable,
        span: Span {
            line: fault.line(),
            column: fault.column(),
        },
    }
}

/// A typed reject naming the runner's reason, carrying no wire or lifecycle vocabulary.
pub(crate) fn reject(code: Code) -> ServerMessage {
    ServerMessage::Reject {
        code: code.as_str().to_string(),
    }
}
