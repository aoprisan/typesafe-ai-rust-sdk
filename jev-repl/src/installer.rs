//! `jev install` — put the MCP server and the skill where an agent will find them.
//!
//! The decisions all live in [`crate::install`]; this is the part that reads the command line,
//! finds the home directory, and writes the files it is told to.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::install::{self, ClientSpec, Kind, Scope, ServerEntry};

/// 0 when it worked, 1 when a file did not, 2 when the command line did not parse.
const OK: u8 = 0;
const FAILED: u8 = 1;
const BAD_USAGE: u8 = 2;

pub fn help() -> String {
    format!(
        "jev install — register jev with a coding agent\n\n\
         \x20 jev install                     the MCP server and the skill, for every agent found\n\
         \x20 jev install mcp                 just the MCP server\n\
         \x20 jev install skill               just the skill\n\
         \x20 jev install --list              the agents, and where each one's files go\n\n\
         Options\n\
         \x20 --client <id[,id]>     {}, or all\n\
         \x20                        (default: every agent installed for this user)\n\
         \x20 --scope user|project   this user (default) or the repository in front of you\n\
         \x20 --name <name>          file the server under this name (default jev)\n\
         \x20 --command <path>       the program the agent runs (default: this binary)\n\
         \x20 --env NAME=VALUE       an environment variable for the server; repeatable\n\
         \x20 --root <dir>           the project root for --scope project (default: this directory)\n\
         \x20 --home <dir>           the home directory to install under (default: yours)\n\
         \x20 --dry-run              say what would be written, write nothing\n\
         \x20 --force                replace a SKILL.md that is not ours\n\n\
         Nothing else in a config file is touched: the entry is merged in, and a file that\n\
         cannot be parsed is reported rather than rewritten.\n",
        install::ids().join(", ")
    )
}

/// What `jev install` was asked to do.
struct Options {
    kinds: Vec<Kind>,
    clients: Option<Vec<&'static ClientSpec>>,
    scope: Scope,
    server: ServerEntry,
    root: PathBuf,
    home: PathBuf,
    dry_run: bool,
    force: bool,
    list: bool,
}

/// The program an agent should run to start the server: this binary, as an absolute path.
fn self_command() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok().or(Some(path)))
        .map_or_else(|| "jev".to_owned(), |path| path.display().to_string())
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        kinds: Vec::new(),
        clients: None,
        scope: Scope::User,
        server: ServerEntry::new("jev", &self_command()),
        root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        home: home_dir(),
        dry_run: false,
        force: false,
        list: false,
    };

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let (name, inline) = match arg.strip_prefix("--").and_then(|_| arg.split_once('=')) {
            Some((name, value)) => (name.to_owned(), Some(value.to_owned())),
            None => (arg.clone(), None),
        };
        let mut value = || -> Result<String, String> {
            match &inline {
                Some(v) => Ok(v.clone()),
                None => {
                    i += 1;
                    args.get(i)
                        .cloned()
                        .ok_or_else(|| format!("{name} needs a value."))
                }
            }
        };
        match name.as_str() {
            "mcp" => options.kinds.push(Kind::Mcp),
            "skill" => options.kinds.push(Kind::Skill),
            "all" => options.kinds.extend([Kind::Mcp, Kind::Skill]),
            "--list" | "--help" | "-h" => options.list = true,
            "--client" => {
                let mut chosen = options.clients.take().unwrap_or_default();
                for id in value()?
                    .split(',')
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                {
                    if id == "all" {
                        chosen.extend(install::CLIENTS.iter());
                        continue;
                    }
                    let client = install::find(id)
                        .ok_or_else(|| format!("unknown agent {id:?}; --list has them."))?;
                    chosen.push(client);
                }
                chosen.dedup_by(|a, b| a.id == b.id);
                options.clients = Some(chosen);
            }
            "--scope" => {
                options.scope = match value()?.as_str() {
                    "user" => Scope::User,
                    "project" => Scope::Project,
                    _ => return Err("--scope takes user or project.".to_owned()),
                }
            }
            "--name" => options.server.name = value()?,
            "--command" => {
                // A command given by hand replaces the whole invocation, not just the program.
                options.server.command = value()?;
                options.server.args = vec!["mcp".to_owned()];
            }
            "--env" => {
                let pair = value()?;
                let Some((name, text)) = pair.split_once('=') else {
                    return Err("--env takes NAME=VALUE.".to_owned());
                };
                if name.is_empty() {
                    return Err("--env takes NAME=VALUE.".to_owned());
                }
                options.server.env.push((name.to_owned(), text.to_owned()));
            }
            "--root" => options.root = PathBuf::from(value()?),
            "--home" => options.home = PathBuf::from(value()?),
            "--dry-run" => options.dry_run = true,
            "--force" => options.force = true,
            other => return Err(format!("unknown option {other:?}; jev install --help.")),
        }
        i += 1;
    }

    if options.kinds.is_empty() {
        options.kinds = vec![Kind::Mcp, Kind::Skill];
    }
    options.kinds.dedup();
    Ok(options)
}

