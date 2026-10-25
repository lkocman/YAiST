//! Building blocks in ArtCraft's style: mono labels, square buttons, badges,
//! number tags and a custom title bar. Adapted from the ArtCraft launcher
//! (MIT OR Apache-2.0).

use std::sync::Arc;

use egui::{vec2, Color32, CornerRadius, Galley, Rect, Response, Sense, Stroke, Ui};

use crate::theme::{self, Tokens};

/// `text` laid out upper case in `family`, `spacing` points between letters.
pub fn caps(ui: &Ui, text: &str, size: f32, family: egui::FontFamily, spacing: f32, color: Color32) -> Arc<Galley> {
    let format = egui::TextFormat { font_id: egui::FontId::new(size, family), color, extra_letter_spacing: spacing, ..Default::default() };
    ui.painter().layout_job(egui::text::LayoutJob::single_section(text.to_uppercase(), format))
}

fn paint_left(ui: &Ui, rect: Rect, galley: Arc<Galley>, color: Color32) {
    let pos = egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0);
    ui.painter().galley_with_override_text_color(pos, galley, color);
}

fn paint_centred(ui: &Ui, rect: Rect, galley: Arc<Galley>, color: Color32) {
    ui.painter().galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, color);
}

/// A mono upper-case label.
pub fn hud(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let galley = caps(ui, text, theme::HUD_SIZE, theme::mono(), theme::HUD_SPACING, color);
    let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::hover());
    paint_left(ui, rect, galley, color);
    resp
}

/// "04 / Firewall": a section eyebrow, the index faint.
pub fn eyebrow(ui: &mut Ui, index: &str, label: &str) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        hud(ui, index, t.faint);
        hud(ui, " / ", t.faint);
        hud(ui, label, t.muted);
    });
}

/// A solid chip with a bold mono label: a module's number tab.
pub fn tag(ui: &mut Ui, text: &str, fill: Color32, ink: Color32) -> Response {
    let galley = caps(ui, text, theme::HUD_SIZE, theme::mono_bold(), theme::HUD_SPACING, ink);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + vec2(16.0, 8.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
    paint_centred(ui, rect, galley, ink);
    resp
}

/// An outlined status label on a sunken fill.
/// A small clickable box in the badge style: filled with `fill` (the
/// suggested choice) or outlined like a badge; inverts on hover.
pub fn badge_button(ui: &mut Ui, text: &str, fill: Option<Color32>) -> Response {
    let t = Tokens::get(ui.ctx());
    let probe = caps(ui, text, 10.0, theme::mono_bold(), 1.0, t.ink);
    let (rect, resp) = ui.allocate_exact_size(vec2(probe.size().x + 16.0, 24.0), Sense::click());
    let hovered = resp.hovered();
    let (bg, ink) = match (fill, hovered) {
        (_, true) => (t.invert_bg, t.invert_fg),
        (Some(f), false) => (f, t.invert_fg),
        (None, false) => (t.sunken, t.ink),
    };
    let stroke = if fill.is_some() || hovered { egui::Stroke::NONE } else { t.hairline() };
    ui.painter().rect(rect, CornerRadius::ZERO, bg, stroke, egui::StrokeKind::Inside);
    let galley = caps(ui, text, 10.0, theme::mono_bold(), 1.0, ink);
    paint_centred(ui, rect, galley, ink);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn badge(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let t = Tokens::get(ui.ctx());
    let galley = caps(ui, text, 10.0, theme::mono(), 1.0, color);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 12.0, 20.0), Sense::hover());
    ui.painter().rect(rect, CornerRadius::ZERO, t.sunken, t.hairline(), egui::StrokeKind::Inside);
    paint_centred(ui, rect, galley, color);
    resp
}

pub fn rule(ui: &mut Ui, height: f32) {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, height), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), t.hairline());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Solid, inverted colours.
    Primary,
    /// Outlined; inverts on hover.
    Secondary,
    /// Solid red, for dangerous confirmations.
    Danger,
}

/// A square button with a mono upper-case label.
pub fn button(ui: &mut Ui, text: &str, kind: Kind, width: f32) -> Response {
    button_sized(ui, text, kind, width, theme::CONTROL_H)
}

pub fn button_sized(ui: &mut Ui, text: &str, kind: Kind, width: f32, height: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let galley = caps(ui, text, theme::HUD_SIZE, theme::mono_bold(), theme::BUTTON_SPACING, t.ink);
    let size = vec2((galley.size().x + 28.0).max(width), height);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    let hovered = resp.hovered() && enabled;
    let (fill, stroke, color) = match kind {
        Kind::Primary => (t.invert_bg.gamma_multiply(if hovered { 0.8 } else { 1.0 }), Stroke::NONE, t.invert_fg),
        Kind::Danger => (t.bad.gamma_multiply(if hovered { 0.8 } else { 1.0 }), Stroke::NONE, t.invert_fg),
        Kind::Secondary if hovered => (t.invert_bg, Stroke::NONE, t.invert_fg),
        Kind::Secondary => (t.bg, Stroke::new(1.0, t.line_strong), t.ink),
    };
    let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.5) };
    ui.painter().rect(rect, CornerRadius::ZERO, fade(fill), Stroke::new(stroke.width, fade(stroke.color)), egui::StrokeKind::Inside);
    paint_centred(ui, rect, galley, fade(color));
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// A suggestion chip: natural-case text on a sunken fill; inverts on hover.
pub fn chip(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let galley = ui.painter().layout_no_wrap(text.to_owned(), egui::FontId::new(13.0, egui::FontFamily::Proportional), t.ink);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + vec2(24.0, 14.0), Sense::click());
    let hovered = resp.hovered();
    let (fill, ink) = if hovered { (t.invert_bg, t.invert_fg) } else { (t.sunken, t.ink) };
    ui.painter().rect(rect, CornerRadius::ZERO, fill, t.hairline(), egui::StrokeKind::Inside);
    paint_centred(ui, rect, galley, ink);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The light/dark switch: a sun in dark mode (switch to light), a moon in
