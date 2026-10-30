//! Drawing the agent sidebar: a 360 px card on the right, from under the ribbon to the bottom of
//! the window, with the conversation above a composer.

use egui::{Align, Color32, CornerRadius, FontFamily, FontId, Layout, Rect, Response, Sense, Stroke, Ui, UiBuilder, pos2, vec2};

use super::markdown;
use super::{ChangeState, Changes, Item, Status, Step, agent_name, available};
use crate::SheetApp;
use crate::theme::{self, Tokens};

/// The sidebar card's width, and the window gutter around it.
pub const SIDEBAR_W: f32 = 360.0;
const GUTTER: f32 = 8.0;
const PAD: f32 = 14.0;

// Line icons on a 24-unit grid (original drawings for the sidebar).
const PROMPT: [&str; 2] = ["m5 7 5 5-5 5", "M13 17h6"];
const NEW_CHAT: [&str; 2] = ["M12 4H6a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-6", "M17.5 3.5a2.1 2.1 0 0 1 3 3L13 14l-4 1 1-4Z"];
const CLOSE: [&str; 1] = ["M6 6l12 12M18 6 6 18"];
const EYE: [&str; 2] = ["M2.5 12s3.5-6.5 9.5-6.5 9.5 6.5 9.5 6.5-3.5 6.5-9.5 6.5S2.5 12 2.5 12Z", "M14.5 12a2.5 2.5 0 1 1-5 0 2.5 2.5 0 0 1 5 0Z"];
const PENCIL: [&str; 1] = ["M16.5 4.5a2.1 2.1 0 0 1 3 3L8 19l-4 1 1-4Z"];
const TOOL: [&str; 2] = ["M4 6h16", "M4 12h10M4 18h7"];
const CROSS: [&str; 1] = ["M7 7l10 10M17 7 7 17"];
const TABLE: [&str; 2] = ["M5.5 4h13a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2Z", "M3.5 10h17M3.5 15h17M9.5 4v16"];
const CHECK: [&str; 1] = ["m5 12.5 4.5 4.5L19 7.5"];
const SEND: [&str; 1] = ["M12 19V5M5.5 11.5 12 5l6.5 6.5"];
const CHANGES: [&str; 2] = ["M6 4h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2Z", "M12 9v6M9 12h6"];
const SHIELD: [&str; 2] = ["M12 21s7.5-3.5 7.5-9.5V5.5L12 3 4.5 5.5v6C4.5 17.5 12 21 12 21Z", "m9 12 2 2 4-4"];
const TREND: [&str; 2] = ["m3 17 6-6 4 4 8-8", "M15 7h6v6"];
const SIGMA: [&str; 1] = ["M18 7V4H6l6 8-6 8h12v-3"];
const ERASER: [&str; 2] = ["m7 20-3.6-3.6a2 2 0 0 1 0-2.8l9.2-9.2a2 2 0 0 1 2.8 0l5.2 5.2a2 2 0 0 1 0 2.8L13 20Z", "M21 20H7M6 11l7.5 7.5"];
const CHART: [&str; 2] = ["M4 4v16h16", "M9 16v-5M13.5 16V8M18 16v-3"];
const PANEL: [&str; 4] = [
    "M5.5 4h13a2.5 2.5 0 0 1 2.5 2.5v11a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 17.5v-11A2.5 2.5 0 0 1 5.5 4Z",
    "M15 4v16",
    "m7 10 2.2 2L7 14",
    "M11 14h1.5",
];

/// An agent's own mark (the upstream icon, white on transparent so it takes the text colour), or
/// `None` for agents without one. Decoded once per context.
fn brand_icon(ctx: &egui::Context, id: &str) -> Option<egui::TextureHandle> {
    let bytes: &[u8] = match id {
        "opencode" | "kilo" => include_bytes!("../../../../assets/agent-icons/opencode.png"),
        "claude" => include_bytes!("../../../../assets/agent-icons/claude.png"),
        "copilot" => include_bytes!("../../../../assets/agent-icons/githubcopilot.png"),
        "cursor-agent" => include_bytes!("../../../../assets/agent-icons/cursor.png"),
        "kimi" => include_bytes!("../../../../assets/agent-icons/kimi.png"),
        "qwen" => include_bytes!("../../../../assets/agent-icons/qwen.png"),
        "dsh" => include_bytes!("../../../../assets/agent-icons/deepseek.png"),
        "amp" => include_bytes!("../../../../assets/agent-icons/amp.png"),
        "hermes" => include_bytes!("../../../../assets/agent-icons/hermes.png"),
        "pi" | "omp" => include_bytes!("../../../../assets/agent-icons/pi.png"),
        _ => return None,
    };
    let key = egui::Id::unique("agent_brand_icon").with(id);
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(key)) {
        return Some(tex);
    }
    let rgba = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let tex = ctx.load_texture(format!("agent-{id}"), egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()), egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(key, tex.clone()));
    Some(tex)
}

