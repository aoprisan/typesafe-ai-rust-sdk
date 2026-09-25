use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use typesafe::http::header::{AUTHORIZATION, HeaderName, HeaderValue};
use typesafe::{
    ApiErrorKind, Choice, Client, Error, Noul, Questions, RetryPolicy, Score, StatusCode,
};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn ok_body() -> serde_json::Value {
    json!({
        "model": "jev-latest",
        "answers": {
            "department": {"type": "choice", "choice": "technical",
                "probabilities": {"billing": 0.159, "technical": 0.84, "sales": 0.001}, "confidence": 0.596},
            "frustration": {"type": "score", "score": 1.035,
                "legend": {"0": "Calm", "1": "Frustrated", "2": "Very angry"},
                "probabilities": {"0": 0.1, "1": 0.76, "2": 0.14}, "confidence": 0.842},
            "is_urgent": {"type": "noul", "noul": 0.999}
        },
        "usage": {"input_tokens": 312, "output_tokens": 48}
    })
}

fn questions() -> Questions {
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
            Score::new("How frustrated", ["Calm", "Frustrated", "Very angry"]),
        )
        .with("is_urgent", Noul::new("The message conveys urgency"))
}

fn client(server: &MockServer) -> Client {
    Client::builder()
        .api_key("sk-test")
        .base_url(format!("{}/", server.uri()))
        .retry(RetryPolicy::default().backoff(Duration::from_millis(1), Duration::from_millis(5)))
        .build()
        .unwrap()
}

#[tokio::test]
async fn system_one_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(header("authorization", "Bearer sk-test"))
        .and(header("content-type", "application/json"))
        .and(header("accept", "application/json"))
        .and(header("x-typesafe-sdk", format!("typesafe-sdk-rust/{}", typesafe::constants::VERSION).as_str()))
        .and(body_json(json!({
            "state": "Stripe keeps failing. ASAP.",
            "model": "jev-latest",
            "questions": {
                "department": {"type": "choice", "instructions": "Which team should handle this", "criteria": {
                    "billing": "Payment or subscription issues",
                    "technical": "Bugs or integration problems",
                    "sales": "Pricing or account questions"}},
                "frustration": {"type": "score", "instructions": "How frustrated",
                    "criteria": ["Calm", "Frustrated", "Very angry"]},
                "is_urgent": {"type": "noul", "instructions": "The message conveys urgency"}
            }
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_body())
                .insert_header("x-typesafe-request-id", "req_123"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let res = client(&server)
        .system_one("Stripe keeps failing. ASAP.", questions())
        .await
        .unwrap();

    assert_eq!(res.model, "jev-latest");
    assert_eq!(res.request_id(), Some("req_123"));
    assert_eq!(res.meta.attempts, 1);
    assert_eq!(res.usage.output_tokens, Some(48));
    assert_eq!(res.choice("department").unwrap().choice, "technical");
    assert_eq!(
        res.score("frustration").unwrap().most_likely_level(),
        Some(1)
    );
    assert!(res.noul("is_urgent").unwrap().is_yes(0.5));
    assert!(res.noul("department").is_none());
    assert_eq!(res.nouls().count(), 1);

    let received = &server.received_requests().await.unwrap()[0];
    assert!(!received.headers.contains_key("x-typesafe-retry-count"));
}

#[derive(Serialize)]
struct Ticket<'a> {
    subject: &'a str,
    messages: Vec<&'a str>,
}

#[derive(Debug, PartialEq)]
enum Dept {
    Billing,
    Technical,
}

impl std::str::FromStr for Dept {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "billing" => Ok(Dept::Billing),
            "technical" => Ok(Dept::Technical),
            other => Err(other.to_owned()),
        }
    }
}

