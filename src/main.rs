//! Phase 5b: 페인 분할 (내장 멀티플렉서 2단계).
//!
//! 탭 하나는 페인들의 이진 분할 트리다. Cmd+D(좌우)/Cmd+Shift+D(상하)로 나누고,
//! Cmd+Option+화살표로 포커스를 옮기고, Cmd+W는 포커스된 페인을 닫는다.

mod ai;
mod layout;
mod mux;
mod renderer;
mod session;

use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as TermEvent, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::viewport_to_point;
use alacritty_terminal::term::TermMode;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

use layout::{Pane, PaneNode, Rect, SplitDir};
use session::AppEvent;

/// 더블/트리플 클릭 판정 간격.
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(400);
/// 휠 한 칸당 스크롤 줄 수.
const SCROLL_LINES_PER_TICK: f32 = 3.0;

struct Tab {
    root: PaneNode,
    /// 포커스된 페인 ID
    focused: usize,
}

struct State {
    window: Arc<Window>,
    renderer: renderer::Renderer,
    tabs: Vec<Tab>,
    active: usize,
}

impl State {
    fn active_tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn focused_pane(&self) -> &Pane {
        let tab = self.active_tab();
        tab.root.pane(tab.focused).expect("포커스된 페인 없음")
    }

    /// 탭 바 아래 콘텐츠 영역.
    fn content_rect(&self) -> Rect {
        let size = self.window.inner_size();
        let bar_h = self.renderer.tab_bar_height();
        Rect {
            x: 0.0,
            y: bar_h,
            w: size.width as f32,
            h: (size.height as f32 - bar_h).max(1.0),
        }
    }

    /// 활성 탭의 페인 배치를 계산한다.
    fn pane_rects(&self) -> Vec<(usize, Rect)> {
        let mut out = Vec::new();
        self.active_tab().root.layout(self.content_rect(), &mut out);
        out
    }

    /// 탭의 모든 페인 세션 크기를 현재 배치에 맞춘다.
    fn relayout_tab(&self, tab_index: usize) {
        let mut rects = Vec::new();
        self.tabs[tab_index].root.layout(self.content_rect(), &mut rects);
        for (id, rect) in rects {
            let (cols, lines) = self.renderer.pane_grid_size(rect);
            if let Some(pane) = self.tabs[tab_index].root.pane(id) {
                pane.session.resize(WindowSize {
                    num_cols: cols as u16,
                    num_lines: lines as u16,
                    cell_width: self.renderer.cell_width as u16,
                    cell_height: self.renderer.cell_height as u16,
                });
            }
        }
    }
}

/// AI 명령 생성 바의 상태.
enum AiState {
    Idle,
    /// 입력 중인 자연어
    Input(String),
    /// 생성 요청 진행 중
    Pending,
    Error(String),
}

struct App {
    proxy: EventLoopProxy<AppEvent>,
    state: Option<State>,
    modifiers: Modifiers,
    clipboard: Option<arboard::Clipboard>,
    next_pane_id: usize,
    ai: AiState,
    /// 취소된 요청의 늦은 응답을 무시하기 위한 시퀀스 번호
    ai_seq: u64,

    // 마우스 상태
    mouse_pos: PhysicalPosition<f64>,
    left_button_down: bool,
    last_click_at: Option<Instant>,
    last_click_point: Option<Point>,
    click_count: u32,
    scroll_accum: f32,

    // IME 조합 중 문자열 (포커스된 페인에 적용)
    preedit: Option<String>,
}

impl App {
    fn make_pane(&mut self, rect: Rect) -> Pane {
        let state = self.state.as_ref().unwrap();
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        let (cols, lines) = state.renderer.pane_grid_size(rect);
        let ws = WindowSize {
            num_cols: cols as u16,
            num_lines: lines as u16,
            cell_width: state.renderer.cell_width as u16,
            cell_height: state.renderer.cell_height as u16,
        };
        let session =
            session::Session::new(session::EventProxy::new(self.proxy.clone(), id), ws);
        Pane {
            id,
            session,
            title: "zsh".to_string(),
        }
    }

    fn new_tab(&mut self) {
        let Some(state) = &self.state else { return };
        let rect = state.content_rect();
        let pane = self.make_pane(rect);
        let id = pane.id;
        let state = self.state.as_mut().unwrap();
        state.tabs.push(Tab {
            root: PaneNode::Leaf(pane),
            focused: id,
        });
        state.active = state.tabs.len() - 1;
        self.preedit = None;
        state.window.request_redraw();
    }

