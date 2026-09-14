//! diaBits: a small flowchart language rendered with box-drawing characters.
//!
//! ```text
//! diaType: flowchart
//! start(Start)
//! a[Go to the pet store]
//! b<Cats or dogs?>
//! c[Buy a cat], d[Buy a dog]
//! end(End)
//!
//! start --> a --> b
//! b --> c | Cats
//! b --> d | Dogs
//! c --> end
//! d --> end
//! ```
//!
//! Each line of shapes is a row of the diagram, top to bottom; shapes on one line share the
//! row. `id[label]` is a box, `id(label)` a pill, `id<label>` a diamond. Lines are `-->`,
//! `<--`, `<->` or `---` between ids, chainable, with `| label` at the end.

use std::collections::{BTreeMap, HashMap};

use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Box,
    Pill,
    Diamond,
}

#[derive(Debug)]
struct Node {
    label: String,
    shape: Shape,
    row: usize,
    /// Position in the row's comma-separated list, counting empty slots.
    slot: usize,
    /// Left column and top row on the grid, and size.
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Node {
    fn center(&self) -> usize {
        self.x + self.w / 2
    }
    fn middle(&self) -> usize {
        self.y + self.h / 2
    }
}

#[derive(Debug)]
struct Edge {
    from: usize,
    to: usize,
    arrow_from: bool,
    arrow_to: bool,
    label: Option<String>,
}

impl Edge {
    /// The two ends ordered top row first, with whether each end gets an arrow head.
    fn vertical(&self, nodes: &[Node]) -> (usize, usize, bool, bool) {
        if nodes[self.from].row <= nodes[self.to].row {
            (self.from, self.to, self.arrow_from, self.arrow_to)
        } else {
            (self.to, self.from, self.arrow_to, self.arrow_from)
        }
    }
}

struct Diagram {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    rows: Vec<Vec<usize>>,
    /// For each row: `Some(slot count)` when the line used empty slots (`,, z(End)`) to put
    /// shapes in explicit columns; `None` when its shapes spread over the columns.
    explicit: Vec<Option<usize>>,
}

/// Columns between shapes on the same row.
const ROW_GAP: usize = 4;

/// Whether an edge is routed down a channel on the right instead of between its rows:
/// edges that skip rows, edges between non-neighbouring shapes of one row, and edges that
/// point back up (or both ways) without lining up, which would be ambiguous on a shared bus.
fn goes_sideways(d: &Diagram, e: usize) -> bool {
    let (u, l, au, _) = d.edges[e].vertical(&d.nodes);
    let (ru, rl) = (d.nodes[u].row, d.nodes[l].row);
    if ru == rl {
        return !neighbours(d, u, l);
    }
    rl > ru + 1 || (au && d.nodes[u].center() != d.nodes[l].center())
}

/// Render a diaBits block to text lines, or explain what is wrong with it.
pub fn render(source: &str) -> Result<Vec<String>, String> {
    let mut diagram = parse(source)?;
    layout(&mut diagram);
    Ok(draw(&diagram))
}

// ----- parsing ------------------------------------------------------------------------

fn parse(source: &str) -> Result<Diagram, String> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut edges = Vec::new();
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut explicit: Vec<Option<usize>> = Vec::new();
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut seen_type = false;
    for (number, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let at = number + 1;
        if let Some(kind) = line.strip_prefix("diaType:") {
            if seen_type {
                return Err(format!("line {at}: diaType given twice"));
            }
            seen_type = true;
            if kind.trim() != "flowchart" {
                return Err(format!(
                    "line {at}: unknown diaType '{}' (only flowchart for now)",
                    kind.trim()
                ));
            }
            continue;
        }
        if line.contains("-->")
            || line.contains("<--")
            || line.contains("<->")
            || line.contains("---")
        {
            let (chain, label) = match line.split_once('|') {
                Some((chain, label)) => (chain, Some(label.trim().to_string())),
                None => (line, None),
            };
            let tokens: Vec<&str> = chain.split_whitespace().collect();
            if tokens.len() < 3 || tokens.len().is_multiple_of(2) {
                return Err(format!("line {at}: expected 'id --> id' (chains allowed)"));
            }
            let count = tokens.len() / 2;
            for i in 0..count {
                let (from, op, to) = (tokens[2 * i], tokens[2 * i + 1], tokens[2 * i + 2]);
                let (arrow_from, arrow_to) = match op {
                    "-->" => (false, true),
                    "<--" => (true, false),
                    "<->" => (true, true),
                    "---" => (false, false),
                    other => {
                        return Err(format!(
                            "line {at}: '{other}' is not one of --> <-- <-> ---"
                        ));
                    }
                };
                let lookup = |id: &str| {
                    ids.get(id).copied().ok_or_else(|| {
                        format!("line {at}: no shape named '{id}' (define it first)")
                    })
                };
                let (from, to) = (lookup(from)?, lookup(to)?);
                if from == to {
                    return Err(format!("line {at}: a line from a shape to itself"));
                }
                edges.push(Edge {
                    from,
                    to,
                    arrow_from,
                    arrow_to,
                    // A label on a chain goes on its last line.
                    label: if i + 1 == count { label.clone() } else { None },
                });
            }
            continue;
        }
        let mut row = Vec::new();
        let mut empty_slots = false;
        let items: Vec<&str> = line.split(',').map(str::trim).collect();
        for (slot, item) in items.iter().enumerate() {
            if item.is_empty() {
                empty_slots = true;
                continue;
            }
            let (id, label, shape) = parse_shape(item).ok_or_else(|| {
                format!("line {at}: '{item}' is not id[label], id(label) or id<label>")
            })?;
            if ids.contains_key(id) {
                return Err(format!("line {at}: '{id}' is defined twice"));
            }
            ids.insert(id.to_string(), nodes.len());
            row.push(nodes.len());
            nodes.push(Node {
                label: label.trim().to_string(),
                shape,
                row: rows.len(),
                slot,
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            });
        }
        if row.is_empty() {
            return Err(format!("line {at}: nothing defined"));
        }
        explicit.push(empty_slots.then_some(items.len()));
        rows.push(row);
    }
    if nodes.is_empty() {
        return Err("no shapes: add lines like id[label]".to_string());
    }
    Ok(Diagram {
        nodes,
        edges,
        rows,
        explicit,
    })
}

