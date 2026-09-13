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
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::sliced::SlicedProtocol;
use ratatui_image::{FontSize, Resize};

use crate::editor::{Action, Editor, Pos};
use crate::markdown::{self, ImageSlot, Item, Rendered};
use crate::wiki::{self, LinkTarget, SearchHit, TreeNode, Wiki};

/// What the status bar is asking for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prompt {
    None,
    /// `/`: find text in the current page.
    FindPage,
    /// `s`: search every page of the wiki.
    SearchWiki,
    /// A link to a missing page was followed: create it? (`pending_create` holds the id).
    CreatePage,
    /// `N`: type the path of a page to create.
    NewPage,
    /// `D`: delete the current page? (y / n)
    DeletePage,
    /// `R`: type the new path for the current page.
    RenamePage,
}

/// A match of the find-in-page query: line and column range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FindMatch {
    pub line: usize,
    pub start: u16,
    pub end: u16,
}

pub struct Find {
    pub query: String,
    pub matches: Vec<FindMatch>,
    pub current: usize,
}

/// The image picker (Ctrl-P in the editor): images under the wiki root, filtered by typing.
pub struct ImagePick {
    pub query: String,
    /// Image paths relative to the wiki root.
    pub all: Vec<String>,
    pub hits: Vec<String>,
    pub sel: usize,
}

/// Image file extensions the page renderer can show.
const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// Page suggestions for a `[[` being typed in the editor.
pub struct Complete {
    /// Position right after the `[[`.
    pub start: Pos,
    pub hits: Vec<(String, String)>,
    pub sel: usize,
}

pub struct Search {
    pub query: String,
    pub hits: Vec<SearchHit>,
    pub sel: usize,
    pub scroll: usize,
    /// List area from the last draw, for mouse clicks.
    pub view: Rect,
}

/// `(view size, scroll offset)` a pop-up image was encoded for.
type PopupKey = ((u16, u16), (u16, u16));

/// An image opened at full size over the page, scrolled in cells.
pub struct ImagePopup {
    pub name: String,
    image: image::DynamicImage,
    pub pixels: (u32, u32),
    /// The whole image in cells at the current font size.
    pub cells: (u16, u16),
    pub off: (u16, u16),
    /// Inner area from the last draw.
    pub view: Rect,
    /// Mouse position and offset when a drag started.
    drag: Option<((u16, u16), (u16, u16))>,
    /// Encoded for `(view size, offset)`.
    protocol: Option<(PopupKey, Protocol)>,
}

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
    Folder {
        path: String,
        expanded: bool,
        /// The folder's `index` page, opened by activating the folder.
        index: Option<String>,
    },
    Page {
        id: String,
    },
}

