//! The agent sidebar: a chat with the desktop's coding agent (OpenCode on Omarchy) about the
//! workbook, docked on the right of the window.
//!
//! The UI only sees an abstract agent ([`AgentConnection`]): send a prompt, cancel, answer a
//! permission request, and poll for [`AgentEvent`]s. The host (the desktop app) spawns the agent
//! process, speaks the Agent Client Protocol to it and gives it GridCraft's MCP server; the agent's
//! tool calls reach this window through a control channel of its own ([`AgentStarted::control`]),
//! so they run as commands like any other control request.
//!
//! Every change the agent makes during one turn lands in one undo group ("Changes by OpenCode"),
//! so one Undo reverts the whole turn; after the turn a card lists the changed ranges with Keep and
//! Undo. The web build has no host agent: the sidebar and its title-bar toggle stay hidden.

use std::sync::mpsc::Receiver;

use gridcraft_engine::core::{CellRef, RangeRef};
use gridcraft_engine::{GroupToken, Session};
use serde_json::{Value as Json, json};

use crate::{ControlRequest, SheetApp};

mod markdown;
pub mod view;

#[cfg(test)]
mod tests;

/// Wakes the UI when the agent has something new (called from the host's reader thread).
pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;

/// Starts the agent: the host spawns it and returns the connection.
pub type StartAgent = Box<dyn FnMut(Waker) -> Result<AgentStarted, String>>;

/// The agent the host offers, and how to start it.
pub struct AgentLaunch {
    /// The agent's id as `omarchy default agent` spells it ("opencode"); picks its icon.
    pub id: String,
    /// The agent's name as people know it ("OpenCode").
    pub name: String,
    /// Why this agent ("Omarchy default agent").
    pub detail: String,
    /// The command line the host runs, shown when it fails ("opencode acp").
    pub command: String,
    /// Where the choice of agent is changed, shown in the empty sidebar ("omarchy default agent").
    pub change_hint: Option<String>,
    pub start: StartAgent,
}

/// A started agent.
pub struct AgentStarted {
    pub connection: Box<dyn AgentConnection>,
    /// Control requests from the agent's tools (GridCraft's MCP server talking to this window).
    pub control: Option<Receiver<ControlRequest>>,
}

/// One running agent session, as the sidebar sees it.
pub trait AgentConnection {
    /// Send a user turn. The reply arrives as events; the turn ends with [`AgentEvent::TurnEnded`].
    fn prompt(&mut self, text: &str) -> Result<(), String>;
    /// Ask the agent to stop the running turn (it still ends with [`AgentEvent::TurnEnded`]).
    fn cancel(&mut self);
    /// Answer a permission request: the chosen option id, or `None` to decline.
    fn answer_permission(&mut self, request: &Json, option: Option<&str>);
    /// Events since the last poll.
    fn poll(&mut self) -> Vec<AgentEvent>;
}

/// What the agent reports.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    /// The session is ready; `model` is the agent's current model when it says.
    Ready { model: Option<String> },
    /// A piece of the agent's reply (Markdown).
    Message(String),
    /// A piece of the agent's reasoning (not shown).
    Thought(String),
    /// The agent started a tool call.
    ToolCall { id: String, title: String, kind: String, status: String },
    /// A tool call moved on (`status`: pending, in_progress, completed, failed).
    ToolCallUpdate { id: String, title: Option<String>, status: Option<String> },
    /// The agent's plan, replacing any earlier one: (step, status).
    Plan(Vec<(String, String)>),
    /// The agent asks before a tool call.
    Permission { request: Json, title: String, options: Vec<PermissionOption> },
    /// The turn is over (`end_turn`, `cancelled`, `max_tokens`, `refusal`, `error`…).
    TurnEnded { stop_reason: String },
    /// Something failed (a prompt was refused, the session didn't start).
    Failed(String),
    /// The agent process ended.
    Exited(String),
}

/// One answer a permission request offers.
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    /// `allow_once`, `allow_always`, `reject_once`, `reject_always`.
    pub kind: String,
}

