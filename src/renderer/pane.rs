//! 페인 하나의 터미널 그리드 그리기: 블록 거터, 셀 배경/글리프, 커서, IME preedit.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;

use super::color::{ansi_to_rgb, mix};
use super::text::FontSize;
use super::{BgInstance, PADDING, PaneView, Renderer, TextInstance};
use crate::fold::{Fold, ViewRow};

impl Renderer {
    /// 페인 사각형에서 그리드 크기(열, 행)를 계산한다.
    pub fn pane_grid_size(&self, rect: crate::layout::Rect) -> (usize, usize) {
        let cols = ((rect.w - PADDING * 2.0) / self.cell_width).floor() as usize;
        let lines = ((rect.h - PADDING * 2.0) / self.cell_height).floor() as usize;
        (cols.max(2), lines.max(1))
    }

    /// 페인 하나를 인스턴스 버퍼에 그린다. 포커스된 페인이면 커서 좌표를 돌려준다.
    pub(super) fn draw_pane(
        &mut self,
        view: &PaneView,
        preedit: Option<&str>,
        bg_instances: &mut Vec<BgInstance>,
        text_instances: &mut Vec<TextInstance>,
    ) -> Option<(f64, f64)> {
        let theme = self.theme;
        let rect = view.rect;
        let origin_x = rect.x + PADDING;
        let origin_y = rect.y + PADDING;
        let mut ime_pos = None;

        // 페인 배경 (구분선은 페인 사이 틈으로 드러난다).
        // iTerm2처럼 기본 배경에만 투명도를 적용한다 (셀 배경색·텍스트는 불투명).
        bg_instances.push(BgInstance {
            rect: [rect.x, rect.y, rect.w, rect.h],
            color: [theme.bg[0], theme.bg[1], theme.bg[2], theme.opacity],
        });

        let term = view.term.lock();
        let grid = term.grid();
        let history = grid.history_size() as i64;
        let cols = grid.columns();
        // 화면 행 목록: 접힌 블록이 없으면 그리드 뷰포트 그대로, 있으면 접힌
        // 범위를 placeholder 한 행으로 줄인 목록 (Phase 25). 아래 셀 루프와
        // 거터·커서·힌트·마우스가 전부 이 목록 하나를 기준으로 삼는다.
        let rows = crate::fold::rows_for(&term, view.folds);
        // 평문 URL 매치는 그리드 좌표라 락이 살아 있는 동안만 유효하다 —
        // 여기서 한 번 스캔하고 같은 락 안의 셀 루프에서만 소비한다.
        // 셀마다 정규식을 돌리지 않는 것은 검색 하이라이트와 같은 관용구.
        let urls = crate::app::links::urls_in_rows(&term, &rows);
        let selection = term.selection.as_ref().and_then(|s| s.to_range(&term));
        let cursor_point = grid.cursor.point;
        let cursor_abs = history + cursor_point.line.0 as i64;
        let cursor_row = rows
            .iter()
            .position(|row| *row == ViewRow::Line(cursor_abs));
        let cursor_visible = cursor_row.is_some();
        let cursor_x = origin_x + cursor_point.column.0 as f32 * self.cell_width;
        let cursor_y = origin_y + cursor_row.unwrap_or(0) as f32 * self.cell_height;
        if cursor_visible {
            ime_pos = Some((cursor_x as f64, (cursor_y + self.cell_height) as f64));
        }

        self.draw_block_gutter(
            view.blocks,
            view.folds,
            &rows,
            rect,
            origin_y,
            cursor_abs,
            bg_instances,
        );

        for (row, view_row) in rows.iter().enumerate() {
            let y = origin_y + row as f32 * self.cell_height;
            let line = match *view_row {
                ViewRow::Blank => continue,
                ViewRow::Fold(i) => {
                    self.draw_fold_row(
                        &view.folds[i],
                        rect,
                        origin_x,
                        y,
                        bg_instances,
                        text_instances,
                    );
                    continue;
                }
                ViewRow::Line(abs) => Line((abs - history) as i32),
            };
            let grid_row = &grid[line];
            for col in 0..cols {
                let cell = &grid_row[Column(col)];
                let point = Point::new(line, Column(col));
                let x = origin_x + col as f32 * self.cell_width;

                let flags = cell.flags;
                if flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }

                let mut fg = ansi_to_rgb(&cell.fg, &theme);
                let mut bg = ansi_to_rgb(&cell.bg, &theme);
                if flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }

                let selected = selection
                    .as_ref()
                    .is_some_and(|range| range.contains(point));
                if selected {
                    bg = theme.selection;
                }

                // 검색 하이라이트는 선택을 이긴다 — 검색이 떠 있는 동안 남아 있는
                // 선택은 거의 항상 낡은 것이다.
                let matched = view.matches.iter().position(|m| m.contains(point, history));
                if let Some(i) = matched {
                    if Some(i) == view.current_match {
                        bg = theme.search_current();
                        fg = theme.bg; // 원색 위에서는 전경도 뒤집어야 읽힌다
                    } else {
                        bg = theme.search_match();
                    }
                }

                let is_cursor =
                    view.focused && cursor_visible && preedit.is_none() && point == cursor_point;
                // 블록 커서만 셀을 반전한다. 바/밑줄은 루프 뒤에서 사각형으로 그린다.
                let block_cursor =
                    is_cursor && theme.cursor_style == crate::config::CursorStyle::Block;
                if block_cursor {
                    bg = theme.cursor;
                    fg = theme.bg;
                }

                // 비활성 페인 디밍: 전경·셀 배경을 기본 배경 쪽으로 죽여 포커스된
                // 페인이 한눈에 드러나게 한다 (iTerm2 Dim inactive split panes).
                // 커서·preedit은 포커스된 페인에만 그려지므로 여기 올 일이 없다.
                if !view.focused && theme.inactive_dim > 0.0 {
                    fg = mix(fg, theme.bg, theme.inactive_dim);
                    bg = mix(bg, theme.bg, theme.inactive_dim);
                }

                let width_cells = if flags.contains(Flags::WIDE_CHAR) {
                    2.0
                } else {
                    1.0
                };
                if block_cursor || selected || matched.is_some() || bg != theme.bg {
                    bg_instances.push(BgInstance {
                        rect: [x, y, self.cell_width * width_cells, self.cell_height],
                        color: [bg[0], bg[1], bg[2], 1.0],
                    });
                }

                // 하이퍼링크(OSC 8 또는 평문 URL): 밑줄로 클릭 가능함을 표시.
                // 겹치면 어차피 같은 밑줄이고, 클릭 쪽에서 OSC 8이 우선한다.
                if cell.hyperlink().is_some() || urls.iter().any(|m| m.contains(&point)) {
                    bg_instances.push(BgInstance {
                        rect: [
                            x,
                            y + self.cell_height - 2.0,
                            self.cell_width * width_cells,
                            1.5,
                        ],
                        color: [fg[0], fg[1], fg[2], 1.0],
                    });
                }

                let c = cell.c;
                if c == ' ' || flags.contains(Flags::HIDDEN) {
                    continue;
                }
                // 주의: self.glyph는 &mut self가 필요하므로 lock 밖으로 문자를 모으는
                // 대신, FairMutex 잠금 중 atlas 업로드를 허용한다 (queue.write_texture는
                // 즉시 반환되므로 잠금 시간에 미치는 영향은 작다).
                if let Some(glyph) = self.glyph(c) {
                    text_instances.push(TextInstance {
                        rect: [
                            x + glyph.offset[0],
                            y + glyph.offset[1],
                            glyph.size[0],
                            glyph.size[1],
                        ],
                        uv: glyph.uv,
                        color: [fg[0], fg[1], fg[2], 1.0],
                    });
                }
            }
        }

        // 바/밑줄 커서 (블록은 셀 반전으로 이미 처리됨)
        if view.focused && cursor_visible && preedit.is_none() {
            self.draw_cursor(cursor_x, cursor_y, bg_instances);
        }

        // IME 조합 중 문자열(preedit)을 커서 위치에 오버레이로 그린다.
        if let (Some(text), true) = (preedit, cursor_visible) {
            self.draw_preedit(text, cursor_x, cursor_y, bg_instances, text_instances);
        }

        ime_pos
    }

    /// 왼쪽 거터의 블록 상태 바 (실행 중/성공/실패).
    ///
    /// 블록의 절대 줄 범위를 화면 행으로 바꿀 때 `rows`를 쓴다 — 접힌 블록은
    /// 프롬프트·명령 줄과 placeholder 행 하나만 차지한다. `cursor_abs`는
    /// 아직 끝나지 않은 블록의 끝이다.
    #[allow(clippy::too_many_arguments)]
    fn draw_block_gutter(
        &self,
        blocks: &[crate::session::Block],
        folds: &[Fold],
        rows: &[ViewRow],
        rect: crate::layout::Rect,
        origin_y: f32,
        cursor_abs: i64,
        bg_instances: &mut Vec<BgInstance>,
    ) {
        if !self.theme.block_gutter {
            return;
        }
        // 행마다 덮는 절대 줄 구간. 블록 ↔ 행 교차 판정에 쓴다.
        let spans: Vec<Option<(i64, i64)>> = rows
            .iter()
            .map(|row| match *row {
                ViewRow::Line(abs) => Some((abs, abs)),
                ViewRow::Fold(i) => Some((folds[i].start, folds[i].end)),
                ViewRow::Blank => None,
            })
            .collect();
        let Some(lo) = spans.iter().flatten().map(|s| s.0).min() else {
            return;
        };
        let hi = spans.iter().flatten().map(|s| s.1).max().unwrap_or(lo);

        for block in blocks {
            if block.cmd_abs.is_none() {
                continue; // 명령이 실행되지 않은 프롬프트는 표시하지 않음
            }
            let end_abs = block.end_abs.map(|d| d - 1).unwrap_or(cursor_abs);
            if end_abs < lo || block.start_abs > hi {
                continue;
            }
            let first = spans
                .iter()
                .position(|s| s.is_some_and(|(_, h)| h >= block.start_abs));
            let last = spans
                .iter()
                .rposition(|s| s.is_some_and(|(l, _)| l <= end_abs));
            let (Some(first), Some(last)) = (first, last) else {
                continue;
            };
            if last < first {
                continue;
            }
            let color = match (block.end_abs, block.exit) {
                (None, _) => self.theme.block_running,
                (_, Some(0)) => self.theme.block_ok,
                (_, Some(_)) => self.theme.block_fail,
                _ => self.theme.block_ok,
            };
            bg_instances.push(BgInstance {
                rect: [
                    rect.x + 1.0,
                    origin_y + first as f32 * self.cell_height,
                    PADDING - 2.0,
                    (last - first + 1) as f32 * self.cell_height,
                ],
                color: [color[0], color[1], color[2], 1.0],
            });
        }
    }

    /// 접힌 출력의 placeholder 행: 옅은 띠 위에 숨긴 줄 수. 클릭하면 펼쳐진다.
    fn draw_fold_row(
        &mut self,
        fold: &Fold,
        rect: crate::layout::Rect,
        origin_x: f32,
        y: f32,
        bg_instances: &mut Vec<BgInstance>,
        text_instances: &mut Vec<TextInstance>,
    ) {
        let theme = self.theme;
        let band = mix(theme.bg, theme.fg, 0.08);
        bg_instances.push(BgInstance {
            rect: [origin_x, y, rect.w - PADDING * 2.0, self.cell_height],
            color: [band[0], band[1], band[2], 1.0],
        });
        let label = fold_label(fold.hidden());
        let dim = mix(theme.fg, theme.bg, 0.35);
        self.draw_text(
            &label,
            origin_x + self.cell_width,
            y,
            rect.x + rect.w - PADDING,
            dim,
            FontSize::Term,
            text_instances,
        );
    }

    /// 설정된 모양의 커서. 블록은 셀 반전으로 이미 그려졌으므로 여기선 건너뛴다.
    fn draw_cursor(&self, cursor_x: f32, cursor_y: f32, bg_instances: &mut Vec<BgInstance>) {
        use crate::config::CursorStyle;
        let c = self.theme.cursor;
        let rect = match self.theme.cursor_style {
            CursorStyle::Bar => [cursor_x, cursor_y, 2.0, self.cell_height],
            CursorStyle::Underline => [
                cursor_x,
                cursor_y + self.cell_height - 2.0,
                self.cell_width,
                2.0,
            ],
            CursorStyle::Block => return,
        };
        bg_instances.push(BgInstance {
            rect,
            color: [c[0], c[1], c[2], 1.0],
        });
    }

    /// IME 조합 중 문자열을 커서 위치에 반전색으로 오버레이한다.
    fn draw_preedit(
        &mut self,
        text: &str,
        cursor_x: f32,
        cursor_y: f32,
        bg_instances: &mut Vec<BgInstance>,
        text_instances: &mut Vec<TextInstance>,
    ) {
        let theme = self.theme;
        let mut x = cursor_x;
        for ch in text.chars() {
            let w = self.char_advance(ch, FontSize::Term);
            bg_instances.push(BgInstance {
                rect: [x, cursor_y, w, self.cell_height],
                color: [theme.fg[0], theme.fg[1], theme.fg[2], 1.0],
            });
            if let Some(glyph) = self.glyph(ch) {
                text_instances.push(TextInstance {
                    rect: [
                        x + glyph.offset[0],
                        cursor_y + glyph.offset[1],
                        glyph.size[0],
                        glyph.size[1],
                    ],
                    uv: glyph.uv,
                    color: [theme.bg[0], theme.bg[1], theme.bg[2], 1.0],
                });
            }
            x += w;
        }
    }
}

/// placeholder 행 문구. 글리프가 폰트에 없으면 렌더러가 조용히 건너뛴다.
pub(crate) fn fold_label(hidden: i64) -> String {
    format!("\u{25B8} {hidden} lines folded")
}
