---
name: jev
description: >-
  Write, check, price, run and score TypeSafe AI System One classification
  requests — yes/no (noul), choice and score questions — as plain-text jev
  sketch pages, using the jev CLI or its MCP server. Use when asked to triage,
  route, moderate, grade, qualify or otherwise classify text with TypeSafe,
  when a .jev page or a /v1/systemone request body is involved, or when writing
  code against jev-repl or typesafe-ai-sdk.
---

# jev — TypeSafe AI System One questions

TypeSafe System One answers **named questions about one piece of text** in a single
call. You send a `state` (the text or JSON to judge) and a set of questions; you get
back one answer per question as a distribution, never as text: a noul is the probability
of yes (that probability is its own confidence), a choice is the winning label with a
probability per label and a confidence, and a score is the probability-weighted level
with a probability per level and a confidence. There is no rationale to read.

There are three kinds of question:

| kind       | writes as                                          | answers with                  |
| ---------- | -------------------------------------------------- | ----------------------------- |
| **noul**   | `name? instructions`                               | yes/no, as a probability      |
| **choice** | `name: instructions` + `label = description` lines | one of the labels             |
| **score**  | `name: instructions` + levels joined by `<`        | one level on an ordered scale |

## Sketch notation: the whole request as one page

A `.jev` page is the request written as text. Everything above the first `---` rule
is the state; everything below it is questions.

```text
The payout failed again, third time this month. I'm done waiting.
---
# a comment
@model jev-latest

is_urgent? The message conveys urgency or time-sensitivity
  yes: A deadline, a threat to leave, or "ASAP"
  no: Routine, no time pressure
  @threshold 0.6

department: Which team should handle this
  billing = Payment or subscription issues
  technical = Bugs or integration problems
  sales
  @confidence 0.7

frustration: How frustrated the customer appears
  Calm < Frustrated but civil < Very angry

shape! {"type": "noul", "instructions": "A hand-built question object"}
```

Rules worth remembering:

- The state is JSON when it parses as JSON, otherwise plain text.
- `name?` is a noul; the optional `yes:` / `no:` lines say what each outcome means.
- `name:` followed by `label = description` lines (or bare labels) is a choice.
- `name:` followed by one line of levels joined with `<` is a score, **lowest first**.
- `name!` takes a raw question object, for shapes the notation does not cover yet.
- Parts of a question can also go on its first line, separated by `|`:
  `is_urgent? Conveys urgency | yes: A deadline | no: No time pressure`
- `@model <name>` pins the model, `#` starts a comment, indentation is decoration.
- `@threshold <0-1>` under a noul, or `@confidence <0-1>` under a choice or a score, is
  the bar its answer is acted on at. It is never sent; it wins over `--threshold`, and
  `jev rust` gates on it.

Question names are the keys of the answer object, so name them the way you want to
read them back: `is_urgent`, `department`, `severity`.

## A conversation as the state

The state does not have to be one message. Written as an array of turns it is a thread,
and the same fixed questions can be re-read after every reply — which is the point: a
noul that climbs from 0.2 to 0.9 over four turns says something a single call cannot.

```text
[
  { "who": "customer", "said": "The payout failed again, third time this month." },
  { "who": "agent", "said": "Can you confirm the last four digits?" },
  { "who": "customer", "said": "I sent them twice already. I want a refund." }
]
---
is_urgent? The message conveys urgency or time-sensitivity
```

Nothing new goes on the wire, so this works everywhere a state does — in a page, in a
`jev eval` case, in an MCP tool call. `jev run page.jev --turn "customer: refund me"`
appends a turn instead of replacing the state, and repeats. `jev cost` counts a call per
turn, because the thread is sent whole every time and the tokens grow faster than it does.

Do not feed answers back into the state. What comes back is a distribution, not a fact,
and reasoning over it next turn compounds confidence instead of adding information — it
also stops a run being reproducible from its page.

## Writing good questions

- One question asks about one thing. Split `is_urgent_and_angry` into two.
- Instructions describe the property, not the answer you are hoping for.
- Spell out the edge: `yes:` and `no:` criteria are what stop a noul from drifting.
- Choice labels need descriptions whenever the label alone is ambiguous, and should
  cover the input — add `other` rather than forcing a wrong bucket.
- Score levels go lowest to highest and should be distinguishable by a stranger.
- A choice takes at most 255 options and a score between 2 and 10 levels; the API
  refuses anything outside that, and `jev check` says so first.
- A noul's answer is a probability; compare it against a threshold you pick
  (`--threshold`, default `0.5`) rather than treating it as a bare boolean.

## The workflow

1. **Draft** the page.
2. **Check** it — `jev check page.jev` names every parse problem with a line number.
3. **Price** it — `jev cost page.jev --price 0.20/1.00` before sending anything in bulk.
4. **Dry run** it — `jev run page.jev --mock` shapes the output without a network call.
   Mock answers are deterministic noise, never judgement: do not report them as results.
