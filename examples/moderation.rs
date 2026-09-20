//! cargo run --example moderation  (needs TYPESAFE_API_KEY)
//! Several yes/no questions in one call, each read against its own threshold.
use typesafe::{Client, Noul, Questions};

/// name, question, what a yes means, threshold at or above which the flag fires
const CHECKS: [(&str, &str, &str, f64); 4] = [
    (
        "harassment",
        "The text attacks, insults or demeans a person",
        "Directed at someone, not just angry in general",
        0.7,
    ),
    (
        "self_promotion",
        "The text exists mainly to advertise something",
        "A link, a discount code, or a pitch with no other content",
        0.8,
    ),
    (
        "off_topic",
        "The text is unrelated to the product being reviewed",
        "About shipping, the seller, or something else entirely",
        0.6,
    ),
    (
        "needs_human",
        "The text describes a situation a moderator should look at",
        "Threats, self-harm, or anything legally sensitive",
        0.3, // deliberately low: err towards a human read
    ),
];

const REVIEW: &str = "Shipping took two weeks and the box was crushed. The lamp itself is fine, \
                      but whoever packed it clearly did not care. Use CODE20 at checkout on my \
                      site for a better one.";

#[tokio::main]
async fn main() -> typesafe::Result<()> {
    let client = Client::from_env()?;

    let mut questions = Questions::new();
    for (name, instructions, yes_means, _) in CHECKS {
        questions.insert(name, Noul::new(instructions).when_true(yes_means));
    }

    let res = client.system_one(REVIEW, questions).await?;

    let mut flagged = Vec::new();
    for (name, _, _, threshold) in CHECKS {
        let answer = res.noul(name).expect("asked, so answered");
        let verdict = if answer.is_yes(threshold) {
            flagged.push(name);
            "FLAG"
        } else {
            "ok"
        };
        println!("{name:<15} {:.3}  (>= {threshold})  {verdict}", answer.noul);
    }

    // Or ignore the thresholds and just look at what worried the model most.
    if let Some((name, answer)) = res.nouls().max_by(|a, b| a.1.noul.total_cmp(&b.1.noul)) {
        println!("strongest signal: {name} at {:.3}", answer.noul);
    }

    println!(
        "{}",
        match flagged.as_slice() {
            [] => "publish".to_owned(),
            flags => format!("hold for review: {}", flags.join(", ")),
        }
    );
    Ok(())
}
