//! 설정 파일: `~/.config/eden/config` (Ghostty식 `key = value`).
//!
//! 설정 없이도 기본값으로 완결된 경험을 준다는 원칙에 따라, 파일이 없으면
//! 전부 기본값을 쓴다. 알 수 없는 키/잘못된 값은 조용히 무시하고 기본값 유지.

use std::path::PathBuf;

/// 커서 모양 (iTerm2의 Cursor Type과 대응: box/vertical bar/underline).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Block,
    Bar,
    Underline,
}

/// macOS 시스템 외양. `theme-light`/`theme-dark` 키가 이 값에 따라 갈린다.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Appearance {
    Light,
    Dark,
}

#[derive(Clone)]
pub struct Config {
    pub font_size: f32,
    /// 폰트 파일 경로(우선). None이면 시스템 기본 후보를 쓴다.
    pub font_path: Option<String>,
    pub scrollback: usize,
    pub background: [f32; 3],
    pub foreground: [f32; 3],
    pub cursor: [f32; 3],
    pub selection: [f32; 3],
    /// 16색 ANSI 팔레트 (0~7 표준, 8~15 밝은색).
    pub palette: [[f32; 3]; 16],
    /// 커서 모양 (기본: 블록 — iTerm2 기본값과 동일).
    pub cursor_style: CursorStyle,
    /// 배경 불투명도 0.2~1.0 (1.0 = 불투명, iTerm2 Transparency 0.0과 동일).
    /// iTerm2처럼 기본 배경에만 적용되고 셀 배경색·텍스트는 불투명을 유지한다.
    pub background_opacity: f32,
    /// 명령 완료 알림 + OSC 9/777 알림 (기본 on).
    pub notify: bool,
    /// 명령 완료 알림의 최소 소요 시간 (초). 이보다 빨리 끝난 명령은 조용히
    /// 넘어간다 — `ls` 같은 짧은 명령마다 울리면 알림이 무의미해진다.
    pub notify_threshold: u64,
    /// Kitty keyboard protocol (CSI u). 인코더는 플래그 5종을 전부 구현했지만
    /// dogfooding 기간의 탈출구로 기본 off — 켜기 전까지는 파서가 프로토콜
    /// 시퀀스를 무시하므로 앱에 지원을 advertise하지 않는다.
    pub kitty_keyboard: bool,
    /// 비활성 페인 디밍 강도 0.0~0.8 (0 = 끔). 분할 시 포커스된 페인이
    /// 어디인지 한눈에 보이게, 나머지 페인의 글자·셀 배경을 배경 쪽으로 죽인다
    /// (iTerm2 "Dim inactive split panes").
    pub inactive_dim: f32,
    /// 블록 상태 거터(왼쪽 세로 줄)를 그릴지.
    pub block_gutter: bool,
    /// 실행 중인 블록의 거터 색.
    pub block_running: [f32; 3],
    /// 성공(exit 0)한 블록의 거터 색.
    pub block_ok: [f32; 3],
    /// 실패한 블록의 거터 색.
    pub block_fail: [f32; 3],
    /// 사용자가 재정의한 키바인딩 (원문 그대로: 코드 문자열, 액션 이름).
    ///
    /// 여기서 파싱까지 하지 않는 이유는 층 분리다 — 키 코드·액션은 winit과
    /// 앱 계층의 개념이라 설정 파서가 알 필요가 없다. 해석과 검증은
    /// `app::action`이 기본 맵을 만들 때 한 번에 한다.
    pub keybinds: Vec<(String, String)>,
}

