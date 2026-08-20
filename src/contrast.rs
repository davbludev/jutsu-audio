//! Contrast, measured rather than eyeballed.
//!
//! A palette picked by eye on one bright monitor is a palette nobody else can
//! read. WCAG gives an arithmetic answer — the ratio between two colours'
//! relative luminance — so the theme can be checked the way audio is: by
//! computing the number and asserting it.
//!
//! The thresholds are WCAG 2.1 AA: 4.5:1 for body text, 3:1 for large or bold
//! text and for the edges of a control a user has to find. Anything the
//! interface actually draws is checked in the tests below, so changing a colour
//! that falls short fails the build rather than shipping.

use eframe::egui::Color32;

/// Body text against its background.
pub const AA_TEXT: f32 = 4.5;
/// Large text, and the boundaries of controls and graphics.
pub const AA_LARGE: f32 = 3.0;

/// The WCAG contrast ratio between two opaque colours: 1.0 for identical, 21.0
/// for black on white. Order does not matter.
#[must_use]
pub fn ratio(one: Color32, two: Color32) -> f32 {
    let (lighter, darker) = {
        let (a, b) = (relative_luminance(one), relative_luminance(two));
        if a >= b { (a, b) } else { (b, a) }
    };
    (lighter + 0.05) / (darker + 0.05)
}

/// WCAG relative luminance: sRGB channels linearised, then weighted by how much
/// the eye takes from each.
#[must_use]
pub fn relative_luminance(color: Color32) -> f32 {
    let channel = |value: u8| {
        let value = f32::from(value) / 255.0;
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;

    /// Every pair the interface actually draws, with the threshold that pair
    /// has to meet. Adding a colour to the theme means adding it here.
    const PAIRS: &[(&str, Color32, Color32, f32)] = &[
        ("body text on a panel", theme::TEXT, theme::PANEL, AA_TEXT),
        ("body text on the canvas", theme::TEXT, theme::BG, AA_TEXT),
        (
            "body text on a raised control",
            theme::TEXT,
            theme::RAISED,
            AA_TEXT,
        ),
        (
            "secondary text on a panel",
            theme::DIM,
            theme::PANEL,
            AA_TEXT,
        ),
        (
            "secondary text on the canvas",
            theme::DIM,
            theme::BG,
            AA_TEXT,
        ),
        // FAINT is used for hints and captions, never for anything a user has
        // to read to operate the editor, so it is held to the large-text bar.
        ("hint text on a panel", theme::FAINT, theme::PANEL, AA_LARGE),
        (
            "hint text on a raised control",
            theme::FAINT,
            theme::RAISED,
            AA_LARGE,
        ),
        (
            "the playhead and selection",
            theme::SIGNAL,
            theme::PANEL,
            AA_LARGE,
        ),
        (
            "the playhead over the canvas",
            theme::SIGNAL,
            theme::BG,
            AA_LARGE,
        ),
        (
            "audio-is-flowing green",
            theme::LIVE,
            theme::PANEL,
            AA_LARGE,
        ),
        ("destructive red", theme::DANGER, theme::PANEL, AA_LARGE),
        (
            "destructive red on its own ground",
            theme::DANGER,
            theme::DANGER_BG,
            AA_TEXT,
        ),
        (
            "markers and the loop region",
            theme::ACCENT,
            theme::PANEL,
            AA_LARGE,
        ),
        (
            "markers over the canvas",
            theme::ACCENT,
            theme::BG,
            AA_LARGE,
        ),
    ];

    #[test]
    fn the_published_vectors_come_out_right() {
        assert!((ratio(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((ratio(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.001);
        // A known pair from the WCAG examples: #777 on white is 4.48:1, which
        // is the reason 4.5 is a threshold people argue about.
        let grey = Color32::from_rgb(0x77, 0x77, 0x77);
        assert!((ratio(grey, Color32::WHITE) - 4.48).abs() < 0.02);
    }

    #[test]
    fn every_colour_the_interface_draws_meets_its_threshold() {
        let mut failures = Vec::new();
        for (what, foreground, background, threshold) in PAIRS {
            let measured = ratio(*foreground, *background);
            if measured < *threshold {
                failures.push(format!("{what}: {measured:.2}:1, needs {threshold:.1}:1"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn separators_are_visible_without_carrying_meaning() {
        // Rules and grid lines are structure, not information — they only have
        // to be seen, and holding them to a text threshold would mean drawing
        // a grid that fights the waveform on top of it.
        assert!(ratio(theme::RULE, theme::PANEL) > 1.15);
        assert!(ratio(theme::GRID, theme::BG) > 1.05);
    }
}
