//! cargo run --example errors  (no API key, no network needed)
//! Every failure the SDK reports and how to tell them apart. The three it can provoke offline
//! are provoked here; the rest are matched in `report` so the shape of each one is visible.
use std::time::Duration;

use typesafe::{ApiErrorKind, Client, Error, Noul, Questions, RetryPolicy};

#[tokio::main]
async fn main() {
    // 1. Config — the client could not be built. Raised by `build`, before anything is sent.
    let err = Client::builder()
        .api_key("not-a-real-key")
        .base_url("ftp://api.typesafe.ai")
        .build()
        .expect_err("ftp is not an http(s) URL");
    report("bad base_url", err);

    // A client that points at a closed port, so nothing here leaves the machine.
    let client = Client::builder()
        .api_key("not-a-real-key")
        .base_url("http://127.0.0.1:9")
        .timeout(Duration::from_millis(500))
        .retry(RetryPolicy::none())
        .build()
        .expect("a valid configuration");

    // 2. InvalidRequest — rejected locally: no questions, empty criteria, unencodable state.
    let err = client
        .system_one("anything", Questions::new())
        .await
        .expect_err("a request with no questions is refused");
    report("no questions", err);

    // 3. Connection — the request never produced a response.
    let err = client
        .system_one("anything", Questions::from([("ok", Noul::new("Fine?"))]))
        .await
        .expect_err("nothing is listening on port 9");
    report("unreachable server", err);
}

/// One place that knows what to do with each kind of failure.
fn report(label: &str, err: Error) {
    print!("{label}: ");
    match err {
        // Programmer error: fix the configuration and restart.
        Error::Config(message) => println!("configuration — {message}"),

        // Programmer error too: the request was never sent, so retrying it changes nothing.
        Error::InvalidRequest(message) => println!("invalid request — {message}"),

        // The server answered, unhappily. `kind` classifies the status.
        Error::Api(api) => {
            print!("HTTP {} ({:?}) — {}", api.status, api.kind, api.message);
            if let Some(id) = api.request_id() {
                print!(" [request {id}]");
            }
            println!();
            match api.kind {
                // Already retried by the default policy; `retry_after` is what the server asked for.
                ApiErrorKind::RateLimit => println!("  back off for {:?}", api.retry_after()),
                // A bad key is not worth retrying.
                ApiErrorKind::Authentication => println!("  check TYPESAFE_API_KEY"),
                // The body was rejected; `api.body` holds the field-by-field detail.
                ApiErrorKind::UnprocessableEntity => println!("  body: {:?}", api.body),
                _ => {}
            }
        }

        // No response: DNS, connect, TLS, reset, body read. The transport error is the source.
        Error::Connection(source) => println!("no response — {source}"),

        // An attempt ran out of time. Retried by default; this is what is left after that.
        Error::Timeout(after) => println!("timed out after {after:?}"),

        // A 2xx whose body does not match the schema. `field_path` says exactly where.
        Error::ResponseValidation(bad) => {
            println!("unusable response at {} — {}", bad.field_path, bad.detail);
        }

        // `Error` is `#[non_exhaustive]`: later versions may add variants.
        other => println!("{other}"),
    }
}
