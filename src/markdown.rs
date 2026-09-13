//! Renders Markdown into styled, word-wrapped terminal lines, remembering where every link and
//! image ended up so the UI can select, click, and draw them.

use std::path::{Path, PathBuf};

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, LinkType, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::wiki::{LinkTarget, parser_options};

pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub links: Vec<Link>,
    pub images: Vec<ImageSlot>,
    /// Links and images in document order: what `n`/`p` step through.
    pub items: Vec<Item>,
    /// Headings in document order, for the table of contents.
    pub headings: Vec<Heading>,
    pub tasks: Vec<TaskSlot>,
}

pub struct Heading {
    /// 1 for `#`, 2 for `##`, and so on.
    pub level: u8,
    pub text: String,
    /// Line the heading starts on.
    pub line: usize,
}

/// Something on the page that can be selected and activated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// Index into [`Rendered::links`].
    Link(usize),
    /// Index into [`Rendered::images`].
    Image(usize),
    /// Index into [`Rendered::tasks`].
    Task(usize),
}

/// A task-list checkbox: where its marker is drawn and where it lives in the source.
pub struct TaskSlot {
    pub line: usize,
    /// Index of the marker span in that line.
    pub span: usize,
    /// Byte range of `[ ]` / `[x]` in the Markdown source.
    pub source: std::ops::Range<usize>,
}

pub struct Link {
    pub target: LinkTarget,
    /// `(line index, span index)` of every span that belongs to this link.
    pub spans: Vec<(usize, usize)>,
}

/// An image on the page, drawn inside a one-cell frame. The frame occupies lines
/// `line ..= line + height + 1` and columns `x - 1 ..= x + width`; the image itself is drawn at
/// `(x, line + 1)`.
pub struct ImageSlot {
    pub path: PathBuf,
    /// First line of the frame.
    pub line: usize,
    /// Column the image starts at (after the frame's left edge).
    pub x: u16,
    pub width: u16,
    pub height: u16,
    /// Bounds the image was prepared for (the cache key).
    pub max_width: u16,
    pub max_height: u16,
}

/// What the renderer needs from the outside world.
pub trait Context {
    fn resolve(&self, raw: &str) -> LinkTarget;
    fn title(&self, id: &str) -> String;
    /// Folder that relative image paths count from (the page's folder).
    fn image_path(&self, raw: &str) -> Option<PathBuf>;
    /// Prepare the image for drawing within `max_width` x `max_height` cells (scaled down to fit,
    /// never up) and return its size in cells, or `None` if it cannot be shown.
    fn image_size(&mut self, path: &Path, max_width: u16, max_height: u16) -> Option<(u16, u16)>;
}

/// Render `markdown` for a page area `width` columns wide and `height` rows tall (the height
/// only bounds images; the page itself scrolls).
pub fn render(markdown: &str, width: u16, height: u16, ctx: &mut dyn Context) -> Rendered {
    let mut r = Renderer {
        width: width.max(10) as usize,
        height: height.max(5),
        ctx,
        lines: Vec::new(),
        cur: Vec::new(),
        cur_width: 0,
        at_line_start: true,
        need_blank: false,
        prefixes: Vec::new(),
        first_prefix: None,
        styles: vec![Style::default()],
        links: Vec::new(),
        cur_link: None,
        images: Vec::new(),
        items: Vec::new(),
        headings: Vec::new(),
        heading: None,
        tasks: Vec::new(),
        lists: Vec::new(),
        in_code_block: false,
        blank_after_nested: false,
        image_alt: None,
        skip_text: false,
        hiding_title: false,
        table: None,
    };
    r.start_line();
    for (event, range) in Parser::new_ext(markdown, parser_options()).into_offset_iter() {
        r.event(event, range);
    }
    r.finish_line();
    while r.lines.last().is_some_and(|l| l.width() == 0) {
        r.lines.pop();
    }
    Rendered {
        lines: r.lines,
        links: r.links,
        images: r.images,
        items: r.items,
        headings: r.headings,
        tasks: r.tasks,
    }
}

