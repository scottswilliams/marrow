pub(super) struct CorsPolicy {
    origin: String,
}

impl CorsPolicy {
    pub(super) fn new(origin: &str) -> Result<Self, &'static str> {
        Ok(Self {
            origin: normalize_loopback_origin(origin)?,
        })
    }

    /// Returns the configured origin when the request origin matches it after
    /// loopback normalization. The configured spelling is always emitted so the
    /// `Access-Control-Allow-Origin` header never reflects request casing or
    /// whitespace.
    pub(super) fn matched_origin(&self, origin: &str) -> Option<&str> {
        normalize_loopback_origin(origin)
            .is_ok_and(|origin| origin == self.origin)
            .then_some(self.origin.as_str())
    }
}

fn normalize_loopback_origin(origin: &str) -> Result<String, &'static str> {
    if origin.trim() != origin {
        return Err("--cors-origin must be a loopback origin without surrounding whitespace");
    }
    let Some((scheme, rest)) = origin.split_once("://") else {
        return Err("--cors-origin must be a loopback origin such as http://localhost:5173");
    };
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return Err("--cors-origin must use http or https");
    }
    if rest.is_empty() || rest.contains('/') || rest.contains('?') || rest.contains('#') {
        return Err("--cors-origin must be an origin, not a URL path");
    }

    let (host, port) = parse_origin_host_port(rest)?;
    if !matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]") {
        return Err("--cors-origin must use a loopback origin");
    }
    Ok(format!("{scheme}://{host}{port}"))
}

fn parse_origin_host_port(rest: &str) -> Result<(String, String), &'static str> {
    if let Some(after_bracket) = rest.strip_prefix('[') {
        let Some(end) = after_bracket.find(']') else {
            return Err("--cors-origin has a malformed IPv6 host");
        };
        let host = &after_bracket[..end];
        let remainder = &after_bracket[end + 1..];
        let port = parse_origin_port(remainder)?;
        return match host {
            "::1" => Ok(("[::1]".into(), port)),
            _ => Err("--cors-origin must use a loopback origin"),
        };
    }

    let (host, port) = match rest.split_once(':') {
        Some((host, port)) => (host, parse_origin_port_with_digits(port)?),
        None => (rest, String::new()),
    };
    if host.contains(':') {
        return Err("--cors-origin has a malformed host");
    }
    Ok((host.to_ascii_lowercase(), port))
}

fn parse_origin_port(remainder: &str) -> Result<String, &'static str> {
    if remainder.is_empty() {
        return Ok(String::new());
    }
    let Some(port) = remainder.strip_prefix(':') else {
        return Err("--cors-origin has a malformed port");
    };
    parse_origin_port_with_digits(port)
}

fn parse_origin_port_with_digits(port: &str) -> Result<String, &'static str> {
    if port.is_empty() {
        return Err("--cors-origin has a malformed port");
    }
    port.parse::<u16>()
        .map_err(|_| "--cors-origin has a malformed port")?;
    Ok(format!(":{port}"))
}
