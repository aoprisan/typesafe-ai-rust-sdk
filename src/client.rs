//! The asynchronous client.

use std::fmt;
use std::future::{Future, IntoFuture};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT,
};
use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::cassette;
use crate::constants::*;
use crate::error::{ApiError, Error, ResponseValidationError, Result, lenient_body};
use crate::question::Questions;
use crate::response::{
    DecodeFailure, DecodedSystemOne, ListModelsResponse, ResponseMeta, SystemOneResponse,
    decode_models, decode_system_one,
};
use crate::retry::RetryPolicy;

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Builder for [`Client`]. Explicit settings win over environment variables; empty or
/// whitespace-only environment values are ignored.
///
/// Its `Debug` output hides the API key and any credential-bearing header.
#[must_use = "a builder does nothing until .build() is called"]
#[derive(Default)]
pub struct ClientBuilder {
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    timeout: Option<Duration>,
    retry: Option<RetryPolicy>,
    headers: HeaderMap,
    http: Option<reqwest::Client>,
    record: Option<PathBuf>,
    replay: Option<PathBuf>,
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .field("retry", &self.retry)
            .field("headers", &redacted(&self.headers))
            .field("http", &self.http)
            .field("record", &self.record)
            .field("replay", &self.replay)
            .finish()
    }
}

impl ClientBuilder {
    /// API key (else `TYPESAFE_API_KEY`). Surrounding whitespace is trimmed; an empty key, or one
    /// with whitespace, control or non-ASCII characters inside it, is rejected by `build`.
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// API root (else `TYPESAFE_BASE_URL`, else `https://api.typesafe.ai`).
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// Default model (else `TYPESAFE_DEFAULT_MODEL`, else `jev-latest`).
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Per-attempt timeout (default 10s).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Default retry policy.
    pub fn retry(mut self, policy: RetryPolicy) -> Self {
        self.retry = Some(policy);
        self
    }

    /// Extra header sent with every request. Authentication and SDK identification headers
    /// cannot be overridden.
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Write each successful System One response to `<dir>/<key>.json` (else `TYPESAFE_RECORD`).
    /// The directory is created if needed. See [`cassette`](crate::cassette).
    pub fn record(mut self, dir: impl Into<PathBuf>) -> Self {
        self.record = Some(dir.into());
        self
    }

    /// Answer System One calls from `<dir>/<key>.json` instead of the network (else
    /// `TYPESAFE_REPLAY`); no API key is needed. A request that was never recorded fails with
    /// [`Error::ReplayMiss`]. See [`cassette`](crate::cassette).
    pub fn replay(mut self, dir: impl Into<PathBuf>) -> Self {
        self.replay = Some(dir.into());
        self
    }

