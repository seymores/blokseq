//! Rendering. Every mockup frame in `dumps/` is produced by this file, so the
//! screenshots are the real renderer rather than drawings of one.

use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Focus, ToastKind, View, KEYMAP};
use crate::db::human_bytes;
use crate::editor::{Editor, Mode};
use crate::model::{properties, JournalDay, Row};
use crate::theme::{self, Theme};

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    // No top bar. What you are looking at is the pane's own title, and what you
    // can do is the hint bar; a second header row only repeated both.
    // The command line and the one-line message area borrow a row while in use.
    let ex_h = if app.ex.is_some() { 1 } else { 0 };
    let msg_h = if app.toast.is_some() { 1 } else { 0 };
    let chunks = Layout::vertical([
        Constraint::Min(4),        // body
        Constraint::Length(msg_h), // what just happened; any key clears it
        Constraint::Length(ex_h),  // : command line
        Constraint::Length(1),     // status
        Constraint::Length(1),     // hints
    ])
    .split(area);

    let caret = match app.view {
        View::Journal(_) | View::Page(_) => render_workspace(f, app, chunks[0]),
        View::Search => {
            render_search(f, app, chunks[0]);
            None
        }
        View::Query => {
            render_query(f, app, chunks[0]);
            None
        }
        View::Backup => {
            render_backup(f, app, chunks[0]);
            None
        }
        View::Help => {
            render_help(f, app, chunks[0]);
            None
        }
        View::Sql => {
            render_sql(f, app, chunks[0]);
            None
        }
    };

    if let Some((x, y)) = caret {
        if app.popup.is_some() {
            render_popup(f, app, x, y, chunks[0]);
        }
    }
    if let Some(p) = app.palette.clone() {
        render_palette(f, app, &p, chunks[0]);
    }
    if msg_h > 0 {
        render_message(f, app, chunks[1]);
    }
    if ex_h > 0 {
        render_ex(f, app, chunks[2]);
    }
    render_status(f, app, chunks[3]);
    render_hints(f, app, chunks[4]);
}

// --------------------------------------------------------------- workspace

/// Journal / page screen: the outline, and optionally the references beside it.
/// There is no page sidebar: `Ctrl-P` (Find) is how you go somewhere.
fn render_workspace(f: &mut Frame, app: &mut App, area: Rect) -> Option<(u16, u16)> {
    let w = area.width;
    // References carry real text, so they need real width; below ~96 columns
    // they crowd the outline and step aside; Ctrl-M is one key away.
    let right_w = if !app.show_meta {
        0
    } else if w >= 140 {
        40
    } else if w >= 120 {
        34
    } else if w >= 106 {
        30
    } else if w >= 96 {
        26
    } else {
        0
    };
    let chunks =
        Layout::horizontal([Constraint::Min(40), Constraint::Length(right_w)]).split(area);

    let caret = render_outliner(f, app, chunks[0]);
    if right_w > 0 {
        render_refs(f, app, chunks[1]);
    }
    caret
}

fn panel(title: &str, focused: bool, extra: Option<Span<'static>>) -> Block<'static> {
    let mut t = vec![Span::styled(
        format!(" {} ", title),
        if focused {
            Theme::section_focus()
        } else {
            Theme::section()
        },
    )];
    if let Some(e) = extra {
        t.push(e);
    }
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Theme::border(focused))
        .style(Theme::panel())
        .title(Line::from(t))
}

// ----------------------------------------------------------------- outliner

fn render_outliner(f: &mut Frame, app: &mut App, area: Rect) -> Option<(u16, u16)> {
    let focused = app.focus == Focus::Main;
    // The pane title is the *only* place you are told what you are looking at:
    // a journal day by date, a page by name, an unwritten day as provisional.
    let title = match &app.view {
        View::Journal(day) => {
            if app.provisional {
                format!("{} · provisional", day.key())
            } else {
                day.key().to_string()
            }
        }
        View::Page(name) => name.clone(),
        _ => "blocks".to_string(),
    };
    let extra = match &app.view {
        View::Journal(day) => {
            let when = if day.date == app.today {
                "today".to_string()
            } else {
                day.relative(app.today)
            };
            Some(Span::styled(
                format!(" {} · {} blocks ", when, app.rows.len()),
                Theme::section().fg(theme::DIM),
            ))
        }
        View::Page(_) => {
            let refs: usize = app.linked.iter().map(|(_, v)| v.len()).sum();
            Some(Span::styled(
                format!(" {} blocks · {} refs ", app.rows.len(), refs),
                Theme::section().fg(theme::DIM),
            ))
        }
        _ => None,
    };
    f.render_widget(panel(&title, focused, extra), area);
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    if inner.width < 8 || inner.height < 3 {
        return None;
    }

    // No header strip: the pane's own title carries the page identity, and a
    // second copy of the date plus a TODAY badge was the top bar's redundancy
    // wearing a different hat.
    let header: Vec<Line> = Vec::new();

    let body_height = inner.height.saturating_sub(header.len() as u16) as usize;
    let width = inner.width as usize;

    // Provisional journal with no rows yet: a ghost first block.
    if app.rows.is_empty() {
        if let Some(ed) = app.editor.clone() {
            let ghost = Row {
                id: -1,
                uuid: String::new(),
                depth: 0,
                content: String::new(),
                status: None,
                collapsed: false,
                has_children: false,
                child_count: 0,
                last_sibling: true,
                ancestor_last: Vec::new(),
            };
            let (ls, cur) = editor_lines(app, &ed, width, &ghost);
            let mut lines = header;
            let base = inner.y + lines.len() as u16;
            lines.extend(ls);
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("   ", Theme::panel()),
                Span::styled(
                    "the first keystroke is what creates this day",
                    Theme::panel().fg(theme::ORANGE),
                ),
            ]));
            f.render_widget(
                Paragraph::new(lines)
                    .style(Theme::panel())
                    .wrap(Wrap { trim: false }),
                inner,
            );
            return Some((inner.x + cur.0, base + cur.1));
        }
        let mut lines = header;
        let ghost = match &app.view {
            View::Journal(_) => vec![
                Line::from(vec![
                    Span::styled("▌ ", Style::default().fg(theme::ACCENT).bg(theme::PANEL)),
                    Span::styled("▏", Theme::caret()),
                    Span::styled(
                        "type to start today's journal — nothing is created until you do",
                        Theme::panel().fg(theme::FAINT),
                    ),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled("   ", Theme::panel()),
                    Span::styled("i", Theme::key()),
                    Span::styled("  edit   ", Theme::dim()),
                    Span::styled("/", Theme::key()),
                    Span::styled("  commands   ", Theme::dim()),
                    Span::styled("[[", Theme::key()),
                    Span::styled("  link a page   ", Theme::dim()),
                    Span::styled("Ctrl-P", Theme::key()),
                    Span::styled("  palette", Theme::dim()),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled("   ", Theme::panel()),
                    Span::styled(
                        "empty journals are pruned: no page row, no file, no orphan day",
                        Theme::panel().fg(theme::FAINT),
                    ),
                ]),
            ],
            _ => vec![
                Line::from(vec![
                    Span::styled("▌ ", Style::default().fg(theme::ACCENT).bg(theme::PANEL)),
                    Span::styled("▏", Theme::caret()),
                    Span::styled("empty page — start typing", Theme::panel().fg(theme::FAINT)),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "   o  new first block",
                    Theme::dim(),
                )),
            ],
        };
        lines.extend(ghost);
        f.render_widget(Paragraph::new(lines).style(Theme::panel()), inner);
        return Some((inner.x + 3, inner.y + 1));
    }

    // Scrolling window over the flattened rows.
    let sel = app.selected;
    let start = if sel >= app.scroll as usize + body_height {
        sel + 1 - body_height
    } else {
        app.scroll as usize
    };
    let show_count = body_height + 2;

    let mut lines: Vec<Line> = header;
    let mut caret: Option<(u16, u16)> = None;
    // Where the cursor row sits, so a popup with no editor (the `Ctrl-]` link
    // chooser) still has something to hang off.
    let mut sel_y: Option<u16> = None;
    let editing_id = app.editor.as_ref().map(|e| e.block_id);

    for (offset, row) in app.rows.iter().enumerate().skip(start).take(show_count) {
        let selected = offset == sel && app.focus == Focus::Main;
        if selected {
            sel_y = Some(inner.y + lines.len() as u16);
        }
        let in_visual = match app.visual {
            Some(anchor) => {
                let (lo, hi) = if anchor <= sel { (anchor, sel) } else { (sel, anchor) };
                offset >= lo && offset <= hi
            }
            None => false,
        };
        let is_editing = Some(row.id) == editing_id;

        if is_editing {
            let ed = app.editor.as_ref().unwrap();
            let (ls, cur) = editor_lines(app, ed, width, row);
            let base_y = inner.y + lines.len() as u16;
            for (i, l) in ls.iter().enumerate() {
                let _ = i;
                lines.push(l.clone());
            }
            caret = Some((
                inner.x + cur.0,
                base_y + cur.1,
            ));
        } else {
            let mut ls = block_lines(app, row, width, selected);
            if in_visual && !selected {
                ls = ls
                    .into_iter()
                    .map(|mut l| {
                        l.spans = l
                            .spans
                            .into_iter()
                            .map(|s| {
                                let bg = theme::SELECT_BG_DIM;
                                Span::styled(s.content, s.style.bg(bg))
                            })
                            .collect();
                        l
                    })
                    .collect();
            }
            lines.extend(ls);
        }
    }

    // Provisional / empty padding + ghost block at the end of a journal.
    if app.view.is_journal() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  ", Theme::panel()),
            Span::styled("• ", Theme::panel().fg(theme::FAINT)),
            Span::styled(
                "type here for the next block — or press o",
                Theme::panel().fg(theme::FAINT),
            ),
        ]));
    }

    let tail: Vec<Line> = lines.into_iter().skip(0).collect();
    f.render_widget(
        Paragraph::new(tail)
            .style(Theme::panel())
            .wrap(Wrap { trim: false }),
        inner,
    );

    if app.rows.len() > body_height {
        let frac = (sel as f64 + 1.0) / app.rows.len() as f64;
        let track = body_height.saturating_sub(1) as f64;
        let y = inner.y + (frac * track) as u16;
        let bar = Rect {
            x: inner.x + inner.width - 1,
            y,
            width: 1,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Span::styled("┃", Style::default().fg(theme::BORDER_FOCUS))),
            bar,
        );
    }
    caret.or(sel_y.map(|y| (inner.x + 4, y)))
}

