//! AI 명령 생성 바 (Cmd+K): 자연어 입력 → 셸 명령 삽입.
//!
//! 생성된 명령은 **실행하지 않고 프롬프트에 삽입만** 한다 — 실행 여부는 항상
//! 사용자 몫이다.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};

use super::App;
use crate::ai;
use crate::session::AppEvent;

/// AI 명령 생성 바의 상태.
pub(super) enum AiState {
    Idle,
    /// 입력 중인 자연어
    Input(String),
    /// 생성 요청 진행 중
    Pending,
    Error(String),
}

/// 컨텍스트로 보낼 화면 텍스트의 최근 줄 수와 최대 바이트.
const CONTEXT_LINES: i32 = 30;
const CONTEXT_MAX_BYTES: usize = 4000;

impl App {
    /// AI 바에 표시할 문자열. 닫혀 있으면 None.
    pub(super) fn ai_bar_line(&self) -> Option<String> {
        match &self.ai {
            AiState::Idle => None,
            AiState::Input(text) => Some(format!(
                "AI> {}{}_   (Enter 생성 / Esc 닫기)",
                text,
                self.preedit.as_deref().unwrap_or("")
            )),
            AiState::Pending => Some("AI> 명령 생성 중...".to_string()),
            AiState::Error(error) => Some(format!("AI 오류: {error}   (Esc 닫기)")),
        }
    }

    /// AI 입력 바의 자연어를 명령 생성 요청으로 보낸다.
    pub(super) fn submit_ai(&mut self) {
        let AiState::Input(text) = &self.ai else {
            return;
        };
        let request = text.trim().to_string();
        if request.is_empty() {
            self.ai = AiState::Idle;
            return;
        }

        let state = self.state.as_ref().unwrap();
        let pane = state.focused_pane();
        let pane_id = pane.id;

        // 컨텍스트 수집: 최근 화면 텍스트 + 마지막 종료 코드
        let screen_tail = {
            let term = pane.session.term.lock();
            let grid = term.grid();
            let history = grid.history_size() as i32;
            let cursor_line = grid.cursor.point.line.0;
            let cols = grid.columns();
            let start_line = (cursor_line - CONTEXT_LINES).max(-history);
            let text = term.bounds_to_string(
                Point::new(Line(start_line), Column(0)),
                Point::new(Line(cursor_line), Column(cols - 1)),
            );
            truncate_tail(text, CONTEXT_MAX_BYTES)
        };
        let last_exit = pane
            .session
            .blocks()
            .iter()
            .rev()
            .find_map(|block| block.exit);

        self.ai_seq += 1;
        let seq = self.ai_seq;
        self.ai = AiState::Pending;
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let context = ai::AiContext {
                screen_tail,
                last_exit,
            };
            let result = ai::generate_command(&request, &context);
            let _ = proxy.send_event(AppEvent::AiResult {
                pane_id,
                seq,
                result,
            });
        });
        state.window.request_redraw();
    }

    /// 생성 결과를 해당 페인의 입력줄에 삽입한다 (실행하지 않음).
    pub(super) fn on_ai_result(
        &mut self,
        pane_id: usize,
        seq: u64,
        result: Result<String, String>,
    ) {
        if seq != self.ai_seq || !matches!(self.ai, AiState::Pending) {
            return; // 취소되었거나 오래된 응답
        }
        let state = self.state.as_ref().unwrap();
        match result {
            Ok(command) => {
                self.ai = AiState::Idle;
                let pane = state
                    .tabs
                    .iter()
                    .find_map(|tab| tab.root.pane(pane_id))
                    .or_else(|| Some(state.focused_pane()));
                if let Some(pane) = pane {
                    // 여러 줄 명령은 개행이 실행으로 이어지지 않게 정리
                    pane.session.write(command.replace('\n', " ").into_bytes());
                }
            }
            Err(error) => self.ai = AiState::Error(error),
        }
        state.window.request_redraw();
    }
}

/// 문자열이 `max` 바이트를 넘으면 뒷부분만 남긴다 (char 경계 유지).
fn truncate_tail(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let cut = text.len() - max;
    let boundary = (cut..text.len())
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(text.len());
    text[boundary..].to_string()
}
