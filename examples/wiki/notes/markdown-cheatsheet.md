# Markdown cheatsheet

A tour of what wikiBits renders.

## Text

Headings: `#` and `##` draw as a full-width band, `###` and deeper are underlined.

Plain, *emphasis*, **strong**, ~~struck~~ and `inline code`. A long paragraph wraps at the
column width, keeping words whole.

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
stacked in a column line up. Empty slots pick a column instead: `,, z(End)` puts `z` in the
third column. `id[label]`
is a box, `id(label)` a rounded pill, `id<label>` a diamond. Lines join ids with `-->`,
`<--`, `<->` or `---`, can be chained, and take a label after `|`. Lines between neighbouring
rows go straight down (fanning out and merging as needed); lines that skip rows run down a
channel on the right. A block that does not parse shows its source with the problem.

---

Back to [[/]] or on to [[getting-started]].