/// 기본 16색 팔레트 (Catppuccin 계열).
const DEFAULT_PALETTE: [[f32; 3]; 16] = [
    [0.180, 0.180, 0.243], // 0 black   #2e2e3e
    [0.953, 0.545, 0.659], // 1 red     #f38ba8
    [0.651, 0.890, 0.631], // 2 green   #a6e3a1
    [0.976, 0.886, 0.686], // 3 yellow  #f9e2af
    [0.537, 0.706, 0.980], // 4 blue    #89b4fa
    [0.796, 0.651, 0.969], // 5 magenta #cba6f7
    [0.580, 0.886, 0.835], // 6 cyan    #94e2d5
    [0.729, 0.761, 0.871], // 7 white   #bac2de
    [0.345, 0.357, 0.439], // 8  br black   #585b70
    [0.953, 0.545, 0.659], // 9  br red     #f38ba8
    [0.651, 0.890, 0.631], // 10 br green   #a6e3a1
    [0.976, 0.886, 0.686], // 11 br yellow  #f9e2af
    [0.537, 0.706, 0.980], // 12 br blue    #89b4fa
    [0.796, 0.651, 0.969], // 13 br magenta #cba6f7
    [0.580, 0.886, 0.835], // 14 br cyan    #94e2d5
    [1.0, 1.0, 1.0],       // 15 br white   #ffffff
];

impl Default for Config {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            font_path: None,
            scrollback: 10_000,
            background: [0.086, 0.086, 0.11],
            foreground: [0.85, 0.85, 0.87],
            cursor: [0.85, 0.85, 0.87],
            selection: [0.23, 0.33, 0.48],
            palette: DEFAULT_PALETTE,
            cursor_style: CursorStyle::Block,
            background_opacity: 1.0,
            notify: true,
            notify_threshold: 10,
            kitty_keyboard: false,
            // 기본 끔 — 기존 사용자의 화면을 조용히 바꾸지 않는다. 켤 때는
            // 0.2 정도가 은은하다 (config.example 참고).
            inactive_dim: 0.0,
            block_gutter: true,
            // 파랑(실행 중)·초록(성공)·빨강(실패). Phase 4부터 렌더러에
            // 하드코딩돼 있던 값을 그대로 기본값으로 옮긴 것이다.
            block_running: [0.35, 0.55, 0.95],
            block_ok: [0.35, 0.72, 0.46],
            block_fail: [0.92, 0.42, 0.46],
            keybinds: Vec::new(),
        }
    }
}

impl Config {
    /// 설정 파일을 읽어 파싱한다. 없으면 기본값. 외양은 다크로 간주한다 —
    /// 창이 생기기 전(외양을 모를 때)의 초기 로드용이고, eden 기본 테마가
    /// 다크이므로 자연스러운 쪽이다.
    pub fn load() -> Self {
        Self::load_for(Appearance::Dark)
    }

    /// 시스템 외양에 맞춰 설정 파일을 읽는다. `theme-light`/`theme-dark` 줄은
    /// 해당 외양일 때만 적용된다 (라이트/다크 자동 전환).
    pub fn load_for(appearance: Appearance) -> Self {
        let Some(path) = config_path() else {
            return Config::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Config::default();
        };
        Config::parse_for(&text, appearance)
    }

    /// 설정 텍스트를 파싱한다 (다크 외양). 테스트 전용 축약 — 실행 경로는
    /// 항상 외양을 아는 `parse_for`를 지난다.
    #[cfg(test)]
    pub fn parse(text: &str) -> Self {
        Self::parse_for(text, Appearance::Dark)
    }

