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
    /// Ctrl-V: the app reads the clipboard (text or an image) and inserts it.
    Paste,
    /// Ctrl-P: the app opens the image picker (Ctrl-I would be Tab in a terminal).
    PickImage,
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
    /// The find / replace bar, when open.
    pub find: Option<EditorFind>,
}

/// The editor's find bar: `Ctrl-F` opens it, `Ctrl-R` adds a replacement.
pub struct EditorFind {
    pub query: String,
    /// `Some` once Ctrl-R was pressed: the replacement being typed.
    pub replace: Option<String>,
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
            find: None,
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
        let line = self.lines[self.cursor.line].clone();
        let (indent, marker) = list_prefix(&line);
        // Enter on an empty list item ends the list instead of adding another marker.
        if !marker.is_empty()
            && line.trim_end().chars().count() == indent.len() + marker.chars().count() - 1
        {
            let start = Pos {
                line: self.cursor.line,
                col: 0,
            };
            let end = Pos {
                line: self.cursor.line,
                col: line.chars().count(),
            };
            self.delete_range(start, end);
            return;
        }
        let at_end = self.cursor.col >= line.chars().count();
        // Continue the list only when breaking at the end of the item; keep the indentation
        // otherwise (nested lists, quoted blocks).
        let next_marker = if at_end {
            next_list_marker(&marker)
        } else {
            String::new()
        };
        self.insert_text(&format!("\n{indent}{next_marker}"));
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
        clipboard_copy(&text);
    }

    fn cut(&mut self) {
        if self.selection().is_none() {
            return;
        }
        self.copy();
        self.snapshot(LastEdit::Other);
        self.delete_selection();
    }

    /// Insert what was last copied or cut here, when the system clipboard has nothing.
    pub fn paste_internal(&mut self) {
        let text = self.clipboard.clone();
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
        if self.find.is_some() {
            self.find_key(key, ctrl);
            return Action::None;
        }
        if ctrl && key.code == KeyCode::Char('f') {
            let query = self
                .selection()
                .map(|(s, e)| self.range_text(s, e))
                .filter(|t| !t.contains('\n'))
                .unwrap_or_default();
            self.find = Some(EditorFind {
                query,
                replace: None,
            });
            return Action::None;
        }
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
            KeyCode::Char('v') if ctrl => return Action::Paste,
            KeyCode::Char('p') if ctrl => return Action::PickImage,
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

    // ----- find and replace --------------------------------------------------------------

    fn find_key(&mut self, key: KeyEvent, ctrl: bool) {
        let Some(find) = &mut self.find else { return };
        match (find.replace.is_some(), key.code) {
            (_, KeyCode::Esc) => self.find = None,
            (_, KeyCode::Char('r')) if ctrl => {
                if find.replace.is_none() {
                    find.replace = Some(String::new());
                }
            }
            (true, KeyCode::Char('a')) if ctrl => self.replace_all(),
            (false, KeyCode::Enter | KeyCode::Down) => self.find_next(false),
            (false, KeyCode::Up) => self.find_next(true),
            (true, KeyCode::Enter) => self.replace_current(),
            (true, KeyCode::Down) => self.find_next(false),
            (true, KeyCode::Up) => self.find_next(true),
            (false, KeyCode::Backspace) => {
                find.query.pop();
            }
            (true, KeyCode::Backspace) => {
                find.replace.as_mut().unwrap().pop();
            }
            (false, KeyCode::Char(c)) if !ctrl => {
                find.query.push(c);
                self.find_next_from_here();
            }
            (true, KeyCode::Char(c)) if !ctrl => find.replace.as_mut().unwrap().push(c),
            _ => {}
        }
    }

    /// Select the match at or after the cursor as the query grows.
    fn find_next_from_here(&mut self) {
        let from = self.selection().map(|(s, _)| s).unwrap_or(self.cursor);
        if let Some((s, e)) = self.find_match_from(from, false) {
            self.anchor = Some(s);
            self.cursor = e;
        }
    }

    /// Select the next (or previous) match, wrapping around.
    fn find_next(&mut self, backwards: bool) {
        let from = match (backwards, self.selection()) {
            (false, Some((_, e))) => e,
            (true, Some((s, _))) => s,
            (_, None) => self.cursor,
        };
        if let Some((s, e)) = self.find_match_from(from, backwards) {
            self.anchor = Some(s);
            self.cursor = e;
            self.preferred_col = None;
        }
    }

    fn find_match_from(&self, from: Pos, backwards: bool) -> Option<(Pos, Pos)> {
        let query: Vec<char> = self
            .find
            .as_ref()?
            .query
            .chars()
            .map(|c| c.to_lowercase().next().unwrap_or(c))
            .collect();
        if query.is_empty() {
            return None;
        }
        let mut matches: Vec<(Pos, Pos)> = Vec::new();
        for (line_no, line) in self.lines.iter().enumerate() {
            let lower: Vec<char> = line
                .chars()
                .map(|c| c.to_lowercase().next().unwrap_or(c))
                .collect();
            let mut at = 0;
            while at + query.len() <= lower.len() {
                if lower[at..at + query.len()] == query[..] {
                    matches.push((
                        Pos {
                            line: line_no,
                            col: at,
                        },
                        Pos {
                            line: line_no,
                            col: at + query.len(),
                        },
                    ));
                    at += query.len();
                } else {
                    at += 1;
                }
            }
        }
        if backwards {
            matches
                .iter()
                .rev()
                .find(|(s, _)| *s < from)
                .or(matches.last())
                .copied()
        } else {
            matches
                .iter()
                .find(|(s, _)| *s >= from)
                .or(matches.first())
                .copied()
        }
    }

    /// Replace the selected match and move to the next one.
    fn replace_current(&mut self) {
        let Some(find) = &self.find else { return };
        let (query, replacement) = (find.query.clone(), find.replace.clone().unwrap_or_default());
        let selected = self
            .selection()
            .map(|(s, e)| self.range_text(s, e))
            .unwrap_or_default();
        if !selected.is_empty() && selected.to_lowercase() == query.to_lowercase() {
            self.snapshot(LastEdit::Other);
            self.delete_selection();
            self.insert_text(&replacement);
        }
        self.find_next(false);
    }

    fn replace_all(&mut self) {
        let Some(find) = &self.find else { return };
        let (query, replacement) = (find.query.clone(), find.replace.clone().unwrap_or_default());
        if query.is_empty() {
            return;
        }
        self.snapshot(LastEdit::Other);
        self.anchor = None;
        let mut count = 0;
        while let Some((s, e)) = self.find_match_from(Pos::default(), false) {
            self.delete_range(s, e);
            self.cursor = s;
            self.insert_text(&replacement);
            count += 1;
            if count > 10_000 || replacement.to_lowercase().contains(&query.to_lowercase()) {
                // Replacing with something that still matches would never end.
                if count == 1 {
                    let mut rest = Pos::default();
                    while let Some((s, e)) = self.find_match_from(rest, false) {
                        if s < rest {
                            break;
                        }
                        self.delete_range(s, e);
                        self.cursor = s;
                        self.insert_text(&replacement);
                        rest = self.cursor;
                    }
                }
                break;
            }
        }
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
        let fenced = fenced_lines(&self.lines);
        let lines: Vec<Line> = rows
            .iter()
            .skip(self.scroll)
            .take(height)
            .map(|row| {
                let chars: Vec<char> = self.lines[row.line].chars().collect();
                let styles = markdown_styles(&chars, fenced[row.line]);
                let mut spans = Vec::new();
                let mut run = String::new();
                let mut run_style = Style::new();
                for (i, c) in chars[row.start..row.end].iter().enumerate() {
                    let col = row.start + i;
                    let pos = Pos {
                        line: row.line,
                        col,
                    };
                    let mut style = styles[col];
                    if selection.is_some_and(|(s, e)| pos >= s && pos < e) {
                        style = style.patch(selected);
                    }
                    if style != run_style && !run.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut run), run_style));
                    }
                    run_style = style;
                    run.push(*c);
                }
                if !run.is_empty() {
                    spans.push(Span::styled(run, run_style));
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

/// Put `text` on the system clipboard through `wl-copy`; false if that did not work.
pub fn clipboard_copy(text: &str) -> bool {
    let Ok(mut child) = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().is_ok_and(|s| s.success())
}

/// Leading indentation and list marker of a line: `("  ", "- [ ] ")`, `("", "3. ")`, or
/// empty strings when the line is not a list item.
fn list_prefix(line: &str) -> (String, String) {
    let indent: String = line.chars().take_while(|c| *c == ' ').collect();
    let rest = &line[indent.len()..];
    let bullet = rest
        .strip_prefix("- ")
        .or_else(|| rest.strip_prefix("* "))
        .or_else(|| rest.strip_prefix("+ "))
        .map(|after| (rest[..2].to_string(), after));
    let number = rest
        .find(". ")
        .filter(|dot| *dot > 0 && rest[..*dot].bytes().all(|b| b.is_ascii_digit()))
        .map(|dot| (rest[..dot + 2].to_string(), &rest[dot + 2..]));
    let Some((mut marker, after)) = bullet.or(number) else {
        return (indent, String::new());
    };
    if after.starts_with("[ ] ") || after.starts_with("[x] ") || after.starts_with("[X] ") {
        marker.push_str(&after[..4]);
    }
    (indent, marker)
}

/// The marker for the item after one with `marker`: numbers count up, tasks start unticked.
fn next_list_marker(marker: &str) -> String {
    let mut next = marker.to_string();
    if let Some(dot) = marker.find(". ")
        && let Ok(n) = marker[..dot].parse::<u64>()
    {
        next = format!("{}. {}", n + 1, &marker[dot + 2..]);
    }
    next.replace("[x] ", "[ ] ").replace("[X] ", "[ ] ")
}

/// Which lines sit inside ``` fences.
fn fenced_lines(lines: &[String]) -> Vec<bool> {
    let mut inside = false;
    lines
        .iter()
        .map(|l| {
            let fence = l.trim_start().starts_with("```") || l.trim_start().starts_with("~~~");
            if fence {
                inside = !inside;
                return true;
            }
            inside
        })
        .collect()
}

/// A style per character of a source line: headings, list markers, links and code.
fn markdown_styles(chars: &[char], fenced: bool) -> Vec<Style> {
    use ratatui::style::Color;
    let dim = Style::new().add_modifier(Modifier::DIM);
    let mut styles = vec![Style::new(); chars.len()];
    if fenced {
        return vec![Style::new().fg(Color::Magenta); chars.len()];
    }
    let text: String = chars.iter().collect();
    let trimmed = text.trim_start();
    let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
        let color = match hashes {
            1 => Color::Yellow,
            2 => Color::Green,
            _ => Color::Blue,
        };
        return vec![Style::new().fg(color).add_modifier(Modifier::BOLD); chars.len()];
    }
    let (indent, marker) = list_prefix(&text);
    for style in styles
        .iter_mut()
        .skip(indent.len())
        .take(marker.chars().count())
    {
        *style = Style::new().fg(Color::Yellow);
    }
    if trimmed.starts_with('>') {
        let at = chars.len() - trimmed.chars().count();
        styles[at] = Style::new().fg(Color::Green);
    }
    // Inline spans: `code`, [[links]], [text](url).
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '`'
            && let Some(len) = chars[i + 1..].iter().position(|c| *c == '`')
        {
            for s in &mut styles[i..=i + 1 + len] {
                *s = Style::new().fg(Color::Magenta);
            }
            i += len + 2;
        } else if chars[i] == '['
            && chars.get(i + 1) == Some(&'[')
            && let Some(len) = chars[i + 2..].windows(2).position(|w| w == [']', ']'])
        {
            for s in &mut styles[i..i + len + 4] {
                *s = Style::new().fg(Color::Cyan);
            }
            i += len + 4;
        } else if chars[i] == '['
            && let Some(close) = chars[i + 1..].iter().position(|c| *c == ']')
            && chars.get(i + close + 2) == Some(&'(')
            && let Some(end) = chars[i + close + 3..].iter().position(|c| *c == ')')
        {
            for s in &mut styles[i..=i + close + 1] {
                *s = Style::new().fg(Color::Cyan);
            }
            for s in &mut styles[i + close + 2..=i + close + 3 + end] {
                *s = dim;
            }
            i += close + end + 4;
        } else {
            i += 1;
        }
    }
    styles
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
    fn lists_continue_on_enter_and_end_on_an_empty_item() {
        let mut e = Editor::new("- [x] done\n1. first");
        e.handle_key(key(KeyCode::End));
        e.handle_key(key(KeyCode::Enter));
        e.handle_key(key(KeyCode::Char('n')));
        assert_eq!(e.text(), "- [x] done\n- [ ] n\n1. first\n");
        e.handle_key(ctrl_key(KeyCode::End));
        e.handle_key(key(KeyCode::Enter));
        assert_eq!(e.text(), "- [x] done\n- [ ] n\n1. first\n2. \n");
        e.handle_key(key(KeyCode::Enter));
        assert_eq!(
            e.text(),
            "- [x] done\n- [ ] n\n1. first\n\n",
            "an empty item ends the list"
        );
    }

    #[test]
    fn find_and_replace() {
        let mut e = Editor::new("Foo bar foo\nfoo");
        e.handle_key(ctrl('f'));
        for c in "foo".chars() {
            e.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(
            e.selection(),
            Some((Pos::default(), Pos { line: 0, col: 3 }))
        );
        e.handle_key(key(KeyCode::Enter));
        assert_eq!(
            e.selection(),
            Some((Pos { line: 0, col: 8 }, Pos { line: 0, col: 11 }))
        );
        e.handle_key(ctrl('r'));
        for c in "baz".chars() {
            e.handle_key(key(KeyCode::Char(c)));
        }
        e.handle_key(key(KeyCode::Enter));
        assert_eq!(e.text(), "Foo bar baz\nfoo\n");
        e.handle_key(ctrl('a'));
        assert_eq!(e.text(), "baz bar baz\nbaz\n");
        e.handle_key(key(KeyCode::Esc));
        assert!(e.find.is_none());
        assert_eq!(list_prefix("  - [ ] x"), ("  ".into(), "- [ ] ".into()));
        assert_eq!(next_list_marker("9. "), "10. ");
        assert_eq!(
            fenced_lines(&[
                "a".into(),
                "```".into(),
                "b".into(),
                "```".into(),
                "c".into()
            ]),
            vec![false, true, true, true, false]
        );
    }

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
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
        assert_eq!(e.text(), "  - item\n  - x\n", "the list continues");

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
