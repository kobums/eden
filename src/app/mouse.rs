//! 마우스: 포커스 이동, 드래그 선택, 더블/트리플 클릭, 휠 스크롤,
//! Cmd+클릭(하이퍼링크 / 블록 선택).

use std::time::{Duration, Instant};

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::viewport_to_point;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta};

use super::{App, State};
use crate::layout::{Pane, Rect};

/// 더블/트리플 클릭 판정 간격.
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(400);
/// 휠 한 칸당 스크롤 줄 수.
const SCROLL_LINES_PER_TICK: f32 = 3.0;
/// 페인 안쪽 여백 (렌더러의 PADDING과 같아야 한다).
const PANE_PADDING: f64 = 8.0;

impl App {
    /// 좌표에 있는 페인 ID와 사각형.
    fn pane_at(state: &State, pos: PhysicalPosition<f64>) -> Option<(usize, Rect)> {
        state
            .pane_rects()
            .into_iter()
            .find(|(_, rect)| rect.contains(pos.x, pos.y))
    }

    /// 포커스된 페인과 그 사각형.
    fn focused_pane_rect(state: &State) -> Option<(&Pane, Rect)> {
        let pane = state.focused_pane();
        let rect = state
            .pane_rects()
            .into_iter()
            .find(|(id, _)| *id == pane.id)
            .map(|(_, rect)| rect)?;
        Some((pane, rect))
    }

    /// 마우스 물리 좌표 → 해당 페인의 그리드 좌표(스크롤백 반영)와 셀 내 좌/우 반쪽.
    fn grid_point(
        pane: &Pane,
        rect: Rect,
        state: &State,
        pos: PhysicalPosition<f64>,
    ) -> (Point, Side) {
        let cell_w = state.renderer.cell_width as f64;
        let cell_h = state.renderer.cell_height as f64;
        let origin_x = rect.x as f64 + PANE_PADDING;
        let origin_y = rect.y as f64 + PANE_PADDING;

        let term = pane.session.term.lock();
        let grid = term.grid();
        let cols = grid.columns();
        let lines = grid.screen_lines();
        let display_offset = grid.display_offset();
        drop(term);

        let col = (((pos.x - origin_x) / cell_w).floor().max(0.0) as usize).min(cols - 1);
        let line = (((pos.y - origin_y) / cell_h).floor().max(0.0) as usize).min(lines - 1);
        let point = viewport_to_point(display_offset, Point::new(line, Column(col)));

        let in_cell_x = (pos.x - origin_x) - col as f64 * cell_w;
        let side = if in_cell_x < cell_w / 2.0 {
            Side::Left
        } else {
            Side::Right
        };
        (point, side)
    }

