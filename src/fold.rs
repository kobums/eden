//! 블록 접기 (Phase 25): 접힌 출력 범위를 건너뛰는 "가상 행" 뷰.
//!
//! Term 그리드는 손대지 않는다 — alacritty 그리드에서 줄을 빼면 선택·검색·
//! 마우스 리포팅·리플레이 복원이 전부 흔들린다. 대신 렌더러와 마우스가 그리드
//! 줄을 직접 보는 대신 이 모듈이 만든 행 목록([`ViewRow`])을 본다. 접힌 범위는
//! 행 하나(placeholder)로 줄어들고, 그만큼 위쪽 히스토리가 화면 안으로 들어온다.
//!
//! 뷰포트는 **아래 기준**이다. 맨 아래 행은 그리드 뷰포트의 맨 아래 줄
//! (`last_abs - display_offset`)과 같고, 거기서 위로 올라가며 행을 채운다.
//! 위 기준으로 잡으면 접힌 줄 수만큼 화면 아래가 비어 프롬프트가 위로 떠 버린다.
//!
//! 접힌 범위 안의 줄은 "정규 줄"인 범위 시작 줄로 대표된다 — 스크롤 위치가
//! 접힌 범위 한가운데를 가리켜도 같은 화면이 나오므로, 휠 스크롤은
//! [`FoldView::scrolled`]로 가상 행 단위로 움직여 접힌 구간에서 멈춰 있는
//! 일이 없게 한다.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Term, TermMode};

/// 접힌 출력 범위 (절대 줄 번호, 양 끝 포함). `key`는 블록의 프롬프트 줄로,
/// 블록이 마크에서 매번 다시 도출돼도 변하지 않는 식별자다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    pub key: i64,
    pub start: i64,
    pub end: i64,
}

impl Fold {
    pub fn hidden(&self) -> i64 {
        self.end - self.start + 1
    }
}

/// 화면 행 하나가 보여 주는 것.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewRow {
    /// 그리드의 절대 줄.
    Line(i64),
    /// 접힌 범위의 placeholder (`folds[i]`).
    Fold(usize),
    /// 히스토리가 바닥나 채울 줄이 없음.
    Blank,
}

/// 접힌 범위 목록 위의 가상 행 계산. `folds`는 시작 줄 오름차순이고 서로
/// 겹치지 않는다 (블록 출력 범위는 원래 그렇다). `last_abs`는 그리드의 마지막
/// 줄(화면 맨 아래 줄)의 절대 번호다.
pub struct FoldView<'a> {
    folds: &'a [Fold],
    last_abs: i64,
}

impl<'a> FoldView<'a> {
    pub fn new(folds: &'a [Fold], last_abs: i64) -> Self {
        Self { folds, last_abs }
    }

    /// `abs`를 덮는 접힌 범위의 인덱스.
    pub fn fold_at(&self, abs: i64) -> Option<usize> {
        let i = self.folds.partition_point(|f| f.end < abs);
        (i < self.folds.len() && self.folds[i].start <= abs).then_some(i)
    }

    /// 접힌 범위 안의 줄은 범위 시작 줄로 대표한다.
    fn canonical(&self, abs: i64) -> i64 {
        match self.fold_at(abs) {
            Some(i) => self.folds[i].start,
            None => abs,
        }
    }

    /// 한 행 위의 정규 줄. 0 아래로 내려갈 수 있다 (호출자가 Blank 처리).
    fn up(&self, abs: i64) -> i64 {
        self.canonical(abs - 1)
    }

    /// 한 행 아래의 정규 줄. `last_abs`에서 멈춘다.
    fn down(&self, abs: i64) -> i64 {
        let next = match self.fold_at(abs) {
            Some(i) => self.folds[i].end + 1,
            None => abs + 1,
        };
        self.canonical(next.min(self.last_abs))
    }

    fn row_of(&self, abs: i64) -> ViewRow {
        match self.fold_at(abs) {
            Some(i) => ViewRow::Fold(i),
            None => ViewRow::Line(abs),
        }
    }

    /// 맨 아래 행이 `bottom_abs`인 화면의 행 목록 (위→아래, 길이 `screen`).
    pub fn rows(&self, bottom_abs: i64, screen: usize) -> Vec<ViewRow> {
        let mut rows = vec![ViewRow::Blank; screen];
        let mut abs = self.canonical(bottom_abs.min(self.last_abs));
        for slot in rows.iter_mut().rev() {
            if abs < 0 {
                break;
            }
            *slot = self.row_of(abs);
            abs = self.up(abs);
        }
        rows
    }

