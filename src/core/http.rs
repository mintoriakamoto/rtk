//! The **single network egress point** for rtk.
//!
//! rtk reads the output of the user's commands, so unrestricted outbound HTTP
//! would be a data-exfiltration risk. Every network call in the codebase must
//! go through this module — enforced by the `ureq-outside-http-module` rule in
//! `.semgrep.yml`, which fails the build on `ureq::` usage anywhere else.
//!
//! Callers today:
//! - `core::telemetry` / `core::telemetry_cmd` — opt-in usage ping and GDPR erasure
//! - `core::sync_cmd` — per-day savings aggregates for portal metering
//!
//! Adding a caller is a deliberate act: state in review what leaves the machine.

use std::time::Duration;

/// Transport- and status-level failures, kept distinct so callers can react to
/// an actionable status (401 → re-auth) without string-matching error text.
#[derive(Debug)]
// With networking compiled out, `Status` is unreachable by construction — the
// stub only ever reports a transport failure. Keep the variant so callers'
// match arms stay identical across builds.
#[cfg_attr(
    not(any(feature = "sync", feature = "telemetry")),
    allow(
        dead_code,
        reason = "no-network build cannot produce a status response"
    )
)]
pub enum HttpError {
    /// The server answered with a non-2xx status. Carries the response body,
    /// which usually holds the server's own explanation.
    Status(u16, String),
    /// The request never got an answer (DNS, TLS, timeout, refused).
    Transport(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Status(code, body) => write!(f, "HTTP {code}: {}", body.trim()),
            HttpError::Transport(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for HttpError {}

/// POST a JSON body. `headers` are sent verbatim in addition to
/// `Content-Type: application/json`.
///
/// Returns the response body on success.
#[cfg(any(feature = "sync", feature = "telemetry"))]
pub fn post_json(
    url: &str,
    body: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
) -> Result<String, HttpError> {
    let mut req = ureq::post(url).set("Content-Type", "application/json");
    for (name, value) in headers {
        req = req.set(name, value);
    }

    match req.timeout(timeout).send_string(body) {
        Ok(response) => Ok(response.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(code, response)) => Err(HttpError::Status(
            code,
            response.into_string().unwrap_or_default(),
        )),
        Err(e) => Err(HttpError::Transport(e.to_string())),
    }
}

/// Networking is compiled out entirely when neither feature is enabled, so a
/// default-off build cannot make an outbound request even by mistake.
#[cfg(not(any(feature = "sync", feature = "telemetry")))]
pub fn post_json(
    _url: &str,
    _body: &str,
    _headers: &[(&str, &str)],
    _timeout: Duration,
) -> Result<String, HttpError> {
    Err(HttpError::Transport(
        "this rtk build has no HTTP client compiled in (build with --features sync)".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_error_displays_code_and_body() {
        let e = HttpError::Status(401, "{\"error\":\"bad token\"}".to_string());
        assert_eq!(e.to_string(), "HTTP 401: {\"error\":\"bad token\"}");
    }

    #[test]
    fn test_transport_error_displays_message() {
        let e = HttpError::Transport("connection refused".to_string());
        assert_eq!(e.to_string(), "connection refused");
    }

    /// Without a network feature the call must fail closed rather than
    /// silently succeed or panic.
    #[cfg(not(any(feature = "sync", feature = "telemetry")))]
    #[test]
    fn test_no_feature_fails_closed() {
        let r = post_json("http://example.com", "{}", &[], Duration::from_secs(1));
        assert!(matches!(r, Err(HttpError::Transport(_))));
    }
}