    pub(super) fn on_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        self.mouse_pos = position;
        if !self.left_button_down {
            return;
        }
        let state = self.state.as_ref().unwrap();
        let Some((pane, rect)) = Self::focused_pane_rect(state) else {
            return;
        };
        let (point, side) = Self::grid_point(pane, rect, state, position);
        let mut term = pane.session.term.lock();
        if let Some(selection) = term.selection.as_mut() {
            selection.update(point, side);
        }
        drop(term);
        state.window.request_redraw();
    }

    pub(super) fn on_mouse_input(&mut self, button_state: ElementState, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }
        if button_state == ElementState::Pressed {
            // 탭 바 클릭 → 탭 전환
            let state = self.state.as_ref().unwrap();
            let hit = state
                .renderer
                .tab_hit(self.mouse_pos.x, self.mouse_pos.y, state.tabs.len());
            if let Some(index) = hit {
                self.switch_tab(index);
                return;
            }

            // 페인 클릭 → 포커스 이동
            let state = self.state.as_mut().unwrap();
            if let Some((pane_id, _)) = Self::pane_at(state, self.mouse_pos) {
                let active = state.active;
                if state.tabs[active].focused != pane_id {
                    state.tabs[active].focused = pane_id;
                    self.preedit = None;
                }
            }

            // Cmd+클릭: 하이퍼링크(OSC 8) 열기, 없으면 블록 전체 선택
            if self.modifiers.state().super_key() {
                if !self.open_hyperlink_at(self.mouse_pos) {
                    self.select_block_at(self.mouse_pos);
                }
                self.left_button_down = false;
                return;
            }
        }

        let state = self.state.as_ref().unwrap();
        match button_state {
            ElementState::Pressed => {
                let Some((pane, rect)) = Self::focused_pane_rect(state) else {
                    return;
                };
                self.left_button_down = true;
                let (point, side) = Self::grid_point(pane, rect, state, self.mouse_pos);

                // 더블/트리플 클릭 판정
                let now = Instant::now();
                let is_multi = self
                    .last_click_at
                    .is_some_and(|at| now - at < MULTI_CLICK_INTERVAL)
                    && self.last_click_point == Some(point);
                self.click_count = if is_multi { self.click_count + 1 } else { 1 };
                self.last_click_at = Some(now);
                self.last_click_point = Some(point);

                let ty = match self.click_count {
                    1 => SelectionType::Simple,
                    2 => SelectionType::Semantic,
                    _ => SelectionType::Lines,
                };
                let mut term = pane.session.term.lock();
                term.selection = Some(Selection::new(ty, point, side));
                drop(term);
                state.window.request_redraw();
            }
            ElementState::Released => {
                self.left_button_down = false;
                // 빈 선택(클릭만)은 해제
                let mut term = state.focused_pane().session.term.lock();
                let empty = term.selection.as_ref().is_some_and(|s| s.is_empty());
                if empty && self.click_count == 1 {
                    term.selection = None;
                }
                drop(term);
                state.window.request_redraw();
            }
        }
    }

    pub(super) fn on_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let state = self.state.as_ref().unwrap();
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => {
                self.scroll_accum = 0.0;
                y * SCROLL_LINES_PER_TICK
            }
            // 트랙패드는 픽셀 단위로 오므로 셀 높이만큼 모아 한 줄씩 소비한다.
            MouseScrollDelta::PixelDelta(pos) => {
                self.scroll_accum += pos.y as f32 / state.renderer.cell_height;
                let lines = self.scroll_accum.trunc();
                self.scroll_accum -= lines;
                lines
            }
        };
        if lines == 0.0 {
            return;
        }

        // 마우스가 올라가 있는 페인을 스크롤 (없으면 포커스된 페인)
        let tab = state.active_tab();
        let pane = Self::pane_at(state, self.mouse_pos)
            .and_then(|(id, _)| tab.root.pane(id))
            .unwrap_or_else(|| state.focused_pane());
        let mut term = pane.session.term.lock();
        if term.mode().contains(TermMode::ALT_SCREEN) {
            // 대체 스크린(less, vim 등)에는 히스토리가 없으므로 화살표로 변환
            drop(term);
            let seq: &[u8] = if lines > 0.0 { b"\x1b[A" } else { b"\x1b[B" };
            let mut bytes = Vec::new();
            for _ in 0..lines.abs() as usize {
                bytes.extend_from_slice(seq);
            }
            pane.session.write(bytes);
        } else {
            term.scroll_display(Scroll::Delta(lines as i32));
            drop(term);
        }
        state.window.request_redraw();
    }

    /// 클릭 위치 셀에 OSC 8 하이퍼링크가 있으면 열고 true를 돌려준다.
    fn open_hyperlink_at(&self, pos: PhysicalPosition<f64>) -> bool {
        let Some(state) = &self.state else {
            return false;
        };
        let Some((pane_id, rect)) = Self::pane_at(state, pos) else {
            return false;
        };
        let Some(pane) = state.active_tab().root.pane(pane_id) else {
            return false;
        };
        let (point, _) = Self::grid_point(pane, rect, state, pos);
        let uri = {
            let term = pane.session.term.lock();
            term.grid()[point].hyperlink().map(|h| h.uri().to_string())
        };
        if let Some(uri) = uri {
            // macOS `open`으로 기본 앱에서 연다
            let _ = std::process::Command::new("open").arg(&uri).spawn();
            return true;
        }
        false
    }

    /// 클릭한 위치가 속한 블록 전체를 선택한다 (Cmd+클릭).
    fn select_block_at(&mut self, pos: PhysicalPosition<f64>) {
        let Some(state) = &self.state else { return };
        let Some((pane_id, rect)) = Self::pane_at(state, pos) else {
            return;
        };
        let Some(pane) = state.active_tab().root.pane(pane_id) else {
            return;
        };
        let (point, _) = Self::grid_point(pane, rect, state, pos);
        let blocks = pane.session.blocks();

        let mut term = pane.session.term.lock();
        let history = term.grid().history_size() as i64;
        let cols = term.grid().columns();
        let cursor_abs = history + term.grid().cursor.point.line.0 as i64;
        let clicked_abs = history + point.line.0 as i64;

        for (i, block) in blocks.iter().enumerate() {
            // 블록의 화면상 범위: 시작(A) ~ 다음 블록 시작 전 줄 (혹은 D-1 / 현재 커서)
            let span_end = blocks
                .get(i + 1)
                .map(|next| next.start_abs - 1)
                .unwrap_or(cursor_abs);
            if block.start_abs <= clicked_abs && clicked_abs <= span_end {
                let sel_end = block
                    .end_abs
                    .map(|d| d - 1)
                    .unwrap_or(span_end)
                    .max(block.start_abs);
                let start = Point::new(Line((block.start_abs - history) as i32), Column(0));
                let end = Point::new(Line((sel_end - history) as i32), Column(cols - 1));
                let mut selection = Selection::new(SelectionType::Lines, start, Side::Left);
                selection.update(end, Side::Right);
                term.selection = Some(selection);
                break;
            }
        }
        drop(term);
        state.window.request_redraw();
    }
}
