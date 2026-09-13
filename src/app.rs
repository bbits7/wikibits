//! Application state: which page is open, what each column shows, and how input changes it.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Rect, Size};
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::sliced::SlicedProtocol;

use crate::markdown::{self, Rendered};
use crate::wiki::{LinkTarget, TreeNode, Wiki};

/// Images taller than this are scaled down to fit.
const MAX_IMAGE_ROWS: u16 = 30;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Tree,
    Content,
    Related,
}

pub struct TreeRow {
    pub depth: usize,
    pub label: String,
    pub kind: TreeKind,
}

pub enum TreeKind {
    Folder { path: String, expanded: bool },
    Page { id: String },
}

pub enum RelatedRow {
    Header(&'static str),
    Page(String),
    Missing(String),
    External(String),
    None,
}

impl RelatedRow {
    pub fn selectable(&self) -> bool {
        !matches!(self, RelatedRow::Header(_) | RelatedRow::None)
    }
}

/// Screen areas from the last draw, for mouse handling.
#[derive(Default, Clone, Copy)]
pub struct Rects {
    pub tree: Rect,
    pub crumbs: Rect,
    pub content: Rect,
    pub related: Rect,
}

pub struct Crumb {
    pub label: String,
    /// Folder path, or `None` for the current page.
    pub folder: Option<String>,
}

pub struct App {
    pub wiki: Wiki,
    pub tree: Vec<TreeRow>,
    pub expanded: HashSet<String>,
    pub tree_sel: usize,
    pub tree_scroll: usize,

    pub current: Option<String>,
    history: Vec<String>,
    future: Vec<String>,

    pub rendered: Option<Rendered>,
    render_width: u16,
    pub scroll: usize,
    pub link_sel: Option<usize>,
    pub content_height: usize,

    pub related: Vec<RelatedRow>,
    pub related_sel: usize,
    pub related_scroll: usize,

    pub focus: Focus,
    picker: Option<Picker>,
    images: HashMap<(PathBuf, u16), Option<Rc<SlicedProtocol>>>,

    pub status: String,
    pub show_help: bool,
    pub should_quit: bool,
    pub rects: Rects,
    /// Breadcrumbs of the current page, with their column ranges from the last draw.
    pub crumbs: Vec<Crumb>,
    pub crumb_columns: Vec<(u16, u16)>,
}

impl App {
    pub fn new(wiki: Wiki, picker: Option<Picker>, start: Option<String>) -> App {
        let mut app = App {
            wiki,
            tree: Vec::new(),
            expanded: HashSet::new(),
            tree_sel: 0,
            tree_scroll: 0,
            current: None,
            history: Vec::new(),
            future: Vec::new(),
            rendered: None,
            render_width: 0,
            scroll: 0,
            link_sel: None,
            content_height: 0,
            related: Vec::new(),
            related_sel: 0,
            related_scroll: 0,
            focus: Focus::Content,
            picker,
            images: HashMap::new(),
            status: String::new(),
            show_help: false,
            should_quit: false,
            rects: Rects::default(),
            crumbs: Vec::new(),
            crumb_columns: Vec::new(),
        };
        app.expand_all();
        app.rebuild_tree();
        let first = start
            .and_then(|p| match app.wiki.resolve("", &p) {
                LinkTarget::Page(id) => Some(id),
                _ => {
                    app.status = format!("No page named '{p}'");
                    None
                }
            })
            .or_else(|| app.wiki.landing_page());
        match first {
            Some(id) => app.open(&id, false),
            None => app.status = format!("No .md files in {}", app.wiki.root.display()),
        }
        app
    }

    pub fn page_title(&self) -> String {
        self.current
            .as_deref()
            .map(|id| self.wiki.title(id))
            .unwrap_or_else(|| "wikiBits".to_string())
    }

    // ----- pages -------------------------------------------------------------------------

    pub fn open(&mut self, id: &str, record: bool) {
        if record {
            if let Some(prev) = self.current.take()
                && prev != id
            {
                self.history.push(prev);
            }
            self.future.clear();
        }
        self.current = Some(id.to_string());
        self.scroll = 0;
        self.link_sel = None;
        self.related_sel = 0;
        self.related_scroll = 0;
        self.status.clear();
        self.render_current();
        self.rebuild_related();
        self.rebuild_crumbs();
        self.reveal_in_tree(id);
    }

