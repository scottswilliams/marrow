//! Known-answer tests freezing every field and golden byte of the five-kind
//! pending-journal frame, plus the hostile decode matrix. These bytes are a
//! durability contract: a change here is a format break, not a refactor.

use marrow_fs_journal::{
    DecodedFrame, FrameCorruption, FrameLawError, FsIdentity, JournalCommon, JournalKind,
    TailState, decode_frame, encode_header, encode_record,
};

const PARENT: FsIdentity = FsIdentity::new(0x0102_0304_0506_0708, 0x1112_1314_1516_1718);
const INODE: FsIdentity = FsIdentity::new(0x2122_2324_2526_2728, 0x3132_3334_3536_3738);

fn generation() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = 0x10 + index as u8;
    }
    bytes
}

fn common() -> JournalCommon {
    JournalCommon {
        generation: generation(),
        parent: PARENT,
        journal_inode: INODE,
    }
}

/// The frozen 16-byte prefix for a kind and header length.
fn prefix(kind_code: u8, header_len: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"MWPEND0\0");
    bytes.push(0); // version
    bytes.push(kind_code);
    bytes.extend_from_slice(&[0, 0]); // reserved
    bytes.extend_from_slice(&header_len.to_be_bytes());
    bytes
}

/// The frozen record layout for a sequence, tag, and payload.
fn record(sequence: u32, tag: u8, payload: &[u8]) -> Vec<u8> {
    let record_len = 5 + payload.len() as u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&record_len.to_be_bytes());
    bytes.extend_from_slice(&sequence.to_be_bytes());
    bytes.push(tag);
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(&record_len.to_be_bytes());
    bytes
}

fn lineage_header() -> Vec<u8> {
    let mut header = common().encode().to_vec();
    header.extend_from_slice(&[0xEE; 136]);
    assert_eq!(header.len(), 184);
    header
}

fn cache_header() -> Vec<u8> {
    let mut header = common().encode().to_vec();
    header.extend_from_slice(&[0xDD; 237]);
    assert_eq!(header.len(), 285);
    header
}

/// The complete golden kind-4 frame: exactly 306 bytes.
fn golden_lineage() -> Vec<u8> {
    let mut frame = prefix(4, 184);
    frame.extend_from_slice(&lineage_header());
    frame.extend_from_slice(&record(0, 1, &[0x01]));
    frame.extend_from_slice(&record(1, 2, &[0xB1; 33]));
    frame.extend_from_slice(&record(2, 3, &[0xB2; 33]));
    assert_eq!(frame.len(), 306);
    frame
}

/// The complete golden kind-5 frame: exactly 713 bytes.
fn golden_cache() -> Vec<u8> {
    let mut frame = prefix(5, 285);
    frame.extend_from_slice(&cache_header());
    frame.extend_from_slice(&record(0, 1, &[0x01]));
    frame.extend_from_slice(&record(1, 2, &[0xC1; 32]));
    frame.extend_from_slice(&record(2, 3, &[0xC2; 128]));
    frame.extend_from_slice(&record(3, 4, &[0xC3; 105]));
    frame.extend_from_slice(&record(4, 5, &[0xC4; 81]));
    assert_eq!(frame.len(), 713);
    frame
}

/// A complete kind-2 frame filling its 4,096-byte ceiling exactly.
fn ceiling_provision() -> Vec<u8> {
    let mut frame = prefix(2, 100);
    frame.extend_from_slice(&[0x77; 100]);
    frame.extend_from_slice(&record(0, 1, &[]));
    frame.extend_from_slice(&record(1, 2, &[]));
    frame.extend_from_slice(&record(2, 3, &[0x99; 3941]));
    assert_eq!(frame.len(), 4096);
    frame
}

fn decode_ok(kind: JournalKind, bytes: &[u8]) -> DecodedFrame {
    decode_frame(kind, bytes).expect("a well-formed frame decodes")
}