5. **Send** it — `jev run page.jev` with `TYPESAFE_API_KEY` set.
6. **Score** it — `jev eval page.jev --cases cases.jsonl` once you have labelled cases.
7. **Ship** it — `jev rust page.jev` prints the session as a program against the SDK.

## Command line

```bash
jev check page.jev                  # parse problems, or what it parsed into
jev json  page.jev                  # the exact body POSTed to /v1/systemone
jev cost  page.jev --price 0.20/1.00
jev run   page.jev --state "..." --threshold 0.7
jev run   page.jev --turn "customer: ..." --turn "agent: ..."   # append turns
jev run   page.jev --json           # the raw response body, for jq
jev eval  page.jev --cases cases.jsonl --min-accuracy 0.9
jev eval  page.jev --compare v2.jev --cases cases.jsonl --fail-on-regression
jev rust  page.jev                  # the session as a Rust program
```

The file argument may be `-` or omitted to read stdin, so a page can be a here-doc.
Exit status is `0` when it worked, `1` when the call or the file did not, `2` when the
command line did not parse. Without `TYPESAFE_API_KEY` every answer is simulated.

A cases file for `jev eval` is JSON Lines, one labelled state per line:

```jsonl
{"state": "My card was declined twice", "expect": {"department": "billing", "is_urgent": true}}
{"state": "The webhook returns 500", "expect": {"department": "technical"}}
```

`expect` names questions from the page: `true`/`false` for a noul, a label for a
choice, a level (name or index) for a score. Questions you leave out are not scored.
When the state is a conversation, a noul can be labelled per turn: `{"by_turn": 3}` is
false before turn 3 and true from it on (`null` for never). The case is sent once per
turn and the report says how early or late the noul noticed.

`--calibrate` writes the bars a run supports back into the page — each noul's best-F1
`@threshold`, and the lowest `@confidence` at which a choice or score reaches
`--target-accuracy` (default 0.9) — changing nothing else. Calibrate on live answers
only: a bar fitted to simulated noise means nothing.

To tell whether a rewrite of a page is better, run both over the same cases with
`--compare`: it prints the change per question, the cases whose answer flipped, and an
exact McNemar test. Report "b is significantly better" only when it says so — with fewer
than six discordant cases it says "too few", and that means the cases cannot tell.

## MCP tools

When the jev MCP server is connected, the same work is available as tools — prefer
them over shelling out, and pass the page as text rather than writing a temp file:

- `jev_notation` — this notation reference, for when a page will not parse.
- `jev_check` — parse a page and report every problem.
- `jev_request` — the exact request body a page would POST.
- `jev_cost` — estimated tokens, priced when rates are given.
- `jev_ask` — send the page (or answer it offline when no key is set) and return the answers.
- `jev_eval` — run a page over labelled cases and score the answers; `compare` takes a
  second page and reports the difference, `calibrate` hands the page back with its bars
  written in.
- `jev_code` — the page as a Rust program against `typesafe-ai-sdk`.
- `jev_presets` — ready-made pages to start from: triage, moderation, lead, reply.

`jev_ask` and `jev_eval` spend money when a key is set. Check the page and look at
`jev_cost` first, and say what a run will cost before starting a large one.

## Writing code against the SDK

`jev rust` prints a working program for the current page — start there instead of
writing a client by hand. Build named questions, send one request, read the answers
back by name.

When the questions are fixed, describe them as a struct with `#[derive(Rubric)]`
(feature `derive`) and `ask` for it: the field name is the question name, the field type
is its answer, and a choice's labels can be an enum, so a misspelled name or an unknown
label is a compile error instead of a `None`.

```rust
use typesafe::{ChoiceOf, Client, NoulAnswer, Rubric, RubricChoice, ScoreAnswer};

#[derive(Rubric)]
struct Triage {
    #[noul("The message conveys urgency")]
    is_urgent: NoulAnswer,
    #[choice("Which team should handle this")]
    department: ChoiceOf<Department>,
    #[score("How frustrated the customer is", levels = ["Calm", "Annoyed", "Furious"])]
    frustration: ScoreAnswer,
}

#[derive(Debug, PartialEq, RubricChoice)]
enum Department {
    #[option("Payment or subscription issues")]
    Billing,
    #[option("Bugs or integration problems")]
    Technical,
}

let client = Client::from_env()?; // TYPESAFE_API_KEY
let triage: Triage = client.ask("The payout failed again.").await?;
triage.is_urgent.noul; // a probability
triage.department.value; // Department::Billing or Department::Technical
```

An answer that comes back missing, of the wrong type, or with a label the enum does not
have is an `Error::ResponseValidation` naming the field. Use `client.system_one` with
`Questions::new().with(...)` only when the questions are built at run time.

Keep the API key in the environment; never write it into a page, a config file or a
committed example.
