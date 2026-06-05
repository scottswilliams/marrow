use std::fmt::Write as _;

use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::DataPathSegment;
use sha2::{Digest, Sha256};

/// Domain-separation tag for the per-cell evidence digest of an activation-default
/// backfill. Backfill staging and crash-resume completion both seed their digest
/// with this exact label; the completion digest is meaningful only because it must
/// equal the staged one, so the two sides share a single const rather than two
/// literals that could silently drift apart.
pub(super) const ACTIVATION_DEFAULT_DIGEST: &str = "marrow-activation-default-v1";

/// Domain-separation tag for the per-row digest folded into a rebuilt-index set
/// digest. The expected (record-derived) and actual (index-order) sides hash each
/// row under this label, so a single const keeps them from diverging by a typo.
pub(super) const INDEX_ROW_DIGEST: &str = "marrow-index-row-v1";

/// Domain-separation tag for the order-independent set digest summarizing a rebuilt
/// index. Expected and actual set digests are compared under this label, so it must
/// be one shared const on both sides of the comparison.
pub(super) const INDEX_SET_DIGEST: &str = "marrow-index-set-v1";

/// Domain-separation tag for the retire-evidence digest stamped at activation.
const ACTIVATION_RETIRE_DIGEST: &str = "marrow-activation-retire-v1";

#[derive(Clone)]
pub(super) struct EvidenceDigest {
    hash: Sha256,
}

impl EvidenceDigest {
    pub(super) fn new(label: &str) -> Self {
        let mut digest = Self {
            hash: Sha256::new(),
        };
        digest.bytes(label.as_bytes());
        digest
    }

    pub(super) fn catalog_id(&mut self, id: &CatalogId) {
        self.bytes(id.as_str().as_bytes());
    }

    pub(super) fn u64(&mut self, value: u64) {
        self.raw(&value.to_be_bytes());
    }

    pub(super) fn bool(&mut self, value: bool) {
        self.raw(&[u8::from(value)]);
    }

    pub(super) fn bytes(&mut self, value: &[u8]) {
        self.u64(value.len() as u64);
        self.raw(value);
    }

    pub(super) fn saved_keys(&mut self, keys: &[SavedKey]) {
        self.u64(keys.len() as u64);
        for key in keys {
            self.saved_key(key);
        }
    }

    pub(super) fn data_path(&mut self, path: &[DataPathSegment]) {
        self.u64(path.len() as u64);
        for segment in path {
            match segment {
                DataPathSegment::Member(id) => {
                    self.raw(&[0]);
                    self.catalog_id(id);
                }
                DataPathSegment::Key(key) => {
                    self.raw(&[1]);
                    self.saved_key(key);
                }
            }
        }
    }

    pub(super) fn finish(&self) -> String {
        let digest = self.finish_bytes();
        let mut out = String::with_capacity("sha256:".len() + digest.len() * 2);
        out.push_str("sha256:");
        for byte in digest {
            write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
        }
        out
    }

    fn saved_key(&mut self, key: &SavedKey) {
        match key {
            SavedKey::Int(value) => {
                self.raw(&[0]);
                self.raw(&value.to_be_bytes());
            }
            SavedKey::Bool(value) => {
                self.raw(&[1]);
                self.bool(*value);
            }
            SavedKey::Str(value) => {
                self.raw(&[2]);
                self.bytes(value.as_bytes());
            }
            SavedKey::Date(value) => {
                self.raw(&[3]);
                self.raw(&value.to_be_bytes());
            }
            SavedKey::Duration(value) => {
                self.raw(&[4]);
                self.raw(&value.to_be_bytes());
            }
            SavedKey::Instant(value) => {
                self.raw(&[5]);
                self.raw(&value.to_be_bytes());
            }
            SavedKey::Bytes(value) => {
                self.raw(&[6]);
                self.bytes(value);
            }
        }
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.hash.update(bytes);
    }

    fn finish_bytes(&self) -> [u8; 32] {
        let digest = self.hash.clone().finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&digest);
        bytes
    }
}

pub(super) fn retire_evidence_digest(
    commit_id: u64,
    records_retired: u64,
    counts: &[(CatalogId, u64)],
) -> String {
    let mut counts = counts.to_vec();
    counts.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    let mut digest = EvidenceDigest::new(ACTIVATION_RETIRE_DIGEST);
    digest.u64(commit_id);
    digest.u64(records_retired);
    digest.u64(counts.len() as u64);
    for (id, count) in counts {
        digest.catalog_id(&id);
        digest.u64(count);
    }
    digest.finish()
}

/// Bounded, order-independent evidence for rebuilt index rows. Expected rows are
/// derived from saved records while actual rows are visited by index order, so the
/// summary cannot depend on traversal order or retain every row digest.
#[derive(Default)]
pub(super) struct EvidenceSetDigest {
    count: u64,
    sum: [u64; 4],
    xor: [u64; 4],
}

impl EvidenceSetDigest {
    pub(super) fn add(&mut self, row: EvidenceDigest) {
        let hash = row.finish_bytes();
        self.count += 1;
        for (slot, chunk) in hash.chunks_exact(8).enumerate() {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(chunk);
            let word = u64::from_be_bytes(bytes);
            self.sum[slot] = self.sum[slot].wrapping_add(word);
            self.xor[slot] ^= word;
        }
    }

    pub(super) fn finish(&self, label: &str) -> String {
        let mut digest = EvidenceDigest::new(label);
        digest.u64(self.count);
        for word in self.sum {
            digest.u64(word);
        }
        for word in self.xor {
            digest.u64(word);
        }
        digest.finish()
    }
}
