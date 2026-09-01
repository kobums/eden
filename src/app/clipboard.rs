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
        let Some(clipboard) = self.clipboard.as_mut() else {
            return;
        };
        let Ok(text) = clipboard.get_text() else {
            return;
        };
        self.paste_text(&text);
    }

    /// 텍스트를 포커스된 페인에 붙여넣는다 (클립보드·파일 드롭 공용 경로).
    pub(super) fn paste_text(&mut self, text: &str) {
        let Some(state) = &self.state else { return };
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

    /// 파일을 창에 드롭하면 경로를 셸 인용해 붙여넣는다 (Terminal.app·iTerm2
    /// 관례). 끝의 공백은 여러 파일을 연달아 드롭했을 때 경로가 붙지 않게 한다.
    pub(super) fn drop_file(&mut self, path: &std::path::Path) {
        let quoted = shell_quote(&path.to_string_lossy());
        self.paste_text(&format!("{quoted} "));
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

/// 경로를 셸에 그대로 넣어도 안전하게 인용한다.
///
/// 특수문자가 없으면 원문 그대로, 있으면 작은따옴표로 감싼다. 내부의 `'`는
/// `'\''`로 끊는 POSIX 공통 규칙. `~`는 문자 자체는 무해하지만 맨 앞에서
/// 홈 확장이 일어나므로 안전 목록에 넣지 않는다.
fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+,:@%".contains(c));
    if safe {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn plain_paths_pass_through_unquoted() {
        assert_eq!(shell_quote("/usr/local/bin/eden"), "/usr/local/bin/eden");
        assert_eq!(
            shell_quote("/a/b-c/d_e.f+g,h:i@j%k"),
            "/a/b-c/d_e.f+g,h:i@j%k"
        );
    }

    #[test]
    fn spaces_and_specials_get_single_quoted() {
        assert_eq!(
            shell_quote("/Users/me/My File.txt"),
            "'/Users/me/My File.txt'"
        );
        assert_eq!(shell_quote("/tmp/a$b"), "'/tmp/a$b'");
        assert_eq!(shell_quote("/tmp/a(b)"), "'/tmp/a(b)'");
    }

    #[test]
    fn non_ascii_paths_get_quoted() {
        // 한글 파일명 — 대부분의 셸에서 무해하지만 안전 쪽으로 인용한다.
        assert_eq!(shell_quote("/tmp/보고서.pdf"), "'/tmp/보고서.pdf'");
    }

    #[test]
    fn embedded_single_quotes_are_escaped() {
        assert_eq!(shell_quote("/tmp/it's.txt"), r"'/tmp/it'\''s.txt'");
    }

    #[test]
    fn tilde_is_not_considered_safe() {
        // 맨 앞 `~`가 그대로 나가면 셸이 홈으로 확장한다.
        assert_eq!(shell_quote("~/notes"), "'~/notes'");
    }

    #[test]
    fn empty_string_becomes_empty_quotes() {
        assert_eq!(shell_quote(""), "''");
    }
}