fn parse_shape(item: &str) -> Option<(&str, &str, Shape)> {
    let open = item.find(['[', '(', '<'])?;
    let id = &item[..open];
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    let (close, shape) = match &item[open..open + 1] {
        "[" => (']', Shape::Box),
        "(" => (')', Shape::Pill),
        _ => ('>', Shape::Diamond),
    };
    let body = &item[open + 1..];
    let label = body.strip_suffix(close)?;
    (!label.trim().is_empty()).then_some((id, label, shape))
}

// ----- layout -------------------------------------------------------------------------

fn layout(d: &mut Diagram) {
    for node in &mut d.nodes {
        let lw = node.label.width();
        node.w = match node.shape {
            Shape::Box | Shape::Pill => lw + 4,
            Shape::Diamond => lw + 6,
        };
        node.h = 3;
    }
    // A grid of columns: as many as the fullest row has shapes (or slots). A row with fewer
    // shapes spreads them over the columns (one shape spans them all) unless it used empty
    // slots to pick columns, and every shape is centred in the columns it spans, so shapes
    // stacked in a column line up.
    let columns = d
        .rows
        .iter()
        .zip(&d.explicit)
        .map(|(row, explicit)| explicit.unwrap_or(row.len()))
        .max()
        .unwrap_or(1);
    let node_span: Vec<(usize, usize)> = d
        .nodes
        .iter()
        .map(|n| match d.explicit[n.row] {
            Some(_) => (n.slot, n.slot + 1),
            None => {
                let len = d.rows[n.row].len();
                let i = d.rows[n.row]
                    .iter()
                    .position(|&m| d.nodes[m].slot == n.slot)
                    .unwrap_or(0);
                (i * columns / len, (i + 1) * columns / len)
            }
        })
        .collect();
    let mut widths = vec![0usize; columns];
    // The gap between two columns widens to fit the label of a line drawn across it.
    let mut gaps = vec![ROW_GAP; columns.saturating_sub(1)];
    for e in &d.edges {
        let (a, b) = (&d.nodes[e.from], &d.nodes[e.to]);
        let Some(label) = &e.label else { continue };
        if a.row != b.row {
            continue;
        }
        let row = &d.rows[a.row];
        let (Some(ia), Some(ib)) = (
            row.iter().position(|&n| n == e.from),
            row.iter().position(|&n| n == e.to),
        ) else {
            continue;
        };
        if ia.abs_diff(ib) != 1 {
            continue;
        }
        let (left, right) = if ia < ib {
            (e.from, e.to)
        } else {
            (e.to, e.from)
        };
        let (_, c1) = node_span[left];
        if node_span[right].0 == c1
            && let Some(gap) = gaps.get_mut(c1 - 1)
        {
            *gap = (*gap).max(label.width() + 2);
        }
    }
    let gap_sum = |gaps: &[usize], c0: usize, c1: usize| gaps[c0..c1 - 1].iter().sum::<usize>();
    // Single-column shapes set the column widths; wider spanning shapes stretch their span.
    for row in &d.rows {
        for &n in row {
            let (c0, c1) = node_span[n];
            if c1 - c0 == 1 {
                widths[c0] = widths[c0].max(d.nodes[n].w);
            }
        }
    }
    for row in &d.rows {
        for &n in row {
            let (c0, c1) = node_span[n];
            let have = widths[c0..c1].iter().sum::<usize>() + gap_sum(&gaps, c0, c1);
            if d.nodes[n].w > have {
                let extra = d.nodes[n].w - have;
                let each = extra.div_ceil(c1 - c0);
                for w in &mut widths[c0..c1] {
                    *w += each;
                }
            }
        }
    }
    let mut starts = Vec::with_capacity(columns);
    let mut x = 0;
    for (c, w) in widths.iter().enumerate() {
        starts.push(x);
        x += w + gaps.get(c).copied().unwrap_or(0);
    }
    for row in &d.rows {
        for &n in row {
            let (c0, c1) = node_span[n];
            let left = starts[c0];
            let total = widths[c0..c1].iter().sum::<usize>() + gap_sum(&gaps, c0, c1);
            // Place by centre, not by left edge, so shapes of different widths in one
            // column share the same centre column exactly.
            let centre = left + total / 2;
            d.nodes[n].x = centre - d.nodes[n].w / 2;
        }
    }
    // Vertical positions: each gap between rows is as tall as its edges need.
    let mut y = 0;
    for r in 0..d.rows.len() {
        for &n in &d.rows[r] {
            d.nodes[n].y = y;
        }
        y += 3;
        if r + 1 < d.rows.len() {
            y += region_plan(d, r).height;
        }
    }
}