    /// Use your own `reqwest::Client` (proxies, TLS, connection pools…). Requires the
    /// `reqwest-client` feature.
    #[cfg(feature = "reqwest-client")]
    #[cfg_attr(docsrs, doc(cfg(feature = "reqwest-client")))]
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.http = Some(client);
        self
    }

    /// Build a [`blocking::Client`](crate::blocking::Client) (feature `blocking`).
    ///
    /// # Errors
    ///
    /// As for [`build`](Self::build), plus [`Error::Config`] if the runtime cannot be started.
    #[cfg(feature = "blocking")]
    #[cfg_attr(docsrs, doc(cfg(feature = "blocking")))]
    pub fn build_blocking(self) -> Result<crate::blocking::Client> {
        crate::blocking::Client::new(self.build()?)
    }

    /// Build the client.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] if no API key is set (unless replaying) or it is malformed, the base URL
    /// is not an http(s) URL, the timeout is zero, the retry policy is invalid, both record and
    /// replay are set, or the record directory cannot be created. The cause, such as the
    /// [`std::io::Error`] or the URL parse error, is the error's `source()`.
    pub fn build(self) -> Result<Client> {
        let cassette = match (
            resolve_path(self.record, RECORD_ENV),
            resolve_path(self.replay, REPLAY_ENV),
        ) {
            (Some(_), Some(_)) => {
                return Err(Error::config(format!(
                    "both record and replay are set, and a client does one or the other; unset \
                     {RECORD_ENV} or {REPLAY_ENV}, or drop one of the builder calls"
                )));
            }
            (Some(dir), None) => {
                std::fs::create_dir_all(&dir).map_err(|e| {
                    Error::config_caused(format!("cannot record into {}", dir.display()), e)
                })?;
                Some(Cassette::Record(dir))
            }
            (None, Some(dir)) => Some(Cassette::Replay(dir)),
            (None, None) => None,
        };
        let replaying = matches!(cassette, Some(Cassette::Replay(_)));
        // A replaying client never sends anything, so it has no use for a key.
        let api_key = resolve(self.api_key, API_KEY_ENV, None)
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty());
        if api_key.is_none() && !replaying {
            return Err(Error::config(format!(
                "no API key was provided; pass api_key or set the {API_KEY_ENV} environment variable"
            )));
        }
        if api_key
            .as_deref()
            .is_some_and(|k| !k.bytes().all(|b| b.is_ascii_graphic()))
        {
            return Err(Error::config(
                "the API key must contain only printable ASCII characters without whitespace",
            ));
        }
        let base_url = resolve(self.base_url, BASE_URL_ENV, Some(DEFAULT_BASE_URL))
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_owned();
        check_base_url(&base_url)?;
        let model = resolve(self.model, DEFAULT_MODEL_ENV, Some(DEFAULT_MODEL)).unwrap_or_default();
        let timeout = check_timeout(self.timeout.unwrap_or(DEFAULT_TIMEOUT))?;
        let retry = self.retry.unwrap_or_default();
        retry.validate()?;

        let mut protected = HeaderMap::new();
        if let Some(api_key) = api_key {
            let mut auth = HeaderValue::from_str(&format!("Bearer {api_key}"))
                .map_err(|e| Error::config_caused("the API key is not a valid header value", e))?;
            auth.set_sensitive(true);
            protected.insert(AUTHORIZATION, auth);
        }
        protected.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let ident = HeaderValue::from_str(&format!("{SDK_NAME}/{VERSION}")).expect("ascii");
        protected.insert(USER_AGENT, ident.clone());
        protected.insert(HeaderName::from_static(SDK_HEADER), ident);
        protected.insert(
            HeaderName::from_static(RUNTIME_HEADER),
            HeaderValue::from_str(&format!(
                "rust ({}; {})",
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
            .expect("ascii"),
        );

        Ok(Client {
            inner: Arc::new(Inner {
                http: self.http.unwrap_or_default(),
                base_url,
                model,
                timeout,
                retry,
                default_headers: self.headers,
                protected,
                cassette,
            }),
        })
    }
}

fn resolve(explicit: Option<String>, env: &str, default: Option<&str>) -> Option<String> {
    explicit
        .or_else(|| {
            std::env::var(env)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        })
        .or_else(|| default.map(str::to_owned))
}

fn resolve_path(explicit: Option<PathBuf>, env: &str) -> Option<PathBuf> {
    explicit.or_else(|| resolve(None, env, None).map(PathBuf::from))
}

fn check_base_url(url: &str) -> Result<()> {
    match reqwest::Url::parse(url) {
        Ok(u) if matches!(u.scheme(), "http" | "https") && u.has_host() => Ok(()),
        Ok(_) => Err(Error::config(format!(
            "base_url must be an http(s) URL with a host, got {url:?}"
        ))),
        Err(e) => Err(Error::config_caused(
            format!("base_url {url:?} is not a valid URL"),
            e,
        )),
    }
}

fn check_timeout(t: Duration) -> Result<Duration> {
    if t.is_zero() {
        Err(Error::config("timeout must be a positive duration"))
    } else {
        Ok(t)
    }
}

