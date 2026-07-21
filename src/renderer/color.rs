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

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::Rgb;

    fn theme() -> Theme {
        Theme::from_config(&crate::config::Config::default())
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    /// 색의 상대 밝기 — 명암 방향만 비교하면 되므로 단순 평균으로 충분하다.
    fn luma(c: [f32; 3]) -> f32 {
        (c[0] + c[1] + c[2]) / 3.0
    }

    #[test]
    fn mix_endpoints_and_midpoint() {
        let a = [0.0, 0.5, 1.0];
        let b = [1.0, 0.5, 0.0];
        assert!(close(mix(a, b, 0.0), a), "t=0이면 첫 색");
        assert!(close(mix(a, b, 1.0), b), "t=1이면 두 번째 색");
        assert!(close(mix(a, b, 0.5), [0.5, 0.5, 0.5]), "t=0.5는 중점");
    }

    #[test]
    fn indexed_passes_through_the_first_sixteen() {
        let t = theme();
        for i in 0..16u8 {
            assert_eq!(
                indexed_color(i, &t),
                t.palette[i as usize],
                "index {i}는 설정 팔레트를 그대로 써야 한다"
            );
        }
    }

    #[test]
    fn indexed_cube_endpoints() {
        let t = theme();
        assert!(
            close(indexed_color(16, &t), [0.0, 0.0, 0.0]),
            "큐브 시작 = 검정"
        );
        assert!(
            close(indexed_color(231, &t), [1.0, 1.0, 1.0]),
            "큐브 끝 = 흰색"
        );
        // 196 = 큐브 인덱스 180 → r=5, g=0, b=0 → 순수 빨강
        assert!(close(indexed_color(196, &t), [1.0, 0.0, 0.0]));
    }

    #[test]
    fn indexed_grayscale_ramp_is_monotonic() {
        let t = theme();
        assert!(close(indexed_color(232, &t), rgb8(8, 8, 8)), "그레이 시작");
        assert!(
            close(indexed_color(255, &t), rgb8(238, 238, 238)),
            "그레이 끝"
        );

        let mut prev = -1.0;
        for i in 232..=255u8 {
            let c = indexed_color(i, &t);
            assert_eq!(c[0], c[1], "회색은 R=G=B");
            assert_eq!(c[1], c[2]);
            assert!(c[0] > prev, "단조 증가해야 한다");
            prev = c[0];
        }
    }

    #[test]
    fn spec_color_passes_through_unchanged() {
        let t = theme();
        let c = ansi_to_rgb(
            &AnsiColor::Spec(Rgb {
                r: 255,
                g: 128,
                b: 0,
            }),
            &t,
        );
        assert_eq!(c[0], 1.0);
        assert!((c[1] - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(c[2], 0.0);
    }

    #[test]
    fn named_colors_map_to_palette_slots() {
        let t = theme();
        assert_eq!(named_color(NamedColor::Red, &t), t.palette[1]);
        assert_eq!(
            named_color(NamedColor::DimRed, &t),
            t.palette[1],
            "Dim도 동일 슬롯"
        );
        assert_eq!(named_color(NamedColor::BrightRed, &t), t.palette[9]);
        assert_eq!(named_color(NamedColor::Background, &t), t.bg);
        assert_eq!(named_color(NamedColor::Foreground, &t), t.fg);
        assert_eq!(named_color(NamedColor::Cursor, &t), t.cursor);
    }

    #[test]
    fn derived_chrome_colors_go_the_right_direction() {
        let t = theme();
        assert!(
            luma(t.chrome()) < luma(t.bg),
            "크롬은 터미널 배경보다 어둡다"
        );
        assert!(luma(t.active_tab()) > luma(t.bg), "활성 탭은 배경보다 밝다");
        assert!(
            luma(t.separator()) > luma(t.active_tab()),
            "구분선이 가장 밝다"
        );
        // 비활성 제목은 전경과 배경 사이에 놓인다.
        let inactive = luma(t.inactive_fg());
        assert!(inactive < luma(t.fg));
        assert!(inactive > luma(t.bg));
    }
}
