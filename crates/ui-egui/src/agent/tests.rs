#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc::{Sender, channel};

use serde_json::{Value as Json, json};

use super::*;
use crate::{ControlRequest, Services, SheetApp};

#[derive(Default)]
struct Log {
    prompts: Vec<String>,
    cancels: usize,
    answers: Vec<(Json, Option<String>)>,
    events: VecDeque<AgentEvent>,
}

struct Fake(Rc<RefCell<Log>>);

impl AgentConnection for Fake {
    fn prompt(&mut self, text: &str) -> Result<(), String> {
        self.0.borrow_mut().prompts.push(text.to_string());
        Ok(())
    }
    fn cancel(&mut self) {
        self.0.borrow_mut().cancels += 1;
    }
    fn answer_permission(&mut self, request: &Json, option: Option<&str>) {
        self.0.borrow_mut().answers.push((request.clone(), option.map(str::to_string)));
    }
    fn poll(&mut self) -> Vec<AgentEvent> {
        self.0.borrow_mut().events.drain(..).collect()
    }
}

struct Rig {
    app: SheetApp,
    log: Rc<RefCell<Log>>,
    tx: Sender<ControlRequest>,
    ctx: egui::Context,
}

fn rig() -> Rig {
    let log: Rc<RefCell<Log>> = Rc::default();
    let (tx, rx) = channel();
    let rx = RefCell::new(Some(rx));
    let l = log.clone();
    let launch = AgentLaunch {
        id: "opencode".into(),
        name: "OpenCode".into(),
        detail: "Omarchy default agent".into(),
        command: "opencode acp".into(),
        change_hint: None,
        start: Box::new(move |_| Ok(AgentStarted { connection: Box::new(Fake(l.clone())), control: rx.borrow_mut().take() })),
    };
    let mut session = Session::new();
    session.new_workbook();
    session.execute("cell.set", json!({"cell": "A1", "input": "Region"})).unwrap();
    session.execute("cell.set", json!({"cell": "B1", "input": "Sales"})).unwrap();
    session.execute("cell.set", json!({"cell": "A2", "input": "North"})).unwrap();
    session.execute("cell.set", json!({"cell": "B2", "input": "120"})).unwrap();
    let app = SheetApp::new(session, Services { agent: Some(launch), ..Default::default() });
    Rig { app, log, tx, ctx: egui::Context::default() }
}

impl Rig {
    fn tick(&mut self) {
        let app = &mut self.app;
        let _ = self.ctx.run_logic(&egui::RawInput::default(), |ctx| app.logic(ctx));
    }
    fn emit(&mut self, e: AgentEvent) {
        self.log.borrow_mut().events.push_back(e);
        self.tick();
    }
    /// One of the agent's tool calls, as its MCP server sends it; returns the reply.
    fn tool(&mut self, method: &str, params: Json) -> Json {
        let (req, reply) = ControlRequest::new(method, params);
        self.tx.send(req).unwrap();
        self.tick();
        reply.try_recv().unwrap()
    }
    fn set(&mut self, cell: &str, input: &str) -> Json {
        self.tool("engine.execute", json!({"command": "cell.set", "params": {"cell": cell, "input": input}}))
    }
    fn text(&mut self, cell: &str) -> String {
        self.app.session.run("cell.get", json!({"cell": cell})).unwrap()["text"].as_str().unwrap_or("").to_string()
    }
    fn undo_len(&self) -> usize {
        self.app.session.active().unwrap().undo.len()
    }
}

#[test]
fn ctrl_j_and_the_command_toggle_the_sidebar() {
    let mut r = rig();
    assert!(!r.app.agent.open);
    assert_eq!(r.app.run("view.agent", json!({})).unwrap(), json!({"on": true}));
    let mut input = egui::RawInput::default();
    input.events.push(egui::Event::Key { key: egui::Key::J, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND });
    let app = &mut r.app;
    let mut out = r.ctx.run_ui(input, |ui| keys(app, ui.ctx()));
    out.textures_delta.clear();
    assert!(!r.app.agent.open, "Ctrl+J closes it again");
    assert_eq!(r.app.run("view.agent", json!({"on": true})).unwrap(), json!({"on": true}));
}

#[test]
fn without_a_host_agent_there_is_no_sidebar() {
    let mut session = Session::new();
    session.new_workbook();
    let mut app = SheetApp::new(session, Services::default());
    assert!(!available(&app));
    assert!(app.run("view.agent", json!({})).is_err());
    assert!(app.run("agent.prompt", json!({"text": "hi"})).is_err());
}

