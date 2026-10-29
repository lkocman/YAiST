use std::time::Duration;

use egui::{vec2, Color32, CornerRadius, FontId, Frame, Margin, RichText, Stroke, Ui};
use serde_json::{Map, Value};
use yaist_core::{Output, ParamKind, Risk, Table};
use yaist_ui::release_notes;
use yaist_ui::welcome::{self, Start};
use yaist_ui::{
    reboot_notice, row_action_is_direct, row_args, tone_of, update_summary, Asked, Cmd, Entry, Form, LlmStatus, ModuleView, Panel, Plan, PlanStatus,
    Session, UpdateLevel, UpdateStatus, UPDATE_ACTION,
};

use crate::theme::{self, Tokens};
use crate::widgets::{self, Kind};

const SIDEBAR_W: f32 = 264.0;
/// The app icon (SVG, sharp at any scale), shown next to the wordmark.
const APP_ICON: &[u8] = include_bytes!("../../../assets/icons/hicolor/scalable/apps/org.opensuse.yaist.svg");
const ROW_H: f32 = 40.0;

/// What the user did this frame; applied once drawing is done.
enum Act {
    Select(usize),
    Refresh(usize),
    Propose(&'static str, Map<String, Value>),
    Form(Form),
    Confirm(u64),
    Cancel(u64),
    Submit(String),
    CheckUpdates,
    Answer(u64, String),
    Retry(u64),
    WhatsNew(release_notes::Target),
}

/// Proposes rolling back to snapshot `n` (a Dangerous plan, confirmed first).
fn rollback(n: u32) -> Act {
    let mut raw = Map::new();
    raw.insert("number".into(), Value::String(n.to_string()));
    Act::Propose("snapshots.rollback", raw)
}

/// What one frame draws, copied out of the shared state so the backend isn't
/// blocked while we lay out.
struct View {
    modules: Vec<ModuleView>,
    selected: usize,
    conversation: Vec<Entry>,
    busy: Option<String>,
    llm: LlmStatus,
    llm_url: String,
    distro: String,
    distro_info: yaist_core::Distro,
    pending: bool,
    updates: UpdateStatus,
    question: Option<Asked>,
}

pub struct App {
    session: Session,
    input: String,
    form: Option<Form>,
    /// A row opened in its own dialog (double-click).
    detail: Option<Detail>,
    /// Modules whose table shows every column even in a narrow window.
    all_columns: std::collections::HashSet<&'static str>,
    acts: Vec<Act>,
    shot: Option<Shot>,
    /// Confirm the demo request's plan once it is up (`YAIST_DEMO_CONFIRM`).
    confirm_demo: bool,
    /// Where to click (`YAIST_DEMO_CLICK`), how often, and when.
    demo_click: Option<(egui::Pos2, usize, std::time::Instant)>,
    /// The choice picked in the open question: (question id, choice index).
    choice: (u64, usize),
    /// Running upgrades whose "what's new" offer was declined.
    no_notes: std::collections::HashSet<u64>,
    /// For the input field's pulse at start.
    opened: std::time::Instant,
}

/// One table row with every column, and the actions for it.
struct Detail {
    /// The table's columns and just this row, for the row actions.
    table: Table,
    actions: Vec<yaist_core::ActionSpec>,
}

/// `YAIST_SCREENSHOT=file.ppm`: save one frame after `YAIST_SCREENSHOT_DELAY`
/// seconds and quit. `YAIST_MODULE` (number), `YAIST_DEMO` (a request) and
/// `YAIST_DEMO_SCENE` (made-up cards, see `yaist_ui::demo`) set the scene;
/// `YAIST_DEMO_CONFIRM=1` confirms the plan the request raises, to show what
/// follows, and `YAIST_DEMO_CLICK=x,y[,clicks[,seconds]]`
/// clicks there (2 for a double-click) after two seconds, or as many as given.
/// For docs and visual checks.
struct Shot {
    path: String,
    after: Duration,
    started: std::time::Instant,
    requested: bool,
    /// Filled by egui's screenshot callback.
    image: std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<egui::ColorImage>>>>,
}

impl App {
    pub fn new(session: Session) -> Self {
        if let Some(i) = std::env::var("YAIST_MODULE").ok().and_then(|m| m.parse::<usize>().ok()) {
            session.select(i.saturating_sub(1));
        }
        if let Ok(demo) = std::env::var("YAIST_DEMO") {
            session.send(Cmd::Submit(demo));
        }
        if let Ok(name) = std::env::var("YAIST_DEMO_SCENE") {
            session.send(Cmd::Demo(name));
        }
        let shot = std::env::var("YAIST_SCREENSHOT").ok().map(|path| Shot {
            path,
            after: Duration::from_secs_f32(std::env::var("YAIST_SCREENSHOT_DELAY").ok().and_then(|d| d.parse().ok()).unwrap_or(4.0)),
            started: std::time::Instant::now(),
            requested: false,
            image: Default::default(),
        });
        let confirm_demo = std::env::var("YAIST_DEMO_CONFIRM").is_ok_and(|c| c == "1");
        let demo_click = std::env::var("YAIST_DEMO_CLICK").ok().and_then(|c| {
            let mut v = c.split(',').map(|n| n.trim().parse::<f32>());
            let (x, y) = (v.next()?.ok()?, v.next()?.ok()?);
            let clicks = v.next().and_then(|n| n.ok()).unwrap_or(1.0) as usize;
            let after = v.next().and_then(|n| n.ok()).unwrap_or(2.0);
            Some((egui::pos2(x, y), clicks, std::time::Instant::now() + Duration::from_secs_f32(after)))
        });
        Self {
            session,
            input: String::new(),
            form: None,
            detail: None,
            all_columns: Default::default(),
            acts: Vec::new(),
            shot,
            confirm_demo,
            demo_click,
            choice: (0, 0),
            no_notes: Default::default(),
            opened: std::time::Instant::now(),
        }
    }

