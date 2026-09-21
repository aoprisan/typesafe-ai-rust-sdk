//! Where the jev MCP server and the jev skill go, for each agent that can host them.
//!
//! Every agent keeps its own file in its own shape, but the job is always the same: put one entry
//! in a config file without disturbing what is already there, and drop one `SKILL.md` in a
//! directory. This module is the pure half — paths and merged text, no filesystem — so the table
//! can be read in a test, and `jev install` stays a thin wrapper over it.
//!
//! ```
//! # use jev_repl::install::{self, Kind, Scope, ServerEntry};
//! let client = install::find("codex").unwrap();
//! let server = ServerEntry::new("jev", "/opt/jev");
//! let merged = install::merge(client, Kind::Mcp, "", &server).unwrap();
//! assert!(merged.contains("[mcp_servers.jev]"));
//! ```

use serde_json::{Map, Value, json};

use crate::skill::{SKILL_FILE, SKILL_MD, SKILL_NAME};

/// How a client writes down an MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `mcpServers` in a JSON file, the shape Claude Code reads.
    McpJson,
    /// A `[mcp_servers.<name>]` table in Codex's `config.toml`.
    CodexToml,
    /// `mcp` in a JSON file, with the command as a list.
    OpencodeJson,
}

/// What is being installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Mcp,
    Skill,
}

/// Installed for this user, or into the repository in front of you.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    User,
    Project,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Mcp => "mcp",
            Kind::Skill => "skill",
        }
    }
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Project => "project",
        }
    }
}

/// One agent: what to call it, where its files live, and what shape they are in.
pub struct ClientSpec {
    pub id: &'static str,
    /// The name a person would recognise.
    pub title: &'static str,
    pub format: Format,
    /// The config file the MCP entry goes in, under the home directory.
    pub mcp_user: &'static [&'static str],
    /// The same, under the project root.
    pub mcp_project: &'static [&'static str],
    /// The directory skills live in, which the skill's own folder goes inside.
    pub skill_user: &'static [&'static str],
    pub skill_project: &'static [&'static str],
    /// Paths under the home directory that mean this agent is installed.
    pub markers: &'static [&'static [&'static str]],
    /// Anything a person needs to know after the file is written.
    pub note: Option<&'static str>,
}

/// The four agents, and where each keeps its things.
///
/// Claude Code and Codex read the config formats their own docs describe. OpenCode keeps MCP
/// servers under `mcp` with the command as a list. pi has no MCP client of its own — the entry is
/// written in the shape its MCP extensions read, which is Claude's.
pub const CLIENTS: &[ClientSpec] = &[
    ClientSpec {
        id: "claude-code",
        title: "Claude Code",
        format: Format::McpJson,
        mcp_user: &[".claude.json"],
        mcp_project: &[".mcp.json"],
        skill_user: &[".claude", "skills"],
        skill_project: &[".claude", "skills"],
        markers: &[&[".claude"], &[".claude.json"]],
        note: None,
    },
    ClientSpec {
        id: "codex",
        title: "Codex CLI",
        format: Format::CodexToml,
        mcp_user: &[".codex", "config.toml"],
        mcp_project: &[".codex", "config.toml"],
        skill_user: &[".codex", "skills"],
        skill_project: &[".codex", "skills"],
        markers: &[&[".codex"]],
        note: None,
    },
    ClientSpec {
        id: "opencode",
        title: "OpenCode",
        format: Format::OpencodeJson,
        mcp_user: &[".config", "opencode", "opencode.json"],
        mcp_project: &["opencode.json"],
        skill_user: &[".config", "opencode", "skills"],
        skill_project: &[".opencode", "skills"],
        markers: &[&[".config", "opencode"]],
        note: None,
    },
    ClientSpec {
        id: "pi",
        title: "pi",
        format: Format::McpJson,
        mcp_user: &[".pi", "agent", "mcp.json"],
        mcp_project: &[".mcp.json"],
        skill_user: &[".pi", "agent", "skills"],
        skill_project: &[".pi", "skills"],
        markers: &[&[".pi"]],
        note: Some(
            "pi has no MCP client built in: install an MCP extension (for example pi-mcp-adapter) \
             to read this entry. The skill works as it is.",
        ),
    },
];

