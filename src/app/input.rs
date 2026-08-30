//! 키보드·IME 입력 → 앱 단축키 또는 PTY 바이트.
//!
//! 우선순위: 팔레트 → AI 바 → Cmd 단축키 → PTY 전달.

use alacritty_terminal::grid::Scroll;
use winit::event::{ElementState, Ime, KeyEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState, NamedKey, PhysicalKey};

use super::App;
use super::action::Chord;
use super::ai_bar::AiState;
use super::kitty_key;
use crate::layout::Pane;

impl App {
    pub(super) fn on_key(&mut self, event: KeyEvent, event_loop: &ActiveEventLoop) {
        if event.state != ElementState::Pressed {
            // release는 kitty 프로토콜(REPORT_EVENT_TYPES)이 요구할 때만
            // 의미가 있다 — 판정은 인코더가 term 모드를 보고 한다.
            self.on_key_release(&event);
            return;
        }
        let mods = self.modifiers.state();
        // IME 조합 중에는 키 이벤트를 무시한다 (조합 결과는 Ime::Commit으로 온다).
        // 단, Cmd 조합은 앱 단축키이므로 조합 중에도 통과시킨다.
        if self.preedit.is_some() && !mods.super_key() {
            return;
        }
        // Cmd/Ctrl 조합은 단축키이므로 오버레이에 글자로 넣지 않는다.
        let typing = !mods.control_key() && !mods.super_key();

        // 오버레이가 열려 있으면 키를 가로챈다.
        if self.palette_key(
            &event.logical_key,
            event.text.as_deref(),
            typing,
            event_loop,
        ) {
            return;
        }
        if self.ai_bar_key(&event.logical_key, event.text.as_deref(), typing) {
            return;
        }
        // 검색 바는 Cmd 조합을 삼키지 않는다 — 바가 열려 있어도 Cmd+C 복사는
        // 되어야 하고, Cmd+F는 아래 단축키 표에서 "다음 매치"로 처리한다.
        if !mods.super_key()
            && self.search_key(&event.logical_key, event.text.as_deref(), typing, mods)
        {
            return;
        }

        // 수식키 자신(Cmd 등)의 press는 단축키가 아니다 — kitty
        // REPORT_ALL_KEYS_AS_ESC 모드가 이 이벤트를 요구하므로 아래로 흘린다.
        if mods.super_key() && !kitty_key::is_modifier_key(&event.logical_key) {
            self.on_command_key(&event, mods, event_loop);
            return;
        }

        self.write_key(&event, mods);
    }

    /// 키 release. press와 같은 가드를 거친다 — IME 조합 중이거나 오버레이가
    /// 열려 있으면 release도 PTY로 새면 안 되고, Cmd 단축키로 소비된 press의
    /// release도 마찬가지다 (수식키 자신의 release는 예외 — kitty
    /// REPORT_ALL_KEYS_AS_ESC가 요구한다).
    fn on_key_release(&mut self, event: &KeyEvent) {
        if self.preedit.is_some() {
            return;
        }
        if self.palette.is_some() || !matches!(self.ai, AiState::Idle) || self.search.is_some() {
            return;
        }
        let mods = self.modifiers.state();
        if mods.super_key() && !kitty_key::is_modifier_key(&event.logical_key) {
            return;
        }
        self.write_key(event, mods);
    }

    /// 키 이벤트를 PTY 바이트로 변환해 보낸다. kitty 프로토콜 플래그가 서
    /// 있으면 kitty 인코더가, 아니면 기존 `key_to_bytes`가 처리한다.
    fn write_key(&mut self, event: &KeyEvent, mods: ModifiersState) {
        let state = self.state.as_ref().unwrap();
        let pane = state.focused_pane();
        // 모드는 매 키마다 읽는다 — 대체 스크린 진입/이탈 시 kitty 모드가
        // 갈리므로 캐시하면 즉시 어긋난다. (문장 끝에서 락이 풀린다.)
        let mode = *pane.session.term.lock().mode();
        let input = kitty_key::KeyInput::from_winit(event);
        let bytes = match kitty_key::encode(&input, mods, mode) {
            kitty_key::Encoded::Bytes(bytes) => Some(bytes),
            kitty_key::Encoded::Legacy => key_to_bytes(event, mods),
            kitty_key::Encoded::Nothing => None,
        };
        if let Some(bytes) = bytes {
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
                if typing_allowed && let (AiState::Input(buffer), Some(text)) = (&mut self.ai, text)
                {
                    buffer.push_str(text);
                }
            }
        }
        self.state.as_ref().unwrap().window.request_redraw();
        true
    }

    /// Cmd 조합 앱 단축키. 키맵(기본표 + 사용자 재정의)을 조회해 액션을 낸다.
    fn on_command_key(
        &mut self,
        event: &KeyEvent,
        mods: ModifiersState,
        event_loop: &ActiveEventLoop,
    ) {
        // 물리 키 위치로 매칭한다. 한글 IME 등 비라틴 입력 소스에서는
        // logical_key가 자모("ㅅ") 등으로 와서 문자 매칭이 실패하기 때문.
        let PhysicalKey::Code(code) = event.physical_key else {
            return;
        };
        let chord = Chord {
            code,
            shift: mods.shift_key(),
            alt: mods.alt_key(),
        };
        if let Some(action) = self.keymap.get(chord) {
            self.run_action(action, event_loop);
        }
    }

    pub(super) fn on_ime(&mut self, ime: Ime) {
        // 아래 분기에서 `&mut self`를 부르므로 state 차용을 들고 있을 수 없다.
        let window = self.state.as_ref().unwrap().window.clone();
        match ime {
            Ime::Preedit(text, _) => {
                self.preedit = if text.is_empty() { None } else { Some(text) };
            }
            Ime::Commit(text) => {
                self.preedit = None;
                // 오버레이가 열려 있으면 한글 확정 입력도 그쪽으로 보낸다.
                if self.search.is_some() {
                    self.search_ime_commit(&text);
                } else if let AiState::Input(buffer) = &mut self.ai {
                    buffer.push_str(&text);
                } else {
                    let pane = self.state.as_ref().unwrap().focused_pane();
                    Self::on_user_input(pane);
                    pane.session.write(text.into_bytes());
                }
            }
            // IME가 꺼지면(입력 소스 전환, Cmd 누름 등) winit은 빈 Preedit
            // 이벤트 없이 Disabled만 보내므로 여기서 preedit을 지워야 한다.
            // 안 지우면 stale preedit이 남아 모든 키 입력이 무시된다.
            Ime::Disabled => self.preedit = None,
            Ime::Enabled => {}
        }
        window.request_redraw();
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
    if mods.control_key()
        && let Key::Character(s) = &event.logical_key
    {
        let c = s.chars().next()?.to_ascii_lowercase();
        if c.is_ascii_lowercase() {
            return Some(vec![c as u8 - b'a' + 1]);
        }
    }

    event
        .text
        .as_ref()
        .filter(|t| !t.is_empty())
        .map(|t| t.as_bytes().to_vec())
}