/// The sidebar's state.
#[derive(Default)]
pub struct AgentPanel {
    pub open: bool,
    pub(crate) conn: Option<Box<dyn AgentConnection>>,
    pub(crate) control_rx: Option<Receiver<ControlRequest>>,
    pub(crate) status: Status,
    pub(crate) items: Vec<Item>,
    pub(crate) draft: String,
    pub(crate) turn: Option<Turn>,
    /// The selection chip was removed from the composer.
    pub(crate) drop_selection: bool,
    pub(crate) model: Option<String>,
    pub(crate) focus_composer: bool,
    /// The last error starting the agent, with the command that was tried.
    pub(crate) start_error: Option<String>,
    /// The context the agent's waker repaints.
    pub(crate) ctx: Option<egui::Context>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Status {
    /// Not started yet.
    #[default]
    Idle,
    Starting,
    Ready,
    Responding,
    /// The agent couldn't start or stopped.
    Failed(String),
}

/// One entry of the conversation.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Item {
    User {
        text: String,
        range: Option<String>,
    },
    /// A tool call or a workbook read/write: "Read Data!A1:H200 196 rows".
    Activity {
        key: String,
        verb: String,
        target: String,
        detail: String,
        state: Step,
    },
    /// The agent's reply, streamed (Markdown).
    Reply(String),
    Plan(Vec<(String, String)>),
    Permission {
        request: Json,
        title: String,
        options: Vec<PermissionOption>,
        answer: Option<String>,
    },
    Changes(Changes),
    /// A plain note in the conversation (stopped, errors).
    Note {
        text: String,
        error: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Running,
    Done,
    Failed,
}

/// What one turn changed, kept or undone with the card's buttons.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Changes {
    pub doc: u64,
    /// "Sheet!A1:B2" with what was done there.
    pub ranges: Vec<(String, String)>,
    pub cells: u64,
    /// The length of the undo list right after the turn's step was pushed.
    pub undo_len: usize,
    pub state: ChangeState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeState {
    Pending,
    Kept,
    Undone,
}

/// The running turn.
pub(crate) struct Turn {
    pub group: Option<GroupToken>,
    pub doc: u64,
    /// Changed ranges ("Sheet!A1:B2", what was done) in order.
    pub changed: Vec<(String, String)>,
    pub cells: u64,
    /// Where the agent is writing now (sheet name, range), outlined on the grid.
    pub target: Option<(String, RangeRef)>,
    pub stopping: bool,
}

/// Rows and columns of the selection preview sent with each prompt, and the longest value.
const PREVIEW_ROWS: u32 = 12;
const PREVIEW_COLS: u32 = 8;
const PREVIEW_VALUE: usize = 40;
/// The longest the whole context block may be.
const CONTEXT_MAX: usize = 4000;
/// The most activity lines a turn shows; later ones fold into the last.
const MAX_ITEMS: usize = 400;

impl AgentPanel {
    /// Whether a turn is running.
    pub fn responding(&self) -> bool {
        self.turn.is_some()
    }
}

/// The agent's name, or "Agent" when the host offers none.
pub(crate) fn agent_name(app: &SheetApp) -> String {
    app.services.agent.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| "Agent".into())
}

/// Whether this build can show the sidebar at all.
pub fn available(app: &SheetApp) -> bool {
    app.services.agent.is_some()
}

/// Per-frame upkeep: Ctrl+J, the agent's events and its control requests.
pub fn logic(app: &mut SheetApp, ctx: &egui::Context) {
    if !available(app) {
        return;
    }
    if app.agent.ctx.is_none() {
        app.agent.ctx = Some(ctx.clone());
    }
    poll_events(app);
    poll_control(app, ctx);
}

/// Ctrl+J (Cmd+J on macOS) opens and closes the sidebar, wherever the focus is.
pub fn keys(app: &mut SheetApp, ctx: &egui::Context) {
    if available(app) && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::J)) {
        let _ = toggle(app, None);
    }
}

/// Opens or closes the sidebar (`view.agent`).
pub(crate) fn toggle(app: &mut SheetApp, on: Option<bool>) -> Result<Json, String> {
    if !available(app) {
        return Err("The agent sidebar is not available here.".into());
    }
    app.agent.open = on.unwrap_or(!app.agent.open);
    if app.agent.open {
        app.agent.focus_composer = true;
    }
    Ok(json!({"on": app.agent.open}))
}

/// Starts the agent when it isn't running. `Err` carries the message the sidebar shows.
fn ensure_started(app: &mut SheetApp) -> Result<(), String> {
    if app.agent.conn.is_some() {
        return Ok(());
    }
    let Some(launch) = app.services.agent.as_mut() else { return Err("The agent sidebar is not available here.".into()) };
    let waker: Waker = match app.agent.ctx.clone() {
        Some(ctx) => {
            let ctx = ctx.clone();
            std::sync::Arc::new(move || ctx.request_repaint())
        }
        None => std::sync::Arc::new(|| {}),
    };
    app.agent.status = Status::Starting;
    match (launch.start)(waker) {
        Ok(started) => {
            app.agent.conn = Some(started.connection);
            app.agent.control_rx = started.control;
            app.agent.start_error = None;
            Ok(())
        }
        Err(e) => {
            let msg = format!("{} didn't start: {e}\nTried: {}", launch.name, launch.command);
            app.agent.status = Status::Failed(msg.clone());
            app.agent.start_error = Some(msg.clone());
            Err(msg)
        }
    }
}

