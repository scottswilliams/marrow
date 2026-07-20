//! Span projection for the sealed image.

use super::decode_code::Decoded;
use super::model::DecodedFunction;
use super::reject;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::SpanRow;

pub(super) fn map_spans(
    function: &DecodedFunction,
    code: &[Decoded],
) -> Result<Vec<SpanRow>, VerifyRejection> {
    if !function.spans.is_empty() {
        if function.spans[0].0 != 0 {
            return Err(reject(
                VerifyPhase::Function,
                "first span must map instruction offset 0",
            ));
        }
    } else if !code.is_empty() {
        return Err(reject(VerifyPhase::Function, "code has no span mappings"));
    }
    let mut rows = Vec::with_capacity(function.spans.len());
    for (offset, line, column) in &function.spans {
        let instr_index = code.iter().position(|d| d.offset == *offset).ok_or(reject(
            VerifyPhase::Function,
            "span offset is not an instruction boundary",
        ))?;
        rows.push(SpanRow {
            instr_index,
            line: *line,
            column: *column,
        });
    }
    Ok(rows)
}
