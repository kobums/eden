//! 스크롤백 검색 (Cmd+F).
//!
//! 정규식 엔진은 alacritty가 이미 갖고 있는 것을 쓴다 — 줄바꿈 래핑·와이드
//! 문자·스크롤백 경계를 모두 처리하고, 패턴에 대문자가 없으면 자동으로
//! 대소문자를 무시한다(smart case).
//!
//! 매치는 그리드 좌표가 아니라 **절대 줄 번호**로 들고 있는다. 그리드
//! `Line`은 PTY가 출력을 낼 때마다 밀리므로 캐시해두면 조용히 어긋난다.
//! 블록·마크가 이미 쓰는 관용구와 같다 (`session.rs`의 `Mark.abs_line`).
//! 덕분에 새 출력이 들어와도 재스캔할 필요가 없다.

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Direction, Line, Point};
use alacritty_terminal::term::search::{RegexIter, RegexSearch};

use crate::session::EventProxy;

/// 한 번의 스캔에서 모을 매치 상한. `.` 같은 병리적 쿼리를 막는다.
const MAX_MATCHES: usize = 1000;

/// 절대 줄 번호로 표현한 매치 범위 (양끝 포함).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct AbsMatch {
    pub(crate) start_line: i64,
    pub(crate) start_col: usize,
    pub(crate) end_line: i64,
    pub(crate) end_col: usize,
}

impl AbsMatch {
    /// 그리드 좌표 매치를 절대 줄 번호로 변환한다.
    fn from_match(m: &std::ops::RangeInclusive<Point>, history: i64) -> Self {
        Self {
            start_line: history + m.start().line.0 as i64,
            start_col: m.start().column.0,
            end_line: history + m.end().line.0 as i64,
            end_col: m.end().column.0,
        }
    }

    /// 이 매치가 `point`(그리드 좌표)를 포함하는지. `history`로 좌표계를 맞춘다.
    pub(crate) fn contains(&self, point: Point, history: i64) -> bool {
        let line = history + point.line.0 as i64;
        let col = point.column.0;
        if line < self.start_line || line > self.end_line {
            return false;
        }
        // 여러 줄에 걸친 매치는 첫 줄의 시작 열과 끝 줄의 끝 열만 따진다.
        if line == self.start_line && col < self.start_col {
            return false;
        }
        if line == self.end_line && col > self.end_col {
            return false;
        }
        true
    }
}

pub(super) struct Search {
    pub(super) query: String,
    /// 검색 대상 페인. 포커스가 옮겨가면 검색을 닫는다.
    pub(super) pane_id: usize,
    /// 컴파일된 정규식. DFA 캐시를 들고 있어 매 프레임 재생성할 수 없다.
    regex: Option<RegexSearch>,
    matches: Vec<AbsMatch>,
    current: usize,
    /// 정규식 컴파일 실패 (입력 도중의 `(`, `[`, `\` 등).
    invalid: bool,
}

impl Search {
    pub(super) fn new(pane_id: usize) -> Self {
        Self {
            query: String::new(),
            pane_id,
            regex: None,
            matches: Vec::new(),
            current: 0,
            invalid: false,
        }
    }

    pub(super) fn matches(&self) -> &[AbsMatch] {
        &self.matches
    }

    /// 현재 선택된 매치의 인덱스. 매치가 없으면 None.
    pub(super) fn current_index(&self) -> Option<usize> {
        (!self.matches.is_empty()).then_some(self.current)
    }

    pub(super) fn current_match(&self) -> Option<AbsMatch> {
        self.matches.get(self.current).copied()
    }

    /// 검색 바 오른쪽에 띄울 상태 문자열.
    pub(super) fn status(&self) -> String {
        if self.invalid {
            return "invalid".to_string();
        }
        if self.query.is_empty() {
            return String::new();
        }
        if self.matches.is_empty() {
            return "no match".to_string();
        }
        format!("{}/{}", self.current + 1, self.matches.len())
    }