pub enum RelatedRow {
    Header(&'static str),
    Page(String),
    Missing(String),
    External(String),
    /// A heading of the current page (index into `rendered.headings`).
    Heading {
        index: usize,
        depth: usize,
        foldable: bool,
        expanded: bool,
    },
    None,
    Blank,
}

impl RelatedRow {
    pub fn selectable(&self) -> bool {
        !matches!(
            self,
            RelatedRow::Header(_) | RelatedRow::None | RelatedRow::Blank
        )
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
    render_height: u16,
    pub scroll: usize,
    /// Selected entry of `rendered.items`.
    pub sel: Option<usize>,
    pub content_height: usize,
    pub popup: Option<ImagePopup>,

    pub related: Vec<RelatedRow>,
    pub related_sel: usize,
    pub related_scroll: usize,
    /// Headings whose sub-headings are shown in "On this page".
    toc_expanded: HashSet<usize>,
    /// Reset `toc_expanded` to the defaults on the next rebuild (a new page was opened).
    toc_fresh: bool,

    pub focus: Focus,
    picker: Option<Picker>,
    images: HashMap<(PathBuf, u16, u16), Option<Rc<SlicedProtocol>>>,

    pub status: String,
    pub show_help: bool,
    /// Show the page's Markdown source instead of rendering it.
    pub raw: bool,
    pub should_quit: bool,
    pub rects: Rects,
    /// Breadcrumbs of the current page, with their column ranges from the last draw.
    pub crumbs: Vec<Crumb>,
    pub crumb_columns: Vec<(u16, u16)>,
    pub prompt: Prompt,
    pub find: Option<Find>,
    pub search: Option<Search>,
    pub editor: Option<Editor>,
    pub complete: Option<Complete>,
    pub image_pick: Option<ImagePick>,
    /// `[[` context the user dismissed with Esc; do not reopen until it changes.
    complete_dismissed: Option<Pos>,
    /// Page id waiting for the create-page confirmation.
    pub pending_create: Option<String>,
    /// Path typed at the new-page or rename prompt.
    pub new_page: String,
    /// Clear the terminal before the next draw (an image may be left on screen).
    pub repaint: bool,
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
            render_height: 0,
            scroll: 0,
            sel: None,
            content_height: 0,
            popup: None,
            related: Vec::new(),
            related_sel: 0,
            related_scroll: 0,
            toc_expanded: HashSet::new(),
            toc_fresh: true,
            focus: Focus::Content,
            picker,
            images: HashMap::new(),
            status: String::new(),
            show_help: false,
            raw: false,
            should_quit: false,
            rects: Rects::default(),
            crumbs: Vec::new(),
            crumb_columns: Vec::new(),
            prompt: Prompt::None,
            find: None,
            search: None,
            editor: None,
            complete: None,
            image_pick: None,
            complete_dismissed: None,
            pending_create: None,
            new_page: String::new(),
            repaint: false,
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

    /// Switch the page between rendered Markdown and its source, keeping the scroll position.
    pub fn toggle_raw(&mut self) {
        self.raw = !self.raw;
        let scroll = self.scroll;
        self.render_current();
        self.scroll = scroll.min(self.max_scroll());
        self.sel = None;
        self.rebuild_related();
        self.status = if self.raw {
            "Showing the Markdown source (v to render)".into()
        } else {
            "Showing the rendered page (v for source)".into()
        };
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
        self.sel = None;
        self.related_sel = 0;
        self.related_scroll = 0;
        self.toc_fresh = true;
        if self.prompt == Prompt::FindPage {
            self.prompt = Prompt::None;
        }
        self.find = None;
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
            LinkTarget::Missing(raw) => match self.new_page_id(raw) {
                Some(id) => {
                    self.pending_create = Some(id);
                    self.prompt = Prompt::CreatePage;
                }
                None => self.status = format!("No page '{raw}', and that is not a page path"),
            },
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
                self.sel = None;
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

    /// Re-render for a new page area. Called from the draw code.
    pub fn set_size(&mut self, width: u16, height: u16) {
        if (width, height) != (self.render_width, self.render_height) {
            self.render_width = width;
            self.render_height = height;
            let scroll = self.scroll;
            self.render_current();
            self.scroll = scroll.min(self.max_scroll());
            self.sel = None;
            self.rebuild_related();
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
        self.rendered = Some(if self.raw {
            markdown::render_raw(&text, self.render_width)
        } else {
            markdown::render(&text, self.render_width, self.render_height, &mut ctx)
        });
    }

    /// Pick up a changed cell size (the window was resized or moved to a monitor with another
    /// scale) so images are rebuilt for the real pixel grid.
    pub fn refresh_font_size(&mut self) {
        let Some(font_size) = font_size_from_window() else {
            return;
        };
        let Some(picker) = &self.picker else { return };
        let current = picker.font_size();
        if (current.width, current.height) == (font_size.width, font_size.height) {
            return;
        }
        self.picker = Some(picker_with_font_size(picker, font_size));
        self.images.clear();
        let scroll = self.scroll;
        self.render_current();
        self.scroll = scroll.min(self.max_scroll());
        self.rebuild_related();
    }

    pub fn image(&self, slot: &ImageSlot) -> Option<Rc<SlicedProtocol>> {
        self.images
            .get(&(slot.path.clone(), slot.max_width, slot.max_height))
            .cloned()
            .flatten()
    }

    // ----- image pop-up ----------------------------------------------------------------

    fn open_image(&mut self, index: usize) {
        let Some(slot) = self.rendered.as_ref().and_then(|r| r.images.get(index)) else {
            return;
        };
        let path = slot.path.clone();
        let decoded = image::ImageReader::open(&path)
            .ok()
            .and_then(|r| r.with_guessed_format().ok())
            .and_then(|r| r.decode().ok());
        let (Some(image), Some(picker)) = (decoded, &self.picker) else {
            self.status = format!("Cannot open {}", path.display());
            return;
        };
        let font = picker.font_size();
        let pixels = (image.width(), image.height());
        let cells = (
            pixels
                .0
                .div_ceil(font.width.max(1) as u32)
                .min(u16::MAX as u32) as u16,
            pixels
                .1
                .div_ceil(font.height.max(1) as u32)
                .min(u16::MAX as u32) as u16,
        );
        self.popup = Some(ImagePopup {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            image,
            pixels,
            cells,
            off: (0, 0),
            view: Rect::default(),
            drag: None,
            protocol: None,
        });
    }

    /// The visible part of the pop-up image, encoded for `view`. Re-encodes only when the view
    /// size or the scroll offset changed.
    pub fn popup_protocol(&mut self, view: Rect) -> Option<&Protocol> {
        let picker = self.picker.as_ref()?;
        let font = picker.font_size();
        let popup = self.popup.as_mut()?;
        popup.view = view;
        let key = ((view.width, view.height), popup.off);
        if popup.protocol.as_ref().is_none_or(|(k, _)| *k != key) {
            let (fw, fh) = (font.width.max(1) as u32, font.height.max(1) as u32);
            let (iw, ih) = popup.pixels;
            let px = (popup.off.0 as u32 * fw).min(iw.saturating_sub(1));
            let py = (popup.off.1 as u32 * fh).min(ih.saturating_sub(1));
            let cw = (view.width as u32 * fw).min(iw - px);
            let ch = (view.height as u32 * fh).min(ih - py);
            if cw == 0 || ch == 0 {
                return None;
            }
            let cropped = popup.image.crop_imm(px, py, cw, ch);
            let cells = Size::new(cw.div_ceil(fw) as u16, ch.div_ceil(fh) as u16);
            // Crop, never scale: one image pixel is one screen pixel in the pop-up.
            let proto = picker
                .new_protocol(cropped, cells, Resize::Crop(None))
                .ok()?;
            popup.protocol = Some((key, proto));
        }
        popup.protocol.as_ref().map(|(_, p)| p)
    }

    fn popup_scroll(&mut self, dx: i32, dy: i32) {
        let Some(popup) = &mut self.popup else { return };
        let max_x = popup.cells.0.saturating_sub(popup.view.width);
        let max_y = popup.cells.1.saturating_sub(popup.view.height);
        popup.off.0 = (popup.off.0 as i32 + dx).clamp(0, max_x as i32) as u16;
        popup.off.1 = (popup.off.1 as i32 + dy).clamp(0, max_y as i32) as u16;
    }

    fn popup_key(&mut self, key: KeyEvent) {
        let Some(popup) = &self.popup else { return };
        let page = popup.view.height.max(1) as i32;
        let far = i32::MAX / 4;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => self.popup = None,
            KeyCode::Char('h') | KeyCode::Left => self.popup_scroll(-4, 0),
            KeyCode::Char('l') | KeyCode::Right => self.popup_scroll(4, 0),
            KeyCode::Char('j') | KeyCode::Down => self.popup_scroll(0, 2),
            KeyCode::Char('k') | KeyCode::Up => self.popup_scroll(0, -2),
            KeyCode::PageDown | KeyCode::Char(' ') => self.popup_scroll(0, page),
            KeyCode::PageUp => self.popup_scroll(0, -page),
            KeyCode::Char('g') | KeyCode::Home => self.popup_scroll(-far, -far),
            KeyCode::Char('G') | KeyCode::End => self.popup_scroll(0, far),
            _ => {}
        }
    }

    fn popup_mouse(&mut self, mouse: MouseEvent) {
        let Some(popup) = &self.popup else { return };
        let outer = Rect {
            x: popup.view.x.saturating_sub(1),
            y: popup.view.y.saturating_sub(1),
            width: popup.view.width + 2,
            height: popup.view.height + 2,
        };
        let at = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(_) if !outer.contains(at.into()) => self.popup = None,
            MouseEventKind::Down(MouseButton::Left) => {
                let off = popup.off;
                self.popup.as_mut().unwrap().drag = Some((at, off));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Dragging moves the image with the pointer, so the offset goes the other way.
                if let Some(((sx, sy), (ox, oy))) = popup.drag {
                    let dx = sx as i32 - at.0 as i32;
                    let dy = sy as i32 - at.1 as i32;
                    let (nx, ny) = (ox as i32 + dx, oy as i32 + dy);
                    let current = popup.off;
                    self.popup_scroll(nx - current.0 as i32, ny - current.1 as i32);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.popup.as_mut().unwrap().drag = None,
            MouseEventKind::ScrollDown => self.popup_scroll(0, 2),
            MouseEventKind::ScrollUp => self.popup_scroll(0, -2),
            MouseEventKind::ScrollRight => self.popup_scroll(4, 0),
            MouseEventKind::ScrollLeft => self.popup_scroll(-4, 0),
            _ => {}
        }
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

    /// First line of an item (link or image).
    fn item_line(&self, index: usize) -> Option<usize> {
        let rendered = self.rendered.as_ref()?;
        match *rendered.items.get(index)? {
            Item::Link(l) => rendered.links[l].spans.first().map(|(line, _)| *line),
            Item::Image(i) => Some(rendered.images[i].line),
        }
    }

    fn visible_items(&self) -> Vec<usize> {
        let range = self.scroll..self.scroll + self.content_height;
        let count = self.rendered.as_ref().map_or(0, |r| r.items.len());
        (0..count)
            .filter(|&i| self.item_line(i).is_some_and(|line| range.contains(&line)))
            .collect()
    }

    pub fn next_item(&mut self) {
        let count = self.rendered.as_ref().map_or(0, |r| r.items.len());
        if count == 0 {
            self.status = "No links or images on this page".into();
            return;
        }
        let next = match self.sel {
            Some(i) => (i + 1) % count,
            None => self.visible_items().first().copied().unwrap_or(0),
        };
        self.select_item(next);
    }

    pub fn prev_item(&mut self) {
        let count = self.rendered.as_ref().map_or(0, |r| r.items.len());
        if count == 0 {
            self.status = "No links or images on this page".into();
            return;
        }
        let prev = match self.sel {
            Some(0) => count - 1,
            Some(i) => i - 1,
            None => self.visible_items().last().copied().unwrap_or(count - 1),
        };
        self.select_item(prev);
    }

    fn select_item(&mut self, index: usize) {
        self.sel = Some(index);
        if let Some(line) = self.item_line(index)
            && (line < self.scroll || line >= self.scroll + self.content_height)
        {
            let target = line.saturating_sub(self.content_height / 3);
            self.scroll = target.min(self.max_scroll());
        }
    }

    /// Follow the selected link, or open the selected image.
    pub fn activate_selected(&mut self) {
        let item = self
            .sel
            .and_then(|i| self.rendered.as_ref()?.items.get(i).copied());
        match item {
            Some(Item::Link(l)) => {
                let target = self.rendered.as_ref().unwrap().links[l].target.clone();
                self.follow(&target);
            }
            Some(Item::Image(i)) => self.open_image(i),
            None => self.status = "Select a link or image first (n / p)".into(),
        }
    }

    /// The link or image under a content cell, if any.
    fn item_at(&self, line: usize, col: u16) -> Option<usize> {
        let rendered = self.rendered.as_ref()?;
        if let Some(i) = rendered.images.iter().position(|s| {
            line >= s.line
                && line <= s.line + s.height as usize + 1
                && col + 1 >= s.x
                && col <= s.x + s.width
        }) {
            return rendered.items.iter().position(|it| *it == Item::Image(i));
        }
        let text_line = rendered.lines.get(line)?;
        let mut acc = 0u16;
        let span_index = text_line.spans.iter().position(|s| {
            let w = s.width() as u16;
            let hit = col >= acc && col < acc + w;
            acc += w;
            hit
        })?;
        let link = rendered
            .links
            .iter()
            .position(|l| l.spans.contains(&(line, span_index)))?;
        rendered.items.iter().position(|it| *it == Item::Link(link))
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
        self.expanded.insert(String::new());
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
                        index,
                        children,
                    } => {
                        let open = expanded.contains(path);
                        out.push(TreeRow {
                            depth,
                            label: name.clone(),
                            kind: TreeKind::Folder {
                                path: path.clone(),
                                expanded: open,
                                index: index.clone(),
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
        // The root index page is the root of the tree; everything else hangs under it.
        let mut nodes = self.wiki.tree();
        let mut rows = Vec::new();
        let mut depth = 0;
        if let Some(home) = self.wiki.folder_index("") {
            nodes.retain(|n| !matches!(n, TreeNode::Page { id, .. } if *id == home));
            let open = self.expanded.contains("");
            rows.push(TreeRow {
                depth: 0,
                label: self.wiki.title(&home),
                kind: TreeKind::Folder {
                    path: String::new(),
                    expanded: open,
                    index: Some(home),
                },
            });
            if !open {
                nodes.clear();
            }
            depth = 1;
        }
        walk(&nodes, depth, &self.expanded, &mut rows);
        self.tree = rows;
        self.tree_sel = self.tree_sel.min(self.tree.len().saturating_sub(1));
    }

    /// Expand the folders above `id` and put the tree cursor on it.
    fn reveal_in_tree(&mut self, id: &str) {
        self.expanded.insert(String::new());
        let mut folder = String::new();
        for part in id.split('/').take(id.matches('/').count()) {
            if !folder.is_empty() {
                folder.push('/');
            }
            folder.push_str(part);
            self.expanded.insert(folder.clone());
        }
        self.rebuild_tree();
        if let Some(i) = self.tree.iter().position(|r| match &r.kind {
            TreeKind::Page { id: p } => p == id,
            TreeKind::Folder { index, .. } => index.as_deref() == Some(id),
        }) {
            self.tree_sel = i;
        }
    }

    pub fn reveal_folder(&mut self, path: &str) {
        self.expanded.insert(String::new());
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
            Some(TreeKind::Folder {
                index: Some(id),
                path,
                ..
            }) => {
                let (id, path) = (id.clone(), path.clone());
                self.expanded.insert(path);
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
                ..
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
            ..
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

    /// The "On this page" rows: headings as a tree, level-1 headings expanded and deeper ones
    /// collapsed by default.
    fn toc_rows(&mut self) -> Vec<RelatedRow> {
        let Some(rendered) = &self.rendered else {
            return Vec::new();
        };
        let headings = &rendered.headings;
        if self.toc_fresh {
            self.toc_fresh = false;
            self.toc_expanded = headings
                .iter()
                .enumerate()
                .filter(|(_, h)| h.level <= 1)
                .map(|(i, _)| i)
                .collect();
        }
        let mut rows = Vec::new();
        let mut ancestors: Vec<usize> = Vec::new();
        for (i, h) in headings.iter().enumerate() {
            while ancestors
                .last()
                .is_some_and(|&a| headings[a].level >= h.level)
            {
                ancestors.pop();
            }
            if ancestors.iter().all(|a| self.toc_expanded.contains(a)) {
                rows.push(RelatedRow::Heading {
                    index: i,
                    depth: ancestors.len(),
                    foldable: headings.get(i + 1).is_some_and(|n| n.level > h.level),
                    expanded: self.toc_expanded.contains(&i),
                });
            }
            ancestors.push(i);
        }
        rows
    }

    fn rebuild_related(&mut self) {
        let mut rows = vec![RelatedRow::Header("On this page")];
        let Some(id) = self.current.clone() else {
            self.related = rows;
            return;
        };
        let toc = self.toc_rows();
        if toc.is_empty() {
            rows.push(RelatedRow::None);
        }
        rows.extend(toc);

        rows.push(RelatedRow::Blank);
        rows.push(RelatedRow::Header("Links on this page"));
        let id = id.as_str();
        let mut section = Vec::new();
        let mut seen: Vec<LinkTarget> = Vec::new();
        if let Some(page) = self.wiki.pages.get(id) {
            for raw in &page.links {
                let target = self.wiki.resolve(id, raw);
                if seen.contains(&target) || matches!(&target, LinkTarget::Page(p) if p == id) {
                    continue;
                }
                section.push(match &target {
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
        self.sort_related(&mut section);
        rows.extend(section);

        rows.push(RelatedRow::Blank);
        rows.push(RelatedRow::Header("Backlinks"));
        let backlinks = self.wiki.backlinks_of(id);
        if backlinks.is_empty() {
            rows.push(RelatedRow::None);
        }
        let mut section: Vec<RelatedRow> = backlinks
            .iter()
            .map(|b| RelatedRow::Page(b.clone()))
            .collect();
        self.sort_related(&mut section);
        rows.extend(section);
        self.related = rows;
        self.related_sel = self
            .related
            .iter()
            .position(RelatedRow::selectable)
            .unwrap_or(0);
    }

    /// Alphabetical by what the column shows, ignoring case.
    fn sort_related(&self, rows: &mut [RelatedRow]) {
        rows.sort_by_cached_key(|row| {
            match row {
                RelatedRow::Page(id) => self.wiki.title(id),
                RelatedRow::Missing(raw) => raw.clone(),
                RelatedRow::External(url) => url.clone(),
                _ => String::new(),
            }
            .to_lowercase()
        });
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

    /// Move the selection `n` selectable rows down (or up when negative), stopping at the ends.
    fn related_move_by(&mut self, n: isize) {
        let step = n.signum();
        for _ in 0..n.abs() {
            let before = self.related_sel;
            self.related_move(step);
            if self.related_sel == before {
                break;
            }
        }
    }

    fn related_activate(&mut self) {
        let target = match self.related.get(self.related_sel) {
            Some(RelatedRow::Page(id)) => LinkTarget::Page(id.clone()),
            Some(RelatedRow::Missing(raw)) => LinkTarget::Missing(raw.clone()),
            Some(RelatedRow::External(url)) => LinkTarget::External(url.clone()),
            Some(RelatedRow::Heading { index, .. }) => {
                // Jump so the heading is the first visible line.
                if let Some(line) = self
                    .rendered
                    .as_ref()
                    .and_then(|r| r.headings.get(*index))
                    .map(|h| h.line)
                {
                    self.scroll = line.min(self.max_scroll());
                    self.sel = None;
                }
                return;
            }
            _ => return,
        };
        self.follow(&target);
    }

    /// Fold or unfold the selected heading: `Some(true)` expands, `Some(false)` collapses,
    /// `None` toggles.
    fn toc_fold(&mut self, expand: Option<bool>) {
        let Some(RelatedRow::Heading {
            index,
            foldable,
            expanded,
            depth,
            ..
        }) = self.related.get(self.related_sel)
        else {
            return;
        };
        let (index, depth) = (*index, *depth);
        if !foldable || expand == Some(*expanded) {
            // `h` on a heading that cannot collapse further goes to its parent.
            if expand == Some(false) && depth > 0
                && let Some(parent) = (0..self.related_sel).rev().find(|&i| {
                    matches!(&self.related[i], RelatedRow::Heading { depth: d, .. } if *d < depth)
                }) {
                    self.related_sel = parent;
                }
            return;
        }
        if !self.toc_expanded.remove(&index) {
            self.toc_expanded.insert(index);
        }
        self.rebuild_related();
        if let Some(i) = self
            .related
            .iter()
            .position(|r| matches!(r, RelatedRow::Heading { index: h, .. } if *h == index))
        {
            self.related_sel = i;
        }
    }

    /// One crumb per folder above the current page, each named after the folder's `index`
    /// page when it has one. A folder index page is its folder's crumb, not an extra one.
    fn rebuild_crumbs(&mut self) {
        let root_name = self
            .wiki
            .root
            .file_name()
            .map(|n| crate::wiki::prettify(&n.to_string_lossy()))
            .unwrap_or_else(|| "Wiki".into());
        let folder_label = |folder: &str, fallback: String| match self.wiki.folder_index(folder) {
            Some(index) => self.wiki.title(&index),
            None => fallback,
        };
        let mut crumbs = vec![Crumb {
            label: folder_label("", root_name),
            folder: Some(String::new()),
        }];
        if let Some(id) = &self.current {
            let parts: Vec<&str> = id.split('/').collect();
            let (folders, page) = parts.split_at(parts.len() - 1);
            let mut folder = String::new();
            for part in folders {
                if !folder.is_empty() {
                    folder.push('/');
                }
                folder.push_str(part);
                crumbs.push(Crumb {
                    label: folder_label(&folder, crate::wiki::prettify(part)),
                    folder: Some(folder.clone()),
                });
            }
            if page != ["index"] {
                crumbs.push(Crumb {
                    label: self.wiki.title(id),
                    folder: None,
                });
            }
        }
        self.crumbs = crumbs;
    }

    fn crumb_activate(&mut self, index: usize) {
        let Some(folder) = self.crumbs.get(index).and_then(|c| c.folder.clone()) else {
            return;
        };
        match self.wiki.folder_index(&folder) {
            Some(page) => self.open(&page, true),
            None if folder.is_empty() => {
                if let Some(home) = self.wiki.landing_page() {
                    self.open(&home, true);
                }
            }
            None => self.reveal_folder(&folder),
        }
    }

    // ----- input -----------------------------------------------------------------------

    // ----- editing and creating pages --------------------------------------------------

    /// Open the current page's source in the editor.
    pub fn start_edit(&mut self) {
        let Some(page) = self.current.as_ref().and_then(|id| self.wiki.pages.get(id)) else {
            return;
        };
        let text = fs::read_to_string(&page.path).unwrap_or_default();
        self.editor = Some(Editor::new(&text));
        self.prompt = Prompt::None;
        self.find = None;
        self.search = None;
        self.popup = None;
        self.focus = Focus::Content;
        self.repaint = true;
    }

    fn save_edit(&mut self) {
        let Some(editor) = self.editor.take() else {
            return;
        };
        let Some(page) = self.current.as_ref().and_then(|id| self.wiki.pages.get(id)) else {
            return;
        };
        let (path, id) = (page.path.clone(), page.id.clone());
        match fs::write(&path, editor.text()) {
            Ok(()) => {
                self.reload();
                self.status = format!("Saved {id}");
            }
            Err(err) => {
                // Keep the text so nothing is lost.
                self.status = format!("Could not save {}: {err}", path.display());
                self.editor = Some(editor);
                return;
            }
        }
        self.repaint = true;
    }

    fn close_edit(&mut self) {
        self.editor = None;
        self.complete = None;
        self.image_pick = None;
        self.repaint = true;
    }

    // ----- images in the editor --------------------------------------------------------

    /// Ctrl-V in the editor: an image on the clipboard is saved into the wiki and linked;
    /// text is inserted as it is.
    fn paste_into_editor(&mut self) {
        let types = Command::new("wl-paste")
            .arg("--list-types")
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let image_type = types
            .lines()
            .find(|t| t.starts_with("image/png") || t.starts_with("image/jpeg"))
            .map(str::to_string);
        if let Some(mime) = image_type {
            let bytes = Command::new("wl-paste")
                .args(["--type", &mime])
                .stderr(Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success() && !o.stdout.is_empty())
                .map(|o| o.stdout);
            if let Some(bytes) = bytes {
                let ext = if mime.starts_with("image/jpeg") {
                    "jpg"
                } else {
                    "png"
                };
                match self.save_pasted_image(&bytes, ext) {
                    Ok(rel) => {
                        self.insert_image_link(&rel);
                        self.status = format!("Saved the pasted image as {rel}");
                    }
                    Err(err) => self.status = format!("Could not save the image: {err:#}"),
                }
                return;
            }
        }
        let text = Command::new("wl-paste")
            .arg("--no-newline")
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        if let Some(editor) = &mut self.editor {
            if text.is_empty() {
                editor.paste_internal();
            } else {
                editor.paste(&text);
            }
        }
        self.update_complete();
    }

    /// Write clipboard image bytes to `assets/<page>-<n>.<ext>` under the wiki root and
    /// return that path relative to the root.
    fn save_pasted_image(&self, bytes: &[u8], ext: &str) -> anyhow::Result<String> {
        let page = self
            .current
            .as_deref()
            .and_then(|id| id.rsplit('/').next())
            .unwrap_or("image");
        let dir = self.wiki.root.join("assets");
        fs::create_dir_all(&dir)?;
        let name = (1..)
            .map(|n| format!("{page}-{n}.{ext}"))
            .find(|name| !dir.join(name).exists())
            .unwrap();
        fs::write(dir.join(&name), bytes)?;
        Ok(format!("assets/{name}"))
    }

    /// Insert `![name](path)` for an image given relative to the wiki root, with the path
    /// written relative to the current page's folder.
    fn insert_image_link(&mut self, image: &str) {
        let page_dir = self
            .current
            .as_deref()
            .and_then(|id| id.rsplit_once('/'))
            .map(|(dir, _)| dir)
            .unwrap_or("");
        let path = relative_path(page_dir, image);
        let name = image
            .rsplit('/')
            .next()
            .and_then(|f| f.rsplit_once('.'))
            .map(|(stem, _)| stem)
            .unwrap_or("image");
        if let Some(editor) = &mut self.editor {
            editor.paste(&format!("![{name}]({path})"));
        }
    }

    fn open_image_pick(&mut self) {
        let root = self.wiki.root.clone();
        let mut all: Vec<String> = walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| IMAGE_EXTENSIONS.contains(&x.to_lowercase().as_str()))
            })
            .filter_map(|e| {
                e.path()
                    .strip_prefix(&root)
                    .ok()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
            })
            .collect();
        all.sort_by_key(|p| p.to_lowercase());
        self.complete = None;
        self.image_pick = Some(ImagePick {
            query: String::new(),
            hits: all.clone(),
            all,
            sel: 0,
        });
    }

    fn refilter_images(&mut self) {
        let Some(pick) = &mut self.image_pick else {
            return;
        };
        let needle = wiki::lower_chars(&pick.query);
        pick.hits = pick
            .all
            .iter()
            .filter(|p| {
                wiki::contains(&wiki::lower_chars(p), &needle).is_some() || needle.is_empty()
            })
            .cloned()
            .collect();
        pick.sel = pick.sel.min(pick.hits.len().saturating_sub(1));
    }

    /// Keys the image picker takes; true if consumed.
    fn image_pick_key(&mut self, key: KeyEvent) -> bool {
        let Some(pick) = &mut self.image_pick else {
            return false;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let count = pick.hits.len().max(1);
        match key.code {
            KeyCode::Esc => self.image_pick = None,
            KeyCode::Down => pick.sel = (pick.sel + 1) % count,
            KeyCode::Up => pick.sel = (pick.sel + count - 1) % count,
            KeyCode::Char('n') if ctrl => pick.sel = (pick.sel + 1) % count,
            KeyCode::Char('p') if ctrl => pick.sel = (pick.sel + count - 1) % count,
            KeyCode::Enter | KeyCode::Tab => {
                if let Some(image) = pick.hits.get(pick.sel).cloned() {
                    self.image_pick = None;
                    self.insert_image_link(&image);
                }
            }
            KeyCode::Backspace => {
                pick.query.pop();
                self.refilter_images();
            }
            KeyCode::Char(c) if !ctrl => {
                pick.query.push(c);
                self.refilter_images();
            }
            _ => {}
        }
        true
    }

    /// Update the `[[` suggestions after the editor changed.
    fn update_complete(&mut self) {
        let context = self.editor.as_ref().and_then(Editor::link_context);
        let Some((start, typed)) = context else {
            self.complete = None;
            self.complete_dismissed = None;
            return;
        };
        if self.complete_dismissed == Some(start) {
            return;
        }
        let needle = wiki::lower_chars(&typed);
        let mut hits: Vec<(usize, String, String)> = self
            .wiki
            .pages
            .values()
            .filter_map(|p| {
                let id = wiki::lower_chars(&p.id);
                let title = wiki::lower_chars(&p.title);
                let rank = if title.starts_with(&needle[..]) || id.starts_with(&needle[..]) {
                    0
                } else if wiki::contains(&title, &needle).is_some()
                    || wiki::contains(&id, &needle).is_some()
                {
                    1
                } else {
                    return None;
                };
                Some((rank, p.title.clone(), p.id.clone()))
            })
            .collect();
        hits.sort_by_cached_key(|(rank, title, id)| (*rank, title.to_lowercase(), id.clone()));
        hits.truncate(8);
        let hits: Vec<(String, String)> = hits.into_iter().map(|(_, t, i)| (i, t)).collect();
        let sel = match &self.complete {
            Some(c) if c.start == start => c.sel.min(hits.len().saturating_sub(1)),
            _ => 0,
        };
        self.complete = Some(Complete { start, hits, sel });
    }

    /// Keys the suggestion list takes before the editor sees them; true if consumed.
    fn complete_key(&mut self, key: KeyEvent) -> bool {
        let Some(complete) = &mut self.complete else {
            return false;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.complete_dismissed = Some(complete.start);
                self.complete = None;
            }
            KeyCode::Down => complete.sel = (complete.sel + 1) % complete.hits.len().max(1),
            KeyCode::Up => {
                complete.sel =
                    (complete.sel + complete.hits.len().max(1) - 1) % complete.hits.len().max(1)
            }
            KeyCode::Char('n') if ctrl => {
                complete.sel = (complete.sel + 1) % complete.hits.len().max(1)
            }
            KeyCode::Char('p') if ctrl => {
                complete.sel =
                    (complete.sel + complete.hits.len().max(1) - 1) % complete.hits.len().max(1)
            }
            KeyCode::Enter | KeyCode::Tab => {
                let start = complete.start;
                let Some((id, _)) = complete.hits.get(complete.sel).cloned() else {
                    return false;
                };
                if let Some(editor) = &mut self.editor {
                    editor.complete_link(start, &id);
                }
                self.complete = None;
            }
            _ => return false,
        }
        true
    }

    /// A page id for a link target that does not exist yet, or `None` if it cannot be a page.
    fn new_page_id(&self, raw: &str) -> Option<String> {
        let target = raw.split('#').next().unwrap_or("").trim();
        let target = target.strip_suffix(".md").unwrap_or(target);
        let id: String = target
            .split('/')
            .map(str::trim)
            .filter(|p| !p.is_empty() && *p != ".")
            .collect::<Vec<_>>()
            .join("/");
        let ok = !id.is_empty()
            && !id.contains("..")
            && !id.contains("://")
            && id
                .chars()
                .all(|c| c.is_alphanumeric() || "-_./ ".contains(c));
        ok.then_some(id)
    }

    /// Create `id.md` with a title heading, open it, and start editing.
    fn create_page(&mut self, id: &str) {
        let path = self.wiki.root.join(format!("{id}.md"));
        if path.exists() {
            self.open(id, true);
            self.start_edit();
            return;
        }
        let title = wiki::prettify(id);
        let result = path
            .parent()
            .map(fs::create_dir_all)
            .unwrap_or(Ok(()))
            // Title, a blank line, and an empty line for the cursor to start on.
            .and_then(|_| fs::write(&path, format!("# {title}\n\n\n")));
        if let Err(err) = result {
            self.status = format!("Could not create {}: {err}", path.display());
            return;
        }
        if let Err(err) = self.wiki.reload() {
            self.status = format!("Reload failed: {err}");
            return;
        }
        self.rebuild_tree();
        self.open(id, true);
        self.start_edit();
        if let Some(editor) = &mut self.editor {
            editor.cursor_to_end();
        }
        self.status = format!("Created {id}");
    }

    fn delete_current(&mut self) {
        let Some(id) = self.current.clone() else {
            return;
        };
        // Land on the folder's index, or the home page, after the page is gone.
        let folder = id.rsplit_once('/').map(|(f, _)| f).unwrap_or("");
        match self.wiki.delete_page(&id) {
            Ok(()) => {
                self.history.retain(|h| *h != id);
                self.future.retain(|h| *h != id);
                self.current = None;
                self.rebuild_tree();
                let next = self
                    .wiki
                    .folder_index(folder)
                    .filter(|i| *i != id)
                    .or_else(|| self.wiki.landing_page());
                match next {
                    Some(next) => self.open(&next, false),
                    None => self.reload(),
                }
                self.status = format!("Deleted {id}");
            }
            Err(err) => self.status = format!("{err:#}"),
        }
    }

    fn rename_current(&mut self, typed: &str) {
        let Some(old) = self.current.clone() else {
            return;
        };
        let Some(new) = self.new_page_id(typed) else {
            self.status = "That is not a page path (folder/name)".into();
            return;
        };
        if new == old {
            return;
        }
        match self.wiki.rename_page(&old, &new) {
            Ok(updated) => {
                for h in self.history.iter_mut().chain(self.future.iter_mut()) {
                    if *h == old {
                        *h = new.clone();
                    }
                }
                self.rebuild_tree();
                self.open(&new, false);
                self.status = match updated {
                    0 => format!("Renamed to {new}"),
                    1 => format!("Renamed to {new}; updated the link in 1 page"),
                    n => format!("Renamed to {new}; updated links in {n} pages"),
                };
            }
            Err(err) => self.status = format!("{err:#}"),
        }
    }

    pub fn handle_paste(&mut self, text: &str) {
        if let Some(editor) = &mut self.editor {
            editor.paste(text);
            self.update_complete();
        } else if let Some(search) = &mut self.search {
            search.query.push_str(text.trim());
            self.research();
        } else if let Some(find) = &mut self.find {
            find.query.push_str(text.trim());
            self.refind(true);
        }
    }

    // ----- find in page and wiki search ------------------------------------------------

    /// Recompute the matches for the find query; `keep` tries to stay on the current match.
    fn refind(&mut self, keep: bool) {
        let Some(find) = &mut self.find else { return };
        let needle = wiki::lower_chars(&find.query);
        let mut matches = Vec::new();
        if !needle.is_empty()
            && let Some(rendered) = &self.rendered
        {
            for (line_no, line) in rendered.lines.iter().enumerate() {
                let text = line.to_string();
                let chars: Vec<char> = text.chars().collect();
                let lower = wiki::lower_chars(&text);
                let mut from = 0;
                while let Some(at) = wiki::contains(&lower[from..], &needle) {
                    let at = from + at;
                    let start = width_of(&chars[..at]);
                    let end = start + width_of(&chars[at..at + needle.len()]);
                    matches.push(FindMatch {
                        line: line_no,
                        start,
                        end,
                    });
                    from = at + needle.len().max(1);
                }
            }
        }
        let previous = find.matches.get(find.current).copied();
        find.matches = matches;
        find.current = match previous.filter(|_| keep) {
            Some(p) => find.matches.iter().position(|m| *m == p).unwrap_or(0),
            None => find
                .matches
                .iter()
                .position(|m| m.line >= self.scroll)
                .unwrap_or(0),
        };
        self.show_current_match();
    }

    fn show_current_match(&mut self) {
        let Some(find) = &self.find else { return };
        let Some(m) = find.matches.get(find.current) else {
            return;
        };
        let line = m.line;
        if line < self.scroll || line >= self.scroll + self.content_height {
            let target = line.saturating_sub(self.content_height / 3);
            self.scroll = target.min(self.max_scroll());
        }
    }

    fn find_step(&mut self, delta: isize) {
        let Some(find) = &mut self.find else { return };
        let count = find.matches.len();
        if count == 0 {
            return;
        }
        find.current = (find.current as isize + delta).rem_euclid(count as isize) as usize;
        self.show_current_match();
    }

    /// Start finding `query` in the current page (empty: just open the prompt).
    pub fn start_find(&mut self, query: &str) {
        self.prompt = Prompt::FindPage;
        self.search = None;
        self.find = Some(Find {
            query: query.to_string(),
            matches: Vec::new(),
            current: 0,
        });
        self.refind(false);
    }

    fn research(&mut self) {
        let Some(search) = &mut self.search else {
            return;
        };
        search.hits = self.wiki.search(&search.query);
        search.sel = 0;
        search.scroll = 0;
    }

    fn search_move(&mut self, delta: isize) {
        if let Some(search) = &mut self.search
            && !search.hits.is_empty()
        {
            let max = search.hits.len() as isize - 1;
            search.sel = (search.sel as isize + delta).clamp(0, max) as usize;
        }
    }

    /// Open the selected search result and find the query in it.
    fn search_open(&mut self) {
        let Some(search) = &self.search else { return };
        let Some(hit) = search.hits.get(search.sel) else {
            return;
        };
        let (id, query) = (hit.id.clone(), search.query.clone());
        self.search = None;
        self.prompt = Prompt::None;
        self.open(&id, true);
        self.start_find(&query);
    }

    fn prompt_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (self.prompt, key.code) {
            (_, KeyCode::Esc) => {
                self.prompt = Prompt::None;
                self.find = None;
                self.search = None;
                self.pending_create = None;
            }
            // Only an explicit "y" creates: Enter (the key that got here) declines.
            (Prompt::CreatePage, KeyCode::Char('y' | 'Y')) => {
                self.prompt = Prompt::None;
                if let Some(id) = self.pending_create.take() {
                    self.create_page(&id);
                }
            }
            (Prompt::CreatePage, _) => {
                self.prompt = Prompt::None;
                self.pending_create = None;
            }
            (Prompt::NewPage, KeyCode::Enter) => {
                self.prompt = Prompt::None;
                let typed = std::mem::take(&mut self.new_page);
                match self.new_page_id(&typed) {
                    Some(id) => self.create_page(&id),
                    None => self.status = "That is not a page path (folder/name)".into(),
                }
            }
            (Prompt::NewPage | Prompt::RenamePage, KeyCode::Backspace) => {
                self.new_page.pop();
            }
            (Prompt::NewPage | Prompt::RenamePage, KeyCode::Char(c)) if !ctrl => {
                self.new_page.push(c)
            }
            (Prompt::RenamePage, KeyCode::Enter) => {
                self.prompt = Prompt::None;
                let typed = std::mem::take(&mut self.new_page);
                self.rename_current(&typed);
            }
            (Prompt::DeletePage, KeyCode::Char('y' | 'Y')) => {
                self.prompt = Prompt::None;
                self.delete_current();
            }
            (Prompt::DeletePage, _) => self.prompt = Prompt::None,
            (Prompt::FindPage, KeyCode::Enter | KeyCode::Down) => self.find_step(1),
            (Prompt::FindPage, KeyCode::Up) => self.find_step(-1),
            (Prompt::FindPage, KeyCode::Char('n')) if ctrl => self.find_step(1),
            (Prompt::FindPage, KeyCode::Char('p')) if ctrl => self.find_step(-1),
            (Prompt::FindPage, KeyCode::Backspace) => {
                if let Some(find) = &mut self.find {
                    find.query.pop();
                }
                self.refind(true);
            }
            (Prompt::FindPage, KeyCode::Char(c)) if !ctrl => {
                if let Some(find) = &mut self.find {
                    find.query.push(c);
                }
                self.refind(true);
            }
            (Prompt::SearchWiki, KeyCode::Enter) => self.search_open(),
            (Prompt::SearchWiki, KeyCode::Down) => self.search_move(1),
            (Prompt::SearchWiki, KeyCode::Up) => self.search_move(-1),
            (Prompt::SearchWiki, KeyCode::Char('j' | 'n')) if ctrl => self.search_move(1),
            (Prompt::SearchWiki, KeyCode::Char('k' | 'p')) if ctrl => self.search_move(-1),
            (Prompt::SearchWiki, KeyCode::PageDown) => self.search_move(10),
            (Prompt::SearchWiki, KeyCode::PageUp) => self.search_move(-10),
            (Prompt::SearchWiki, KeyCode::Backspace) => {
                if let Some(search) = &mut self.search {
                    search.query.pop();
                }
                self.research();
            }
            (Prompt::SearchWiki, KeyCode::Char(c)) if !ctrl => {
                if let Some(search) = &mut self.search {
                    search.query.push(c);
                }
                self.research();
            }
            _ => {}
        }
    }

    // ----- input -----------------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.show_help {
            self.show_help = false;
            return;
        }
        if self.popup.is_some() {
            self.popup_key(key);
            return;
        }
        if self.editor.is_some() {
            if self.image_pick_key(key) || self.complete_key(key) {
                return;
            }
            let action = self.editor.as_mut().unwrap().handle_key(key);
            match action {
                Action::Save => self.save_edit(),
                Action::Discard => self.close_edit(),
                Action::Paste => self.paste_into_editor(),
                Action::PickImage => self.open_image_pick(),
                Action::None => self.update_complete(),
            }
            return;
        }
        if self.prompt != Prompt::None {
            self.prompt_key(key);
            return;
        }
        self.status.clear();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('e') => self.start_edit(),
            KeyCode::Char('N') => {
                self.prompt = Prompt::NewPage;
                self.new_page.clear();
            }
            KeyCode::Char('R') if self.current.is_some() => {
                self.prompt = Prompt::RenamePage;
                self.new_page = self.current.clone().unwrap_or_default();
            }
            KeyCode::Char('D') if self.current.is_some() => self.prompt = Prompt::DeletePage,
            KeyCode::Char('/') => self.start_find(""),
            KeyCode::Char('s') => {
                self.prompt = Prompt::SearchWiki;
                self.find = None;
                self.search = Some(Search {
                    query: String::new(),
                    hits: Vec::new(),
                    sel: 0,
                    scroll: 0,
                    view: Rect::default(),
                });
            }
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
            KeyCode::Char('v') => self.toggle_raw(),
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
                Focus::Tree => self.tree_key(key, ctrl),
                Focus::Content => self.content_key(key, ctrl),
                Focus::Related => self.related_key(key, ctrl),
            },
        }
    }

    fn cycle_focus(&mut self, delta: isize) {
        const ORDER: [Focus; 3] = [Focus::Tree, Focus::Content, Focus::Related];
        let i = ORDER.iter().position(|f| *f == self.focus).unwrap_or(1) as isize;
        self.focus = ORDER[(i + delta).rem_euclid(3) as usize];
    }

    fn tree_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.tree_move(1),
            KeyCode::Char('k') | KeyCode::Up => self.tree_move(-1),
            KeyCode::Char('g') | KeyCode::Home => self.tree_sel = 0,
            KeyCode::Char('G') | KeyCode::End => self.tree_sel = self.tree.len().saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char('d') if key.code != KeyCode::Char('d') || ctrl => {
                self.tree_move(self.rects.tree.height.max(1) as isize)
            }
            KeyCode::PageUp | KeyCode::Char('u') if key.code != KeyCode::Char('u') || ctrl => {
                self.tree_move(-(self.rects.tree.height.max(1) as isize))
            }
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
            KeyCode::Char('n') => self.next_item(),
            KeyCode::Char('p') => self.prev_item(),
            KeyCode::Enter => self.activate_selected(),
            KeyCode::Esc => self.sel = None,
            _ => {}
        }
    }

    fn related_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.related_move(1),
            KeyCode::Char('k') | KeyCode::Up => self.related_move(-1),
            KeyCode::PageDown | KeyCode::Char('d') if key.code != KeyCode::Char('d') || ctrl => {
                self.related_move_by(self.rects.related.height.max(1) as isize)
            }
            KeyCode::PageUp | KeyCode::Char('u') if key.code != KeyCode::Char('u') || ctrl => {
                self.related_move_by(-(self.rects.related.height.max(1) as isize))
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.related_move_by(-(self.related.len() as isize))
            }
            KeyCode::Char('G') | KeyCode::End => self.related_move_by(self.related.len() as isize),
            KeyCode::Enter => self.related_activate(),
            KeyCode::Char('l') | KeyCode::Right => self.toc_fold(Some(true)),
            KeyCode::Char('h') | KeyCode::Left => self.toc_fold(Some(false)),
            KeyCode::Char(' ') => self.toc_fold(None),
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
        if self.popup.is_some() {
            self.popup_mouse(mouse);
            return;
        }
        if let Some(editor) = &mut self.editor {
            editor.handle_mouse(mouse);
            self.update_complete();
            return;
        }
        if let Some(search) = &self.search {
            let view = search.view;
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left)
                    if view.contains((mouse.column, mouse.row).into()) =>
                {
                    let row = (mouse.row - view.y) as usize / 2 + search.scroll;
                    if row < search.hits.len() {
                        self.search.as_mut().unwrap().sel = row;
                        self.search_open();
                    }
                }
                MouseEventKind::Down(_) => {
                    self.search = None;
                    self.prompt = Prompt::None;
                }
                MouseEventKind::ScrollDown => self.search_move(1),
                MouseEventKind::ScrollUp => self.search_move(-1),
                _ => {}
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
                    if let Some(i) = self.item_at(line, col) {
                        self.sel = Some(i);
                        self.activate_selected();
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

/// `target` (relative to the wiki root) written relative to the folder `from` (also root
/// relative, `""` for the root): `../assets/x.png` from `projects`.
fn relative_path(from: &str, target: &str) -> String {
    let from: Vec<&str> = from.split('/').filter(|p| !p.is_empty()).collect();
    let target: Vec<&str> = target.split('/').filter(|p| !p.is_empty()).collect();
    let common = from.iter().zip(&target).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = vec![".."; from.len() - common];
    parts.extend(&target[common..]);
    parts.join("/")
}

/// Display width of a run of characters.
fn width_of(chars: &[char]) -> u16 {
    use unicode_width::UnicodeWidthChar;
    chars.iter().map(|c| c.width().unwrap_or(0) as u16).sum()
}

/// A picker like `picker` but sized for `font_size` cells.
#[allow(deprecated)] // from_fontsize is the only way to set a size the terminal did not report
pub fn picker_with_font_size(picker: &Picker, font_size: FontSize) -> Picker {
    let mut sized = Picker::from_fontsize(font_size);
    sized.set_protocol_type(picker.protocol_type());
    sized
}

/// The terminal's cell size in pixels, from the window size it reports; `None` until the window
/// is mapped and reports real pixel dimensions.
pub fn font_size_from_window() -> Option<FontSize> {
    let w = ratatui::crossterm::terminal::window_size().ok()?;
    if w.columns == 0 || w.rows == 0 || w.width == 0 || w.height == 0 {
        return None;
    }
    Some(FontSize::new(w.width / w.columns, w.height / w.rows))
}

/// What the Markdown renderer needs while rendering one page.
struct RenderCtx<'a> {
    wiki: &'a Wiki,
    page: &'a str,
    page_dir: PathBuf,
    picker: Option<&'a Picker>,
    images: &'a mut HashMap<(PathBuf, u16, u16), Option<Rc<SlicedProtocol>>>,
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

    fn image_size(&mut self, path: &Path, max_width: u16, max_height: u16) -> Option<(u16, u16)> {
        let key = (path.to_path_buf(), max_width, max_height);
        if !self.images.contains_key(&key) {
            let proto = self.picker.and_then(|picker| {
                let img = image::ImageReader::open(path)
                    .ok()?
                    .with_guessed_format()
                    .ok()?
                    .decode()
                    .ok()?;
                let bound = Size::new(max_width, max_height);
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

#[cfg(test)]
mod tests {
    use super::relative_path;

    #[test]
    fn image_paths_are_relative_to_the_page_folder() {
        assert_eq!(relative_path("", "assets/a.png"), "assets/a.png");
        assert_eq!(relative_path("projects", "assets/a.png"), "../assets/a.png");
        assert_eq!(
            relative_path("projects/dotfiles", "projects/shot.png"),
            "../shot.png"
        );
        assert_eq!(relative_path("notes", "notes/pics/b.jpg"), "pics/b.jpg");
    }
}