/// The client with this id, if it is one we know.
pub fn find(id: &str) -> Option<&'static ClientSpec> {
    CLIENTS.iter().find(|c| c.id == id)
}

/// Every client id, for the help text and for `--client all`.
pub fn ids() -> Vec<&'static str> {
    CLIENTS.iter().map(|c| c.id).collect()
}

/// The server as a client writes it down.
#[derive(Debug, Clone)]
pub struct ServerEntry {
    /// The key it is filed under; also what an agent prefixes its tools with.
    pub name: String,
    /// The program to run.
    pub command: String,
    pub args: Vec<String>,
    /// Environment for the server process, over what it inherits, in the order it was given.
    pub env: Vec<(String, String)>,
}

impl ServerEntry {
    pub fn new(name: &str, command: &str) -> Self {
        Self {
            name: name.to_owned(),
            command: command.to_owned(),
            args: vec!["mcp".to_owned()],
            env: Vec::new(),
        }
    }
}

/// The file one kind of install writes for one client at one scope.
pub fn file(client: &ClientSpec, kind: Kind, scope: Scope) -> Vec<String> {
    let base = match (kind, scope) {
        (Kind::Mcp, Scope::User) => client.mcp_user,
        (Kind::Mcp, Scope::Project) => client.mcp_project,
        (Kind::Skill, Scope::User) => client.skill_user,
        (Kind::Skill, Scope::Project) => client.skill_project,
    };
    let mut segments: Vec<String> = base.iter().map(|s| (*s).to_owned()).collect();
    if kind == Kind::Skill {
        segments.push(SKILL_NAME.to_owned());
        segments.push(SKILL_FILE.to_owned());
    }
    segments
}

/// The MCP entry, in the shape this client reads.
pub fn entry(format: Format, server: &ServerEntry) -> Value {
    let mut env = Map::new();
    for (name, value) in &server.env {
        env.insert(name.clone(), Value::String(value.clone()));
    }
    if format == Format::OpencodeJson {
        let mut command = vec![Value::String(server.command.clone())];
        command.extend(server.args.iter().map(|a| Value::String(a.clone())));
        let mut value = json!({ "type": "local", "command": command, "enabled": true });
        if !env.is_empty() {
            value["environment"] = Value::Object(env);
        }
        return value;
    }
    let mut value = json!({
        "type": "stdio",
        "command": server.command,
        "args": server.args,
    });
    if !env.is_empty() {
        value["env"] = Value::Object(env);
    }
    value
}

/// The key an MCP server is filed under in this client's config.
fn section(format: Format) -> &'static str {
    match format {
        Format::OpencodeJson => "mcp",
        _ => "mcpServers",
    }
}

/// The config file with the entry in it, keeping everything else exactly as it was.
///
/// An empty or missing file becomes a fresh one; a file that is not JSON is left alone and
/// reported, because a hand-edited config is not something to guess at.
fn merge_json(format: Format, existing: &str, server: &ServerEntry) -> Result<String, String> {
    let mut root = if existing.trim().is_empty() {
        let mut fresh = Map::new();
        if format == Format::OpencodeJson {
            fresh.insert(
                "$schema".to_owned(),
                Value::String("https://opencode.ai/config.json".to_owned()),
            );
        }
        fresh
    } else {
        match serde_json::from_str::<Value>(existing) {
            Ok(Value::Object(map)) => map,
            _ => {
                return Err(
                    "the file is not a JSON object; fix or move it and run again.".to_owned(),
                );
            }
        }
    };

    let key = section(format);
    let mut servers = match root.remove(key) {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    servers.insert(server.name.clone(), entry(format, server));
    root.insert(key.to_owned(), Value::Object(servers));
    Ok(format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(root)).map_err(|e| e.to_string())?
    ))
}