    /// 데몬의 기존 세션 ID에 붙어 새 탭으로 복원한다.
    fn attach_tab(&mut self, session_id: u64) {
        let Some(state) = &self.state else { return };
        let rect = state.content_rect();
        let id = self.next_pane_id;
        let (cols, lines) = state.renderer.pane_grid_size(rect);
        let ws = WindowSize {
            num_cols: cols as u16,
            num_lines: lines as u16,
            cell_width: state.renderer.cell_width as u16,
            cell_height: state.renderer.cell_height as u16,
        };
        let Some(session) =
            session::Session::attach(session::EventProxy::new(self.proxy.clone(), id), session_id, ws)
        else {
            return; // 세션이 이미 사라졌으면 건너뛴다
        };
        self.next_pane_id += 1;
        let pane = Pane {
            id,
            session,
            title: "zsh".to_string(),
        };
        let state = self.state.as_mut().unwrap();
        state.tabs.push(Tab {
            root: PaneNode::Leaf(pane),
            focused: id,
        });
        state.window.request_redraw();
    }

    /// 포커스된 페인을 분할한다.
    fn split_pane(&mut self, dir: SplitDir) {
        let Some(state) = &self.state else { return };
        let target = state.active_tab().focused;
        let rect = state.content_rect();
        let pane = self.make_pane(rect); // 크기는 relayout에서 재조정
        let new_id = pane.id;

        let state = self.state.as_mut().unwrap();
        let active = state.active;
        if state.tabs[active].root.split_leaf(target, dir, pane).is_none() {
            state.tabs[active].focused = new_id;
            state.relayout_tab(active);
            self.preedit = None;
            state.window.request_redraw();
        }
    }

    /// 페인을 닫는다. 탭의 마지막 페인이면 탭을 닫고, 마지막 탭이면 종료한다.
    /// `kill`이 true면 데몬 세션(셸)도 종료한다 (Cmd+W). false면 로컬 정리만
    /// (셸이 이미 exit한 경우 — TermEvent::Exit).
    fn close_pane(&mut self, pane_id: usize, kill: bool, event_loop: &ActiveEventLoop) {
        let Some(state) = &mut self.state else { return };
        let Some(tab_index) = state
            .tabs
            .iter()
            .position(|t| t.root.pane(pane_id).is_some())
        else {
            return;
        };

        // 명시적 닫기: 데몬 세션 종료 (다음 실행에서 복원되지 않도록)
        if kill {
            if let Some(pane) = state.tabs[tab_index].root.pane(pane_id) {
                pane.session.kill();
            }
        }

        let is_last_pane = state.tabs[tab_index].root.panes().len() == 1;
        if is_last_pane {
            state.tabs.remove(tab_index);
            if state.tabs.is_empty() {
                event_loop.exit();
                return;
            }
            if state.active >= state.tabs.len() {
                state.active = state.tabs.len() - 1;
            }
        } else {
            state.tabs[tab_index].root.remove(pane_id);
            if state.tabs[tab_index].focused == pane_id {
                state.tabs[tab_index].focused =
                    state.tabs[tab_index].root.first_id().unwrap_or(0);
            }
            state.relayout_tab(tab_index);
        }
        self.preedit = None;
        state.window.request_redraw();
    }

    fn switch_tab(&mut self, index: usize) {
        let Some(state) = &mut self.state else { return };
        if index < state.tabs.len() && index != state.active {
            state.active = index;
            self.preedit = None;
            let title = state.focused_pane().title.clone();
            state.window.set_title(&title);
            state.window.request_redraw();
        }
    }

