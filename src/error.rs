//! Error types.
//!
//! The hierarchy mirrors the official Python SDK: configuration and request-validation errors are
//! raised before anything is sent; [`ApiError`] covers unsuccessful HTTP responses;
//! [`Error::Connection`] and [`Error::Timeout`] cover requests that never produced a response; and
//! [`ResponseValidationError`] covers a 2xx response whose body does not match the schema.

use std::fmt;
use std::time::{Duration, SystemTime};

use http::header::HeaderMap;
use serde_json::Value;

use crate::constants::{
    MAX_ERROR_BODY_LENGTH, REQUEST_ID_HEADER, RETRY_AFTER_HEADER, RETRY_AFTER_MS_HEADER,
};

/// Convenience alias for results returned by this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Any failure produced by the SDK.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The client could not be configured (missing API key, invalid base URL, invalid timeout,
    /// invalid retry policy).
    #[error("{0}")]
    Config(String),

    /// The request was rejected locally before being sent (no questions, empty choice or score
    /// criteria, malformed raw question, or a body that cannot be encoded as JSON).
    #[error("{0}")]
    InvalidRequest(String),

    /// The server returned an unsuccessful HTTP status after any retries.
    #[error(transparent)]
    Api(Box<ApiError>),

    /// The request could not reach the server or the response could not be read (DNS, connect,
    /// TLS, reset, body read). The underlying HTTP client's error is available via `source()`.
    #[error("Connection error: {0}")]
    Connection(#[source] Box<dyn std::error::Error + Send + Sync + 'static>),

    /// The request exceeded its configured timeout.
    #[error("Request timed out (timeout={}s).", .0.as_secs_f64())]
    Timeout(Duration),

    /// The server returned a successful status but the body is missing required data.
    #[error(transparent)]
    ResponseValidation(Box<ResponseValidationError>),

    /// The client is replaying and this request was never recorded. Nothing was sent: a replaying
    /// client does not fall back to the network. Record it first (`TYPESAFE_RECORD=<dir>`); see
    /// [`crate::cassette`].
    #[error("No recording for this request: {} does not exist (replaying, so nothing was sent).", path.display())]
    ReplayMiss {
        /// The request's cassette key.
        key: String,
        /// The file that would have held the response.
        path: std::path::PathBuf,
    },
}

impl Error {
    /// The API error, if this is one.
    pub fn as_api(&self) -> Option<&ApiError> {
        match self {
            Error::Api(e) => Some(e),
            _ => None,
        }
    }

    /// The HTTP status associated with this error, if any.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Api(e) => Some(e.status),
            Error::ResponseValidation(e) => Some(e.status),
            _ => None,
        }
    }

    /// The `x-typesafe-request-id` of the failing response, if any.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Error::Api(e) => e.request_id(),
            Error::ResponseValidation(e) => header_str(&e.headers, REQUEST_ID_HEADER),
            _ => None,
        }
    }
}

/// Classification of an unsuccessful HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ApiErrorKind {
    /// 400
    BadRequest,
    /// 401
    Authentication,
    /// 403
    PermissionDenied,
    /// 404
    NotFound,
    /// 422 — the request body failed server-side validation.
    UnprocessableEntity,
    /// 429
    RateLimit,
    /// Any 5xx, including TypeSafe's `529 Overloaded`.
    InternalServer,
    /// Any other non-success status.
    Other,
}

impl ApiErrorKind {
    /// Map a status code to its kind.
    pub fn from_status(status: u16) -> Self {
        match status {
            400 => Self::BadRequest,
            401 => Self::Authentication,
            403 => Self::PermissionDenied,
            404 => Self::NotFound,
            422 => Self::UnprocessableEntity,
            429 => Self::RateLimit,
            s if s >= 500 => Self::InternalServer,
            _ => Self::Other,
        }
    }
}

/// An unsuccessful HTTP response.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ApiError {
    /// HTTP status code.
    pub status: u16,
    /// Classification of `status`.
    pub kind: ApiErrorKind,
    /// Human-readable message extracted from the body.
    pub message: String,
    /// The JSON error body, the raw text as [`Value::String`] when it is not JSON, or `None` when empty.
    pub body: Option<Value>,
    /// Response headers.
    pub headers: HeaderMap,
    /// `"METHOD url"` without credentials, query or fragment.
    pub endpoint: Option<String>,
}