/// The icon of a suggestion in the empty sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestIcon {
    Trend,
    Sum,
    Clean,
    Chart,
    Table,
}

fn icon(p: &egui::Painter, rect: Rect, paths: &[&str], c: Color32) {
    crate::icons::paint_svg(p, rect, 24.0, paths, 1.35, c);
}

fn ui_font(size: f32) -> FontId {
    theme::ui_font(size)
}

fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(theme::MONO.into()))
}

/// Text a step quieter than `text_dim` (hints, counts, the plan's numbers).
fn faint(t: &Tokens) -> Color32 {
    t.text_dim.lerp_to_gamma(t.text_disabled, 0.45)
}

/// Dashed outline of `rect`.
pub(crate) fn dashed_rect(p: &egui::Painter, r: Rect, stroke: Stroke) {
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in pts.windows(2) {
        if let [a, b] = w {
            p.extend(egui::Shape::dashed_line(&[*a, *b], stroke, 4.0, 3.0));
        }
    }
}

/// The title-bar toggle: "Agent" with the sidebar icon, filled while the sidebar is open.
pub fn title_toggle(app: &mut SheetApp, ui: &mut Ui, height: f32) {
    if !available(app) {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let on = app.agent.open;
    let font = theme::ui_bold(13.0);
    let label = ui.painter().layout_no_wrap("Agent".into(), font, t.text);
    let w = 8.0 + 16.0 + 6.0 + label.size().x + 10.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click());
    let c = if on { t.accent } else { t.text };
    if on {
        ui.painter().rect_filled(rect, 7.0, t.accent_soft);
    } else if let Some(fill) = crate::widgets::hover_fill(ui, resp.id, resp.hovered(), t.hover) {
        ui.painter().rect_filled(rect, 7.0, fill);
    }
    icon(ui.painter(), Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - 8.0), vec2(16.0, 16.0)), &PANEL, c);
    ui.painter().galley(pos2(rect.left() + 30.0, rect.center().y - label.size().y / 2.0), label, c);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, "Agent"));
    let tip = format!("{} (Ctrl+J)", if on { "Hide the agent" } else { "Ask the agent about this workbook" });
    if resp.on_hover_text(tip).clicked() {
        app.run_or_alert("view.agent", serde_json::json!({}));
    }
}

/// The sidebar, when it is open.
pub fn show(app: &mut SheetApp, ui: &mut Ui) {
    if !app.agent.open || !available(app) {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let margin = egui::Margin { left: 0, right: GUTTER as i8, top: 0, bottom: GUTTER as i8 };
    egui::Panel::right("agent_sidebar")
        .show_separator_line(false)
        .resizable(false)
        .exact_size(SIDEBAR_W + GUTTER)
        .frame(egui::Frame::NONE.fill(t.window).inner_margin(margin))
        .show(ui, |ui| {
            let card = ui.max_rect();
            ui.painter().rect_filled(card, 10.0, t.ribbon);
            ui.scope_builder(UiBuilder::new().max_rect(card).id_salt("agent_card"), |ui| {
                ui.set_clip_rect(card);
                egui::Panel::top("agent_head")
                    .show_separator_line(false)
                    .exact_size(54.0)
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| header(app, ui, &t));
                egui::Panel::bottom("agent_composer")
                    .show_separator_line(false)
                    .resizable(false)
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 10, right: 10, top: 6, bottom: 10 }))
                    .show(ui, |ui| composer(app, ui, &t));
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: PAD as i8, right: 0, top: 0, bottom: 6 }))
                    .show(ui, |ui| {
                        if app.agent.items.is_empty() {
                            empty(app, ui, &t);
                        } else {
                            conversation(app, ui, &t);
                        }
                    });
            });
        });
}

fn header(app: &mut SheetApp, ui: &mut Ui, t: &Tokens) {
    let r = ui.max_rect();
    let p = ui.painter().clone();
    let tile = Rect::from_min_size(pos2(r.left() + PAD, r.center().y - 15.0), vec2(30.0, 30.0));
    p.rect_filled(tile, 8.0, t.agent_soft);
    let id = app.services.agent.as_ref().map(|a| a.id.clone()).unwrap_or_default();
    match brand_icon(ui.ctx(), &id) {
        Some(tex) => {
            let r = Rect::from_center_size(tile.center(), vec2(18.0, 18.0));
            p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), t.text);
        }
        None => icon(&p, Rect::from_center_size(tile.center(), vec2(16.0, 16.0)), &PROMPT, t.agent),
    }
    let name = agent_name(app);
    let x = tile.right() + 10.0;
    p.text(pos2(x, r.center().y - 9.0), egui::Align2::LEFT_CENTER, &name, theme::ui_bold(14.0), t.text);
    let (dot, status) = status_line(app, t);
    p.circle_filled(pos2(x + 3.0, r.center().y + 9.0), 3.0, dot);
    let status = p.layout(status, ui_font(12.0), t.text_dim, (r.right() - x - 80.0).max(40.0));
    let row = status.rows.first().map(|row| row.text()).unwrap_or_default();
    p.text(pos2(x + 12.0, r.center().y + 9.0), egui::Align2::LEFT_CENTER, row, ui_font(12.0), t.text_dim);
    let close = Rect::from_min_size(pos2(r.right() - 10.0 - 28.0, r.center().y - 14.0), vec2(28.0, 28.0));
    let new = close.translate(vec2(-30.0, 0.0));
    if small_icon_button(ui, new, &NEW_CHAT, t, "New chat").clicked() {
        app.run_or_alert("agent.newChat", serde_json::json!({}));
    }
    if small_icon_button(ui, close, &CLOSE, t, "Close (Ctrl+J)").clicked() {
        app.run_or_alert("view.agent", serde_json::json!({"on": false}));
    }
}