    /// `top_abs`가 화면 맨 위 행이 되는 display offset (0 이상, 히스토리
    /// 상한은 호출자가 clamp). 접힌 범위가 없으면 `history - top_abs`와 같다.
    pub fn offset_for_top(&self, top_abs: i64, screen: usize) -> i64 {
        let mut abs = self.canonical(top_abs.max(0));
        for _ in 1..screen {
            abs = self.down(abs);
        }
        (self.last_abs - abs).max(0)
    }

    /// `abs`가 화면 가운데쯤 오는 display offset (검색 히트용 — 주변 맥락이
    /// 보여야 한다).
    pub fn offset_centered(&self, abs: i64, screen: usize) -> i64 {
        let mut top = self.canonical(abs.max(0));
        for _ in 0..screen / 2 {
            let next = self.up(top);
            if next < 0 {
                break;
            }
            top = next;
        }
        self.offset_for_top(top, screen)
    }

    /// 휠 스크롤: 현재 `offset`에서 가상 행 `delta`만큼(양수 = 위로, 과거
    /// 방향) 움직인 새 display offset. `history`는 offset 상한이다.
    pub fn scrolled(&self, offset: i64, delta: i64, history: i64) -> i64 {
        let min_bottom = self.canonical(self.last_abs - history);
        let mut bottom = self.canonical(self.last_abs - offset);
        if delta > 0 {
            for _ in 0..delta {
                let next = self.up(bottom);
                if next < min_bottom {
                    break;
                }
                bottom = next;
            }
        } else {
            for _ in 0..-delta {
                bottom = self.down(bottom);
            }
        }
        (self.last_abs - bottom).clamp(0, history)
    }
}

/// Term의 현재 스크롤 위치에서 화면 행 목록을 만든다.
///
/// 접힌 범위가 없거나 대체 스크린(vim·less)이면 그리드 뷰포트 그대로다 —
/// 대체 스크린에는 블록이 없고 마우스 리포팅 좌표가 화면과 일치해야 한다.
pub fn rows_for<T: EventListener>(term: &Term<T>, folds: &[Fold]) -> Vec<ViewRow> {
    let grid = term.grid();
    let history = grid.history_size() as i64;
    let screen = grid.screen_lines();
    let offset = grid.display_offset() as i64;
    let last_abs = history + screen as i64 - 1;
    if folds.is_empty() || term.mode().contains(TermMode::ALT_SCREEN) {
        let top = history - offset;
        return (0..screen as i64).map(|i| ViewRow::Line(top + i)).collect();
    }
    FoldView::new(folds, last_abs).rows(last_abs - offset, screen)
}

