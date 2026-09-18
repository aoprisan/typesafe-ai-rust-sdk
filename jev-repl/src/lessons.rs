//! The guided track: eleven short lessons, each with one command to try.

pub struct Lesson {
    pub title: &'static str,
    pub body: &'static [&'static str],
    /// Dropped into the input line by Ctrl-T (or `:try`).
    pub try_this: &'static str,
}

pub const LESSONS: &[Lesson] = &[
    Lesson {
        title: "State and questions",
        body: &[
            "jev answers System One questions: fast, typed judgements about a piece of state.",
            "You send two things — a `state` (the text or JSON being judged) and a map of named questions — and get one answer per question back, under the same names.",
            "Start with the state. Everything else in this session is asked about it.",
        ],
        try_this: ":state I've been trying to connect my Stripe account for 3 days and it keeps failing. I'm losing sales. Please help ASAP.",
    },
    Lesson {
        title: "Noul: the probability of yes",
        body: &[
            "A noul is a yes/no question, but the answer is not a boolean — it is `noul`, the probability of yes, from 0 to 1.",
            "Add one, then run `:ask` to send the whole session.",
        ],
        try_this: ":noul is_urgent The message conveys urgency or time-sensitivity",
    },
    Lesson {
        title: "Thresholds, not booleans",
        body: &[
            "Because a noul is a probability, you pick the threshold: `answer.is_yes(0.8)` is a different product decision from `is_yes(0.5)`.",
            "Pick the threshold from the cost of being wrong — a cheap auto-reply can run at 0.5, a refund cannot.",
            "`:threshold 0.8` changes what this REPL calls a yes, so you can watch the same answer flip.",
        ],
        try_this: ":threshold 0.8",
    },
    Lesson {
        title: "Choice: one of N",
        body: &[
            "A choice picks one label from a set you define. You get the winning `choice`, the probability of every label, and a `confidence` derived from the spread.",
            "Options are `label=description`. The descriptions are instructions to the model, so they are worth writing well.",
        ],
        try_this: ":choice department Which team should handle this | billing=Payment or subscription issues | technical=Bugs or integration problems | sales=Pricing or account questions",
    },
    Lesson {
        title: "Confidence gating",
        body: &[
            "Two labels at 0.48 and 0.47 have a winner, but not a decision. That is what `confidence` is for.",
            "The useful shape is: act automatically above a confidence bar, route to a human below it. Never branch on the label alone.",
            "Run `:ask` again and read the confidence line before the label.",
        ],
        try_this: ":ask",
    },
    Lesson {
        title: "Score: ordered levels",
        body: &[
            "A score rates the state along levels you define, in order. The answer is probability-weighted, so it lands between levels: 1.4 means \"past level 1, not quite level 2\".",
            "Level 0 is the first one you list. Keep them monotonic — one axis, low to high.",
        ],
        try_this: ":score frustration How frustrated the customer appears | Calm, just stating facts | Frustrated but civil | Very angry, strong language",
    },
    Lesson {
        title: "Criteria sharpen the question",
        body: &[
            "Every question type takes criteria: `yes:`/`no:` for a noul, option descriptions for a choice, level descriptions for a score.",
            "Vague criteria are the usual cause of a low-confidence answer. Say what a yes actually requires.",
            "Instructions and criteria also accept JSON objects, for rubrics with structure: `:noul refund {\"task\": \"…\", \"ignore\": [\"signatures\"]}`.",
            "If the pipes are a lot to remember, `:build` opens the same question as a form with the JSON beside it.",
        ],
        try_this: ":noul is_urgent The message conveys urgency | yes: Explicit deadline, or money being lost right now | no: Routine question with no time pressure",
    },
    Lesson {
        title: "The wire format",
        body: &[
            "`:json` prints the exact body this session POSTs to /v1/systemone: your state, the model, and questions as name → {type, instructions, criteria}.",
            "Answers come back keyed by the same names, which is why names are yours to choose and worth keeping stable.",
            "`:last` prints the last response body verbatim, next to the typed answers the SDK decoded from it.",
        ],
        try_this: ":json",
    },
    Lesson {
        title: "What a call costs",
        body: &[
            "Every question is paid for twice: once in the request that carries it, once in the answer it asks for. A choice over eight labels comes back with eight probabilities; a score echoes its whole legend.",
            "`:cost` estimates both sides, per question, so an expensive question is visible before it is sent. The tokens are estimated from the body — the `usage` on a live answer is the counted truth.",
            "Rates are yours to supply, in dollars per million tokens: `:cost 0.20/1.00`, or `JEV_PRICE=0.20/1.00` in the environment. Nothing here guesses what a model charges.",
        ],
        try_this: ":cost",
    },
    Lesson {
        title: "Models and per-call options",
        body: &[
            "`jev-latest` is an alias that moves; pin a version when you need reproducibility.",
            "`:models` lists what the account can use, `:model jev-2` switches this session, and `:timeout 3` changes the per-attempt timeout the way `.timeout()` does on a call.",
            "Retries are on by default: 2 retries, exponential backoff, a 30 s budget, and Retry-After is honoured.",
        ],
        try_this: ":models",
    },
    Lesson {
        title: "Out of the REPL",
        body: &[
            "`:rust` prints this session as a compiling program against this SDK — the same state, questions and model, with the answer lookups filled in.",
            "That is the whole loop: shape the questions here where iterating is cheap, then paste the generated code into your service.",
        ],
        try_this: ":rust",
    },
];
