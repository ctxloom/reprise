fn resolve_endpoint(raw: &str, default_port: u16, secure: bool) -> String {
    let trimmed = raw.trim();
    let mut host = String::new();
    let mut port = default_port;
    if let Some((left, right)) = trimmed.split_once(':') {
        host.push_str(left);
        match right.parse::<u16>() {
            Ok(parsed) => port = parsed,
            Err(_) => port = default_port,
        }
    } else {
        host.push_str(trimmed);
    }
    let scheme = if secure { "https" } else { "http" };
    let mut out = String::new();
    out.push_str(scheme);
    out.push_str("://");
    out.push_str(&host);
    out.push(':');
    out.push_str(&port.to_string());
    out
}
