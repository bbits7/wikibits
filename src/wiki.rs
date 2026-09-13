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
    fn normalize_collapses_dots() {
        assert_eq!(normalize("a/./b/../c"), "a/c");
        assert_eq!(normalize("/x//y/"), "x/y");
    }
}
