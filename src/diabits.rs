//! diaBits: small flowchart and tree languages rendered with box-drawing characters.
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
//!
//! ```text
//! diaType: tree
//! root(Live Game State)
//! .vitals[Hero Vitals]
//! ..hp[Hit Points]
//! .party[Party Roster]
//! ```
//!
//! A tree has one shape per line; the leading dots say how deep it sits, one more than its
//! parent, and the lines from a parent down and across to its children are implied. Any
//! number of roots (no dots) can follow each other.

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Flowchart,
    Tree,
}

/// The `diaType:` line, wherever it is in the block; a block without one is a flowchart.
fn kind(source: &str) -> Result<Kind, String> {
    let mut kind = None;
    for (number, raw) in source.lines().enumerate() {
        let Some(rest) = raw.trim().strip_prefix("diaType:") else {
            continue;
        };
        let at = number + 1;
        if kind.is_some() {
            return Err(format!("line {at}: diaType given twice"));
        }
        kind = Some(match rest.trim() {
            "flowchart" => Kind::Flowchart,
            "tree" => Kind::Tree,
            other => {
                return Err(format!(
                    "line {at}: unknown diaType '{other}' (flowchart or tree)"
                ));
            }
        });
    }
    Ok(kind.unwrap_or(Kind::Flowchart))
}

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
    match kind(source)? {
        Kind::Flowchart => {
            let mut diagram = parse(source)?;
            layout(&mut diagram);
            Ok(draw(&diagram))
        }
        Kind::Tree => {
            let mut nodes = parse_tree(source)?;
            layout_tree(&mut nodes);
            Ok(draw_tree(&nodes))
        }
    }
}

// ----- parsing ------------------------------------------------------------------------

