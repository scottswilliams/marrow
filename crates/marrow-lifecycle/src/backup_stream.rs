//! Bounded logical-backup framing. This layer checks bytes and completion, not
//! image admission or logical cell validity; those remain with their owners.
//!
//! The header is `MWBK`, version 0, then u32-length image and head blocks.
//! Each tag-1 record holds u32-length key and value blocks. Tag 0 ends the
//! stream with a u64 cell count and 32-byte digest, followed by exact EOF.
//! All integers are big-endian. The digest starts with the magic/version and
//! chains the image block, head block, each cell record, and tag/count footer.

use std::io::{self, Read, Write};

use marrow_image::{StoreBackupDigest, bounds::MAX_IMAGE_BYTES};
use marrow_kernel::durable::{ExportSink, MAX_KEY_LEN, MAX_VALUE_LEN};

use crate::{FormatError, MAX_HEAD_FILE_BYTES};

const PREFIX: &[u8; 5] = b"MWBK\0";

#[derive(Debug)]
pub(crate) enum StreamError {
    Io(io::Error),
    Format(FormatError),
}

impl From<io::Error> for StreamError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<FormatError> for StreamError {
    fn from(error: FormatError) -> Self {
        Self::Format(error)
    }
}

pub(crate) struct Header {
    pub image: Vec<u8>,
    pub head: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Reading,
    Finished,
    Failed,
}

struct Chain(StoreBackupDigest);

impl Chain {
    fn new() -> Self {
        Self(StoreBackupDigest::compute(PREFIX))
    }
    fn step(&mut self, record: &[u8]) {
        let mut payload = Vec::with_capacity(32 + record.len());
        payload.extend_from_slice(self.0.bytes());
        payload.extend_from_slice(record);
        self.0 = StoreBackupDigest::compute(&payload);
    }
}

fn block(bytes: &[u8], maximum: usize, field: &'static str) -> Result<Vec<u8>, FormatError> {
    if bytes.len() > maximum {
        return Err(FormatError::LengthOverflow { field });
    }
    let length = u32::try_from(bytes.len()).map_err(|_| FormatError::LengthOverflow { field })?;
    let mut record = Vec::with_capacity(4 + bytes.len());
    record.extend_from_slice(&length.to_be_bytes());
    record.extend_from_slice(bytes);
    Ok(record)
}

fn cell_record(key: &[u8], value: &[u8]) -> Result<Vec<u8>, FormatError> {
    let mut record = vec![1];
    record.extend_from_slice(&block(key, MAX_KEY_LEN, "backup key")?);
    record.extend_from_slice(&block(value, MAX_VALUE_LEN, "backup value")?);
    Ok(record)
}

fn ordered(previous: &Option<Vec<u8>>, key: &[u8]) -> Result<(), FormatError> {
    if previous
        .as_ref()
        .is_some_and(|before| before.as_slice() >= key)
    {
        Err(FormatError::Malformed {
            reason: "backup cells are not strictly ordered",
        })
    } else {
        Ok(())
    }
}

fn footer(count: u64) -> [u8; 9] {
    let mut bytes = [0; 9];
    bytes[1..].copy_from_slice(&count.to_be_bytes());
    bytes
}

/// Output remains private until full logical validation and file publication.
pub(crate) struct Encoder<'a> {
    output: &'a mut dyn Write,
    chain: Chain,
    previous: Option<Vec<u8>>,
    count: u64,
    state: State,
}

impl<'a> Encoder<'a> {
    pub fn new(output: &'a mut dyn Write, image: &[u8], head: &[u8]) -> Result<Self, StreamError> {
        let image = block(image, MAX_IMAGE_BYTES, "backup image")?;
        let head = block(head, MAX_HEAD_FILE_BYTES as usize, "backup head")?;
        let mut chain = Chain::new();
        output.write_all(PREFIX)?;
        for record in [&image, &head] {
            output.write_all(record)?;
            chain.step(record);
        }
        Ok(Self {
            output,
            chain,
            previous: None,
            count: 0,
            state: State::Reading,
        })
    }

    pub fn finish(mut self) -> io::Result<StoreBackupDigest> {
        if self.state != State::Reading {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "backup output previously failed",
            ));
        }
        let footer = footer(self.count);
        self.chain.step(&footer);
        self.output.write_all(&footer)?;
        self.output.write_all(self.chain.0.bytes())?;
        self.output.flush()?;
        Ok(self.chain.0)
    }
}

