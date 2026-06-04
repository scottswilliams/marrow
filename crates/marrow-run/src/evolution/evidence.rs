use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::DataPathSegment;

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

#[derive(Clone)]
pub(super) struct EvidenceDigest {
    hash: u64,
}

impl EvidenceDigest {
    pub(super) fn new(label: &str) -> Self {
        let mut digest = Self { hash: FNV_OFFSET };
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
        format!("fnv1a64:{:016x}", self.hash)
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
        for byte in bytes {
            self.hash ^= u64::from(*byte);
            self.hash = self.hash.wrapping_mul(FNV_PRIME);
        }
    }
}

pub(super) fn retire_evidence_digest(
    commit_id: u64,
    records_retired: u64,
    counts: &[(CatalogId, u64)],
) -> String {
    let mut counts = counts.to_vec();
    counts.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    let mut digest = EvidenceDigest::new("marrow-activation-retire-v1");
    digest.u64(commit_id);
    digest.u64(records_retired);
    digest.u64(counts.len() as u64);
    for (id, count) in counts {
        digest.catalog_id(&id);
        digest.u64(count);
    }
    digest.finish()
}

#[derive(Default)]
pub(super) struct EvidenceSetDigest {
    count: u64,
    sum: u64,
    xor: u64,
}

impl EvidenceSetDigest {
    pub(super) fn add(&mut self, row: EvidenceDigest) {
        let hash = row.hash;
        self.count += 1;
        self.sum = self.sum.wrapping_add(hash);
        self.xor ^= hash;
    }

    pub(super) fn finish(&self, label: &str) -> String {
        let mut digest = EvidenceDigest::new(label);
        digest.u64(self.count);
        digest.u64(self.sum);
        digest.u64(self.xor);
        digest.finish()
    }
}
