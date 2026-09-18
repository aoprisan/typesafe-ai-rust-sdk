//! Offline answers, so the shapes can be learned without an API key.
//!
//! Deterministic: the same state and question always produce the same numbers. Plausible, not
//! predictive — nothing here reasons about anything.

use serde_json::{Value, json};
use typesafe::{Answer, ModelMetadata};

/// Simulate an answer to one question. `None` for question shapes the simulator cannot fake.
pub fn answer(state: &Value, name: &str, question: &Value) -> Option<Answer> {
    let state = state.to_string();
    let kind = question.get("type")?.as_str()?;
    let instructions = question
        .get("instructions")
        .map(ToString::to_string)
        .unwrap_or_default();
    let seed = format!("{state}\u{1}{name}\u{1}{instructions}");
    match kind {
        "noul" => {
            let p = round(0.04 + 0.92 * unit(&[&seed, "noul"]));
            from_value(json!({ "noul": p }))
        }
        "choice" => {
            let labels: Vec<String> = question
                .get("criteria")?
                .as_object()?
                .keys()
                .cloned()
                .collect();
            let probs = distribution(&seed, &state, &labels);
            let (choice, _) = probs
                .iter()
                .cloned()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap_or_default();
            let map: serde_json::Map<String, Value> =
                probs.iter().map(|(l, p)| (l.clone(), json!(p))).collect();
            let conf = confidence(probs.iter().map(|(_, p)| *p));
            from_value(json!({"choice": choice, "probabilities": map, "confidence": conf}))
        }
        "score" => {
            let levels = question.get("criteria")?.as_array()?;
            let keys: Vec<String> = (0..levels.len()).map(|i| i.to_string()).collect();
            let probs = distribution(&seed, &state, &keys);
            let score = round(
                probs
                    .iter()
                    .enumerate()
                    .map(|(i, (_, p))| i as f64 * p)
                    .sum(),
            );
            let legend: serde_json::Map<String, Value> = levels
                .iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v.clone()))
                .collect();
            let map: serde_json::Map<String, Value> =
                probs.iter().map(|(k, p)| (k.clone(), json!(p))).collect();
            let conf = confidence(probs.iter().map(|(_, p)| *p));
            from_value(
                json!({"score": score, "confidence": conf, "legend": legend, "probabilities": map}),
            )
        }
        _ => None,
    }
}

/// One answer as it arrives on the wire — what `:last` shows offline, and what the cost estimate
/// measures to say what an answer of this shape costs.
pub fn answer_json(answer: &Answer) -> Value {
    match answer {
        Answer::Noul(a) => json!({"type": "noul", "noul": a.noul}),
        Answer::Choice(a) => json!({
            "type": "choice", "choice": a.choice,
            "probabilities": a.probabilities.iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect::<serde_json::Map<String, Value>>(),
            "confidence": a.confidence,
        }),
        Answer::Score(a) => json!({
            "type": "score", "score": a.score, "confidence": a.confidence,
            "legend": a.legend.iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<serde_json::Map<String, Value>>(),
            "probabilities": a.probabilities.iter()
                .map(|(k, v)| (k.to_string(), json!(v)))
                .collect::<serde_json::Map<String, Value>>(),
        }),
        // An answer variant a later SDK adds: this one has no shape to measure or print.
        _ => Value::Null,
    }
}

/// What `:models` shows offline.
pub fn models() -> Vec<ModelMetadata> {
    [
        (
            "jev-latest",
            "Alias for the newest jev release",
            "2026-05-01",
        ),
        ("jev-2", "Previous generation", "2025-11-12"),
    ]
    .into_iter()
    .filter_map(|(name, description, release_date)| {
        serde_json::from_value(json!({
            "name": name, "description": description, "release_date": release_date
        }))
        .ok()
    })
    .collect()
}

fn from_value(v: Value) -> Option<Answer> {
    // The answer structs are `#[non_exhaustive]`, so they are built through their `Deserialize`
    // impls — which also keeps this honest about the wire shape.
    if v.get("noul").is_some() {
        return serde_json::from_value(v).ok().map(Answer::Noul);
    }
    if v.get("choice").is_some() {
        return serde_json::from_value(v).ok().map(Answer::Choice);
    }
    serde_json::from_value(v).ok().map(Answer::Score)
}

/// Weights per label, nudged up when the label's word shows up in the state, then normalized.
fn distribution(seed: &str, state: &str, labels: &[String]) -> Vec<(String, f64)> {
    let lower = state.to_lowercase();
    let mut raw: Vec<(String, f64)> = labels
        .iter()
        .map(|label| {
            let u = unit(&[seed, label]);
            let mut w = 0.02 + u * u * u;
            if label.len() > 3 && lower.contains(&label.to_lowercase()) {
                w *= 4.0;
            }
            (label.clone(), w)
        })
        .collect();
    let total: f64 = raw.iter().map(|(_, w)| w).sum();
    if total <= 0.0 {
        let even = round(1.0 / raw.len().max(1) as f64);
        return raw.into_iter().map(|(l, _)| (l, even)).collect();
    }
    for (_, w) in &mut raw {
        *w = round(*w / total);
    }
    // Rounding leaves a few thousandths on the table; give them to the leader.
    let drift = 1.0 - raw.iter().map(|(_, w)| w).sum::<f64>();
    if let Some(top) = raw
        .iter_mut()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|_| drift.abs() > f64::EPSILON)
    {
        top.1 = round(top.1 + drift);
    }
    raw
}

/// 0 when the distribution is flat, 1 when it is certain — the same direction the API reports.
fn confidence(probs: impl Iterator<Item = f64>) -> f64 {
    let probs: Vec<f64> = probs.collect();
    let n = probs.len();
    if n < 2 {
        return 1.0;
    }
    let max = probs.iter().copied().fold(0.0, f64::max);
    let floor = 1.0 / n as f64;
    round(((max - floor) / (1.0 - floor)).clamp(0.0, 1.0))
}

fn unit(parts: &[&str]) -> f64 {
    (fnv(parts) % 100_000) as f64 / 100_000.0
}

fn fnv(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}
