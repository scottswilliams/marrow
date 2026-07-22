//! Span projection for the sealed image.

use super::decode_code::Decoded;
use super::model::DecodedFunction;
use super::reject;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::SpanRow;

struct SpanProjection {
    rows: Vec<SpanRow>,
    probes: usize,
}

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
    let projection = project_spans(&function.spans, code)?;
    if projection.probes > code.len() {
        return Err(reject(
            VerifyPhase::Function,
            "span offset is not an instruction boundary",
        ));
    }
    Ok(projection.rows)
}

fn project_spans(
    spans: &[(u32, u32, u32)],
    code: &[Decoded],
) -> Result<SpanProjection, VerifyRejection> {
    let mut rows = Vec::with_capacity(spans.len());
    let mut cursor = code.iter().enumerate();
    let mut remaining_probe_budget = code.len();
    let mut probes = 0;
    for (offset, line, column) in spans {
        let instr_index = loop {
            let (instr_index, decoded) = cursor.next().ok_or(reject(
                VerifyPhase::Function,
                "span offset is not an instruction boundary",
            ))?;
            remaining_probe_budget = remaining_probe_budget.checked_sub(1).ok_or(reject(
                VerifyPhase::Function,
                "span offset is not an instruction boundary",
            ))?;
            probes += 1;
            if decoded.offset == *offset {
                break instr_index;
            }
            if decoded.offset > *offset {
                return Err(reject(
                    VerifyPhase::Function,
                    "span offset is not an instruction boundary",
                ));
            }
        };
        rows.push(SpanRow {
            instr_index,
            line: *line,
            column: *column,
        });
    }
    Ok(SpanProjection { rows, probes })
}

#[cfg(test)]
mod tests {
    use crate::sealed::SealedInstr;

    use super::*;

    #[test]
    fn map_spans_full_mapping_probe_count_is_linear() {
        const INSTRUCTION_COUNT: usize = 4_096;

        let code: Vec<_> = (0..INSTRUCTION_COUNT)
            .map(|offset| Decoded {
                instr: SealedInstr::Return,
                offset: offset as u32,
            })
            .collect();
        let spans: Vec<_> = (0..INSTRUCTION_COUNT)
            .map(|offset| (offset as u32, 1, 1))
            .collect();

        let projection = project_spans(&spans, &code).expect("project valid full mapping");
        assert_eq!(projection.probes, INSTRUCTION_COUNT);
        assert_eq!(projection.rows.len(), INSTRUCTION_COUNT);
        assert!(
            projection.probes <= code.len(),
            "{} probes exceed {} decoded instructions",
            projection.probes,
            code.len()
        );
    }
}
