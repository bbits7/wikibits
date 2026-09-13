//! wikiBits: a terminal wiki built on a folder of plain Markdown files.

mod app;
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
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let dir = match cli.dir {
        Some(dir) => dir,
        None => PathBuf::from(std::env::var("HOME").context("HOME is not set")?).join("Wiki"),
    };
    if !dir.is_dir() {
        bail!(
            "{} is not a folder. Create it, or pass the folder holding your .md files.",
            dir.display()
        );
    }
    let wiki = Wiki::load(&dir)?;

    let mut terminal = ratatui::init();
    let picker = query_image_support();
    let _ = execute!(stdout(), EnableMouseCapture);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        hook(info);
    }));

    let result = run(&mut terminal, wiki, picker, cli.page);

    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// Ask the terminal which image protocol it speaks (sixel in foot). Must run before any events
/// are read.
///
/// Not inside tmux: unless passthrough is enabled, tmux swallows the query, and the library's
/// reader thread then waits on stdin forever and eats the first keystroke. Images fall back to
/// block characters there.
fn query_image_support() -> Option<Picker> {
    if std::env::var_os("TMUX").is_some() {
        return Some(Picker::halfblocks());
    }
    Picker::from_query_stdio().ok()
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
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.handle_key(key),
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                _ => {}
            }
        }
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