#[test]
fn identity_projection_is_the_frozen_sixteen_byte_layout() {
    let bytes = PARENT.to_bytes();
    assert_eq!(
        bytes,
        [
            1, 2, 3, 4, 5, 6, 7, 8, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18
        ]
    );
    assert_eq!(FsIdentity::from_bytes(bytes), PARENT);
}

#[test]
fn journal_common_is_the_frozen_forty_eight_byte_layout() {
    let encoded = common().encode();
    assert_eq!(&encoded[0..16], &generation());
    assert_eq!(&encoded[16..32], &PARENT.to_bytes());
    assert_eq!(&encoded[32..48], &INODE.to_bytes());
    assert_eq!(JournalCommon::decode(&encoded), common());
}

#[test]
fn kind_codes_ceilings_and_registries_are_frozen() {
    let facts = [
        (JournalKind::Ids, 1, 2_101_248, 3),
        (JournalKind::Provision, 2, 4_096, 3),
        (JournalKind::Rebind, 3, 4_096, 6),
        (JournalKind::Lineage, 4, 4_096, 3),
        (JournalKind::Cache, 5, 4_096, 5),
    ];
    for (kind, code, ceiling, phases) in facts {
        assert_eq!(kind.code(), code);
        assert_eq!(JournalKind::from_code(code), Some(kind));
        assert_eq!(kind.ceiling(), ceiling);
        assert_eq!(kind.phase_count(), phases);
    }
    assert_eq!(JournalKind::from_code(0), None);
    assert_eq!(JournalKind::from_code(6), None);
}

#[test]
fn the_golden_lineage_frame_is_byte_exact() {
    let golden = golden_lineage();

    let mut encoded = encode_header(JournalKind::Lineage, &lineage_header()).expect("header");
    assert_eq!(encoded.len(), 200);
    assert_eq!(&encoded[0..8], b"MWPEND0\0");
    assert_eq!(encoded[8], 0, "version");
    assert_eq!(encoded[9], 4, "kind byte");
    assert_eq!(&encoded[10..12], &[0, 0], "reserved");
    assert_eq!(&encoded[12..16], &[0, 0, 0, 0xB8], "header_len 184");

    for (sequence, tag, payload) in [
        (0u32, 1u8, vec![0x01]),
        (1, 2, vec![0xB1; 33]),
        (2, 3, vec![0xB2; 33]),
    ] {
        encoded.extend_from_slice(
            &encode_record(JournalKind::Lineage, sequence, tag, &payload).expect("record"),
        );
    }
    assert_eq!(encoded, golden);

    let decoded = decode_ok(JournalKind::Lineage, &golden);
    assert_eq!(decoded.kind(), JournalKind::Lineage);
    assert_eq!(decoded.row_header(), &lineage_header()[..]);
    assert_eq!(decoded.records().len(), 3);
    assert_eq!(decoded.records()[0].sequence(), 0);
    assert_eq!(decoded.records()[0].phase_tag(), 1);
    assert_eq!(decoded.records()[0].payload(), &[0x01]);
    assert_eq!(decoded.records()[2].payload(), &[0xB2; 33]);
    assert_eq!(decoded.tail(), &TailState::Clean);
    assert!(decoded.is_complete());
    assert_eq!(decoded.journal_common(), Some(common()));
}

#[test]
fn the_golden_cache_frame_is_byte_exact() {
    let golden = golden_cache();

    let mut encoded = encode_header(JournalKind::Cache, &cache_header()).expect("header");
    assert_eq!(encoded.len(), 301);
    assert_eq!(encoded[9], 5, "kind byte");
    assert_eq!(&encoded[12..16], &[0, 0, 0x01, 0x1D], "header_len 285");
    for (sequence, tag, payload) in [
        (0u32, 1u8, vec![0x01]),
        (1, 2, vec![0xC1; 32]),
        (2, 3, vec![0xC2; 128]),
        (3, 4, vec![0xC3; 105]),
        (4, 5, vec![0xC4; 81]),
    ] {
        encoded.extend_from_slice(
            &encode_record(JournalKind::Cache, sequence, tag, &payload).expect("record"),
        );
    }
    assert_eq!(encoded, golden);

    let decoded = decode_ok(JournalKind::Cache, &golden);
    assert_eq!(decoded.records().len(), 5);
    assert!(decoded.is_complete());
    assert_eq!(decoded.journal_common(), Some(common()));
}

