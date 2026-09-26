//! Redacting the broker's bearer and URL out of any text a human or log may read.

pub(crate) fn redact_secret(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "[redacted]")
    }
}

pub(crate) fn redact_broker_text(text: &str, url: Option<&str>, bearer: Option<&str>) -> String {
    let mut redacted = bearer
        .map(|bearer| redact_secret(text, bearer))
        .unwrap_or_else(|| text.to_string());
    let Some(url) = url else {
        return redacted;
    };
    redacted = redact_secret(&redacted, url);
    let Some(endpoint) = url
        .strip_prefix("http://")
        .and_then(|url| url.split('/').next())
    else {
        return redacted;
    };
    redacted = redact_secret(&redacted, endpoint);
    if let Some(port) = endpoint
        .rsplit_once(':')
        .map(|(_, port)| port)
        .filter(|port| !port.is_empty())
    {
        redacted = redact_secret(&redacted, port);
    }
    redacted
}
