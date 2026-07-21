//! 커맨드 팔레트 (Cmd+Shift+P): 액션 검색 + 실행.

use winit::event_loop::ActiveEventLoop;

use super::App;
use super::ai_bar::AiState;
use crate::layout::SplitDir;

/// 커맨드 팔레트 액션.
#[derive(Clone, Copy)]
pub(super) enum PaletteAction {
    NewTab,
    CloseTab,
    SplitRight,
    SplitDown,
    ToggleZoom,
    NextTab,
    PrevTab,
    AiGenerate,
    Search,
    JumpPrev,
    JumpNext,
    CopyLastOutput,
}

/// 팔레트에 표시되는 액션 목록 (이름, 동작).
const PALETTE_ACTIONS: &[(&str, PaletteAction)] = &[
    ("New Tab", PaletteAction::NewTab),
    ("Close Tab", PaletteAction::CloseTab),
    ("Split Right", PaletteAction::SplitRight),
    ("Split Down", PaletteAction::SplitDown),
    ("Toggle Zoom Pane", PaletteAction::ToggleZoom),
    ("Next Tab", PaletteAction::NextTab),
    ("Previous Tab", PaletteAction::PrevTab),
    ("AI: Generate Command", PaletteAction::AiGenerate),
    ("Search Scrollback", PaletteAction::Search),
    ("Jump to Previous Prompt", PaletteAction::JumpPrev),
    ("Jump to Next Prompt", PaletteAction::JumpNext),
    ("Copy Last Command Output", PaletteAction::CopyLastOutput),
];

/// 커맨드 팔레트 상태.
pub(super) struct Palette {
    pub(super) query: String,
    pub(super) selected: usize,
}

impl App {
    /// 현재 쿼리로 필터된 팔레트 액션들.
    pub(super) fn palette_matches(query: &str) -> Vec<(&'static str, PaletteAction)> {
        let q = query.to_lowercase();
        PALETTE_ACTIONS
            .iter()
            .filter(|(name, _)| q.is_empty() || name.to_lowercase().contains(&q))
            .copied()
            .collect()
    }

    /// 선택된 팔레트 액션을 실행한다.
    pub(super) fn run_palette_action(
        &mut self,
        action: PaletteAction,
        event_loop: &ActiveEventLoop,
    ) {
        match action {
            PaletteAction::NewTab => self.new_tab(),
            PaletteAction::CloseTab => {
                let focused = self.state.as_ref().unwrap().active_tab().focused;
                self.close_pane(focused, true, event_loop);
            }
            PaletteAction::SplitRight => self.split_pane(SplitDir::Row),
            PaletteAction::SplitDown => self.split_pane(SplitDir::Column),
            PaletteAction::ToggleZoom => self.toggle_zoom(),
            PaletteAction::NextTab => self.cycle_tab(1),
            PaletteAction::PrevTab => self.cycle_tab(-1),
            PaletteAction::AiGenerate => {
                self.ai = AiState::Input(String::new());
                self.state.as_ref().unwrap().window.request_redraw();
            }
            PaletteAction::Search => self.toggle_search(),
            PaletteAction::JumpPrev => self.jump_to_prompt(-1),
            PaletteAction::JumpNext => self.jump_to_prompt(1),
            PaletteAction::CopyLastOutput => self.copy_last_output(),
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
                    self.run_palette_action(action, event_loop);
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