    fn screenshot(&mut self, ctx: &egui::Context) {
        if self.confirm_demo {
            let pending = self.session.state().pending_plan().map(|p| p.id);
            if let Some(id) = pending {
                self.session.send(Cmd::Confirm(id));
                self.confirm_demo = false;
            }
        }
        let Some(shot) = &mut self.shot else { return };
        ctx.request_repaint_after(Duration::from_millis(200));
        if !shot.requested && shot.started.elapsed() >= shot.after {
            let (slot, repaint) = (shot.image.clone(), ctx.clone());
            let callback = egui::ScreenshotCallback::new(move |img| {
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(img);
                repaint.request_repaint();
            });
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(callback));
            shot.requested = true;
        }
        let image = shot.image.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(img) = image {
            let mut ppm = format!("P6\n{} {}\n255\n", img.size[0], img.size[1]).into_bytes();
            for p in &img.pixels {
                ppm.extend_from_slice(&[p.r(), p.g(), p.b()]);
            }
            if let Err(e) = std::fs::write(&shot.path, ppm) {
                eprintln!("writing {}: {e}", shot.path);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn view(&self) -> View {
        let s = self.session.state();
        View {
            modules: s.modules.clone(),
            selected: s.selected,
            conversation: s.conversation.clone(),
            busy: s.busy.clone(),
            llm: s.llm.clone(),
            llm_url: s.llm_url.clone(),
            distro: if s.distro.pretty.is_empty() { s.distro.id.clone() } else { s.distro.pretty.clone() },
            distro_info: s.distro.clone(),
            pending: s.pending_plan().is_some(),
            updates: s.updates.clone(),
            question: s.question.clone(),
        }
    }

    fn apply(&mut self) {
        for act in std::mem::take(&mut self.acts) {
            match act {
                Act::Select(i) => self.session.select(i),
                Act::Refresh(i) => self.session.send(Cmd::Refresh(i)),
                Act::Propose(action, raw) => self.session.send(Cmd::Propose { action, raw }),
                Act::Form(f) => self.form = Some(f),
                Act::Confirm(id) => self.session.send(Cmd::Confirm(id)),
                Act::Cancel(id) => self.session.send(Cmd::Cancel(id)),
                Act::Retry(id) => self.session.send(Cmd::Retry(id)),
                Act::Submit(text) => self.session.send(Cmd::Submit(text)),
                Act::CheckUpdates => self.session.send(Cmd::CheckUpdates),
                Act::Answer(id, value) => self.session.send(Cmd::Answer { id, value }),
                Act::WhatsNew(target) => self.session.send(Cmd::WhatsNew(target)),
            }
        }
    }
}

impl eframe::App for App {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let Some((pos, clicks, at)) = self.demo_click else { return };
        ctx.request_repaint_after(Duration::from_millis(50));
        if std::time::Instant::now() < at {
            return;
        }
        // One click per frame, so egui sees a double-click as one.
        self.demo_click = (clicks > 1).then(|| (pos, clicks - 1, at));
        let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        raw_input.events.extend([egui::Event::PointerMoved(pos), button(true), button(false)]);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let t = Tokens::get(&ctx);
        let v = self.view();
        if v.busy.is_some() {
            ctx.request_repaint_after(Duration::from_millis(80));
        }

        egui::Panel::top("header").frame(Frame::new().fill(t.bg)).exact_size(theme::HEADER_H).resizable(false).show(ui, |ui| self.header(ui, &v));
        egui::Panel::bottom("footer")
            .frame(Frame::new().fill(t.bg).inner_margin(Margin::symmetric(20, 0)))
            .exact_size(theme::FOOTER_H)
            .resizable(false)
            .show(ui, |ui| footer(ui, &v));
        egui::Panel::left("modules").frame(Frame::new().fill(t.bg)).exact_size(SIDEBAR_W).resizable(false).show(ui, |ui| self.sidebar(ui, &v));
        egui::Panel::right("conversation")
            .frame(Frame::new().fill(t.bg))
            .default_size(470.0)
            .size_range(380.0..=760.0)
            .resizable(true)
            .show(ui, |ui| self.conversation(ui, &v));
        egui::CentralPanel::default()
            .frame(Frame::new().fill(t.bg).inner_margin(Margin { left: 28, right: 28, top: 22, bottom: 0 }))
            .show(ui, |ui| self.module_page(ui, &v));

        self.form_dialog(&ctx);
        self.detail_dialog(&ctx);
        if let Some(q) = &v.question {
            self.question_dialog(&ctx, q);
        }
        widgets::window_edges(&ctx);
        self.apply();
        self.screenshot(&ctx);
    }
}

// ---- header and footer ---------------------------------------------------------------------------

impl App {
    fn header(&mut self, ui: &mut Ui, v: &View) {
        let t = Tokens::get(ui.ctx());
        widgets::title_bar_drag(ui);
        let r = ui.max_rect();
        ui.painter().hline(r.x_range(), r.bottom() - 0.5, t.hairline());
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            // The icon carries the name, so no text label beside it.
            ui.add_space(22.0);
            ui.add(egui::Image::from_bytes("bytes://yaist-icon.svg", APP_ICON).fit_to_exact_size(vec2(35.0, 35.0))).on_hover_text(yaist_ui::NAME);
            ui.add_space(24.0);
            widgets::rule(ui, theme::HEADER_H);
            ui.add_space(18.0);
            widgets::tag(ui, "System", t.invert_bg, t.invert_fg);
            ui.add_space(12.0);
            match welcome::distribution_logo(welcome::LOGO_INLINE) {
                Some(uri) => {
                    ui.add(egui::Image::new(uri).max_height(20.0).tint(t.ink_strong)).on_hover_text(&v.distro);
                    ui.add_space(8.0);
                    widgets::hud(ui, &v.distro_info.version, t.muted);
                }
                None => {
                    widgets::hud(ui, &v.distro, t.muted);
                }
            }
            ui.add_space(16.0);
            self.update_badge(ui, v);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                widgets::window_buttons(ui, theme::HEADER_H);
                let dark = ui.ctx().theme() == egui::Theme::Dark;
                if widgets::theme_toggle(ui, dark, theme::HEADER_H).clicked() {
                    ui.ctx().set_theme(if dark { egui::ThemePreference::Light } else { egui::ThemePreference::Dark });
                }
                widgets::rule(ui, theme::HEADER_H);
                ui.add_space(16.0);
                match &v.llm {
                    LlmStatus::Ready(name) => widgets::badge(ui, &format!("model · {name}"), t.good),
                    LlmStatus::Checking => widgets::badge(ui, "model · checking", t.muted),
                    LlmStatus::Offline => widgets::badge(ui, "model · offline", t.warn),
                };
                ui.add_space(10.0);
                widgets::hud(ui, "local ai", t.faint);
            });
        });
    }
}

fn footer(ui: &mut Ui, v: &View) {
    let t = Tokens::get(ui.ctx());
    let r = ui.max_rect();
    ui.painter().hline(r.x_range(), r.top() + 0.5, t.hairline());
    ui.horizontal_centered(|ui| {
        widgets::hud(ui, "YAiST 0.1 · prototype · changes ask polkit", t.faint);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            widgets::hud(ui, &format!("llm · {}", v.llm_url.trim_start_matches("http://")), t.faint);
        });
    });
}

/// The YAiST wordmark, with the AI in Geeko green.
fn wordmark(ui: &mut Ui, size: f32) {
    let t = Tokens::get(ui.ctx());
    let mut job = egui::text::LayoutJob::default();
    for (text, accent) in yaist_ui::WORDMARK {
        let color = if accent { t.brand } else { t.ink_strong };
        job.append(text, 0.0, egui::TextFormat { font_id: FontId::new(size, theme::display()), color, ..Default::default() });
    }
    ui.label(job);
}

// ---- sidebar -------------------------------------------------------------------------------------

impl App {
    fn sidebar(&mut self, ui: &mut Ui, v: &View) {
        let t = Tokens::get(ui.ctx());
        let full = ui.max_rect();
        ui.painter().vline(full.right() - 0.5, full.y_range(), t.hairline());
        ui.add_space(22.0);
        ui.horizontal(|ui| {
            ui.add_space(22.0);
            widgets::eyebrow(ui, &format!("{:02}", v.modules.len()), "Modules");
        });
        ui.add_space(14.0);
        let width = SIDEBAR_W - 1.0;
        let top = ui.cursor().top();
        ui.painter().hline(full.left()..=full.left() + width, top, t.hairline());
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.module_list(ui, v, width));
    }

    fn module_list(&mut self, ui: &mut Ui, v: &View, width: f32) {
        let t = Tokens::get(ui.ctx());
        ui.spacing_mut().item_spacing.y = 0.0;
        for (i, m) in v.modules.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(width, 58.0), egui::Sense::click());
            let selected = i == v.selected;
            let p = ui.painter();
            if selected || resp.hovered() {
                p.rect_filled(rect, CornerRadius::ZERO, t.sunken);
            }
            if selected {
                p.rect_filled(egui::Rect::from_min_size(rect.min, vec2(3.0, rect.height())), CornerRadius::ZERO, t.module(i));
            }
            p.hline(rect.x_range(), rect.bottom() - 0.5, t.hairline());
            let galley = widgets::caps(ui, &format!("{:02}", i), theme::HUD_SIZE, theme::mono_bold(), theme::HUD_SPACING, t.invert_fg);
            let tag = egui::Rect::from_min_size(rect.min + vec2(22.0, 10.0), galley.size() + vec2(14.0, 8.0));
            p.rect_filled(tag, CornerRadius::ZERO, t.module(i));
            p.galley_with_override_text_color(tag.center() - galley.size() / 2.0, galley, t.invert_fg);
            p.text(
                egui::pos2(tag.right() + 12.0, tag.center().y),
                egui::Align2::LEFT_CENTER,
                m.spec.title,
                FontId::new(15.0, theme::semibold()),
                if selected { t.ink_strong } else { t.ink },
            );
            p.text(
                egui::pos2(rect.left() + 22.0, rect.top() + 42.0),
                egui::Align2::LEFT_CENTER,
                m.spec.tagline,
                FontId::new(12.0, egui::FontFamily::Proportional),
                t.muted,
            );
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.acts.push(Act::Select(i));
            }
        }
    }
}

// ---- module page ---------------------------------------------------------------------------------