#[tokio::test]
async fn structured_state_overrides_and_protected_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(header("authorization", "Bearer sk-test"))
        .and(header("x-tenant", "acme"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;

    let res = client(&server)
        .system_one(
            Ticket {
                subject: "Payouts",
                messages: vec!["hi", "help"],
            },
            questions(),
        )
        .model("jev-2")
        .extra_body("metadata", json!({"trace": "t1"}))
        .header(AUTHORIZATION, HeaderValue::from_static("Bearer hijack"))
        .header(
            HeaderName::from_static("x-tenant"),
            HeaderValue::from_static("acme"),
        )
        .header(
            HeaderName::from_static("x-typesafe-retry-count"),
            HeaderValue::from_static("9"),
        )
        .timeout(Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(
        res.choice("department").unwrap().parse::<Dept>(),
        Ok(Dept::Technical)
    );

    let req = &server.received_requests().await.unwrap()[0];
    let body: serde_json::Value = req.body_json().unwrap();
    assert_eq!(
        body["state"],
        json!({"subject": "Payouts", "messages": ["hi", "help"]})
    );
    assert_eq!(body["model"], "jev-2");
    assert_eq!(body["metadata"], json!({"trace": "t1"}));
    assert!(!req.headers.contains_key("x-typesafe-retry-count"));
}

#[tokio::test]
async fn wire_body_keeps_question_order_and_extra_body_replaces_fields() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;

    client(&server)
        .system_one("x", questions())
        .extra_body("model", json!("override"))
        .extra_body("future", json!(1))
        .await
        .unwrap();

    let req = &server.received_requests().await.unwrap()[0];
    let text = String::from_utf8(req.body.clone()).unwrap();
    let pos = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing"))
    };
    assert!(pos("\"state\"") < pos("\"questions\""));
    assert!(pos("\"department\"") < pos("\"frustration\""));
    assert!(pos("\"frustration\"") < pos("\"is_urgent\""));
    assert_eq!(text.matches("\"model\":").count(), 1);
    let body: serde_json::Value = req.body_json().unwrap();
    assert_eq!(body["model"], "override");
    assert_eq!(body["future"], 1);
}

#[tokio::test]
async fn retries_429_honoring_retry_after_ms_then_succeeds() {
    struct Flaky(std::sync::atomic::AtomicU32);
    impl Respond for Flaky {
        fn respond(&self, _: &Request) -> ResponseTemplate {
            if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                ResponseTemplate::new(429)
                    .insert_header("retry-after-ms", "20")
                    .set_body_json(json!({"error": {"message": "slow down"}}))
            } else {
                ResponseTemplate::new(200).set_body_json(ok_body())
            }
        }
    }
    let server = MockServer::start().await;
    Mock::given(path("/v1/systemone"))
        .respond_with(Flaky(Default::default()))
        .expect(2)
        .mount(&server)
        .await;

    let t0 = std::time::Instant::now();
    let res = client(&server).system_one("x", questions()).await.unwrap();
    assert!(t0.elapsed() >= Duration::from_millis(20));
    assert_eq!(res.meta.attempts, 2);
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs[1].headers.get("x-typesafe-retry-count").unwrap(), "1");
}

#[tokio::test]
async fn overloaded_529_exhausts_retries() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(529).set_body_string("Overloaded"))
        .expect(3)
        .mount(&server)
        .await;

    let err = client(&server)
        .system_one("x", questions())
        .await
        .unwrap_err();
    let api = err.as_api().unwrap();
    assert_eq!(api.status.as_u16(), 529);
    assert_eq!(api.kind, ApiErrorKind::InternalServer);
    assert_eq!(api.message, "Overloaded");
    assert!(err.to_string().starts_with("POST http://127.0.0.1"));
    assert!(err.to_string().ends_with("/v1/systemone: 529 Overloaded"));
}

