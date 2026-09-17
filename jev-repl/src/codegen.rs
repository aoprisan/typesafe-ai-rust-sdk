//! Turn the current session into a program written against this SDK.

use serde_json::Value;

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
        out.push_str(&reader(name, q, threshold));
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

fn reader(name: &str, q: &Value, threshold: f64) -> String {
    match kind(q) {
        "noul" => format!(
            "    let {name} = res.noul({name:?}).expect(\"asked\");\n\
             \x20   println!(\"{name}: {{:.2}} → {{}}\", {name}.noul, {name}.is_yes({threshold:.2}));\n"
        ),
        "choice" => format!(
            "    let {name} = res.choice({name:?}).expect(\"asked\");\n\
             \x20   if {name}.confidence >= 0.6 {{\n\
             \x20       println!(\"{name}: {{}}\", {name}.choice);\n\
             \x20   }} else {{\n\
             \x20       println!(\"{name}: unsure ({{:.2}}), send to a human\", {name}.confidence);\n\
             \x20   }}\n"
        ),
        "score" => format!(
            "    let {name} = res.score({name:?}).expect(\"asked\");\n\
             \x20   println!(\"{name}: {{:.2}} of {{}} (confidence {{:.2}})\", {name}.score, {name}.legend.len() - 1, {name}.confidence);\n"
        ),
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
