//! 블록 접기·재실행 (Phase 25).
//!
//! 접기는 세션의 `folded` 집합을 토글하는 것뿐이다 — 화면 반영은 렌더러·마우스가
//! 공유하는 행 목록([`crate::fold`])이 맡는다.
//!
//! 재실행은 **사용자가 이미 실행했던 명령줄**을 다시 보내는 것이라 Cmd+K의
//! "입력줄에 넣기만" 원칙의 예외다. 대신 두 가지 방어선을 둔다:
//! 셸이 명령을 실행 중이면 보내지 않고(실행 중인 프로그램의 stdin으로 들어간다),
//! 셸 통합이 자른 길이에 닿은 명령줄은 잘렸을 수 있으므로 넣기만 하고 Enter를
//! 치지 않는다.

use super::App;
use crate::session::Block;

impl App {
    pub(super) fn toggle_fold_last(&mut self) {
        let Some(state) = &self.state else { return };
        let session = &state.focused_pane().session;
        if let Some(block) = session.last_foldable_block() {
            session.toggle_fold(&block);
            state.window.request_redraw();
        }
    }

    pub(super) fn unfold_all(&mut self) {
        let Some(state) = &self.state else { return };
        state.focused_pane().session.unfold_all();
        state.window.request_redraw();
    }

    pub(super) fn rerun_last(&mut self) {
        let Some(state) = &self.state else { return };
        if let Some(block) = state.focused_pane().session.last_completed_block() {
            self.rerun_block(&block);
        }
    }

    /// 블록의 명령줄을 포커스된 페인의 셸에 다시 보낸다.
    ///
    /// 입력줄에 남아 있던 글자는 Ctrl-U로 지운다 — zsh(kill-whole-line)와
    /// bash(unix-line-discard, 커서는 보통 줄 끝) 모두에서 줄이 비워진다.
    pub(super) fn rerun_block(&mut self, block: &Block) {
        let Some(state) = &self.state else { return };
        let session = &state.focused_pane().session;
        let Some(cmd) = block.rerunnable() else {
            return;
        };
        if session.is_running() {
            return;
        }
        session.write(b"\x15".to_vec());
        match cmd {
            Ok(cmd) => {
                self.insert_command(cmd);
                let state = self.state.as_ref().unwrap();
                state.focused_pane().session.write(b"\r".to_vec());
            }
            // 잘렸을 수 있는 명령줄: 사용자가 보고 고친 뒤 직접 Enter.
            Err(cmd) => self.insert_command(cmd),
        }
    }
}