fn status_line(app: &SheetApp, t: &Tokens) -> (Color32, String) {
    let a = &app.agent;
    let detail = app.services.agent.as_ref().map(|l| l.detail.clone()).unwrap_or_default();
    match &a.status {
        Status::Idle | Status::Ready => (t.accent, if detail.is_empty() { "Ready".into() } else { format!("Ready · {detail}") }),
        Status::Starting => (t.text_dim, "Starting…".into()),
        Status::Responding => match &a.turn {
            Some(turn) if !turn.changed.is_empty() => {
                let sheet = turn.target.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
                (t.agent, format!("Editing {sheet} · {} cells", turn.cells))
            }
            _ => (t.agent, "Responding".into()),
        },
        Status::Failed(_) => (t.danger, "Not running".into()),
    }
}

fn small_icon_button(ui: &mut Ui, rect: Rect, paths: &[&str], t: &Tokens, tip: &str) -> Response {
    let resp = ui.interact(rect, ui.scope_id().with(tip), Sense::click());
    if let Some(fill) = crate::widgets::hover_fill(ui, resp.id, resp.hovered(), t.hover) {
        ui.painter().rect_filled(rect, 6.0, fill);
    }
    icon(ui.painter(), Rect::from_center_size(rect.center(), vec2(16.0, 16.0)), paths, t.text_dim);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip));
    resp.on_hover_text(tip)
}

/// A rounded text button: `primary` fills it with the accent.
fn button(ui: &mut Ui, label: &str, lead: Option<&[&str]>, fill: Color32, ink: Color32, border: Option<Color32>) -> Response {
    let font = theme::ui_bold(12.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, ink);
    let lead_w = if lead.is_some() { 19.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0 + lead_w + galley.size().x, 28.0), Sense::click());
    let f = if resp.hovered() { fill.lerp_to_gamma(ink, 0.08) } else { fill };
    ui.painter().rect_filled(rect, 7.0, f);
    if let Some(b) = border {
        ui.painter().rect_stroke(rect, 7.0, Stroke::new(1.0, b), egui::StrokeKind::Inside);
    }
    let mut x = rect.left() + 12.0;
    if let Some(paths) = lead {
        icon(ui.painter(), Rect::from_min_size(pos2(x, rect.center().y - 6.5), vec2(13.0, 13.0)), paths, ink);
        x += lead_w;
    }
    ui.painter().galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, ink);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    resp
}

fn empty(app: &mut SheetApp, ui: &mut Ui, t: &Tokens) {
    let width = ui.available_width() - PAD;
    let title = app.session.active().map(|d| d.display_title()).unwrap_or_default();
    let name = agent_name(app);
    let (detail, hint) = app.services.agent.as_ref().map(|l| (l.detail.clone(), l.change_hint.clone())).unwrap_or_default();
    let suggestions = super::suggestions(&app.session);
    let failed = match &app.agent.status {
        Status::Failed(e) => Some(e.clone()),
        _ => None,
    };
    let mut picked = None;
    let mut retry = false;
    ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 8.0;
        for (i, (kind, text)) in suggestions.iter().enumerate().rev() {
            if suggestion(ui, *kind, text, t, width, i).clicked() {
                picked = Some(text.clone());
            }
        }
        ui.add_space(6.0);
        if let Some(h) = hint {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(egui::RichText::new("Change agents with").font(ui_font(12.0)).color(faint(t)));
                egui::Frame::NONE.fill(t.window).corner_radius(4.0).inner_margin(egui::Margin::symmetric(5, 1)).show(ui, |ui| {
                    ui.label(egui::RichText::new(h).font(mono(11.0)).color(t.text_dim));
                });
            });
        }
        let about = if detail.is_empty() {
            format!("{name} can read every sheet in this workbook, explain numbers and formulas, and make changes. Everything it changes in one go is one step you can keep or undo.")
        } else {
            format!(
                "{name} is your {}. It can read every sheet in this workbook, explain numbers and formulas, and make changes. Everything it changes in one go is one step you can keep or undo.",
                detail
            )
        };
        ui.add(egui::Label::new(egui::RichText::new(about).font(ui_font(13.0)).color(t.text_dim)).wrap());
        ui.add(egui::Label::new(egui::RichText::new(format!("Ask about {title}")).font(theme::ui_bold(21.0)).color(t.text)).wrap());
        if let Some(e) = &failed {
            ui.add_space(12.0);
            if button(ui, "Try again", None, t.hover, t.text, None).clicked() {
                retry = true;
            }
            ui.add(egui::Label::new(egui::RichText::new(e).font(ui_font(12.5)).color(t.danger)).wrap());
        }
    });
    if retry {
        app.agent.status = Status::Idle;
        app.agent.start_error = None;
    }
    if let Some(text) = picked {
        // A failure shows in the conversation, not in a message box.
        let _ = app.run("agent.prompt", serde_json::json!({"text": text}));
    }
}

