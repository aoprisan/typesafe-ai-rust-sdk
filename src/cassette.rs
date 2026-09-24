//! Record and replay: System One responses kept on disk, one file per request.
//!
//! A client built with [`ClientBuilder::record`](crate::ClientBuilder::record) (or
//! `TYPESAFE_RECORD=<dir>`) writes each successful response body to `<dir>/<key>.json`; one built
//! with [`ClientBuilder::replay`](crate::ClientBuilder::replay) (or `TYPESAFE_REPLAY=<dir>`) answers
//! from those files and never touches the network, so it needs no API key. A request with no file
//! is [`Error::ReplayMiss`](crate::Error::ReplayMiss), not a call.
//!
//! The key is the SHA-256, in lowercase hex, of the exact JSON body the request would POST —
//! `{"state":…,"model":…,"questions":…}` without whitespace, questions in the order they were
//! added, plus any `extra_body` fields. The file is that response body as compact JSON with its
//! keys in the order the server sent them, and a newline. That is also what `jev eval --cache`
//! keeps, so an eval cache directory is a cassette directory and the other way round.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Serialize, Serializer};
use serde_json::{Number, Value};
use sha2::{Digest, Sha256};

use crate::question::Questions;

/// The key of a request with no `extra_body`: the hash of `{"state", "model", "questions"}` as
/// compact JSON.
///
/// # Panics
///
/// Never: a `Value` and a `Questions` always encode as JSON.
///
/// ```
/// use typesafe::{Noul, Questions, cassette, json};
///
/// let questions = Questions::new().with("is_urgent", Noul::new("The message conveys urgency"));
/// let key = cassette::key(&json!("The payout failed again."), "jev-latest", &questions);
/// assert_eq!(key, "4bb6a561cd7ce28500dc6aa8fc821771e45e4f811f1195c2441263651a7dca55");
/// ```
pub fn key(state: &Value, model: &str, questions: &Questions) -> String {
    #[derive(Serialize)]
    struct Body<'a> {
        state: &'a Value,
        model: &'a str,
        questions: &'a Questions,
    }
    let body = serde_json::to_vec(&Body {
        state,
        model,
        questions,
    })
    .expect("a Value and a Questions always encode");
    body_key(&body)
}

/// The key of a request body that has already been encoded.
pub(crate) fn body_key(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Where the response for `key` is kept under `dir`.
pub fn path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.json"))
}

/// Keep a response body. The file is written whole, then renamed into place, so concurrent
/// readers never see half of one.
pub(crate) fn write(dir: &Path, key: &str, body: &[u8]) -> std::io::Result<()> {
    let json: Json = serde_json::from_slice(body)?;
    let mut text = serde_json::to_string(&json)?;
    text.push('\n');
    let file = path(dir, key);
    let partial = dir.join(format!(".{key}.{}.partial", std::process::id()));
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, &file)
}

/// Any JSON value, with object keys kept in the order they were read whichever way serde_json's
/// `preserve_order` feature is set — which `serde_json::Value` cannot promise — so a recording
/// has the same bytes as the one `jev eval --cache` writes for the same response.
enum Json {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Json>),
    Object(IndexMap<String, Json>),
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Json::Null => s.serialize_unit(),
            Json::Bool(b) => s.serialize_bool(*b),
            Json::Number(n) => n.serialize(s),
            Json::String(t) => s.serialize_str(t),
            Json::Array(a) => a.serialize(s),
            Json::Object(o) => o.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct JsonVisitor;

        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = Json;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("any JSON value")
            }

            fn visit_unit<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }

            fn visit_bool<E>(self, b: bool) -> Result<Json, E> {
                Ok(Json::Bool(b))
            }

            fn visit_i64<E>(self, n: i64) -> Result<Json, E> {
                Ok(Json::Number(n.into()))
            }

            fn visit_u64<E>(self, n: u64) -> Result<Json, E> {
                Ok(Json::Number(n.into()))
            }

            fn visit_f64<E: de::Error>(self, n: f64) -> Result<Json, E> {
                Number::from_f64(n)
                    .map(Json::Number)
                    .ok_or_else(|| E::custom("a number JSON cannot hold"))
            }

            fn visit_str<E>(self, s: &str) -> Result<Json, E> {
                Ok(Json::String(s.to_owned()))
            }

            fn visit_string<E>(self, s: String) -> Result<Json, E> {
                Ok(Json::String(s))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
                let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Json::Array(items))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
                let mut object = IndexMap::with_capacity(map.size_hint().unwrap_or(0));
                while let Some((k, v)) = map.next_entry()? {
                    object.insert(k, v);
                }
                Ok(Json::Object(object))
            }
        }

        d.deserialize_any(JsonVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{Choice, Noul, Score};
    use serde_json::json;

    /// The fixture every implementation of the cassette contract (this SDK, `jev`, the
    /// TypeScript port) must hash to the same digest.
    #[test]
    fn key_is_the_hash_of_the_compact_body() {
        let questions =
            Questions::new().with("is_urgent", Noul::new("The message conveys urgency"));
        let state = json!("The payout failed again.");
        assert_eq!(
            key(&state, "jev-latest", &questions),
            "4bb6a561cd7ce28500dc6aa8fc821771e45e4f811f1195c2441263651a7dca55"
        );
        assert_eq!(
            key(&state, "jev-latest", &questions),
            body_key(
                br#"{"state":"The payout failed again.","model":"jev-latest","questions":{"is_urgent":{"type":"noul","instructions":"The message conveys urgency"}}}"#
            )
        );
    }

    #[test]
    fn key_follows_question_order_and_every_field() {
        let a = Questions::new()
            .with("x", Noul::new("x"))
            .with("y", Score::new("y", ["lo", "hi"]));
        let b = Questions::new()
            .with("y", Score::new("y", ["lo", "hi"]))
            .with("x", Noul::new("x"));
        let state = json!("s");
        assert_ne!(key(&state, "m", &a), key(&state, "m", &b));
        assert_ne!(key(&state, "m", &a), key(&state, "n", &a));
        assert_ne!(key(&state, "m", &a), key(&json!("t"), "m", &a));
        let c = Questions::new().with("c", Choice::from_labels("c", ["p", "q"]));
        assert_eq!(key(&state, "m", &c).len(), 64);
    }

    #[test]
    fn recordings_are_compact_and_keep_server_order() {
        let dir = std::env::temp_dir().join(format!("typesafe-cassette-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let body = br#"{ "model": "m",
            "answers": {"z": {"type": "noul", "noul": 0.5}, "a": {"type": "noul", "noul": 1e-5}},
            "usage": {} }"#;
        write(&dir, "k", body).unwrap();
        let text = std::fs::read_to_string(path(&dir, "k")).unwrap();
        assert_eq!(
            text,
            "{\"model\":\"m\",\"answers\":{\"z\":{\"type\":\"noul\",\"noul\":0.5},\"a\":{\"type\":\"noul\",\"noul\":0.00001}},\"usage\":{}}\n"
        );
        assert!(write(&dir, "bad", b"not json").is_err());
        assert!(!path(&dir, "bad").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