/// Sends a prompt (`agent.prompt`): the user's text with the workbook context, as one turn whose
/// changes form one undo step.
pub(crate) fn send(app: &mut SheetApp, text: &str) -> Result<Json, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Type a message for the agent first.".into());
    }
    if app.agent.turn.is_some() {
        return Err("The agent is still answering. Stop it first, or wait for it to finish.".into());
    }
    app.agent.open = true;
    let (context, range) = context(app, !app.agent.drop_selection);
    app.agent.items.push(Item::User { text: text.to_string(), range: range.clone() });
    if let Err(e) = ensure_started(app) {
        app.agent.items.push(Item::Note { text: e.clone(), error: true });
        return Err(e);
    }
    let full = format!("{context}\n\n{text}");
    let doc = app.session.active().map(|d| d.uid).unwrap_or_default();
    let label = format!("Changes by {}", agent_name(app));
    // An undo group already open on the workbook (a running macro debugger) can't take another
    // action's changes: the turn still runs, its changes are then that action's.
    let group = if app.session.active().is_some_and(|d| app.session.in_group(d.uid)) { None } else { app.session.group_begin(&label).ok() };
    app.agent.turn = Some(Turn { group, doc, changed: Vec::new(), cells: 0, target: None, stopping: false });
    app.agent.status = Status::Responding;
    let sent = app.agent.conn.as_mut().map(|c| c.prompt(&full)).unwrap_or_else(|| Err("the agent is not running".into()));
    if let Err(e) = sent {
        app.agent.items.push(Item::Note { text: format!("Couldn't send: {e}"), error: true });
        end_turn(app);
        app.agent.status = Status::Failed(e.clone());
        return Err(e);
    }
    app.agent.draft.clear();
    Ok(json!({"sent": true, "range": range}))
}

/// Stops the running turn (`agent.stop`). Cells written so far stay; Undo removes the whole turn.
pub(crate) fn stop(app: &mut SheetApp) -> Result<Json, String> {
    let Some(turn) = app.agent.turn.as_mut() else { return Ok(json!({"stopped": false})) };
    turn.stopping = true;
    // Unanswered permission requests are declined with the turn.
    for item in &mut app.agent.items {
        if let Item::Permission { answer, .. } = item
            && answer.is_none()
        {
            *answer = Some("Declined".into());
        }
    }
    if let Some(c) = app.agent.conn.as_mut() {
        c.cancel();
    }
    Ok(json!({"stopped": true}))
}

/// Starts a new conversation (`agent.newChat`): the agent process restarts with a fresh session.
pub(crate) fn new_chat(app: &mut SheetApp) -> Result<Json, String> {
    if app.agent.turn.is_some() {
        let _ = stop(app);
        end_turn(app);
    }
    app.agent.conn = None;
    app.agent.control_rx = None;
    app.agent.items.clear();
    app.agent.status = Status::Idle;
    app.agent.drop_selection = false;
    app.agent.focus_composer = true;
    Ok(json!({}))
}

/// Answers the latest unanswered permission request (`agent.permission {allow: bool}`).
pub(crate) fn answer_permission(app: &mut SheetApp, allow: bool) -> Result<Json, String> {
    let Some(item) = app.agent.items.iter_mut().rev().find(|i| matches!(i, Item::Permission { answer: None, .. })) else {
        return Err("The agent isn't asking for anything.".into());
    };
    let Item::Permission { request, options, answer, .. } = item else { return Err("no request".into()) };
    let want = if allow { ["allow_once", "allow_always"] } else { ["reject_once", "reject_always"] };
    let option = want.iter().find_map(|k| options.iter().find(|o| o.kind == *k)).map(|o| o.id.clone());
    *answer = Some(if allow { "Allowed".into() } else { "Declined".into() });
    let request = request.clone();
    if let Some(c) = app.agent.conn.as_mut() {
        c.answer_permission(&request, option.as_deref());
    }
    Ok(json!({"allowed": allow, "option": option}))
}

/// Keeps or undoes the latest pending changes card (`agent.keep`, `agent.undo`).
pub(crate) fn settle_changes(app: &mut SheetApp, keep: bool) -> Result<Json, String> {
    let Some(at) = app.agent.items.iter().rposition(|i| matches!(i, Item::Changes(c) if c.state == ChangeState::Pending)) else {
        return Err("There are no changes from the agent to keep or undo.".into());
    };
    let Some(Item::Changes(ch)) = app.agent.items.get(at).cloned() else { return Err("no changes".into()) };
    if keep {
        set_change_state(app, at, ChangeState::Kept);
        return Ok(json!({"kept": true}));
    }
    // Only while the turn's step is still the latest on its workbook: undoing more would take
    // later edits with it.
    let latest = app.session.documents().iter().find(|d| d.uid == ch.doc).map(|d| d.undo.len());
    match latest {
        Some(len) if len == ch.undo_len => {}
        Some(_) => return Err("Other changes were made after the agent's. Use Undo in the title bar to step back through them.".into()),
        None => return Err("That workbook is closed.".into()),
    }
    if app.session.active().map(|d| d.uid) != Some(ch.doc) {
        return Err("Switch back to the workbook the agent changed first.".into());
    }
    app.run("edit.undo", json!({}))?;
    set_change_state(app, at, ChangeState::Undone);
    Ok(json!({"undone": true}))
}

fn set_change_state(app: &mut SheetApp, at: usize, state: ChangeState) {
    if let Some(Item::Changes(c)) = app.agent.items.get_mut(at) {
        c.state = state;
    }
}

