//! wikiBits: a terminal wiki built on a folder of plain Markdown files.

mod app;
mod editor;
mod markdown;
mod ui;
mod wiki;

use std::io::stdout;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use notify::{EventKind, RecursiveMode, Watcher};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use ratatui_image::picker::Picker;

use crate::app::App;
use crate::wiki::Wiki;

/// A terminal wiki built on plain Markdown files.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Folder holding the wiki's .md files (default: ~/Wiki).
    dir: Option<PathBuf>,
    /// Page to open first, e.g. `projects/calcbits`.
    #[arg(short, long)]
    page: Option<String>,
    /// Print what the terminal reports about image support and cell size, then exit.
    #[arg(long)]
    probe: bool,
}

/// The starter wiki, offered when the default folder does not exist yet.
const STARTER: &[(&str, &[u8])] = &[
    ("index.md", include_bytes!("../examples/wiki/index.md")),
    (
        "getting-started.md",
        include_bytes!("../examples/wiki/getting-started.md"),
    ),
    (
        "notes/index.md",
        include_bytes!("../examples/wiki/notes/index.md"),
    ),
    (
        "notes/markdown-cheatsheet.md",
        include_bytes!("../examples/wiki/notes/markdown-cheatsheet.md"),
    ),
    (
        "assets/calcbits.png",
        include_bytes!("../examples/wiki/assets/calcbits.png"),
    ),
];

/// Ask on the plain terminal whether to create `dir` from the starter wiki; true if created.
fn offer_starter_wiki(dir: &std::path::Path) -> Result<bool> {
    use std::io::{BufRead, Write};
    print!(
        "{} does not exist yet. Create it with a few starter pages? [y/N] ",
        dir.display()
    );
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    if !matches!(answer.trim(), "y" | "Y" | "yes") {
        return Ok(false);
    }
    for (name, bytes) in STARTER {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)?;
    }
    println!("Created {} with {} files.", dir.display(), STARTER.len());
    Ok(true)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.probe {
        return probe();
    }
    let (dir, is_default) = match cli.dir {
        Some(dir) => (dir, false),
        None => (
            PathBuf::from(std::env::var("HOME").context("HOME is not set")?).join("Wiki"),
            true,
        ),
    };
    if !dir.is_dir() && !(is_default && offer_starter_wiki(&dir)?) {
        bail!(
            "{} is not a folder. Create it, or pass the folder holding your .md files.",
            dir.display()
        );
    }
    let wiki = Wiki::load(&dir)?;

    let mut terminal = ratatui::init();
    let picker = query_image_support();
    let _ = execute!(stdout(), EnableMouseCapture, EnableBracketedPaste);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
        hook(info);
    }));

    let result = run(&mut terminal, wiki, picker, cli.page);

    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

/// Show how the terminal answers the image and cell-size queries, for debugging image sizing.
fn probe() -> Result<()> {
    use std::fmt::Write as _;
    let window = ratatui::crossterm::terminal::window_size().ok();
    let mut terminal = ratatui::init();
    let picker = Picker::from_query_stdio();
    ratatui::restore();
    drop(terminal.clear());
    let mut out = String::new();
    match picker {
        Ok(p) => {
            let _ = writeln!(out, "protocol: {:?}", p.protocol_type());
            let _ = writeln!(out, "font size (px): {:?}", p.font_size());
        }
        Err(e) => {
            let _ = writeln!(out, "query failed: {e}");
        }
    }
    if let Some(w) = window {
        let _ = writeln!(
            out,
            "window: {} cols x {} rows, {} x {} px -> {}x{} px per cell",
            w.columns,
            w.rows,
            w.width,
            w.height,
            w.width.checked_div(w.columns).unwrap_or(0),
            w.height.checked_div(w.rows).unwrap_or(0)
        );
    }
    for var in ["TERM", "TMUX", "GDK_SCALE", "WAYLAND_DISPLAY"] {
        let _ = writeln!(out, "{var}={}", std::env::var(var).unwrap_or_default());
    }
    print!("{out}");
    // Terminal queries need the real stdout, so results can also go to a file.
    if let Ok(path) = std::env::var("WIKIBITS_PROBE_OUT") {
        let _ = std::fs::write(path, &out);
    }
    Ok(())
}

/// Ask the terminal which image protocol it speaks (sixel in foot). Must run before any events
/// are read.
///
/// A freshly opened window may answer before it is mapped, with a placeholder 80x24 size and
/// cells far larger than the real ones, so wait until the terminal reports a real pixel size
/// and take the cell size from that (see [`app::font_size_from_window`]).
///
/// Not inside tmux: unless passthrough is enabled, tmux swallows the query, and the library's
/// reader thread then waits on stdin forever and eats the first keystroke. Images fall back to
/// block characters there.
fn query_image_support() -> Option<Picker> {
    if std::env::var_os("TMUX").is_some() {
        return Some(Picker::halfblocks());
    }
    let start = std::time::Instant::now();
    while app::font_size_from_window().is_none() && start.elapsed() < Duration::from_millis(1500) {
        std::thread::sleep(Duration::from_millis(25));
    }
    let mut picker = Picker::from_query_stdio().ok()?;
    if let Some(font_size) = app::font_size_from_window() {
        picker = app::picker_with_font_size(&picker, font_size);
    }
    Some(picker)
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    wiki: Wiki,
    picker: Option<Picker>,
    page: Option<String>,
) -> Result<()> {
    let root = wiki.root.clone();
    let mut app = App::new(wiki, picker, page);

    // Reload when files under the wiki folder change. A failing watcher only costs live reload.
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .ok();
    if let Some(w) = &mut watcher
        && w.watch(&root, RecursiveMode::Recursive).is_err()
    {
        app.status = "File watching unavailable; press r to reload".into();
    }

    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;
        if app.should_quit {
            break;
        }
        let had_overlay = app.popup.is_some() || app.show_help || app.search.is_some();
        // Handle everything that is already waiting before drawing again, so a burst of
        // mouse drag or wheel events does not cost one redraw each.
        let mut wait = Duration::from_millis(250);
        while event::poll(wait)? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.handle_key(key),
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                Event::Paste(text) => app.handle_paste(&text),
                _ => {}
            }
            wait = Duration::ZERO;
        }
        // An overlay that covered an inline image leaves its pixels behind: the cells under an
        // image are never rewritten, so the terminal keeps showing what was drawn there last.
        // Repaint everything when an overlay goes away.
        if (had_overlay && app.popup.is_none() && !app.show_help && app.search.is_none())
            || std::mem::take(&mut app.repaint)
        {
            terminal.clear()?;
        }
        app.refresh_font_size();
        let mut changed = false;
        while let Ok(res) = rx.try_recv() {
            if let Ok(ev) = res
                && !matches!(ev.kind, EventKind::Access(_))
            {
                changed = true;
            }
        }
        if changed {
            app.reload();
        }
    }
    Ok(())
}