#[test]
fn record_encoding_is_the_frozen_layout() {
    let encoded = encode_record(JournalKind::Provision, 1, 2, &[0xAB, 0xCD]).expect("record");
    assert_eq!(encoded, [0, 0, 0, 7, 0, 0, 0, 1, 2, 0xAB, 0xCD, 0, 0, 0, 7]);
}

#[test]
fn a_frame_at_its_exact_ceiling_decodes_and_one_more_byte_refuses() {
    let exact = ceiling_provision();
    let decoded = decode_ok(JournalKind::Provision, &exact);
    assert_eq!(decoded.records().len(), 3);
    assert!(decoded.is_complete());
    assert_eq!(decoded.journal_common(), None);

    let mut over = exact;
    over.push(0);
    assert_eq!(
        decode_frame(JournalKind::Provision, &over),
        Err(FrameCorruption::Oversized { limit: 4096 })
    );
}

#[test]
fn noncanonical_bytes_at_the_ceiling_are_bounded_corruption() {
    let garbage = vec![0x5A; 4096];
    assert_eq!(
        decode_frame(JournalKind::Rebind, &garbage),
        Err(FrameCorruption::BadMagic)
    );
}

#[test]
fn the_ids_ceiling_admits_its_exact_size_and_refuses_the_next_byte() {
    // 16 + 32 header + rec0 (13) + rec1 (13) + rec2 (13 + payload) = 2,101,248.
    let payload_len = 2_101_248 - 16 - 32 - 13 - 13 - 13;
    let mut frame = prefix(1, 32);
    frame.extend_from_slice(&[0x11; 32]);
    frame.extend_from_slice(&record(0, 1, &[]));
    frame.extend_from_slice(&record(1, 2, &[]));
    frame.extend_from_slice(&record(2, 3, &vec![0x42; payload_len]));
    assert_eq!(frame.len(), 2_101_248);
    assert!(decode_frame(JournalKind::Ids, &frame).is_ok());

    frame.push(0);
    assert_eq!(
        decode_frame(JournalKind::Ids, &frame),
        Err(FrameCorruption::Oversized { limit: 2_101_248 })
    );
}

#[test]
fn kinds_one_to_three_may_skip_registry_phases() {
    // The unique-next-record law lets an open-registry row skip a phase (a
    // binding-only transition); tags must only advance strictly.
    let mut frame = prefix(1, 4);
    frame.extend_from_slice(&[0x22; 4]);
    frame.extend_from_slice(&record(0, 1, b"p"));
    frame.extend_from_slice(&record(1, 3, b"q"));
    let decoded = decode_ok(JournalKind::Ids, &frame);
    assert_eq!(decoded.records().len(), 2);
    assert!(decoded.is_complete(), "tag 3 is the Ids terminal phase");
}

#[test]
fn truncated_prefixes_are_corruption() {
    assert_eq!(
        decode_frame(JournalKind::Provision, &[]),
        Err(FrameCorruption::TooShort { found: 0 })
    );
    let golden = golden_lineage();
    assert_eq!(
        decode_frame(JournalKind::Lineage, &golden[..15]),
        Err(FrameCorruption::TooShort { found: 15 })
    );
}

#[test]
fn a_bad_magic_bad_version_or_nonzero_reserved_is_corruption() {
    let golden = golden_lineage();

    let mut bad_magic = golden.clone();
    bad_magic[0] = b'X';
    assert_eq!(
        decode_frame(JournalKind::Lineage, &bad_magic),
        Err(FrameCorruption::BadMagic)
    );

    let mut bad_version = golden.clone();
    bad_version[8] = 1;
    assert_eq!(
        decode_frame(JournalKind::Lineage, &bad_version),
        Err(FrameCorruption::BadVersion { found: 1 })
    );

    let mut reserved = golden.clone();
    reserved[11] = 1;
    assert_eq!(
        decode_frame(JournalKind::Lineage, &reserved),
        Err(FrameCorruption::NonzeroReserved { found: 1 })
    );
}

