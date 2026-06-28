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

pub(crate) fn identifier_span_in(source: &str, span: SourceSpan, name: &str) -> Option<SourceSpan> {
    let (start, end) = find_identifier_in_span(source, span, name, true)?;
    Some(source_span_at(source, start, end))
}

pub(crate) fn last_identifier_span_in(
    source: &str,
    span: SourceSpan,
    name: &str,
) -> Option<SourceSpan> {
    let (start, end) = find_identifier_in_span(source, span, name, false)?;
    Some(source_span_at(source, start, end))
}

fn find_identifier_in_span(
    source: &str,
    span: SourceSpan,
    name: &str,
    first: bool,
) -> Option<(usize, usize)> {
    let end_byte = if first {
        span.end_byte.saturating_add(1)
    } else {
        span.end_byte
    }
    .min(source.len());
    if span.start_byte > end_byte {
        return None;
    }
    let text = source.get(span.start_byte..end_byte)?;
    let mut cursor = 0;
    let mut found = None;
    while let Some(rest) = text.get(cursor..) {
        let Some(offset) = rest.find(name) else {
            break;
        };
        let start = span.start_byte + cursor + offset;
        let end = start + name.len();
        if identifier_boundary(source.as_bytes(), start, end) {
            if first {
                return Some((start, end));
            }
            found = Some((start, end));
        }
        cursor += offset + name.len();
    }
    found
}

fn identifier_boundary(bytes: &[u8], start: usize, end: usize) -> bool {
    let before = start
        .checked_sub(1)
        .and_then(|index| bytes.get(index))
        .is_some_and(|byte| is_identifier_byte(*byte));
    let after = bytes.get(end).is_some_and(|byte| is_identifier_byte(*byte));
    !before && !after
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}