fn suggestion(ui: &mut Ui, kind: SuggestIcon, text: &str, t: &Tokens, width: f32, i: usize) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 34.0), Sense::click());
    let fill = if resp.hovered() { t.hover } else { t.window };
    ui.painter().rect_filled(rect, 8.0, fill);
    let (paths, c): (&[&str], Color32) = match kind {
        SuggestIcon::Trend => (&TREND, t.cat(theme::Cat::Red)),
        SuggestIcon::Sum => (&SIGMA, t.accent),
        SuggestIcon::Clean => (&ERASER, t.text),
        SuggestIcon::Chart => (&CHART, t.accent),
        SuggestIcon::Table => (&TABLE, t.accent),
    };
    icon(ui.painter(), Rect::from_min_size(pos2(rect.left() + 12.0, rect.center().y - 7.0), vec2(14.0, 14.0)), paths, c);
    let galley = ui.painter().layout(text.to_string(), ui_font(13.0), t.text, rect.width() - 50.0);
    let row = galley.rows.first().map(|r| r.text()).unwrap_or_default();
    ui.painter().text(pos2(rect.left() + 38.0, rect.center().y), egui::Align2::LEFT_CENTER, row, ui_font(13.0), t.text);
    let _ = i;
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, text));
    resp
}

fn conversation(app: &mut SheetApp, ui: &mut Ui, t: &Tokens) {
    let items = app.agent.items.clone();
    let mut action: Option<(&str, serde_json::Value)> = None;
    let height_id = ui.scope_id().with("agent_content_h");
    let last_h: f32 = ui.ctx().data(|d| d.get_temp(height_id)).unwrap_or(0.0);
    egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        let width = ui.available_width() - PAD;
        ui.set_width(width);
        // Short conversations sit at the bottom, by the composer.
        let room = ui.clip_rect().height() - last_h;
        if room > 0.0 {
            ui.add_space(room);
        }
        let top = ui.cursor().top();
        ui.spacing_mut().item_spacing.y = 4.0;
        let mut prev_activity = false;
        for (i, item) in items.iter().enumerate() {
            let activity = matches!(item, Item::Activity { .. });
            // Tool calls fold into one line: only the latest of a run is drawn.
            if activity && matches!(items.get(i + 1), Some(Item::Activity { .. })) {
                continue;
            }
            // Activity lines stack; everything else gets air above it.
            ui.add_space(if i == 0 || (activity && prev_activity) { 0.0 } else { 10.0 });
            prev_activity = activity;
            match item {
                Item::User { text, range } => user_bubble(ui, t, text, range.as_deref(), width),
                Item::Activity { verb, target, detail, state, .. } => activity_row(ui, t, verb, target, detail, *state, width),
                Item::Reply(md) => reply(ui, t, md, width),
                Item::Plan(steps) => plan(ui, t, steps, width),
                Item::Permission { title, options, answer, .. } => {
                    if let Some(a) = permission_card(ui, t, &agent_name(app), title, options, answer.as_deref(), width) {
                        action = Some(("agent.permission", serde_json::json!({"allow": a})));
                    }
                }
                Item::Changes(c) => {
                    if let Some(keep) = changes_card(ui, t, &agent_name(app), c, width) {
                        action = Some((if keep { "agent.keep" } else { "agent.undo" }, serde_json::json!({})));
                    }
                }
                Item::Note { text, error } => {
                    ui.add(egui::Label::new(egui::RichText::new(text).font(ui_font(12.5)).color(if *error { t.danger } else { faint(t) })).wrap());
                }
            }
        }
        if let Some(turn) = &app.agent.turn {
            if !turn.changed.is_empty() {
                ui.add_space(10.0);
                if working_card(ui, t, turn, width) {
                    action = Some(("agent.stop", serde_json::json!({})));
                }
            }
            ui.add_space(8.0);
            working_line(ui, t, app, width);
        }
        let h = ui.cursor().top() - top;
        ui.ctx().data_mut(|d| d.insert_temp(height_id, h));
    });
    if let Some((id, p)) = action {
        app.run_or_alert(id, p);
    }
}