impl View {
    pub fn is_journal(&self) -> bool {
        matches!(self, View::Journal(_))
    }
}

/// Prefix with indent guides + bullet, then the block content.
fn block_lines(app: &App, row: &Row, width: usize, selected: bool) -> Vec<Line<'static>> {
    let bg = if selected {
        theme::SELECT_BG
    } else {
        theme::PANEL
    };
    let mut prefix: Vec<Span> = Vec::new();
    if selected {
        prefix.push(Span::styled("▌", Style::default().fg(theme::ACCENT).bg(bg)));
    } else {
        prefix.push(Span::styled(" ", Style::default().bg(bg)));
    }
    for (i, last) in row.ancestor_last.iter().enumerate() {
        let g = if *last { "  " } else { "│ " };
        let c = if *last { theme::PANEL } else { theme::FAINT };
        prefix.push(Span::styled(g, Style::default().fg(c).bg(bg)));
        let _ = i;
    }
    let bullet = if row.has_children {
        if row.collapsed {
            "▸ "
        } else {
            "▾ "
        }
    } else {
        "• "
    };
    let bullet_style = if selected {
        Style::default().fg(theme::ACCENT).bg(bg)
    } else if row.has_children {
        Style::default().fg(theme::DIM).bg(bg)
    } else {
        Style::default().fg(theme::FAINT).bg(bg)
    };
    prefix.push(Span::styled(bullet, bullet_style));

    let prefix_w = prefix_width(&prefix);
    let text_w = width.saturating_sub(prefix_w);
    // A block with links says so, so that "can I go somewhere from here?" is
    // answerable by looking. `Ctrl-]` / `gf` follows them.
    let links = crate::model::parse_refs(&row.content).len();
    let mut out: Vec<Line> = Vec::new();
    // Alt-Enter puts real newlines inside one block; render them as continuation
    // lines that hang under the text, not under the bullet.
    for (i, para) in row.content.split('\n').enumerate() {
        let mut spans: Vec<Span> = if i == 0 {
            prefix.clone()
        } else {
            vec![Span::styled(" ".repeat(prefix_w), Style::default().bg(bg))]
        };
        spans.extend(inline_spans(app, para, text_w, bg, selected));
        if i == 0 && links > 0 {
            spans.push(Span::styled(
                format!(" ↗{}", links),
                Style::default()
                    .fg(if selected { theme::ACCENT } else { theme::FAINT })
                    .bg(bg),
            ));
        }
        if i == 0 && row.collapsed && row.has_children {
            spans.push(Span::styled(
                format!("  ▸ {} collapsed", row.child_count),
                Style::default().fg(theme::FAINT).bg(bg),
            ));
        }
        out.push(Line::from(spans));
    }
    if out.is_empty() {
        out.push(Line::from(prefix));
    }
    out
}

fn prefix_width(spans: &[Span]) -> usize {
    spans
        .iter()
        .map(|s| Line::from(s.clone()).width())
        .sum::<usize>()
}

/// The block editor: soft-wrapped lines with an explicit caret. Returns the
/// lines and the caret offset (col, row) relative to the start of the block.
fn editor_lines(app: &App, ed: &Editor, width: usize, row: &Row) -> (Vec<Line<'static>>, (u16, u16)) {
    let bg = theme::SELECT_BG;
    let mut prefix: Vec<Span> = vec![Span::styled(
        "▌",
        Style::default().fg(theme::CARET).bg(bg),
    )];
    for last in &row.ancestor_last {
        let g = if *last { "  " } else { "│ " };
        let c = if *last { theme::PANEL } else { theme::FAINT };
        prefix.push(Span::styled(g, Style::default().fg(c).bg(bg)));
    }
    let bullet = if row.has_children {
        if row.collapsed {
            "▸ "
        } else {
            "▾ "
        }
    } else {
        "• "
    };
    prefix.push(Span::styled(bullet, Style::default().fg(theme::CARET).bg(bg)));
    let hang = " ".repeat(prefix_width(&prefix));

    let text: String = ed.chars.iter().collect();
    let styled = styled_chars(app, &text, bg, true);
    // keep the caret on screen: one column narrower than the pane
    let wrap_w = width.saturating_sub(prefix_width(&prefix) - 1).max(8);
    let lines_idx = wrap_ranges(&ed.chars, wrap_w);

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut caret = (prefix_width(&prefix) as u16, 0u16);
    for (n, (start, end)) in lines_idx.iter().enumerate() {
        let mut spans: Vec<Span> = if n == 0 {
            prefix.clone()
        } else {
            vec![Span::styled(hang.clone(), Style::default().bg(bg))]
        };
        let mut run: Vec<(char, Style)> = Vec::new();
        for i in *start..*end {
            if i == ed.cursor {
                caret = (prefix_width(&spans) as u16, n as u16);
                if ed.fake_caret {
                    run.push(('▏', Theme::caret().bg(bg)));
                }
            }
            if let Some((c, st)) = styled.get(i) {
                run.push((*c, st.patch(Style::default().bg(bg))));
            }
        }
        if ed.cursor >= *end && (ed.cursor == *end) && n + 1 == lines_idx.len() {
            caret = ((prefix_width(&spans) + run.len()) as u16, n as u16);
            if ed.fake_caret {
                run.push(('▏', Theme::caret().bg(bg)));
            }
        }
        spans.extend(runs_to_spans(run));
        out.push(Line::from(spans));
    }
    (out, caret)
}

