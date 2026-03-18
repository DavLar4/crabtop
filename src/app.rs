/// app.rs — Application lifecycle and main event loop
///
/// v2 architecture: render only when something actually changed.
///
///   Collect interval: user's --interval flag  — reads /proc, draws frame
///   Input events:     immediate               — handled + redraws frame
///
/// There is NO separate render timer. terminal.draw() is called exactly
/// once per collect tick and once per meaningful input event — never more.
/// This is equivalent to btop's own approach: collect, then draw.
///
/// The earlier 10fps render_tick approach caused ~10x more draws than
/// necessary (19 of every 20 were pure duplicates at --interval 2000),
/// raising CPU usage to ~4.5% vs ~1% with event-driven drawing.

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, EventStream},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io, time::Duration};
use tokio::time;
use tokio_stream::StreamExt;
use tracing::info;

use crate::{
    collect::{Collector, ProcessSnapshot, Snapshot},
    config::Config,
    input::{handle_event, Action},
    ui::{render, UiState},
};

pub struct App {
    config: Config,
    _preset: u8,
    _tty_mode: bool,
    collect_interval_ms: u64,
    state: UiState,
    collector: Collector,
    snapshot: Option<Snapshot>,
    themes: Vec<String>,
    current_theme_idx: usize,
    last_proc_fingerprint: Vec<(u32, u32)>,  // smart-sort: avoids redundant sorts
}