impl ApiError {
    pub(crate) fn new(
        status: u16,
        body: Option<Value>,
        headers: HeaderMap,
        endpoint: Option<String>,
    ) -> Self {
        let message = match body.as_ref().and_then(extract_message) {
            Some(m) => m,
            None => match &body {
                None => "status code (no body)".to_owned(),
                Some(Value::String(s)) => truncate(s),
                Some(v) => truncate(&v.to_string()),
            },
        };
        Self {
            status,
            kind: ApiErrorKind::from_status(status),
            message,
            body,
            headers,
            endpoint,
        }
    }

    /// The `x-typesafe-request-id` response header.
    pub fn request_id(&self) -> Option<&str> {
        header_str(&self.headers, REQUEST_ID_HEADER)
    }

    /// The wait the server asked for via `retry-after-ms` or `Retry-After`.
    pub fn retry_after(&self) -> Option<Duration> {
        parse_retry_after(&self.headers)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(endpoint) = &self.endpoint {
            write!(f, "{endpoint}: ")?;
        }
        write!(f, "{} {}", self.status, self.message)?;
        if let Some(id) = self.request_id() {
            write!(f, " (request_id={id})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ApiError {}

/// A successful response whose body is missing or has structurally invalid required data.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ResponseValidationError {
    /// HTTP status code (2xx).
    pub status: u16,
    /// Dotted path to the offending field, e.g. `answers.tone.confidence`.
    pub field_path: String,
    /// Underlying decoder message.
    pub detail: String,
    /// The decoded body (or raw text), if any.
    pub body: Option<Value>,
    /// Response headers.
    pub headers: HeaderMap,
    /// `"METHOD url"` without credentials, query or fragment.
    pub endpoint: Option<String>,
}

impl fmt::Display for ResponseValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(endpoint) = &self.endpoint {
            write!(f, "{endpoint}: ")?;
        }
        write!(
            f,
            "{} Invalid response data at '{}': {}",
            self.status, self.field_path, self.detail
        )?;
        if let Some(id) = header_str(&self.headers, REQUEST_ID_HEADER) {
            write!(f, " (request_id={id})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ResponseValidationError {}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn truncate(raw: &str) -> String {
    if raw.chars().count() > MAX_ERROR_BODY_LENGTH {
        let mut s: String = raw.chars().take(MAX_ERROR_BODY_LENGTH).collect();
        s.push('…');
        s
    } else {
        raw.to_owned()
    }
}

/// Decode an error body leniently: empty → `None`, JSON → value, anything else → string.
pub(crate) fn lenient_body(bytes: &[u8]) -> Option<Value> {
    if bytes.is_empty() {
        return None;
    }
    Some(
        serde_json::from_slice(bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(bytes).into_owned())),
    )
}

/// Pull a human-readable message out of the error shapes the API (FastAPI) may return.
pub(crate) fn extract_message(body: &Value) -> Option<String> {
    let non_empty = |s: &str| (!s.is_empty()).then(|| s.to_owned());
    let obj = match body {
        Value::String(s) => return non_empty(s),
        Value::Object(o) => o,
        _ => return None,
    };
    let str_at = |v: Option<&Value>, key: &str| {
        v.and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    match obj.get("error") {
        Some(Value::String(s)) => return Some(s.clone()),
        e @ Some(Value::Object(_)) => {
            if let Some(m) = str_at(e, "message") {
                return Some(m);
            }
        }
        _ => {}
    }
    if let Some(Value::String(m)) = obj.get("message") {
        return Some(m.clone());
    }
    match obj.get("detail") {
        Some(Value::String(s)) => Some(s.clone()),
        d @ Some(Value::Object(_)) => str_at(d, "message"),
        Some(Value::Array(entries)) => {
            let parts: Vec<String> = entries
                .iter()
                .filter_map(|entry| {
                    let msg = entry.get("msg")?.as_str()?;
                    let path = entry
                        .get("loc")
                        .and_then(Value::as_array)
                        .map(|loc| {
                            loc.iter()
                                .filter(|item| item.as_str() != Some("body"))
                                .map(|item| match item {
                                    Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                })
                                .collect::<Vec<_>>()
                                .join(".")
                        })
                        .unwrap_or_default();
                    Some(if path.is_empty() {
                        msg.to_owned()
                    } else {
                        format!("{path}: {msg}")
                    })
                })
                .collect();
            (!parts.is_empty()).then(|| parts.join("; "))
        }
        _ => None,
    }
}

/// Parse `retry-after-ms` (milliseconds) then `Retry-After` (seconds or HTTP date).
pub(crate) fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    if let Some(raw) = header_str(headers, RETRY_AFTER_MS_HEADER) {
        let raw = raw.trim();
        if let Ok(ms) = if raw.is_empty() {
            Ok(0.0)
        } else {
            raw.parse::<f64>()
        } && ms.is_finite()
            && ms >= 0.0
            && let Ok(delay) = Duration::try_from_secs_f64(ms / 1000.0)
        {
            return Some(delay);
        }
    }
    let raw = header_str(headers, RETRY_AFTER_HEADER)?;
    let trimmed = raw.trim();
    match if trimmed.is_empty() {
        Ok(0.0)
    } else {
        trimmed.parse::<f64>()
    } {
        Ok(secs) if secs.is_finite() && secs >= 0.0 => Duration::try_from_secs_f64(secs).ok(),
        Ok(_) => None,
        Err(_) => {
            let at = httpdate::parse_http_date(trimmed).ok()?;
            Some(
                at.duration_since(SystemTime::now())
                    .unwrap_or(Duration::ZERO),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::HeaderValue;
    use serde_json::json;

    #[test]
    fn extracts_fastapi_validation_details() {
        let body = json!({"detail": [
            {"loc": ["body", "questions", "x", "criteria"], "msg": "Field required", "type": "missing"},
            {"loc": ["body", "model"], "msg": "Bad model", "type": "value_error"}
        ]});
        assert_eq!(
            extract_message(&body).unwrap(),
            "questions.x.criteria: Field required; model: Bad model"
        );
    }

    #[test]
    fn extracts_message_precedence() {
        assert_eq!(
            extract_message(&json!({"error": "e", "message": "m"})).unwrap(),
            "e"
        );
        assert_eq!(
            extract_message(&json!({"error": {"message": "em"}})).unwrap(),
            "em"
        );
        assert_eq!(
            extract_message(&json!({"message": "m", "detail": "d"})).unwrap(),
            "m"
        );
        assert_eq!(
            extract_message(&json!({"detail": {"message": "dm"}})).unwrap(),
            "dm"
        );
        assert_eq!(extract_message(&json!({"other": 1})), None);
        assert_eq!(extract_message(&json!("")), None);
    }

    #[test]
    fn long_bodies_are_truncated() {
        let err = ApiError::new(
            500,
            Some(json!({"x": "y".repeat(500)})),
            HeaderMap::new(),
            None,
        );
        assert_eq!(err.message.chars().count(), MAX_ERROR_BODY_LENGTH + 1);
        assert!(err.message.ends_with('…'));
    }

    #[test]
    fn empty_body_message() {
        let err = ApiError::new(
            503,
            None,
            HeaderMap::new(),
            Some("GET http://x/v1/models".into()),
        );
        assert_eq!(
            err.to_string(),
            "GET http://x/v1/models: 503 status code (no body)"
        );
        assert_eq!(err.kind, ApiErrorKind::InternalServer);
    }

    #[test]
    fn retry_after_variants() {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER_MS_HEADER, HeaderValue::from_static("250"));
        h.insert(RETRY_AFTER_HEADER, HeaderValue::from_static("9"));
        assert_eq!(parse_retry_after(&h), Some(Duration::from_millis(250)));

        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER_HEADER, HeaderValue::from_static("2"));
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(2)));

        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER_HEADER, HeaderValue::from_static("-1"));
        assert_eq!(parse_retry_after(&h), None);

        let mut h = HeaderMap::new();
        h.insert(
            RETRY_AFTER_HEADER,
            HeaderValue::from_static("  Wed, 21 Oct 2015 07:28:00 GMT "),
        );
        assert_eq!(parse_retry_after(&h), Some(Duration::ZERO));

        // Too long for a `Duration`: ignored (falling back to `Retry-After`), never a panic.
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER_MS_HEADER, HeaderValue::from_static("1e300"));
        assert_eq!(parse_retry_after(&h), None);
        h.insert(RETRY_AFTER_HEADER, HeaderValue::from_static("3"));
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(3)));
    }
}
