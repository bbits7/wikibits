//! Draws the three columns, the status line and the help popup.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use ratatui_image::Image;
use ratatui_image::sliced::SlicedImage;

use crate::app::{App, Focus, RelatedRow, TreeKind};
use crate::markdown::{EXTERNAL_LINK, Item, MISSING_LINK, PAGE_LINK};

const ACCENT: Color = Color::Cyan;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let [main, status] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);
    let side = (area.width / 5).clamp(22, 40);
    let [tree, content, related] = Layout::horizontal([
        Constraint::Length(side),
        Constraint::Min(20),
        Constraint::Length(side),
    ])
    .areas(main);

    draw_tree(f, app, tree);
    draw_content(f, app, content);
    draw_related(f, app, related);
    draw_status(f, app, status);
    if app.popup.is_some() {
        draw_popup(f, app, area);
    }
    if app.show_help {
        draw_help(f, area);
    }
}

/// The selected image at full size, scrolled with the keys or the mouse wheel.
fn draw_popup(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(popup) = &app.popup else { return };
    let width = (popup.cells.0 + 2).min(area.width.saturating_sub(4)).max(4);
    let height = (popup.cells.1 + 2)
        .min(area.height.saturating_sub(2))
        .max(3);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let position = if popup.off == (0, 0) {
        String::new()
    } else {
        format!("  ·  at {},{}", popup.off.0, popup.off.1)
    };
    let title = format!(
        " {}  {}x{} px{position}  ·  h/j/k/l or drag  Esc close ",
        popup.name, popup.pixels.0, popup.pixels.1
    );
    let block = pane(&title, true);
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);
    if let Some(proto) = app.popup_protocol(inner) {
        f.render_widget(Image::new(proto), inner);
    }
}

fn pane(title: &str, focused: bool) -> Block<'static> {
    let border = if focused {
        Style::new().fg(ACCENT)
    } else {
        Style::new().add_modifier(Modifier::DIM)
    };
    let title_style = if focused {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border);
    if title.is_empty() {
        block
    } else {
        block.title(Span::styled(format!(" {title} "), title_style))
    }
}

fn selection_style(focused: bool) -> Style {
    if focused {
        Style::new().add_modifier(Modifier::REVERSED)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    }
}

/// Keep `sel` inside the window of `height` rows starting at `scroll`.
fn keep_visible(scroll: &mut usize, sel: usize, height: usize) {
    if height == 0 {
        return;
    }
    if sel < *scroll {
        *scroll = sel;
    } else if sel >= *scroll + height {
        *scroll = sel + 1 - height;
    }
}

