//! 키보드·IME 입력 → 앱 단축키 또는 PTY 바이트.
//!
//! 우선순위: 팔레트 → AI 바 → Cmd 단축키 → PTY 전달.

use alacritty_terminal::grid::Scroll;
use winit::event::{ElementState, Ime, KeyEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey};

use super::ai_bar::AiState;
use super::palette::Palette;
use super::App;
use crate::layout::{Pane, SplitDir};

impl App {
    pub(super) fn on_key(&mut self, event: KeyEvent, event_loop: &ActiveEventLoop) {
        if event.state != ElementState::Pressed {
            return;
        }
        // IME 조합 중에는 키 이벤트를 무시한다 (조합 결과는 Ime::Commit으로 온다).
        if self.preedit.is_some() {
            return;
        }
        let mods = self.modifiers.state();
        // Cmd/Ctrl 조합은 단축키이므로 오버레이에 글자로 넣지 않는다.
        let typing = !mods.control_key() && !mods.super_key();

        // 오버레이가 열려 있으면 키를 가로챈다.
        if self.palette_key(&event.logical_key, event.text.as_deref(), typing, event_loop) {
            return;
        }
        if self.ai_bar_key(&event.logical_key, event.text.as_deref(), typing) {
            return;
        }

        if mods.super_key() {
            self.on_command_key(&event, mods, event_loop);
            return;
        }

        if let Some(bytes) = key_to_bytes(&event, mods) {
            let state = self.state.as_ref().unwrap();
            let pane = state.focused_pane();
            Self::on_user_input(pane);
            pane.session.write(bytes);
            state.window.request_redraw();
        }
    }

    /// AI 바가 열려 있을 때의 키 처리. 처리했으면 true.
    fn ai_bar_key(&mut self, key: &Key, text: Option<&str>, typing_allowed: bool) -> bool {
        if matches!(self.ai, AiState::Idle) {
            return false;
        }
        match key {
            Key::Named(NamedKey::Escape) => {
                self.ai = AiState::Idle;
                self.ai_seq += 1; // 진행 중이던 요청 응답 무시
            }
            Key::Named(NamedKey::Enter) => self.submit_ai(),
            Key::Named(NamedKey::Backspace) => {
                if let AiState::Input(buffer) = &mut self.ai {
                    buffer.pop();
                }
            }
            _ => {
                if typing_allowed {
                    if let (AiState::Input(buffer), Some(text)) = (&mut self.ai, text) {
                        buffer.push_str(text);
                    }
                }
            }
        }
        self.state.as_ref().unwrap().window.request_redraw();
        true
    }

    /// Cmd 조합 앱 단축키.
    fn on_command_key(
        &mut self,
        event: &KeyEvent,
        mods: ModifiersState,
        event_loop: &ActiveEventLoop,
    ) {
        // Cmd+Option+화살표: 페인 포커스 이동
        if mods.alt_key() {
            match event.logical_key.as_ref() {
                Key::Named(NamedKey::ArrowLeft) => self.move_focus(-1.0, 0.0),
                Key::Named(NamedKey::ArrowRight) => self.move_focus(1.0, 0.0),
                Key::Named(NamedKey::ArrowUp) => self.move_focus(0.0, -1.0),
                Key::Named(NamedKey::ArrowDown) => self.move_focus(0.0, 1.0),
                _ => {}
            }
            return;
        }
        match event.logical_key.as_ref() {
            // 커맨드 팔레트 (Cmd+Shift+P)
            Key::Character("p") | Key::Character("P") if mods.shift_key() => {
                self.palette = Some(Palette {
                    query: String::new(),
                    selected: 0,
                });
                self.state.as_ref().unwrap().window.request_redraw();
            }
            // AI 명령 생성 바
            Key::Character("k") => {
                self.ai = AiState::Input(String::new());
                self.state.as_ref().unwrap().window.request_redraw();
            }
            // 탭
            Key::Character("t") => self.new_tab(),
            Key::Character("w") => {
                let focused = self.state.as_ref().unwrap().active_tab().focused;
                self.close_pane(focused, true, event_loop);
            }
            Key::Character(digit)
                if digit.len() == 1 && digit.chars().next().unwrap().is_ascii_digit() =>
            {
                let n = digit.chars().next().unwrap() as usize - '0' as usize;
                if n >= 1 {
                    self.switch_tab(n - 1);
                }
            }
            Key::Character("}") => self.cycle_tab(1),
            Key::Character("{") => self.cycle_tab(-1),
            // 분할
            Key::Character("d") if mods.shift_key() => self.split_pane(SplitDir::Column),
            Key::Character("D") => self.split_pane(SplitDir::Column),
            Key::Character("d") => self.split_pane(SplitDir::Row),
            // 복사/붙여넣기
            Key::Character("c") if mods.shift_key() => self.copy_last_output(),
            Key::Character("C") => self.copy_last_output(),
            Key::Character("c") => self.copy_selection(),
            Key::Character("v") => self.paste(),
            // OSC 133 마크 기반 프롬프트 점프
            Key::Named(NamedKey::ArrowUp) => self.jump_to_prompt(-1),
            Key::Named(NamedKey::ArrowDown) => self.jump_to_prompt(1),
            _ => {}
        }
    }

