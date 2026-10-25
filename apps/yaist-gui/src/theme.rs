//! ArtCraft's visual language — hard edges, hairline rules, Archivo display
//! type, Inter body text, Geist Mono labels — in the openSUSE palette.
//! Structure adapted from the ArtCraft launcher's theme (MIT OR Apache-2.0).

use std::sync::Arc;

use egui::epaint::text::VariationCoords;
use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak, Stroke, TextStyle, Visuals};
use yaist_ui::palette::{self, Rgb};

/// The palette's colour roles as egui colours.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub dark: bool,
    pub bg: Color32,
    pub sunken: Color32,
    pub ink: Color32,
    pub ink_strong: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub line: Color32,
    pub line_strong: Color32,
    pub accent: Color32,
    pub brand: Color32,
    pub invert_bg: Color32,
    pub invert_fg: Color32,
    pub good: Color32,
    pub warn: Color32,
    pub bad: Color32,
    pub p: palette::Tokens,
}

pub fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

impl Tokens {
    fn from(p: palette::Tokens) -> Self {
        Self {
            dark: p.dark,
            bg: rgb(p.bg),
            sunken: rgb(p.sunken),
            ink: rgb(p.ink),
            ink_strong: rgb(p.ink_strong),
            muted: rgb(p.muted),
            faint: rgb(p.faint),
            line: rgb(p.line),
            line_strong: rgb(p.line_strong),
            accent: rgb(p.accent),
            brand: rgb(p.brand),
            invert_bg: rgb(p.invert_bg),
            invert_fg: rgb(p.invert_fg),
            good: rgb(p.good),
            warn: rgb(p.warn),
            bad: rgb(p.bad),
            p,
        }
    }

    pub fn get(ctx: &egui::Context) -> Self {
        if ctx.theme() == egui::Theme::Dark {
            Self::from(palette::Tokens::dark())
        } else {
            Self::from(palette::Tokens::light())
        }
    }

    pub fn hairline(&self) -> Stroke {
        Stroke::new(1.0, self.line)
    }

    pub fn module(&self, i: usize) -> Color32 {
        rgb(self.p.module_accent(i))
    }

    pub fn tone(&self, tone: yaist_ui::Tone) -> Color32 {
        rgb(self.p.tone(tone))
    }

    pub fn risk(&self, risk: yaist_core::Risk) -> Color32 {
        rgb(self.p.risk(risk))
    }
}

pub fn shadow(t: &Tokens) -> egui::epaint::Shadow {
    egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(if t.dark { 140 } else { 36 }) }
}

fn visuals(t: &Tokens) -> Visuals {
    let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.bg;
    v.window_fill = t.bg;
    v.extreme_bg_color = t.sunken;
    v.text_edit_bg_color = Some(t.sunken);
    v.faint_bg_color = t.sunken;
    v.code_bg_color = t.sunken;
    v.window_stroke = Stroke::new(1.0, t.line_strong);
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.window_shadow = shadow(t);
    v.popup_shadow = shadow(t);
    v.override_text_color = Some(t.ink);
    v.hyperlink_color = t.accent;
    v.selection.bg_fill = t.accent;
    v.selection.stroke = Stroke::new(1.0, t.invert_fg);
    v.warn_fg_color = t.warn;
    v.error_fg_color = t.bad;
    for (w, fill, stroke, fg) in [
        (&mut v.widgets.noninteractive, t.bg, t.line, t.muted),
        (&mut v.widgets.inactive, t.bg, t.line, t.ink),
        (&mut v.widgets.hovered, t.sunken, t.line_strong, t.ink_strong),
        (&mut v.widgets.active, t.sunken, t.line_strong, t.ink_strong),
        (&mut v.widgets.open, t.sunken, t.line_strong, t.ink_strong),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0, stroke);
        w.corner_radius = CornerRadius::ZERO;
        w.fg_stroke = Stroke::new(1.0, fg);
        w.expansion = 0.0;
    }
    v
}

