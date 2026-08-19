//! Console theme: the shared palette, egui style, and the small drawing helpers
//! every panel reuses.
//!
//! One saturated colour (`SIGNAL`) and it always means the same thing — the
//! playhead, and whatever is selected. Green is reserved for "audio is flowing",
//! red for destructive actions. Everything else is neutral.

use eframe::egui::{self, Color32, CornerRadius, FontId, Response, RichText, Stroke, Vec2};

pub const BG: Color32 = Color32::from_rgb(0x10, 0x12, 0x16);
pub const PANEL: Color32 = Color32::from_rgb(0x17, 0x1a, 0x20);
pub const RAISED: Color32 = Color32::from_rgb(0x1e, 0x22, 0x2a);
pub const RULE: Color32 = Color32::from_rgb(0x26, 0x2b, 0x34);
pub const GRID: Color32 = Color32::from_rgb(0x1c, 0x20, 0x27);
pub const TEXT: Color32 = Color32::from_rgb(0xdf, 0xe3, 0xea);
pub const DIM: Color32 = Color32::from_rgb(0x83, 0x8b, 0x99);
pub const FAINT: Color32 = Color32::from_rgb(0x5c, 0x64, 0x72);
pub const SIGNAL: Color32 = Color32::from_rgb(0xf0, 0xa5, 0x31);
pub const LIVE: Color32 = Color32::from_rgb(0x57, 0xc9, 0x8a);
pub const DANGER: Color32 = Color32::from_rgb(0xe0, 0x73, 0x6b);
/// Markers and the loop region: cool, so they read as positions on the
/// timeline rather than as levels or state.
pub const ACCENT: Color32 = Color32::from_rgb(0x4f, 0xd6, 0xff);
pub const DANGER_BG: Color32 = Color32::from_rgb(0x33, 0x1f, 0x21);

/// Console is a 2px-radius design. One constant so it stays that way.
pub const RADIUS: CornerRadius = CornerRadius::same(2);

pub fn body() -> FontId {
    FontId::proportional(12.0)
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

pub fn configure(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "Inter".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/InterVariable.ttf"
        ))),
    );
    if let Some(proportional) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        proportional.insert(0, "Inter".to_owned());
    }
    context.set_fonts(fonts);

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = BG;
    visuals.faint_bg_color = RAISED;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = RAISED;
    visuals.selection.stroke = Stroke::new(1.0_f32, SIGNAL);
    visuals.window_stroke = Stroke::new(1.0_f32, RULE);

    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = RADIUS;
    }
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, RULE);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, DIM);
    visuals.widgets.inactive.bg_fill = RAISED;
    visuals.widgets.inactive.weak_bg_fill = RAISED;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, RULE);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x28, 0x2d, 0x37);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x28, 0x2d, 0x37);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, SIGNAL);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT);
    visuals.widgets.active.bg_fill = Color32::from_rgb(0x30, 0x36, 0x42);
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(0x30, 0x36, 0x42);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, SIGNAL);

    // A visible keyboard focus ring — egui will not draw one otherwise.
    visuals.widgets.hovered.expansion = 0.0;
    visuals.widgets.active.expansion = 0.0;
    context.set_visuals(visuals);

    let mut style = (*context.style()).clone();
    style.spacing.item_spacing = Vec2::new(6.0, 6.0);
    style.spacing.button_padding = Vec2::new(9.0, 4.0);
    style.spacing.slider_width = 150.0;
    style.spacing.interact_size.y = 22.0;
    for (text_style, font) in [
        (egui::TextStyle::Body, FontId::proportional(12.0)),
        (egui::TextStyle::Button, FontId::proportional(12.0)),
        (egui::TextStyle::Small, FontId::proportional(10.0)),
        (egui::TextStyle::Heading, FontId::proportional(15.0)),
        (egui::TextStyle::Monospace, FontId::monospace(11.0)),
    ] {
        style.text_styles.insert(text_style, font);
    }
    context.set_style(style);
}

pub fn panel(fill: Color32) -> egui::Frame {
    egui::Frame::new().fill(fill)
}

/// The uppercase tracking-wide column label used at the head of every panel.
pub fn column_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .size(9.5)
            .color(FAINT)
            .strong()
            .extra_letter_spacing(1.4),
    );
}

/// A flat toolbar button. `active` gives it the selected treatment.
pub fn tool_button(ui: &mut egui::Ui, text: &str, active: bool) -> Response {
    let button =
        egui::Button::new(
            RichText::new(text)
                .size(11.5)
                .color(if active { SIGNAL } else { DIM }),
        )
        .fill(if active { RAISED } else { Color32::TRANSPARENT })
        .stroke(Stroke::new(
            1.0_f32,
            if active { RULE } else { Color32::TRANSPARENT },
        ))
        .corner_radius(RADIUS);
    ui.add(button)
}

