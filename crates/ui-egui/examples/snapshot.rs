//! Renders the whole GridCraft window offscreen (wgpu, no window) to a PNG.
//!
//! `cargo run --release -p gridcraft-ui-egui --example snapshot -- [--sample sales] [--in file.xlsx]
//!  [--size 1440x900] [--scale 2] [--cmd 'id={json}']... [--tab Insert] [--dark] [--theme darkGreen|omarchy] [--backstage] [--language en|es|zh|ja|ko|ru|pt|de|cs|fr|tr]
//!  [--dialog 'name={json}'] [--hf-edit header|footer]
//!  [--quick-analysis formatting|charts|totals] [--agent collapsed|empty|conversation|working] out.png`
//!
//! `--agent` shows the agent sidebar in one of its states, driven by a scripted fake agent (no
//! process, no network): its reads and writes go through the sidebar's control channel as a real
//! agent's MCP tools would.

use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut sample = None;
    let mut input = None;
    let mut size = (1440.0f32, 900.0f32);
    let mut scale = 2.0f32;
    let mut cmds: Vec<String> = vec![];
    let mut tab = None;
    let mut dark = false;
    let mut palette = String::new();
    let mut backstage = false;
    let mut thumbnail = false;
    let mut pane: Option<String> = None;
    let mut chart: Option<u32> = None;
    let mut dialog: Option<String> = None;
    let mut quick: Option<String> = None;
    let mut language: Option<String> = None;
    let mut hf_edit: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut out = "snapshot.png".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--sample" => {
                i += 1;
                sample = args.get(i).cloned();
            }
            "--in" => {
                i += 1;
                input = args.get(i).cloned();
            }
            "--size" => {
                i += 1;
                if let Some((w, h)) = args.get(i).and_then(|s| s.split_once('x')) {
                    size = (w.parse().unwrap_or(1440.0), h.parse().unwrap_or(900.0));
                }
            }
            "--scale" => {
                i += 1;
                scale = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(2.0);
            }
            "--cmd" => {
                i += 1;
                if let Some(c) = args.get(i) {
                    cmds.push(c.clone());
                }
            }
            "--tab" => {
                i += 1;
                tab = args.get(i).cloned();
            }
            "--dark" => dark = true,
            "--backstage" => backstage = true,
            "--thumbnail" => thumbnail = true,
            "--theme" => {
                i += 1;
                palette = args.get(i).cloned().unwrap_or_default();
            }
            "--pane" => {
                i += 1;
                pane = args.get(i).cloned();
            }
            "--chart" => {
                i += 1;
                chart = args.get(i).and_then(|s| s.parse().ok());
            }
            "--dialog" => {
                i += 1;
                dialog = args.get(i).cloned();
            }
            "--hf-edit" => {
                i += 1;
                hf_edit = args.get(i).cloned();
            }
            "--quick-analysis" => {
                i += 1;
                quick = args.get(i).cloned();
            }
            "--agent" => {
                i += 1;
                agent = args.get(i).cloned();
            }
            "--language" => {
                i += 1;
                language = args.get(i).cloned();
            }
            other => out = other.to_string(),
        }
        i += 1;
    }
    // Catalog the installed fonts first (the app scans in the background from its first frame), so
    // the workbook's fonts are drawn in their own faces, not a substitute.
    let _ = gridcraft_ui_egui::fonts::catalog();
    // --thumbnail: the workbook's Backstage thumbnail, drawn by the app's own renderer.
    if thumbnail {
        let wb = match (&input, &sample) {
            (Some(p), _) => gridcraft_engine::io::open_bytes(p, &std::fs::read(p).expect("read")).expect("open").0,
            (None, Some(s)) => gridcraft_engine::sample::build(s).expect("sample"),
            _ => gridcraft_engine::model::Workbook::default(),
        };
        let img = gridcraft_ui_egui::thumbnail::render(wb, 0, dark).expect("thumbnail");
        let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
        image::RgbaImage::from_raw(img.size[0] as u32, img.size[1] as u32, rgba).expect("image").save(&out).expect("save");
        println!("wrote {out} ({}x{})", img.size[0], img.size[1]);
        return;
    }
    let mut session = Session::new();
    if let Some(p) = &input {
        session.execute("file.open", json!({"path": p})).expect("open");
    } else if let Some(s) = &sample {
        session.execute("file.new", json!({"sample": s})).expect("sample");
    } else {
        session.new_workbook();
    }
    for c in &cmds {
        let (id, p) = c.split_once('=').map(|(a, b)| (a.to_string(), serde_json::from_str(b).unwrap_or(json!({})))).unwrap_or((c.clone(), json!({})));
        if let Err(e) = session.execute(&id, p) {
            eprintln!("{id}: {e}");
        }
    }
    let script = fake::Script::default();
    let (control_tx, rx) = std::sync::mpsc::channel();
    let control_rx = std::rc::Rc::new(std::cell::RefCell::new(Some(rx)));
    let agent_state = agent.clone();
    let mut harness =
        egui_kittest::Harness::builder().with_size(egui::vec2(size.0, size.1)).with_pixels_per_point(scale).with_max_steps(12).wgpu().build_ui_state(
            move |ui, app: &mut SheetApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            {
                let mut app = SheetApp::new(session, Default::default());
                if agent.is_some() {
                    app.services.agent = Some(fake::launch(script.clone(), control_rx.clone()));
                }
                app.ui.dark = dark;
                app.ui.palette = palette.clone();
                if backstage {
                    app.open_backstage(gridcraft_ui_egui::backstage::Page::Home);
                }
                if let Some(code) = &language
                    && let Some(l) = gridcraft_ui_egui::i18n::Language::parse(code)
                {
                    app.ui.language = l;
                }
                if let Some(t) = tab {
                    app.ui.ribbon_tab = t;
                }
                app.grid.pane = pane;
                app.selected_chart = chart;
                if let Some(d) = &dialog {
                    let (name, p) =
                        d.split_once('=').map(|(a, b)| (a, serde_json::from_str(b).unwrap_or(json!({})))).unwrap_or((d.as_str(), json!({})));
                    app.open_dialog(name, p);
                }
                // Page Layout view: edit the first page's centre header (or footer) section.
                let active = app.session.active().and_then(|d| d.wb.active().cloned().map(|s| (d.wb.active_sheet(), s)));
                if let (Some(part), Some((si, sh))) = (&hf_edit, active) {
                    let zone = gridcraft_engine::paper::HfZone { footer: part == "footer", section: 1 };
                    gridcraft_ui_egui::paper_view::begin(&mut app, si, &sh, (0, 0), zone);
                }
                app
            },
        );
    // The workbook's installed fonts bind once the font scan is done; frames past the first few
    // apply them, so the snapshot shows them as the app does.
    harness.run_steps(14);
    // The File view's thumbnails are drawn on a worker thread: give them time to arrive.
    if backstage {
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(150));
            harness.run_steps(2);
        }
    }
    if let Some(state) = &agent_state {
        fake::play(&mut harness, state, &script, &control_tx);
    }
    // --quick-analysis formatting|charts|totals: the Quick Analysis card open on that tab.
    if let Some(tab) = &quick {
        gridcraft_ui_egui::quick_analysis::open(&harness.ctx, tab);
        harness.run_steps(8);
    }
    let img = harness.render().expect("render");
    img.save(&out).expect("save png");
    eprintln!("wrote {out} ({}x{})", img.width(), img.height());
}

