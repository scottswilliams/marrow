use std::path::PathBuf;

use marrow_json::surface::{
    SurfaceCursorTokenCodec, SurfaceCursorTokenKey, SurfaceCursorTokenKeyId,
};

pub(super) enum CursorTokenKeySource {
    Env(String),
    File(PathBuf),
}

pub(super) struct RemoteCursorToken {
    codec: SurfaceCursorTokenCodec,
}

impl RemoteCursorToken {
    pub(super) fn load(key_id: &str, source: &CursorTokenKeySource) -> Result<Self, String> {
        let key_id = SurfaceCursorTokenKeyId::parse(key_id)
            .map_err(|error| format!("invalid cursor token key id: {}", error.message()))?;
        let key = match source {
            CursorTokenKeySource::Env(name) => {
                let value = std::env::var(name).map_err(|_| {
                    format!("cursor token key environment variable {name} is not set or is empty")
                })?;
                SurfaceCursorTokenKey::from_source_line(&value).map_err(|error| {
                    format!(
                        "invalid cursor token key environment variable: {}",
                        error.message()
                    )
                })?
            }
            CursorTokenKeySource::File(path) => {
                let metadata = std::fs::metadata(path)
                    .map_err(|error| format!("failed to read cursor token key file: {error}"))?;
                if !metadata.is_file() {
                    return Err("cursor token key file must be a regular file".to_string());
                }
                let bytes = std::fs::read(path)
                    .map_err(|error| format!("failed to read cursor token key file: {error}"))?;
                let value = String::from_utf8(bytes)
                    .map_err(|_| "cursor token key file must be UTF-8".to_string())?;
                SurfaceCursorTokenKey::from_source_line(&value).map_err(|error| {
                    format!("invalid cursor token key file: {}", error.message())
                })?
            }
        };
        Ok(Self {
            codec: SurfaceCursorTokenCodec::new(key_id, key),
        })
    }

    pub(super) fn codec(&self) -> &SurfaceCursorTokenCodec {
        &self.codec
    }
}
