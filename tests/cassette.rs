//! Record and replay: responses kept on disk and served back without the network.

use std::path::{Path, PathBuf};

use serde_json::json;
use typesafe::{Client, Error, Noul, Questions, RetryPolicy, cassette};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A fresh directory per test, so tests running in parallel keep out of each other's way.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("typesafe-cassette-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    dir
}

fn questions() -> Questions {
    Questions::new().with("is_urgent", Noul::new("The message conveys urgency"))
}

const STATE: &str = "The payout failed again.";

/// Whitespace and a key order that is not alphabetical, as a server might send it.
const BODY: &str = r#"{
  "model": "jev-latest",
  "answers": {"is_urgent": {"type": "noul", "noul": 0.97}},
  "usage": {"output_tokens": 3, "input_tokens": 20}
}"#;

async fn server(status: u16, body: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(status).set_body_raw(body.to_owned(), "application/json"),
        )
        .mount(&server)
        .await;
    server
}

fn recording(server: &MockServer, dir: &Path) -> Client {
    Client::builder()
        .api_key("sk-test")
        .base_url(server.uri())
        .retry(RetryPolicy::none())
        .record(dir)
        .build()
        .unwrap()
}

fn replaying(dir: &Path) -> Client {
    // No key, and a base URL nothing listens on: a replaying client needs neither.
    Client::builder()
        .base_url("http://127.0.0.1:9")
        .replay(dir)
        .build()
        .unwrap()
}

#[tokio::test]
async fn records_then_replays_without_the_network() {
    let dir = scratch("round-trip");
    let server = server(200, BODY).await;
    let live = recording(&server, &dir)
        .system_one(STATE, questions())
        .await
        .unwrap();

    let key = cassette::key(&json!(STATE), "jev-latest", &questions());
    let file = cassette::path(&dir, &key);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "{\"model\":\"jev-latest\",\"answers\":{\"is_urgent\":{\"type\":\"noul\",\"noul\":0.97}},\
         \"usage\":{\"output_tokens\":3,\"input_tokens\":20}}\n"
    );

    let replayed = replaying(&dir)
        .system_one(STATE, questions())
        .await
        .unwrap();
    assert_eq!(replayed.answers, live.answers);
    assert_eq!(replayed.usage, live.usage);
    assert_eq!(replayed.meta.attempts, 0);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_request_never_recorded_is_a_replay_miss() {
    let dir = scratch("miss");
    std::fs::create_dir_all(&dir).unwrap();
    let err = replaying(&dir)
        .system_one(STATE, questions())
        .model("jev-2")
        .await
        .unwrap_err();
    let Error::ReplayMiss { key, path } = &err else {
        panic!("expected ReplayMiss, got {err:?}");
    };
    assert_eq!(key, &cassette::key(&json!(STATE), "jev-2", &questions()));
    assert_eq!(path, &cassette::path(&dir, key));
    assert!(err.to_string().contains("nothing was sent"), "{err}");

    // Local validation still comes first.
    let err = replaying(&dir)
        .system_one(STATE, Questions::new())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(_)), "{err:?}");
}

#[tokio::test]
async fn the_key_covers_the_whole_body() {
    let dir = scratch("extra-body");
    let server = server(200, BODY).await;
    recording(&server, &dir)
        .system_one(STATE, questions())
        .extra_body("temperature", 0)
        .await
        .unwrap();
    let plain = cassette::key(&json!(STATE), "jev-latest", &questions());
    assert!(!cassette::path(&dir, &plain).exists());

    let client = replaying(&dir);
    let err = client.system_one(STATE, questions()).await.unwrap_err();
    assert!(matches!(err, Error::ReplayMiss { .. }), "{err:?}");
    client
        .system_one(STATE, questions())
        .extra_body("temperature", 0)
        .await
        .unwrap();
}

#[tokio::test]
async fn only_successful_responses_are_recorded() {
    let dir = scratch("failures");
    let bad_status = server(500, r#"{"detail": "down"}"#).await;
    let err = recording(&bad_status, &dir)
        .system_one(STATE, questions())
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(500));
    let bad_body = server(200, r#"{"answers": {}}"#).await;
    let err = recording(&bad_body, &dir)
        .system_one(STATE, questions())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ResponseValidation(_)), "{err:?}");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
}

#[tokio::test]
async fn a_corrupt_recording_is_a_validation_error() {
    let dir = scratch("corrupt");
    std::fs::create_dir_all(&dir).unwrap();
    let key = cassette::key(&json!(STATE), "jev-latest", &questions());
    std::fs::write(cassette::path(&dir, &key), "{\"model\": \"jev-latest\"}\n").unwrap();
    let err = replaying(&dir)
        .system_one(STATE, questions())
        .await
        .unwrap_err();
    let Error::ResponseValidation(e) = &err else {
        panic!("expected ResponseValidation, got {err:?}");
    };
    assert_eq!(e.field_path, "answers");
    assert!(e.endpoint.as_deref().unwrap().starts_with("replay "));
}

#[tokio::test]
async fn a_replaying_client_does_not_list_models() {
    let err = replaying(&scratch("models"))
        .models()
        .list()
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Config(_)), "{err:?}");
}

#[test]
fn record_and_replay_together_is_a_config_error() {
    let err = Client::builder()
        .api_key("sk-test")
        .record(scratch("both"))
        .replay(scratch("both"))
        .build()
        .unwrap_err();
    assert!(matches!(err, Error::Config(_)), "{err:?}");
    // Recording still sends, so it still needs a key.
    if std::env::var(typesafe::constants::API_KEY_ENV).is_err() {
        let err = Client::builder()
            .record(scratch("keyless"))
            .build()
            .unwrap_err();
        assert!(matches!(err, Error::Config(_)), "{err:?}");
    }
}

#[cfg(feature = "blocking")]
#[test]
fn the_blocking_client_replays_too() {
    let dir = scratch("blocking");
    std::fs::create_dir_all(&dir).unwrap();
    let key = cassette::key(&json!(STATE), "jev-latest", &questions());
    std::fs::write(cassette::path(&dir, &key), BODY).unwrap();
    let client = typesafe::blocking::Client::new(replaying(&dir)).unwrap();
    let res = client.system_one(STATE, questions()).send().unwrap();
    assert_eq!(res.noul("is_urgent").unwrap().noul, 0.97);
}
