//! 평문 URL 자동 감지 (Phase 16).
//!
//! OSC 8로 선언되지 않은, 화면에 그냥 찍힌 `https://…` 텍스트를 찾아
//! 밑줄을 긋고 Cmd+클릭으로 열 수 있게 한다. 정규식 엔진은 검색(Phase 11)과
//! 같은 alacritty 것을 쓴다 — 줄바꿈 래핑·와이드 문자를 알아서 처리한다.
//!
//! 검색과 달리 **뷰포트만** 스캔한다. 매 redraw마다 돌기 때문에 스크롤백
//! 전체는 과하고, 클릭할 수 있는 건 어차피 보이는 것뿐이다. 그래서 매치도
//! 절대 줄 번호(`AbsMatch`)가 아니라 그리드 좌표(`Match`) 그대로 쓴다 —
//! 스캔한 그 프레임 안에서만(같은 락 안에서) 소비하므로 좌표가 밀릴 틈이 없다.

use std::sync::{Mutex, OnceLock};

use alacritty_terminal::Term;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Direction, Line, Point};
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};

/// 한 화면에서 모을 매치 상한. 뷰포트 한정이라 실제로는 닿을 일이 없지만,
/// `yes https://a.com` 같은 화면 전체 도배에도 상한을 둔다.
const MAX_MATCHES: usize = 256;

/// URL 스킴 allowlist (Alacritty hints 기본값 차용).
///
/// `www.` 같은 스킴 없는 휴리스틱은 일부러 안 한다 — 오탐이 많고,
/// 스킴이 없으면 `open`에 넘길 수도 없다.
const URL_REGEX: &str =
    r#"(https?://|file://|git://|ssh:|ftp://|mailto:)[^\x00-\x1f\x7f-\x9f<>"\s{}⟨⟩`^]+"#;

/// 끝 문장부호 정리 후 이것만 남으면 매치를 버린다 (`https://.` 같은 입력).
const BARE_SCHEMES: &[&str] = &[
    "https://", "http://", "file://", "git://", "ssh:", "ftp://", "mailto:",
];

/// 컴파일된 정규식. `RegexSearch`는 DFA 캐시 때문에 스캔 시 `&mut`가
/// 필요하므로 `OnceLock`만으로는 안 되고 Mutex로 감싼다. 스캔은 메인
/// 스레드(redraw)에서만 일어나 경합은 사실상 없다.
fn regex() -> &'static Mutex<RegexSearch> {
    static REGEX: OnceLock<Mutex<RegexSearch>> = OnceLock::new();
    REGEX.get_or_init(|| {
        Mutex::new(RegexSearch::new(URL_REGEX).expect("URL 정규식은 컴파일 상수다"))
    })
}

/// 뷰포트(화면에 보이는 영역)만 스캔해 평문 URL 매치를 돌려준다.
///
/// 매치는 **현재 그리드 좌표**다. 반환값은 스캔에 쓴 `term` 락이 살아 있는
/// 동안만 유효하다 — 락을 놓고 나면 PTY 출력으로 좌표가 밀릴 수 있으므로
/// 프레임을 넘겨 캐시하면 안 된다.
pub(crate) fn visible_urls<T: EventListener>(term: &Term<T>) -> Vec<Match> {
    let grid = term.grid();
    let cols = grid.columns();
    let screen_lines = grid.screen_lines();
    if cols == 0 || screen_lines == 0 {
        return Vec::new();
    }

    // 스크롤을 올렸으면(display_offset > 0) 뷰포트는 그리드 좌표계에서
    // 위로 밀려 있다. 화면 최상단 = Line(-offset).
    let offset = grid.display_offset() as i32;
    let start = Point::new(Line(-offset), Column(0));
    let end = Point::new(Line(screen_lines as i32 - 1 - offset), Column(cols - 1));

    let mut regex = regex()
        .lock()
        .expect("URL 정규식 락이 poison될 코드가 없다");
    RegexIter::new(start, end, Direction::Right, term, &mut regex)
        .take(MAX_MATCHES)
        .filter_map(|m| trim_trailing(term, m))
        .collect()
}

/// `point`(그리드 좌표)를 덮는 평문 URL이 있으면 그 문자열을 돌려준다.
///
/// OSC 8 확인 뒤의 폴백으로만 쓴다 — 명시된 링크가 추론보다 우선이다.
pub(crate) fn url_at<T: EventListener>(term: &Term<T>, point: Point) -> Option<String> {
    visible_urls(term)
        .into_iter()
        .find(|m| m.contains(&point))
        .map(|m| term.bounds_to_string(*m.start(), *m.end()))
}