struct Inner {
    http: reqwest::Client,
    base_url: String,
    model: String,
    timeout: Duration,
    retry: RetryPolicy,
    default_headers: HeaderMap,
    protected: HeaderMap,
    cassette: Option<Cassette>,
}

/// Headers go through [`redacted`], so the API key and any gateway credential stay hidden.
impl fmt::Debug for Inner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Inner")
            .field("http", &self.http)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .field("retry", &self.retry)
            .field("default_headers", &redacted(&self.default_headers))
            .field("protected", &redacted(&self.protected))
            .field("cassette", &self.cassette)
            .finish()
    }
}

/// Where System One responses are recorded to or replayed from.
#[derive(Debug)]
enum Cassette {
    Record(PathBuf),
    Replay(PathBuf),
}

/// TypeSafe API client. Cheap to clone; clones share the connection pool.
///
/// ```no_run
/// use typesafe::{Client, Choice, Noul, Questions, Score};
///
/// # async fn run() -> typesafe::Result<()> {
/// let client = Client::from_env()?;
/// let res = client
///     .system_one(
///         "I've been trying to connect Stripe for 3 days. Please help ASAP.",
///         Questions::new()
///             .with("department", Choice::new("Which team should handle this")
///                 .option("billing", "Payment or subscription issues")
///                 .option("technical", "Bugs or integration problems"))
///             .with("frustration", Score::new("How frustrated", ["Calm", "Frustrated", "Very angry"]))
///             .with("is_urgent", Noul::new("The message conveys urgency")),
///     )
///     .await?;
/// println!("{}", res.choice("department").unwrap().choice);
/// # Ok(()) }
/// ```
#[derive(Debug, Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    /// Start configuring a client.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// A client configured entirely from the environment.
    ///
    /// # Errors
    ///
    /// [`Error::Config`], as for [`ClientBuilder::build`].
    pub fn from_env() -> Result<Self> {
        Self::builder().build()
    }

    /// The default model.
    pub fn default_model(&self) -> &str {
        &self.inner.model
    }

    /// Ask typed questions about `state` (a string, or anything `Serialize` that becomes a JSON
    /// object/array). Returns a request builder: `.await` it directly or set per-call options first.
    pub fn system_one<S: Serialize>(
        &self,
        state: S,
        questions: impl Into<Questions>,
    ) -> SystemOneRequest {
        SystemOneRequest {
            client: self.clone(),
            state: serde_json::to_value(state),
            questions: questions.into(),
            model: None,
            extra_body: Map::new(),
            opts: CallOptions::default(),
        }
    }

    /// The Models resource.
    pub fn models(&self) -> Models {
        Models {
            client: self.clone(),
        }
    }

    async fn execute(
        &self,
        method: Method,
        path: &str,
        body: Option<Bytes>,
        opts: CallOptions,
    ) -> Result<(Bytes, ResponseMeta, String)> {
        let inner = &self.inner;
        let retry = opts.retry.as_ref().unwrap_or(&inner.retry);
        retry.validate()?;
        let timeout = check_timeout(opts.timeout.unwrap_or(inner.timeout))?;
        let url = format!("{}{}", inner.base_url, path);
        let endpoint = format!("{method} {}", redact_url(&url));

        let mut headers = inner.default_headers.clone();
        headers.extend(opts.headers);
        headers.remove(RETRY_COUNT_HEADER);
        headers.extend(inner.protected.clone());
        if body.is_some() {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }

        let started = Instant::now();
        let mut attempts: u32 = 0;
        loop {
            let mut h = headers.clone();
            if attempts > 0 {
                h.insert(
                    HeaderName::from_static(RETRY_COUNT_HEADER),
                    HeaderValue::from(attempts),
                );
                tracing::info!(%endpoint, retry = attempts, "retrying");
            }
            attempts += 1;
            // `Bytes` clones share one buffer, so a retry does not copy the body.
            let result = self
                .attempt(&method, &url, &endpoint, h, body.clone(), timeout)
                .await;
            match result {
                Ok((bytes, status, resp_headers)) => {
                    let meta = ResponseMeta {
                        status,
                        headers: resp_headers,
                        attempts,
                    };
                    return Ok((bytes, meta, endpoint));
                }
                Err(err) => {
                    if !retry.is_retryable(&err) {
                        return Err(err);
                    }
                    let delay = retry.delay(attempts, &err);
                    if retry.should_stop(attempts, started.elapsed(), delay) {
                        return Err(err);
                    }
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
    }

    async fn attempt(
        &self,
        method: &Method,
        url: &str,
        endpoint: &str,
        headers: HeaderMap,
        body: Option<Bytes>,
        timeout: Duration,
    ) -> Result<(Bytes, StatusCode, HeaderMap)> {
        let t0 = Instant::now();
        tracing::debug!(%endpoint, "->");
        if tracing::enabled!(tracing::Level::TRACE) {
            tracing::trace!(%endpoint, headers = ?redacted(&headers),
                body = %body.as_deref().map(String::from_utf8_lossy).unwrap_or_default(), "->");
        }
        let mut req = self
            .inner
            .http
            .request(method.clone(), url)
            .headers(headers)
            .timeout(timeout);
        if let Some(b) = body {
            req = req.body(b);
        }
        let map_err = |e: reqwest::Error| {
            tracing::debug!(%endpoint, error = %e, "<- transport error");
            if e.is_timeout() {
                Error::Timeout(timeout)
            } else {
                Error::Connection(Box::new(e))
            }
        };
        let resp = req.send().await.map_err(map_err)?;
        let status = resp.status();
        let resp_headers = resp.headers().clone();
        let bytes = resp.bytes().await.map_err(map_err)?;

        tracing::debug!(
            %endpoint,
            status = status.as_u16(),
            elapsed_ms = t0.elapsed().as_millis() as u64,
            request_id = resp_headers.get(REQUEST_ID_HEADER).and_then(|v| v.to_str().ok()).unwrap_or("-"),
            "<-"
        );
        if tracing::enabled!(tracing::Level::TRACE) {
            tracing::trace!(%endpoint, headers = ?redacted(&resp_headers),
                body = %String::from_utf8_lossy(&bytes), "<-");
        }

        if !status.is_success() {
            return Err(Error::Api(Box::new(ApiError::new(
                status,
                lenient_body(&bytes),
                resp_headers,
                Some(endpoint.to_owned()),
            ))));
        }
        Ok((bytes, status, resp_headers))
    }
}

fn validation_error(
    status: StatusCode,
    body: Option<Value>,
    headers: HeaderMap,
    endpoint: &str,
    f: DecodeFailure,
) -> Error {
    Error::ResponseValidation(Box::new(ResponseValidationError {
        status,
        field_path: f.path,
        detail: f.detail,
        body,
        headers,
        endpoint: Some(endpoint.to_owned()),
    }))
}

fn redact_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut u) => {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => url.to_owned(),
    }
}

