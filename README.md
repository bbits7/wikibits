# wikiBits

A terminal wiki built on a folder of plain Markdown files.

```
wikibits            # opens ~/Wiki
wikibits ~/notes    # opens another folder
wikibits -p projects/calcbits
```

Three columns: the folder tree on the left (current page highlighted), the rendered
page in the middle with breadcrumbs above it, and on the right the backlinks plus the
links found on the page. Images render inline in terminals that support sixel (foot),
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
| `Enter` | open the selected page, link or folder |
| `n` / `p` | select the next / previous link on the page |
| `v` | toggle between the rendered page and its Markdown source |
| `h` / `l` | collapse / expand a folder |
| `b`, `Backspace` / `f` | back / forward |
| `H` | home page |
| `r` | reload |
| `?` | help |
| `q` | quit |

The mouse works too: click to open, wheel to scroll, right-click to go back.

## Images

Images render through the terminal's graphics protocol (sixel in foot). If they come out the
wrong size, `wikibits --probe` prints what the terminal reports about protocol and cell size.
Inside tmux, images are drawn with block characters.

## Build and install

Needs a Rust toolchain (`rustup default stable`).

```sh
bin/install     # cargo build --release, then copies the binary to ~/.local/bin
cargo test      # unit tests for the index and the Markdown renderer
```
