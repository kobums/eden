//! Phase 5: 탭 (내장 멀티플렉서 1단계).
//!
//! 탭마다 독립된 세션(PTY + 그리드 + 블록)을 가지며,
//! Cmd+T/W/1-9/Shift+[] 로 탭을 만들고 오가고 닫는다.

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

use session::TabEvent;

/// 더블/트리플 클릭 판정 간격.
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(400);
/// 휠 한 칸당 스크롤 줄 수.
const SCROLL_LINES_PER_TICK: f32 = 3.0;

struct Tab {
    id: usize,
    session: session::Session,
    title: String,
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

    fn window_size(&self) -> WindowSize {
        let size = self.window.inner_size();
        let (cols, lines) = self.renderer.grid_size(size.width, size.height);
        WindowSize {
            num_cols: cols as u16,
            num_lines: lines as u16,
            cell_width: self.renderer.cell_width as u16,
            cell_height: self.renderer.cell_height as u16,
        }
    }
}

struct App {
    proxy: EventLoopProxy<TabEvent>,
    state: Option<State>,
    modifiers: Modifiers,
    clipboard: Option<arboard::Clipboard>,
    next_tab_id: usize,

    // 마우스 상태
    mouse_pos: PhysicalPosition<f64>,
    left_button_down: bool,
    last_click_at: Option<Instant>,
    last_click_point: Option<Point>,
    click_count: u32,
    scroll_accum: f32,

    // IME 조합 중 문자열 (활성 탭에 적용)
    preedit: Option<String>,
}

impl App {
    fn new_tab(&mut self) {
        let Some(state) = &mut self.state else { return };
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let proxy = session::EventProxy::new(self.proxy.clone(), id);
        let session = session::Session::new(proxy, state.window_size());
        state.tabs.push(Tab {
            id,
            session,
            title: "zsh".to_string(),
        });
        state.active = state.tabs.len() - 1;
        self.preedit = None;
        state.window.request_redraw();
    }

    fn close_tab(&mut self, index: usize, event_loop: &ActiveEventLoop) {
        let Some(state) = &mut self.state else { return };
        if index >= state.tabs.len() {
            return;
        }
        state.tabs.remove(index);
        if state.tabs.is_empty() {
            event_loop.exit();
            return;
        }
        if state.active >= state.tabs.len() {
            state.active = state.tabs.len() - 1;
        }
        self.preedit = None;
        state.window.request_redraw();
    }

    fn switch_tab(&mut self, index: usize) {
        let Some(state) = &mut self.state else { return };
        if index < state.tabs.len() && index != state.active {
            state.active = index;
            self.preedit = None;
            let title = state.tabs[index].title.clone();
            state.window.set_title(&title);
            state.window.request_redraw();
        }
    }

    /// 마우스 물리 좌표 → 그리드 좌표(스크롤백 반영)와 셀 내 좌/우 반쪽.
    fn grid_point(state: &State, pos: PhysicalPosition<f64>) -> (Point, Side) {
        let cell_w = state.renderer.cell_width as f64;
        let cell_h = state.renderer.cell_height as f64;
        let pad_x = 8.0;
        let origin_y = state.renderer.content_origin_y() as f64;

        let tab = state.active_tab();
        let term = tab.session.term.lock();
        let grid = term.grid();
        let cols = grid.columns();
        let lines = grid.screen_lines();
        let display_offset = grid.display_offset();
        drop(term);

        let col = (((pos.x - pad_x) / cell_w).floor().max(0.0) as usize).min(cols - 1);
        let line = (((pos.y - origin_y) / cell_h).floor().max(0.0) as usize).min(lines - 1);
        let point = viewport_to_point(display_offset, Point::new(line, Column(col)));

        let in_cell_x = (pos.x - pad_x) - col as f64 * cell_w;
        let side = if in_cell_x < cell_w / 2.0 {
            Side::Left
        } else {
            Side::Right
        };
        (point, side)
    }

    fn copy_selection(&mut self) {
        let Some(state) = &self.state else { return };
        let text = state.active_tab().session.term.lock().selection_to_string();
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
        let tab = state.active_tab();
        let bracketed = tab
            .session
            .term
            .lock()
            .mode()
            .contains(TermMode::BRACKETED_PASTE);
        if bracketed {
            let mut bytes = b"\x1b[200~".to_vec();
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            tab.session.write(bytes);
        } else {
            // 개행이 실행으로 이어지는 사고를 줄이기 위해 \n → \r 정규화
            tab.session.write(text.replace('\n', "\r").into_bytes());
        }
    }