/// A scripted agent for `--agent`: events go to the sidebar through a shared queue, and its tool
/// calls through the sidebar's control channel.
mod fake {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::sync::mpsc::{Receiver, Sender};

    use gridcraft_ui_egui::agent::{AgentConnection, AgentEvent, AgentLaunch, AgentStarted};
    use gridcraft_ui_egui::{ControlRequest, SheetApp};
    use serde_json::{Value, json};

    #[derive(Clone, Default)]
    pub struct Script(Rc<RefCell<VecDeque<AgentEvent>>>);

    struct Conn(Script);

    impl AgentConnection for Conn {
        fn prompt(&mut self, _text: &str) -> Result<(), String> {
            Ok(())
        }
        fn cancel(&mut self) {}
        fn answer_permission(&mut self, _request: &Value, _option: Option<&str>) {}
        fn poll(&mut self) -> Vec<AgentEvent> {
            self.0.0.borrow_mut().drain(..).collect()
        }
    }

    pub fn launch(script: Script, control: Rc<RefCell<Option<Receiver<ControlRequest>>>>) -> AgentLaunch {
        AgentLaunch {
            id: "opencode".into(),
            name: "OpenCode".into(),
            detail: "Omarchy default agent".into(),
            command: "opencode acp".into(),
            change_hint: Some("omarchy default agent".into()),
            start: Box::new(move |_| Ok(AgentStarted { connection: Box::new(Conn(script.clone())), control: control.borrow_mut().take() })),
        }
    }

