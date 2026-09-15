//! Storeless request dispatch and one-shot provisioning over a launched image.
//!
//! Requests are admitted against the verified export signature, then run on the VM.
//! Successful values are encoded while borrowed; a completed frame or an encoding
//! error leaves this owner. Runtime faults and admission rejects retain their typed
//! response grammar.

use marrow_codes::Code;
use marrow_local_wire::{ClientMessage, EncodedFrame, Json, ServerMessage, WireError};

use crate::descriptor::Service;
use crate::dispatch;
use crate::transfer;

impl crate::channel::Handler for Service {
    fn handle(
        &mut self,
        message: ClientMessage,
        turn: Option<u32>,
    ) -> Result<EncodedFrame, WireError> {
        Service::handle(self, message, turn)
    }
}

impl Service {
    /// Produce a complete response while borrowed results are live. A `Hello`
    /// after the handshake is a protocol error, not a second handshake.
    pub fn handle(
        &self,
        message: ClientMessage,
        turn: Option<u32>,
    ) -> Result<EncodedFrame, WireError> {
        let turn = turn.unwrap_or(0);
        match message {
            ClientMessage::Hello { .. } => reject(Code::RunnerHandshake).encode_frame(turn),
            ClientMessage::Request { export, args } => {
                self.handle_request(export.bytes(), &args, turn)
            }
            ClientMessage::Provision { store, approval } => {
                self.handle_provision(&store, &approval).encode_frame(turn)
            }
        }
    }

    /// Provision a fresh persistent store for the launched image at `store`, gated by the
    /// accepted-report `approval` token. Borrows the prepared image, rebuilds the report the
    /// approval must match (so an approval for a different store or image is refused), and
    /// publishes a complete store, reporting uncertainty if the final directory sync fails.
    /// Ordinary refusal is a typed reject. If removal of an owned unpublished stage
    /// also fails, the reply retains both the primary code and cleanup evidence.
    fn handle_provision(&self, store: &str, approval: &str) -> ServerMessage {
        let approval = marrow_lifecycle::ProvisionApproval::from_token(approval);
        provision_reply(marrow_lifecycle::provision_image(
            std::path::Path::new(store),
            &self.image,
            &approval,
        ))
    }

    fn handle_request(
        &self,
        export: &[u8; 32],
        args: &[Json],
        turn: u32,
    ) -> Result<EncodedFrame, WireError> {
        let Some(served) = self.lookup(export) else {
            return reject(Code::RunnerUnknownExport).encode_frame(turn);
        };
        if served.is_durable() {
            return reject(Code::RunnerDurableUnsupported).encode_frame(turn);
        }
        let image = self.image.image();
        let selected = image
            .function(served.func())
            .expect("served function belongs to this image");
        let function = selected.body();
        if function.params().len() != args.len() {
            return reject(Code::RunnerArgMismatch).encode_frame(turn);
        }
        let mut values = Vec::with_capacity(args.len());
        for (ty, json) in function.params().iter().zip(args) {
            match transfer::decode_arg(image, ty, json) {
                Some(value) => values.push(value),
                None => return reject(Code::RunnerArgMismatch).encode_frame(turn),
            }
        }
        match marrow_vm::run(selected, values) {
            Ok(value) => dispatch::value_frame(image, value.as_ref(), turn),
            Err(fault) => dispatch::fault_message(&fault).encode_frame(turn),
        }
    }
}

fn provision_reply(
    result: Result<marrow_lifecycle::Provisioned, marrow_lifecycle::ProvisionImageError>,
) -> ServerMessage {
    match result {
        Ok(provisioned) => ServerMessage::Provisioned {
            instance: provisioned.instance.to_hex(),
        },
        Err(error) => match error.uncertainty() {
            Some((reason, instance)) => ServerMessage::ProvisionUncertain {
                reason,
                instance: instance.to_hex(),
            },
            None => match error.cleanup() {
                Some(cleanup) => ServerMessage::ProvisionFailed {
                    code: error.code(),
                    stage: cleanup
                        .stage
                        .file_name()
                        .and_then(|name| name.to_str())
                        .expect("provision creates a generated ASCII stage component")
                        .to_string(),
                    os_error: cleanup.source.raw_os_error(),
                },
                None => ServerMessage::Reject { code: error.code() },
            },
        },
    }
}

fn reject(code: Code) -> ServerMessage {
    ServerMessage::Reject { code }
}

#[cfg(test)]
mod tests {
    #[test]
    fn provision_uncertainty_preserves_reason_and_instance_in_actual_reply_projection() {
        let instance = marrow_lifecycle::StoreInstanceId::from_bytes([0x12; 16]);
        for (reason, error) in [
            (
                marrow_codes::StoreUncertainty::Publication,
                marrow_lifecycle::ProvisionFault::PublicationUncertain {
                    instance,
                    source: std::io::Error::from(std::io::ErrorKind::Other),
                },
            ),
            (
                marrow_codes::StoreUncertainty::Activation,
                marrow_lifecycle::ProvisionFault::ActivationUncertain {
                    instance,
                    source: marrow_lifecycle::AdmissionError {
                        entry: marrow_lifecycle::StoreEntry::Envelope,
                        fault: marrow_lifecycle::AdmissionFault::MultiplyLinked { links: 2 },
                    },
                },
            ),
        ] {
            let reply = super::provision_reply(Err(
                marrow_lifecycle::ProvisionImageError::Provision(error.into()),
            ));
            let expected = marrow_local_wire::ServerMessage::ProvisionUncertain {
                reason,
                instance: instance.to_hex(),
            };
            assert_eq!(reply, expected);
            let encoded = reply.encode_frame(0).expect("encode actual reply");
            assert_eq!(
                marrow_local_wire::ServerMessage::decode(&encoded.as_bytes()[4..]),
                Ok(expected)
            );
        }
        let cleanup =
            marrow_lifecycle::ProvisionImageError::Provision(marrow_lifecycle::ProvisionError {
                fault: marrow_lifecycle::ProvisionFault::AlreadyProvisioned,
                cleanup: Some(marrow_lifecycle::ProvisionCleanupFailure {
                    stage: std::path::PathBuf::from("parent/.marrow-provisioning.123.0"),
                    source: std::io::Error::from_raw_os_error(13),
                }),
            });
        let reply = super::provision_reply(Err(cleanup));
        assert_eq!(
            reply,
            marrow_local_wire::ServerMessage::ProvisionFailed {
                code: marrow_codes::Code::StoreLocked,
                stage: ".marrow-provisioning.123.0".into(),
                os_error: Some(13),
            }
        );
        let frame = reply.encode_with_turn(99).unwrap();
        assert_eq!(
            marrow_local_wire::ServerMessage::decode_with_turn(&frame[4..]),
            Ok((reply, None))
        );
        assert!(matches!(
            super::provision_reply(Err(marrow_lifecycle::ProvisionImageError::Unapproved)),
            marrow_local_wire::ServerMessage::Reject { .. }
        ));
    }
}
