//! cargo run --example concurrent  (needs TYPESAFE_API_KEY)
//! One client, many documents: clones are cheap and share the connection pool, so each
//! document gets its own task and one failure does not sink the batch.
use std::collections::BTreeMap;
use std::time::Duration;

use typesafe::{Choice, Client, Noul, Questions};

const FEEDBACK: [&str; 5] = [
    "Setup took ten minutes and the docs were right about everything. Delighted.",
    "It works, I suppose. Nothing here I could not have written myself in an afternoon.",
    "Third outage this month. We are already looking at alternatives.",
    "Love the product, but the invoice says 12 seats and we have 9. Who do I talk to?",
    "Does the export honour custom fields? Cannot tell from the changelog.",
];

#[tokio::main]
async fn main() -> typesafe::Result<()> {
    let client = Client::from_env()?;
    let questions = Questions::new()
        .with(
            "sentiment",
            Choice::from_labels(
                "How the writer feels about the product",
                ["positive", "neutral", "negative"],
            ),
        )
        .with(
            "needs_reply",
            Noul::new("The writer is waiting for an answer from us"),
        );

    // Fan out. `Client` is `Arc` inside, so cloning it does not open new connections.
    let handles: Vec<_> = FEEDBACK
        .iter()
        .map(|text| {
            let (client, questions) = (client.clone(), questions.clone());
            tokio::spawn(async move {
                client
                    .system_one(*text, questions)
                    .timeout(Duration::from_secs(5))
                    .await
            })
        })
        .collect();

    // Gather. Each document reports its own outcome; a failed one is skipped, not fatal.
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    let mut replies = Vec::new();
    for (text, handle) in FEEDBACK.iter().zip(handles) {
        let snippet: String = text.chars().take(44).collect();
        match handle.await.expect("the task did not panic") {
            Ok(res) => {
                let sentiment = res.choice("sentiment").expect("asked, so answered");
                let needs_reply = res.noul("needs_reply").expect("asked, so answered");
                *tally.entry(sentiment.choice.clone()).or_default() += 1;
                if needs_reply.is_yes(0.6) {
                    replies.push(snippet.clone());
                }
                println!(
                    "{:<9} {:.2}  reply? {:.2}  {snippet}…",
                    sentiment.choice, sentiment.confidence, needs_reply.noul
                );
            }
            Err(err) => eprintln!("skipped ({err})  {snippet}…"),
        }
    }

    println!("\n{tally:?}");
    println!("{} of {} want an answer", replies.len(), FEEDBACK.len());
    Ok(())
}
