//! cargo run --example blocking --features blocking  (needs TYPESAFE_API_KEY)
//! No async runtime in sight: the same API from an ordinary `fn main`.
use std::time::Duration;

use typesafe::blocking::Client;
use typesafe::{Choice, Noul, Questions, Score};

fn main() -> typesafe::Result<()> {
    // The blocking client owns a private current-thread runtime, so build it once and keep it;
    // never call it from inside async code.
    let client = Client::from_env()?;

    for model in client.models().list().send()?.models {
        println!("model {}", model.name);
    }

    let res = client
        .system_one(
            "The payout failed again, third time this month. I am done waiting.",
            Questions::new()
                .with(
                    "department",
                    Choice::new("Which team should handle this")
                        .option("billing", "Payment or subscription issues")
                        .option("technical", "Bugs or integration problems"),
                )
                .with(
                    "frustration",
                    Score::new(
                        "How frustrated the writer sounds",
                        ["Calm", "Frustrated but civil", "Very angry"],
                    ),
                )
                .with("is_urgent", Noul::new("The message conveys urgency")),
        )
        .timeout(Duration::from_secs(5)) // the same per-call options as the async request
        .send()?;

    println!(
        "department  {}",
        res.choice("department").expect("asked").choice
    );
    println!(
        "frustration {:.2}",
        res.score("frustration").expect("asked").score
    );
    println!(
        "urgent      {}",
        res.noul("is_urgent").expect("asked").is_yes(0.8)
    );
    Ok(())
}
