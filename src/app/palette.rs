//! 커맨드 팔레트 (Cmd+Shift+P): 액션 검색 + 실행.

use winit::event_loop::ActiveEventLoop;

use super::App;
use super::action::Action;
use super::ai_bar::AiState;
use crate::layout::SplitDir;

/// 팔레트에 표시되는 액션 목록 (표시 이름, 동작).
///
/// 액션 자체는 키바인딩과 공유한다([`Action`]) — 예전에는 팔레트가 자기
/// enum을 따로 갖고 있어 단축키 표와 조용히 갈라질 수 있었다.
const PALETTE_ACTIONS: &[(&str, Action)] = &[
    ("New Tab", Action::NewTab),
    ("Close Pane", Action::ClosePane),
    ("Split Right", Action::SplitRight),
    ("Split Down", Action::SplitDown),
    ("Toggle Zoom Pane", Action::ToggleZoom),
    ("Next Tab", Action::NextTab),
    ("Previous Tab", Action::PrevTab),
    ("AI: Generate Command", Action::AiGenerate),
    ("Search Scrollback", Action::Search),
    ("Jump to Previous Prompt", Action::JumpPrevPrompt),
    ("Jump to Next Prompt", Action::JumpNextPrompt),
    ("Copy Last Command Output", Action::CopyLastOutput),
    ("Increase Font Size", Action::FontSizeUp),
    ("Decrease Font Size", Action::FontSizeDown),
    ("Reset Font Size", Action::FontSizeReset),
];

/// 커맨드 팔레트 상태.
pub(super) struct Palette {
    pub(super) query: String,
    pub(super) selected: usize,
}

impl App {
    /// 현재 쿼리로 필터된 팔레트 액션들.
    pub(super) fn palette_matches(query: &str) -> Vec<(&'static str, Action)> {
        let q = query.to_lowercase();
        PALETTE_ACTIONS
            .iter()
            .filter(|(name, _)| q.is_empty() || name.to_lowercase().contains(&q))
            .copied()
            .collect()
    }

    /// 액션 하나를 실행한다. 키바인딩과 팔레트가 함께 쓰는 유일한 실행 지점이다.
    ///
    /// 여기서 각 메서드를 부르기만 하고 상태 변경은 메서드 안에 남겨 둔다 —
    /// 레이아웃 저장(Phase 19) 같은 부수 효과가 메서드 본문에 걸려 있어서,
    /// 로직을 이쪽으로 끌어오면 팔레트로 실행할 때만 훅이 빠지게 된다.
    pub(super) fn run_action(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        match action {
            Action::NewTab => self.new_tab(),
            Action::ClosePane => {
                let focused = self.state.as_ref().unwrap().active_tab().focused;
                self.close_pane(focused, true, event_loop);
            }
            Action::SplitRight => self.split_pane(SplitDir::Row),
            Action::SplitDown => self.split_pane(SplitDir::Column),
            Action::ToggleZoom => self.toggle_zoom(),
            Action::NextTab => self.cycle_tab(1),
            Action::PrevTab => self.cycle_tab(-1),
            Action::AiGenerate => {
                self.ai = AiState::Input(String::new());
                self.state.as_ref().unwrap().window.request_redraw();
            }
            Action::Search => self.toggle_search(),
            Action::Palette => {
                self.palette = Some(Palette {
                    query: String::new(),
                    selected: 0,
                });
                self.state.as_ref().unwrap().window.request_redraw();
            }
            Action::JumpPrevPrompt => self.jump_to_prompt(-1),
            Action::JumpNextPrompt => self.jump_to_prompt(1),
            Action::Copy => self.copy_selection(),
            Action::CopyLastOutput => self.copy_last_output(),
            Action::Paste => self.paste(),
            Action::FontSizeUp => self.adjust_font_size(1.0),
            Action::FontSizeDown => self.adjust_font_size(-1.0),
            Action::FontSizeReset => self.reset_font_size(),
            Action::FocusLeft => self.move_focus(-1.0, 0.0),
            Action::FocusRight => self.move_focus(1.0, 0.0),
            Action::FocusUp => self.move_focus(0.0, -1.0),
            Action::FocusDown => self.move_focus(0.0, 1.0),
            // 표시용 번호는 1부터, 내부 인덱스는 0부터.
            Action::SelectTab(n) => self.switch_tab(n - 1),
        }
    }

    /// 활성 탭을 `delta`만큼 순환 이동한다 (음수는 이전 방향).
    pub(super) fn cycle_tab(&mut self, delta: isize) {
        let state = self.state.as_ref().unwrap();
        let count = state.tabs.len();
        if count == 0 {
            return;
        }
        let next = (state.active as isize + delta).rem_euclid(count as isize) as usize;
        self.switch_tab(next);
    }

    /// OSC 133 마크 기반 프롬프트 점프 (`-1` 이전, `1` 다음).
    pub(super) fn jump_to_prompt(&mut self, delta: i32) {
        let state = self.state.as_ref().unwrap();
        state.focused_pane().session.jump_to_prompt(delta);
        state.window.request_redraw();
    }

    /// 팔레트가 열려 있을 때의 키 처리. 처리했으면 true.
    pub(super) fn palette_key(
        &mut self,
        key: &winit::keyboard::Key,
        text: Option<&str>,
        typing_allowed: bool,
        event_loop: &ActiveEventLoop,
    ) -> bool {
        use winit::keyboard::{Key, NamedKey};
        if self.palette.is_none() {
            return false;
        }
        match key {
            Key::Named(NamedKey::Escape) => self.palette = None,
            Key::Named(NamedKey::Enter) => {
                let p = self.palette.take().unwrap();
                let matches = Self::palette_matches(&p.query);
                if let Some((_, action)) = matches.get(p.selected).copied() {
                    self.run_action(action, event_loop);
                }
            }
            Key::Named(NamedKey::ArrowDown) => self.move_palette_selection(1),
            Key::Named(NamedKey::ArrowUp) => self.move_palette_selection(-1),
            Key::Named(NamedKey::Backspace) => {
                if let Some(p) = &mut self.palette {
                    p.query.pop();
                    p.selected = 0;
                }
            }
            _ => {
                if typing_allowed && let (Some(p), Some(text)) = (&mut self.palette, text) {
                    p.query.push_str(text);
                    p.selected = 0;
                }
            }
        }
        self.state.as_ref().unwrap().window.request_redraw();
        true
    }

    /// 팔레트 선택을 위/아래로 순환 이동.
    fn move_palette_selection(&mut self, delta: isize) {
        let Some(p) = &mut self.palette else { return };
        let n = Self::palette_matches(&p.query).len();
        if n > 0 {
            p.selected = (p.selected as isize + delta).rem_euclid(n as isize) as usize;
        }
    }
}
