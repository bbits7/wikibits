//! Draws the three columns, the status line and the help popup.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use ratatui_image::Image;
use ratatui_image::sliced::SlicedImage;

use crate::app::{App, Focus, Prompt, RelatedRow, TreeKind};
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
    if app.search.is_some() {
        draw_search(f, app, main);
    }
    if app.complete.is_some() {
        draw_complete(f, app, area);
    }
    if app.image_pick.is_some() {
        draw_image_pick(f, app, area);
    }
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
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border);
    if title.is_empty() {
        block
    } else {
        block.title(Span::styled(format!(" {title} "), title_style(focused)))
    }
}

fn title_style(focused: bool) -> Style {
    if focused {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    }
}

/// A column is a title line above a bordered box; returns `(title, box)`.
fn column(area: Rect) -> (Rect, Rect) {
    let [title, boxed] = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(area);
    // Indent the title to line up with the box's contents.
    let title = Rect {
        x: title.x + 1,
        width: title.width.saturating_sub(1),
        ..title
    };
    (title, boxed)
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
    let (title, area) = column(area);
    f.render_widget(Paragraph::new("Pages").style(title_style(focused)), title);
    let block = pane("", focused);
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
    let (title, area) = column(area);
    f.render_widget(Paragraph::new(crumbs_title(app, title, focused)), title);
    let block = pane("", focused);
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

    if let Some(editor) = &mut app.editor {
        editor.render(f, body);
        return;
    }

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
        Some(Item::Task(t)) => {
            let slot = &rendered.tasks[*t];
            if slot.line >= scroll
                && let Some(s) = lines
                    .get_mut(slot.line - scroll)
                    .and_then(|l| l.spans.get_mut(slot.span))
            {
                s.style = s.style.add_modifier(Modifier::REVERSED);
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
    if let Some(find) = &app.find {
        for (i, m) in find.matches.iter().enumerate() {
            if m.line >= scroll
                && let Some(line) = lines.get_mut(m.line - scroll)
            {
                let style = if i == find.current {
                    Style::new().fg(Color::Black).bg(Color::Yellow)
                } else {
                    Style::new().fg(Color::Black).bg(Color::DarkGray)
                };
                highlight_columns(line, m.start, m.end, style);
            }
        }
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

/// The breadcrumbs for the line above the page box, remembering where each crumb lands so a
/// click on it can be told apart.
fn crumbs_title(app: &mut App, area: Rect, focused: bool) -> Line<'static> {
    let mut spans = Vec::new();
    let mut columns = Vec::new();
    let mut col = 0u16;
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
    if let Some(editor) = &app.editor {
        let mark = if editor.modified() {
            " (editing •)"
        } else {
            " (editing)"
        };
        spans.push(Span::styled(mark, Style::new().fg(Color::Yellow)));
    } else if app.raw {
        spans.push(Span::styled(
            " (source)",
            Style::new().add_modifier(Modifier::DIM),
        ));
    }
    app.crumb_columns = columns;
    app.rects.crumbs = area;
    Line::from(spans)
}

fn draw_related(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Related;
    let (title, area) = column(area);
    f.render_widget(Paragraph::new("Related").style(title_style(focused)), title);
    let block = pane("", focused);
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

/// Restyle the cells of `line` in columns `start..end`, splitting spans as needed.
fn highlight_columns(line: &mut Line<'static>, start: u16, end: u16, style: Style) {
    use unicode_width::UnicodeWidthChar;
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut col = 0u16;
    for span in line.spans.drain(..) {
        let mut run = String::new();
        let mut run_hit = false;
        for c in span.content.chars() {
            let hit = col >= start && col < end;
            if hit != run_hit && !run.is_empty() {
                let s = if run_hit {
                    span.style.patch(style)
                } else {
                    span.style
                };
                out.push(Span::styled(std::mem::take(&mut run), s));
            }
            run_hit = hit;
            run.push(c);
            col += c.width().unwrap_or(0) as u16;
        }
        if !run.is_empty() {
            let s = if run_hit {
                span.style.patch(style)
            } else {
                span.style
            };
            out.push(Span::styled(run, s));
        }
    }
    line.spans = out;
}

/// The image picker: images under the wiki root, filtered by what is typed.
fn draw_image_pick(f: &mut Frame, app: &App, area: Rect) {
    let (Some(pick), Some(editor)) = (&app.image_pick, &app.editor) else {
        return;
    };
    let width = pick
        .all
        .iter()
        .map(|p| unicode_width::UnicodeWidthStr::width(p.as_str()) as u16 + 4)
        .max()
        .unwrap_or(30)
        .clamp(34, 70);
    let shown = pick.hits.len().clamp(1, 10) as u16;
    let height = shown + 3;
    let at = editor.cursor_screen;
    let x = at.x.min(area.x + area.width.saturating_sub(width));
    let y = if at.y + 1 + height <= area.y + area.height {
        at.y + 1
    } else {
        at.y.saturating_sub(height)
    };
    let rect = Rect {
        x,
        y,
        width,
        height,
    };
    let block = pane("Insert image", true);
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);
    let mut lines = vec![Line::from(vec![
        Span::styled(" Filter: ", Style::new().fg(Color::Yellow)),
        Span::raw(pick.query.clone()),
        Span::styled("▏", Style::new().fg(ACCENT)),
    ])];
    if pick.hits.is_empty() {
        let msg = if pick.all.is_empty() {
            " no images in the wiki folder"
        } else {
            " no image matches"
        };
        lines.push(Line::styled(msg, Style::new().add_modifier(Modifier::DIM)));
    }
    let first = pick.sel.saturating_sub(shown as usize - 1);
    for (i, path) in pick
        .hits
        .iter()
        .enumerate()
        .skip(first)
        .take(shown as usize)
    {
        let style = if i == pick.sel {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        };
        let mut text = format!(" {path}");
        let pad = (inner.width as usize)
            .saturating_sub(unicode_width::UnicodeWidthStr::width(text.as_str()));
        text.push_str(&" ".repeat(pad));
        lines.push(Line::styled(text, style));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Page suggestions for a `[[` link, anchored under (or above) the editor's cursor.
fn draw_complete(f: &mut Frame, app: &App, area: Rect) {
    let (Some(complete), Some(editor)) = (&app.complete, &app.editor) else {
        return;
    };
    let rows: Vec<String> = complete
        .hits
        .iter()
        .map(|(id, title)| format!(" {title}  {id} "))
        .collect();
    let width = rows
        .iter()
        .map(|r| unicode_width::UnicodeWidthStr::width(r.as_str()) as u16)
        .max()
        .unwrap_or(20)
        .clamp(20, 60)
        + 2;
    let height = rows.len().max(1) as u16 + 2;
    let at = editor.cursor_screen;
    let x = at.x.min(area.x + area.width.saturating_sub(width));
    let y = if at.y + 1 + height <= area.y + area.height {
        at.y + 1
    } else {
        at.y.saturating_sub(height)
    };
    let rect = Rect {
        x,
        y,
        width,
        height,
    };
    let block = pane("", true);
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);
    let lines: Vec<Line> = if rows.is_empty() {
        vec![Line::styled(
            " no matching page",
            Style::new().add_modifier(Modifier::DIM),
        )]
    } else {
        complete
            .hits
            .iter()
            .enumerate()
            .map(|(i, (id, title))| {
                let style = if i == complete.sel {
                    Style::new().add_modifier(Modifier::REVERSED)
                } else {
                    Style::new()
                };
                let mut text = format!(" {title}  ");
                let pad = (inner.width as usize).saturating_sub(
                    unicode_width::UnicodeWidthStr::width(text.as_str()) + id.len() + 1,
                );
                text.push_str(&" ".repeat(pad));
                Line::from(vec![
                    Span::styled(text, style.add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{id} "), style.add_modifier(Modifier::DIM)),
                ])
            })
            .collect()
    };
    f.render_widget(Paragraph::new(lines), inner);
}

/// The wiki search results, two lines per page, under the prompt in the status bar.
fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(search) = &mut app.search else {
        return;
    };
    let width = (area.width * 2 / 3).clamp(30, 100).min(area.width);
    let rows = search.hits.len().clamp(1, 12) as u16 * 2;
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + 2,
        width,
        height: (rows + 2).min(area.height),
    };
    let title = if search.query.trim().is_empty() {
        " Search the wiki ".to_string()
    } else {
        format!(
            " {} page{} for \"{}\" ",
            search.hits.len(),
            if search.hits.len() == 1 { "" } else { "s" },
            search.query
        )
    };
    let block = pane(&title, true);
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);
    search.view = inner;
    let visible = (inner.height / 2) as usize;
    if search.sel < search.scroll {
        search.scroll = search.sel;
    } else if visible > 0 && search.sel >= search.scroll + visible {
        search.scroll = search.sel + 1 - visible;
    }
    let mut lines: Vec<Line> = Vec::new();
    if search.hits.is_empty() {
        let msg = if search.query.trim().is_empty() {
            "Type to search page titles and text."
        } else {
            "No page matches."
        };
        lines.push(Line::styled(msg, Style::new().add_modifier(Modifier::DIM)));
    }
    for (i, hit) in search
        .hits
        .iter()
        .enumerate()
        .skip(search.scroll)
        .take(visible)
    {
        let selected = i == search.sel;
        let title_style = if selected {
            Style::new()
                .fg(ACCENT)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
        };
        let mut head = format!(" {}", hit.title);
        let pad = (inner.width as usize)
            .saturating_sub(unicode_width::UnicodeWidthStr::width(head.as_str()));
        head.push_str(&" ".repeat(pad));
        lines.push(Line::from(vec![Span::styled(head, title_style)]));
        lines.push(Line::from(vec![
            Span::styled(
                format!("   {}", hit.id),
                Style::new().add_modifier(Modifier::DIM),
            ),
            Span::styled(
                if hit.snippet.is_empty() {
                    String::new()
                } else {
                    format!("  {}", hit.snippet)
                },
                Style::new(),
            ),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    if let Some(editor) = &app.editor {
        let line = if editor.confirm {
            Line::from(vec![
                Span::styled("Save changes? ", Style::new().fg(Color::Yellow)),
                Span::raw("y = save   n = discard   Esc = keep editing"),
            ])
        } else {
            Line::styled(
                "Ctrl-S save  Esc back  Shift+arrows select  Ctrl-C/X/V copy/cut/paste  Ctrl-Z/Y undo/redo  Ctrl-P image",
                Style::new().add_modifier(Modifier::DIM),
            )
        };
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.prompt == Prompt::CreatePage {
        let id = app.pending_create.clone().unwrap_or_default();
        let line = Line::from(vec![
            Span::styled(
                format!("Create page '{id}'? "),
                Style::new().fg(Color::Yellow),
            ),
            Span::raw("y = create and edit   Enter or n = no"),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.prompt == Prompt::DeletePage {
        let id = app.current.clone().unwrap_or_default();
        let linkers = app.wiki.backlinks_of(&id).len();
        let warning = match linkers {
            0 => String::new(),
            1 => " (1 page links to it)".to_string(),
            n => format!(" ({n} pages link to it)"),
        };
        let line = Line::from(vec![
            Span::styled(
                format!("Delete page '{id}'{warning}? "),
                Style::new().fg(Color::Red),
            ),
            Span::raw("y = delete   n = keep"),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.prompt == Prompt::NewPage || app.prompt == Prompt::RenamePage {
        let label = if app.prompt == Prompt::NewPage {
            "New page (folder/name): "
        } else {
            "Rename page to (folder/name): "
        };
        let hint = if app.prompt == Prompt::NewPage {
            "   Enter create  Esc cancel"
        } else {
            "   Enter rename (links to it are updated)  Esc cancel"
        };
        let line = Line::from(vec![
            Span::styled(label, Style::new().fg(Color::Yellow)),
            Span::raw(app.new_page.clone()),
            Span::styled("▏", Style::new().fg(ACCENT)),
            Span::styled(hint, Style::new().add_modifier(Modifier::DIM)),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.prompt != Prompt::None {
        let (label, query, extra) = match app.prompt {
            Prompt::FindPage => {
                let find = app.find.as_ref();
                let query = find.map(|f| f.query.as_str()).unwrap_or("");
                let extra = match find {
                    Some(f) if !f.query.is_empty() && f.matches.is_empty() => {
                        "  no match".to_string()
                    }
                    Some(f) if !f.matches.is_empty() => {
                        format!("  {}/{}", f.current + 1, f.matches.len())
                    }
                    _ => String::new(),
                };
                ("Find in page: ", query.to_string(), extra)
            }
            _ => (
                "Search wiki: ",
                app.search
                    .as_ref()
                    .map(|s| s.query.clone())
                    .unwrap_or_default(),
                String::new(),
            ),
        };
        let line = Line::from(vec![
            Span::styled(label, Style::new().fg(Color::Yellow)),
            Span::raw(query),
            Span::styled("▏", Style::new().fg(ACCENT)),
            Span::styled(extra, Style::new().add_modifier(Modifier::DIM)),
            Span::styled(
                "   Enter next  Up previous  Esc close",
                Style::new().add_modifier(Modifier::DIM),
            ),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    let hints = match app.focus {
        _ if app.popup.is_some() => "h/j/k/l scroll  PgUp/PgDn  g/G  Esc close",
        Focus::Tree => "j/k move  Enter open  h/l fold  Tab pane  b back  ? help  q quit",
        Focus::Content => {
            "j/k scroll  n/p link  Enter follow  x tick  v source  Tab pane  b back  ? help  q quit"
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
        ("n / p", "next / previous link, image or task"),
        ("x", "tick / untick the selected task (Enter does too)"),
        ("v", "toggle Markdown source / rendered page"),
        ("/", "find in this page (Enter / Up step, Esc closes)"),
        ("s", "search the wiki (Enter opens the page at the match)"),
        ("e", "edit this page"),
        ("N", "new page (type folder/name)"),
        ("R", "rename / move this page (links to it are updated)"),
        ("D", "delete this page (asks first)"),
        ("Enter on a red link", "offers to create that page"),
        ("", ""),
        ("Editor", ""),
        ("Ctrl-S  Esc", "save and return / return (asks if changed)"),
        ("Shift+arrows  Ctrl-A", "select / select all"),
        (
            "Ctrl-C  Ctrl-X  Ctrl-V",
            "copy / cut / paste (system clipboard)",
        ),
        ("Ctrl-Z  Ctrl-Y", "undo / redo"),
        (
            "Ctrl+Left/Right",
            "word left / right;  Tab inserts two spaces",
        ),
        (
            "[[",
            "suggests pages as you type; Enter / Tab inserts the link",
        ),
        (
            "Ctrl-P",
            "insert an image from the wiki folder (type to filter)",
        ),
        (
            "Ctrl-V with an image",
            "saves it to assets/ and inserts the link",
        ),
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
