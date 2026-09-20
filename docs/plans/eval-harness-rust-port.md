# Plan: `jev eval` in Rust, a port of the TypeScript harness (v1)

This is an implementation plan meant to be executed step by step, in order, with a commit after
each step. It is a **port**, not a design: the contract was decided in the TypeScript repo and is
not reopened here. Where this document is silent, the TypeScript implementation decides — read it,
do not invent.

## Why

The two REPLs share a notation, a preset list, a lesson track and a simulator whose numbers are
pinned to match. `jev eval` should be the same command in both, so that a page and a cases file
written against one run unchanged against the other, and so that a reader who learned the harness
in one language is not made to learn it twice.

## What this ports

The reference is `aoprisan/jev-ts-repl`:

- the plan: `docs/plans/eval-harness-v1.md` (merged in PR #10);
- the code it produces: `src/repl/evaluate.ts`, the `runEval` half of `src/cli.ts`,
  `test/evaluate.test.ts` and the `eval` blocks of `test/cli.test.ts`.

**Do not start this port before that implementation has landed.** A port of a plan is a second
guess at the same problem; a port of a working reference is a translation, and the parity check in
"Definition of done" only means something once there is something to be at parity with.

## What is identical, and stays identical

Copy these verbatim from the reference. Every string below is part of the contract in both ports,
and the tests pin them in both.

- The command line: `jev eval [page] --cases <file>`, the new flags `--cases`, `--concurrency`
  (default 4), `--cache`, `--max-cost`, `--min-accuracy`, and the existing `--model`,
  `--threshold`, `--price`, `--timeout`, `--mock`, `--json`.
- The three usage errors, word for word: `--state does not apply to eval: the cases carry the
states.`, `the page and the cases cannot both come from stdin.`, and `--max-cost needs rates: pass
--price <in>/<out> or set JEV_PRICE.`
- Exit status 0, 1 and 2, for exactly the reasons the reference gives them.
- The cases file: JSON Lines, `state` required and non-empty, `expect` required with at least one
  key naming a non-raw question, optional string `id`, the per-kind rules for expected values, and
  errors reported as `cases line N: <message>` before anything is sent.
- The predictions: `is_yes(threshold)`, `answer.choice`, `rounded_level()`.
- Every metric and its definition: the threshold sweep and its columns, the Brier score, the best
  threshold with ties going to the lowest, the confusion matrix with its `other` column, the
  confidence gate at `0, 0.2, 0.4, 0.6, 0.8`, exact / within one / MAE, and which number each kind
  contributes to `--min-accuracy`.
- The text report's shape and the JSON report's schema, including `·` for an undefined rate in the
  text and `null` for it in the JSON.
- The cost preflight line, the refusal line, and the rule that a refusal sends nothing.
- The cache's key (the SHA-256 of the compact request body) and its read-before-send,
  write-after-decode, never-in-mock-mode behaviour.
- The concurrency semantics: workers pull from a shared queue, results are reported in file order,
  a case that fails records its error and the run continues.

## What already exists here and must be reused, not rewritten

| Need                        | Reuse                                                                                     | Where                      |
| --------------------------- | ----------------------------------------------------------------------------------------- | -------------------------- |
| Load a page or request body | `headless::load`, `headless::sendable`                                                    | `jev-repl/src/headless.rs` |
| Send a request              | `Client::system_one(..).model(..).timeout(..)`, retries, `Retry-After`                    | `src/client.rs`            |
| Offline answers             | `headless::mock_answers`                                                                  | `jev-repl/src/headless.rs` |
| Live answers                | `headless::live_answers`                                                                  | `jev-repl/src/headless.rs` |
| Predicted values            | `NoulAnswer::is_yes`, `ScoreAnswer::rounded_level`, `Answer::confidence`                  | `src/response.rs`          |
| Cost before sending         | `cost::estimate`, `cost::price_estimate`, `cost::price_usage`, `cost::usd`                | `jev-repl/src/cost.rs`     |
| Rates from the environment  | `cost::rates_from_env`, `cost::parse_rates`                                               | `jev-repl/src/cost.rs`     |
| Table rendering             | `format::plain`, `format::dim`, `format::bold`, and `cost_lines` as the model for spacing | `jev-repl/src/format.rs`   |
| Level text                  | `format::text_of`                                                                         | `jev-repl/src/format.rs`   |
| Error text                  | `format::error_lines`                                                                     | `jev-repl/src/format.rs`   |
| CLI flag parsing            | `parse_options` (extend it)                                                               | `jev-repl/src/main.rs`     |
| Reading a file or stdin     | `read_input`                                                                              | `jev-repl/src/main.rs`     |
| CLI tests with a fake API   | the `jev(..)` helper and `wiremock::MockServer`                                           | `jev-repl/tests/cli.rs`    |

## Repo conventions to follow

- **MSRV 1.88, edition 2024.** `just repl-check` is the gate: `cargo fmt -p jev-repl --check`,
  `cargo clippy -p jev-repl --all-targets -- -D warnings`, `cargo test -p jev-repl`. CI runs the
  same on 1.88 and stable.
- **Doc comments are prose**, one or two sentences saying why, in the voice of the existing files.
  Read `jev-repl/src/headless.rs` and `jev-repl/src/cost.rs` first and match them.
- **The SDK crate is not touched.** `typesafe-ai-sdk` is published and versioned; this port adds
  nothing to its public API. If a step seems to need one, the step is wrong (see delta 1).
- **No version bump.** `jev-repl` already sits at an unreleased `0.3.0`; releases are a separate
  step. This repo has no `CHANGELOG.md`, so there is no changelog entry to write (delta 7).
- **Commit messages** are one sentence in the imperative, no prefix, like the existing history
  ("Let a session be run without a terminal").

## Nothing existing changes, with two named exceptions

This is an addition. The REPL, the notation, the presets, the lessons, the five existing one-shot
commands with their flags, output and exit codes, and the simulator's pinned numbers all behave
exactly as before. Existing tests are not edited, only added to.

The two exceptions, both additive and both spelled out below: `session.rs` gains
`is_empty_value` (delta 3) and a compact sibling to `request_json` (delta 2), and `headless.rs`
gains `cached_answers` alongside `live_answers` (delta 1).

## The nine deltas from the TypeScript plan

These are the only places where the port deviates, and each one is decided here.

### 1. A cached body cannot become a `SystemOneResponse`

The reference decodes a cache hit with `decodeSystemOne` and rebuilds a response with
`makeSystemOneResponse`. Neither exists here: `response::decode_system_one` is `pub(crate)`, and
`SystemOneResponse`, `ResponseMeta` and `Answer` are all `#[non_exhaustive]`, so a second crate
cannot build one.

**Decision.** Do not rebuild a response, and do not add anything to the SDK to make it possible.
`Outcome::Ok` carries `(Vec<Answered>, Option<Usage>)` — everything the report actually reads — and
a cache hit produces that pair directly. Add to `headless.rs`, next to `live_answers`:

```rust
/// The answers a cached body carries, lined up with the questions that were asked.
///
/// The wire body is all the cache keeps, and `SystemOneResponse` cannot be rebuilt outside the SDK
/// crate, so this is `live_answers` for a response that arrived from disk instead of the network.
pub fn cached_answers(session: &Session, body: &Value) -> Option<(Vec<Answered>, Option<Usage>)>;
```

Match each answer object on its `"type"` tag and deserialize into `NoulAnswer`, `ChoiceAnswer` or
`ScoreAnswer` — the answer structs derive `Deserialize`, which is how `mock::from_value` already
builds them. An answer whose type this build does not know is skipped, the way the SDK's decoder
skips it. Return `None` when the body is not a System One body at all, and treat that as a cache
miss rather than an error.

### 2. There is no SHA-256, and `request_json` is pretty-printed

**Decision.** Add `sha2 = "0.10"` to `jev-repl`'s dependencies. It is already in `Cargo.lock`
through rustls, so the lock churn is a line. The TypeScript rule forbidding runtime dependencies is
a packaging rule for npm and has no analogue here; this is the only new dependency the port takes.

The key is the lowercase hex SHA-256 of the compact request body. `Session::request_json` prints
with `to_string_pretty`, so add its sibling in `session.rs`:

```rust
/// The same body `request_json` shows, with the whitespace taken out: what a cache key hashes.
pub fn request_json_compact(&self, model: &str) -> String;
```

Serialize through the same private `Body` struct so the field order stays `state`, `model`,
`questions` — the order the reference's `compact({ state, model, questions })` produces. A cache
directory is therefore shareable between the two ports in practice. That is a convenience, not a
contract: no test pins cross-port key equality, and none should.

The reference writes the response text exactly as it arrived. Here only the decoded `raw: Value` is
available, so write `serde_json::to_string(&response.raw)`. The bytes may differ from what the
server sent; what decodes out of them does not.

### 3. `is_empty_value` exists only as a method on `Session`

A case's state is arbitrary JSON, and the rule for "empty enough that there is nothing to judge"
lives inside `Session::state_is_empty`.

**Decision.** Extract it in `session.rs` as `pub fn is_empty_value(v: &Value) -> bool` and have
`state_is_empty` call it. Behaviour does not change and no existing test moves.

### 4. The purity rule does not port

`src/repl/evaluate.ts` must avoid `node:` imports so the web build can import it, and
`test/core.test.ts` walks the module graph to enforce it. There is no web build here and no graph
to walk.

**Decision.** `evaluate.rs` still keeps files, the environment and hashing out of itself — flags,
`std::fs`, `std::env` and `sha2` all live in `main.rs` — but as a convention, not a test. Do not
invent an enforcement mechanism for it.

### 5. The worker pool is tokio's, not hand-rolled

**Decision.** `run` spawns one task per case onto a `tokio::task::JoinSet`, gated by a
`tokio::sync::Semaphore` with `concurrency` permits, and writes each result into
`Vec<Option<Outcome>>` at the case's index, so file order survives whatever finishes first. Both
are already dependencies (`tokio` with `rt-multi-thread` and `sync`), and `Client` is `Clone` over
an `Arc`, so a worker holds a cheap copy. `evaluate.rs` may use `tokio`; that is not a violation of
delta 4.

### 6. The subcommand surface is five, not six

There is no `ts` command here. `eval` joins `run`, `json`, `cost`, `rust` and `check` in
`headless::COMMANDS`, and the hand-written `Options` block in `main.rs::help()` grows the five new
flags. `is_command` needs no change.

### 7. There is no changelog, and the version is already ahead

**Decision.** Skip the reference's changelog step entirely, and do not touch `jev-repl`'s version;
`0.3.0` is unreleased and this rides in it.

### 8. `{:.2}` and `toFixed(2)` disagree on ties

Rust's float formatting rounds a tie to even; JavaScript's `toFixed` rounds it up. A metric that
lands exactly on `0.125` would print `0.12` here and `0.13` there, which would break a report
compared across ports.

**Decision.** Every rate and probability in the text report goes through one helper:

```rust
/// Two decimals, rounding a tie away from zero the way the TypeScript port's `toFixed(2)` does.
fn two(x: f64) -> String {
    format!("{:.2}", (x * 100.0).round() / 100.0)
}
```

Pin it with a test on `0.125`, `0.005` and `0.0` so nobody quietly replaces it with `{:.2}`.

### 9. The test layout differs

`test/evaluate.test.ts` becomes `jev-repl/tests/eval.rs`; the CLI cases go into the existing
`jev-repl/tests/cli.rs` next to their neighbours. There is no `describe.runIf(built)` gate to port
— `CARGO_BIN_EXE_jev` means the binary is always there. The live server is `wiremock`: match on the
request body to vary the answer, and count with `MockServer::received_requests`.

## Module design

### `jev-repl/src/evaluate.rs` (new)

```rust
/// One labelled state: what to judge, and what the rubric should say about it.
pub struct Case {
    /// 1-based line in the cases file, for messages.
    pub line: usize,
    pub id: Option<String>,
    pub state: Value,
    /// Question name → expectation, in the order the file gave them, already checked against the
    /// session's questions. A `Vec` and not a map, because that is the shape `Session` uses for
    /// questions and it saves a dependency.
    pub expect: Vec<(String, Expectation)>,
}

pub enum Expectation {
    Noul { yes: bool },
    Choice { label: String },
    Score { level: usize },
}

/// Parse JSON Lines into cases, checking every expectation against `session`.
pub fn parse_cases(text: &str, session: &Session) -> Result<Vec<Case>, String>;

/// What one case's request came back as.
pub enum Outcome {
    Ok { answers: Vec<Answered>, usage: Option<Usage> },
    Failed { error: String },
}

/// Send every case through `ask`, at most `concurrency` at a time; results are in case order.
pub async fn run<F, Fut>(
    session: &Session,
    cases: &[Case],
    ask: F,
    concurrency: usize,
) -> Vec<Outcome>
where
    F: Fn(Session) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = Outcome> + Send + 'static;

pub struct Report { /* the JSON report's shape, with numbers unrounded */ }

pub struct ReportOptions<'a> {
    pub model: &'a str,
    pub threshold: f64,
    pub rates: Option<Rates>,
}

pub fn report(
    session: &Session,
    cases: &[Case],
    outcomes: &[Outcome],
    options: ReportOptions<'_>,
) -> Report;

/// The text report, as lines the terminal draws.
pub fn report_lines(report: &Report) -> Vec<Line<'static>>;

/// The JSON report, ready for `to_string_pretty`.
pub fn report_json(report: &Report) -> Value;

/// Questions whose accuracy is below `bar`, for --min-accuracy.
pub fn below_bar(report: &Report, bar: f64) -> Vec<(String, f64)>;

/// The preflight estimate: tokens summed over every case, priced when rates are known.
pub fn preflight(
    session: &Session,
    cases: &[Case],
    model: &str,
    rates: Option<Rates>,
) -> Preflight;
```

`parse_cases` returns the first problem as a `String`, the way `headless::load` does, and that
string already carries its `cases line N:` prefix. Keep the per-kind metric functions private and
test them through `report`.

### `jev-repl/src/lib.rs`

`pub mod evaluate;`, in alphabetical place. There is no second entry point to export from.

### `jev-repl/src/main.rs`

- `Options` grows `cases: Option<String>`, `concurrency: usize` (default 4), `cache: Option<String>`,
  `max_cost: Option<f64>`, `min_accuracy: Option<f64>`, with the reference's validation messages.
- `one_shot` branches on `command == "eval"` into a new `run_eval`, after the page is loaded.
- `run_eval` order: reject `--state`; check `--max-cost` has rates; read and parse the cases;
  build `ask` (mock, or live with an optional cache); preflight (live only); `run`; `report`;
  print; decide the exit code. Keep it near a hundred lines by pushing everything else into
  `evaluate.rs`.
- The cache lives here: `mkdir -p` the directory, hash with `sha2`, read before sending, write
  after a successful decode, and never touch it in mock mode.

## Steps

Each step ends with `just repl-check` green and one commit.

### Step 0: read the reference

Read `src/repl/evaluate.ts`, `runEval` in `src/cli.ts`, `test/evaluate.test.ts` and the `eval`
blocks of `test/cli.test.ts` from `jev-ts-repl`. No commit.

### Step 1: cases

`is_empty_value` in `session.rs`; `Case`, `Expectation`, `parse_cases` in a new `evaluate.rs`.
Port `test/evaluate.test.ts`'s parsing block case for case into `tests/eval.rs`, including every
rejection and the `cases line N:` prefix on each.

### Step 2: metrics

`Outcome`, `Report`, `report`, `below_bar`, and the `two` helper from delta 8 with its tie test.
Port the reference's metric tests with the same hand-computed numbers.

### Step 3: runner

`run`, per delta 5. Port the four runner tests: concurrency is never exceeded (count in the fake),
results come back in case order when later cases finish first, a failing `ask` becomes a
`Failed` outcome without stopping the others, and a concurrency above the case count is fine.

### Step 4: rendering

`report_lines`, `report_json`, `preflight`. Assert the same landmarks the reference does: a header
line per kind, the `*` on the chosen threshold, `best f1 at`, the `confusion` heading, the totals
line, and the `≈` marker when the tokens were estimated.

### Step 5: the module

`pub mod evaluate;` in `lib.rs`. Nothing else; there is no smoke script here.

### Step 6: the command

`headless::COMMANDS`, `Options`, `parse_options`, `run_eval`, the cache, and the `help()` text.
Port the CLI tests into `tests/cli.rs`: the mock-side block (exit 0 with all three headers, `--json`,
a bad cases line, the three usage errors, `--concurrency 0`, and `--min-accuracy 1` naming a
question), and a live block against `wiremock` (four cases answered from the request body, the same
run twice with `--cache` making zero requests the second time, `--max-cost` refusing with no
requests seen, and a 400 on one state leaving the others scored).

### Step 7: docs

A `Scoring a rubric` subsection under `## Outside` in `jev-repl/README.md`, mirroring the
reference's README section: the command, the cases format, the three kinds of expectation, the exit
codes, and one example report of about ten lines.

### Step 8: finish

`just repl-check`, then `cargo test -p jev-repl` on 1.88 as CI does, then the parity check below.
Push the branch. Do not open a pull request unless asked.

## Definition of done

- All eight steps committed, CI green on 1.88 and stable.
- `jev eval` on the `triage` preset saved as a page and a ten-line cases file prints the report in
  mock mode with no key set, and `--json` piped through `jq .questions.is_urgent.best` prints a
  threshold.
- **Parity:** the same page and the same cases file, in mock mode, produce byte-identical stdout
  from the TypeScript `jev eval` and this one. The simulators are already pinned to each other, so
  a difference is a bug in this port — in `two`, in the report's spacing, or in the metric — and
  not a reason to change the reference.
- No change to the `typesafe-ai-sdk` public API, no version bump, one new dependency (`sha2`).

## Out of scope, on purpose

- Expectations in the sketch notation. The notation is shared with the TypeScript REPL and this
  port is not the place to move it.
- Anything the reference put out of scope: shared rate limiting, progress output, resuming a run,
  comparing two models in one run, metrics not listed.
- A `ts` subcommand, a changelog, and any change to the published SDK crate.
