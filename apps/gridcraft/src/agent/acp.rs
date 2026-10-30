//! A client for the Agent Client Protocol (<https://agentclientprotocol.com>): JSON-RPC 2.0 as
//! newline-delimited JSON over the agent process's stdin and stdout, hand-written on std threads
//! (no async runtime), as the MCP server is.
//!
//! [`AcpClient::spawn`] starts the agent and runs the handshake on a reader thread
//! (`initialize`, then `session/new` with the client's working folder and MCP servers). Prompts sent
//! before the session exists wait for it. Everything the agent reports arrives as
//! [`AgentEvent`]s on a channel, and the waker runs after each one so the UI repaints.
//!
//! The client offers no file system or terminal of its own (its capabilities say so), so the
//! agent works on the workbook through the MCP servers it is given; `fs/*` and `terminal/*`
//! requests are refused.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use gridcraft_ui_egui::agent::{AgentConnection, AgentEvent, PermissionOption, Waker};
use serde_json::{Value as Json, json};

/// The protocol version this client speaks.
pub const PROTOCOL_VERSION: u64 = 1;
/// The longest line accepted from the agent; a longer one ends the connection.
const MAX_LINE: u64 = 32 * 1024 * 1024;
/// Lines of the agent's standard error kept for error messages.
const STDERR_LINES: usize = 12;

/// How to start the agent.
#[derive(Clone, Debug)]
pub struct Config {
    pub program: String,
    pub args: Vec<String>,
    /// The session's working folder (`session/new` `cwd`), and the process's.
    pub cwd: std::path::PathBuf,
    /// Extra environment for the agent process.
    pub env: Vec<(String, String)>,
    /// `session/new` `mcpServers`.
    pub mcp_servers: Json,
    /// The agent's name, for messages ("OpenCode").
    pub name: String,
}

impl Config {
    /// The command line, for messages.
    pub fn command_line(&self) -> String {
        std::iter::once(self.program.as_str()).chain(self.args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
    }
}

/// What a request we sent was for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    Initialize,
    NewSession,
    Prompt,
}

#[derive(Default)]
struct Shared {
    session: Option<String>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    /// Prompts sent before the session existed.
    queued: VecDeque<String>,
    /// Permission requests not answered yet (their JSON-RPC ids).
    open_permissions: Vec<Json>,
    closed: bool,
}

/// One running agent.
pub struct AcpClient {
    child: Child,
    shared: Arc<Mutex<Shared>>,
    writer: Arc<Mutex<ChildStdin>>,
    events: Receiver<AgentEvent>,
    tx: Sender<AgentEvent>,
    waker: Waker,
}

/// Writes one JSON-RPC message as a line.
fn write_line(writer: &Mutex<ChildStdin>, msg: &Json) -> std::io::Result<()> {
    let mut w = writer.lock().map_err(|_| std::io::Error::other("the agent's input is poisoned"))?;
    let mut line = msg.to_string();
    line.push('\n');
    w.write_all(line.as_bytes())?;
    w.flush()
}

/// Sends a request and remembers what it was for.
fn request(shared: &Mutex<Shared>, writer: &Mutex<ChildStdin>, method: &str, params: Json, what: Pending) -> std::io::Result<()> {
    let id = {
        let mut s = shared.lock().map_err(|_| std::io::Error::other("poisoned"))?;
        let id = s.next_id;
        s.next_id = s.next_id.wrapping_add(1);
        s.pending.insert(id, what);
        id
    };
    write_line(writer, &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
}

fn prompt_params(session: &str, text: &str) -> Json {
    json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]})
}

