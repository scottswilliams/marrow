//! Serving one request against a launched image, shared by the two attached sessions.
//!
//! The native attached session ([`crate::AttachedService`], over a persistent store) and the
//! ephemeral-memory attached session ([`crate::AttachedEphemeralService`], over a process-local
//! in-memory store) decode, run, and encode a `Request` identically — an unknown export and an
//! argument-shape mismatch are the same typed rejects, a storeless export runs without a
//! session, and a durable run projects onto the wire the same way. Only the [`SessionHost`] the
//! durable export runs against differs, so that one classifier lives here rather than being
//! duplicated per attachment kind.
//!
//! [`SessionHost`]: marrow_kernel::durable::SessionHost

use marrow_codes::Code;
use marrow_image::ExportId;
use marrow_local_wire::{Json, ServerMessage, Span};
use marrow_verify::{SealedExport, VerifiedImage};
use marrow_vm::{DurableRun, Value};

use crate::transfer;

/// Resolve a `Request`'s export id and decode its args against the export's verified signature.
///
/// On success the sealed export and the decoded runtime values are returned for the caller to
/// run against its host; on failure a typed reject is returned ready to send — an unknown
/// export or an argument count/shape mismatch, never a partial reply.
pub(crate) fn decode_request<'a>(
    image: &'a VerifiedImage,
    export_id: &[u8; 32],
    args: &[Json],
) -> Result<(&'a SealedExport, Vec<Value>), ServerMessage> {
    let Some(export) = image.export_by_id(ExportId::from_bytes(*export_id)) else {
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
    Ok((export, values))
}

/// Run a storeless export (empty demand) with no session and project its outcome onto the wire.
/// A storeless export needs no attachment, so both session kinds run it the same way.
pub(crate) fn run_storeless(
    image: &VerifiedImage,
    export: &SealedExport,
    values: Vec<Value>,
) -> ServerMessage {
    match marrow_vm::run(image, export.function(), values) {
        Ok(value) => value_message(image, value.as_ref()),
        Err(fault) => fault_message(&fault),
    }
}

/// Project a durable run outcome onto a wire response. A verified durable export whose shape the
/// attachment cannot serve, or a session that could not open, is a typed reject — never a
/// partial reply.
pub(crate) fn project_durable_run(image: &VerifiedImage, run: DurableRun) -> ServerMessage {
    match run {
        DurableRun::Ran(Ok(value)) => value_message(image, value.as_ref()),
        DurableRun::Ran(Err(fault)) => fault_message(&fault),
        DurableRun::Parked => reject(Code::RunnerDurableUnsupported),
        DurableRun::Failed(code) => ServerMessage::Reject {
            code: code.to_string(),
        },
    }
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

/// A typed reject naming the runner's reason, carrying no wire or lifecycle vocabulary.
pub(crate) fn reject(code: Code) -> ServerMessage {
    ServerMessage::Reject {
        code: code.as_str().to_string(),
    }
}
