//! Ready-made sessions to poke at: `:preset <name>`.

pub struct Preset {
    pub name: &'static str,
    pub about: &'static str,
    /// Commands replayed as if typed.
    pub script: &'static [&'static str],
}

pub const PRESETS: &[Preset] = &[
    Preset {
        name: "triage",
        about: "Route a support ticket: department, frustration, urgency",
        script: &[
            ":state Hi, I've been trying to connect my Stripe account for 3 days and it keeps failing. I'm losing sales. Please help ASAP.",
            ":choice department Which team should handle this | billing=Payment or subscription issues | technical=Bugs or integration problems | sales=Pricing or account questions",
            ":score frustration How frustrated the customer appears | Calm, just stating facts | Frustrated but civil | Very angry, strong language",
            ":noul is_urgent The message conveys urgency or time-sensitivity",
        ],
    },
    Preset {
        name: "moderation",
        about: "Screen user content before it is published",
        script: &[
            ":state You people are useless. Fix my order or I'll come down there myself.",
            ":noul is_threat The message threatens violence against a person | yes: A stated intent to harm someone | no: Anger, insults or profanity with no threat of harm",
            ":score severity How far out of bounds this is | Fine as written | Rude but publishable | Abusive, needs review | Remove immediately",
            ":choice action What to do with this content | publish=Nothing wrong with it | flag=A human should look | block=Clearly violates the rules",
        ],
    },
    Preset {
        name: "lead",
        about: "Qualify an inbound sales email",
        script: &[
            ":state Hi — we're a 300-person logistics company evaluating vendors this quarter. Budget is approved. Can you send pricing for 250 seats?",
            ":noul has_budget The sender indicates budget is available or approved",
            ":score readiness How close this is to a buying decision | Just browsing | Researching options | Actively evaluating vendors | Ready to buy now",
            ":choice size Company size implied by the message | smb=Under 50 people | mid=50 to 1000 people | enterprise=Over 1000 people",
        ],
    },
    Preset {
        name: "reply",
        about: "Grade a draft reply before it is sent",
        script: &[
            ":state Draft reply: \"That's not our problem. You configured it wrong. Read the docs.\"",
            ":noul answers_question The reply actually addresses what was asked",
            ":score tone Tone of the reply | Warm and helpful | Neutral | Curt | Hostile",
            ":noul safe_to_send This reply can go out without a human reading it first | yes: Accurate, on-topic and civil | no: Rude, evasive or likely to make things worse",
        ],
    },
];

pub fn find(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name)
}
