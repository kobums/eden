//! 클립보드: 선택 복사, 붙여넣기, 마지막 명령 출력 복사.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::TermMode;

use super::App;

impl App {
    /// 비어 있지 않은 텍스트만 클립보드에 넣는다.
    fn set_clipboard(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        if let Some(clipboard) = self.clipboard.as_mut() {
            let _ = clipboard.set_text(text);
        }
    }

    pub(super) fn copy_selection(&mut self) {
        let Some(state) = &self.state else { return };
        let text = state
            .focused_pane()
            .session
            .term
            .lock()
            .selection_to_string();
        if let Some(text) = text {
            self.set_clipboard(text);
        }
    }

    pub(super) fn paste(&mut self) {
        let Some(state) = &self.state else { return };
        let Some(clipboard) = self.clipboard.as_mut() else {
            return;
        };
        let Ok(text) = clipboard.get_text() else {
            return;
        };
        let pane = state.focused_pane();
        let bracketed = pane
            .session
            .term
            .lock()
            .mode()
            .contains(TermMode::BRACKETED_PASTE);
        if bracketed {
            let mut bytes = b"\x1b[200~".to_vec();
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            pane.session.write(bytes);
        } else {
            // 개행이 실행으로 이어지는 사고를 줄이기 위해 \n → \r 정규화
            pane.session.write(text.replace('\n', "\r").into_bytes());
        }
    }

    /// 마지막으로 완료된 명령의 출력을 클립보드로 복사한다 (Cmd+Shift+C).
    pub(super) fn copy_last_output(&mut self) {
        let Some(state) = &self.state else { return };
        let pane = state.focused_pane();
        let Some((start_abs, end_abs)) = pane.session.last_output_range() else {
            return;
        };
        let text = {
            let term = pane.session.term.lock();
            let history = term.grid().history_size() as i64;
            let cols = term.grid().columns();
            let start = Point::new(Line((start_abs - history) as i32), Column(0));
            let end = Point::new(Line((end_abs - history) as i32), Column(cols - 1));
            term.bounds_to_string(start, end)
        };
        self.set_clipboard(text);
    }
}