fn user_bubble(ui: &mut Ui, t: &Tokens, text: &str, range: Option<&str>, width: f32) {
    let max = 280.0f32.min(width - 20.0);
    let galley = ui.painter().layout(text.to_string(), ui_font(13.0), t.text, max - 24.0);
    let chip = range.map(|r| ui.painter().layout_no_wrap(r.to_string(), mono(11.0), t.text_dim));
    let inner_w = galley.size().x.max(chip.as_ref().map_or(0.0, |c| c.size().x + 17.0));
    let h = 9.0 + galley.size().y + chip.as_ref().map_or(0.0, |_| 6.0 + 14.0) + 9.0;
    let (row, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let rect = Rect::from_min_size(pos2(row.right() - inner_w - 24.0, row.top()), vec2(inner_w + 24.0, h));
    ui.painter().rect_filled(rect, CornerRadius { nw: 12, ne: 12, sw: 12, se: 4 }, t.hover);
    let gh = galley.size().y;
    ui.painter().galley(pos2(rect.left() + 12.0, rect.top() + 9.0), galley, t.text);
    if let Some(c) = chip {
        let y = rect.top() + 9.0 + gh + 6.0;
        icon(ui.painter(), Rect::from_min_size(pos2(rect.left() + 12.0, y + 1.0), vec2(12.0, 12.0)), &TABLE, t.accent);
        ui.painter().galley(pos2(rect.left() + 29.0, y), c, t.text_dim);
    }
}

fn activity_row(ui: &mut Ui, t: &Tokens, verb: &str, target: &str, detail: &str, state: Step, width: f32) {
    let mut job = egui::text::LayoutJob { wrap: egui::text::TextWrapping { max_width: width - 21.0, ..Default::default() }, ..Default::default() };
    let fmt = |font: FontId, color: Color32| egui::TextFormat { font_id: font, color, line_height: Some(16.0), ..Default::default() };
    job.append(verb, 0.0, fmt(ui_font(12.0), t.text_dim));
    if !target.is_empty() {
        let is_ref = target.contains('!')
            || target.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && target.chars().any(|c| c.is_ascii_digit()) && !target.contains(' ');
        job.append(target, if verb.is_empty() { 0.0 } else { 7.0 }, fmt(if is_ref { mono(11.0) } else { ui_font(12.0) }, t.text_dim));
    }
    if !detail.is_empty() {
        job.append(detail, 7.0, fmt(ui_font(12.0), if state == Step::Failed { t.danger } else { faint(t) }));
    }
    let galley = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(vec2(width, galley.size().y.max(16.0)), Sense::hover());
    let ic = Rect::from_min_size(pos2(rect.left(), rect.top() + 1.0), vec2(14.0, 14.0));
    let c = faint(t);
    match state {
        Step::Failed => icon(ui.painter(), ic, &CROSS, t.danger),
        Step::Running => spinner(ui, ic.shrink(1.5), t.agent),
        Step::Done => {
            let paths: &[&str] = match verb {
                "Read" | "Looked over" | "Looked at" | "Looked up" | "Searched" | "Fetched" => &EYE,
                "Wrote" | "Edited" => &PENCIL,
                _ => &TOOL,
            };
            icon(ui.painter(), ic, paths, c);
        }
    }
    ui.painter().galley(pos2(rect.left() + 21.0, rect.top()), galley, t.text_dim);
}

fn spinner(ui: &Ui, r: Rect, c: Color32) {
    let time = ui.input(|i| i.time) as f32;
    let start = time * 5.0;
    let pts: Vec<egui::Pos2> = (0..=16)
        .map(|k| {
            let a = start + k as f32 / 16.0 * std::f32::consts::PI * 1.4;
            r.center() + vec2(a.cos(), a.sin()) * (r.width() / 2.0)
        })
        .collect();
    ui.painter().add(egui::Shape::line(pts, Stroke::new(1.6, c)));
    ui.ctx().request_repaint();
}

fn reply(ui: &mut Ui, t: &Tokens, md: &str, width: f32) {
    let st = markdown::Style { text: t.text, dim: t.text_dim, code: t.agent, size: 13.0, line_height: 20.0 };
    let job = markdown::layout(md.trim(), width, &st);
    ui.add(egui::Label::new(job).selectable(true));
}

fn plan(ui: &mut Ui, t: &Tokens, steps: &[(String, String)], width: f32) {
    ui.label(egui::RichText::new("PLAN").font(theme::ui_bold(11.0)).color(faint(t)));
    for (i, (step, status)) in steps.iter().enumerate() {
        let done = status == "completed";
        let c = if done { faint(t) } else { t.text };
        let galley = ui.painter().layout(step.clone(), ui_font(13.0), c, width - 20.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, galley.size().y.max(19.0)), Sense::hover());
        ui.painter().text(pos2(rect.left(), rect.top() + 9.5), egui::Align2::LEFT_CENTER, format!("{}", i + 1), ui_font(13.0), faint(t));
        ui.painter().galley(pos2(rect.left() + 20.0, rect.top() + (19.0 - galley.rows.first().map_or(19.0, |r| r.height())) / 2.0), galley, c);
    }
}

