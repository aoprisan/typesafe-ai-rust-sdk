//! The client, its requests and its errors can cross threads and be shared between them.

fn assert_send_sync<T: Send + Sync>() {}

struct Rubric;

impl typesafe::Rubric for Rubric {
    fn questions() -> typesafe::Questions {
        typesafe::Questions::new()
    }

    fn from_response(_: &typesafe::SystemOneResponse) -> typesafe::Result<Self> {
        Ok(Rubric)
    }
}

#[test]
fn public_types_are_send_and_sync() {
    assert_send_sync::<typesafe::Client>();
    assert_send_sync::<typesafe::ClientBuilder>();
    assert_send_sync::<typesafe::SystemOneRequest>();
    assert_send_sync::<typesafe::AskRequest<Rubric>>();
    assert_send_sync::<typesafe::ListModelsRequest>();
    assert_send_sync::<typesafe::Error>();
    assert_send_sync::<typesafe::SystemOneResponse>();
    assert_send_sync::<typesafe::RetryPolicy>();
}

#[cfg(feature = "blocking")]
#[test]
fn the_blocking_client_is_send_and_sync() {
    assert_send_sync::<typesafe::blocking::Client>();
    assert_send_sync::<typesafe::blocking::SystemOneRequest<'static>>();
}