    /// 설정 텍스트를 주어진 외양으로 파싱한다.
    pub fn parse_for(text: &str, appearance: Appearance) -> Self {
        let mut config = Config::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            config.apply(key, value, appearance);
        }
        config
    }

    /// 이름있는 컬러 프리셋을 적용한다. `theme = <name>`. 개별 색 키가 뒤에
    /// 오면 그 값이 프리셋을 덮어쓴다.
    fn apply_preset(&mut self, name: &str) {
        let preset = match name.to_lowercase().as_str() {
            "guezwhoz" => &GUEZWHOZ,
            "catppuccin-latte" | "latte" => &CATPPUCCIN_LATTE,
            "catppuccin" => return, // 기본값이 이미 catppuccin
            _ => return,            // 알 수 없는 프리셋 무시
        };
        if let Some(c) = parse_hex(preset.background) {
            self.background = c;
        }
        if let Some(c) = parse_hex(preset.foreground) {
            self.foreground = c;
        }
        if let Some(c) = parse_hex(preset.cursor) {
            self.cursor = c;
        }
        if let Some(c) = parse_hex(preset.selection) {
            self.selection = c;
        }
        for (i, hex) in preset.palette.iter().enumerate() {
            if let Some(c) = parse_hex(hex) {
                self.palette[i] = c;
            }
        }
    }

    fn apply(&mut self, key: &str, value: &str, appearance: Appearance) {
        match key {
            "theme" => self.apply_preset(value),
            // 시스템 외양별 프리셋 — 맞지 않는 외양의 줄은 통째로 무시된다.
            // `theme`과 같은 순서 규칙: 뒤에 오는 개별 색 키가 이긴다.
            "theme-light" => {
                if appearance == Appearance::Light {
                    self.apply_preset(value);
                }
            }
            "theme-dark" => {
                if appearance == Appearance::Dark {
                    self.apply_preset(value);
                }
            }
            "font-size" => {
                if let Ok(v) = value.parse::<f32>()
                    && (6.0..=72.0).contains(&v)
                {
                    self.font_size = v;
                }
            }
            "font-path" => self.font_path = Some(value.to_string()),
            "scrollback" => {
                if let Ok(v) = value.parse::<usize>() {
                    self.scrollback = v.min(1_000_000);
                }
            }
            "background" => {
                if let Some(c) = parse_hex(value) {
                    self.background = c;
                }
            }
            "foreground" => {
                if let Some(c) = parse_hex(value) {
                    self.foreground = c;
                }
            }
            "cursor-color" => {
                if let Some(c) = parse_hex(value) {
                    self.cursor = c;
                }
            }
            "selection-color" => {
                if let Some(c) = parse_hex(value) {
                    self.selection = c;
                }
            }
            "cursor-style" => {
                self.cursor_style = match value.to_lowercase().as_str() {
                    "bar" | "beam" => CursorStyle::Bar,
                    "underline" => CursorStyle::Underline,
                    _ => CursorStyle::Block,
                };
            }
            "notify" => match value.to_lowercase().as_str() {
                "on" => self.notify = true,
                "off" => self.notify = false,
                _ => {} // 잘못된 값은 기본값 유지
            },
            "notify-threshold" => {
                if let Ok(v) = value.parse::<u64>() {
                    self.notify_threshold = v;
                }
            }
            "background-opacity" => {
                if let Ok(v) = value.parse::<f32>()
                    && (0.2..=1.0).contains(&v)
                {
                    self.background_opacity = v;
                }
            }
            "kitty-keyboard" => match value.to_lowercase().as_str() {
                "on" | "true" => self.kitty_keyboard = true,
                "off" | "false" => self.kitty_keyboard = false,
                _ => {} // 알 수 없는 값은 무시 — 기본값 유지
            },
            "inactive-dim" => {
                if let Ok(v) = value.parse::<f32>()
                    && (0.0..=0.8).contains(&v)
                {
                    self.inactive_dim = v;
                }
            }
            "block-gutter" => match value.to_lowercase().as_str() {
                "on" | "true" => self.block_gutter = true,
                "off" | "false" => self.block_gutter = false,
                _ => {}
            },
            "block-running-color" => {
                if let Some(c) = parse_hex(value) {
                    self.block_running = c;
                }
            }
            "block-ok-color" => {
                if let Some(c) = parse_hex(value) {
                    self.block_ok = c;
                }
            }
            "block-fail-color" => {
                if let Some(c) = parse_hex(value) {
                    self.block_fail = c;
                }
            }
            // `keybind = cmd+t = new-tab` — 값 안에 두 번째 `=`가 있다.
            // 검증 없이 원문만 모은다 (해석은 app::action).
            "keybind" => {
                if let Some((chord, action)) = value.split_once('=') {
                    self.keybinds
                        .push((chord.trim().to_string(), action.trim().to_string()));
                }
            }
            // palette-0 ~ palette-15: 16색 ANSI 팔레트
            _ if key.starts_with("palette-") => {
                if let Ok(idx) = key["palette-".len()..].parse::<usize>()
                    && idx < 16
                    && let Some(c) = parse_hex(value)
                {
                    self.palette[idx] = c;
                }
            }
            _ => {} // 알 수 없는 키 무시
        }
    }
}

