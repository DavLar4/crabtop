/// ui.rs — Terminal UI rendering using ratatui

use crate::collect::{CpuSnapshot, DiskSnapshot, MemSnapshot, NetworkSnapshot, ProcessSnapshot, Snapshot, TempSnapshot};
use crate::config::{Config, ProcSort};
use crate::theme::Theme;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        BarChart, Block, Borders, Cell, Gauge, Paragraph, Row, Sparkline, Table, TableState,
    },
    Frame,
};

pub struct UiState {
    pub proc_table_state: TableState,
    pub proc_sort: ProcSort,
    pub proc_reversed: bool,
    pub selected_box: Box_,
    /// Cached sorted process list — only rebuilt when sort order or data changes
    pub sorted_procs: Vec<ProcessSnapshot>,
    /// Tracks whether a re-sort is needed next frame
    pub sort_dirty: bool,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Box_ {
    Cpu,
    Memory,
    Network,
    Processes,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            proc_table_state: TableState::default(),
            proc_sort: ProcSort::Cpu,
            proc_reversed: false,
            selected_box: Box_::Processes,
            sorted_procs: Vec::new(),
            sort_dirty: true,
        }
    }
}

/// Render the full UI for one frame.
pub fn render(frame: &mut Frame, snapshot: &Snapshot, config: &Config, state: &mut UiState) {
    let theme = Theme::by_name(&config.theme);
    let area = frame.size();

    // Rebuild sorted process list only when flagged dirty (new data or sort change)
    if state.sort_dirty || state.sorted_procs.is_empty() {
        state.sorted_procs = snapshot.processes.clone();
        match state.proc_sort {
            ProcSort::Cpu     => state.sorted_procs.sort_by(|a, b| b.cpu_usage.partial_cmp(&a.cpu_usage).unwrap_or(std::cmp::Ordering::Equal)),
            ProcSort::Memory  => state.sorted_procs.sort_by(|a, b| b.mem_bytes.cmp(&a.mem_bytes)),
            ProcSort::Pid     => state.sorted_procs.sort_by_key(|p| p.pid),
            ProcSort::Name    => state.sorted_procs.sort_by(|a, b| a.name.cmp(&b.name)),
            ProcSort::Threads => state.sorted_procs.sort_by(|a, b| b.threads.cmp(&a.threads)),
        }
        if state.proc_reversed {
            state.sorted_procs.reverse();
        }
        state.sort_dirty = false;
    }

    // ── Layout ───────────────────────────────────────────────────────────────
    // Row 0: title bar
    // Row 1: CPU | MEM
    // Row 2: TEMPS — full width, giving the bar chart maximum horizontal space
    // Row 3: PROCESSES (left, 70%) | DISK stacked over NET (right, 30%)
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),      // title
            Constraint::Percentage(30), // CPU + MEM
            Constraint::Percentage(27), // TEMPS full-width — tall enough for 6+ sensors
            Constraint::Min(0),         // PROCESSES + DISK/NET — takes whatever remains
        ])
        .split(area);

    render_title(frame, rows[0], &theme, snapshot);
    render_top_row(frame, rows[1], snapshot, &theme);
    render_temps(frame, rows[2], &snapshot.temps, &theme);
    render_bottom_row(frame, rows[3], snapshot, &theme, &state.sorted_procs, &mut state.proc_table_state);
}

fn render_title(frame: &mut Frame, area: Rect, theme: &Theme, _snapshot: &Snapshot) {
    let uptime_secs = System::uptime();
    let hours = uptime_secs / 3600;
    let mins = (uptime_secs % 3600) / 60;

    let line = Line::from(vec![
        Span::styled(
            " crabtop ",
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("v{} ", env!("CARGO_PKG_VERSION")),
            Style::default().fg(theme.main_fg),
        ),
        Span::styled(
            " render: on-change ",
            Style::default().fg(theme.hi_fg),
        ),
        Span::styled(
            format!(" Uptime: {hours}h {mins}m "),
            Style::default().fg(theme.hi_fg),
        ),
    ]);
    let para = Paragraph::new(line)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(theme.box_cpu_color)))
        .alignment(Alignment::Center);
    frame.render_widget(para, area);
}

