//! cargo run --example triage  (needs TYPESAFE_API_KEY)
//! Confidence-gated routing: act on the answer only when the model is sure.
use typesafe::{Choice, Client, Noul, Questions, Score};

#[tokio::main]
async fn main() -> typesafe::Result<()> {
    let client = Client::from_env()?;
    let ticket = "Hi, I've been trying to connect my Stripe account for 3 days and it keeps failing. \
                  I'm losing sales. Please help ASAP.";

    let res = client
        .system_one(
            ticket,
            Questions::new()
                .with(
                    "department",
                    Choice::new("Which team should handle this")
                        .option("billing", "Payment or subscription issues")
                        .option("technical", "Bugs or integration problems")
                        .option("sales", "Pricing or account questions"),
                )
                .with(
                    "frustration",
                    Score::new(
                        "How frustrated the customer appears",
                        [
                            "Calm, just stating facts",
                            "Frustrated but civil",
                            "Very angry, strong language",
                        ],
                    ),
                )
                .with(
                    "is_urgent",
                    Noul::new("The message conveys urgency or time-sensitivity"),
                ),
        )
        .await?;

    let dept = res.choice("department").expect("asked");
    let route = if dept.confidence >= 0.5 {
        dept.choice.as_str()
    } else {
        "human-review"
    };
    println!("route → {route} (confidence {:.2})", dept.confidence);
    println!(
        "frustration {:.2}",
        res.score("frustration").expect("asked").score
    );
    println!(
        "urgent? {}",
        res.noul("is_urgent").expect("asked").is_yes(0.8)
    );
    println!("request id {:?}, usage {:?}", res.request_id(), res.usage);
    Ok(())
}
