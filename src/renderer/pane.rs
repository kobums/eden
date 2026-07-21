//! 페인 하나의 터미널 그리드 그리기: 블록 거터, 셀 배경/글리프, 커서, IME preedit.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;

use super::color::ansi_to_rgb;
use super::text::FontSize;
use super::{BgInstance, PADDING, PaneView, Renderer, TextInstance};

/// 블록 상태 바 색: 실행 중 / 성공 / 실패
const BLOCK_RUNNING: [f32; 3] = [0.35, 0.55, 0.95];
const BLOCK_OK: [f32; 3] = [0.35, 0.72, 0.46];
const BLOCK_FAIL: [f32; 3] = [0.92, 0.42, 0.46];

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
        let content = term.renderable_content();
        // 스크롤백을 위로 올렸을 때: 그리드 좌표(line)는 화면 좌표(row)와
        // display_offset만큼 어긋난다.
        let display_offset = content.display_offset as i32;
        let selection = content.selection;
        let cursor_point = content.cursor.point;
        let history = term.grid().history_size() as i64;
        let cursor_row = cursor_point.line.0 + display_offset;
        let visible_lines = Dimensions::screen_lines(term.grid()) as i32;
        let cursor_visible = cursor_row >= 0 && cursor_row < visible_lines;
        let cursor_x = origin_x + cursor_point.column.0 as f32 * self.cell_width;
        let cursor_y = origin_y + cursor_row as f32 * self.cell_height;
        if cursor_visible {
            ime_pos = Some((cursor_x as f64, (cursor_y + self.cell_height) as f64));
        }

        self.draw_block_gutter(
            view.blocks,
            rect,
            origin_y,
            history - display_offset as i64,
            history + cursor_point.line.0 as i64,
            visible_lines,
            bg_instances,
        );

        for indexed in content.display_iter {
            let row = indexed.point.line.0 + display_offset;
            if row < 0 {
                continue;
            }
            let x = origin_x + indexed.point.column.0 as f32 * self.cell_width;
            let y = origin_y + row as f32 * self.cell_height;

            let flags = indexed.flags;
            if flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            let mut fg = ansi_to_rgb(&indexed.fg, &theme);
            let mut bg = ansi_to_rgb(&indexed.bg, &theme);
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }

            let selected = selection
                .as_ref()
                .is_some_and(|range| range.contains(indexed.point));
            if selected {
                bg = theme.selection;
            }

            // 검색 하이라이트는 선택을 이긴다 — 검색이 떠 있는 동안 남아 있는
            // 선택은 거의 항상 낡은 것이다.
            let matched = view
                .matches
                .iter()
                .position(|m| m.contains(indexed.point, history));
            if let Some(i) = matched {
                if Some(i) == view.current_match {
                    bg = theme.search_current();
                    fg = theme.bg; // 원색 위에서는 전경도 뒤집어야 읽힌다
                } else {
                    bg = theme.search_match();
                }
            }

            let is_cursor = view.focused
                && cursor_visible
                && preedit.is_none()
                && indexed.point == cursor_point;
            // 블록 커서만 셀을 반전한다. 바/밑줄은 루프 뒤에서 사각형으로 그린다.
            let block_cursor = is_cursor && theme.cursor_style == crate::config::CursorStyle::Block;
            if block_cursor {
                bg = theme.cursor;
                fg = theme.bg;
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

            // OSC 8 하이퍼링크: 밑줄로 클릭 가능함을 표시
            if indexed.hyperlink().is_some() {
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

            let c = indexed.c;
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
    /// `top_abs`는 화면 최상단의 절대 줄 번호로, 블록의 절대 줄을 화면 행으로
    /// 바꾸는 기준이다. `bottom_abs`는 커서가 있는 절대 줄로, 아직 끝나지 않은
    /// 블록의 끝으로 쓴다.
    fn draw_block_gutter(
        &self,
        blocks: &[crate::session::Block],
        rect: crate::layout::Rect,
        origin_y: f32,
        top_abs: i64,
        bottom_abs: i64,
        visible_lines: i32,
        bg_instances: &mut Vec<BgInstance>,
    ) {
        for block in blocks {
            if block.cmd_abs.is_none() {
                continue; // 명령이 실행되지 않은 프롬프트는 표시하지 않음
            }
            let end_abs = block.end_abs.map(|d| d - 1).unwrap_or(bottom_abs);
            let top_row = block.start_abs - top_abs;
            let bottom_row = (end_abs - top_abs).min(visible_lines as i64 - 1);
            if bottom_row < 0 || top_row >= visible_lines as i64 {
                continue;
            }
            let top_row = top_row.max(0);
            let color = match (block.end_abs, block.exit) {
                (None, _) => BLOCK_RUNNING,
                (_, Some(0)) => BLOCK_OK,
                (_, Some(_)) => BLOCK_FAIL,
                _ => BLOCK_OK,
            };
            bg_instances.push(BgInstance {
                rect: [
                    rect.x + 1.0,
                    origin_y + top_row as f32 * self.cell_height,
                    PADDING - 2.0,
                    (bottom_row - top_row + 1) as f32 * self.cell_height,
                ],
                color: [color[0], color[1], color[2], 1.0],
            });
        }
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