/// 매치 끝의 문장부호를 잘라낸 범위를 돌려준다. 산문·마크다운 속 URL은
/// `https://a.com.` / `(https://a.com)`처럼 문장부호가 붙어 오는 게 보통이다.
///
/// 괄호는 무조건 자르면 `https://a.com/b(c)` 같은 정상 URL이 깨지므로,
/// **매치 안의 여닫이 짝**을 세서 짝 없는 닫는 괄호만 자른다.
/// (여는 괄호가 매치 밖에 있는 `(https://a.com)` 케이스가 바로 짝 없음이다.)
fn trim_trailing<T: EventListener>(term: &Term<T>, m: Match) -> Option<Match> {
    let text = term.bounds_to_string(*m.start(), *m.end());
    let mut chars: Vec<char> = text.chars().collect();
    let mut cut = 0usize;

    loop {
        let should_cut = match chars.last() {
            Some('.' | ',' | ';' | ':' | '!' | '?') => true,
            Some(')') => count(&chars, ')') > count(&chars, '('),
            Some(']') => count(&chars, ']') > count(&chars, '['),
            _ => false,
        };
        if !should_cut {
            break;
        }
        chars.pop();
        cut += 1;
    }

    // 문장부호를 걷어냈더니 스킴만 남았다면 URL이 아니다 (`https://.` 등).
    let rest: String = chars.iter().collect();
    if rest.is_empty() || BARE_SCHEMES.contains(&rest.as_str()) {
        return None;
    }
    if cut == 0 {
        return Some(m);
    }

    // 끝점을 cut 셀만큼 되돌린다. 잘리는 문자는 전부 ASCII 문장부호(1셀)라
    // 문자 수 = 셀 수가 보장된다. 래핑된 매치면 윗줄로 넘어간다.
    let cols = term.grid().columns();
    let mut end = *m.end();
    for _ in 0..cut {
        if end.column.0 == 0 {
            end.line = Line(end.line.0 - 1);
            end.column = Column(cols - 1);
        } else {
            end.column = Column(end.column.0 - 1);
        }
    }
    Some(*m.start()..=end)
}

