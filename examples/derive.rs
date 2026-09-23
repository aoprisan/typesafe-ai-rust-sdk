//! cargo run --example derive --features derive  (needs TYPESAFE_API_KEY)
//! The rubric as a struct: its fields are the questions, and the answers come back into it. A
//! misspelled question name or an answer read as the wrong type no longer compiles.
use typesafe::{ChoiceOf, Client, NoulAnswer, Rubric, RubricChoice, ScoreAnswer};

#[derive(Debug, Rubric)]
struct Triage {
    #[noul(
        "The message conveys urgency",
        yes = "A deadline, a threat to leave, or \"ASAP\"",
        no = "Routine, no time pressure"
    )]
    is_urgent: NoulAnswer,

    #[choice("Which team should handle this")]
    department: ChoiceOf<Department>,

    #[score(
        "How frustrated the customer appears",
        levels = ["Calm", "Frustrated but civil", "Very angry"]
    )]
    frustration: ScoreAnswer,

    /// The customer asks for their money back
    #[noul]
    #[rubric(rename = "wants_refund")]
    refund: f64, // the probability of "yes", when that is all you need
}

/// The options of the `department` choice. The label is the variant name in snake_case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, RubricChoice)]
enum Department {
    #[option("Payment or subscription issues")]
    Billing,
    /// Bugs or integration problems
    Technical,
    #[option("Pricing or account questions")]
    Sales,
}

#[tokio::main]
async fn main() -> typesafe::Result<()> {
    // What goes on the wire, derived from the struct: print it before paying for it.
    println!(
        "{}",
        typesafe::serde_json::to_string_pretty(&Triage::questions()).expect("questions encode")
    );

    let client = Client::from_env()?;
    let triage: Triage = client
        .ask("The payout failed again, third time this month. I'm done waiting. Refund me.")
        .await?;

    println!(
        "department  {:?} (confidence {:.2})",
        *triage.department,
        triage.department.confidence()
    );
    println!("frustration {:.2}", triage.frustration.score);
    println!("urgent      {}", triage.is_urgent.is_yes(0.8));
    println!("refund      {:.2}", triage.refund);

    // The enum is a plain enum: match on it, exhaustively.
    match *triage.department {
        Department::Billing if triage.frustration.score >= 1.5 => println!("→ payments on-call"),
        Department::Billing => println!("→ billing queue"),
        Department::Technical => println!("→ engineering triage"),
        Department::Sales => println!("→ account manager"),
    }
    Ok(())
}