#[test]
fn an_unknown_or_unexpected_kind_byte_is_corruption() {
    let golden = golden_lineage();

    let mut unknown = golden.clone();
    unknown[9] = 0;
    assert_eq!(
        decode_frame(JournalKind::Lineage, &unknown),
        Err(FrameCorruption::BadKind { found: 0 })
    );
    unknown[9] = 6;
    assert_eq!(
        decode_frame(JournalKind::Lineage, &unknown),
        Err(FrameCorruption::BadKind { found: 6 })
    );

    assert_eq!(
        decode_frame(JournalKind::Cache, &golden),
        Err(FrameCorruption::WrongKind {
            expected: JournalKind::Cache,
            found: JournalKind::Lineage,
        })
    );
}

#[test]
fn a_closed_kind_requires_its_exact_header_length() {
    let mut frame = prefix(4, 183);
    frame.extend_from_slice(&[0xEE; 183]);
    assert_eq!(
        decode_frame(JournalKind::Lineage, &frame),
        Err(FrameCorruption::BadHeaderLength { found: 183 })
    );
}

#[test]
fn an_open_kind_header_is_bounded_by_its_ceiling() {
    let frame = prefix(2, 0xFFFF_FFFF);
    assert_eq!(
        decode_frame(JournalKind::Provision, &frame),
        Err(FrameCorruption::BadHeaderLength { found: 0xFFFF_FFFF })
    );
}

#[test]
fn a_frame_ending_inside_its_header_is_corruption() {
    let mut frame = prefix(2, 100);
    frame.extend_from_slice(&[0x77; 50]);
    assert_eq!(
        decode_frame(JournalKind::Provision, &frame),
        Err(FrameCorruption::HeaderTruncated {
            expected: 100,
            found: 50,
        })
    );
}

#[test]
fn an_impossible_declared_record_length_is_corruption() {
    let mut frame = prefix(2, 4);
    frame.extend_from_slice(&[0x77; 4]);
    frame.extend_from_slice(&4u32.to_be_bytes()); // below the 5-byte base
    assert_eq!(
        decode_frame(JournalKind::Provision, &frame),
        Err(FrameCorruption::BadRecordLength {
            sequence: 0,
            found: 4,
        })
    );

    let mut over = prefix(2, 4);
    over.extend_from_slice(&[0x77; 4]);
    over.extend_from_slice(&0xFFFF_FF00u32.to_be_bytes()); // over the ceiling
    assert_eq!(
        decode_frame(JournalKind::Provision, &over),
        Err(FrameCorruption::BadRecordLength {
            sequence: 0,
            found: 0xFFFF_FF00,
        })
    );
}

#[test]
fn a_mismatched_length_echo_is_corruption() {
    let mut frame = prefix(2, 4);
    frame.extend_from_slice(&[0x77; 4]);
    let mut broken = record(0, 1, b"xy");
    let last = broken.len() - 1;
    broken[last] ^= 0xFF;
    frame.extend_from_slice(&broken);
    assert_eq!(
        decode_frame(JournalKind::Provision, &frame),
        Err(FrameCorruption::LengthEchoMismatch { sequence: 0 })
    );
}

#[test]
fn a_skipped_or_repeated_sequence_is_corruption() {
    let mut skipped = prefix(2, 4);
    skipped.extend_from_slice(&[0x77; 4]);
    skipped.extend_from_slice(&record(0, 1, b"a"));
    skipped.extend_from_slice(&record(2, 2, b"b"));
    assert_eq!(
        decode_frame(JournalKind::Provision, &skipped),
        Err(FrameCorruption::SequenceNotDense {
            expected: 1,
            found: 2,
        })
    );

    let mut repeated = prefix(2, 4);
    repeated.extend_from_slice(&[0x77; 4]);
    repeated.extend_from_slice(&record(0, 1, b"a"));
    repeated.extend_from_slice(&record(0, 2, b"b"));
    assert_eq!(
        decode_frame(JournalKind::Provision, &repeated),
        Err(FrameCorruption::SequenceNotDense {
            expected: 1,
            found: 0,
        })
    );

    let mut first = prefix(2, 4);
    first.extend_from_slice(&[0x77; 4]);
    first.extend_from_slice(&record(1, 1, b"a"));
    assert_eq!(
        decode_frame(JournalKind::Provision, &first),
        Err(FrameCorruption::SequenceNotDense {
            expected: 0,
            found: 1,
        })
    );
}