impl App {
    fn module_page(&mut self, ui: &mut Ui, v: &View) {
        let t = Tokens::get(ui.ctx());
        let i = v.selected;
        let m = &v.modules[i];
        if m.spec.id == "welcome" {
            return self.welcome_page(ui, v);
        }
        if m.spec.id == "community" {
            return self.community_page(ui, v);
        }
        if m.spec.id == "upgrade" {
            return self.upgrade_page(ui, v);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            widgets::tag(ui, &format!("{:02}", i), t.module(i), t.invert_fg);
            widgets::eyebrow(ui, "module", m.spec.title);
            if let Panel::Ready(out) = &m.panel {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    widgets::hud(ui, &format!("{} items", yaist_ui::output_rows(out)), t.faint);
                });
            }
        });
        ui.add_space(12.0);
        ui.label(RichText::new(m.spec.title).font(FontId::new(32.0, theme::display())).color(t.ink_strong));
        ui.label(RichText::new(m.spec.tagline).size(14.0).color(t.muted));
        ui.add_space(16.0);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for (k, a) in m.actions.iter().enumerate() {
                let kind = if k == 0 { Kind::Primary } else { Kind::Secondary };
                if widgets::button(ui, a.title, kind, 0.0).on_hover_text(a.description).clicked() {
                    if a.params.is_empty() {
                        self.acts.push(Act::Propose(a.id, Map::new()));
                    } else {
                        self.acts.push(Act::Form(Form::new(*a, &Map::new())));
                    }
                }
            }
            if widgets::button(ui, "Refresh", Kind::Secondary, 0.0).on_hover_text("Reload this panel").clicked() {
                self.acts.push(Act::Refresh(i));
            }
        });
        ui.add_space(18.0);
        if m.spec.id == "packages" {
            self.update_banner(ui, v, false);
            ui.add_space(18.0);
        }

        match &m.panel {
            Panel::Loading => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0).color(t.muted));
                    widgets::hud(ui, "loading", t.muted);
                });
            }
            Panel::Locked => {
                ui.label(RichText::new("Reading this needs the administrator password, so it isn't loaded on every visit.").color(t.muted));
                ui.add_space(8.0);
                if widgets::button(ui, &format!("Show {}", m.spec.title), Kind::Primary, 0.0).on_hover_text("Asks for your password").clicked() {
                    self.acts.push(Act::Refresh(v.selected));
                }
            }
            Panel::Failed(e) => {
                Frame::new().stroke(Stroke::new(1.0, t.bad)).inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
                    ui.label(RichText::new(e).color(t.bad));
                });
            }
            Panel::Ready(Output::Table(table)) => self.table(ui, m, table),
            Panel::Ready(Output::Facts(facts)) => facts_grid(ui, facts, 16.0),
            Panel::Ready(Output::Text(s) | Output::Done(s)) => {
                ui.label(s);
            }
        }
    }

    fn table(&mut self, ui: &mut Ui, m: &ModuleView, table: &Table) {
        let t = Tokens::get(ui.ctx());
        let full_w = ui.available_width();
        // In a narrow table the row actions go into the ··· menu, so the
        // columns keep their room.
        let visible_actions = if full_w < 760.0 { 0 } else { m.row_actions.len().min(2) };
        let menu = m.row_actions.len() > visible_actions;
        let actions_w = if m.row_actions.is_empty() { 0.0 } else { visible_actions as f32 * 90.0 + if menu { 40.0 } else { 0.0 } + 8.0 };
        let cols_w = full_w - actions_w - 28.0;
        let longest = |c: usize| table.rows.iter().take(300).map(|r| r[c].chars().count()).max().unwrap_or(0).max(table.columns[c].title.len());
        let is_badge = |c: usize| yaist_ui::is_status_column(table.columns[c].key);
        let badge_w = |c: usize| longest(c) as f32 * 7.6 + 26.0;
        // A column is readable at about two dozen characters; when not all
        // of them fit, only the essential ones are shown (unless asked).
        let readable = |c: usize| if is_badge(c) { badge_w(c) } else { longest(c).clamp(6, 24) as f32 * 7.6 + 16.0 };
        let n = table.columns.len();
        let fits = (0..n).map(readable).sum::<f32>() <= cols_w;
        let all = self.all_columns.contains(m.spec.id);
        let shown: Vec<usize> = (0..n).filter(|&c| fits || all || table.is_essential(c)).collect();
        let hidden = n - shown.len();
        // Badges get room for their longest label, the first column for its
        // content (it names the row); the rest share what is left by length.
        let first = shown[0];
        let badges: f32 = shown.iter().skip(1).filter(|&&c| is_badge(c)).map(|&c| badge_w(c)).sum();
        let flexible = shown.iter().skip(1).any(|&c| !is_badge(c));
        // Up to its content; with no other text column, all the room there is.
        let room = if flexible { cols_w * 0.45 } else { cols_w - badges };
        let first_w = if is_badge(first) { badge_w(first) } else { (longest(first).clamp(6, 40) as f32 * 7.6 + 16.0).min(room.max(120.0)) };
        let fixed: f32 = first_w + badges;
        let weight = |c: usize| longest(c).clamp(4, 40) as f32 + 1.0;
        let total: f32 = shown.iter().skip(1).filter(|&&c| !is_badge(c)).map(|&c| weight(c)).sum::<f32>().max(1.0);
        let widths: Vec<f32> = shown
            .iter()
            .map(|&c| {
                if c == first {
                    first_w
                } else if is_badge(c) {
                    badge_w(c)
                } else {
                    (cols_w - fixed).max(80.0) * weight(c) / total
                }
            })
            .collect();

        if hidden > 0 || (all && !fits) {
            let label = if hidden > 0 { format!("+ {hidden} columns") } else { "fewer columns".into() };
            ui.allocate_ui_with_layout(vec2(full_w, 22.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let toggle = ui.add(egui::Button::new(RichText::new(label).size(12.0).color(t.accent)).frame(false));
                if toggle.on_hover_text("Double-click a row to see all of its fields").clicked() {
                    if all {
                        self.all_columns.remove(m.spec.id);
                    } else {
                        self.all_columns.insert(m.spec.id);
                    }
                }
            });
        }
        let (head, _) = ui.allocate_exact_size(vec2(full_w, 34.0), egui::Sense::hover());
        let p = ui.painter();
        p.hline(head.x_range(), head.top() + 0.5, Stroke::new(1.0, t.line_strong));
        p.hline(head.x_range(), head.bottom() - 0.5, t.hairline());
        let mut x = head.left() + 14.0;
        for (&c, w) in shown.iter().zip(&widths) {
            let g = widgets::caps(ui, table.columns[c].title, theme::HUD_SIZE, theme::mono(), theme::HUD_SPACING, t.faint);
            ui.painter().galley(egui::pos2(x, head.center().y - g.size().y / 2.0), g, t.faint);
            x += w;
        }
        if table.rows.is_empty() {
            ui.add_space(18.0);
            widgets::hud(ui, "  nothing here", t.faint);
            return;
        }
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, ROW_H, table.rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &table.rows[range] {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), egui::Sense::click());
                if resp.contains_pointer() {
                    ui.painter().rect_filled(rect, CornerRadius::ZERO, t.sunken);
                }
                if resp.double_clicked() {
                    let one = Table { columns: table.columns.clone(), rows: vec![row.clone()], essential: Vec::new() };
                    self.detail = Some(Detail { table: one, actions: m.row_actions.clone() });
                }
                ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, t.hairline());
                let mut x = rect.left() + 14.0;
                for (&c, w) in shown.iter().zip(&widths) {
                    let value = &row[c];
                    let cell = egui::Rect::from_min_size(egui::pos2(x, rect.top()), vec2((w - 10.0).max(10.0), ROW_H));
                    x += w;
                    let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(cell).layout(egui::Layout::left_to_right(egui::Align::Center)));
                    cu.set_clip_rect(cell.intersect(ui.clip_rect()));
                    if is_badge(c) && !value.is_empty() {
                        widgets::badge(&mut cu, value, t.tone(tone_of(value)));
                    } else {
                        let text =
                            if c == 0 { RichText::new(value).family(theme::medium()).color(t.ink_strong) } else { RichText::new(value).color(t.ink) };
                        cu.add(egui::Label::new(text).truncate().selectable(false));
                    }
                }
                if !m.row_actions.is_empty() {
                    let area = egui::Rect::from_min_max(egui::pos2(rect.right() - actions_w, rect.top()), rect.max - vec2(8.0, 0.0));
                    let mut au = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::right_to_left(egui::Align::Center)));
                    au.spacing_mut().item_spacing.x = 6.0;
                    if menu {
                        au.menu_button(RichText::new("···").color(t.muted), |ui| {
                            for a in &m.row_actions[visible_actions..] {
                                if ui.button(a.title).clicked() {
                                    self.row_action(a, table, row);
                                    ui.close();
                                }
                            }
                        });
                    }
                    for a in m.row_actions[..visible_actions].iter().rev() {
                        if widgets::button_sized(&mut au, a.title, Kind::Secondary, 84.0, 26.0).on_hover_text(a.description).clicked() {
                            self.row_action(a, table, row);
                        }
                    }
                }
            }
        });
    }

    /// One row with all its fields, for narrow windows and long values.
    fn detail_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &self.detail else { return };
        let t = Tokens::get(ctx);
        let row = d.table.rows[0].clone();
        let modal = egui::Modal::new(egui::Id::unique("detail"))
            .frame(Frame::new().fill(t.bg).stroke(Stroke::new(1.0, t.line_strong)).shadow(theme::shadow(&t)).inner_margin(theme::PAD as i8));
        let mut chosen = None;
        let mut close = false;
        let resp = modal.show(ctx, |ui| {
            ui.set_width(520.0);
            ui.label(RichText::new(&row[0]).font(FontId::new(22.0, theme::display())).color(t.ink_strong));
            ui.add_space(12.0);
            egui::Grid::new("detail-fields").num_columns(2).spacing(vec2(18.0, 8.0)).show(ui, |ui| {
                for (col, value) in d.table.columns.iter().zip(&row) {
                    widgets::hud(ui, col.title, t.faint);
                    if yaist_ui::is_status_column(col.key) && !value.is_empty() {
                        widgets::badge(ui, value, t.tone(tone_of(value)));
                    } else {
                        ui.add(egui::Label::new(RichText::new(if value.is_empty() { "–" } else { value }).color(t.ink)).wrap());
                    }
                    ui.end_row();
                }
            });
            ui.add_space(16.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                for a in &d.actions {
                    if widgets::button(ui, a.title, Kind::Secondary, 0.0).on_hover_text(a.description).clicked() {
                        chosen = Some(*a);
                    }
                }
                if widgets::button(ui, "Close", Kind::Primary, 0.0).clicked() {
                    close = true;
                }
            });
        });
        if let Some(a) = chosen {
            let table = d.table.clone();
            self.detail = None;
            self.row_action(&a, &table, &row);
        } else if close || resp.should_close() {
            self.detail = None;
        }
    }

    /// The start screen: greeting, starting points, questions to try, the
    /// system at a glance and community links.
    fn welcome_page(&mut self, ui: &mut Ui, v: &View) {
        let t = Tokens::get(ui.ctx());
        let d = &v.distro_info;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            // Artwork and text centred against each other, so neither leaves a gap.
            // A fixed-height row: centring inside the scroll area would use its
            // whole height.
            let art = welcome::welcome_art();
            let art_has_brand = art.as_ref().is_some_and(|(_, brand)| *brand);
            let hero_h = if art_has_brand { 180.0 } else { 220.0 };
            ui.allocate_ui_with_layout(vec2(ui.available_width(), hero_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                match &art {
                    Some((uri, _)) => {
                        ui.add(egui::Image::new(uri.clone()).max_size(vec2(240.0, 180.0)));
                    }
                    None => {
                        let (rect, _) = ui.allocate_exact_size(vec2(240.0, 170.0), egui::Sense::hover());
                        crate::art::chameleon(ui.painter(), rect, &t);
                    }
                }
                ui.add_space(28.0);
                ui.vertical(|ui| {
                    widgets::hud(ui, "Welcome to", t.muted);
                    ui.add_space(4.0);
                    // The openSUSE artwork carries the brand, so the title shows
                    // the release; with the drawn chameleon it shows both.
                    let name = if d.pretty.is_empty() { "openSUSE".to_owned() } else { d.pretty.clone() };
                    let (brand, release) = match name.split_once(' ') {
                        Some((b, r)) => (b.to_owned(), r.to_owned()),
                        None => (String::new(), name.clone()),
                    };
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        if let Some(uri) = welcome::distribution_logo(welcome::LOGO_SQUARE) {
                            ui.add(egui::Image::new(uri).fit_to_exact_size(vec2(40.0, 40.0))).on_hover_text(&v.distro);
                        }
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            let font = FontId::new(38.0, theme::display());
                            if !art_has_brand && !brand.is_empty() {
                                ui.label(RichText::new(&brand).font(font.clone()).color(t.accent));
                            }
                            ui.label(RichText::new(&release).font(font).color(t.ink_strong));
                        });
                    });
                    ui.add_space(8.0);
                    let (mode, explanation) = welcome::mode(d);
                    let support = welcome::support(d, yaist_core::date::today());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        widgets::tag(ui, mode, t.invert_bg, t.invert_fg).on_hover_text(explanation);
                        match (&support, support.label()) {
                            (welcome::Support::EndOfLife { .. }, Some(label)) => {
                                let resp = widgets::tag(ui, label, t.bad, t.invert_fg).interact(egui::Sense::click());
                                if resp.on_hover_text("See upgrade options").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    if let Some(i) = v.modules.iter().position(|m| m.spec.id == "upgrade") {
                                        self.acts.push(Act::Select(i));
                                    }
                                }
                            }
                            (_, Some(label)) => {
                                widgets::tag(ui, label, t.invert_bg, t.invert_fg);
                            }
                            _ => {}
                        }
                    });
                    ui.add_space(4.0);
                    let sentence = support.sentence();
                    if !sentence.is_empty() {
                        let colour = match support {
                            welcome::Support::EndOfLife { .. } => t.bad,
                            welcome::Support::Until { soon: true, .. } => t.warn,
                            _ => t.ink,
                        };
                        ui.label(RichText::new(sentence).size(13.0).color(colour));
                    }
                });
            });
            ui.add_space(18.0);
            self.update_banner(ui, v, true);
            ui.add_space(22.0);

            widgets::eyebrow(ui, "01", "Get started");
            ui.add_space(10.0);
            let items = welcome::start_items(d);
            let cols = if ui.available_width() > 720.0 { 3 } else { 2 };
            let card_w = (ui.available_width() / cols as f32).floor();
            for (row, chunk) in items.chunks(cols).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (k, item) in chunk.iter().enumerate() {
                        let n = row * cols + k + 1;
                        let (rect, resp) = ui.allocate_exact_size(vec2(card_w, 132.0), egui::Sense::click());
                        let p = ui.painter();
                        p.rect(rect, CornerRadius::ZERO, if resp.hovered() { t.sunken } else { t.bg }, t.hairline(), egui::StrokeKind::Inside);
                        let mut cu =
                            ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(16.0)).layout(egui::Layout::top_down(egui::Align::Min)));
                        widgets::tag(&mut cu, &format!("{n:02}"), t.module(n), t.invert_fg);
                        cu.add_space(8.0);
                        cu.label(RichText::new(item.title).family(theme::semibold()).size(16.0).color(t.ink_strong));
                        cu.label(RichText::new(item.text).size(13.0).color(t.muted));
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            self.start(item.start, v);
                        }
                    }
                });
            }
            ui.add_space(24.0);

            widgets::eyebrow(ui, "02", "Try asking");
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                for s in welcome::suggestions(d) {
                    if widgets::chip(ui, s).clicked() {
                        self.acts.push(Act::Submit(s.to_owned()));
                    }
                }
            });
            ui.add_space(24.0);

            widgets::eyebrow(ui, "03", "Your system");
            ui.add_space(10.0);
            match &v.modules[v.selected].panel {
                Panel::Ready(Output::Facts(f)) => facts_grid(ui, f, 6.0),
                Panel::Failed(e) => {
                    ui.label(RichText::new(e).color(t.bad));
                }
                _ => {
                    widgets::hud(ui, "loading", t.faint);
                }
            }
            ui.add_space(24.0);

            widgets::eyebrow(ui, "04", "Join the community");
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                for title in welcome::FEATURED {
                    if let Some(l) = welcome::link(title) {
                        if widgets::button(ui, &format!("{} ↗", l.title), Kind::Secondary, 0.0).on_hover_text(l.text).clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(l.url));
                        }
                    }
                }
                if widgets::button(ui, "All community places", Kind::Primary, 0.0).clicked() {
                    if let Some(i) = v.modules.iter().position(|m| m.spec.id == "community") {
                        self.acts.push(Act::Select(i));
                    }
                }
            });
            ui.add_space(24.0);
        });
    }

    /// Header badge: solid and coloured while updates wait, so it is seen
    /// from every screen; opens Packages.
    fn update_badge(&mut self, ui: &mut Ui, v: &View) {
        let t = Tokens::get(ui.ctx());
        if v.updates.reboot_needed() {
            let resp = widgets::tag(ui, "reboot to apply", t.warn, t.invert_fg).interact(egui::Sense::click());
            if resp.on_hover_text("Installed changes wait for a reboot").clicked() {
                self.acts.push(Act::Propose(yaist_ui::REBOOT_ACTION, Map::new()));
            }
            ui.add_space(8.0);
        }
        let (level, badge, ..) = update_summary(&v.updates, v.distro_info.transactional);
        let resp = match level {
            UpdateLevel::Security => widgets::tag(ui, &badge, t.bad, t.invert_fg),
            UpdateLevel::Available => widgets::tag(ui, &badge, t.warn, t.invert_fg),
            UpdateLevel::UpToDate => widgets::badge(ui, &badge, t.good),
            UpdateLevel::Unknown => widgets::badge(ui, &badge, t.faint),
        };
        let resp = resp.interact(egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Open Packages");
        if resp.clicked() {
            if let Some(i) = v.modules.iter().position(|m| m.spec.id == "packages") {
                self.acts.push(Act::Select(i));
            }
        }
    }

    /// The update banner: large and coloured when updates wait, a quiet line
    /// when the system is up to date.
    fn update_banner(&mut self, ui: &mut Ui, v: &View, with_list_button: bool) {
        let t = Tokens::get(ui.ctx());
        if let Some((headline, detail)) = reboot_notice(&v.updates, v.distro_info.transactional) {
            let frame = Frame::new().fill(t.sunken).stroke(t.hairline()).inner_margin(Margin { left: 20, right: 16, top: 16, bottom: 16 });
            let resp = frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                widgets::hud(ui, "pending", t.warn);
                ui.label(RichText::new(&headline).font(FontId::new(22.0, theme::display())).color(t.ink_strong));
                ui.label(RichText::new(&detail).size(13.0).color(t.muted));
                ui.add_space(10.0);
                if widgets::button(ui, "Reboot now", Kind::Danger, 140.0).clicked() {
                    self.acts.push(Act::Propose(yaist_ui::REBOOT_ACTION, Map::new()));
                }
            });
            let r = resp.response.rect;
            ui.painter().rect_filled(egui::Rect::from_min_size(r.min, vec2(4.0, r.height())), CornerRadius::ZERO, t.warn);
            ui.add_space(12.0);
        }
        let (level, _, headline, detail) = update_summary(&v.updates, v.distro_info.transactional);
        let colour = match level {
            UpdateLevel::Security => t.bad,
            UpdateLevel::Available => t.warn,
            UpdateLevel::UpToDate => t.good,
            UpdateLevel::Unknown => t.faint,
        };
        let big = matches!(level, UpdateLevel::Available | UpdateLevel::Security);
        let pad = if big { 18 } else { 12 };
        let frame = Frame::new().fill(t.sunken).stroke(t.hairline()).inner_margin(Margin { left: 20, right: 16, top: pad, bottom: pad });
        let resp = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            if big {
                widgets::hud(ui, if level == UpdateLevel::Security { "security updates" } else { "updates" }, colour);
                ui.add_space(2.0);
                ui.label(RichText::new(&headline).font(FontId::new(24.0, theme::display())).color(t.ink_strong));
                ui.label(RichText::new(&detail).size(13.0).color(t.muted));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if widgets::button(ui, "Install all updates", Kind::Primary, 180.0).clicked() {
                        self.acts.push(Act::Propose(UPDATE_ACTION, Map::new()));
                    }
                    // Leap's patches can be limited to security fixes.
                    let d = &v.distro_info;
                    if level == UpdateLevel::Security && !d.rolling && !d.transactional {
                        let b = widgets::button(ui, "Security updates only", Kind::Secondary, 0.0);
                        if b.on_hover_text("zypper patch --category security").clicked() {
                            self.acts.push(Act::Propose(yaist_ui::SECURITY_ACTION, Map::new()));
                        }
                    }
                    if with_list_button && widgets::button(ui, "See list", Kind::Secondary, 0.0).clicked() {
                        if let Some(i) = v.modules.iter().position(|m| m.spec.id == "packages") {
                            self.acts.push(Act::Select(i));
                        }
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&headline).family(theme::semibold()).size(15.0).color(t.ink_strong));
                        if !detail.is_empty() {
                            ui.label(RichText::new(&detail).size(13.0).color(t.muted));
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can_check = !matches!(v.updates, UpdateStatus::Checking);
                        if can_check && widgets::button_sized(ui, "Check again", Kind::Secondary, 0.0, 28.0).clicked() {
                            self.acts.push(Act::CheckUpdates);
                        }
                    });
                });
            }
        });
        // A coloured rail down the left edge.
        let r = resp.response.rect;
        ui.painter().rect_filled(egui::Rect::from_min_size(r.min, vec2(4.0, r.height())), CornerRadius::ZERO, colour);
    }

    /// Upgrade and migration targets as large tiles, like the Community page.
    fn upgrade_page(&mut self, ui: &mut Ui, v: &View) {
        let t = Tokens::get(ui.ctx());
        let i = v.selected;
        let m = &v.modules[i];
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                widgets::tag(ui, &format!("{i:02}"), t.module(i), t.invert_fg);
                widgets::eyebrow(ui, "module", "Upgrade");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::button_sized(ui, "Refresh", Kind::Secondary, 0.0, 28.0).clicked() {
                        self.acts.push(Act::Refresh(i));
                    }
                });
            });
            ui.add_space(12.0);
            ui.label(RichText::new("Upgrade or migrate").font(FontId::new(32.0, theme::display())).color(t.ink_strong));
            ui.label(
                RichText::new(format!(
                    "You run {}. Repositories that may not work on the target are disabled first, and a snapshot lets you roll back.",
                    v.distro
                ))
                .size(14.0)
                .color(t.muted),
            );
            ui.add_space(18.0);
            let table = match &m.panel {
                Panel::Ready(Output::Table(table)) => table.clone(),
                Panel::Ready(Output::Text(s)) => {
                    ui.label(RichText::new(s).color(t.ink));
                    return;
                }
                Panel::Failed(e) => {
                    ui.label(RichText::new(e).color(t.bad));
                    return;
                }
                _ => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0).color(t.muted));
                        widgets::hud(ui, "checking what this system can move to", t.muted);
                    });
                    return;
                }
            };
            let cols = if ui.available_width() > 760.0 { 2 } else { 1 };
            let w = (ui.available_width() / cols as f32).floor();
            let cell = |row: &[String], key: &str| table.cell(row, key).unwrap_or("").to_owned();
            for chunk in table.rows.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for row in chunk {
                        let (target, title, kind, status) = (cell(row, "target"), cell(row, "title"), cell(row, "kind"), cell(row, "status"));
                        let (description, notes) = (cell(row, "description"), cell(row, "notes"));
                        let available = status != "not available";
                        let colour = match kind.as_str() {
                            "upgrade" => t.module(0),
                            "migrate" => t.module(1),
                            _ => t.module(3),
                        };
                        let frame = Frame::new().fill(t.bg).stroke(t.hairline()).inner_margin(Margin::same(18));
                        let (rect, _) = ui.allocate_exact_size(vec2(w, 230.0), egui::Sense::hover());
                        let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
                        frame.show(&mut cu, |ui| {
                            ui.set_width(w - 36.0);
                            ui.set_height(194.0);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                widgets::tag(ui, &kind, colour, t.invert_fg);
                                widgets::badge(ui, &status, t.tone(tone_of(&status)));
                            });
                            ui.add_space(8.0);
                            ui.label(RichText::new(&title).font(FontId::new(22.0, theme::display())).color(if available {
                                t.ink_strong
                            } else {
                                t.muted
                            }));
                            ui.label(RichText::new(&description).size(13.5).color(t.ink));
                            if !notes.is_empty() {
                                ui.add_space(2.0);
                                ui.add(egui::Label::new(RichText::new(&notes).size(12.0).color(t.faint)).truncate());
                            }
                            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                                let label = match (kind.as_str(), target.as_str()) {
                                    (_, "dup") => "Upgrade now".to_owned(),
                                    ("upgrade", _) => format!("Upgrade to {}", target.trim_start_matches("leap:")),
                                    ("support", _) => "Register and migrate".to_owned(),
                                    _ => "Migrate".to_owned(),
                                };
                                let kind_btn = if available { Kind::Primary } else { Kind::Secondary };
                                let resp = ui.add_enabled_ui(available, |ui| widgets::button(ui, &label, kind_btn, 180.0)).inner;
                                if resp.on_hover_text(&notes).clicked() {
                                    if target == "sle" {
                                        if let Some(spec) = self.session.registry.get("upgrade.to_sle").copied() {
                                            self.acts.push(Act::Form(Form::new(spec, &Map::new())));
                                        }
                                    } else {
                                        let mut raw = Map::new();
                                        raw.insert("target".into(), Value::String(target.clone()));
                                        self.acts.push(Act::Propose("upgrade.migrate", raw));
                                    }
                                }
                            });
                        });
                    }
                });
            }
        });
    }

    /// The community places as grouped cards that open in the browser.
    fn community_page(&mut self, ui: &mut Ui, v: &View) {
        let t = Tokens::get(ui.ctx());
        let i = v.selected;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                widgets::tag(ui, &format!("{i:02}"), t.module(i), t.invert_fg);
                widgets::eyebrow(ui, "module", "Community");
            });
            ui.add_space(12.0);
            ui.label(RichText::new("openSUSE community").font(FontId::new(32.0, theme::display())).color(t.ink_strong));
            ui.label(
                RichText::new("Talk to people, thank them, find events and help, and see how openSUSE is built and tested.")
                    .size(14.0)
                    .color(t.muted),
            );
            ui.add_space(18.0);
            let cols = if ui.available_width() > 720.0 { 3 } else { 2 };
            for (g, group) in welcome::GROUPS.iter().enumerate() {
                widgets::eyebrow(ui, &format!("{:02}", g + 1), group);
                ui.add_space(8.0);
                let links: Vec<_> = welcome::LINKS.iter().filter(|l| l.group == *group).collect();
                let w = (ui.available_width() / cols as f32).floor();
                for chunk in links.chunks(cols) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for l in chunk {
                            let (rect, resp) = ui.allocate_exact_size(vec2(w, 76.0), egui::Sense::click());
                            let hovered = resp.hovered();
                            let p = ui.painter();
                            p.rect(rect, CornerRadius::ZERO, if hovered { t.sunken } else { t.bg }, t.hairline(), egui::StrokeKind::Inside);
                            p.text(
                                rect.min + vec2(16.0, 14.0),
                                egui::Align2::LEFT_TOP,
                                format!("{} ↗", l.title),
                                FontId::new(15.0, theme::semibold()),
                                if hovered { t.accent } else { t.ink_strong },
                            );
                            let text = ui.painter().layout(l.text.to_owned(), FontId::new(12.5, egui::FontFamily::Proportional), t.muted, w - 32.0);
                            ui.painter().galley(rect.min + vec2(16.0, 38.0), text, t.muted);
                            if resp.on_hover_text(l.url).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(l.url));
                            }
                        }
                    });
                }
                ui.add_space(18.0);
            }
        });
    }

    fn start(&mut self, start: Start, v: &View) {
        match start {
            Start::Module(id) => {
                if let Some(i) = v.modules.iter().position(|m| m.spec.id == id) {
                    self.acts.push(Act::Select(i));
                }
            }
            Start::Action(id) => {
                let Some(spec) = self.session.registry.get(id).copied() else { return };
                if spec.params.iter().any(|p| p.required) {
                    self.acts.push(Act::Form(Form::new(spec, &Map::new())));
                } else {
                    self.acts.push(Act::Propose(spec.id, Map::new()));
                }
            }
        }
    }

    fn row_action(&mut self, a: &yaist_core::ActionSpec, table: &Table, row: &[String]) {
        let raw = row_args(a, table, row);
        if row_action_is_direct(a, table) {
            self.acts.push(Act::Propose(a.id, raw));
        } else {
            self.acts.push(Act::Form(Form::new(*a, &raw)));
        }
    }
}

