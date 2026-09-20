//! cargo run --example retries  (needs TYPESAFE_API_KEY)
//! Two callers of the same API want opposite things from a retry: a background job wants the
//! answer eventually, a request behind a user wants an answer now or not at all.
use std::time::Duration;

use typesafe::{ApiErrorKind, Client, Error, Noul, Questions, RetryPolicy};

const TEXT: &str = "Does the export honour custom fields? The changelog does not say.";

#[tokio::main]
async fn main() -> typesafe::Result<()> {
    // Patient: bounded by a total budget rather than by a count, and willing to sit out a long
    // `Retry-After`. `retry_if` adds to the built-in rules, it does not replace them.
    let patient = RetryPolicy::default()
        .max_retries(5)
        .backoff(Duration::from_millis(200), Duration::from_secs(4))
        .jitter(0.5)
        .budget(Some(Duration::from_secs(20)))
        .retry_if(|err| err.status() == Some(409));

    let client = Client::builder()
        .retry(patient)
        .timeout(Duration::from_secs(8))
        .build()?;

    let questions = Questions::from([("is_question", Noul::new("The text asks something"))]);

    match client.system_one(TEXT, questions.clone()).await {
        Ok(res) => println!(
            "background: {:.2} after {} attempt(s)",
            res.noul("is_question").expect("asked, so answered").noul,
            res.meta.attempts, // 1 means it worked first time; retries send X-TypeSafe-Retry-Count
        ),
        // Every retry the policy allowed is already spent by the time this arrives.
        Err(Error::Api(api)) if api.kind == ApiErrorKind::RateLimit => {
            println!(
                "background: still rate limited; the server wants {:?}",
                api.retry_after()
            );
        }
        Err(err) => return Err(err),
    }

    // Impatient: in front of a user, a late answer is a wrong answer. One attempt, two seconds,
    // and a fallback for everything else. Per-call options override the client's.
    let interactive = client
        .system_one(TEXT, questions)
        .retry(RetryPolicy::none())
        .timeout(Duration::from_secs(2))
        .await;

    match interactive {
        Ok(res) => println!(
            "interactive: {:.2}",
            res.noul("is_question").expect("asked, so answered").noul
        ),
        Err(Error::Timeout(after)) => println!("interactive: gave up after {after:?}, carrying on"),
        Err(err) => println!("interactive: {err}, carrying on"),
    }
    Ok(())
}