/// Ends the running turn: its undo group becomes one step, and a card lists what changed.
fn end_turn(app: &mut SheetApp) {
    let Some(mut turn) = app.agent.turn.take() else { return };
    let changed = match turn.group.take() {
        Some(token) => app.session.group_end(token).unwrap_or(false),
        None => !turn.changed.is_empty(),
    };
    app.after_engine();
    if changed && !turn.changed.is_empty() {
        let undo_len = app.session.documents().iter().find(|d| d.uid == turn.doc).map(|d| d.undo.len()).unwrap_or_default();
        let (ranges, cells) = coalesce(turn.changed);
        app.agent.items.push(Item::Changes(Changes { doc: turn.doc, ranges, cells, undo_len, state: ChangeState::Pending }));
    }
    for item in &mut app.agent.items {
        if let Item::Activity { state, .. } = item
            && *state == Step::Running
        {
            *state = Step::Done;
        }
    }
}

/// Splits "Sheet!A1:B2" into the sheet and the range (`None` for a whole sheet).
fn split_qualified(q: &str) -> (String, Option<RangeRef>) {
    match q.rsplit_once('!') {
        Some((s, b)) => (gridcraft_engine::core::unquote_sheet(s), RangeRef::parse(b)),
        None => (gridcraft_engine::core::unquote_sheet(q), None),
    }
}

/// Whether `a` and `b` together cover exactly their bounding rectangle (one holds the other, or
/// they sit side by side along a whole edge), so they read as one range.
fn joins(a: &RangeRef, b: &RangeRef) -> bool {
    if a.contains_range(b) || b.contains_range(a) {
        return true;
    }
    let same_cols = a.start.col == b.start.col && a.end.col == b.end.col;
    let same_rows = a.start.row == b.start.row && a.end.row == b.end.row;
    let rows_touch = a.end.row.saturating_add(1) >= b.start.row && b.end.row.saturating_add(1) >= a.start.row;
    let cols_touch = a.end.col.saturating_add(1) >= b.start.col && b.end.col.saturating_add(1) >= a.start.col;
    (same_cols && rows_touch) || (same_rows && cols_touch)
}

/// The changed ranges as the card lists them: writes of the same kind to neighbouring ranges
/// merge ("E20", "E21:E23" → "E20:E23"), and a range written again keeps one row. Also returns
/// how many distinct cells changed.
pub(crate) fn coalesce(list: Vec<(String, String)>) -> (Vec<(String, String)>, u64) {
    let mut out: Vec<(String, Option<RangeRef>, String)> = Vec::new();
    for (q, what) in list {
        let (sheet, range) = split_qualified(&q);
        let hit = out.iter_mut().find(|(s, r, w)| {
            *s == sheet
                && match (r, &range) {
                    (Some(a), Some(b)) => (*w == what && joins(a, b)) || a == b,
                    (None, None) => true,
                    _ => false,
                }
        });
        match hit {
            Some((_, r, w)) => {
                if let (Some(a), Some(b)) = (r.as_mut(), range) {
                    // The same range again: one row naming each thing done there.
                    if *a == b && !w.split(", ").any(|x| x == what) {
                        *w = format!("{w}, {what}");
                    }
                    *a = a.union(&b);
                }
            }
            None => out.push((sheet, range, what)),
        }
    }
    // Distinct cells, counted exactly while that stays cheap.
    let total: u64 = out.iter().filter_map(|(_, r, _)| r.map(|r| r.count())).fold(0, u64::saturating_add);
    let cells = if total <= 200_000 {
        let mut seen = std::collections::HashSet::new();
        for (s, r, _) in &out {
            if let Some(r) = r {
                for c in r.iter() {
                    seen.insert((s.clone(), c.row, c.col));
                }
            }
        }
        seen.len() as u64
    } else {
        total
    };
    let rows = out.into_iter().map(|(s, r, w)| (r.map(|r| qualified(&s, &r)).unwrap_or_else(|| sheet_ref(&s)), w)).collect();
    (rows, cells)
}

/// Applies the agent's events.
fn poll_events(app: &mut SheetApp) {
    let Some(conn) = app.agent.conn.as_mut() else { return };
    let events = conn.poll();
    for e in events {
        apply_event(app, e);
    }
}