fn render_top_row(frame: &mut Frame, area: Rect, snapshot: &Snapshot, theme: &Theme) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_cpu(frame, cols[0], &snapshot.cpu, theme);
    render_memory(frame, cols[1], &snapshot.memory, theme);
}

fn render_cpu(frame: &mut Frame, area: Rect, cpu: &CpuSnapshot, theme: &Theme) {
    let block = Block::default()
        .title(format!(" CPU: {} ", cpu.brand))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.box_cpu_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(2), Constraint::Min(2)])
        .split(inner);

    // Overall usage gauge
    let cpu_color = Theme::gradient(theme.cpu_start, theme.cpu_end, cpu.total_usage);
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(cpu_color))
        .label(format!("{:.1}%", cpu.total_usage))
        .ratio((cpu.total_usage / 100.0).clamp(0.0, 1.0) as f64);
    frame.render_widget(gauge, rows[0]);

    // Sparkline history — convert VecDeque to Vec for ratatui
    let history_data: Vec<u64> = cpu.history.iter().map(|v| *v as u64).collect();
    let sparkline = Sparkline::default()
        .data(&history_data)
        .style(Style::default().fg(cpu_color));
    frame.render_widget(sparkline, rows[1]);

    // Per-core bars (if enough space)
    // FIX (memory leak): previously used Box::leak() to produce &'static str
    // labels, leaking one allocation per core per frame (~30+ leaks/min).
    // Now we build owned Strings first, then borrow them — zero leaks.
    if !cpu.core_usage.is_empty() {
        let labels: Vec<String> = (0..cpu.core_usage.len())
            .map(|i| format!("C{i}"))
            .collect();
        let bar_data: Vec<(&str, u64)> = cpu
            .core_usage
            .iter()
            .enumerate()
            .map(|(i, v)| (labels[i].as_str(), *v as u64))
            .collect();
        let bars = BarChart::default()
            .data(&bar_data)
            .bar_width(3)
            .bar_gap(1)
            .bar_style(Style::default().fg(theme.cpu_start))
            .value_style(Style::default().fg(Color::Reset));
        frame.render_widget(bars, rows[2]);
    }
}

fn render_memory(frame: &mut Frame, area: Rect, mem: &MemSnapshot, theme: &Theme) {
    let block = Block::default()
        .title(format!(" MEM: {} GiB total ", bytes_to_gib(mem.total_bytes)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.box_mem_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);

    // RAM gauge
    let mem_color = Theme::gradient(theme.mem_start, theme.mem_end, mem.used_pct());
    let ram_gauge = Gauge::default()
        .gauge_style(Style::default().fg(mem_color))
        .label(format!(
            "RAM  {:.1}G / {:.1}G  ({:.1}%)",
            bytes_to_gib(mem.used_bytes),
            bytes_to_gib(mem.total_bytes),
            mem.used_pct()
        ))
        .ratio((mem.used_pct() / 100.0).clamp(0.0, 1.0) as f64);
    frame.render_widget(ram_gauge, rows[0]);

    // Swap gauge
    let swap_color = Theme::gradient(theme.swap_start, theme.swap_end, mem.swap_pct());
    let swap_gauge = Gauge::default()
        .gauge_style(Style::default().fg(swap_color))
        .label(format!(
            "SWAP {:.1}G / {:.1}G  ({:.1}%)",
            bytes_to_gib(mem.swap_used),
            bytes_to_gib(mem.swap_total),
            mem.swap_pct()
        ))
        .ratio((mem.swap_pct() / 100.0).clamp(0.0, 1.0) as f64);
    frame.render_widget(swap_gauge, rows[1]);

    // Memory history sparkline — convert VecDeque to Vec for ratatui
    let hist: Vec<u64> = mem.history.iter().map(|v| *v as u64).collect();
    let sparkline = Sparkline::default()
        .data(&hist)
        .style(Style::default().fg(mem_color));
    frame.render_widget(sparkline, rows[3]);
}

fn render_bottom_row(
    frame: &mut Frame,
    area: Rect,
    snapshot: &Snapshot,
    theme: &Theme,
    procs: &[ProcessSnapshot],
    table_state: &mut TableState,
) {
    // PROCESSES takes the left 70%; DISK and NET are stacked vertically on the right 30%
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(70),  // PROCESSES
            Constraint::Percentage(30),  // DISK + NET
        ])
        .split(area);

    render_processes(frame, cols[0], procs, theme, table_state);

    // Right column: DISK is content-sized at the top; NET fills the remaining
    // height with Min(0) so its bottom edge aligns with PROCESSES.
    let disk_h = (snapshot.disks.len() as u16 + 3).max(5); // data + header + 2 borders
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(disk_h),
            Constraint::Min(0), // NET stretches to fill — bottom aligns with PROCESSES
        ])
        .split(cols[1]);

    render_disks(frame, right[0], &snapshot.disks, theme);
    render_network(frame, right[1], &snapshot.network, theme);
}

