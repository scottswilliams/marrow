pub(super) struct CorsPolicy {
    origin: String,
    profile: CorsProfile,
}

const LOCAL_ALLOW_HEADERS: &str = "Content-Type";
const REMOTE_ALLOW_HEADERS: &str = "Content-Type, Authorization";
const REMOTE_REQUEST_HEADERS: [&str; 2] = ["content-type", "authorization"];

#[derive(Clone, Copy)]
enum CorsProfile {
    Local,
    Remote,
}

#[derive(Clone)]
pub(super) struct CorsMatch {
    pub(super) origin: String,
    pub(super) allow_headers: &'static str,
    pub(super) vary: &'static str,
}

impl CorsPolicy {
    pub(super) fn local(origin: &str) -> Result<Self, &'static str> {
        Ok(Self {
            origin: normalize_loopback_origin(origin)?,
            profile: CorsProfile::Local,
        })
    }

    pub(super) fn remote(origin: &str) -> Result<Self, &'static str> {
        Ok(Self {
            origin: validate_remote_origin(origin)?.to_string(),
            profile: CorsProfile::Remote,
        })
    }

    pub(super) fn match_origin(&self, origin: &str) -> Option<CorsMatch> {
        let matched = match self.profile {
            CorsProfile::Local => {
                normalize_loopback_origin(origin).is_ok_and(|origin| origin == self.origin)
            }
            CorsProfile::Remote => origin == self.origin,
        };
        matched.then(|| CorsMatch {
            origin: self.origin.clone(),
            allow_headers: self.allow_headers(),
            vary: self.vary(),
        })
    }

    pub(super) fn allow_headers(&self) -> &'static str {
        match self.profile {
            CorsProfile::Local => LOCAL_ALLOW_HEADERS,
            CorsProfile::Remote => REMOTE_ALLOW_HEADERS,
        }
    }

    pub(super) fn remote_request_headers_match(&self, headers: &str) -> bool {
        matches!(self.profile, CorsProfile::Remote)
            && requested_headers_match(headers, &REMOTE_REQUEST_HEADERS)
    }

    pub(super) fn vary(&self) -> &'static str {
        match self.profile {
            CorsProfile::Local => "Origin",
            CorsProfile::Remote => {
                "Origin, Access-Control-Request-Method, Access-Control-Request-Headers"
            }
        }
    }

    pub(super) fn is_remote(&self) -> bool {
        matches!(self.profile, CorsProfile::Remote)
    }
}

fn validate_remote_origin(origin: &str) -> Result<&str, &'static str> {
    if origin.is_empty()
        || origin
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err("--remote-cors-origin must be an origin without controls or whitespace");
    }
    if matches!(origin, "*" | "null") {
        return Err("--remote-cors-origin must not be wildcard or null");
    }
    let Some((scheme, rest)) = origin.split_once("://") else {
        return Err("--remote-cors-origin must be an origin such as https://app.example.com");
    };
    if !matches!(scheme, "http" | "https") {
        return Err("--remote-cors-origin must use http or https");
    }
    if rest.is_empty() || rest.contains('/') || rest.contains('?') || rest.contains('#') {
        return Err("--remote-cors-origin must be an origin, not a URL path");
    }
    parse_remote_origin_host_port(rest)?;
    Ok(origin)
}

fn parse_remote_origin_host_port(rest: &str) -> Result<(), &'static str> {
    if rest.contains('@') {
        return Err("--remote-cors-origin must not include userinfo");
    }
    if let Some(after_bracket) = rest.strip_prefix('[') {
        let Some(end) = after_bracket.find(']') else {
            return Err("--remote-cors-origin has a malformed IPv6 host");
        };
        let host = &after_bracket[..end];
        if host.is_empty() || host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err("--remote-cors-origin has a malformed IPv6 host");
        }
        parse_remote_origin_port(&after_bracket[end + 1..])?;
        return Ok(());
    }

    if rest.contains(']') || rest.matches(':').count() > 1 {
        return Err("--remote-cors-origin has a malformed host");
    }
    let (host, port) = match rest.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (rest, None),
    };
    validate_remote_origin_host(host)?;
    if let Some(port) = port {
        parse_remote_origin_port_digits(port)?;
    }
    Ok(())
}

fn validate_remote_origin_host(host: &str) -> Result<(), &'static str> {
    if host.is_empty() || host.starts_with('.') || host.ends_with('.') {
        return Err("--remote-cors-origin has a malformed host");
    }
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        return Ok(());
    }
    for label in host.split('.') {
        if label.is_empty()
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("--remote-cors-origin has a malformed host");
        }
    }
    Ok(())
}

fn parse_remote_origin_port(remainder: &str) -> Result<(), &'static str> {
    if remainder.is_empty() {
        return Ok(());
    }
    let Some(port) = remainder.strip_prefix(':') else {
        return Err("--remote-cors-origin has a malformed port");
    };
    parse_remote_origin_port_digits(port)
}

fn parse_remote_origin_port_digits(port: &str) -> Result<(), &'static str> {
    if port.is_empty() || port.parse::<u16>().is_err() {
        return Err("--remote-cors-origin has a malformed port");
    }
    Ok(())
}

fn requested_headers_match(headers: &str, allowed: &[&str]) -> bool {
    let mut seen = vec![false; allowed.len()];
    let mut count = 0;
    for header in headers.split(',') {
        let header = header.trim();
        if header.is_empty() {
            return false;
        }
        let Some(index) = allowed
            .iter()
            .position(|allowed| header.eq_ignore_ascii_case(allowed))
        else {
            return false;
        };
        if seen[index] {
            return false;
        }
        seen[index] = true;
        count += 1;
    }
    count == allowed.len() && seen.into_iter().all(|seen| seen)
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
