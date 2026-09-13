//! A plain, non-modal text editor for the page source: type to insert, arrows to move,
//! Shift+arrows to select, Ctrl-C/X/V for the system clipboard, Ctrl-Z/Y to undo and redo.
//! Long lines soft-wrap at the pane width.

use std::io::Write;
use std::process::{Command, Stdio};

use ratatui::Frame;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthChar;

/// A place in the text: line and character index within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

/// What the editor wants the app to do after a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Write the text and leave the editor.
    Save,
    /// Leave the editor without saving.
    Discard,
}

/// One screen row of a soft-wrapped line: characters `start..end` of `line`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Row {
    line: usize,
    start: usize,
    end: usize,
}

#[derive(Clone)]
struct Snapshot {
    lines: Vec<String>,
    cursor: Pos,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LastEdit {
    Typing,
    Other,
}

pub struct Editor {
    lines: Vec<String>,
    cursor: Pos,
    /// Other end of the selection, if any.
    anchor: Option<Pos>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: LastEdit,
    original: String,
    /// Column to aim for when moving up and down.
    preferred_col: Option<usize>,
    /// First visual row shown.
    scroll: usize,
    /// Text area from the last draw.
    view: Rect,
    /// Fallback clipboard when the system one is unavailable.
    clipboard: String,
    /// Esc was pressed with unsaved changes: waiting for y / n / Esc.
    pub confirm: bool,
    /// Where the cursor was drawn last, for pop-ups anchored to it.
    pub cursor_screen: Position,
}

impl Editor {
    pub fn new(text: &str) -> Editor {
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Editor {
            lines,
            cursor: Pos::default(),
            anchor: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: LastEdit::Other,
            original: text.to_string(),
            preferred_col: None,
            scroll: 0,
            view: Rect::default(),
            clipboard: String::new(),
            confirm: false,
            cursor_screen: Position::default(),
        }
    }

    /// If the cursor is inside an unfinished `[[link`, the position right after `[[` and the
    /// text typed so far.
    pub fn link_context(&self) -> Option<(Pos, String)> {
        let line = &self.lines[self.cursor.line];
        let chars: Vec<char> = line.chars().collect();
        let before = &chars[..self.cursor.col];
        let open = before
            .windows(2)
            .rposition(|w| w == ['[', '['])
            .map(|i| i + 2)?;
        let typed: String = before[open..].iter().collect();
        if typed.contains("]]") || typed.contains('|') || typed.contains('[') {
            return None;
        }
        Some((
            Pos {
                line: self.cursor.line,
                col: open,
            },
            typed,
        ))
    }

    /// Replace the text typed after `[[` with `id` and close the link with `]]`.
    pub fn complete_link(&mut self, start: Pos, id: &str) {
        self.snapshot(LastEdit::Other);
        self.anchor = None;
        self.delete_range(start, self.cursor);
        let rest: String = self.lines[self.cursor.line]
            .chars()
            .skip(self.cursor.col)
            .take(2)
            .collect();
        self.insert_text(id);
        if rest == "]]" {
            self.cursor.col += 2;
        } else {
            self.insert_text("]]");
        }
    }

    /// The text, always ending in a newline.
    pub fn text(&self) -> String {
        let mut text = self.lines.join("\n");
        text.push('\n');
        text
    }

    pub fn modified(&self) -> bool {
        self.text().trim_end() != self.original.trim_end()
    }

    #[cfg(test)]
    pub fn cursor(&self) -> Pos {
        self.cursor
    }

    pub fn cursor_to_end(&mut self) {
        let line = self.lines.len() - 1;
        self.cursor = Pos {
            line,
            col: self.line_len(line),
        };
    }

    // ----- text access -----------------------------------------------------------------

    fn line_len(&self, line: usize) -> usize {
        self.lines[line].chars().count()
    }

    fn byte_at(&self, pos: Pos) -> usize {
        self.lines[pos.line]
            .char_indices()
            .nth(pos.col)
            .map(|(i, _)| i)
            .unwrap_or(self.lines[pos.line].len())
    }

