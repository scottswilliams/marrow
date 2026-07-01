use std::path::PathBuf;

pub(super) enum AuthTokenSource {
    Env(String),
    File(PathBuf),
}

pub(super) struct RemoteAuthToken {
    token: String,
}

impl RemoteAuthToken {
    pub(super) fn load(source: &AuthTokenSource) -> Result<Self, String> {
        let token = match source {
            AuthTokenSource::Env(name) => {
                let value = std::env::var(name).map_err(|_| {
                    format!("auth token environment variable {name} is not set or is empty")
                })?;
                validate_token("auth token environment variable", &value)?
            }
            AuthTokenSource::File(path) => {
                let metadata = std::fs::metadata(path)
                    .map_err(|error| format!("failed to read auth token file: {error}"))?;
                if !metadata.is_file() {
                    return Err("auth token file must be a regular file".to_string());
                }
                let bytes = std::fs::read(path)
                    .map_err(|error| format!("failed to read auth token file: {error}"))?;
                let value = String::from_utf8(bytes)
                    .map_err(|_| "auth token file must be UTF-8".to_string())?;
                validate_token("auth token file", &value)?
            }
        };
        Ok(Self { token })
    }

    pub(super) fn matches_bearer(&self, value: &str) -> bool {
        let Some(token) = value.strip_prefix("Bearer ") else {
            return false;
        };
        constant_time_eq(token.as_bytes(), self.token.as_bytes())
    }
}

fn validate_token(source: &str, value: &str) -> Result<String, String> {
    let token = strip_one_line_ending(value);
    if token.is_empty() {
        return Err(format!("{source} must not be empty"));
    }
    if token.trim() != token || token.contains(['\r', '\n']) {
        return Err(format!(
            "{source} must be one line with no leading or trailing whitespace"
        ));
    }
    Ok(token.to_string())
}

fn strip_one_line_ending(value: &str) -> &str {
    value
        .strip_suffix("\r\n")
        .or_else(|| value.strip_suffix('\n'))
        .unwrap_or(value)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = left.len() ^ right.len();
    let len = left.len().max(right.len());
    for index in 0..len {
        let left = left.get(index).copied().unwrap_or(0);
        let right = right.get(index).copied().unwrap_or(0);
        diff |= usize::from(left ^ right);
    }
    diff == 0
}