/// A TOML basic string: the two escapes a path or a key can actually need.
fn toml_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A bare key where TOML allows one, quoted where it does not.
fn toml_key(name: &str) -> String {
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        name.to_owned()
    } else {
        toml_string(name)
    }
}

/// The `[mcp_servers.<name>]` table Codex reads.
pub fn toml_table(server: &ServerEntry) -> String {
    let args = server
        .args
        .iter()
        .map(|a| toml_string(a))
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = format!(
        "[mcp_servers.{}]\ncommand = {}\nargs = [{args}]\n",
        toml_key(&server.name),
        toml_string(&server.command),
    );
    if !server.env.is_empty() {
        let pairs = server
            .env
            .iter()
            .map(|(name, value)| format!("{} = {}", toml_key(name), toml_string(value)))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("env = {{ {pairs} }}\n"));
    }
    out
}

/// The same, for a file of TOML.
///
/// There is no TOML parser here on purpose: a config file is someone's, and a round trip through a
/// parser would reflow their comments and reorder their tables. So the table is found as text and
/// replaced as text — from its header to the next table that is not one of its own subtables.
fn merge_toml(existing: &str, server: &ServerEntry) -> Result<String, String> {
    let table = toml_table(server);
    let header = format!("[mcp_servers.{}]", toml_key(&server.name));
    let lines: Vec<&str> = existing.split('\n').collect();
    let Some(start) = lines.iter().position(|line| line.trim() == header) else {
        let body = existing.trim_end();
        return Ok(if body.is_empty() {
            table
        } else {
            format!("{body}\n\n{table}")
        });
    };
    let subtable = format!("[mcp_servers.{}.", toml_key(&server.name));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| {
            let line = line.trim();
            line.starts_with('[') && !line.starts_with(&subtable)
        })
        .map_or(lines.len(), |(at, _)| at);

    let before = lines[..start].join("\n").trim_end().to_owned();
    let after = lines[end..].join("\n").trim_start().to_owned();
    let head = if before.is_empty() {
        String::new()
    } else {
        format!("{before}\n\n")
    };
    let tail = if after.is_empty() {
        String::new()
    } else {
        format!("\n{after}")
    };
    Ok(format!("{head}{table}{tail}"))
}

/// What this file should contain once jev is installed, given what it contains now.
///
/// `existing` is the current text, or `""` when there is no file yet.
pub fn merge(
    client: &ClientSpec,
    kind: Kind,
    existing: &str,
    server: &ServerEntry,
) -> Result<String, String> {
    if kind == Kind::Skill {
        return Ok(SKILL_MD.to_owned());
    }
    match client.format {
        Format::CodexToml => merge_toml(existing, server),
        format => merge_json(format, existing, server),
    }
}

/// Whether a `SKILL.md` already there is a copy of ours, possibly an older one.
///
/// Installing over our own file is an upgrade; installing over a file someone wrote or edited is
/// not, so it takes `--force`. The frontmatter name is the only mark a skill file carries.
pub fn looks_like_ours(existing: &str) -> bool {
    if existing.trim().is_empty() {
        return true;
    }
    let Some(rest) = existing.strip_prefix("---") else {
        return false;
    };
    let frontmatter = match rest.find("\n---") {
        Some(end) => &rest[..end],
        None => rest,
    };
    frontmatter
        .lines()
        .any(|line| line.trim_end() == format!("name: {SKILL_NAME}"))
}

/// One line describing a file, for `--dry-run` and for the summary.
pub fn describe(client: &ClientSpec, kind: Kind, scope: Scope, path: &str) -> String {
    let what = match kind {
        Kind::Mcp => "MCP server",
        Kind::Skill => "skill",
    };
    format!("{} {what} ({}): {path}", client.title, scope.as_str())
}
