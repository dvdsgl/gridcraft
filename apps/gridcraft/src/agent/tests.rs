#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::disallowed_methods)] // native-only tests: the clock is fine here

use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use gridcraft_ui_egui::agent::{AgentConnection, AgentEvent};
use serde_json::{Value as Json, json};

use super::acp::{AcpClient, Config};
use super::*;

/// The fake agent: this test binary run again with `GRIDCRAFT_FAKE_ACP` set, so it speaks ACP on
/// its standard streams instead of running tests. Without the variable it is an ordinary (empty)
/// test. The harness's own lines on stdout ("running 1 test") are not JSON, which the client
/// ignores, as it must for any agent that prints a banner.
#[test]
fn fake_agent() {
    let Ok(mode) = std::env::var("GRIDCRAFT_FAKE_ACP") else { return };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    // The harness has written "test … ... " without a line end: end its line first.
    writeln!(out).unwrap();
    let mut send = |v: Json| {
        writeln!(out, "{v}").unwrap();
        out.flush().unwrap();
    };
    let update = |u: Json| json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1", "update": u}});
    let chunk = |t: &str| update(json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": t}}));
    let mut prompt_id = Json::Null;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Json>(&line) else { continue };
        let id = msg["id"].clone();
        match msg["method"].as_str() {
            Some("initialize") => {
                assert_eq!(msg["params"]["protocolVersion"], 1);
                if mode == "crash" {
                    eprintln!("boom: no model is configured");
                    std::process::exit(3);
                }
                send(json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": 1, "agentCapabilities": {}, "authMethods": []}}));
            }
            Some("session/new") => {
                if mode == "noauth" {
                    send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": "Authentication required"}}));
                    continue;
                }
                let servers = msg["params"]["mcpServers"].as_array().map_or(0, Vec::len);
                let name = msg["params"]["mcpServers"][0]["name"].as_str().unwrap_or("").to_string();
                send(json!({"jsonrpc": "2.0", "id": id, "result": {"sessionId": "s1", "models": {"currentModelId": "fake/model"}}}));
                // Tell the test what the session got.
                send(json!({"note": format!("{servers} {name}")}));
            }
            Some("session/prompt") => {
                prompt_id = id;
                let text = msg["params"]["prompt"][0]["text"].as_str().unwrap_or("");
                assert_eq!(msg["params"]["sessionId"], "s1");
                send(chunk("Hello "));
                if mode == "slow" {
                    continue; // wait for session/cancel
                }
                send(update(
                    json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Read notes.txt", "kind": "read", "status": "pending"}),
                ));
                // A client method GridCraft doesn't offer is refused, not left hanging.
                send(json!({"jsonrpc": "2.0", "id": 7, "method": "fs/read_text_file", "params": {"sessionId": "s1", "path": "/etc/hostname"}}));
                send(json!({
                    "jsonrpc": "2.0", "id": 8, "method": "session/request_permission",
                    "params": {"sessionId": "s1", "toolCall": {"toolCallId": "t1", "title": "Read notes.txt"},
                        "options": [{"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"}, {"optionId": "no", "name": "Reject", "kind": "reject_once"}]}
                }));
                if text.contains("echo") {
                    send(chunk(&format!("[{}]", text.len())));
                }
            }
            Some("session/cancel") => {
                send(json!({"jsonrpc": "2.0", "id": prompt_id, "result": {"stopReason": "cancelled"}}));
            }
            None if id == 7 => {
                let refused = msg["error"]["code"] == -32601;
                send(chunk(if refused { "fs refused, " } else { "fs answered?! " }));
            }
            None if id == 8 => {
                let outcome = &msg["result"]["outcome"];
                let picked =
                    if outcome["outcome"] == "selected" { outcome["optionId"].as_str().unwrap_or("?").to_string() } else { "cancelled".into() };
                send(chunk(&format!("{picked}, ")));
                send(update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed"})));
                send(update(json!({"sessionUpdate": "plan", "entries": [{"content": "Read", "status": "completed", "priority": "high"}]})));
                send(chunk("world"));
                send(json!({"jsonrpc": "2.0", "id": prompt_id, "result": {"stopReason": "end_turn"}}));
            }
            _ => {}
        }
    }
    std::process::exit(0);
}

fn fake(mode: &str) -> Config {
    Config {
        program: std::env::current_exe().unwrap().to_string_lossy().into_owned(),
        args: vec!["agent::tests::fake_agent".into(), "--exact".into(), "--nocapture".into(), "--test-threads=1".into()],
        cwd: std::env::temp_dir(),
        env: vec![("GRIDCRAFT_FAKE_ACP".into(), mode.into())],
        mcp_servers: mcp_server(std::path::Path::new("/opt/gridcraft"), "127.0.0.1:1", "t0ken"),
        name: "Fake".into(),
    }
}

/// Events until `stop` says so (or 20 s pass).
fn collect(c: &mut AcpClient, mut stop: impl FnMut(&AgentEvent, &mut AcpClient) -> bool) -> Vec<AgentEvent> {
    let mut all = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        for e in c.poll() {
            let done = stop(&e, c);
            all.push(e);
            if done {
                return all;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out; events so far: {all:#?}");
}

fn reply(events: &[AgentEvent]) -> String {
    events.iter().filter_map(|e| if let AgentEvent::Message(t) = e { Some(t.as_str()) } else { None }).collect()
}

#[test]
fn a_turn_streams_tool_calls_permissions_and_a_plan() {
    let mut c = AcpClient::spawn(fake("chat"), std::sync::Arc::new(|| {})).unwrap();
    // Sent before the session exists: it waits for it.
    c.prompt("hi").unwrap();
    let events = collect(&mut c, |e, c| {
        if let AgentEvent::Permission { request, options, .. } = e {
            assert_eq!(options.len(), 2);
            c.answer_permission(request, Some("allow-once"));
        }
        matches!(e, AgentEvent::TurnEnded { .. })
    });
    assert_eq!(events.first(), Some(&AgentEvent::Ready { model: Some("fake/model".into()) }));
    assert_eq!(reply(&events), "Hello fs refused, allow-once, world");
    assert!(events.contains(&AgentEvent::ToolCall {
        id: "t1".into(),
        title: "Read notes.txt".into(),
        kind: "read".into(),
        status: "pending".into()
    }));
    assert!(events.contains(&AgentEvent::ToolCallUpdate { id: "t1".into(), title: None, status: Some("completed".into()) }));
    assert!(events.contains(&AgentEvent::Plan(vec![("Read".into(), "completed".into())])));
    assert_eq!(events.last(), Some(&AgentEvent::TurnEnded { stop_reason: "end_turn".into() }));
    // A second turn on the same session.
    c.prompt("echo me").unwrap();
    let events = collect(&mut c, |e, c| {
        if let AgentEvent::Permission { request, .. } = e {
            c.answer_permission(request, None);
        }
        matches!(e, AgentEvent::TurnEnded { .. })
    });
    assert_eq!(reply(&events), "Hello [7]fs refused, cancelled, world");
}

#[test]
fn stop_cancels_the_turn() {
    let mut c = AcpClient::spawn(fake("slow"), std::sync::Arc::new(|| {})).unwrap();
    c.prompt("go").unwrap();
    let mut cancelled = false;
    let events = collect(&mut c, |e, c| {
        if matches!(e, AgentEvent::Message(_)) && !cancelled {
            cancelled = true;
            c.cancel();
        }
        matches!(e, AgentEvent::TurnEnded { .. })
    });
    assert_eq!(events.last(), Some(&AgentEvent::TurnEnded { stop_reason: "cancelled".into() }));
}

#[test]
fn an_agent_that_dies_says_why() {
    let mut c = AcpClient::spawn(fake("crash"), std::sync::Arc::new(|| {})).unwrap();
    c.prompt("hi").unwrap();
    let events = collect(&mut c, |e, _| matches!(e, AgentEvent::Exited(_)));
    let Some(AgentEvent::Exited(msg)) = events.last() else { panic!("{events:?}") };
    assert!(msg.contains("Fake stopped") && msg.contains("boom: no model is configured"), "{msg}");
    assert!(c.closed());
    assert!(c.prompt("again").is_err(), "a stopped agent refuses prompts");
}

#[test]
fn a_refused_session_fails_the_waiting_prompt() {
    let mut c = AcpClient::spawn(fake("noauth"), std::sync::Arc::new(|| {})).unwrap();
    c.prompt("hi").unwrap();
    let events = collect(&mut c, |e, _| matches!(e, AgentEvent::TurnEnded { .. }));
    assert!(matches!(&events[0], AgentEvent::Failed(m) if m.contains("Authentication required")), "{events:?}");
    assert_eq!(events.last(), Some(&AgentEvent::TurnEnded { stop_reason: "error".into() }));
}

#[test]
fn a_missing_agent_is_named() {
    let mut cfg = fake("chat");
    cfg.program = "/nonexistent/gridcraft-agent".into();
    let Err(e) = AcpClient::spawn(cfg, std::sync::Arc::new(|| {})) else { panic!("spawned a missing program") };
    assert!(e.contains("/nonexistent/gridcraft-agent") && e.contains("not found"), "{e}");
}

#[test]
fn the_session_gets_gridcraft_as_its_mcp_server() {
    let v = mcp_server(std::path::Path::new("/usr/bin/gridcraft"), "127.0.0.1:4242", "abc");
    assert_eq!(
        v,
        json!([{"name": "gridcraft", "command": "/usr/bin/gridcraft", "args": ["--mcp-bridge", "127.0.0.1:4242"], "env": [{"name": "GRIDCRAFT_CONTROL_TOKEN", "value": "abc"}]}])
    );
}

#[test]
fn omarchy_s_default_agent_and_its_user_install() {
    let home = std::env::temp_dir().join(format!("gridcraft-agent-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    assert_eq!(configured(&home), None, "no setting, no choice");
    std::fs::create_dir_all(home.join(".config/omarchy/defaults")).unwrap();
    std::fs::write(home.join(".config/omarchy/defaults/agent"), "opencode\n").unwrap();
    let c = configured(&home).unwrap();
    assert_eq!((c.id.as_str(), c.name.as_str(), c.omarchy), ("opencode", "OpenCode", true));
    assert_eq!(acp_args("opencode"), Some(vec!["acp".to_string()]));
    assert_eq!(acp_args("claude"), None, "agents without an ACP server are not run");
    assert_eq!(program(Some(&home), "opencode"), "opencode", "found on PATH without a user install");
    std::fs::create_dir_all(home.join(".local/bin")).unwrap();
    std::fs::write(home.join(".local/bin/opencode"), "#!/bin/sh\n").unwrap();
    assert_eq!(program(Some(&home), "opencode"), home.join(".local/bin/opencode").to_string_lossy());
    // A mise wrapper is not the user's own install.
    std::fs::write(home.join(".local/bin/opencode"), "#!/bin/sh\nexec mise x opencode -- opencode \"$@\"\n").unwrap();
    assert_eq!(program(Some(&home), "opencode"), "opencode");
    let _ = std::fs::remove_dir_all(&home);
}

/// One real turn with the configured agent (OpenCode), its tools pointed at a GridCraft window
/// running headless in this test: the sidebar's whole path (ACP, `gridcraft --mcp-bridge`, the
/// private control channel, commands). Needs the agent, its model and network, so it is ignored
/// by default: `cargo build --release -p gridcraft && cargo test --release -p gridcraft --
/// --ignored opencode_end_to_end --nocapture`.
#[test]
#[ignore = "runs the real agent (network, model credentials)"]
fn opencode_end_to_end() {
    use gridcraft_engine::Session;
    use gridcraft_ui_egui::{Services, SheetApp};
    // The bridge is the app's own executable, next to the test binary's folder.
    let exe = std::env::current_exe().unwrap();
    let app_exe = exe.parent().and_then(|d| d.parent()).map(|d| d.join("gridcraft")).unwrap();
    assert!(app_exe.exists(), "build the app first: {}", app_exe.display());
    let mut session = Session::new();
    session.new_workbook();
    session.execute("cell.set", json!({"cell": "B2", "input": "Kestrel 4711"})).unwrap();
    // The real launcher, with the app's executable as the MCP bridge.
    let launch = launch_with(Some(app_exe)).expect("an agent");
    eprintln!("agent: {} ({})", launch.name, launch.command);
    let mut app = SheetApp::new(session, Services { agent: Some(launch), ..Default::default() });
    let ctx = egui::Context::default();
    let tick = |app: &mut SheetApp| {
        let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| app.logic(ctx));
    };
    tick(&mut app);
    let sent = app.run("agent.prompt", json!({"text": "What is in cell B2 of the active sheet? Use the gridcraft tools to read it, then answer with just the value.", "selection": false}));
    eprintln!("prompt: {sent:?}");
    let deadline = Instant::now() + Duration::from_secs(240);
    tick(&mut app);
    while app.agent.responding() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        tick(&mut app);
    }
    let summary = gridcraft_ui_egui::agent::inspect(&app);
    eprintln!("{}", serde_json::to_string_pretty(&summary).unwrap());
    assert!(!app.agent.responding(), "the turn ended");
    let text = summary.to_string();
    assert!(text.contains("4711"), "the reply names the value");
    // A second turn that changes the workbook: one undo step, a changes card, and its Undo.
    let undo_before = app.session.active().map(|d| d.undo.len()).unwrap_or(0);
    let sent = app.run("agent.prompt", json!({"text": "Set cell C2 to the text Done. Use the gridcraft tools.", "selection": false}));
    eprintln!("prompt: {sent:?}");
    let deadline = Instant::now() + Duration::from_secs(240);
    tick(&mut app);
    while app.agent.responding() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        tick(&mut app);
    }
    let summary = gridcraft_ui_egui::agent::inspect(&app);
    eprintln!("{}", serde_json::to_string_pretty(&summary["items"].as_array().map(|a| a[a.len().saturating_sub(4)..].to_vec())).unwrap());
    let c2 = app.session.run("cell.get", json!({"cell": "C2"})).unwrap()["text"].as_str().unwrap_or("").to_string();
    assert_eq!(c2, "Done");
    assert_eq!(app.session.active().map(|d| d.undo.len()), Some(undo_before + 1), "one undo step for the turn");
    app.run("agent.undo", json!({})).unwrap();
    let c2 = app.session.run("cell.get", json!({"cell": "C2"})).unwrap()["text"].as_str().unwrap_or("").to_string();
    assert_eq!(c2, "", "the card's Undo took it back");
}
