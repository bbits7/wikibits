//! The wiki index: every `.md` file under the root, its title, and the links between pages.
//!
//! A page is identified by its path relative to the root, without the `.md` extension and with
//! `/` separators (`projects/calcbits`). That is also what `[[projects/calcbits]]` links refer to.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use pulldown_cmark::{CodeBlockKind, Event, LinkType, Options, Parser, Tag, TagEnd};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct Page {
    pub id: String,
    pub path: PathBuf,
    pub title: String,
    /// Raw link targets as written in the file, in document order, without duplicates.
    pub links: Vec<String>,
    /// The file's Markdown source.
    pub text: String,
}

/// A page matching a wiki search.
pub struct SearchHit {
    pub id: String,
    pub title: String,
    /// Whether the title itself matched.
    pub in_title: bool,
    /// The first matching line, trimmed around the match; empty if only the title matched.
    pub snippet: String,
}

/// Where a link points once resolved against the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    Page(String),
    Missing(String),
    External(String),
}

#[derive(Debug, Clone)]
pub enum TreeNode {
    Folder {
        name: String,
        path: String,
        /// The folder's `index` page, which stands for the folder itself.
        index: Option<String>,
        children: Vec<TreeNode>,
    },
    Page {
        id: String,
        title: String,
    },
}

/// Hidden file in a folder listing its pages and subfolders by name (`backlog`, never
/// `backlog.md` or `backlog/`), one per line, in the order the tree shows them. Names it leaves
/// out follow, sorted by title, so a folder without one sorts alphabetically.
pub const ORDER_FILE: &str = ".order";

/// Image file extensions the page renderer can show.
pub const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

pub struct Wiki {
    pub root: PathBuf,
    pub pages: BTreeMap<String, Page>,
    /// Page id -> ids of pages linking to it, in id order.
    pub backlinks: HashMap<String, Vec<String>>,
    /// Folder path (`""` for the root) -> the names in its order file.
    pub orders: HashMap<String, Vec<String>>,
    /// Folder path (`""` for the root) -> the page that stands for it: its `index` page,
    /// else the page named like the folder inside it (`recipes/recipes`, any case), else
    /// the page beside it (`recipes.md` next to `recipes/`).
    folder_pages: HashMap<String, String>,
}

impl Wiki {
    pub fn load(root: &Path) -> Result<Wiki> {
        let root = root
            .canonicalize()
            .with_context(|| format!("cannot open wiki folder {}", root.display()))?;
        let mut wiki = Wiki {
            root,
            pages: BTreeMap::new(),
            backlinks: HashMap::new(),
            orders: HashMap::new(),
            folder_pages: HashMap::new(),
        };
        wiki.reload()?;
        Ok(wiki)
    }

    pub fn reload(&mut self) -> Result<()> {
        let mut pages = BTreeMap::new();
        let mut orders = HashMap::new();
        let walker = WalkDir::new(&self.root)
            .follow_links(true)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| !is_hidden(e.file_name()));
        for entry in walker.filter_map(|e| e.ok()) {
            let path = entry.path();
            if entry.file_type().is_dir() {
                if let Some(folder) = self.folder_for(path)
                    && let Some(names) = read_order(path)
                {
                    orders.insert(folder, names);
                }
                continue;
            }
            if !entry.file_type().is_file() || path.extension().is_none_or(|e| e != "md") {
                continue;
            }
            let Some(id) = self.id_for(path) else {
                continue;
            };
            let text = fs::read_to_string(path).unwrap_or_default();
            pages.insert(
                id.clone(),
                Page {
                    title: extract_title(&text).unwrap_or_else(|| prettify(&id)),
                    links: extract_links(&text),
                    id,
                    path: path.to_path_buf(),
                    text,
                },
            );
        }
        self.pages = pages;
        self.orders = orders;
        self.folder_pages = self.find_folder_pages();

