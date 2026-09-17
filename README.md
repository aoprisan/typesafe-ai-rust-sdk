# typesafe-sdk (Rust)

Rust client for the [TypeSafe AI](https://typesafe.ai) **System One** API: send a `state` plus named,
typed questions and get typed answers back.

| Question | Answer                                                          |
| -------- | --------------------------------------------------------------- |
| `Noul`   | probability of "yes" (0–1)                                      |
| `Choice` | selected label, per-label probabilities, confidence             |
| `Score`  | probability-weighted level, legend, per-level probabilities, confidence |

Behaviour mirrors the official Python SDK (`typesafe-sdk` 0.6.0): the same environment variables,
defaults, retry semantics, error classification and forward-compatible response decoding.

> Unofficial. Not affiliated with TypeSafe AI.

## Install

```toml
[dependencies]
typesafe-sdk = "0.1"                                     # async (bring your own Tokio runtime)
# typesafe-sdk = { version = "0.1", features = ["blocking"] }
```

The library is imported as `typesafe`. MSRV: Rust 1.88. TLS is rustls.

## Quick start

```rust
use typesafe::{Choice, Client, Noul, Questions, Score};

#[tokio::main]
async fn main() -> typesafe::Result<()> {
    let client = Client::from_env()?; // TYPESAFE_API_KEY

    let res = client
        .system_one(
            "I've been trying to connect my Stripe account for 3 days. Please help ASAP.",
            Questions::new()
                .with("department", Choice::new("Which team should handle this")
                    .option("billing", "Payment or subscription issues")
                    .option("technical", "Bugs or integration problems")
                    .option("sales", "Pricing or account questions"))
                .with("frustration", Score::new("How frustrated the customer appears",
                    ["Calm", "Frustrated but civil", "Very angry"]))
                .with("is_urgent", Noul::new("The message conveys urgency")),
        )
        .await?;

    let dept = res.choice("department").unwrap();
    if dept.confidence > 0.5 {
        println!("route to {}", dept.choice);
    }
    println!("{:.2}", res.score("frustration").unwrap().score);
    println!("{}", res.noul("is_urgent").unwrap().is_yes(0.8));
    Ok(())
}
```

`cargo run --example triage` runs the same flow against the live API.

### State

`state` is anything `Serialize`: a string, `json!({...})`, or your own struct.

```rust
#[derive(serde::Serialize)]
struct Ticket<'a> { subject: &'a str, messages: Vec<&'a str> }

client.system_one(Ticket { subject: "Payouts", messages: vec!["…"] }, questions).await?;
```

### Structured instructions and rubrics

Instructions, option descriptions and score levels accept any JSON value:

```rust
use typesafe::json;
Score::new(json!({"task": "rate tone", "ignore": ["signatures"]}),
           [json!({"level": "neutral"}), json!("hostile")]);
Noul::new("Is this a refund request?").when_true("Explicit ask for money back");
Choice::from_labels("Sentiment", ["positive", "neutral", "negative"]);
```

### Typed choices

```rust
#[derive(Debug)]
enum Dept { Billing, Technical }
impl std::str::FromStr for Dept { /* … */ }

let dept: Dept = res.choice("department").unwrap().parse()?;
```

### Per-call options

Requests implement `IntoFuture`, so you can `.await` them directly or configure them first:

```rust
client.system_one(state, questions)
    .model("jev-latest")
    .timeout(Duration::from_secs(3))
    .retry(RetryPolicy::none())
    .header(HeaderName::from_static("x-tenant"), HeaderValue::from_static("acme"))
    .extra_body("some_new_field", json!(true))   // shallow-merged last
    .await?;
```

Authentication and SDK-identification headers cannot be overridden.

### Models

```rust
for m in client.models().list().await?.models {
    println!("{} ({})", m.name, m.release_date);
}
```

### Blocking

```rust
let client = typesafe::blocking::Client::from_env()?;
let res = client.system_one("text", questions).send()?;
```

The blocking client owns a private current-thread runtime; don't call it from inside async code.

## Configuration

| Builder method  | Environment variable      | Default                   |
| --------------- | ------------------------- | ------------------------- |
| `api_key`       | `TYPESAFE_API_KEY`        | required                  |
| `base_url`      | `TYPESAFE_BASE_URL`       | `https://api.typesafe.ai` |
| `model`         | `TYPESAFE_DEFAULT_MODEL`  | `jev-latest`              |
| `timeout`       |                           | 10 s per attempt          |
| `retry`         |                           | `RetryPolicy::default()`  |
| `http_client`   |                           | a fresh `reqwest::Client` |

Explicit values win; blank environment values are ignored.

## Retries

`RetryPolicy::default()` matches the Python SDK:

- 2 retries after the first attempt,
- exponential backoff from 0.5 s to 5 s with 25 % subtractive jitter,
- retries on 408, 429 and 500–599 (including TypeSafe's `529 Overloaded`), connection errors and timeouts,
- honours `retry-after-ms` and `Retry-After` (seconds or HTTP date),
- a 30 s total budget per call: it stops *before* a wait that would exceed it,
- retries send `X-TypeSafe-Retry-Count`.

```rust
RetryPolicy::default()
    .max_retries(5)
    .backoff(Duration::from_millis(200), Duration::from_secs(2))
    .budget(Some(Duration::from_secs(10)))
    .retry_if(|e| e.status() == Some(409));
```

## Errors

```rust
match client.system_one(state, questions).await {
    Err(typesafe::Error::Api(e)) if e.kind == ApiErrorKind::RateLimit => {
        eprintln!("rate limited, retry after {:?} (request {:?})", e.retry_after(), e.request_id());
    }
    Err(typesafe::Error::ResponseValidation(e)) => eprintln!("bad field {}", e.field_path),
    Err(e) => eprintln!("{e}"),
    Ok(res) => { /* … */ }
}
```

| Variant              | When                                                                   |
| -------------------- | ---------------------------------------------------------------------- |
| `Config`             | missing API key, zero timeout, invalid retry policy                     |
| `InvalidRequest`     | no questions, empty score criteria, malformed raw question, unencodable state |
| `Api`                | non-2xx after retries; `kind`, `message`, `body`, `request_id()`, `retry_after()` |
| `Connection`         | no response (DNS, connect, reset, body read)                            |
| `Timeout`            | an attempt exceeded its timeout                                         |
| `ResponseValidation` | 2xx body missing required data; `field_path` like `answers.tone.confidence` |

Error messages from FastAPI-style validation bodies are flattened, e.g.
`questions.frustration.criteria: List should have at least 2 items`.

## Forward compatibility

- Answer types this version does not know are skipped (logged via `tracing` at WARN) and remain in
  `response.raw`.
- Unknown response fields are ignored.
- `Question::Raw(json!({...}))` sends a hand-built question; `extra_body` adds top-level fields.

## Logging

Uses `tracing`: INFO for each request/response line, DEBUG for headers and bodies. Secret headers are
redacted; bodies are not.

## Differences from the Python SDK

- Answers are looked up with `res.noul(name)` / `res.choice(name)` / `res.score(name)` or iterated with
  `nouls()` / `choices()` / `scores()`; Score maps are keyed by `u32`.
- `ResponseMeta` exposes status, headers and the number of attempts.
- No `TYPESAFE_LOG_LEVEL`; configure your `tracing` subscriber instead.

## Development

```sh
just          # fmt-check + clippy + tests
just live     # smoke test against the real API (needs TYPESAFE_API_KEY)
```

## License

MIT
