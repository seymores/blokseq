//! Palette + glyphs. Deliberately a single struct so a mockup frame can be
//! rendered in `--theme light` later without touching the widgets.

use ratatui::style::{Color, Modifier, Style};

pub const BG: Color = Color::Rgb(13, 17, 23);
pub const PANEL: Color = Color::Rgb(17, 22, 30);
pub const PANEL_ALT: Color = Color::Rgb(22, 28, 38);
pub const BORDER: Color = Color::Rgb(45, 55, 72);
pub const BORDER_FOCUS: Color = Color::Rgb(96, 130, 200);
pub const FG: Color = Color::Rgb(205, 214, 228);
pub const DIM: Color = Color::Rgb(108, 122, 143);
pub const FAINT: Color = Color::Rgb(70, 80, 98);
pub const ACCENT: Color = Color::Rgb(122, 162, 247);
pub const GREEN: Color = Color::Rgb(126, 231, 135);
pub const YELLOW: Color = Color::Rgb(230, 195, 110);
pub const ORANGE: Color = Color::Rgb(255, 158, 100);
pub const PURPLE: Color = Color::Rgb(187, 154, 247);
pub const CYAN: Color = Color::Rgb(115, 218, 202);
pub const RED: Color = Color::Rgb(247, 118, 142);
pub const SELECT_BG: Color = Color::Rgb(35, 46, 66);
pub const SELECT_BG_DIM: Color = Color::Rgb(26, 34, 49);
pub const CARET: Color = Color::Rgb(255, 213, 128);

pub struct Theme;

impl Theme {
    pub fn base() -> Style {
        Style::default().fg(FG).bg(BG)
    }
    pub fn panel() -> Style {
        Style::default().fg(FG).bg(PANEL)
    }
    pub fn dim() -> Style {
        Style::default().fg(DIM).bg(PANEL)
    }
    pub fn faint() -> Style {
        Style::default().fg(FAINT).bg(PANEL)
    }
    pub fn border(focused: bool) -> Style {
        let c = if focused { BORDER_FOCUS } else { BORDER };
        Style::default().fg(c).bg(PANEL)
    }
    pub fn title() -> Style {
        Style::default()
            .fg(ACCENT)
            .bg(BG)
            .add_modifier(Modifier::BOLD)
    }
    pub fn section() -> Style {
        Style::default()
            .fg(FAINT)
            .bg(PANEL)
            .add_modifier(Modifier::BOLD)
    }
    pub fn section_focus() -> Style {
        Style::default()
            .fg(ACCENT)
            .bg(PANEL)
            .add_modifier(Modifier::BOLD)
    }
    pub fn selected() -> Style {
        Style::default().fg(FG).bg(SELECT_BG)
    }
    pub fn selected_dim() -> Style {
        Style::default().fg(DIM).bg(SELECT_BG)
    }
    pub fn statusbar() -> Style {
        Style::default().fg(FG).bg(PANEL_ALT)
    }
    pub fn hintbar() -> Style {
        Style::default().fg(DIM).bg(BG)
    }
    pub fn badge(bg: Color) -> Style {
        Style::default()
            .fg(BG)
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    }
    pub fn key() -> Style {
        Style::default()
            .fg(CYAN)
            .bg(BG)
            .add_modifier(Modifier::BOLD)
    }
    pub fn overlay() -> Style {
        Style::default().fg(FG).bg(PANEL_ALT)
    }
    pub fn popup_border() -> Style {
        Style::default().fg(ACCENT).bg(PANEL_ALT)
    }
    pub fn caret() -> Style {
        Style::default()
            .fg(CARET)
            .bg(SELECT_BG)
            .add_modifier(Modifier::BOLD)
    }

    /// The caret inside the block editor: the cell it sits on, inverted. A
    /// *style*, never an extra character -- an inserted glyph pushed every
    /// character after the cursor one cell to the right, so the text crawled
    /// sideways whenever the caret moved.
    pub fn caret_block() -> Style {
        Style::default().fg(BG).bg(CARET)
    }
}

/// Status keyword -> colour, Logseq's TODO/DOING/DONE/NOW/LATER set.
pub fn status_color(status: &str) -> Color {
    match status {
        "TODO" => YELLOW,
        "DOING" | "NOW" => ACCENT,
        "DONE" => GREEN,
        "LATER" | "WAITING" => PURPLE,
        "CANCELED" => RED,
        _ => DIM,
    }
}

pub const MODES: [&str; 5] = ["TODO", "DOING", "DONE", "LATER", "CANCELED"];

/// Cycle order for Ctrl-Enter.
pub fn next_status(cur: Option<&str>) -> Option<&'static str> {
    match cur {
        None => Some("TODO"),
        Some("TODO") => Some("DOING"),
        Some("DOING") => Some("DONE"),
        Some("DONE") => None,
        Some("LATER") => Some("DOING"),
        Some("CANCELED") => None,
        _ => Some("TODO"),
    }
}
