//! The terminal client: the same layout as the GUI — modules, the module's
//! actionable panel, and the conversation with the input prompt — drawn with
//! ratatui in the openSUSE palette.

use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState};
use ratatui::Frame;
use yaist_core::{ActionSpec, Output, ParamKind, Risk};
use yaist_ui::palette::{Rgb, Tokens};
use yaist_ui::welcome::{self, Start};
use yaist_ui::{reboot_notice, update_summary, UpdateLevel, UPDATE_ACTION};
use yaist_ui::{row_action_is_direct, row_args, tone_of, Asked, Cmd, Entry, Form, LlmStatus, Panel, PlanStatus, Session, State};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Modules,
    Panel,
    Input,
}

struct Menu {
    items: Vec<ActionSpec>,
    selected: usize,
    /// The table row the menu was opened on, for row actions.
    row: Option<Vec<String>>,
}

struct App {
    session: Session,
    t: Tokens,
    focus: Focus,
    input: String,
    /// Cursor position in characters.
    cursor: usize,
    history: Vec<String>,
    history_pos: Option<usize>,
    table: TableState,
    /// Lines scrolled up from the bottom of the conversation.
    scroll_up: u16,
    menu: Option<Menu>,
    form: Option<Form>,
    form_field: usize,
    plan_choice: usize,
    /// The choice picked in the open question: (question id, choice index).
    question_choice: (u64, usize),
    started: Instant,
    quit: bool,
}

fn c(rgb: Rgb) -> Color {
    Color::Rgb(rgb[0], rgb[1], rgb[2])
}

pub fn run() -> Result<()> {
    let mut app = App::new(Session::start(|| {}));
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        while !app.quit {
            terminal.draw(|f| app.draw(f))?;
            if event::poll(Duration::from_millis(120))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        app.key(key);
                    }
                }
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