/// The page-metadata panel, shown by `Ctrl-M`. It leads with what the page *is*
/// — kind, timestamps, counts, properties — and then the references, because
/// "what is this page" and "what points at it" are the same question asked twice.
fn render_refs(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Right;
    let total: usize = app.linked.iter().map(|(_, v)| v.len()).sum();
    f.render_widget(
        panel(
            "PAGE METADATA",
            focused,
            Some(Span::styled(
                format!(" {} refs ", total),
                Theme::section().fg(theme::ACCENT),
            )),
        ),
        area,
    );
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let mut lines: Vec<Line> = Vec::new();

    // --- the metadata itself
    if let Some(m) = app.meta.as_ref() {
        let kv = |k: &str, v: String, color: ratatui::style::Color| {
            Line::from(vec![
                Span::styled(format!(" {:<11}", k), Theme::panel().fg(theme::FAINT)),
                Span::styled(v, Style::default().fg(color).bg(theme::PANEL)),
            ])
        };
        lines.push(kv(
            "kind",
            if m.is_journal { "journal" } else { "page" }.to_string(),
            if m.is_journal { theme::GREEN } else { theme::ACCENT },
        ));
        lines.push(kv("created", short_stamp(&m.created_at), theme::DIM));
        lines.push(kv("updated", short_stamp(&m.updated_at), theme::DIM));
        lines.push(kv(
            "blocks",
            format!(
                "{} block{}",
                m.blocks,
                if m.blocks == 1 { "" } else { "s" }
            ),
            theme::FG,
        ));
        lines.push(kv(
            "links",
            format!("{} in · {} out", m.refs_in, m.refs_out),
            theme::FG,
        ));
        if m.properties.is_empty() {
            lines.push(kv("properties", "none".into(), theme::FAINT));
        } else {
            for (k, v) in m.properties.iter().take(6) {
                lines.push(kv(
                    k,
                    truncate(v, inner.width.saturating_sub(15) as usize),
                    theme::CYAN,
                ));
            }
        }
        lines.push(Line::from(Span::styled(
            "─".repeat(inner.width as usize),
            Theme::faint(),
        )));
    }

    // --- linked references
    lines.push(Line::from(vec![
        Span::styled("LINKED ", Theme::section()),
        Span::styled(format!("({})", total), Theme::faint()),
    ]));
    let mut idx = 0usize;
    for (page, hits) in &app.linked {
        let badge = if let Some(d) = crate::model::parse_journal_key(page) {
            d.relative(app.today)
        } else {
            page.clone()
        };
        let focus_style = if focused {
            Theme::section_focus()
        } else {
            Theme::section()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", badge), focus_style),
            Span::styled(format!("({})", hits.len()), Theme::faint()),
        ]));
        for h in hits {
            let selected = focused && idx == app.linked_selected;
            let bg = if selected { theme::SELECT_BG } else { theme::PANEL };
            let style = Style::default().fg(theme::FG).bg(bg);
            let content_w = inner.width.saturating_sub(4) as usize;
            let flat = h.content.replace('\n', " ");
            let text: String = if flat.chars().count() > content_w {
                flat.chars().take(content_w.saturating_sub(1)).collect::<String>() + "…"
            } else {
                flat
            };
            lines.push(Line::from(vec![
                Span::styled(
                    if selected { "▸ " } else { "· " },
                    Style::default()
                        .fg(if selected { theme::ACCENT } else { theme::FAINT })
                        .bg(bg),
                ),
                Span::styled(text, style),
            ]));
            idx += 1;
        }
        lines.push(Line::from(""));
    }
    if app.linked.is_empty() {
        lines.push(Line::from(Span::styled(
            " no links yet — [[page]] or ((block)) makes one",
            Theme::dim().fg(theme::FAINT),
        )));
    }

    // --- unlinked mentions
    lines.push(Line::from(Span::styled(
        "─".repeat(inner.width as usize),
        Theme::faint(),
    )));
    lines.push(Line::from(vec![
        Span::styled("UNLINKED ", Theme::section()),
        Span::styled("(mentions of this title)", Theme::faint()),
    ]));
    for name in app.unlinked.iter().take(4) {
        let room = inner.width.saturating_sub(3) as usize;
        lines.push(Line::from(vec![
            Span::styled("· ", Theme::faint()),
            Span::styled(
                truncate(&name.content.replace('\n', " "), room),
                Theme::panel().fg(theme::FAINT),
            ),
        ]));
    }
    if app.unlinked.is_empty() {
        lines.push(Line::from(Span::styled("· none", Theme::faint())));
    }
    f.render_widget(Paragraph::new(lines).style(Theme::panel()), inner);
}

/// `2026-10-08T09:41:12` -> `2026-10-08 09:41`
fn short_stamp(ts: &str) -> String {
    let (d, t) = ts.split_once('T').unwrap_or((ts, ""));
    let hhmm: String = t.chars().take(5).collect();
    if hhmm.is_empty() {
        d.to_string()
    } else {
        format!("{} {}", d, hhmm)
    }
}

// ------------------------------------------------------------ inline parser

/// Char-level styling for block text. One function so the outliner and the
/// editor can never drift apart.
pub fn styled_chars(app: &App, text: &str, bg: ratatui::style::Color, _editing: bool) -> Vec<(char, Style)> {
    let base = Style::default().fg(theme::FG).bg(bg);
    let mut out: Vec<(char, Style)> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    let mut line_start = true;
    // A leading status keyword on the first line becomes a badge.
    let mut status_len = 0usize;
    for (idx, w) in ["CANCELED", "DOING", "LATER", "TODO", "DONE", "NOW", "WAITING"]
        .iter()
        .enumerate()
    {
        let _ = idx;
        let wc: Vec<char> = w.chars().collect();
        if chars.len() >= wc.len() && chars[..wc.len()] == wc[..] {
            let after_ok = chars
                .get(wc.len())
                .map(|c| c.is_whitespace())
                .unwrap_or(true);
            if after_ok {
                status_len = wc.len();
                let color = theme::status_color(w);
                for c in &wc {
                    out.push((*c, Style::default().fg(theme::BG).bg(color).bold()));
                }
                if let Some(c) = chars.get(wc.len()) {
                    out.push((*c, base));
                }
                i = wc.len() + 1;
                break;
            }
        }
    }
    let _ = status_len;

    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            out.push((c, base));
            line_start = true;
            i += 1;
            continue;
        }
        // [[page]] and ((block)) and #tag
        if c == '[' && chars.get(i + 1) == Some(&'[') {
            if let Some(end) = find_seq(&chars, i + 2, ']', ']') {
                let name: String = chars[i + 2..end].iter().collect();
                let style = Style::default().fg(theme::ACCENT).bg(bg).bold();
                let bracket = Style::default().fg(theme::FAINT).bg(bg);
                out.push(('[', bracket));
                out.push(('[', bracket));
                for ch in name.chars() {
                    out.push((ch, style));
                }
                out.push((']', bracket));
                out.push((']', bracket));
                i = end + 2;
                continue;
            }
        }
        if c == '(' && chars.get(i + 1) == Some(&'(') {
            if let Some(end) = find_seq(&chars, i + 2, ')', ')') {
                let uuid: String = chars[i + 2..end].iter().collect();
                let label = app
                    .db
                    .block_by_uuid(&uuid)
                    .map(|b| crate::app::snippet(&b.content, 28))
                    .unwrap_or_else(|| "unresolved".into());
                let style = Style::default().fg(theme::PURPLE).bg(bg);
                let paren = Style::default().fg(theme::FAINT).bg(bg);
                out.push(('(', paren));
                out.push(('(', paren));
                for ch in format!("↗ {}", label).chars() {
                    out.push((ch, style));
                }
                out.push((')', paren));
                out.push((')', paren));
                i = end + 2;
                continue;
            }
        }
        if c == '#' && (i == 0 || chars[i - 1].is_whitespace()) {
            let mut j = i + 1;
            while j < chars.len()
                && (chars[j].is_alphanumeric() || chars[j] == '-' || chars[j] == '_' || chars[j] == '/')
            {
                j += 1;
            }
            if j > i + 1 {
                let style = Style::default().fg(theme::GREEN).bg(bg);
                out.push(('#', Style::default().fg(theme::GREEN).bg(bg).dim()));
                for ch in chars[i + 1..j].iter() {
                    out.push((*ch, style));
                }
                i = j;
                continue;
            }
        }
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            if let Some(end) = find_seq(&chars, i + 2, '*', '*') {
                let inner: String = chars[i + 2..end].iter().collect();
                let style = Style::default().fg(theme::FG).bg(bg).bold();
                for ch in inner.chars() {
                    out.push((ch, style));
                }
                i = end + 2;
                continue;
            }
        }
        if c == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|c| *c == '`') {
                let end = i + 1 + end;
                let inner: String = chars[i + 1..end].iter().collect();
                let style = Style::default().fg(theme::ORANGE).bg(theme::PANEL_ALT);
                for ch in format!(" {} ", inner).chars() {
                    out.push((ch, style));
                }
                i = end + 1;
                continue;
            }
        }
        if c == '~' && chars.get(i + 1) == Some(&'~') {
            if let Some(end) = find_seq(&chars, i + 2, '~', '~') {
                let inner: String = chars[i + 2..end].iter().collect();
                let style = Style::default()
                    .fg(theme::DIM)
                    .bg(bg)
                    .add_modifier(Modifier::CROSSED_OUT);
                for ch in inner.chars() {
                    out.push((ch, style));
                }
                i = end + 2;
                continue;
            }
        }
        // key:: value at the start of a line
        if line_start && c.is_alphabetic() {
            let mut j = i;
            while j < chars.len()
                && (chars[j].is_alphanumeric() || chars[j] == '-' || chars[j] == '_')
            {
                j += 1;
            }
            if j + 1 < chars.len() && chars[j] == ':' && chars[j + 1] == ':' {
                for ch in chars[i..j].iter() {
                    out.push((*ch, Style::default().fg(theme::CYAN).bg(bg).dim()));
                }
                out.push((':', Style::default().fg(theme::FAINT).bg(bg)));
                out.push((':', Style::default().fg(theme::FAINT).bg(bg)));
                line_start = false;
                i = j + 2;
                continue;
            }
        }
        out.push((c, base));
        line_start = c == '\n';
        i += 1;
    }
    out
}

