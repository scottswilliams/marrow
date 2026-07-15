//! Bounded, seeded, deterministic fuzz over the canonical temporal text codec.
//!
//! Two properties over the reusable oracle: a parser never panics on arbitrary bytes
//! (it returns `None` or a value), and the codec round-trips — formatting a raw
//! scalar and parsing it back yields the original, and re-formatting a parsed value
//! reproduces the canonical text. No external fuzz dependency; a fixed iteration
//! budget keeps it in the default suite. A minimized counterexample becomes a fixture.

use marrow_temporal::{
    SUPPORTED_DATE_MAX_DAYS, SUPPORTED_DATE_MIN_DAYS, SUPPORTED_INSTANT_MAX_NANOS,
    SUPPORTED_INSTANT_MIN_NANOS, format_date, format_duration, format_instant, parse_date,
    parse_duration, parse_instant,
};

/// A tiny deterministic xorshift RNG (no external dependency).
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn byte(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }
}

fn seed() -> u64 {
    std::env::var("MARROW_FUZZ_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0x2026_0715_C04D_A7E5)
}

/// The parsers must never panic and never allocate unboundedly: every parse of
/// arbitrary bytes returns `None` or a value, and any accepted value re-formats to
/// exactly the input text (canonical inputs are the only accepted ones).
fn parse_oracle(bytes: &[u8]) {
    if let Some(days) = parse_date(bytes) {
        assert!(marrow_temporal::supported_date_days(days));
        assert_eq!(format_date(days).as_deref().map(str::as_bytes), Some(bytes));
    }
    if let Some(nanos) = parse_instant(bytes) {
        assert!(marrow_temporal::supported_instant_nanos(nanos));
        assert_eq!(
            format_instant(nanos).as_deref().map(str::as_bytes),
            Some(bytes)
        );
    }
    if let Some(nanos) = parse_duration(bytes) {
        assert_eq!(format_duration(nanos).as_bytes(), bytes);
    }
}

#[test]
fn parsers_never_panic_on_arbitrary_bytes() {
    let mut rng = Rng(seed());
    // A mix of purely random bytes and near-canonical mutations (digits, separators),
    // so the driver exercises both the reject paths and the fields of real forms.
    let templates: [&[u8]; 3] = [b"2026-07-15", b"2026-07-15T12:34:56.789Z", b"-PT12345.678S"];
    for _ in 0..50_000 {
        let len = (rng.byte() % 30) as usize;
        let mut bytes: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
        parse_oracle(&bytes);

        // A mutated near-canonical form: flip one byte of a template.
        let template = templates[(rng.byte() as usize) % templates.len()];
        bytes = template.to_vec();
        if !bytes.is_empty() {
            let pos = (rng.byte() as usize) % bytes.len();
            bytes[pos] = rng.byte();
        }
        parse_oracle(&bytes);
    }
}

#[test]
fn raw_scalars_round_trip_through_canonical_text() {
    let mut rng = Rng(seed());
    for _ in 0..50_000 {
        // A supported date/instant and an arbitrary duration each format and parse
        // back to the original.
        let span = (SUPPORTED_DATE_MAX_DAYS as i64 - SUPPORTED_DATE_MIN_DAYS as i64) as u64;
        let days = SUPPORTED_DATE_MIN_DAYS + (rng.next_u64() % span) as i32;
        let text = format_date(days).expect("supported date formats");
        assert_eq!(parse_date(text.as_bytes()), Some(days));

        let ispan = (SUPPORTED_INSTANT_MAX_NANOS - SUPPORTED_INSTANT_MIN_NANOS) as u128;
        let nanos = SUPPORTED_INSTANT_MIN_NANOS
            + (((rng.next_u64() as u128) << 64 | rng.next_u64() as u128) % ispan) as i128;
        let text = format_instant(nanos).expect("supported instant formats");
        assert_eq!(parse_instant(text.as_bytes()), Some(nanos));

        let dur = (rng.next_u64() as i128) << 64 | rng.next_u64() as i128;
        let text = format_duration(dur);
        assert_eq!(parse_duration(text.as_bytes()), Some(dur));
    }
}