fn facts_grid(ui: &mut Ui, facts: &[(String, String)], gap: f32) {
    let t = Tokens::get(ui.ctx());
    Frame::new().stroke(t.hairline()).inner_margin(Margin::symmetric(16, 14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::Grid::new(ui.next_auto_id()).num_columns(2).spacing(vec2(24.0, gap / 2.0 + 4.0)).show(ui, |ui| {
            for (k, val) in facts {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| widgets::hud(ui, k, t.faint));
                if val.contains('\n') {
                    ui.label(RichText::new(val).monospace().size(12.0).color(t.ink));
                } else {
                    ui.label(RichText::new(val).color(t.ink_strong));
                }
                ui.end_row();
            }
        });
    });
}

// ---- conversation --------------------------------------------------------------------------------

impl App {
    fn conversation(&mut self, ui: &mut Ui, v: &View) {
        let t = Tokens::get(ui.ctx());
        let full = ui.max_rect();
        // Nothing asked yet: only the start notice (and maybe warnings).
        let fresh = v.conversation.iter().all(|e| matches!(e, Entry::Notice(_) | Entry::Error(_))) && self.input.is_empty();
        ui.painter().vline(full.left() + 0.5, full.y_range(), t.hairline());

        egui::Panel::top("conv-head").frame(Frame::new().inner_margin(Margin { left: 20, right: 16, top: 18, bottom: 12 })).show(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::eyebrow(ui, "ai", "Conversation");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::button_sized(ui, "Clear", Kind::Secondary, 0.0, 26.0).clicked() {
                        self.acts.push(Act::Submit("/clear".into()));
                    }
                });
            });
        });

        egui::Panel::bottom("conv-input").frame(Frame::new().inner_margin(Margin { left: 20, right: 16, top: 10, bottom: 16 })).show(ui, |ui| {
            if let Some(what) = &v.busy {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0).color(t.accent));
                    widgets::hud(ui, what, t.muted);
                });
                ui.add_space(6.0);
            } else if v.pending {
                widgets::hud(ui, "a change is waiting for you above", t.warn);
                ui.add_space(6.0);
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (send_w, h) = (84.0, theme::CONTROL_H + 6.0);
                let (field, _) = ui.allocate_exact_size(vec2(ui.available_width() - send_w - 8.0, h), egui::Sense::hover());
                // Until the first request: an accent outline, pulsing three
                // times at start, so the conversation is noticed.
                let stroke = if fresh {
                    let age = self.opened.elapsed().as_secs_f32();
                    if age < 3.0 {
                        ui.ctx().request_repaint();
                        let a = (age * std::f32::consts::PI).sin().abs();
                        Stroke::new(1.0 + 2.0 * a, t.accent.gamma_multiply(0.45 + 0.55 * a))
                    } else {
                        Stroke::new(1.5, t.accent)
                    }
                } else {
                    Stroke::new(1.0, t.line_strong)
                };
                ui.painter().rect(field, CornerRadius::ZERO, t.sunken, stroke, egui::StrokeKind::Inside);
                let mut fu = ui.new_child(
                    egui::UiBuilder::new().max_rect(field.shrink2(vec2(12.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                let edit = fu.add(
                    egui::TextEdit::singleline(&mut self.input)
                        .hint_text(RichText::new("Ask YAiST, or type /help").color(t.faint))
                        .frame(Frame::NONE)
                        .desired_width(field.width() - 24.0)
                        .font(FontId::new(14.0, egui::FontFamily::Proportional)),
                );
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let send = widgets::button_sized(ui, "Send", Kind::Primary, send_w, h).clicked();
                if (enter || send) && !self.input.trim().is_empty() {
                    self.acts.push(Act::Submit(std::mem::take(&mut self.input)));
                    edit.request_focus();
                }
            });
        });

        egui::CentralPanel::default().frame(Frame::new().inner_margin(Margin { left: 20, right: 12, top: 0, bottom: 0 })).show(ui, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                if fresh {
                    self.greeting(ui, v);
                }
                for e in &v.conversation {
                    self.entry(ui, e);
                    ui.add_space(10.0);
                }
            });
        });
    }

    /// The empty conversation invites a first request, with examples that
    /// send themselves.
    fn greeting(&mut self, ui: &mut Ui, v: &View) {
        let t = Tokens::get(ui.ctx());
        ui.add_space(18.0);
        ui.label(RichText::new("How can I help?").font(FontId::new(28.0, theme::display())).color(t.ink_strong));
        ui.label(RichText::new("Ask in plain language. YAiST picks the right tool and shows every change before it runs.").size(13.5).color(t.muted));
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for q in yaist_ui::welcome::prompts(v.modules[v.selected].spec.id, &v.distro_info).into_iter().take(4) {
                if widgets::chip(ui, q).clicked() {
                    self.acts.push(Act::Submit(q.to_owned()));
                }
            }
        });
        ui.add_space(18.0);
    }

    fn entry(&mut self, ui: &mut Ui, e: &Entry) {
        let t = Tokens::get(ui.ctx());
        match e {
            Entry::User(s) => {
                widgets::hud(ui, "you", t.faint);
                Frame::new().fill(t.sunken).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(s).size(14.0).color(t.ink_strong));
                });
            }
            Entry::Command(s) => {
                Frame::new().fill(t.sunken).stroke(t.hairline()).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(format!("› {s}")).family(theme::mono()).size(13.0).color(t.ink_strong));
                });
            }
            Entry::AgentRan { command, shell, ok } => {
                let (mark, color) = if *ok { ("✓", t.faint) } else { ("✗", t.bad) };
                ui.label(RichText::new(format!("{mark} ran {command}")).family(theme::mono()).size(12.0).color(color));
                for c in shell {
                    ui.label(RichText::new(format!("  $ {c}")).family(theme::mono()).size(11.5).color(t.faint));
                }
            }
            Entry::Assistant { text, elapsed } => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    wordmark(ui, 13.0);
                    ui.add_space(10.0);
                    widgets::hud(ui, &format!("{:.1} s", elapsed.as_secs_f32()), t.faint);
                });
                ui.label(RichText::new(text).size(14.0).color(t.ink));
            }
            Entry::Plan(p) => self.plan_card(ui, p),
            Entry::Notes(n) => self.notes_card(ui, n),
            Entry::Result { title, shell, output } => {
                widgets::hud(ui, title, t.faint);
                shell_block(ui, shell);
                match output {
                    Output::Table(table) => compact_table(ui, table),
                    Output::Facts(f) => facts_grid(ui, f, 4.0),
                    Output::Text(s) | Output::Done(s) => {
                        let text = if s.contains('\n') { RichText::new(s).monospace().size(12.0) } else { RichText::new(s) };
                        ui.label(text.color(t.ink));
                    }
                }
            }
            Entry::Error(s) => {
                ui.label(RichText::new(format!("✗ {s}")).color(t.bad));
            }
            Entry::Notice(s) => {
                let text = if s.contains('\n') { RichText::new(s).family(theme::mono()).size(12.0) } else { RichText::new(s).size(13.0) };
                ui.label(text.color(t.muted));
            }
        }
    }

    /// A proposed change. Everything on it comes from the action's definition
    /// and validated arguments, never from model-written text.
    fn plan_card(&mut self, ui: &mut Ui, p: &Plan) {
        let t = Tokens::get(ui.ctx());
        let risk = t.risk(p.spec.risk);
        let stroke = if p.status == PlanStatus::Pending { Stroke::new(1.0, t.line_strong) } else { t.hairline() };
        Frame::new().fill(t.bg).stroke(stroke).inner_margin(Margin::same(14)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                widgets::tag(ui, "plan", risk, t.invert_fg);
                widgets::badge(ui, p.spec.risk.label(), risk);
                ui.label(RichText::new(p.spec.title).family(theme::semibold()).size(16.0).color(t.ink_strong));
            });
            ui.add_space(6.0);
            Frame::new().fill(t.sunken).inner_margin(Margin::symmetric(10, 7)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(&p.command).family(theme::mono()).size(12.5).color(t.ink_strong));
            });
            if !p.spec.params.is_empty() {
                ui.add_space(4.0);
                egui::Grid::new(("plan", p.id)).num_columns(2).spacing(vec2(16.0, 4.0)).show(ui, |ui| {
                    for param in p.spec.params {
                        widgets::hud(ui, &param.name.replace('_', " "), t.faint);
                        let v = p.args.shown(&p.spec, param.name);
                        ui.label(RichText::new(if v.is_empty() { "default".into() } else { v }).color(t.ink));
                        ui.end_row();
                    }
                });
            }
            if !p.shell.is_empty() {
                ui.add_space(4.0);
                widgets::hud(ui, "in a terminal", t.faint);
                shell_block(ui, &p.shell);
            }
            ui.add_space(4.0);
            ui.label(RichText::new(p.spec.description).size(12.5).color(t.muted));
            ui.add_space(8.0);
            match &p.status {
                PlanStatus::Pending => {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let kind = if p.spec.risk == Risk::Dangerous { Kind::Danger } else { Kind::Primary };
                        if widgets::button(ui, "Confirm", kind, 120.0).clicked() {
                            self.acts.push(Act::Confirm(p.id));
                        }
                        if widgets::button(ui, "Edit", Kind::Secondary, 0.0).clicked() {
                            self.acts.push(Act::Form(Form::edit(p)));
                        }
                        if widgets::button(ui, "Cancel", Kind::Secondary, 0.0).clicked() {
                            self.acts.push(Act::Cancel(p.id));
                        }
                    });
                }
                PlanStatus::Running => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(12.0).color(t.muted));
                        widgets::hud(ui, "running", t.muted);
                        // The latest line says what is happening right now.
                        if let Some(last) = p.log.last() {
                            ui.add(egui::Label::new(RichText::new(last).family(theme::mono()).size(11.5).color(t.muted)).truncate());
                        }
                    });
                    if !p.log.is_empty() {
                        ui.add_space(4.0);
                        log_block(ui, p);
                    }
                    self.notes_offer(ui, p);
                }
                PlanStatus::Done(m) => {
                    status_line(ui, "done", t.good, m, t.ink);
                    // Second thoughts after an upgrade: back to how it was.
                    if let Some(n) = yaist_ui::snapshot_before(p) {
                        ui.add_space(6.0);
                        if widgets::badge_button(ui, &format!("Roll back to snapshot {n}"), None)
                            .on_hover_text("Proposes booting the snapshot taken before the upgrade; you confirm it first")
                            .clicked()
                        {
                            self.acts.push(rollback(n));
                        }
                    }
                }
                PlanStatus::Failed(m) => {
                    status_line(ui, "failed", t.bad, m, t.bad);
                    // Only when the user wants to: quiet links, not a prompt. The
                    // likelier place comes first, with the reason.
                    if yaist_ui::reportable(p) {
                        let (likely, why) = yaist_ui::report_suggestion(m);
                        let short = yaist_ui::distro_short(&self.session.state().distro);
                        let mut links = [("opensuse", format!("Report a {short} bug")), ("yaist", "Report a YAiST issue".to_owned())];
                        if likely == "yaist" {
                            links.reverse();
                        }
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            for (i, (target, label)) in links.iter().enumerate() {
                                // The suggested one in the plan colour, the other outlined.
                                let fill = (i == 0 && likely != "none").then(|| t.risk(Risk::Change));
                                let b = widgets::badge_button(ui, label, fill);
                                if b.on_hover_text("Opens the report prefilled in your browser; complete and submit it there").clicked() {
                                    if let Some(raw) = yaist_ui::report_args(p, target) {
                                        self.acts.push(Act::Propose(yaist_ui::REPORT_ACTION, raw));
                                    }
                                }
                            }
                        });
                        // The reasoning as a tip: a tag and italics in the warning colour.
                        let tip =
                            if likely == "none" { format!("Probably not a bug: {why}.") } else { format!("Suggested: {} ({why}).", links[0].1) };
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            widgets::badge(ui, "tip", t.warn);
                            ui.label(RichText::new(tip).size(12.0).italics().color(t.warn));
                        });
                    }
                    // Ways forward: the same again (a mirror syncing, the network
                    // back), or after an upgrade, the snapshot taken before it.
                    // An upgrade that stopped part-way continues where it stopped;
                    // one that changed nothing has nothing to roll back.
                    let verdict = yaist_ui::verdict(p);
                    let (label, tip) = if verdict == Some("incomplete") && p.spec.id == "upgrade.migrate" {
                        ("Continue the upgrade", "Proposes running the upgrade again: it picks up where it stopped, with the downloads kept")
                    } else {
                        ("Retry", "Proposes the same change again")
                    };
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if widgets::button(ui, label, Kind::Secondary, 0.0).on_hover_text(tip).clicked() {
                            self.acts.push(Act::Retry(p.id));
                        }
                        if let Some(n) = yaist_ui::snapshot_before(p).filter(|_| verdict != Some("unchanged")) {
                            if widgets::button(ui, &format!("Roll back to snapshot {n}"), Kind::Secondary, 0.0)
                                .on_hover_text("Proposes booting the snapshot taken before the upgrade; you confirm it first")
                                .clicked()
                            {
                                self.acts.push(rollback(n));
                            }
                        }
                    });
                }
                PlanStatus::Cancelled => status_line(ui, "cancelled", t.faint, "", t.faint),
            }
            if !p.log.is_empty() && p.status != PlanStatus::Running {
                ui.add_space(6.0);
                ui.collapsing(RichText::new(format!("OUTPUT · {} LINES", p.log.len())).family(theme::mono()).size(11.0).color(t.faint), |ui| {
                    log_block(ui, p);
                });
            }
            if !p.executed.is_empty() {
                ui.add_space(6.0);
                ui.collapsing(RichText::new("WHAT YAiST RAN").family(theme::mono()).size(11.0).color(t.faint), |ui| {
                    for c in &p.executed {
                        ui.label(RichText::new(c).family(theme::mono()).size(11.5).color(t.muted));
                    }
                });
            }
        });
    }

    /// While an upgrade to a new release runs: what's new in it, if the user
    /// wants to hear it. Once, and quietly, like the report links.
    fn notes_offer(&mut self, ui: &mut Ui, p: &Plan) {
        if p.spec.id != "upgrade.migrate" || self.no_notes.contains(&p.id) {
            return;
        }
        let Some(target) = release_notes::Target::of_upgrade(p.args.str("target")) else { return };
        let release = target.name();
        let told = self.session.state().conversation.iter().any(|e| matches!(e, Entry::Notes(n) if n.release == release));
        if told {
            return;
        }
        let t = Tokens::get(ui.ctx());
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            widgets::badge(ui, "while you wait", t.accent);
            ui.label(RichText::new(format!("Hear what's new in {release}?")).size(13.0).color(t.ink));
            if widgets::badge_button(ui, "Tell me", Some(t.risk(Risk::Change)))
                .on_hover_text("The release notes' entries for this system, with links, and a short summary")
                .clicked()
            {
                self.acts.push(Act::WhatsNew(target.clone()));
            }
            if widgets::badge_button(ui, "Not now", None).clicked() {
                self.no_notes.insert(p.id);
            }
        });
    }

    /// What's new in a release: the entries for this system, each a link,
    /// and the full notes online and on disk.
    fn notes_card(&mut self, ui: &mut Ui, n: &release_notes::Notes) {
        let t = Tokens::get(ui.ctx());
        Frame::new().fill(t.bg).stroke(t.hairline()).inner_margin(Margin::same(14)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                widgets::tag(ui, "what's new", t.accent, t.invert_fg);
                ui.label(RichText::new(&n.release).family(theme::semibold()).size(16.0).color(t.ink_strong));
            });
            ui.add_space(4.0);
            ui.label(RichText::new(format!("From the {}.", n.source)).size(12.5).color(t.muted));
            if let Some(c) = &n.caveat {
                ui.label(RichText::new(c).size(12.5).italics().color(t.warn));
            }
            if !n.entries.is_empty() {
                ui.add_space(8.0);
                widgets::hud(ui, &format!("{} of {} entries concern this system", n.entries.len(), n.total), t.faint);
                ui.add_space(4.0);
                for e in &n.entries {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let link = ui.add(egui::Link::new(RichText::new(&e.title).size(13.5).color(t.ink)));
                        if link.on_hover_text(&e.link).clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(&e.link));
                        }
                        ui.label(RichText::new(format!("· {}", e.why)).size(12.0).color(t.faint));
                    });
                }
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if widgets::badge_button(ui, "Read all online ↗", None).on_hover_text(&n.online).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(&n.online));
                }
                if let Some(local) = &n.local {
                    if widgets::badge_button(ui, "Open the copy on this machine", None).on_hover_text(local).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(format!("file://{local}")));
                    }
                }
            });
        });
    }

    /// A question from the running change, e.g. a package conflict. It can't
    /// be dismissed: the change waits for the answer, and cancelling is one
    /// of the choices.
    fn question_dialog(&mut self, ctx: &egui::Context, asked: &Asked) {
        let t = Tokens::get(ctx);
        let q = &asked.question;
        if self.choice.0 != asked.id {
            let default = q.choices.iter().position(|c| c.default).unwrap_or(q.choices.len().saturating_sub(1));
            self.choice = (asked.id, default);
        }
        let modal = egui::Modal::new(egui::Id::unique("question").with(asked.id))
            .frame(Frame::new().fill(t.bg).stroke(Stroke::new(1.0, t.line_strong)).shadow(theme::shadow(&t)).inner_margin(theme::PAD as i8));
        modal.show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(RichText::new(&q.title).font(FontId::new(22.0, theme::display())).color(t.ink_strong));
            ui.add_space(8.0);
            if !q.text.is_empty() {
                Frame::new().fill(t.sunken).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        ui.label(RichText::new(&q.text).family(theme::mono()).size(12.5).color(t.ink));
                    });
                });
                ui.add_space(12.0);
            }
            for (i, c) in q.choices.iter().enumerate() {
                ui.radio_value(&mut self.choice.1, i, RichText::new(&c.label).size(13.5).color(t.ink));
                ui.add_space(2.0);
            }
            ui.add_space(12.0);
            if widgets::button(ui, "Continue", Kind::Primary, 120.0).clicked() {
                if let Some(c) = q.choices.get(self.choice.1) {
                    self.acts.push(Act::Answer(asked.id, c.value.clone()));
                }
            }
        });
    }

    fn form_dialog(&mut self, ctx: &egui::Context) {
        let Some(form) = &mut self.form else { return };
        let t = Tokens::get(ctx);
        let modal = egui::Modal::new(egui::Id::unique("form").with(form.spec.id))
            .frame(Frame::new().fill(t.bg).stroke(Stroke::new(1.0, t.line_strong)).shadow(theme::shadow(&t)).inner_margin(theme::PAD as i8));
        let mut submit = false;
        let mut close = false;
        let resp = modal.show(ctx, |ui| {
            ui.set_width(460.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.label(RichText::new(form.spec.title).font(FontId::new(22.0, theme::display())).color(t.ink_strong));
                widgets::badge(ui, form.spec.risk.label(), t.risk(form.spec.risk));
            });
            ui.add_space(4.0);
            ui.label(RichText::new(form.spec.description).size(13.0).color(t.muted));
            ui.add_space(14.0);
            for (i, p) in form.spec.params.iter().enumerate() {
                let label = format!("{}{}", p.name.replace('_', " "), if p.required { "" } else { " · optional" });
                widgets::hud(ui, &label, t.faint);
                ui.add_space(2.0);
                match p.kind {
                    ParamKind::Bool => {
                        let mut on = form.values[i] == "true";
                        if ui.checkbox(&mut on, RichText::new(p.help).color(t.ink)).changed() {
                            form.values[i] = on.to_string();
                        }
                    }
                    ParamKind::Choice(opts) => {
                        egui::ComboBox::from_id_salt(("choice", p.name)).selected_text(form.values[i].clone()).width(220.0).show_ui(ui, |ui| {
                            for o in opts {
                                ui.selectable_value(&mut form.values[i], (*o).to_owned(), *o);
                            }
                        });
                        ui.label(RichText::new(p.help).size(12.0).color(t.faint));
                    }
                    ParamKind::Text | ParamKind::Integer => {
                        let field = if p.multiline {
                            egui::TextEdit::multiline(&mut form.values[i]).desired_rows(6)
                        } else {
                            egui::TextEdit::singleline(&mut form.values[i]).password(p.secret)
                        };
                        let edit = ui.add(field.desired_width(f32::INFINITY).margin(vec2(10.0, 8.0)));
                        if i == 0 && !ui.ctx().memory(|m| m.has_focus(edit.id)) && form.values[i].is_empty() {
                            edit.request_focus();
                        }
                        if !p.multiline && edit.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter)) {
                            submit = true;
                        }
                        ui.label(RichText::new(p.help).size(12.0).color(t.faint));
                    }
                }
                ui.add_space(10.0);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if widgets::button(ui, "Review", Kind::Primary, 120.0).clicked() {
                    submit = true;
                }
                if widgets::button(ui, "Cancel", Kind::Secondary, 0.0).clicked() {
                    close = true;
                }
            });
        });
        if submit {
            if let Some(f) = self.form.take() {
                f.submit(&self.session);
            }
        } else if close || resp.should_close() {
            self.form = None;
        }
    }
}