    /// 방향키로 포커스를 인접 페인으로 옮긴다.
    fn move_focus(&mut self, dx: f32, dy: f32) {
        let Some(state) = &mut self.state else { return };
        let rects = state.pane_rects();
        let focused = state.active_tab().focused;
        let Some(&(_, from)) = rects.iter().find(|(id, _)| *id == focused) else {
            return;
        };
        let (fx, fy) = from.center();

        let target = rects
            .iter()
            .filter(|(id, _)| *id != focused)
            .filter(|(_, r)| {
                let (cx, cy) = r.center();
                // 이동 방향의 반평면에 있는 페인만 후보
                (cx - fx) * dx + (cy - fy) * dy > 0.0
            })
            .min_by(|(_, a), (_, b)| {
                let da = {
                    let (cx, cy) = a.center();
                    (cx - fx).powi(2) + (cy - fy).powi(2)
                };
                let db = {
                    let (cx, cy) = b.center();
                    (cx - fx).powi(2) + (cy - fy).powi(2)
                };
                da.total_cmp(&db)
            })
            .map(|(id, _)| *id);

        if let Some(id) = target {
            let active = state.active;
            state.tabs[active].focused = id;
            self.preedit = None;
            state.window.request_redraw();
        }
    }

    /// 좌표에 있는 페인 ID와 사각형.
    fn pane_at(state: &State, pos: PhysicalPosition<f64>) -> Option<(usize, Rect)> {
        state
            .pane_rects()
            .into_iter()
            .find(|(_, rect)| rect.contains(pos.x, pos.y))
    }