    pub fn back(&mut self) {
        if let Some(prev) = self.history.pop() {
            if let Some(cur) = self.current.take() {
                self.future.push(cur);
            }
            self.open(&prev, false);
        } else {
            self.status = "Nothing to go back to".into();
        }
    }

    pub fn forward(&mut self) {
        if let Some(next) = self.future.pop() {
            if let Some(cur) = self.current.take() {
                self.history.push(cur);
            }
            self.open(&next, false);
        } else {
            self.status = "Nothing to go forward to".into();
        }
    }

    pub fn follow(&mut self, target: &LinkTarget) {
        match target {
            LinkTarget::Page(id) => {
                let id = id.clone();
                self.open(&id, true);
            }
            LinkTarget::Missing(raw) => {
                self.status = format!("No page '{raw}' (yet)");
            }
            LinkTarget::External(url) => {
                let ok = Command::new("xdg-open")
                    .arg(url)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .is_ok();
                self.status = if ok {
                    format!("Opened {url}")
                } else {
                    format!("Could not open {url}")
                };
            }
        }
    }

    /// Reread the wiki folder after files changed on disk.
    pub fn reload(&mut self) {
        if let Err(err) = self.wiki.reload() {
            self.status = format!("Reload failed: {err}");
            return;
        }
        self.images.clear();
        self.rebuild_tree();
        match self.current.clone() {
            Some(id) if self.wiki.pages.contains_key(&id) => {
                let scroll = self.scroll;
                self.render_current();
                self.scroll = scroll.min(self.max_scroll());
                self.link_sel = None;
                self.rebuild_related();
                self.rebuild_crumbs();
                self.reveal_in_tree(&id);
            }
            _ => match self.wiki.landing_page() {
                Some(id) => self.open(&id, false),
                None => {
                    self.current = None;
                    self.rendered = None;
                    self.related.clear();
                    self.crumbs.clear();
                }
            },
        }
    }

    /// Re-render for a new content width. Called from the draw code.
    pub fn set_width(&mut self, width: u16) {
        if width != self.render_width {
            self.render_width = width;
            let scroll = self.scroll;
            self.render_current();
            self.scroll = scroll.min(self.max_scroll());
            self.link_sel = None;
        }
    }

    fn render_current(&mut self) {
        let Some(id) = self.current.clone() else {
            self.rendered = None;
            return;
        };
        if self.render_width == 0 {
            return;
        }
        let Some(page) = self.wiki.pages.get(&id) else {
            return;
        };
        let text = fs::read_to_string(&page.path).unwrap_or_default();
        let page_dir = page
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let mut ctx = RenderCtx {
            wiki: &self.wiki,
            page: &id,
            page_dir,
            picker: self.picker.as_ref(),
            images: &mut self.images,
        };
        self.rendered = Some(markdown::render(&text, self.render_width, &mut ctx));
    }

    pub fn image(&self, path: &Path, max_width: u16) -> Option<Rc<SlicedProtocol>> {
        self.images
            .get(&(path.to_path_buf(), max_width))
            .cloned()
            .flatten()
    }

    // ----- scrolling and links --------------------------------------------------------

    pub fn max_scroll(&self) -> usize {
        let lines = self.rendered.as_ref().map(|r| r.lines.len()).unwrap_or(0);
        lines.saturating_sub(self.content_height)
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.max_scroll() as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, max) as usize;
    }

    fn link_line(&self, index: usize) -> Option<usize> {
        self.rendered
            .as_ref()?
            .links
            .get(index)?
            .spans
            .first()
            .map(|(line, _)| *line)
    }

