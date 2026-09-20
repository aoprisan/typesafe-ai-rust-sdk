//! cargo run --example models  (needs TYPESAFE_API_KEY)
//! The smallest call there is: which models this account can use.
use typesafe::Client;

#[tokio::main]
async fn main() -> typesafe::Result<()> {
    let client = Client::from_env()?;
    let res = client.models().list().await?;

    println!("default model: {}", client.default_model());
    for m in &res.models {
        println!("  {:<14} {:<12} {}", m.name, m.release_date, m.description);
    }
    println!(
        "{} model(s) in {} attempt(s), request id {:?}",
        res.models.len(),
        res.meta.attempts,
        res.meta.request_id()
    );
    Ok(())
}