    /// 쿼리를 다시 컴파일하고 스크롤백 전체를 훑는다.
    ///
    /// 타이핑마다 호출된다 — 10k줄 DFA 스캔은 1ms 미만이고, "3/17" 카운터가
    /// 어차피 전체 개수를 요구하므로 부분 스캔은 의미가 없다.
    pub(super) fn rescan(&mut self, term: &Term<EventProxy>) {
        self.matches.clear();
        self.current = 0;

        if self.query.is_empty() {
            self.regex = None;
            self.invalid = false;
            return;
        }

        match RegexSearch::new(&self.query) {
            Ok(regex) => {
                self.regex = Some(regex);
                self.invalid = false;
            }
            Err(_) => {
                // 입력 도중의 미완성 정규식. 조용히 매치 없음으로 두고 바에 표시한다.
                self.regex = None;
                self.invalid = true;
                return;
            }
        }

        let regex = self.regex.as_mut().expect("방금 Some으로 채웠다");
        let grid = term.grid();
        let history = grid.history_size() as i64;
        let cols = grid.columns();
        let screen_lines = grid.screen_lines();
        if cols == 0 || screen_lines == 0 {
            return;
        }

        // 스크롤백 맨 위부터 화면 맨 아래까지. RegexIter는 end에서 멈추므로
        // 순환하지 않는다 — 직접 origin을 전진시킬 때의 무한 루프 위험이 없다.
        let start = Point::new(Line(-(history as i32)), Column(0));
        let end = Point::new(Line(screen_lines as i32 - 1), Column(cols - 1));

        self.matches = RegexIter::new(start, end, Direction::Right, term, regex)
            .take(MAX_MATCHES)
            .map(|m| AbsMatch::from_match(&m, history))
            .collect();
    }

    /// 다음(`1`) / 이전(`-1`) 매치로 이동하고 그 매치를 돌려준다.
    pub(super) fn step(&mut self, direction: i32) -> Option<AbsMatch> {
        if self.matches.is_empty() {
            return None;
        }
        let len = self.matches.len() as i32;
        self.current = (self.current as i32 + direction).rem_euclid(len) as usize;
        self.current_match()
    }

    /// 스캔 직후, 화면에서 가장 가까운 매치를 현재 매치로 삼는다.
    ///
    /// 증분 검색에서 커서가 화면 밖 먼 매치로 튀지 않도록 하는 장치다.
    pub(super) fn select_nearest(&mut self, top_abs: i64) {
        if self.matches.is_empty() {
            return;
        }
        // 화면 위쪽부터 첫 매치, 없으면 (전부 화면 위에 있으면) 마지막 매치.
        self.current = self
            .matches
            .iter()
            .position(|m| m.end_line >= top_abs)
            .unwrap_or(self.matches.len() - 1);
    }
}

// --- App 배선 ---

use winit::keyboard::{Key, NamedKey};

use super::App;

impl App {
    /// Cmd+F. 닫혀 있으면 열고, 이미 열려 있으면 다음 매치로 간다(브라우저 관례).
    pub(super) fn toggle_search(&mut self) {
        if self.search.is_some() {
            self.step_search(1);
            return;
        }
        let Some(state) = &self.state else { return };
        self.search = Some(Search::new(state.active_tab().focused));
        // 검색 바가 입력을 받으므로 AI 바는 닫는다 (같은 자리를 쓴다).
        self.ai = super::ai_bar::AiState::Idle;
        self.preedit = None;
        state.window.request_redraw();
    }

    pub(super) fn close_search(&mut self) {
        self.search = None;
        self.preedit = None;
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }

    /// 포커스가 검색 대상 페인을 떠났으면 검색을 닫는다.
    ///
    /// 탭 전환·페인 이동·페인 종료 등 포커스가 바뀌는 경로가 여러 개라
    /// 각 지점에 흩뿌리는 대신 매 프레임 `redraw`에서 한 번 확인한다.
    /// (여기서 `request_redraw`를 부르면 그리는 도중 다시 그리기를 요청해
    /// 무한 루프가 되므로 상태만 정리한다.)
    pub(super) fn close_search_if_pane_changed(&mut self) {
        let Some(search) = &self.search else { return };
        let still_focused = self
            .state
            .as_ref()
            .is_some_and(|s| s.active_tab().focused == search.pane_id);
        if !still_focused {
            self.search = None;
            self.preedit = None;
        }
    }