fn find_seq(chars: &[char], from: usize, a: char, b: char) -> Option<usize> {
    let mut i = from;
    while i + 1 < chars.len() {
        if chars[i] == a && chars[i + 1] == b {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn inline_spans(
    app: &App,
    text: &str,
    width: usize,
    bg: ratatui::style::Color,
    _selected: bool,
) -> Vec<Span<'static>> {
    let chars = styled_chars(app, text, bg, false);
    if chars.is_empty() {
        return vec![Span::styled("", Theme::panel())];
    }
    let mut runs: Vec<(char, Style)> = Vec::new();
    let mut col = 0usize;
    for (c, s) in chars {
        if *&c == '\n' {
            runs.push((' ', s));
            col += 1;
            continue;
        }
        if col >= width.saturating_sub(1) {
            runs.push(('…', s));
            break;
        }
        runs.push((c, s));
        col += 1;
    }
    runs_to_spans(runs)
}

fn runs_to_spans(runs: Vec<(char, Style)>) -> Vec<Span<'static>> {
    let mut out: Vec<Span> = Vec::new();
    let mut cur = String::new();
    let mut cur_style = Style::default();
    for (c, s) in runs {
        if cur.is_empty() {
            cur_style = s;
            cur.push(c);
        } else if s == cur_style {
            cur.push(c);
        } else {
            out.push(Span::styled(std::mem::take(&mut cur), cur_style));
            cur_style = s;
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(Span::styled(cur, cur_style));
    }
    out
}

/// Word wrap over char indices, honouring explicit newlines.
fn wrap_ranges(chars: &[char], width: usize) -> Vec<(usize, usize)> {
    let width = width.max(4);
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    let mut last_space: Option<usize> = None;
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\n' {
            out.push((start, i));
            start = i + 1;
            last_space = None;
            i += 1;
            continue;
        }
        if i - start >= width {
            let brk = match last_space {
                Some(s) if s > start => s,
                _ => i,
            };
            out.push((start, brk));
            start = if brk == i { i } else { brk + 1 };
            last_space = None;
            continue;
        }
        if chars[i] == ' ' {
            last_space = Some(i);
        }
        i += 1;
    }
    out.push((start, chars.len()));
    out
}

// ------------------------------------------------------------------ popups

fn render_popup(f: &mut Frame, app: &App, x: u16, y: u16, bounds: Rect) {
    let Some(popup) = app.popup.as_ref() else { return };
    let height = (popup.candidates.len() as u16 + 3).min(11);
    let width = 54u16.min(bounds.width.saturating_sub(4));
    let px = x.min(bounds.x + bounds.width.saturating_sub(width + 1));
    let py = if y + height + 1 < bounds.y + bounds.height {
        y + 1
    } else {
        y.saturating_sub(height)
    };
    let rect = Rect {
        x: px,
        y: py.max(bounds.y),
        width,
        height,
    };
    f.render_widget(Clear, rect);
    let typed = if popup.query.is_empty() {
        popup.trigger.prefix().to_string()
    } else {
        format!("{}{}", popup.trigger.prefix(), popup.query)
    };
    let title = if typed.is_empty() {
        format!(" {} ", popup.trigger.title())
    } else {
        format!(" {} · {} ", popup.trigger.title(), typed)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Theme::popup_border())
        .style(Theme::overlay())
        .title(Line::from(Span::styled(title, Theme::section_focus())));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::new();
    for (i, c) in popup.candidates.iter().enumerate() {
        let selected = i == popup.selected;
        let bg = if selected {
            theme::SELECT_BG
        } else {
            theme::PANEL_ALT
        };
        let label_style = Style::default()
            .fg(if selected { theme::FG } else { theme::DIM })
            .bg(bg);
        let kind_color = match c.kind.as_str() {
            "journal" => theme::GREEN,
            "block" => theme::PURPLE,
            "command" => theme::YELLOW,
            _ => theme::ACCENT,
        };
        let label_max = inner.width.saturating_sub(22) as usize;
        let mut label = c.label.clone();
        if label.chars().count() > label_max {
            label = label.chars().take(label_max.saturating_sub(1)).collect::<String>() + "…";
        }
        let pad = " ".repeat(label_max.saturating_sub(label.chars().count()));
        lines.push(Line::from(vec![
            Span::styled(if selected { "▸ " } else { "  " }, label_style),
            Span::styled(label, label_style),
            Span::styled(pad, label_style),
            Span::styled(format!(" {} ", c.kind), Style::default().fg(theme::BG).bg(kind_color).bold()),
            Span::styled(
                {
                    let used = 2 + label_max + c.kind.chars().count() + 2;
                    let room = (inner.width as usize).saturating_sub(used + 3);
                    format!(" {}", crate::app::snippet(&c.detail, room))
                },
                Style::default().fg(theme::FAINT).bg(bg),
            ),
        ]));
    }
    f.render_widget(Paragraph::new(lines).style(Theme::overlay()), inner);

    // footer help
    let footer = Rect {
        x: rect.x + 1,
        y: rect.y + rect.height - 2,
        width: rect.width.saturating_sub(2),
        height: 1,
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ↑↓ ", Theme::key()),
            Span::styled("choose  ", Theme::dim().bg(theme::PANEL_ALT)),
            Span::styled("Tab ", Theme::key()),
            Span::styled("complete  ", Theme::dim().bg(theme::PANEL_ALT)),
            Span::styled("Esc ", Theme::key()),
            Span::styled("cancel", Theme::dim().bg(theme::PANEL_ALT)),
        ]))
        .style(Theme::overlay()),
        footer,
    );
}

/// `Ctrl-P` — Find. With no query it is "what I touched last"; with a query it
/// is a search across page names and block text. This is the navigation.
fn render_palette(f: &mut Frame, app: &App, p: &crate::app::Palette, bounds: Rect) {
    let _ = app;
    let width = 78u16.min(bounds.width.saturating_sub(6));
    let height = ((p.items.len() as u16) + 4)
        .clamp(7, 20)
        .min(bounds.height.saturating_sub(3));
    let rect = Rect {
        x: bounds.x + (bounds.width.saturating_sub(width)) / 2,
        y: bounds.y + (bounds.height.saturating_sub(height)) / 4,
        width,
        height,
    };
    f.render_widget(Clear, rect);
    let hint = if p.recent { "recent" } else { "matches" };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Theme::popup_border())
        .style(Theme::overlay())
        .title(Line::from(vec![
            Span::styled(" Find ", Theme::section_focus()),
            Span::styled(format!(" {} ", hint), Theme::faint()),
        ]));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(inner);
    let prompt = if p.query.is_empty() {
        "type to search page names and block text".to_string()
    } else {
        p.query.clone()
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("› ", Style::default().fg(theme::ACCENT).bg(theme::PANEL_ALT).bold()),
                Span::styled(
                    prompt,
                    if p.query.is_empty() {
                        Theme::overlay().fg(theme::FAINT)
                    } else {
                        Theme::overlay()
                    },
                ),
                Span::styled("▏", Style::default().fg(theme::CARET).bg(theme::PANEL_ALT)),
            ]),
            Line::from(Span::styled(
                "─".repeat(inner.width as usize),
                Theme::faint().bg(theme::PANEL_ALT),
            )),
        ])
        .style(Theme::overlay()),
        rows[0],
    );

    if p.items.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  nothing matches — Esc to go back",
                Style::default().fg(theme::FAINT).bg(theme::PANEL_ALT),
            )))
            .style(Theme::overlay()),
            rows[1],
        );
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for (n, item) in p.items.iter().take(rows[1].height as usize).enumerate() {
        let selected = n == p.selected;
        let bg = if selected {
            theme::SELECT_BG
        } else {
            theme::PANEL_ALT
        };
        let style = Style::default()
            .fg(if selected { theme::FG } else { theme::DIM })
            .bg(bg);
        let label_max = 30usize;
        let label: String = item.label.chars().take(label_max).collect();
        let pad = " ".repeat(label_max.saturating_sub(label.chars().count()));
        lines.push(Line::from(vec![
            Span::styled(if selected { " ▸ " } else { "   " }, style),
            Span::styled(
                label,
                style.patch(Style::default().fg(if selected {
                    theme::ACCENT
                } else {
                    theme::DIM
                })),
            ),
            Span::styled(pad, style),
            Span::styled(
                truncate(&item.detail, inner.width.saturating_sub(label_max as u16 + 6) as usize),
                style.patch(Style::default().fg(theme::FAINT)),
            ),
        ]));
    }
    f.render_widget(
        Paragraph::new(lines).style(Theme::overlay().bg(theme::PANEL_ALT)),
        rows[1],
    );
}

