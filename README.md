# wikiBits

A terminal wiki built on a folder of plain Markdown files.

```
wikibits            # opens ~/Wiki
wikibits ~/notes    # opens another folder
wikibits -p projects/calcbits
```

Three columns: the folder tree on the left (current page highlighted), the rendered
page in the middle with breadcrumbs in its frame, and on the right "On this page" (the
headings as a foldable outline; `Enter` jumps to one), the links found on the page, and
the backlinks. Images render inline in terminals that support sixel (foot),
kitty or iTerm2 graphics. Files are re-read when they change on disk.

## Pages and links

- A page is a `.md` file; folders group pages.
- The page's title is its first `# Heading`. Without one, the file name is used
  (`my-wiki-page` shows as "My wiki page").
- `[[folder/page]]` links to a page by its path from the wiki root, without `.md`,
  and displays that page's title. `[[folder/page|label]]` shows your own text.
- Ordinary Markdown links to `.md` files work too; `http(s)` links open in the browser.
- A folder's `index.md` stands for the folder: the folder takes its title in the tree,
  opening the folder shows it, and `[[/projects]]` links to it. The root `index.md` is the
  wiki's home page (`[[/]]`). Index pages are only linked through their folder: a direct
  `[[projects/index]]` shows as a missing link, and `[[` completion offers the folder path.

## Keys

| Key | Action |
|-----|--------|
| `Tab` / `Shift-Tab`, `1` `2` `3` | switch column |
| `j` / `k`, arrows, `PgUp` / `PgDn`, `g` / `G` | move / scroll |
| `Enter` | open the selected page, link or folder; open the selected image full size |
| `n` / `p` | select the next / previous link, image or task on the page |
| `x` | tick or untick the selected task (`Enter` or a click on the checkbox does too) |
| `v` | toggle between the rendered page and its Markdown source |
| `/` | find in the page: matches highlight as you type, `Enter`/`Up` step through them |
| `s` | search the wiki (all words must match): live list of pages, `Enter` opens one at the match |
| `e` | edit the page in place; `N` creates a new page by path (prefilled with the current folder); `C` creates a child of the current page; `Enter` on a missing link offers to create it |
| `R` / `D` | rename (move) the page, updating links to it everywhere / move it to `.trash/`, after confirmation |
| `w` | wiki report: broken links, orphan pages and recently changed pages (`Enter` opens one) |
| `y` | copy a `[[link]]` to the current page to the clipboard (or the selected text) |
| `V` | select text with the keyboard: arrows move, `Shift`+arrows extend, `y` copies, `Esc` ends; dragging with the mouse selects and copies too |
| `h` / `l` | collapse / expand a folder |
| `b`, `Backspace` / `f` | back / forward |
| `H` | home page |
| `r` | reload |
| `?` | help |
| `q` | quit |

The mouse works too: click to open, wheel to scroll, right-click to go back.

## Diagrams

A ```diaBits block draws a flowchart with box-drawing characters — no external tools:

```diaBits
diaType: flowchart
start(Start)
a[Go to the pet store]
b<Cats or dogs?>
c[Buy a cat], d[Buy a dog]
end(End)