/// The agents this user has, judged by whether their directories exist.
pub fn detect(home: &Path) -> Vec<&'static ClientSpec> {
    install::CLIENTS
        .iter()
        .filter(|client| {
            client.markers.iter().any(|marker| {
                let mut path = home.to_path_buf();
                path.extend(marker.iter());
                path.exists()
            })
        })
        .collect()
}

/// Where a file lands on this machine.
fn path_of(options: &Options, client: &ClientSpec, kind: Kind) -> PathBuf {
    let mut path = match options.scope {
        Scope::User => options.home.clone(),
        Scope::Project => options.root.clone(),
    };
    path.extend(install::file(client, kind, options.scope));
    path
}

/// The agents and their paths, for `--list`.
fn list(options: &Options, out: &mut dyn FnMut(&str)) {
    let found: Vec<&str> = detect(&options.home).iter().map(|c| c.id).collect();
    for client in install::CLIENTS {
        let installed = if found.contains(&client.id) {
            " — installed"
        } else {
            ""
        };
        out(&format!("{} ({}){installed}\n", client.title, client.id));
        for scope in [Scope::User, Scope::Project] {
            for kind in [Kind::Mcp, Kind::Skill] {
                let base = if scope == Scope::User { "~" } else { "." };
                let path = install::file(client, kind, scope).join("/");
                out(&format!(
                    "  {:<5} {:<7} {base}/{path}\n",
                    kind.as_str(),
                    scope.as_str()
                ));
            }
        }
        if let Some(note) = client.note {
            out(&format!("  note: {note}\n"));
        }
        out("\n");
    }
}

/// Install, or say what installing would do.
pub fn run(args: &[String]) -> u8 {
    // Written rather than printed: `jev install --list | head` closes the pipe, and a summary
    // line is not worth a panic.
    let mut out = |text: &str| {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(text.as_bytes());
        let _ = stdout.flush();
    };
    let options = match parse_options(args) {
        Ok(options) => options,
        Err(e) => {
            eprintln!("jev install: {e}");
            return BAD_USAGE;
        }
    };
    if options.list {
        out(&help());
        out("\n");
        list(&options, &mut out);
        return OK;
    }

    let clients = match &options.clients {
        Some(chosen) => chosen.clone(),
        None => {
            let found = detect(&options.home);
            if found.is_empty() {
                eprintln!(
                    "jev install: no agent found under this home directory.\n\
                     Pass --client <{}>, or --client all; jev install --list has them.",
                    install::ids().join("|")
                );
                return FAILED;
            }
            found
        }
    };

    let mut notes: BTreeSet<&'static str> = BTreeSet::new();
    let mut failed = false;
    let mut wrote = 0usize;
    for client in clients {
        for kind in &options.kinds {
            let kind = *kind;
            let path = path_of(&options, client, kind);
            let shown = path.display().to_string();
            let where_ = install::describe(client, kind, options.scope, &shown);
            let existing = if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(e) => {
                        eprintln!("jev install: could not read {shown}: {e}");
                        failed = true;
                        continue;
                    }
                }
            } else {
                String::new()
            };
            if kind == Kind::Skill && !options.force && !install::looks_like_ours(&existing) {
                eprintln!(
                    "jev install: {shown} was not written by jev; pass --force to replace it."
                );
                failed = true;
                continue;
            }

            let merged = match install::merge(client, kind, &existing, &options.server) {
                Ok(text) => text,
                Err(e) => {
                    eprintln!("jev install: {shown}: {e}");
                    failed = true;
                    continue;
                }
            };
            if merged == existing {
                out(&format!("{where_} — already there\n"));
                continue;
            }
            let verb = if existing.is_empty() {
                "created"
            } else {
                "updated"
            };
            if options.dry_run {
                out(&format!("{where_} — would be {verb}\n"));
                continue;
            }
            if let Some(parent) = path.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                eprintln!("jev install: could not make {}: {e}", parent.display());
                failed = true;
                continue;
            }
            if let Err(e) = std::fs::write(&path, merged) {
                eprintln!("jev install: could not write {shown}: {e}");
                failed = true;
                continue;
            }
            out(&format!("{where_} — {verb}\n"));
            wrote += 1;
            if let Some(note) = client.note {
                notes.insert(note);
            }
        }
    }

    for note in &notes {
        out(&format!("\nnote: {note}\n"));
    }
    if wrote > 0 && options.kinds.contains(&Kind::Mcp) {
        out(
            "\nThe server is started by the agent, so it inherits that agent's environment:\n\
             set TYPESAFE_API_KEY there, or pass --env TYPESAFE_API_KEY=… to write it into the \
             config.\nRestart the agent to pick up the new server.\n",
        );
    }
    if failed { FAILED } else { OK }
}