    fn clamp(&self, pos: Pos) -> Pos {
        let line = pos.line.min(self.lines.len() - 1);
        Pos {
            line,
            col: pos.col.min(self.line_len(line)),
        }
    }

    /// The selection as an ordered range, if it is not empty.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            return None;
        }
        Some((anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    fn range_text(&self, start: Pos, end: Pos) -> String {
        if start.line == end.line {
            return self.lines[start.line][self.byte_at(start)..self.byte_at(end)].to_string();
        }
        let mut text = self.lines[start.line][self.byte_at(start)..].to_string();
        for line in &self.lines[start.line + 1..end.line] {
            text.push('\n');
            text.push_str(line);
        }
        text.push('\n');
        text.push_str(&self.lines[end.line][..self.byte_at(end)]);
        text
    }

    // ----- editing ---------------------------------------------------------------------

    fn snapshot(&mut self, edit: LastEdit) {
        if !(edit == LastEdit::Typing && self.last_edit == LastEdit::Typing) {
            self.undo.push(Snapshot {
                lines: self.lines.clone(),
                cursor: self.cursor,
            });
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        self.last_edit = edit;
    }

    fn delete_range(&mut self, start: Pos, end: Pos) {
        let tail = self.lines[end.line][self.byte_at(end)..].to_string();
        let head_end = self.byte_at(start);
        self.lines[start.line].truncate(head_end);
        self.lines[start.line].push_str(&tail);
        self.lines.drain(start.line + 1..=end.line);
        self.cursor = start;
        self.anchor = None;
    }

    /// Remove the selection, if any; returns whether something was removed.
    fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some((start, end)) => {
                self.delete_range(start, end);
                true
            }
            None => {
                self.anchor = None;
                false
            }
        }
    }

    fn insert_text(&mut self, text: &str) {
        let at = self.byte_at(self.cursor);
        let line = &mut self.lines[self.cursor.line];
        let tail = line.split_off(at);
        let mut pieces = text.split('\n');
        line.push_str(pieces.next().unwrap_or(""));
        let mut row = self.cursor.line;
        for piece in pieces {
            row += 1;
            self.lines.insert(row, piece.to_string());
        }
        self.cursor = Pos {
            line: row,
            col: self.lines[row].chars().count(),
        };
        self.lines[row].push_str(&tail);
        self.anchor = None;
        self.preferred_col = None;
    }

    pub fn insert(&mut self, text: &str) {
        let typing = text.chars().count() == 1 && text != "\n";
        self.snapshot(if typing {
            LastEdit::Typing
        } else {
            LastEdit::Other
        });
        self.delete_selection();
        self.insert_text(text);
    }

    fn newline(&mut self) {
        self.snapshot(LastEdit::Other);
        self.delete_selection();
        // Keep the indentation of the current line (nested lists, quoted blocks).
        let indent: String = self.lines[self.cursor.line]
            .chars()
            .take(self.cursor.col)
            .take_while(|c| *c == ' ')
            .collect();
        self.insert_text(&format!("\n{indent}"));
    }

    fn backspace(&mut self) {
        self.snapshot(LastEdit::Other);
        if self.delete_selection() {
            return;
        }
        let Pos { line, col } = self.cursor;
        if col > 0 {
            let start = Pos { line, col: col - 1 };
            self.delete_range(start, self.cursor);
        } else if line > 0 {
            let start = Pos {
                line: line - 1,
                col: self.line_len(line - 1),
            };
            self.delete_range(start, self.cursor);
        }
        self.preferred_col = None;
    }

    fn delete_forward(&mut self) {
        self.snapshot(LastEdit::Other);
        if self.delete_selection() {
            return;
        }
        let Pos { line, col } = self.cursor;
        if col < self.line_len(line) {
            self.delete_range(self.cursor, Pos { line, col: col + 1 });
        } else if line + 1 < self.lines.len() {
            self.delete_range(
                self.cursor,
                Pos {
                    line: line + 1,
                    col: 0,
                },
            );
        }
    }

    fn undo(&mut self) {
        if let Some(snap) = self.undo.pop() {
            self.redo.push(Snapshot {
                lines: std::mem::replace(&mut self.lines, snap.lines),
                cursor: self.cursor,
            });
            self.cursor = self.clamp(snap.cursor);
            self.anchor = None;
            self.last_edit = LastEdit::Other;
        }
    }

    fn redo(&mut self) {
        if let Some(snap) = self.redo.pop() {
            self.undo.push(Snapshot {
                lines: std::mem::replace(&mut self.lines, snap.lines),
                cursor: self.cursor,
            });
            self.cursor = self.clamp(snap.cursor);
            self.anchor = None;
            self.last_edit = LastEdit::Other;
        }
    }

    // ----- clipboard -------------------------------------------------------------------

    fn copy(&mut self) {
        let Some((start, end)) = self.selection() else {
            return;
        };
        let text = self.range_text(start, end);
        self.clipboard = text.clone();
        if let Ok(mut child) = Command::new("wl-copy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }

    fn cut(&mut self) {
        if self.selection().is_none() {
            return;
        }
        self.copy();
        self.snapshot(LastEdit::Other);
        self.delete_selection();
    }

    fn paste_clipboard(&mut self) {
        let system = Command::new("wl-paste")
            .arg("--no-newline")
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
        let text = system.unwrap_or_else(|| self.clipboard.clone());
        if !text.is_empty() {
            self.paste(&text);
        }
    }

    /// Insert text that arrived from outside (the clipboard or a terminal paste).
    pub fn paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        self.snapshot(LastEdit::Other);
        self.delete_selection();
        self.insert_text(&text);
    }

    // ----- movement --------------------------------------------------------------------

    fn move_to(&mut self, pos: Pos, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = self.clamp(pos);
        self.last_edit = LastEdit::Other;
    }

    fn left(&mut self, select: bool) {
        let Pos { line, col } = self.cursor;
        let target = if col > 0 {
            Pos { line, col: col - 1 }
        } else if line > 0 {
            Pos {
                line: line - 1,
                col: self.line_len(line - 1),
            }
        } else {
            self.cursor
        };
        self.move_to(target, select);
        self.preferred_col = None;
    }

    fn right(&mut self, select: bool) {
        let Pos { line, col } = self.cursor;
        let target = if col < self.line_len(line) {
            Pos { line, col: col + 1 }
        } else if line + 1 < self.lines.len() {
            Pos {
                line: line + 1,
                col: 0,
            }
        } else {
            self.cursor
        };
        self.move_to(target, select);
        self.preferred_col = None;
    }

    fn is_word(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    fn word_left(&mut self, select: bool) {
        let chars: Vec<char> = self.lines[self.cursor.line].chars().collect();
        let mut col = self.cursor.col;
        if col == 0 {
            self.left(select);
            return;
        }
        while col > 0 && !Self::is_word(chars[col - 1]) {
            col -= 1;
        }
        while col > 0 && Self::is_word(chars[col - 1]) {
            col -= 1;
        }
        let line = self.cursor.line;
        self.move_to(Pos { line, col }, select);
        self.preferred_col = None;
    }

    fn word_right(&mut self, select: bool) {
        let chars: Vec<char> = self.lines[self.cursor.line].chars().collect();
        let mut col = self.cursor.col;
        if col >= chars.len() {
            self.right(select);
            return;
        }
        while col < chars.len() && Self::is_word(chars[col]) {
            col += 1;
        }
        while col < chars.len() && !Self::is_word(chars[col]) {
            col += 1;
        }
        let line = self.cursor.line;
        self.move_to(Pos { line, col }, select);
        self.preferred_col = None;
    }

    /// Move `delta` visual rows, keeping the visual column.
    fn vertical(&mut self, delta: isize, select: bool) {
        let width = self.view.width.max(1) as usize;
        let rows = self.rows(width);
        let (vrow, vcol) = visual_of(&self.lines, &rows, self.cursor);
        let col = *self.preferred_col.get_or_insert(vcol);
        let target = (vrow as isize + delta).clamp(0, rows.len() as isize - 1) as usize;
        let row = rows[target];
        let pos = pos_at(&self.lines, row, col);
        self.move_to(pos, select);
        self.preferred_col = Some(col);
    }

    fn line_start(&mut self, select: bool) {
        let line = self.cursor.line;
        self.move_to(Pos { line, col: 0 }, select);
        self.preferred_col = None;
    }

    fn line_end(&mut self, select: bool) {
        let line = self.cursor.line;
        let col = self.line_len(line);
        self.move_to(Pos { line, col }, select);
        self.preferred_col = None;
    }

    // ----- input -----------------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if self.confirm {
            self.confirm = false;
            return match key.code {
                KeyCode::Char('y' | 'Y') | KeyCode::Enter => Action::Save,
                KeyCode::Char('n' | 'N') => Action::Discard,
                _ => Action::None,
            };
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let page = self.view.height.max(1) as isize;
        match key.code {
            KeyCode::Esc => {
                if self.modified() {
                    self.confirm = true;
                    return Action::None;
                }
                return Action::Discard;
            }
            KeyCode::Char('s') if ctrl => return Action::Save,
            KeyCode::Char('z') if ctrl && shift => self.redo(),
            KeyCode::Char('z') if ctrl => self.undo(),
            KeyCode::Char('y') if ctrl => self.redo(),
            KeyCode::Char('a') if ctrl => {
                self.anchor = Some(Pos::default());
                self.cursor = Pos {
                    line: self.lines.len() - 1,
                    col: self.line_len(self.lines.len() - 1),
                };
            }
            KeyCode::Char('c') if ctrl => self.copy(),
            KeyCode::Char('x') if ctrl => self.cut(),
            KeyCode::Char('v') if ctrl => self.paste_clipboard(),
            KeyCode::Left if ctrl => self.word_left(shift),
            KeyCode::Right if ctrl => self.word_right(shift),
            KeyCode::Left => self.left(shift),
            KeyCode::Right => self.right(shift),
            KeyCode::Up => self.vertical(-1, shift),
            KeyCode::Down => self.vertical(1, shift),
            KeyCode::PageUp => self.vertical(-page, shift),
            KeyCode::PageDown => self.vertical(page, shift),
            KeyCode::Home if ctrl => self.move_to(Pos::default(), shift),
            KeyCode::End if ctrl => {
                let line = self.lines.len() - 1;
                let col = self.line_len(line);
                self.move_to(Pos { line, col }, shift);
            }
            KeyCode::Home => self.line_start(shift),
            KeyCode::End => self.line_end(shift),
            KeyCode::Enter => self.newline(),
            KeyCode::Tab => self.insert("  "),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete_forward(),
            KeyCode::Char(c) if !ctrl => self.insert(&c.to_string()),
            _ => {}
        }
        Action::None
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let at = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(pos) = self.pos_at_screen(at) {
                    self.move_to(pos, false);
                    self.preferred_col = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(pos) = self.pos_at_screen(at) {
                    self.move_to(pos, true);
                }
            }
            MouseEventKind::ScrollDown => self.scroll += 3,
            MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_sub(3),
            _ => {}
        }
    }

    fn pos_at_screen(&self, (x, y): (u16, u16)) -> Option<Pos> {
        let view = self.view;
        if !view.contains(Position { x, y }) {
            return None;
        }
        let rows = self.rows(view.width.max(1) as usize);
        let vrow = (y - view.y) as usize + self.scroll;
        let row = *rows.get(vrow).or(rows.last())?;
        Some(pos_at(&self.lines, row, (x - view.x) as usize))
    }

    // ----- layout and drawing ----------------------------------------------------------

    fn rows(&self, width: usize) -> Vec<Row> {
        wrap_rows(&self.lines, width)
    }

    /// Draw the text into `area` and place the terminal cursor.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.view = area;
        let width = area.width.max(1) as usize;
        let rows = self.rows(width);
        let (cursor_row, cursor_col) = visual_of(&self.lines, &rows, self.cursor);
        let height = area.height.max(1) as usize;
        if cursor_row < self.scroll {
            self.scroll = cursor_row;
        } else if cursor_row >= self.scroll + height {
            self.scroll = cursor_row + 1 - height;
        }
        self.scroll = self.scroll.min(rows.len().saturating_sub(height));

        let selection = self.selection();
        let selected = Style::new().add_modifier(Modifier::REVERSED);
        let lines: Vec<Line> = rows
            .iter()
            .skip(self.scroll)
            .take(height)
            .map(|row| {
                let chars: Vec<char> = self.lines[row.line].chars().collect();
                let mut spans = Vec::new();
                let mut run = String::new();
                let mut run_selected = false;
                for (i, c) in chars[row.start..row.end].iter().enumerate() {
                    let pos = Pos {
                        line: row.line,
                        col: row.start + i,
                    };
                    let is_selected = selection.is_some_and(|(s, e)| pos >= s && pos < e);
                    if is_selected != run_selected && !run.is_empty() {
                        let style = if run_selected { selected } else { Style::new() };
                        spans.push(Span::styled(std::mem::take(&mut run), style));
                    }
                    run_selected = is_selected;
                    run.push(*c);
                }
                if !run.is_empty() {
                    let style = if run_selected { selected } else { Style::new() };
                    spans.push(Span::styled(run, style));
                }
                // A selected line break shows as a selected cell at the row's end.
                if row.end == chars.len()
                    && selection.is_some_and(|(s, e)| {
                        let end_of_line = Pos {
                            line: row.line,
                            col: chars.len(),
                        };
                        end_of_line >= s && end_of_line < e
                    })
                {
                    spans.push(Span::styled(" ", selected));
                }
                Line::from(spans)
            })
            .collect();
        f.render_widget(Paragraph::new(lines), area);
        let x = area.x + (cursor_col as u16).min(area.width.saturating_sub(1));
        let y = area.y + (cursor_row - self.scroll) as u16;
        self.cursor_screen = Position { x, y };
        f.set_cursor_position(self.cursor_screen);
    }
}

fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// Soft-wrap every line at `width` columns, breaking after a space where possible.
fn wrap_rows(lines: &[String], width: usize) -> Vec<Row> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for (line_no, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let mut start = 0;
        loop {
            let mut used = 0;
            let mut end = start;
            let mut last_space = None;
            while end < chars.len() {
                let w = char_width(chars[end]);
                if used + w > width {
                    break;
                }
                used += w;
                end += 1;
                if chars[end - 1] == ' ' {
                    last_space = Some(end);
                }
            }
            if end < chars.len()
                && let Some(space) = last_space
                && space > start
            {
                end = space;
            }
            if end == start && end < chars.len() {
                end = start + 1;
            }
            rows.push(Row {
                line: line_no,
                start,
                end,
            });
            if end >= chars.len() {
                break;
            }
            start = end;
        }
    }
    rows
}

/// Visual row index and column of a text position.
fn visual_of(lines: &[String], rows: &[Row], pos: Pos) -> (usize, usize) {
    let chars: Vec<char> = lines[pos.line].chars().collect();
    let line_rows: Vec<(usize, &Row)> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.line == pos.line)
        .collect();
    let last = line_rows.len() - 1;
    for (n, (index, row)) in line_rows.iter().enumerate() {
        if pos.col < row.end || n == last {
            let col = chars[row.start..pos.col.min(row.end)]
                .iter()
                .map(|c| char_width(*c))
                .sum();
            return (*index, col);
        }
    }
    (0, 0)
}