        let mut backlinks: HashMap<String, Vec<String>> = HashMap::new();
        for page in self.pages.values() {
            for raw in &page.links {
                if let LinkTarget::Page(target) = self.resolve(&page.id, raw) {
                    if target == page.id {
                        continue;
                    }
                    let list = backlinks.entry(target).or_default();
                    if !list.contains(&page.id) {
                        list.push(page.id.clone());
                    }
                }
            }
        }
        self.backlinks = backlinks;
        Ok(())
    }

    /// Page id for a file under the root, or `None` if it is outside the root.
    pub fn id_for(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.root).ok()?.with_extension("");
        let id = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        (!id.is_empty()).then_some(id)
    }

    /// Folder path (`""` for the root) for a directory under the root.
    fn folder_for(&self, dir: &Path) -> Option<String> {
        let rel = dir.strip_prefix(&self.root).ok()?;
        Some(
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
        )
    }

    pub fn title(&self, id: &str) -> String {
        self.pages
            .get(id)
            .map(|p| p.title.clone())
            .unwrap_or_else(|| prettify(id))
    }

    pub fn backlinks_of(&self, id: &str) -> &[String] {
        self.backlinks.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The page that stands for each folder. `index` wins; without one, a page inside the
    /// folder with the folder's own name does (the root's is named after the wiki folder),
    /// exact case first; without that, the page beside the folder with its name
    /// (`recipes.md` next to `recipes/`, the layout Azure DevOps wikis use).
    fn find_folder_pages(&self) -> HashMap<String, String> {
        let root_name = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut found: HashMap<String, String> = HashMap::new();
        for id in self.pages.keys() {
            let (folder, name) = split_id(id);
            let folder_name = if folder.is_empty() {
                root_name.as_str()
            } else {
                last_segment(folder)
            };
            // 0 = index, 1 = the folder's name exactly, 2 = the folder's name in another case.
            let rank = |name: &str| match name {
                "index" => Some(0),
                n if n == folder_name => Some(1),
                n if n.eq_ignore_ascii_case(folder_name) => Some(2),
                _ => None,
            };
            let Some(new) = rank(name) else { continue };
            let better = found
                .get(folder)
                .and_then(|old| rank(last_segment(old)))
                .is_none_or(|old| new < old);
            if better {
                found.insert(folder.to_string(), id.clone());
            }
        }
        // A page beside a folder of the same name: `a/b.md` next to `a/b/`.
        for id in self.pages.keys() {
            let mut folder = split_id(id).0;
            while !folder.is_empty() {
                if self.pages.contains_key(folder) && !found.contains_key(folder) {
                    found.insert(folder.to_string(), folder.to_string());
                }
                folder = split_id(folder).0;
            }
        }
        found
    }

    /// The folder a page stands for (`""` for the root), if it is a folder's page: the
    /// folder it is in, or for a page beside its folder, the folder with the page's own path.
    pub fn folder_of_page<'a>(&self, id: &'a str) -> Option<&'a str> {
        let is_page_of = |folder: &str| self.folder_pages.get(folder).map(String::as_str) == Some(id);
        let (folder, _) = split_id(id);
        if is_page_of(folder) {
            Some(folder)
        } else {
            is_page_of(id).then_some(id)
        }
    }

    /// Whether a page stands for its folder: an `index` page, or one named like the folder.
    pub fn is_folder_page(&self, id: &str) -> bool {
        self.folder_of_page(id).is_some()
    }

    /// The link target that reaches a page: the folder for a folder's page (`/` for the
    /// root), the page id otherwise.
    pub fn link_target(&self, id: &str) -> String {
        match self.folder_of_page(id) {
            Some("") => "/".to_string(),
            Some(folder) => folder.to_string(),
            None => id.to_string(),
        }
    }

    /// The page to show first: the root folder's page, else `home` or `readme` at the root,
    /// else the first page.
    pub fn landing_page(&self) -> Option<String> {
        self.folder_index("")
            .or_else(|| {
                ["index", "home", "readme"].iter().find_map(|name| {
                    self.pages
                        .keys()
                        .find(|id| id.eq_ignore_ascii_case(name))
                        .cloned()
                })
            })
            .or_else(|| self.pages.keys().next().cloned())
    }

    /// Resolve a link written in page `from` to a page id.
    ///
    /// Wiki links count from the wiki root: `[[projects/calcbits]]`. As a courtesy the link also
    /// works relative to the current page's folder, with or without `.md`, ignoring case, and by
    /// bare file name when that name is unique in the wiki.
    pub fn resolve(&self, from: &str, raw: &str) -> LinkTarget {
        let raw = raw.trim();
        if raw.contains("://") || raw.starts_with("mailto:") {
            return LinkTarget::External(raw.to_string());
        }
        let mut target = raw.split('#').next().unwrap_or("").trim();
        if target.is_empty() {
            // `[[#Heading]]`: a place on this page.
            if link_anchor(raw).is_some() {
                return LinkTarget::Page(from.to_string());
            }
            return LinkTarget::Missing(raw.to_string());
        }
        if let Some(stripped) = target.strip_suffix(".md") {
            target = stripped;
        }
        let target = target.trim_matches('/');
        // Index pages are reached through their folder: [[folder]], or [[/]] for the home
        // page. A direct [[folder/index]] stays red so it gets fixed.
        if is_index_id(target) {
            return LinkTarget::Missing(raw.to_string());
        }

        let from_dir = from.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
        let candidates = [
            normalize(target),
            normalize(&format!("{from_dir}/{target}")),
        ];
        for candidate in &candidates {
            if let Some(id) = self.find_page(candidate) {
                return LinkTarget::Page(id);
            }
        }
        let name = target.rsplit('/').next().unwrap_or(target);
        let mut by_name = self.pages.keys().filter(|id| {
            id.rsplit('/')
                .next()
                .unwrap_or(id)
                .eq_ignore_ascii_case(name)
        });
        if let (Some(id), None) = (by_name.next(), by_name.next()) {
            return LinkTarget::Page(id.clone());
        }
        LinkTarget::Missing(raw.to_string())
    }

    /// The page with this exact id, or the page of the folder with this path (`""` is the
    /// root), either matched exactly or ignoring case.
    fn find_page(&self, candidate: &str) -> Option<String> {
        if self.pages.contains_key(candidate) {
            return Some(candidate.to_string());
        }
        if let Some(page) = self.folder_pages.get(candidate) {
            return Some(page.clone());
        }
        self.pages
            .keys()
            .find(|id| id.eq_ignore_ascii_case(candidate))
            .or_else(|| {
                self.folder_pages
                    .iter()
                    .find(|(folder, _)| folder.eq_ignore_ascii_case(candidate))
                    .map(|(_, page)| page)
            })
            .cloned()
    }

    /// The page that stands for a folder (`""` for the root), if it has one: its `index`
    /// page, else the page named like the folder.
    pub fn folder_index(&self, folder: &str) -> Option<String> {
        self.folder_pages.get(folder).cloned()
    }

    /// Image files under the root (hidden folders excluded) whose file name no page mentions.
    pub fn unreferenced_images(&self) -> Vec<PathBuf> {
        let texts: Vec<&str> = self.pages.values().map(|p| p.text.as_str()).collect();
        WalkDir::new(&self.root)
            .into_iter()
            .filter_entry(|e| !is_hidden(e.file_name()))
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| IMAGE_EXTENSIONS.contains(&x.to_lowercase().as_str()))
            })
            .filter(|e| {
                let name = e.file_name().to_string_lossy();
                !texts.iter().any(|t| t.contains(name.as_ref()))
            })
            .map(|e| e.into_path())
            .collect()
    }

    /// Move the images no page mentions into `.trash/` under the root, where the wiki does
    /// not look. Returns the moved file names.
    pub fn trash_unreferenced_images(&mut self) -> Result<Vec<String>> {
        let unused = self.unreferenced_images();
        if unused.is_empty() {
            return Ok(Vec::new());
        }
        let trash = self.root.join(".trash");
        fs::create_dir_all(&trash)?;
        let mut moved = Vec::new();
        for path in unused {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let mut target = trash.join(&name);
            let mut n = 1;
            while target.exists() {
                target = trash.join(format!("{n}-{name}"));
                n += 1;
            }
            fs::rename(&path, &target)
                .with_context(|| format!("cannot move {} to .trash", path.display()))?;
            self.remove_empty_folders(path.parent());
            moved.push(name);
        }
        Ok(moved)
    }

    /// Commit every change in the wiki folder when it is a git repository. `None` when it is
    /// not one, `Some(false)` when there was nothing to commit or git failed.
    pub fn git_commit(&self, message: &str) -> Option<bool> {
        if !self.root.join(".git").exists() {
            return None;
        }
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        git(&["add", "-A"]);
        Some(git(&["commit", "-q", "-m", message]))
    }

    /// Rewrite the relative links inside a page file that moved from folder `from` to folder
    /// `to` (both root relative), so its images and `.md` links still point at the same files.
    fn rebase_page_links(&self, path: &Path, from: &str, to: &str) -> Result<()> {
        if from == to {
            return Ok(());
        }
        let text = fs::read_to_string(path)?;
        let rebased = rewrite_links(&text, |kind, raw| {
            if kind != LinkKind::Markdown
                || raw.contains("://")
                || raw.starts_with('/')
                || raw.starts_with('#')
                || raw.starts_with("mailto:")
            {
                return None;
            }
            let (target, fragment) = raw.split_once('#').unwrap_or((raw, ""));
            let absolute = normalize(&format!("{from}/{target}"));
            let mut new = relative_path(to, &absolute);
            if !fragment.is_empty() {
                new = format!("{new}#{fragment}");
            }
            (new != raw).then_some(new)
        });
        if rebased != text {
            fs::write(path, rebased)?;
        }
        Ok(())
    }

    /// A folder holding nothing but `index.md` becomes the plain page `folder.md` again, and links to `folder/index` are rewritten to `folder`. Checks
    /// the deepest folders first so a whole chain collapses. Returns the moves, old to new.
    pub fn demote_lonely_folders(&mut self) -> Result<Vec<(String, String)>> {
        let mut folders: Vec<PathBuf> = WalkDir::new(&self.root)
            .min_depth(1)
            .into_iter()
            .filter_entry(|e| !is_hidden(e.file_name()))
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_dir())
            .map(|e| e.into_path())
            .collect();
        folders.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        let mut moved = Vec::new();
        for folder in folders {
            let entries: Vec<PathBuf> = fs::read_dir(&folder)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| !is_hidden(p.file_name().unwrap_or_default()))
                .collect();
            let only_index =
                entries.len() == 1 && entries[0].file_name().is_some_and(|n| n == "index.md");
            if !only_index || folder.with_extension("md").exists() {
                continue;
            }
            fs::rename(&entries[0], folder.with_extension("md"))?;
            let _ = fs::remove_file(folder.join(ORDER_FILE));
            let _ = fs::remove_dir(&folder);
            if let Some(id) = self.id_for(&folder.with_extension("md")) {
                let parent = id.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
                self.rebase_page_links(&folder.with_extension("md"), &id, parent)?;
                moved.push((format!("{id}/index"), id));
            }
        }
        if moved.is_empty() {
            return Ok(moved);
        }
        // Links written as [[folder/index]] must follow the page to [[folder]].
        for page in self.pages.values() {
            let rewritten = rewrite_links(&page.text, |_, raw| {
                // Such links are written as [[folder/index]], which the resolver refuses,
                // so match them by their normalized text.
                let target = normalize(
                    raw.split('#')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .trim_end_matches(".md")
                        .trim_matches('/'),
                );
                moved
                    .iter()
                    .find(|(old, _)| *old == target)
                    .map(|(_, new)| retarget(raw, new))
            });
            if rewritten != page.text {
                fs::write(&page.path, rewritten)?;
            }
        }
        self.reload()?;
        Ok(moved)
    }

    /// Move a page's file into `.trash/` (keeping its folder path) and remove folders it
    /// leaves empty.
    pub fn delete_page(&mut self, id: &str) -> Result<()> {
        let page = self
            .pages
            .get(id)
            .with_context(|| format!("no page {id}"))?;
        let mut target = self.root.join(".trash").join(format!("{id}.md"));
        let mut n = 1;
        while target.exists() {
            target = self.root.join(".trash").join(format!("{id}-{n}.md"));
            n += 1;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&page.path, &target)
            .with_context(|| format!("cannot move {} to .trash", page.path.display()))?;
        let old_path = page.path.clone();
        self.follow_in_order(id, None)?;
        self.remove_empty_folders(old_path.parent());
        self.reload()
    }

    /// Links that do not reach a page: `(page id, link as written)`, in page order.
    pub fn broken_links(&self) -> Vec<(String, String)> {
        self.pages
            .values()
            .flat_map(|page| {
                page.links
                    .iter()
                    .filter_map(|raw| match self.resolve(&page.id, raw) {
                        LinkTarget::Missing(_) => Some((page.id.clone(), raw.clone())),
                        _ => None,
                    })
            })
            .collect()
    }

    /// Pages nothing links to, apart from the home page.
    pub fn orphan_pages(&self) -> Vec<String> {
        self.pages
            .keys()
            .filter(|id| self.folder_of_page(id) != Some("") && self.backlinks_of(id).is_empty())
            .cloned()
            .collect()
    }

    /// Pages by modification time, newest first, with how long ago each changed.
    pub fn recently_changed(&self, limit: usize) -> Vec<(String, String)> {
        let now = std::time::SystemTime::now();
        let mut pages: Vec<(std::time::SystemTime, String)> = self
            .pages
            .values()
            .filter_map(|p| Some((fs::metadata(&p.path).ok()?.modified().ok()?, p.id.clone())))
            .collect();
        pages.sort_by_key(|a| std::cmp::Reverse(a.0));
        pages
            .into_iter()
            .take(limit)
            .map(|(time, id)| {
                let secs = now.duration_since(time).map(|d| d.as_secs()).unwrap_or(0);
                let ago = match secs {
                    0..=59 => "just now".to_string(),
                    60..=3599 => format!("{} min ago", secs / 60),
                    3600..=86399 => format!("{} h ago", secs / 3600),
                    _ => format!("{} days ago", secs / 86400),
                };
                (id, ago)
            })
            .collect()
    }

    /// Move a page to a new id (`folder/name`) and rewrite the links that point at it in the
    /// other pages. A plain page above the new place stays where it is: `notes.md` beside the
    /// new `notes/` folder is that folder's page.
    pub fn rename_page(&mut self, old: &str, new: &str) -> Result<Moved> {
        if !self.pages.contains_key(old) {
            anyhow::bail!("no page {old}");
        }
        let target = self.root.join(format!("{new}.md"));
        if target.exists() {
            anyhow::bail!("a page named {new} already exists");
        }
        let page = self
            .pages
            .get(old)
            .with_context(|| format!("no page {old}"))?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&page.path, &target).with_context(|| {
            format!(
                "cannot move {} to {}",
                page.path.display(),
                target.display()
            )
        })?;
        let old_path = page.path.clone();
        self.follow_in_order(old, Some(new))?;
        self.remove_empty_folders(old_path.parent());
        let from_dir = old.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let to_dir = new.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        self.rebase_page_links(&target, from_dir, to_dir)?;

        let mut updated = 0;
        let linkers: Vec<String> = self.backlinks_of(old).to_vec();
        for id in linkers {
            let Some(linker) = self.pages.get(&id) else {
                continue;
            };
            let rewritten = rewrite_links(&linker.text, |_, raw| {
                (self.resolve(&id, raw) == LinkTarget::Page(old.to_string()))
                    .then(|| retarget(raw, new))
            });
            if rewritten != linker.text {
                fs::write(&linker.path, rewritten)?;
                updated += 1;
            }
        }
        self.reload()?;
        Ok(Moved {
            id: new.to_string(),
            updated,
        })
    }

    /// Move the page `id` under the sibling listed just above it in the tree, as that
    /// sibling's last child: with `[notes, todo]`, moving `todo` makes `notes/todo`, and a
    /// plain `notes` page stays beside the new folder as its page. `None` when nothing is above it.
    pub fn move_into_previous(&mut self, id: &str) -> Result<Option<Moved>> {
        let (folder, name) = split_id(id);
        let names = self.sibling_names(folder);
        let Some(at) = names.iter().position(|n| n == name) else {
            anyhow::bail!("{name} is not in {}", display_folder(folder));
        };
        if at == 0 {
            return Ok(None);
        }
        let parent = join_id(folder, &names[at - 1]);
        let moved = self.rename_page(id, &join_id(&parent, name))?;
        // Last among its new siblings, and pinned there whatever the alphabet says later.
        let mut names = self.sibling_names(&parent);
        names.retain(|n| n != name);
        names.push(name.to_string());
        self.write_order(&parent, names)?;
        Ok(Some(moved))
    }

    /// Move the page `id` out of its folder, to just after that folder among the folder's
    /// siblings. `None` when it is already at the top level.
    pub fn move_out(&mut self, id: &str) -> Result<Option<Moved>> {
        let (folder, name) = split_id(id);
        if folder.is_empty() {
            return Ok(None);
        }
        let (grandparent, folder_name) = split_id(folder);
        let new = join_id(grandparent, name);
        if self.root.join(&new).is_dir() {
            anyhow::bail!("a folder named {new} already exists");
        }
        let moved = self.rename_page(id, &new)?;
        let mut names = self.sibling_names(grandparent);
        names.retain(|n| n != name);
        let at = names
            .iter()
            .position(|n| n == folder_name)
            .map_or(names.len(), |i| i + 1);
        names.insert(at, name.to_string());
        self.write_order(grandparent, names)?;
        Ok(Some(moved))
    }

    /// Remove `folder` and its parents while they are empty (an order file alone counts as
    /// empty), stopping at the root; removed folders leave their parent's order file.
    fn remove_empty_folders(&mut self, folder: Option<&Path>) {
        let mut current = folder;
        while let Some(dir) = current {
            if dir == self.root
                || fs::read_dir(dir)
                    .map(|mut d| d.any(|e| e.is_ok_and(|e| e.file_name() != ORDER_FILE)))
                    .unwrap_or(true)
            {
                break;
            }
            let _ = fs::remove_file(dir.join(ORDER_FILE));
            if fs::remove_dir(dir).is_err() {
                break;
            }
            if let Some(path) = self.folder_for(dir) {
                let _ = self.follow_in_order(&path, None);
            }
            current = dir.parent();
        }
    }

    /// Pages whose title or text contains `query`, ignoring case: title matches first, then
    /// by title.
    pub fn search(&self, query: &str) -> Vec<SearchHit> {
        // Every word must appear somewhere in the title or text.
        let words: Vec<Vec<char>> = query.split_whitespace().map(lower_chars).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<SearchHit> = self
            .pages
            .values()
            .filter_map(|page| {
                let title = lower_chars(&page.title);
                let text = lower_chars(&page.text);
                let in_title = words.iter().all(|w| contains(&title, w).is_some());
                let all_found = in_title
                    || words
                        .iter()
                        .all(|w| contains(&title, w).is_some() || contains(&text, w).is_some());
                if !all_found {
                    return None;
                }
                let first = &words[0];
                let snippet = page.text.lines().find_map(|line| {
                    contains(&lower_chars(line), first).map(|at| snippet(line, at, first.len()))
                });
                (in_title || snippet.is_some()).then(|| SearchHit {
                    id: page.id.clone(),
                    title: page.title.clone(),
                    in_title,
                    snippet: snippet.unwrap_or_default(),
                })
            })
            .collect();
        hits.sort_by_cached_key(|h| (!h.in_title, h.title.to_lowercase()));
        hits
    }

    /// Folder/page tree, sorted by displayed name ignoring case. A folder with an
    /// `index` page takes that page's title, and the page is not listed among its children.
    pub fn tree(&self) -> Vec<TreeNode> {
        let mut root = Vec::new();
        for page in self.pages.values() {
            insert_into_tree(&mut root, "", &page.id, page);
        }
        fold_indexes(&mut root, &self.folder_pages);
        sort_tree(&mut root, "", &self.orders);
        root
    }

    /// Move the page or subfolder `name` of `folder` one place up (`delta` -1) or down (+1)
    /// among its siblings, writing the folder's order file. The first move in a folder writes
    /// out the whole current order. Returns false when it is already first or last.
    pub fn move_in_order(&mut self, folder: &str, name: &str, delta: isize) -> Result<bool> {
        let mut names = self.sibling_names(folder);
        let Some(at) = names.iter().position(|n| n == name) else {
            anyhow::bail!("{name} is not in {}", display_folder(folder));
        };
        let to = at as isize + delta;
        if to < 0 || to >= names.len() as isize {
            return Ok(false);
        }
        names.swap(at, to as usize);
        self.write_order(folder, names)?;
        Ok(true)
    }

    /// Names of the pages and subfolders of `folder` in tree order (the root folder's page,
    /// which always leads, left out).
    fn sibling_names(&self, folder: &str) -> Vec<String> {
        let mut nodes = self.tree();
        let mut depth = 0;
        while depth < folder.len() {
            // Step into the child folder whose path is the next prefix of `folder`.
            let next = folder[depth..]
                .find('/')
                .map_or(folder.len(), |i| depth + i);
            let Some(children) = nodes.into_iter().find_map(|n| match n {
                TreeNode::Folder { path, children, .. } if path == folder[..next] => Some(children),
                _ => None,
            }) else {
                return Vec::new();
            };
            nodes = children;
            depth = next + 1;
        }
        nodes
            .iter()
            .filter_map(|n| match n {
                TreeNode::Page { id, .. } if self.folder_of_page(id) == Some("") => None,
                TreeNode::Page { id, .. } => Some(last_segment(id).to_string()),
                TreeNode::Folder { path, .. } => Some(last_segment(path).to_string()),
            })
            .collect()
    }

    /// Replace `folder`'s order file with `names`, or remove it when there are none.
    fn write_order(&mut self, folder: &str, names: Vec<String>) -> Result<()> {
        let file = self.root.join(folder).join(ORDER_FILE);
        if names.is_empty() {
            if file.exists() {
                fs::remove_file(&file)?;
            }
            self.orders.remove(folder);
        } else {
            fs::write(&file, names.join("\n") + "\n")
                .with_context(|| format!("cannot write {}", file.display()))?;
            self.orders.insert(folder.to_string(), names);
        }
        Ok(())
    }

    /// Keep the order files in step with a page moving from `old` to `new`: renamed within
    /// its folder it keeps its place; moved to another folder it leaves the old folder's list
    /// (unless a folder of that name is still there).
    fn follow_in_order(&mut self, old: &str, new: Option<&str>) -> Result<()> {
        let (old_dir, old_name) = split_id(old);
        let Some(mut names) = self.orders.get(old_dir).cloned() else {
            return Ok(());
        };
        let Some(at) = names.iter().position(|n| n == old_name) else {
            return Ok(());
        };
        match new.map(split_id) {
            Some((new_dir, new_name)) if new_dir == old_dir => {
                names.retain(|n| n != new_name);
                let at = names.iter().position(|n| n == old_name).unwrap_or(at);
                names[at] = new_name.to_string();
            }
            _ => {
                let base = self.root.join(old_dir).join(old_name);
                if base.is_dir() || base.with_extension("md").is_file() {
                    return Ok(());
                }
                names.remove(at);
            }
        }
        if self.root.join(old_dir).is_dir() {
            self.write_order(old_dir, names)?;
        }
        Ok(())
    }
}