/// The Markdown source as-is, one text line per line, long lines wrapped at `width` columns.
/// `#` heading lines (outside fenced code blocks) still feed the outline.
pub fn render_raw(text: &str, width: u16) -> Rendered {
    use unicode_width::UnicodeWidthChar;
    let width = width.max(10) as usize;
    let mut lines = Vec::new();
    let mut headings = Vec::new();
    let mut in_fence = false;
    for source in text.lines() {
        let trimmed = source.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        } else if !in_fence && let Some((level, heading)) = source_heading(trimmed) {
            headings.push(Heading {
                level,
                text: heading,
                line: lines.len(),
            });
        }
        let source = source.replace('\t', "    ");
        let mut line = String::new();
        let mut used = 0;
        for ch in source.chars() {
            let w = ch.width().unwrap_or(0);
            if used + w > width {
                lines.push(Line::raw(std::mem::take(&mut line)));
                used = 0;
            }
            line.push(ch);
            used += w;
        }
        lines.push(Line::raw(line));
    }
    Rendered {
        lines,
        links: Vec::new(),
        images: Vec::new(),
        items: Vec::new(),
        headings,
        tasks: Vec::new(),
    }
}

/// Level and text of a `# Heading` source line, if it is one.
fn source_heading(line: &str) -> Option<(u8, String)> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = line[hashes..].strip_prefix(' ')?;
    let text = rest.trim().trim_end_matches('#').trim();
    (!text.is_empty()).then(|| (hashes as u8, text.to_string()))
}

#[derive(Clone)]
struct Chunk {
    text: String,
    style: Style,
    link: Option<usize>,
}

struct Table {
    rows: Vec<Vec<Vec<Chunk>>>,
    header_rows: usize,
}

struct Renderer<'a> {
    width: usize,
    /// Rows available for one image (frame included).
    height: u16,
    ctx: &'a mut dyn Context,
    lines: Vec<Line<'static>>,
    cur: Vec<Span<'static>>,
    cur_width: usize,
    at_line_start: bool,
    need_blank: bool,
    /// One continuation prefix per open list item / block quote.
    prefixes: Vec<Span<'static>>,
    /// Replaces the innermost prefix on the next line only (the list bullet).
    first_prefix: Option<Span<'static>>,
    styles: Vec<Style>,
    links: Vec<Link>,
    cur_link: Option<usize>,
    images: Vec<ImageSlot>,
    items: Vec<Item>,
    headings: Vec<Heading>,
    tasks: Vec<TaskSlot>,
    /// The heading being written: level, first line, and its text so far.
    heading: Option<(u8, usize, String)>,
    /// `None` for bullet lists, `Some(next number)` for ordered ones.
    lists: Vec<Option<u64>>,
    in_code_block: bool,
    /// A nested list just ended: separate the next item of the outer list with a blank line.
    blank_after_nested: bool,
    /// Collects an image's alt text while inside the image tag.
    image_alt: Option<(String, String)>,
    /// Ignore text events (used for wiki links that show the page title instead).
    skip_text: bool,
    /// The page's leading `# Title` is being read for the outline but not drawn: the
    /// breadcrumbs already show it.
    hiding_title: bool,
    table: Option<Table>,
}

pub const PAGE_LINK: Style = Style::new()
    .fg(Color::Cyan)
    .add_modifier(Modifier::UNDERLINED);
pub const MISSING_LINK: Style = Style::new()
    .fg(Color::Red)
    .add_modifier(Modifier::UNDERLINED);
pub const EXTERNAL_LINK: Style = Style::new()
    .fg(Color::Blue)
    .add_modifier(Modifier::UNDERLINED);
const CODE: Style = Style::new().fg(Color::Magenta);
const DIM: Style = Style::new().add_modifier(Modifier::DIM);
/// The frame around an image; the UI recolors it when the image is selected.
pub const FRAME: Style = Style::new().add_modifier(Modifier::DIM);