    fn visible_links(&self) -> Vec<usize> {
        let Some(rendered) = &self.rendered else {
            return Vec::new();
        };
        let range = self.scroll..self.scroll + self.content_height;
        rendered
            .links
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.spans
                    .first()
                    .is_some_and(|(line, _)| range.contains(line))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn next_link(&mut self) {
        let count = self.rendered.as_ref().map(|r| r.links.len()).unwrap_or(0);
        if count == 0 {
            self.status = "No links on this page".into();
            return;
        }
        let next = match self.link_sel {
            Some(i) => (i + 1) % count,
            None => self.visible_links().first().copied().unwrap_or(0),
        };
        self.select_link(next);
    }

    pub fn prev_link(&mut self) {
        let count = self.rendered.as_ref().map(|r| r.links.len()).unwrap_or(0);
        if count == 0 {
            self.status = "No links on this page".into();
            return;
        }
        let prev = match self.link_sel {
            Some(0) => count - 1,
            Some(i) => i - 1,
            None => self.visible_links().last().copied().unwrap_or(count - 1),
        };
        self.select_link(prev);
    }

    fn select_link(&mut self, index: usize) {
        self.link_sel = Some(index);
        if let Some(line) = self.link_line(index)
            && (line < self.scroll || line >= self.scroll + self.content_height)
        {
            let target = line.saturating_sub(self.content_height / 3);
            self.scroll = target.min(self.max_scroll());
        }
    }

    pub fn follow_selected_link(&mut self) {
        let target = self
            .link_sel
            .and_then(|i| self.rendered.as_ref()?.links.get(i))
            .map(|l| l.target.clone());
        match target {
            Some(t) => self.follow(&t),
            None => self.status = "Select a link first (n / p)".into(),
        }
    }

    /// The link under a content cell, if any.
    fn link_at(&self, line: usize, col: u16) -> Option<usize> {
        let rendered = self.rendered.as_ref()?;
        let text_line = rendered.lines.get(line)?;
        let mut acc = 0u16;
        let span_index = text_line.spans.iter().position(|s| {
            let w = s.width() as u16;
            let hit = col >= acc && col < acc + w;
            acc += w;
            hit
        })?;
        rendered
            .links
            .iter()
            .position(|l| l.spans.contains(&(line, span_index)))
    }

    // ----- tree ------------------------------------------------------------------------

    fn expand_all(&mut self) {
        fn walk(nodes: &[TreeNode], out: &mut HashSet<String>) {
            for node in nodes {
                if let TreeNode::Folder { path, children, .. } = node {
                    out.insert(path.clone());
                    walk(children, out);
                }
            }
        }
        walk(&self.wiki.tree(), &mut self.expanded);
    }

    fn rebuild_tree(&mut self) {
        fn walk(
            nodes: &[TreeNode],
            depth: usize,
            expanded: &HashSet<String>,
            out: &mut Vec<TreeRow>,
        ) {
            for node in nodes {
                match node {
                    TreeNode::Folder {
                        name,
                        path,
                        children,
                    } => {
                        let open = expanded.contains(path);
                        out.push(TreeRow {
                            depth,
                            label: name.clone(),
                            kind: TreeKind::Folder {
                                path: path.clone(),
                                expanded: open,
                            },
                        });
                        if open {
                            walk(children, depth + 1, expanded, out);
                        }
                    }
                    TreeNode::Page { id, title } => out.push(TreeRow {
                        depth,
                        label: title.clone(),
                        kind: TreeKind::Page { id: id.clone() },
                    }),
                }
            }
        }
        let mut rows = Vec::new();
        walk(&self.wiki.tree(), 0, &self.expanded, &mut rows);
        self.tree = rows;
        self.tree_sel = self.tree_sel.min(self.tree.len().saturating_sub(1));
    }

    /// Expand the folders above `id` and put the tree cursor on it.
    fn reveal_in_tree(&mut self, id: &str) {
        let mut folder = String::new();
        for part in id.split('/').take(id.matches('/').count()) {
            if !folder.is_empty() {
                folder.push('/');
            }
            folder.push_str(part);
            self.expanded.insert(folder.clone());
        }
        self.rebuild_tree();
        if let Some(i) = self
            .tree
            .iter()
            .position(|r| matches!(&r.kind, TreeKind::Page { id: p } if p == id))
        {
            self.tree_sel = i;
        }
    }

    pub fn reveal_folder(&mut self, path: &str) {
        let mut folder = String::new();
        for part in path.split('/') {
            if !folder.is_empty() {
                folder.push('/');
            }
            folder.push_str(part);
            self.expanded.insert(folder.clone());
        }
        self.rebuild_tree();
        if let Some(i) = self
            .tree
            .iter()
            .position(|r| matches!(&r.kind, TreeKind::Folder { path: p, .. } if p == path))
        {
            self.tree_sel = i;
        }
        self.focus = Focus::Tree;
    }

    fn tree_move(&mut self, delta: isize) {
        if self.tree.is_empty() {
            return;
        }
        let max = self.tree.len() as isize - 1;
        self.tree_sel = (self.tree_sel as isize + delta).clamp(0, max) as usize;
    }

    fn tree_toggle(&mut self) {
        if let Some(TreeRow {
            kind: TreeKind::Folder { path, .. },
            ..
        }) = self.tree.get(self.tree_sel)
        {
            let path = path.clone();
            if !self.expanded.remove(&path) {
                self.expanded.insert(path);
            }
            self.rebuild_tree();
        }
    }

    fn tree_activate(&mut self) {
        match self.tree.get(self.tree_sel).map(|r| &r.kind) {
            Some(TreeKind::Page { id }) => {
                let id = id.clone();
                self.open(&id, true);
            }
            Some(TreeKind::Folder { .. }) => self.tree_toggle(),
            None => {}
        }
    }

    fn tree_expand(&mut self) {
        match self.tree.get(self.tree_sel).map(|r| &r.kind) {
            Some(TreeKind::Folder {
                path,
                expanded: false,
            }) => {
                self.expanded.insert(path.clone());
                self.rebuild_tree();
            }
            Some(TreeKind::Folder { .. }) => self.tree_move(1),
            Some(TreeKind::Page { .. }) => self.tree_activate(),
            None => {}
        }
    }

    /// Collapse the folder under the cursor, or jump to the parent folder.
    fn tree_collapse(&mut self) {
        let Some(row) = self.tree.get(self.tree_sel) else {
            return;
        };
        if let TreeKind::Folder {
            path,
            expanded: true,
        } = &row.kind
        {
            self.expanded.remove(path);
            self.rebuild_tree();
            return;
        }
        let depth = row.depth;
        if depth == 0 {
            return;
        }
        if let Some(parent) = (0..self.tree_sel)
            .rev()
            .find(|&i| self.tree[i].depth < depth)
        {
            self.tree_sel = parent;
        }
    }

    // ----- related column and breadcrumbs ---------------------------------------------

    fn rebuild_related(&mut self) {
        let mut rows = vec![RelatedRow::Header("Backlinks")];
        let Some(id) = self.current.as_deref() else {
            self.related = rows;
            return;
        };
        let backlinks = self.wiki.backlinks_of(id);
        if backlinks.is_empty() {
            rows.push(RelatedRow::None);
        }
        rows.extend(backlinks.iter().map(|b| RelatedRow::Page(b.clone())));

        rows.push(RelatedRow::Header("Links on this page"));
        let mut seen: Vec<LinkTarget> = Vec::new();
        if let Some(page) = self.wiki.pages.get(id) {
            for raw in &page.links {
                let target = self.wiki.resolve(id, raw);
                if seen.contains(&target) || matches!(&target, LinkTarget::Page(p) if p == id) {
                    continue;
                }
                rows.push(match &target {
                    LinkTarget::Page(p) => RelatedRow::Page(p.clone()),
                    LinkTarget::Missing(r) => RelatedRow::Missing(r.clone()),
                    LinkTarget::External(u) => RelatedRow::External(u.clone()),
                });
                seen.push(target);
            }
        }
        if seen.is_empty() {
            rows.push(RelatedRow::None);
        }
        self.related = rows;
        self.related_sel = self
            .related
            .iter()
            .position(RelatedRow::selectable)
            .unwrap_or(0);
    }

    fn related_move(&mut self, delta: isize) {
        let mut i = self.related_sel as isize;
        loop {
            i += delta;
            if i < 0 || i >= self.related.len() as isize {
                return;
            }
            if self.related[i as usize].selectable() {
                self.related_sel = i as usize;
                return;
            }
        }
    }

    fn related_activate(&mut self) {
        let target = match self.related.get(self.related_sel) {
            Some(RelatedRow::Page(id)) => LinkTarget::Page(id.clone()),
            Some(RelatedRow::Missing(raw)) => LinkTarget::Missing(raw.clone()),
            Some(RelatedRow::External(url)) => LinkTarget::External(url.clone()),
            _ => return,
        };
        self.follow(&target);
    }

    fn rebuild_crumbs(&mut self) {
        let root_name = self
            .wiki
            .root
            .file_name()
            .map(|n| crate::wiki::prettify(&n.to_string_lossy()))
            .unwrap_or_else(|| "Wiki".into());
        let mut crumbs = vec![Crumb {
            label: root_name,
            folder: Some(String::new()),
        }];
        if let Some(id) = &self.current {
            let mut folder = String::new();
            let parts: Vec<&str> = id.split('/').collect();
            for part in &parts[..parts.len() - 1] {
                if !folder.is_empty() {
                    folder.push('/');
                }
                folder.push_str(part);
                crumbs.push(Crumb {
                    label: crate::wiki::prettify(part),
                    folder: Some(folder.clone()),
                });
            }
            crumbs.push(Crumb {
                label: self.wiki.title(id),
                folder: None,
            });
        }
        self.crumbs = crumbs;
    }

    fn crumb_activate(&mut self, index: usize) {
        match self.crumbs.get(index).and_then(|c| c.folder.clone()) {
            Some(folder) if folder.is_empty() => {
                if let Some(home) = self.wiki.landing_page() {
                    self.open(&home, true);
                }
            }
            Some(folder) => self.reveal_folder(&folder),
            None => {}
        }
    }

    // ----- input -----------------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.show_help {
            self.show_help = false;
            return;
        }
        self.status.clear();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if ctrl => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Tab => self.cycle_focus(1),
            KeyCode::BackTab => self.cycle_focus(-1),
            KeyCode::Char('1') => self.focus = Focus::Tree,
            KeyCode::Char('2') => self.focus = Focus::Content,
            KeyCode::Char('3') => self.focus = Focus::Related,
            KeyCode::Char('b') | KeyCode::Backspace => self.back(),
            KeyCode::Char('f') => self.forward(),
            KeyCode::Char('r') => {
                self.reload();
                self.status = "Reloaded".into();
            }
            KeyCode::Char('H') => {
                if let Some(home) = self.wiki.landing_page() {
                    self.open(&home, true);
                }
            }
            _ => match self.focus {
                Focus::Tree => self.tree_key(key),
                Focus::Content => self.content_key(key, ctrl),
                Focus::Related => self.related_key(key),
            },
        }
    }

    fn cycle_focus(&mut self, delta: isize) {
        const ORDER: [Focus; 3] = [Focus::Tree, Focus::Content, Focus::Related];
        let i = ORDER.iter().position(|f| *f == self.focus).unwrap_or(1) as isize;
        self.focus = ORDER[(i + delta).rem_euclid(3) as usize];
    }

    fn tree_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.tree_move(1),
            KeyCode::Char('k') | KeyCode::Up => self.tree_move(-1),
            KeyCode::Char('g') | KeyCode::Home => self.tree_sel = 0,
            KeyCode::Char('G') | KeyCode::End => self.tree_sel = self.tree.len().saturating_sub(1),
            KeyCode::PageDown => self.tree_move(10),
            KeyCode::PageUp => self.tree_move(-10),
            KeyCode::Enter => self.tree_activate(),
            KeyCode::Char(' ') => self.tree_toggle(),
            KeyCode::Char('l') | KeyCode::Right => self.tree_expand(),
            KeyCode::Char('h') | KeyCode::Left => self.tree_collapse(),
            _ => {}
        }
    }

    fn content_key(&mut self, key: KeyEvent, ctrl: bool) {
        let page = self.content_height.max(1) as isize;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_by(-1),
            KeyCode::Char('d') if ctrl => self.scroll_by(page / 2),
            KeyCode::Char('u') if ctrl => self.scroll_by(-page / 2),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_by(page - 1),
            KeyCode::PageUp => self.scroll_by(-(page - 1)),
            KeyCode::Char('g') | KeyCode::Home => self.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => self.scroll = self.max_scroll(),
            KeyCode::Char('n') => self.next_link(),
            KeyCode::Char('p') => self.prev_link(),
            KeyCode::Enter => self.follow_selected_link(),
            KeyCode::Esc => self.link_sel = None,
            _ => {}
        }
    }

    fn related_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.related_move(1),
            KeyCode::Char('k') | KeyCode::Up => self.related_move(-1),
            KeyCode::Enter => self.related_activate(),
            _ => {}
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.show_help {
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                self.show_help = false;
            }
            return;
        }
        let (x, y) = (mouse.column, mouse.row);
        let rects = self.rects;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if rects.tree.contains((x, y).into()) {
                    self.focus = Focus::Tree;
                    let row = (y - rects.tree.y) as usize + self.tree_scroll;
                    if row < self.tree.len() {
                        self.tree_sel = row;
                        self.tree_activate();
                    }
                } else if rects.crumbs.contains((x, y).into()) {
                    let col = x - rects.crumbs.x;
                    if let Some(i) = self
                        .crumb_columns
                        .iter()
                        .position(|(s, e)| col >= *s && col < *e)
                    {
                        self.crumb_activate(i);
                    }
                } else if rects.content.contains((x, y).into()) {
                    self.focus = Focus::Content;
                    let line = (y - rects.content.y) as usize + self.scroll;
                    let col = x - rects.content.x;
                    if let Some(i) = self.link_at(line, col) {
                        self.link_sel = Some(i);
                        self.follow_selected_link();
                    }
                } else if rects.related.contains((x, y).into()) {
                    self.focus = Focus::Related;
                    let row = (y - rects.related.y) as usize + self.related_scroll;
                    if self.related.get(row).is_some_and(RelatedRow::selectable) {
                        self.related_sel = row;
                        self.related_activate();
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) => self.back(),
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let delta = if mouse.kind == MouseEventKind::ScrollDown {
                    1
                } else {
                    -1
                };
                if rects.tree.contains((x, y).into()) {
                    self.tree_move(delta);
                } else if rects.related.contains((x, y).into()) {
                    self.related_move(delta);
                } else {
                    self.scroll_by(delta * 3);
                }
            }
            _ => {}
        }
    }
}

