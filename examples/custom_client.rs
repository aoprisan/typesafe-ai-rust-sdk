//! cargo run --example custom_client --features reqwest-client  (needs TYPESAFE_API_KEY)
//! Everything the client takes from the environment, set in code instead: credentials, endpoint,
//! model, deadlines, a header on every request, and the HTTP client underneath.
use std::time::Duration;

use typesafe::http::header::{HeaderName, HeaderValue};
use typesafe::{Client, Noul, Questions, RetryPolicy};

#[tokio::main]
async fn main() -> typesafe::Result<()> {
    // Your own reqwest client: connection pool, connect deadline, proxies, TLS roots. The SDK
    // sets the per-request timeout itself, so leave that to `ClientBuilder::timeout`.
    let http = reqwest::Client::builder()
        .pool_max_idle_per_host(32)
        .connect_timeout(Duration::from_secs(2))
        .build()
        .expect("a usable HTTP client");

    let client = Client::builder()
        // Explicit values win over TYPESAFE_API_KEY, TYPESAFE_BASE_URL and
        // TYPESAFE_DEFAULT_MODEL; blank environment values are ignored.
        .api_key(std::env::var("TYPESAFE_API_KEY").expect("TYPESAFE_API_KEY"))
        .base_url("https://api.typesafe.ai")
        .model("jev-latest")
        .timeout(Duration::from_secs(5))
        .retry(RetryPolicy::default().max_retries(1))
        // Sent with every request. Authentication and SDK-identification headers are protected
        // and cannot be overridden here.
        .header(
            HeaderName::from_static("x-tenant"),
            HeaderValue::from_static("acme"),
        )
        .http_client(http)
        .build()?;

    let res = client
        .system_one(
            "Our invoice says 12 seats and we have 9.",
            Questions::from([("is_billing", Noul::new("This is about money"))]),
        )
        .model("jev-latest") // per call, overriding the client default
        .await?;

    println!(
        "{} says {:.3} (HTTP {}, {} attempt(s))",
        res.model,
        res.noul("is_billing").expect("asked, so answered").noul,
        res.meta.status,
        res.meta.attempts
    );
    Ok(())
}
