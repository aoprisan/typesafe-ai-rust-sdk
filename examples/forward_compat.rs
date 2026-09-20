//! cargo run --example forward_compat  (needs TYPESAFE_API_KEY)
//! Using API features this SDK version does not model: a hand-built question, an extra body
//! field, and the raw body for answers the typed map does not recognise.
use typesafe::{Answer, Client, Noul, Questions, json};

#[tokio::main]
async fn main() -> typesafe::Result<()> {
    let client = Client::from_env()?;

    let res = client
        .system_one(
            "The payout failed again, third time this month. I am done waiting.",
            Questions::new()
                .with("is_urgent", Noul::new("The message conveys urgency"))
                // Any JSON object with a non-empty `type` is sent as written. Use this for
                // question types or fields released after this SDK version.
                .with(
                    "tone",
                    json!({
                        "type": "choice",
                        "instructions": "The register of the message",
                        "criteria": {"formal": null, "casual": null, "hostile": null},
                        "some_new_field": true,
                    }),
                ),
        )
        // Top-level body fields, shallow-merged last. A key named `state`, `model` or
        // `questions` replaces the standard field, so keep to new names.
        .extra_body("trace", json!(true))
        .await?;

    // Answers of a type this version knows become typed values...
    for (name, answer) in &res.answers {
        let confidence = answer
            .confidence()
            .map_or_else(String::new, |c| format!(" (confidence {c:.2})"));
        match answer {
            Answer::Noul(a) => println!("{name} [noul] {:.3}{confidence}", a.noul),
            Answer::Choice(a) => println!("{name} [choice] {}{confidence}", a.choice),
            Answer::Score(a) => println!("{name} [score] {:.2}{confidence}", a.score),
            // `Answer` is `#[non_exhaustive]`; later versions add variants here.
            other => println!("{name} [{}]", other.kind()),
        }
    }

    // ...and anything else is skipped with a `tracing` warning, but is still in `raw`, along
    // with every field of the response, known or not.
    if let Some(answers) = res.raw["answers"].as_object() {
        for (name, value) in answers {
            if !res.answers.contains_key(name.as_str()) {
                println!("{name} [unrecognised] {value}");
            }
        }
    }
    println!("model {}, usage {:?}", res.model, res.usage);
    Ok(())
}