/// A bordered card with a tinted header row; `body` draws below the header.
fn card(ui: &mut Ui, t: &Tokens, width: f32, head: impl FnOnce(&mut Ui), body: impl FnOnce(&mut Ui)) {
    let frame = egui::Frame::NONE.fill(t.ribbon).stroke(Stroke::new(1.0, t.agent_border)).corner_radius(10.0);
    frame.show(ui, |ui| {
        ui.set_width(width - 2.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::Frame::NONE
            .fill(t.agent_wash)
            .corner_radius(CornerRadius { nw: 9, ne: 9, sw: 0, se: 0 })
            .inner_margin(egui::Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.set_width(width - 26.0);
                ui.horizontal(head);
            });
        body(ui);
    });
}

fn permission_card(
    ui: &mut Ui,
    t: &Tokens,
    name: &str,
    title: &str,
    options: &[super::PermissionOption],
    answer: Option<&str>,
    width: f32,
) -> Option<bool> {
    let mut out = None;
    card(
        ui,
        t,
        width,
        |ui| {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 16.0), Sense::hover());
            icon(ui.painter(), Rect::from_center_size(r.center(), vec2(14.0, 14.0)), &SHIELD, t.agent);
            ui.label(egui::RichText::new(format!("{name} asks to")).font(theme::ui_bold(12.5)).color(t.text));
        },
        |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 12, top: 8, bottom: 10 }).show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(title).font(ui_font(13.0)).color(t.text)).wrap());
                ui.add_space(8.0);
                match answer {
                    Some(a) => {
                        ui.label(egui::RichText::new(a).font(ui_font(12.0)).color(faint(t)));
                    }
                    None => {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let can_allow = options.iter().any(|o| o.kind.starts_with("allow"));
                            if can_allow && button(ui, "Allow once", Some(&CHECK), t.accent, t.on_accent, None).clicked() {
                                out = Some(true);
                            }
                            if button(ui, "Deny", None, t.hover, t.text, None).clicked() {
                                out = Some(false);
                            }
                        });
                    }
                }
            });
        },
    );
    out
}

/// "G10:G17 · 8 cells", or "3 ranges · 24 cells".
fn changes_summary(ranges: &[(String, String)], cells: u64) -> String {
    let what = match ranges {
        [(r, _)] => r.rsplit_once('!').map(|(_, b)| b.to_string()).unwrap_or(r.clone()),
        list => format!("{} ranges", list.len()),
    };
    if cells > 0 { format!("{what} · {cells} {}", if cells == 1 { "cell" } else { "cells" }) } else { what }
}

fn changes_card(ui: &mut Ui, t: &Tokens, name: &str, c: &Changes, width: f32) -> Option<bool> {
    let mut out = None;
    let summary = changes_summary(&c.ranges, c.cells);
    card(
        ui,
        t,
        width,
        |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 16.0), Sense::hover());
            icon(ui.painter(), Rect::from_center_size(r.center(), vec2(14.0, 14.0)), &CHANGES, t.agent);
            ui.label(egui::RichText::new(format!("Changes by {name}")).font(theme::ui_bold(12.5)).color(t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let right = match c.state {
                    ChangeState::Pending => summary.clone(),
                    ChangeState::Kept => "Kept".into(),
                    ChangeState::Undone => "Undone".into(),
                };
                ui.label(egui::RichText::new(right).font(mono(11.0)).color(t.agent));
            });
        },
        |ui| {
            if c.state != ChangeState::Pending {
                return;
            }
            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 4)).show(ui, |ui| {
                // One sheet: its name is left off the rows.
                let one_sheet =
                    c.ranges.iter().filter_map(|(q, _)| q.rsplit_once('!').map(|(s, _)| s)).collect::<std::collections::HashSet<_>>().len() <= 1;
                for (range, what) in c.ranges.iter().take(8) {
                    ui.add_space(5.0);
                    // Top-aligned, with the description wrapping: a long list of what was done
                    // must not widen the card (and with it the whole sidebar).
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let shown = if one_sheet { range.rsplit_once('!').map_or(range.as_str(), |(_, b)| b) } else { range.as_str() };
                        ui.add_sized(
                            vec2(56.0_f32.max(ui.painter().layout_no_wrap(shown.to_string(), mono(11.0), t.text_dim).size().x), 16.0),
                            egui::Label::new(egui::RichText::new(shown).font(mono(11.0)).color(t.text_dim)),
                        );
                        ui.add(egui::Label::new(egui::RichText::new(what).font(ui_font(12.5)).color(t.text)).wrap());
                    });
                }
                if c.ranges.len() > 8 {
                    ui.add_space(5.0);
                    ui.label(egui::RichText::new(format!("and {} more", c.ranges.len() - 8)).font(ui_font(12.0)).color(faint(t)));
                }
                ui.add_space(5.0);
            });
            let (line, _) = ui.allocate_exact_size(vec2(width - 2.0, 1.0), Sense::hover());
            ui.painter().rect_filled(line, 0.0, t.separator);
            egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 10, top: 8, bottom: 10 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if button(ui, "Keep", Some(&CHECK), t.accent, t.on_accent, None).clicked() {
                        out = Some(true);
                    }
                    if button(ui, "Undo", None, t.hover, t.text, None).clicked() {
                        out = Some(false);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(egui::RichText::new("Ctrl+Z undoes it").font(ui_font(11.5)).color(faint(t)));
                    });
                });
            });
        },
    );
    out
}

