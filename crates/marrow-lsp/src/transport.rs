//! Bounded LSP header framing over a byte stream.
//!
//! [`FrameReader`] reads one `Content-Length`-framed message at a time into a bounded
//! owned body. It validates the header block and the length before allocating the
//! body, so a hostile or malformed length never drives an unbounded allocation. Header
//! block and body are each bounded ([`MAX_HEADER_BLOCK_BYTES`], [`MAX_FRAME_BODY_BYTES`]);
//! a violation is a typed [`FramingFault`], never a panic or an unbounded read.
//!
//! The reader owns exactly one frame-under-construction. A clean end of input between
//! frames is [`FrameEvent::Eof`]; an end of input mid-frame is a
//! [`FramingFault::TruncatedFrame`]. This module performs no JSON parsing — the body
//! bytes are handed to [`crate::protocol::decode`] by the coordinator.

use std::io::{self, BufRead, Write};

use crate::capacities::{MAX_FRAME_BODY_BYTES, MAX_HEADER_BLOCK_BYTES};

/// One framing step: a complete message body, or a clean end of input.
pub enum FrameEvent {
    /// A complete framed message body.
    Frame(Vec<u8>),
    /// The stream ended cleanly between frames.
    Eof,
}

/// A typed framing fault. Every variant is terminal: the transport cannot recover a
/// frame boundary after one.
#[derive(Debug, PartialEq, Eq)]
pub enum FramingFault {
    /// The header block exceeded [`MAX_HEADER_BLOCK_BYTES`] before a blank line.
    HeaderBlockTooLarge,
    /// A header line was not `Name: value`.
    MalformedHeaderLine,
    /// No `Content-Length` header preceded the blank line.
    MissingContentLength,
    /// More than one `Content-Length` header appeared.
    DuplicateContentLength,
    /// The `Content-Length` value was not a non-negative integer.
    InvalidContentLength,
    /// The declared body length exceeded [`MAX_FRAME_BODY_BYTES`].
    BodyTooLarge {
        /// The declared `Content-Length` that exceeded the bound.
        declared: usize,
    },
    /// The stream ended after a partial frame (header or body).
    TruncatedFrame,
    /// A memory reservation for the bounded body failed.
    Allocation,
}

/// The result of one framing step.
pub type FrameResult = Result<FrameEvent, FrameError>;

/// A framing step failed with an I/O error or a typed framing fault.
#[derive(Debug)]
pub enum FrameError {
    /// The underlying stream reported an I/O error.
    Io(io::Error),
    /// The bytes did not frame under the LSP header grammar and bounds.
    Fault(FramingFault),
}

impl From<io::Error> for FrameError {
    fn from(error: io::Error) -> Self {
        FrameError::Io(error)
    }
}

/// A bounded frame reader over a buffered byte stream.
pub struct FrameReader<R> {
    reader: R,
}

impl<R: BufRead> FrameReader<R> {
    /// Wrap a buffered reader.
    pub fn new(reader: R) -> Self {
        Self { reader }
    }

    /// Read the next complete frame, a clean end of input, or a framing fault.
    pub fn next_frame(&mut self) -> FrameResult {
        let Some(length) = self.read_header_block()? else {
            return Ok(FrameEvent::Eof);
        };
        let body = self.read_body(length)?;
        Ok(FrameEvent::Frame(body))
    }

    /// Read and validate the header block, returning the body length. `Ok(None)` is a
    /// clean end of input before any header byte.
    fn read_header_block(&mut self) -> Result<Option<usize>, FrameError> {
        let mut content_length: Option<usize> = None;
        let mut consumed = 0usize;
        let mut any_byte = false;
        loop {
            let mut line = Vec::new();
            let read = read_line_bounded(&mut self.reader, &mut line, MAX_HEADER_BLOCK_BYTES - consumed)?;
            match read {
                LineRead::Eof if !any_byte && line.is_empty() => return Ok(None),
                LineRead::Eof => return Err(FrameError::Fault(FramingFault::TruncatedFrame)),
                LineRead::TooLong => {
                    return Err(FrameError::Fault(FramingFault::HeaderBlockTooLarge));
                }
                LineRead::Line => {}
            }
            any_byte = true;
            consumed += line.len();
            // Strip the CRLF (or lone LF) terminator.
            let content = strip_eol(&line);
            if content.is_empty() {
                // Blank line: end of the header block.
                return match content_length {
                    Some(length) => Ok(Some(length)),
                    None => Err(FrameError::Fault(FramingFault::MissingContentLength)),
                };
            }
            if let Some(value) = parse_content_length_line(content)? {
                if content_length.replace(value).is_some() {
                    return Err(FrameError::Fault(FramingFault::DuplicateContentLength));
                }
            }
        }
    }

