# typesafe-ai-sdk

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
typesafe-ai-sdk = "0.2"                                                 # async (bring your own Tokio runtime)
# typesafe-ai-sdk = { version = "0.2", features = ["blocking"] }        # sync client
# typesafe-ai-sdk = { version = "0.2", features = ["reqwest-client"] }  # bring your own reqwest::Client
```

The library is imported as `typesafe`. MSRV: Rust 1.88. TLS is rustls; `HTTPS_PROXY`-style
environment variables are honoured.

## Quick start

```rust,no_run
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

```rust,ignore
#[derive(serde::Serialize)]
struct Ticket<'a> { subject: &'a str, messages: Vec<&'a str> }

client.system_one(Ticket { subject: "Payouts", messages: vec!["…"] }, questions).await?;
```

### Structured instructions and rubrics

Instructions, option descriptions and score levels accept any JSON value:

```rust
use typesafe::{Choice, Noul, Score, json};
Score::new(json!({"task": "rate tone", "ignore": ["signatures"]}),
           [json!({"level": "neutral"}), json!("hostile")]);
Noul::new("Is this a refund request?").when_true("Explicit ask for money back");
Choice::from_labels("Sentiment", ["positive", "neutral", "negative"]);
```

### Typed choices

```rust,ignore
#[derive(Debug)]
enum Dept { Billing, Technical }
impl std::str::FromStr for Dept { /* … */ }

let dept: Dept = res.choice("department").unwrap().parse()?;
```

### Per-call options

Requests implement `IntoFuture`, so you can `.await` them directly or configure them first:

```rust,ignore
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

```rust,ignore
for m in client.models().list().await?.models {
    println!("{} ({})", m.name, m.release_date);
}
```

### Blocking

```rust,ignore
let client = typesafe::blocking::Client::from_env()?;
let res = client.system_one("text", questions).send()?;
```

The blocking client owns a private current-thread runtime; don't call it from inside async code.

## Learn it interactively

`jev` is a terminal REPL for shaping questions before you write any code:

```sh
cargo install jev-repl && jev     # from crates.io
just repl                         # from this checkout (or: cargo run -p jev-repl)
```

```text
:preset triage                                   # a ready-made session to poke at
:state The payout failed again, third time.      # bare text works too
:noul is_urgent The message conveys urgency | yes: A deadline | no: Routine
:choice department Which team | billing=Payments | technical=Bugs
:score frustration How frustrated | Calm | Annoyed | Furious
<Enter>                                          # send; answers come back with their distributions
```

- `:lesson` walks an eleven-step track from "what is a noul" to what a call costs.
- `:sketch` opens the whole request as one page of text (below).
- `:build` opens a form for composing a question, with the JSON it will send rendered as you type.
- `:json` shows the exact request body, `:last` the raw response, and `:rust` the same session as a
  program written against this SDK.
- `:cost` estimates what a call spends before it is sent — tokens per question for the request and
  for the answer it asks for — and prices them at rates you give it: `:cost 0.20/1.00` is dollars
  per million tokens, input then output, and `JEV_PRICE=0.20/1.00` sets the same at startup.
  Without rates it counts tokens and stops there; a live answer is priced from the `usage` the API
  reports.
- Without `TYPESAFE_API_KEY` it starts in mock mode: answers are simulated locally (deterministic,
  not predictive) so the shapes can be learned offline. `:key <api-key>` switches to live calls.
- A session saved with `:save` runs from a script: with a subcommand `jev` opens no terminal at
  all, so `jev run triage.jev` sends the page and prints the answers, `jev run --json` hands the
  raw body to `jq`, and `jev json`, `jev cost`, `jev rust` and `jev check` print the body, the
  token table, the code and the parse. Exit status is 0 when it worked, 1 when the call or the
  file did not, 2 when the command line did not parse.

### Sketch mode: the request as a page

Requests are rubrics, and rubrics are easier to write on paper than in a form. `:sketch` (or
Ctrl-K) opens the session as one page of plain text; the type of each question is read off its
punctuation, so there is nothing to select:

```text
The payout failed again, third time this month. I'm done waiting.
---
is_urgent? The message conveys urgency or time-sensitivity
  yes: A deadline, a threat to leave, or "ASAP"
  no: Routine, no time pressure

department: Which team should handle this
  billing = Payment or subscription issues
  technical = Bugs or integration problems
  sales

frustration: How frustrated the customer appears
  Calm < Frustrated but civil < Very angry