fn count(chars: &[char], target: char) -> usize {
    chars.iter().filter(|&&c| c == target).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Scroll;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use crate::session::TermSize;

    /// PTY도 데몬도 없이 진짜 `Term`을 만든다 (search.rs 테스트와 동일).
    fn term(cols: usize, lines: usize, scrollback: usize) -> Term<VoidListener> {
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        Term::new(
            config,
            &TermSize {
                columns: cols,
                lines,
            },
            VoidListener,
        )
    }

    /// 파서를 거쳐 텍스트를 흘려넣는다 (실제 PTY 출력과 같은 경로).
    fn feed(term: &mut Term<VoidListener>, text: &str) {
        let mut parser: Processor = Processor::new();
        parser.advance(term, text.as_bytes());
    }

    /// 매치가 가리키는 텍스트. 정리 규칙 검증은 결국 "무엇이 남았나"다.
    fn texts(t: &Term<VoidListener>) -> Vec<String> {
        visible_urls(t)
            .iter()
            .map(|m| t.bounds_to_string(*m.start(), *m.end()))
            .collect()
    }

    #[test]
    fn finds_a_basic_url() {
        let mut t = term(60, 10, 100);
        feed(&mut t, "see https://example.com/path here\r\n");
        assert_eq!(texts(&t), vec!["https://example.com/path"]);
    }

    #[test]
    fn finds_multiple_urls_and_schemes() {
        let mut t = term(80, 10, 100);
        feed(
            &mut t,
            "https://a.com and http://b.org\r\nmailto:x@y.z\r\nfile:///tmp/f\r\n",
        );
        assert_eq!(
            texts(&t),
            vec![
                "https://a.com",
                "http://b.org",
                "mailto:x@y.z",
                "file:///tmp/f"
            ]
        );
    }

    #[test]
    fn plain_text_has_no_matches() {
        let mut t = term(60, 10, 100);
        feed(&mut t, "no links here, just http talk without scheme\r\n");
        assert!(visible_urls(&t).is_empty());
    }

    #[test]
    fn wrapped_url_is_a_single_match() {
        // 20열 화면에 긴 URL — 두 줄로 래핑되지만 한 매치여야 한다.
        let mut t = term(20, 10, 100);
        feed(&mut t, "https://example.com/aaaa/bbbb\r\n");

        let urls = visible_urls(&t);
        assert_eq!(urls.len(), 1, "래핑돼도 매치는 하나");
        let m = &urls[0];
        assert!(
            m.end().line > m.start().line,
            "실제로 줄을 넘어야 유효한 테스트"
        );
        assert_eq!(texts(&t), vec!["https://example.com/aaaa/bbbb"]);
    }

    #[test]
    fn trailing_punctuation_is_trimmed() {
        let mut t = term(60, 10, 100);
        feed(&mut t, "read https://a.com. then https://b.org, ok?\r\n");
        assert_eq!(texts(&t), vec!["https://a.com", "https://b.org"]);
    }

    #[test]
    fn unmatched_closing_paren_is_trimmed() {
        // 여는 괄호는 매치 밖(스킴 앞)에 있다 — 매치 안에서는 짝이 없다.
        let mut t = term(60, 10, 100);
        feed(&mut t, "(https://a.com) and [https://b.org]\r\n");
        assert_eq!(texts(&t), vec!["https://a.com", "https://b.org"]);
    }

    #[test]
    fn balanced_parens_inside_the_url_survive() {
        // 위키백과식 URL — 짝이 맞으므로 잘라내면 안 된다.
        let mut t = term(60, 10, 100);
        feed(&mut t, "https://a.com/b(c)\r\n");
        assert_eq!(texts(&t), vec!["https://a.com/b(c)"]);
    }

    #[test]
    fn mixed_trailing_punctuation_is_trimmed_repeatedly() {
        // 마크다운 링크 끝: `).` — 안쪽부터 하나씩 걷어내야 한다.
        let mut t = term(60, 10, 100);
        feed(&mut t, "([link](https://a.com/b(c)).)\r\n");
        assert_eq!(texts(&t), vec!["https://a.com/b(c)"]);
    }

    #[test]
    fn scheme_only_leftover_is_dropped() {
        let mut t = term(60, 10, 100);
        feed(&mut t, "broken https://. nothing\r\n");
        assert!(visible_urls(&t).is_empty(), "스킴만 남으면 URL이 아니다");
    }

    #[test]
    fn scrollback_is_not_scanned() {
        // 화면 5줄에 URL을 먼저 찍고 30줄로 밀어낸다.
        let mut t = term(60, 5, 100);
        feed(&mut t, "https://gone.example\r\n");
        for i in 0..30 {
            feed(&mut t, &format!("filler {i}\r\n"));
        }
        assert!(t.grid().history_size() > 0, "히스토리로 밀려나야 한다");
        assert!(
            visible_urls(&t).is_empty(),
            "뷰포트 밖(스크롤백)은 스캔하지 않는다"
        );
    }

    #[test]
    fn scrolled_up_viewport_finds_the_url_there() {
        // 스크롤백으로 밀린 URL이라도 뷰포트를 올려서 보이면 잡혀야 한다.
        let mut t = term(60, 5, 100);
        feed(&mut t, "https://old.example\r\n");
        for i in 0..30 {
            feed(&mut t, &format!("filler {i}\r\n"));
        }
        t.scroll_display(Scroll::Top);
        assert_eq!(texts(&t), vec!["https://old.example"]);
    }

    #[test]
    fn url_at_hits_the_url_cells_and_misses_elsewhere() {
        let mut t = term(60, 10, 100);
        feed(&mut t, "go https://a.com. now\r\n");

        // "go " 다음 3열부터 URL. 문장부호 정리 후 마지막 셀은 'm'(15열).
        let on = Point::new(Line(0), Column(3));
        let last = Point::new(Line(0), Column(15));
        let dot = Point::new(Line(0), Column(16));
        let off = Point::new(Line(1), Column(0));
        assert_eq!(url_at(&t, on).as_deref(), Some("https://a.com"));
        assert_eq!(url_at(&t, last).as_deref(), Some("https://a.com"));
        assert_eq!(url_at(&t, dot), None, "잘려나간 문장부호 셀은 URL이 아니다");
        assert_eq!(url_at(&t, off), None);
    }

    #[test]
    fn trimming_a_wrapped_match_steps_back_across_the_line_break() {
        // 닫는 괄호가 정확히 다음 줄 첫 칸에 오도록 폭을 맞춘다 —
        // 끝점 되돌리기가 줄 경계를 넘는 경로의 회귀 방지.
        let url = "https://ex.com/pad";
        let mut t = term(url.len() + 1, 10, 100); // "(" + url = 폭, ")"는 둘째 줄
        feed(&mut t, &format!("({url})\r\n"));

        let urls = visible_urls(&t);
        assert_eq!(urls.len(), 1);
        assert_eq!(
            urls[0].end().line,
            Line(0),
            "끝점이 래핑 전 줄로 되돌아와야 한다"
        );
        assert_eq!(texts(&t), vec![url.to_string()]);
    }
}
