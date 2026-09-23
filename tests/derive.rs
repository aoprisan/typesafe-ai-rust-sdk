//! `#[derive(Rubric)]` and `#[derive(RubricChoice)]` against a mock server.
#![cfg(feature = "derive")]

use serde_json::json;
use typesafe::rubric::UnknownLabel;
use typesafe::{
    ChoiceAnswer, ChoiceOf, Client, Error, NoulAnswer, RetryPolicy, Rubric, RubricChoice,
    ScoreAnswer,
};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Debug, Rubric)]
struct Triage {
    #[noul("The message conveys urgency", yes = "A deadline", no = "Routine")]
    is_urgent: NoulAnswer,
    #[choice("Which team should handle this")]
    department: ChoiceOf<Department>,
    #[score("How frustrated", levels = ["Calm", "Frustrated", "Very angry"])]
    frustration: ScoreAnswer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, RubricChoice)]
enum Department {
    #[option("Payment or subscription issues")]
    Billing,
    /// Bugs or integration
    /// problems
    Technical,
    #[rubric(rename = "sales_team")]
    Sales,
}

/// Every other field type and spelling the derive accepts.
#[derive(Debug, Rubric)]
struct Loose {
    /// Wants money back
    #[noul]
    #[rubric(rename = "refund")]
    wants_refund: f64,
    #[choice("Sentiment", labels = ["positive", "negative"])]
    sentiment: String,
    #[choice("Which team")]
    team: Department,
    #[choice("Tone", labels = ["warm", "cold"])]
    tone: ChoiceAnswer,
    #[score("Severity", levels = ["Low", "High"])]
    severity: f64,
}

fn triage_body() -> serde_json::Value {
    json!({
        "model": "jev-latest",
        "answers": {
            "is_urgent": {"type": "noul", "noul": 0.93},
            "department": {"type": "choice", "choice": "technical",
                "probabilities": {"billing": 0.1, "technical": 0.85, "sales_team": 0.05}, "confidence": 0.7},
            "frustration": {"type": "score", "score": 1.4,
                "legend": {"0": "Calm", "1": "Frustrated", "2": "Very angry"},
                "probabilities": {"0": 0.1, "1": 0.4, "2": 0.5}, "confidence": 0.6}
        },
        "usage": {}
    })
}

async fn serve(body: serde_json::Value) -> (MockServer, Client) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test")
        .base_url(server.uri())
        .retry(RetryPolicy::none())
        .build()
        .unwrap();
    (server, client)
}

#[test]
fn questions_follow_the_struct() {
    assert_eq!(
        serde_json::to_value(Triage::questions()).unwrap(),
        json!({
            "is_urgent": {"type": "noul", "instructions": "The message conveys urgency",
                "criteria": {"true": "A deadline", "false": "Routine"}},
            "department": {"type": "choice", "instructions": "Which team should handle this",
                "criteria": {"billing": "Payment or subscription issues",
                    "technical": "Bugs or integration problems", "sales_team": null}},
            "frustration": {"type": "score", "instructions": "How frustrated",
                "criteria": ["Calm", "Frustrated", "Very angry"]}
        })
    );
    let names: Vec<_> = Triage::questions()
        .iter()
        .map(|(n, _)| n.to_owned())
        .collect();
    assert_eq!(names, ["is_urgent", "department", "frustration"]);

    assert_eq!(
        serde_json::to_value(Loose::questions()).unwrap(),
        json!({
            "refund": {"type": "noul", "instructions": "Wants money back"},
            "sentiment": {"type": "choice", "instructions": "Sentiment",
                "criteria": {"positive": null, "negative": null}},
            "team": {"type": "choice", "instructions": "Which team",
                "criteria": {"billing": "Payment or subscription issues",
                    "technical": "Bugs or integration problems", "sales_team": null}},
            "tone": {"type": "choice", "instructions": "Tone", "criteria": {"warm": null, "cold": null}},
            "severity": {"type": "score", "instructions": "Severity", "criteria": ["Low", "High"]}
        })
    );
}

#[test]
fn choice_enums_round_trip_their_labels() {
    assert_eq!(
        Department::OPTIONS,
        &[
            ("billing", Some("Payment or subscription issues")),
            ("technical", Some("Bugs or integration problems")),
            ("sales_team", None),
        ]
    );
    assert_eq!(Department::Sales.label(), "sales_team");
    assert_eq!(
        Department::from_label("technical"),
        Some(Department::Technical)
    );
    // `FromStr` comes with the derive, so `ChoiceAnswer::parse` works as it always did.
    assert_eq!("billing".parse::<Department>(), Ok(Department::Billing));
    let err: UnknownLabel = "Billing".parse::<Department>().unwrap_err();
    assert_eq!(
        err.to_string(),
        r#"unknown label "Billing"; expected one of "billing", "technical", "sales_team""#
    );
}