pub(crate) fn apply_event(app: &mut SheetApp, e: AgentEvent) {
    let a = &mut app.agent;
    match e {
        AgentEvent::Ready { model } => {
            a.model = model.or(a.model.take());
            if a.status != Status::Responding {
                a.status = Status::Ready;
            }
        }
        AgentEvent::Message(text) => match a.items.last_mut() {
            Some(Item::Reply(r)) => r.push_str(&text),
            _ => a.items.push(Item::Reply(text)),
        },
        AgentEvent::Thought(_) => {}
        AgentEvent::ToolCall { id, title, kind, status } => {
            // GridCraft's own tools show as the reads and writes they made (see `poll_control`).
            if is_gridcraft_tool(&title) {
                return;
            }
            let state = step_of(&status);
            if a.items.len() < MAX_ITEMS {
                a.items.push(Item::Activity { key: id, verb: verb_for_kind(&kind).into(), target: title, detail: String::new(), state });
            }
        }
        AgentEvent::ToolCallUpdate { id, title, status } => {
            // It turned out to be one of GridCraft's own tools: its reads and writes show instead.
            if title.as_deref().is_some_and(is_gridcraft_tool) {
                a.items.retain(|i| !matches!(i, Item::Activity { key, .. } if *key == id));
                return;
            }
            if let Some(Item::Activity { target, state, .. }) =
                a.items.iter_mut().rev().find(|i| matches!(i, Item::Activity { key, .. } if *key == id))
            {
                if let Some(t) = title.filter(|t| !t.is_empty()) {
                    *target = t;
                }
                if let Some(s) = status {
                    *state = step_of(&s);
                }
            }
        }
        AgentEvent::Plan(steps) => match a.items.iter_mut().rev().find(|i| matches!(i, Item::Plan(_))) {
            // One plan per turn, updated in place.
            Some(Item::Plan(p)) if a.turn.is_some() => *p = steps,
            _ => a.items.push(Item::Plan(steps)),
        },
        AgentEvent::Permission { request, title, options } => {
            a.items.push(Item::Permission { request, title, options, answer: None });
            a.open = true;
        }
        AgentEvent::TurnEnded { stop_reason } => {
            let stopped = a.turn.as_ref().is_some_and(|t| t.stopping) || stop_reason == "cancelled";
            if stopped {
                a.items.push(Item::Note { text: "Stopped. Cells written so far stay; Undo removes the whole change.".into(), error: false });
            } else if stop_reason == "max_tokens" || stop_reason == "max_turn_requests" {
                a.items.push(Item::Note { text: "The agent stopped: it reached its length limit.".into(), error: false });
            } else if stop_reason == "refusal" {
                a.items.push(Item::Note { text: "The agent declined to continue.".into(), error: false });
            }
            if a.status == Status::Responding {
                a.status = Status::Ready;
            }
            end_turn(app);
        }
        AgentEvent::Failed(msg) => {
            a.items.push(Item::Note { text: msg.clone(), error: true });
            if a.turn.is_none() {
                a.status = Status::Failed(msg);
            }
        }
        AgentEvent::Exited(msg) => {
            a.items.push(Item::Note { text: msg.clone(), error: true });
            a.status = Status::Failed(msg);
            a.conn = None;
            a.control_rx = None;
            end_turn(app);
        }
    }
}

fn step_of(status: &str) -> Step {
    match status {
        "completed" => Step::Done,
        "failed" => Step::Failed,
        _ => Step::Running,
    }
}

/// GridCraft's MCP tools, as agents title their calls ("gridcraft_read_range", "read_range").
fn is_gridcraft_tool(title: &str) -> bool {
    let t = title.to_ascii_lowercase();
    t.contains("gridcraft")
        || [
            "read_range",
            "write_range",
            "set_cell",
            "get_cell",
            "execute_command",
            "inspect_workbook",
            "list_commands",
            "format_range",
            "insert_chart",
            "create_table",
            "sort_range",
            "evaluate_formula",
        ]
        .iter()
        .any(|n| t == *n || t.ends_with(&format!("_{n}")))
}

fn verb_for_kind(kind: &str) -> &'static str {
    match kind {
        "read" => "Read",
        "edit" => "Edited",
        "delete" => "Deleted",
        "move" => "Moved",
        "search" => "Searched",
        "execute" => "Ran",
        "think" => "Thought",
        "fetch" => "Fetched",
        _ => "",
    }
}

/// Runs the agent's control requests (its MCP tools acting on this window), noting what they read
/// and wrote for the conversation, the grid outline and the changes card.
fn poll_control(app: &mut SheetApp, ctx: &egui::Context) {
    let Some(rx) = app.agent.control_rx.take() else { return };
    while let Ok(req) = rx.try_recv() {
        if !req.take() {
            continue;
        }
        let before = app.session.active().map(|d| (d.uid, d.wb.clone()));
        let reply = crate::control::handle_now(app, ctx, &req.method, &req.params);
        let changed = match (&before, app.session.active()) {
            (Some((uid, wb)), Some(d)) => d.uid != *uid || !std::sync::Arc::ptr_eq(wb, &d.wb),
            _ => false,
        };
        note_request(app, &req.method, &req.params, &reply, changed);
        let _ = req.reply.send(reply);
    }
    app.agent.control_rx = Some(rx);
}