/// The card shown while the agent writes: what it has written so far, a progress bar and Stop.
/// Returns whether Stop was clicked.
fn working_card(ui: &mut Ui, t: &Tokens, turn: &super::Turn, width: f32) -> bool {
    let mut stop = false;
    let cells = turn.cells;
    egui::Frame::NONE.fill(t.ribbon).stroke(Stroke::new(1.0, t.agent_border)).corner_radius(10.0).inner_margin(egui::Margin::symmetric(12, 10)).show(
        ui,
        |ui| {
            ui.set_width(width - 26.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Applying change").font(theme::ui_bold(12.5)).color(t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("{cells} {}", if cells == 1 { "cell" } else { "cells" })).font(mono(11.0)).color(t.agent));
                });
            });
            ui.add_space(8.0);
            // Indeterminate progress: the agent doesn't say how much is left.
            let (bar, _) = ui.allocate_exact_size(vec2(width - 26.0, 4.0), Sense::hover());
            ui.painter().rect_filled(bar, 2.0, t.separator);
            let phase = (ui.input(|i| i.time) as f32 * 0.6).fract();
            let seg_w = bar.width() * 0.35;
            let x0 = bar.left() + (bar.width() + seg_w) * phase - seg_w;
            let seg = Rect::from_min_max(pos2(x0.max(bar.left()), bar.top()), pos2((x0 + seg_w).min(bar.right()), bar.bottom()));
            if seg.width() > 0.0 {
                ui.painter().rect_filled(seg, 2.0, t.agent);
            }
            ui.ctx().request_repaint();
            ui.add_space(8.0);
            let n = turn.changed.len();
            for (i, (range, what)) in turn.changed.iter().enumerate().skip(n.saturating_sub(5)) {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 16.0), Sense::hover());
                    let latest = i + 1 == n;
                    if latest && !turn.stopping {
                        spinner(ui, Rect::from_center_size(r.center(), vec2(11.0, 11.0)), t.agent);
                    } else {
                        icon(ui.painter(), Rect::from_center_size(r.center(), vec2(13.0, 13.0)), &CHECK, t.accent);
                    }
                    ui.label(egui::RichText::new(what).font(if latest { theme::ui_bold(12.5) } else { ui_font(12.5) }).color(if latest {
                        t.text
                    } else {
                        t.text_dim
                    }));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let short = range.rsplit_once('!').map(|(_, b)| b.to_string()).unwrap_or(range.clone());
                        ui.label(egui::RichText::new(short).font(mono(11.0)).color(if latest { t.agent } else { faint(t) }));
                    });
                });
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                if !turn.stopping && button(ui, "Stop", Some(&["M7 7h10v10H7Z"]), t.ribbon, t.danger, Some(t.input_border)).clicked() {
                    stop = true;
                }
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("Cells written so far stay. Ctrl+Z removes the whole change.").font(ui_font(11.5)).color(faint(t)),
                    )
                    .wrap(),
                );
            });
        },
    );
    stop
}

/// "Writing Dashboard!G15 •••" under the conversation while the agent works.
fn working_line(ui: &mut Ui, t: &Tokens, app: &SheetApp, width: f32) {
    let turn = app.agent.turn.as_ref();
    let replied = matches!(app.agent.items.last(), Some(Item::Reply(_)));
    let (verb, target) = match turn.and_then(|t| t.target.clone()) {
        Some((sheet, r)) => ("Writing", super::qualified(&sheet, &r)),
        None if turn.is_some_and(|t| t.stopping) => ("Stopping", String::new()),
        None if replied => ("Writing", String::new()),
        None => ("Thinking", String::new()),
    };
    let (rect, _) = ui.allocate_exact_size(vec2(width, 16.0), Sense::hover());
    let p = ui.painter();
    let mut x = rect.left();
    if !target.is_empty() {
        icon(p, Rect::from_min_size(pos2(x, rect.top() + 1.0), vec2(14.0, 14.0)), &PENCIL, t.text_dim);
        x += 21.0;
    }
    let g = p.layout_no_wrap(verb.to_string(), ui_font(12.0), if target.is_empty() { faint(t) } else { t.text });
    let gw = g.size().x;
    p.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, t.text);
    x += gw + 7.0;
    if !target.is_empty() {
        let g = p.layout_no_wrap(target, mono(11.0), t.agent);
        let gw = g.size().x;
        p.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, t.agent);
        x += gw + 8.0;
    }
    let time = ui.input(|i| i.time) as f32;
    for k in 0..3 {
        let phase = ((time * 2.2 - k as f32 * 0.25).rem_euclid(1.0) - 0.5).abs() * 2.0;
        let alpha = 0.25 + 0.75 * phase;
        p.circle_filled(pos2(x + 2.5 + k as f32 * 8.0, rect.center().y), 2.5, t.agent.gamma_multiply(alpha));
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
}