fn parse(source: &str) -> Result<Diagram, String> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut edges = Vec::new();
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut explicit: Vec<Option<usize>> = Vec::new();
    let mut ids: HashMap<String, usize> = HashMap::new();
    for (number, raw) in source.lines().enumerate() {
        let line = raw.trim();
        // The diaType line was read by `kind`.
        if line.is_empty() || line.starts_with("diaType:") {
            continue;
        }
        let at = number + 1;
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

/// Columns a shape takes for its label.
fn shape_width(shape: Shape, label: &str) -> usize {
    match shape {
        Shape::Box | Shape::Pill => label.width() + 4,
        Shape::Diamond => label.width() + 6,
    }
}

// ----- layout -------------------------------------------------------------------------

fn layout(d: &mut Diagram) {
    for node in &mut d.nodes {
        node.w = shape_width(node.shape, &node.label);
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
    // Lines routed down the left side need a margin for their channels.
    let margin = side_plan(d).left_margin();
    for n in &mut d.nodes {
        n.x += margin;
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

// ----- side channels ------------------------------------------------------------------

/// A line routed down a channel beside the diagram.
#[derive(Clone, Copy, Debug)]
struct Route {
    edge: usize,
    left: bool,
    /// Which channel on that side, counting out from the shapes.
    channel: usize,
    /// An end without an arrow head attaches at the shape's corner (the bottom corner of
    /// the upper shape, the top corner of the lower one) instead of the middle of its side,
    /// because another line's arrow head arrives at that side of the shape.
    upper_corner: bool,
    lower_corner: bool,
}

struct SidePlan {
    routes: Vec<Route>,
    /// Channels and widest label on each side.
    left_channels: usize,
    right_channels: usize,
    left_label: usize,
    right_label: usize,
}

impl SidePlan {
    /// Columns the shapes move right to make room for the channels on the left.
    fn left_margin(&self) -> usize {
        if self.left_channels == 0 {
            return 0;
        }
        let labels = if self.left_label > 0 {
            self.left_label + 2
        } else {
            0
        };
        3 + (self.left_channels - 1) * (3 + self.left_label) + labels
    }

    /// Column of channel `k` on the left (`margin` as `left_margin`) or the right of a
    /// diagram whose shapes end at `content`.
    fn column(&self, route: &Route, margin: usize, content: usize) -> usize {
        if route.left {
            margin - 3 - route.channel * (3 + self.left_label)
        } else {
            content + 2 + route.channel * (3 + self.right_label)
        }
    }

    fn width(&self, margin: usize, content: usize) -> usize {
        if self.right_channels == 0 {
            return content;
        }
        let outermost = Route {
            edge: 0,
            left: false,
            channel: self.right_channels - 1,
            upper_corner: false,
            lower_corner: false,
        };
        self.column(&outermost, margin, content) + 1 + self.right_label
    }
}

/// Plan the lines that run down the sides. Each takes the side where it crosses fewer of
/// the lines placed before it (the right when it makes no difference; a line between two
/// shapes of one row always the right), then the innermost channel on that side no line
/// sharing any of its rows is using — so a line nests inside the longer ones around it,
/// and lines that share no rows share a channel.
fn side_plan(d: &Diagram) -> SidePlan {
    let rows = |e: usize| {
        let (u, l, _, _) = d.edges[e].vertical(&d.nodes);
        (d.nodes[u].row, d.nodes[l].row)
    };
    let crosses = |e: usize, f: usize| {
        let ((a, b), (c, dd)) = (rows(e), rows(f));
        (a < c && c < b && b < dd) || (c < a && a < dd && dd < b)
    };
    let overlaps = |e: usize, f: usize| {
        let ((a, b), (c, dd)) = (rows(e), rows(f));
        !(b < c || dd < a)
    };
    let mut side: Vec<usize> = (0..d.edges.len())
        .filter(|&e| goes_sideways(d, e))
        .collect();
    // Whether edge `e` has an arrow head at shape `n`.
    let headed = |e: usize, n: usize| {
        let (u, l, au, al) = d.edges[e].vertical(&d.nodes);
        (u == n && au) || (l == n && al)
    };
    // Shapes where one of the edges arrives with an arrow head and the other just leaves:
    // on one side they would meet at the same point.
    let conflicts = |e: usize, f: usize| {
        let (u, l, _, _) = d.edges[e].vertical(&d.nodes);
        let (fu, fl, _, _) = d.edges[f].vertical(&d.nodes);
        [u, l]
            .iter()
            .filter(|&&n| (n == fu || n == fl) && headed(e, n) != headed(f, n))
            .count()
    };
    // Longest lines pick their side first: the shorter ones then nest around them. Meeting
    // another line at a shape counts for less than crossing one.
    side.sort_by_key(|&e| {
        let (a, b) = rows(e);
        std::cmp::Reverse(b - a)
    });
    let mut left_of = vec![false; d.edges.len()];
    let mut placed: Vec<usize> = Vec::new();
    for &e in &side {
        let (a, b) = rows(e);
        if a != b {
            let cost = |left: bool| {
                placed
                    .iter()
                    .filter(|&&f| left_of[f] == left)
                    .map(|&f| 2 * usize::from(crosses(e, f)) + conflicts(e, f))
                    .sum::<usize>()
            };
            left_of[e] = cost(true) < cost(false);
        }
        placed.push(e);
    }
    side.sort_by_key(|&e| {
        let (a, b) = rows(e);
        b - a
    });
    let mut routes: Vec<Route> = Vec::new();
    for &e in &side {
        let left = left_of[e];
        let mut channel = 0;
        while routes
            .iter()
            .any(|r| r.left == left && r.channel == channel && overlaps(e, r.edge))
        {
            channel += 1;
        }
        routes.push(Route {
            edge: e,
            left,
            channel,
            upper_corner: false,
            lower_corner: false,
        });
    }
    // Whether some other line on that side arrives at shape `n` with an arrow head.
    let headed_at = |routes: &[Route], i: usize, n: usize| {
        routes.iter().enumerate().any(|(j, r)| {
            let (c, dd) = rows(r.edge);
            j != i && r.left == routes[i].left && c != dd && headed(r.edge, n)
        })
    };
    for i in 0..routes.len() {
        let (u, l, au, al) = d.edges[routes[i].edge].vertical(&d.nodes);
        let (a, b) = rows(routes[i].edge);
        if a == b {
            continue;
        }
        routes[i].upper_corner = !au && headed_at(&routes, i, u);
        routes[i].lower_corner = !al && headed_at(&routes, i, l);
    }
    routes.sort_by_key(|r| r.edge);
    let label = |left: bool| {
        routes
            .iter()
            .filter(|r| r.left == left)
            .filter_map(|r| d.edges[r.edge].label.as_deref().map(UnicodeWidthStr::width))
            .max()
            .unwrap_or(0)
    };
    let channels = |left: bool| {
        routes
            .iter()
            .filter(|r| r.left == left)
            .map(|r| r.channel + 1)
            .max()
            .unwrap_or(0)
    };
    SidePlan {
        left_channels: channels(true),
        right_channels: channels(false),
        left_label: label(true),
        right_label: label(false),
        routes,
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
            // A line running on through a corner makes a junction.
            ('┐' | '┌', '─') | ('─', '┐' | '┌') => '┬',
            ('┘' | '└', '─') | ('─', '┘' | '└') => '┴',
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
    // Edges that skip rows, or join non-neighbouring shapes of a row, use channels down
    // the sides.
    let plan = side_plan(d);
    let margin = plan.left_margin();
    let mut g = Grid::new(plan.width(margin, content), height);

    for n in &d.nodes {
        draw_shape(&mut g, n.x, n.y, n.w, n.shape, &n.label);
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
    for route in &plan.routes {
        draw_side(&mut g, d, route, plan.column(route, margin, content));
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

fn draw_shape(g: &mut Grid, x: usize, y: usize, w: usize, shape: Shape, label: &str) {
    let inner = w - 2;
    let pad = |s: &str, width: usize| {
        let extra = width.saturating_sub(s.width());
        let left = extra / 2;
        format!("{}{}{}", " ".repeat(left), s, " ".repeat(extra - left))
    };
    match shape {
        Shape::Box | Shape::Pill => {
            let (tl, tr, bl, br) = if shape == Shape::Box {
                ('┌', '┐', '└', '┘')
            } else {
                ('╭', '╮', '╰', '╯')
            };
            g.text(x, y, &format!("{tl}{}{tr}", "─".repeat(inner)));
            g.text(x, y + 1, &format!("│{}│", pad(label, inner)));
            g.text(x, y + 2, &format!("{bl}{}{br}", "─".repeat(inner)));
        }
        Shape::Diamond => {
            let inner = w - 4;
            g.text(x, y, &format!(" ╱{}╲", "‾".repeat(inner)));
            g.text(x, y + 1, &format!("⟨{}⟩", pad(label, w - 2)));
            g.text(x, y + 2, &format!(" ╲{}╱", "_".repeat(inner)));
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

/// An edge routed down (or up) a channel beside the diagram.
fn draw_side(g: &mut Grid, d: &Diagram, route: &Route, col: usize) {
    let (u, l, au, al) = d.edges[route.edge].vertical(&d.nodes);
    let (upper, lower) = (&d.nodes[u], &d.nodes[l]);
    let left = route.left;
    let uy = if route.upper_corner {
        upper.y + upper.h - 1
    } else {
        upper.middle()
    };
    let ly = if route.lower_corner {
        lower.y
    } else {
        lower.middle()
    };
    // The arm from the shape to the channel, joined to a box's corner when it starts there
    // (a diamond's top and bottom rows start with a blank cell before its edge).
    let arm = |g: &mut Grid, n: &Node, y: usize, corner: Option<char>| {
        let xs = if left { col + 1..n.x } else { n.x + n.w..col };
        for x in xs {
            g.put(x, y, '─');
        }
        match (corner, n.shape) {
            (None, _) => {}
            (Some(_), Shape::Diamond) => {
                if left {
                    g.put(n.x, y, '─');
                }
            }
            (Some(c), _) => g.put(if left { n.x } else { n.x + n.w - 1 }, y, c),
        }
    };
    arm(g, upper, uy, route.upper_corner.then_some('┴'));
    arm(g, lower, ly, route.lower_corner.then_some('┬'));
    if uy == ly {
        // Same row, not neighbours: loop out and back on the row below.
        g.put(col, uy, '┐');
        g.put(col, uy + 1, '│');
        g.put(col, uy + 2, '┘');
        return;
    }
    for y in uy + 1..ly {
        g.put(col, y, '│');
    }
    g.put(col, uy, if left { '┌' } else { '┐' });
    g.put(col, ly, if left { '└' } else { '┘' });
    let head = |g: &mut Grid, n: &Node, y: usize| {
        if left {
            g.put(n.x - 1, y, '▶');
        } else {
            g.put(n.x + n.w, y, '◀');
        }
    };
    if au {
        head(g, upper, uy);
    }
    if al {
        head(g, lower, ly);
    }
    if let Some(label) = &d.edges[route.edge].label {
        let y = (uy + ly) / 2;
        if left {
            g.text(col - 1 - label.width(), y, label);
        } else {
            g.text(col + 2, y, label);
        }
    }
}

// ----- trees --------------------------------------------------------------------------

#[derive(Debug)]
struct TreeNode {
    label: String,
    shape: Shape,
    /// How many leading dots: 0 for a root, one more than the parent for a child.
    depth: usize,
    x: usize,
    y: usize,
    w: usize,
}

/// Columns a child sits to the right of its parent.
const TREE_INDENT: usize = 5;
/// Column of the trunk down from a parent, from the parent's left edge.
const TREE_TRUNK: usize = 2;

fn parse_tree(source: &str) -> Result<Vec<TreeNode>, String> {
    let mut nodes: Vec<TreeNode> = Vec::new();
    let mut ids: HashMap<String, usize> = HashMap::new();
    for (number, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("diaType:") {
            continue;
        }
        let at = number + 1;
        let depth = line.chars().take_while(|&c| c == '.').count();
        let item = line[depth..].trim_start();
        if item.contains("-->")
            || item.contains("<--")
            || item.contains("<->")
            || item.contains("---")
        {
            return Err(format!(
                "line {at}: a tree has no lines: a child follows its parent with one more leading dot"
            ));
        }
        if nodes.is_empty() && depth > 0 {
            return Err(format!(
                "line {at}: the first shape is a root: no leading dots"
            ));
        }
        let above = nodes.last().map_or(0, |n| n.depth);
        if depth > above + 1 {
            return Err(format!(
                "line {at}: {depth} dots, but the shape above has {above}: one more dot per level"
            ));
        }
        let (id, label, shape) = parse_shape(item).ok_or_else(|| {
            format!("line {at}: '{item}' is not id[label], id(label) or id<label>")
        })?;
        if ids.contains_key(id) {
            return Err(format!("line {at}: '{id}' is defined twice"));
        }
        ids.insert(id.to_string(), nodes.len());
        nodes.push(TreeNode {
            label: label.trim().to_string(),
            shape,
            depth,
            x: 0,
            y: 0,
            w: 0,
        });
    }
    if nodes.is_empty() {
        return Err("no shapes: add lines like id[label] and .child[label]".to_string());
    }
    Ok(nodes)
}

/// Shapes in order, each three rows tall, children indented under their parent; a blank
/// row separates one root's tree from the next.
fn layout_tree(nodes: &mut [TreeNode]) {
    let mut y = 0;
    // Left edge of the current shape at each depth.
    let mut lefts: Vec<usize> = Vec::new();
    for (i, n) in nodes.iter_mut().enumerate() {
        n.w = shape_width(n.shape, &n.label);
        if i > 0 && n.depth == 0 {
            y += 1;
        }
        n.x = if n.depth == 0 {
            0
        } else {
            lefts[n.depth - 1] + TREE_INDENT
        };
        lefts.truncate(n.depth);
        lefts.push(n.x);
        n.y = y;
        y += 3;
    }
}

fn draw_tree(nodes: &[TreeNode]) -> Vec<String> {
    let width = nodes.iter().map(|n| n.x + n.w).max().unwrap_or(0);
    let height = nodes.iter().map(|n| n.y + 3).max().unwrap_or(0);
    let mut g = Grid::new(width, height);
    for n in nodes {
        draw_shape(&mut g, n.x, n.y, n.w, n.shape, &n.label);
    }
    for (i, n) in nodes.iter().enumerate() {
        // A trunk down from the parent, with an arm across to each child.
        let children: Vec<&TreeNode> = nodes[i + 1..]
            .iter()
            .take_while(|m| m.depth > n.depth)
            .filter(|m| m.depth == n.depth + 1)
            .collect();
        let Some(last) = children.last() else {
            continue;
        };
        let trunk = n.x + TREE_TRUNK;
        if n.shape != Shape::Diamond {
            g.put(trunk, n.y + 2, '┬');
        }
        for y in n.y + 3..last.y + 1 {
            g.put(trunk, y, '│');
        }
        for child in &children {
            let y = child.y + 1;
            g.put(
                trunk,
                y,
                if std::ptr::eq(*child, *last) {
                    '└'
                } else {
                    '├'
                },
            );
            for x in trunk + 1..child.x {
                g.put(x, y, '─');
            }
        }
    }
    g.lines()
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
        assert!(
            render("diaType: tree\n.a[A]")
                .unwrap_err()
                .contains("the first shape is a root")
        );
        assert!(
            render("diaType: tree\na[A]\n...b[B]")
                .unwrap_err()
                .contains("one more dot per level")
        );
        assert!(
            render("diaType: tree\na[A]\nb[B]\na --> b")
                .unwrap_err()
                .contains("a tree has no lines")
        );
    }

    #[test]
    fn side_lines_take_both_sides_and_nest() {
        // A decision ladder: the long line goes right, the one it would cross goes left, the
        // short one nests inside; nothing crosses and no two lines share a point.
        let src = "a(Start)\nb<Gates?>\nc[Step]\nd<Scan?>\ne[Divert]\nf[Chores]\ng(Advance)\na --> b\nb --> c | ok\nb --> d | blocked\nd --> e | found\nd --> f | none\nc --> g";
        let out = render(src).unwrap();
        let text = joined(&out);
        println!("{text}");
        assert!(!text.contains('┼'), "no crossings");
        assert!(
            text.contains("┌──") && text.contains("▶⟨"),
            "a line down the left"
        );
        assert!(text.contains("blocked") && text.contains("none"));
        let chores = out.iter().position(|l| l.contains("Chores")).unwrap();
        let advance = out.iter().position(|l| l.contains("Advance")).unwrap();
        assert!(
            out[chores].contains("│◀──┘") && out[advance].contains("│◀────────┘"),
            "both arrive from the right, the inner line nested"
        );

        // Fan-outs from one shape share a bus of junctions; a line leaving a shape where
        // another arrives takes the other side.
        let out = render("a[A]\nb[B]\nc[C]\nd[D]\ne[E]\na --> b --> c --> d --> e\na --> c | ac\na --> d | ad\na --> e\nc --> e | ce").unwrap();
        let text = joined(&out);
        println!("{text}");
        assert!(text.contains("│ A │──┬────┬────┐"));
        assert!(text.contains("┌──│ C │◀─┘") && text.contains("└─▶│ E │"));
        assert!(!text.contains('┼'));

        // Opposite lines between the same two shapes: the leaving end moves to the corner.
        let out = render("a[A]\nb[B]\nc[C]\na --> b --> c\na --> c\nc --> a | back").unwrap();
        let text = joined(&out);
        println!("{text}");
        assert!(text.contains("┌─▶│ A │──┐") && text.contains("└──│ C │◀─┘"));
    }

    #[test]
    fn a_tree_hangs_children_under_their_parent() {
        let src = "diaType: tree\nr1(Root 1)\n.g1[Parent]\n..c1[Kid 1]\n..c2<Kid 2?>\n.g2[Aunt]\nr2(Root 2)\n.x[Only]";
        let out = render(src).unwrap();
        let text = joined(&out);
        println!("{text}");
        assert_eq!(
            out,
            vec![
                "╭────────╮",
                "│ Root 1 │",
                "╰─┬──────╯",
                "  │  ┌────────┐",
                "  ├──│ Parent │",
                "  │  └─┬──────┘",
                "  │    │  ┌───────┐",
                "  │    ├──│ Kid 1 │",
                "  │    │  └───────┘",
                "  │    │   ╱‾‾‾‾‾‾‾‾╲",
                "  │    └──⟨  Kid 2?  ⟩",
                "  │        ╲________╱",
                "  │  ┌──────┐",
                "  └──│ Aunt │",
                "     └──────┘",
                "",
                "╭────────╮",
                "│ Root 2 │",
                "╰─┬──────╯",
                "  │  ┌──────┐",
                "  └──│ Only │",
                "     └──────┘",
            ]
        );
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