/// Records one of the agent's requests in the conversation.
pub(crate) fn note_request(app: &mut SheetApp, method: &str, params: &Json, reply: &Json, changed: bool) {
    let ok = reply.get("ok").and_then(Json::as_bool).unwrap_or(false);
    let result = reply.get("result").cloned().unwrap_or(Json::Null);
    let (command, inner) = if method == "engine.execute" {
        (params.get("command").and_then(Json::as_str).unwrap_or("").to_string(), params.get("params").cloned().unwrap_or(Json::Null))
    } else {
        (method.to_string(), params.clone())
    };
    let active_sheet = app.session.active().and_then(|d| d.wb.active().map(|s| s.name.clone())).unwrap_or_default();
    let (sheet, range) = target_of(&inner, &active_sheet);
    let place = |range: &Option<RangeRef>| range.map(|r| qualified(&sheet, &r)).unwrap_or_default();
    let (verb, target, detail) = match command.as_str() {
        "sheet.read" => {
            let shown = result
                .get("range")
                .and_then(Json::as_str)
                .map(|r| qualified(&sheet, &RangeRef::parse(r).unwrap_or(RangeRef::cell(CellRef::new(0, 0)))));
            let rows = result.get("values").and_then(Json::as_array).map(Vec::len).unwrap_or(0);
            ("Read".to_string(), shown.unwrap_or_else(|| place(&range)), if rows > 1 { format!("{rows} rows") } else { String::new() })
        }
        "cell.get" => ("Read".to_string(), place(&range), String::new()),
        "document.inspect" => ("Looked over".to_string(), "the workbook".to_string(), String::new()),
        "engine.commands" => ("Looked up".to_string(), "commands".to_string(), String::new()),
        "formulas.functions" => ("Looked up".to_string(), "functions".to_string(), String::new()),
        "formulas.evaluate" => ("Evaluated".to_string(), inner.get("formula").and_then(Json::as_str).unwrap_or("").to_string(), String::new()),
        "ui.screenshot" => ("Looked at".to_string(), "the window".to_string(), String::new()),
        "selection.set" => ("Selected".to_string(), place(&range), String::new()),
        "window.list" => return,
        _ if method == "engine.execute" => {
            let label = gridcraft_engine::find_command(&command).map(|c| c.label.trim_end_matches('…').to_string()).unwrap_or(command.clone());
            let verb = match command.as_str() {
                "cell.set" | "range.setValues" => "Wrote".to_string(),
                _ => label.clone(),
            };
            (verb, place(&range), String::new())
        }
        _ => return,
    };
    let state = if ok { Step::Done } else { Step::Failed };
    let detail = if ok { detail } else { reply.get("error").and_then(Json::as_str).map(|e| first_line(e, 80)).unwrap_or_else(|| "failed".into()) };
    let in_card = ok && changed && app.agent.turn.is_some();
    if !in_card && app.agent.items.len() < MAX_ITEMS {
        let key = format!("ctl{}", app.agent.items.len());
        app.agent.items.push(Item::Activity { key, verb: verb.clone(), target: target.clone(), detail, state });
    }
    if ok
        && changed
        && let Some(turn) = app.agent.turn.as_mut()
    {
        let r = range.unwrap_or(RangeRef::cell(CellRef::new(0, 0)));
        let formula = inner.get("input").and_then(Json::as_str).is_some_and(|s| s.trim_start().starts_with('='));
        let what = match verb.as_str() {
            "Wrote" if formula => "Formulas".to_string(),
            "Wrote" => "Values".to_string(),
            _ => verb,
        };
        if range.is_some() {
            turn.target = Some((sheet.clone(), r));
            turn.cells = turn.cells.saturating_add(r.count());
            turn.changed.push((qualified(&sheet, &r), what));
        } else {
            turn.changed.push((sheet_ref(&sheet), what));
        }
    }
}

fn first_line(s: &str, max: usize) -> String {
    let l = s.lines().next().unwrap_or("");
    if l.chars().count() > max { format!("{}…", l.chars().take(max).collect::<String>()) } else { l.to_string() }
}

/// The sheet and range a command's params name: `range`, `cell`, `target` or `over`, with or
/// without a sheet prefix, or the `sheet` param.
fn target_of(p: &Json, active_sheet: &str) -> (String, Option<RangeRef>) {
    let mut sheet = p.get("sheet").and_then(Json::as_str).unwrap_or(active_sheet).to_string();
    for k in ["range", "cell", "target", "over", "at"] {
        let Some(s) = p.get(k).and_then(Json::as_str) else { continue };
        let (sh, body) = match s.rsplit_once('!') {
            Some((a, b)) => (Some(gridcraft_engine::core::unquote_sheet(a)), b),
            None => (None, s),
        };
        if let Some(r) = RangeRef::parse(&body.replace('$', "")) {
            if let Some(sh) = sh {
                sheet = sh;
            }
            return (sheet, Some(r));
        }
    }
    (sheet, None)
}

fn sheet_ref(sheet: &str) -> String {
    gridcraft_engine::formula::quote_sheet(sheet)
}

/// "Data!A1:H200".
pub(crate) fn qualified(sheet: &str, r: &RangeRef) -> String {
    if sheet.is_empty() { r.a1() } else { format!("{}!{}", sheet_ref(sheet), r.a1()) }
}

