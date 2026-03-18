/// input.rs — Keyboard and mouse input handling

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use crate::config::ProcSort;
use crate::ui::{Box_, UiState};

#[derive(Debug, Clone)]
pub enum Action {
    Quit,
    NextPreset,
    PrevPreset,
    FocusBox(Box_),
    ScrollUp,
    ScrollDown,
    SortBy(ProcSort),
    ToggleReverse,
    ToggleTree,
    KillProcess,
    NextTheme,
    Refresh,
    ShowHelp,
    None,
}

/// Map a terminal event to an app action.
pub fn handle_event(event: Event, state: &UiState) -> Action {
    match event {
        Event::Key(key) => handle_key(key, state),
        Event::Mouse(mouse) => handle_mouse(mouse),
        _ => Action::None,
    }
}

fn handle_key(key: KeyEvent, _state: &UiState) -> Action {
    // Global quit — Ctrl-C always works
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Action::Quit;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => Action::Quit,

        // Navigation
        KeyCode::Tab => Action::NextPreset,
        KeyCode::BackTab => Action::PrevPreset,

        // Box focus
        KeyCode::Char('1') => Action::FocusBox(Box_::Cpu),
        KeyCode::Char('2') => Action::FocusBox(Box_::Memory),
        KeyCode::Char('3') => Action::FocusBox(Box_::Network),
        KeyCode::Char('4') => Action::FocusBox(Box_::Processes),

        // Scroll in process list
        KeyCode::Up | KeyCode::Char('k') => Action::ScrollUp,
        KeyCode::Down | KeyCode::Char('j') => Action::ScrollDown,

        // Process sorting
        KeyCode::Char('c') => Action::SortBy(ProcSort::Cpu),
        KeyCode::Char('m') => Action::SortBy(ProcSort::Memory),
        KeyCode::Char('p') => Action::SortBy(ProcSort::Pid),
        KeyCode::Char('n') => Action::SortBy(ProcSort::Name),
        KeyCode::Char('t') => Action::SortBy(ProcSort::Threads),

        // Toggle reverse
        KeyCode::Char('r') => Action::ToggleReverse,

        // Process tree
        KeyCode::Char('e') => Action::ToggleTree,

        // Kill selected process (sends SIGTERM, not SIGKILL by default)
        KeyCode::Char('K') => Action::KillProcess,

        // Theme cycling
        KeyCode::Char('T') => Action::NextTheme,

        // Force refresh
        KeyCode::Char('R') | KeyCode::F(5) => Action::Refresh,

        // Help
        KeyCode::Char('?') | KeyCode::Char('h') | KeyCode::F(1) => Action::ShowHelp,

        _ => Action::None,
    }
}

fn handle_mouse(mouse: MouseEvent) -> Action {
    match mouse.kind {
        MouseEventKind::ScrollUp => Action::ScrollUp,
        MouseEventKind::ScrollDown => Action::ScrollDown,
        _ => Action::None,
    }
}
