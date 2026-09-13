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
  wiki's home page (`[[/]]`).

## Keys

| Key | Action |
|-----|--------|
| `Tab` / `Shift-Tab`, `1` `2` `3` | switch column |
| `j` / `k`, arrows, `PgUp` / `PgDn`, `g` / `G` | move / scroll |
| `Enter` | open the selected page, link or folder; open the selected image full size |
| `n` / `p` | select the next / previous link or image on the page |
| `v` | toggle between the rendered page and its Markdown source |
| `/` | find in the page: matches highlight as you type, `Enter`/`Up` step through them |
| `s` | search the wiki: live list of matching pages, `Enter` opens one at the match |
| `e` | edit the page in place; `N` creates a new page by path; `Enter` on a missing link offers to create it |
| `R` / `D` | rename (move) the page, updating links to it everywhere / delete it, after confirmation |
| `h` / `l` | collapse / expand a folder |
| `b`, `Backspace` / `f` | back / forward |
| `H` | home page |
| `r` | reload |
| `?` | help |
| `q` | quit |

The mouse works too: click to open, wheel to scroll, right-click to go back.

## Editing

`e` opens the page's Markdown in the middle pane as a plain editor: type to insert, arrows,
`Home`/`End`, `PgUp`/`PgDn`, `Ctrl+Left/Right` by word, `Shift`+arrows to select, `Ctrl-A`
select all, `Ctrl-C`/`Ctrl-X`/`Ctrl-V` with the system clipboard (`wl-copy`/`wl-paste`; the
terminal's own paste works too), `Ctrl-Z`/`Ctrl-Y` undo and redo, `Tab` two spaces, and long
lines soft-wrap. Typing `[[` pops up matching pages as you type; `Enter` or `Tab` inserts the
chosen page and closes the link with `]]`. `Ctrl-S` saves and returns to the rendered page; `Esc` returns, asking first
if there are unsaved changes. `N` creates a page from a `folder/name` path, and following a
link to a page that does not exist offers to create it. `R` renames or moves the page (type
the new `folder/name`; every `[[link]]` to it in other pages is rewritten, and emptied folders
are removed) and `D` deletes it after a confirmation that mentions how many pages link to it.

## Images

Images render through the terminal's graphics protocol (sixel in foot), scaled down to fit the
page area when they are too big (never scaled up). Each image sits in a thin frame; select it
with `n`/`p` (or click it) and press `Enter` to see it at full size in a pop-up, scrolling with
`h`/`j`/`k`/`l`, the arrows, the mouse wheel or by dragging it; `Esc` closes it. If images come out the wrong
size, `wikibits --probe` prints what the terminal reports about protocol and cell size. Inside
tmux, images are drawn with block characters.

## Build and install

Needs a Rust toolchain (`rustup default stable`).

```sh
bin/install     # cargo build --release, then copies the binary to ~/.local/bin
cargo test      # unit tests for the index and the Markdown renderer
```