/// 행 목록에서 연속된 그리드 줄 구간들 `[시작, 끝]`(절대 줄). 접힌 자리에서
/// 끊긴다 — URL 스캔처럼 "보이는 줄"만 훑고 싶을 때 쓴다.
pub fn line_runs(rows: &[ViewRow]) -> Vec<(i64, i64)> {
    let mut runs: Vec<(i64, i64)> = Vec::new();
    for row in rows {
        if let ViewRow::Line(abs) = *row {
            match runs.last_mut() {
                Some(run) if run.1 + 1 == abs => run.1 = abs,
                _ => runs.push((abs, abs)),
            }
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(key: i64, start: i64, end: i64) -> Fold {
        Fold { key, start, end }
    }

    #[test]
    fn rows_without_folds_are_the_viewport() {
        let view = FoldView::new(&[], 19);
        let rows = view.rows(19, 5);
        let expect: Vec<_> = (15..=19).map(ViewRow::Line).collect();
        assert_eq!(rows, expect);
    }

    #[test]
    fn fold_collapses_to_one_row_and_pulls_history_in() {
        // 출력 10..=17이 접힘. 맨 아래 19, 화면 5줄.
        let folds = [fold(9, 10, 17)];
        let view = FoldView::new(&folds, 19);
        let rows = view.rows(19, 5);
        assert_eq!(
            rows,
            vec![
                ViewRow::Line(8),
                ViewRow::Line(9),
                ViewRow::Fold(0),
                ViewRow::Line(18),
                ViewRow::Line(19),
            ]
        );
    }

    #[test]
    fn bottom_inside_fold_shows_the_placeholder_at_bottom() {
        let folds = [fold(9, 10, 17)];
        let view = FoldView::new(&folds, 19);
        let rows = view.rows(13, 3);
        assert_eq!(
            rows,
            vec![ViewRow::Line(8), ViewRow::Line(9), ViewRow::Fold(0)]
        );
    }

    #[test]
    fn history_exhaustion_leaves_blank_rows_on_top() {
        let folds = [fold(0, 1, 5)];
        let view = FoldView::new(&folds, 7);
        let rows = view.rows(7, 5);
        assert_eq!(
            rows,
            vec![
                ViewRow::Blank,
                ViewRow::Line(0),
                ViewRow::Fold(0),
                ViewRow::Line(6),
                ViewRow::Line(7),
            ]
        );
    }

    #[test]
    fn adjacent_folds_each_get_a_row() {
        let folds = [fold(0, 1, 3), fold(4, 5, 8)];
        let view = FoldView::new(&folds, 10);
        let rows = view.rows(10, 6);
        assert_eq!(
            rows,
            vec![
                ViewRow::Line(0),
                ViewRow::Fold(0),
                ViewRow::Line(4),
                ViewRow::Fold(1),
                ViewRow::Line(9),
                ViewRow::Line(10),
            ]
        );
    }

    #[test]
    fn offset_for_top_matches_plain_formula_without_folds() {
        // history = 15, 화면 5줄 → last_abs = 19. top=3이면 offset = 15-3 = 12.
        let view = FoldView::new(&[], 19);
        assert_eq!(view.offset_for_top(3, 5), 12);
        // 바닥 근처: top=17이면 뷰포트가 끝에 닿아 offset 0.
        assert_eq!(view.offset_for_top(17, 5), 0);
    }

    #[test]
    fn offset_for_top_skips_folded_lines() {
        let folds = [fold(9, 10, 17)];
        let view = FoldView::new(&folds, 19);
        // top=8, 화면 5줄: 8, 9, [접힘], 18, 19 → 맨 아래 19 → offset 0.
        assert_eq!(view.offset_for_top(8, 5), 0);
        // top=7: 7, 8, 9, [접힘], 18 → offset 1.
        assert_eq!(view.offset_for_top(7, 5), 1);
    }

    #[test]
    fn centered_offset_puts_target_mid_screen() {
        let view = FoldView::new(&[], 99); // history 90, 화면 10줄
        let offset = view.offset_centered(50, 10);
        let rows = view.rows(99 - offset, 10);
        assert_eq!(rows[5], ViewRow::Line(50));
    }

    #[test]
    fn scrolling_up_steps_over_a_fold_in_one_tick() {
        let folds = [fold(9, 10, 17)];
        let view = FoldView::new(&folds, 19);
        let history = 15;
        // 맨 아래(offset 0)에서 한 칸 위: 바닥이 18 → offset 1.
        assert_eq!(view.scrolled(0, 1, history), 1);
        // 다시 한 칸: 바닥이 접힌 행(정규 줄 10) → offset 9.
        assert_eq!(view.scrolled(1, 1, history), 9);
        // 또 한 칸: 바닥이 9 → offset 10.
        assert_eq!(view.scrolled(9, 1, history), 10);
        // 내려오기: 9에서 한 칸 아래는 접힌 행(10), 그 아래는 18.
        assert_eq!(view.scrolled(10, -1, history), 9);
        assert_eq!(view.scrolled(9, -1, history), 1);
        assert_eq!(view.scrolled(1, -5, history), 0, "바닥 아래로는 못 간다");
    }

    #[test]
    fn scrolling_up_stops_at_history_top() {
        let view = FoldView::new(&[], 19);
        assert_eq!(view.scrolled(14, 3, 15), 15);
        assert_eq!(view.scrolled(15, 1, 15), 15);
    }

    #[test]
    fn line_runs_break_at_folds_and_blanks() {
        let rows = [
            ViewRow::Blank,
            ViewRow::Line(3),
            ViewRow::Line(4),
            ViewRow::Fold(0),
            ViewRow::Line(10),
        ];
        assert_eq!(line_runs(&rows), vec![(3, 4), (10, 10)]);
        assert!(line_runs(&[ViewRow::Blank]).is_empty());
    }

    #[test]
    fn fold_at_uses_binary_search_over_sorted_folds() {
        let folds = [fold(0, 1, 3), fold(4, 5, 8), fold(20, 21, 30)];
        let view = FoldView::new(&folds, 40);
        assert_eq!(view.fold_at(0), None);
        assert_eq!(view.fold_at(1), Some(0));
        assert_eq!(view.fold_at(3), Some(0));
        assert_eq!(view.fold_at(4), None);
        assert_eq!(view.fold_at(8), Some(1));
        assert_eq!(view.fold_at(25), Some(2));
        assert_eq!(view.fold_at(31), None);
    }
}