#[test]
fn tag_law_violations_are_corruption() {
    let mut zero = prefix(2, 4);
    zero.extend_from_slice(&[0x77; 4]);
    zero.extend_from_slice(&record(0, 0, b"a"));
    assert_eq!(
        decode_frame(JournalKind::Provision, &zero),
        Err(FrameCorruption::TagOutOfRegistry {
            sequence: 0,
            found: 0,
        })
    );

    let mut beyond = prefix(2, 4);
    beyond.extend_from_slice(&[0x77; 4]);
    beyond.extend_from_slice(&record(0, 4, b"a"));
    assert_eq!(
        decode_frame(JournalKind::Provision, &beyond),
        Err(FrameCorruption::TagOutOfRegistry {
            sequence: 0,
            found: 4,
        })
    );

    let mut unprepared = prefix(2, 4);
    unprepared.extend_from_slice(&[0x77; 4]);
    unprepared.extend_from_slice(&record(0, 2, b"a"));
    assert_eq!(
        decode_frame(JournalKind::Provision, &unprepared),
        Err(FrameCorruption::FirstTagNotPrepared { found: 2 })
    );

    let mut stalled = prefix(2, 4);
    stalled.extend_from_slice(&[0x77; 4]);
    stalled.extend_from_slice(&record(0, 1, b"a"));
    stalled.extend_from_slice(&record(1, 1, b"b"));
    assert_eq!(
        decode_frame(JournalKind::Provision, &stalled),
        Err(FrameCorruption::TagNotAdvancing {
            sequence: 1,
            previous: 1,
            found: 1,
        })
    );

    // Closed kinds have dense registries: a skip is corruption there.
    let mut sparse = prefix(4, 184);
    sparse.extend_from_slice(&lineage_header());
    sparse.extend_from_slice(&record(0, 1, &[0x01]));
    sparse.extend_from_slice(&record(1, 3, &[0xB1; 33]));
    assert_eq!(
        decode_frame(JournalKind::Lineage, &sparse),
        Err(FrameCorruption::TagNotDense {
            sequence: 1,
            found: 3,
        })
    );
}

#[test]
fn a_closed_kind_record_needs_its_exact_position_size() {
    let mut frame = prefix(4, 184);
    frame.extend_from_slice(&lineage_header());
    frame.extend_from_slice(&record(0, 1, &[0x01, 0x02]));
    assert_eq!(
        decode_frame(JournalKind::Lineage, &frame),
        Err(FrameCorruption::WrongRecordSize {
            sequence: 0,
            expected: 6,
            found: 7,
        })
    );
}

#[test]
fn bytes_after_the_terminal_phase_are_corruption() {
    let mut golden = golden_lineage();
    golden.push(0x00);
    assert_eq!(
        decode_frame(JournalKind::Lineage, &golden),
        Err(FrameCorruption::TrailingBytes { found: 1 })
    );
}