    pub(super) fn on_ime(&mut self, ime: Ime) {
        let state = self.state.as_ref().unwrap();
        match ime {
            Ime::Preedit(text, _) => {
                self.preedit = if text.is_empty() { None } else { Some(text) };
            }
            Ime::Commit(text) => {
                self.preedit = None;
                // AI 입력 바가 열려 있으면 한글 확정 입력도 그쪽으로
                if let AiState::Input(buffer) = &mut self.ai {
                    buffer.push_str(&text);
                } else {
                    let pane = state.focused_pane();
                    Self::on_user_input(pane);
                    pane.session.write(text.into_bytes());
                }
            }
            Ime::Enabled | Ime::Disabled => {}
        }
        state.window.request_redraw();
    }

    /// 입력이 발생하면 선택을 해제하고 화면을 맨 아래로 되돌린다.
    pub(super) fn on_user_input(pane: &Pane) {
        let mut term = pane.session.term.lock();
        term.selection = None;
        term.scroll_display(Scroll::Bottom);
    }
}

/// 키 입력을 PTY로 보낼 바이트 시퀀스로 변환한다.
fn key_to_bytes(event: &KeyEvent, mods: ModifiersState) -> Option<Vec<u8>> {
    if let Key::Named(named) = &event.logical_key {
        let seq: Option<&[u8]> = match named {
            NamedKey::Enter => Some(b"\r"),
            NamedKey::Backspace => Some(b"\x7f"),
            NamedKey::Tab => Some(b"\t"),
            NamedKey::Escape => Some(b"\x1b"),
            NamedKey::ArrowUp => Some(b"\x1b[A"),
            NamedKey::ArrowDown => Some(b"\x1b[B"),
            NamedKey::ArrowRight => Some(b"\x1b[C"),
            NamedKey::ArrowLeft => Some(b"\x1b[D"),
            NamedKey::Home => Some(b"\x1b[H"),
            NamedKey::End => Some(b"\x1b[F"),
            NamedKey::PageUp => Some(b"\x1b[5~"),
            NamedKey::PageDown => Some(b"\x1b[6~"),
            NamedKey::Delete => Some(b"\x1b[3~"),
            NamedKey::Space => {
                if mods.control_key() {
                    return Some(vec![0]);
                }
                Some(b" ")
            }
            _ => None,
        };
        if let Some(seq) = seq {
            return Some(seq.to_vec());
        }
    }

    // Ctrl+A..Z → C0 제어 문자
    if mods.control_key() {
        if let Key::Character(s) = &event.logical_key {
            let c = s.chars().next()?.to_ascii_lowercase();
            if c.is_ascii_lowercase() {
                return Some(vec![c as u8 - b'a' + 1]);
            }
        }
    }

    event
        .text
        .as_ref()
        .filter(|t| !t.is_empty())
        .map(|t| t.as_bytes().to_vec())
}
