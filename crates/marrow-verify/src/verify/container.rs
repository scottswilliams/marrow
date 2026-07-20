//! Phase 1: container framing — magic, version, digest slot, and section table.

use super::code_tables::{decode_consts, decode_exports, decode_functions, decode_spans};
use super::durable::decode_durable;
use super::model::DecodedImage;
use super::reject;
use super::tables::{
    decode_collections, decode_enums, decode_strings, decode_test_entries, decode_types,
    reject_value_type_cycles, validate_record_field_refs,
};
use crate::reader::Reader;
use crate::reject::{VerifyPhase, VerifyRejection};
use marrow_image::image_id;

/// The container framing constants.
const MAGIC: &[u8; 4] = b"MWI\0";
const VERSION: u8 = 0x00;
const DIGEST_SLOT_END: usize = 37;

pub(super) fn decode_container(bytes: &[u8]) -> Result<DecodedImage, VerifyRejection> {
    if bytes.len() > marrow_image::bounds::MAX_IMAGE_BYTES {
        return Err(reject(
            VerifyPhase::Envelope,
            "image exceeds the size bound",
        ));
    }
    let mut reader = Reader::new(bytes);
    let magic = reader
        .take(4)
        .ok_or(reject(VerifyPhase::Envelope, "short magic"))?;
    if magic != MAGIC {
        return Err(reject(VerifyPhase::Envelope, "bad magic"));
    }
    let version = reader
        .u8()
        .ok_or(reject(VerifyPhase::Envelope, "short version"))?;
    if version != VERSION {
        return Err(reject(VerifyPhase::Envelope, "unsupported version"));
    }
    let stored_digest = reader
        .take(32)
        .ok_or(reject(VerifyPhase::Envelope, "short digest slot"))?;
    // Recompute the digest over the payload (every byte after the digest slot).
    let payload = &bytes[DIGEST_SLOT_END..];
    if image_id(payload).0.as_slice() != stored_digest {
        return Err(reject(VerifyPhase::Envelope, "digest mismatch"));
    }

    let section_count = reader
        .u8()
        .ok_or(reject(VerifyPhase::Envelope, "short section count"))?;
    if section_count != 10 {
        return Err(reject(VerifyPhase::Envelope, "section count must be 10"));
    }
    let mut sections: Vec<(u8, &[u8])> = Vec::with_capacity(10);
    let mut last_id = 0u8;
    for _ in 0..10 {
        let id = reader
            .u8()
            .ok_or(reject(VerifyPhase::Envelope, "short section id"))?;
        if id <= last_id {
            return Err(reject(
                VerifyPhase::Envelope,
                "section ids must strictly ascend",
            ));
        }
        last_id = id;
        let len = reader
            .u32()
            .ok_or(reject(VerifyPhase::Envelope, "short section length"))?
            as usize;
        let body = reader
            .take(len)
            .ok_or(reject(VerifyPhase::Envelope, "section length past input"))?;
        sections.push((id, body));
    }
    if !reader.is_empty() {
        return Err(reject(
            VerifyPhase::Envelope,
            "trailing bytes after sections",
        ));
    }
    // Section ids strictly ascend and there are exactly 10, so they are exactly 1..10.
    for (index, (id, _)) in sections.iter().enumerate() {
        if *id != (index as u8 + 1) {
            return Err(reject(
                VerifyPhase::Envelope,
                "section ids must be exactly 1..10",
            ));
        }
    }

    // Phase 2: decode each table. Spans are decoded per function, in FUNCTIONS
    // order, so they are attached to the already-decoded function list.
    let strings = decode_strings(sections[0].1)?;
    let types = decode_types(sections[1].1, strings.len())?;
    let enums = decode_enums(sections[8].1, strings.len(), types.len())?;
    let collections = decode_collections(sections[9].1, types.len(), enums.len())?;
    validate_record_field_refs(&types, enums.len(), collections.len())?;
    reject_value_type_cycles(&types, &enums)?;
    let (roots, sites, site_paths, durable_contract, durable_descriptor) =
        decode_durable(sections[2].1, &strings, &types, &enums)?;
    let consts = decode_consts(sections[3].1, &strings)?;
    let mut functions = decode_functions(
        sections[4].1,
        strings.len(),
        types.len(),
        enums.len(),
        collections.len(),
        roots.len(),
    )?;
    let exports = decode_exports(sections[5].1, functions.len())?;
    decode_spans(sections[6].1, &mut functions)?;
    let test_entries = decode_test_entries(sections[7].1, strings.len(), functions.len())?;

    Ok(DecodedImage {
        image_id: image_id(payload),
        strings,
        types,
        enums,
        collections,
        roots,
        sites,
        site_paths,
        durable_contract,
        durable_descriptor,
        consts,
        functions,
        exports,
        test_entries,
    })
}