pub fn apply(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Dark, visuals(&Tokens::from(palette::Tokens::dark())));
    ctx.set_visuals_of(egui::Theme::Light, visuals(&Tokens::from(palette::Tokens::light())));
    // YAIST_THEME=light starts in the light scheme (screenshots).
    let light = std::env::var("YAIST_THEME").is_ok_and(|t| t == "light");
    ctx.set_theme(if light { egui::ThemePreference::Light } else { egui::ThemePreference::Dark });
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.icon_spacing = 10.0;
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.interact_size.y = CONTROL_H;
        style.spacing.menu_margin = egui::Margin::same(4);
        style.spacing.window_margin = egui::Margin::same(4);
        style.text_styles = [
            (TextStyle::Heading, FontId::new(24.0, display())),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(13.5, medium())),
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();
    });
}

/// Height of buttons and inputs.
pub const CONTROL_H: f32 = 34.0;
/// Mono labels: 11 px Geist Mono, upper case, 0.15 em apart.
pub const HUD_SIZE: f32 = 11.0;
pub const HUD_SPACING: f32 = HUD_SIZE * 0.15;
pub const BUTTON_SPACING: f32 = HUD_SIZE * 0.12;
pub const PAD: f32 = 24.0;
pub const HEADER_H: f32 = 52.0;
pub const FOOTER_H: f32 = 32.0;

pub fn medium() -> FontFamily {
    FontFamily::Name("medium".into())
}
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}
pub fn display() -> FontFamily {
    FontFamily::Name("display".into())
}
pub fn mono() -> FontFamily {
    FontFamily::Name("mono".into())
}
pub fn mono_bold() -> FontFamily {
    FontFamily::Name("mono-bold".into())
}

type Face<'a> = (&'a str, &'static str, &'a [([u8; 4], f32)], FontFamily);

/// Installs the three variable fonts at the weights (and width) the design
/// uses. Every family falls back to Inter, which also covers the symbols.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::empty();
    let faces: [Face<'_>; 7] = [
        ("Inter", "Inter-Variable.ttf", &[(*b"wght", 400.0), (*b"opsz", 14.0)], FontFamily::Proportional),
        ("Inter-Medium", "Inter-Variable.ttf", &[(*b"wght", 500.0), (*b"opsz", 14.0)], medium()),
        ("Inter-SemiBold", "Inter-Variable.ttf", &[(*b"wght", 600.0), (*b"opsz", 14.0)], semibold()),
        ("Archivo-Display", "Archivo-Variable.ttf", &[(*b"wght", 620.0), (*b"wdth", 118.0)], display()),
        ("GeistMono", "GeistMono-Variable.ttf", &[(*b"wght", 400.0)], FontFamily::Monospace),
        ("GeistMono-Medium", "GeistMono-Variable.ttf", &[(*b"wght", 500.0)], mono()),
        ("GeistMono-Bold", "GeistMono-Variable.ttf", &[(*b"wght", 700.0)], mono_bold()),
    ];
    for (name, file, coords, family) in faces {
        let Some(data) = font_data(file) else {
            eprintln!("yaist: font {file} not found; text in that style falls back to Inter");
            fonts.families.insert(family, vec!["Inter".to_owned()]);
            continue;
        };
        // These fonts carry no hinting; subpixel binning would blur small text.
        let tweak = FontTweak { coords: VariationCoords::new(coords.iter().copied()), subpixel_binning: Some(false), ..Default::default() };
        fonts.font_data.insert(name.to_owned(), Arc::new(data.tweak(tweak)));
        let mut stack = vec![name.to_owned()];
        if name != "Inter" {
            stack.push("Inter".to_owned());
        }
        fonts.families.insert(family, stack);
    }
    ctx.set_fonts(fonts);
}

#[cfg(feature = "bundled-fonts")]
fn font_data(file: &str) -> Option<FontData> {
    let bytes: &'static [u8] = match file {
        "Inter-Variable.ttf" => include_bytes!("../../../assets/fonts/Inter-Variable.ttf"),
        "Archivo-Variable.ttf" => include_bytes!("../../../assets/fonts/Archivo-Variable.ttf"),
        "GeistMono-Variable.ttf" => include_bytes!("../../../assets/fonts/GeistMono-Variable.ttf"),
        _ => return None,
    };
    Some(FontData::from_static(bytes))
}

#[cfg(not(feature = "bundled-fonts"))]
fn font_data(file: &str) -> Option<FontData> {
    let dir = std::env::var("YAIST_FONT_DIR").unwrap_or_else(|_| "/usr/share/yaist/fonts".into());
    std::fs::read(std::path::Path::new(&dir).join(file)).ok().map(FontData::from_owned)
}