pub fn flat_button(ui: &mut egui::Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(11.5).color(TEXT))
            .fill(RAISED)
            .stroke(Stroke::new(1.0_f32, RULE))
            .corner_radius(RADIUS),
    )
}

pub fn danger_button(ui: &mut egui::Ui, text: &str, width: f32) -> Response {
    ui.add_sized(
        [width, 26.0],
        egui::Button::new(RichText::new(text).size(11.5).color(DANGER))
            .fill(DANGER_BG)
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(0x4a, 0x2a, 0x2a)))
            .corner_radius(RADIUS),
    )
}

/// Segmented peak meter. `level` is linear 0..1; anything at or above
/// `CLIP_AT` lights the top segments in the signal colour.
pub fn peak_meter(ui: &mut egui::Ui, level: f32) -> Response {
    const SEGMENTS: usize = 14;
    const CLIP_AT: usize = 11;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(78.0, 10.0), egui::Sense::hover());
    let painter = ui.painter();
    let gap = 1.5;
    let segment_width = (rect.width() - gap * (SEGMENTS - 1) as f32) / SEGMENTS as f32;
    // Meters read in dB, not amplitude: -60 dBFS at the left, 0 dBFS at the right.
    let normalized = if level <= 0.0 {
        0.0
    } else {
        ((20.0 * level.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
    };
    let lit = (normalized * SEGMENTS as f32).round() as usize;
    for index in 0..SEGMENTS {
        let left = rect.left() + index as f32 * (segment_width + gap);
        let bar = egui::Rect::from_min_size(
            egui::pos2(left, rect.top()),
            Vec2::new(segment_width, rect.height()),
        );
        let color = if index >= lit {
            RULE
        } else if index >= CLIP_AT {
            SIGNAL
        } else {
            LIVE
        };
        painter.rect_filled(bar, CornerRadius::ZERO, color);
    }
    response
}

/// Elides against the real laid-out width. Costs one text layout per candidate,
/// so it belongs on lists with a handful of rows, not on the timeline.
#[must_use]
pub fn elide_measured(ui: &egui::Ui, text: &str, font: &FontId, width: f32) -> String {
    let measure = |candidate: &str| {
        ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(candidate.to_owned(), font.clone(), TEXT)
                .size()
                .x
        })
    };
    if width <= 0.0 || measure(text) <= width {
        return text.to_owned();
    }
    let mut kept = String::new();
    for character in text.chars() {
        let mut candidate = kept.clone();
        candidate.push(character);
        candidate.push('…');
        if measure(&candidate) > width {
            break;
        }
        kept.push(character);
    }
    kept.push('…');
    kept
}

/// Cheap character-count elision. The timeline paints thousands of labels a
/// second, and laying text out just to measure it is not worth the cost.
#[must_use]
pub fn elide(text: &str, width: f32) -> String {
    let budget = (width / 5.6).floor().max(1.0) as usize;
    if text.chars().count() <= budget {
        return text.to_owned();
    }
    let mut shortened: String = text.chars().take(budget.saturating_sub(1)).collect();
    shortened.push('…');
    shortened
}

/// `frames` at `sample_rate`, as `mm:ss.mmm`. A zero rate falls back to 48 kHz
/// rather than dividing by zero.
pub fn format_time(frames: u64, sample_rate: u32) -> String {
    let rate = u64::from(if sample_rate == 0 {
        48_000
    } else {
        sample_rate
    });
    let milliseconds = frames.saturating_mul(1_000) / rate;
    format!(
        "{:02}:{:02}.{:03}",
        milliseconds / 60_000,
        (milliseconds / 1_000) % 60,
        milliseconds % 1_000
    )
}

/// Short ruler label: `0:00` under a minute of material, `1:02` past it.
pub fn format_ruler(seconds: f64) -> String {
    let total = seconds.max(0.0);
    let minutes = (total / 60.0).floor() as u64;
    let remainder = total - minutes as f64 * 60.0;
    if remainder.fract().abs() < 0.001 || total >= 10.0 {
        format!("{minutes}:{:02}", remainder.round() as u64)
    } else {
        format!("{minutes}:{remainder:04.1}")
    }
}

pub fn linear_to_db(level: f32) -> f32 {
    if level <= 0.000_015 {
        f32::NEG_INFINITY
    } else {
        20.0 * level.log10()
    }
}

pub fn format_dbfs(level: f32) -> String {
    let db = linear_to_db(level);
    if db.is_infinite() {
        "  -inf".to_owned()
    } else {
        format!("{db:6.1}")
    }
}