/// 이름있는 컬러 프리셋 (hex 문자열, 로드 시 파싱).
struct Preset {
    background: &'static str,
    foreground: &'static str,
    cursor: &'static str,
    selection: &'static str,
    palette: [&'static str; 16],
}

/// Catppuccin Latte — 라이트 프리셋. `theme-light`와 짝을 이루라고 넣었다
/// (기본 팔레트가 Catppuccin Mocha 계열이라 라이트 짝도 같은 패밀리로).
const CATPPUCCIN_LATTE: Preset = Preset {
    background: "#eff1f5",
    foreground: "#4c4f69",
    cursor: "#dc8a78",
    selection: "#acb0be",
    palette: [
        "#5c5f77", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#acb0be",
        "#6c6f85", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#bcc0cc",
    ],
};

/// Guezwhoz — iTerm2 다크 프리셋.
const GUEZWHOZ: Preset = Preset {
    background: "#1d1d1d",
    foreground: "#d9d9d9",
    cursor: "#99d4b1",
    selection: "#245354",
    palette: [
        "#333333", "#e85181", "#7ad694", "#b7d074", "#5aa0d6", "#9a90e0", "#58d6ce", "#d9d9d9",
        "#808080", "#e85181", "#afd7af", "#d1ed85", "#64b2ed", "#a398ed", "#61ede4", "#ededed",
    ],
};

fn config_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config/eden/config"))
}