    fn read_body(&mut self, length: usize) -> Result<Vec<u8>, FrameError> {
        if length > MAX_FRAME_BODY_BYTES {
            return Err(FrameError::Fault(FramingFault::BodyTooLarge { declared: length }));
        }
        let mut body = Vec::new();
        body.try_reserve_exact(length)
            .map_err(|_| FrameError::Fault(FramingFault::Allocation))?;
        body.resize(length, 0);
        match self.reader.read_exact(&mut body) {
            Ok(()) => Ok(body),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                Err(FrameError::Fault(FramingFault::TruncatedFrame))
            }
            Err(error) => Err(FrameError::Io(error)),
        }
    }
}

enum LineRead {
    Line,
    Eof,
    TooLong,
}

/// Read one `\n`-terminated line into `line`, but never more than `budget` bytes. A
/// line longer than the budget is [`LineRead::TooLong`]. A partial final line with no
/// newline followed by EOF is [`LineRead::Eof`] with the bytes read.
fn read_line_bounded<R: BufRead>(
    reader: &mut R,
    line: &mut Vec<u8>,
    budget: usize,
) -> io::Result<LineRead> {
    loop {
        let available = match reader.fill_buf() {
            Ok(buffer) => buffer,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            return Ok(LineRead::Eof);
        }
        match available.iter().position(|&byte| byte == b'\n') {
            Some(index) => {
                if line.len() + index + 1 > budget {
                    return Ok(LineRead::TooLong);
                }
                line.extend_from_slice(&available[..=index]);
                reader.consume(index + 1);
                return Ok(LineRead::Line);
            }
            None => {
                if line.len() + available.len() > budget {
                    return Ok(LineRead::TooLong);
                }
                line.extend_from_slice(available);
                let consumed = available.len();
                reader.consume(consumed);
            }
        }
    }
}

fn strip_eol(line: &[u8]) -> &[u8] {
    let without_lf = line.strip_suffix(b"\n").unwrap_or(line);
    without_lf.strip_suffix(b"\r").unwrap_or(without_lf)
}

/// Parse one header line. Returns `Some(length)` for `Content-Length`, `None` for any
/// other well-formed header, or a fault for a malformed line or bad length value.
fn parse_content_length_line(content: &[u8]) -> Result<Option<usize>, FrameError> {
    let Some(colon) = content.iter().position(|&byte| byte == b':') else {
        return Err(FrameError::Fault(FramingFault::MalformedHeaderLine));
    };
    let name = &content[..colon];
    if !name.eq_ignore_ascii_case(b"content-length") {
        return Ok(None);
    }
    let value = content[colon + 1..]
        .iter()
        .copied()
        .skip_while(|byte| byte.is_ascii_whitespace())
        .collect::<Vec<u8>>();
    let text = std::str::from_utf8(&value)
        .map_err(|_| FrameError::Fault(FramingFault::InvalidContentLength))?
        .trim_end();
    text.parse::<usize>()
        .map(Some)
        .map_err(|_| FrameError::Fault(FramingFault::InvalidContentLength))
}

