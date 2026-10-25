# Markdown cheatsheet

A tour of what wikiBits renders.

## Text

Headings: `#` and `##` draw as a full-width band, `###` and deeper are underlined.

Plain, *emphasis*, **strong**, ~~struck~~ and `inline code`. A long paragraph wraps at the
column width, keeping words whole.

A `<br>` or a literal `\n` breaks the line<br>like this,\nor this.

## Lists

- Bullets
- With nesting
  - Like this
  - And this
- Back out (a blank line separates this from the nested items above)

1. Numbered
2. Lists
3. Count up

- [x] A finished task
- [ ] An open task — select it with `n` and press `x` to tick it

## Links to a place on a page

`[[getting-started#Keys]]` opens a page at a heading (its text, or its slug `#keys`), and
`[[#Table]]` jumps within this page. A hidden `{#name}` anchor marks a line without drawing
anything: this paragraph carries one {#anchor-demo}, so `[[notes/markdown-cheatsheet#anchor-demo]]`
lands here even though it has no heading. {#shown-demo|An anchor with text} draws its text
and can be linked to as `#shown-demo`.

## Quote

> Wikis are the original hypertext for people who just want to write.

## Code

```rust
fn main() {
    println!("hello, wiki");
}
```

## Table

| Column | Meaning |
|--------|---------|
| Pages | the folder tree |
| Related | outline, links and backlinks |
| Breaks | `<br>` or `\n` start<br>a new line\nin a cell |

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
is a box, `id(label)` a rounded pill, `id<label>` a diamond. Lines join ids with `-->`,
`<--`, `<->` or `---`, can be chained, and take a label after `|`. Lines between neighbouring
rows go straight down (fanning out and merging as needed); lines that skip rows run down
channels beside the diagram: each takes the side where it crosses fewer other lines, nests
inside the longer lines around it, and shares a channel with lines it shares no rows with. A
line that leaves a shape on a side where another line arrives leaves from the shape's corner,
so the two never meet at one point. A block that does not parse shows its source with the
problem. A diagram wider than the page scrolls sideways (`h` / `l`, the arrows, or a sideways
wheel).

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

---

Back to [[/]] or on to [[getting-started]].