/// Renders the TUI into memory after typing `inputs` (one line each, with a
/// pause for the backend) and prints the screen as text. For checks without
/// a terminal; plans are cancelled at the end.
pub fn snapshot(inputs: &[String], width: u16, height: u16) -> Result<()> {
    let mut app = App::new(Session::start(|| {}));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))?;
    let settle = |app: &App| {
        for _ in 0..200 {
            std::thread::sleep(Duration::from_millis(100));
            let s = app.session.state();
            let updates_done = !matches!(s.updates, yaist_ui::UpdateStatus::Checking);
            if s.busy.is_none() && s.llm != LlmStatus::Checking && updates_done && !matches!(s.modules[s.selected].panel, Panel::Loading) {
                break;
            }
        }
    };
    settle(&app);
    for line in inputs {
        for ch in line.chars() {
            app.key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        std::thread::sleep(Duration::from_millis(300));
        settle(&app);
    }
    terminal.draw(|f| app.draw(f))?;
    let buf = terminal.backend().buffer().clone();
    for y in 0..height {
        let line: String = (0..width).map(|x| buf[(x, y)].symbol().to_owned()).collect();
        println!("{}", line.trim_end());
    }
    let pending = app.session.state().pending_plan().map(|p| p.id);
    if let Some(id) = pending {
        app.session.send(Cmd::Cancel(id));
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

impl App {
    fn new(session: Session) -> Self {
        Self {
            session,
            t: Tokens::dark(),
            focus: Focus::Input,
            input: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_pos: None,
            table: TableState::default().with_selected(Some(0)),
            scroll_up: 0,
            menu: None,
            form: None,
            form_field: 0,
            plan_choice: 0,
            question_choice: (0, 0),
            started: Instant::now(),
            quit: false,
        }
    }
}

impl App {
    // ---- input ---------------------------------------------------------------------------------

    fn key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q')) {
            self.quit = true;
            return;
        }
        // A running change waits for this answer; it comes first.
        let question = self.session.state().question.clone();
        if let Some(q) = question {
            return self.question_key(key, &q);
        }
        if self.form.is_some() {
            return self.form_key(key);
        }
        if self.menu.is_some() {
            return self.menu_key(key);
        }
        match key.code {
            KeyCode::PageUp => return self.scroll_up = self.scroll_up.saturating_add(8),
            KeyCode::PageDown => return self.scroll_up = self.scroll_up.saturating_sub(8),
            KeyCode::F(5) => {
                let i = self.session.state().selected;
                return self.session.send(Cmd::Refresh(i));
            }
            KeyCode::Tab => return self.cycle_focus(true),
            KeyCode::BackTab => return self.cycle_focus(false),
            _ => {}
        }
        let pending = self.session.state().pending_plan().map(|p| p.id);
        if let Some(id) = pending {
            if self.plan_key(key, id) {
                return;
            }
        }
        match self.focus {
            Focus::Modules => self.modules_key(key),
            Focus::Panel => self.panel_key(key),
            Focus::Input if pending.is_none() => self.input_key(key),
            Focus::Input => {}
        }
    }

    /// The selected choice of `q`, starting at its default.
    fn question_selected(&self, q: &Asked) -> usize {
        if self.question_choice.0 == q.id {
            self.question_choice.1
        } else {
            let choices = &q.question.choices;
            choices.iter().position(|c| c.default).unwrap_or(choices.len().saturating_sub(1))
        }
    }

    fn question_key(&mut self, key: KeyEvent, q: &Asked) {
        let n = q.question.choices.len();
        if n == 0 {
            return;
        }
        let sel = self.question_selected(q);
        let sel = match key.code {
            KeyCode::Up | KeyCode::Char('k') => (sel + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') => (sel + 1) % n,
            KeyCode::Char(d @ '1'..='9') => (d as usize - '1' as usize).min(n - 1),
            KeyCode::Enter => {
                let value = q.question.choices[sel].value.clone();
                return self.session.send(Cmd::Answer { id: q.id, value });
            }
            _ => sel,
        };
        self.question_choice = (q.id, sel);
    }

    fn cycle_focus(&mut self, forward: bool) {
        let order = [Focus::Modules, Focus::Panel, Focus::Input];
        let i = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        self.focus = order[if forward { (i + 1) % 3 } else { (i + 2) % 3 }];
    }

    /// Keys for the pending plan; returns whether the key was used.
    fn plan_key(&mut self, key: KeyEvent, id: u64) -> bool {
        let choice = match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Some(0),
            KeyCode::Char('e') | KeyCode::Char('E') => Some(1),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some(2),
            KeyCode::Left => {
                self.plan_choice = (self.plan_choice + 2) % 3;
                return true;
            }
            KeyCode::Right => {
                self.plan_choice = (self.plan_choice + 1) % 3;
                return true;
            }
            KeyCode::Enter if self.focus == Focus::Input => Some(self.plan_choice),
            _ => None,
        };
        let Some(choice) = choice else { return false };
        match choice {
            0 => self.session.send(Cmd::Confirm(id)),
            1 => {
                let form = self.session.state().pending_plan().map(Form::edit);
                if let Some(f) = form {
                    self.open_form(f);
                }
            }
            _ => self.session.send(Cmd::Cancel(id)),
        }
        self.plan_choice = 0;
        true
    }

    fn modules_key(&mut self, key: KeyEvent) {
        let (sel, n) = {
            let s = self.session.state();
            (s.selected, s.modules.len())
        };
        let next = match key.code {
            KeyCode::Up | KeyCode::Char('k') => sel.checked_sub(1).unwrap_or(n - 1),
            KeyCode::Down | KeyCode::Char('j') => (sel + 1) % n,
            KeyCode::Char(d @ '0'..='9') => (d as usize - '0' as usize).min(n - 1),
            KeyCode::Enter | KeyCode::Right => {
                self.focus = Focus::Panel;
                return;
            }
            _ => return,
        };
        self.session.select(next);
        self.table.select(Some(0));
    }

    fn panel_key(&mut self, key: KeyEvent) {
        let (rows, welcome_items) = {
            let s = self.session.state();
            if s.modules[s.selected].spec.id == "welcome" {
                let items = welcome::start_items(&s.distro);
                (items.len(), Some(items))
            } else {
                let rows = match &s.modules[s.selected].panel {
                    Panel::Ready(Output::Table(t)) => t.rows.len(),
                    _ => 0,
                };
                (rows, None)
            }
        };
        if let (Some(items), KeyCode::Enter) = (&welcome_items, key.code) {
            if let Some(item) = items.get(self.table.selected().unwrap_or(0)) {
                self.start(item.start);
            }
            return;
        }
        let sel = self.table.selected().unwrap_or(0);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.table.select(Some(sel.saturating_sub(1))),
            KeyCode::Down | KeyCode::Char('j') => self.table.select(Some((sel + 1).min(rows.saturating_sub(1)))),
            KeyCode::Home => self.table.select(Some(0)),
            KeyCode::End => self.table.select(Some(rows.saturating_sub(1))),
            KeyCode::Left => self.focus = Focus::Modules,
            KeyCode::Enter => self.open_menu(true),
            KeyCode::Char('a') => self.open_menu(false),
            KeyCode::Char('u') => {
                self.session.send(Cmd::Propose { action: UPDATE_ACTION, raw: Default::default() });
                self.focus = Focus::Input;
            }
            // Leap only: just the security patches.
            KeyCode::Char('s')
                if {
                    let d = &self.session.state().distro;
                    !d.rolling && !d.transactional
                } =>
            {
                self.session.send(Cmd::Propose { action: yaist_ui::SECURITY_ACTION, raw: Default::default() });
                self.focus = Focus::Input;
            }
            KeyCode::Char('r') => {
                let i = self.session.state().selected;
                self.session.send(Cmd::Refresh(i));
            }
            _ => {}
        }
    }

    fn start(&mut self, start: Start) {
        match start {
            Start::Module(id) => {
                let i = self.session.state().module_index(id);
                if let Some(i) = i {
                    self.session.select(i);
                    self.table.select(Some(0));
                }
            }
            Start::Action(id) => {
                let Some(spec) = self.session.registry.get(id).copied() else { return };
                if spec.params.iter().any(|p| p.required) {
                    self.open_form(Form::new(spec, &Default::default()));
                } else {
                    self.session.send(Cmd::Propose { action: spec.id, raw: Default::default() });
                    self.focus = Focus::Input;
                }
            }
        }
    }

    fn input_key(&mut self, key: KeyEvent) {
        let len = self.input.chars().count();
        match key.code {
            KeyCode::Enter => {
                let text = std::mem::take(&mut self.input);
                self.cursor = 0;
                self.history_pos = None;
                if !text.trim().is_empty() {
                    self.history.push(text.clone());
                    self.scroll_up = 0;
                    self.session.send(Cmd::Submit(text));
                }
            }
            KeyCode::Char(ch) => {
                let at = self.byte_at(self.cursor);
                self.input.insert(at, ch);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let at = self.byte_at(self.cursor);
                self.input.remove(at);
            }
            KeyCode::Delete if self.cursor < len => {
                let at = self.byte_at(self.cursor);
                self.input.remove(at);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = len,
            KeyCode::Up if !self.history.is_empty() => {
                let pos = self.history_pos.map_or(self.history.len() - 1, |p| p.saturating_sub(1));
                self.history_pos = Some(pos);
                self.input = self.history[pos].clone();
                self.cursor = self.input.chars().count();
            }
            KeyCode::Down => {
                if let Some(p) = self.history_pos {
                    if p + 1 < self.history.len() {
                        self.history_pos = Some(p + 1);
                        self.input = self.history[p + 1].clone();
                    } else {
                        self.history_pos = None;
                        self.input.clear();
                    }
                    self.cursor = self.input.chars().count();
                }
            }
            KeyCode::Esc => {
                self.input.clear();
                self.cursor = 0;
            }
            _ => {}
        }
    }

    fn byte_at(&self, chars: usize) -> usize {
        self.input.char_indices().nth(chars).map_or(self.input.len(), |(i, _)| i)
    }

    /// Row actions for the selected row (`on_row`), or the module's own actions.
    fn open_menu(&mut self, on_row: bool) {
        let s = self.session.state();
        let m = &s.modules[s.selected];
        let row = match (&m.panel, on_row) {
            (Panel::Ready(Output::Table(t)), true) => self.table.selected().and_then(|i| t.rows.get(i)).cloned(),
            _ => None,
        };
        let on_rows = row.is_some() && !m.row_actions.is_empty();
        let items = if on_rows { m.row_actions.clone() } else { m.actions.clone() };
        let row = if on_rows { row } else { None };
        drop(s);
        if !items.is_empty() {
            self.menu = Some(Menu { items, selected: 0, row });
        }
    }

    fn menu_key(&mut self, key: KeyEvent) {
        let Some(menu) = &mut self.menu else { return };
        match key.code {
            KeyCode::Esc => self.menu = None,
            KeyCode::Up | KeyCode::Char('k') => menu.selected = menu.selected.checked_sub(1).unwrap_or(menu.items.len() - 1),
            KeyCode::Down | KeyCode::Char('j') => menu.selected = (menu.selected + 1) % menu.items.len(),
            KeyCode::Enter => {
                let menu = self.menu.take().expect("menu");
                let spec = menu.items[menu.selected];
                let s = self.session.state();
                let table = match &s.modules[s.selected].panel {
                    Panel::Ready(Output::Table(t)) => Some(t.clone()),
                    _ => None,
                };
                drop(s);
                match (menu.row, table) {
                    (Some(row), Some(t)) => {
                        let raw = row_args(&spec, &t, &row);
                        if row_action_is_direct(&spec, &t) {
                            self.session.send(Cmd::Propose { action: spec.id, raw });
                            self.focus = Focus::Input;
                        } else {
                            self.open_form(Form::new(spec, &raw));
                        }
                    }
                    _ if spec.params.is_empty() => {
                        self.session.send(Cmd::Propose { action: spec.id, raw: Default::default() });
                        self.focus = Focus::Input;
                    }
                    _ => self.open_form(Form::new(spec, &Default::default())),
                }
            }
            _ => {}
        }
    }

    fn open_form(&mut self, form: Form) {
        self.form = Some(form);
        self.form_field = 0;
    }

    fn form_key(&mut self, key: KeyEvent) {
        let Some(form) = &mut self.form else { return };
        let n = form.values.len();
        let i = self.form_field;
        let kind = form.spec.params.get(i).map(|p| p.kind);
        match key.code {
            KeyCode::Esc => self.form = None,
            KeyCode::Enter => {
                let form = self.form.take().expect("form");
                form.submit(&self.session);
                self.focus = Focus::Input;
            }
            KeyCode::Up | KeyCode::BackTab if n > 0 => self.form_field = (i + n - 1) % n,
            KeyCode::Down | KeyCode::Tab if n > 0 => self.form_field = (i + 1) % n,
            KeyCode::Char(' ') if matches!(kind, Some(ParamKind::Bool)) => form.toggle(i),
            KeyCode::Left if matches!(kind, Some(ParamKind::Choice(_))) => form.cycle(i, false),
            KeyCode::Right if matches!(kind, Some(ParamKind::Choice(_))) => form.cycle(i, true),
            KeyCode::Left | KeyCode::Right if matches!(kind, Some(ParamKind::Bool)) => form.toggle(i),
            KeyCode::Char(ch) if matches!(kind, Some(ParamKind::Text | ParamKind::Integer)) => form.values[i].push(ch),
            KeyCode::Backspace if matches!(kind, Some(ParamKind::Text | ParamKind::Integer)) => {
                form.values[i].pop();
            }
            _ => {}
        }
    }

    // ---- drawing -------------------------------------------------------------------------------

    fn style(&self, fg: Rgb) -> Style {
        Style::new().fg(c(fg)).bg(c(self.t.bg))
    }

    fn hud(&self, text: &str, fg: Rgb) -> Span<'static> {
        Span::styled(text.to_uppercase(), self.style(fg))
    }

    fn tag(&self, text: &str, fill: Rgb) -> Span<'static> {
        Span::styled(format!(" {text} "), Style::new().fg(c(self.t.invert_fg)).bg(c(fill)).add_modifier(Modifier::BOLD))
    }

    fn badge(&self, text: &str, fg: Rgb) -> Span<'static> {
        Span::styled(format!(" {} ", text.to_uppercase()), Style::new().fg(c(fg)).bg(c(self.t.sunken)))
    }

    fn block(&self, focused: bool) -> Block<'static> {
        Block::new()
            .borders(Borders::ALL)
            .border_style(self.style(if focused { self.t.line_strong } else { self.t.line }))
            .style(self.style(self.t.ink))
    }

    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        f.render_widget(Block::new().style(self.style(self.t.ink)), area);
        let [header, body, footer] = Layout::vertical([Constraint::Length(2), Constraint::Min(5), Constraint::Length(1)]).areas(area);

        // The state guard borrows the session, so the widget state that
        // drawing updates is taken out of `self` meanwhile.
        let mut table = std::mem::take(&mut self.table);
        let mut scroll_up = self.scroll_up;
        let s = self.session.state();
        self.draw_header(f, header, &s);
        let [side, main] = Layout::horizontal([Constraint::Length(36), Constraint::Min(30)]).areas(body);
        self.draw_modules(f, side, &s);
        let (panel_area, conv_area) = if main.width >= 150 {
            let [a, b] = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(main);
            (a, b)
        } else {
            let [a, b] = Layout::vertical([Constraint::Percentage(52), Constraint::Percentage(48)]).areas(main);
            (a, b)
        };
        self.draw_panel(f, panel_area, &s, &mut table);
        self.draw_conversation(f, conv_area, &s, &mut scroll_up);
        self.draw_footer(f, footer, &s);
        drop(s);
        self.table = table;
        self.scroll_up = scroll_up;

        if let Some(menu) = &self.menu {
            self.draw_menu(f, area, menu);
        }
        if let Some(form) = &self.form {
            self.draw_form(f, area, form);
        }
        let question = self.session.state().question.clone();
        if let Some(q) = question {
            self.draw_question(f, area, &q);
        }
    }

    fn draw_header(&self, f: &mut Frame, area: Rect, s: &State) {
        let t = &self.t;
        let llm = match &s.llm {
            LlmStatus::Ready(name) => self.badge(&format!("model · {name}"), t.good),
            LlmStatus::Checking => self.badge("model · checking", t.muted),
            LlmStatus::Offline => self.badge("model · offline", t.warn),
        };
        let mut left = vec![Span::raw(" ")];
        left.extend(self.wordmark());
        let left = Line::from(
            [
                left,
                vec![
                    Span::raw(" "),
                    Span::styled(" │ ", self.style(t.line)),
                    self.tag("system", t.invert_bg),
                    Span::raw(" "),
                    self.hud(&s.distro.pretty, t.muted),
                    Span::raw("  "),
                    self.update_badge(s),
                ],
            ]
            .concat(),
        );
        let right = Line::from(vec![llm, Span::raw(" ")]).right_aligned();
        let block = Block::new().borders(Borders::BOTTOM).border_style(self.style(t.line)).style(self.style(t.ink));
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(Paragraph::new(left), inner);
        f.render_widget(Paragraph::new(right), inner);
    }

    fn draw_modules(&self, f: &mut Frame, area: Rect, s: &State) {
        let t = &self.t;
        let block = self.block(self.focus == Focus::Modules).title(Line::from(self.hud(" modules ", t.faint)));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines = Vec::new();
        for (i, m) in s.modules.iter().enumerate() {
            let selected = i == s.selected;
            let bg = if selected { t.sunken } else { t.bg };
            let st = |fg: Rgb| Style::new().fg(c(fg)).bg(c(bg));
            let width = inner.width as usize;
            let mut title = vec![
                Span::styled(if selected { "▌" } else { " " }, st(t.module_accent(i))),
                self.tag(&format!("{i:02}"), t.module_accent(i)),
                Span::styled(format!(" {}", m.spec.title), st(if selected { t.ink_strong } else { t.ink }).add_modifier(Modifier::BOLD)),
            ];
            let used: usize = title.iter().map(|s| s.content.chars().count()).sum();
            title.push(Span::styled(" ".repeat(width.saturating_sub(used)), st(t.ink)));
            lines.push(Line::from(title));
            let tagline = format!("      {}", m.spec.tagline);
            lines.push(Line::from(Span::styled(format!("{tagline:<width$}"), st(t.faint))));
            lines.push(Line::from(""));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_panel(&self, f: &mut Frame, area: Rect, s: &State, table_state: &mut TableState) {
        let t = self.t;
        let m = &s.modules[s.selected];
        let accent = t.module_accent(s.selected);
        let block = self.block(self.focus == Focus::Panel).title(Line::from(vec![
            Span::raw(" "),
            self.hud(&format!("{:02} / {}", s.selected, m.spec.title), t.faint),
            Span::raw(" "),
        ]));
        let inner = block.inner(area);
        f.render_widget(block, area);
        if m.spec.id == "welcome" {
            return self.draw_welcome(f, inner, s, table_state);
        }
        let [head, buttons, content] = Layout::vertical([Constraint::Length(2), Constraint::Length(2), Constraint::Min(1)]).areas(inner);

        let mut head_lines = vec![Line::from(vec![
            Span::styled(format!(" {}", m.spec.title), self.style(t.ink_strong).add_modifier(Modifier::BOLD)),
            Span::styled("  ", self.style(t.ink)),
            Span::styled(m.spec.tagline.to_owned(), self.style(t.muted)),
        ])];
        if m.spec.id == "packages" {
            head_lines.push(self.update_line(s));
        }
        f.render_widget(Paragraph::new(head_lines), head);

        let mut spans = vec![Span::raw(" ")];
        for a in &m.actions {
            spans.push(Span::styled(
                format!(" {} ", a.title.to_uppercase()),
                Style::new().fg(c(t.invert_fg)).bg(c(t.invert_bg)).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw("  "));
        }
        if !m.actions.is_empty() {
            spans.push(self.hud("[a]", t.faint));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), buttons);

        match &m.panel {
            Panel::Loading => f.render_widget(Paragraph::new(Line::from(self.hud(" loading…", t.faint))), content),
            Panel::Locked => f.render_widget(
                Paragraph::new(Line::from(Span::styled(" Reading this needs the administrator password: press r to load it.", self.style(t.muted)))),
                content,
            ),
            Panel::Failed(e) => f.render_widget(Paragraph::new(Line::from(Span::styled(format!(" {e}"), self.style(t.bad)))), content),
            Panel::Ready(Output::Table(table)) => {
                let header = Row::new(table.columns.iter().map(|col| Cell::from(col.title.to_uppercase()).style(self.style(t.faint)))).height(1);
                let rows = table.rows.iter().map(|r| {
                    Row::new(r.iter().zip(&table.columns).map(|(v, col)| {
                        if yaist_ui::is_status_column(col.key) {
                            Cell::from(v.clone()).style(Style::new().fg(c(t.tone(tone_of(v)))))
                        } else {
                            Cell::from(v.clone())
                        }
                    }))
                });
                let widths: Vec<Constraint> = table
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, _)| {
                        let w = table.rows.iter().take(200).map(|r| r[i].chars().count()).max().unwrap_or(0).max(table.columns[i].title.len());
                        Constraint::Max(w.min(40) as u16 + 1)
                    })
                    .collect();
                let widget = Table::new(rows, widths)
                    .header(header)
                    .style(self.style(t.ink))
                    .row_highlight_style(Style::new().bg(c(t.sunken)).fg(c(t.ink_strong)).add_modifier(Modifier::BOLD))
                    .highlight_symbol(Span::styled("▌", Style::new().fg(c(accent))));
                if table_state.selected().is_some_and(|i| i >= table.rows.len()) {
                    table_state.select(Some(table.rows.len().saturating_sub(1)));
                }
                f.render_stateful_widget(widget, content, table_state);
            }
            Panel::Ready(Output::Facts(facts)) => {
                let mut lines = Vec::new();
                for (k, v) in facts {
                    let mut first = true;
                    for part in v.lines() {
                        let key = if first { format!(" {:<18}", k.to_uppercase()) } else { " ".repeat(19) };
                        lines.push(Line::from(vec![Span::styled(key, self.style(t.faint)), Span::styled(part.to_owned(), self.style(t.ink))]));
                        first = false;
                    }
                }
                f.render_widget(Paragraph::new(lines), content);
            }
            Panel::Ready(Output::Text(text) | Output::Done(text)) => {
                f.render_widget(Paragraph::new(text.clone()).style(self.style(t.ink)).wrap(ratatui::widgets::Wrap { trim: false }), content);
            }
        }
    }

    /// The YAiST wordmark, with the AI in Geeko green.
    fn wordmark(&self) -> Vec<Span<'static>> {
        yaist_ui::WORDMARK
            .iter()
            .map(|(text, accent)| {
                Span::styled(*text, self.style(if *accent { self.t.brand } else { self.t.ink_strong }).add_modifier(Modifier::BOLD))
            })
            .collect()
    }

    fn update_colour(&self, level: UpdateLevel) -> Rgb {
        match level {
            UpdateLevel::Security => self.t.bad,
            UpdateLevel::Available => self.t.warn,
            UpdateLevel::UpToDate => self.t.good,
            UpdateLevel::Unknown => self.t.faint,
        }
    }

    /// Solid and coloured while updates wait, so it is seen on every screen.
    fn update_badge(&self, s: &State) -> Span<'static> {
        if s.updates.reboot_needed() {
            return self.tag("REBOOT TO APPLY", self.t.warn);
        }
        let (level, badge, ..) = update_summary(&s.updates, s.distro.transactional);
        let colour = self.update_colour(level);
        match level {
            UpdateLevel::Available | UpdateLevel::Security => self.tag(&badge.to_uppercase(), colour),
            _ => self.badge(&badge, colour),
        }
    }

    /// The update banner as one line: a coloured rail, the headline, detail
    /// and the key to install.
    fn update_line(&self, s: &State) -> Line<'static> {
        let t = &self.t;
        if let Some((headline, detail)) = reboot_notice(&s.updates, s.distro.transactional) {
            return Line::from(vec![
                Span::styled(" ▌", self.style(t.warn)),
                Span::styled(format!("{headline}  "), self.style(t.ink_strong).add_modifier(Modifier::BOLD)),
                Span::styled(detail, self.style(t.muted)),
                Span::raw("  "),
                Span::styled(" /system reboot ", Style::new().fg(c(t.invert_fg)).bg(c(t.warn)).add_modifier(Modifier::BOLD)),
            ]);
        }
        let (level, _, headline, detail) = update_summary(&s.updates, s.distro.transactional);
        let colour = self.update_colour(level);
        let mut spans = vec![
            Span::styled(" ▌", self.style(colour)),
            Span::styled(
                format!("{headline}  "),
                self.style(if level == UpdateLevel::Unknown { t.muted } else { t.ink_strong }).add_modifier(Modifier::BOLD),
            ),
            Span::styled(detail, self.style(t.muted)),
        ];
        if matches!(level, UpdateLevel::Available | UpdateLevel::Security) {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(" [U] INSTALL UPDATES ", Style::new().fg(c(t.invert_fg)).bg(c(t.invert_bg)).add_modifier(Modifier::BOLD)));
        }
        Line::from(spans)
    }

    fn draw_welcome(&self, f: &mut Frame, area: Rect, s: &State, cursor: &mut TableState) {
        let t = self.t;
        let d = &s.distro;
        let mut lines: Vec<Line> = Vec::new();
        let name = if d.pretty.is_empty() { "openSUSE".to_owned() } else { d.pretty.clone() };
        let (brand, rest) = name.split_once(' ').map(|(a, b)| (a.to_owned(), format!(" {b}"))).unwrap_or((name.clone(), String::new()));
        let support = welcome::support(d, yaist_core::date::today());
        for (i, art) in welcome::ASCII_CHAMELEON.iter().enumerate() {
            let mut spans = vec![Span::styled(format!(" {art}"), self.style(t.accent))];
            match i {
                2 => spans.push(self.hud("  welcome to", t.muted)),
                3 => {
                    spans.push(Span::styled(format!("  {brand}"), self.style(t.accent).add_modifier(Modifier::BOLD)));
                    spans.push(Span::styled(rest.clone(), self.style(t.ink_strong).add_modifier(Modifier::BOLD)));
                }
                4 => {
                    let (mode, _) = welcome::mode(d);
                    spans.push(Span::raw("  "));
                    spans.push(self.tag(&mode.to_uppercase(), t.invert_bg));
                    if let Some(label) = support.label() {
                        let fill = if matches!(support, welcome::Support::EndOfLife { .. }) { t.bad } else { t.invert_bg };
                        spans.push(Span::raw(" "));
                        spans.push(self.tag(&label.to_uppercase(), fill));
                    }
                }
                5 => {
                    let colour = match support {
                        welcome::Support::EndOfLife { .. } => t.bad,
                        welcome::Support::Until { soon: true, .. } => t.warn,
                        _ => t.muted,
                    };
                    spans.push(Span::styled(format!("  {}", support.sentence()), self.style(colour)));
                }
                _ => {}
            }
            lines.push(Line::from(spans));
        }
        lines.push(Line::from(""));
        lines.push(self.update_line(s));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::raw(" "), self.hud("01 / get started", t.faint), self.hud("   ↑↓ choose · enter start", t.faint)]));
        let items = welcome::start_items(d);
        let sel = cursor.selected().unwrap_or(0).min(items.len().saturating_sub(1));
        cursor.select(Some(sel));
        for (i, item) in items.iter().enumerate() {
            let focused = self.focus == Focus::Panel && i == sel;
            let bg = if focused { t.sunken } else { t.bg };
            lines.push(Line::from(vec![
                Span::styled(if focused { " ▌" } else { "  " }, Style::new().fg(c(t.accent)).bg(c(bg))),
                self.tag(&format!("{:02}", i + 1), t.module_accent(i + 1)),
                Span::styled(format!(" {:<22}", item.title), Style::new().fg(c(t.ink_strong)).bg(c(bg)).add_modifier(Modifier::BOLD)),
                Span::styled(item.text.to_owned(), Style::new().fg(c(t.muted)).bg(c(bg))),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::raw(" "), self.hud("02 / try asking", t.faint)]));
        let mut chips = vec![Span::raw(" ")];
        for q in welcome::suggestions(d) {
            chips.push(Span::styled(format!(" {q} "), Style::new().fg(c(t.ink)).bg(c(t.sunken))));
            chips.push(Span::raw("  "));
        }
        lines.push(Line::from(chips));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::raw(" "), self.hud("03 / your system", t.faint)]));
        if let Panel::Ready(Output::Facts(facts)) = &s.modules[s.selected].panel {
            for (k, v) in facts.iter().filter(|(_, v)| !v.contains('\n')) {
                lines.push(Line::from(vec![
                    Span::styled(format!(" {:<18}", k.to_uppercase()), self.style(t.faint)),
                    Span::styled(v.clone(), self.style(t.ink)),
                ]));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw(" "),
            self.hud("04 / join the community", t.faint),
            self.hud("   more in the community module", t.faint),
        ]));
        for title in welcome::FEATURED {
            if let Some(l) = welcome::link(title) {
                lines.push(Line::from(vec![
                    Span::styled(format!(" {:<14}", l.title), self.style(t.ink_strong)),
                    Span::styled(l.url.to_owned(), self.style(t.accent)),
                    Span::styled(format!("  {}", l.text), self.style(t.muted)),
                ]));
            }
        }
        f.render_widget(Paragraph::new(lines), area);
    }

    fn draw_conversation(&self, f: &mut Frame, area: Rect, s: &State, scroll_up: &mut u16) {
        let t = self.t;
        let block = self.block(self.focus == Focus::Input).title(Line::from(self.hud(" conversation ", t.faint)));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let [log_area, input_area] = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).areas(inner);
        let width = log_area.width.saturating_sub(1) as usize;

        let mut lines: Vec<Line> = Vec::new();
        // Nothing asked yet: invite a first request.
        if s.conversation.iter().all(|e| matches!(e, Entry::Notice(_) | Entry::Error(_))) {
            lines.push(Line::from(Span::styled(" How can I help?", self.style(t.ink_strong).add_modifier(Modifier::BOLD))));
            lines.push(Line::from(Span::styled(" Ask in plain language, for example:", self.style(t.muted))));
            for q in welcome::suggestions(&s.distro).into_iter().take(4) {
                lines.push(Line::from(vec![Span::styled("   › ", self.style(t.accent)), Span::styled(q, self.style(t.ink))]));
            }
            lines.push(Line::from(""));
        }
        for e in &s.conversation {
            self.entry_lines(e, width, &mut lines);
            lines.push(Line::from(""));
        }
        if let Some(what) = &s.busy {
            let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let frame = frames[(self.started.elapsed().as_millis() / 90) as usize % frames.len()];
            lines.push(Line::from(vec![Span::styled(format!(" {frame} "), self.style(t.accent)), Span::styled(what.clone(), self.style(t.muted))]));
        }
        let height = log_area.height as usize;
        let max_up = lines.len().saturating_sub(height) as u16;
        *scroll_up = (*scroll_up).min(max_up);
        let top = max_up - *scroll_up;
        f.render_widget(Paragraph::new(lines).scroll((top, 0)), log_area);

        if let Some(plan) = s.pending_plan() {
            let labels = [("y", "confirm"), ("e", "edit"), ("n", "cancel")];
            let mut spans = vec![
                self.badge(plan.spec.risk.label(), t.risk(plan.spec.risk)),
                Span::raw(" "),
                Span::styled(plan.spec.title.to_owned(), self.style(t.ink_strong).add_modifier(Modifier::BOLD)),
                Span::raw("   "),
            ];
            for (i, (key, l)) in labels.iter().enumerate() {
                let style = if i == self.plan_choice {
                    let fill = if i == 0 && plan.spec.risk == Risk::Dangerous { t.bad } else { t.invert_bg };
                    Style::new().fg(c(t.invert_fg)).bg(c(fill)).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(c(t.ink)).bg(c(t.sunken))
                };
                spans.push(Span::styled(format!(" [{}] {} ", key.to_uppercase(), l.to_uppercase()), style));
                spans.push(Span::raw(" "));
            }
            let block = Block::new().borders(Borders::TOP).border_style(self.style(t.line)).style(self.style(t.ink));
            let inner = block.inner(input_area);
            f.render_widget(block, input_area);
            f.render_widget(Paragraph::new(Line::from(spans)), inner);
            return;
        }

        let focused = self.focus == Focus::Input;
        let block =
            Block::new().borders(Borders::TOP).border_style(self.style(if focused { t.line_strong } else { t.line })).style(self.style(t.ink));
        let inner = block.inner(input_area);
        f.render_widget(block, input_area);
        let prompt = Span::styled(" › ", self.style(if focused { t.accent } else { t.faint }).add_modifier(Modifier::BOLD));
        let line = if self.input.is_empty() {
            Line::from(vec![prompt, Span::styled("Ask YAiST, or type /help", self.style(t.faint))])
        } else {
            Line::from(vec![prompt, Span::styled(self.input.clone(), self.style(t.ink_strong))])
        };
        f.render_widget(Paragraph::new(line), inner);
        if focused {
            f.set_cursor_position((inner.x + 3 + self.cursor as u16, inner.y));
        }
    }

    fn entry_lines(&self, e: &Entry, width: usize, out: &mut Vec<Line<'static>>) {
        let t = &self.t;
        let text = |out: &mut Vec<Line<'static>>, s: &str, fg: Rgb, indent: usize| {
            for l in wrap(s, width.saturating_sub(indent)) {
                out.push(Line::from(Span::styled(format!("{}{l}", " ".repeat(indent)), self.style(fg))));
            }
        };
        match e {
            Entry::User(s) => {
                out.push(Line::from(vec![self.hud(" you", t.muted)]));
                text(out, s, t.ink_strong, 1);
            }
            Entry::Command(s) => out.push(Line::from(vec![
                Span::styled(" › ", self.style(t.faint)),
                Span::styled(s.clone(), self.style(t.ink_strong).add_modifier(Modifier::BOLD)),
            ])),
            Entry::AgentRan { command, shell, ok } => {
                out.push(Line::from(vec![
                    Span::styled(if *ok { "   ✓ ran " } else { "   ✗ ran " }, self.style(if *ok { t.faint } else { t.bad })),
                    Span::styled(command.clone(), self.style(t.faint)),
                ]));
                for c in shell {
                    out.push(Line::from(Span::styled(clip(&format!("     $ {c}"), width), self.style(t.faint))));
                }
            }
            Entry::Assistant { text: s, elapsed } => {
                let mut spans = vec![Span::raw(" ")];
                spans.extend(self.wordmark());
                spans.push(Span::styled(format!("  {:.1} s", elapsed.as_secs_f32()), self.style(t.faint)));
                out.push(Line::from(spans));
                text(out, s, t.ink, 1);
            }
            Entry::Plan(p) => {
                let risk = t.risk(p.spec.risk);
                out.push(Line::from(vec![
                    Span::raw(" "),
                    self.tag("plan", risk),
                    Span::raw(" "),
                    self.badge(p.spec.risk.label(), risk),
                    Span::styled(format!("  {}", p.spec.title), self.style(t.ink_strong).add_modifier(Modifier::BOLD)),
                ]));
                out.push(Line::from(vec![Span::styled(" │ ", self.style(risk)), Span::styled(p.command.clone(), self.style(t.ink))]));
                for param in p.spec.params {
                    let v = p.args.shown(&p.spec, param.name);
                    let v = if v.is_empty() { "(default)".to_owned() } else { v };
                    out.push(Line::from(vec![
                        Span::styled(" │ ", self.style(risk)),
                        Span::styled(format!("{:<12}", param.name), self.style(t.faint)),
                        Span::styled(v, self.style(t.ink)),
                    ]));
                }
                if !p.shell.is_empty() {
                    out.push(Line::from(vec![Span::styled(" │ ", self.style(risk)), self.hud("in a terminal", t.faint)]));
                    for c in &p.shell {
                        out.push(Line::from(vec![
                            Span::styled(" │ ", self.style(risk)),
                            Span::styled("$ ", self.style(t.accent)),
                            Span::styled(clip(c, width.saturating_sub(5)), self.style(t.ink_strong)),
                        ]));
                    }
                }
                for l in wrap(p.spec.description, width.saturating_sub(3)) {
                    out.push(Line::from(vec![Span::styled(" │ ", self.style(risk)), Span::styled(l, self.style(t.muted))]));
                }
                let status = match &p.status {
                    PlanStatus::Pending => Line::from(vec![
                        Span::styled(" └ ", self.style(risk)),
                        self.hud("waiting for you: [y] confirm  [e] edit  [n] cancel", t.warn),
                    ]),
                    PlanStatus::Running => Line::from(vec![Span::styled(" └ ", self.style(risk)), self.hud("running…", t.muted)]),
                    PlanStatus::Done(m) => Line::from(vec![
                        Span::styled(" └ ", self.style(t.good)),
                        self.badge("done", t.good),
                        Span::styled(format!(" {m}"), self.style(t.ink)),
                    ]),
                    PlanStatus::Failed(m) => Line::from(vec![
                        Span::styled(" └ ", self.style(t.bad)),
                        self.badge("failed", t.bad),
                        Span::styled(format!(" {m}"), self.style(t.bad)),
                    ]),
                    PlanStatus::Cancelled => Line::from(vec![Span::styled(" └ ", self.style(t.faint)), self.badge("cancelled", t.faint)]),
                };
                out.push(status);
                if !p.executed.is_empty() {
                    out.push(Line::from(vec![Span::raw("   "), self.hud("executed", t.faint)]));
                    for c in &p.executed {
                        out.push(Line::from(Span::styled(clip(&format!("   {c}"), width), self.style(t.faint))));
                    }
                }
            }
            Entry::Result { title, shell, output } => {
                out.push(Line::from(vec![Span::raw(" "), self.hud(title, t.faint)]));
                for c in shell {
                    out.push(Line::from(vec![
                        Span::styled(" $ ", self.style(t.accent)),
                        Span::styled(clip(c, width.saturating_sub(3)), self.style(t.muted)),
                    ]));
                }
                match output {
                    Output::Table(table) => {
                        let widths: Vec<usize> = (0..table.columns.len())
                            .map(|i| {
                                table
                                    .rows
                                    .iter()
                                    .take(15)
                                    .map(|r| r[i].chars().count())
                                    .chain([table.columns[i].title.len()])
                                    .max()
                                    .unwrap_or(0)
                                    .min(28)
                            })
                            .collect();
                        let fmt = |cells: Vec<String>| {
                            cells.iter().zip(&widths).map(|(c, w)| format!("{:<w$}", clip(c, *w), w = *w)).collect::<Vec<_>>().join("  ")
                        };
                        out.push(Line::from(Span::styled(
                            clip(&format!(" {}", fmt(table.columns.iter().map(|c| c.title.to_uppercase()).collect())), width),
                            self.style(t.faint),
                        )));
                        for r in table.rows.iter().take(15) {
                            out.push(Line::from(Span::styled(clip(&format!(" {}", fmt(r.clone())), width), self.style(t.ink))));
                        }
                        if table.rows.len() > 15 {
                            out.push(Line::from(Span::styled(format!(" … {} more", table.rows.len() - 15), self.style(t.faint))));
                        }
                        if table.rows.is_empty() {
                            out.push(Line::from(Span::styled(" (nothing)", self.style(t.faint))));
                        }
                    }
                    Output::Facts(facts) => {
                        for (k, v) in facts {
                            for (i, part) in v.lines().enumerate() {
                                let key = if i == 0 { format!(" {:<16}", k) } else { " ".repeat(17) };
                                out.push(Line::from(vec![
                                    Span::styled(key, self.style(t.faint)),
                                    Span::styled(clip(part, width.saturating_sub(17)), self.style(t.ink)),
                                ]));
                            }
                        }
                    }
                    Output::Text(s) | Output::Done(s) => text(out, s, t.ink, 1),
                }
            }
            Entry::Error(s) => text(out, &format!("✗ {s}"), t.bad, 1),
            Entry::Notice(s) => text(out, s, t.muted, 1),
        }
    }

    fn draw_footer(&self, f: &mut Frame, area: Rect, s: &State) {
        let t = &self.t;
        let keys = if s.question.is_some() {
            "↑↓ choose · enter answer"
        } else if self.form.is_some() {
            "enter review · tab next field · space toggle · ←→ choose · esc cancel"
        } else if self.menu.is_some() {
            "↑↓ choose · enter select · esc close"
        } else if s.pending_plan().is_some() {
            "y confirm · e edit · n cancel · tab focus"
        } else {
            match self.focus {
                Focus::Modules => "↑↓ module · enter open · tab focus · ctrl-q quit",
                Focus::Panel => "↑↓ row · enter row actions · a module actions · u install all updates · s security only · r refresh · tab focus",
                Focus::Input => "enter send · ↑ history · pgup/pgdn scroll · tab focus · ctrl-q quit",
            }
        };
        let right = format!("llm {} ", s.llm_url);
        f.render_widget(Paragraph::new(Line::from(vec![Span::raw(" "), self.hud(keys, t.faint)])), area);
        f.render_widget(Paragraph::new(Line::from(self.hud(&right, t.faint)).right_aligned()), area);
    }

    fn popup(&self, area: Rect, w: u16, h: u16) -> Rect {
        let w = w.min(area.width.saturating_sub(4));
        let h = h.min(area.height.saturating_sub(2));
        Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
    }

    fn draw_menu(&self, f: &mut Frame, area: Rect, menu: &Menu) {
        let t = &self.t;
        let rect = self.popup(area, 54, menu.items.len() as u16 + 4);
        f.render_widget(Clear, rect);
        let title = if menu.row.is_some() { " row actions " } else { " actions " };
        let block = self.block(true).title(Line::from(self.hud(title, t.faint)));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let lines: Vec<Line> = menu
            .items
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let sel = i == menu.selected;
                let st = if sel { Style::new().fg(c(t.invert_fg)).bg(c(t.invert_bg)).add_modifier(Modifier::BOLD) } else { self.style(t.ink) };
                Line::from(vec![
                    Span::styled(format!(" {:<28}", a.title.to_uppercase()), st),
                    Span::raw(" "),
                    self.badge(a.risk.label(), t.risk(a.risk)),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner });
    }

    fn draw_form(&self, f: &mut Frame, area: Rect, form: &Form) {
        let t = &self.t;
        let rect = self.popup(area, 72, form.spec.params.len() as u16 * 3 + 7);
        f.render_widget(Clear, rect);
        let block = self.block(true).title(Line::from(vec![
            Span::raw(" "),
            self.hud(form.spec.title, t.ink_strong),
            Span::raw(" "),
            self.badge(form.spec.risk.label(), t.risk(form.spec.risk)),
            Span::raw(" "),
        ]));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let mut lines =
            vec![Line::from(Span::styled(clip(&format!(" {}", form.spec.description), inner.width as usize), self.style(t.muted))), Line::from("")];
        for (i, (p, v)) in form.spec.params.iter().zip(&form.values).enumerate() {
            let sel = i == self.form_field;
            let label = format!(" {}{}", p.name.replace('_', " "), if p.required { "" } else { " (optional)" });
            lines.push(Line::from(self.hud(&label, if sel { t.accent } else { t.faint })));
            let value = match p.kind {
                ParamKind::Bool => {
                    if v == "true" {
                        "[x] yes".to_owned()
                    } else {
                        "[ ] no".to_owned()
                    }
                }
                ParamKind::Choice(_) => format!("‹ {v} ›"),
                _ if p.secret => format!("{}{}", "•".repeat(v.chars().count()), if sel { "▏" } else { "" }),
                _ => format!("{v}{}", if sel { "▏" } else { "" }),
            };
            let st = if sel { Style::new().fg(c(t.ink_strong)).bg(c(t.sunken)) } else { self.style(t.ink) };
            lines.push(Line::from(vec![Span::styled(format!(" {:<w$}", value, w = inner.width.saturating_sub(2) as usize), st)]));
            lines.push(Line::from(Span::styled(if sel { format!(" {}", p.help) } else { String::new() }, self.style(t.faint))));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_question(&self, f: &mut Frame, area: Rect, asked: &Asked) {
        let t = &self.t;
        let q = &asked.question;
        let width = area.width.saturating_sub(8).min(96);
        let text_w = width.saturating_sub(4) as usize;
        let mut text: Vec<String> = wrap(&q.text, text_w);
        // Long texts (a license) are cut; the choices must stay on screen.
        let room = area.height.saturating_sub(q.choices.len() as u16 * 2 + 8) as usize;
        if text.len() > room {
            text.truncate(room.saturating_sub(1));
            text.push("…".into());
        }
        let choice_lines: Vec<Vec<String>> = q.choices.iter().map(|c| wrap(&c.label, text_w.saturating_sub(6))).collect();
        let height = text.len() + choice_lines.iter().map(Vec::len).sum::<usize>() + 5;
        let rect = self.popup(area, width, height as u16);
        f.render_widget(Clear, rect);
        let block = self.block(true).title(Line::from(vec![Span::raw(" "), self.hud(&q.title, t.ink_strong), Span::raw(" ")]));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let mut lines: Vec<Line> = vec![Line::from("")];
        lines.extend(text.into_iter().map(|l| Line::from(Span::styled(format!(" {l}"), self.style(t.ink)))));
        lines.push(Line::from(""));
        let sel = self.question_selected(asked);
        for (i, wrapped) in choice_lines.iter().enumerate() {
            let st = if i == sel { Style::new().fg(c(t.invert_fg)).bg(c(t.invert_bg)).add_modifier(Modifier::BOLD) } else { self.style(t.ink) };
            for (k, l) in wrapped.iter().enumerate() {
                let lead = if k == 0 { format!(" {} ", i + 1) } else { "   ".into() };
                lines.push(Line::from(vec![Span::styled(lead, self.style(t.faint)), Span::styled(format!(" {l} "), st)]));
            }
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}

fn clip(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_owned()
    } else {
        s.chars().take(w.saturating_sub(1)).chain(['…']).collect()
    }
}

/// Word-wraps `s` to `width` columns, keeping its line breaks.
fn wrap(s: &str, width: usize) -> Vec<String> {
    let width = width.max(10);
    let mut out = Vec::new();
    for para in s.lines() {
        let mut line = String::new();
        for word in para.split(' ') {
            let wl = word.chars().count();
            let ll = line.chars().count();
            if ll > 0 && ll + 1 + wl > width {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            let mut word = word.to_owned();
            while word.chars().count() > width {
                let head: String = word.chars().take(width).collect();
                out.push(head);
                word = word.chars().skip(width).collect();
            }
            line.push_str(&word);
        }
        out.push(line);
    }
    out
}
