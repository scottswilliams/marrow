use base64ct::{Base64UrlUnpadded, Encoding};
use chacha20poly1305::aead::{Aead, Generate, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

use super::SurfaceCursorJson;

pub const SURFACE_CURSOR_TOKEN_PROFILE_VERSION: &str = "surface.cursor_token.v1";

const TOKEN_PREFIX: &str = "mct1";
const NONCE_BYTES: usize = 24;
const KEY_BYTES: usize = 32;
const MAX_TOKEN_BYTES: usize = 4096;
const MAX_PLAINTEXT_BYTES: usize = 2048;
const AAD_MODE: &str = "surface.cursor_token.http.page";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceCursorTokenKeyId {
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceCursorTokenKey {
    bytes: [u8; KEY_BYTES],
}

#[derive(Debug, Clone)]
pub struct SurfaceCursorTokenCodec {
    key_id: SurfaceCursorTokenKeyId,
    key: SurfaceCursorTokenKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceCursorTokenError {
    kind: SurfaceCursorTokenErrorKind,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceCursorTokenErrorKind {
    Key,
    Cursor,
    StaleCursor,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorTokenPlaintext {
    profile_version: String,
    cursor: SurfaceCursorJson,
}

impl SurfaceCursorTokenKeyId {
    pub fn parse(value: &str) -> Result<Self, SurfaceCursorTokenError> {
        if value.is_empty()
            || value.len() > 32
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(SurfaceCursorTokenError::key(
                "cursor token key id must be 1-32 characters of A-Z, a-z, 0-9, '_' or '-'",
            ));
        }
        Ok(Self {
            value: value.to_string(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }
}

impl SurfaceCursorTokenKey {
    pub fn from_source_line(value: &str) -> Result<Self, SurfaceCursorTokenError> {
        let value = strip_one_line_ending(value);
        if value.is_empty() {
            return Err(SurfaceCursorTokenError::key(
                "cursor token key source must not be empty",
            ));
        }
        if value.trim() != value || value.contains(['\r', '\n']) {
            return Err(SurfaceCursorTokenError::key(
                "cursor token key source must be one line with no leading or trailing whitespace",
            ));
        }
        let bytes = decode_canonical_base64url(value, SurfaceCursorTokenErrorKind::Key)?;
        let bytes: [u8; KEY_BYTES] = bytes.try_into().map_err(|_| {
            SurfaceCursorTokenError::key("cursor token key must decode to exactly 32 bytes")
        })?;
        Ok(Self { bytes })
    }

    fn as_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.bytes
    }
}

impl SurfaceCursorTokenCodec {
    pub fn new(key_id: SurfaceCursorTokenKeyId, key: SurfaceCursorTokenKey) -> Self {
        Self { key_id, key }
    }

    pub fn key_id(&self) -> &SurfaceCursorTokenKeyId {
        &self.key_id
    }

    pub fn encode(
        &self,
        operation_tag: &str,
        cursor: &SurfaceCursorJson,
    ) -> Result<String, SurfaceCursorTokenError> {
        let plaintext = CursorTokenPlaintext {
            profile_version: SURFACE_CURSOR_TOKEN_PROFILE_VERSION.to_string(),
            cursor: cursor.clone(),
        };
        let plaintext = serde_json::to_vec(&plaintext).map_err(|_| {
            SurfaceCursorTokenError::cursor("surface cursor token plaintext could not be encoded")
        })?;
        if plaintext.len() > MAX_PLAINTEXT_BYTES {
            return Err(SurfaceCursorTokenError::cursor(
                "surface cursor token plaintext is too large",
            ));
        }

        let cipher = XChaCha20Poly1305::new_from_slice(self.key.as_bytes())
            .expect("cursor token keys are exactly 32 bytes");
        let nonce = XNonce::generate();
        let aad = token_aad(self.key_id.as_str(), operation_tag);
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| SurfaceCursorTokenError::cursor("surface cursor could not be sealed"))?;
        let token = format!(
            "{TOKEN_PREFIX}.{}.{}.{}",
            self.key_id.as_str(),
            Base64UrlUnpadded::encode_string(&nonce),
            Base64UrlUnpadded::encode_string(&ciphertext)
        );
        if token.len() > MAX_TOKEN_BYTES {
            return Err(SurfaceCursorTokenError::cursor(
                "surface cursor token is too large",
            ));
        }
        Ok(token)
    }

    pub fn decode(
        &self,
        operation_tag: &str,
        token: &str,
    ) -> Result<SurfaceCursorJson, SurfaceCursorTokenError> {
        if token.len() > MAX_TOKEN_BYTES {
            return Err(SurfaceCursorTokenError::cursor(
                "surface cursor token is too large",
            ));
        }
        let mut parts = token.split('.');
        let prefix = parts.next();
        let key_id = parts.next();
        let nonce = parts.next();
        let ciphertext = parts.next();
        if prefix != Some(TOKEN_PREFIX)
            || key_id != Some(self.key_id.as_str())
            || parts.next().is_some()
        {
            return Err(SurfaceCursorTokenError::cursor(
                "surface cursor token is malformed",
            ));
        }
        let nonce = decode_token_part(nonce)?;
        let nonce: [u8; NONCE_BYTES] = nonce.try_into().map_err(|_| {
            SurfaceCursorTokenError::cursor("surface cursor token nonce is malformed")
        })?;
        let ciphertext = decode_token_part(ciphertext)?;
        let cipher = XChaCha20Poly1305::new_from_slice(self.key.as_bytes())
            .expect("cursor token keys are exactly 32 bytes");
        let aad = token_aad(self.key_id.as_str(), operation_tag);
        let plaintext = cipher
            .decrypt(
                (&nonce[..])
                    .try_into()
                    .expect("cursor token nonces are exactly 24 bytes"),
                Payload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| {
                SurfaceCursorTokenError::cursor("surface cursor token could not be opened")
            })?;
        if plaintext.len() > MAX_PLAINTEXT_BYTES {
            return Err(SurfaceCursorTokenError::cursor(
                "surface cursor token plaintext is too large",
            ));
        }
        let plaintext: CursorTokenPlaintext = serde_json::from_slice(&plaintext).map_err(|_| {
            SurfaceCursorTokenError::cursor("surface cursor token plaintext is malformed")
        })?;
        if plaintext.profile_version != SURFACE_CURSOR_TOKEN_PROFILE_VERSION {
            return Err(SurfaceCursorTokenError::stale_cursor(
                "surface cursor token profile version is not active",
            ));
        }
        Ok(plaintext.cursor)
    }
}

impl SurfaceCursorTokenError {
    pub fn kind(&self) -> SurfaceCursorTokenErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    fn key(message: impl Into<String>) -> Self {
        Self {
            kind: SurfaceCursorTokenErrorKind::Key,
            message: message.into(),
        }
    }

    fn cursor(message: impl Into<String>) -> Self {
        Self {
            kind: SurfaceCursorTokenErrorKind::Cursor,
            message: message.into(),
        }
    }

    fn stale_cursor(message: impl Into<String>) -> Self {
        Self {
            kind: SurfaceCursorTokenErrorKind::StaleCursor,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SurfaceCursorTokenError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SurfaceCursorTokenError {}

fn strip_one_line_ending(value: &str) -> &str {
    value
        .strip_suffix("\r\n")
        .or_else(|| value.strip_suffix('\n'))
        .unwrap_or(value)
}

fn decode_token_part(value: Option<&str>) -> Result<Vec<u8>, SurfaceCursorTokenError> {
    let Some(value) = value else {
        return Err(SurfaceCursorTokenError::cursor(
            "surface cursor token is malformed",
        ));
    };
    decode_canonical_base64url(value, SurfaceCursorTokenErrorKind::Cursor)
}

fn decode_canonical_base64url(
    value: &str,
    kind: SurfaceCursorTokenErrorKind,
) -> Result<Vec<u8>, SurfaceCursorTokenError> {
    let bytes = Base64UrlUnpadded::decode_vec(value)
        .map_err(|_| token_decode_error(kind, "base64url value is malformed"))?;
    if Base64UrlUnpadded::encode_string(&bytes) != value {
        return Err(token_decode_error(
            kind,
            "base64url value must be canonical and unpadded",
        ));
    }
    Ok(bytes)
}

fn token_decode_error(kind: SurfaceCursorTokenErrorKind, message: &str) -> SurfaceCursorTokenError {
    match kind {
        SurfaceCursorTokenErrorKind::Key => SurfaceCursorTokenError::key(message),
        SurfaceCursorTokenErrorKind::Cursor => SurfaceCursorTokenError::cursor(message),
        SurfaceCursorTokenErrorKind::StaleCursor => SurfaceCursorTokenError::stale_cursor(message),
    }
}

fn token_aad(key_id: &str, operation_tag: &str) -> Vec<u8> {
    let mut aad = Vec::new();
    aad_part(&mut aad, SURFACE_CURSOR_TOKEN_PROFILE_VERSION.as_bytes());
    aad_part(&mut aad, key_id.as_bytes());
    aad_part(&mut aad, operation_tag.as_bytes());
    aad_part(&mut aad, AAD_MODE.as_bytes());
    aad
}

fn aad_part(aad: &mut Vec<u8>, part: &[u8]) {
    let len = u32::try_from(part.len()).expect("AAD parts fit in u32");
    aad.extend_from_slice(&len.to_be_bytes());
    aad.extend_from_slice(part);
}
