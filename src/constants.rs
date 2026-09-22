//! Environment-variable names, defaults and protocol constants.

use std::time::Duration;

/// Environment variable for the API key.
pub const API_KEY_ENV: &str = "TYPESAFE_API_KEY";
/// Environment variable for the API base URL.
pub const BASE_URL_ENV: &str = "TYPESAFE_BASE_URL";
/// Environment variable for the default model.
pub const DEFAULT_MODEL_ENV: &str = "TYPESAFE_DEFAULT_MODEL";
/// Environment variable naming a directory to record responses into.
pub const RECORD_ENV: &str = "TYPESAFE_RECORD";
/// Environment variable naming a directory to replay responses from.
pub const REPLAY_ENV: &str = "TYPESAFE_REPLAY";

/// Default API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
/// Default model name.
pub const DEFAULT_MODEL: &str = "jev-latest";
/// Default timeout for each HTTP attempt.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// SDK identifier sent in `User-Agent` and `X-TypeSafe-SDK`.
pub const SDK_NAME: &str = "typesafe-sdk-rust";

pub(crate) const SYSTEM_ONE_PATH: &str = "/v1/systemone";
pub(crate) const MODELS_PATH: &str = "/v1/models";

pub(crate) const MAX_ERROR_BODY_LENGTH: usize = 200;

pub(crate) const SDK_HEADER: &str = "x-typesafe-sdk";
pub(crate) const RUNTIME_HEADER: &str = "x-typesafe-runtime";
pub(crate) const RETRY_COUNT_HEADER: &str = "x-typesafe-retry-count";
pub(crate) const REQUEST_ID_HEADER: &str = "x-typesafe-request-id";
pub(crate) const RETRY_AFTER_HEADER: &str = "retry-after";
pub(crate) const RETRY_AFTER_MS_HEADER: &str = "retry-after-ms";

pub(crate) const SECRET_HEADERS: [&str; 6] = [
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "api-key",
    "cookie",
    "set-cookie",
];