#[tokio::test]
async fn validation_422_is_not_retried_and_message_is_extracted() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(422)
                .insert_header("x-typesafe-request-id", "req_9")
                .set_body_json(json!({"detail": [
                    {"loc": ["body", "questions", "frustration", "criteria"],
                     "msg": "List should have at least 2 items", "type": "too_short"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let err = client(&server)
        .system_one("x", questions())
        .await
        .unwrap_err();
    let api = err.as_api().unwrap();
    assert_eq!(api.kind, ApiErrorKind::UnprocessableEntity);
    assert_eq!(
        api.message,
        "questions.frustration.criteria: List should have at least 2 items"
    );
    assert_eq!(err.request_id(), Some("req_9"));
    assert!(err.to_string().ends_with("(request_id=req_9)"));
}

#[tokio::test]
async fn retry_after_beyond_budget_stops_immediately() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "60"))
        .expect(1)
        .mount(&server)
        .await;

    let t0 = std::time::Instant::now();
    let err = client(&server)
        .system_one("x", questions())
        .await
        .unwrap_err();
    assert!(t0.elapsed() < Duration::from_secs(5));
    assert_eq!(
        err.as_api().unwrap().retry_after(),
        Some(Duration::from_secs(60))
    );
    assert_eq!(err.as_api().unwrap().message, "status code (no body)");
}

#[tokio::test]
async fn custom_predicate_retries_otherwise_final_status() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(409))
        .expect(2)
        .mount(&server)
        .await;

    let policy = RetryPolicy::default()
        .max_retries(1)
        .backoff(Duration::ZERO, Duration::ZERO)
        .retry_if(|e| e.status() == Some(StatusCode::CONFLICT));
    let err = client(&server)
        .system_one("x", questions())
        .retry(policy)
        .await
        .unwrap_err();
    assert_eq!(err.as_api().unwrap().kind, ApiErrorKind::Other);
}

#[tokio::test]
async fn timeouts_map_to_timeout_error() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_body())
                .set_delay(Duration::from_millis(500)),
        )
        .mount(&server)
        .await;

    let err = client(&server)
        .system_one("x", questions())
        .timeout(Duration::from_millis(50))
        .retry(RetryPolicy::none())
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::Timeout(d) if d == Duration::from_millis(50)),
        "{err:?}"
    );
}

#[tokio::test]
async fn connection_errors_are_retried_then_surface() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let c = Client::builder()
        .api_key("k")
        .base_url(format!("http://127.0.0.1:{port}"))
        .retry(RetryPolicy::default().backoff(Duration::from_millis(1), Duration::from_millis(1)))
        .build()
        .unwrap();
    let err = c.system_one("x", questions()).await.unwrap_err();
    assert!(matches!(err, Error::Connection(_)), "{err:?}");
}

#[tokio::test]
async fn invalid_success_body_reports_field_path() {
    let server = MockServer::start().await;
    let mut body = ok_body();
    body["answers"]["department"]
        .as_object_mut()
        .unwrap()
        .remove("confidence");
    Mock::given(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(&server)
        .await;

    match client(&server)
        .system_one("x", questions())
        .await
        .unwrap_err()
    {
        Error::ResponseValidation(e) => {
            assert_eq!(e.field_path, "answers.department.confidence");
            assert_eq!(e.status, StatusCode::OK);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn local_validation_happens_before_sending() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let c = client(&server);
    assert!(matches!(
        c.system_one("x", Questions::new()).await,
        Err(Error::InvalidRequest { .. })
    ));
    assert!(matches!(
        c.system_one("x", Questions::new().with("c", Choice::new("no options")))
            .await,
        Err(Error::InvalidRequest { .. })
    ));
    let q = Questions::new().with("raw", json!({"type": "score", "criteria": []}));
    assert!(matches!(
        c.system_one("x", q).await,
        Err(Error::InvalidRequest { .. })
    ));
}

#[tokio::test]
async fn lists_models() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models": [
            {"name": "jev-latest", "description": "Flagship", "release_date": "2026-05-01"}
        ]})))
        .mount(&server)
        .await;

    let res = client(&server).models().list().await.unwrap();
    assert_eq!(res.models[0].name, "jev-latest");
    let req = &server.received_requests().await.unwrap()[0];
    assert!(!req.headers.contains_key("content-type"));
}

#[test]
fn config_validation() {
    if std::env::var(typesafe::constants::API_KEY_ENV).is_err() {
        assert!(matches!(
            Client::builder().build(),
            Err(Error::Config { .. })
        ));
    }
    assert!(matches!(
        Client::builder()
            .api_key("k")
            .timeout(Duration::ZERO)
            .build(),
        Err(Error::Config { .. })
    ));
    for bad in ["", "not a url", "ftp://x", "http://"] {
        assert!(
            matches!(
                Client::builder().api_key("k").base_url(bad).build(),
                Err(Error::Config { .. })
            ),
            "{bad:?}"
        );
    }
    let c = Client::builder().api_key("k").build().unwrap();
    assert_eq!(c.default_model(), "jev-latest");
}