#[test]
fn a_turn_s_changes_are_one_undo_step_with_a_card() {
    let mut r = rig();
    r.app.run("selection.set", json!({"range": "A1:B2"})).unwrap();
    let before = r.undo_len();
    r.app.run("agent.prompt", json!({"text": "Add a forecast"})).unwrap();
    assert!(r.app.agent.responding());
    // The prompt carries the workbook context and the selection's values.
    let sent = r.log.borrow().prompts[0].clone();
    assert!(sent.contains("Selection: Sheet1!A1:B2") && sent.contains("Region\tSales") && sent.ends_with("Add a forecast"), "{sent}");
    r.emit(AgentEvent::Ready { model: Some("opencode/big-pickle".into()) });
    let read = r.tool("sheet.read", json!({"range": "A1:B2"}));
    assert_eq!(read["ok"], true);
    r.emit(AgentEvent::Message("North sells **120**.".into()));
    assert_eq!(r.set("C1", "Forecast")["ok"], true);
    assert_eq!(r.set("C2", "=B2*1.1")["ok"], true);
    assert_eq!(r.tool("engine.execute", json!({"command": "range.setValues", "params": {"range": "C3:C4", "values": [["1"], ["2"]]}}))["ok"], true);
    // While the turn runs, its target is outlined and Undo waits for it.
    let (ranges, label) = overlay(&r.app).unwrap();
    assert_eq!(ranges, vec![("Sheet1".to_string(), RangeRef::parse("C3:C4").unwrap())]);
    assert_eq!(label, "OpenCode · writing C3:C4");
    assert!(gridcraft_engine::cmd::can_undo(&r.app.session).is_err());
    r.emit(AgentEvent::TurnEnded { stop_reason: "end_turn".into() });
    assert!(!r.app.agent.responding());
    assert_eq!(r.undo_len(), before + 1, "the whole turn is one undo step");
    assert_eq!(r.app.session.active().unwrap().undo.last().unwrap().label, "Changes by OpenCode");
    let card = r.app.agent.items.iter().find_map(|i| if let Item::Changes(c) = i { Some(c.clone()) } else { None }).unwrap();
    let rows: Vec<(&str, &str)> = card.ranges.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    assert_eq!(rows, vec![("Sheet1!C1", "Values"), ("Sheet1!C2", "Formulas"), ("Sheet1!C3:C4", "Values")], "neighbouring writes of a kind merge");
    assert_eq!(card.cells, 4);
    let summary = inspect(&r.app).to_string();
    assert!(summary.contains("Read Sheet1!A1:B2") && summary.contains("North sells **120**."), "{summary}");
    // Undo on the card takes the whole turn back.
    assert_eq!(r.text("C2"), "132");
    r.app.run("agent.undo", json!({})).unwrap();
    assert_eq!((r.text("C1"), r.text("C2"), r.text("C4")), (String::new(), String::new(), String::new()));
    assert_eq!(r.undo_len(), before);
    assert!(r.app.run("agent.undo", json!({})).is_err(), "nothing left to undo");
    assert!(overlay(&r.app).is_none());
}

#[test]
fn keep_leaves_the_changes_and_undo_waits_for_later_edits() {
    let mut r = rig();
    r.app.run("agent.prompt", json!({"text": "fill"})).unwrap();
    r.set("D1", "x");
    r.emit(AgentEvent::TurnEnded { stop_reason: "end_turn".into() });
    // A later edit of the user's own: the card can't undo past it.
    r.app.run("cell.set", json!({"cell": "E1", "input": "mine"})).unwrap();
    assert!(r.app.run("agent.undo", json!({})).unwrap_err().contains("Other changes"));
    r.app.run("agent.keep", json!({})).unwrap();
    assert_eq!(r.text("D1"), "x");
    assert!(r.app.run("agent.keep", json!({})).is_err(), "no pending card left");
}

#[test]
fn a_turn_without_changes_leaves_no_step_or_card() {
    let mut r = rig();
    let before = r.undo_len();
    r.app.run("agent.prompt", json!({"text": "what is B2?"})).unwrap();
    r.tool("cell.get", json!({"cell": "B2"}));
    r.emit(AgentEvent::Message("120".into()));
    r.emit(AgentEvent::TurnEnded { stop_reason: "end_turn".into() });
    assert_eq!(r.undo_len(), before);
    assert!(!r.app.agent.items.iter().any(|i| matches!(i, Item::Changes(_))));
    assert!(gridcraft_engine::cmd::can_undo(&r.app.session).is_ok(), "the undo group is closed");
}

#[test]
fn stop_cancels_and_keeps_what_was_written() {
    let mut r = rig();
    r.app.run("agent.prompt", json!({"text": "fill"})).unwrap();
    r.set("D1", "1");
    r.app.run("agent.stop", json!({})).unwrap();
    assert_eq!(r.log.borrow().cancels, 1);
    r.emit(AgentEvent::TurnEnded { stop_reason: "cancelled".into() });
    assert_eq!(r.text("D1"), "1");
    assert!(inspect(&r.app).to_string().contains("Stopped."));
    assert!(r.app.agent.items.iter().any(|i| matches!(i, Item::Changes(_))));
}

