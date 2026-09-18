# jev

A terminal REPL for shaping [TypeSafe AI](https://typesafe.ai) System One requests before you
write any code: send a `state` plus named, typed questions and read the typed answers back.

```sh
cargo install jev-repl
jev            # with TYPESAFE_API_KEY for live answers; without it, answers are simulated
```

| Question | Answer                                                          |
| -------- | --------------------------------------------------------------- |
| `noul`   | probability of "yes" (0–1)                                      |
| `choice` | selected label, per-label probabilities, confidence             |
| `score`  | probability-weighted level, legend, per-level probabilities, confidence |

## Inside

```text
:preset triage                                   # a ready-made session to poke at
:state The payout failed again, third time.      # bare text works too
:noul is_urgent The message conveys urgency | yes: A deadline | no: Routine
:choice department Which team | billing=Payments | technical=Bugs
:score frustration How frustrated | Calm | Annoyed | Furious
<Enter>                                          # send; answers come back with their distributions
```

- `:lesson` walks a ten-step track from "what is a noul" to confidence gating.
- `:sketch` (Ctrl-K) opens the whole request as one page of text, with the question types read
  off the punctuation, a gutter that says what each line became, and a live preview of the JSON,
  simulated answers, or the same request as Rust:

  ```text
  The payout failed again, third time this month.
  ---
  is_urgent? The message conveys urgency
    yes: A deadline or a threat to leave
  department: Which team should handle this
    billing = Payment or subscription issues
    technical = Bugs or integration problems
  frustration: How frustrated the customer appears
    Calm < Frustrated but civil < Very angry
  ```

- `:build` opens a form for one question; `:json` shows the exact request body, `:last` the raw
  response, `:rust` the session as a program against
  [`typesafe-ai-sdk`](https://crates.io/crates/typesafe-ai-sdk).
- `:save triage.jev` / `:open triage.jev` keep sessions as sketch pages; other paths use the
  request JSON.

Without `TYPESAFE_API_KEY` it starts in mock mode: answers are simulated locally (deterministic,
not predictive) so the shapes can be learned offline. `:key <api-key>` switches to live calls.

Unofficial. Not affiliated with TypeSafe AI. MIT licensed; source and issues at
[aoprisan/typesafe-ai-rust-sdk](https://github.com/aoprisan/typesafe-ai-rust-sdk).