fn draw_tree(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Tree;
    let block = pane("Pages", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.rects.tree = inner;

    keep_visible(&mut app.tree_scroll, app.tree_sel, inner.height as usize);
    let width = inner.width as usize;
    let lines: Vec<Line> = app
        .tree
        .iter()
        .enumerate()
        .skip(app.tree_scroll)
        .take(inner.height as usize)
        .map(|(i, row)| {
            let indent = "  ".repeat(row.depth);
            let current = |id: &str| app.current.as_deref() == Some(id);
            let (marker, label_style) = match &row.kind {
                TreeKind::Folder {
                    expanded, index, ..
                } => {
                    let marker = if *expanded { "▾ " } else { "▸ " };
                    let style = if index.as_deref().is_some_and(current) {
                        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().add_modifier(Modifier::BOLD)
                    };
                    (marker, style)
                }
                TreeKind::Page { id } if current(id) => {
                    ("  ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
                }
                TreeKind::Page { .. } => ("  ", Style::new()),
            };
            let mut text = format!("{indent}{marker}{}", row.label);
            let pad = width.saturating_sub(unicode_width::UnicodeWidthStr::width(text.as_str()));
            text.push_str(&" ".repeat(pad));
            let style = if i == app.tree_sel {
                label_style.patch(selection_style(focused))
            } else {
                label_style
            };
            Line::styled(text, style)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_content(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Content;
    let block = pane("", focused).title(crumbs_title(app, area, focused));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 1 {
        return;
    }
    let body = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    app.rects.content = body;
    app.content_height = body.height as usize;
    app.set_size(body.width, body.height);

    let Some(rendered) = &app.rendered else {
        let msg = Paragraph::new(app.status.clone()).dim();
        f.render_widget(msg, body);
        return;
    };
    let scroll = app.scroll.min(app.max_scroll());
    app.scroll = scroll;
    let mut lines: Vec<Line> = rendered
        .lines
        .iter()
        .skip(scroll)
        .take(body.height as usize)
        .cloned()
        .collect();
    match app.sel.and_then(|i| rendered.items.get(i)) {
        Some(Item::Link(l)) => {
            for &(line, span) in &rendered.links[*l].spans {
                if line >= scroll
                    && let Some(s) = lines
                        .get_mut(line - scroll)
                        .and_then(|l| l.spans.get_mut(span))
                {
                    s.style = s.style.add_modifier(Modifier::REVERSED);
                }
            }
        }
        Some(Item::Image(i)) => {
            let slot = &rendered.images[*i];
            for line in slot.line..=slot.line + slot.height as usize + 1 {
                if line >= scroll
                    && let Some(s) = lines
                        .get_mut(line - scroll)
                        .and_then(|l| l.spans.last_mut())
                {
                    s.style = Style::new().fg(ACCENT).add_modifier(Modifier::BOLD);
                }
            }
        }
        None => {}
    }
    f.render_widget(Paragraph::new(Text::from(lines)), body);

    for slot in &rendered.images {
        // The image sits one row below the frame's top line.
        let y = slot.line as i32 + 1 - scroll as i32;
        if y + slot.height as i32 <= 0 || y >= body.height as i32 {
            continue;
        }
        if let Some(proto) = app.image(slot) {
            let image = SlicedImage::new(&proto, (slot.x as i16, y as i16).into());
            f.render_widget(image, body);
        }
    }
}

/// The breadcrumbs as the page frame's title, remembering where each crumb lands so a click
/// on it can be told apart.
fn crumbs_title(app: &mut App, area: Rect, focused: bool) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    let mut columns = Vec::new();
    let mut col = 1u16;
    let last = app.crumbs.len().saturating_sub(1);
    for (i, crumb) in app.crumbs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                " › ",
                Style::new().add_modifier(Modifier::DIM),
            ));
            col += 3;
        }
        let width = unicode_width::UnicodeWidthStr::width(crumb.label.as_str()) as u16;
        let style = if i == last {
            let mut s = Style::new().add_modifier(Modifier::BOLD);
            if focused {
                s = s.fg(ACCENT);
            }
            s
        } else {
            Style::new().fg(ACCENT)
        };
        spans.push(Span::styled(crumb.label.clone(), style));
        columns.push((col, col + width));
        col += width;
    }
    if app.raw {
        spans.push(Span::styled(
            " (source)",
            Style::new().add_modifier(Modifier::DIM),
        ));
    }
    spans.push(Span::raw(" "));
    app.crumb_columns = columns;
    // The title starts right after the top-left corner.
    app.rects.crumbs = Rect {
        x: area.x + 1,
        y: area.y,
        width: area.width.saturating_sub(2),
        height: 1,
    };
    Line::from(spans)
}