impl ExportSink for Encoder<'_> {
    fn cell(&mut self, key: &[u8], value: &[u8]) -> io::Result<()> {
        if self.state != State::Reading {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "backup output previously failed",
            ));
        }
        self.state = State::Failed;
        ordered(&self.previous, key)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let count = self
            .count
            .checked_add(1)
            .ok_or_else(|| io::Error::new(io::ErrorKind::FileTooLarge, "backup count overflow"))?;
        let record = cell_record(key, value)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.output.write_all(&record)?;
        self.chain.step(&record);
        self.previous = Some(key.to_vec());
        self.count = count;
        self.state = State::Reading;
        Ok(())
    }
}

pub(crate) struct Decoder<'a> {
    input: &'a mut dyn Read,
    chain: Chain,
    previous: Option<Vec<u8>>,
    count: u64,
    state: State,
}

fn exact<const N: usize>(input: &mut dyn Read) -> Result<[u8; N], StreamError> {
    let mut bytes = [0; N];
    input.read_exact(&mut bytes).map_err(read_error)?;
    Ok(bytes)
}

fn read_block(
    input: &mut dyn Read,
    maximum: usize,
    field: &'static str,
) -> Result<Vec<u8>, StreamError> {
    let length = u32::from_be_bytes(exact(input)?) as usize;
    if length > maximum {
        return Err(FormatError::LengthOverflow { field }.into());
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes).map_err(read_error)?;
    Ok(bytes)
}

fn read_error(error: io::Error) -> StreamError {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        StreamError::Format(FormatError::Truncated)
    } else {
        StreamError::Io(error)
    }
}

impl<'a> Decoder<'a> {
    pub fn new(input: &'a mut dyn Read) -> Result<(Self, Header), StreamError> {
        let prefix = exact::<5>(input)?;
        if prefix[..4] != PREFIX[..4] {
            return Err(FormatError::BadMagic.into());
        }
        if prefix[4] != 0 {
            return Err(FormatError::UnknownVersion { found: prefix[4] }.into());
        }
        let image = read_block(input, MAX_IMAGE_BYTES, "backup image")?;
        let head = read_block(input, MAX_HEAD_FILE_BYTES as usize, "backup head")?;
        let mut chain = Chain::new();
        chain.step(&block(&image, MAX_IMAGE_BYTES, "backup image")?);
        chain.step(&block(&head, MAX_HEAD_FILE_BYTES as usize, "backup head")?);
        Ok((
            Self {
                input,
                chain,
                previous: None,
                count: 0,
                state: State::Reading,
            },
            Header { image, head },
        ))
    }

    pub fn next_cell(&mut self) -> Result<Option<(Vec<u8>, Vec<u8>)>, StreamError> {
        match self.state {
            State::Finished => return Ok(None),
            State::Failed => {
                return Err(FormatError::Malformed {
                    reason: "backup input previously failed",
                }
                .into());
            }
            State::Reading => {}
        }
        self.state = State::Failed;
        let cell = self.read_record()?;
        self.state = if cell.is_some() {
            State::Reading
        } else {
            State::Finished
        };
        Ok(cell)
    }

