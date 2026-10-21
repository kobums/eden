//! 키보드 힌트 모드 (Cmd+Shift+O) — kitty hints / Vimium 방식.
//!
//! 포커스된 페인에 보이는 URL마다 짧은 라벨을 띄우고, 라벨을 치면 그 링크를
//! 기본 앱으로 연다. 링크 감지는 평문 URL 밑줄(Phase 16)의 `visible_urls`를
//! 그대로 쓴다 — 밑줄이 그어진 것이 곧 힌트 대상이다.
//!
//! 라벨 글자는 **물리 키 위치**로 받는다. 한글 입력 소스에서도 `a` 자리를
//! 누르면 `a`다 (단축키 매칭과 같은 이유, Phase 15).

use alacritty_terminal::grid::Dimensions;
use winit::event::KeyEvent;
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

use super::App;
use crate::renderer::{self, HintBadge};

/// 라벨 글자 — 홈로우 먼저 (손가락 이동이 적은 순).
const ALPHABET: &[u8] = b"asdfghjklqwertyuiopzxcvbnm";

/// 힌트 모드 상태. 열려 있는 동안 키 입력을 전부 가로챈다.
pub(super) struct Hints {
    pub(super) pane_id: usize,
    targets: Vec<Target>,
    typed: String,
}

struct Target {
    label: String,
    url: String,
    /// 링크 첫 글자의 절대 줄 번호 (스크롤백 포함)와 열
    abs_line: i64,
    col: usize,
}

/// 라벨 `n`개. 26개 이하면 한 글자, 넘으면 전부 두 글자 — 섞으면 `a`와 `as`처럼
/// 한 라벨이 다른 라벨의 접두가 돼 `a`를 치는 순간 어느 쪽인지 정할 수 없다.
fn labels(n: usize) -> Vec<String> {
    if n <= ALPHABET.len() {
        return ALPHABET[..n]
            .iter()
            .map(|&c| (c as char).to_string())
            .collect();
    }
    ALPHABET
        .iter()
        .flat_map(|&a| {
            ALPHABET
                .iter()
                .map(move |&b| format!("{}{}", a as char, b as char))
        })
        .take(n)
        .collect()
}

/// 물리 키 → 라벨 글자.
fn letter(key: PhysicalKey) -> Option<char> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    let c = match code {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        _ => return None,
    };
    Some(c)
}

impl App {
    /// 포커스된 페인의 보이는 URL로 힌트 모드를 연다. 링크가 없으면 열지 않는다.
    pub(super) fn open_hints(&mut self) {
        let state = self.state.as_ref().unwrap();
        let pane = state.focused_pane();
        let targets: Vec<(String, i64, usize)> = {
            let folds = pane.session.folds();
            let term = pane.session.term.lock();
            let history = term.grid().history_size() as i64;
            let rows = pane.session.view_rows(&folds, &term);
            crate::app::links::urls_in_rows(&term, &rows)
                .into_iter()
                .map(|m| {
                    let url = term.bounds_to_string(*m.start(), *m.end());
                    (url, history + m.start().line.0 as i64, m.start().column.0)
                })
                .collect()
        };
        if targets.is_empty() {
            return;
        }
        let labels = labels(targets.len());
        self.hints = Some(Hints {
            pane_id: pane.id,
            targets: targets
                .into_iter()
                .zip(labels)
                .map(|((url, abs_line, col), label)| Target {
                    label,
                    url,
                    abs_line,
                    col,
                })
                .collect(),
            typed: String::new(),
        });
        state.window.request_redraw();
    }

    /// 힌트 모드가 열려 있을 때의 키 처리. 처리했으면 true (열려 있으면 항상 삼킨다).
    pub(super) fn hints_key(&mut self, event: &KeyEvent) -> bool {
        let Some(hints) = &mut self.hints else {
            return false;
        };
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => self.hints = None,
            Key::Named(NamedKey::Backspace) => {
                hints.typed.pop();
            }
            _ => {
                if let Some(c) = letter(event.physical_key) {
                    let typed = format!("{}{c}", hints.typed);
                    // 어떤 라벨의 접두도 아니면 오타 — 무시한다
                    if hints.targets.iter().any(|t| t.label.starts_with(&typed)) {
                        hints.typed = typed;
                    }
                    if let Some(target) = hints.targets.iter().find(|t| t.label == hints.typed) {
                        let _ = std::process::Command::new("open").arg(&target.url).spawn();
                        self.hints = None;
                    }
                }
            }
        }
        self.state.as_ref().unwrap().window.request_redraw();
        true
    }

    /// 그릴 라벨들 (화면 픽셀 좌표). 아직 남은 후보만, 화면에 보이는 것만.
    pub(super) fn hint_badges(
        hints: &Hints,
        pane: &crate::layout::Pane,
        rect: crate::layout::Rect,
        cell_w: f32,
        cell_h: f32,
    ) -> Vec<HintBadge> {
        let folds = pane.session.folds();
        let term = pane.session.term.lock();
        // 접힌 블록이 있으면 화면 행 ≠ 뷰포트 줄 — 렌더러와 같은 행 목록을 쓴다.
        let rows = pane.session.view_rows(&folds, &term);
        hints
            .targets
            .iter()
            .filter(|t| t.label.starts_with(&hints.typed))
            .filter_map(|t| {
                let row = rows
                    .iter()
                    .position(|r| *r == crate::fold::ViewRow::Line(t.abs_line))?;
                Some(HintBadge {
                    x: rect.x + renderer::PADDING + t.col as f32 * cell_w,
                    y: rect.y + renderer::PADDING + row as f32 * cell_h,
                    label: t.label.clone(),
                    typed: hints.typed.chars().count(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn few_targets_get_single_homerow_letters() {
        assert_eq!(labels(3), ["a", "s", "d"]);
        assert_eq!(labels(0), Vec::<String>::new());
        assert_eq!(labels(26).len(), 26);
    }

    #[test]
    fn many_targets_get_two_letters_and_no_label_prefixes_another() {
        let l = labels(30);
        assert_eq!(l.len(), 30);
        assert!(l.iter().all(|s| s.len() == 2));
        assert_eq!(&l[..2], ["aa", "as"]);
        for a in &l {
            for b in &l {
                assert!(a == b || !b.starts_with(a.as_str()));
            }
        }
    }

    #[test]
    fn physical_keys_map_to_letters() {
        assert_eq!(letter(PhysicalKey::Code(KeyCode::KeyA)), Some('a'));
        assert_eq!(letter(PhysicalKey::Code(KeyCode::Digit1)), None);
    }
}
