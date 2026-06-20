use marrow_syntax::SourceSpan;

/// The span of a whole-file diagnostic: the start of the file. A diagnostic that
/// names a file but no declaration within it still points somewhere an editor can
/// place, never the unplaceable `0:0`.
pub(crate) fn start_of_file() -> SourceSpan {
    SourceSpan {
        line: 1,
        column: 1,
        ..SourceSpan::default()
    }
}

pub(crate) fn source_span_at(source: &str, start_byte: usize, end_byte: usize) -> SourceSpan {
    let prefix = &source.as_bytes()[..start_byte.min(source.len())];
    let line_start = prefix
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |index| index + 1);
    SourceSpan {
        start_byte,
        end_byte,
        line: prefix.iter().filter(|&&byte| byte == b'\n').count() as u32 + 1,
        column: start_byte.saturating_sub(line_start) as u32 + 1,
    }
}
