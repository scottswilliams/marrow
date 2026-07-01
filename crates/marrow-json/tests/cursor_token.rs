use marrow_json::surface::{
    SurfaceCursorBoundaryJson, SurfaceCursorJson, SurfaceCursorTokenCodec,
    SurfaceCursorTokenErrorKind, SurfaceCursorTokenKey, SurfaceCursorTokenKeyId,
};

const VALID_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn sample_cursor(operation_tag: &str) -> SurfaceCursorJson {
    SurfaceCursorJson {
        operation_tag: operation_tag.to_string(),
        store_uid: "018f3f86-4f69-7d8f-b9d6-6a64f47db5a1".to_string(),
        commit_id: Some(42),
        catalog_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string(),
        source_digest: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .to_string(),
        engine_profile_digest: "0123456789abcdef".to_string(),
        boundary: SurfaceCursorBoundaryJson::RootIdentity {
            identity: marrow_json::surface::SurfaceIdentityJson {
                store_catalog_id: "cat_books".to_string(),
                keys: vec![marrow_json::surface::SurfaceKeyJson::Int {
                    value: "1".to_string(),
                }],
            },
        },
    }
}

#[test]
fn cursor_token_key_id_accepts_only_short_url_safe_identifiers() {
    assert_eq!(
        SurfaceCursorTokenKeyId::parse("kid_1-ABC")
            .expect("valid key id")
            .as_str(),
        "kid_1-ABC"
    );

    for value in ["", "x.y", "with space", "ümlaut", "a".repeat(33).as_str()] {
        let error = SurfaceCursorTokenKeyId::parse(value).expect_err("invalid key id");
        assert_eq!(error.kind(), SurfaceCursorTokenErrorKind::Key);
    }
}

#[test]
fn cursor_token_key_source_line_matches_remote_auth_token_grammar() {
    assert!(SurfaceCursorTokenKey::from_source_line(VALID_KEY).is_ok());
    assert!(SurfaceCursorTokenKey::from_source_line(&format!("{VALID_KEY}\n")).is_ok());
    assert!(SurfaceCursorTokenKey::from_source_line(&format!("{VALID_KEY}\r\n")).is_ok());

    for value in [
        format!(" {VALID_KEY}"),
        format!("{VALID_KEY} "),
        format!("{VALID_KEY}\n\n"),
        format!("{VALID_KEY}\r"),
        format!("{VALID_KEY}="),
    ] {
        let error = SurfaceCursorTokenKey::from_source_line(&value).expect_err("invalid key line");
        assert_eq!(error.kind(), SurfaceCursorTokenErrorKind::Key);
    }
}

#[test]
fn cursor_token_key_must_decode_to_exactly_thirty_two_bytes() {
    let too_short = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let too_long = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    for value in [too_short, too_long] {
        let error = SurfaceCursorTokenKey::from_source_line(value).expect_err("invalid key length");
        assert_eq!(error.kind(), SurfaceCursorTokenErrorKind::Key);
    }
}

#[test]
fn cursor_token_round_trips_with_canonical_public_shape() {
    let codec = token_codec("kid-1");
    let cursor = sample_cursor("op-page");

    let token = codec.encode("op-page", &cursor).expect("encode token");

    assert!(token.starts_with("mct1.kid-1."), "{token}");
    assert!(!token.contains('='), "{token}");
    assert!(token.len() <= 4096, "{token}");
    assert_eq!(
        codec.decode("op-page", &token).expect("decode token"),
        cursor
    );
}

#[test]
fn cursor_token_rejects_tamper_and_aad_operation_mismatch_as_surface_cursor() {
    let codec = token_codec("kid-1");
    let cursor = sample_cursor("op-page");
    let token = codec.encode("op-page", &cursor).expect("encode token");

    let mut tampered = token.clone().into_bytes();
    let last = tampered.last_mut().expect("token has bytes");
    *last = if *last == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(tampered).expect("token remains utf8");

    for value in [tampered.as_str(), token.as_str()] {
        let error = codec
            .decode("other-op", value)
            .expect_err("wrong AAD must not decrypt as a cursor");
        assert_eq!(error.kind(), SurfaceCursorTokenErrorKind::Cursor);
    }
}

#[test]
fn cursor_token_rejects_wrong_key_id_and_noncanonical_token_parts() {
    let codec = token_codec("kid-1");
    let token = codec
        .encode("op-page", &sample_cursor("op-page"))
        .expect("encode token");
    let wrong_kid = token.replacen("mct1.kid-1.", "mct1.kid-2.", 1);
    let padded_nonce = token.replacen('.', ".AA=", 1);

    for value in [
        wrong_kid.as_str(),
        padded_nonce.as_str(),
        "mct1.kid-1.bad.bad",
    ] {
        let error = codec.decode("op-page", value).expect_err("invalid token");
        assert_eq!(error.kind(), SurfaceCursorTokenErrorKind::Cursor);
    }
}

#[test]
fn cursor_token_enforces_plaintext_and_token_size_limits() {
    let codec = token_codec("kid-1");
    let mut cursor = sample_cursor("op-page");
    cursor.source_digest = format!("sha256:{}", "b".repeat(3000));

    let error = codec
        .encode("op-page", &cursor)
        .expect_err("oversized plaintext is refused");
    assert_eq!(error.kind(), SurfaceCursorTokenErrorKind::Cursor);

    let oversized = format!("mct1.kid-1.{}.{}", "A".repeat(32), "A".repeat(4096));
    let error = codec
        .decode("op-page", &oversized)
        .expect_err("oversized token is refused");
    assert_eq!(error.kind(), SurfaceCursorTokenErrorKind::Cursor);
}

fn token_codec(kid: &str) -> SurfaceCursorTokenCodec {
    SurfaceCursorTokenCodec::new(
        SurfaceCursorTokenKeyId::parse(kid).expect("valid key id"),
        SurfaceCursorTokenKey::from_source_line(VALID_KEY).expect("valid key"),
    )
}