    /// 쿼리가 바뀐 뒤 다시 훑고, 화면에서 가장 가까운 매치를 고른다.
    fn rescan_search(&mut self) {
        let Some(search) = &mut self.search else {
            return;
        };
        let Some(state) = &self.state else { return };
        let Some(pane) = state.active_tab().root.pane(search.pane_id) else {
            return;
        };

        // 뷰포트 맨 위의 절대 줄을 먼저 읽고 락을 놓는다.
        let (top_abs, _) = {
            let term = pane.session.term.lock();
            let history = term.grid().history_size() as i64;
            let offset = term.grid().display_offset() as i64;
            (history - offset, history)
        };

        {
            let term = pane.session.term.lock();
            search.rescan(&term);
        }
        search.select_nearest(top_abs);

        if let Some(m) = search.current_match() {
            pane.session.scroll_to_abs(m.start_line);
        }
        state.window.request_redraw();
    }

    /// 다음(1)/이전(-1) 매치로 이동하고 화면을 맞춘다.
    fn step_search(&mut self, direction: i32) {
        let Some(search) = &mut self.search else {
            return;
        };
        let Some(state) = &self.state else { return };
        let target = search.step(direction);
        if let (Some(m), Some(pane)) = (target, state.active_tab().root.pane(search.pane_id)) {
            pane.session.scroll_to_abs(m.start_line);
        }
        state.window.request_redraw();
    }

    /// 검색 바가 열려 있을 때의 키 처리. 처리했으면 true.
    pub(super) fn search_key(
        &mut self,
        key: &Key,
        text: Option<&str>,
        typing_allowed: bool,
        mods: winit::keyboard::ModifiersState,
    ) -> bool {
        if self.search.is_none() {
            return false;
        }
        match key {
            Key::Named(NamedKey::Escape) => {
                // 스크롤 위치는 그대로 둔다 — 브라우저·에디터의 Cmd+F와 같다.
                self.close_search();
                return true;
            }
            Key::Named(NamedKey::Enter) => {
                self.step_search(if mods.shift_key() { -1 } else { 1 });
                return true;
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some(search) = &mut self.search {
                    search.query.pop();
                }
                self.rescan_search();
                return true;
            }
            _ => {}
        }

        if typing_allowed {
            if let (Some(search), Some(text)) = (&mut self.search, text) {
                // 제어 문자는 넣지 않는다 (Tab 등이 쿼리에 섞이면 정규식이 깨진다).
                if !text.is_empty() && !text.chars().any(char::is_control) {
                    search.query.push_str(text);
                    self.rescan_search();
                }
            }
        }
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
        true
    }

