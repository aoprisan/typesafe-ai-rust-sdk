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

- `:lesson` walks an eleven-step track from "what is a noul" to what a call costs.
- `:sketch` (Ctrl-K) opens the whole request as one page of text, with the question types read
  off the punctuation, a gutter that says what each line became, and a live preview of the JSON,
  simulated answers, the same request as Rust, or what a call would cost:

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

  `@threshold 0.6` under a noul, or `@confidence 0.7` under a choice or a score, keeps the bar
  its answer is acted on at with the question; the gutter calls it `bar`, and it never goes on the
  wire.

- `:turn` grows the state into a conversation, so a rubric can be re-read after every reply
  instead of sampled once. The questions stay exactly as they are and only the state gets longer:

  ```text
  :turn customer: The payout failed again, third time this month.
  :turn agent: Sorry about that — can you confirm the last four digits?
  <Enter>                                          # every question, over the whole thread
  :turn customer: I have sent them twice already. I want a refund now.
  <Enter>                                          # the same questions again; watch is_urgent move
  ```

  `:trend` does the re-asking for you: every question after each turn — the first turn, the first
  two, and so on — drawn as one line per question, so you can see when a rubric noticed rather than
  only where it ended up. A choice is followed through the label it ended on, a score along its
  levels, and the last column names every turn the answer changed:

  ```text
  trend · 4 turns
    is_urgent    noul    ▂▂▅▇  0.12 → 0.91            turn 3 yes
    department   choice  ▃▃▆▇  technical 0.35 → 0.80  turn 2 technical
    frustration  score   ▂▂▄▇  0.20 → 1.70 of 2       turn 3 level 1 · turn 4 level 2
    ≈ 612 in / 400 out tokens over 4 calls — estimated, since nothing was sent.
  ```

  It is a call per turn, live or not, and the cost line counts them all.

  Nothing new goes on the wire — the `state` is simply an array, `[{"who": …, "said": …}, …]`,
  which is why a page can hold one and `jev eval` can score one without knowing anything new. The
  speaker is the first word only when it ends in a colon, so a line typed without one keeps all of
  its words. `:turn list` shows the thread, `:turn drop` takes the last one back, and a state that
  is already plain text becomes the first turn rather than being thrown away. Answers stay out of
  it: what comes back is a distribution, not something to reason over next turn.

