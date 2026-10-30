//! The agent behind the sidebar: Omarchy's default coding agent, driven over the Agent Client
//! Protocol, with GridCraft's MCP server pointed at this window.
//!
//! Omarchy's `omarchy-agent` is a launcher that opens the default agent in a terminal, unattended;
//! the sidebar needs the agent itself, so this reads the same setting
//! (`~/.config/omarchy/defaults/agent`) and the same user-install rule, then runs the agent's ACP
//! server (`opencode acp`). `GRIDCRAFT_AGENT_COMMAND` replaces the command line (tests, other
//! agents).
//!
//! Starting the agent also starts a private control channel (a port the system picks, a token kept
//! in memory). The agent gets GridCraft's MCP server as `gridcraft --mcp-bridge ADDR` with the
//! token in its environment, so every tool call it makes reaches this window as a control request,
//! and runs as a command.

pub mod acp;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use gridcraft_ui_egui::agent::{AgentLaunch, AgentStarted, Waker};
use serde_json::json;

/// The configured agent: its id ("opencode") and display name ("OpenCode").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub name: String,
    /// Where the choice came from ("Omarchy default agent").
    pub detail: String,
    pub omarchy: bool,
}

/// Display names, as `omarchy-default-agent` gives them.
fn display_name(id: &str) -> String {
    match id {
        "opencode" => "OpenCode",
        "pi" => "Pi",
        "omp" => "Oh My Pi",
        "ori" => "Ori",
        "claude" => "Claude Code",
        "codex" => "Codex",
        "crush" => "Crush",
        "grok" => "Grok",
        "openclaw" => "OpenClaw",
        "agy" => "Antigravity",
        "hermes" => "Hermes",
        "copilot" => "GitHub Copilot",
        "muse" => "Muse Code",
        "cursor-agent" => "Cursor CLI",
        "kimi" => "Kimi",
        "qwen" => "Qwen Code",
        "dsh" => "DeepSeek Harness",
        "kilo" => "Kilo",
        "devin" => "Devin",
        "amp" => "Amp",
        "commandcode" => "Command Code",
        other => other,
    }
    .to_string()
}

/// The arguments that start an agent's ACP server, for the agents known to have one.
fn acp_args(id: &str) -> Option<Vec<String>> {
    match id {
        // Kilo is built on OpenCode and shares its commands.
        "opencode" | "kilo" => Some(vec!["acp".into()]),
        _ => None,
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Reads the default agent the way `omarchy-default-agent` does: the first line of
/// `~/.config/omarchy/defaults/agent`.
pub fn configured(home: &Path) -> Option<Choice> {
    let text = std::fs::read_to_string(home.join(".config/omarchy/defaults/agent")).ok()?;
    let id = text.lines().next()?.trim().to_string();
    if id.is_empty() {
        return None;
    }
    Some(Choice { name: display_name(&id), id, detail: "Omarchy default agent".into(), omarchy: true })
}

/// `omarchy-mise-install` wrappers install the tool they wrap when run; a symlink is the user's
/// own install.
fn mise_wrapper(path: &Path) -> bool {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return false;
    }
    let mut head = vec![0u8; 4096];
    let n = std::fs::File::open(path).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
    let text = String::from_utf8_lossy(head.get(..n).unwrap_or(&[]));
    text.contains("mise use ") || text.contains("mise x ")
}

/// The program to run: the user's own install in `~/.local/bin` wins, as in `omarchy-agent`;
/// otherwise the name, found on `PATH`.
fn program(home: Option<&Path>, id: &str) -> String {
    if let Some(h) = home {
        let user = h.join(".local/bin").join(id);
        if user.is_file() && !mise_wrapper(&user) {
            return user.to_string_lossy().into_owned();
        }
    }
    id.to_string()
}

/// The session's working folder: an empty folder of GridCraft's own, so the agent works on the
/// workbook through its tools rather than wandering through files.
fn work_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join("gridcraft").join("agent");
    if std::fs::create_dir_all(&dir).is_ok() { dir } else { std::env::temp_dir() }
}

