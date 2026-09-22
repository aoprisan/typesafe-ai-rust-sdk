# Examples

Each file is a small program that stands on its own. Except where noted they need an API key:

```sh
export TYPESAFE_API_KEY=sk-…
cargo run --example triage
```

| Example                                      | What it shows                                                                    |
| -------------------------------------------- | -------------------------------------------------------------------------------- |
| [`errors`](errors.rs)                         | every failure the SDK reports, and how to tell them apart — **no key or network needed** |
| [`models`](models.rs)                         | the smallest call there is: `GET /v1/models`                                       |
| [`triage`](triage.rs)                         | one Choice, one Score, one Noul; acting on the answer only when confidence is high |
| [`moderation`](moderation.rs)                 | several yes/no questions in one call, each with its own threshold                  |
| [`structured_state`](structured_state.rs)     | a `Serialize` struct as state, JSON rubrics, and an answer parsed into an enum     |
| [`concurrent`](concurrent.rs)                 | classifying a batch: cloned clients, per-call timeouts, one failure at a time      |
| [`retries`](retries.rs)                       | a patient policy for a background job, no retries at all behind a user             |
| [`forward_compat`](forward_compat.rs)         | hand-built questions, `extra_body`, and reading answers from `raw`                 |
| [`derive`](derive.rs)                         | the rubric as a struct, the answers decoded into it — `--features derive`          |
| [`blocking`](blocking.rs)                     | the same API from a plain `fn main` — `--features blocking`                        |
| [`custom_client`](custom_client.rs)           | full `ClientBuilder` configuration and your own `reqwest::Client` — `--features reqwest-client` |

The three feature-gated ones need the feature on the command line:

```sh
cargo run --example blocking --features blocking
cargo run --example custom_client --features reqwest-client
cargo run --example derive --features derive
```

To shape questions before writing any of this, the [`jev`](../jev-repl) REPL sends them
interactively and prints the same session as Rust (`:rust`).