#[test]
fn an_incomplete_final_record_is_a_validated_tail_candidate() {
    let full = golden_lineage();
    // Cut inside record 2 (offset 306 - 20 leaves 26 of its 46 bytes).
    let cut = &full[..306 - 20];
    let decoded = decode_ok(JournalKind::Lineage, cut);
    assert_eq!(decoded.records().len(), 2);
    assert!(!decoded.is_complete());
    match decoded.tail() {
        TailState::IncompletePrefix { bytes } => {
            assert_eq!(bytes.as_slice(), &full[306 - 46..306 - 20]);
        }
        TailState::Clean => panic!("a cut record must be an incomplete tail"),
    }

    // A tail whose visible declared length breaks the kind's law is
    // corruption, not a candidate.
    let mut bad_visible = full[..260].to_vec();
    bad_visible.extend_from_slice(&9u32.to_be_bytes());
    assert_eq!(
        decode_frame(JournalKind::Lineage, &bad_visible),
        Err(FrameCorruption::WrongRecordSize {
            sequence: 2,
            expected: 38,
            found: 9,
        })
    );

    // A tail whose visible sequence field is wrong is corruption.
    let mut bad_sequence = full[..260].to_vec();
    bad_sequence.extend_from_slice(&38u32.to_be_bytes());
    bad_sequence.extend_from_slice(&7u32.to_be_bytes());
    assert_eq!(
        decode_frame(JournalKind::Lineage, &bad_sequence),
        Err(FrameCorruption::SequenceNotDense {
            expected: 2,
            found: 7,
        })
    );

    // A tail whose visible tag is wrong is corruption.
    let mut bad_tag = full[..260].to_vec();
    bad_tag.extend_from_slice(&38u32.to_be_bytes());
    bad_tag.extend_from_slice(&2u32.to_be_bytes());
    bad_tag.push(9);
    assert_eq!(
        decode_frame(JournalKind::Lineage, &bad_tag),
        Err(FrameCorruption::TagOutOfRegistry {
            sequence: 2,
            found: 9,
        })
    );
}

#[test]
fn a_header_only_frame_decodes_with_no_records() {
    // The frame layer reports structure; the journal layer refuses a claimed
    // or pending journal without its Prepared record.
    let mut frame = prefix(2, 4);
    frame.extend_from_slice(&[0x77; 4]);
    let decoded = decode_ok(JournalKind::Provision, &frame);
    assert!(decoded.records().is_empty());
    assert_eq!(decoded.tail(), &TailState::Clean);
    assert!(!decoded.is_complete());
}

#[test]
fn producer_law_refusals_are_typed() {
    assert_eq!(
        encode_header(JournalKind::Lineage, &[0xEE; 183]),
        Err(FrameLawError::WrongHeaderLength {
            kind: JournalKind::Lineage,
            expected: 184,
            found: 183,
        })
    );
    assert_eq!(
        encode_header(JournalKind::Provision, &[0x77; 4096]),
        Err(FrameLawError::HeaderOverCeiling {
            kind: JournalKind::Provision,
            found: 4096,
        })
    );
    assert_eq!(
        encode_record(JournalKind::Provision, 0, 0, b""),
        Err(FrameLawError::TagOutOfRegistry {
            kind: JournalKind::Provision,
            found: 0,
        })
    );
    assert_eq!(
        encode_record(JournalKind::Ids, 0, 4, b""),
        Err(FrameLawError::TagOutOfRegistry {
            kind: JournalKind::Ids,
            found: 4,
        })
    );
    assert_eq!(
        encode_record(JournalKind::Ids, 1, 1, b""),
        Err(FrameLawError::TagBehindSequence {
            sequence: 1,
            found: 1,
        })
    );
    assert_eq!(
        encode_record(JournalKind::Ids, 3, 3, b""),
        Err(FrameLawError::SequenceOutOfRegistry {
            kind: JournalKind::Ids,
            found: 3,
        })
    );
    assert_eq!(
        encode_record(JournalKind::Lineage, 1, 3, &[0xB1; 33]),
        Err(FrameLawError::TagNotDense {
            sequence: 1,
            found: 3,
        })
    );
    assert_eq!(
        encode_record(JournalKind::Lineage, 0, 1, &[0x01, 0x02]),
        Err(FrameLawError::WrongPayloadLength {
            sequence: 0,
            expected: 1,
            found: 2,
        })
    );
    assert_eq!(
        encode_record(JournalKind::Provision, 0, 1, &[0u8; 5000]),
        Err(FrameLawError::PayloadOverCeiling {
            kind: JournalKind::Provision,
            found: 5000,
        })
    );
}
