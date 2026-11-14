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
- The page's title is the `# Heading` on its first line. If the page starts with anything
  else, the file name is used, even when a `# Heading` comes later
  (`my-wiki-page` shows as "My wiki page").
- `[[folder/page]]` links to a page by its path from the wiki root, without `.md`,
  and displays that page's title. `[[folder/page|label]]` shows your own text.
- `[[folder/page#Heading]]` opens the page scrolled to that heading (its text, any case, or
  its slug: `#my-heading`); `[[#Heading]]` jumps within the current page. `{#name}` anywhere
  in a page is a hidden anchor: it is not drawn, and `[[folder/page#name]]` scrolls to the
  line it sits on — so a link can point at a paragraph that has no heading. `{#name|text}`
  draws its text in place. In a table cell an anchor marks the row, and a cell can hold
  links to headings and anchors like any other text.
- `<mark>text</mark>` highlights the text in yellow (black on the terminal's yellow). It
  works within a paragraph, list item, heading or table cell; a `<mark>` left open ends
  there.
- The terminal window's title is `wiki-folder: Page title`, so a window list or a tab in a
  window group says which wiki and page it holds. The page title is kept whole; the folder
  name is shortened with `…` when the two pass 64 characters together, and left out when
  there is no room for it.
- Ordinary Markdown links to `.md` files work too; `http(s)` links open in the browser.
- A folder's `index.md` stands for the folder: the folder takes its title in the tree,
  opening the folder shows it, and `[[/projects]]` links to it. The root `index.md` is the
  wiki's home page (`[[/]]`). Index pages are only linked through their folder: a direct
  `[[projects/index]]` shows as a missing link, and `[[` completion offers the folder path.
- A folder without an `index.md` can keep its page in a file named after the folder
  instead, inside it or beside it: `recipes/recipes.md` (any case) or `recipes.md` next to
  `recipes/` (the layout Azure DevOps wikis use) stands for the folder in every way an index
  page would, so the tree shows one row, not a page and a folder. A file named after the
  wiki's own folder (`Wiki.md` in `~/Wiki`) is the home page. When a folder has more than
  one, `index.md` wins, then the file inside, then the file beside; the others are ordinary
  pages.
- wikiBits itself uses the file beside the folder: giving the page `backlog.md` a child
  creates `backlog/` next to it and leaves `backlog.md` where it is, untouched. It never
  creates an `index.md`; folders that already have one keep working as before.
- The pages column sorts each folder alphabetically by title until you reorder it with
  `J` / `K`. The order is kept in a hidden `.order` file in that folder: one page or
  subfolder name per line (`backlog`, without `.md` or `/`). Names it leaves out follow,
  alphabetically; renames, moves and deletes keep it up to date.

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
| `Y` | copy the current page's path (shown at the bottom right; clicking it there copies it too) |
| `V` | select text with the keyboard: arrows move, `Shift`+arrows extend, `y` copies, `Esc` ends; dragging with the mouse selects and copies too, and a double-click selects and copies a word |
| `h` / `l`, ← / → | collapse / expand a folder; in the page, scroll sideways when a diagram or code block is wider than it |
| `J` / `K` | move the selected page or folder down / up within its folder (outside the pages column: the current page) |
| `Ctrl+↑` / `Ctrl+↓` | move the page under the page or folder listed above it, as its last child (a plain page above stays where it is, beside the new folder, as the folder's page) / out of its folder, to just after the folder; links to the page are rewritten as for a rename |
| `b`, `Backspace` / `f` | back / forward, to the place on the page you left: after a link to a heading or anchor, `b` lands on that link, and a jump within the page is a step too |
| `H` | home page |
| `r` | reload |
| `?` | help |
| `q` | quit |

The mouse works too: click to open, wheel to scroll (over the pages or related column it scrolls
the list without moving the selection; a sideways wheel or trackpad swipe scrolls a wide page
sideways), right-click to go back.

## Tables and images

A table is drawn with its columns as wide as their widest cell. When that is wider than
the page, the widest text columns are squeezed and their text wraps. An image in a table
cell is drawn in the cell, the row grows to the picture's height, and the text in the other
cells wraps beside it — a picture on the left and its details on the right:

```markdown
| Axe | Weapon · silver |
|---|---|
| ![Axe](assets/axe.png) | **Bonus:** +4 strength · *This weapon increases your strength by 4.* |
```

A picture in a cell is bounded to half the page width so there is always room for the text.
Markdown insists on a header row; leave it empty (`| | |`) and neither it nor the rule under
it is drawn, so a table can be one row of picture and details.

Cells can span. A cell holding only `^^` merges with the cell above it, and one holding only
`<<` merges with the cell to its left — so a picture can sit beside two rows of text:

```markdown
| | |
|---|---|
| ![Axe](assets/axe.png) | **Weapon** · silver · **Bonus:** +4 strength |
| ^^ | *This weapon increases your strength by 4.* |
```
A `[[folder/page|label]]` link works in a cell: the pipe in a link never splits the cell. Any
other pipe inside a cell needs escaping (`\|`).

## Diagrams

A ```diaBits block draws a flowchart or a tree with box-drawing characters — no external tools:

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
stacked in a column line up. Empty slots pick a column instead: `,, z(End)` puts `z` in the
third column. `id[label]`
is a box, `id(label)` a rounded pill, `id<label>` a diamond; a label can hold commas
(`a[Hello, world]`) and breaks over lines at `\n` (`a[Go to the\npet store]`), making the
shape taller; a many-line diamond keeps its points on every row. Lines join ids with `-->`,
`<--`, `<->` or `---`, can be chained, and take a label after `|`. Lines between neighbouring
rows go straight down (fanning out and merging as needed). A line that skips rows runs
straight down too when its column is empty in between, or down its empty column and across
into the side of a shape in another column; otherwise it runs down a channel in a gap
between columns or beside the diagram, which widens to make room. A line never crosses a
shape: it only uses a gap it can reach along its rows, takes the gap where it crosses fewer
other lines, nests inside the longer lines around it, and shares a channel with lines it
shares no rows with. A line along a row that skips a shape loops under the row instead. A
line that leaves a shape on a side where another line arrives leaves from the shape's corner,
so the two never meet at one point. A block that does not parse shows its source with the
problem. A diagram wider than the page scrolls sideways (`h` / `l`, the arrows, or a sideways
wheel).

A line can be told where to leave and enter its shapes when the default route is not the one
you want. Put a side after the id, in brackets: `a(R) --> b(T)` leaves the right of `a` and
enters the top of `b`, with `T`, `B`, `L` or `R` for top, bottom, left or right. Each side
has three anchors, and `a(T:R) --> b(B:L)` picks one: `L`, `M` or `R` along the top and
bottom, `T`, `M` or `B` down the left and right; without one the line takes the middle.
Either end can be left plain (`a(R) --> b`) and takes whichever side makes the shorter line.

```diaBits
a(First)
b(Second)
c(Third)
d(Fourth)

a --> b --> c --> d
c(R) --> b(R) | inner right
d(R) --> a(R) | outer right
c(L:T) --> b(L) | inner left
c(L:B) --> a(L:T) | middle left
d(L) --> a(L:T) | outer left
```

These lines are drawn after the others, shortest first, each by the clear path with the
fewest corners, then the shortest: a longer line runs outside the shorter ones it spans, a
line crosses another only when going round would take more corners, and none runs through a
shape or a label. The diagram grows to give them room. Two lines told to use the same anchor
share it: the second runs straight into the stretch of the first that enters the shape, and
they reach it together (`outer left` and `middle left` above, each with two corners). An anchor that an ordinary line already uses gives way to the next
one on that side. Ids, sides and anchors are matched whatever their case: `A(l:t)` is
`a(L:T)`.

`diaType: tree` draws a tree instead: one shape per line, with leading dots saying how deep
it sits (one more than its parent), and the lines from a parent down and across to its
children drawn for you. Any number of roots (no dots) can follow each other:

```diaBits
diaType: tree
root(Live Game State)
.vitals[Hero Vitals]
..hp[Hit Points]
..mp[Mana]
.party[Party Roster]
.map[Map and Viewport]
```

A label can hold `[[links]]`, in either kind of diagram: `a[Go to [[pets/store]]]` shows the
page's title in the box, `b([[pets/store|the store]])` your own text, and a line's label can
link too (`a --> b | see [[pets/store]]`). The link is selectable and opens like any other,
counts as a link to that page (backlinks, the wiki report), and is rewritten when the page
is renamed.

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
`backlog.md` is a plain page creates the folder `backlog/` beside it: `backlog.md` is not
moved or changed, links to `backlog` keep working, and the tree shows the page and its folder
as one row. When the last child is deleted or moved away the empty folder is removed and the
page is a plain page again. A folder from before this rule that is left with nothing but
`index.md` still becomes the plain page `folder.md`, and links written as `[[backlog/index]]`
are rewritten to `[[backlog]]`. `R` renames or moves the page (type
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
with `n`/`p` (or click it) and press `Enter` to see it at full size in a pop-up. `+`/`-` zoom in
and out around the middle (`f` fits the whole image on the screen, `0` is back to full size);
`Ctrl`+wheel or a double-click zooms at the pointer. Pan with `h`/`j`/`k`/`l`, the arrows, the
mouse wheel or by dragging the image; `Esc` closes it. If images come out the wrong size,
`wikibits --probe` prints what the terminal reports about protocol and cell size. Inside tmux,
images are drawn with block characters.

## Web viewer

`viewer/wikibits-viewer.html` is a read-only viewer for a wiki kept in a GitHub repository:
one self-contained file, no server or build step. Open it in a browser and paste the link to
the wiki's folder (`https://github.com/owner/repo/tree/main/wiki`), or open it as
`wikibits-viewer.html?wiki=<that link>` to go straight there; `#/folder/page` after it opens
a page. It shows the same three columns (tree, page, outline / links / backlinks) and follows
the same rules: titles, `[[links]]`, index pages, `.order`, spanning table cells and diaBits
diagrams. `s` searches, `v` shows the source, `w` the wiki report, `?` the keys.

A private repository needs a fine-grained GitHub token with read-only Contents access (it is
sent only to `api.github.com`); a token also lifts GitHub's limit of 60 requests an hour for
anonymous use. Pages are cached in the browser, so reopening an unchanged wiki costs one
request. "Open a folder on this computer" reads a local wiki folder instead. Raw HTML in pages
is shown as text, so a wiki cannot run scripts in the viewer.

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