/// The MCP server entry for `session/new`: this executable as a bridge to `addr`.
pub fn mcp_server(exe: &Path, addr: &str, token: &str) -> serde_json::Value {
    json!([{
        "name": "gridcraft",
        "command": exe.to_string_lossy(),
        "args": ["--mcp-bridge", addr],
        "env": [{"name": "GRIDCRAFT_CONTROL_TOKEN", "value": token}],
    }])
}

/// The sidebar's agent: what it is called and how it starts. `None` leaves the sidebar out.
pub fn launch() -> Option<AgentLaunch> {
    launch_with(None)
}

/// [`launch`], with `bridge` as the program the agent runs for GridCraft's MCP server (this
/// executable when `None`).
pub fn launch_with(bridge: Option<PathBuf>) -> Option<AgentLaunch> {
    let home = home();
    let custom = std::env::var("GRIDCRAFT_AGENT_COMMAND").ok().filter(|c| !c.trim().is_empty());
    let choice = match &custom {
        Some(_) => Some(Choice {
            id: "custom".into(),
            name: std::env::var("GRIDCRAFT_AGENT_NAME").unwrap_or_else(|_| "Agent".into()),
            detail: String::new(),
            omarchy: false,
        }),
        None => home.as_deref().and_then(configured),
    };
    let omarchy = Path::new("/usr/share/omarchy").exists() || choice.as_ref().is_some_and(|c| c.omarchy);
    // No setting at all: OpenCode when it is installed, else a sidebar that says what to do.
    let choice = choice.unwrap_or(Choice { id: "opencode".into(), name: "OpenCode".into(), detail: String::new(), omarchy: false });
    let (prog, args) = match &custom {
        Some(c) => {
            let mut parts = c.split_whitespace().map(str::to_string);
            (parts.next().unwrap_or_default(), parts.collect())
        }
        None => (program(home.as_deref(), &choice.id), acp_args(&choice.id).unwrap_or_default()),
    };
    let supported = custom.is_some() || acp_args(&choice.id).is_some();
    let cfg = acp::Config { program: prog, args, cwd: work_dir(), env: Vec::new(), mcp_servers: json!([]), name: choice.name.clone() };
    let command = cfg.command_line();
    let name = choice.name.clone();
    let start = move |waker: Waker| -> Result<AgentStarted, String> {
        if !supported {
            return Err(format!(
                "GridCraft talks to agents over the Agent Client Protocol, which {name} doesn't offer. OpenCode does: run `omarchy default agent opencode`."
            ));
        }
        let (addr, token, control) =
            crate::control_server::start_private(waker.clone()).map_err(|e| format!("no control channel for the agent: {e}"))?;
        let exe = match &bridge {
            Some(b) => b.clone(),
            None => std::env::current_exe().map_err(|e| format!("can't find GridCraft's own program for the agent's tools: {e}"))?,
        };
        let mut cfg = cfg.clone();
        cfg.mcp_servers = mcp_server(&exe, &addr, &token);
        let client = acp::AcpClient::spawn(cfg, waker)?;
        Ok(AgentStarted { connection: Box::new(client), control: Some(control) })
    };
    Some(AgentLaunch {
        id: choice.id,
        name: choice.name,
        detail: choice.detail,
        command,
        change_hint: omarchy.then(|| "omarchy default agent".to_string()),
        start: Box::new(start),
    })
}

/// `gridcraft --mcp-bridge ADDR`: GridCraft's MCP server on stdio, forwarding to the window
/// listening on `ADDR` (token from `GRIDCRAFT_CONTROL_TOKEN`). The agent sidebar's agent runs it.
pub fn mcp_bridge(addr: &str) -> Result<(), String> {
    let addr = gridcraft_mcp::control_addr(addr);
    let remote = gridcraft_mcp::Remote::connect(&addr).map_err(|e| format!("cannot connect to the GridCraft window at {addr}: {e}"))?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    gridcraft_mcp::Server::new(Box::new(remote)).serve(stdin.lock(), stdout.lock()).map_err(|e| e.to_string())
}