#[tokio::test]
async fn ask_sends_the_rubric_and_decodes_the_answers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(body_json(json!({
            "state": "The payout failed again.",
            "model": "jev-2",
            "questions": serde_json::to_value(Triage::questions()).unwrap(),
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(triage_body()))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test")
        .base_url(server.uri())
        .build()
        .unwrap();

    let t: Triage = client
        .ask("The payout failed again.")
        .model("jev-2")
        .await
        .unwrap();
    assert_eq!(t.is_urgent.noul, 0.93);
    assert_eq!(*t.department, Department::Technical);
    assert_eq!(t.department.confidence(), 0.7);
    assert_eq!(t.department.probability(&Department::Sales), Some(0.05));
    assert_eq!(t.frustration.most_likely_level(), Some(2));
}

#[tokio::test]
async fn every_field_type_decodes() {
    let (_server, client) = serve(json!({
        "model": "jev-latest",
        "answers": {
            "refund": {"type": "noul", "noul": 0.25},
            "sentiment": {"type": "choice", "choice": "negative",
                "probabilities": {"positive": 0.2, "negative": 0.8}, "confidence": 0.5},
            "team": {"type": "choice", "choice": "billing",
                "probabilities": {"billing": 1.0}, "confidence": 1.0},
            "tone": {"type": "choice", "choice": "cold",
                "probabilities": {"warm": 0.4, "cold": 0.6}, "confidence": 0.2},
            "severity": {"type": "score", "score": 0.75, "legend": {"0": "Low", "1": "High"},
                "probabilities": {"0": 0.25, "1": 0.75}, "confidence": 0.5}
        }
    }))
    .await;
    let l: Loose = client.ask("text").await.unwrap();
    assert_eq!(l.wants_refund, 0.25);
    assert_eq!(l.sentiment, "negative");
    assert_eq!(l.team, Department::Billing);
    assert_eq!(l.tone.probability("warm"), Some(0.4));
    assert_eq!(l.severity, 0.75);
}

fn validation(err: Error) -> (String, String) {
    match err {
        Error::ResponseValidation(e) => (e.field_path, e.detail),
        other => panic!("expected a ResponseValidation error, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unknown_label_names_the_field() {
    let mut body = triage_body();
    body["answers"]["department"]["choice"] = json!("legal");
    let (_server, client) = serve(body).await;
    let (path, detail) = validation(client.ask::<Triage>("x").await.unwrap_err());
    assert_eq!(path, "answers.department.choice");
    assert!(detail.contains(r#"unknown label "legal""#), "{detail}");
}

#[tokio::test]
async fn a_missing_or_mistyped_answer_names_the_field() {
    let mut body = triage_body();
    body["answers"].as_object_mut().unwrap().remove("is_urgent");
    let (_server, client) = serve(body).await;
    let (path, detail) = validation(client.ask::<Triage>("x").await.unwrap_err());
    assert_eq!(path, "answers.is_urgent");
    assert_eq!(detail, "no answer; expected a noul");

    let mut body = triage_body();
    body["answers"]["frustration"] = json!({"type": "noul", "noul": 0.5});
    let (_server, client) = serve(body).await;
    let (path, detail) = validation(client.ask::<Triage>("x").await.unwrap_err());
    assert_eq!(path, "answers.frustration");
    assert_eq!(detail, "expected a score answer, got a noul");

    let mut body = triage_body();
    body["answers"]["is_urgent"] = json!({"type": "span", "start": 1});
    let (_server, client) = serve(body).await;
    let (path, detail) = validation(client.ask::<Triage>("x").await.unwrap_err());
    assert_eq!(path, "answers.is_urgent");
    assert!(detail.contains("does not know"), "{detail}");
}

#[tokio::test]
async fn from_response_decodes_a_response_you_already_have() {
    let (_server, client) = serve(triage_body()).await;
    let res = client.system_one("x", Triage::questions()).await.unwrap();
    let t = Triage::from_response(&res).unwrap();
    assert_eq!(t.department.value, Department::Technical);
    assert_eq!(
        res.choice("department").unwrap().parse(),
        Ok(Department::Technical)
    );
}

#[cfg(feature = "blocking")]
#[test]
fn blocking_ask() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (_server, client) = rt.block_on(serve(triage_body()));
    let client = typesafe::blocking::Client::new(client).unwrap();
    let t: Triage = client.ask("x").send().unwrap();
    assert_eq!(*t.department, Department::Technical);
}