/// The text position at visual column `col` of `row`.
fn pos_at(lines: &[String], row: Row, col: usize) -> Pos {
    let chars: Vec<char> = lines[row.line].chars().collect();
    let mut used = 0;
    let mut at = row.start;
    while at < row.end {
        let w = char_width(chars[at]);
        if used + w > col {
            break;
        }
        used += w;
        at += 1;
    }
    // Do not land after a wrap point's trailing space, which belongs to this row.
    if at == row.end && row.end < chars.len() && at > row.start {
        at -= 1;
        if chars[at] != ' ' {
            at += 1;
        }
    }
    Pos {
        line: row.line,
        col: at.min(chars.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    #[test]
    fn link_completion() {
        let mut e = Editor::new("see [[pro");
        e.handle_key(key(KeyCode::End));
        let (start, typed) = e.link_context().unwrap();
        assert_eq!((start, typed.as_str()), (Pos { line: 0, col: 6 }, "pro"));
        e.complete_link(start, "projects/calcbits");
        assert_eq!(e.text(), "see [[projects/calcbits]]\n");
        assert_eq!(e.cursor(), Pos { line: 0, col: 25 });
        assert!(e.link_context().is_none(), "closed links do not complete");

        let mut e = Editor::new("[[a]] and [[b");
        e.handle_key(key(KeyCode::End));
        assert_eq!(e.link_context().unwrap().1, "b");
    }

    #[test]
    fn typing_and_undo() {
        let mut e = Editor::new("ab\n");
        e.handle_key(key(KeyCode::End));
        e.handle_key(key(KeyCode::Char('c')));
        e.handle_key(key(KeyCode::Char('d')));
        assert_eq!(e.text(), "abcd\n");
        assert!(e.modified());
        e.handle_key(ctrl('z'));
        assert_eq!(e.text(), "ab\n", "a typing run undoes as one step");
        e.handle_key(ctrl('y'));
        assert_eq!(e.text(), "abcd\n");
        assert_eq!(e.handle_key(key(KeyCode::Esc)), Action::None);
        assert!(e.confirm);
        assert_eq!(e.handle_key(key(KeyCode::Char('n'))), Action::Discard);
    }

    #[test]
    fn selection_cut_and_paste() {
        let mut e = Editor::new("hello world");
        for _ in 0..5 {
            e.handle_key(shift(KeyCode::Right));
        }
        assert_eq!(
            e.selection(),
            Some((Pos::default(), Pos { line: 0, col: 5 }))
        );
        e.handle_key(ctrl('x'));
        assert_eq!(e.text(), " world\n");
        e.handle_key(key(KeyCode::End));
        e.paste("!\nnext");
        assert_eq!(e.text(), " world!\nnext\n");
        assert_eq!(e.cursor(), Pos { line: 1, col: 4 });
        e.handle_key(key(KeyCode::Backspace));
        e.handle_key(key(KeyCode::Backspace));
        assert_eq!(e.text(), " world!\nne\n");
    }

    #[test]
    fn newline_keeps_indentation_and_words_wrap() {
        let mut e = Editor::new("  - item");
        e.handle_key(key(KeyCode::End));
        e.handle_key(key(KeyCode::Enter));
        e.handle_key(key(KeyCode::Char('x')));
        assert_eq!(e.text(), "  - item\n  x\n");

        let lines = vec!["one two three".to_string(), "".to_string()];
        let rows = wrap_rows(&lines, 8);
        assert_eq!(
            rows,
            vec![
                Row {
                    line: 0,
                    start: 0,
                    end: 8
                },
                Row {
                    line: 0,
                    start: 8,
                    end: 13
                },
                Row {
                    line: 1,
                    start: 0,
                    end: 0
                },
            ]
        );
        assert_eq!(visual_of(&lines, &rows, Pos { line: 0, col: 9 }), (1, 1));
        assert_eq!(pos_at(&lines, rows[1], 3), Pos { line: 0, col: 11 });
    }
}
