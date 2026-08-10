//! The kind-1 identity-publication row header: one frozen layout, one decoder.
//!
//! ```text
//! JournalCommon[48]      # generation, parent identity, journal-inode identity
//! next_inode[16]         # the staged successor's inode identity
//! u8  base_presence      # 0 absent, 1 present
//! u8  stage_name_len
//! stage_name             # the fixed stage entry name
//! u32_be(base_len) || base_bytes
//! u32_be(next_len) || next_bytes
//! ```
//!
//! The header is the whole plan. Recovery needs no captured project and no
//! second read of the caller's successor: the exact bytes the plan was admitted
//! against and the exact bytes it installs are both durable before the claim,
//! which is why the kind's ceiling admits two full ledgers plus overhead.
//!
//! The generation slot carries the displaced artifact's inode identity — the
//! exact durable generation this publication replaces. A plan admitted against
//! an absent artifact displaces no generation, so the slot is zero and
//! `base_presence` is the authority on which arm the plan takes.

use marrow_fs_journal::{FsIdentity, JournalCommon};

use super::{LEDGER_BYTE_CEILING, STAGE_NAME};

/// The 48-byte shared leading common, as the journal owner freezes it.
const COMMON_LEN: usize = 48;
/// An absent generation: a plan admitted against an absent artifact displaces
/// no durable generation.
const NO_GENERATION: [u8; 16] = [0; 16];

/// The decoded kind-1 row header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowHeader {
    /// The admitted `.marrow` directory's identity at claim time.
    pub(crate) parent: FsIdentity,
    /// The claim file's own inode identity at claim time.
    pub(crate) journal_inode: FsIdentity,
    /// The displaced generation's inode identity, or `None` when the plan was
    /// admitted against an absent artifact.
    pub(crate) base: Option<FsIdentity>,
    /// The staged successor's inode identity.
    pub(crate) next_inode: FsIdentity,
    /// The exact captured artifact bytes; empty when [`Self::base`] is `None`.
    pub(crate) base_bytes: Vec<u8>,
    /// The exact canonical successor bytes.
    pub(crate) next_bytes: Vec<u8>,
}

/// Why a byte run is not a valid kind-1 row header. A malformed header is
/// retained corruption: it authorizes no artifact mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeaderCorruption {
    /// The header ends inside a fixed field.
    Truncated,
    /// The presence byte is neither 0 nor 1.
    BadPresence,
    /// The generation slot disagrees with the presence byte.
    GenerationPresenceMismatch,
    /// The header names a stage entry other than the fixed one.
    StageNameDrift,
    /// A byte run declares a length past the ledger's fixed bound.
    LengthOverBound,
    /// Bytes follow the successor run.
    TrailingBytes,
}

impl std::fmt::Display for HeaderCorruption {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Truncated => "the publication header ends inside a fixed field",
            Self::BadPresence => "the publication header's artifact-presence byte is not 0 or 1",
            Self::GenerationPresenceMismatch => {
                "the publication header's generation slot disagrees with its presence byte"
            }
            Self::StageNameDrift => "the publication header names a foreign stage entry",
            Self::LengthOverBound => "the publication header declares a run past the ledger bound",
            Self::TrailingBytes => "bytes follow the publication header's successor run",
        };
        formatter.write_str(text)
    }
}

impl RowHeader {
    /// The frozen encoding.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let common = JournalCommon {
            generation: self.base.map_or(NO_GENERATION, FsIdentity::to_bytes),
            parent: self.parent,
            journal_inode: self.journal_inode,
        };
        let stage = STAGE_NAME.as_bytes();
        let mut bytes = Vec::with_capacity(
            COMMON_LEN + 16 + 2 + stage.len() + 8 + self.base_bytes.len() + self.next_bytes.len(),
        );
        bytes.extend_from_slice(&common.encode());
        bytes.extend_from_slice(&self.next_inode.to_bytes());
        bytes.push(u8::from(self.base.is_some()));
        bytes.push(u8::try_from(stage.len()).expect("the fixed stage name is far below 256 bytes"));
        bytes.extend_from_slice(stage);
        push_run(&mut bytes, &self.base_bytes);
        push_run(&mut bytes, &self.next_bytes);
        bytes
    }

    /// Decode the frozen encoding, validating every structurally visible field.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, HeaderCorruption> {
        let mut reader = Reader::new(bytes);
        let common = JournalCommon::decode(reader.array::<COMMON_LEN>()?);
        let next_inode = FsIdentity::from_bytes(*reader.array::<16>()?);
        let presence = reader.byte()?;
        let base_present = match presence {
            0 => false,
            1 => true,
            _ => return Err(HeaderCorruption::BadPresence),
        };
        if base_present == (common.generation == NO_GENERATION) {
            return Err(HeaderCorruption::GenerationPresenceMismatch);
        }
        let stage_len = usize::from(reader.byte()?);
        if reader.take(stage_len)? != STAGE_NAME.as_bytes() {
            return Err(HeaderCorruption::StageNameDrift);
        }
        let base_bytes = reader.run()?.to_vec();
        let next_bytes = reader.run()?.to_vec();
        if !reader.is_empty() {
            return Err(HeaderCorruption::TrailingBytes);
        }
        if !base_present && !base_bytes.is_empty() {
            return Err(HeaderCorruption::GenerationPresenceMismatch);
        }
        Ok(Self {
            parent: common.parent,
            journal_inode: common.journal_inode,
            base: base_present.then(|| FsIdentity::from_bytes(common.generation)),
            next_inode,
            base_bytes,
            next_bytes,
        })
    }
}

fn push_run(bytes: &mut Vec<u8>, run: &[u8]) {
    let len = u32::try_from(run.len()).expect("an admitted ledger run fits the fixed bound");
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(run);
}

/// A bounded forward reader over the header bytes. Every read is checked; no
/// declared length allocates before it is compared against the fixed bound.
struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], HeaderCorruption> {
        if self.bytes.len() < len {
            return Err(HeaderCorruption::Truncated);
        }
        let (head, rest) = self.bytes.split_at(len);
        self.bytes = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<&'a [u8; N], HeaderCorruption> {
        Ok(self
            .take(N)?
            .try_into()
            .expect("the slice was taken at exactly N bytes"))
    }

    fn byte(&mut self) -> Result<u8, HeaderCorruption> {
        Ok(self.take(1)?[0])
    }

    fn run(&mut self) -> Result<&'a [u8], HeaderCorruption> {
        let len = u32::from_be_bytes(*self.array::<4>()?) as usize;
        if len > LEDGER_BYTE_CEILING {
            return Err(HeaderCorruption::LengthOverBound);
        }
        self.take(len)
    }

    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}
