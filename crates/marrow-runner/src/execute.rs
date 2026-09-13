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
    /// A parked durable shape, a mismatched
    /// approval, or a taken destination each surface as a typed reject.
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
        Err(error) => match error.uncertain_instance() {
            Some(instance) => ServerMessage::ProvisionUncertain {
                instance: instance.to_hex(),
            },
            None => ServerMessage::Reject {
                code: error.code().to_string(),
            },
        },
    }
}

fn reject(code: Code) -> ServerMessage {
    ServerMessage::Reject {
        code: code.as_str().to_string(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn publication_uncertainty_preserves_instance_in_actual_reply_projection() {
        let instance = marrow_lifecycle::StoreInstanceId::from_bytes([0x12; 16]);
        let error = marrow_lifecycle::ProvisionImageError::Provision(
            marrow_lifecycle::ProvisionError::PublicationUncertain {
                instance,
                source: std::io::Error::from(std::io::ErrorKind::Other),
            },
        );
        let reply = super::provision_reply(Err(error));
        assert_eq!(
            reply,
            marrow_local_wire::ServerMessage::ProvisionUncertain {
                instance: instance.to_hex()
            }
        );
        assert!(matches!(
            super::provision_reply(Err(marrow_lifecycle::ProvisionImageError::Unapproved)),
            marrow_local_wire::ServerMessage::Reject { .. }
        ));
    }
}