start --> a --> b
b --> c | Cats
b --> d | Dogs
c --> end
d --> end
```

Each line of shapes is a row, top to bottom; shapes on one line share the row. The fullest
row sets the number of columns, each column is as wide as its widest shape, and every shape
is centred in its column (a row with fewer shapes spreads them over the columns), so shapes
stacked in a column line up. `id[label]`
is a box, `id(label)` a rounded pill, `id<label>` a diamond. Lines join ids with `-->`,
`<--`, `<->` or `---`, can be chained, and take a label after `|`. Lines between neighbouring
rows go straight down (fanning out and merging as needed); lines that skip rows run down a
channel on the right. A block that does not parse shows its source with the problem.

## Editing

`e` opens the page's Markdown in the middle pane as a plain editor: type to insert, arrows,
`Home`/`End`, `PgUp`/`PgDn`, `Ctrl+Left/Right` by word, `Shift`+arrows to select, `Ctrl-A`
select all, `Ctrl-C`/`Ctrl-X`/`Ctrl-V` with the system clipboard (`wl-copy`/`wl-paste`; the
terminal's own paste works too), `Ctrl-Z`/`Ctrl-Y` undo and redo, `Tab` two spaces, and long
lines soft-wrap. Headings, list markers, links and code are coloured. `Enter` on a list item
starts the next item (numbers count up, task boxes start unticked); `Enter` on an empty item
ends the list. `Ctrl-F` finds in the text (`Enter`/`Up` step through matches); `Ctrl-R` then
asks for a replacement (`Enter` replaces and moves on, `Ctrl-A` replaces all). Typing `[[` pops up matching pages as you type; `Enter` or `Tab` inserts the
chosen page and closes the link with `]]`. `Ctrl-P` lists the images in the wiki folder
(type to filter) and inserts `![name](path)` for the chosen one; pasting an image with `Ctrl-V`
saves it as `assets/<page>-<n>.png` and inserts the link. After a save or a page delete,
images no page mentions any more are moved to `.trash/` in the wiki folder (never deleted
outright), and the status bar says which. `Ctrl-S` saves and returns to the rendered page; `Esc` returns, asking first
if there are unsaved changes. `N` creates a page from a `folder/name` path, and following a
link to a page that does not exist offers to create it. Creating `backlog/history` when
`backlog.md` is a plain page turns it into a folder: the page becomes `backlog/index.md`, so
links to `backlog` keep working and the tree shows it as a folder. The reverse happens too:
after a delete or rename, a folder left with nothing but `index.md` becomes the plain page
again, and links written as `[[backlog/index]]` are rewritten to `[[backlog]]`. `R` renames or moves the page (type
the new `folder/name`; every `[[link]]` to it in other pages is rewritten, and emptied folders
are removed) and `D` moves it to `.trash/` after a confirmation that mentions how many pages
link to it.

## History

If the wiki folder is a git repository, every save, create, rename and delete is committed
automatically ("Edit projects/wikibits"), so `git log` and `git diff` in the wiki folder are
its history and a push is its backup. Set it up once with `git init` there (add `.trash/` to
`.gitignore`). Moving a page to another folder rewrites the relative image and `.md` links
inside it, so they keep pointing at the same files.

## Images

Images render through the terminal's graphics protocol (sixel in foot), scaled down to fit the
page area when they are too big (never scaled up). Each image sits in a thin frame; select it
with `n`/`p` (or click it) and press `Enter` to see it at full size in a pop-up, scrolling with
`h`/`j`/`k`/`l`, the arrows, the mouse wheel or by dragging it; `Esc` closes it. If images come out the wrong
size, `wikibits --probe` prints what the terminal reports about protocol and cell size. Inside
tmux, images are drawn with block characters.

## Install

**Prebuilt (Linux x86_64):** download `wikibits-<version>-x86_64-linux.tar.gz` from the
[releases page](https://github.com/bbits7/wikibits/releases), unpack it and run
`bin/install`. It puts the (statically linked) binary in `~/.local/bin` and adds a launcher
entry and icon. Then run `wikibits`: if `~/Wiki` does not exist it offers to create it with a
few starter pages.

**With Rust:** `cargo install --git https://github.com/bbits7/wikibits`, or clone and run
`bin/install` (builds with `cargo build --release` first). `cargo test` runs the unit tests.

**What it expects:** a terminal that draws images with the sixel or kitty protocol for
inline pictures (foot, kitty, WezTerm, Ghostty…; others get block characters), `wl-clipboard`
(`wl-copy` / `wl-paste`) for copy and paste on Wayland, `xdg-open` for web links, and `git`
if you want the automatic history. Built and tested on Omarchy (Arch Linux, Hyprland).