/// A credential-bearing header: the known names, plus anything that says it carries one, as an AI
/// gateway's own key does (`cf-aig-authorization`, `x-portkey-api-key`, `x-gateway-token`).
fn is_secret(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SECRET_HEADERS.contains(&name.as_str())
        || ["authorization", "api-key", "token", "secret"]
            .iter()
            .any(|part| name.contains(part))
}

fn redacted(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(k, v)| {
            let value = if is_secret(k.as_str()) {
                "[REDACTED]".to_owned()
            } else {
                v.to_str().unwrap_or("<binary>").to_owned()
            };
            (k.as_str().to_owned(), value)
        })
        .collect()
}

#[derive(Debug, Default, Clone)]
struct CallOptions {
    retry: Option<RetryPolicy>,
    timeout: Option<Duration>,
    headers: HeaderMap,
}

macro_rules! call_option_methods {
    () => {
        /// Override the retry policy for this call.
        pub fn retry(mut self, policy: RetryPolicy) -> Self {
            self.opts.retry = Some(policy);
            self
        }

        /// Override the per-attempt timeout for this call.
        pub fn timeout(mut self, timeout: Duration) -> Self {
            self.opts.timeout = Some(timeout);
            self
        }

        /// Add a header for this call (protected headers still win).
        pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
            self.opts.headers.insert(name, value);
            self
        }
    };
}

