//! The wiki index: every `.md` file under the root, its title, and the links between pages.
//!
//! A page is identified by its path relative to the root, without the `.md` extension and with
//! `/` separators (`projects/calcbits`). That is also what `[[projects/calcbits]]` links refer to.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag};
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

/// Image file extensions the page renderer can show.
pub const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

pub struct Wiki {
    pub root: PathBuf,
    pub pages: BTreeMap<String, Page>,
    /// Page id -> ids of pages linking to it, in id order.
    pub backlinks: HashMap<String, Vec<String>>,
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
        };
        wiki.reload()?;
        Ok(wiki)
    }

    pub fn reload(&mut self) -> Result<()> {
        let mut pages = BTreeMap::new();
        let walker = WalkDir::new(&self.root)
            .follow_links(true)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| !is_hidden(e.file_name()));
        for entry in walker.filter_map(|e| e.ok()) {
            let path = entry.path();
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

    pub fn title(&self, id: &str) -> String {
        self.pages
            .get(id)
            .map(|p| p.title.clone())
            .unwrap_or_else(|| prettify(id))
    }

    pub fn backlinks_of(&self, id: &str) -> &[String] {
        self.backlinks.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The page to show first: `index`, `home` or `readme` at the root, else the first page.
    pub fn landing_page(&self) -> Option<String> {
        ["index", "home", "readme"]
            .iter()
            .find_map(|name| {
                self.pages
                    .keys()
                    .find(|id| id.eq_ignore_ascii_case(name))
                    .cloned()
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
            return LinkTarget::Missing(raw.to_string());
        }
        if let Some(stripped) = target.strip_suffix(".md") {
            target = stripped;
        }
        let target = target.trim_matches('/');

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

    /// The page with this exact id, or the `index` page of the folder with this path
    /// (`""` is the root), either matched exactly or ignoring case.
    fn find_page(&self, candidate: &str) -> Option<String> {
        let index = index_id(candidate);
        if self.pages.contains_key(candidate) {
            return Some(candidate.to_string());
        }
        if self.pages.contains_key(&index) {
            return Some(index);
        }
        self.pages
            .keys()
            .find(|id| id.eq_ignore_ascii_case(candidate) || id.eq_ignore_ascii_case(&index))
            .cloned()
    }

    /// The `index` page of a folder (`""` for the root), if it exists.
    pub fn folder_index(&self, folder: &str) -> Option<String> {
        let index = index_id(folder);
        self.pages.contains_key(&index).then_some(index)
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
    pub fn trash_unreferenced_images(&self) -> Result<Vec<String>> {
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

    /// Make room for a page under `id`: every ancestor that is a plain page (`backlog.md`)
    /// becomes that folder's index (`backlog/index.md`). Returns the ids that moved, old to
    /// new. Does not reload.
    pub fn promote_ancestors_to_folders(&self, id: &str) -> Result<Vec<(String, String)>> {
        let mut moved = Vec::new();
        let parts: Vec<&str> = id.split('/').collect();
        for depth in 1..parts.len() {
            let folder = parts[..depth].join("/");
            let page_file = self.root.join(format!("{folder}.md"));
            let index_file = self.root.join(&folder).join("index.md");
            if page_file.is_file() && !index_file.exists() {
                fs::create_dir_all(self.root.join(&folder))?;
                fs::rename(&page_file, &index_file).with_context(|| {
                    format!(
                        "cannot move {} to {}",
                        page_file.display(),
                        index_file.display()
                    )
                })?;
                moved.push((folder.clone(), format!("{folder}/index")));
            }
        }
        Ok(moved)
    }

    /// The opposite of promotion: a folder holding nothing but `index.md` becomes the plain
    /// page `folder.md` again, and links to `folder/index` are rewritten to `folder`. Checks
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
            let _ = fs::remove_dir(&folder);
            if let Some(id) = self.id_for(&folder.with_extension("md")) {
                moved.push((format!("{id}/index"), id));
            }
        }
        if moved.is_empty() {
            return Ok(moved);
        }
        // Links written as [[folder/index]] must follow the page to [[folder]].
        for page in self.pages.values() {
            let rewritten = rewrite_links(&page.text, |raw| {
                let LinkTarget::Page(target) = self.resolve(&page.id, raw) else {
                    return None;
                };
                moved
                    .iter()
                    .find(|(old, _)| *old == target)
                    .map(|(_, new)| {
                        if raw.trim_end().ends_with(".md") {
                            format!("{new}.md")
                        } else {
                            new.clone()
                        }
                    })
            });
            if rewritten != page.text {
                fs::write(&page.path, rewritten)?;
            }
        }
        self.reload()?;
        Ok(moved)
    }

    /// Delete a page's file and remove folders it leaves empty.
    pub fn delete_page(&mut self, id: &str) -> Result<()> {
        let page = self
            .pages
            .get(id)
            .with_context(|| format!("no page {id}"))?;
        fs::remove_file(&page.path)
            .with_context(|| format!("cannot delete {}", page.path.display()))?;
        self.remove_empty_folders(page.path.parent());
        self.reload()
    }

    /// Move a page to a new id (`folder/name`) and rewrite the links that point at it in the
    /// other pages. Returns how many pages had links updated.
    pub fn rename_page(&mut self, old: &str, new: &str) -> Result<usize> {
        let page = self
            .pages
            .get(old)
            .with_context(|| format!("no page {old}"))?;
        let target = self.root.join(format!("{new}.md"));
        if target.exists() {
            anyhow::bail!("a page named {new} already exists");
        }
        self.promote_ancestors_to_folders(new)?;
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
        self.remove_empty_folders(page.path.parent());

        let mut updated = 0;
        let linkers: Vec<String> = self.backlinks_of(old).to_vec();
        for id in linkers {
            let Some(linker) = self.pages.get(&id) else {
                continue;
            };
            let rewritten = rewrite_links(&linker.text, |raw| {
                (self.resolve(&id, raw) == LinkTarget::Page(old.to_string())).then(|| {
                    if raw.trim_end().ends_with(".md") {
                        format!("{new}.md")
                    } else {
                        new.to_string()
                    }
                })
            });
            if rewritten != linker.text {
                fs::write(&linker.path, rewritten)?;
                updated += 1;
            }
        }
        self.reload()?;
        Ok(updated)
    }

    /// Remove `folder` and its parents while they are empty, stopping at the root.
    fn remove_empty_folders(&self, folder: Option<&Path>) {
        let mut current = folder;
        while let Some(dir) = current {
            if dir == self.root
                || fs::read_dir(dir)
                    .map(|mut d| d.next().is_some())
                    .unwrap_or(true)
                || fs::remove_dir(dir).is_err()
            {
                break;
            }
            current = dir.parent();
        }
    }

    /// Pages whose title or text contains `query`, ignoring case: title matches first, then
    /// by title.
    pub fn search(&self, query: &str) -> Vec<SearchHit> {
        let needle: Vec<char> = lower_chars(query.trim());
        if needle.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<SearchHit> = self
            .pages
            .values()
            .filter_map(|page| {
                let in_title = contains(&lower_chars(&page.title), &needle).is_some();
                let snippet = page.text.lines().find_map(|line| {
                    contains(&lower_chars(line), &needle).map(|at| snippet(line, at, needle.len()))
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
        fold_indexes(&mut root);
        sort_tree(&mut root);
        root
    }
}

/// Id of the `index` page of a folder (`""` for the root).
fn index_id(folder: &str) -> String {
    if folder.is_empty() {
        "index".to_string()
    } else {
        format!("{folder}/index")
    }
}

fn fold_indexes(nodes: &mut [TreeNode]) {
    for node in nodes {
        if let TreeNode::Folder {
            name,
            path,
            index,
            children,
        } = node
        {
            let wanted = index_id(path);
            if let Some(pos) = children
                .iter()
                .position(|c| matches!(c, TreeNode::Page { id, .. } if *id == wanted))
                && let TreeNode::Page { id, title } = children.remove(pos)
            {
                *name = title;
                *index = Some(id);
            }
            fold_indexes(children);
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

/// Folders and pages together, by displayed name ignoring case; the root `index` page leads.
fn sort_tree(nodes: &mut [TreeNode]) {
    nodes.sort_by_cached_key(|n| match n {
        TreeNode::Page { id, .. } if id == "index" => (0, String::new()),
        TreeNode::Folder { name, .. } => (1, name.to_lowercase()),
        TreeNode::Page { title, .. } => (1, title.to_lowercase()),
    });
    for node in nodes {
        if let TreeNode::Folder { children, .. } = node {
            sort_tree(children);
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

/// Replace link targets in `text`: `replace` gets each `[[target]]` / `[[target|label]]`
/// target and each `[label](target)` target and returns the new target, or `None` to keep it.
pub fn rewrite_links(text: &str, replace: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(inner) = rest.strip_prefix("[[")
            && let Some(close) = inner.find("]]")
        {
            let body = &inner[..close];
            let (target, label) = body
                .split_once('|')
                .map_or((body, None), |(t, l)| (t, Some(l)));
            out.push_str("[[");
            out.push_str(&replace(target).unwrap_or_else(|| target.to_string()));
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
            out.push_str(&replace(target).unwrap_or_else(|| target.to_string()));
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

/// The first `# Heading` in the file, skipping YAML front matter.
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
    lines.find_map(|line| {
        let heading = line.trim_start().strip_prefix("# ")?.trim();
        (!heading.is_empty()).then(|| heading.trim_end_matches('#').trim().to_string())
    })
}

pub fn parser_options() -> Options {
    Options::ENABLE_WIKILINKS
        | Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
}

/// Every link target in the document, in order, without duplicates.
fn extract_links(text: &str) -> Vec<String> {
    let mut links: Vec<String> = Vec::new();
    for event in Parser::new_ext(text, parser_options()) {
        if let Event::Start(Tag::Link {
            link_type,
            dest_url,
            ..
        }) = event
        {
            if matches!(link_type, LinkType::Email) {
                continue;
            }
            let dest = dest_url.to_string();
            if !links.contains(&dest) {
                links.push(dest);
            }
        }
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_comes_from_first_heading() {
        assert_eq!(
            extract_title("intro\n\n# My Page #\n## sub").as_deref(),
            Some("My Page")
        );
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
    fn links_are_collected_once() {
        let links = extract_links("[[a/b]] and [[a/b|again]] then [c](c.md) <https://x.y>");
        assert_eq!(links, vec!["a/b", "c.md", "https://x.y"]);
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

        let updated = wiki.rename_page("old/thing", "new/place").unwrap();
        assert_eq!(updated, 1);
        assert!(!dir.join("old").exists(), "the emptied folder is removed");
        assert!(wiki.pages.contains_key("new/place"));
        assert_eq!(
            fs::read_to_string(dir.join("index.md")).unwrap(),
            "# Home\n[[new/place]] [[new/place|it]] [a](new/place.md) [[other]]"
        );
        assert_eq!(wiki.backlinks_of("new/place"), ["index"]);

        wiki.delete_page("new/place").unwrap();
        assert!(!dir.join("new").exists());
        assert!(!wiki.pages.contains_key("new/place"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_page_becomes_a_folder_index_when_it_gets_a_child() {
        let dir = std::env::temp_dir().join(format!("wikibits-promote-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("index.md"),
            "# Home\n[[backlog]] [[backlog/history]]",
        )
        .unwrap();
        fs::write(dir.join("backlog.md"), "# Backlog").unwrap();
        let mut wiki = Wiki::load(&dir).unwrap();

        let moved = wiki
            .promote_ancestors_to_folders("backlog/history")
            .unwrap();
        assert_eq!(
            moved,
            vec![("backlog".to_string(), "backlog/index".to_string())]
        );
        assert!(dir.join("backlog/index.md").is_file());
        assert!(!dir.join("backlog.md").exists());
        wiki.reload().unwrap();
        assert_eq!(
            wiki.resolve("index", "backlog"),
            LinkTarget::Page("backlog/index".into())
        );
        assert!(
            wiki.promote_ancestors_to_folders("backlog/history")
                .unwrap()
                .is_empty()
        );
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
    fn unreferenced_images_go_to_trash() {
        let dir = std::env::temp_dir().join(format!("wikibits-images-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("assets")).unwrap();
        fs::create_dir_all(dir.join("old")).unwrap();
        fs::write(dir.join("index.md"), "# Home\n![used](assets/used.png)").unwrap();
        fs::write(dir.join("assets/used.png"), b"x").unwrap();
        fs::write(dir.join("assets/unused.png"), b"x").unwrap();
        fs::write(dir.join("old/stale.jpg"), b"x").unwrap();
        let wiki = Wiki::load(&dir).unwrap();
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