    /// IME 확정 문자열을 검색 쿼리에 넣는다.
    pub(super) fn search_ime_commit(&mut self, text: &str) {
        if let Some(search) = &mut self.search {
            search.query.push_str(text);
        }
        self.rescan_search();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(start_line: i64, start_col: usize, end_line: i64, end_col: usize) -> AbsMatch {
        AbsMatch {
            start_line,
            start_col,
            end_line,
            end_col,
        }
    }

    fn search_with(matches: Vec<AbsMatch>) -> Search {
        let mut s = Search::new(0);
        s.query = "x".to_string();
        s.matches = matches;
        s
    }

    #[test]
    fn abs_conversion_round_trips_through_history() {
        // history 100, 그리드 Line(-40) → 절대 60
        let point = Point::new(Line(-40), Column(3));
        let range = point..=Point::new(Line(-40), Column(7));
        let am = AbsMatch::from_match(&range, 100);
        assert_eq!(am.start_line, 60);
        assert_eq!(am.start_col, 3);
        assert_eq!(am.end_line, 60);
        assert_eq!(am.end_col, 7);

        // 화면 안(양수 Line)도 동일 규칙
        let range = Point::new(Line(5), Column(0))..=Point::new(Line(5), Column(1));
        assert_eq!(AbsMatch::from_match(&range, 100).start_line, 105);
    }

    #[test]
    fn contains_respects_columns_on_a_single_line() {
        let am = m(50, 3, 50, 7);
        let history = 100;
        // 절대 50 = 그리드 Line(-50)
        let at = |col| Point::new(Line(-50), Column(col));
        assert!(!am.contains(at(2), history), "시작 열 이전");
        assert!(am.contains(at(3), history), "시작 열 포함");
        assert!(am.contains(at(7), history), "끝 열 포함");
        assert!(!am.contains(at(8), history), "끝 열 이후");
    }

    #[test]
    fn contains_spans_multiple_lines() {
        // 절대 50의 5열부터 절대 52의 2열까지
        let am = m(50, 5, 52, 2);
        let history = 100;
        let at = |line: i64, col| Point::new(Line((line - history) as i32), Column(col));

        assert!(!am.contains(at(50, 4), history), "첫 줄은 시작 열부터");
        assert!(am.contains(at(50, 5), history));
        assert!(am.contains(at(51, 0), history), "중간 줄은 전체");
        assert!(am.contains(at(51, 999), history));
        assert!(am.contains(at(52, 2), history));
        assert!(!am.contains(at(52, 3), history), "끝 줄은 끝 열까지");
        assert!(!am.contains(at(49, 0), history), "범위 밖 줄");
        assert!(!am.contains(at(53, 0), history));
    }

    #[test]
    fn status_reflects_the_search_state() {
        let mut s = Search::new(0);
        assert_eq!(s.status(), "", "빈 쿼리는 아무것도 표시하지 않는다");

        s.query = "foo".to_string();
        assert_eq!(s.status(), "no match");

        s.matches = vec![m(1, 0, 1, 2), m(5, 0, 5, 2), m(9, 0, 9, 2)];
        assert_eq!(s.status(), "1/3");
        s.current = 2;
        assert_eq!(s.status(), "3/3");

        s.invalid = true;
        assert_eq!(s.status(), "invalid", "정규식 오류가 최우선");
    }

    #[test]
    fn step_wraps_in_both_directions() {
        let mut s = search_with(vec![m(1, 0, 1, 1), m(2, 0, 2, 1), m(3, 0, 3, 1)]);

        assert_eq!(s.step(1), Some(m(2, 0, 2, 1)));
        assert_eq!(s.step(1), Some(m(3, 0, 3, 1)));
        assert_eq!(s.step(1), Some(m(1, 0, 1, 1)), "끝에서 처음으로 순환");

        assert_eq!(s.step(-1), Some(m(3, 0, 3, 1)), "처음에서 끝으로 순환");
        assert_eq!(s.step(-1), Some(m(2, 0, 2, 1)));
    }

    #[test]
    fn step_on_an_empty_result_set_is_a_noop() {
        let mut s = search_with(Vec::new());
        assert_eq!(s.step(1), None);
        assert_eq!(s.step(-1), None);
        assert_eq!(s.current_index(), None);
    }

    #[test]
    fn select_nearest_prefers_the_first_match_at_or_below_the_viewport_top() {
        let mut s = search_with(vec![m(10, 0, 10, 1), m(50, 0, 50, 1), m(90, 0, 90, 1)]);

        s.select_nearest(0);
        assert_eq!(s.current, 0, "전부 화면 아래면 첫 매치");

        s.select_nearest(40);
        assert_eq!(s.current, 1, "화면 위쪽을 지난 첫 매치");

        s.select_nearest(50);
        assert_eq!(s.current, 1, "경계는 포함");

        s.select_nearest(999);
        assert_eq!(s.current, 2, "전부 화면 위면 마지막 매치");
    }
}