/// The folder and the last segment of an id or path (`""` folder at the root).
fn split_id(id: &str) -> (&str, &str) {
    id.rsplit_once('/').unwrap_or(("", id))
}

/// A link's target as written, pointed at the page `new` instead: `page.md#top` becomes
/// `new.md#top`, keeping the `.md` and the anchor.
fn retarget(raw: &str, new: &str) -> String {
    let (target, anchor) = raw.split_once('#').unwrap_or((raw, ""));
    let mut out = new.to_string();
    if target.trim_end().ends_with(".md") {
        out.push_str(".md");
    }
    if !anchor.is_empty() {
        out.push('#');
        out.push_str(anchor);
    }
    out
}

/// `folder/name`, or `name` at the root.
fn join_id(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_string()
    } else {
        format!("{folder}/{name}")
    }
}

/// What a page move did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    /// The page's new id.
    pub id: String,
    /// How many pages had links to it rewritten.
    pub updated: usize,
}

fn last_segment(path: &str) -> &str {
    split_id(path).1
}

fn display_folder(folder: &str) -> &str {
    if folder.is_empty() {
        "the top folder"
    } else {
        folder
    }
}

/// Names listed in a folder's order file, if it has one.
fn read_order(dir: &Path) -> Option<Vec<String>> {
    let text = fs::read_to_string(dir.join(ORDER_FILE)).ok()?;
    Some(
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Whether an id names an index page (`index` or `folder/index`). For a page that exists,
/// `Wiki::is_folder_page` also knows the pages named like their folder.
pub fn is_index_id(id: &str) -> bool {
    id == "index" || id.ends_with("/index")
}

/// The link target that reaches a page: the folder for an index page (`/` for the root).
pub fn link_target_for(id: &str) -> String {
    match id.strip_suffix("/index") {
        Some(folder) => folder.to_string(),
        None if id == "index" => "/".to_string(),
        None => id.to_string(),
    }
}

fn fold_indexes(nodes: &mut Vec<TreeNode>, folder_pages: &HashMap<String, String>) {
    for node in nodes.iter_mut() {
        if let TreeNode::Folder {
            name,
            path,
            index,
            children,
        } = node
        {
            if let Some(wanted) = folder_pages.get(path.as_str())
                && let Some(pos) = children
                    .iter()
                    .position(|c| matches!(c, TreeNode::Page { id, .. } if id == wanted))
                && let TreeNode::Page { id, title } = children.remove(pos)
            {
                *name = title;
                *index = Some(id);
            }
            fold_indexes(children, folder_pages);
        }
    }
    // A page beside its folder (`b.md` next to `b/`) folds into the folder row the same way.
    let beside: Vec<(String, String)> = nodes
        .iter()
        .filter_map(|n| match n {
            TreeNode::Page { id, title } if folder_pages.get(id.as_str()) == Some(id) => {
                Some((id.clone(), title.clone()))
            }
            _ => None,
        })
        .collect();
    for (page, title) in beside {
        let folder = nodes.iter_mut().find_map(|n| match n {
            TreeNode::Folder {
                name, path, index, ..
            } if *path == page => Some((name, index)),
            _ => None,
        });
        if let Some((name, index)) = folder {
            *name = title;
            *index = Some(page.clone());
            nodes.retain(|n| !matches!(n, TreeNode::Page { id, .. } if *id == page));
        }
    }
}

fn insert_into_tree(nodes: &mut Vec<TreeNode>, prefix: &str, rest: &str, page: &Page) {
    match rest.split_once('/') {
        None => nodes.push(TreeNode::Page {
            id: page.id.clone(),
            title: page.title.clone(),
        }),
        Some((folder, remainder)) => {
            let path = if prefix.is_empty() {
                folder.to_string()
            } else {
                format!("{prefix}/{folder}")
            };
            let existing = nodes.iter_mut().find_map(|n| match n {
                TreeNode::Folder {
                    path: p, children, ..
                } if *p == path => Some(children),
                _ => None,
            });
            match existing {
                Some(children) => insert_into_tree(children, &path, remainder, page),
                None => {
                    let mut children = Vec::new();
                    insert_into_tree(&mut children, &path, remainder, page);
                    nodes.push(TreeNode::Folder {
                        name: prettify(folder),
                        path,
                        index: None,
                        children,
                    });
                }
            }
        }
    }
}

/// Folders and pages together: the root `index` page leads, then the names in the folder's
/// order file, then the rest by displayed name ignoring case.
fn sort_tree(nodes: &mut [TreeNode], folder: &str, orders: &HashMap<String, Vec<String>>) {
    let order = orders.get(folder).map(Vec::as_slice).unwrap_or(&[]);
    let rank = |name: &str| order.iter().position(|n| n == name).unwrap_or(usize::MAX);
    nodes.sort_by_cached_key(|n| match n {
        TreeNode::Page { id, .. } if id == "index" => (0, 0, String::new()),
        TreeNode::Folder { name, path, .. } => (1, rank(last_segment(path)), name.to_lowercase()),
        TreeNode::Page { id, title } => (1, rank(last_segment(id)), title.to_lowercase()),
    });
    for node in nodes {
        if let TreeNode::Folder { path, children, .. } = node {
            sort_tree(children, path, orders);
        }
    }
}

fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

/// Collapse `.` and `..` segments in a slash-separated path.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LinkKind {
    /// `[[target]]` or `[[target|label]]`.
    Wiki,
    /// `[label](target)` or `![alt](target)`.
    Markdown,
}

/// Replace link targets in `text`: `replace` gets each `[[target]]` / `[[target|label]]`
/// target and each `[label](target)` target and returns the new target, or `None` to keep it.
pub fn rewrite_links(text: &str, replace: impl Fn(LinkKind, &str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(inner) = rest.strip_prefix("[[")
            && !inner.starts_with('[')
            && let Some(close) = inner.find("]]")
        {
            let body = &inner[..close];
            let (target, label) = body
                .split_once('|')
                .map_or((body, None), |(t, l)| (t, Some(l)));
            out.push_str("[[");
            out.push_str(&replace(LinkKind::Wiki, target).unwrap_or_else(|| target.to_string()));
            if let Some(label) = label {
                out.push('|');
                out.push_str(label);
            }
            out.push_str("]]");
            rest = &inner[close + 2..];
        } else if let Some(inner) = rest.strip_prefix("](")
            && let Some(close) = inner.find(')')
        {
            let target = &inner[..close];
            out.push_str("](");
            out.push_str(
                &replace(LinkKind::Markdown, target).unwrap_or_else(|| target.to_string()),
            );
            out.push(')');
            rest = &inner[close + 1..];
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// `target` (relative to the wiki root) written relative to the folder `from` (also root
/// relative, `""` for the root): `../assets/x.png` from `projects`.
pub fn relative_path(from: &str, target: &str) -> String {
    let from: Vec<&str> = from.split('/').filter(|p| !p.is_empty()).collect();
    let target: Vec<&str> = target.split('/').filter(|p| !p.is_empty()).collect();
    let common = from.iter().zip(&target).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = vec![".."; from.len() - common];
    parts.extend(&target[common..]);
    parts.join("/")
}

/// Lower-cased characters, one per input character, so positions line up with the original.
pub fn lower_chars(text: &str) -> Vec<char> {
    text.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// Character index where `needle` first occurs in `haystack`.
pub fn contains(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| haystack[i..i + needle.len()] == *needle)
}

/// `line` cut down to about 70 characters around the match at character `at`.
fn snippet(line: &str, at: usize, len: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    let start = at.saturating_sub(25);
    let end = (at + len + 45).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Turn a slug like `my-wiki-page` into `My wiki page`.
pub fn prettify(slug: &str) -> String {
    let name = slug.rsplit('/').next().unwrap_or(slug);
    let mut chars = name.replace(['-', '_'], " ").chars().collect::<Vec<_>>();
    if let Some(first) = chars.first_mut() {
        *first = first.to_uppercase().next().unwrap_or(*first);
    }
    chars.into_iter().collect()
}

/// The page's `# Heading`, if that is the first line of the file (YAML front matter and
/// blank lines before it aside). A `# Heading` further down is an ordinary heading, and the
/// page is then named after its file.
fn extract_title(text: &str) -> Option<String> {
    let mut lines = text.lines().peekable();
    if lines.peek().is_some_and(|l| l.trim() == "---") {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
        }
    }
    let line = lines.find(|line| !line.trim().is_empty())?;
    let heading = line.trim_start().strip_prefix("# ")?.trim();
    (!heading.is_empty()).then(|| heading.trim_end_matches('#').trim().to_string())
}

pub fn parser_options() -> Options {
    Options::ENABLE_WIKILINKS
        | Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
}

/// Every link target in the document, in order, without duplicates — including the
/// `[[links]]` written in the labels of ```diaBits blocks.
fn extract_links(text: &str) -> Vec<String> {
    let mut links: Vec<String> = Vec::new();
    let mut add = |dest: String| {
        if !links.contains(&dest) {
            links.push(dest);
        }
    };
    let mut in_diagram = false;
    let (text, _) = escape_link_pipes(text);
    for event in Parser::new_ext(&text, parser_options()) {
        match event {
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) => {
                // A link to a place on this page is not a link to another page.
                if !matches!(link_type, LinkType::Email) && !dest_url.starts_with('#') {
                    add(wiki_dest(link_type, &dest_url).to_string());
                }
            }
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang))) => {
                in_diagram = lang.trim().eq_ignore_ascii_case("diabits");
            }
            Event::End(TagEnd::CodeBlock) => in_diagram = false,
            Event::Text(source) if in_diagram => {
                for target in diagram_links(&source) {
                    add(target.to_string());
                }
            }
            _ => {}
        }
    }
    links
}