    /// 마지막으로 완료된 명령의 출력을 클립보드로 복사한다 (Cmd+Shift+C).
    fn copy_last_output(&mut self) {
        let Some(state) = &self.state else { return };
        let tab = state.active_tab();
        let Some((start_abs, end_abs)) = tab.session.last_output_range() else {
            return;
        };
        let text = {
            let term = tab.session.term.lock();
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
        let (point, _) = Self::grid_point(state, pos);
        let tab = state.active_tab();
        let blocks = tab.session.blocks();

        let mut term = tab.session.term.lock();
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

    /// 입력이 발생하면 선택을 해제하고 화면을 맨 아래로 되돌린다.
    fn on_user_input(tab: &Tab) {
        let mut term = tab.session.term.lock();
        term.selection = None;
        term.scroll_display(Scroll::Bottom);
    }

    fn redraw(&mut self) {
        let Some(state) = &mut self.state else { return };
        let tab = &state.tabs[state.active];
        let blocks = tab.session.blocks();
        let titles: Vec<String> = state.tabs.iter().map(|t| t.title.clone()).collect();
        let active = state.active;
        let tab = &state.tabs[active];
        let ime_pos = state.renderer.draw(
            &tab.session.term,
            self.preedit.as_deref(),
            &blocks,
            &titles,
            active,
        );
        // IME 후보창을 커서 바로 아래에 배치
        if let Some((x, y)) = ime_pos {
            state.window.set_ime_cursor_area(
                PhysicalPosition::new(x, y),
                PhysicalSize::new(
                    state.renderer.cell_width as f64,
                    state.renderer.cell_height as f64,
                ),
            );
        }
    }
}

impl ApplicationHandler<TabEvent> for App {
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
        self.new_tab();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, (tab_id, event): TabEvent) {
        let Some(state) = &mut self.state else {
            return;
        };
        let Some(index) = state.tabs.iter().position(|t| t.id == tab_id) else {
            return;
        };
        match event {
            TermEvent::Wakeup => {
                if index == state.active {
                    state.window.request_redraw();
                }
            }
            // 터미널이 앱의 질의(커서 위치 등)에 응답할 때 — 반드시 PTY로 되돌려준다.
            TermEvent::PtyWrite(text) => state.tabs[index].session.write(text.into_bytes()),
            TermEvent::Title(title) => {
                state.tabs[index].title = title.clone();
                if index == state.active {
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
                state.tabs[index].session.write(formatter(&text).into_bytes());
            }
            // 셸 종료 → 해당 탭 닫기
            TermEvent::Exit => self.close_tab(index, event_loop),
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
                let ws = state.window_size();
                for tab in &state.tabs {
                    tab.session.resize(ws);
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

                // 앱 단축키 (Cmd 조합)
                if mods.super_key() {
                    match event.logical_key.as_ref() {
                        // 탭
                        Key::Character("t") => self.new_tab(),
                        Key::Character("w") => {
                            let active = self.state.as_ref().unwrap().active;
                            self.close_tab(active, event_loop);
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
                        // 복사/붙여넣기
                        Key::Character("c") if mods.shift_key() => self.copy_last_output(),
                        Key::Character("C") => self.copy_last_output(),
                        Key::Character("c") => self.copy_selection(),
                        Key::Character("v") => self.paste(),
                        // OSC 133 마크 기반 프롬프트 점프
                        Key::Named(NamedKey::ArrowUp) => {
                            let state = self.state.as_ref().unwrap();
                            state.active_tab().session.jump_to_prompt(-1);
                            state.window.request_redraw();
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            let state = self.state.as_ref().unwrap();
                            state.active_tab().session.jump_to_prompt(1);
                            state.window.request_redraw();
                        }
                        _ => {}
                    }
                    return;
                }

                if let Some(bytes) = key_to_bytes(&event, mods) {
                    let state = self.state.as_ref().unwrap();
                    let tab = state.active_tab();
                    Self::on_user_input(tab);
                    tab.session.write(bytes);
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
                        let tab = state.active_tab();
                        Self::on_user_input(tab);
                        tab.session.write(text.into_bytes());
                    }
                    Ime::Enabled | Ime::Disabled => {}
                }
                state.window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_pos = position;
                if self.left_button_down {
                    let state = self.state.as_ref().unwrap();
                    let (point, side) = Self::grid_point(state, position);
                    let mut term = state.active_tab().session.term.lock();
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
                // Cmd+클릭: 블록 전체 선택
                if button_state == ElementState::Pressed && self.modifiers.state().super_key() {
                    self.select_block_at(self.mouse_pos);
                    self.left_button_down = false;
                    return;
                }
                let state = self.state.as_ref().unwrap();
                match button_state {
                    ElementState::Pressed => {
                        self.left_button_down = true;
                        let (point, side) = Self::grid_point(state, self.mouse_pos);

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
                        let mut term = state.active_tab().session.term.lock();
                        term.selection = Some(Selection::new(ty, point, side));
                        drop(term);
                        state.window.request_redraw();
                    }
                    ElementState::Released => {
                        self.left_button_down = false;
                        // 빈 선택(클릭만)은 해제
                        let mut term = state.active_tab().session.term.lock();
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

                let tab = state.active_tab();
                let mut term = tab.session.term.lock();
                if term.mode().contains(TermMode::ALT_SCREEN) {
                    // 대체 스크린(less, vim 등)에는 히스토리가 없으므로 화살표로 변환
                    drop(term);
                    let seq: &[u8] = if lines > 0.0 { b"\x1b[A" } else { b"\x1b[B" };
                    let mut bytes = Vec::new();
                    for _ in 0..lines.abs() as usize {
                        bytes.extend_from_slice(seq);
                    }
                    tab.session.write(bytes);
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
    let event_loop = EventLoop::<TabEvent>::with_user_event()
        .build()
        .expect("이벤트 루프 생성 실패");
    let proxy = event_loop.create_proxy();
    let mut app = App {
        proxy,
        state: None,
        modifiers: Modifiers::default(),
        clipboard: None,
        next_tab_id: 0,
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