impl Renderer<'_> {
    fn style(&self) -> Style {
        *self.styles.last().unwrap()
    }

    fn push_style(&mut self, patch: Style) {
        let s = self.style().patch(patch);
        self.styles.push(s);
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn prefix_width(&self) -> usize {
        self.prefixes.iter().map(|p| p.width()).sum()
    }

    fn start_line(&mut self) {
        let mut spans = self.prefixes.clone();
        // The bullet stays pending until a line carrying it is written, so a loose list item
        // (whose text starts a new paragraph) still gets it.
        if let Some(first) = &self.first_prefix
            && let Some(last) = spans.last_mut()
        {
            *last = first.clone();
        }
        self.cur_width = spans.iter().map(|s| s.width()).sum();
        self.cur = spans;
        self.at_line_start = true;
    }

    fn newline(&mut self) {
        let line = Line::from(std::mem::take(&mut self.cur));
        self.lines.push(line);
        self.first_prefix = None;
        self.start_line();
    }

    /// End the current line if anything was written on it.
    fn finish_line(&mut self) {
        if !self.at_line_start {
            self.newline();
        }
    }

    fn blank_line(&mut self) {
        let mut spans = self.prefixes.clone();
        if let Some(last) = spans.last_mut() {
            last.content = last.content.trim_end().to_string().into();
        }
        self.lines.push(Line::from(spans));
    }

    /// Called before a block element: separates it from the previous block.
    fn start_block(&mut self) {
        self.finish_line();
        if self.need_blank && !self.lines.is_empty() {
            self.blank_line();
        }
        self.need_blank = false;
        self.start_line();
    }

    fn end_block(&mut self) {
        self.finish_line();
        self.need_blank = true;
    }

    fn emit(&mut self, text: &str, style: Style, link: Option<usize>) {
        if text.is_empty() {
            return;
        }
        if let Some((_, _, heading_text)) = &mut self.heading {
            heading_text.push_str(text);
        }
        if let Some(table) = &mut self.table {
            if let Some(cell) = table.rows.last_mut().and_then(|r| r.last_mut()) {
                cell.push(Chunk {
                    text: text.to_string(),
                    style,
                    link,
                });
            }
            return;
        }
        self.cur.push(Span::styled(text.to_string(), style));
        self.cur_width += text.width();
        self.at_line_start = false;
        if let Some(link) = link {
            self.links[link]
                .spans
                .push((self.lines.len(), self.cur.len() - 1));
        }
    }

    /// Write inline text, wrapping at word boundaries.
    fn write(&mut self, text: &str) {
        let style = self.style();
        let link = self.cur_link;
        if self.table.is_some() {
            self.emit(&text.replace('\n', " "), style, link);
            return;
        }
        for word in text.split_inclusive(' ') {
            let w = word.trim_end().width();
            if !self.at_line_start && self.cur_width + w > self.width {
                self.trim_trailing_space();
                self.newline();
            }
            if self.at_line_start && word.trim().is_empty() {
                continue;
            }
            self.emit(word, style, link);
        }
    }

    fn trim_trailing_space(&mut self) {
        if let Some(last) = self.cur.last_mut() {
            let trimmed = last.content.trim_end().to_string();
            self.cur_width -= last.content.width() - trimmed.width();
            last.content = trimmed.into();
        }
    }

    fn write_code_block(&mut self, text: &str) {
        let style = self.style().patch(CODE);
        for piece in text.split_inclusive('\n') {
            let body = piece.strip_suffix('\n');
            self.emit(body.unwrap_or(piece), style, None);
            if body.is_some() {
                self.at_line_start = false;
                self.newline();
            }
        }
    }

    fn link_style(target: &LinkTarget) -> Style {
        match target {
            LinkTarget::Page(_) => PAGE_LINK,
            LinkTarget::Missing(_) => MISSING_LINK,
            LinkTarget::External(_) => EXTERNAL_LINK,
        }
    }

    fn start_link(&mut self, link_type: LinkType, dest: &str) {
        let target = self.ctx.resolve(dest);
        let style = Self::link_style(&target);
        let is_wiki = matches!(link_type, LinkType::WikiLink { .. });
        let show_title = matches!(link_type, LinkType::WikiLink { has_pothole: false });
        self.links.push(Link {
            target: target.clone(),
            spans: Vec::new(),
        });
        self.cur_link = Some(self.links.len() - 1);
        self.items.push(Item::Link(self.links.len() - 1));
        self.push_style(style);
        if show_title {
            let text = match &target {
                LinkTarget::Page(id) => self.ctx.title(id),
                _ => dest.to_string(),
            };
            self.write(&text);
            self.skip_text = true;
        } else if is_wiki && matches!(target, LinkTarget::Missing(_)) {
            // Piped link to a page that does not exist yet: keep the label, the color says it all.
        }
    }

    fn end_link(&mut self) {
        self.skip_text = false;
        self.cur_link = None;
        self.pop_style();
    }

    fn end_image(&mut self) {
        let Some((dest, alt)) = self.image_alt.take() else {
            return;
        };
        let placeholder = if alt.is_empty() {
            format!("[image: {dest}]")
        } else {
            format!("[image: {alt}]")
        };
        let Some(path) = self.ctx.image_path(&dest) else {
            self.write(&placeholder);
            return;
        };
        let x = self.prefix_width();
        // Leave room for the one-cell frame around the image.
        let max_width = self.width.saturating_sub(x + 2).max(1) as u16;
        let max_height = self.height.saturating_sub(2).max(1);
        match self.ctx.image_size(&path, max_width, max_height) {
            Some((width, height)) if self.table.is_none() => {
                self.finish_line();
                self.images.push(ImageSlot {
                    path,
                    line: self.lines.len(),
                    x: x as u16 + 1,
                    width,
                    height,
                    max_width,
                    max_height,
                });
                self.items.push(Item::Image(self.images.len() - 1));
                let w = width as usize;
                self.frame_line(&format!("╭{}╮", "─".repeat(w)));
                for _ in 0..height {
                    self.frame_line(&format!("│{}│", " ".repeat(w)));
                }
                self.frame_line(&format!("╰{}╯", "─".repeat(w)));
                if !alt.is_empty() {
                    let style = self.style().patch(DIM.add_modifier(Modifier::ITALIC));
                    self.emit(&alt, style, None);
                    self.newline();
                }
            }
            _ => {
                let style = self.style().patch(DIM);
                self.emit(&placeholder, style, None);
            }
        }
    }

    /// One line of an image frame, after any list or quote prefix.
    fn frame_line(&mut self, text: &str) {
        self.start_line();
        self.cur.push(Span::styled(text.to_string(), FRAME));
        self.at_line_start = false;
        self.newline();
    }

    fn end_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![0usize; columns];
        for row in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                let w: usize = cell.iter().map(|c| c.text.width()).sum();
                widths[i] = widths[i].max(w);
            }
        }
        for (r, row) in table.rows.iter().enumerate() {
            for (i, width) in widths.iter().enumerate() {
                if i > 0 {
                    self.emit(" │ ", DIM, None);
                }
                let mut used = 0;
                if let Some(cell) = row.get(i) {
                    for chunk in cell {
                        let style = if r < table.header_rows {
                            chunk.style.add_modifier(Modifier::BOLD)
                        } else {
                            chunk.style
                        };
                        self.emit(&chunk.text, style, chunk.link);
                        used += chunk.text.width();
                    }
                }
                if used < *width {
                    self.emit(&" ".repeat(width - used), Style::default(), None);
                }
            }
            self.at_line_start = false;
            self.newline();
            if r + 1 == table.header_rows {
                let rule = widths
                    .iter()
                    .map(|w| "─".repeat(*w))
                    .collect::<Vec<_>>()
                    .join("─┼─");
                self.emit(&rule, DIM, None);
                self.newline();
            }
        }
    }

    fn event(&mut self, event: Event, range: std::ops::Range<usize>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if self.hiding_title {
                    if let Some((_, _, heading)) = &mut self.heading {
                        heading.push_str(&text);
                    }
                } else if let Some((_, alt)) = &mut self.image_alt {
                    alt.push_str(&text);
                } else if self.in_code_block {
                    self.write_code_block(&text);
                } else if !self.skip_text {
                    self.write(&text);
                }
            }
            Event::Code(code) => {
                let style = self.style().patch(CODE);
                let link = self.cur_link;
                let text = code.to_string();
                if !self.at_line_start && self.cur_width + text.width() > self.width {
                    self.trim_trailing_space();
                    self.newline();
                }
                self.emit(&text, style, link);
            }
            Event::SoftBreak => self.write(" "),
            Event::HardBreak => {
                self.trim_trailing_space();
                self.at_line_start = false;
                self.newline();
            }
            Event::Rule => {
                self.start_block();
                let rule = "─".repeat(self.width.saturating_sub(self.prefix_width()));
                self.emit(&rule, DIM, None);
                self.end_block();
            }
            Event::TaskListMarker(done) => {
                // The checkbox replaces the list bullet, and the item's continuation lines
                // indent to match its width.
                let mark = if done { "✅ " } else { "⬛ " };
                let indent = " ".repeat(mark.width());
                if let Some(bullet) = self.prefixes.last_mut() {
                    *bullet = Span::raw(indent);
                }
                self.first_prefix = Some(Span::raw(mark));
                if self.at_line_start {
                    self.start_line();
                }
                self.tasks.push(TaskSlot {
                    line: self.lines.len(),
                    span: self.prefixes.len().saturating_sub(1),
                    source: range,
                });
                self.items.push(Item::Task(self.tasks.len() - 1));
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                let style = self.style().patch(DIM);
                let link = self.cur_link;
                self.emit(html.trim_end(), style, link);
            }
            Event::FootnoteReference(name) => {
                let style = self.style().patch(DIM);
                self.emit(&format!("[^{name}]"), style, None);
            }
            Event::InlineMath(m) | Event::DisplayMath(m) => self.write(&m),
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.start_block(),
            Tag::Heading { level, .. } if level == HeadingLevel::H1 && self.lines.is_empty() => {
                // The first thing on the page is its title heading: outline only.
                self.hiding_title = true;
                self.heading = Some((1, 0, String::new()));
            }
            Tag::Heading { level, .. } => {
                self.start_block();
                let style = match level {
                    HeadingLevel::H1 => Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    HeadingLevel::H2 => Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
                    HeadingLevel::H3 => Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
                    _ => Style::new().add_modifier(Modifier::BOLD),
                };
                self.push_style(style);
                let marker = match level {
                    HeadingLevel::H1 => "",
                    HeadingLevel::H2 => "## ",
                    HeadingLevel::H3 => "### ",
                    HeadingLevel::H4 => "#### ",
                    HeadingLevel::H5 => "##### ",
                    HeadingLevel::H6 => "###### ",
                };
                if !marker.is_empty() {
                    let style = self.style().patch(DIM);
                    self.emit(marker, style, None);
                }
                let number = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.heading = Some((number, self.lines.len(), String::new()));
            }
            Tag::BlockQuote(_) => {
                self.start_block();
                self.prefixes
                    .push(Span::styled("│ ", Style::new().fg(Color::Green)));
                self.start_line();
            }
            Tag::CodeBlock(kind) => {
                self.start_block();
                if let CodeBlockKind::Fenced(lang) = kind
                    && !lang.is_empty()
                {
                    self.emit(&format!("{lang} "), DIM, None);
                    self.newline();
                }
                self.prefixes.push(Span::raw("  "));
                self.start_line();
                self.in_code_block = true;
            }
            Tag::List(start) => {
                if self.lists.is_empty() {
                    self.start_block();
                } else {
                    self.finish_line();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.finish_line();
                if self.blank_after_nested {
                    self.blank_line();
                    self.blank_after_nested = false;
                }
                self.need_blank = false;
                let bullet = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let s = format!("{n}. ");
                        *n += 1;
                        s
                    }
                    _ => "• ".to_string(),
                };
                let indent = " ".repeat(bullet.width());
                self.prefixes.push(Span::raw(indent));
                self.first_prefix = Some(Span::styled(bullet, Style::new().fg(Color::Yellow)));
                self.start_line();
            }
            Tag::Emphasis => self.push_style(Style::new().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(Style::new().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(Style::new().add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => self.start_link(link_type, &dest_url),
            Tag::Image { dest_url, .. } => {
                self.image_alt = Some((dest_url.to_string(), String::new()));
            }
            Tag::Table(_) => {
                self.start_block();
                self.table = Some(Table {
                    rows: Vec::new(),
                    header_rows: 0,
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.rows.push(Vec::new());
                }
            }
            Tag::TableCell => {
                if let Some(row) = self.table.as_mut().and_then(|t| t.rows.last_mut()) {
                    row.push(Vec::new());
                }
            }
            Tag::FootnoteDefinition(name) => {
                self.start_block();
                let style = self.style().patch(DIM);
                self.emit(&format!("[^{name}]: "), style, None);
            }
            Tag::MetadataBlock(_) => self.skip_text = true,
            Tag::HtmlBlock => self.start_block(),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock | TagEnd::FootnoteDefinition => self.end_block(),
            TagEnd::Heading(_) => {
                if let Some((level, line, text)) = self.heading.take() {
                    self.headings.push(Heading {
                        level,
                        text: text.split_whitespace().collect::<Vec<_>>().join(" "),
                        line,
                    });
                }
                if self.hiding_title {
                    self.hiding_title = false;
                } else {
                    self.pop_style();
                    self.end_block();
                }
            }
            TagEnd::BlockQuote(_) => {
                self.finish_line();
                self.prefixes.pop();
                self.need_blank = true;
                self.start_line();
            }
            TagEnd::CodeBlock => {
                self.finish_line();
                self.in_code_block = false;
                self.prefixes.pop();
                self.need_blank = true;
                self.start_line();
            }
            TagEnd::List(_) => {
                self.lists.pop();
                self.finish_line();
                self.need_blank = self.lists.is_empty();
                // Back at an outer level: the next outer item gets a blank line before it.
                self.blank_after_nested = !self.lists.is_empty();
            }
            TagEnd::Item => {
                self.finish_line();
                self.prefixes.pop();
                self.first_prefix = None;
                self.need_blank = false;
                self.start_line();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => self.pop_style(),
            TagEnd::Link => self.end_link(),
            TagEnd::Image => self.end_image(),
            TagEnd::Table => {
                self.end_table();
                self.end_block();
            }
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.header_rows = t.rows.len();
                }
            }
            TagEnd::MetadataBlock(_) => self.skip_text = false,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ctx;
    impl Context for Ctx {
        fn resolve(&self, raw: &str) -> LinkTarget {
            match raw {
                "a/b" => LinkTarget::Page("a/b".into()),
                r if r.contains("://") => LinkTarget::External(r.into()),
                r => LinkTarget::Missing(r.into()),
            }
        }
        fn title(&self, _id: &str) -> String {
            "Page B".into()
        }
        fn image_path(&self, raw: &str) -> Option<PathBuf> {
            Some(PathBuf::from(raw))
        }
        fn image_size(&mut self, _: &Path, w: u16, h: u16) -> Option<(u16, u16)> {
            Some((10.min(w), 3.min(h)))
        }
    }

    fn text(r: &Rendered) -> Vec<String> {
        r.lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn raw_view_keeps_source_lines() {
        let r = render_raw("# T\n\n- [[a/b]] and *more*\n0123456789abc", 10);
        assert_eq!(
            text(&r),
            vec!["# T", "", "- [[a/b]] ", "and *more*", "0123456789", "abc"]
        );
        assert!(r.links.is_empty());
    }

    #[test]
    fn raw_view_still_finds_headings() {
        let r = render_raw(
            "# Top\n\n```\n# not a heading\n```\n## Two ##\n#nope\n###### Six",
            80,
        );
        let got: Vec<(u8, &str, usize)> = r
            .headings
            .iter()
            .map(|h| (h.level, h.text.as_str(), h.line))
            .collect();
        assert_eq!(got, vec![(1, "Top", 0), (2, "Two", 5), (6, "Six", 7)]);
    }

    #[test]
    fn the_title_heading_is_in_the_outline_but_not_drawn() {
        let r = render("# Title here\n\ntext\n\n# Another h1", 80, 40, &mut Ctx);
        assert_eq!(text(&r), vec!["text", "", "Another h1"]);
        let got: Vec<(u8, &str, usize)> = r
            .headings
            .iter()
            .map(|h| (h.level, h.text.as_str(), h.line))
            .collect();
        assert_eq!(got, vec![(1, "Title here", 0), (1, "Another h1", 2)]);
    }

    #[test]
    fn headings_are_collected_with_their_lines() {
        let r = render(
            "intro\n\n# Top\n\ntext\n\n## Two *words*\n\n### Deep",
            80,
            40,
            &mut Ctx,
        );
        let got: Vec<(u8, &str, usize)> = r
            .headings
            .iter()
            .map(|h| (h.level, h.text.as_str(), h.line))
            .collect();
        assert_eq!(
            got,
            vec![(1, "Top", 2), (2, "Two words", 6), (3, "Deep", 8)]
        );
    }

    #[test]
    fn wraps_words() {
        let r = render("one two three four five", 10, 40, &mut Ctx);
        assert_eq!(text(&r), vec!["one two", "three four", "five"]);
    }

    #[test]
    fn wiki_links_show_titles_and_are_tracked() {
        let r = render(
            "See [[a/b]] and [[a/b|that]] or [[nope]].",
            80,
            40,
            &mut Ctx,
        );
        assert_eq!(text(&r), vec!["See Page B and that or nope."]);
        assert_eq!(r.links.len(), 3);
        assert_eq!(r.links[0].target, LinkTarget::Page("a/b".into()));
        assert_eq!(r.links[2].target, LinkTarget::Missing("nope".into()));
        let (line, span) = r.links[1].spans[0];
        assert_eq!(r.lines[line].spans[span].content, "that");
    }

    #[test]
    fn a_blank_line_follows_a_nested_list() {
        let r = render(
            "* One\n* Two\n  * Two A\n  * Two B\n* Three\n* Four\n\n1. Main\n2. Parent\n   1. Child\n   2. Child\n3. Main\n4. Main",
            80,
            40,
            &mut Ctx,
        );
        assert_eq!(
            text(&r),
            vec![
                "• One",
                "• Two",
                "  • Two A",
                "  • Two B",
                "",
                "• Three",
                "• Four",
                "",
                "1. Main",
                "2. Parent",
                "   1. Child",
                "   2. Child",
                "",
                "3. Main",
                "4. Main",
            ]
        );
    }

    #[test]
    fn nested_list_at_the_end_adds_no_blank() {
        let r = render("* One\n  * Deep\n\nAfter", 80, 40, &mut Ctx);
        assert_eq!(text(&r), vec!["• One", "  • Deep", "", "After"]);
    }

    #[test]
    fn task_items_use_checkbox_bullets() {
        let src = "- [ ] open item\n- [x] done item here";
        let r = render(src, 14, 40, &mut Ctx);
        assert_eq!(text(&r), vec!["⬛ open item", "✅ done item", "   here"]);
        assert_eq!(r.items, vec![Item::Task(0), Item::Task(1)]);
        let t = &r.tasks[1];
        assert_eq!((t.line, &src[t.source.clone()]), (1, "[x]"));
        assert_eq!(r.lines[t.line].spans[t.span].content, "✅ ");
    }

    #[test]
    fn loose_list_items_keep_their_bullets() {
        let r = render("- one\n\n- [x] two\n\n  more", 80, 40, &mut Ctx);
        assert_eq!(text(&r), vec!["• one", "✅ two", "", "   more"]);
    }

    #[test]
    fn lists_and_quotes_get_prefixes() {
        let r = render(
            "- one\n- two\n  - nested\n\n> quoted\n> text",
            80,
            40,
            &mut Ctx,
        );
        assert_eq!(
            text(&r),
            vec!["• one", "• two", "  • nested", "", "│ quoted text"]
        );
    }

    #[test]
    fn images_get_a_frame_and_join_the_items() {
        let r = render(
            "before [[a/b]]\n\n![alt](pic.png)\n\nafter",
            80,
            40,
            &mut Ctx,
        );
        assert_eq!(r.images.len(), 1);
        let slot = &r.images[0];
        assert_eq!((slot.line, slot.x, slot.width, slot.height), (2, 1, 10, 3));
        assert_eq!(
            text(&r),
            vec![
                "before Page B",
                "",
                "╭──────────╮",
                "│          │",
                "│          │",
                "│          │",
                "╰──────────╯",
                "alt",
                "",
                "after"
            ]
        );
        assert_eq!(r.items, vec![Item::Link(0), Item::Image(0)]);
    }

    /// Reports whatever bound it is given, like a huge image scaled down to fit.
    struct Huge;
    impl Context for Huge {
        fn resolve(&self, raw: &str) -> LinkTarget {
            Ctx.resolve(raw)
        }
        fn title(&self, id: &str) -> String {
            Ctx.title(id)
        }
        fn image_path(&self, raw: &str) -> Option<PathBuf> {
            Ctx.image_path(raw)
        }
        fn image_size(&mut self, _: &Path, w: u16, h: u16) -> Option<(u16, u16)> {
            Some((w, h))
        }
    }

    #[test]
    fn images_are_bounded_by_the_page_area_minus_the_frame() {
        let r = render("![alt](pic.png)", 80, 6, &mut Huge);
        assert_eq!((r.images[0].width, r.images[0].height), (78, 4));
        assert_eq!(r.lines[0].to_string().len(), "╭╮".len() + 78 * "─".len());
    }

    #[test]
    fn tables_line_up() {
        let r = render("| a | bb |\n|---|---|\n| ccc | d |", 80, 40, &mut Ctx);
        assert_eq!(text(&r), vec!["a   │ bb", "────┼───", "ccc │ d "]);
    }
}
