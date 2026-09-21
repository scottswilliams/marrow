//! Known-answer tests freezing every field and golden byte of the
//! pending-journal frame, plus the hostile decode matrix. These bytes are a
//! durability contract: a change here is a format break, not a refactor.

use super::*;

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

/// The frozen 16-byte prefix for a header length.
fn prefix(header_len: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"MWPEND0\0");
    bytes.push(0); // version
    bytes.push(1); // kind
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

/// A row header: the 48-byte leading common and a 32-byte tail.
fn header() -> Vec<u8> {
    let mut header = common().encode().to_vec();
    header.extend_from_slice(&[0xEE; 32]);
    assert_eq!(header.len(), 80);
    header
}

/// The complete golden frame: exactly 202 bytes.
fn golden() -> Vec<u8> {
    let mut frame = prefix(80);
    frame.extend_from_slice(&header());
    frame.extend_from_slice(&record(0, 1, &[0x01]));
    frame.extend_from_slice(&record(1, 2, &[0xB1; 33]));
    frame.extend_from_slice(&record(2, 3, &[0xB2; 33]));
    assert_eq!(frame.len(), 202);
    frame
}

fn decode_ok(bytes: &[u8]) -> DecodedFrame {
    decode_frame(bytes).expect("a well-formed frame decodes")
}