// ------------------------------------------------------------- other views

fn render_search(f: &mut Frame, app: &mut App, area: Rect) {
    let cols = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).split(area);
    f.render_widget(
        panel(
            "SEARCH",
            true,
            Some(Span::styled(
                format!(" {} hits · bm25 ", app.search_results.len()),
                Theme::section().fg(theme::DIM),
            )),
        ),
        cols[0],
    );
    let inner = Rect {
        x: cols[0].x + 1,
        y: cols[0].y + 1,
        width: cols[0].width.saturating_sub(2),
        height: cols[0].height.saturating_sub(2),
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("▸ ", Style::default().fg(theme::ACCENT).bg(theme::PANEL).bold()),
            Span::styled(
                if app.search_query.is_empty() {
                    "type to search every block".to_string()
                } else {
                    app.search_query.clone()
                },
                if app.search_query.is_empty() {
                    Theme::panel().fg(theme::FAINT)
                } else {
                    Theme::panel().fg(theme::FG).bold()
                },
            ),
            Span::styled("▏", Style::default().fg(theme::CARET).bg(theme::PANEL)),
            Span::styled(
                if app.db.stats.fts {
                    "   FTS5 · porter unicode61"
                } else {
                    "   LIKE fallback"
                },
                Theme::faint(),
            ),
        ]),
        Line::from(Span::styled(
            "─".repeat(inner.width as usize),
            Theme::faint(),
        )),
    ];
    for (i, hit) in app.search_results.iter().enumerate() {
        let selected = i == app.search_selected;
        let bg = if selected { theme::SELECT_BG } else { theme::PANEL };
        let style = Style::default().fg(theme::FG).bg(bg);
        let badge = if hit.is_journal {
            crate::model::parse_journal_key(&hit.page)
                .map(|d| d.relative(app.today))
                .unwrap_or_else(|| hit.page.clone())
        } else {
            hit.page.clone()
        };
        let w = inner.width.saturating_sub(20) as usize;
        let text: String = hit.content.replace('\n', " ").chars().take(w).collect();
        lines.push(Line::from(vec![
            Span::styled(if selected { "▌" } else { " " }, Style::default().fg(theme::ACCENT).bg(bg)),
            Span::styled(format!(" {:<14}", truncate(&badge, 14)), {
                let c = if hit.is_journal { theme::GREEN } else { theme::ACCENT };
                Style::default().fg(c).bg(bg)
            }),
            Span::styled(text, style),
        ]));
    }
    f.render_widget(Paragraph::new(lines).style(Theme::panel()), inner);

    // preview pane
    f.render_widget(panel("BLOCK PREVIEW", false, None), cols[1]);
    let pinner = Rect {
        x: cols[1].x + 1,
        y: cols[1].y + 1,
        width: cols[1].width.saturating_sub(2),
        height: cols[1].height.saturating_sub(2),
    };
    let mut plines: Vec<Line> = Vec::new();
    if let Some(hit) = app.search_results.get(app.search_selected) {
        let day = crate::model::parse_journal_key(&hit.page).map(|d| d.title());
        plines.push(Line::from(vec![
            Span::styled("source  ", Theme::faint()),
            Span::styled(
                day.unwrap_or_else(|| format!("Pages / {}", hit.page)),
                Theme::panel().fg(theme::ACCENT),
            ),
        ]));
        plines.push(Line::from(vec![
            Span::styled("block   ", Theme::faint()),
            Span::styled(
                app.db
                    .block(hit.block_id)
                    .map(|b| b.uuid)
                    .unwrap_or_else(|| "-".into()),
                Theme::panel().fg(theme::PURPLE),
            ),
        ]));
        plines.push(Line::from(""));
        let w = pinner.width as usize;
        for chunk in chunk_chars(&hit.content.replace('\n', " "), w.saturating_sub(2)) {
            plines.push(Line::from(Span::styled(
                format!("  {}", chunk),
                Theme::panel(),
            )));
        }
        plines.push(Line::from(""));
        plines.push(Line::from(vec![
            Span::styled("  ⏎ ", Theme::key()),
            Span::styled("open page   ", Theme::dim()),
            Span::styled("g ", Theme::key()),
            Span::styled("zoom block   ", Theme::dim()),
        ]));
        plines.push(Line::from(""));
        plines.push(Line::from(Span::styled("  matched refs", Theme::section())));
        for (p, hits) in app.linked.iter().take(3) {
            plines.push(Line::from(vec![
                Span::styled("  · ", Theme::faint()),
                Span::styled(format!("{} ({})", truncate(p, 18), hits.len()), Theme::dim()),
            ]));
        }
        if app.linked.is_empty() {
            plines.push(Line::from(Span::styled("  · no backlinks", Theme::faint())));
        }
    } else {
        plines.push(Line::from(Span::styled(
            "  nothing selected",
            Theme::dim(),
        )));
    }
    f.render_widget(Paragraph::new(plines).style(Theme::panel()).wrap(Wrap { trim: false }), pinner);
}