    fn read_record(&mut self) -> Result<Option<(Vec<u8>, Vec<u8>)>, StreamError> {
        match exact::<1>(self.input)?[0] {
            1 => {
                let key = read_block(self.input, MAX_KEY_LEN, "backup key")?;
                let value = read_block(self.input, MAX_VALUE_LEN, "backup value")?;
                ordered(&self.previous, &key)?;
                self.count = self
                    .count
                    .checked_add(1)
                    .ok_or(FormatError::LengthOverflow {
                        field: "backup count",
                    })?;
                self.chain.step(&cell_record(&key, &value)?);
                self.previous = Some(key.clone());
                Ok(Some((key, value)))
            }
            0 => {
                let count = u64::from_be_bytes(exact(self.input)?);
                if count != self.count {
                    return Err(FormatError::Malformed {
                        reason: "backup count differs",
                    }
                    .into());
                }
                self.chain.step(&footer(count));
                let expected = exact::<32>(self.input)?;
                if self.chain.0.bytes() != &expected {
                    return Err(FormatError::DigestMismatch.into());
                }
                match self.input.read_exact(&mut [0]) {
                    Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
                    Err(error) => Err(StreamError::Io(error)),
                    Ok(()) => Err(FormatError::TrailingBytes.into()),
                }
            }
            _ => Err(FormatError::UnknownDiscriminant {
                field: "backup record",
            }
            .into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fixture() -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = Encoder::new(&mut bytes, b"i", b"h").unwrap();
        encoder.cell(b"a", b"x").unwrap();
        encoder.cell(b"b", b"y").unwrap();
        encoder.finish().unwrap();
        bytes
    }

    fn consume(bytes: &[u8]) -> Result<(), StreamError> {
        let mut input = Cursor::new(bytes);
        let (mut decoder, _) = Decoder::new(&mut input)?;
        while decoder.next_cell()?.is_some() {}
        Ok(())
    }

    #[test]
    fn version_zero_known_answer() {
        // Framing and trailer computed independently with Python hashlib.sha256.
        let expected = b"MWBK\0\0\0\0\x01i\0\0\0\x01h\x01\0\0\0\x01a\0\0\0\x01x\x01\0\0\0\x01b\0\0\0\x01y\0\0\0\0\0\0\0\0\x02\x3d\xaf\x69\xd3\x9a\xad\x6e\x7a\x13\xed\x89\xa8\x09\x46\xac\xf7\x57\x86\x9c\x52\x92\x3a\x32\x56\x23\x85\x06\x6d\x61\xc9\x3f\xbe";
        assert_eq!(fixture(), expected);
        consume(expected).unwrap();
    }

    #[test]
    fn unknown_format_and_wrong_count_refuse_before_completion() {
        for (position, byte, expected) in [
            (0, b'X', FormatError::BadMagic),
            (4, 1, FormatError::UnknownVersion { found: 1 }),
            (
                15,
                2,
                FormatError::UnknownDiscriminant {
                    field: "backup record",
                },
            ),
            (
                45,
                3,
                FormatError::Malformed {
                    reason: "backup count differs",
                },
            ),
        ] {
            let mut bytes = fixture();
            bytes[position] = byte;
            assert!(
                matches!(consume(&bytes), Err(StreamError::Format(found)) if found == expected)
            );
        }
    }

    #[test]
    fn framing_is_independent_of_read_chunking_and_retains_header_bytes() {
        struct Chunks(Cursor<Vec<u8>>);
        impl Read for Chunks {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                let length = bytes.len().min(3);
                self.0.read(&mut bytes[..length])
            }
        }
        let mut input = Chunks(Cursor::new(fixture()));
        let (mut decoder, header) = Decoder::new(&mut input).unwrap();
        assert_eq!(header.image, b"i");
        assert_eq!(header.head, b"h");
        assert_eq!(
            decoder.next_cell().unwrap(),
            Some((b"a".to_vec(), b"x".to_vec()))
        );
        assert_eq!(
            decoder.next_cell().unwrap(),
            Some((b"b".to_vec(), b"y".to_vec()))
        );
        assert!(decoder.next_cell().unwrap().is_none());
        assert!(decoder.next_cell().unwrap().is_none());
    }

    #[test]
    fn every_truncated_prefix_refuses_and_trailing_data_is_not_completion() {
        let bytes = fixture();
        for end in 0..bytes.len() {
            assert!(
                matches!(
                    consume(&bytes[..end]),
                    Err(StreamError::Format(FormatError::Truncated))
                ),
                "prefix {end}"
            );
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(matches!(
            consume(&extra),
            Err(StreamError::Format(FormatError::TrailingBytes))
        ));
        for position in [9, 14, 25, bytes.len() - 1] {
            let mut damaged = bytes.clone();
            damaged[position] ^= 1;
            assert!(
                matches!(
                    consume(&damaged),
                    Err(StreamError::Format(FormatError::DigestMismatch))
                ),
                "byte {position}"
            );
        }
    }

    #[test]
    fn lengths_refuse_before_reading_or_allocating_the_declared_body() {
        let mut bytes = PREFIX.to_vec();
        bytes.extend_from_slice(&u32::MAX.to_be_bytes());
        let mut input = Cursor::new(bytes);
        assert!(matches!(
            Decoder::new(&mut input),
            Err(StreamError::Format(FormatError::LengthOverflow {
                field: "backup image"
            }))
        ));
        assert_eq!(input.position(), 9);
        let mut bytes = fixture()[..10].to_vec();
        bytes.extend_from_slice(&u32::MAX.to_be_bytes());
        let mut input = Cursor::new(bytes);
        assert!(matches!(
            Decoder::new(&mut input),
            Err(StreamError::Format(FormatError::LengthOverflow {
                field: "backup head"
            }))
        ));
        assert_eq!(input.position(), 14);
        let mut bytes = fixture()[..15].to_vec();
        bytes.push(1);
        bytes.extend_from_slice(&u32::MAX.to_be_bytes());
        let mut input = Cursor::new(bytes);
        let (mut decoder, _) = Decoder::new(&mut input).unwrap();
        assert!(matches!(
            decoder.next_cell(),
            Err(StreamError::Format(FormatError::LengthOverflow {
                field: "backup key"
            }))
        ));
        let mut bytes = fixture()[..21].to_vec();
        bytes.extend_from_slice(&u32::MAX.to_be_bytes());
        let mut input = Cursor::new(bytes);
        let (mut decoder, _) = Decoder::new(&mut input).unwrap();
        assert!(matches!(
            decoder.next_cell(),
            Err(StreamError::Format(FormatError::LengthOverflow {
                field: "backup value"
            }))
        ));
    }

    #[test]
    fn repeated_keys_refuse_before_digest_and_cannot_be_skipped() {
        let mut bytes = fixture();
        bytes[31] = b'a';
        let mut input = Cursor::new(bytes);
        let (mut decoder, _) = Decoder::new(&mut input).unwrap();
        assert!(decoder.next_cell().unwrap().is_some());
        assert!(matches!(
            decoder.next_cell(),
            Err(StreamError::Format(FormatError::Malformed {
                reason: "backup cells are not strictly ordered"
            }))
        ));
        assert!(matches!(
            decoder.next_cell(),
            Err(StreamError::Format(FormatError::Malformed {
                reason: "backup input previously failed"
            }))
        ));
    }

    #[test]
    fn count_overflow_cannot_write_completion() {
        let mut bytes = Vec::new();
        let mut encoder = Encoder::new(&mut bytes, b"i", b"h").unwrap();
        encoder.count = u64::MAX;
        assert_eq!(
            encoder.cell(b"a", b"x").unwrap_err().kind(),
            io::ErrorKind::FileTooLarge
        );
        assert!(encoder.finish().is_err());
        assert_eq!(bytes.len(), 15);
        let bytes = fixture();
        let mut input = Cursor::new(bytes);
        let (mut decoder, _) = Decoder::new(&mut input).unwrap();
        decoder.count = u64::MAX;
        assert!(matches!(
            decoder.next_cell(),
            Err(StreamError::Format(FormatError::LengthOverflow {
                field: "backup count"
            }))
        ));
    }

    #[test]
    fn operational_read_failure_is_not_reported_as_bad_format() {
        struct Failed;
        impl Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        }
        assert!(
            matches!(Decoder::new(&mut Failed), Err(StreamError::Io(error))
            if error.kind() == io::ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn partial_write_failure_is_sticky_even_if_the_sink_recovers() {
        struct FailsOnce {
            bytes: Vec<u8>,
            allowance: usize,
        }
        impl Write for FailsOnce {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.allowance == 0 {
                    self.allowance = usize::MAX;
                    return Err(io::Error::from(io::ErrorKind::StorageFull));
                }
                let n = self.allowance.min(bytes.len());
                self.bytes.extend_from_slice(&bytes[..n]);
                self.allowance -= n;
                Ok(n)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut output = FailsOnce {
            bytes: Vec::new(),
            allowance: 18,
        };
        let mut encoder = Encoder::new(&mut output, b"i", b"h").unwrap();
        assert_eq!(
            encoder.cell(b"a", b"x").unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
        assert_eq!(
            encoder.cell(b"b", b"y").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(encoder.finish().is_err());
        assert_eq!(output.bytes.len(), 18);
        assert!(matches!(
            consume(&output.bytes),
            Err(StreamError::Format(FormatError::Truncated))
        ));
    }

    #[test]
    fn flush_failure_cannot_return_a_successful_completion_digest() {
        struct FailsFlush(Vec<u8>);
        impl Write for FailsFlush {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::from(io::ErrorKind::StorageFull))
            }
        }
        let mut output = FailsFlush(Vec::new());
        let mut encoder = Encoder::new(&mut output, b"i", b"h").unwrap();
        encoder.cell(b"a", b"x").unwrap();
        assert_eq!(
            encoder.finish().unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
        // Complete private bytes do not establish durable publication.
        assert!(consume(&output.0).is_ok());
    }
}