/// The context block sent ahead of each prompt (workbook, sheet, selection and a bounded
/// preview of its values), and the selection chip's text.
pub(crate) fn context(app: &SheetApp, with_selection: bool) -> (String, Option<String>) {
    let Some(d) = app.session.active() else { return ("[GridCraft] No workbook is open.".into(), None) };
    let Some(sh) = d.wb.active() else { return (format!("[GridCraft] Workbook: {}", d.display_title()), None) };
    let sel = d.selection.current();
    let chip = qualified(&sh.name, &sel);
    let sheets: Vec<String> = d.wb.sheets.iter().map(|s| s.name.clone()).collect();
    let mut out = String::new();
    out.push_str("[GridCraft context]\n");
    out.push_str(&format!("Workbook: {}\n", d.display_title()));
    out.push_str(&format!("Sheets: {}\n", sheets.join(", ")));
    out.push_str(&format!("Active sheet: {}\n", sh.name));
    if let Some(u) = sh.used_range() {
        out.push_str(&format!("Used range: {}\n", u.a1()));
    }
    if with_selection {
        out.push_str(&format!("Selection: {chip}\n"));
        let rows = sel.height().min(PREVIEW_ROWS);
        let cols = sel.width().min(PREVIEW_COLS);
        let mut preview = String::new();
        for r in 0..rows {
            let row: Vec<String> = (0..cols)
                .map(|c| {
                    let cell = CellRef::new(sel.start.row.saturating_add(r), sel.start.col.saturating_add(c));
                    let text = gridcraft_engine::display::cell_text(&d.wb, sh, cell);
                    let text: String = text.replace(['\t', '\n'], " ");
                    if text.chars().count() > PREVIEW_VALUE { format!("{}…", text.chars().take(PREVIEW_VALUE).collect::<String>()) } else { text }
                })
                .collect();
            preview.push_str(&row.join("\t"));
            preview.push('\n');
        }
        if !preview.trim().is_empty() {
            let more = if sel.height() > rows || sel.width() > cols {
                format!(" (first {rows}×{cols} of {}×{})", sel.height(), sel.width())
            } else {
                String::new()
            };
            out.push_str(&format!("Selected values{more}, tab-separated:\n{preview}"));
        }
    }
    out.push_str(
        "Use the gridcraft MCP tools to read and change this workbook in the open GridCraft window (read_range, write_range, set_cell, \
         execute_command…). Changes show live and the user can undo them.",
    );
    if out.len() > CONTEXT_MAX {
        let mut cut = CONTEXT_MAX;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push('…');
    }
    (out, with_selection.then_some(chip))
}

/// Suggestions for the empty sidebar, drawn from the workbook: (icon, text).
pub(crate) fn suggestions(session: &Session) -> Vec<(view::SuggestIcon, String)> {
    use view::SuggestIcon as I;
    let mut out = Vec::new();
    let Some(d) = session.active() else { return out };
    let Some(sh) = d.wb.active() else { return out };
    let name = sh.name.clone();
    let sel = d.selection.current();
    let used = sh.used_range();
    if used.is_none() {
        out.push((I::Trend, "Set up a budget tracker on this sheet".to_string()));
        out.push((I::Table, "Make a table of monthly expenses".to_string()));
        out.push((I::Chart, "Sketch a sales dashboard with sample data".to_string()));
        out.push((I::Clean, "Explain what GridCraft's agent can do".to_string()));
        return out;
    }
    out.push((I::Trend, format!("Explain what's on {name}")));
    if !sel.is_single() {
        out.push((I::Sum, format!("Summarize {}", sel.a1())));
    } else {
        out.push((I::Sum, "Add totals under the data".to_string()));
    }
    let other = d.wb.sheets.iter().map(|s| s.name.clone()).find(|n| *n != name);
    out.push((
        I::Clean,
        match other {
            Some(o) => format!("Clean up the {o} sheet"),
            None => "Find errors or odd values".to_string(),
        },
    ));
    // A text header over numbers names the chart.
    let header = used.and_then(|u| {
        (u.start.col..=u.end.col.min(u.start.col.saturating_add(20))).find_map(|c| {
            let h = sh.value(CellRef::new(u.start.row, c));
            let below = sh.value(CellRef::new(u.start.row.saturating_add(1), c));
            match (h, below) {
                (gridcraft_engine::core::Value::Text(t), gridcraft_engine::core::Value::Number(_)) if !t.trim().is_empty() => Some(t.to_string()),
                _ => None,
            }
        })
    });
    out.push((
        I::Chart,
        match header {
            Some(h) => format!("Chart {}", h.trim()),
            None => "Chart the selected data".to_string(),
        },
    ));
    out
}

/// The grid outline: where the agent is writing now, or what its latest turn changed.
pub(crate) fn overlay(app: &SheetApp) -> Option<(Vec<(String, RangeRef)>, String)> {
    let name = agent_name(app);
    if let Some(turn) = &app.agent.turn {
        let (sheet, r) = turn.target.clone()?;
        return Some((vec![(sheet, r)], format!("{name} · writing {}", r.a1())));
    }
    let ch = app.agent.items.iter().rev().find_map(|i| match i {
        Item::Changes(c) if c.state == ChangeState::Pending => Some(c),
        _ => None,
    })?;
    if app.session.active().map(|d| d.uid) != Some(ch.doc) {
        return None;
    }
    let list: Vec<(String, RangeRef)> = ch
        .ranges
        .iter()
        .filter_map(|(q, _)| {
            let (s, b) = q.rsplit_once('!')?;
            Some((gridcraft_engine::core::unquote_sheet(s), RangeRef::parse(b)?))
        })
        .collect();
    (!list.is_empty()).then(|| (list, format!("{name} · changed")))
}