fn render_query(f: &mut Frame, app: &mut App, area: Rect) {
    let cols = Layout::horizontal([
        Constraint::Percentage(34),
        Constraint::Percentage(33),
        Constraint::Percentage(33),
    ])
    .split(area);
    for (i, status) in ["TODO", "DOING", "DONE"].iter().enumerate() {
        let hits = app.by_status(status);
        let color = theme::status_color(status);
        let focused = i == app.query_col;
        f.render_widget(
            panel(
                status,
                focused,
                Some(Span::styled(format!(" {} ", hits.len()), Theme::section().fg(color))),
            ),
            cols[i],
        );
        let inner = Rect {
            x: cols[i].x + 1,
            y: cols[i].y + 1,
            width: cols[i].width.saturating_sub(2),
            height: cols[i].height.saturating_sub(2),
        };
        let mut lines: Vec<Line> = Vec::new();
        if hits.is_empty() {
            lines.push(Line::from(Span::styled("empty", Theme::dim().fg(theme::FAINT))));
        }
        for (n, h) in hits.iter().take(inner.height as usize - 1).enumerate() {
            let badge = if h.is_journal {
                crate::model::parse_journal_key(&h.page)
                    .map(|d| d.relative(app.today))
                    .unwrap_or_else(|| h.page.clone())
            } else {
                h.page.clone()
            };
            let cursor = focused && n == app.query_row;
            let bg = if cursor { theme::SELECT_BG } else { theme::PANEL };
            lines.push(Line::from(vec![
                Span::styled(
                    if cursor { "▌" } else { " " },
                    Style::default().fg(theme::ACCENT).bg(bg),
                ),
                Span::styled("• ", Style::default().fg(color).bg(bg)),
                Span::styled(
                    format!("{:<8}", truncate(&badge, 8)),
                    Style::default().fg(theme::FAINT).bg(bg),
                ),
                Span::styled(
                    truncate(
                        &h.content.replace('\n', " "),
                        inner.width.saturating_sub(13) as usize,
                    ),
                    Style::default().fg(theme::FG).bg(bg),
                ),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("query: {{:status \"{}\"}}", status.to_lowercase()),
            Theme::faint(),
        )));
        f.render_widget(Paragraph::new(lines).style(Theme::panel()), inner);
    }
}

fn render_backup(f: &mut Frame, app: &mut App, area: Rect) {
    let cols = Layout::horizontal([Constraint::Percentage(46), Constraint::Percentage(54)]).split(area);
    f.render_widget(panel("STORAGE", true, None), cols[0]);
    let inner = Rect {
        x: cols[0].x + 1,
        y: cols[0].y + 1,
        width: cols[0].width.saturating_sub(2),
        height: cols[0].height.saturating_sub(2),
    };
    let s = &app.db.stats;
    let mut lines: Vec<Line> = Vec::new();
    let kv = |k: &str, v: String, c: ratatui::style::Color| {
        Line::from(vec![
            Span::styled(format!("  {:<19}", k), Theme::faint()),
            Span::styled(v, Style::default().fg(c).bg(theme::PANEL)),
        ])
    };
    lines.push(kv("database", app.db.path.to_string_lossy().to_string(), theme::FG));
    lines.push(kv(
        "engine",
        format!("SQLite · WAL · {}", if s.fts { "FTS5" } else { "LIKE" }),
        theme::FG,
    ));
    lines.push(kv("file size", human_bytes(s.file_bytes), theme::FG));
    lines.push(kv("wal size", human_bytes(s.wal_bytes), theme::DIM));
    lines.push(kv(
        "integrity",
        if app.integrity.is_empty() {
            app.db.integrity_check()
        } else {
            app.integrity.clone()
        },
        theme::GREEN,
    ));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  tables", Theme::section())));
    for (t, n) in [
        ("pages", s.pages),
        ("journals", s.journals),
        ("blocks", s.blocks),
        ("refs", s.refs),
        ("properties", app.db.prop_count()),
    ] {
        lines.push(kv(t, format!("{} rows", n), theme::FG));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  maintenance", Theme::section())));
    lines.push(kv(
        "empty blocks",
        format!("{} (transient)", app.db.empty_block_count()),
        theme::YELLOW,
    ));
    lines.push(kv(
        "pruned journals",
        if app.pruned.is_empty() {
            "none pending".into()
        } else {
            format!("{} removed", app.pruned.len())
        },
        theme::GREEN,
    ));
    for name in app.pruned.iter().take(6) {
        lines.push(Line::from(vec![
            Span::styled("      · ", Theme::faint()),
            Span::styled(name.clone(), Theme::panel().fg(theme::DIM)),
            Span::styled("  empty → dropped", Theme::faint()),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  layout", Theme::section())));
    for l in [
        "~/.blok/",
        "├── blok.db            blocks · pages · refs · FTS5",
        "├── blok.db-wal        crash safety, checkpointed on quit",
        "├── snapshots/         VACUUM INTO single-file copies",
        "└── snapshots/queue/   upload descriptors for the remote",
    ] {
        lines.push(Line::from(Span::styled(
            format!("  {}", l),
            Theme::panel().fg(theme::DIM),
        )));
    }
    f.render_widget(Paragraph::new(lines).style(Theme::panel()), inner);

    f.render_widget(panel("SNAPSHOTS → REMOTE", false, None), cols[1]);
    let rinner = Rect {
        x: cols[1].x + 1,
        y: cols[1].y + 1,
        width: cols[1].width.saturating_sub(2),
        height: cols[1].height.saturating_sub(2),
    };
    let mut rlines: Vec<Line> = Vec::new();
    rlines.push(Line::from(vec![
        Span::styled("  remote  ", Theme::faint()),
        Span::styled(app.db.remote_target(), Theme::panel().fg(theme::CYAN).bold()),
        Span::styled("   rclone copy --checksum", Theme::faint()),
    ]));
    rlines.push(Line::from(Span::styled(
        "  one file in, one file out — no document merge, no conflict soup",
        Theme::panel().fg(theme::DIM),
    )));
    rlines.push(Line::from(""));
    rlines.push(Line::from(vec![
        Span::styled("  WHEN            SIZE      STATE     FILE", Theme::section()),
    ]));
    let snaps = app.db.snapshots(12);
    if snaps.is_empty() {
        rlines.push(Line::from(Span::styled(
            "  no snapshots yet — press Ctrl-S or run :backup",
            Theme::panel().fg(theme::FAINT),
        )));
    }
    for snap in &snaps {
        let after = snap
            .taken_at
            .split('T')
            .nth(1)
            .map(|s| s.to_string())
            .unwrap_or_else(|| snap.taken_at.clone());
        let state_color = match snap.state.as_str() {
            "uploaded" => theme::GREEN,
            "queued" => theme::YELLOW,
            _ => theme::DIM,
        };
        rlines.push(Line::from(vec![
            Span::styled(format!("  {:<14}", after), Theme::panel().fg(theme::DIM)),
            Span::styled(format!("{:<9}", human_bytes(snap.bytes)), Theme::panel()),
            Span::styled(format!("{:<9}", snap.state), Style::default().fg(state_color).bg(theme::PANEL)),
            Span::styled(
                snap.file.rsplit('/').next().unwrap_or(&snap.file).to_string(),
                Theme::panel().fg(theme::FG),
            ),
        ]));
    }
    rlines.push(Line::from(""));
    rlines.push(Line::from(Span::styled("  WHY THIS IS SAFE", Theme::section())));
    for l in [
        "VACUUM INTO writes a consistent snapshot while the app keeps running.",
        "The snapshot is a plain SQLite file: restore = copy it back and open.",
        "WAL means a crash mid-write replays instead of corrupting.",
        "Syncing a live .db by file copy is not safe; syncing a snapshot is.",
        "History is the snapshot list: point-in-time restore without git.",
    ] {
        rlines.push(Line::from(vec![
            Span::styled("  ▸ ", Style::default().fg(theme::ACCENT).bg(theme::PANEL)),
            Span::styled(l, Theme::panel().fg(theme::DIM)),
        ]));
    }
    if let Some(desc) = &app.upload_desc {
        rlines.push(Line::from(""));
        rlines.push(Line::from(vec![
            Span::styled("  upload descriptor  ", Theme::faint()),
            Span::styled(
                desc.rsplit('/').next().unwrap_or(desc).to_string(),
                Theme::panel().fg(theme::PURPLE),
            ),
        ]));
    }
    f.render_widget(
        Paragraph::new(rlines).style(Theme::panel()).wrap(Wrap { trim: false }),
        rinner,
    );
}

fn render_help(f: &mut Frame, app: &App, area: Rect) {
    let _ = app;
    let cols = Layout::horizontal([Constraint::Percentage(52), Constraint::Percentage(48)]).split(area);
    // Split at a section boundary near the middle, so the columns are balanced
    // and no group is orphaned the way a naive half-split leaves them.
    let half = KEYMAP.len() / 2;
    let split = KEYMAP
        .iter()
        .enumerate()
        .filter(|(_, (_, d, _))| d.is_empty())
        .map(|(i, _)| i)
        .min_by_key(|i| (*i as i64 - half as i64).abs())
        .unwrap_or(half)
        .max(1);
    for (i, slice) in [&KEYMAP[..split], &KEYMAP[split..]].iter().enumerate() {
        f.render_widget(
            panel(
                if i == 0 { "KEYMAP · NAVIGATION & EDITING" } else { "KEYMAP · LINKS & PERSISTENCE" },
                i == 0,
                None,
            ),
            cols[i],
        );
        let inner = Rect {
            x: cols[i].x + 1,
            y: cols[i].y + 1,
            width: cols[i].width.saturating_sub(2),
            height: cols[i].height.saturating_sub(2),
        };
        let mut lines: Vec<Line> = Vec::new();
        for (k, d, scope) in slice.iter() {
            if d.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("  {}", k.to_uppercase()),
                    Theme::section_focus(),
                )));
                continue;
            }
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<20}", k), Theme::key()),
                Span::styled(
                    format!("{:<40}", truncate(d, 40)),
                    Theme::panel().fg(theme::FG),
                ),
                Span::styled(*scope, Theme::faint()),
            ]));
        }
        f.render_widget(Paragraph::new(lines).style(Theme::panel()), inner);
    }
}

// -------------------------------------------------------------------- chrome

