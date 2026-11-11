use anyhow::{Context, Result};
use cells::app::{App, DragState, Modal};
use cells::config::Config;
use cells::cursor::{emergency_cursor_cleanup, probe_terminal_cursor_caps};
use cells::input::handle_event;
use cells::ui;
use cells::workbook::WorkbookState;
use clap::Parser;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyboardEnhancementFlags, MouseEventKind, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui_image::picker::Picker;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "cells",
    version,
    about = "Terminal spreadsheet with classic software cross cursor, menus, Excel formulas, and pixel charts"
)]
struct Cli {
    /// File to open or create (.xlsx, .csv, .tsv, .json, .jsonl, .parquet, .sqlite)
    file: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load();

    let (wb, meta) = if let Some(ref path) = cli.file {
        WorkbookState::open_path(path)?
    } else if !io::stdin().is_terminal() {
        let mut piped = String::new();
        io::stdin()
            .read_to_string(&mut piped)
            .context("failed reading piped stdin")?;
        reattach_controlling_tty()?;
        (WorkbookState::from_stdin_str(&piped)?, None)
    } else {
        (WorkbookState::new_empty()?, None)
    };

    install_panic_hook();
    enable_raw_mode().context("failed to enable terminal raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;

    // Query Kitty / Sixel / iTerm2 capabilities & font pixel dimensions after entering alt screen.
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());

    execute!(stdout, EnableMouseCapture, EnableBracketedPaste)
        .context("failed to enable mouse capture")?;
    let cursor_caps = probe_terminal_cursor_caps(&picker);
    let kbd_enhanced = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok();

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("failed to initialize ratatui terminal")?;
    let mut app = App::new_with_cursor_caps(wb, meta, cfg, picker, cursor_caps);

    let run_res = run_loop(&mut terminal, &mut app);

    if kbd_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    app.cursor_rt.shutdown(terminal.backend_mut());
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = terminal.show_cursor();

    run_res
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    let mut needs_draw = true;
    loop {
        if let Some(ready) = app.chart_worker.poll_ready() {
            app.cached_chart = Some(ready);
            needs_draw = true;
        }

        if needs_draw {
            terminal.draw(|f| ui::draw(f, app))?;
            let _ = app.cursor_rt.sync_terminal_cursor(
                terminal.backend_mut(),
                &app.picker,
                app.layout.screen,
                app.mouse_px,
                app.cursor_shape,
            );
            needs_draw = false;
        }

        app.tick_housekeeping();

        if app.should_quit {
            break;
        }

        if event::poll(Duration::from_millis(25))? {
            let prev_cell = app.mouse_cell;
            let prev_shape = app.cursor_shape;
            let prev_hover = app.chart_hover.clone();
            let prev_completion_idx = app.edit.as_ref().map(|e| e.completion_idx);

            let first = event::read()?;
            let mut only_mouse_moved =
                matches!(first, Event::Mouse(ref m) if matches!(m.kind, MouseEventKind::Moved));
            handle_event(app, first);
            // Drain queued burst events (trackpad scroll / drag / 240Hz pixel move) so rendering stays snappy.
            for _ in 0..32 {
                if app.should_quit || !event::poll(Duration::ZERO)? {
                    break;
                }
                let ev = event::read()?;
                if !matches!(ev, Event::Mouse(ref m) if matches!(m.kind, MouseEventKind::Moved)) {
                    only_mouse_moved = false;
                }
                handle_event(app, ev);
            }

            let subcell_cursor_only = only_mouse_moved
                && matches!(app.drag, DragState::None)
                && app.top_menu.is_none()
                && app.context_menu.is_none()
                && matches!(app.modal, Modal::None)
                && app.mouse_cell == prev_cell
                && app.cursor_shape == prev_shape
                && app.chart_hover == prev_hover
                && app.edit.as_ref().map(|e| e.completion_idx) == prev_completion_idx;

            if subcell_cursor_only {
                let _ = app.cursor_rt.sync_terminal_cursor(
                    terminal.backend_mut(),
                    &app.picker,
                    app.layout.screen,
                    app.mouse_px,
                    app.cursor_shape,
                );
            } else {
                needs_draw = true;
            }
        }
    }
    Ok(())
}

fn install_panic_hook() {
    let orig = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut out = io::stdout();
        emergency_cursor_cleanup(&mut out);
        let _ = disable_raw_mode();
        let _ = execute!(
            out,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        orig(info);
    }));
}

#[cfg(unix)]
fn reattach_controlling_tty() -> Result<()> {
    use std::os::fd::AsRawFd;
    let tty = std::fs::File::open("/dev/tty")
        .context("stdin was piped, and /dev/tty could not be opened for interactive input")?;
    let rc = unsafe { libc::dup2(tty.as_raw_fd(), libc::STDIN_FILENO) };
    if rc < 0 {
        return Err(anyhow::anyhow!(
            "dup2(/dev/tty, STDIN) failed: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn reattach_controlling_tty() -> Result<()> {
    Ok(())
}
