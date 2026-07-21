//! 마우스: 포커스 이동, 드래그 선택, 더블/트리플 클릭, 휠 스크롤,
//! Cmd+클릭(하이퍼링크 / 블록 선택), 그리고 TTY 앱으로의 마우스 리포팅.
//!
//! 리포트 바이트를 만드는 순수 로직은 [`super::mouse_report`]에 있다.
//! 여기서는 winit 이벤트 해석과 `term` 락 관리만 한다.

use std::time::{Duration, Instant};

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::viewport_to_point;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta};

use super::mouse_report::{self, ReportKind};
use super::{App, State};
use crate::layout::{Pane, Rect};
use crate::renderer::PADDING;

/// 더블/트리플 클릭 판정 간격.
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(400);
/// 휠 한 칸당 스크롤 줄 수.
const SCROLL_LINES_PER_TICK: f32 = 3.0;

/// 클릭 지점의 그리드 좌표와 뷰포트 좌표.
///
/// 선택·블록·하이퍼링크는 스크롤백을 반영한 `point`를 쓰고,
/// 마우스 리포팅은 화면 기준인 `col`/`row`를 쓴다.
#[derive(Clone, Copy)]
struct CellHit {
    point: Point,
    side: Side,
    col: usize,
    row: usize,
}

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

    /// 마우스 물리 좌표 → 해당 페인의 셀 좌표.
    fn grid_point(pane: &Pane, rect: Rect, state: &State, pos: PhysicalPosition<f64>) -> CellHit {
        let cell_w = state.renderer.cell_width as f64;
        let cell_h = state.renderer.cell_height as f64;
        let origin_x = rect.x as f64 + PADDING as f64;
        let origin_y = rect.y as f64 + PADDING as f64;

        let term = pane.session.term.lock();
        let grid = term.grid();
        let cols = grid.columns();
        let lines = grid.screen_lines();
        let display_offset = grid.display_offset();
        drop(term);

        let (col, row, side) = mouse_report::cell_at(
            pos.x, pos.y, origin_x, origin_y, cell_w, cell_h, cols, lines,
        );
        let point = viewport_to_point(display_offset, Point::new(row, Column(col)));

        CellHit {
            point,
            side,
            col,
            row,
        }
    }

    /// 마우스 리포팅이 켜져 있고 사용자가 로컬 동작을 요구하지 않았는지.
    ///
    /// Shift는 표준 탈출구다 — htop 같은 전체화면 앱에서 텍스트를 선택할
    /// 유일한 수단이므로 Shift가 눌려 있으면 리포팅하지 않는다.
    /// Cmd는 하이퍼링크·블록 선택에 이미 쓰이므로 마찬가지로 제외한다.
    fn should_report(&self, pane: &Pane) -> Option<TermMode> {
        let mods = self.modifiers.state();
        if mods.shift_key() || mods.super_key() {
            return None;
        }
        // 모드만 복사하고 락은 즉시 놓는다 — 리더 스레드가 같은 락을 다툰다.
        let mode = {
            let term = pane.session.term.lock();
            *term.mode()
        };
        mode.intersects(TermMode::MOUSE_MODE).then_some(mode)
    }

    /// 리포트 바이트를 만들어 PTY로 보낸다. `term` 락은 이미 풀린 상태여야 한다.
    fn send_report(
        &self,
        pane: &Pane,
        mode: TermMode,
        button: Option<MouseButton>,
        kind: ReportKind,
        hit: CellHit,
    ) {
        let Some(code) = mouse_report::button_code(button, kind, self.modifiers.state()) else {
            return;
        };
        let press = kind != ReportKind::Release;
        if let Some(bytes) = mouse_report::encode(mode, code, hit.col, hit.row, press) {
            pane.session.write(bytes);
        }
    }

    pub(super) fn on_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        self.mouse_pos = position;

        let state = self.state.as_ref().unwrap();
        let Some((pane, rect)) = Self::focused_pane_rect(state) else {
            return;
        };

        // 마우스 리포팅이 켜져 있으면 이동도 앱으로 보낸다.
        // 1003(MOUSE_MOTION)은 버튼과 무관하게, 1002(MOUSE_DRAG)는 누른 동안만.
        if let Some(mode) = self.should_report(pane) {
            let motion = if mode.contains(TermMode::MOUSE_MOTION) {
                true
            } else if mode.contains(TermMode::MOUSE_DRAG) {
                self.held_button.is_some()
            } else {
                false
            };
            if motion {
                let hit = Self::grid_point(pane, rect, state, position);
                // 같은 셀 안의 픽셀 이동은 앱에 의미가 없다 — 보내면 소켓만 채운다.
                if self.last_report_cell != Some((hit.col, hit.row)) {
                    self.last_report_cell = Some((hit.col, hit.row));
                    let button = self.held_button;
                    self.send_report(pane, mode, button, ReportKind::Motion, hit);
                }
                return;
            }
            // 리포팅 모드지만 이 이동은 보고 대상이 아니다 (예: 1000 = 클릭만).
            return;
        }

        if !self.left_button_down {
            return;
        }
        let hit = Self::grid_point(pane, rect, state, position);
        let mut term = pane.session.term.lock();
        if let Some(selection) = term.selection.as_mut() {
            selection.update(hit.point, hit.side);
        }
        drop(term);
        state.window.request_redraw();
    }

    pub(super) fn on_mouse_input(&mut self, button_state: ElementState, button: MouseButton) {
        if button_state == ElementState::Pressed {
            // 탭 바 클릭 → 탭 전환 (좌클릭만)
            if button == MouseButton::Left {
                let state = self.state.as_ref().unwrap();
                let hit =
                    state
                        .renderer
                        .tab_hit(self.mouse_pos.x, self.mouse_pos.y, state.tabs.len());
                if let Some(index) = hit {
                    self.switch_tab(index);
                    return;
                }

                // 페인 클릭 → 포커스 이동.
                // 리포팅보다 먼저 해야 비포커스 페인 클릭이 엉뚱한 세션으로 가지 않는다.
                let state = self.state.as_mut().unwrap();
                if let Some((pane_id, _)) = Self::pane_at(state, self.mouse_pos) {
                    let active = state.active;
                    if state.tabs[active].focused != pane_id {
                        state.tabs[active].focused = pane_id;
                        self.preedit = None;
                        self.last_report_cell = None;
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
        }

        let state = self.state.as_ref().unwrap();
        let Some((pane, rect)) = Self::focused_pane_rect(state) else {
            return;
        };

        // 마우스 리포팅이 켜져 있으면 선택 대신 앱으로 보낸다.
        if let Some(mode) = self.should_report(pane) {
            let hit = Self::grid_point(pane, rect, state, self.mouse_pos);
            let kind = match button_state {
                ElementState::Pressed => ReportKind::Press,
                ElementState::Released => ReportKind::Release,
            };
            self.send_report(pane, mode, Some(button), kind, hit);
            self.held_button = match button_state {
                ElementState::Pressed => Some(button),
                ElementState::Released => None,
            };
            if button_state == ElementState::Released {
                self.last_report_cell = None;
            }
            return;
        }

        // 아래 로컬 동작(선택)은 좌클릭에만 해당한다.
        if button != MouseButton::Left {
            return;
        }

        match button_state {
            ElementState::Pressed => {
                self.left_button_down = true;
                let hit = Self::grid_point(pane, rect, state, self.mouse_pos);

                // 더블/트리플 클릭 판정
                let now = Instant::now();
                let is_multi = self
                    .last_click_at
                    .is_some_and(|at| now - at < MULTI_CLICK_INTERVAL)
                    && self.last_click_point == Some(hit.point);
                self.click_count = if is_multi { self.click_count + 1 } else { 1 };
                self.last_click_at = Some(now);
                self.last_click_point = Some(hit.point);

                let ty = match self.click_count {
                    1 => SelectionType::Simple,
                    2 => SelectionType::Semantic,
                    _ => SelectionType::Lines,
                };
                let mut term = pane.session.term.lock();
                term.selection = Some(Selection::new(ty, hit.point, hit.side));
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

        // 마우스가 올라가 있는 페인을 스크롤 (없으면 포커스된 페인).
        // 리포팅에는 페인 사각형도 필요하므로 함께 해석한다.
        let tab = state.active_tab();
        let Some((pane, rect)) = Self::pane_at(state, self.mouse_pos)
            .and_then(|(id, rect)| tab.root.pane(id).map(|pane| (pane, rect)))
            .or_else(|| Self::focused_pane_rect(state))
        else {
            return;
        };
        let ticks = lines.abs() as usize;

        // 1) 마우스 리포팅이 켜져 있으면 휠 버튼(64/65)으로 보고한다.
        if let Some(mode) = self.should_report(pane) {
            let hit = Self::grid_point(pane, rect, state, self.mouse_pos);
            let kind = if lines > 0.0 {
                ReportKind::WheelUp
            } else {
                ReportKind::WheelDown
            };
            for _ in 0..ticks {
                self.send_report(pane, mode, None, kind, hit);
            }
            state.window.request_redraw();
            return;
        }

        let mode = {
            let term = pane.session.term.lock();
            *term.mode()
        };

        // 2) 대체 스크린(less, vim 등)에는 히스토리가 없으므로 화살표로 변환한다.
        //    ALTERNATE_SCROLL이 꺼져 있으면 앱이 이 변환을 원치 않는다는 뜻이다.
        if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            // 커서 키 모드(DECCKM)에서는 CSI가 아니라 SS3 형식을 기대한다.
            let app_cursor = mode.contains(TermMode::APP_CURSOR);
            let seq: &[u8] = match (lines > 0.0, app_cursor) {
                (true, false) => b"\x1b[A",
                (false, false) => b"\x1b[B",
                (true, true) => b"\x1bOA",
                (false, true) => b"\x1bOB",
            };
            let mut bytes = Vec::with_capacity(seq.len() * ticks);
            for _ in 0..ticks {
                bytes.extend_from_slice(seq);
            }
            pane.session.write(bytes);
        } else {
            // 3) 그 외에는 로컬 스크롤백 스크롤.
            let mut term = pane.session.term.lock();
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
        let point = Self::grid_point(pane, rect, state, pos).point;
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
        let point = Self::grid_point(pane, rect, state, pos).point;
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
