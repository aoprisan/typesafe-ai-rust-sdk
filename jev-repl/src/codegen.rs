//! Turn the current session into a program written against this SDK.

use serde_json::Value;

use crate::evaluate;
use crate::session::Session;

pub fn rust(session: &Session, model: &str, threshold: f64) -> String {
    let questions: Vec<(String, Value)> = session
        .questions
        .iter()
        .filter_map(|(name, q)| Some((name.clone(), serde_json::to_value(q).ok()?)))
        .collect();

    let mut out = String::new();
    out.push_str("#[tokio::main]\nasync fn main() -> typesafe::Result<()> {\n");
    out.push_str("    let client = Client::from_env()?; // TYPESAFE_API_KEY\n\n");
    out.push_str("    let res = client\n        .system_one(\n");
    out.push_str(&format!("            {},\n", state_literal(&session.state)));
    out.push_str("            Questions::new()\n");
    for (name, q) in &questions {
        out.push_str(&format!(
            "                .with({:?}, {})\n",
            name,
            builder(q, 20)
        ));
    }
    out.push_str("        )\n");
    out.push_str(&format!("        .model({model:?})\n"));
    out.push_str("        .await?;\n\n");

    if questions.is_empty() {
        out.push_str("    // add questions in the REPL and run :rust again\n");
    }
    for (name, q) in &questions {
        out.push_str(&reader(name, q, session.bar(name), threshold));
    }
    out.push_str("\n    Ok(())\n}\n");

    // Imports are worked out from what the body actually used, `json!` included.
    let mut imports = vec!["Client", "Questions"];
    for (_, q) in &questions {
        match kind(q) {
            "noul" => imports.push("Noul"),
            "choice" => imports.push("Choice"),
            "score" => imports.push("Score"),
            _ => {}
        }
    }
    if out.contains("json!(") {
        imports.push("json");
    }
    imports.sort_unstable();
    imports.dedup();

    format!(
        "// Cargo.toml: typesafe-ai-sdk = \"0.1\"\nuse typesafe::{{{}}};\n\n{out}",
        imports.join(", ")
    )
}

fn kind(q: &Value) -> &str {
    q.get("type").and_then(Value::as_str).unwrap_or("raw")
}

fn builder(q: &Value, indent: usize) -> String {
    let pad = " ".repeat(indent + 4);
    let instructions = q.get("instructions").map(literal).unwrap_or_default();
    match kind(q) {
        "noul" => {
            let mut s = format!("Noul::new({instructions})");
            let criteria = q.get("criteria");
            if let Some(v) = criteria.and_then(|c| c.get("true")) {
                s.push_str(&format!("\n{pad}.when_true({})", literal(v)));
            }
            if let Some(v) = criteria.and_then(|c| c.get("false")) {
                s.push_str(&format!("\n{pad}.when_false({})", literal(v)));
            }
            s
        }
        "choice" => {
            let mut s = format!("Choice::new({instructions})");
            if let Some(criteria) = q.get("criteria").and_then(Value::as_object) {
                for (label, desc) in criteria {
                    s.push_str(&match desc {
                        Value::Null => format!("\n{pad}.label({label:?})"),
                        v => format!("\n{pad}.option({label:?}, {})", literal(v)),
                    });
                }
            }
            s
        }
        "score" => {
            let levels: Vec<String> = q
                .get("criteria")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(literal).collect())
                .unwrap_or_default();
            format!(
                "Score::new(\n{pad}{instructions},\n{pad}[{}],\n{})",
                levels.join(", "),
                " ".repeat(indent)
            )
        }
        _ => format!("json!({q})"),
    }
}

/// What a choice is gated at when the page names no bar: the README's rule of thumb.
const CHOICE_GATE: f64 = 0.6;

/// A threshold as code: two decimals, the way it has always been printed, unless that would change
/// it — a bar someone wrote as `0.625` is `0.625` in the program too.
fn threshold_literal(t: f64) -> String {
    let fixed = evaluate::two(t);
    if fixed.parse::<f64>().ok() == Some(t) {
        fixed
    } else {
        format!("{t}")
    }
}

/// A float literal Rust reads as an `f64`: `0.7` stays `0.7`, but `1` has to be `1.0`.
fn float_literal(n: f64) -> String {
    let text = format!("{n}");
    if text.contains(['.', 'e']) {
        text
    } else {
        format!("{text}.0")
    }
}

/// How one answer is read back: a noul at its threshold, a choice (and a score with a bar) gated
/// on confidence. `bar` is the page's `@threshold` or `@confidence` for this question.
fn reader(name: &str, q: &Value, bar: Option<f64>, threshold: f64) -> String {
    match kind(q) {
        "noul" => {
            let cut = threshold_literal(bar.unwrap_or(threshold));
            format!(
                "    let {name} = res.noul({name:?}).expect(\"asked\");\n\
                 \x20   println!(\"{name}: {{:.2}} → {{}}\", {name}.noul, {name}.is_yes({cut}));\n"
            )
        }
        "choice" => {
            let gate = float_literal(bar.unwrap_or(CHOICE_GATE));
            format!(
                "    let {name} = res.choice({name:?}).expect(\"asked\");\n\
                 \x20   if {name}.confidence >= {gate} {{\n\
                 \x20       println!(\"{name}: {{}}\", {name}.choice);\n\
                 \x20   }} else {{\n\
                 \x20       println!(\"{name}: unsure ({{:.2}}), send to a human\", {name}.confidence);\n\
                 \x20   }}\n"
            )
        }
        // A score with a bar is gated like a choice: act above it, hand the rest to a person.
        "score" => match bar {
            Some(bar) => {
                let gate = float_literal(bar);
                format!(
                    "    let {name} = res.score({name:?}).expect(\"asked\");\n\
                     \x20   if {name}.confidence >= {gate} {{\n\
                     \x20       println!(\"{name}: {{:.2}} of {{}} (confidence {{:.2}})\", {name}.score, {name}.legend.len() - 1, {name}.confidence);\n\
                     \x20   }} else {{\n\
                     \x20       println!(\"{name}: unsure ({{:.2}}), send to a human\", {name}.confidence);\n\
                     \x20   }}\n"
                )
            }
            None => format!(
                "    let {name} = res.score({name:?}).expect(\"asked\");\n\
                 \x20   println!(\"{name}: {{:.2}} of {{}} (confidence {{:.2}})\", {name}.score, {name}.legend.len() - 1, {name}.confidence);\n"
            ),
        },
        _ => format!("    // {name}: a raw question — read it from res.raw\n"),
    }
}

fn state_literal(state: &Value) -> String {
    match state {
        Value::String(s) => format!("{s:?}"),
        other => format!("json!({other})"),
    }
}

fn literal(v: &Value) -> String {
    match v {
        Value::String(s) => format!("{s:?}"),
        other => format!("json!({other})"),
    }
}
