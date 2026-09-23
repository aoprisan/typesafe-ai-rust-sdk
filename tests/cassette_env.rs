//! `TYPESAFE_RECORD` and `TYPESAFE_REPLAY`. A binary of its own, with one test, because the
//! environment is shared by every test in a process.

use typesafe::constants::{RECORD_ENV, REPLAY_ENV};
use typesafe::{Client, Error, Noul, Questions};

#[tokio::test]
async fn the_environment_turns_replay_on_and_both_is_refused() {
    let dir = std::env::temp_dir().join(format!("typesafe-cassette-env-{}", std::process::id()));
    // SAFETY: the only test in this binary, so no other thread reads the environment meanwhile.
    unsafe {
        std::env::set_var(REPLAY_ENV, &dir);
        std::env::remove_var(RECORD_ENV);
    }
    let client = Client::builder().build().expect("replaying needs no key");
    let err = client
        .system_one("x", Questions::from([("q", Noul::new("?"))]))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ReplayMiss { .. }), "{err:?}");

    // SAFETY: as above.
    unsafe { std::env::set_var(RECORD_ENV, &dir) };
    let err = Client::builder().api_key("sk-test").build().unwrap_err();
    assert!(err.to_string().contains(RECORD_ENV), "{err}");

    // Blank values are ignored, like every other variable.
    // SAFETY: as above.
    unsafe { std::env::set_var(RECORD_ENV, "  ") };
    assert!(Client::builder().api_key("sk-test").build().is_ok());
    unsafe {
        std::env::remove_var(RECORD_ENV);
        std::env::remove_var(REPLAY_ENV);
    }
}