fn draw_related(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Related;
    let block = pane("Related", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.rects.related = inner;

    keep_visible(
        &mut app.related_scroll,
        app.related_sel,
        inner.height as usize,
    );
    let width = inner.width as usize;
    let lines: Vec<Line> = app
        .related
        .iter()
        .enumerate()
        .skip(app.related_scroll)
        .take(inner.height as usize)
        .map(|(i, row)| {
            let (text, style) = match row {
                RelatedRow::Header(h) => (
                    h.to_string(),
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                RelatedRow::Page(id) => (
                    format!("  {}", app.wiki.title(id)),
                    PAGE_LINK.remove_modifier(Modifier::UNDERLINED),
                ),
                RelatedRow::Missing(raw) => (
                    format!("  {raw} ?"),
                    MISSING_LINK.remove_modifier(Modifier::UNDERLINED),
                ),
                RelatedRow::External(url) => (
                    format!("  {url}"),
                    EXTERNAL_LINK.remove_modifier(Modifier::UNDERLINED),
                ),
                RelatedRow::Heading {
                    index,
                    depth,
                    foldable,
                    expanded,
                } => {
                    let marker = match (foldable, expanded) {
                        (false, _) => "  ",
                        (true, true) => "▾ ",
                        (true, false) => "▸ ",
                    };
                    let text = app
                        .rendered
                        .as_ref()
                        .and_then(|r| r.headings.get(*index))
                        .map(|h| h.text.as_str())
                        .unwrap_or("");
                    let style = if *foldable {
                        Style::new().add_modifier(Modifier::BOLD)
                    } else {
                        Style::new()
                    };
                    (format!("{}{marker}{text}", "  ".repeat(*depth)), style)
                }
                RelatedRow::None => (
                    "  (none)".to_string(),
                    Style::new().add_modifier(Modifier::DIM),
                ),
                RelatedRow::Blank => (String::new(), Style::new()),
            };
            let mut text = text;
            let pad = width.saturating_sub(unicode_width::UnicodeWidthStr::width(text.as_str()));
            text.push_str(&" ".repeat(pad));
            let style = if i == app.related_sel && row.selectable() {
                style.patch(selection_style(focused))
            } else {
                style
            };
            Line::styled(text, style)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let hints = match app.focus {
        _ if app.popup.is_some() => "h/j/k/l scroll  PgUp/PgDn  g/G  Esc close",
        Focus::Tree => "j/k move  Enter open  h/l fold  Tab pane  b back  ? help  q quit",
        Focus::Content => {
            "j/k scroll  n/p link  Enter follow  v source  Tab pane  b back  ? help  q quit"
        }
        Focus::Related => "j/k move  Enter open  h/l fold  Tab pane  b back  ? help  q quit",
    };
    let left = if app.status.is_empty() {
        Span::styled(hints, Style::new().add_modifier(Modifier::DIM))
    } else {
        Span::styled(app.status.clone(), Style::new().fg(Color::Yellow))
    };
    let right = app.current.clone().unwrap_or_default();
    let right_width = unicode_width::UnicodeWidthStr::width(right.as_str()) as u16;
    f.render_widget(Paragraph::new(Line::from(left)), area);
    if right_width + 2 < area.width {
        let right_area = Rect {
            x: area.x + area.width - right_width - 1,
            width: right_width,
            ..area
        };
        f.render_widget(Paragraph::new(right).dim(), right_area);
    }
}

fn draw_help(f: &mut Frame, area: Rect) {
    let text = [
        ("Everywhere", ""),
        ("Tab / Shift-Tab", "next / previous column"),
        ("1 2 3", "focus pages / page / related"),
        ("b  Backspace", "back"),
        ("f", "forward"),
        ("H", "home page"),
        ("r", "reload the wiki folder"),
        ("?", "this help"),
        ("q", "quit"),
        ("", ""),
        ("Pages column", ""),
        ("j / k", "move"),
        ("PgUp / PgDn  g / G", "a page up / down, top / bottom"),
        ("Enter", "open page / fold folder"),
        ("h / l", "collapse / expand"),
        ("", ""),
        ("Related column", ""),
        (
            "j / k  PgUp / PgDn  g / G",
            "move, a page at a time, top / bottom",
        ),
        ("Enter", "jump to the heading / open the page"),
        ("h / l  Space", "fold / unfold a heading"),
        ("", ""),
        ("Page", ""),
        ("j / k  PgUp / PgDn", "scroll"),
        ("g / G", "top / bottom"),
        ("n / p", "next / previous link or image"),
        ("v", "toggle Markdown source / rendered page"),
        (
            "Enter",
            "follow the selected link / open the image full size",
        ),
        ("", ""),
        ("Image pop-up", ""),
        (
            "h / j / k / l, arrows",
            "scroll (PgUp / PgDn a screen, g / G top / bottom)",
        ),
        ("mouse", "drag the image, or use the wheel"),
        ("Esc  q  Enter", "close"),
        ("", ""),
        (
            "Mouse",
            "click to open, wheel to scroll, right-click to go back",
        ),
    ];
    let lines: Vec<Line> = text
        .iter()
        .map(|(key, desc)| {
            if desc.is_empty() {
                Line::styled(
                    key.to_string(),
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )
            } else {
                Line::from(vec![
                    Span::styled(format!("{key:<20}"), Style::new().fg(ACCENT)),
                    Span::raw(desc.to_string()),
                ])
            }
        })
        .collect();
    let height = (lines.len() as u16 + 2).min(area.height);
    let width = 70.min(area.width);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    f.render_widget(Clear, popup);
    f.render_widget(Paragraph::new(lines).block(pane("Keys", true)), popup);
}