/// Write one framed message: a `Content-Length` header, a blank line, then the body.
/// The whole frame is written and flushed as one unit.
pub fn write_frame<W: Write>(writer: &mut W, body: &[u8]) -> io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn framed(body: &str) -> Vec<u8> {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, body.as_bytes()).unwrap();
        buffer
    }

    fn read_all(bytes: &[u8]) -> Vec<FrameResult> {
        let mut reader = FrameReader::new(Cursor::new(bytes.to_vec()));
        let mut out = Vec::new();
        loop {
            match reader.next_frame() {
                Ok(FrameEvent::Eof) => {
                    out.push(Ok(FrameEvent::Eof));
                    return out;
                }
                other => {
                    let terminal = matches!(&other, Err(_));
                    out.push(other);
                    if terminal {
                        return out;
                    }
                }
            }
        }
    }

    #[test]
    fn roundtrips_one_frame() {
        let bytes = framed(r#"{"method":"m"}"#);
        let mut reader = FrameReader::new(Cursor::new(bytes));
        let FrameEvent::Frame(body) = reader.next_frame().unwrap() else {
            panic!("expected frame");
        };
        assert_eq!(body, br#"{"method":"m"}"#);
    }

    #[test]
    fn reads_two_frames_then_clean_eof() {
        let mut bytes = framed("a");
        bytes.extend(framed("bb"));
        let events = read_all(&bytes);
        assert!(matches!(events[0], Ok(FrameEvent::Frame(ref b)) if b == b"a"));
        assert!(matches!(events[1], Ok(FrameEvent::Frame(ref b)) if b == b"bb"));
        assert!(matches!(events[2], Ok(FrameEvent::Eof)));
    }

    #[test]
    fn clean_eof_on_empty_stream() {
        let events = read_all(b"");
        assert!(matches!(events[0], Ok(FrameEvent::Eof)));
    }

    #[test]
    fn accepts_extra_content_type_header() {
        let frame = "Content-Length: 1\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\nx";
        let mut reader = FrameReader::new(Cursor::new(frame.as_bytes().to_vec()));
        let FrameEvent::Frame(body) = reader.next_frame().unwrap() else {
            panic!();
        };
        assert_eq!(body, b"x");
    }

    #[test]
    fn missing_content_length_is_fault() {
        let frame = "Content-Type: x\r\n\r\nx";
        let mut reader = FrameReader::new(Cursor::new(frame.as_bytes().to_vec()));
        assert!(matches!(
            reader.next_frame(),
            Err(FrameError::Fault(FramingFault::MissingContentLength))
        ));
    }

    #[test]
    fn duplicate_content_length_is_fault() {
        let frame = "Content-Length: 1\r\nContent-Length: 2\r\n\r\nx";
        let mut reader = FrameReader::new(Cursor::new(frame.as_bytes().to_vec()));
        assert!(matches!(
            reader.next_frame(),
            Err(FrameError::Fault(FramingFault::DuplicateContentLength))
        ));
    }

    #[test]
    fn invalid_content_length_is_fault() {
        for bad in ["Content-Length: -1\r\n\r\n", "Content-Length: x\r\n\r\n"] {
            let mut reader = FrameReader::new(Cursor::new(bad.as_bytes().to_vec()));
            assert!(matches!(
                reader.next_frame(),
                Err(FrameError::Fault(FramingFault::InvalidContentLength))
            ));
        }
    }

    #[test]
    fn overlarge_body_length_is_fault_without_allocation() {
        let declared = MAX_FRAME_BODY_BYTES + 1;
        let frame = format!("Content-Length: {declared}\r\n\r\n");
        let mut reader = FrameReader::new(Cursor::new(frame.into_bytes()));
        assert!(matches!(
            reader.next_frame(),
            Err(FrameError::Fault(FramingFault::BodyTooLarge { .. }))
        ));
    }

    #[test]
    fn mid_body_eof_is_truncated() {
        let frame = "Content-Length: 10\r\n\r\nshort";
        let mut reader = FrameReader::new(Cursor::new(frame.as_bytes().to_vec()));
        assert!(matches!(
            reader.next_frame(),
            Err(FrameError::Fault(FramingFault::TruncatedFrame))
        ));
    }

    #[test]
    fn mid_header_eof_is_truncated() {
        let frame = "Content-Length: 10\r\n";
        let mut reader = FrameReader::new(Cursor::new(frame.as_bytes().to_vec()));
        assert!(matches!(
            reader.next_frame(),
            Err(FrameError::Fault(FramingFault::TruncatedFrame))
        ));
    }

    #[test]
    fn header_block_over_bound_is_fault() {
        let mut frame = String::new();
        while frame.len() < MAX_HEADER_BLOCK_BYTES + 16 {
            frame.push_str("X-Pad: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n");
        }
        let mut reader = FrameReader::new(Cursor::new(frame.into_bytes()));
        assert!(matches!(
            reader.next_frame(),
            Err(FrameError::Fault(FramingFault::HeaderBlockTooLarge))
        ));
    }

    #[test]
    fn lone_lf_line_endings_are_accepted() {
        let frame = "Content-Length: 1\n\nx";
        let mut reader = FrameReader::new(Cursor::new(frame.as_bytes().to_vec()));
        let FrameEvent::Frame(body) = reader.next_frame().unwrap() else {
            panic!();
        };
        assert_eq!(body, b"x");
    }
}