    type H<'a> = egui_kittest::Harness<'a, SheetApp>;

    fn emit(h: &mut H, script: &Script, e: AgentEvent) {
        script.0.borrow_mut().push_back(e);
        h.run_steps(2);
    }

    fn tool(h: &mut H, tx: &Sender<ControlRequest>, method: &str, params: Value) {
        let (req, _reply) = ControlRequest::new(method, params);
        let _ = tx.send(req);
        h.run_steps(2);
    }

    fn set(h: &mut H, tx: &Sender<ControlRequest>, cell: &str, input: &str) {
        tool(h, tx, "engine.execute", json!({"command": "cell.set", "params": {"cell": cell, "input": input}}));
    }

    pub fn play(h: &mut H, state: &str, script: &Script, tx: &Sender<ControlRequest>) {
        if state == "collapsed" {
            return;
        }
        let _ = h.state_mut().run("selection.set", json!({"range": "B11:F17"}));
        let _ = h.state_mut().run("view.agent", json!({"on": true}));
        h.run_steps(3);
        if state == "empty" {
            return;
        }
        let ask = "Which coffee has the thinnest margin, and why? Then add margins to the fastest-growing list.";
        if let Err(e) = h.state_mut().run("agent.prompt", json!({"text": ask})) {
            eprintln!("agent.prompt: {e}");
        }
        emit(h, script, AgentEvent::Ready { model: Some("openrouter/particle-ai/particle-1".into()) });
        tool(h, tx, "sheet.read", json!({"range": "B10:F18"}));
        tool(h, tx, "sheet.read", json!({"sheet": "Data", "range": "A1:H200"}));
        emit(
            h,
            script,
            AgentEvent::Message(
                "**Sierra Decaf** has the thinnest margin at 52.4%, almost 6 points under the 58.1% average. Decaf adds a toll-processing \
                 cost per pound, and its price hasn't moved since October.\n\nI'll add each coffee's margin next to the fastest-growing list."
                    .into(),
            ),
        );
        emit(
            h,
            script,
            AgentEvent::Plan(vec![
                ("Add a Margin column in E20:E23".into(), "in_progress".into()),
                ("Look up each coffee's margin from the ranked table".into(), "pending".into()),
            ]),
        );
        set(h, tx, "E20", "Margin");
        set(h, tx, "E21", "=VLOOKUP(B21,$B$11:$F$17,5,FALSE)");
        set(h, tx, "E22", "=VLOOKUP(B22,$B$11:$F$17,5,FALSE)");
        if state == "working" {
            h.run_steps(4);
            return;
        }
        set(h, tx, "E23", "=VLOOKUP(B23,$B$11:$F$17,5,FALSE)");
        tool(h, tx, "engine.execute", json!({"command": "home.percent", "params": {"range": "E21:E23"}}));
        // A long list of changes on one range, so the Changes card's wrapping shows.
        for cmd in ["home.bold", "home.italic", "home.underline", "home.alignRight", "home.wrapText", "home.strikethrough"] {
            tool(h, tx, "engine.execute", json!({"command": cmd, "params": {"range": "E21:E23"}}));
        }
        emit(h, script, AgentEvent::Message("\n\nDone: E21:E23 look up each coffee's margin, so they follow the table.".into()));
        emit(h, script, AgentEvent::TurnEnded { stop_reason: "end_turn".into() });
        h.run_steps(4);
    }
}