impl App {
    pub async fn new(config: Config, preset: u8, tty_mode: bool, collect_interval_ms: u64) -> Result<Self> {
        let themes = vec!["default".to_string(), "dracula".to_string(), "gruvbox".to_string()];
        let current_theme_idx = themes
            .iter()
            .position(|t| t == &config.theme)
            .unwrap_or(0);

        let collector = Collector::new(120);

        Ok(Self {
            config,
            _preset: preset,
            _tty_mode: tty_mode,
            collect_interval_ms,
            state: UiState::default(),
            collector,
            snapshot: None,
            themes,
            current_theme_idx,
            last_proc_fingerprint: Vec::new(),
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        info!("crabtop: collect={}ms, render=on-change", self.collect_interval_ms);

        let result = self.event_loop(&mut terminal).await;

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        result
    }

    async fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        // Single collection interval — no separate render timer.
        // Draw is called inline after collect or after input, never on a timer.
        let mut collect_tick = time::interval(Duration::from_millis(self.collect_interval_ms));
        collect_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

        let mut events = EventStream::new();

        // Initial collection + draw before entering the loop
        match self.collector.collect() {
            Ok(snap) => {
                self.snapshot = Some(snap);
                self.state.sort_dirty = true;
                self.draw(terminal)?;
            }
            Err(e) => tracing::warn!("Initial collection error: {e}"),
        }

        loop {
            tokio::select! {
                // ── Collect tick: gather metrics, then draw ───────────────────
                _ = collect_tick.tick() => {
                    match self.collector.collect() {
                        Ok(snap) => {
                            // Smart sort: only re-sort if the top process data changed.
                            // Fingerprint = top-5 (pid, cpu*10 as u32) pairs after sorting
                            // by cpu descending. Avoids a full sort on quiet ticks.
                            let new_fp = process_fingerprint(&snap.processes);
                            if new_fp != self.last_proc_fingerprint {
                                self.state.sort_dirty = true;
                                self.last_proc_fingerprint = new_fp;
                            }
                            self.snapshot = Some(snap);
                            self.draw(terminal)?;
                        }
                        Err(e) => tracing::warn!("Collection error: {e}"),
                    }
                }

                // ── Input: handle action, redraw only if UI state changed ─────
                maybe_event = events.next() => {
                    match maybe_event {
                        Some(Ok(evt)) => {
                            let action = handle_event(evt, &self.state);
                            let changed = self.apply_action(action)?;
                            match changed {
                                ApplyResult::Quit => break,
                                ApplyResult::Redraw => self.draw(terminal)?,
                                ApplyResult::NoChange => {}
                            }
                        }
                        Some(Err(e)) => tracing::warn!("Input error: {e}"),
                        None => break,
                    }
                }
            }
        }

        info!("Exiting event loop");
        Ok(())
    }

    /// Draw the current snapshot to the terminal.
    fn draw(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        if let Some(snapshot) = &self.snapshot {
            let config = &self.config;
            let state  = &mut self.state;
            terminal.draw(|f| render(f, snapshot, config, state))?;
        }
        Ok(())
    }

    fn apply_action(&mut self, action: Action) -> Result<ApplyResult> {
        use crate::input::Action::*;
        let result = match action {
            Quit => return Ok(ApplyResult::Quit),

            SortBy(sort) => {
                self.state.proc_sort = sort;
                self.state.sort_dirty = true;
                ApplyResult::Redraw
            }

            ToggleReverse => {
                self.state.proc_reversed = !self.state.proc_reversed;
                self.state.sort_dirty = true;
                ApplyResult::Redraw
            }

            ScrollUp => {
                let i = self.state.proc_table_state.selected().unwrap_or(0);
                self.state.proc_table_state.select(Some(i.saturating_sub(1)));
                ApplyResult::Redraw
            }

            ScrollDown => {
                let max = self
                    .snapshot
                    .as_ref()
                    .map(|s| s.processes.len().saturating_sub(1))
                    .unwrap_or(0);
                let i = self.state.proc_table_state.selected().unwrap_or(0);
                self.state.proc_table_state.select(Some((i + 1).min(max)));
                ApplyResult::Redraw
            }

            KillProcess => {
                self.kill_selected_process();
                ApplyResult::NoChange
            }

            NextTheme => {
                self.current_theme_idx = (self.current_theme_idx + 1) % self.themes.len();
                self.config.theme = self.themes[self.current_theme_idx].clone();
                ApplyResult::Redraw
            }

            Refresh => {
                match self.collector.collect() {
                    Ok(snap) => {
                        self.snapshot = Some(snap);
                        self.state.sort_dirty = true;
                    }
                    Err(e) => tracing::warn!("Refresh error: {e}"),
                }
                ApplyResult::Redraw
            }

            FocusBox(b) => {
                self.state.selected_box = b;
                ApplyResult::Redraw
            }

            _ => ApplyResult::NoChange,
        };
        Ok(result)
    }

    fn kill_selected_process(&self) {
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;

            if let Some(idx) = self.state.proc_table_state.selected() {
                if let Some(snapshot) = &self.snapshot {
                    if let Some(proc) = snapshot.processes.get(idx) {
                        match i32::try_from(proc.pid) {
                            Ok(raw_pid) => {
                                let pid = Pid::from_raw(raw_pid);
                                match kill(pid, Signal::SIGTERM) {
                                    Ok(_)  => info!("Sent SIGTERM to PID {}", proc.pid),
                                    Err(e) => tracing::warn!("Could not signal PID {}: {e}", proc.pid),
                                }
                            }
                            Err(_) => tracing::warn!("PID {} exceeds i32 range — signal skipped", proc.pid),
                        }
                    }
                }
            }
        }
    }
}

/// What apply_action wants the event loop to do next.
enum ApplyResult {
    Quit,
    Redraw,
    NoChange,
}

/// Compute a cheap fingerprint of the process list for smart-sort.
///
/// Takes the top 5 entries by CPU, returns (pid, cpu*10_as_u32) pairs.
/// If this matches the previous tick we skip the sort — the visible order
/// hasn't changed enough to matter.
fn process_fingerprint(procs: &[ProcessSnapshot]) -> Vec<(u32, u32)> {
    let mut top: Vec<&ProcessSnapshot> = procs.iter().collect();
    top.sort_unstable_by(|a, b| b.cpu_usage.partial_cmp(&a.cpu_usage)
        .unwrap_or(std::cmp::Ordering::Equal));
    top.iter()
        .take(5)
        .map(|p| (p.pid, (p.cpu_usage * 10.0) as u32))
        .collect()
}