/// What the Markdown renderer needs while rendering one page.
struct RenderCtx<'a> {
    wiki: &'a Wiki,
    page: &'a str,
    page_dir: PathBuf,
    picker: Option<&'a Picker>,
    images: &'a mut HashMap<(PathBuf, u16), Option<Rc<SlicedProtocol>>>,
}

impl markdown::Context for RenderCtx<'_> {
    fn resolve(&self, raw: &str) -> LinkTarget {
        self.wiki.resolve(self.page, raw)
    }

    fn title(&self, id: &str) -> String {
        self.wiki.title(id)
    }

    fn image_path(&self, raw: &str) -> Option<PathBuf> {
        if raw.contains("://") {
            return None;
        }
        let raw = raw.split('#').next().unwrap_or(raw).trim();
        let raw = raw.trim_start_matches('/');
        [self.page_dir.join(raw), self.wiki.root.join(raw)]
            .into_iter()
            .find(|p| p.is_file())
    }

    fn image_size(&mut self, path: &Path, max_width: u16) -> Option<(u16, u16)> {
        let key = (path.to_path_buf(), max_width);
        if !self.images.contains_key(&key) {
            let proto = self.picker.and_then(|picker| {
                let img = image::ImageReader::open(path)
                    .ok()?
                    .with_guessed_format()
                    .ok()?
                    .decode()
                    .ok()?;
                let bound = Size::new(max_width, MAX_IMAGE_ROWS);
                SlicedProtocol::new_with_resize(picker, img, bound, Resize::Fit(None))
                    .ok()
                    .map(Rc::new)
            });
            self.images.insert(key.clone(), proto);
        }
        let proto = self.images.get(&key)?.as_ref()?;
        let size = proto.size();
        Some((size.width, size.height))
    }
}