/// Terminal commands, one per line, with a Geeko-green prompt.
fn shell_block(ui: &mut Ui, cmds: &[String]) {
    if cmds.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    Frame::new().fill(t.sunken).inner_margin(Margin::symmetric(10, 7)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            // "copied" stays for a moment after a click, per block.
            let id = ui.scope_id().with(("copy", cmds));
            let now = ui.input(|i| i.time);
            let copied = ui.data(|d| d.get_temp::<f64>(id)).is_some_and(|at| now - at < 1.5);
            if widgets::badge_button(ui, if copied { "copied" } else { "copy" }, None).on_hover_text("Copy the commands").clicked() {
                ui.ctx().copy_text(cmds.join("\n"));
                ui.data_mut(|d| d.insert_temp(id, now));
            }
            if copied {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
            }
            ui.vertical(|ui| {
                for c in cmds {
                    let mut job = egui::text::LayoutJob::default();
                    let font = FontId::new(12.5, theme::mono());
                    job.append("$ ", 0.0, egui::TextFormat { font_id: font.clone(), color: t.accent, ..Default::default() });
                    job.append(c, 0.0, egui::TextFormat { font_id: font, color: t.ink_strong, ..Default::default() });
                    ui.label(job);
                }
            });
        });
    });
}

/// What a change's commands printed: scrollback that follows new lines
/// while it runs (until scrolled up), only the visible rows laid out.
fn log_block(ui: &mut Ui, p: &Plan) {
    let t = Tokens::get(ui.ctx());
    let font = FontId::new(11.5, theme::mono());
    let row = ui.fonts_mut(|f| f.row_height(&font));
    Frame::new().fill(t.sunken).inner_margin(Margin::symmetric(10, 7)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical()
            .id_salt(("log", p.id))
            .max_height(row * 12.0)
            .auto_shrink([false, true])
            .stick_to_bottom(true)
            .show_rows(ui, row, p.log.len(), |ui, rows| {
                for line in &p.log[rows] {
                    ui.add(egui::Label::new(RichText::new(line).font(font.clone()).color(t.muted)).truncate());
                }
            });
    });
}

