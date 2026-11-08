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
    // The rest is set by `layout`.
    /// How many columns the grid has.
    columns: usize,
    /// The columns each shape spans: the first and one past the last.
    span: Vec<(usize, usize)>,
    /// Lines that skip rows but run straight down the empty cells of their column.
    through: Vec<bool>,
    /// Lines that skip rows drawn as an elbow: down the empty column of their upper shape,
    /// then along the lower shape's row into its side.
    elbow: Vec<bool>,
    /// Lines routed down a channel in a gap.
    routes: Vec<Route>,
    /// The gaps a line can run down: gap 0 is left of the first column, gap `columns` right
    /// of the last, and gap `g` between columns `g - 1` and `g`.
    gaps: Vec<Gap>,
    /// Rows of text the diagram takes.
    height: usize,
}

/// A line routed down a channel in a gap between columns or beside the diagram.
#[derive(Clone, Copy, Debug)]
struct Route {
    edge: usize,
    gap: usize,
    /// Which channel of that gap, counting out from the shapes (on the left) or from the
    /// left edge of the gap.
    channel: usize,
    /// An end without an arrow head attaches at the shape's corner (the bottom corner of
    /// the upper shape, the top corner of the lower one) instead of the middle of its side,
    /// because another line's arrow head arrives at that side of the shape.
    upper_corner: bool,
    lower_corner: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Gap {
    /// Channels in use and the widest label on them.
    channels: usize,
    label: usize,
    /// Where it starts and how wide it is.
    x: usize,
    width: usize,
    /// Columns at its start kept for the label of a line drawn along a row across it.
    lead: usize,
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
        columns: 0,
        span: Vec::new(),
        through: Vec::new(),
        elbow: Vec::new(),
        routes: Vec::new(),
        gaps: Vec::new(),
        height: 0,
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

/// Whether an edge asks for a channel in a gap rather than a bus between its rows: edges
/// that skip rows (unless they can go straight through or take an elbow), and edges that
/// point back up (or both ways) without lining up, which would be ambiguous on a shared
/// bus. Edges along a row are never routed: neighbours join directly and others loop under
/// the row.
fn wants_channel(d: &Diagram, e: usize) -> bool {
    let (u, l, au, _) = d.edges[e].vertical(&d.nodes);
    let (ru, rl) = (d.nodes[u].row, d.nodes[l].row);
    if ru == rl {
        return false;
    }
    (rl > ru + 1 && !d.through[e] && !d.elbow[e])
        || (au && d.nodes[u].center() != d.nodes[l].center())
}

/// The rows of the two ends of an edge, top one first.
fn edge_rows(d: &Diagram, e: usize) -> (usize, usize) {
    let (u, l, _, _) = d.edges[e].vertical(&d.nodes);
    (d.nodes[u].row, d.nodes[l].row)
}

/// The leftmost and rightmost centre column of the two ends of an edge.
fn edge_extent(d: &Diagram, e: usize) -> (usize, usize) {
    let (a, b) = (
        d.nodes[d.edges[e].from].center(),
        d.nodes[d.edges[e].to].center(),
    );
    (a.min(b), a.max(b))
}

/// Whether an edge that skips rows is drawn straight down instead of round a channel: its
/// ends sit in the same columns, no shape on the rows between touches those columns, and
/// no line drawn between those rows crosses or shares the column (that would need a
/// junction).
fn goes_through(d: &Diagram, e: usize) -> bool {
    let (u, l, _, _) = d.edges[e].vertical(&d.nodes);
    let (ru, rl) = (d.nodes[u].row, d.nodes[l].row);
    if rl <= ru + 1 || d.span[u] != d.span[l] {
        return false;
    }
    let (c0, c1) = d.span[u];
    let x = d.nodes[u].center();
    let blocked = d
        .nodes
        .iter()
        .zip(&d.span)
        .any(|(n, &(m0, m1))| n.row > ru && n.row < rl && m0 < c1 && c0 < m1);
    if blocked {
        return false;
    }
    !(0..d.edges.len()).any(|f| {
        if f == e {
            return false;
        }
        let (fu, fl, _, _) = d.edges[f].vertical(&d.nodes);
        let (ra, rb) = (d.nodes[fu].row, d.nodes[fl].row);
        let between = if rb > ra + 1 {
            // Skipping lines run through their own column or down a channel.
            false
        } else if ra < rb {
            ru <= ra && rb <= rl
        } else if neighbours(d, fu, fl) {
            ru < ra && ra < rl
        } else {
            // A loop under row `ra` lives in the region below it.
            ru <= ra && ra < rl
        };
        let (lo, hi) = edge_extent(d, f);
        between && lo <= x && x <= hi
    })
}

/// Whether an edge that skips rows is drawn as an elbow instead of round a channel: straight
/// down from the upper shape through its own empty column, then along the lower shape's row
/// into its side. The column holds no shape on the rows between or on the lower row, no
/// shape on the lower row sits between the column and the lower shape, and no other line
/// crosses the column or the arm: a bus or loop between the rows, a line along a row, or a
/// line down the same column (through or elbow, which are decided in written order).
fn takes_elbow(d: &Diagram, e: usize) -> bool {
    let (u, l, _, _) = d.edges[e].vertical(&d.nodes);
    let (ru, rl) = (d.nodes[u].row, d.nodes[l].row);
    let ((c0, c1), (l0, l1)) = (d.span[u], d.span[l]);
    if rl <= ru + 1 || (c0 < l1 && l0 < c1) {
        return false;
    }
    let x = d.nodes[u].center();
    // The arm runs right from the column into the lower shape's left side, or left.
    let left = c1 <= l0;
    let blocked = d
        .nodes
        .iter()
        .zip(&d.span)
        .enumerate()
        .any(|(m, (n, &(m0, m1)))| {
            if n.row <= ru || n.row > rl || m == l {
                false
            } else if n.row < rl {
                m0 < c1 && c0 < m1
            } else if left {
                m1 > c0 && m0 < l0
            } else {
                m0 < c1 && m1 > l1
            }
        });
    if blocked {
        return false;
    }
    let arm = if left {
        x..=d.nodes[l].center()
    } else {
        d.nodes[l].center()..=x
    };
    !(0..d.edges.len()).any(|f| {
        if f == e {
            return false;
        }
        let (fu, fl, _, _) = d.edges[f].vertical(&d.nodes);
        let (ra, rb) = (d.nodes[fu].row, d.nodes[fl].row);
        let (lo, hi) = edge_extent(d, f);
        if rb > ra + 1 {
            if !(d.through[f] || d.elbow[f]) {
                // Channels run down the gaps.
                return false;
            }
            // Down the same column from the same shape, down a column the arm crosses, or
            // (an elbow) along a row across the column.
            fu == u
                || (ra < rl && rl <= rb && arm.contains(&d.nodes[fu].center()))
                || (d.elbow[f] && ru < rb && rb <= rl && lo <= x && x <= hi)
        } else {
            let between = if ra < rb {
                ru <= ra && rb <= rl
            } else if neighbours(d, fu, fl) {
                ru < ra && ra <= rl
            } else {
                // A loop under row `ra` lives in the region below it.
                ru <= ra && ra < rl
            };
            between && lo <= x && x <= hi
        }
    })
}

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
    d.columns = columns;
    d.span = d
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
    let mut leads = vec![0usize; columns.saturating_sub(1)];
    for e in &d.edges {
        let (a, b) = (&d.nodes[e.from], &d.nodes[e.to]);
        let Some(label) = &e.label else { continue };
        if a.row != b.row || !neighbours(d, e.from, e.to) {
            continue;
        }
        let (left, right) = if d.span[e.from].0 < d.span[e.to].0 {
            (e.from, e.to)
        } else {
            (e.to, e.from)
        };
        let (_, c1) = d.span[left];
        if d.span[right].0 == c1
            && let Some(lead) = leads.get_mut(c1 - 1)
        {
            *lead = (*lead).max(label.width() + 2);
            gaps[c1 - 1] = gaps[c1 - 1].max(*lead);
        }
    }
    let gap_sum = |gaps: &[usize], c0: usize, c1: usize| gaps[c0..c1 - 1].iter().sum::<usize>();
    // Single-column shapes set the column widths; wider spanning shapes stretch their span.
    for (n, &(c0, c1)) in d.span.iter().enumerate() {
        if c1 - c0 == 1 {
            widths[c0] = widths[c0].max(d.nodes[n].w);
        }
    }
    for (n, &(c0, c1)) in d.span.iter().enumerate() {
        let have = widths[c0..c1].iter().sum::<usize>() + gap_sum(&gaps, c0, c1);
        if d.nodes[n].w > have {
            let extra = d.nodes[n].w - have;
            let each = extra.div_ceil(c1 - c0);
            for w in &mut widths[c0..c1] {
                *w += each;
            }
        }
    }
    // Place the shapes, `margin` columns in: by centre, not by left edge, so shapes of
    // different widths in one column share the same centre column exactly. Also notes where
    // each gap starts.
    let place = |d: &mut Diagram, widths: &[usize], gaps: &[usize], margin: usize| {
        let mut starts = Vec::with_capacity(columns);
        let mut x = margin;
        for (c, w) in widths.iter().enumerate() {
            starts.push(x);
            x += w + gaps.get(c).copied().unwrap_or(0);
        }
        for (n, &(c0, c1)) in d.span.iter().enumerate() {
            let total = widths[c0..c1].iter().sum::<usize>() + gap_sum(gaps, c0, c1);
            let centre = starts[c0] + total / 2;
            d.nodes[n].x = centre - d.nodes[n].w / 2;
        }
        d.gaps[0].x = 0;
        d.gaps[0].width = margin;
        for g in 1..columns {
            d.gaps[g].x = starts[g - 1] + widths[g - 1];
            d.gaps[g].width = gaps[g - 1];
        }
        d.gaps[columns].x = starts[columns - 1] + widths[columns - 1];
    };
    d.gaps = vec![Gap::default(); columns + 1];
    for g in 1..columns {
        d.gaps[g].lead = leads[g - 1];
    }
    place(d, &widths, &gaps, 0);
    // With the shapes in place, decide how each line runs, then make room for the channels:
    // the gaps they use widen, and the shapes move right of the channels on the left.
    d.through = (0..d.edges.len()).map(|e| goes_through(d, e)).collect();
    d.elbow = vec![false; d.edges.len()];
    for e in 0..d.edges.len() {
        d.elbow[e] = takes_elbow(d, e);
    }
    channel_plan(d);
    for g in 1..columns {
        gaps[g - 1] = gaps[g - 1].max(gap_width(d.gaps[g], g, columns));
    }
    place(d, &widths, &gaps, gap_width(d.gaps[0], 0, columns));
    d.gaps[columns].width = gap_width(d.gaps[columns], columns, columns);
    // Vertical positions: each gap between rows is as tall as its edges need.
    let mut y = 0;
    for r in 0..d.rows.len() {
        for &n in &d.rows[r] {
            d.nodes[n].y = y;
        }
        y += 3;
        if r + 1 < region_count(d) + 1 {
            y += region_plan(d, r).height;
        }
    }
    d.height = y;
}

/// Columns a gap needs for its channels: outside the shapes a channel sits two columns out
/// from them, with its label beyond it (on the left, before it); between columns a channel
/// keeps a column clear on each side, after the room kept for labels along the rows.
fn gap_width(gap: Gap, g: usize, columns: usize) -> usize {
    if gap.channels == 0 {
        return 0;
    }
    let step = 3 + gap.label;
    let more = (gap.channels - 1) * step;
    if g == 0 {
        3 + more + if gap.label > 0 { gap.label + 2 } else { 0 }
    } else if g == columns {
        3 + more + gap.label
    } else {
        gap.lead + 4 + more + if gap.label > 0 { gap.label + 1 } else { 0 }
    }
}

/// The column a route's channel runs down.
fn channel_x(d: &Diagram, route: &Route) -> usize {
    let gap = d.gaps[route.gap];
    let step = 3 + gap.label;
    if route.gap == 0 {
        gap.x + gap.width - 3 - route.channel * step
    } else {
        gap.x + gap.lead + 2 + route.channel * step
    }
}

/// Whether gap `g` lies left of shape `n`.
fn gap_left_of(d: &Diagram, n: usize, g: usize) -> bool {
    g <= d.span[n].0
}

/// Regions between rows, plus one under the last row when a line loops under it.
fn region_count(d: &Diagram) -> usize {
    let last = d.rows.len() - 1;
    let loops = d.rows[last].iter().any(|&n| {
        d.edges.iter().any(|e| {
            (e.from == n || e.to == n)
                && d.nodes[e.from].row == d.nodes[e.to].row
                && !neighbours(d, e.from, e.to)
        })
    });
    last + usize::from(loops)
}

/// Plan the lines that run down channels. A line may use a gap it can reach from both its
/// shapes along their rows without crossing another shape, and that no shape on the rows
/// between spans. Among those it takes the gap where it crosses the fewest lines (lines
/// placed before it, lines along the rows it passes, buses between them), the rightmost
/// when it makes no difference; the longest lines pick first so the shorter ones nest
/// around them. In a gap it takes the innermost channel no line sharing any of its rows is
/// using, so lines that share no rows share a channel. When no gap is clear a skipping line
/// takes the least blocked one anyway; a back edge between neighbouring rows stays on a bus.
fn channel_plan(d: &mut Diagram) {
    let gaps = d.columns + 1;
    let crosses = |e: usize, f: usize| {
        let ((a, b), (c, dd)) = (edge_rows(d, e), edge_rows(d, f));
        (a < c && c < b && b < dd) || (c < a && a < dd && dd < b)
    };
    let overlaps = |e: usize, f: usize| {
        let ((a, b), (c, dd)) = (edge_rows(d, e), edge_rows(d, f));
        !(b < c || dd < a)
    };
    // Whether edge `e` has an arrow head at shape `n`.
    let headed = |e: usize, n: usize| {
        let (u, l, au, al) = d.edges[e].vertical(&d.nodes);
        (u == n && au) || (l == n && al)
    };
    // Shapes crossed along its row to reach gap `g` from shape `n`.
    let arm_blocks = |n: usize, g: usize| -> usize {
        let (c0, c1) = d.span[n];
        if c0 < g && g < c1 {
            return 100;
        }
        d.rows[d.nodes[n].row]
            .iter()
            .filter(|&&m| m != n)
            .filter(|&&m| {
                let (m0, m1) = d.span[m];
                if g <= c0 {
                    m0 < c0 && m1 > g
                } else {
                    m1 > c1 && m0 < g
                }
            })
            .count()
    };
    // Shapes on the rows between the ends of `e` that span gap `g`.
    let drop_blocks = |e: usize, g: usize| -> usize {
        let (ru, rl) = edge_rows(d, e);
        d.nodes
            .iter()
            .zip(&d.span)
            .filter(|&(n, &(m0, m1))| n.row > ru && n.row < rl && m0 < g && g < m1)
            .count()
    };
    let blocks = |e: usize, g: usize| {
        let (u, l, _, _) = d.edges[e].vertical(&d.nodes);
        arm_blocks(u, g) + arm_blocks(l, g) + drop_blocks(e, g)
    };
    // Lines not routed through channels that a channel in gap `g` between the rows of `e`
    // would cross: a line along a row between them (2), or along one of its own rows,
    // which it joins instead (1); a bus between two rows in its range (2); an elbow's arm.
    let line_crossings = |e: usize, g: usize| -> usize {
        let (ru, rl) = edge_rows(d, e);
        (0..d.edges.len())
            .filter(|&f| f != e)
            .map(|f| {
                let (fu, fl, _, _) = d.edges[f].vertical(&d.nodes);
                let (ra, rb) = (d.nodes[fu].row, d.nodes[fl].row);
                if ra == rb && !neighbours(d, fu, fl) {
                    // A loop under row `ra` runs across the region below it.
                    let (lo, hi) = edge_extent(d, f);
                    let x = d.gaps[g].x;
                    usize::from(ru <= ra && ra < rl && lo < x && x <= hi) * 2
                } else if ra == rb {
                    if ra < ru || ra > rl {
                        return 0;
                    }
                    let (left, right) = if d.span[fu].0 < d.span[fl].0 {
                        (fu, fl)
                    } else {
                        (fl, fu)
                    };
                    if d.span[left].1 <= g && g <= d.span[right].0 {
                        if ra == ru || ra == rl { 1 } else { 2 }
                    } else {
                        0
                    }
                } else if rb == ra + 1 && ru <= ra && rb <= rl && !wants_channel(d, f) {
                    let (lo, hi) = edge_extent(d, f);
                    let x = d.gaps[g].x;
                    usize::from(lo < x && x <= hi) * 2
                } else if d.elbow[f] && ru <= rb && rb <= rl {
                    // An elbow's arm runs along row `rb` from its column to its shape.
                    let (lo, hi) = edge_extent(d, f);
                    let x = d.gaps[g].x;
                    if !(lo < x && x <= hi) {
                        0
                    } else if rb == ru || rb == rl {
                        1
                    } else {
                        2
                    }
                } else {
                    0
                }
            })
            .sum()
    };
    let mut side: Vec<usize> = (0..d.edges.len())
        .filter(|&e| wants_channel(d, e))
        .collect();
    // Longest lines pick their gap first: the shorter ones then nest around them. Meeting
    // another line at a shape counts for less than crossing one.
    side.sort_by_key(|&e| {
        let (a, b) = edge_rows(d, e);
        std::cmp::Reverse(b - a)
    });
    let mut gap_of: Vec<Option<usize>> = vec![None; d.edges.len()];
    for &e in &side {
        let (u, l, _, _) = d.edges[e].vertical(&d.nodes);
        let (ru, rl) = edge_rows(d, e);
        let cost = |g: usize| {
            let placed: usize = gap_of
                .iter()
                .enumerate()
                .filter_map(|(f, gf)| gf.map(|gf| (f, gf)))
                .map(|(f, gf)| {
                    let (fu, fl, _, _) = d.edges[f].vertical(&d.nodes);
                    // Shapes where one line arrives with an arrow head and the other just
                    // leaves, both from the same side: they would meet at one point.
                    let meets = [u, l]
                        .iter()
                        .filter(|&&n| {
                            (n == fu || n == fl)
                                && gap_left_of(d, n, g) == gap_left_of(d, n, gf)
                                && headed(e, n) != headed(f, n)
                        })
                        .count();
                    2 * usize::from(gf == g && crosses(e, f)) + meets
                })
                .sum();
            100 * blocks(e, g) + placed + line_crossings(e, g)
        };
        let best = (0..gaps)
            .map(|g| (cost(g), g))
            .min_by_key(|&(c, g)| (c, std::cmp::Reverse(g)));
        let Some((cost, g)) = best else { continue };
        // A back edge between neighbouring rows is clearer on a bus than through a shape.
        if cost >= 100 && rl == ru + 1 {
            continue;
        }
        gap_of[e] = Some(g);
    }
    side.retain(|&e| gap_of[e].is_some());
    side.sort_by_key(|&e| {
        let (a, b) = edge_rows(d, e);
        b - a
    });
    let mut routes: Vec<Route> = Vec::new();
    for &e in &side {
        let gap = gap_of[e].unwrap_or(0);
        let mut channel = 0;
        while routes
            .iter()
            .any(|r| r.gap == gap && r.channel == channel && overlaps(e, r.edge))
        {
            channel += 1;
        }
        routes.push(Route {
            edge: e,
            gap,
            channel,
            upper_corner: false,
            lower_corner: false,
        });
    }
    // Whether some other line arrives at that side of shape `n` with an arrow head.
    let headed_at = |routes: &[Route], i: usize, n: usize| {
        routes.iter().enumerate().any(|(j, r)| {
            j != i
                && gap_left_of(d, n, r.gap) == gap_left_of(d, n, routes[i].gap)
                && headed(r.edge, n)
        })
    };
    // Whether a line along the row joins shape `n` on the side of gap `g`.
    let row_line_at = |n: usize, g: usize| {
        d.edges.iter().any(|f| {
            let m = if f.from == n {
                f.to
            } else if f.to == n {
                f.from
            } else {
                return false;
            };
            d.nodes[m].row == d.nodes[n].row
                && neighbours(d, n, m)
                && (d.span[m].0 < d.span[n].0) == gap_left_of(d, n, g)
        })
    };
    // Whether an elbow arrives at shape `n` on the side of gap `g`.
    let elbow_at = |n: usize, g: usize| {
        (0..d.edges.len()).any(|f| {
            let (fu, fl, _, _) = d.edges[f].vertical(&d.nodes);
            d.elbow[f]
                && fl == n
                && (d.nodes[fu].center() < d.nodes[n].center()) == gap_left_of(d, n, g)
        })
    };
    for i in 0..routes.len() {
        let (u, l, au, al) = d.edges[routes[i].edge].vertical(&d.nodes);
        let g = routes[i].gap;
        routes[i].upper_corner =
            row_line_at(u, g) || elbow_at(u, g) || (!au && headed_at(&routes, i, u));
        routes[i].lower_corner =
            row_line_at(l, g) || elbow_at(l, g) || (!al && headed_at(&routes, i, l));
    }
    routes.sort_by_key(|r| r.edge);
    for g in 0..gaps {
        let mine = routes.iter().filter(|r| r.gap == g);
        d.gaps[g].channels = mine.clone().map(|r| r.channel + 1).max().unwrap_or(0);
        d.gaps[g].label = mine
            .filter_map(|r| d.edges[r.edge].label.as_deref().map(UnicodeWidthStr::width))
            .max()
            .unwrap_or(0);
    }
    d.routes = routes;
}

/// A bus: a horizontal line between two rows joining a set of upper nodes to lower nodes.
/// A line along a row that skips a shape is a bus too: down from both shapes and across.
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

fn same_row(d: &Diagram, e: usize) -> bool {
    d.nodes[d.edges[e].from].row == d.nodes[d.edges[e].to].row
}

/// Plan the edges between row `r` and row `r + 1`: the edges joining the rows that are not
/// routed down a channel, and the loops under row `r`.
fn region_plan(d: &Diagram, r: usize) -> RegionPlan {
    let routed = |e: usize| d.routes.iter().any(|route| route.edge == e);
    let edges: Vec<usize> = (0..d.edges.len())
        .filter(|&e| {
            let (u, l, _, _) = d.edges[e].vertical(&d.nodes);
            let (ru, rl) = (d.nodes[u].row, d.nodes[l].row);
            if ru == rl {
                ru == r && !neighbours(d, u, l)
            } else {
                ru == r && rl == r + 1 && !routed(e)
            }
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
        if group.len() == 1 && !same_row(d, group[0]) {
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
            let ends = if same_row(d, e) {
                &mut uppers
            } else {
                &mut lowers
            };
            if !ends.contains(&l) {
                ends.push(l);
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
    // Lines passing straight through from further up end or start here too; an elbow
    // starts here and carries its label beside it.
    let ends_here = |e: usize| {
        let (ru, rl) = edge_rows(d, e);
        rl == r + 1 && (ru == r || d.through[e])
    };
    let starts_here = |e: usize| {
        let (ru, rl) = edge_rows(d, e);
        ru == r && (rl == r + 1 || d.through[e] || d.elbow[e])
    };
    let elbow_here = |e: usize| d.elbow[e] && edge_rows(d, e).0 == r;
    let labelled = (0..d.edges.len()).any(|e| {
        (edges.contains(&e) || ends_here(e) || elbow_here(e)) && d.edges[e].label.is_some()
    });
    // An arrow head pointing up into a shape of row `r`.
    let two_way = (0..d.edges.len()).any(|e| {
        let (_, _, au, al) = d.edges[e].vertical(&d.nodes);
        (edges.contains(&e) && (au || (same_row(d, e) && al))) || (starts_here(e) && au)
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
    // Under the last row only loops are drawn: the exit row and the bus rows.
    if r + 1 == d.rows.len() {
        height = levels + 1;
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

/// The arms of a line character: up 1, down 2, left 4, right 8.
fn arms(c: char) -> Option<u8> {
    Some(match c {
        '│' => 3,
        '─' => 12,
        '┌' => 10,
        '┐' => 6,
        '└' => 9,
        '┘' => 5,
        '├' => 11,
        '┤' => 7,
        '┬' => 14,
        '┴' => 13,
        '┼' => 15,
        _ => return None,
    })
}

fn line_char(arms: u8) -> char {
    match arms {
        1..=3 => '│',
        4 | 8 | 12 => '─',
        10 => '┌',
        6 => '┐',
        9 => '└',
        5 => '┘',
        11 => '├',
        7 => '┤',
        14 => '┬',
        13 => '┴',
        15 => '┼',
        _ => ' ',
    }
}

impl Grid {
    fn new(w: usize, h: usize) -> Grid {
        Grid {
            cells: vec![vec![' '; w.max(1)]; h.max(1)],
        }
    }

    fn reach(&mut self, x: usize, y: usize) -> bool {
        if y >= self.cells.len() {
            return false;
        }
        if x >= self.cells[y].len() {
            let extra = x + 1 - self.cells[y].len();
            for row in &mut self.cells {
                row.extend(std::iter::repeat_n(' ', extra));
            }
        }
        true
    }

    /// Place a character. Two lines meeting in a cell join: a crossing of `─` and `│` is
    /// `┼`, a line running on through a corner makes a junction. Text, arrow heads and
    /// shapes overwrite; a line never draws over them.
    fn put(&mut self, x: usize, y: usize, c: char) {
        if !self.reach(x, y) {
            return;
        }
        let cell = &mut self.cells[y][x];
        *cell = match (arms(*cell), arms(c)) {
            (Some(old), Some(new)) => line_char(old | new),
            _ if *cell == ' ' => c,
            (_, None) => c,
            _ => *cell,
        };
    }

    /// Place a character whatever is there.
    fn set(&mut self, x: usize, y: usize, c: char) {
        if self.reach(x, y) {
            self.cells[y][x] = c;
        }
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
    let right = d.gaps[d.columns];
    let mut g = Grid::new(right.x + right.width, d.height);

    for n in &d.nodes {
        draw_shape(&mut g, n.x, n.y, n.w, n.shape, &n.label);
    }
    for r in 0..region_count(d) {
        draw_region(&mut g, d, r);
    }
    for route in &d.routes {
        draw_side(&mut g, d, route, channel_x(d, route));
    }
    // Lines along the rows go last: their labels move out of the way of a channel's arm.
    for e in 0..d.edges.len() {
        let (u, l, au, al) = d.edges[e].vertical(&d.nodes);
        if d.nodes[u].row == d.nodes[l].row && neighbours(d, u, l) {
            draw_same_row(&mut g, d, u, l, au, al, d.edges[e].label.as_deref());
        } else if d.through[e] {
            draw_through(&mut g, d, e);
        } else if d.elbow[e] {
            draw_elbow(&mut g, d, e);
        }
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
        let (u, _, au, al) = d.edges[e].vertical(&d.nodes);
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
        // Whether one of the bus's edges points an arrow head up into shape `n`.
        let arrow_up = |n: usize| {
            bus.edges.iter().any(|&e| {
                let (u, l, au, al) = d.edges[e].vertical(&d.nodes);
                (u == n && au) || (l == n && al && same_row(d, e))
            })
        };
        for &u in &bus.uppers {
            let x = d.nodes[u].center();
            for y in top..bus_row {
                g.put(x, y, '│');
            }
            if arrow_up(u) {
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
        // The bus itself, with junctions where lines leave it.
        for x in lo..=hi {
            let up = bus.uppers.iter().any(|&u| d.nodes[u].center() == x);
            let down = bus.lowers.iter().any(|&l| d.nodes[l].center() == x);
            let arms = u8::from(up)
                | (u8::from(down) << 1)
                | (u8::from(x > lo) << 2)
                | (u8::from(x < hi) << 3);
            g.put(x, bus_row, line_char(arms));
        }
        // Labels sit on the segment that belongs to one edge only; a loop's label beside
        // where it leaves its shape.
        for &e in &bus.edges {
            let Some(label) = &d.edges[e].label else {
                continue;
            };
            let (u, l, _, _) = d.edges[e].vertical(&d.nodes);
            let into_l = bus
                .edges
                .iter()
                .filter(|&&o| !same_row(d, o) && d.edges[o].vertical(&d.nodes).1 == l)
                .count();
            if !same_row(d, e) && into_l == 1 {
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
        // Centred over the line, which is what the gap was widened for; before the channels
        // when the gap holds some.
        let (li, ri) = if d.nodes[a].x < d.nodes[b].x {
            (a, b)
        } else {
            (b, a)
        };
        let g1 = d.span[li].1;
        let mut room = right.x.saturating_sub(left.x + left.w);
        if d.span[ri].0 == g1 && d.gaps[g1].channels > 0 {
            room = d.gaps[g1].lead;
        }
        let at = left.x + left.w + room.saturating_sub(label.width()) / 2;
        // Above the line, unless a channel's arm runs there: then below it.
        let taken =
            (at..at + label.width()).any(|x| g.cells[y - 1].get(x).is_some_and(|&c| c != ' '));
        g.text(at, if taken { y + 1 } else { y - 1 }, label);
    }
}

/// An edge that skips rows drawn straight down its empty column; its label sits where a
/// direct edge's would, on the entry row above the lower shape.
fn draw_through(g: &mut Grid, d: &Diagram, e: usize) {
    let (u, l, au, al) = d.edges[e].vertical(&d.nodes);
    let (upper, lower) = (&d.nodes[u], &d.nodes[l]);
    let x = upper.center();
    let top = upper.y + upper.h;
    for y in top..lower.y {
        g.put(x, y, '│');
    }
    if au {
        g.put(x, top, '▲');
    }
    if al {
        g.put(x, lower.y - 1, '▼');
    }
    if let Some(label) = &d.edges[e].label {
        g.text(x + 2, lower.y - 2, label);
    }
}

/// An edge that skips rows drawn as an elbow: straight down the upper shape's empty column,
/// then along the lower shape's row into its side. Its label sits beside the line on the
/// entry row of the region below the upper shape, where a direct edge's would.
fn draw_elbow(g: &mut Grid, d: &Diagram, e: usize) {
    let (u, l, au, al) = d.edges[e].vertical(&d.nodes);
    let (upper, lower) = (&d.nodes[u], &d.nodes[l]);
    let x = upper.center();
    let top = upper.y + upper.h;
    let y = lower.middle();
    for row in top..y {
        g.put(x, row, '│');
    }
    if au {
        g.put(x, top, '▲');
    }
    let left = x < lower.center();
    let edge = if left { lower.x } else { lower.x + lower.w - 1 };
    let xs = if left { x + 1..edge } else { edge + 1..x };
    for col in xs {
        g.put(col, y, '─');
    }
    g.put(x, y, if left { '└' } else { '┘' });
    if al {
        g.put(
            if left { edge - 1 } else { edge + 1 },
            y,
            if left { '▶' } else { '◀' },
        );
    }
    if let Some(label) = &d.edges[e].label {
        let plan = region_plan(d, upper.row);
        let row = if plan.levels > 0 || plan.height > 2 {
            top + plan.height - 2
        } else {
            top
        };
        g.text(x + 2, row, label);
    }
}

/// An edge routed down (or up) a channel in a gap.
fn draw_side(g: &mut Grid, d: &Diagram, route: &Route, col: usize) {
    let (u, l, au, al) = d.edges[route.edge].vertical(&d.nodes);
    let (upper, lower) = (&d.nodes[u], &d.nodes[l]);
    // Whether the channel lies left of each shape: the arms may leave to different sides
    // when the channel runs between their columns.
    let (upper_left, lower_left) = (gap_left_of(d, u, route.gap), gap_left_of(d, l, route.gap));
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
    // The cell of the shape's outline the arm meets on row `y`: a diamond's top and bottom
    // rows are a cell narrower on each side than its middle.
    let boundary = |n: &Node, left: bool, y: usize| match (n.shape, left, y != n.middle()) {
        (Shape::Diamond, true, true) => n.x + 1,
        (Shape::Diamond, false, true) => n.x + n.w - 2,
        (_, true, _) => n.x,
        (_, false, _) => n.x + n.w - 1,
    };
    // The arm from the shape to the channel, ending in an arrow head or, when it leaves a
    // box's corner, joined to it.
    let arm = |g: &mut Grid, n: &Node, left: bool, y: usize, headed: bool, junction: char| {
        let edge = boundary(n, left, y);
        let xs = if left { col + 1..edge } else { edge + 1..col };
        for x in xs {
            g.put(x, y, '─');
        }
        if headed {
            g.put(
                if left { edge - 1 } else { edge + 1 },
                y,
                if left { '▶' } else { '◀' },
            );
        } else if y != n.middle() && n.shape != Shape::Diamond {
            g.set(edge, y, junction);
        }
    };
    arm(g, upper, upper_left, uy, au, '┴');
    arm(g, lower, lower_left, ly, al, '┬');
    for y in uy + 1..ly {
        g.put(col, y, '│');
    }
    g.put(col, uy, if upper_left { '┌' } else { '┐' });
    g.put(col, ly, if lower_left { '└' } else { '┘' });
    if let Some(label) = &d.edges[route.edge].label {
        let y = uy.midpoint(ly);
        if route.gap == 0 {
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

    #[test]
    fn a_skipping_line_runs_straight_through_an_empty_column() {
        // The "No" line from the diamond skips the row holding only the pill, whose empty
        // slot leaves the column free: it goes straight down instead of round the side.
        let src = "a<Need affinity?>, b<Gift given?>, c[Give a gift]\n, d(Grind)\ne<Wear an item?>, f<In the bag?>, g[Equip]\na --> b | Yes\nb --> c | No\nb --> d | Yes\na --> e | No\ne --> f | Yes\nf --> g | Yes";
        let out = render(src).unwrap();
        let text = joined(&out);
        println!("{text}");
        assert_eq!(
            &out[3..12],
            &[
                "          │                      │",
                "          │                      │ Yes",
                "          │                      ▼",
                "          │                  ╭───────╮",
                "          │                  │ Grind │",
                "          │                  ╰───────╯",
                "          │",
                "          │ No",
                "          ▼",
            ]
        );
        // Nothing left of the shapes or beyond the widest row: no line goes round.
        assert!(
            out.iter()
                .all(|l| l.is_empty() || l.starts_with(' ') || l.starts_with('⟨'))
        );
        assert!(out.iter().all(|l| l.width() <= out[0].width()), "{text}");
        // Grind spread over the whole row blocks the column: the line goes down the left
        // margin, touching no shape.
        let out = render(&src.replace(", d(Grind)", "d(Grind)")).unwrap();
        let text = joined(&out);
        println!("{text}");
        assert!(
            out[1].starts_with("    ┌──⟨  Need affinity?  ⟩"),
            "{}",
            out[1]
        );
        assert!(text.contains("\n No │"), "label beside the channel");
        let wear = out
            .iter()
            .position(|l| l.contains("Wear an item?"))
            .unwrap();
        assert!(
            out[wear].starts_with("    └──▶⟨  Wear an item?  ⟩"),
            "{}",
            out[wear]
        );
    }

    #[test]
    fn lines_never_cross_a_shape() {
        // From the right column to the left column two rows down: the only gap reachable
        // from both ends without crossing a shape is the one between the columns, which
        // widens to hold the channel and its label. The arm leaves Act 1's bottom corner
        // because "yes" arrives at the side it would otherwise use.
        let src = "a<Q1?>, b[Act 1]\nc<Q2?>, d[Act 2]\ne<Q3?>, f[Act 3]\na --> b | yes\na --> c | no\nb --> e | wide\nc --> d\nc --> e\ne --> f";
        let out = render(src).unwrap();
        println!("{}", joined(&out));
        assert_eq!(
            out,
            vec![
                " ╱‾‾‾‾‾╲  yes          ┌───────┐",
                "⟨  Q1?  ⟩─────────────▶│ Act 1 │",
                " ╲_____╱        ┌──────┴───────┘",
                "    │           │",
                "    │ no        │",
                "    ▼           │",
                " ╱‾‾‾‾‾╲        │ wide ┌───────┐",
                "⟨  Q2?  ⟩───────┼─────▶│ Act 2 │",
                " ╲_____╱        │      └───────┘",
                "    │           │",
                "    ▼           │",
                " ╱‾‾‾‾‾╲◀───────┘      ┌───────┐",
                "⟨  Q3?  ⟩─────────────▶│ Act 3 │",
                " ╲_____╱               └───────┘",
            ]
        );
    }

    #[test]
    fn a_line_along_a_row_that_skips_a_shape_loops_under_it() {
        let out = render("a[A], b[B], c[C]\na --> c | over").unwrap();
        println!("{}", joined(&out));
        assert_eq!(
            out,
            vec![
                "┌───┐    ┌───┐    ┌───┐",
                "│ A │    │ B │    │ C │",
                "└───┘    └───┘    └───┘",
                "  │ over            ▲",
                "  └─────────────────┘",
            ]
        );
        // With a row below, the loop shares a bus with the lines going down.
        let out = render("a[A], b[B], c[C]\nd[D]\na --> c\na --> d\nc --> d").unwrap();
        println!("{}", joined(&out));
        assert_eq!(
            &out[3..7],
            &[
                "  │                 ▲",
                "  └────────┬────────┘",
                "           │",
                "           ▼",
            ]
        );
    }

    #[test]
    fn a_skipping_line_takes_an_elbow_into_a_shape_in_another_column() {
        // "No" skips two rows whose first column is empty: it drops straight down that
        // column and turns into the side of the diamond in the next column, its label where
        // the "Yes" beside it sits.
        let src = "a<Shop nearby?>, b<Known?>\n, c<Short?>\n, d<Monster?>\na --> b | Yes\nb --> c | Yes\nc --> d | No\na --> d | No";
        let out = render(src).unwrap();
        println!("{}", joined(&out));
        assert_eq!(
            out,
            vec![
                " ╱‾‾‾‾‾‾‾‾‾‾‾‾‾‾╲  Yes   ╱‾‾‾‾‾‾‾‾╲",
                "⟨  Shop nearby?  ⟩─────▶⟨  Known?  ⟩",
                " ╲______________╱        ╲________╱",
                "         │                    │",
                "         │ No                 │ Yes",
                "         │                    ▼",
                "         │               ╱‾‾‾‾‾‾‾‾╲",
                "         │              ⟨  Short?  ⟩",
                "         │               ╲________╱",
                "         │                    │",
                "         │                    │ No",
                "         │                    ▼",
                "         │              ╱‾‾‾‾‾‾‾‾‾‾╲",
                "         └────────────▶⟨  Monster?  ⟩",
                "                        ╲__________╱",
            ]
        );
        // The mirror image turns left into the shape's right side.
        let out =
            render("a<A?>, b<B?>\nc<C?>,\nd<D?>,\nb --> a | Yes\na --> c\nc --> d\nb --> d | No")
                .unwrap();
        println!("{}", joined(&out));
        assert_eq!(
            &out[9..13],
            &[
                "    │            │",
                "    ▼            │",
                " ╱‾‾‾‾╲          │",
                "⟨  D?  ⟩◀────────┘"
            ]
        );
        // A shape in the column on the lower row leaves no room for the arm: the line takes
        // a channel between the columns instead.
        let out =
            render("a<A?>, b<B?>\n, c<C?>\ne[E], d<D?>\na --> b\nb --> c\nc --> d\na --> d | No")
                .unwrap();
        println!("{}", joined(&out));
        assert_eq!(out[2], " ╲____╱───┐     ╲____╱");
        assert_eq!(out[11], "  │ E │   └───▶⟨  D?  ⟩");
        // Two skipping lines from one shape: the first takes the column, the second goes
        // round.
        let out = render("a<A?>, b<B?>\n, c<C?>\n, d<D?>\n, f<F?>\na --> b\nb --> c\nc --> d\nd --> f\na --> d | one\na --> f | two")
            .unwrap();
        println!("{}", joined(&out));
        assert_eq!(out[1], "     ┌──⟨  A?  ⟩───▶⟨  B?  ⟩");
        assert_eq!(out[12], "     │      └──────▶⟨  D?  ⟩");
        assert_eq!(out[17], "     └─────────────▶⟨  F?  ⟩");
    }
}