impl AcpClient {
    /// Starts the agent and its handshake. Fails only when the process can't be started.
    pub fn spawn(cfg: Config, waker: Waker) -> Result<AcpClient, String> {
        let mut cmd = Command::new(&cfg.program);
        cmd.args(&cfg.args).current_dir(&cfg.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!("`{}` was not found (is {} installed?)", cfg.program, cfg.name),
            _ => format!("couldn't run `{}`: {e}", cfg.command_line()),
        })?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the agent's standard streams are not available".into());
        };
        let writer = Arc::new(Mutex::new(stdin));
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, events) = channel();
        let tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
        {
            let tail = Arc::clone(&tail);
            std::thread::Builder::new()
                .name("agent-stderr".into())
                .spawn(move || {
                    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                        log::debug!("agent: {line}");
                        if let Ok(mut t) = tail.lock() {
                            if t.len() >= STDERR_LINES {
                                t.pop_front();
                            }
                            t.push_back(line);
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        request(
            &shared,
            &writer,
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
                "clientInfo": {"name": "gridcraft", "title": "GridCraft", "version": env!("CARGO_PKG_VERSION")},
            }),
            Pending::Initialize,
        )
        .map_err(|e| format!("couldn't talk to `{}`: {e}", cfg.command_line()))?;
        let reader = Reader { shared: Arc::clone(&shared), writer: Arc::clone(&writer), tx: tx.clone(), waker: Arc::clone(&waker), cfg, tail };
        std::thread::Builder::new().name("agent-acp".into()).spawn(move || reader.run(stdout)).map_err(|e| e.to_string())?;
        Ok(AcpClient { child, shared, writer, events, tx, waker })
    }

    fn emit(&self, e: AgentEvent) {
        let _ = self.tx.send(e);
        (self.waker)();
    }

    /// Whether the agent process is still there.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn closed(&self) -> bool {
        self.shared.lock().map(|s| s.closed).unwrap_or(true)
    }
}

impl AgentConnection for AcpClient {
    fn prompt(&mut self, text: &str) -> Result<(), String> {
        let session = {
            let mut s = self.shared.lock().map_err(|_| "the agent connection is broken".to_string())?;
            if s.closed {
                return Err("the agent has stopped".into());
            }
            match s.session.clone() {
                Some(id) => id,
                None => {
                    s.queued.push_back(text.to_string());
                    return Ok(());
                }
            }
        };
        request(&self.shared, &self.writer, "session/prompt", prompt_params(&session, text), Pending::Prompt).map_err(|e| e.to_string())
    }

