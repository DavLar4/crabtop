/// theme.rs — Color theme support compatible with btop++ theme files

use ratatui::style::Color;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Theme {
    pub name: String,
    // Box borders and labels
    pub main_bg: Color,
    pub main_fg: Color,
    pub title: Color,
    pub hi_fg: Color,
    pub selected_bg: Color,
    pub selected_fg: Color,
    // CPU graph colors (gradient low→high)
    pub cpu_start: Color,
    pub cpu_mid: Color,
    pub cpu_end: Color,
    // Memory colors
    pub mem_start: Color,
    pub mem_end: Color,
    pub swap_start: Color,
    pub swap_end: Color,
    // Network
    pub net_download: Color,
    pub net_upload: Color,
    // Process list
    pub proc_misc: Color,
    pub proc_color: Color,
    pub proc_selected: Color,
    // Meter / graph box borders
    pub box_cpu_color: Color,
    pub box_mem_color: Color,
    pub box_net_color: Color,
    pub box_proc_color: Color,
}

impl Theme {
    pub fn default_theme() -> Self {
        Self {
            name: "default".into(),
            main_bg: Color::Reset,
            main_fg: Color::White,
            title: Color::Cyan,
            hi_fg: Color::Yellow,
            selected_bg: Color::DarkGray,
            selected_fg: Color::White,
            cpu_start: Color::Rgb(0, 180, 0),
            cpu_mid: Color::Rgb(220, 180, 0),
            cpu_end: Color::Rgb(220, 60, 0),
            mem_start: Color::Rgb(0, 100, 220),
            mem_end: Color::Rgb(0, 200, 255),
            swap_start: Color::Rgb(100, 0, 200),
            swap_end: Color::Rgb(200, 100, 255),
            net_download: Color::Rgb(0, 180, 100),
            net_upload: Color::Rgb(220, 80, 0),
            proc_misc: Color::Rgb(150, 150, 150),
            proc_color: Color::Rgb(200, 200, 200),
            proc_selected: Color::Rgb(60, 130, 200),
            box_cpu_color: Color::Rgb(60, 160, 100),
            box_mem_color: Color::Rgb(60, 100, 200),
            box_net_color: Color::Rgb(180, 100, 60),
            box_proc_color: Color::Rgb(140, 60, 180),
        }
    }

    pub fn by_name(name: &str) -> Self {
        match name {
            "dracula" => Self::dracula(),
            "gruvbox" => Self::gruvbox(),
            _ => Self::default_theme(),
        }
    }

    fn dracula() -> Self {
        Self {
            name: "dracula".into(),
            main_bg: Color::Rgb(40, 42, 54),
            main_fg: Color::Rgb(248, 248, 242),
            title: Color::Rgb(189, 147, 249),
            hi_fg: Color::Rgb(255, 184, 108),
            selected_bg: Color::Rgb(68, 71, 90),
            selected_fg: Color::Rgb(248, 248, 242),
            cpu_start: Color::Rgb(80, 250, 123),
            cpu_mid: Color::Rgb(255, 184, 108),
            cpu_end: Color::Rgb(255, 85, 85),
            mem_start: Color::Rgb(139, 233, 253),
            mem_end: Color::Rgb(98, 114, 164),
            swap_start: Color::Rgb(189, 147, 249),
            swap_end: Color::Rgb(255, 121, 198),
            net_download: Color::Rgb(80, 250, 123),
            net_upload: Color::Rgb(255, 184, 108),
            proc_misc: Color::Rgb(98, 114, 164),
            proc_color: Color::Rgb(248, 248, 242),
            proc_selected: Color::Rgb(68, 71, 90),
            box_cpu_color: Color::Rgb(80, 250, 123),
            box_mem_color: Color::Rgb(139, 233, 253),
            box_net_color: Color::Rgb(255, 184, 108),
            box_proc_color: Color::Rgb(189, 147, 249),
        }
    }

    fn gruvbox() -> Self {
        Self {
            name: "gruvbox".into(),
            main_bg: Color::Rgb(40, 40, 40),
            main_fg: Color::Rgb(235, 219, 178),
            title: Color::Rgb(250, 189, 47),
            hi_fg: Color::Rgb(254, 128, 25),
            selected_bg: Color::Rgb(80, 73, 69),
            selected_fg: Color::Rgb(235, 219, 178),
            cpu_start: Color::Rgb(184, 187, 38),
            cpu_mid: Color::Rgb(250, 189, 47),
            cpu_end: Color::Rgb(251, 73, 52),
            mem_start: Color::Rgb(131, 165, 152),
            mem_end: Color::Rgb(142, 192, 124),
            swap_start: Color::Rgb(177, 98, 134),
            swap_end: Color::Rgb(211, 134, 155),
            net_download: Color::Rgb(142, 192, 124),
            net_upload: Color::Rgb(254, 128, 25),
            proc_misc: Color::Rgb(146, 131, 116),
            proc_color: Color::Rgb(235, 219, 178),
            proc_selected: Color::Rgb(80, 73, 69),
            box_cpu_color: Color::Rgb(184, 187, 38),
            box_mem_color: Color::Rgb(131, 165, 152),
            box_net_color: Color::Rgb(254, 128, 25),
            box_proc_color: Color::Rgb(211, 134, 155),
        }
    }

    /// Map a 0.0–100.0 value to a gradient color between start and end.
    pub fn gradient(start: Color, end: Color, pct: f32) -> Color {
        let t = (pct / 100.0).clamp(0.0, 1.0);
        match (start, end) {
            (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => Color::Rgb(
                lerp_u8(r1, r2, t),
                lerp_u8(g1, g2, t),
                lerp_u8(b1, b2, t),
            ),
            _ => end,
        }
    }
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    // Safe: clamped t, no overflow
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}