fn status_line(ui: &mut Ui, label: &str, color: Color32, msg: &str, msg_color: Color32) {
    ui.horizontal_wrapped(|ui| {
        widgets::badge(ui, label, color);
        if !msg.is_empty() {
            ui.label(RichText::new(msg).size(13.0).color(msg_color));
        }
    });
}

/// A result table in the conversation: the first rows, in mono.
fn compact_table(ui: &mut Ui, table: &Table) {
    let t = Tokens::get(ui.ctx());
    const MAX: usize = 12;
    Frame::new().stroke(t.hairline()).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::horizontal().id_salt(ui.next_auto_id()).show(ui, |ui| {
            egui::Grid::new(ui.next_auto_id()).num_columns(table.columns.len()).spacing(vec2(14.0, 3.0)).show(ui, |ui| {
                for c in &table.columns {
                    widgets::hud(ui, c.title, t.faint);
                }
                ui.end_row();
                for row in table.rows.iter().take(MAX) {
                    for (c, v) in row.iter().enumerate() {
                        let color = if yaist_ui::is_status_column(table.columns[c].key) { t.tone(tone_of(v)) } else { t.ink };
                        let short: String = if v.chars().count() > 40 { v.chars().take(39).chain(['…']).collect() } else { v.clone() };
                        ui.label(RichText::new(short).family(theme::mono()).size(12.0).color(color));
                    }
                    ui.end_row();
                }
            });
        });
        if table.rows.len() > MAX {
            widgets::hud(ui, &format!("… {} more", table.rows.len() - MAX), t.faint);
        } else if table.rows.is_empty() {
            widgets::hud(ui, "nothing", t.faint);
        }
    });
}