/// Paints [`overlay`] on the sheet `sh` (called from the grid's overlay pass).
pub(crate) fn paint_overlay(app: &SheetApp, p: &egui::Painter, geo: &crate::grid::Geo, sh: &gridcraft_engine::model::Sheet) {
    let Some((ranges, label)) = overlay(app) else { return };
    let t = crate::theme::Tokens::get(p.ctx());
    // A dark sheet turns the colours painted on it over: paint what comes out as the dark agent
    // colour, with white (which becomes the dark paper) for the tag's text.
    let dark_sheet = t.dark && app.ui.dark_sheet;
    let (c, ink) = if dark_sheet { (t.sheet_agent_dark, t.grid_bg) } else { (t.sheet_agent, t.on_agent) };
    let mut first: Option<egui::Rect> = None;
    for (sheet, r) in ranges.iter().filter(|(n, _)| n.eq_ignore_ascii_case(&sh.name)) {
        let rect = geo.range_rect(sh, *r);
        if !p.clip_rect().intersects(rect) {
            continue;
        }
        let _ = sheet;
        p.rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 22));
        view::dashed_rect(p, rect.shrink(0.5), egui::Stroke::new(1.5, c));
        if first.is_none() {
            first = Some(rect);
        }
    }
    if let Some(rect) = first {
        let font = crate::theme::ui_bold(11.0);
        let galley = p.layout_no_wrap(label, font, ink);
        let size = galley.size() + egui::vec2(12.0, 4.0);
        let tag = egui::Rect::from_min_size(rect.left_top() - egui::vec2(0.0, size.y + 1.0), size);
        // Inside the top edge when there is no room above.
        let tag = if tag.top() < p.clip_rect().top() { tag.translate(egui::vec2(0.0, size.y + 2.0)) } else { tag };
        p.rect_filled(tag, 4.0, c);
        p.galley(tag.min + egui::vec2(6.0, 2.0), galley, ink);
    }
}

/// The sidebar's command descriptions for `engine.commands`.
pub(crate) const COMMANDS: [(&str, &str, &str); 7] = [
    ("view.agent", "Agent", "{on?: bool} (toggles without it; Ctrl+J) → {on}: the agent sidebar"),
    ("agent.prompt", "Ask Agent", "{text, selection?: bool (default true: send the selection as context)} → {sent, range}"),
    ("agent.stop", "Stop Agent", "{} → {stopped}: stops the running turn; cells written so far stay"),
    ("agent.keep", "Keep Agent Changes", "{} → {kept}: keeps the latest changes card"),
    ("agent.undo", "Undo Agent Changes", "{} → {undone}: undoes the agent's latest turn as one step"),
    ("agent.permission", "Answer Agent Permission", "{allow: bool} → {allowed, option}"),
    ("agent.newChat", "New Agent Chat", "{} → {}: starts a fresh conversation"),
];

/// UI commands of the sidebar; `None` when `id` isn't one.
pub(crate) fn run_command(app: &mut SheetApp, id: &str, p: &Json) -> Option<Result<Json, String>> {
    let r = match id {
        "view.agent" => toggle(app, p.get("on").and_then(Json::as_bool)),
        "agent.prompt" => {
            if let Some(sel) = p.get("selection").and_then(Json::as_bool) {
                app.agent.drop_selection = !sel;
            }
            let text = p.get("text").and_then(Json::as_str).unwrap_or("").to_string();
            send(app, &text)
        }
        "agent.stop" => stop(app),
        "agent.keep" => settle_changes(app, true),
        "agent.undo" => settle_changes(app, false),
        "agent.permission" => answer_permission(app, p.get("allow").and_then(Json::as_bool).unwrap_or(false)),
        "agent.newChat" => new_chat(app),
        _ => return None,
    };
    Some(r)
}

/// A summary for `ui.inspect` and tests.
pub fn inspect(app: &SheetApp) -> Json {
    let a = &app.agent;
    json!({
        "available": available(app),
        "open": a.open,
        "status": format!("{:?}", a.status),
        "responding": a.responding(),
        "items": a.items.iter().map(|i| match i {
            Item::User { text, range } => json!({"user": text, "range": range}),
            Item::Activity { verb, target, detail, state, .. } => json!({"activity": format!("{verb} {target} {detail}").trim().to_string(), "state": format!("{state:?}")}),
            Item::Reply(t) => json!({"reply": t}),
            Item::Plan(p) => json!({"plan": p.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>()}),
            Item::Permission { title, answer, .. } => json!({"permission": title, "answer": answer}),
            Item::Changes(c) => json!({"changes": c.ranges.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>(), "cells": c.cells, "state": format!("{:?}", c.state)}),
            Item::Note { text, error } => json!({"note": text, "error": error}),
        }).collect::<Vec<_>>(),
    })
}