/// The pipe of a `[[page|label]]` link or a `{#anchor|text}` would split a table cell, so on
/// a table row (a line starting or ending with `|`, outside a fenced code block) every such
/// pipe is escaped (`\|`) before the Markdown is parsed: the table keeps its cells and the
/// link still parses. Returns the text and the byte offsets (in that text) of the
/// backslashes added, for `source_offset` to map offsets back.
pub fn escape_link_pipes(text: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut added = Vec::new();
    let mut in_fence = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        }
        let table_row = trimmed.starts_with('|') || trimmed.ends_with('|');
        if in_fence || !table_row || !(line.contains("[[") || line.contains("{#")) {
            out.push_str(line);
            continue;
        }
        let mut escape = |out: &mut String, body: &str| {
            let mut escaped = false;
            for c in body.chars() {
                if c == '|' && !escaped {
                    added.push(out.len());
                    out.push('\\');
                }
                out.push(c);
                escaped = c == '\\';
            }
        };
        let mut rest = line;
        while !rest.is_empty() {
            if let Some(inner) = rest.strip_prefix("[[")
                && !inner.starts_with('[')
                && let Some(close) = inner.find("]]")
            {
                out.push_str("[[");
                escape(&mut out, &inner[..close]);
                out.push_str("]]");
                rest = &inner[close + 2..];
            } else if let Some(inner) = rest.strip_prefix("{#")
                && let Some(close) = inner.find('}')
                && !inner[..close].contains('{')
            {
                out.push_str("{#");
                escape(&mut out, &inner[..close]);
                out.push('}');
                rest = &inner[close + 1..];
            } else {
                let c = rest.chars().next().unwrap();
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    (out, added)
}

/// A link's destination as written: outside a table the parser keeps the backslash of an
/// escaped pipe (`[[page\|label]]`) on the end of a wiki link's target.
pub fn wiki_dest(link_type: LinkType, dest: &str) -> &str {
    match link_type {
        LinkType::WikiLink { .. } => dest.strip_suffix('\\').unwrap_or(dest),
        _ => dest,
    }
}

/// The heading or anchor a link points at: what follows `#` in `[[page#Heading]]`.
pub fn link_anchor(raw: &str) -> Option<String> {
    let (_, anchor) = raw.split_once('#')?;
    let anchor = anchor.trim();
    (!anchor.is_empty()).then(|| anchor.to_string())
}

/// A heading's text as a slug, so `[[page#my-heading]]` finds "My Heading": lower case,
/// punctuation dropped, spaces as dashes (the same rule as the web viewer's heading ids).
pub fn heading_slug(text: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in text.trim().chars() {
        if c.is_whitespace() {
            space = true;
        } else if c.is_alphanumeric() || c == '-' {
            if space && !out.is_empty() {
                out.push('-');
            }
            space = false;
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// A byte offset into the text `escape_link_pipes` returned, as an offset into its source.
pub fn source_offset(added: &[usize], offset: usize) -> usize {
    offset - added.iter().filter(|&&a| a < offset).count()
}

/// The targets of the `[[page]]` / `[[page|label]]` links in a diagram's source.
pub fn diagram_links(source: &str) -> Vec<&str> {
    let mut targets = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find("[[") {
        let inner = &rest[start + 2..];
        // `a[[[page]]]`: the link starts at the last of the opening brackets.
        if inner.starts_with('[') {
            rest = &rest[start + 1..];
            continue;
        }
        let Some(close) = inner.find("]]") else { break };
        let body = &inner[..close];
        if !body.contains('\n') {
            targets.push(body.split_once('|').map_or(body, |(t, _)| t));
        }
        rest = &inner[close + 2..];
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_comes_from_a_heading_on_the_first_line() {
        assert_eq!(
            extract_title("\n# My Page #\n## sub").as_deref(),
            Some("My Page")
        );
        // A heading further down does not name the page.
        assert_eq!(extract_title("intro\n\n# My Page\n## sub"), None);
        assert_eq!(extract_title("## Section\n\n# Later"), None);
        assert_eq!(
            extract_title("---\ntitle: x\n---\n# Real").as_deref(),
            Some("Real")
        );
        assert_eq!(extract_title("## only h2"), None);
    }

    #[test]
    fn slugs_become_readable() {
        assert_eq!(prettify("my-wiki-page"), "My wiki page");
        assert_eq!(prettify("projects/calc_bits"), "Calc bits");
    }

    #[test]
    fn anchors_and_slugs() {
        assert_eq!(link_anchor("a/b#My Heading").as_deref(), Some("My Heading"));
        assert_eq!(link_anchor("a/b#"), None);
        assert_eq!(link_anchor("a/b"), None);
        assert_eq!(heading_slug("My  Heading: Two!"), "my-heading-two");
        assert_eq!(
            heading_slug(" Übung macht den Meister "),
            "übung-macht-den-meister"
        );
        let links = extract_links("[[a/b#Sec]] [[#here]] [x](#here) [y](c.md#top)");
        assert_eq!(links, vec!["a/b#Sec", "c.md#top"]);
    }

    #[test]
    fn link_pipes_are_escaped_outside_code() {
        let (text, added) = escape_link_pipes(
            "| [[a|b]] | x {#n|shown} |\n```\n| [[c|d]] |\n```\n- [ ] [[e\\|f]] [[g|h]] {#i|j}",
        );
        assert_eq!(
            text,
            "| [[a\\|b]] | x {#n\\|shown} |\n```\n| [[c|d]] |\n```\n- [ ] [[e\\|f]] [[g|h]] {#i|j}"
        );
        assert_eq!(added, vec![5, 18]);
        assert_eq!(source_offset(&added, 5), 5);
        assert_eq!(source_offset(&added, 10), 9);
        let links = extract_links("| h |\n|---|\n| [[a/b|that]] and [[c|d]] |\n\n[[e\\|f]]");
        assert_eq!(links, vec!["a/b", "c", "e"]);
    }

    #[test]
    fn links_are_rewritten_inside_diagram_shapes() {
        let out = rewrite_links("a[[[old]]]\nb([[old|B]])", |_, raw| {
            (raw == "old").then(|| "new".to_string())
        });
        assert_eq!(out, "a[[[new]]]\nb([[new|B]])");
    }

    #[test]
    fn links_are_collected_once() {
        let links = extract_links("[[a/b]] and [[a/b|again]] then [c](c.md) <https://x.y>");
        assert_eq!(links, vec!["a/b", "c.md", "https://x.y"]);
        let links = extract_links(
            "```diaBits\na[[[d]]]\nb([[e|E]])\na --> b | [[a/b]]\n```\n```\n[[not]]\n```",
        );
        assert_eq!(links, vec!["d", "e", "a/b"]);
    }

    #[test]
    fn index_pages_are_only_reached_through_their_folder() {
        assert_eq!(link_target_for("projects/index"), "projects");
        assert_eq!(link_target_for("index"), "/");
        assert_eq!(link_target_for("projects/calcbits"), "projects/calcbits");
        assert!(is_index_id("a/b/index") && is_index_id("index") && !is_index_id("indexes"));
    }

    #[test]
    fn a_page_named_like_its_folder_stands_for_the_folder() {
        let dir = std::env::temp_dir().join(format!("wikibits-named-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let root = dir.join("garden");
        fs::create_dir_all(root.join("Recipes/soups")).unwrap();
        fs::create_dir_all(root.join("both")).unwrap();
        fs::write(root.join("garden.md"), "# The Garden\n[[Recipes]] [[both]]").unwrap();
        fs::write(root.join("Recipes/recipes.md"), "# All Recipes\n[[soups]]").unwrap();
        fs::write(root.join("Recipes/cake.md"), "# Cake\n[[/]]").unwrap();
        fs::write(root.join("Recipes/soups/soups.md"), "Just soup, no heading.").unwrap();
        fs::write(root.join("both/index.md"), "# Both Index").unwrap();
        fs::write(root.join("both/both.md"), "# Both Named").unwrap();
        let wiki = Wiki::load(&root).unwrap();

        // Folder links, the home link and the landing page reach the named pages.
        assert_eq!(wiki.landing_page().as_deref(), Some("garden"));
        assert_eq!(wiki.folder_index("").as_deref(), Some("garden"));
        assert_eq!(wiki.folder_index("Recipes").as_deref(), Some("Recipes/recipes"));
        assert_eq!(
            wiki.resolve("garden", "recipes"),
            LinkTarget::Page("Recipes/recipes".into())
        );
        assert_eq!(
            wiki.resolve("Recipes/recipes", "soups"),
            LinkTarget::Page("Recipes/soups/soups".into())
        );
        assert_eq!(
            wiki.resolve("Recipes/cake", "/"),
            LinkTarget::Page("garden".into())
        );
        assert_eq!(wiki.link_target("Recipes/soups/soups"), "Recipes/soups");
        assert_eq!(wiki.link_target("garden"), "/");
        assert_eq!(wiki.link_target("Recipes/cake"), "Recipes/cake");
        assert_eq!(wiki.backlinks_of("Recipes/recipes"), ["garden"]);
        assert!(!wiki.orphan_pages().contains(&"garden".to_string()));

        // A page beside its folder stands for it too, as Azure DevOps wikis lay pages out.
        fs::create_dir_all(root.join("Data/Card")).unwrap();
        fs::write(root.join("Data/Card.md"), "# The Card\n[[Card/detail]]").unwrap();
        fs::write(root.join("Data/Card/detail.md"), "# Detail\n[[Data/Card]]").unwrap();
        fs::write(root.join("Data/plain.md"), "# Plain").unwrap();
        let wiki = Wiki::load(&root).unwrap();
        assert_eq!(wiki.folder_index("Data/Card").as_deref(), Some("Data/Card"));
        assert_eq!(wiki.folder_of_page("Data/Card"), Some("Data/Card"));
        assert_eq!(wiki.link_target("Data/Card"), "Data/Card");
        assert!(!wiki.is_folder_page("Data/plain") && !wiki.is_folder_page("Data/Card/detail"));
        assert_eq!(
            wiki.resolve("Data/Card/detail", "Data/Card"),
            LinkTarget::Page("Data/Card".into())
        );
        assert_eq!(wiki.backlinks_of("Data/Card"), ["Data/Card/detail"]);

        // An index page wins; the page named like the folder is then an ordinary page.
        assert_eq!(wiki.folder_index("both").as_deref(), Some("both/index"));
        assert!(wiki.is_folder_page("both/index") && !wiki.is_folder_page("both/both"));

        // The tree folds each one into its folder row.
        let tree = wiki.tree();
        let folder = |nodes: &[TreeNode], want: &str| {
            nodes
                .iter()
                .find_map(|n| match n {
                    TreeNode::Folder {
                        name,
                        path,
                        index,
                        children,
                    } if path == want => Some((name.clone(), index.clone(), children.clone())),
                    _ => None,
                })
                .unwrap()
        };
        let (name, index, children) = folder(&tree, "Recipes");
        assert_eq!(name, "All Recipes");
        assert_eq!(index.as_deref(), Some("Recipes/recipes"));
        assert_eq!(children.len(), 2, "cake and the soups folder");
        let (name, index, children) = folder(&children, "Recipes/soups");
        assert_eq!(name, "Soups", "no heading on its first line: named after the file");
        assert_eq!(index.as_deref(), Some("Recipes/soups/soups"));
        assert!(children.is_empty());
        let (_, _, data) = folder(&tree, "Data");
        assert_eq!(data.len(), 2, "the Card folder and the plain page, not Card twice");
        let (name, index, children) = folder(&data, "Data/Card");
        assert_eq!(name, "The Card");
        assert_eq!(index.as_deref(), Some("Data/Card"));
        assert_eq!(children.len(), 1);
        let (_, index, children) = folder(&tree, "both");
        assert_eq!(index.as_deref(), Some("both/index"));
        assert_eq!(children.len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folder_links_open_the_folder_index() {
        let dir = std::env::temp_dir().join(format!("wikibits-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("projects")).unwrap();
        fs::write(dir.join("index.md"), "# Home\n[[/projects]] [[/]]").unwrap();
        fs::write(dir.join("projects/index.md"), "# Projects\n[[calc]]").unwrap();
        fs::write(dir.join("projects/calc.md"), "# Calc").unwrap();
        let wiki = Wiki::load(&dir).unwrap();

        assert_eq!(
            wiki.resolve("index", "/projects"),
            LinkTarget::Page("projects/index".into())
        );
        assert_eq!(
            wiki.resolve("projects/calc", "/"),
            LinkTarget::Page("index".into())
        );
        assert_eq!(
            wiki.resolve("projects/calc", "Projects/"),
            LinkTarget::Page("projects/index".into())
        );
        assert_eq!(
            wiki.resolve("projects/index", "calc"),
            LinkTarget::Page("projects/calc".into())
        );
        // A heading on another page, or on this one.
        assert_eq!(
            wiki.resolve("projects/index", "calc#Usage"),
            LinkTarget::Page("projects/calc".into())
        );
        assert_eq!(
            wiki.resolve("projects/calc", "#Usage"),
            LinkTarget::Page("projects/calc".into())
        );
        assert_eq!(
            wiki.resolve("projects/calc", "#"),
            LinkTarget::Missing("#".into())
        );
        assert_eq!(
            wiki.resolve("index", "projects/index"),
            LinkTarget::Missing("projects/index".into())
        );
        assert_eq!(
            wiki.resolve("projects/calc", "index"),
            LinkTarget::Missing("index".into())
        );
        assert_eq!(wiki.folder_index(""), Some("index".into()));
        assert_eq!(wiki.folder_index("nope"), None);
        assert_eq!(wiki.backlinks_of("projects/index"), ["index"]);

        let tree = wiki.tree();
        assert!(matches!(&tree[0], TreeNode::Page { id, .. } if id == "index"));
        let TreeNode::Folder {
            name,
            index,
            children,
            ..
        } = &tree[1]
        else {
            panic!("expected the folder after the home page");
        };
        assert_eq!(name, "Projects");
        assert_eq!(index.as_deref(), Some("projects/index"));
        assert_eq!(children.len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_matches_titles_and_text() {
        let dir = std::env::temp_dir().join(format!("wikibits-search-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("index.md"), "# Home\n\nNothing here.").unwrap();
        fs::write(dir.join("apples.md"), "# Apples\n\nGreen ones.").unwrap();
        fs::write(
            dir.join("pie.md"),
            "# Pie\n\nBest made with Apples and cinnamon, baked slowly.",
        )
        .unwrap();
        let wiki = Wiki::load(&dir).unwrap();
        let hits = wiki.search("apple");
        let got: Vec<(&str, bool, &str)> = hits
            .iter()
            .map(|h| (h.id.as_str(), h.in_title, h.snippet.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("apples", true, "# Apples"),
                (
                    "pie",
                    false,
                    "Best made with Apples and cinnamon, baked slowly."
                ),
            ]
        );
        assert!(wiki.search("   ").is_empty());
        assert_eq!(
            wiki.search("cinnamon apples").len(),
            1,
            "both words, any order"
        );
        assert!(
            wiki.search("cinnamon nothing").is_empty(),
            "words in different pages"
        );
        assert_eq!(
            contains(&lower_chars("Éclair"), &lower_chars("éCL")),
            Some(0)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rename_moves_the_file_and_rewrites_links() {
        let dir = std::env::temp_dir().join(format!("wikibits-rename-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("old")).unwrap();
        fs::write(
            dir.join("index.md"),
            "# Home\n[[old/thing]] [[/old/thing|it]] [a](old/thing.md) [[other]]",
        )
        .unwrap();
        fs::write(dir.join("old/thing.md"), "# Thing").unwrap();
        fs::write(dir.join("other.md"), "# Other").unwrap();
        let mut wiki = Wiki::load(&dir).unwrap();

        fs::write(
            dir.join("old/thing.md"),
            "# Thing\n![p](../assets/p.png) [h](../index.md) [x](https://x.y/a.png)",
        )
        .unwrap();
        let updated = wiki.rename_page("old/thing", "new/place").unwrap().updated;
        assert_eq!(updated, 1);
        assert_eq!(
            fs::read_to_string(dir.join("new/place.md")).unwrap(),
            "# Thing\n![p](../assets/p.png) [h](../index.md) [x](https://x.y/a.png)",
            "same depth: unchanged"
        );
        wiki.rename_page("new/place", "deeper/still/place").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("deeper/still/place.md")).unwrap(),
            "# Thing\n![p](../../assets/p.png) [h](../../index.md) [x](https://x.y/a.png)"
        );
        wiki.rename_page("deeper/still/place", "new/place").unwrap();
        assert!(!dir.join("old").exists(), "the emptied folder is removed");
        assert!(wiki.pages.contains_key("new/place"));
        assert_eq!(
            fs::read_to_string(dir.join("index.md")).unwrap(),
            "# Home\n[[new/place]] [[new/place|it]] [a](new/place.md) [[other]]"
        );
        assert_eq!(wiki.backlinks_of("new/place"), ["index"]);

        wiki.delete_page("new/place").unwrap();
        assert!(!dir.join("new").exists());
        assert!(
            dir.join(".trash/new/place.md").is_file(),
            "deleted pages go to .trash"
        );
        assert!(!wiki.pages.contains_key("new/place"));
        assert_eq!(
            wiki.broken_links(),
            vec![
                ("index".to_string(), "new/place".to_string()),
                ("index".to_string(), "new/place.md".to_string())
            ]
        );
        assert!(wiki.orphan_pages().is_empty(), "other is linked from index");
        assert_eq!(wiki.recently_changed(1).len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_page_stays_beside_the_folder_of_its_children() {
        let dir = std::env::temp_dir().join(format!("wikibits-beside-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("index.md"),
            "# Home\n[[backlog]] [[backlog/history]] [[old]]",
        )
        .unwrap();
        fs::write(dir.join("backlog.md"), "# Backlog\n[h](index.md)").unwrap();
        fs::write(dir.join("old.md"), "# Old").unwrap();
        let mut wiki = Wiki::load(&dir).unwrap();

        // A child arrives: the parent page is not moved or rewritten.
        let moved = wiki.rename_page("old", "backlog/history").unwrap();
        assert_eq!(moved.id, "backlog/history");
        assert!(dir.join("backlog/history.md").is_file());
        assert!(!dir.join("backlog/index.md").exists());
        assert_eq!(
            fs::read_to_string(dir.join("backlog.md")).unwrap(),
            "# Backlog\n[h](index.md)"
        );
        assert_eq!(wiki.folder_index("backlog").as_deref(), Some("backlog"));
        assert_eq!(
            wiki.resolve("index", "backlog"),
            LinkTarget::Page("backlog".into())
        );
        assert_eq!(wiki.link_target("backlog"), "backlog");
        // One row for the page and its folder.
        assert_eq!(wiki.sibling_names(""), ["backlog"]);

        // The last child leaves: the folder goes, the page is a plain page again.
        wiki.rename_page("backlog/history", "old").unwrap();
        assert!(wiki.demote_lonely_folders().unwrap().is_empty());
        assert!(!dir.join("backlog").exists() && dir.join("backlog.md").is_file());
        assert!(!wiki.is_folder_page("backlog"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_page_moves_under_the_sibling_above_it_and_back_out() {
        let dir = std::env::temp_dir().join(format!("wikibits-nest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("index.md"), "# Home\n[[notes]] [[todo]] [[zed]]").unwrap();
        fs::write(
            dir.join("notes.md"),
            "# Notes\n[[todo#Soon|soon]] [b](todo.md#Soon)",
        )
        .unwrap();
        fs::write(dir.join("todo.md"), "# Todo").unwrap();
        fs::write(dir.join("zed.md"), "# Zed").unwrap();
        let mut wiki = Wiki::load(&dir).unwrap();

        assert_eq!(
            wiki.move_into_previous("notes").unwrap(),
            None,
            "nothing above"
        );
        let moved = wiki.move_into_previous("todo").unwrap().unwrap();
        assert_eq!(moved.id, "notes/todo");
        assert_eq!(moved.updated, 2);
        assert!(dir.join("notes/todo.md").is_file());
        assert!(
            !dir.join("notes/index.md").exists(),
            "the page above stays beside the new folder"
        );
        assert_eq!(
            fs::read_to_string(dir.join("notes.md")).unwrap(),
            "# Notes\n[[notes/todo#Soon|soon]] [b](notes/todo.md#Soon)",
            "its links follow the moved page, anchors kept"
        );
        assert_eq!(
            fs::read_to_string(dir.join("index.md")).unwrap(),
            "# Home\n[[notes]] [[notes/todo]] [[zed]]"
        );

        // Into an existing folder: last, ahead of the alphabet.
        let moved = wiki.move_into_previous("zed").unwrap().unwrap();
        assert_eq!(moved.id, "notes/zed");
        fs::write(dir.join("notes/apple.md"), "# Apple").unwrap();
        wiki.reload().unwrap();
        assert_eq!(wiki.sibling_names("notes"), ["todo", "zed", "apple"]);

        // Out again: right after the folder it left, at the top level.
        let moved = wiki.move_out("notes/todo").unwrap().unwrap();
        assert_eq!(moved.id, "todo");
        assert_eq!(wiki.sibling_names(""), ["notes", "todo"]);
        assert_eq!(wiki.move_out("todo").unwrap(), None, "already at the top");
        assert_eq!(
            fs::read_to_string(dir.join("index.md")).unwrap(),
            "# Home\n[[notes]] [[todo]] [[notes/zed]]"
        );
        wiki.move_out("notes/zed").unwrap().unwrap();
        assert_eq!(wiki.sibling_names(""), ["notes", "zed", "todo"]);
        assert_eq!(wiki.sibling_names("notes"), ["apple"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_with_only_an_index_becomes_a_page_again() {
        let dir = std::env::temp_dir().join(format!("wikibits-demote-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("backlog")).unwrap();
        fs::create_dir_all(dir.join("keep")).unwrap();
        fs::write(
            dir.join("index.md"),
            "# Home\n[[backlog/index]] [[backlog]] [[keep]]",
        )
        .unwrap();
        fs::write(dir.join("backlog/index.md"), "# Backlog").unwrap();
        fs::write(dir.join("keep/index.md"), "# Keep").unwrap();
        fs::write(dir.join("keep/child.md"), "# Child").unwrap();
        let mut wiki = Wiki::load(&dir).unwrap();

        let moved = wiki.demote_lonely_folders().unwrap();
        assert_eq!(
            moved,
            vec![("backlog/index".to_string(), "backlog".to_string())]
        );
        assert!(dir.join("backlog.md").is_file());
        assert!(!dir.join("backlog").exists());
        assert!(
            dir.join("keep/index.md").is_file(),
            "folders with other pages stay"
        );
        assert_eq!(
            fs::read_to_string(dir.join("index.md")).unwrap(),
            "# Home\n[[backlog]] [[backlog]] [[keep]]"
        );
        assert_eq!(
            wiki.resolve("index", "backlog"),
            LinkTarget::Page("backlog".into())
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn order_files_put_pages_in_a_chosen_order() {
        let dir = std::env::temp_dir().join(format!("wikibits-order-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("notes")).unwrap();
        for name in [
            "index",
            "apple",
            "banana",
            "cherry",
            "notes/one",
            "notes/two",
        ] {
            fs::write(dir.join(format!("{name}.md")), "").unwrap();
        }
        let mut wiki = Wiki::load(&dir).unwrap();
        assert_eq!(
            wiki.sibling_names(""),
            ["apple", "banana", "cherry", "notes"],
            "no order file: alphabetical"
        );
        assert!(!dir.join(ORDER_FILE).exists());

        assert!(wiki.move_in_order("", "cherry", -1).unwrap());
        assert_eq!(
            wiki.sibling_names(""),
            ["apple", "cherry", "banana", "notes"]
        );
        assert_eq!(
            fs::read_to_string(dir.join(ORDER_FILE)).unwrap(),
            "apple\ncherry\nbanana\nnotes\n",
            "the first move writes out the whole folder"
        );
        assert!(
            !wiki.move_in_order("", "apple", -1).unwrap(),
            "already first"
        );
        assert!(wiki.move_in_order("notes", "one", 1).unwrap());
        assert_eq!(wiki.sibling_names("notes"), ["two", "one"]);

        // Unlisted pages follow the listed ones; the order survives a reload.
        fs::write(dir.join("aardvark.md"), "").unwrap();
        wiki.reload().unwrap();
        assert_eq!(
            wiki.sibling_names(""),
            ["apple", "cherry", "banana", "notes", "aardvark"]
        );

        // A rename in place keeps the spot; a move to another folder leaves the list.
        wiki.rename_page("cherry", "date").unwrap();
        assert_eq!(
            wiki.sibling_names(""),
            ["apple", "date", "banana", "notes", "aardvark"]
        );
        wiki.rename_page("date", "notes/date").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(ORDER_FILE)).unwrap(),
            "apple\nbanana\nnotes\n"
        );
        assert_eq!(wiki.sibling_names("notes"), ["two", "one", "date"]);

        // Becoming a folder and back again keeps the place: the name does not change.
        wiki.rename_page("apple", "banana/apple").unwrap();
        assert_eq!(wiki.sibling_names(""), ["banana", "notes", "aardvark"]);
        assert!(dir.join("banana.md").is_file() && dir.join("banana/apple.md").is_file());
        wiki.rename_page("banana/apple", "zebra").unwrap();
        assert!(dir.join("banana.md").is_file() && !dir.join("banana").exists());
        assert_eq!(
            wiki.sibling_names(""),
            ["banana", "notes", "aardvark", "zebra"]
        );

        // A folder left with only its order file is removed.
        for id in ["notes/one", "notes/two", "notes/date"] {
            wiki.delete_page(id).unwrap();
        }
        assert!(!dir.join("notes").exists());
        assert_eq!(
            fs::read_to_string(dir.join(ORDER_FILE)).unwrap(),
            "banana\n",
            "deleted names leave the list"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unreferenced_images_go_to_trash() {
        let dir = std::env::temp_dir().join(format!("wikibits-images-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("assets")).unwrap();
        fs::create_dir_all(dir.join("old")).unwrap();
        fs::write(dir.join("index.md"), "# Home\n![used](assets/used.png)").unwrap();
        fs::write(dir.join("assets/used.png"), b"x").unwrap();
        fs::write(dir.join("assets/unused.png"), b"x").unwrap();
        fs::write(dir.join("old/stale.jpg"), b"x").unwrap();
        let mut wiki = Wiki::load(&dir).unwrap();
        let mut moved = wiki.trash_unreferenced_images().unwrap();
        moved.sort();
        assert_eq!(moved, vec!["stale.jpg", "unused.png"]);
        assert!(dir.join("assets/used.png").exists());
        assert!(dir.join(".trash/unused.png").exists());
        assert!(!dir.join("old").exists(), "an emptied folder is removed");
        assert!(wiki.trash_unreferenced_images().unwrap().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn normalize_collapses_dots() {
        assert_eq!(normalize("a/./b/../c"), "a/c");
        assert_eq!(normalize("/x//y/"), "x/y");
    }
}