```

- Everything above the first `---` line is the state (JSON if it parses as JSON).
- `name?` asks yes/no (a noul); `yes:` / `no:` lines describe the outcomes.
- `name:` followed by `label = description` lines (or bare labels) is a choice.
- `name:` followed by levels joined with `<` is a score, lowest first.
- `name! {json}` sends a hand-built question object; `@model jev-2` pins the model; `#` comments.
- Parts can share the first line: `tone: Rate the reply | Warm < Neutral < Hostile`.

While you type, a gutter says what each line became (`noul`, `option`, `level`, …) and marks the
ones it could not place, the status line explains whatever the cursor is on, and the pane beside
the page cycles (Ctrl-P) between the JSON that would be sent, simulated answers so the shape of
the response is visible before anything is sent, the same request as Rust, and what the call would
cost. Ctrl-S applies the
page to the session, Ctrl-G applies and sends it, Alt-↑/↓ moves lines so questions and levels can
be reordered. A page with problems is never applied; the cursor jumps to the first one instead.

The page is a file format too: `:save triage.jev` writes it, `:open triage.jev` reads it back, and
`:sketch show` prints the current session in the notation.

The REPL lives in [`jev-repl/`](jev-repl) as a separate workspace member and is published as its
own crate, [`jev-repl`](https://crates.io/crates/jev-repl), so its TUI dependencies stay out of
the library.

## Configuration

| Builder method  | Environment variable      | Default                   |
| --------------- | ------------------------- | ------------------------- |
| `api_key`       | `TYPESAFE_API_KEY`        | required                  |
| `base_url`      | `TYPESAFE_BASE_URL`       | `https://api.typesafe.ai` |
| `model`         | `TYPESAFE_DEFAULT_MODEL`  | `jev-latest`              |
| `timeout`       |                           | 10 s per attempt          |
| `retry`         |                           | `RetryPolicy::default()`  |
| `http_client`   |                           | a fresh `reqwest::Client` (feature `reqwest-client`) |

Explicit values win; blank environment values are ignored. Header types come from the `http` crate,
re-exported as `typesafe::http`.

## Retries

`RetryPolicy::default()` matches the Python SDK:

- 2 retries after the first attempt,
- exponential backoff from 0.5 s to 5 s with 25 % subtractive jitter,
- retries on 408, 429 and 500–599 (including TypeSafe's `529 Overloaded`), connection errors and timeouts,
- honours `retry-after-ms` and `Retry-After` (seconds or HTTP date),
- a 30 s total budget per call: it stops *before* a wait that would exceed it,
- retries send `X-TypeSafe-Retry-Count`.

```rust
use std::time::Duration;
use typesafe::RetryPolicy;
RetryPolicy::default()
    .max_retries(5)
    .backoff(Duration::from_millis(200), Duration::from_secs(2))
    .budget(Some(Duration::from_secs(10)))
    .retry_if(|e| e.status() == Some(409));
```

## Errors

```rust,ignore
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
| `Config`             | missing API key, invalid base URL, zero timeout, invalid retry policy   |
| `InvalidRequest`     | no questions, empty choice/score criteria, malformed raw question, unencodable state |
| `Api`                | non-2xx after retries; `kind`, `message`, `body`, `request_id()`, `retry_after()` |
| `Connection`         | no response (DNS, connect, reset, body read); HTTP client error in `source()` |
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

Uses `tracing`: INFO when a request is retried, DEBUG for each request/response line, TRACE for
headers and bodies. Secret headers are redacted; bodies (including your `state`) are not.

## Differences from the Python SDK

- Answers are looked up with `res.noul(name)` / `res.choice(name)` / `res.score(name)` or iterated with
  `nouls()` / `choices()` / `scores()`; Score maps are keyed by `u32`.
- Typed answer maps keep server order; `response.raw` uses `serde_json::Map` ordering.
- `ResponseMeta` exposes status, headers and the number of attempts.
- No `TYPESAFE_LOG_LEVEL`; configure your `tracing` subscriber instead.

## Development

```sh
just          # fmt-check + clippy + tests, for the library and the REPL
just live     # smoke test against the real API (needs TYPESAFE_API_KEY)
just repl     # the learning REPL
```

## Releasing

```sh
just publish-dry    # package and verify locally, no upload
just publish        # upload; needs a crates.io token (`cargo login`)
```

The REPL is released the same way with `just publish-repl-dry` / `just publish-repl`; it depends
on a published library version, so publish the library first when both change.

Or let CI do it: push a tag matching the crate's `version` — `v0.1.0` for the library,
`jev-v0.1.0` for the REPL (`git tag v0.1.0 && git push origin v0.1.0`). That runs
[`.github/workflows/release.yml`](.github/workflows/release.yml), which re-runs fmt, clippy and
the tests, checks the tag against that crate's manifest version, and publishes it with the
`CARGO_REGISTRY_TOKEN` repository secret.

## License

MIT