/// `#rrggbb` 또는 `rrggbb`를 [0,1] RGB로 파싱한다.
fn parse_hex(s: &str) -> Option<[f32; 3]> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_accepts_both_prefixed_and_bare() {
        assert_eq!(parse_hex("#000000"), Some([0.0, 0.0, 0.0]));
        assert_eq!(parse_hex("ffffff"), Some([1.0, 1.0, 1.0]));
        let c = parse_hex("#ff8000").unwrap();
        assert_eq!(c[0], 1.0);
        assert!((c[1] - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(c[2], 0.0);
    }

    #[test]
    fn parse_hex_rejects_bad_input() {
        assert_eq!(parse_hex("#12345"), None, "5자리");
        assert_eq!(parse_hex("1234567"), None, "7자리");
        assert_eq!(parse_hex("zzzzzz"), None, "16진수 아님");
        assert_eq!(parse_hex(""), None);
    }

    #[test]
    fn empty_input_yields_defaults() {
        let c = Config::parse("");
        let d = Config::default();
        assert_eq!(c.font_size, d.font_size);
        assert_eq!(c.scrollback, d.scrollback);
        assert_eq!(c.background, d.background);
    }

    #[test]
    fn font_size_is_range_checked() {
        assert_eq!(Config::parse("font-size = 18").font_size, 18.0);
        assert_eq!(Config::parse("font-size = 6").font_size, 6.0, "하한 포함");
        assert_eq!(Config::parse("font-size = 72").font_size, 72.0, "상한 포함");

        let default = Config::default().font_size;
        assert_eq!(
            Config::parse("font-size = 3").font_size,
            default,
            "너무 작음"
        );
        assert_eq!(
            Config::parse("font-size = 200").font_size,
            default,
            "너무 큼"
        );
        assert_eq!(Config::parse("font-size = abc").font_size, default);
    }

    #[test]
    fn background_opacity_is_range_checked() {
        assert_eq!(
            Config::parse("background-opacity = 0.5").background_opacity,
            0.5
        );
        assert_eq!(
            Config::parse("background-opacity = 0.2").background_opacity,
            0.2
        );

        let default = Config::default().background_opacity;
        assert_eq!(
            Config::parse("background-opacity = 0.1").background_opacity,
            default,
            "0.2 미만은 거부 — 창이 사실상 안 보이게 된다"
        );
        assert_eq!(
            Config::parse("background-opacity = 1.5").background_opacity,
            default
        );
    }

    #[test]
    fn notify_accepts_on_off_and_keeps_default_otherwise() {
        assert!(Config::parse("notify = on").notify);
        assert!(!Config::parse("notify = off").notify);
        assert!(!Config::parse("notify = OFF").notify, "대소문자 무시");
        assert!(Config::parse("notify = nonsense").notify, "기본값(on) 유지");
        assert!(Config::default().notify, "기본 on");
    }

    #[test]
    fn notify_threshold_parses_seconds() {
        assert_eq!(Config::parse("notify-threshold = 30").notify_threshold, 30);
        assert_eq!(
            Config::parse("notify-threshold = 0").notify_threshold,
            0,
            "0은 '모든 명령 완료를 알림'이라는 유효한 선택"
        );
        assert_eq!(
            Config::parse("notify-threshold = -5").notify_threshold,
            10,
            "음수·비숫자는 기본값 유지"
        );
        assert_eq!(Config::parse("notify-threshold = abc").notify_threshold, 10);
    }

    #[test]
    fn inactive_dim_is_range_checked() {
        assert_eq!(Config::parse("inactive-dim = 0.2").inactive_dim, 0.2);
        assert_eq!(Config::parse("inactive-dim = 0").inactive_dim, 0.0);
        assert_eq!(Config::parse("inactive-dim = 0.8").inactive_dim, 0.8);

        let default = Config::default().inactive_dim;
        assert_eq!(default, 0.0, "기본 끔 — 기존 화면을 조용히 바꾸지 않는다");
        assert_eq!(
            Config::parse("inactive-dim = 0.9").inactive_dim,
            default,
            "너무 세면 비활성 페인이 안 보인다"
        );
        assert_eq!(Config::parse("inactive-dim = -0.1").inactive_dim, default);
        assert_eq!(Config::parse("inactive-dim = abc").inactive_dim, default);
    }

    #[test]
    fn scrollback_clamps_at_one_million() {
        assert_eq!(Config::parse("scrollback = 500").scrollback, 500);
        assert_eq!(Config::parse("scrollback = 99999999").scrollback, 1_000_000);
    }

    #[test]
    fn palette_index_is_bounds_checked() {
        let c = Config::parse("palette-3 = #ff0000");
        assert_eq!(c.palette[3], [1.0, 0.0, 0.0]);

        // 16 이상은 무시 — 배열이 16칸이라 패닉을 막아야 한다.
        let c = Config::parse("palette-16 = #ff0000");
        assert_eq!(c.palette, DEFAULT_PALETTE);
        let c = Config::parse("palette-999 = #ff0000");
        assert_eq!(c.palette, DEFAULT_PALETTE);
        let c = Config::parse("palette-abc = #ff0000");
        assert_eq!(c.palette, DEFAULT_PALETTE);
    }

    #[test]
    fn cursor_style_falls_back_to_block() {
        assert!(Config::parse("cursor-style = bar").cursor_style == CursorStyle::Bar);
        assert!(Config::parse("cursor-style = beam").cursor_style == CursorStyle::Bar);
        assert!(Config::parse("cursor-style = BEAM").cursor_style == CursorStyle::Bar);
        assert!(Config::parse("cursor-style = underline").cursor_style == CursorStyle::Underline);
        assert!(Config::parse("cursor-style = nonsense").cursor_style == CursorStyle::Block);
    }

    #[test]
    fn comments_blanks_and_malformed_lines_are_skipped() {
        let c = Config::parse(
            "\
# 주석
   # 들여쓴 주석

font-size = 20
= 값만 있는 줄
키만 있는 줄
  scrollback   =   777
",
        );
        assert_eq!(c.font_size, 20.0);
        assert_eq!(c.scrollback, 777, "키·값 주변 공백은 trim");
    }

    #[test]
    fn kitty_keyboard_defaults_off_and_parses_on_off() {
        assert!(
            !Config::default().kitty_keyboard,
            "dogfooding 전까지 기본 off"
        );
        assert!(Config::parse("kitty-keyboard = on").kitty_keyboard);
        assert!(Config::parse("kitty-keyboard = ON").kitty_keyboard);
        assert!(Config::parse("kitty-keyboard = true").kitty_keyboard);
        assert!(!Config::parse("kitty-keyboard = off").kitty_keyboard);
        assert!(
            !Config::parse("kitty-keyboard = maybe").kitty_keyboard,
            "알 수 없는 값은 기본값 유지"
        );
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let c = Config::parse("nonexistent-key = whatever\nfont-size = 16");
        assert_eq!(c.font_size, 16.0);
    }

    #[test]
    fn explicit_color_overrides_a_preset_declared_before_it() {
        // 순서 의존적이다: theme이 먼저 오면 뒤의 개별 키가 이긴다.
        let c = Config::parse("theme = guezwhoz\nbackground = #000000");
        assert_eq!(c.background, [0.0, 0.0, 0.0]);

        // 반대 순서면 프리셋이 덮어쓴다 — 문서화된 실제 동작.
        let c = Config::parse("background = #000000\ntheme = guezwhoz");
        assert_eq!(c.background, parse_hex("#1d1d1d").unwrap());
    }

    #[test]
    fn unknown_preset_leaves_defaults_alone() {
        let c = Config::parse("theme = nonexistent");
        assert_eq!(c.background, Config::default().background);
    }

    // ─── 라이트/다크 외양별 프리셋 ───

    #[test]
    fn appearance_keys_apply_only_to_the_matching_appearance() {
        let text = "theme-light = latte\ntheme-dark = guezwhoz";
        let light = Config::parse_for(text, Appearance::Light);
        assert_eq!(light.background, parse_hex("#eff1f5").unwrap());
        let dark = Config::parse_for(text, Appearance::Dark);
        assert_eq!(dark.background, parse_hex("#1d1d1d").unwrap());
    }

    #[test]
    fn parse_defaults_to_dark_appearance() {
        // Config::parse는 다크로 간주한다 — theme-light 줄은 무시된다.
        let c = Config::parse("theme-light = latte");
        assert_eq!(c.background, Config::default().background);
    }

    #[test]
    fn missing_appearance_key_keeps_defaults_for_that_appearance() {
        // theme-dark만 있으면 라이트 외양에선 기본값 그대로다.
        let c = Config::parse_for("theme-dark = guezwhoz", Appearance::Light);
        assert_eq!(c.background, Config::default().background);
    }

    #[test]
    fn explicit_color_still_overrides_an_appearance_preset() {
        // `theme`과 같은 순서 규칙이 외양 키에도 적용된다.
        let c = Config::parse_for(
            "theme-light = latte\nbackground = #123456",
            Appearance::Light,
        );
        assert_eq!(c.background, parse_hex("#123456").unwrap());
    }

    #[test]
    fn latte_preset_is_actually_light() {
        let c = Config::parse("theme = latte");
        let luma = |v: [f32; 3]| (v[0] + v[1] + v[2]) / 3.0;
        assert!(luma(c.background) > 0.8, "라이트 배경");
        assert!(luma(c.foreground) < 0.5, "어두운 전경");
    }
}