/// A pending `POST /v1/systemone`. Configure it, then `.await` it (or call [`send`](Self::send)).
#[must_use = "requests do nothing until awaited"]
#[derive(Debug)]
pub struct SystemOneRequest {
    client: Client,
    state: serde_json::Result<Value>,
    questions: Questions,
    model: Option<String>,
    extra_body: Map<String, Value>,
    opts: CallOptions,
}

impl SystemOneRequest {
    call_option_methods!();

    /// Override the model for this call.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Add a top-level body field. A key named `state`, `model` or `questions` replaces the
    /// standard field. Useful for API fields this SDK version does not model yet.
    pub fn extra_body(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.extra_body.insert(key.into(), value.into());
        self
    }

    /// Send the request.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidRequest`] if the state cannot be encoded or the questions are rejected
    ///   locally (none, or a choice or score without criteria); nothing is sent.
    /// - [`Error::Config`] if a per-call timeout or retry policy is invalid.
    /// - [`Error::Api`], [`Error::Connection`] or [`Error::Timeout`] once retries are exhausted.
    /// - [`Error::ResponseValidation`] if a 2xx body does not decode.
    /// - [`Error::ReplayMiss`] if replaying and the request was never recorded, or
    ///   [`Error::Config`] if its recording exists but cannot be read.
    pub async fn send(self) -> Result<SystemOneResponse> {
        let state = self.state.map_err(|e| {
            Error::invalid_request_caused("the state could not be encoded as JSON", e)
        })?;
        self.questions.validate()?;
        let model = self
            .model
            .unwrap_or_else(|| self.client.inner.model.clone());
        // Serialized as a struct (not via `serde_json::Value`) so question order reaches the wire
        // unchanged. `extra_body` keys replace the standard field of the same name.
        let extra = &self.extra_body;
        let body = SystemOneBody {
            state: (!extra.contains_key("state")).then_some(&state),
            model: (!extra.contains_key("model")).then_some(&model),
            questions: (!extra.contains_key("questions")).then_some(&self.questions),
            extra,
        };
        let bytes = serde_json::to_vec(&body).map_err(|e| {
            Error::invalid_request_caused("the request body could not be encoded as JSON", e)
        })?;

        // Only a recording or replaying client needs the body's hash.
        let cassette = self
            .client
            .inner
            .cassette
            .as_ref()
            .map(|c| (c, cassette::body_key(&bytes)));
        let (bytes, meta, endpoint) = match &cassette {
            Some((Cassette::Replay(dir), key)) => replay(dir, key)?,
            _ => {
                self.client
                    .execute(Method::POST, SYSTEM_ONE_PATH, Some(bytes.into()), self.opts)
                    .await?
            }
        };
        let decoded = decode_system_one(&bytes);
        if let (Ok(_), Some((Cassette::Record(dir), key))) = (&decoded, &cassette)
            && let Err(e) = cassette::write(dir, key, &bytes)
        {
            tracing::warn!(dir = %dir.display(), %key, error = %e, "could not record the response");
        }
        match decoded {
            Ok(DecodedSystemOne {
                model,
                usage,
                answers,
                raw,
            }) => Ok(SystemOneResponse {
                model,
                usage,
                answers,
                raw,
                meta,
            }),
            Err(f) => Err(validation_error(
                meta.status,
                lenient_body(&bytes),
                meta.headers,
                &endpoint,
                f,
            )),
        }
    }
}