#[test]
fn api_key_is_trimmed_and_checked() {
    assert!(Client::builder().api_key("  sk-test\n").build().is_ok());
    for bad in ["", "   ", "sk test", "sk\ttest", "sk-\u{7f}", "sk-é"] {
        let err = Client::builder().api_key(bad).build().unwrap_err();
        assert!(matches!(err, Error::Config { .. }), "{bad:?}");
        let expected = if bad.trim().is_empty() {
            "no API key"
        } else {
            "printable ASCII"
        };
        assert!(err.to_string().contains(expected), "{bad:?}: {err}");
    }
}

#[tokio::test]
async fn errors_follow_the_conventions_and_keep_their_cause() {
    use std::error::Error as _;

    // The URL parser's error is the cause, not part of the message.
    let err = Client::builder()
        .api_key("k")
        .base_url("not a url")
        .build()
        .unwrap_err();
    assert_eq!(err.to_string(), "base_url \"not a url\" is not a valid URL");
    assert!(err.source().is_some(), "{err:?}");

    // State that cannot be JSON keeps serde_json's error.
    let state: std::collections::HashMap<(u8, u8), u8> = [((1, 2), 3)].into();
    let c = Client::builder().api_key("k").build().unwrap();
    let err = c.system_one(state, questions()).await.unwrap_err();
    assert!(matches!(err, Error::InvalidRequest { .. }), "{err:?}");
    assert_eq!(err.to_string(), "the state could not be encoded as JSON");
    assert!(err.source().unwrap().is::<typesafe::serde_json::Error>());

    // Lowercase, no trailing period.
    let err = c.system_one("x", Questions::new()).await.unwrap_err();
    assert_eq!(err.to_string(), "at least one question is required");
    assert_eq!(
        Error::Timeout(Duration::from_secs(10)).to_string(),
        "request timed out (timeout=10s)"
    );
}

#[cfg(feature = "reqwest-client")]
#[tokio::test]
async fn custom_reqwest_client() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/models"))
        .and(header("x-from-reqwest", "1"))
        .and(header(
            "x-typesafe-sdk",
            format!("typesafe-sdk-rust/{}", typesafe::constants::VERSION).as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models": []})))
        .expect(1)
        .mount(&server)
        .await;

    let mut defaults = typesafe::http::HeaderMap::new();
    defaults.insert("x-from-reqwest", HeaderValue::from_static("1"));
    // A default header set on the reqwest client must not be able to spoof SDK identification.
    defaults.insert("x-typesafe-sdk", HeaderValue::from_static("spoof"));
    let http = reqwest::Client::builder()
        .default_headers(defaults)
        .build()
        .unwrap();
    let c = Client::builder()
        .api_key("k")
        .base_url(server.uri())
        .http_client(http)
        .build()
        .unwrap();
    assert!(c.models().list().await.unwrap().models.is_empty());
}

/// An AI gateway sits in front of the API under a path of its own and wants a key of its own.
#[tokio::test]
async fn goes_through_an_ai_gateway() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/acct/gw/typesafe/v1/systemone"))
        .and(header("cf-aig-authorization", "Bearer gw-key"))
        .and(header("authorization", "Bearer sk-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .expect(1)
        .mount(&server)
        .await;
    let c = Client::builder()
        .api_key("sk-test")
        .base_url(format!("{}/v1/acct/gw/typesafe/", server.uri()))
        .header(
            HeaderName::from_static("cf-aig-authorization"),
            HeaderValue::from_static("Bearer gw-key"),
        )
        .build()
        .unwrap();
    c.system_one("x", questions()).await.unwrap();
}

#[cfg(feature = "blocking")]
#[test]
fn blocking_client() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let server = MockServer::start().await;
        Mock::given(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
            .mount(&server)
            .await;
        server
    });
    let c = typesafe::blocking::Client::new(
        Client::builder()
            .api_key("k")
            .base_url(server.uri())
            .build()
            .unwrap(),
    )
    .unwrap();
    let res = c
        .system_one("x", questions())
        .model("jev-latest")
        .send()
        .unwrap();
    assert_eq!(res.choice("department").unwrap().choice, "technical");
    drop(server);
}
