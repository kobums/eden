//! Phase 2: 쓸 수 있는 터미널.
//!
//! Phase 1(창 + 렌더링 + 키 입력) 위에 스크롤백, 마우스 선택/클립보드,
//! 한글 IME(preedit 오버레이), 타이핑 시 자동 하단 스크롤을 얹는다.

mod renderer;
mod session;

use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as TermEvent, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::viewport_to_point;
use alacritty_terminal::term::TermMode;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

/// 더블/트리플 클릭 판정 간격.
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(400);
/// 휠 한 칸당 스크롤 줄 수.
const SCROLL_LINES_PER_TICK: f32 = 3.0;

struct State {
    window: Arc<Window>,
    renderer: renderer::Renderer,
    session: session::Session,
}

struct App {
    proxy: EventLoopProxy<TermEvent>,
    state: Option<State>,
    modifiers: Modifiers,
    clipboard: Option<arboard::Clipboard>,

    // 마우스 상태
    mouse_pos: PhysicalPosition<f64>,
    left_button_down: bool,
    last_click_at: Option<Instant>,
    last_click_point: Option<Point>,
    click_count: u32,
    scroll_accum: f32,

    // IME 조합 중 문자열
    preedit: Option<String>,
}

impl App {
    fn window_size(renderer: &renderer::Renderer, width: u32, height: u32) -> WindowSize {
        let (cols, lines) = renderer.grid_size(width, height);
        WindowSize {
            num_cols: cols as u16,
            num_lines: lines as u16,
            cell_width: renderer.cell_width as u16,
            cell_height: renderer.cell_height as u16,
        }
    }

    /// 마우스 물리 좌표 → 그리드 좌표(스크롤백 반영)와 셀 내 좌/우 반쪽.
    fn grid_point(state: &State, pos: PhysicalPosition<f64>) -> (Point, Side) {
        let cell_w = state.renderer.cell_width as f64;
        let cell_h = state.renderer.cell_height as f64;
        let pad = 8.0;

        let term = state.session.term.lock();
        let grid = term.grid();
        let cols = grid.columns();
        let lines = grid.screen_lines();
        let display_offset = grid.display_offset();
        drop(term);

        let col = (((pos.x - pad) / cell_w).floor().max(0.0) as usize).min(cols - 1);
        let line = (((pos.y - pad) / cell_h).floor().max(0.0) as usize).min(lines - 1);
        let point = viewport_to_point(display_offset, Point::new(line, Column(col)));

        let in_cell_x = (pos.x - pad) - col as f64 * cell_w;
        let side = if in_cell_x < cell_w / 2.0 {
            Side::Left
        } else {
            Side::Right
        };
        (point, side)
    }

    fn copy_selection(&mut self) {
        let Some(state) = &self.state else { return };
        let text = state.session.term.lock().selection_to_string();
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
        let bracketed = state
            .session
            .term
            .lock()
            .mode()
            .contains(TermMode::BRACKETED_PASTE);
        if bracketed {
            let mut bytes = b"\x1b[200~".to_vec();
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            state.session.write(bytes);
        } else {
            // 개행이 실행으로 이어지는 사고를 줄이기 위해 \n → \r 정규화
            state.session.write(text.replace('\n', "\r").into_bytes());
        }
    }

    /// 입력이 발생하면 선택을 해제하고 화면을 맨 아래로 되돌린다.
    fn on_user_input(state: &State) {
        let mut term = state.session.term.lock();
        term.selection = None;
        term.scroll_display(Scroll::Bottom);
    }

    fn redraw(&mut self) {
        let Some(state) = &mut self.state else { return };
        let ime_pos = state
            .renderer
            .draw(&state.session.term, self.preedit.as_deref());
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

impl ApplicationHandler<TermEvent> for App {
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
        let size = window.inner_size();
        let ws = Self::window_size(&renderer, size.width, size.height);
        let session = session::Session::new(session::EventProxy::new(self.proxy.clone()), ws);

        self.clipboard = arboard::Clipboard::new().ok();
        self.state = Some(State {
            window,
            renderer,
            session,
        });
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: TermEvent) {
        let Some(state) = &mut self.state else {
            return;
        };
        match event {
            TermEvent::Wakeup => state.window.request_redraw(),
            // 터미널이 앱의 질의(커서 위치 등)에 응답할 때 — 반드시 PTY로 되돌려준다.
            TermEvent::PtyWrite(text) => state.session.write(text.into_bytes()),
            TermEvent::Title(title) => state.window.set_title(&title),
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
                state.session.write(formatter(&text).into_bytes());
            }
            TermEvent::Exit => event_loop.exit(),
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
                let ws = Self::window_size(&state.renderer, size.width, size.height);
                state.session.resize(ws);
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
                        Key::Character("c") => self.copy_selection(),
                        Key::Character("v") => self.paste(),
                        _ => {}
                    }
                    return;
                }

                if let Some(bytes) = key_to_bytes(&event, mods) {
                    let state = self.state.as_ref().unwrap();
                    Self::on_user_input(state);
                    state.session.write(bytes);
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
                        Self::on_user_input(state);
                        state.session.write(text.into_bytes());
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
                    let mut term = state.session.term.lock();
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
                        let mut term = state.session.term.lock();
                        term.selection = Some(Selection::new(ty, point, side));
                        drop(term);
                        state.window.request_redraw();
                    }
                    ElementState::Released => {
                        self.left_button_down = false;
                        // 빈 선택(클릭만)은 해제
                        let mut term = state.session.term.lock();
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

                let mut term = state.session.term.lock();
                if term.mode().contains(TermMode::ALT_SCREEN) {
                    // 대체 스크린(less, vim 등)에는 히스토리가 없으므로 화살표로 변환
                    drop(term);
                    let seq: &[u8] = if lines > 0.0 { b"\x1b[A" } else { b"\x1b[B" };
                    let mut bytes = Vec::new();
                    for _ in 0..lines.abs() as usize {
                        bytes.extend_from_slice(seq);
                    }
                    state.session.write(bytes);
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
    let event_loop = EventLoop::<TermEvent>::with_user_event()
        .build()
        .expect("이벤트 루프 생성 실패");
    let proxy = event_loop.create_proxy();
    let mut app = App {
        proxy,
        state: None,
        modifiers: Modifiers::default(),
        clipboard: None,
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