fn composer(app: &mut SheetApp, ui: &mut Ui, t: &Tokens) {
    let responding = app.agent.responding();
    let chip = if app.agent.drop_selection { None } else { super::context(app, true).1 };
    let width = ui.available_width();
    let edit_id = ui.scope_id().with("agent_draft");
    let focused = ui.memory(|m| m.has_focus(edit_id));
    // Enter sends and Shift+Enter starts a new line; Esc stops a running turn.
    let mut send = false;
    if focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
        send = true;
    }
    if (focused || responding) && responding && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        app.run_or_alert("agent.stop", serde_json::json!({}));
    }
    let mut remove_chip = false;
    let mut click_send = false;
    egui::Frame::NONE
        .fill(t.window)
        .stroke(Stroke::new(1.0, t.ribbon_border))
        .corner_radius(10.0)
        .inner_margin(egui::Margin { left: 12, right: 10, top: 10, bottom: 8 })
        .show(ui, |ui| {
            ui.set_width(width - 24.0);
            ui.spacing_mut().item_spacing.y = 8.0;
            if let Some(range) = &chip {
                egui::Frame::NONE
                    .fill(t.ribbon)
                    .stroke(Stroke::new(1.0, t.ribbon_border))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(7, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let (r, _) = ui.allocate_exact_size(vec2(12.0, 14.0), Sense::hover());
                            icon(ui.painter(), Rect::from_center_size(r.center(), vec2(12.0, 12.0)), &TABLE, t.accent);
                            ui.label(egui::RichText::new(range).font(mono(11.0)).color(t.text_dim));
                            let (x, resp) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
                            icon(ui.painter(), x.shrink(2.0), &CLOSE, if resp.hovered() { t.text } else { faint(t) });
                            if resp.on_hover_text("Don't send the selection").clicked() {
                                remove_chip = true;
                            }
                        });
                    });
            }
            let hint = if responding { "Reply, or press Esc to stop" } else { "Ask about the workbook, or describe a change" };
            let edit = egui::TextEdit::multiline(&mut app.agent.draft)
                .id(edit_id)
                .frame(egui::Frame::NONE)
                .font(ui_font(13.0))
                .text_color(t.text)
                .hint_text(egui::RichText::new(hint).font(ui_font(13.0)).color(faint(t)))
                .desired_rows(1)
                .desired_width(f32::INFINITY);
            let resp = ui.add(edit);
            if app.agent.focus_composer {
                resp.request_focus();
                app.agent.focus_composer = false;
            }
            ui.horizontal(|ui| {
                let model = app.agent.model.clone().unwrap_or_else(|| "Default model".into());
                let model = model.rsplit('/').next().unwrap_or(&model).to_string();
                ui.label(egui::RichText::new(model).font(ui_font(12.0)).color(t.text_dim))
                    .on_hover_text("The agent's model (set in the agent's own settings)");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (r, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
                    let empty = app.agent.draft.trim().is_empty();
                    if responding {
                        ui.painter().circle_filled(r.center(), 15.0, t.text);
                        ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(10.0, 10.0)), 2.0, t.ribbon);
                    } else {
                        let fill = if empty { t.text_disabled } else { t.accent };
                        ui.painter().circle_filled(r.center(), 15.0, fill);
                        icon(ui.painter(), Rect::from_center_size(r.center(), vec2(16.0, 16.0)), &SEND, t.on_accent);
                    }
                    let tip = if responding { "Stop (Esc)" } else { "Send (Enter)" };
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip));
                    if resp.on_hover_text(tip).clicked() {
                        if responding {
                            app.run_or_alert("agent.stop", serde_json::json!({}));
                        } else {
                            click_send = true;
                        }
                    }
                });
            });
        });
    if remove_chip {
        app.agent.drop_selection = true;
    }
    if (send || click_send) && !responding && !app.agent.draft.trim().is_empty() {
        let text = app.agent.draft.clone();
        let _ = app.run("agent.prompt", serde_json::json!({"text": text}));
        app.agent.drop_selection = false;
        app.agent.focus_composer = true;
    }
}