    /// 마우스 물리 좌표 → 해당 페인의 그리드 좌표(스크롤백 반영)와 셀 내 좌/우 반쪽.
    fn grid_point(pane: &Pane, rect: Rect, state: &State, pos: PhysicalPosition<f64>) -> (Point, Side) {
        let cell_w = state.renderer.cell_width as f64;
        let cell_h = state.renderer.cell_height as f64;
        let origin_x = rect.x as f64 + 8.0;
        let origin_y = rect.y as f64 + 8.0;

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

    fn copy_selection(&mut self) {
        let Some(state) = &self.state else { return };
        let text = state.focused_pane().session.term.lock().selection_to_string();
        if let (Some(text), Some(clipboard)) = (text, self.clipboard.as_mut()) {
            if !text.is_empty() {
                let _ = clipboard.set_text(text);
            }
        }
    }

    fn paste(&mut self) {
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
    fn copy_last_output(&mut self) {
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
        if let Some(clipboard) = self.clipboard.as_mut() {
            if !text.is_empty() {
                let _ = clipboard.set_text(text);
            }
        }
    }

    /// 클릭한 위치가 속한 블록 전체를 선택한다 (Cmd+클릭).
    fn select_block_at(&mut self, pos: PhysicalPosition<f64>) {
        let Some(state) = &self.state else { return };
        let Some((pane_id, rect)) = Self::pane_at(state, pos) else {
            return;
        };
        let tab = state.active_tab();
        let Some(pane) = tab.root.pane(pane_id) else { return };
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
                let sel_end = block.end_abs.map(|d| d - 1).unwrap_or(span_end).max(block.start_abs);
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

    /// AI 입력 바의 자연어를 명령 생성 요청으로 보낸다.
    fn submit_ai(&mut self) {
        let AiState::Input(text) = &self.ai else {
            return;
        };
        let request = text.trim().to_string();
        if request.is_empty() {
            self.ai = AiState::Idle;
            return;
        }

        let state = self.state.as_ref().unwrap();
        let pane = state.focused_pane();
        let pane_id = pane.id;

        // 컨텍스트 수집: 최근 화면 텍스트 + 마지막 종료 코드
        let screen_tail = {
            let term = pane.session.term.lock();
            let grid = term.grid();
            let history = grid.history_size() as i32;
            let cursor_line = grid.cursor.point.line.0;
            let cols = grid.columns();
            let start_line = (cursor_line - 30).max(-history);
            let text = term.bounds_to_string(
                Point::new(Line(start_line), Column(0)),
                Point::new(Line(cursor_line), Column(cols - 1)),
            );
            // 너무 길면 끝부분만
            let max = 4000;
            if text.len() > max {
                let cut = text.len() - max;
                let boundary = (cut..text.len())
                    .find(|i| text.is_char_boundary(*i))
                    .unwrap_or(text.len());
                text[boundary..].to_string()
            } else {
                text
            }
        };
        let last_exit = pane
            .session
            .blocks()
            .iter()
            .rev()
            .find_map(|block| block.exit);

        self.ai_seq += 1;
        let seq = self.ai_seq;
        self.ai = AiState::Pending;
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let context = ai::AiContext {
                screen_tail,
                last_exit,
            };
            let result = ai::generate_command(&request, &context);
            let _ = proxy.send_event(AppEvent::AiResult {
                pane_id,
                seq,
                result,
            });
        });
        state.window.request_redraw();
    }

    /// 입력이 발생하면 선택을 해제하고 화면을 맨 아래로 되돌린다.
    fn on_user_input(pane: &Pane) {
        let mut term = pane.session.term.lock();
        term.selection = None;
        term.scroll_display(Scroll::Bottom);
    }

    fn redraw(&mut self) {
        let Some(state) = &mut self.state else { return };
        // renderer(가변)와 tabs(불변)를 동시에 빌리기 위해 필드를 분리 차용한다.
        let State {
            window,
            renderer,
            tabs,
            active,
        } = state;
        let content = {
            let size = window.inner_size();
            let bar = renderer.tab_bar_height();
            Rect {
                x: 0.0,
                y: bar,
                w: size.width as f32,
                h: (size.height as f32 - bar).max(1.0),
            }
        };
        let tab = &tabs[*active];
        let focused = tab.focused;
        let mut rects = Vec::new();
        tab.root.layout(content, &mut rects);

        // 페인별 블록을 미리 수집 (draw 중 marks 잠금을 피하기 위해)
        let block_lists: Vec<_> = rects
            .iter()
            .map(|(id, _)| {
                tab.root
                    .pane(*id)
                    .map(|p| p.session.blocks())
                    .unwrap_or_default()
            })
            .collect();

        let views: Vec<renderer::PaneView> = rects
            .iter()
            .zip(block_lists.iter())
            .filter_map(|((id, rect), blocks)| {
                tab.root.pane(*id).map(|pane| renderer::PaneView {
                    term: &pane.session.term,
                    blocks,
                    rect: *rect,
                    focused: *id == focused,
                })
            })
            .collect();

        let titles: Vec<String> = tabs
            .iter()
            .map(|t| {
                t.root
                    .pane(t.focused)
                    .map(|p| p.title.clone())
                    .unwrap_or_else(|| "zsh".to_string())
            })
            .collect();

        // AI 바 표시 문자열 (입력 중이면 preedit도 함께 보여준다)
        let ai_line = match &self.ai {
            AiState::Idle => None,
            AiState::Input(text) => Some(format!(
                "AI> {}{}_   (Enter 생성 / Esc 닫기)",
                text,
                self.preedit.as_deref().unwrap_or("")
            )),
            AiState::Pending => Some("AI> 명령 생성 중...".to_string()),
            AiState::Error(error) => Some(format!("AI 오류: {error}   (Esc 닫기)")),
        };
        // AI 바가 열려 있으면 preedit은 바에서 렌더링하므로 페인에는 넘기지 않는다
        let pane_preedit = if matches!(self.ai, AiState::Idle) {
            self.preedit.as_deref()
        } else {
            None
        };

        let ime_pos = renderer.draw(&views, pane_preedit, &titles, *active, ai_line.as_deref());
        drop(views);

        // IME 후보창을 커서 바로 아래에 배치
        if let Some((x, y)) = ime_pos {
            window.set_ime_cursor_area(
                PhysicalPosition::new(x, y),
                PhysicalSize::new(renderer.cell_width as f64, renderer.cell_height as f64),
            );
        }
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("terminal")
            .with_inner_size(LogicalSize::new(960.0, 640.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("창 생성 실패"));
        window.set_ime_allowed(true);

        let renderer = renderer::Renderer::new(Arc::clone(&window));
        self.clipboard = arboard::Clipboard::new().ok();
        self.state = Some(State {
            window,
            renderer,
            tabs: Vec::new(),
            active: 0,
        });

        // 데몬을 확인하고, 살아있는 세션이 있으면 탭으로 복원한다 (detach 후 attach).
        let _ = mux::ensure_daemon();
        let surviving = session::Session::list();
        if surviving.is_empty() {
            self.new_tab();
        } else {
            for id in surviving {
                self.attach_tab(id);
            }
            let empty = self.state.as_ref().unwrap().tabs.is_empty();
            if empty {
                // 모든 세션이 attach 직전에 사라졌으면 새로 만든다
                self.new_tab();
            } else {
                self.state.as_mut().unwrap().active = 0;
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, app_event: AppEvent) {
        if self.state.is_none() {
            return;
        }
        // AI 생성 결과: 명령을 해당 페인의 입력줄에 삽입한다 (실행하지 않음)
        if let AppEvent::AiResult {
            pane_id,
            seq,
            result,
        } = app_event
        {
            if seq != self.ai_seq || !matches!(self.ai, AiState::Pending) {
                return; // 취소되었거나 오래된 응답
            }
            let state = self.state.as_ref().unwrap();
            match result {
                Ok(command) => {
                    self.ai = AiState::Idle;
                    let pane = state
                        .tabs
                        .iter()
                        .find_map(|tab| tab.root.pane(pane_id))
                        .or_else(|| Some(state.focused_pane()));
                    if let Some(pane) = pane {
                        // 여러 줄 명령은 개행이 실행으로 이어지지 않게 정리
                        pane.session
                            .write(command.replace('\n', " ").into_bytes());
                    }
                }
                Err(error) => self.ai = AiState::Error(error),
            }
            state.window.request_redraw();
            return;
        }

        let AppEvent::Term(pane_id, event) = app_event else {
            return;
        };
        let state = self.state.as_mut().unwrap();
        let Some(tab_index) = state
            .tabs
            .iter()
            .position(|t| t.root.pane(pane_id).is_some())
        else {
            return;
        };
        match event {
            TermEvent::Wakeup => {
                if tab_index == state.active {
                    state.window.request_redraw();
                }
            }
            // 터미널이 앱의 질의(커서 위치 등)에 응답할 때 — 반드시 PTY로 되돌려준다.
            TermEvent::PtyWrite(text) => {
                if let Some(pane) = state.tabs[tab_index].root.pane(pane_id) {
                    pane.session.write(text.into_bytes());
                }
            }
            TermEvent::Title(title) => {
                if let Some(pane) = state.tabs[tab_index].root.pane_mut(pane_id) {
                    pane.title = title.clone();
                }
                if tab_index == state.active && state.tabs[tab_index].focused == pane_id {
                    state.window.set_title(&title);
                }
                state.window.request_redraw();
            }
            // OSC 52: 앱이 클립보드에 쓰기를 요청
            TermEvent::ClipboardStore(_, text) => {
                if let Some(clipboard) = self.clipboard.as_mut() {
                    let _ = clipboard.set_text(text);
                }
            }
            // OSC 52: 앱이 클립보드 읽기를 요청
            TermEvent::ClipboardLoad(_, formatter) => {
                let text = self
                    .clipboard
                    .as_mut()
                    .and_then(|c| c.get_text().ok())
                    .unwrap_or_default();
                if let Some(pane) = state.tabs[tab_index].root.pane(pane_id) {
                    pane.session.write(formatter(&text).into_bytes());
                }
            }
            // 셸 종료 → 해당 페인 닫기
            TermEvent::Exit => self.close_pane(pane_id, false, event_loop),
            _ => {}
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.state.is_none() {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers,
            WindowEvent::Resized(size) => {
                let state = self.state.as_mut().unwrap();
                state.renderer.resize(size.width, size.height);
                for i in 0..state.tabs.len() {
                    state.relayout_tab(i);
                }
                state.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                // IME 조합 중에는 키 이벤트를 무시한다 (조합 결과는 Ime::Commit으로 온다).
                if self.preedit.is_some() {
                    return;
                }
                let mods = self.modifiers.state();

                // AI 입력 바 활성 중: 키 입력을 바로 가로챈다
                if !matches!(self.ai, AiState::Idle) {
                    match &event.logical_key {
                        Key::Named(NamedKey::Escape) => {
                            self.ai = AiState::Idle;
                            self.ai_seq += 1; // 진행 중이던 요청 응답 무시
                        }
                        Key::Named(NamedKey::Enter) => self.submit_ai(),
                        Key::Named(NamedKey::Backspace) => {
                            if let AiState::Input(text) = &mut self.ai {
                                text.pop();
                            }
                        }
                        _ => {
                            if !mods.control_key() && !mods.super_key() {
                                if let (AiState::Input(buffer), Some(text)) =
                                    (&mut self.ai, &event.text)
                                {
                                    buffer.push_str(text);
                                }
                            }
                        }
                    }
                    self.state.as_ref().unwrap().window.request_redraw();
                    return;
                }

                // 앱 단축키 (Cmd 조합)
                if mods.super_key() {
                    // Cmd+Option+화살표: 페인 포커스 이동
                    if mods.alt_key() {
                        match event.logical_key.as_ref() {
                            Key::Named(NamedKey::ArrowLeft) => self.move_focus(-1.0, 0.0),
                            Key::Named(NamedKey::ArrowRight) => self.move_focus(1.0, 0.0),
                            Key::Named(NamedKey::ArrowUp) => self.move_focus(0.0, -1.0),
                            Key::Named(NamedKey::ArrowDown) => self.move_focus(0.0, 1.0),
                            _ => {}
                        }
                        return;
                    }
                    match event.logical_key.as_ref() {
                        // AI 명령 생성 바
                        Key::Character("k") => {
                            self.ai = AiState::Input(String::new());
                            self.state.as_ref().unwrap().window.request_redraw();
                        }
                        // 탭
                        Key::Character("t") => self.new_tab(),
                        Key::Character("w") => {
                            let focused = self.state.as_ref().unwrap().active_tab().focused;
                            self.close_pane(focused, true, event_loop);
                        }
                        Key::Character(digit)
                            if digit.len() == 1
                                && digit.chars().next().unwrap().is_ascii_digit() =>
                        {
                            let n = digit.chars().next().unwrap() as usize - '0' as usize;
                            if n >= 1 {
                                self.switch_tab(n - 1);
                            }
                        }
                        Key::Character("}") => {
                            let state = self.state.as_ref().unwrap();
                            let next = (state.active + 1) % state.tabs.len();
                            self.switch_tab(next);
                        }
                        Key::Character("{") => {
                            let state = self.state.as_ref().unwrap();
                            let prev = (state.active + state.tabs.len() - 1) % state.tabs.len();
                            self.switch_tab(prev);
                        }
                        // 분할
                        Key::Character("d") if mods.shift_key() => {
                            self.split_pane(SplitDir::Column)
                        }
                        Key::Character("D") => self.split_pane(SplitDir::Column),
                        Key::Character("d") => self.split_pane(SplitDir::Row),
                        // 복사/붙여넣기
                        Key::Character("c") if mods.shift_key() => self.copy_last_output(),
                        Key::Character("C") => self.copy_last_output(),
                        Key::Character("c") => self.copy_selection(),
                        Key::Character("v") => self.paste(),
                        // OSC 133 마크 기반 프롬프트 점프
                        Key::Named(NamedKey::ArrowUp) => {
                            let state = self.state.as_ref().unwrap();
                            state.focused_pane().session.jump_to_prompt(-1);
                            state.window.request_redraw();
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            let state = self.state.as_ref().unwrap();
                            state.focused_pane().session.jump_to_prompt(1);
                            state.window.request_redraw();
                        }
                        _ => {}
                    }
                    return;
                }

                if let Some(bytes) = key_to_bytes(&event, mods) {
                    let state = self.state.as_ref().unwrap();
                    let pane = state.focused_pane();
                    Self::on_user_input(pane);
                    pane.session.write(bytes);
                    state.window.request_redraw();
                }
            }
            WindowEvent::Ime(ime) => {
                let state = self.state.as_ref().unwrap();
                match ime {
                    Ime::Preedit(text, _) => {
                        self.preedit = if text.is_empty() { None } else { Some(text) };
                    }
                    Ime::Commit(text) => {
                        self.preedit = None;
                        // AI 입력 바가 열려 있으면 한글 확정 입력도 그쪽으로
                        if let AiState::Input(buffer) = &mut self.ai {
                            buffer.push_str(&text);
                        } else {
                            let pane = state.focused_pane();
                            Self::on_user_input(pane);
                            pane.session.write(text.into_bytes());
                        }
                    }
                    Ime::Enabled | Ime::Disabled => {}
                }
                state.window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_pos = position;
                if self.left_button_down {
                    let state = self.state.as_ref().unwrap();
                    let pane = state.focused_pane();
                    let rects = state.pane_rects();
                    let Some(&(_, rect)) =
                        rects.iter().find(|(id, _)| *id == pane.id)
                    else {
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
            }
            WindowEvent::MouseInput { state: button_state, button, .. } => {
                if button != MouseButton::Left {
                    return;
                }
                // 탭 바 클릭 → 탭 전환
                if button_state == ElementState::Pressed {
                    let state = self.state.as_ref().unwrap();
                    let hit = state.renderer.tab_hit(
                        self.mouse_pos.x,
                        self.mouse_pos.y,
                        state.tabs.len(),
                    );
                    if let Some(index) = hit {
                        self.switch_tab(index);
                        return;
                    }
                }
                // 페인 클릭 → 포커스 이동
                if button_state == ElementState::Pressed {
                    let state = self.state.as_mut().unwrap();
                    if let Some((pane_id, _)) = Self::pane_at(state, self.mouse_pos) {
                        let active = state.active;
                        if state.tabs[active].focused != pane_id {
                            state.tabs[active].focused = pane_id;
                            self.preedit = None;
                        }
                    }
                }
                // Cmd+클릭: 블록 전체 선택
                if button_state == ElementState::Pressed && self.modifiers.state().super_key() {
                    self.select_block_at(self.mouse_pos);
                    self.left_button_down = false;
                    return;
                }
                let state = self.state.as_ref().unwrap();
                match button_state {
                    ElementState::Pressed => {
                        let pane = state.focused_pane();
                        let rects = state.pane_rects();
                        let Some(&(_, rect)) =
                            rects.iter().find(|(id, _)| *id == pane.id)
                        else {
                            return;
                        };
                        self.left_button_down = true;
                        let (point, side) =
                            Self::grid_point(pane, rect, state, self.mouse_pos);

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
            WindowEvent::MouseWheel { delta, .. } => {
                let state = self.state.as_ref().unwrap();
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => {
                        self.scroll_accum = 0.0;
                        y * SCROLL_LINES_PER_TICK
                    }
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
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            _ => {}
        }
    }
}

/// 키 입력을 PTY로 보낼 바이트 시퀀스로 변환한다.
fn key_to_bytes(event: &KeyEvent, mods: ModifiersState) -> Option<Vec<u8>> {
    if let Key::Named(named) = &event.logical_key {
        let seq: Option<&[u8]> = match named {
            NamedKey::Enter => Some(b"\r"),
            NamedKey::Backspace => Some(b"\x7f"),
            NamedKey::Tab => Some(b"\t"),
            NamedKey::Escape => Some(b"\x1b"),
            NamedKey::ArrowUp => Some(b"\x1b[A"),
            NamedKey::ArrowDown => Some(b"\x1b[B"),
            NamedKey::ArrowRight => Some(b"\x1b[C"),
            NamedKey::ArrowLeft => Some(b"\x1b[D"),
            NamedKey::Home => Some(b"\x1b[H"),
            NamedKey::End => Some(b"\x1b[F"),
            NamedKey::PageUp => Some(b"\x1b[5~"),
            NamedKey::PageDown => Some(b"\x1b[6~"),
            NamedKey::Delete => Some(b"\x1b[3~"),
            NamedKey::Space => {
                if mods.control_key() {
                    return Some(vec![0]);
                }
                Some(b" ")
            }
            _ => None,
        };
        if let Some(seq) = seq {
            return Some(seq.to_vec());
        }
    }

    // Ctrl+A..Z → C0 제어 문자
    if mods.control_key() {
        if let Key::Character(s) = &event.logical_key {
            let c = s.chars().next()?.to_ascii_lowercase();
            if c.is_ascii_lowercase() {
                return Some(vec![c as u8 - b'a' + 1]);
            }
        }
    }

    event
        .text
        .as_ref()
        .filter(|t| !t.is_empty())
        .map(|t| t.as_bytes().to_vec())
}

fn main() {
    // `--daemon`: mux 데몬으로 실행 (세션/PTY 소유, GUI와 독립적으로 생존)
    if std::env::args().any(|a| a == "--daemon") {
        mux::run_daemon();
    }

    let event_loop = EventLoop::<AppEvent>::with_user_event()
        .build()
        .expect("이벤트 루프 생성 실패");
    let proxy = event_loop.create_proxy();
    let mut app = App {
        proxy,
        state: None,
        modifiers: Modifiers::default(),
        clipboard: None,
        next_pane_id: 0,
        ai: AiState::Idle,
        ai_seq: 0,
        mouse_pos: PhysicalPosition::new(0.0, 0.0),
        left_button_down: false,
        last_click_at: None,
        last_click_point: None,
        click_count: 0,
        scroll_accum: 0.0,
        preedit: None,
    };
    event_loop.run_app(&mut app).expect("이벤트 루프 실행 실패");
}