/// A bus: a horizontal line between two rows joining a set of upper nodes to lower nodes.
struct Bus {
    uppers: Vec<usize>,
    lowers: Vec<usize>,
    edges: Vec<usize>,
    /// Which bus row of the region it is drawn on.
    level: usize,
}

struct RegionPlan {
    /// Edges drawn straight down (single edge, ends lined up).
    direct: Vec<usize>,
    buses: Vec<Bus>,
    levels: usize,
    height: usize,
}

/// Plan the edges between row `r` and row `r + 1`.
fn region_plan(d: &Diagram, r: usize) -> RegionPlan {
    let edges: Vec<usize> = (0..d.edges.len())
        .filter(|&e| {
            let (u, l, _, _) = d.edges[e].vertical(&d.nodes);
            d.nodes[u].row == r && d.nodes[l].row == r + 1 && !goes_sideways(d, e)
        })
        .collect();
    // Union-find over the nodes the edges touch, so fan-outs and merges share a bus.
    let mut parent: BTreeMap<usize, usize> = BTreeMap::new();
    fn find(parent: &mut BTreeMap<usize, usize>, n: usize) -> usize {
        let p = *parent.entry(n).or_insert(n);
        if p == n {
            n
        } else {
            let root = find(parent, p);
            parent.insert(n, root);
            root
        }
    }
    for &e in &edges {
        let (u, l, _, _) = d.edges[e].vertical(&d.nodes);
        let (ru, rl) = (find(&mut parent, u), find(&mut parent, l));
        if ru != rl {
            parent.insert(ru, rl);
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &e in &edges {
        let (u, _, _, _) = d.edges[e].vertical(&d.nodes);
        let root = find(&mut parent, u);
        groups.entry(root).or_default().push(e);
    }
    let mut direct = Vec::new();
    let mut buses: Vec<Bus> = Vec::new();
    for group in groups.into_values() {
        if group.len() == 1 {
            let (u, l, _, _) = d.edges[group[0]].vertical(&d.nodes);
            if d.nodes[u].center() == d.nodes[l].center() {
                direct.push(group[0]);
                continue;
            }
        }
        let mut uppers: Vec<usize> = Vec::new();
        let mut lowers: Vec<usize> = Vec::new();
        for &e in &group {
            let (u, l, _, _) = d.edges[e].vertical(&d.nodes);
            if !uppers.contains(&u) {
                uppers.push(u);
            }
            if !lowers.contains(&l) {
                lowers.push(l);
            }
        }
        buses.push(Bus {
            uppers,
            lowers,
            edges: group,
            level: 0,
        });
    }
    // Buses whose spans overlap go on separate rows.
    let span = |b: &Bus| {
        let xs = b
            .uppers
            .iter()
            .chain(&b.lowers)
            .map(|&n| d.nodes[n].center());
        (xs.clone().min().unwrap_or(0), xs.max().unwrap_or(0))
    };
    let mut levels = 0;
    let mut placed: Vec<(usize, usize, usize)> = Vec::new();
    buses.sort_by_key(|b| span(b).0);
    for bus in &mut buses {
        let (lo, hi) = span(bus);
        let mut level = 0;
        while placed
            .iter()
            .any(|&(l, plo, phi)| l == level && lo <= phi + 1 && plo <= hi + 1)
        {
            level += 1;
        }
        bus.level = level;
        levels = levels.max(level + 1);
        placed.push((level, lo, hi));
    }
    let labelled = edges.iter().any(|&e| d.edges[e].label.is_some());
    let two_way = edges.iter().any(|&e| {
        let (_, _, au, _) = d.edges[e].vertical(&d.nodes);
        au
    });
    // Rows: exit (│ or ▲), bus rows, entry (│ label), arrow (▼ or │). Direct unlabelled
    // edges only need the exit and arrow rows; an upward arrow head needs its own row.
    let mut height = 2;
    if levels > 0 || labelled {
        height = levels + 3;
    }
    if two_way {
        height = height.max(3);
    }
    RegionPlan {
        direct,
        buses,
        levels,
        height,
    }
}

// ----- drawing ------------------------------------------------------------------------

struct Grid {
    cells: Vec<Vec<char>>,
}

impl Grid {
    fn new(w: usize, h: usize) -> Grid {
        Grid {
            cells: vec![vec![' '; w.max(1)]; h.max(1)],
        }
    }

    /// Place a character, turning a crossing of `─` and `│` into `┼`.
    fn put(&mut self, x: usize, y: usize, c: char) {
        if y >= self.cells.len() {
            return;
        }
        if x >= self.cells[y].len() {
            let extra = x + 1 - self.cells[y].len();
            for row in &mut self.cells {
                row.extend(std::iter::repeat_n(' ', extra));
            }
        }
        let cell = &mut self.cells[y][x];
        *cell = match (*cell, c) {
            ('─', '│') | ('│', '─') => '┼',
            (' ', c) => c,
            (_, c) if c != '─' && c != '│' => c,
            (old, _) => old,
        };
    }

    fn text(&mut self, x: usize, y: usize, text: &str) {
        let mut col = x;
        for c in text.chars() {
            self.put(col, y, c);
            col += UnicodeWidthStr::width(c.to_string().as_str()).max(1);
        }
    }

    fn lines(&self) -> Vec<String> {
        self.cells
            .iter()
            .map(|row| row.iter().collect::<String>().trim_end().to_string())
            .collect()
    }
}

fn draw(d: &Diagram) -> Vec<String> {
    let content = d.nodes.iter().map(|n| n.x + n.w).max().unwrap_or(0);
    let height = d.nodes.iter().map(|n| n.y + n.h).max().unwrap_or(0);
    // Edges that skip rows, or join non-neighbouring shapes of a row, use channels on the
    // right; each gets its own column.
    let side: Vec<usize> = (0..d.edges.len())
        .filter(|&e| goes_sideways(d, e))
        .collect();
    let label_room = side
        .iter()
        .filter_map(|&e| d.edges[e].label.as_deref().map(UnicodeWidthStr::width))
        .max()
        .unwrap_or(0);
    let channel = |k: usize| content + 2 + k * (3 + label_room);
    let width = if side.is_empty() {
        content
    } else {
        channel(side.len() - 1) + 1 + label_room
    };
    let mut g = Grid::new(width, height);

    for node in &d.nodes {
        draw_shape(&mut g, node);
    }
    for r in 0..d.rows.len().saturating_sub(1) {
        draw_region(&mut g, d, r);
    }
    for e in 0..d.edges.len() {
        let (u, l, au, al) = d.edges[e].vertical(&d.nodes);
        if d.nodes[u].row == d.nodes[l].row && neighbours(d, u, l) {
            draw_same_row(&mut g, d, u, l, au, al, d.edges[e].label.as_deref());
        }
    }
    for (k, &e) in side.iter().enumerate() {
        draw_side(&mut g, d, e, channel(k));
    }
    g.lines()
}

fn neighbours(d: &Diagram, a: usize, b: usize) -> bool {
    let row = &d.rows[d.nodes[a].row];
    let (ia, ib) = (
        row.iter().position(|&n| n == a),
        row.iter().position(|&n| n == b),
    );
    matches!((ia, ib), (Some(ia), Some(ib)) if ia.abs_diff(ib) == 1)
}

fn draw_shape(g: &mut Grid, n: &Node) {
    let inner = n.w - 2;
    let pad = |s: &str, width: usize| {
        let extra = width.saturating_sub(s.width());
        let left = extra / 2;
        format!("{}{}{}", " ".repeat(left), s, " ".repeat(extra - left))
    };
    match n.shape {
        Shape::Box | Shape::Pill => {
            let (tl, tr, bl, br) = if n.shape == Shape::Box {
                ('┌', '┐', '└', '┘')
            } else {
                ('╭', '╮', '╰', '╯')
            };
            g.text(n.x, n.y, &format!("{tl}{}{tr}", "─".repeat(inner)));
            g.text(n.x, n.y + 1, &format!("│{}│", pad(&n.label, inner)));
            g.text(n.x, n.y + 2, &format!("{bl}{}{br}", "─".repeat(inner)));
        }
        Shape::Diamond => {
            let inner = n.w - 4;
            g.text(n.x, n.y, &format!(" ╱{}╲", "‾".repeat(inner)));
            g.text(n.x, n.y + 1, &format!("⟨{}⟩", pad(&n.label, n.w - 2)));
            g.text(n.x, n.y + 2, &format!(" ╲{}╱", "_".repeat(inner)));
        }
    }
}

fn draw_region(g: &mut Grid, d: &Diagram, r: usize) {
    let plan = region_plan(d, r);
    let top = d.rows[r]
        .iter()
        .map(|&n| d.nodes[n].y + d.nodes[n].h)
        .max()
        .unwrap_or(0);
    let bottom = top + plan.height; // first row of the lower shapes
    let arrow_row = bottom - 1;
    let entry_row = if plan.levels > 0 || plan.height > 2 {
        bottom - 2
    } else {
        top
    };

    for &e in &plan.direct {
        let (u, l, au, al) = d.edges[e].vertical(&d.nodes);
        let x = d.nodes[u].center();
        for y in top..bottom {
            g.put(x, y, '│');
        }
        if au {
            g.put(x, top, '▲');
        }
        g.put(x, arrow_row, if al { '▼' } else { '│' });
        if let Some(label) = &d.edges[e].label {
            g.text(x + 2, entry_row, label);
        }
        let _ = l;
    }

    for bus in &plan.buses {
        let bus_row = top + 1 + bus.level;
        let xs: Vec<usize> = bus
            .uppers
            .iter()
            .chain(&bus.lowers)
            .map(|&n| d.nodes[n].center())
            .collect();
        let (lo, hi) = (
            xs.iter().copied().min().unwrap_or(0),
            xs.iter().copied().max().unwrap_or(0),
        );
        for x in lo..=hi {
            g.put(x, bus_row, '─');
        }
        for &u in &bus.uppers {
            let x = d.nodes[u].center();
            for y in top..bus_row {
                g.put(x, y, '│');
            }
            let arrow_up = bus
                .edges
                .iter()
                .any(|&e| d.edges[e].vertical(&d.nodes).0 == u && d.edges[e].vertical(&d.nodes).2);
            if arrow_up {
                g.put(x, top, '▲');
            }
        }
        for &l in &bus.lowers {
            let x = d.nodes[l].center();
            for y in bus_row + 1..bottom {
                g.put(x, y, '│');
            }
            let arrow_down = bus
                .edges
                .iter()
                .any(|&e| d.edges[e].vertical(&d.nodes).1 == l && d.edges[e].vertical(&d.nodes).3);
            g.put(x, arrow_row, if arrow_down { '▼' } else { '│' });
        }
        // Junctions on the bus.
        for x in lo..=hi {
            let from_above = bus.uppers.iter().any(|&u| d.nodes[u].center() == x);
            let to_below = bus.lowers.iter().any(|&l| d.nodes[l].center() == x);
            let c = match (x == lo, x == hi, from_above, to_below) {
                (_, _, true, true) => '┼',
                (true, true, true, false) => '│',
                (true, true, false, true) => '│',
                (true, false, true, false) => '└',
                (true, false, false, true) => '┌',
                (false, true, true, false) => '┘',
                (false, true, false, true) => '┐',
                (_, _, true, false) => '┴',
                (_, _, false, true) => '┬',
                _ => continue,
            };
            g.put(x, bus_row, c);
        }
        // Labels sit on the segment that belongs to one edge only.
        for &e in &bus.edges {
            let Some(label) = &d.edges[e].label else {
                continue;
            };
            let (u, l, _, _) = d.edges[e].vertical(&d.nodes);
            let into_l = bus
                .edges
                .iter()
                .filter(|&&o| d.edges[o].vertical(&d.nodes).1 == l)
                .count();
            if into_l == 1 {
                g.text(d.nodes[l].center() + 2, entry_row, label);
            } else {
                g.text(d.nodes[u].center() + 2, top, label);
            }
        }
    }
}

fn draw_same_row(
    g: &mut Grid,
    d: &Diagram,
    a: usize,
    b: usize,
    aa: bool,
    ab: bool,
    label: Option<&str>,
) {
    let (left, right, arrow_left, arrow_right) = if d.nodes[a].x < d.nodes[b].x {
        (&d.nodes[a], &d.nodes[b], aa, ab)
    } else {
        (&d.nodes[b], &d.nodes[a], ab, aa)
    };
    let y = left.middle();
    for x in left.x + left.w..right.x {
        g.put(x, y, '─');
    }
    if arrow_left {
        g.put(left.x + left.w, y, '◀');
    }
    if arrow_right {
        g.put(right.x - 1, y, '▶');
    }
    if let Some(label) = label {
        // Centred over the line, which is what the gap was widened for.
        let gap = right.x.saturating_sub(left.x + left.w);
        let at = left.x + left.w + gap.saturating_sub(label.width()) / 2;
        g.text(at, y - 1, label);
    }
}

/// An edge routed down (or up) a channel on the right of the diagram.
fn draw_side(g: &mut Grid, d: &Diagram, e: usize, col: usize) {
    let (u, l, au, al) = d.edges[e].vertical(&d.nodes);
    let (upper, lower) = (&d.nodes[u], &d.nodes[l]);
    let (uy, ly) = (upper.middle(), lower.middle());
    for x in upper.x + upper.w..col {
        g.put(x, uy, '─');
    }
    for x in lower.x + lower.w..col {
        g.put(x, ly, '─');
    }
    if uy == ly {
        // Same row, not neighbours: loop out and back on the row below.
        let y = uy + 2;
        for yy in uy..=y {
            g.put(col, yy, '│');
        }
        g.put(col, uy, '┐');
        g.put(col, y, '┘');
        return;
    }
    for y in uy + 1..ly {
        g.put(col, y, '│');
    }
    g.put(col, uy, '┐');
    g.put(col, ly, '┘');
    if au {
        g.put(upper.x + upper.w, uy, '◀');
    }
    if al {
        g.put(lower.x + lower.w, ly, '◀');
    }
    if let Some(label) = &d.edges[e].label {
        g.text(col + 2, (uy + ly) / 2, label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(lines: &[String]) -> String {
        lines.join("\n")
    }

    #[test]
    fn two_boxes_in_a_column() {
        let out = render("one[One]\ntwo[Two]\none --> two").unwrap();
        assert_eq!(
            joined(&out),
            "┌─────┐\n│ One │\n└─────┘\n   │\n   ▼\n┌─────┐\n│ Two │\n└─────┘"
        );
    }

    #[test]
    fn the_pet_store() {
        let src = "diaType: flowchart\nstart(Start)\na[Go to the pet store]\nb<Cats or dogs?>\nc[Buy a cat], d[Buy a dog]\nend(End)\n\nstart --> a --> b\nb --> c | Cats\nb --> d | Dogs\nc --> end\nd --> end\n";
        let out = render(src).unwrap();
        let text = joined(&out);
        println!("{text}");
        assert!(text.contains("╭───────╮"), "pill");
        assert!(text.contains("⟨"), "diamond");
        assert!(
            text.contains("┬") && text.contains("┴"),
            "fan out and merge buses"
        );
        assert!(
            text.contains("│ Cats") || text.contains("│  Cats"),
            "label on the branch"
        );
        assert_eq!(text.matches('▼').count(), 5, "one arrow head per line");
        assert!(out.iter().all(|l| l.width() <= 60));
    }

    #[test]
    fn arrows_lines_and_side_channels() {
        let src = "a[A]\nb[B]\nc[C]\na <-> b\nb --- c\na --> c | later";
        let out = render(src).unwrap();
        let text = joined(&out);
        println!("{text}");
        assert!(text.contains('▲') && text.contains('▼'), "two-way arrow");
        assert!(
            text.contains("┐") && text.contains("◀"),
            "side channel with an arrow head"
        );
        assert!(text.contains("later"));
        let side_rows: Vec<&str> = out
            .iter()
            .map(|s| s.as_str())
            .filter(|l| l.ends_with('┐') || l.contains("┘"))
            .collect();
        assert!(!side_rows.is_empty());
    }

    #[test]
    fn errors_are_explained() {
        assert!(
            render("a[A]\na --> zz")
                .unwrap_err()
                .contains("no shape named 'zz'")
        );
        assert!(
            render("diaType: pie\na[A]")
                .unwrap_err()
                .contains("unknown diaType")
        );
        assert!(
            render("a[A]\nb[B]\na --> b ==> a")
                .unwrap_err()
                .contains("not one of")
        );
        assert!(render("hello").unwrap_err().contains("is not id[label]"));
        assert!(render("a[A], b[B]\na -- b").is_err());
    }

    #[test]
    fn shapes_in_a_column_line_up() {
        let out = render("a[A], b[A much longer label]\nc[C], d[D]\na --> c\nb --> d").unwrap();
        let text = joined(&out);
        println!("{text}");
        assert!(
            !text.contains('┬') && !text.contains('┴'),
            "straight lines, no buses"
        );
        assert_eq!(text.matches('▼').count(), 2);
        // The lone shape of a one-shape row is centred over both columns.
        let out = render("a[A], b[B]\nc[Centre]\na --> c\nb --> c").unwrap();
        let text = joined(&out);
        println!("{text}");
        assert!(
            text.contains("└") && text.contains("┘") && text.contains("┬"),
            "merge bus"
        );
    }

    #[test]
    fn single_shapes_share_one_centre_and_back_edges_go_sideways() {
        let out = render("a[X], b[Y]\nc[Some odd]\nd[Even ln]\nz(End)\nc --> d --> z").unwrap();
        let text = joined(&out);
        println!("{text}");
        assert!(
            !text.contains('┬') && !text.contains('┴'),
            "no bus between centred shapes"
        );
        let arrow_columns: Vec<usize> = out
            .iter()
            .filter_map(|l| l.chars().position(|c| c == '▼'))
            .collect();
        assert_eq!(
            arrow_columns,
            vec![arrow_columns[0]; 2],
            "arrows share one column"
        );

        let out = render("a[A], b[B]\nc<Choice?>\na --> c\nc --> b | back").unwrap();
        let text = joined(&out);
        println!("{text}");
        assert!(text.contains("◀"), "the back edge enters B from the side");
        assert!(text.contains("│ back") || text.contains("back"));
        assert_eq!(
            text.matches('▲').count(),
            0,
            "no upward head on a shared bus"
        );
    }

    #[test]
    fn empty_slots_pick_the_column() {
        let out = render("a[A], b[B], c[C]\n,, z(End)\n, y[Y]\nc --> z\nz --> y").unwrap();
        let text = joined(&out);
        println!("{text}");
        assert_eq!(out[0].matches('┌').count(), 3, "three columns");
        let arrow_col = out[4].chars().position(|c| c == '▼').unwrap();
        let c_col = out[1].chars().position(|c| c == 'C').unwrap();
        assert_eq!(arrow_col, c_col, "End sits straight under C");
        assert!(text.contains("┘"), "z --> y crosses columns through a bus");
    }

    #[test]
    fn same_row_neighbours_join_horizontally() {
        let out = render("a[A], b[B]\na --> b").unwrap();
        assert_eq!(out[1], "│ A │───▶│ B │");
        // A label widens the gap so it fits above the line, and the columns below follow.
        let out =
            render("a[A], b[B]\nc[C], d[D]\na --> b | a long label\na --> c\nb --> d").unwrap();
        let text = joined(&out);
        println!("{text}");
        assert!(
            out[0].contains("┌───┐ a long label ┌───┐"),
            "label centred in the widened gap"
        );
        assert!(out[1].contains("│ A │─────────────▶│ B │"));
        assert!(!text.contains('┬'), "the columns still line up");
    }
}
