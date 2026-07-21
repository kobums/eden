//! 테마 색과 ANSI 색 변환.

use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};

/// 설정에서 온 배경/전경/선택/커서 색 + 16색 ANSI 팔레트.
#[derive(Clone, Copy)]
pub(super) struct Theme {
    pub(super) bg: [f32; 3],
    pub(super) fg: [f32; 3],
    pub(super) selection: [f32; 3],
    pub(super) cursor: [f32; 3],
    pub(super) palette: [[f32; 3]; 16],
    pub(super) cursor_style: crate::config::CursorStyle,
    /// 기본 배경 불투명도 (셀 배경색·텍스트는 항상 불투명 — iTerm2와 동일)
    pub(super) opacity: f32,
}

impl Theme {
    pub(super) fn from_config(config: &crate::config::Config) -> Self {
        Self {
            bg: config.background,
            fg: config.foreground,
            selection: config.selection,
            cursor: config.cursor,
            palette: config.palette,
            cursor_style: config.cursor_style,
            opacity: config.background_opacity,
        }
    }

    /// UI 크롬(탭 바·상태바·페인 사이 틈) 배경색. 터미널 배경보다 어둡게.
    pub(super) fn chrome(&self) -> [f32; 3] {
        mix(self.bg, [0.0; 3], 0.35)
    }

    /// 활성 탭 배경색. 크롬보다 살짝 밝게.
    pub(super) fn active_tab(&self) -> [f32; 3] {
        mix(self.bg, [1.0; 3], 0.10)
    }

    /// 탭 사이 구분선 색.
    pub(super) fn separator(&self) -> [f32; 3] {
        mix(self.bg, [1.0; 3], 0.22)
    }

    /// 비활성 탭 제목 색. 전경색을 배경 쪽으로 죽인 것.
    pub(super) fn inactive_fg(&self) -> [f32; 3] {
        mix(self.fg, self.bg, 0.45)
    }
}

/// ANSI 색 → RGB. 설정 팔레트(16색)와 256색 확장을 지원한다.
pub(super) fn ansi_to_rgb(color: &AnsiColor, theme: &Theme) -> [f32; 3] {
    match color {
        AnsiColor::Spec(rgb) => [
            rgb.r as f32 / 255.0,
            rgb.g as f32 / 255.0,
            rgb.b as f32 / 255.0,
        ],
        AnsiColor::Named(named) => named_color(*named, theme),
        AnsiColor::Indexed(idx) => indexed_color(*idx, theme),
    }
}

fn named_color(named: NamedColor, theme: &Theme) -> [f32; 3] {
    let p = &theme.palette;
    match named {
        NamedColor::Black | NamedColor::DimBlack => p[0],
        NamedColor::Red | NamedColor::DimRed => p[1],
        NamedColor::Green | NamedColor::DimGreen => p[2],
        NamedColor::Yellow | NamedColor::DimYellow => p[3],
        NamedColor::Blue | NamedColor::DimBlue => p[4],
        NamedColor::Magenta | NamedColor::DimMagenta => p[5],
        NamedColor::Cyan | NamedColor::DimCyan => p[6],
        NamedColor::White | NamedColor::DimWhite => p[7],
        NamedColor::BrightBlack => p[8],
        NamedColor::BrightRed => p[9],
        NamedColor::BrightGreen => p[10],
        NamedColor::BrightYellow => p[11],
        NamedColor::BrightBlue => p[12],
        NamedColor::BrightMagenta => p[13],
        NamedColor::BrightCyan => p[14],
        NamedColor::BrightWhite => p[15],
        NamedColor::Background => theme.bg,
        NamedColor::Foreground | NamedColor::BrightForeground => theme.fg,
        NamedColor::Cursor => theme.cursor,
        _ => theme.fg,
    }
}

fn indexed_color(idx: u8, theme: &Theme) -> [f32; 3] {
    match idx {
        0..=15 => theme.palette[idx as usize],
        16..=231 => {
            let i = idx as u32 - 16;
            let steps = [0u8, 95, 135, 175, 215, 255];
            let r = steps[(i / 36) as usize];
            let g = steps[((i % 36) / 6) as usize];
            let b = steps[(i % 6) as usize];
            rgb8(r, g, b)
        }
        232..=255 => {
            let v = 8 + (idx as u32 - 232) * 10;
            rgb8(v as u8, v as u8, v as u8)
        }
    }
}

fn rgb8(r: u8, g: u8, b: u8) -> [f32; 3] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

/// 두 색을 t(0~1)로 섞는다. UI 크롬 색을 테마에서 파생할 때 사용.
fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}
