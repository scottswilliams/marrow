//! The one export classifier and the wire projections every handler shares.
//!
//! The storeless [`Service`](crate::Service) and the native attached session
//! ([`crate::AttachedService`]) decode a `Request` identically — an unknown export and an
//! argument-shape mismatch are the same typed rejects, and the export's verified demand
//! decides its [`Route`] once here. A storeless export runs without a session through
//! [`run_storeless`]; only the attached session runs a durable one.
//!
//! Decoding borrows the image immutably and finishes to owned values plus the copied
//! export identity, so the borrow ends before an attachment is driven mutably.

use marrow_codes::{Code, DurableCommitState};
use marrow_image::ExportId;
use marrow_local_wire::{EncodedFrame, Json, ServerMessage, Span, WireError};
use marrow_verify::{VerifiedFunction, VerifiedImage};
use marrow_vm::Value;

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

/// A `Request`'s export resolved in the image: its identity, its verified function, and the
/// route its demand decides — known before any argument is looked at, so a handler that
/// serves only one route refuses the other regardless of the arguments' shape.
pub(crate) struct ResolvedExport<'i> {
    pub(crate) export: ExportId,
    pub(crate) function: VerifiedFunction<'i>,
    pub(crate) route: Route,
}

/// Resolve a `Request`'s export id; an unknown export is a typed reject ready to send.
pub(crate) fn resolve_export<'i>(
    image: &'i VerifiedImage,
    export_id: &[u8; 32],
) -> Result<ResolvedExport<'i>, ServerMessage> {
    let export_id = ExportId::from_bytes(*export_id);
    let Some(export) = image.export_by_id(export_id) else {
        return Err(reject(Code::RunnerUnknownExport));
    };
    let function = image
        .function(export.function())
        .expect("verified export function");
    let route = if function.demand().is_empty() {
        Route::Storeless
    } else {
        Route::Durable
    };
    Ok(ResolvedExport {
        export: export_id,
        function,
        route,
    })
}

/// Decode `args` against the export's verified signature into owned runtime values; an
/// argument count or shape mismatch is a typed reject, never a partial reply.
pub(crate) fn decode_args(
    image: &VerifiedImage,
    function: VerifiedFunction<'_>,
    args: &[Json],
) -> Result<Vec<Value>, ServerMessage> {
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
    Ok(values)
}

/// Resolve a `Request`'s export and decode its args, for a handler that serves both routes.
pub(crate) fn decode_request(
    image: &VerifiedImage,
    export_id: &[u8; 32],
    args: &[Json],
) -> Result<DecodedRequest, ServerMessage> {
    let resolved = resolve_export(image, export_id)?;
    let values = decode_args(image, resolved.function, args)?;
    Ok(DecodedRequest {
        export: resolved.export,
        route: resolved.route,
        values,
    })
}

/// Run a storeless export (empty demand) with no session and project its outcome onto the wire.
/// A storeless export needs no attachment, so both services run it the same way, and an
/// attached service whose store refused still serves it. The export was resolved by
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
        code: fault.code(),
        span: Span {
            line: fault.line(),
            column: fault.column(),
        },
    }
}

/// Encode an incomplete invocation: its source-mapped fault beside the classified durable
/// state, two orthogonal facts.
pub(crate) fn incomplete_message(
    fault: &marrow_vm::RuntimeFault,
    durable: DurableCommitState,
) -> ServerMessage {
    ServerMessage::Incomplete {
        code: fault.code(),
        durable,
        span: Span {
            line: fault.line(),
            column: fault.column(),
        },
    }
}

/// A typed reject naming the runner's reason, carrying no wire or lifecycle vocabulary.
pub(crate) fn reject(code: Code) -> ServerMessage {
    ServerMessage::Reject { code }
}