/// Whether the frame's last record is the terminal registry phase.
fn complete(frame: &DecodedFrame) -> bool {
    is_terminal(frame.records().last().map_or(0, PhaseRecord::phase_tag))
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
fn the_kind_byte_ceiling_and_registry_are_frozen() {
    assert_eq!(KIND, 1);
    assert_eq!(CEILING, 2_101_248);
    assert_eq!(PHASE_COUNT, 3);
}

#[test]
fn the_golden_frame_is_byte_exact() {
    let golden = golden();

    let mut encoded = encode_header(&header()).expect("header");
    assert_eq!(encoded.len(), 96);
    assert_eq!(&encoded[0..8], b"MWPEND0\0");
    assert_eq!(encoded[8], 0, "version");
    assert_eq!(encoded[9], 1, "kind byte");
    assert_eq!(&encoded[10..12], &[0, 0], "reserved");
    assert_eq!(&encoded[12..16], &[0, 0, 0, 0x50], "header_len 80");

    for (sequence, tag, payload) in [
        (0u32, 1u8, vec![0x01]),
        (1, 2, vec![0xB1; 33]),
        (2, 3, vec![0xB2; 33]),
    ] {
        encoded.extend_from_slice(&encode_record(sequence, tag, &payload).expect("record"));
    }
    assert_eq!(encoded, golden);

    let decoded = decode_ok(&golden);
    assert_eq!(decoded.row_header(), &header()[..]);
    assert_eq!(decoded.records().len(), 3);
    assert_eq!(decoded.records()[0].phase_tag(), 1);
    assert_eq!(decoded.records()[0].payload(), &[0x01]);
    assert_eq!(decoded.records()[2].payload(), &[0xB2; 33]);
    assert_eq!(decoded.tail(), &TailState::Clean);
    assert!(complete(&decoded));
    let leading: &[u8; JOURNAL_COMMON_LEN] = decoded.row_header()[..JOURNAL_COMMON_LEN]
        .try_into()
        .expect("the header begins with the 48-byte common");
    assert_eq!(JournalCommon::decode(leading), common());
}

#[test]
fn record_encoding_is_the_frozen_layout() {
    let encoded = encode_record(1, 2, &[0xAB, 0xCD]).expect("record");
    assert_eq!(encoded, [0, 0, 0, 7, 0, 0, 0, 1, 2, 0xAB, 0xCD, 0, 0, 0, 7]);
}

#[test]
fn a_frame_at_its_exact_ceiling_decodes_and_one_more_byte_refuses() {
    // 16 + 32 header + rec0 (13) + rec1 (13) + rec2 (13 + payload) = the ceiling.
    let payload_len = CEILING - 16 - 32 - 13 - 13 - 13;
    let mut frame = prefix(32);
    frame.extend_from_slice(&[0x11; 32]);
    frame.extend_from_slice(&record(0, 1, &[]));
    frame.extend_from_slice(&record(1, 2, &[]));
    frame.extend_from_slice(&record(2, 3, &vec![0x42; payload_len]));
    assert_eq!(frame.len(), CEILING);
    assert!(complete(&decode_ok(&frame)));

    frame.push(0);
    assert_eq!(decode_frame(&frame), Err(FrameCorruption::Oversized));
}

#[test]
fn noncanonical_bytes_at_the_ceiling_are_bounded_corruption() {
    let garbage = vec![0x5A; CEILING];
    assert_eq!(decode_frame(&garbage), Err(FrameCorruption::BadMagic));
}

#[test]
fn registry_phases_may_be_skipped() {
    // The unique-next-record law lets a row skip a phase; tags must only
    // advance strictly.
    let mut frame = prefix(4);
    frame.extend_from_slice(&[0x22; 4]);
    frame.extend_from_slice(&record(0, 1, b"p"));
    frame.extend_from_slice(&record(1, 3, b"q"));
    let decoded = decode_ok(&frame);
    assert_eq!(decoded.records().len(), 2);
    assert!(complete(&decoded), "tag 3 is the terminal phase");
}

#[test]
fn truncated_prefixes_are_corruption() {
    assert_eq!(
        decode_frame(&[]),
        Err(FrameCorruption::TooShort { found: 0 })
    );
    assert_eq!(
        decode_frame(&golden()[..15]),
        Err(FrameCorruption::TooShort { found: 15 })
    );
}

#[test]
fn a_bad_magic_bad_version_or_nonzero_reserved_is_corruption() {
    let golden = golden();

    let mut bad_magic = golden.clone();
    bad_magic[0] = b'X';
    assert_eq!(decode_frame(&bad_magic), Err(FrameCorruption::BadMagic));

    let mut bad_version = golden.clone();
    bad_version[8] = 1;
    assert_eq!(
        decode_frame(&bad_version),
        Err(FrameCorruption::BadVersion { found: 1 })
    );

    let mut reserved = golden.clone();
    reserved[11] = 1;
    assert_eq!(
        decode_frame(&reserved),
        Err(FrameCorruption::NonzeroReserved { found: 1 })
    );
}

/// Every byte other than the one kind is refused, including the codes of the
/// kinds that once existed.
#[test]
fn a_kind_byte_naming_another_kind_is_corruption() {
    let mut frame = golden();
    for found in [0, 2, 3, 4, 5, 0xFF] {
        frame[9] = found;
        assert_eq!(
            decode_frame(&frame),
            Err(FrameCorruption::BadKind { found })
        );
    }
}

#[test]
fn a_header_is_bounded_by_the_ceiling() {
    let frame = prefix(0xFFFF_FFFF);
    assert_eq!(
        decode_frame(&frame),
        Err(FrameCorruption::BadHeaderLength { found: 0xFFFF_FFFF })
    );
}

#[test]
fn a_frame_ending_inside_its_header_is_corruption() {
    let mut frame = prefix(100);
    frame.extend_from_slice(&[0x77; 50]);
    assert_eq!(
        decode_frame(&frame),
        Err(FrameCorruption::HeaderTruncated {
            expected: 100,
            found: 50,
        })
    );
}

#[test]
fn an_impossible_declared_record_length_is_corruption() {
    let mut frame = prefix(4);
    frame.extend_from_slice(&[0x77; 4]);
    frame.extend_from_slice(&4u32.to_be_bytes()); // below the 5-byte base
    assert_eq!(
        decode_frame(&frame),
        Err(FrameCorruption::BadRecordLength {
            sequence: 0,
            found: 4,
        })
    );

    let mut over = prefix(4);
    over.extend_from_slice(&[0x77; 4]);
    over.extend_from_slice(&0xFFFF_FF00u32.to_be_bytes()); // over the ceiling
    assert_eq!(
        decode_frame(&over),
        Err(FrameCorruption::Record {
            sequence: 0,
            law: RecordLaw::OverCeiling {
                end: 0xFFFF_FF00 - 5 + 13 + 20,
            },
        })
    );
}

#[test]
fn a_mismatched_length_echo_is_corruption() {
    let mut frame = prefix(4);
    frame.extend_from_slice(&[0x77; 4]);
    let mut broken = record(0, 1, b"xy");
    let last = broken.len() - 1;
    broken[last] ^= 0xFF;
    frame.extend_from_slice(&broken);
    assert_eq!(
        decode_frame(&frame),
        Err(FrameCorruption::LengthEchoMismatch { sequence: 0 })
    );
}

#[test]
fn a_skipped_or_repeated_sequence_is_corruption() {
    let mut skipped = prefix(4);
    skipped.extend_from_slice(&[0x77; 4]);
    skipped.extend_from_slice(&record(0, 1, b"a"));
    skipped.extend_from_slice(&record(2, 2, b"b"));
    assert_eq!(
        decode_frame(&skipped),
        Err(FrameCorruption::SequenceNotDense {
            expected: 1,
            found: 2,
        })
    );

    let mut repeated = prefix(4);
    repeated.extend_from_slice(&[0x77; 4]);
    repeated.extend_from_slice(&record(0, 1, b"a"));
    repeated.extend_from_slice(&record(0, 2, b"b"));
    assert_eq!(
        decode_frame(&repeated),
        Err(FrameCorruption::SequenceNotDense {
            expected: 1,
            found: 0,
        })
    );

    let mut first = prefix(4);
    first.extend_from_slice(&[0x77; 4]);
    first.extend_from_slice(&record(1, 1, b"a"));
    assert_eq!(
        decode_frame(&first),
        Err(FrameCorruption::SequenceNotDense {
            expected: 0,
            found: 1,
        })
    );
}

#[test]
fn tag_law_violations_are_corruption() {
    let mut zero = prefix(4);
    zero.extend_from_slice(&[0x77; 4]);
    zero.extend_from_slice(&record(0, 0, b"a"));
    assert_eq!(
        decode_frame(&zero),
        Err(FrameCorruption::Record {
            sequence: 0,
            law: RecordLaw::TagOutOfRegistry { found: 0 },
        })
    );

    let mut beyond = prefix(4);
    beyond.extend_from_slice(&[0x77; 4]);
    beyond.extend_from_slice(&record(0, 4, b"a"));
    assert_eq!(
        decode_frame(&beyond),
        Err(FrameCorruption::Record {
            sequence: 0,
            law: RecordLaw::TagOutOfRegistry { found: 4 },
        })
    );

    let mut unprepared = prefix(4);
    unprepared.extend_from_slice(&[0x77; 4]);
    unprepared.extend_from_slice(&record(0, 2, b"a"));
    assert_eq!(
        decode_frame(&unprepared),
        Err(FrameCorruption::Record {
            sequence: 0,
            law: RecordLaw::FirstTagNotPrepared { found: 2 },
        })
    );

    let mut stalled = prefix(4);
    stalled.extend_from_slice(&[0x77; 4]);
    stalled.extend_from_slice(&record(0, 1, b"a"));
    stalled.extend_from_slice(&record(1, 1, b"b"));
    assert_eq!(
        decode_frame(&stalled),
        Err(FrameCorruption::Record {
            sequence: 1,
            law: RecordLaw::TagNotAdvancing {
                previous: 1,
                found: 1,
            },
        })
    );
}

#[test]
fn bytes_after_the_terminal_phase_are_corruption() {
    let mut golden = golden();
    golden.push(0x00);
    assert_eq!(
        decode_frame(&golden),
        Err(FrameCorruption::TrailingBytes { found: 1 })
    );
}

#[test]
fn an_incomplete_final_record_is_a_validated_tail_candidate() {
    let full = golden();
    // Cut inside record 2 (offset 202 - 20 leaves 26 of its 46 bytes).
    let cut = &full[..202 - 20];
    let decoded = decode_ok(cut);
    assert_eq!(decoded.records().len(), 2);
    assert!(!complete(&decoded));
    match decoded.tail() {
        TailState::IncompletePrefix { bytes } => {
            assert_eq!(bytes.as_slice(), &full[202 - 46..202 - 20]);
        }
        TailState::Clean => panic!("a cut record must be an incomplete tail"),
    }

    // A tail whose visible declared length breaks the record law is
    // corruption, not a candidate.
    let mut bad_visible = full[..156].to_vec();
    bad_visible.extend_from_slice(&4u32.to_be_bytes());
    assert_eq!(
        decode_frame(&bad_visible),
        Err(FrameCorruption::BadRecordLength {
            sequence: 2,
            found: 4,
        })
    );

    // A tail whose visible sequence field is wrong is corruption.
    let mut bad_sequence = full[..156].to_vec();
    bad_sequence.extend_from_slice(&38u32.to_be_bytes());
    bad_sequence.extend_from_slice(&7u32.to_be_bytes());
    assert_eq!(
        decode_frame(&bad_sequence),
        Err(FrameCorruption::SequenceNotDense {
            expected: 2,
            found: 7,
        })
    );

    // A tail whose visible tag is wrong is corruption.
    let mut bad_tag = full[..156].to_vec();
    bad_tag.extend_from_slice(&38u32.to_be_bytes());
    bad_tag.extend_from_slice(&2u32.to_be_bytes());
    bad_tag.push(9);
    assert_eq!(
        decode_frame(&bad_tag),
        Err(FrameCorruption::Record {
            sequence: 2,
            law: RecordLaw::TagOutOfRegistry { found: 9 },
        })
    );
}

#[test]
fn a_header_only_frame_decodes_with_no_records() {
    // The frame layer reports structure; the journal layer refuses a claimed
    // or pending journal without its Prepared record.
    let mut frame = prefix(4);
    frame.extend_from_slice(&[0x77; 4]);
    let decoded = decode_ok(&frame);
    assert!(decoded.records().is_empty());
    assert_eq!(decoded.tail(), &TailState::Clean);
    assert!(!complete(&decoded));
}

#[test]
fn producer_law_refusals_are_typed() {
    assert_eq!(
        encode_header(&vec![0x77; CEILING]),
        Err(FrameLawError::HeaderOverCeiling { found: CEILING })
    );
    assert_eq!(
        encode_record(0, 0, b""),
        Err(FrameLawError::Record {
            sequence: 0,
            law: RecordLaw::TagOutOfRegistry { found: 0 },
        })
    );
    assert_eq!(
        encode_record(0, 4, b""),
        Err(FrameLawError::Record {
            sequence: 0,
            law: RecordLaw::TagOutOfRegistry { found: 4 },
        })
    );
    assert_eq!(
        encode_record(1, 1, b""),
        Err(FrameLawError::Record {
            sequence: 1,
            law: RecordLaw::TagNotAdvancing {
                previous: 1,
                found: 1,
            },
        })
    );
    assert_eq!(
        encode_record(3, 3, b""),
        Err(FrameLawError::Record {
            sequence: 3,
            law: RecordLaw::SequenceOutOfRegistry,
        })
    );
    assert_eq!(
        encode_record(0, 1, &vec![0u8; CEILING]),
        Err(FrameLawError::Record {
            sequence: 0,
            law: RecordLaw::OverCeiling {
                end: 16 + 13 + CEILING,
            },
        })
    );
}