fn render_disks(frame: &mut Frame, area: Rect, disks: &[DiskSnapshot], theme: &Theme) {
    let block = Block::default()
        .title(" DISK ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.box_net_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows: Vec<Row> = disks
        .iter()
        .map(|d| {
            let pct = d.used_pct();
            let bar = usage_bar(pct, 10);
            Row::new(vec![
                Cell::from(d.mount.as_str()),
                Cell::from(bar),
                Cell::from(format!("{:.0}%", pct)),
                Cell::from(format!("{:.1}G", bytes_to_gib(d.total_bytes))),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(35),
            Constraint::Percentage(30),
            Constraint::Percentage(15),
            Constraint::Percentage(20),
        ],
    )
    .header(Row::new(["Mount", "Usage", "%", "Size"]).style(Style::default().fg(theme.hi_fg)));
    frame.render_widget(table, inner);
}

fn render_network(frame: &mut Frame, area: Rect, net: &NetworkSnapshot, theme: &Theme) {
    let block = Block::default()
        .title(" NET ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.box_net_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if net.interfaces.is_empty() {
        let para = Paragraph::new("No network interfaces detected");
        frame.render_widget(para, inner);
        return;
    }

    let rows: Vec<Row> = net
        .interfaces
        .iter()
        .map(|iface| {
            Row::new(vec![
                Cell::from(iface.name.as_str()),
                Cell::from(format_speed(iface.rx_bytes_per_sec))
                    .style(Style::default().fg(theme.net_download)),
                Cell::from(format_speed(iface.tx_bytes_per_sec))
                    .style(Style::default().fg(theme.net_upload)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(40),
            Constraint::Percentage(30),
            Constraint::Percentage(30),
        ],
    )
    .header(Row::new(["Interface", "↓ Down", "↑ Up"]).style(Style::default().fg(theme.hi_fg)));
    frame.render_widget(table, inner);
}


fn render_temps(frame: &mut Frame, area: Rect, temps: &[TempSnapshot], theme: &Theme) {
    let block = Block::default()
        .title(" TEMPS ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.box_net_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if temps.is_empty() {
        let para = Paragraph::new("No sensors detected
(install lm-sensors?)")
            .style(Style::default().fg(theme.main_fg));
        frame.render_widget(para, inner);
        return;
    }

    // Scale bars relative to the actual range of readings so small differences
    // are visible. The floor is pinned to the lowest reading minus a small
    // margin; the ceiling is the highest known threshold or the hottest reading
    // plus a margin — whichever is greater. This ensures a 38°C and 46°C
    // sensor never render as the same bar length.
    let min_c = temps.iter().map(|t| t.celsius).fold(f32::INFINITY, f32::min);
    let max_threshold = temps.iter()
        .filter_map(|t| t.crit.or(t.high))
        .fold(0.0f32, f32::max);
    let max_c = temps.iter().map(|t| t.celsius).fold(0.0f32, f32::max);
    // Ceiling: use highest threshold if known, otherwise hottest reading + 20°C headroom
    let scale_max = if max_threshold > max_c { max_threshold } else { max_c + 20.0 };
    // Floor: coolest reading minus 10°C so even the coldest bar shows some fill
    let scale_min = (min_c - 10.0).max(0.0);
    let scale_range = (scale_max - scale_min).max(1.0);

    // Bar width scales with panel width: bar column is ~45% of inner width,
    // minus 2 for the brackets. Clamped to a sensible min/max.
    // Now that TEMPS spans full terminal width this gives ~30-40 fill chars
    // on a typical 200-col terminal, vs the old fixed 12.
    let bar_width = ((inner.width as f32 * 0.45) as usize).saturating_sub(2).clamp(8, 60);

    let rows: Vec<Row> = temps.iter().map(|t| {
        // Colour code by severity
        let temp_color = if t.is_critical() {
            Color::Red
        } else if t.is_high() {
            Color::Yellow
        } else {
            Color::Green
        };

        let temp_str = format!("{:.1}°C", t.celsius);

        // Bar scaled to the actual min–max range of current readings
        let ratio = ((t.celsius - scale_min) / scale_range).clamp(0.0, 1.0);
        let filled = (ratio * bar_width as f32).round() as usize;
        let bar = format!("[{}{}]",
            "█".repeat(filled),
            "░".repeat(bar_width - filled));

        // Threshold annotation
        let thresh = match (t.high, t.crit) {
            (_, Some(c)) => format!("/{:.0}°C!", c),
            (Some(h), _) => format!("/{:.0}°C", h),
            _             => String::new(),
        };

        Row::new(vec![
            Cell::from(t.label.as_str()),
            Cell::from(bar).style(Style::default().fg(temp_color)),
            Cell::from(format!("{}{}", temp_str, thresh))
                .style(Style::default().fg(temp_color)),
        ])
    }).collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(20),  // Sensor label
            Constraint::Percentage(65),  // Bar — wide now that panel spans full width
            Constraint::Percentage(15),  // Temp value
        ],
    )
    .header(Row::new(["Sensor", "Load", "Temp"])
        .style(Style::default().fg(theme.hi_fg)));
    frame.render_widget(table, inner);
}

fn render_processes(
    frame: &mut Frame,
    area: Rect,
    procs: &[ProcessSnapshot],
    theme: &Theme,
    table_state: &mut TableState,
) {
    let block = Block::default()
        .title(format!(" PROCESSES ({}) ", procs.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.box_proc_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header = Row::new(["PID", "Name", "CPU%", "MEM", "Threads", "User"])
        .style(Style::default().fg(theme.hi_fg).add_modifier(Modifier::BOLD));

    // Reuse a single format buffer per row to reduce allocations
    let rows: Vec<Row> = procs
        .iter()
        .map(|p| {
            Row::new(vec![
                Cell::from(p.pid.to_string()),
                Cell::from(p.name.as_str()),
                Cell::from(format!("{:.1}", p.cpu_usage)),
                Cell::from(format_bytes(p.mem_bytes)),
                Cell::from(p.threads.to_string()),
                Cell::from(p.user.as_str()),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Percentage(35),
            Constraint::Length(7),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Percentage(20),
        ],
    )
    .header(header)
    .highlight_style(Style::default().bg(theme.selected_bg).fg(theme.selected_fg))
    .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, inner, table_state);
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn bytes_to_gib(bytes: u64) -> f64 {
    bytes as f64 / 1_073_741_824.0
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1}G", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1}M", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{bytes}B")
    }
}

fn format_speed(bytes_per_sec: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

fn usage_bar(pct: f32, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f32).round() as usize;
    let filled = filled.min(width);
    format!("[{}{}]", "█".repeat(filled), "░".repeat(width - filled))
}

// Re-export sysinfo System for uptime
use sysinfo::System;
