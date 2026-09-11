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
use marrow_local_wire::{DurableState, EncodedFrame, Json, ServerMessage, Span, WireError};
use marrow_verify::VerifiedImage;
use marrow_vm::{
    DurableCommitState, DurableExecutionFault, DurableRun, IncompleteDisposition, Value,
};

use crate::transfer;

/// Where a decoded request runs, from the export's verified demand.
pub(crate) enum Route {
    /// An empty demand: no session, no attachment.
    Storeless,
    /// A durable demand: one session on the attachment.
    Durable,
}

/// A decoded `Request`: the export's identity, its route, and its arguments as owned runtime
/// values. Owns nothing borrowed from the image.
pub(crate) struct DecodedRequest {
    pub(crate) export: ExportId,
    pub(crate) route: Route,
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
    let function = image
        .function(export.function())
        .expect("verified export function");
    if function.body().params().len() != args.len() {
        return Err(reject(Code::RunnerArgMismatch));
    }
    let mut values = Vec::with_capacity(args.len());
    for (ty, json) in function.body().params().iter().zip(args) {
        match transfer::decode_arg(image, ty, json) {
            Some(value) => values.push(value),
            None => return Err(reject(Code::RunnerArgMismatch)),
        }
    }
    let route = if function.demand().is_empty() {
        Route::Storeless
    } else {
        Route::Durable
    };
    Ok(DecodedRequest {
        export: export_id,
        route,
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
    turn: u32,
) -> Result<EncodedFrame, WireError> {
    let Some(export) = image.export_by_id(export) else {
        return reject(Code::RunnerUnknownExport).encode_frame(turn);
    };
    let function = image
        .function(export.function())
        .expect("verified export function");
    match marrow_vm::run(function, values) {
        Ok(value) => value_frame(image, value.as_ref(), turn),
        Err(fault) => fault_message(&fault).encode_frame(turn),
    }
}

/// Encoding failure cannot discard the attachment's retirement decision.
#[must_use = "a retirement projection must close its attached service even if encoding failed"]
pub(crate) enum RunProjection {
    Reply(Result<EncodedFrame, WireError>),
    RetireAfter(Result<EncodedFrame, WireError>),
}

/// Project the attachment's own run of an export. The caller applies retirement
/// before returning the contained encoding result.
pub(crate) fn project_durable_run(
    image: &VerifiedImage,
    run: Option<DurableRun>,
    turn: u32,
) -> RunProjection {
    let Some(run) = run else {
        return RunProjection::Reply(reject(Code::RunnerUnknownExport).encode_frame(turn));
    };
    let response = match run {
        DurableRun::Ran(Ok(value)) => value_frame(image, value.as_ref(), turn),
        DurableRun::Ran(Err(DurableExecutionFault::Runtime(fault))) => {
            fault_message(&fault).encode_frame(turn)
        }
        DurableRun::Ran(Err(DurableExecutionFault::Incomplete(incomplete))) => {
            return match incomplete.into_disposition() {
                IncompleteDisposition::Classified { fault, durable } => {
                    let response = incomplete_message(&fault, durable).encode_frame(turn);
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
                    RunProjection::RetireAfter(
                        incomplete_message(&fault, DurableCommitState::Unknown).encode_frame(turn),
                    )
                }
            };
        }
        DurableRun::Parked => reject(Code::RunnerDurableUnsupported).encode_frame(turn),
        DurableRun::Failed(code) => ServerMessage::Reject {
            code: code.to_string(),
        }
        .encode_frame(turn),
    };
    RunProjection::Reply(response)
}

/// Complete the response while the result and image are still borrowed.
pub(crate) fn value_frame(
    image: &VerifiedImage,
    value: Option<&Value>,
    turn: u32,
) -> Result<EncodedFrame, WireError> {
    EncodedFrame::value(turn, |slot| match value {
        None => slot.null(),
        Some(value) => transfer::encode_value(image, value, slot),
    })
}

/// Encode a source-mapped runtime fault into a `Fault` response.
pub(crate) fn fault_message(fault: &marrow_vm::RuntimeFault) -> ServerMessage {
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
