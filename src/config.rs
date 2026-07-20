//! 설정 파일: `~/.config/terminal-dev/config` (Ghostty식 `key = value`).
//!
//! 설정 없이도 기본값으로 완결된 경험을 준다는 원칙에 따라, 파일이 없으면
//! 전부 기본값을 쓴다. 알 수 없는 키/잘못된 값은 조용히 무시하고 기본값 유지.

use std::path::PathBuf;

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
        }
    }
}

impl Config {
    /// 설정 파일을 읽어 파싱한다. 없으면 기본값.
    pub fn load() -> Self {
        let mut config = Config::default();
        let Some(path) = config_path() else {
            return config;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return config;
        };
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
            config.apply(key, value);
        }
        config
    }

    /// 이름있는 컬러 프리셋을 적용한다. `theme = <name>`. 개별 색 키가 뒤에
    /// 오면 그 값이 프리셋을 덮어쓴다.
    fn apply_preset(&mut self, name: &str) {
        let preset = match name.to_lowercase().as_str() {
            "guezwhoz" => &GUEZWHOZ,
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

    fn apply(&mut self, key: &str, value: &str) {
        match key {
            "theme" => self.apply_preset(value),
            "font-size" => {
                if let Ok(v) = value.parse::<f32>() {
                    if (6.0..=72.0).contains(&v) {
                        self.font_size = v;
                    }
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
            // palette-0 ~ palette-15: 16색 ANSI 팔레트
            _ if key.starts_with("palette-") => {
                if let Ok(idx) = key["palette-".len()..].parse::<usize>() {
                    if idx < 16 {
                        if let Some(c) = parse_hex(value) {
                            self.palette[idx] = c;
                        }
                    }
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
    Some(PathBuf::from(home).join(".config/terminal-dev/config"))
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