    fn cancel(&mut self) {
        let (session, open, queued) = match self.shared.lock() {
            Ok(mut s) => (s.session.clone(), std::mem::take(&mut s.open_permissions), std::mem::take(&mut s.queued).len()),
            Err(_) => return,
        };
        // Unanswered permission requests are answered "cancelled", as the protocol asks.
        for id in open {
            let _ = write_line(&self.writer, &json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": {"outcome": "cancelled"}}}));
        }
        if let Some(session) = session {
            let _ = write_line(&self.writer, &json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}}));
        }
        // A prompt still waiting for the session never reached the agent: it ends here.
        if queued > 0 {
            self.emit(AgentEvent::TurnEnded { stop_reason: "cancelled".into() });
        }
    }

    fn answer_permission(&mut self, request: &Json, option: Option<&str>) {
        if let Ok(mut s) = self.shared.lock() {
            s.open_permissions.retain(|id| id != request);
        }
        let outcome = match option {
            Some(id) => json!({"outcome": "selected", "optionId": id}),
            None => json!({"outcome": "cancelled"}),
        };
        let _ = write_line(&self.writer, &json!({"jsonrpc": "2.0", "id": request, "result": {"outcome": outcome}}));
    }

    fn poll(&mut self) -> Vec<AgentEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for AcpClient {
    fn drop(&mut self) {
        if let Ok(mut s) = self.shared.lock() {
            s.closed = true;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The reader thread: the agent's output, line by line.
struct Reader {
    shared: Arc<Mutex<Shared>>,
    writer: Arc<Mutex<ChildStdin>>,
    tx: Sender<AgentEvent>,
    waker: Waker,
    cfg: Config,
    tail: Arc<Mutex<VecDeque<String>>>,
}

impl Reader {
    fn emit(&self, e: AgentEvent) {
        let _ = self.tx.send(e);
        (self.waker)();
    }

    fn run(self, stdout: impl Read) {
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::new();
        // `GRIDCRAFT_ACP_TRACE=1` prints every message from the agent on standard error.
        let trace = std::env::var_os("GRIDCRAFT_ACP_TRACE").is_some();
        loop {
            buf.clear();
            match reader.by_ref().take(MAX_LINE).read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if buf.last() != Some(&b'\n') && buf.len() as u64 >= MAX_LINE {
                log::warn!("agent: a message over {MAX_LINE} bytes; closing the connection");
                break;
            }
            // Agents may print other things on stdout (a banner, a test harness's chatter): only
            // JSON objects are messages.
            let Ok(msg) = serde_json::from_slice::<Json>(&buf) else { continue };
            log::trace!("agent: {}", String::from_utf8_lossy(&buf).trim_end());
            if trace {
                let _ = writeln!(std::io::stderr(), "acp <- {}", String::from_utf8_lossy(&buf).trim_end());
            }
            if msg.is_object() {
                self.handle(&msg);
            }
        }
        let stopped_by_us = self.shared.lock().map(|s| s.closed).unwrap_or(true);
        if let Ok(mut s) = self.shared.lock() {
            s.closed = true;
        }
        if !stopped_by_us {
            let tail = self.tail.lock().map(|t| t.iter().cloned().collect::<Vec<_>>().join("\n")).unwrap_or_default();
            let mut msg = format!("{} stopped (`{}`).", self.cfg.name, self.cfg.command_line());
            if !tail.trim().is_empty() {
                msg.push_str(&format!("\n{}", tail.trim()));
            }
            self.emit(AgentEvent::Exited(msg));
        }
    }

    fn handle(&self, msg: &Json) {
        let method = msg.get("method").and_then(Json::as_str);
        let id = msg.get("id").filter(|v| !v.is_null()).cloned();
        match (method, id) {
            (Some(m), Some(id)) => self.on_request(m, id, msg.get("params").unwrap_or(&Json::Null)),
            (Some(m), None) => self.on_notification(m, msg.get("params").unwrap_or(&Json::Null)),
            (None, Some(id)) => self.on_response(&id, msg),
            (None, None) => {}
        }
    }

    fn on_response(&self, id: &Json, msg: &Json) {
        let Some(n) = id.as_u64() else { return };
        let what = self.shared.lock().ok().and_then(|mut s| s.pending.remove(&n));
        let Some(what) = what else { return };
        let error = msg.get("error").map(|e| {
            let text = e.get("message").and_then(Json::as_str).unwrap_or("unknown error").to_string();
            match e.get("data") {
                Some(Json::String(d)) if !d.is_empty() => format!("{text}: {d}"),
                Some(d @ Json::Object(_)) => format!("{text} {d}"),
                _ => text,
            }
        });
        let result = msg.get("result").cloned().unwrap_or(Json::Null);
        match (what, error) {
            (Pending::Initialize, Some(e)) => self.emit(AgentEvent::Failed(format!("{} refused to start: {e}", self.cfg.name))),
            (Pending::Initialize, None) => {
                let params = json!({"cwd": self.cfg.cwd.to_string_lossy(), "mcpServers": self.cfg.mcp_servers.clone()});
                if let Err(e) = request(&self.shared, &self.writer, "session/new", params, Pending::NewSession) {
                    self.emit(AgentEvent::Failed(format!("couldn't start a session: {e}")));
                }
            }
            (Pending::NewSession, Some(e)) => {
                // Prompts that waited for the session end with it.
                let queued = self.shared.lock().map(|mut s| std::mem::take(&mut s.queued).len()).unwrap_or(0);
                self.emit(AgentEvent::Failed(format!("{} couldn't start a session: {e}", self.cfg.name)));
                for _ in 0..queued {
                    self.emit(AgentEvent::TurnEnded { stop_reason: "error".into() });
                }
            }
            (Pending::NewSession, None) => {
                let Some(session) = result.get("sessionId").and_then(Json::as_str).map(str::to_string) else {
                    self.emit(AgentEvent::Failed(format!("{} started a session without an id", self.cfg.name)));
                    return;
                };
                let queued: Vec<String> = match self.shared.lock() {
                    Ok(mut s) => {
                        s.session = Some(session.clone());
                        s.queued.drain(..).collect()
                    }
                    Err(_) => Vec::new(),
                };
                self.emit(AgentEvent::Ready { model: model_of(&result) });
                for text in queued {
                    if let Err(e) = request(&self.shared, &self.writer, "session/prompt", prompt_params(&session, &text), Pending::Prompt) {
                        self.emit(AgentEvent::Failed(format!("couldn't send the prompt: {e}")));
                        self.emit(AgentEvent::TurnEnded { stop_reason: "error".into() });
                    }
                }
            }
            (Pending::Prompt, Some(e)) => {
                self.emit(AgentEvent::Failed(e));
                self.emit(AgentEvent::TurnEnded { stop_reason: "error".into() });
            }
            (Pending::Prompt, None) => {
                let reason = result.get("stopReason").and_then(Json::as_str).unwrap_or("end_turn").to_string();
                self.emit(AgentEvent::TurnEnded { stop_reason: reason });
            }
        }
    }

    fn on_request(&self, method: &str, id: Json, params: &Json) {
        match method {
            "session/request_permission" => {
                let call = params.get("toolCall").unwrap_or(&Json::Null);
                let title = call.get("title").and_then(Json::as_str).filter(|t| !t.is_empty()).unwrap_or("use a tool").to_string();
                let options = params
                    .get("options")
                    .and_then(Json::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|o| {
                                Some(PermissionOption {
                                    id: o.get("optionId")?.as_str()?.to_string(),
                                    name: o.get("name").and_then(Json::as_str).unwrap_or("").to_string(),
                                    kind: o.get("kind").and_then(Json::as_str).unwrap_or("").to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if let Ok(mut s) = self.shared.lock() {
                    s.open_permissions.push(id.clone());
                }
                self.emit(AgentEvent::Permission { request: id, title, options });
            }
            _ => {
                // No file system, terminal or other client methods: the workbook is reached
                // through the MCP server.
                let reply = json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("GridCraft does not offer {method}")}});
                let _ = write_line(&self.writer, &reply);
            }
        }
    }

    fn on_notification(&self, method: &str, params: &Json) {
        if method != "session/update" {
            return;
        }
        let Some(u) = params.get("update") else { return };
        let s = |k: &str| u.get(k).and_then(Json::as_str).unwrap_or("").to_string();
        let event = match u.get("sessionUpdate").and_then(Json::as_str).unwrap_or("") {
            "agent_message_chunk" => text_of(u).map(AgentEvent::Message),
            "agent_thought_chunk" => text_of(u).map(AgentEvent::Thought),
            "tool_call" => Some(AgentEvent::ToolCall {
                id: s("toolCallId"),
                title: tool_title(u).unwrap_or_else(|| s("title")),
                kind: s("kind"),
                status: s("status"),
            }),
            "tool_call_update" => Some(AgentEvent::ToolCallUpdate {
                id: s("toolCallId"),
                title: tool_title(u).or_else(|| u.get("title").and_then(Json::as_str).map(str::to_string)),
                status: u.get("status").and_then(Json::as_str).map(str::to_string),
            }),
            "plan" => u.get("entries").and_then(Json::as_array).map(|e| {
                AgentEvent::Plan(
                    e.iter()
                        .map(|x| {
                            (
                                x.get("content").and_then(Json::as_str).unwrap_or("").to_string(),
                                x.get("status").and_then(Json::as_str).unwrap_or("").to_string(),
                            )
                        })
                        .collect(),
                )
            }),
            "config_option_update" => model_of(u).map(|m| AgentEvent::Ready { model: Some(m) }),
            _ => None,
        };
        if let Some(e) = event {
            self.emit(e);
        }
    }
}

/// A tool call's title from what it runs. OpenCode reaches MCP tools through one generic
/// `execute` tool whose input is a script (`tools.gridcraft.get_cell(…)`): a call that uses
/// GridCraft's tools is titled "gridcraft" (the sidebar shows those as the reads and writes they
/// make), a tool search "Looked up tools", and a bare `execute` "Working".
fn tool_title(u: &Json) -> Option<String> {
    let code = u.pointer("/rawInput/code").and_then(Json::as_str).unwrap_or("");
    if code.contains("gridcraft") {
        return Some("gridcraft".into());
    }
    if code.contains("search(") {
        return Some("Looked up tools".into());
    }
    (u.get("title").and_then(Json::as_str) == Some("execute")).then(|| "Working".to_string())
}

/// The text of a content block (`{"content": {"type": "text", "text": …}}`).
fn text_of(u: &Json) -> Option<String> {
    let c = u.get("content")?;
    match c.get("type").and_then(Json::as_str) {
        Some("text") => c.get("text").and_then(Json::as_str).map(str::to_string),
        _ => None,
    }
}

/// The session's current model: `models.currentModelId`, or the `model` config option.
fn model_of(v: &Json) -> Option<String> {
    if let Some(m) = v.pointer("/models/currentModelId").and_then(Json::as_str) {
        return Some(m.to_string());
    }
    v.get("configOptions")
        .and_then(Json::as_array)?
        .iter()
        .find(|o| o.get("id").and_then(Json::as_str) == Some("model") || o.get("category").and_then(Json::as_str) == Some("model"))
        .and_then(|o| o.get("currentValue").and_then(Json::as_str))
        .map(str::to_string)
}