/// The recorded response for `key`, in the shape a live call returns: no headers, no attempts.
fn replay(dir: &std::path::Path, key: &str) -> Result<(Bytes, ResponseMeta, String)> {
    let path = cassette::path(dir, key);
    let bytes = std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::ReplayMiss {
            key: key.to_owned(),
            path: path.clone(),
        },
        _ => Error::config_caused(format!("cannot read the recording {}", path.display()), e),
    })?;
    tracing::debug!(path = %path.display(), "<- replayed");
    let meta = ResponseMeta {
        status: StatusCode::OK,
        headers: HeaderMap::new(),
        attempts: 0,
    };
    Ok((bytes.into(), meta, format!("replay {}", path.display())))
}

#[derive(Serialize)]
struct SystemOneBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    questions: Option<&'a Questions>,
    #[serde(flatten)]
    extra: &'a Map<String, Value>,
}

impl IntoFuture for SystemOneRequest {
    type Output = Result<SystemOneResponse>;
    type IntoFuture = BoxFuture<'static, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.send())
    }
}

/// The Models resource.
#[derive(Debug, Clone)]
pub struct Models {
    client: Client,
}

impl Models {
    /// `GET /v1/models`.
    pub fn list(&self) -> ListModelsRequest {
        ListModelsRequest {
            client: self.client.clone(),
            opts: CallOptions::default(),
        }
    }
}

/// A pending `GET /v1/models`.
#[must_use = "requests do nothing until awaited"]
#[derive(Debug)]
pub struct ListModelsRequest {
    client: Client,
    opts: CallOptions,
}

impl ListModelsRequest {
    call_option_methods!();

    /// Send the request.
    ///
    /// # Errors
    ///
    /// - [`Error::Config`] if a per-call timeout or retry policy is invalid, or the client is
    ///   replaying (model listings are not recorded).
    /// - [`Error::Api`], [`Error::Connection`] or [`Error::Timeout`] once retries are exhausted.
    /// - [`Error::ResponseValidation`] if a 2xx body does not decode.
    pub async fn send(self) -> Result<ListModelsResponse> {
        if let Some(Cassette::Replay(dir)) = &self.client.inner.cassette {
            return Err(Error::config(format!(
                "listing models is not recorded, so a client replaying from {} cannot answer it",
                dir.display()
            )));
        }
        let (bytes, meta, endpoint) = self
            .client
            .execute(Method::GET, MODELS_PATH, None, self.opts)
            .await?;
        match decode_models(&bytes) {
            Ok((models, raw)) => Ok(ListModelsResponse { models, raw, meta }),
            Err(f) => Err(validation_error(
                meta.status,
                lenient_body(&bytes),
                meta.headers,
                &endpoint,
                f,
            )),
        }
    }
}

impl IntoFuture for ListModelsRequest {
    type Output = Result<ListModelsResponse>;
    type IntoFuture = BoxFuture<'static, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.send())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_hides_the_key() {
        let builder = Client::builder()
            .api_key("sk-very-secret")
            .header(
                HeaderName::from_static("x-portkey-api-key"),
                HeaderValue::from_static("gw-very-secret"),
            )
            .base_url("https://example.test");
        let shown = format!("{builder:?}");
        assert!(!shown.contains("very-secret"), "{shown}");
        assert!(
            shown.contains("***") && shown.contains("example.test"),
            "{shown}"
        );

        let client = builder.build().unwrap();
        let shown = format!("{client:?}");
        assert!(!shown.contains("very-secret"), "{shown}");
        assert!(shown.contains("example.test"), "{shown}");
    }

    #[test]
    fn masks_a_gateways_key_as_well_as_the_apis() {
        for name in [
            "Authorization",
            "cookie",
            "cf-aig-authorization",
            "x-portkey-api-key",
            "x-gateway-token",
            "x-client-secret",
        ] {
            assert!(is_secret(name), "{name}");
        }
        for name in ["content-type", "x-typesafe-request-id", "retry-after"] {
            assert!(!is_secret(name), "{name}");
        }
    }
}
