//! cargo run --example structured_state  (needs TYPESAFE_API_KEY)
//! The state is your own struct, the rubrics are JSON, and the answer parses into an enum.
use std::str::FromStr;

use serde::Serialize;
use typesafe::{Choice, Client, Questions, Score, json};

#[derive(Serialize)]
struct Ticket<'a> {
    subject: &'a str,
    plan: &'a str,
    opened_days_ago: u32,
    messages: Vec<Message<'a>>,
}

#[derive(Serialize)]
struct Message<'a> {
    from: &'a str,
    body: &'a str,
}

/// The labels a `Choice` may come back with, as a type the rest of the program can match on.
#[derive(Debug, PartialEq, Eq)]
enum Department {
    Billing,
    Technical,
    Sales,
}

impl FromStr for Department {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "billing" => Ok(Department::Billing),
            "technical" => Ok(Department::Technical),
            "sales" => Ok(Department::Sales),
            other => Err(format!(
                "the model answered with an unknown label: {other:?}"
            )),
        }
    }
}

#[tokio::main]
async fn main() -> typesafe::Result<()> {
    let client = Client::from_env()?;

    // Anything `Serialize` is a valid state: it reaches the model as JSON, keys and all.
    let ticket = Ticket {
        subject: "Payouts still failing",
        plan: "enterprise",
        opened_days_ago: 3,
        messages: vec![
            Message {
                from: "customer",
                body: "Stripe connect fails with 'account not linked'. Third day now.",
            },
            Message {
                from: "support",
                body: "Could you re-authorise the connection and let us know?",
            },
            Message {
                from: "customer",
                body: "Did that twice. We are losing sales while this sits here.",
            },
        ],
    };

    let res = client
        .system_one(
            ticket,
            Questions::new()
                .with(
                    "department",
                    Choice::new(json!({
                        "task": "Route this ticket to the team that can resolve it",
                        "read": "the customer messages only; ignore support replies",
                    }))
                    .option("billing", "Payments, invoices, subscriptions, payouts")
                    .option("technical", "Bugs, integrations, API errors")
                    .option("sales", "Pricing, plan changes, new purchases"),
                )
                .with(
                    // Instructions and every level may be structured, which keeps a long rubric
                    // readable and lets you version it alongside the code.
                    "escalation",
                    Score::new(
                        json!({
                            "task": "How far up this should go",
                            "weigh": ["plan", "days open", "tone of the last message"],
                        }),
                        [
                            json!({"level": "queue", "means": "ordinary turnaround is fine"}),
                            json!({"level": "priority", "means": "same-day answer expected"}),
                            json!({"level": "on-call", "means": "page someone now"}),
                        ],
                    ),
                ),
        )
        .await?;

    let choice = res.choice("department").expect("asked, so answered");
    let department: Department = choice.parse().expect("a label we defined");
    println!(
        "department: {department:?} (confidence {:.2})",
        choice.confidence
    );
    for (label, probability) in choice.ranked() {
        println!("  {label:<10} {probability:.3}");
    }

    let escalation = res.score("escalation").expect("asked, so answered");
    let level = escalation.rounded_level();
    println!(
        "escalation: {:.2} → level {level} {}",
        escalation.score,
        escalation
            .legend
            .get(&level)
            .map(ToString::to_string)
            .unwrap_or_default()
    );

    match department {
        Department::Billing if escalation.score >= 1.5 => println!("→ page the payments on-call"),
        Department::Billing => println!("→ billing queue"),
        Department::Technical => println!("→ engineering triage"),
        Department::Sales => println!("→ account manager"),
    }
    Ok(())
}