/// light mode. Drawn, so it needs no symbol font.
pub fn theme_toggle(ui: &mut Ui, dark: bool, height: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(height, height), Sense::click());
    let hovered = resp.hovered();
    let p = ui.painter();
    if hovered {
        p.rect_filled(rect, CornerRadius::ZERO, t.invert_bg);
    }
    p.vline(rect.left() + 0.5, rect.y_range(), t.hairline());
    let (ink, fill) = if hovered { (t.invert_fg, t.invert_bg) } else { (t.muted, t.bg) };
    let c = rect.center();
    if dark {
        p.circle_stroke(c, 4.5, Stroke::new(1.3, ink));
        for i in 0..8 {
            let a = i as f32 * std::f32::consts::TAU / 8.0;
            let d = vec2(a.cos(), a.sin());
            p.line_segment([c + d * 7.0, c + d * 9.5], Stroke::new(1.3, ink));
        }
    } else {
        p.circle_filled(c, 7.0, ink);
        p.circle_filled(c + vec2(3.5, -2.5), 6.0, fill);
    }
    resp.on_hover_text("Light / dark").on_hover_cursor(egui::CursorIcon::PointingHand)
}

const RESIZE_EDGE: f32 = 5.0;

/// Dragging the header's empty space moves the window; double-click maximizes.
pub fn title_bar_drag(ui: &mut Ui) {
    let r = ui.max_rect();
    let rect = Rect::from_min_max(r.min + vec2(RESIZE_EDGE, RESIZE_EDGE), r.max - vec2(RESIZE_EDGE, 0.0));
    let resp = ui.interact(rect, ui.scope_id().with("title-bar"), Sense::click_and_drag());
    if resp.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if resp.double_clicked() {
        let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    }
}

/// Minimize, maximize and close, for a right-to-left layout.
pub fn window_buttons(ui: &mut Ui, height: f32) {
    let ctx = ui.ctx().clone();
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    for (i, label) in ["Close", if maximized { "Restore" } else { "Maximize" }, "Minimize"].into_iter().enumerate() {
        let t = Tokens::get(&ctx);
        let (rect, _) = ui.allocate_exact_size(vec2(46.0, height), Sense::hover());
        let hit = Rect::from_min_max(rect.min + vec2(0.0, RESIZE_EDGE), rect.max);
        let resp = ui.interact(hit, ui.scope_id().with(("window-button", i)), Sense::click());
        let (fill, ink) = match (i, resp.hovered()) {
            (0, true) => (t.bad, t.invert_fg),
            (_, true) => (t.line, t.ink_strong),
            _ => (Color32::TRANSPARENT, t.muted),
        };
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::ZERO, fill);
        let c = rect.center();
        let stroke = Stroke::new(1.0, ink);
        let s = 5.0;
        match i {
            0 => {
                p.line_segment([c + vec2(-s, -s), c + vec2(s, s)], stroke);
                p.line_segment([c + vec2(-s, s), c + vec2(s, -s)], stroke);
            }
            1 => {
                p.rect_stroke(Rect::from_center_size(c, vec2(2.0 * s, 2.0 * s)), CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            }
            _ => {
                p.hline(c.x - s..=c.x + s, c.y, stroke);
            }
        }
        if resp.on_hover_text(label).clicked() {
            match i {
                0 => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                1 => ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized)),
                _ => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            }
        }
    }
    rule(ui, height);
}

/// A hairline round the borderless window, and resizing from its edges.
pub fn window_edges(ctx: &egui::Context) {
    use egui::viewport::ResizeDirection as D;
    use egui::CursorIcon as C;
    let (maximized, fullscreen) = ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false)));
    if maximized || fullscreen {
        return;
    }
    let t = Tokens::get(ctx);
    let r = ctx.content_rect();
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("window-border"))).rect_stroke(
        r,
        CornerRadius::ZERO,
        t.hairline(),
        egui::StrokeKind::Inside,
    );
    let Some(p) = ctx.input(|i| i.pointer.hover_pos()).filter(|p| r.contains(*p)) else { return };
    let near = |d: f32| d < RESIZE_EDGE;
    let corner = |d: f32| d < 3.0 * RESIZE_EDGE;
    let (left, right, top, bottom) = (p.x - r.left(), r.right() - p.x, p.y - r.top(), r.bottom() - p.y);
    let hit = if (near(top) && corner(left)) || (near(left) && corner(top)) {
        Some((D::NorthWest, C::ResizeNorthWest))
    } else if (near(top) && corner(right)) || (near(right) && corner(top)) {
        Some((D::NorthEast, C::ResizeNorthEast))
    } else if (near(bottom) && corner(left)) || (near(left) && corner(bottom)) {
        Some((D::SouthWest, C::ResizeSouthWest))
    } else if (near(bottom) && corner(right)) || (near(right) && corner(bottom)) {
        Some((D::SouthEast, C::ResizeSouthEast))
    } else if near(top) {
        Some((D::North, C::ResizeNorth))
    } else if near(bottom) {
        Some((D::South, C::ResizeSouth))
    } else if near(left) {
        Some((D::West, C::ResizeWest))
    } else if near(right) {
        Some((D::East, C::ResizeEast))
    } else {
        None
    };
    if let Some((direction, cursor)) = hit {
        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
        }
    }
}