/// One line: the mode (vim needs it), whether the caret is in a block, the
/// pending vim prefix, and where you are in the outline. The page's identity is
/// the pane title, so it is not repeated here.
fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let mode_color = match app.mode {
        Mode::Normal => theme::ACCENT,
        Mode::Insert => theme::GREEN,
        Mode::Visual => theme::PURPLE,
    };
    let mut spans = vec![
        Span::styled(
            format!(" {} ", app.mode.label()),
            Style::default().fg(theme::BG).bg(mode_color).bold(),
        ),
        Span::styled(" ", Theme::statusbar()),
    ];
    if app.text_focus && app.mode == Mode::Normal {
        // A cursor inside the block, not a mode: say so.
        spans.push(Span::styled(
            " in block ",
            Style::default().fg(theme::BG).bg(theme::CYAN).bold(),
        ));
        spans.push(Span::styled(" ", Theme::statusbar()));
    }
    if !app.pending.is_empty() {
        spans.push(Span::styled(
            format!(" {} ", app.pending),
            Style::default().fg(theme::BG).bg(theme::CARET).bold(),
        ));
        spans.push(Span::styled(" ", Theme::statusbar()));
    }
    match &app.view {
        View::Journal(_) | View::Page(_) => {
            spans.push(Span::styled(
                format!("{:>3}/{}", app.selected + 1, app.rows.len().max(1)),
                Theme::statusbar().fg(theme::FG),
            ));
            if app.rows.iter().any(|r| r.depth > 0) {
                let depth = app.selected_row().map(|r| r.depth).unwrap_or(0);
                if depth > 0 {
                    spans.push(Span::styled(
                        format!("  depth {}", depth),
                        Theme::statusbar().fg(theme::FAINT),
                    ));
                }
            }
        }
        View::Search => spans.push(Span::styled(
            format!("{} hits", app.search_results.len()),
            Theme::statusbar().fg(theme::DIM),
        )),
        View::Query => spans.push(Span::styled(
            "TODO board",
            Theme::statusbar().fg(theme::DIM),
        )),
        View::Backup => spans.push(Span::styled(
            format!(
                "{} · {}",
                app.db.integrity_check(),
                human_bytes(app.db.stats.file_bytes)
            ),
            Theme::statusbar().fg(theme::DIM),
        )),
        View::Help => spans.push(Span::styled(
            "Esc goes back",
            Theme::statusbar().fg(theme::DIM),
        )),
        View::Sql => {
            let msg = app
                .sql
                .as_ref()
                .map(|c| c.message.clone())
                .unwrap_or_default();
            spans.push(Span::styled(msg, Theme::statusbar().fg(theme::DIM)));
        }
    }
    let left = Line::from(spans);

    // Quiet on the right: no engine trivia, no WAL sizes, no snapshot nag. The
    // one thing worth a chip is the pane you hid, so it is not a mystery.
    let mut right_spans: Vec<Span> = Vec::new();
    if !app.show_meta {
        right_spans.push(Span::styled(
            " Ctrl-M page metadata ",
            Theme::statusbar().fg(theme::FAINT),
        ));
    }
    // Quiet on the right, and not a second copy of the hint bar: the hints
    // already advertise whatever is relevant, including Ctrl-M.
    let right = Line::from(Vec::<Span>::new());

    let halves = Layout::horizontal([Constraint::Min(40), Constraint::Length(40)]).split(area);
    f.render_widget(Paragraph::new(left).style(Theme::statusbar()), halves[0]);
    f.render_widget(
        Paragraph::new(right)
            .style(Theme::statusbar())
            .alignment(Alignment::Right),
        halves[1],
    );
}

/// The hint bar, computed from what is actually possible *right now*: which
/// mode, whether the caret is in a block, which pane has focus, and whether the
/// selected block even has links to follow. This is the app's own manual.
fn render_hints(f: &mut Frame, app: &App, area: Rect) {
    let links = app
        .selected_row()
        .map(|r| app.links_in(r.id).len())
        .unwrap_or(0);
    let pairs: Vec<(String, String)> = match app.mode {
        Mode::Insert => vec![
            ("Esc".into(), "stop editing".into()),
            ("⏎".into(), "new block".into()),
            ("Alt-⏎".into(), "newline in this block".into()),
            ("/".into(), "commands".into()),
            ("[[".into(), "link page".into()),
            ("((".into(), "block ref".into()),
            ("#".into(), "tag".into()),
        ],
        Mode::Visual => vec![
            ("j/k".into(), "extend the range".into()),
            ("> <".into(), "indent / outdent".into()),
            ("d".into(), "delete".into()),
            ("y".into(), "yank".into()),
            ("Esc".into(), "leave".into()),
        ],
        Mode::Normal => match app.focus {
            Focus::Right => vec![
                ("j/k".into(), "references".into()),
                ("⏎".into(), "open the referencing block".into()),
                ("Esc".into(), "back to the blocks".into()),
            ],
            Focus::Main => match app.view {
                View::Journal(_) | View::Page(_) if app.text_focus => vec![
                    ("j/k".into(), "back to the blocks".into()),
                    ("w b e 0 $".into(), "move".into()),
                    ("x dw cw".into(), "delete, change".into()),
                    ("⏎ i a".into(), "type".into()),
                    ("o".into(), "new block".into()),
                    ("Ctrl-]".into(), "follow link".into()),
                    ("Esc".into(), "drop the caret".into()),
                ],
                View::Journal(_) | View::Page(_) => {
                    let mut v: Vec<(String, String)> = Vec::new();
                    if links > 0 {
                        v.push((
                            "Ctrl-]".into(),
                            format!(
                                "follow {} link{}",
                                links,
                                if links == 1 { "" } else { "s" }
                            ),
                        ));
                    }
                    v.extend([
                        ("j/k".into(), "blocks".into()),
                        ("⏎ / i".into(), "edit this block".into()),
                        ("o".into(), "new block".into()),
                        ("h/l".into(), "parent / children".into()),
                        ("za".into(), "fold".into()),
                        ("Ctrl-P".into(), "Find any page".into()),
                        ("[ ]".into(), "prev / next day".into()),
                        ("/".into(), "search".into()),
                        (":".into(), "commands".into()),
                        ("?".into(), "all keys".into()),
                    ]);
                    if !app.show_meta {
                        v.insert(0, (app.meta_key().into(), "page metadata".into()));
                    } else if app.focus == Focus::Main {
                        v.push((app.meta_key().into(), "hide metadata".into()));
                    }
                    v
                }
                View::Search => vec![
                    ("type".into(), "search every block".into()),
                    ("↑↓".into(), "results".into()),
                    ("⏎".into(), "open".into()),
                    ("Esc".into(), "back".into()),
                ],
                View::Query => vec![
                    ("h/l".into(), "columns".into()),
                    ("j/k".into(), "rows".into()),
                    ("⏎".into(), "jump to the block".into()),
                    ("Esc".into(), "back".into()),
                ],
                View::Backup => vec![
                    ("Ctrl-S".into(), "snapshot".into()),
                    (":sql".into(), "console".into()),
                    ("Esc".into(), "back".into()),
                ],
                View::Help => vec![("Esc".into(), "back".into()), ("q".into(), "quit".into())],
                View::Sql => vec![
                    ("⏎".into(), "run".into()),
                    (".tables".into(), "list tables".into()),
                    ("↑↓".into(), "history".into()),
                    ("Esc".into(), "leave".into()),
                ],
            },
        },
    };
    let mut spans: Vec<Span> = Vec::new();
    for (k, d) in pairs {
        spans.push(Span::styled(format!(" {} ", k), Theme::key()));
        spans.push(Span::styled(format!("{}  ", d), Theme::dim().bg(theme::BG)));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Theme::hintbar()),
        area,
    );
}

/// The last action, on one line, above the hint bar. It replaces the floating
/// bottom-right box whose "sqlite" title made it look like a database dialog.
/// Any keypress clears it (`handle_key`), so it is a message, not a window.
fn render_message(f: &mut Frame, app: &App, area: Rect) {
    let Some(t) = app.toast.as_ref() else { return };
    let (color, mark) = match t.kind {
        ToastKind::Info => (theme::ACCENT, "·"),
        ToastKind::Good => (theme::GREEN, "✓"),
        ToastKind::Warn => (theme::ORANGE, "!"),
    };
    f.render_widget(Clear, area);
    let mut spans = vec![
        Span::styled(
            format!(" {} ", mark),
            Style::default().fg(theme::BG).bg(color).bold(),
        ),
        Span::styled(
            format!(" {} ", t.text),
            Style::default().fg(theme::FG).bg(theme::PANEL_ALT).bold(),
        ),
    ];
    if let Some(sub) = &t.sub {
        let room = (area.width as usize).saturating_sub(t.text.chars().count() + 6);
        if room > 8 {
            spans.push(Span::styled(
                format!("  {} ", truncate(sub, room)),
                Style::default().fg(theme::FAINT).bg(theme::PANEL_ALT),
            ));
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Theme::overlay()),
        area,
    );
}

// ------------------------------------------------------------------ helpers

