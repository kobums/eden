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
}

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

    fn apply(&mut self, key: &str, value: &str) {
        match key {
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
            _ => {} // 알 수 없는 키 무시
        }
    }
}

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