- `:build` (Ctrl-B) opens a form for one question and stays open on the type just added, so a
  rubric of three nouls is three names rather than three type-pickings; it lists what the session
  already holds and asks before a new question replaces one of them. `:json` shows the exact
  request body, `:last` the raw response, `:rust` the session as a program against
  [`typesafe-ai-sdk`](https://crates.io/crates/typesafe-ai-sdk).
- Alt-←/→ cross a word wherever there is text to edit, Alt-Backspace deletes one, and Alt-↑/↓ move
  the line under the cursor in sketch mode. Terminals spell Alt in several ways — a modified
  arrow, the Meta bit, an Esc prefix, or `Alt-b`/`Alt-f` — and all of them are read; Ctrl-←/→ works
  too, for the terminals that send only that.
- `:cost` says what a call is about to cost, per question and on both sides of the wire — a choice
  over eight labels comes back with eight probabilities, a score echoes its whole legend:

  ```text
                          in   out
  department   choice     74    58
  frustration  score      60    93
  is_urgent    noul       32    19
  state                   40     ·
  envelope                22    16
  total                  228   186   414 tokens per call
    $0.000232 per call   ·   $0.2316 per 1,000 calls
    at $0.20/$1.00 per Mtok
  ```

  A conversation is priced as what it is — a call per turn over a state that keeps growing, so the
  tokens climb faster than the transcript does:

  ```text
  state        3 turns    84     ·
  total                  272   186   458 tokens per call
    $0.000240 per call   ·   $0.2404 per 1,000 calls
    asked after every turn: 3 calls, 732 in / 558 out   1290 tokens for the thread   ·   $0.000704
  ```

  Rates are yours to supply, because nothing here knows what a model charges: `:cost 0.20/1.00` is
  dollars per million tokens, input then output, and `JEV_PRICE=0.20/1.00` sets the same at
  startup. Without them the table counts tokens and stops there. Tokens are estimated from the
  body — roughly four characters a token — so they are a shape, not an invoice; a live answer
  carries the counted `usage`, and the REPL prices that instead.

- `:save triage.jev` / `:open triage.jev` keep sessions as sketch pages; other paths use the
  request JSON.

## Outside

A session shaped in the REPL and saved with `:save` is something a script can run. With a
subcommand `jev` never opens a terminal: it reads a page (or stdin), prints one thing, and says
with its exit status whether it worked — 0 when it did, 1 when the call or the file did not, 2 when
the command line did not parse.

```sh
jev run triage.jev                       # send it; the answers, with their distributions
jev run triage.jev --json | jq .answers  # the raw response body instead
jev eval triage.jev --cases cases.jsonl  # score the rubric over states you have labelled
jev json triage.jev                      # the exact request body it would POST
jev cost triage.jev --price 0.20/1.00    # the token table, priced
jev rust triage.jev                      # the session as a program against the SDK
jev check triage.jev                     # parse only: every problem, with line numbers
```

The file is a `.jev` page or a request body, and which one it is comes from the text rather than
the name, so a body piped back in works the same: `jev json page.jev | jev cost`. `-`, or no file
at all, reads stdin.

```sh
jev run triage.jev --state "$(cat ticket.txt)" --json | jq '.answers.is_urgent.noul'
```

`--turn` appends to the state instead of replacing it, and repeats, so a thread can be driven from
a script the same way it is typed in the REPL:

```sh
jev run thread.jev --turn "agent: We are looking into it." --turn "customer: I want a refund now."
```

A page can also start out as a conversation: put the array above the `---` and it is read back as
one, by `jev run`, by `jev cost`, and by a `state` in a `jev eval` case.

`--state <text>` sets or replaces the state, `--model <name>` picks the model, `--threshold <0-1>`
says what counts as a yes for a noul, `--price <in>/<out>` prices the table, `--timeout <seconds>`
bounds a live attempt, and `--mock` stays offline even with a key set. Without a key `jev run`
simulates the answers and says so on stderr, so the stdout of a mock run is still the answer page.

### Scoring a rubric

One call tells you what the model said about one ticket. It does not tell you where to put the
threshold, or how sure a choice has to be before a script may act on it — and this README leaves
both calls to you. `jev eval` is how you make them with data: it runs a saved page over states you
have already labelled and reports what the rubric got right.

```sh
jev eval triage.jev --cases cases.jsonl --price 0.20/1.00
jev eval triage.jev --cases cases.jsonl --json | jq .questions.is_urgent.best
```

The cases are JSON Lines, one labelled state per line. `state` is what to judge — a string or any
JSON — and `expect` names the questions on the page it is labelled for; questions it leaves out are
still asked and simply not scored. An `id` is optional and shows up in the report.

```jsonl
{"id": "t-001", "state": "Stripe has been failing for 3 days, I'm losing sales", "expect": {"is_urgent": true, "department": "technical", "frustration": 2}}
{"state": {"subject": "Invoice question", "body": "Can I get a copy of last month's invoice?"}, "expect": {"department": "billing", "is_urgent": false}}
```

An expectation is written the way its question is answered: a noul takes `true` or `false`, a
choice takes one of its option labels, and a score takes a level index or the text of one of its
levels. A bad line stops the run before anything is sent, named by its line in the file.

```text
  is_urgent    noul    40 cases · Brier 0.11
    threshold   acc   prec   rec    f1
    0.40        0.82  0.74   0.94   0.83
    0.50 *      0.85  0.80   0.89   0.84
    0.60        0.88  0.86   0.89   0.87
    best f1 at 0.60

  department   choice  40 cases · accuracy 0.78
    confidence ≥   coverage  accuracy
    0.00           1.00      0.78
    0.60           0.55      0.95
    confusion, rows expected, columns predicted
                billing  technical  sales
    billing     12       2          0

  40 cases · 40 answered · 0 errors
  4812 in / 3120 out tokens · $0.0041
```

A conversation can be labelled turn by turn. `{"by_turn": 3}` on a noul says it should be false
for the first two turns and true from the third on (`{"by_turn": null}`: never); the case is then
sent once per turn, each prefix is scored as a case of its own, and the case's other labels apply
to the whole conversation only. The noul's block gains a line that says when it noticed:

```jsonl
{"id": "t-9", "state": [{"who": "customer", "said": "Hi"}, {"who": "customer", "said": "It is down"}, {"who": "customer", "said": "We lose money every minute"}], "expect": {"is_urgent": {"by_turn": 3}, "department": "technical"}}
```

```text
    by turn  12 threads · 7 on time · 2 early · 2 late · 1 missed · 0 false alarms · mean latency +0.18 turns
```

That is the whole point of the table: the sweep says what a threshold buys, and the gate says what
a confidence bar buys — 0.95 accuracy over 55% of the tickets, with the rest going to a person.

`--cases <file>` is the only new flag that is required; `-` reads them from stdin, which the page
cannot also do. `--concurrency <n>` sends that many at a time (default 4), `--cache <dir>` keeps
each response so a second run sends nothing (the directory is in the SDK's cassette format, so
`TYPESAFE_REPLAY=<dir>` replays it through the library), `--max-cost <dollars>` refuses a run whose estimate is
above it (rates required), and `--min-accuracy <0-1>` exits 1 when a question scores below it. A
live run prints its estimate on stderr before sending anything. `--state` does not apply: the cases
carry the states. Exit status is 0 when every case answered and every bar was met, 1 when a case
errored or a bar was missed, 2 when the command line did not parse.

### Keeping the threshold with the question

Once the table has told you where the threshold goes, the page is where to keep it. A bar is
written under its question, and it is the one thing on a page that never goes on the wire — it is
what you do with the answer, not part of the question:

```text
is_urgent? The message conveys urgency
  yes: A deadline, or money being lost now
  @threshold 0.6

department: Which team should handle this
  billing = Payment or subscription issues
  technical = Bugs or integration problems
  @confidence 0.65
```

`@threshold` is where a noul starts reading as yes; `@confidence` is how sure a choice or a score
has to be before it is acted on, with everything below it sent to a person. A question's own bar
wins over `--threshold` and `:threshold`, which stay the default for nouls without one — in the
answer page, in the eval report's starred row, and in `jev rust` and `:rust`, which gate on the
page's numbers instead of the 0.5 and 0.6 they otherwise write. `:save` and `:open` keep bars in a
`.jev` page; a request body has nowhere to put one.

`--calibrate` writes them for you:

```sh
jev eval triage.jev --cases cases.jsonl --calibrate
```

```text
  calibration  target accuracy 0.90
    is_urgent    @threshold 0.6     was none   f1 0.87
    department   @confidence 0.65   was none   accuracy 0.92 over 0.55 of cases
    frustration  left alone         no confidence bar reaches accuracy 0.90 (best 0.84 at 0.75)
  wrote 2 bars to triage.jev
```

A noul gets the threshold with the best F1. A choice or a score gets the lowest confidence bar at
which the answers it lets through are right `--target-accuracy` of the time (0.9 unless you say
otherwise) — the lowest, because every step up hands more of the work to a person. Only the bar
lines change: comments, blank lines and the order you wrote things in stay as they were. A run in
which any case errored writes nothing, since the bars would be fitted to the cases that happened to
work.

### Comparing two pages

A new page reads better than the old one on the three tickets you tried it on; that is not the same
as being better. `--compare` runs a second page over the same cases and says what moved, case by
case, and whether it moved more than chance would:

```sh
jev eval triage.jev --compare triage-v2.jev --cases cases.jsonl
```

```text
  department   choice  40 paired cases
                  a       b       Δ
    accuracy      0.78    0.90    +0.12
    9 fixed · 1 broke · 2 changed
    McNemar p 0.021 over 10 discordant pairs: b is significantly better
    case 2           sales → billing       fixed
    case 17 (t-017)  billing → technical   broke
```

The first page is `a`, the one after `--compare` is `b`, and every delta is `b − a`, measured only
on the cases both pages answered. A flip is `fixed` when `b` put right what `a` got wrong, `broke`
the other way round, and `changed` when both were wrong in different ways. The significance line is
an exact McNemar test over the fixed and broke cases; with fewer than six of them no difference can
reach p < 0.05, and it says so rather than printing a number that would be read as one.

A question only one page asks is listed as such, and a label may name a question on either page.
Both runs share one estimate for `--max-cost`, one pool for `--concurrency` and one `--cache`, so a
comparison costs what the two runs cost and a re-run costs nothing. `--fail-on-regression` exits 1
when `b` is significantly worse on any question — the line to put in CI — and `--json` prints both
reports whole next to the comparison.

Without `TYPESAFE_API_KEY` it starts in mock mode: answers are simulated locally (deterministic,
not predictive) so the shapes can be learned offline. `:key <api-key>` switches to live calls.

## In a coding agent

`jev` is also an MCP server and a skill, so an agent can shape and send a page without you
pasting one in. One command registers both with whichever agents you have:

```sh
jev install                      # every agent found under your home directory
jev install --client codex       # or name one: claude-code, codex, opencode, pi
jev install mcp --scope project  # just the server, into the repository in front of you
jev install --list               # the agents, and the file each one gets
```

The MCP server is the one-shot commands again, offered as tools: `jev_notation` (the notation
itself, for when a page will not parse), `jev_check`, `jev_request`, `jev_cost`, `jev_ask`,
`jev_eval`, `jev_code` and `jev_presets`. It speaks JSON-RPC over stdin and stdout — `jev mcp`
runs it by hand — and it holds no key of its own: the agent starts it, so it inherits the agent's
environment, or you write one in with `--env TYPESAFE_API_KEY=…`.

The skill is [`skills/jev/SKILL.md`](skills/jev/SKILL.md): the notation, what makes a question
worth asking, and the check-price-run-score loop. It is the SKILL.md format every one of these
agents reads, so it works the same in all four.

| Agent | MCP entry | Skill |
| --- | --- | --- |
| Claude Code | `~/.claude.json`, or `.mcp.json` in the project | `~/.claude/skills/jev/` |
| Codex CLI | `~/.codex/config.toml` | `~/.codex/skills/jev/` |
| OpenCode | `~/.config/opencode/opencode.json` | `~/.config/opencode/skills/jev/` |
| pi | `~/.pi/agent/mcp.json` | `~/.pi/agent/skills/jev/` |

Nothing else in those files is touched — the entry is merged in, and a config that cannot be
parsed is reported rather than rewritten. pi has no MCP client of its own yet, so its entry is
written in the shape its MCP extensions read; the skill works there as it is.

Unofficial. Not affiliated with TypeSafe AI. MIT licensed; source and issues at
[aoprisan/typesafe-ai-rust-sdk](https://github.com/aoprisan/typesafe-ai-rust-sdk).