/// vim's command line: the `:`, what you have typed, matching commands.
fn render_ex(f: &mut Frame, app: &App, area: Rect) {
    let Some(ex) = app.ex.as_ref() else { return };
    f.render_widget(Clear, area);
    let mut spans = vec![
        Span::styled(
            ":",
            Style::default().fg(theme::CARET).bg(theme::PANEL_ALT).bold(),
        ),
        Span::styled(ex.input.clone(), Theme::overlay()),
        Span::styled("▏", Style::default().fg(theme::CARET).bg(theme::PANEL_ALT)),
        Span::styled("  ", Theme::overlay()),
    ];
    let matches = app.ex_matches();
    for (name, desc) in matches.iter().take(4) {
        spans.push(Span::styled(
            format!(" {} ", name),
            Style::default().fg(theme::ACCENT).bg(theme::PANEL_ALT),
        ));
        spans.push(Span::styled(
            format!("{}  ", truncate(desc, 34)),
            Style::default().fg(theme::FAINT).bg(theme::PANEL_ALT),
        ));
    }
    if matches.is_empty() {
        spans.push(Span::styled(
            "no such command — Tab completes, :help lists",
            Style::default().fg(theme::ORANGE).bg(theme::PANEL_ALT),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(Theme::overlay()), area);
}

/// The troubleshooting console: where storage internals live now, instead of
/// permanently occupying the status bar.
fn render_sql(f: &mut Frame, app: &App, area: Rect) {
    let Some(console) = app.sql.as_ref() else { return };
    let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(area);

    f.render_widget(
        panel(
            "SQL CONSOLE",
            true,
            Some(Span::styled(
                " read-only · SELECT, PRAGMA, EXPLAIN, WITH ",
                Theme::section().fg(theme::DIM),
            )),
        ),
        rows[0],
    );
    let prompt_area = Rect {
        x: rows[0].x + 1,
        y: rows[0].y + 1,
        width: rows[0].width.saturating_sub(2),
        height: rows[0].height.saturating_sub(2),
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    "sqlite> ",
                    Style::default().fg(theme::GREEN).bg(theme::PANEL).bold(),
                ),
                Span::styled(console.input.clone(), Theme::panel().fg(theme::FG)),
                Span::styled("▏", Style::default().fg(theme::CARET).bg(theme::PANEL)),
            ]),
            Line::from(Span::styled(console.message.clone(), Theme::faint())),
        ])
        .style(Theme::panel()),
        prompt_area,
    );

    f.render_widget(panel("RESULT", false, None), rows[1]);
    let out = Rect {
        x: rows[1].x + 1,
        y: rows[1].y + 1,
        width: rows[1].width.saturating_sub(2),
        height: rows[1].height.saturating_sub(2),
    };

    if console.rows.is_empty() {
        let mut help: Vec<Line> = vec![
            Line::from(Span::styled(
                "  The engine stays off the screen until something is wrong.",
                Theme::panel().fg(theme::FAINT),
            )),
            Line::from(""),
        ];
        for (cmd, desc) in [
            (".tables", "tables and views, with row counts"),
            (".schema blocks", "the DDL for a table"),
            ("SELECT * FROM blocks LIMIT 20", "what actually got stored"),
            ("PRAGMA integrity_check", "is the file sound"),
            ("PRAGMA journal_mode", "wal, and the pragmas in force"),
            ("SELECT * FROM backup_log ORDER BY id DESC", "snapshot history"),
            ("SELECT * FROM refs WHERE from_block = 42", "why a backlink is missing"),
            ("EXPLAIN QUERY PLAN SELECT ...", "did the index get used"),
        ] {
            help.push(Line::from(vec![
                Span::styled(format!("  {:<46}", cmd), Theme::key()),
                Span::styled(desc, Theme::panel().fg(theme::DIM)),
            ]));
        }
        f.render_widget(Paragraph::new(help).style(Theme::panel()), out);
        return;
    }

    // A minimal table, sized to the content and clipped to the pane.
    let cols = console.columns.len();
    let avail = out.width.saturating_sub(2) as usize;
    let mut widths = vec![0usize; cols];
    for (i, name) in console.columns.iter().enumerate() {
        widths[i] = name.chars().count().min(30);
    }
    for row in console.rows.iter().take(80) {
        for (i, cell) in row.iter().enumerate().take(cols) {
            let one = cell.replace('\n', " ");
            widths[i] = widths[i].max(one.chars().count().min(30));
        }
    }
    let total: usize = widths.iter().sum::<usize>() + cols * 2;
    if total > avail && cols > 0 {
        let per = (total - avail) / cols + 1;
        for w in widths.iter_mut() {
            *w = w.saturating_sub(per).max(4);
        }
    }
    let mut lines: Vec<Line> = Vec::new();
    let mut header = Vec::new();
    for (i, name) in console.columns.iter().enumerate() {
        header.push(Span::styled(
            format!(" {:<width$}", truncate(name, widths[i]), width = widths[i]),
            Theme::section_focus(),
        ));
    }
    lines.push(Line::from(header));
    lines.push(Line::from(Span::styled(
        "─".repeat(widths.iter().sum::<usize>() + cols * 2),
        Theme::faint(),
    )));
    for row in console.rows.iter() {
        let mut spans = Vec::new();
        for (i, cell) in row.iter().enumerate().take(cols) {
            let one = cell.replace('\n', " ");
            let color = if one == "NULL" {
                theme::FAINT
            } else if !one.is_empty() && one.chars().all(|c| c.is_ascii_digit()) {
                theme::CYAN
            } else {
                theme::FG
            };
            spans.push(Span::styled(
                format!(" {:<width$}", truncate(&one, widths[i]), width = widths[i]),
                Style::default().fg(color).bg(theme::PANEL),
            ));
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines).style(Theme::panel()), out);
}

pub fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        return s.to_string();
    }
    let t: String = s.chars().take(w.saturating_sub(1)).collect();
    format!("{}…", t)
}

fn chunk_chars(s: &str, w: usize) -> Vec<String> {
    let w = w.max(8);
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        if cur.chars().count() + word.chars().count() + 1 > w {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn block_cursor_line(app: &App) -> Option<Line<'static>> {
    app.selected_row().map(|r| {
        Line::from(vec![
            Span::styled("selected ", Theme::faint()),
            Span::styled(r.uuid.clone(), Theme::panel().fg(theme::PURPLE)),
        ])
    })
}

pub fn property_table(app: &App, page_id: i64) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for b in app.db.page_blocks(page_id) {
        for (k, v) in properties(&b.content) {
            out.push((k, v));
        }
    }
    out
}

pub fn journal_badge(app: &App, day: &JournalDay) -> String {
    day.relative(app.today)
}

pub fn caret_position_for(app: &App) -> Option<Position> {
    let _ = app;
    None
}

/// Render-level regression tests. The bug that prompted these: when the top bar
/// was deleted the layout shifted, and two views kept drawing into the row that
/// had become the one-line message area -- so Help and Storage rendered into a
/// height-0 rectangle and the frames were silently blank. A frame that draws
/// nothing is not a subtle failure, it just needs something to notice it.
#[cfg(test)]
mod render_tests {
    use crate::app::{App, View};
    use crate::db::Db;
    use crate::model::PageKind;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn app(name: &str) -> App {
        let path =
            std::env::temp_dir().join(format!("blok-render-{}-{}.db", std::process::id(), name));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
        let db = Db::open(&path).expect("open test db");
        let today = crate::db::today();
        let mut app = App::new(db, today);
        let page = app.db.ensure_page("Test Page", PageKind::Page);
        app.db.create_block(page.id, None, None, "first block");
        app.db
            .create_block(page.id, None, None, "second block with [[Test Page]]");
        app.goto_page("Test Page");
        app
    }

    /// Count cells that carry a visible glyph.
    fn painted(app: &mut App, w: u16, h: u16) -> usize {
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("terminal");
        term.draw(|f| crate::ui::render(f, app)).expect("draw");
        let buf = term.backend().buffer();
        let mut n = 0usize;
        for y in 0..h {
            for x in 0..w {
                if !buf[(x, y)].symbol().trim().is_empty() {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn every_view_draws_something() {
        let views: Vec<(&str, View)> = vec![
            ("journal", View::Journal(crate::model::JournalDay::new(crate::db::today()))),
            ("page", View::Page("Test Page".into())),
            ("find", View::Page("Test Page".into())),
            ("search", View::Search),
            ("board", View::Query),
            ("storage", View::Backup),
            ("help", View::Help),
            ("sql", View::Sql),
        ];
        for (name, view) in views {
            let mut a = app(name);
            a.set_view(view.clone());
            if name == "find" {
                a.open_palette();
            }
            if name == "sql" {
                a.open_sql();
            }
            let n = painted(&mut a, 100, 30);
            assert!(
                n > 200,
                "{view:?} drew only {n} cells -- it is rendering off-screen or into a zero-height row"
            );
        }
    }

    #[test]
    fn the_metadata_panel_renders_when_shown_and_not_when_hidden() {
        let mut a = app("meta_render");
        let hidden = painted(&mut a, 100, 30);
        a.show_meta = true;
        let shown = painted(&mut a, 100, 30);
        assert!(
            shown > hidden,
            "showing the panel must paint more, not the same ({hidden} -> {shown})"
        );
    }
}