#[test]
fn permission_requests_are_answered_from_the_card() {
    let mut r = rig();
    r.app.run("agent.prompt", json!({"text": "go"})).unwrap();
    let options = vec![
        PermissionOption { id: "a1".into(), name: "Allow".into(), kind: "allow_once".into() },
        PermissionOption { id: "r1".into(), name: "Reject".into(), kind: "reject_once".into() },
    ];
    r.emit(AgentEvent::Permission { request: json!(5), title: "Run a command".into(), options: options.clone() });
    r.app.run("agent.permission", json!({"allow": true})).unwrap();
    r.emit(AgentEvent::Permission { request: json!(6), title: "Again".into(), options });
    r.app.run("agent.permission", json!({"allow": false})).unwrap();
    assert_eq!(r.log.borrow().answers, vec![(json!(5), Some("a1".into())), (json!(6), Some("r1".into()))]);
    assert!(r.app.run("agent.permission", json!({"allow": true})).is_err(), "nothing left to answer");
}

#[test]
fn an_agent_that_won_t_start_says_so_with_the_command() {
    let mut session = Session::new();
    session.new_workbook();
    let launch = AgentLaunch {
        id: "opencode".into(),
        name: "OpenCode".into(),
        detail: String::new(),
        command: "opencode acp".into(),
        change_hint: None,
        start: Box::new(|_| Err("`opencode` was not found".into())),
    };
    let mut app = SheetApp::new(session, Services { agent: Some(launch), ..Default::default() });
    let e = app.run("agent.prompt", json!({"text": "hi"})).unwrap_err();
    assert!(e.contains("OpenCode didn't start") && e.contains("Tried: opencode acp"), "{e}");
    assert!(matches!(app.agent.status, Status::Failed(_)));
    assert!(!app.agent.responding());
    assert!(gridcraft_engine::cmd::can_undo(&app.session).is_ok() || app.session.active().unwrap().undo.is_empty());
}

#[test]
fn an_agent_that_exits_mid_turn_ends_the_turn() {
    let mut r = rig();
    r.app.run("agent.prompt", json!({"text": "fill"})).unwrap();
    r.set("D1", "1");
    r.emit(AgentEvent::Exited("OpenCode stopped (`opencode acp`).".into()));
    assert!(!r.app.agent.responding());
    assert!(r.app.agent.conn.is_none());
    assert!(gridcraft_engine::cmd::can_undo(&r.app.session).is_ok(), "the group closed with the turn");
    // The next prompt starts the agent again.
    r.app.run("agent.prompt", json!({"text": "again"})).unwrap();
    assert!(r.app.agent.responding());
}

#[test]
fn the_context_is_bounded() {
    let mut r = rig();
    let long = "x".repeat(500);
    r.app.run("cell.set", json!({"cell": "A3", "input": long})).unwrap();
    r.app.run("selection.set", json!({"range": "A1:Z100"})).unwrap();
    let (ctx, chip) = context(&r.app, true);
    assert_eq!(chip.as_deref(), Some("Sheet1!A1:Z100"));
    assert!(ctx.len() <= CONTEXT_MAX + 4, "{}", ctx.len());
    assert!(ctx.contains("first 12×8 of 100×26"), "{ctx}");
    assert!(!ctx.contains(&"x".repeat(41)), "long values are cut");
    let (ctx, chip) = context(&r.app, false);
    assert!(chip.is_none() && !ctx.contains("Selection"));
}

#[test]
fn coalescing_merges_neighbours_and_counts_distinct_cells() {
    let v = |a: &str, w: &str| (a.to_string(), w.to_string());
    let (rows, cells) =
        coalesce(vec![v("S!E20", "Values"), v("S!E21", "Values"), v("S!E22:E23", "Values"), v("S!E21:E23", "Percent Style"), v("S!G1", "Values")]);
    assert_eq!(rows, vec![v("S!E20:E23", "Values"), v("S!E21:E23", "Percent Style"), v("S!G1", "Values")]);
    assert_eq!(cells, 5);
    // The same range written again keeps one row with its latest action.
    let (rows, _) = coalesce(vec![v("S!A1:B2", "Values"), v("S!A1:B2", "Bold")]);
    assert_eq!(rows, vec![v("S!A1:B2", "Values, Bold")]);
}

#[test]
fn suggestions_come_from_the_workbook() {
    let r = rig();
    let s: Vec<String> = suggestions(&r.app.session).into_iter().map(|(_, t)| t).collect();
    assert_eq!(s.len(), 4);
    assert!(s[0].contains("Sheet1"), "{s:?}");
    assert!(s.iter().any(|t| t == "Chart Sales"), "a text header over numbers names the chart: {s:?}");
}

#[test]
fn opencode_s_script_tool_calls_on_gridcraft_tools_give_way_to_their_reads() {
    let mut r = rig();
    r.app.run("agent.prompt", json!({"text": "B2?"})).unwrap();
    r.emit(AgentEvent::ToolCall { id: "c1".into(), title: "Working".into(), kind: "other".into(), status: "pending".into() });
    assert!(inspect(&r.app).to_string().contains("\"activity\":\"Working\""));
    r.emit(AgentEvent::ToolCallUpdate { id: "c1".into(), title: Some("gridcraft".into()), status: Some("in_progress".into()) });
    r.tool("engine.execute", json!({"command": "cell.get", "params": {"cell": "B2"}}));
    let s = inspect(&r.app).to_string();
    assert!(!s.contains("Working") && s.contains("Read Sheet1!B2"), "{s}");
}
